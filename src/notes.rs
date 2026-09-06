use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::logging::utc_iso_now;
use crate::settings::{settings, Settings};

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
        let notes_dir_raw = s.get_str_default("notes_dir", "");
        let notes_dir = if notes_dir_raw.is_empty() {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        } else {
            expand_user(&notes_dir_raw)
        };
        let configured = s.get_str("notes_file");
        let name = filename.map(|f| f.to_string()).unwrap_or_else(|| {
            if !configured.is_empty() {
                configured.clone()
            } else {
                "notes.md".to_string()
            }
        });
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
        if Path::new(&name).is_absolute() {
            if filename.is_some() {
                return Err("Note filename must be relative to the notes directory".to_string());
            }
        }
        let mut name = name;
        if !name.ends_with(".md") {
            name.push_str(".md");
        }
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

    pub fn write_note(&self, text: &str, filename: Option<&str>) -> Result<PathBuf, String> {
        let target = self.note_target(filename)?;
        let ts = human_now();
        let entry = format!("\n## {}\n\n{}\n", ts, text);
        let (mut f, _) = open_note_file(&target)?;
        f.write_all(entry.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        Ok(target.path)
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
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
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

fn human_now() -> String {
    // Python: datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    let full = utc_iso_now();
    // Local time approximation would require tz; use a UTC ISO date prefix.
    // Reuse RFC3339 but replace 'T' with space and drop sub-seconds/tz.
    full.replace('T', " ")
        .split('.')
        .next()
        .unwrap_or("")
        .to_string()
        + " UTC"
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

pub fn spawn_detached(program: &str, args: &[&str]) -> Option<std::process::Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}
