use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Mirrors `tpgk/persistence.py::validate_name`.
pub fn validate_name(name: &str, kind: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err(format!("{} name cannot be empty", kind));
    }
    if name.chars().count() > 100 {
        return Err(format!("{} name is too long", kind));
    }
    for c in name.chars() {
        if c == '/' || c == '\\' || c == '\0' || (c as u32) < 32 {
            return Err(format!("{} name contains invalid characters", kind));
        }
    }
    let base = std::path::Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if base != name {
        return Err(format!("{} name must not contain a path", kind));
    }
    Ok(name.to_string())
}

/// Return a process-unique temporary path for an atomic persistence write.
pub fn temporary_path(dir: &Path, prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{}_{}_{}", prefix, std::process::id(), n))
}

/// Create a private temporary file without following a pre-existing symlink.
/// Retry on a name collision so the predictable process-local counter cannot
/// be used to force a write failure or redirect the write target.
pub fn write_private_temp(dir: &Path, prefix: &str, contents: &[u8]) -> std::io::Result<PathBuf> {
    for _ in 0..32 {
        let path = temporary_path(dir, prefix);
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if let Err(e) = file.write_all(contents).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        return Ok(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_like_python() {
        assert!(validate_name("work", "Profile").is_ok());
        assert!(validate_name("../work", "Profile").is_err());
        assert!(validate_name("", "Profile").is_err());
        assert!(validate_name("a\0b", "Profile").is_err());
    }

    #[test]
    fn temporary_paths_are_unique() {
        let dir = Path::new("/tmp");
        assert_ne!(
            temporary_path(dir, "settings"),
            temporary_path(dir, "settings")
        );
    }
}
