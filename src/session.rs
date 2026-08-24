use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::logging::LOGGER;
use crate::persistence::validate_name;
use crate::persistence::write_private_temp;
use crate::settings;

fn session_dir() -> PathBuf {
    settings::config_dir().join("sessions")
}

fn ensure_dir() {
    let _ = fs::create_dir_all(&session_dir());
    let _ = fs::set_permissions(&session_dir(), fs::Permissions::from_mode(0o700));
}

pub fn session_path(name: &str) -> Result<PathBuf, String> {
    ensure_dir();
    let name = validate_name(name, "Session")?;
    Ok(session_dir().join(format!("{}.json", name)))
}

#[derive(Clone, Default)]
pub struct TabEntry {
    pub base_title: String,
    pub title: String,
    pub cwd: String,
}

#[derive(Clone, Default)]
pub struct SessionData {
    pub split_mode: String,
    pub tabs_left: Vec<TabEntry>,
    pub tabs_right: Vec<TabEntry>,
}

const SPLIT_MODES: [&str; 3] = ["single", "vertical", "horizontal"];
const MAX_TABS: usize = 100;

pub fn save_session(name: &str, data: &SessionData) -> bool {
    ensure_dir();
    let payload = json!({
        "timestamp": crate::logging::utc_iso_now(),
        "split_mode": data.split_mode,
        "tabs_left": data.tabs_left.iter().map(|t| {
            json!({"base_title": t.base_title, "title": t.title, "cwd": t.cwd})
        }).collect::<Vec<_>>(),
        "tabs_right": data.tabs_right.iter().map(|t| {
            json!({"base_title": t.base_title, "title": t.title, "cwd": t.cwd})
        }).collect::<Vec<_>>(),
    });
    let path = match session_path(name) {
        Ok(path) => path,
        Err(e) => {
            LOGGER.warning(&e);
            return false;
        }
    };
    match write_private_temp(
        &session_dir(),
        "session_tmp",
        serde_json::to_string_pretty(&payload)
            .unwrap_or_default()
            .as_bytes(),
    ) {
        Ok(tmp) => {
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
            if fs::rename(&tmp, &path).is_ok() {
                return true;
            }
            LOGGER.error("session_save_failed");
            false
        }
        Err(e) => {
            LOGGER.error(&format!("session_save_failed error={}", e));
            false
        }
    }
}

pub fn export_session(name: &str, destination: &str) -> Result<(), String> {
    let source = session_path(name)?;
    let content = fs::read(&source).map_err(|e| format!("could not read session: {}", e))?;
    let destination = PathBuf::from(destination);
    let parent = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    if !parent.is_dir() {
        return Err("export destination directory does not exist".to_string());
    }
    let temp = write_private_temp(parent, "session_export", &content)
        .map_err(|e| format!("could not write session export: {}", e))?;
    if fs::rename(&temp, &destination).is_err() {
        let _ = fs::remove_file(&temp);
        return Err("could not finalize session export".to_string());
    }
    let _ = fs::set_permissions(&destination, fs::Permissions::from_mode(0o600));
    Ok(())
}

fn validate_tab(v: &Value) -> Option<TabEntry> {
    if !v.is_object() {
        return None;
    }
    let obj = v.as_object().unwrap();
    let mut cwd = obj
        .get("cwd")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if !std::path::Path::new(&cwd).is_dir() {
        cwd = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    let base_title = obj
        .get("base_title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let title = obj
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| base_title.as_str())
        .to_string();
    Some(TabEntry {
        cwd,
        base_title: base_title.chars().take(200).collect(),
        title: title.chars().take(200).collect(),
    })
}

fn validate_state(data: &Value) -> Option<SessionData> {
    if !data.is_object() {
        return None;
    }
    let split_mode = data
        .get("split_mode")
        .and_then(|x| x.as_str())
        .unwrap_or("single")
        .to_string();
    if !SPLIT_MODES.contains(&split_mode.as_str()) {
        return None;
    }
    let mut out = SessionData {
        split_mode,
        ..Default::default()
    };
    for (key, dst) in [
        ("tabs_left", &mut out.tabs_left),
        ("tabs_right", &mut out.tabs_right),
    ] {
        let arr = data.get(key);
        if !arr.map(|a| a.is_array()).unwrap_or(true) {
            return None;
        }
        if let Some(arr) = arr.and_then(|a| a.as_array()) {
            if arr.len() > MAX_TABS {
                return None;
            }
            for item in arr {
                match validate_tab(item) {
                    Some(t) => dst.push(t),
                    None => return None,
                }
            }
        }
    }
    if out.tabs_left.is_empty() {
        return None;
    }
    Some(out)
}

pub fn load_session(name: &str) -> Option<SessionData> {
    let p = session_path(name).ok()?;
    if !p.exists() {
        return None;
    }
    match fs::read_to_string(&p) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(data) => match validate_state(&data) {
                Some(v) => Some(v),
                None => {
                    LOGGER.warning("session_load_invalid");
                    None
                }
            },
            Err(e) => {
                LOGGER.error(&format!("session_load_failed error={}", e));
                None
            }
        },
        Err(e) => {
            LOGGER.error(&format!("session_load_failed error={}", e));
            None
        }
    }
}

fn valid_listed_name(name: &str) -> bool {
    validate_name(name, "Session").is_ok()
}

pub fn list_sessions() -> Vec<String> {
    if !session_dir().is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&session_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(base) = name.strip_suffix(".json") {
                if base != "last" && valid_listed_name(base) {
                    out.push(base.to_string());
                }
            }
        }
    }
    out.sort();
    out.reverse();
    out
}

pub fn delete_session(name: &str) {
    let Ok(p) = session_path(name) else {
        return;
    };
    if p.exists() {
        let _ = fs::remove_file(&p);
    }
}
