//! Filesystem locations and small settings lookups shared by the GUI, the
//! CLI and the PTY broker.
//!
//! The broker has no widget toolkit linked into it, so it cannot reach the GUI
//! settings module. Keeping these helpers display-free lets both binaries agree
//! on where the configuration and the remote sockets live.

use std::path::PathBuf;

/// The user's Oxterm configuration directory.
pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("oxterm")
}

/// The user's Oxterm data directory: plugins, locks and per-plugin state.
pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join("oxterm");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".local").join("share").join("oxterm")
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
