use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::logging::local_human_now;
use crate::settings::{settings, Settings};

/// Default notes directory under the XDG data dir
/// (`~/.local/share/oxterm/notes`), instead of littering the home directory.
pub fn default_notes_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/"))
        .join("oxterm")
        .join("notes")
}

/// One-time migration away from the legacy default notes directory (the home
/// directory). It only triggers when the setting still equals the home dir and
/// no `notes.md` was already written there, so existing notes are never lost.
pub fn migrate_legacy_notes_dir() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let s = settings();
    if s.get_str("notes_dir") != home.to_string_lossy() {
        return;
    }
    if home.join("notes.md").exists() {
        return;
    }
    let target = default_notes_dir();
    s.set_str("notes_dir", &target.to_string_lossy());
    crate::logging::log_warning(&format!(
        "notes_dir_migrated from={} to={}",
        home.display(),
        target.display()
    ));
}

/// Resolve the directory notes are written to, honouring the (optional)
/// setting and falling back to [`default_notes_dir`].
pub fn resolve_notes_dir() -> PathBuf {
    let raw = settings().get_str("notes_dir");
    if raw.trim().is_empty() {
        default_notes_dir()
    } else {
        expand_user(&raw)
    }
}

/// Normalize a user-provided note name: trim it, reject traversal and absolute
/// paths, and make sure it ends in `.md`.
pub fn validate_note_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Note filename cannot be empty".to_string());
    }
    if Path::new(name).is_absolute() {
        return Err("Note filename must be relative to the notes directory".to_string());
    }
    relative_note_components(Path::new(name))?;
    let mut name = name.to_string();
    if !name.to_ascii_lowercase().ends_with(".md") {
        name.push_str(".md");
    }
    Ok(name)
}

/// List the Markdown files in the notes directory, newest first (falling back
/// to name order when timestamps are unavailable).
pub fn list_note_files() -> Vec<String> {
    let dir = resolve_notes_dir();
    let mut files: Vec<(String, u64)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_md = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("md"))
                .unwrap_or(false);
            if !is_md || !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            files.push((name.to_string(), modified));
        }
    }
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    files.into_iter().map(|(name, _)| name).collect()
}

/// Context recorded alongside a note entry so it can be understood later.
#[derive(Default, Clone)]
pub struct NoteContext {
    pub source: Option<String>,
    pub cwd: Option<String>,
    pub host: Option<String>,
}

impl NoteContext {
    /// Context for a manually composed note (from the Notes menu).
    pub fn manual() -> NoteContext {
        NoteContext {
            source: Some("manual".to_string()),
            ..NoteContext::default()
        }
    }

    /// Context for text taken from a terminal.
    pub fn from_terminal(source: &str, cwd: &str) -> NoteContext {
        NoteContext {
            source: Some(source.to_string()),
            cwd: Some(cwd.to_string()),
            host: hostname(),
        }
    }
}

pub struct NotesManager {
    _settings: Settings,
}

impl NotesManager {
    pub fn new() -> NotesManager {
        NotesManager {
            _settings: settings().clone(),
        }
    }

    fn note_target(&self, filename: Option<&str>) -> Result<NoteTarget, String> {
        let s = settings();
        let notes_dir = resolve_notes_dir();
        let configured = s.get_str("notes_file");
        let name = filename.map(|f| f.to_string()).unwrap_or_else(|| {
            if !configured.is_empty() {
                configured.clone()
            } else {
                "notes.md".to_string()
            }
        });
        // Expand "~/..." up front so it is treated as an absolute path rather
        // than a literal "~" directory inside the notes directory.
        let name = if name == "~" || name.starts_with("~/") {
            expand_user(&name).to_string_lossy().into_owned()
        } else {
            name
        };
        let configured_external = filename.is_none() && Path::new(&name).is_absolute();
        if configured_external {
            let path = expand_user(&name);
            let (parent, file_name) = split_absolute_note_path(&path)?;
            let parent_fd = open_directory_tree(&parent, true)?;
            return Ok(NoteTarget {
                path,
                parent: parent_fd,
                file_name,
            });
        }
        if Path::new(&name).is_absolute() && filename.is_some() {
            return Err("Note filename must be relative to the notes directory".to_string());
        }
        let name = validate_note_name(&name)?;
        let components = relative_note_components(Path::new(&name))?;
        let notes_dir = absolute_path(&notes_dir)?;
        let root = open_directory_tree(&notes_dir, true)?;
        let (parent, file_name) = open_relative_parent(root, &components)?;
        Ok(NoteTarget {
            path: notes_dir.join(&name),
            parent,
            file_name,
        })
    }

    /// Resolve the note target on the calling (GTK) thread, then append and
    /// fsync on a worker thread so large notes never stall the UI. `done` runs
    /// back on the main thread.
    pub fn write_note_async<F>(
        &self,
        text: String,
        filename: Option<&str>,
        context: NoteContext,
        done: F,
    ) where
        F: FnOnce(Result<PathBuf, String>) + 'static,
    {
        let target = match self.note_target(filename) {
            Ok(target) => target,
            Err(e) => {
                done(Err(e));
                return;
            }
        };
        let done = crate::Sendable(done);
        std::thread::spawn(move || {
            let result = append_note(target, &text, context);
            crate::run_on_main(move || {
                let done = done;
                (done.0)(result)
            });
        });
    }

    pub fn open_notes(&self, filename: Option<&str>) -> Result<PathBuf, String> {
        let target = self.note_target(filename)?;
        let (mut f, created) = open_note_file(&target)?;
        if created {
            f.write_all(b"# Oxterm Notes\n\n")
                .map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }

        if let Some(opener) = find_in_path("xdg-open") {
            spawn_detached(&opener, &[&target.path.to_string_lossy()]);
            return Ok(target.path);
        }
        let editor = settings().get_str_default("editor_command", "nano");
        let parts = if editor.is_empty() {
            vec!["nano".to_string()]
        } else {
            shell_words::split(&editor).unwrap_or_else(|_| vec![editor.clone()])
        };
        if let Some(first) = parts.first() {
            let mut args = parts[1..].to_vec();
            args.push(target.path.to_string_lossy().to_string());
            spawn_detached(first, &args.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        }
        Ok(target.path)
    }
}

/// Open the notes directory in the configured file manager (or `xdg-open`).
pub fn open_notes_folder() -> Result<PathBuf, String> {
    let dir = resolve_notes_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dir_str = dir.to_string_lossy().to_string();
    let file_manager = settings().get_str("file_manager");
    if !file_manager.is_empty() && spawn_detached(&file_manager, &[&dir_str]) {
        return Ok(dir);
    }
    if let Some(opener) = find_in_path("xdg-open") {
        spawn_detached(&opener, &[&dir_str]);
    }
    Ok(dir)
}

fn append_note(target: NoteTarget, text: &str, context: NoteContext) -> Result<PathBuf, String> {
    let ts = local_human_now();
    let mut entry = format!("\n## {}\n", ts);
    if settings().get_bool("notes_record_context") {
        let mut fields = Vec::new();
        if let Some(source) = context.source.filter(|s| !s.trim().is_empty()) {
            fields.push(format!("source: {}", source.trim()));
        }
        if let Some(cwd) = context.cwd.filter(|s| !s.trim().is_empty()) {
            fields.push(format!("cwd: {}", cwd.trim()));
        }
        if let Some(host) = context.host.filter(|s| !s.trim().is_empty()) {
            fields.push(format!("host: {}", host.trim()));
        }
        if !fields.is_empty() {
            entry.push_str(&format!("<!-- {} -->\n", fields.join(" | ")));
        }
    }
    entry.push_str(&format!("\n{}\n", text.trim_end()));

    let (mut f, _) = open_note_file(&target)?;
    f.write_all(entry.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(target.path)
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

struct NoteTarget {
    path: PathBuf,
    parent: fs::File,
    file_name: CString,
}

fn relative_note_components(path: &Path) -> Result<Vec<CString>, String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => components.push(os_string(name)?),
            _ => return Err("Note filename must not contain traversal".to_string()),
        }
    }
    if components.is_empty() {
        return Err("Note filename cannot be empty".to_string());
    }
    Ok(components)
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Notes directory must not contain traversal".to_string());
    }
    Ok(path)
}

fn split_absolute_note_path(path: &Path) -> Result<(PathBuf, CString), String> {
    let path = absolute_path(path)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "Note filename cannot be empty".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Note path must have a parent directory".to_string())?
        .to_path_buf();
    Ok((parent, os_string(file_name)?))
}

fn os_string(value: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(value.as_bytes())
        .map_err(|_| "Note path contains an invalid character".to_string())
}

fn open_directory_tree(path: &Path, create: bool) -> Result<fs::File, String> {
    let mut current = open_directory(Path::new("/"))?;
    for component in path.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => {
                let name = os_string(name)?;
                current = open_directory_at(&current, &name, create)?;
            }
            _ => return Err("Note path must not contain traversal".to_string()),
        }
    }
    Ok(current)
}

fn open_relative_parent(
    mut current: fs::File,
    components: &[CString],
) -> Result<(fs::File, CString), String> {
    let (file_name, parents) = components
        .split_last()
        .ok_or_else(|| "Note filename cannot be empty".to_string())?;
    for parent in parents {
        current = open_directory_at(&current, parent, true)?;
    }
    Ok((current, file_name.clone()))
}

fn open_directory(path: &Path) -> Result<fs::File, String> {
    let path = os_string(path.as_os_str())?;
    open_fd(unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    })
}

fn open_directory_at(parent: &fs::File, name: &CString, create: bool) -> Result<fs::File, String> {
    if create {
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    open_fd(unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    })
}

fn open_note_file(target: &NoteTarget) -> Result<(fs::File, bool), String> {
    let flags =
        libc::O_WRONLY | libc::O_APPEND | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let created = unsafe {
        libc::openat(
            target.parent.as_raw_fd(),
            target.file_name.as_ptr(),
            flags | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
    };
    let (fd, created) = if created >= 0 {
        (created, true)
    } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
        (
            unsafe { libc::openat(target.parent.as_raw_fd(), target.file_name.as_ptr(), flags) },
            false,
        )
    } else {
        return Err(std::io::Error::last_os_error().to_string());
    };
    let file = open_fd(fd)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe { stat.assume_init().st_mode } & libc::S_IFMT != libc::S_IFREG {
        return Err("Note path must be a regular file".to_string());
    }
    // Only tighten permissions on files we created; leave a user's existing
    // (possibly shared) notes file as they configured it.
    if created && unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((file, created))
}

fn open_fd(fd: libc::c_int) -> Result<fs::File, String> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_before_creating_directories() {
        assert!(relative_note_components(Path::new("../outside.md")).is_err());
        assert!(relative_note_components(Path::new("notes/../outside.md")).is_err());
        assert!(relative_note_components(Path::new("/outside.md")).is_err());
    }

    #[test]
    fn validates_and_normalizes_note_names() {
        assert_eq!(validate_note_name(" project ").unwrap(), "project.md");
        assert_eq!(validate_note_name("ideas.md").unwrap(), "ideas.md");
        assert_eq!(validate_note_name("ideas.MD").unwrap(), "ideas.MD");
        assert!(validate_note_name("").is_err());
        assert!(validate_note_name("  ").is_err());
        assert!(validate_note_name("/etc/passwd").is_err());
        assert!(validate_note_name("../escape.md").is_err());
        assert!(validate_note_name("sub/note.md").unwrap() == "sub/note.md");
    }

    #[test]
    fn rejects_symlinked_parent_directory() {
        let root = std::env::temp_dir().join(format!(
            "oxterm-notes-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("/tmp", root.join("linked")).unwrap();
        let root_fd = open_directory_tree(&root, false).unwrap();
        let name = CString::new("linked").unwrap();
        assert!(open_directory_at(&root_fd, &name, false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

fn expand_user(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        home.join(rest)
    } else if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else {
        PathBuf::from(path)
    }
}

fn find_in_path(cmd: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

pub fn which(cmd: &str) -> Option<String> {
    find_in_path(cmd)
}

pub fn spawn_detached(program: &str, args: &[&str]) -> bool {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            reap_in_background(child);
            true
        }
        Err(_) => false,
    }
}

/// Wait for a launched helper on a throwaway thread so it never lingers as a
/// zombie for the lifetime of the terminal.
pub fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}
