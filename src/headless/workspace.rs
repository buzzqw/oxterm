//! Declarative workspaces: a JSON file that opens tabs, splits and commands.
//!
//! ```json
//! {
//!   "version": 1,
//!   "name": "backend",
//!   "layout": "vertical",
//!   "panes": [
//!     { "tabs": [
//!         { "title": "api", "cwd": "~/src/api", "command": "cargo watch -x run" },
//!         { "title": "tests", "cwd": "~/src/api", "command": "cargo test --watch" }
//!     ] },
//!     { "tabs": [
//!         { "title": "logs", "command": "tail -f /var/log/app.log" }
//!     ] }
//!   ]
//! }
//! ```
//!
//! Parsing and validation live here so `oxterm workspace` can fail before it
//! touches a window and the server can re-check what it receives.

use std::path::Path;

use serde_json::Value;

/// Largest accepted workspace file.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Accepted workspace version.
pub const CURRENT_VERSION: u64 = 1;

/// A window has at most two panes (two notebooks).
pub const MAX_PANES: usize = 2;

/// Tabs per pane.
pub const MAX_TABS: usize = 32;

/// Bound on a shell command line stored in the file.
pub const MAX_COMMAND: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Single,
    Vertical,
    Horizontal,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Single => "single",
            Layout::Vertical => "vertical",
            Layout::Horizontal => "horizontal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub title: Option<String>,
    /// Canonical directory, already expanded and checked.
    pub cwd: Option<String>,
    /// Shell command line, run through `/bin/sh -c`.
    pub command: Option<String>,
    /// Tab label colour (`#rrggbb`), validated.
    pub color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub layout: Layout,
    pub panes: Vec<Pane>,
}

/// Read a workspace file, enforcing the size limit.
pub fn read(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot read {}: {}", path.display(), error))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{} is larger than {} KiB",
            path.display(),
            MAX_FILE_BYTES / 1024
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {}", path.display(), error))
}

pub fn load(path: &Path) -> Result<Workspace, String> {
    parse(&read(path)?)
}

pub fn parse(raw: &str) -> Result<Workspace, String> {
    if raw.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "the workspace is larger than {} KiB",
            MAX_FILE_BYTES / 1024
        ));
    }
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {}", error))?;
    from_value(&value)
}

pub fn from_value(value: &Value) -> Result<Workspace, String> {
    let object = value
        .as_object()
        .ok_or("the workspace must contain a JSON object")?;
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .ok_or("the workspace has no \"version\" (expected 1)")?;
    if version > CURRENT_VERSION {
        return Err(format!(
            "the workspace was written by a newer Oxterm (version {}; this build reads {})",
            version, CURRENT_VERSION
        ));
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if name.chars().count() > 128 {
        return Err("the workspace name is longer than 128 characters".to_string());
    }
    let layout = match object
        .get("layout")
        .and_then(Value::as_str)
        .unwrap_or("single")
    {
        "single" => Layout::Single,
        "vertical" => Layout::Vertical,
        "horizontal" => Layout::Horizontal,
        other => {
            return Err(format!(
                "unknown layout '{}' (use single, vertical or horizontal)",
                other
            ))
        }
    };
    let panes_value = object
        .get("panes")
        .and_then(Value::as_array)
        .ok_or("the workspace has no \"panes\" array")?;
    if panes_value.is_empty() || panes_value.len() > MAX_PANES {
        return Err(format!(
            "a workspace has one or two panes, not {}",
            panes_value.len()
        ));
    }
    if layout == Layout::Single && panes_value.len() != 1 {
        return Err("layout \"single\" accepts exactly one pane".to_string());
    }
    if layout != Layout::Single && panes_value.len() != 2 {
        return Err(format!(
            "layout \"{}\" needs exactly two panes",
            layout.as_str()
        ));
    }
    let mut panes = Vec::with_capacity(panes_value.len());
    for (index, pane) in panes_value.iter().enumerate() {
        panes.push(parse_pane(pane, index)?);
    }
    Ok(Workspace {
        name,
        layout,
        panes,
    })
}

fn parse_pane(value: &Value, index: usize) -> Result<Pane, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("pane {} is not an object", index + 1))?;
    let tabs_value = object
        .get("tabs")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("pane {} has no \"tabs\" array", index + 1))?;
    if tabs_value.is_empty() || tabs_value.len() > MAX_TABS {
        return Err(format!(
            "pane {} has {} tabs; it needs 1 to {}",
            index + 1,
            tabs_value.len(),
            MAX_TABS
        ));
    }
    let mut tabs = Vec::with_capacity(tabs_value.len());
    for (tab_index, tab) in tabs_value.iter().enumerate() {
        tabs.push(parse_tab(tab, index, tab_index)?);
    }
    Ok(Pane { tabs })
}

fn parse_tab(value: &Value, pane: usize, index: usize) -> Result<Tab, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("pane {} tab {} is not an object", pane + 1, index + 1))?;
    let title = match object.get("title") {
        None | Some(Value::Null) => None,
        Some(Value::String(title)) => {
            let title = title.trim();
            if title.is_empty() {
                None
            } else if title.chars().count() > 128 {
                return Err(format!(
                    "pane {} tab {}: the title is longer than 128 characters",
                    pane + 1,
                    index + 1
                ));
            } else {
                Some(title.to_string())
            }
        }
        Some(_) => {
            return Err(format!(
                "pane {} tab {}: \"title\" must be a string",
                pane + 1,
                index + 1
            ))
        }
    };
    let cwd = match object.get("cwd") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => Some(
            super::control_cli::expand_dir(raw.trim())
                .map_err(|error| format!("pane {} tab {}: {}", pane + 1, index + 1, error))?,
        ),
        Some(_) => {
            return Err(format!(
                "pane {} tab {}: \"cwd\" must be a string",
                pane + 1,
                index + 1
            ))
        }
    };
    let command = match object.get("command") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => {
            if raw.len() > MAX_COMMAND {
                return Err(format!(
                    "pane {} tab {}: the command is longer than {} bytes",
                    pane + 1,
                    index + 1,
                    MAX_COMMAND
                ));
            }
            Some(raw.clone())
        }
        Some(_) => {
            return Err(format!(
                "pane {} tab {}: \"command\" must be a string",
                pane + 1,
                index + 1
            ))
        }
    };
    let color = match object.get("color") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => Some(
            super::triggers::parse_color(raw.trim())
                .map_err(|error| format!("pane {} tab {}: {}", pane + 1, index + 1, error))?,
        ),
        Some(_) => {
            return Err(format!(
                "pane {} tab {}: \"color\" must be a string",
                pane + 1,
                index + 1
            ))
        }
    };
    Ok(Tab {
        title,
        cwd,
        command,
        color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> Result<Workspace, String> {
        super::parse(value)
    }

    #[test]
    fn parses_a_single_pane_workspace() {
        let workspace = parse(
            r##"{
                "version": 1,
                "name": "backend",
                "panes": [{
                    "tabs": [
                        { "title": "api", "cwd": "/tmp", "command": "cargo run",
                          "color": "#ABC" }
                    ]
                }]
            }"##,
        )
        .unwrap();
        assert_eq!(workspace.name, "backend");
        assert_eq!(workspace.layout, Layout::Single);
        assert_eq!(workspace.panes.len(), 1);
        let tab = &workspace.panes[0].tabs[0];
        assert_eq!(tab.title.as_deref(), Some("api"));
        assert_eq!(tab.cwd.as_deref(), Some("/tmp"));
        assert_eq!(tab.command.as_deref(), Some("cargo run"));
        assert_eq!(tab.color.as_deref(), Some("#aabbcc"));
    }

    #[test]
    fn parses_two_panes_and_defaults() {
        let workspace = parse(
            r#"{
                "version": 1,
                "layout": "horizontal",
                "panes": [
                    { "tabs": [{}] },
                    { "tabs": [{ "command": "tail -f log" }] }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(workspace.layout, Layout::Horizontal);
        assert_eq!(workspace.name, "");
        assert_eq!(workspace.panes[0].tabs[0].title, None);
        assert_eq!(
            workspace.panes[1].tabs[0].command.as_deref(),
            Some("tail -f log")
        );
    }

    #[test]
    fn rejects_invalid_documents() {
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"version": 99, "panes": []}"#).is_err());
        assert!(parse(r#"{"version": 1}"#).is_err());
        assert!(parse(r#"{"version": 1, "panes": []}"#).is_err());
        assert!(
            parse(r#"{"version": 1, "layout": "diagonal", "panes": [{"tabs":[{}]}]}"#).is_err()
        );
        assert!(parse(
            r#"{"version": 1, "layout": "single", "panes": [{"tabs":[{}]},{"tabs":[{}]}]}"#
        )
        .is_err());
        assert!(parse(r#"{"version": 1, "panes": [{"tabs":[]}]}"#).is_err());
        assert!(
            parse(r#"{"version": 1, "panes": [{"tabs":[{"cwd":"/definitely/not/here"}]}]}"#)
                .is_err()
        );
        assert!(parse(r##"{"version": 1, "panes": [{"tabs":[{"color":"red"}]}]}"##).is_err());
    }
}
