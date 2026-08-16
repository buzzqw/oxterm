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
