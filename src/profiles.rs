use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;

use crate::logging::LOGGER;
use crate::persistence::validate_name;
use crate::settings::{self, Settings};

fn profile_dir() -> PathBuf {
    settings::config_dir().join("profiles")
}

fn ensure_dir() {
    let _ = fs::create_dir_all(&profile_dir());
    let _ = fs::set_permissions(&profile_dir(), fs::Permissions::from_mode(0o700));
}

pub fn profile_path(name: &str) -> PathBuf {
    ensure_dir();
    let name = validate_name(name, "Profile").unwrap_or_else(|e| {
        LOGGER.error(&e);
        "default".to_string()
    });
    profile_dir().join(format!("{}.json", name))
}

const COLOR_RE: &str = "^#[0-9a-fA-F]{6}$";

fn is_hex(s: &str) -> bool {
    regex::Regex::new(COLOR_RE).unwrap().is_match(s)
}

const PROFILE_KEYS: &[&str] = &[
    "font_name", "font_size", "color_scheme", "foreground_color", "background_color",
    "cursor_color", "cursor_shape", "highlight_color", "highlight_bg_color", "opacity",
    "enable_transparency", "scrollback_lines", "scrollbar_position", "custom_palette",
    "allow_bold_text", "cursor_blink", "tab_title_color", "tab_active_title_color",
    "shell_command", "login_shell", "encoding", "osc133", "backspace_binding",
    "delete_binding", "scroll_on_output", "scroll_on_keystroke", "window_padding_horizontal",
    "window_padding_vertical", "bell_notification", "undercurl_style",
];

fn validate_profile(data: &Value) -> Option<Value> {
    if !data.is_object() {
        return None;
    }
    let src = data.as_object().unwrap();
    let defs = settings::defaults();
    let mut valid = serde_json::Map::new();
    for key in PROFILE_KEYS {
        let Some(value) = src.get(*key) else {
            continue;
        };
        let default = defs.get(*key).unwrap_or(&Value::Null);
        let ok = match default {
            Value::Bool(_) => value.is_boolean(),
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    if !value.is_number() || value.is_f64() {
                        false
                    } else {
                        let v = value.as_i64().unwrap_or(0);
                        match *key {
                            "font_size" => (4..=128).contains(&v),
                            "scrollback_lines" => (-1..=1_000_000).contains(&v),
                            k if k.starts_with("window_padding_") => (0..=100).contains(&v),
                            _ => true,
                        }
                    }
                } else {
                    value.is_number()
                        && !value.is_boolean()
                        && if *key == "opacity" {
                            (0.1..=1.0).contains(&value.as_f64().unwrap_or(0.0))
                        } else {
                            true
                        }
                }
            }
            Value::String(_) => value.is_string() && value.as_str().unwrap().chars().count() <= 4096,
            Value::Null => value.is_null(),
            Value::Object(_) => {
                if let Value::Object(m) = value {
                    !m.iter().any(|(k, v)| {
                        k.chars().count() > 100 || !v.is_string() || v.as_str().unwrap().chars().count() > 4096
                    })
                } else {
                    false
                }
            }
            Value::Array(_) => false,
        };
        if !ok {
            return None;
        }
        valid.insert(key.to_string(), value.clone());
    }
    if let Some(Value::Object(pal)) = valid.get("custom_palette") {
        if pal.len() > 16
            || pal
                .iter()
                .any(|(k, v)| !v.is_string() || !is_hex(v.as_str().unwrap()) || k.is_empty())
        {
            return None;
        }
    }
    Some(Value::Object(valid))
}

pub fn save_profile(name: &str, settings_data: &Value) -> bool {
    let mut to_save = serde_json::Map::new();
    if let Some(obj) = settings_data.as_object() {
        for key in PROFILE_KEYS {
            if let Some(v) = obj.get(*key) {
                to_save.insert(key.to_string(), v.clone());
            }
        }
    }
    let validated = match validate_profile(&Value::Object(to_save)) {
        Some(v) => v,
        None => {
            LOGGER.warning("profile_save_invalid");
            return false;
        }
    };
    ensure_dir();
    let path = profile_path(name);
    let tmp = profile_dir().join(".profile_tmp");
    let _ = fs::remove_file(&tmp);
    match fs::write(&tmp, serde_json::to_string_pretty(&validated).unwrap_or_default()) {
        Ok(()) => {
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
            if fs::rename(&tmp, &path).is_ok() {
                return true;
            }
            LOGGER.error("profile_save_failed");
            false
        }
        Err(e) => {
            LOGGER.error(&format!("profile_save_failed error={}", e));
            false
        }
    }
}

pub fn load_profile(name: &str) -> Option<Value> {
    let p = profile_path(name);
    if !p.exists() {
        return None;
    }
    match fs::read_to_string(&p) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(data) => match validate_profile(&data) {
                Some(v) => Some(v),
                None => {
                    LOGGER.warning("profile_load_invalid");
                    None
                }
            },
            Err(e) => {
                LOGGER.error(&format!("profile_load_failed error={}", e));
                None
            }
        },
        Err(e) => {
            LOGGER.error(&format!("profile_load_failed error={}", e));
            None
        }
    }
}

pub fn delete_profile(name: &str) {
    let p = profile_path(name);
    if p.exists() {
        let _ = fs::remove_file(&p);
    }
}

pub fn apply_profile(settings_obj: &Settings, name: &str) -> bool {
    match load_profile(name) {
        Some(data) => {
            let mut updates = BTreeMap::new();
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    updates.insert(k.clone(), v.clone());
                }
            }
            let _ = settings_obj.set_many(updates);
            settings_obj.notify_changed();
            true
        }
        None => false,
    }
}

fn valid_listed_name(name: &str) -> bool {
    validate_name(name, "Profile").is_ok()
}

pub fn list_profiles() -> Vec<String> {
    if !profile_dir().is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&profile_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(base) = name.strip_suffix(".json") {
                if valid_listed_name(base) {
                    out.push(base.to_string());
                }
            }
        }
    }
    out.sort();
    out
}
