//! User interface extensions declared in `~/.config/oxterm/extensions.json`.
//!
//! The file holds two independent lists:
//!
//! * `widgets` — commands whose output appears in a thin bar at the bottom of
//!   every window, refreshed on an interval;
//! * `palette` — extra entries for the command palette (`Ctrl+Shift+P`), each
//!   running a shell command in a new tab.
//!
//! ```json
//! {
//!   "version": 1,
//!   "widgets": [
//!     { "id": "git", "label": "git", "command": "git branch --show-current",
//!       "interval": 5, "color": "#8be9fd" }
//!   ],
//!   "palette": [
//!     { "label": "Open in VS Code", "detail": "Open this directory",
//!       "command": "code ." }
//!   ]
//! }
//! ```
//!
//! Parsing and command execution are display-free. The GTK bar and palette
//! integration live outside this module.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const CURRENT_VERSION: u64 = 1;
pub const MAX_WIDGETS: usize = 16;
pub const MAX_PALETTE: usize = 64;
pub const MAX_COMMAND: usize = 8192;
pub const MIN_INTERVAL: u64 = 1;
pub const MAX_INTERVAL: u64 = 3600;
pub const DEFAULT_INTERVAL: u64 = 10;
pub const DEFAULT_MAX_CHARS: u64 = 120;
pub const MAX_CHARS_LIMIT: u64 = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Widget {
    pub id: String,
    /// Optional prefix shown before the command output.
    pub label: String,
    /// Shell command, run through `/bin/sh -c`.
    pub command: String,
    /// Seconds between refreshes.
    pub interval: u64,
    /// Optional `#rrggbb` colour for the text.
    pub color: Option<String>,
    /// Maximum characters shown, ellipsized beyond this.
    pub max_chars: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub id: String,
    pub label: String,
    pub detail: String,
    /// Shell command, run in a new tab.
    pub command: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiConfig {
    pub widgets: Vec<Widget>,
    pub palette: Vec<PaletteEntry>,
}

impl UiConfig {
    pub fn is_empty(&self) -> bool {
        self.widgets.is_empty() && self.palette.is_empty()
    }

    /// A missing file is an empty configuration, not an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
        };
        if metadata.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{} is larger than {} KiB",
                path.display(),
                MAX_FILE_BYTES / 1024
            ));
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {}", path.display(), error))?;
        Self::parse(&raw)
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {}", error))?;
        let object = value
            .as_object()
            .ok_or("the extensions file must contain a JSON object")?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("the extensions file has no \"version\" (expected 1)")?;
        if version > CURRENT_VERSION {
            return Err(format!(
                "the extensions file was written by a newer Oxterm (version {}; this build reads {})",
                version, CURRENT_VERSION
            ));
        }
        let widgets = parse_widgets(object.get("widgets"))?;
        let palette = parse_palette(object.get("palette"))?;
        Ok(Self { widgets, palette })
    }
}

fn parse_widgets(value: Option<&Value>) -> Result<Vec<Widget>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value.as_array().ok_or("\"widgets\" must be an array")?;
    if list.len() > MAX_WIDGETS {
        return Err(format!(
            "too many widgets: {} (limit {})",
            list.len(),
            MAX_WIDGETS
        ));
    }
    let mut widgets: Vec<Widget> = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("widget {} is not an object", index + 1))?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("widget-{}", index + 1));
        if id.chars().count() > 128 {
            return Err(format!("widget id is too long: {}", id));
        }
        if widgets.iter().any(|widget| widget.id == id) {
            return Err(format!("duplicate widget id: {}", id));
        }
        let command = required_text(object, "command", &id, "widget")?;
        if command.len() > MAX_COMMAND {
            return Err(format!(
                "widget {}: the command is longer than {} bytes",
                id, MAX_COMMAND
            ));
        }
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .trim()
            .to_string();
        let interval = match object.get("interval") {
            None | Some(Value::Null) => DEFAULT_INTERVAL,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| format!("widget {}: \"interval\" must be an integer", id))?,
        };
        if !(MIN_INTERVAL..=MAX_INTERVAL).contains(&interval) {
            return Err(format!(
                "widget {}: interval must be between {} and {} seconds",
                id, MIN_INTERVAL, MAX_INTERVAL
            ));
        }
        let max_chars = match object.get("max_chars") {
            None | Some(Value::Null) => DEFAULT_MAX_CHARS,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| format!("widget {}: \"max_chars\" must be an integer", id))?,
        };
        if !(1..=MAX_CHARS_LIMIT).contains(&max_chars) {
            return Err(format!(
                "widget {}: max_chars must be between 1 and {}",
                id, MAX_CHARS_LIMIT
            ));
        }
        let color = match object.get("color") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) if raw.trim().is_empty() => None,
            Some(Value::String(raw)) => Some(
                super::triggers::parse_color(raw.trim())
                    .map_err(|error| format!("widget {}: {}", id, error))?,
            ),
            Some(_) => return Err(format!("widget {}: \"color\" must be a string", id)),
        };
        widgets.push(Widget {
            id,
            label,
            command,
            interval,
            color,
            max_chars: max_chars as usize,
        });
    }
    Ok(widgets)
}

fn parse_palette(value: Option<&Value>) -> Result<Vec<PaletteEntry>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value.as_array().ok_or("\"palette\" must be an array")?;
    if list.len() > MAX_PALETTE {
        return Err(format!(
            "too many palette entries: {} (limit {})",
            list.len(),
            MAX_PALETTE
        ));
    }
    let mut entries: Vec<PaletteEntry> = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("palette entry {} is not an object", index + 1))?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("palette-{}", index + 1));
        if id.chars().count() > 128 {
            return Err(format!("palette id is too long: {}", id));
        }
        if entries.iter().any(|entry| entry.id == id) {
            return Err(format!("duplicate palette id: {}", id));
        }
        let label = required_text(object, "label", &id, "palette entry")?;
        let command = required_text(object, "command", &id, "palette entry")?;
        if command.len() > MAX_COMMAND {
            return Err(format!(
                "palette entry {}: the command is longer than {} bytes",
                id, MAX_COMMAND
            ));
        }
        let detail = object
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        entries.push(PaletteEntry {
            id,
            label,
            detail,
            command,
        });
    }
    Ok(entries)
}

fn required_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
    id: &str,
    what: &str,
) -> Result<String, String> {
    match object.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(format!("{} {}: \"{}\" is required", what, id, key)),
    }
}

/// Run a widget or palette command with a hard timeout.
///
/// Standard output is returned as-is; standard error is discarded so a failing
/// `git` cannot fill the log or the screen. A command that exceeds `timeout` is
/// killed and reported as an error.
pub fn run_command(command: &str, cwd: Option<&Path>, timeout: Duration) -> Result<String, String> {
    let mut builder = Command::new("sh");
    builder
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        builder.current_dir(cwd);
    }
    let mut child = builder.spawn().map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().ok_or("cannot read command output")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} ms", timeout.as_millis()));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(error.to_string());
            }
        }
    }
    let mut text = String::new();
    stdout
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_widgets_and_palette_entries() {
        let config = UiConfig::parse(
            r##"{
                "version": 1,
                "widgets": [
                    { "id": "git", "label": "git", "command": "git status",
                      "interval": 5, "color": "#ABC", "max_chars": 40 }
                ],
                "palette": [
                    { "label": "Open in VS Code", "detail": "Editor",
                      "command": "code ." }
                ]
            }"##,
        )
        .unwrap();
        assert_eq!(config.widgets.len(), 1);
        let widget = &config.widgets[0];
        assert_eq!(widget.id, "git");
        assert_eq!(widget.label, "git");
        assert_eq!(widget.interval, 5);
        assert_eq!(widget.color.as_deref(), Some("#aabbcc"));
        assert_eq!(widget.max_chars, 40);
        assert_eq!(config.palette.len(), 1);
        assert_eq!(config.palette[0].id, "palette-1");
        assert_eq!(config.palette[0].label, "Open in VS Code");
        assert_eq!(config.palette[0].command, "code .");
    }

    #[test]
    fn defaults_are_sane_and_a_missing_file_is_empty() {
        let config = UiConfig::parse(
            r#"{"version":1,"widgets":[{"command":"date"}],
                "palette":[{"label":"X","command":"true"}]}"#,
        )
        .unwrap();
        assert_eq!(config.widgets[0].id, "widget-1");
        assert_eq!(config.widgets[0].interval, DEFAULT_INTERVAL);
        assert_eq!(config.widgets[0].max_chars, DEFAULT_MAX_CHARS as usize);
        let path = std::env::temp_dir().join("oxterm-no-such-extensions.json");
        let _ = std::fs::remove_file(&path);
        assert!(UiConfig::load(&path).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_configurations() {
        assert!(UiConfig::parse("{}").is_err());
        assert!(UiConfig::parse(r#"{"version":99}"#).is_err());
        assert!(UiConfig::parse(r#"{"version":1,"widgets":{}}"#).is_err());
        assert!(
            UiConfig::parse(r#"{"version":1,"widgets":[{"command":"x","interval":0}]}"#).is_err()
        );
        assert!(
            UiConfig::parse(r##"{"version":1,"widgets":[{"command":"x","color":"red"}]}"##)
                .is_err()
        );
        assert!(UiConfig::parse(r#"{"version":1,"palette":[{"command":"x"}]}"#).is_err());
    }

    #[test]
    fn runs_commands_and_reports_timeouts() {
        let output = run_command("printf hello", None, Duration::from_secs(2)).unwrap();
        assert_eq!(output, "hello");
        let error = run_command("sleep 5", None, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("timed out"), "unexpected error: {error}");
    }
}
