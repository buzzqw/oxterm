//! Diagnostics report shared by `oxterm --diagnostics` and the Help menu.
//!
//! The report contains paths, versions and limits, never settings values, so it
//! can be pasted into a bug report without leaking configuration secrets.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{framing, settings, spec};

pub struct Report<'a> {
    pub version: &'a str,
    pub build: &'a str,
    pub rustc: &'a str,
    pub config_file: &'a Path,
    pub config_dir: &'a Path,
    pub remote_dir: Option<&'a Path>,
    pub notes_dir: Option<&'a Path>,
    /// Keys overridden for this session (`-o`, profile); values are omitted.
    pub overrides: &'a [String],
    /// Runtime facts only the GUI knows (GTK/VTE versions, panes, session id).
    pub extra: &'a [(String, String)],
}

pub fn render(report: &Report<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("oxterm {}\n", report.version));
    out.push_str(&format!("build: {}\n", report.build));
    out.push_str(&format!("rustc: {}\n", report.rustc));
    out.push_str(&format!(
        "os: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    out.push_str(&format!("display: {}\n", display()));
    out.push_str(&format!("shell: {}\n", shell()));
    out.push_str(&format!(
        "config-dir: {}\n",
        describe_dir(report.config_dir)
    ));
    out.push_str(&format!(
        "config-file: {}\n",
        describe_file(report.config_file)
    ));
    if let Some(path) = report.remote_dir {
        out.push_str(&format!("remote-dir: {}\n", describe_dir(path)));
    }
    if let Some(path) = report.notes_dir {
        out.push_str(&format!("notes-dir: {}\n", describe_dir(path)));
    }
    out.push_str(&format!("settings-keys: {}\n", spec::SETTINGS.len()));
    if report.overrides.is_empty() {
        out.push_str("session-overrides: none\n");
    } else {
        out.push_str(&format!(
            "session-overrides: {}\n",
            report.overrides.join(", ")
        ));
    }
    for (key, value) in report.extra {
        out.push_str(&format!("{key}: {value}\n"));
    }
    out.push_str("limits:\n");
    out.push_str(&format!(
        "  settings file: {} MiB\n",
        settings::MAX_SETTINGS_BYTES / (1024 * 1024)
    ));
    out.push_str(&format!(
        "  control request / response: {} MiB / {} MiB\n",
        framing::MAX_REQUEST_FRAME / (1024 * 1024),
        framing::MAX_RESPONSE_FRAME / (1024 * 1024)
    ));
    out.push_str(&format!(
        "  control connections: {}\n",
        framing::MAX_CONTROL_CLIENTS
    ));
    out.push_str(&format!(
        "  pty broker frame / pending / clients: {} KiB / {} MiB / {}\n",
        framing::MAX_BROKER_FRAME / 1024,
        framing::MAX_BROKER_PENDING / (1024 * 1024),
        framing::MAX_BROKER_CLIENTS
    ));
    out.push_str("  osc133 line buffer: 64 KiB\n");
    out
}

fn display() -> String {
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    let x11 = std::env::var("DISPLAY").ok();
    match (wayland, x11) {
        (Some(wayland), _) => format!("wayland ({wayland})"),
        (None, Some(display)) => format!("x11 ({display})"),
        (None, None) => "none (headless)".to_string(),
    }
}

fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string())
}

fn describe_dir(path: &Path) -> String {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => format!(
            "{} [{}, mode {:04o}]",
            path.display(),
            if meta.is_dir() { "dir" } else { "not a dir" },
            meta.permissions().mode() & 0o777
        ),
        Err(error) => format!("{} [{}]", path.display(), error.kind()),
    }
}

fn describe_file(path: &Path) -> String {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            format!("{} [symbolic link]", path.display())
        }
        Ok(meta) => format!(
            "{} [{} bytes, mode {:04o}]",
            path.display(),
            meta.len(),
            meta.permissions().mode() & 0o777
        ),
        Err(error) => format!("{} [{}]", path.display(), error.kind()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_paths_limits_and_overrides_without_values() {
        let dir = std::env::temp_dir().join(format!("oxterm-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), b"{}").unwrap();
        let overrides = vec!["font_size".to_string()];
        let extra = vec![("gtk".to_string(), "3.24.38".to_string())];
        let report = Report {
            version: "1.2.3",
            build: "release",
            rustc: "rustc 1.82.0",
            config_file: &dir.join("settings.json"),
            config_dir: &dir,
            remote_dir: Some(&dir),
            notes_dir: Some(&dir),
            overrides: &overrides,
            extra: &extra,
        };
        let text = render(&report);
        assert!(text.contains("oxterm 1.2.3"));
        assert!(text.contains("session-overrides: font_size"));
        assert!(!text.contains("font_size="));
        assert!(text.contains("gtk: 3.24.38"));
        assert!(text.contains("control request / response"));
        assert!(text.contains(&format!("settings-keys: {}", spec::SETTINGS.len())));
        assert!(text.contains("settings.json [2 bytes"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
