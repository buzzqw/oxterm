//! What the file browser asks of `sftp` and `lftp`, and how it reads the answers.
//!
//! Both programs are driven by a script on standard input, one short run per
//! operation. A password never travels on a command line, and `sftp` reuses an
//! open shared connection, so browsing a host with a tab open needs no new login.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

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
    let month = words.iter().position(|(_, word)| MONTHS.contains(word))?;
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

fn sftp_command(connection: &Connection) -> Command {
    let effective = ssh_config::effective_config(&connection.target_args(), PROBE_TIMEOUT);
    let mut command = Command::new("sftp");
    // `-b -` stops at the first error. ssh takes the first value of an option, so
    // this keeps a stored password possible where `-b` would imply BatchMode.
    command.args(["-q", "-b", "-", "-o", "BatchMode=no"]);
    if let Some(config) = effective.as_deref() {
        command.args(ssh_config::sharing_options(config, &paths::config_dir()));
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

/// Run one script and return its output. Blocks, so call it off the GUI thread.
pub fn run(connection: &Connection, operation_script: &str) -> Result<String, String> {
    let ftp = connection.protocol == Protocol::Ftp;
    let (mut command, input) = if ftp {
        (
            lftp_command(),
            format!("{}{}", ftp_preamble(connection)?, operation_script),
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
        .map_err(|error| format!("cannot run {}: {}", program, error))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("cannot talk to {}: {}", program, error))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{} failed: {}", program, error))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let errors = String::from_utf8_lossy(&output.stderr);
    let message = errors
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("the connection failed");
    Err(message.trim().to_string())
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
    let output = run(
        connection,
        &script(connection.protocol, &Operation::List(path.to_string()))?,
    )?;
    Ok(parse_listing(&output))
}

/// Delete a file or a folder with all it holds. `sftp` removes only empty folders,
/// so its tree is walked here, one level at a time.
pub fn delete(connection: &Connection, path: &str, is_dir: bool) -> Result<(), String> {
    let protocol = connection.protocol;
    if !is_dir {
        run(
            connection,
            &script(protocol, &Operation::DeleteFile(path.to_string()))?,
        )?;
        return Ok(());
    }
    if protocol != Protocol::Ftp {
        for entry in list(connection, path)? {
            // A link to a folder is removed as a link: its target is not ours.
            delete(connection, &join(path, &entry.name), entry.is_dir)?;
        }
    }
    run(
        connection,
        &script(protocol, &Operation::DeleteDir(path.to_string()))?,
    )?;
    Ok(())
}

/// Copy a file or folder in either direction.
pub fn transfer(
    connection: &Connection,
    upload: bool,
    is_dir: bool,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let protocol = connection.protocol;
    let text = if protocol == Protocol::Ftp && !is_dir {
        ftp_file_transfer(upload, from, to)?
    } else if upload {
        script(
            protocol,
            &Operation::Upload {
                local: from.to_string(),
                remote: to.to_string(),
            },
        )?
    } else {
        script(
            protocol,
            &Operation::Download {
                remote: from.to_string(),
                local: to.to_string(),
            },
        )?
    };
    run(connection, &text)?;
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
    fn sizes_read_well() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
