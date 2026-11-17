//! Filesystem locations and small settings lookups shared by the GUI, the
//! CLI and the PTY broker.
//!
//! The broker has no widget toolkit linked into it, so it cannot reach the GUI
//! settings module. Keeping these helpers display-free lets both binaries agree
//! on where the configuration and the remote sockets live.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The user's Oxterm configuration directory.
pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("oxterm")
}

/// Where OpenSSH keeps its ControlMaster sockets.
///
/// `$XDG_RUNTIME_DIR` is preferred: the path is short (Unix socket paths are
/// limited to about 100 bytes), private, and cleaned up at logout. A long home
/// directory must not make connection sharing fail with "ControlPath too long",
/// so the fallbacks are shorter temporary locations with a per-user name and
/// mode 0700.
pub fn ssh_socket_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(compute_ssh_socket_dir).clone()
}

fn compute_ssh_socket_dir() -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(runtime).join("oxterm"));
    }
    candidates.push(config_dir().join("sockets"));
    // `geteuid` names the fallback per user; 0700 keeps other accounts out.
    candidates.push(PathBuf::from("/tmp").join(format!("oxterm-{}", unsafe { libc::geteuid() })));
    for candidate in candidates {
        if let Some(dir) = usable_socket_dir(candidate) {
            return dir;
        }
    }
    // Everything failed: the caller still gets a path, and OpenSSH reports the
    // real error instead of Oxterm silently dropping connection sharing.
    config_dir().join("sockets")
}

fn usable_socket_dir(candidate: PathBuf) -> Option<PathBuf> {
    // `%C` expands to a 40-character SHA-1; Linux allows 107 bytes after the
    // terminating NUL, and `cm-` costs three more.
    if candidate.to_string_lossy().len() + "cm-".len() + 40 > 100 {
        return None;
    }
    create_private_dir(&candidate).ok()?;
    Some(candidate)
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return check_private_dir(path);
    }
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
        // Verify even after creating: another process may have won the race
        // and made the directory first (or `mkdir -p` accepted an existing one).
        Ok(()) => check_private_dir(path),
        Err(_) if path.is_dir() => check_private_dir(path),
        Err(error) => Err(error),
    }
}

/// A socket directory must belong to this user and stay unreadable to others:
/// an attacker who can write there could race or replace a master socket.
fn check_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    let owned = metadata.uid() == unsafe { libc::geteuid() };
    let private = metadata.permissions().mode() & 0o077 == 0;
    if owned && private {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket directory is not private",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_directory_that_would_not_fit_is_refused() {
        let dir = PathBuf::from("/tmp").join("x".repeat(90));
        assert_eq!(usable_socket_dir(dir), None);
    }
}

/// The user's Oxterm data directory, reserved for extension state.
pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join("oxterm");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".local").join("share").join("oxterm")
}

/// The user's Oxterm state directory (logs and the agent audit trail).
/// Respects `XDG_STATE_HOME`, like other desktop applications.
pub fn state_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join("oxterm");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".local").join("state").join("oxterm")
}

/// The control byte sent by the tmux-style prefix, read from the stored
/// settings. A separate `oxterm attach` process does not inherit the GUI's
/// in-memory overrides, so the on-disk document is the right source.
pub fn tmux_prefix_byte() -> u8 {
    let path = config_dir().join("settings.json");
    let prefix = super::settings::load_file(&path)
        .ok()
        .and_then(|loaded| {
            loaded
                .settings
                .get("tmux_prefix_key")
                .and_then(|value| value.as_str())
                .and_then(|value| value.chars().next())
        })
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_alphabetic())
        .unwrap_or('b');
    (prefix as u8) & 0x1f
}
