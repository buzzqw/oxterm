//! What the file browser asks of `sftp` and `lftp`, and how it reads the answers.
//!
//! Both programs are driven by a script on standard input, one short run per
//! operation. A password never travels on a command line, and `sftp` reuses an
//! open shared connection, so browsing a host with a tab open needs no new login.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::connections::{Auth, Connection, Protocol, PROBE_TIMEOUT};
use super::{askpass, paths, ssh_config};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: String,
    pub permissions: String,
}

/// Read one `ls -l` line: `drwxr-xr-x 2 user group 4096 Jan 1 12:00 name`.
/// Owners may be missing, as some FTP servers leave them out, so the fields are
/// found from the permissions and the date, not by position alone.
fn parse_line(line: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches('\r');
    let kind = line.chars().next()?;
    if !matches!(kind, '-' | 'd' | 'l' | 'b' | 'c' | 'p' | 's') {
        return None;
    }
    let permissions = line.split_whitespace().next()?;
    if permissions.chars().count() < 10 {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Walk the words by byte offset, so a name keeps its inner spaces exactly.
    let mut words: Vec<(usize, &str)> = Vec::new();
    let mut position = 0;
    for word in line.split_whitespace() {
        let start = line[position..].find(word)? + position;
        words.push((start, word));
        position = start + word.len();
    }
    let month = words
        .iter()
        .position(|(_, word)| MONTHS.iter().any(|name| word.eq_ignore_ascii_case(name)))?;
    let size = words.get(month.checked_sub(1)?)?.1.parse().ok()?;
    let (name_start, _) = *words.get(month + 3)?;
    let modified = format!(
        "{} {} {}",
        words[month].1,
        words[month + 1].1,
        words[month + 2].1
    );
    let mut name = &line[name_start..];
    if kind == 'l' {
        name = name.split(" -> ").next()?;
    }
    // `sftp` answers with the whole path, and `lftp` marks a folder with a slash.
    // A real name cannot contain one, so the last part is always the name.
    let name = name.trim_end_matches('/').rsplit('/').next()?;
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(RemoteEntry {
        name: name.to_string(),
        is_dir: kind == 'd',
        is_link: kind == 'l',
        size,
        modified,
        permissions: permissions.to_string(),
    })
}

/// Folders first, then by name, ignoring case.
pub fn parse_listing(output: &str) -> Vec<RemoteEntry> {
    let mut entries: Vec<RemoteEntry> = output.lines().filter_map(parse_line).collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Quote a path for a line of an `sftp` or `lftp` script. A line break cannot be
/// quoted in either, and would start a new command, so such a path is refused.
pub fn quote_path(path: &str) -> Result<String, String> {
    if path.contains(['\n', '\r']) {
        return Err("This file name contains a line break".to_string());
    }
    Ok(format!(
        "\"{}\"",
        path.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Join a directory and a name.
pub fn join(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{}{}", directory, name)
    } else {
        format!("{}/{}", directory, name)
    }
}

/// The folder above `path`.
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
    }
}

/// A name typed by a person must stay in the folder it was typed for.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("That is not a valid name".to_string());
    }
    if name.contains('/') || name.chars().any(char::is_control) {
        return Err("A name cannot contain / or control characters".to_string());
    }
    Ok(())
}

/// One operation on the remote side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Home,
    List(String),
    Download { remote: String, local: String },
    Upload { local: String, remote: String },
    MakeDir(String),
    Rename { from: String, to: String },
    DeleteFile(String),
    DeleteDir(String),
}

/// The script for an operation.
pub fn script(protocol: Protocol, operation: &Operation) -> Result<String, String> {
    let ftp = protocol == Protocol::Ftp;
    Ok(match operation {
        Operation::Home => "pwd\n".to_string(),
        Operation::List(path) => {
            if ftp {
                format!(
                    "cd {}\ncls -la --time-style=\"%b %d %H:%M\"\n",
                    quote_path(path)?
                )
            } else {
                format!("ls -la {}\n", quote_path(path)?)
            }
        }
        Operation::Download { remote, local } => {
            if ftp {
                format!(
                    "mirror --no-perms {} {}\n",
                    quote_path(remote)?,
                    quote_path(local)?
                )
            } else {
                format!("get -pr {} {}\n", quote_path(remote)?, quote_path(local)?)
            }
        }
        Operation::Upload { local, remote } => {
            if ftp {
                format!(
                    "mirror -R --no-perms {} {}\n",
                    quote_path(local)?,
                    quote_path(remote)?
                )
            } else {
                format!("put -pr {} {}\n", quote_path(local)?, quote_path(remote)?)
            }
        }
        Operation::MakeDir(path) => format!("mkdir {}\n", quote_path(path)?),
        Operation::Rename { from, to } => {
            let verb = if ftp { "mv" } else { "rename" };
            format!("{} {} {}\n", verb, quote_path(from)?, quote_path(to)?)
        }
        Operation::DeleteFile(path) => format!("rm {}\n", quote_path(path)?),
        Operation::DeleteDir(path) => {
            if ftp {
                format!("rm -r {}\n", quote_path(path)?)
            } else {
                format!("rmdir {}\n", quote_path(path)?)
            }
        }
    })
}

/// An FTP transfer of one file: `mirror` only takes folders.
pub fn ftp_file_transfer(upload: bool, from: &str, to: &str) -> Result<String, String> {
    Ok(format!(
        "{} {} -o {}\n",
        if upload { "put" } else { "get" },
        quote_path(from)?,
        quote_path(to)?
    ))
}

/// Whether the browser can open this entry at all.
pub fn supports_browsing(connection: &Connection) -> bool {
    matches!(
        connection.protocol,
        Protocol::Ssh | Protocol::Sftp | Protocol::Ftp
    )
}

/// `ssh -G` starts a process, and a browser runs one `sftp` per operation, down
/// to one per file when deleting a folder. The answer changes when someone edits
/// a config file, so it is kept briefly rather than asked for every time.
fn effective_config_cached(connection: &Connection) -> Option<String> {
    const FRESH_FOR: Duration = Duration::from_secs(30);
    type Entry = (Vec<String>, Instant, Option<String>);
    static CACHE: Mutex<Option<Entry>> = Mutex::new(None);
    let args = connection.target_args();
    if let Ok(cache) = CACHE.lock() {
        if let Some((key, when, value)) = cache.as_ref() {
            if *key == args && when.elapsed() < FRESH_FOR {
                return value.clone();
            }
        }
    }
    let value = ssh_config::effective_config(&args, PROBE_TIMEOUT);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((args, Instant::now(), value.clone()));
    }
    value
}

/// How long a directory listing may be reused. Short enough that changes made
/// outside Oxterm show up quickly, long enough that walking back and forth in
/// the browser does not spawn a new `sftp`/`lftp` for every step.
const LIST_CACHE_FRESH_FOR: Duration = Duration::from_secs(8);
const LIST_CACHE_MAX: usize = 128;

type ListCache = Mutex<HashMap<String, (Instant, Vec<RemoteEntry>)>>;

fn list_cache() -> &'static ListCache {
    static CACHE: OnceLock<ListCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(connection: &Connection, path: &str) -> String {
    format!(
        "{:?}|{}|{}|{}|{}|{}",
        connection.protocol,
        connection.host,
        connection.port.unwrap_or(0),
        connection.user,
        connection.id,
        path
    )
}

/// Drop cached listings after a change. Called by every mutating operation.
pub fn invalidate(_connection: &Connection) {
    if let Ok(mut cache) = list_cache().lock() {
        cache.clear();
    }
}

fn cached_listing(connection: &Connection, path: &str) -> Option<Vec<RemoteEntry>> {
    let cache = list_cache().lock().ok()?;
    let (at, entries) = cache.get(&cache_key(connection, path))?;
    (at.elapsed() < LIST_CACHE_FRESH_FOR).then(|| entries.clone())
}

fn store_listing(connection: &Connection, path: &str, entries: &[RemoteEntry]) {
    if let Ok(mut cache) = list_cache().lock() {
        if cache.len() >= LIST_CACHE_MAX {
            cache.clear();
        }
        cache.insert(
            cache_key(connection, path),
            (Instant::now(), entries.to_vec()),
        );
    }
}

fn sftp_command(connection: &Connection) -> Command {
    let effective = effective_config_cached(connection);
    let mut command = Command::new("sftp");
    // `ls -l` formats the month with the user's locale; the listing parser
    // expects the C-locale abbreviations.
    command.env("LC_TIME", "C");
    // `-b -` stops at the first error. ssh takes the first value of an option, so
    // this keeps a stored password possible where `-b` would imply BatchMode.
    command.args(["-q", "-b", "-", "-o", "BatchMode=no"]);
    if !connection.no_sharing {
        if let Some(config) = effective.as_deref() {
            command.args(ssh_config::sharing_options(
                config,
                &paths::ssh_socket_dir(),
            ));
        }
    }
    command.args(&connection.sftp_argv()[1..]);
    if connection.uses_keyring() {
        if let Ok(environment) =
            super::connections::askpass_environment(connection, effective.as_deref())
        {
            command.envs(environment);
        }
    }
    command
}

fn lftp_command() -> Command {
    let mut command = Command::new("lftp");
    // `cls` formats the month with the user's locale; keep the parser input
    // deterministic without changing the filename charset handling.
    command.env("LC_TIME", "C");
    command.arg("-f").arg("/dev/stdin");
    command
}

fn ftp_preamble(connection: &Connection) -> Result<String, String> {
    let password = if connection.auth == Auth::SecretService {
        askpass::lookup_secret(&connection.id).unwrap_or_default()
    } else {
        String::new()
    };
    let user = if connection.user.is_empty() {
        "anonymous"
    } else {
        &connection.user
    };
    let mut script =
        String::from("set cmd:fail-exit yes\nset net:max-retries 1\nset net:timeout 15\n");
    script.push_str(if connection.ftp_tls {
        "set ftp:ssl-force yes\nset ftp:ssl-protect-data yes\n"
    } else {
        "set ftp:ssl-allow no\n"
    });
    let port = connection.port.unwrap_or(21);
    // `user` rather than `open -u`, which splits on a comma in the password.
    script.push_str(&format!(
        "open -p {} {}\nuser {} {}\n",
        port,
        quote_path(&connection.host)?,
        quote_path(user)?,
        quote_path(&password)?,
    ));
    Ok(script)
}

/// Why a remote operation did not complete.
#[derive(Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The user cancelled the transfer or closed the browser.
    Cancelled,
    /// The remote program failed; the message fits the status line.
    Failed(String),
}

impl TransferError {
    pub fn message(&self) -> String {
        match self {
            TransferError::Cancelled => "cancelled".to_string(),
            TransferError::Failed(message) => message.clone(),
        }
    }
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

/// Run one script and return its output. Blocks, so call it off the GUI thread.
pub fn run(connection: &Connection, operation_script: &str) -> Result<String, String> {
    run_cancellable(connection, operation_script, &AtomicBool::new(false))
        .map_err(|error| error.message())
}

/// Run one script, killing the remote program when `cancel` becomes true.
pub fn run_cancellable(
    connection: &Connection,
    operation_script: &str,
    cancel: &AtomicBool,
) -> Result<String, TransferError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(TransferError::Cancelled);
    }
    let ftp = connection.protocol == Protocol::Ftp;
    let (mut command, input) = if ftp {
        (
            lftp_command(),
            format!(
                "{}{}",
                ftp_preamble(connection).map_err(TransferError::Failed)?,
                operation_script
            ),
        )
    } else {
        (sftp_command(connection), operation_script.to_string())
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A new session: with no controlling terminal nothing can stop to prompt where
    // nobody sees it, and the askpass helper fails cleanly instead.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let program = if ftp { "lftp" } else { "sftp" };
    let mut child = command
        .spawn()
        .map_err(|error| TransferError::Failed(format!("cannot run {}: {}", program, error)))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(input.as_bytes()) {
            kill_tree(&mut child);
            return Err(TransferError::Failed(format!(
                "cannot talk to {}: {}",
                program, error
            )));
        }
    }
    wait_for_output(child, program, Some(cancel))
}

/// Stop a child and anything it started.
///
/// The program runs in its own session, so a group kill also reaches helpers
/// (a shell running the operation, for instance) that would otherwise keep the
/// output pipes open and make cancellation wait for them. No group with the
/// child's id exists unless it leads one, which makes this safe for callers
/// that spawned without `setsid`.
fn kill_tree(child: &mut Child) {
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait for a child while draining both pipes on their own threads.
///
/// `Child::wait_with_output` blocks until the end and cannot be interrupted;
/// polling `try_wait` gives cancellation a chance to kill a transfer that
/// would otherwise keep the socket and the disk busy after the window closed.
fn wait_for_output(
    mut child: Child,
    program: &str,
    cancel: Option<&AtomicBool>,
) -> Result<String, TransferError> {
    let mut out_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buffer);
            buffer
        })
    });
    let mut err_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buffer);
            buffer
        })
    });

    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            kill_tree(&mut child);
            if let Some(handle) = out_reader.take() {
                let _ = handle.join();
            }
            if let Some(handle) = err_reader.take() {
                let _ = handle.join();
            }
            return Err(TransferError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_reader
                    .take()
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                let stderr = err_reader
                    .take()
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                if status.success() {
                    return Ok(String::from_utf8_lossy(&stdout).to_string());
                }
                let errors = String::from_utf8_lossy(&stderr);
                let message = errors
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("the connection failed");
                return Err(TransferError::Failed(message.trim().to_string()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                kill_tree(&mut child);
                return Err(TransferError::Failed(format!(
                    "{} failed: {}",
                    program, error
                )));
            }
        }
    }
}

/// The folder a session starts in.
pub fn home_directory(connection: &Connection) -> Result<String, String> {
    let output = run(connection, &script(connection.protocol, &Operation::Home)?)?;
    let line = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.starts_with("sftp>"))
        .unwrap_or("/");
    let path = line
        .trim()
        .strip_prefix("Remote working directory: ")
        .unwrap_or(line.trim());
    // lftp prints a URL.
    Ok(match path.strip_prefix("ftp://") {
        Some(rest) => rest
            .find('/')
            .map_or("/", |index| &rest[index..])
            .to_string(),
        None => path.to_string(),
    })
}

pub fn list(connection: &Connection, path: &str) -> Result<Vec<RemoteEntry>, String> {
    if let Some(entries) = cached_listing(connection, path) {
        return Ok(entries);
    }
    list_uncached(connection, path)
}

/// List without consulting or filling the cache. Used while planning a
/// recursive delete, where a stale entry could leave a folder non-empty.
fn list_uncached(connection: &Connection, path: &str) -> Result<Vec<RemoteEntry>, String> {
    let output = run(
        connection,
        &script(connection.protocol, &Operation::List(path.to_string()))?,
    )?;
    let entries = parse_listing(&output);
    store_listing(connection, path, &entries);
    Ok(entries)
}

fn collect_tree(
    connection: &Connection,
    path: &str,
    files: &mut Vec<String>,
    dirs: &mut Vec<String>,
) -> Result<(), String> {
    for entry in list_uncached(connection, path)? {
        let child = join(path, &entry.name);
        if entry.is_dir {
            dirs.push(child.clone());
            collect_tree(connection, &child, files, dirs)?;
        } else {
            files.push(child);
        }
    }
    Ok(())
}

/// Delete a file or a folder with all it holds. `sftp` removes only empty
/// folders, so its tree is walked first and every removal is sent in one
/// batch: one process instead of one per file.
pub fn delete(connection: &Connection, path: &str, is_dir: bool) -> Result<(), String> {
    let protocol = connection.protocol;
    if !is_dir {
        run(
            connection,
            &script(protocol, &Operation::DeleteFile(path.to_string()))?,
        )?;
        invalidate(connection);
        return Ok(());
    }
    if protocol == Protocol::Ftp {
        // `lftp` removes the whole tree itself.
        run(
            connection,
            &script(protocol, &Operation::DeleteDir(path.to_string()))?,
        )?;
        invalidate(connection);
        return Ok(());
    }
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    collect_tree(connection, path, &mut files, &mut dirs)?;
    let mut batch = String::new();
    for file in &files {
        batch.push_str("rm ");
        batch.push_str(&quote_path(file)?);
        batch.push('\n');
    }
    // Children were pushed before their parent; removing in reverse leaves
    // every folder empty by the time its own `rmdir` runs.
    for dir in dirs.iter().rev() {
        batch.push_str("rmdir ");
        batch.push_str(&quote_path(dir)?);
        batch.push('\n');
    }
    batch.push_str("rmdir ");
    batch.push_str(&quote_path(path)?);
    batch.push('\n');
    run(connection, &batch)?;
    invalidate(connection);
    Ok(())
}

/// Copy a file or folder in either direction.
pub fn transfer(
    connection: &Connection,
    upload: bool,
    is_dir: bool,
    from: &str,
    to: &str,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    let protocol = connection.protocol;
    let text = if protocol == Protocol::Ftp && !is_dir {
        ftp_file_transfer(upload, from, to).map_err(TransferError::Failed)?
    } else if upload {
        script(
            protocol,
            &Operation::Upload {
                local: from.to_string(),
                remote: to.to_string(),
            },
        )
        .map_err(TransferError::Failed)?
    } else {
        script(
            protocol,
            &Operation::Download {
                remote: from.to_string(),
                local: to.to_string(),
            },
        )
        .map_err(TransferError::Failed)?
    };
    run_cancellable(connection, &text, cancel)?;
    invalidate(connection);
    Ok(())
}

/// Copy the contents of one directory into another, merging and overwriting.
///
/// SFTP's `put -r src dst` nests `src` inside an existing `dst`; adding `/.` to
/// the source copies its contents instead, which is what a sync means.
pub fn sync_script(
    protocol: Protocol,
    upload: bool,
    from: &str,
    to: &str,
) -> Result<String, String> {
    if protocol == Protocol::Ftp {
        let operation = if upload {
            Operation::Upload {
                local: from.to_string(),
                remote: to.to_string(),
            }
        } else {
            Operation::Download {
                remote: from.to_string(),
                local: to.to_string(),
            }
        };
        return script(protocol, &operation);
    }
    let source = format!("{}/.", from.trim_end_matches('/'));
    let target = format!("{}/", to.trim_end_matches('/'));
    Ok(format!(
        "{} -pr {} {}\n",
        if upload { "put" } else { "get" },
        quote_path(&source)?,
        quote_path(&target)?
    ))
}

pub fn sync_directory(
    connection: &Connection,
    upload: bool,
    from: &str,
    to: &str,
    cancel: &AtomicBool,
) -> Result<(), TransferError> {
    let text = sync_script(connection.protocol, upload, from, to).map_err(TransferError::Failed)?;
    run_cancellable(connection, &text, cancel)?;
    invalidate(connection);
    Ok(())
}

/// A readable size.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings_are_read_with_their_odd_names() {
        let output = "sftp> ls -la \"/x\"\n\
drwxr-xr-x    2 andres   andres       4096 Nov  1 12:00 .\n\
drwxr-xr-x    3 andres   andres       4096 Nov  1 12:00 ..\n\
-rw-r--r--    1 andres   andres        120 Nov  1 12:00 two  spaces.txt\n\
drwxr-xr-x    2 andres   andres       4096 Jan  1  2026 Zeta\n\
drwxr-xr-x    2 andres   andres       4096 Jan  1  2026 alpha\n\
lrwxrwxrwx    1 andres   andres          7 Nov  1 12:00 link -> /etc\n";
        let entries = parse_listing(output);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Zeta", "link", "two  spaces.txt"]);
        assert!(entries[0].is_dir);
        assert!(entries[2].is_link);
        assert_eq!(entries[3].size, 120);
        assert_eq!(entries[1].modified, "Jan 1 2026");
    }

    #[test]
    fn full_paths_and_folder_marks_are_reduced_to_the_name() {
        let output = "drwxr-xr-x 2 u g 4096 Nov 1 15:01 /home/a/./\n\
drwxr-xr-x 2 u g 4096 Nov 1 15:01 /home/a/../\n\
drwxr-xr-x 2 u g 4096 Nov 1 15:01 /home/a/.cache/\n\
-rw-r--r-- 1 u g 5 Nov 1 15:01 /home/a/two words.txt\n\
lrwxrwxrwx 1 u g 7 Nov 1 15:01 /home/a/link -> /etc/x\n\
drwxr-xr-x 2 ftp ftp 0 Nov 01 15:16 folder/\n";
        let names: Vec<String> = parse_listing(output)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, [".cache", "folder", "link", "two words.txt"]);
    }

    #[test]
    fn a_name_that_looks_like_a_date_stays_a_name() {
        let entries = parse_listing("-rw-r--r-- 1 u g 5 Nov 1 12:00 Jan 2 notes\n");
        assert_eq!(entries[0].name, "Jan 2 notes");
    }

    #[test]
    fn servers_without_owners_still_parse() {
        let entries = parse_listing("-rw-r--r-- 1 ftp 77 Mar 3 10:10 a.txt\n");
        assert_eq!(entries[0].size, 77);
        assert_eq!(entries[0].name, "a.txt");
    }

    #[test]
    fn noise_is_ignored() {
        assert!(parse_listing("total 12\nsftp> pwd\nnot a listing\n").is_empty());
    }

    #[test]
    fn paths_cannot_escape_their_quotes_or_start_a_command() {
        assert_eq!(quote_path("a b").unwrap(), "\"a b\"");
        assert_eq!(quote_path("a\"b\\c").unwrap(), "\"a\\\"b\\\\c\"");
        assert!(quote_path("a\nrm -rf /").is_err());
        assert!(script(Protocol::Sftp, &Operation::DeleteFile("a\nrm x".into())).is_err());
    }

    #[test]
    fn names_stay_in_their_folder() {
        for bad in ["", ".", "..", "a/b", "../x", "a\nb"] {
            assert!(check_name(bad).is_err(), "{:?}", bad);
        }
        assert!(check_name("a b.txt").is_ok());
    }

    #[test]
    fn paths_join_and_go_up() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/x", "a"), "/x/a");
        assert_eq!(parent("/x/y"), "/x");
        assert_eq!(parent("/x/"), "/");
        assert_eq!(parent("/"), "/");
    }

    #[test]
    fn scripts_differ_by_protocol() {
        let list = Operation::List("/x".into());
        assert_eq!(script(Protocol::Sftp, &list).unwrap(), "ls -la \"/x\"\n");
        assert!(script(Protocol::Ftp, &list).unwrap().contains("cd \"/x\""));
        let rename = Operation::Rename {
            from: "a".into(),
            to: "b".into(),
        };
        assert_eq!(
            script(Protocol::Sftp, &rename).unwrap(),
            "rename \"a\" \"b\"\n"
        );
        assert_eq!(script(Protocol::Ftp, &rename).unwrap(), "mv \"a\" \"b\"\n");
        let get = Operation::Download {
            remote: "/r".into(),
            local: "/l".into(),
        };
        assert!(script(Protocol::Sftp, &get).unwrap().starts_with("get -pr"));
        assert_eq!(
            ftp_file_transfer(true, "/l", "/r").unwrap(),
            "put \"/l\" -o \"/r\"\n"
        );
    }

    #[test]
    fn sync_scripts_copy_contents_not_the_folder_itself() {
        assert_eq!(
            sync_script(Protocol::Sftp, true, "/tmp/a", "/tmp/b").unwrap(),
            "put -pr \"/tmp/a/.\" \"/tmp/b/\"\n"
        );
        assert_eq!(
            sync_script(Protocol::Sftp, false, "/tmp/a/", "/tmp/b/").unwrap(),
            "get -pr \"/tmp/a/.\" \"/tmp/b/\"\n"
        );
        assert!(sync_script(Protocol::Ftp, true, "/l", "/r")
            .unwrap()
            .contains("mirror -R"));
        assert!(sync_script(Protocol::Ftp, false, "/r", "/l")
            .unwrap()
            .contains("mirror "));
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn parses_listings_from_any_month_locale() {
        let upper = "-rw-r--r-- 1 user group 42 Nov 13 08:44 there.txt";
        let lower = "-rw-r--r-- 1 user group 42 nov 13 08:44 there.txt";
        for line in [upper, lower] {
            let entry = parse_line(line).expect(line);
            assert_eq!(entry.name, "there.txt");
            assert_eq!(entry.size, 42);
            assert!(entry.modified.eq_ignore_ascii_case("nov 13 08:44"));
        }
        assert_eq!(
            parse_listing("-rw-r--r-- 1 u g 1 nov 13 08:44 there.txt").len(),
            1
        );
    }

    fn shell(script: &str) -> Child {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Mirror the transfer code: the shell leads its own session, so a group
        // kill stops the command it runs even when the shell does not exec it.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        command.spawn().expect("spawn sh")
    }

    #[test]
    fn waits_for_output_and_returns_stdout() {
        let output = wait_for_output(shell("printf hello"), "sh", None).unwrap();
        assert_eq!(output, "hello");
    }

    #[test]
    fn a_failed_program_returns_its_last_error_line() {
        let error = wait_for_output(
            shell("printf 'first\\n' >&2; printf '  boom  \\n' >&2; exit 3"),
            "sh",
            None,
        )
        .unwrap_err();
        assert_eq!(error, TransferError::Failed("boom".to_string()));
    }

    #[test]
    fn cancellation_kills_a_long_running_program() {
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            trigger.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        // The shell waits on a child of its own, so the program that holds the
        // output pipes is not the direct child: a plain kill would leave it
        // running and reading would hang until it exits.
        let result = wait_for_output(shell("sleep 10 & wait"), "sh", Some(&cancel));
        assert_eq!(result, Err(TransferError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancellation must not wait for the program"
        );
    }

    #[test]
    fn an_already_cancelled_request_reports_cancelled() {
        let cancel = AtomicBool::new(true);
        let mut child = shell("printf should-not-run");
        // The child is already spawned by the helper, so this checks the
        // waiting side: kill it immediately and expect Cancelled.
        child.kill().unwrap();
        assert_eq!(
            wait_for_output(child, "sh", Some(&cancel)),
            Err(TransferError::Cancelled)
        );
    }
}
