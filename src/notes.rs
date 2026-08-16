use std::fs;
use std::fs::OpenOptions;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

    fn get_notes_path(&self, filename: Option<&str>) -> Result<PathBuf, String> {
        let s = settings();
        let notes_dir_raw = s.get_str_default("notes_dir", "");
        let notes_dir = if notes_dir_raw.is_empty() {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        } else {
            expand_user(&notes_dir_raw)
        };
        let notes_dir_abs = std::fs::canonicalize(&notes_dir).unwrap_or_else(|_| notes_dir.clone());
        let configured = s.get_str("notes_file");
        let name = filename.map(|f| f.to_string()).unwrap_or_else(|| {
            if !configured.is_empty() {
                configured.clone()
            } else {
                "notes.md".to_string()
            }
        });
        if Path::new(&name).is_absolute() {
            if filename.is_some() {
                return Err("Note filename must be relative to the notes directory".to_string());
            }
            return Ok(
                std::fs::canonicalize(expand_user(&name)).unwrap_or_else(|_| expand_user(&name))
            );
        }
        let mut name = name;
        if !name.ends_with(".md") {
            name.push_str(".md");
        }
        let path = notes_dir_abs.join(&name);
        let parent = std::fs::canonicalize(path.parent().unwrap_or(Path::new(".")))
            .unwrap_or_else(|_| notes_dir_abs.clone());
        if parent.starts_with(&notes_dir_abs) {
            Ok(path)
        } else {
            Err("Note filename must stay inside the notes directory".to_string())
        }
    }

    fn ensure_parent(&self, path: &Path, allow_configured_external: bool) -> Result<(), String> {
        let s = settings();
        let notes_dir_raw = s.get_str_default("notes_dir", "");
        let notes_dir = if notes_dir_raw.is_empty() {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        } else {
            expand_user(&notes_dir_raw)
        };
        let notes_dir = std::fs::canonicalize(&notes_dir).unwrap_or(notes_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let parent_canon =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            if !allow_configured_external && !parent_canon.starts_with(&notes_dir) {
                return Err("Note path escapes the notes directory".to_string());
            }
        }
        Ok(())
    }

    pub fn write_note(&self, text: &str, filename: Option<&str>) -> Result<PathBuf, String> {
        let path = self.get_notes_path(filename)?;
        let configured = settings().get_str("notes_file");
        let allow_external = filename.is_none() && Path::new(&configured).is_absolute();
        self.ensure_parent(&path, allow_external)?;
        let ts = human_now();
        let entry = format!("\n## {}\n\n{}\n", ts, text);
        let mut opts = OpenOptions::new();
        opts.create(true)
            .append(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        let mut f = opts.open(&path).map_err(|e| e.to_string())?;
        use std::io::Write;
        f.write_all(entry.as_bytes()).map_err(|e| e.to_string())?;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        Ok(path)
    }

    pub fn open_notes(&self, filename: Option<&str>) -> Result<PathBuf, String> {
        let path = self.get_notes_path(filename)?;
        let configured = settings().get_str("notes_file");
        let allow_external = filename.is_none() && Path::new(&configured).is_absolute();
        self.ensure_parent(&path, allow_external)?;
        if fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("Note path must not be a symbolic link".to_string());
        }
        if !path.is_file() {
            let mut opts = OpenOptions::new();
            opts.create(true)
                .write(true)
                .append(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW);
            let mut f = opts.open(&path).map_err(|e| e.to_string())?;
            use std::io::Write;
            f.write_all(b"# TPGK Notes\n\n")
                .map_err(|e| e.to_string())?;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }

        if let Some(opener) = find_in_path("xdg-open") {
            spawn_detached(&opener, &[&path.to_string_lossy()]);
            return Ok(path);
        }
        let editor = settings().get_str_default("editor_command", "nano");
        let parts = if editor.is_empty() {
            vec!["nano".to_string()]
        } else {
            shell_words::split(&editor).unwrap_or_else(|_| vec![editor.clone()])
        };
        if let Some(first) = parts.first() {
            let mut args = parts[1..].to_vec();
            args.push(path.to_string_lossy().to_string());
            spawn_detached(first, &args.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        }
        Ok(path)
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
