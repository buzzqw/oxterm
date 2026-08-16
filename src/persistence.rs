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
