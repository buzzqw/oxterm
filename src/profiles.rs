use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;

use crate::logging::LOGGER;
use crate::persistence::validate_name;
use crate::persistence::{sync_parent_directory, write_private_temp};
use crate::settings::{self, Settings};

fn profile_dir() -> PathBuf {
    settings::config_dir().join("profiles")
}

fn ensure_dir() {
    let _ = fs::create_dir_all(&profile_dir());
    let _ = fs::set_permissions(&profile_dir(), fs::Permissions::from_mode(0o700));
}

pub fn profile_path(name: &str) -> Result<PathBuf, String> {
    ensure_dir();
    let name = validate_name(name, "Profile")?;
    Ok(profile_dir().join(format!("{}.json", name)))
}

const COLOR_RE: &str = "^#[0-9a-fA-F]{6}$";

fn is_hex(s: &str) -> bool {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(COLOR_RE).unwrap())
        .is_match(s)
}

const PROFILE_KEYS: &[&str] = &[
    "font_name",
    "font_size",
    "color_scheme",
    "foreground_color",
    "background_color",
    "cursor_color",
    "cursor_shape",
    "highlight_color",
    "highlight_bg_color",
    "opacity",
    "enable_transparency",
    "scrollback_lines",
    "scrollbar_position",
    "custom_palette",
    "allow_bold_text",
    "cursor_blink",
    "tab_title_color",
    "tab_active_title_color",
    "shell_command",
    "login_shell",
    "encoding",
    "osc133",
    "backspace_binding",
    "delete_binding",
    "scroll_on_output",
    "scroll_on_keystroke",
    "window_padding_horizontal",
    "window_padding_vertical",
    "bell_notification",
    "undercurl_style",
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
        if defs.get(*key).is_none() || !Settings::valid_value(key, value) {
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
    let path = match profile_path(name) {
        Ok(path) => path,
        Err(e) => {
            LOGGER.warning(&e);
            return false;
        }
    };
    match write_private_temp(
        &profile_dir(),
        "profile_tmp",
        serde_json::to_string_pretty(&validated)
            .unwrap_or_default()
            .as_bytes(),
    ) {
        Ok(tmp) => {
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
            if let Err(e) = fs::rename(&tmp, &path) {
                let _ = fs::remove_file(&tmp);
                LOGGER.error(&format!("profile_save_failed error={}", e));
                return false;
            }
            if let Err(e) = sync_parent_directory(&path) {
                LOGGER.error(&format!("profile_save_failed error={}", e));
                return false;
            }
            true
        }
        Err(e) => {
            LOGGER.error(&format!("profile_save_failed error={}", e));
            false
        }
    }
}

pub fn load_profile(name: &str) -> Option<Value> {
    let p = profile_path(name).ok()?;
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
    let Ok(p) = profile_path(name) else {
        return;
    };
    if p.exists() {
        let _ = fs::remove_file(&p);
    }
}

pub fn apply_profile(settings_obj: impl Borrow<Settings>, name: &str) -> bool {
    match load_profile(name) {
        Some(data) => {
            let mut updates = BTreeMap::new();
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    updates.insert(k.clone(), v.clone());
                }
            }
            let settings_obj = settings_obj.borrow();
            match settings_obj.set_many(updates) {
                Ok(()) => {
                    settings_obj.notify_changed();
                    true
                }
                Err(e) => {
                    LOGGER.warning(&format!("profile_apply_failed error={}", e));
                    false
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_values_settings_would_reject() {
        assert!(validate_profile(&json!({"cursor_color": "not-a-color"})).is_none());
        assert!(validate_profile(&json!({"font_size": 3})).is_none());
        assert!(validate_profile(&json!({"cursor_color": "#aabbcc"})).is_some());
    }
}
