use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use glib::prelude::*;
use glib::subclass::prelude::*;
use serde_json::{json, Map, Value};

use crate::logging::LOGGER;
use crate::persistence::temporary_path;

const HEX_COLOR_RE: &str = "^#[0-9a-fA-F]{6}$";

pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("tpgk")
}

/// Optional alternative settings file, set via `--config FILE`. When present it
/// replaces the default `settings.json` path so a throwaway configuration can be
/// used without touching the user's real config.
static CONFIG_FILE_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Point the settings file at `path` (from `--config`). Must be called before
/// settings are first loaded to have any effect.
pub fn set_config_file_override(path: PathBuf) {
    let _ = CONFIG_FILE_OVERRIDE.set(path);
}

pub fn config_file() -> PathBuf {
    if let Some(p) = CONFIG_FILE_OVERRIDE.get() {
        return p.clone();
    }
    config_dir().join("settings.json")
}

/// Session-only setting overrides coming from the command line (`-o key=value`,
/// `--font`, `--font-size`, `-p/--profile`). They take precedence over the
/// loaded configuration on every read but are **never** written back to disk, so
/// the user's stored settings stay untouched.
static OVERRIDES: OnceLock<Mutex<Map<String, Value>>> = OnceLock::new();

fn overrides() -> &'static Mutex<Map<String, Value>> {
    OVERRIDES.get_or_init(|| Mutex::new(Map::new()))
}

/// Register a session-only override for `key`. Applied on top of the stored
/// settings for all subsequent reads without being persisted.
pub fn set_override(key: &str, value: Value) {
    overrides().lock().unwrap().insert(key.to_string(), value);
}

fn override_value(key: &str) -> Option<Value> {
    overrides().lock().unwrap().get(key).cloned()
}

fn is_hex_color(s: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(HEX_COLOR_RE).unwrap())
        .is_match(s)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Cached copy of the default settings. `defaults()` used to rebuild the whole
/// JSON tree (and re-read environment variables) on *every* call, and it is hit
/// once per key while loading/validating settings, so building it once and
/// reusing a shared reference removes a large amount of redundant allocation on
/// startup and on every settings write.
fn defaults_ref() -> &'static Value {
    static DEFAULTS: OnceLock<Value> = OnceLock::new();
    DEFAULTS.get_or_init(build_defaults)
}

pub fn defaults() -> Value {
    defaults_ref().clone()
}

fn build_defaults() -> Value {
    let shell = env_or("SHELL", "/bin/bash");
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let editor = env_or("EDITOR", "nano");
    json!({
        "font_name": "Monospace",
        "font_size": 12,
        "color_scheme": "Dark (Default)",
        "foreground_color": "",
        "background_color": "",
        "cursor_color": "#ffffff",
        "cursor_shape": "block",
        "highlight_color": "#ffffff",
        "highlight_bg_color": "#446688",
        "scrollback_lines": 10000,
        "scrollbar_position": "right",
        "opacity": 1.0,
        "shell_command": shell,
        "notes_file": "",
        "notes_dir": home,
        "editor_command": editor,
        "ai_provider": "",
        "ai_models": {
            "openai": "", "claude": "", "gemini": "",
            "deepseek": "", "ollama": "", "custom": "",
        },
        "ai_urls": {
            "ollama": "http://localhost:11434/v1/chat/completions",
            "custom": "",
        },
        "ai_keys": {
            "openai": "", "claude": "", "gemini": "",
            "deepseek": "", "ollama": "", "custom": "",
        },
        "ai_last_provider": "",
        "ai_system_prompts": {},
        "osc133": false,
        "enable_transparency": false,
        "confirm_close": true,
        "tab_title": "Terminal",
        "tab_title_color": "#ffffff",
        "tab_active_title_color": "#ffffff",
        "encoding": "UTF-8",
        "auto_copy_selection": true,
        "file_manager": "",
        "backspace_binding": "ascii-del",
        "delete_binding": "escape-sequence",
        "custom_palette": Value::Null,
        "show_unsafe_paste_dialog": true,
        "show_tabs": true,
        "show_menubar": true,
        "show_toolbar": true,
        "show_stats": false,
        "dynamic_title": "replace",
        "login_shell": true,
        "cursor_blink": true,
        "scroll_on_output": true,
        "scroll_on_keystroke": true,
        "allow_bold_text": true,
        "terminal_columns": 80,
        "terminal_rows": 24,
        "window_padding_horizontal": 2,
        "window_padding_vertical": 2,
        "bell_notification": false,
        "undercurl_style": "single",
        "broadcast_input": false,
        "active_profile": "",
        "session_restore": true,
        "history_enabled": true,
        "hint_mode_enabled": true,
        "vi_copy_mode_enabled": false,
    })
}

pub type ColorMap = BTreeMap<&'static str, &'static str>;

fn init_data() -> BTreeMap<&'static str, ColorMap> {
    let dark: ColorMap = BTreeMap::from([
        ("black", "#0a0a0a"),
        ("red", "#cc0000"),
        ("green", "#4e9a06"),
        ("yellow", "#c4a000"),
        ("blue", "#3465a4"),
        ("magenta", "#75507b"),
        ("cyan", "#06989a"),
        ("white", "#d3d7cf"),
        ("brightblack", "#555753"),
        ("brightred", "#ef2929"),
        ("brightgreen", "#8ae234"),
        ("brightyellow", "#fce94f"),
        ("brightblue", "#729fcf"),
        ("brightmagenta", "#ad7fa8"),
        ("brightcyan", "#34e2e2"),
        ("brightwhite", "#eeeeec"),
    ]);
    let light: ColorMap = BTreeMap::from([
        ("black", "#2e3436"),
        ("red", "#cc0000"),
        ("green", "#4e9a06"),
        ("yellow", "#c4a000"),
        ("blue", "#3465a4"),
        ("magenta", "#75507b"),
        ("cyan", "#06989a"),
        ("white", "#d3d7cf"),
        ("brightblack", "#555753"),
        ("brightred", "#ef2929"),
        ("brightgreen", "#8ae234"),
        ("brightyellow", "#fce94f"),
        ("brightblue", "#729fcf"),
        ("brightmagenta", "#ad7fa8"),
        ("brightcyan", "#34e2e2"),
        ("brightwhite", "#eeeeec"),
    ]);
    let solarized_dark: ColorMap = BTreeMap::from([
        ("black", "#002b36"),
        ("red", "#dc322f"),
        ("green", "#859900"),
        ("yellow", "#b58900"),
        ("blue", "#268bd2"),
        ("magenta", "#d33682"),
        ("cyan", "#2aa198"),
        ("white", "#eee8d5"),
        ("brightblack", "#073642"),
        ("brightred", "#cb4b16"),
        ("brightgreen", "#586e75"),
        ("brightyellow", "#657b83"),
        ("brightblue", "#839496"),
        ("brightmagenta", "#6c71c4"),
        ("brightcyan", "#93a1a1"),
        ("brightwhite", "#fdf6e3"),
    ]);
    let solarized_light: ColorMap = BTreeMap::from([
        ("black", "#073642"),
        ("red", "#dc322f"),
        ("green", "#859900"),
        ("yellow", "#b58900"),
        ("blue", "#268bd2"),
        ("magenta", "#d33682"),
        ("cyan", "#2aa198"),
        ("white", "#eee8d5"),
        ("brightblack", "#002b36"),
        ("brightred", "#cb4b16"),
        ("brightgreen", "#586e75"),
        ("brightyellow", "#657b83"),
        ("brightblue", "#839496"),
        ("brightmagenta", "#6c71c4"),
        ("brightcyan", "#93a1a1"),
        ("brightwhite", "#fdf6e3"),
    ]);
    let gruvbox: ColorMap = BTreeMap::from([
        ("black", "#282828"),
        ("red", "#cc241d"),
        ("green", "#98971a"),
        ("yellow", "#d79921"),
        ("blue", "#458588"),
        ("magenta", "#b16286"),
        ("cyan", "#689d6a"),
        ("white", "#a89984"),
        ("brightblack", "#928374"),
        ("brightred", "#fb4934"),
        ("brightgreen", "#b8bb26"),
        ("brightyellow", "#fabd2f"),
        ("brightblue", "#83a598"),
        ("brightmagenta", "#d3869b"),
        ("brightcyan", "#8ec07c"),
        ("brightwhite", "#ebdbb2"),
    ]);
    let monokai: ColorMap = BTreeMap::from([
        ("black", "#272822"),
        ("red", "#f92672"),
        ("green", "#a6e22e"),
        ("yellow", "#f4bf75"),
        ("blue", "#66d9ef"),
        ("magenta", "#ae81ff"),
        ("cyan", "#a1efe4"),
        ("white", "#f8f8f2"),
        ("brightblack", "#75715e"),
        ("brightred", "#f92672"),
        ("brightgreen", "#a6e22e"),
        ("brightyellow", "#f4bf75"),
        ("brightblue", "#66d9ef"),
        ("brightmagenta", "#ae81ff"),
        ("brightcyan", "#a1efe4"),
        ("brightwhite", "#f9f8f5"),
    ]);
    let nord: ColorMap = BTreeMap::from([
        ("black", "#3b4252"),
        ("red", "#bf616a"),
        ("green", "#a3be8c"),
        ("yellow", "#ebcb8b"),
        ("blue", "#81a1c1"),
        ("magenta", "#b48ead"),
        ("cyan", "#88c0d0"),
        ("white", "#e5e9f0"),
        ("brightblack", "#4c566a"),
        ("brightred", "#bf616a"),
        ("brightgreen", "#a3be8c"),
        ("brightyellow", "#ebcb8b"),
        ("brightblue", "#81a1c1"),
        ("brightmagenta", "#b48ead"),
        ("brightcyan", "#8fbcbb"),
        ("brightwhite", "#eceff4"),
    ]);
    let matrix: ColorMap = BTreeMap::from([
        ("black", "#0d0208"),
        ("red", "#008f11"),
        ("green", "#00ff41"),
        ("yellow", "#00ff41"),
        ("blue", "#008f11"),
        ("magenta", "#00ff41"),
        ("cyan", "#00ff41"),
        ("white", "#00ff41"),
        ("brightblack", "#003b00"),
        ("brightred", "#00ff41"),
        ("brightgreen", "#00ff41"),
        ("brightyellow", "#00ff41"),
        ("brightblue", "#00ff41"),
        ("brightmagenta", "#00ff41"),
        ("brightcyan", "#00ff41"),
        ("brightwhite", "#00ff41"),
    ]);
    let mut all = BTreeMap::from([
        ("Dark (Default)", dark),
        ("Light", light),
        ("Solarized Dark", solarized_dark),
        ("Solarized Light", solarized_light),
        ("Gruvbox Dark", gruvbox),
        ("Monokai", monokai),
        ("Nord", nord),
        ("Matrix", matrix),
    ]);
    let schemes = [
        ("Dark (Default)", "#d3d7cf", "#0a0a0a"),
        ("Light", "#2e3436", "#ffffff"),
        ("Solarized Dark", "#839496", "#002b36"),
        ("Solarized Light", "#657b83", "#fdf6e3"),
        ("Gruvbox Dark", "#ebdbb2", "#282828"),
        ("Monokai", "#f8f8f2", "#272822"),
        ("Nord", "#e5e9f0", "#3b4252"),
        ("Matrix", "#00ff41", "#0d0208"),
    ];
    for (name, foreground, background) in schemes {
        if let Some(palette) = all.get_mut(name) {
            palette.insert("foreground", foreground);
            palette.insert("background", background);
        }
    }
    all
}

pub fn color_schemes() -> &'static BTreeMap<&'static str, ColorMap> {
    static SCHEMES: OnceLock<BTreeMap<&'static str, ColorMap>> = OnceLock::new();
    SCHEMES.get_or_init(|| {
        let all = init_data();
        all.into_iter()
            .map(|(k, v)| {
                (k, {
                    v.into_iter()
                        .filter(|(kk, _)| matches!(*kk, "foreground" | "background"))
                        .collect()
                })
            })
            .collect()
    })
}

pub fn color_palettes() -> &'static BTreeMap<&'static str, ColorMap> {
    static PALETTES: OnceLock<BTreeMap<&'static str, ColorMap>> = OnceLock::new();
    PALETTES.get_or_init(|| {
        let all = init_data();
        all.into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .filter(|(kk, _)| !matches!(*kk, "foreground" | "background"))
                        .collect(),
                )
            })
            .collect()
    })
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SettingsObject {
        pub data: Mutex<Value>,
        pub batch: Cell<bool>,
        pub loaded: AtomicBool,
        pub save_lock: Mutex<()>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsObject {
        const NAME: &'static str = "TpgkSettings";
        type Type = super::Settings;
    }

    impl ObjectImpl for SettingsObject {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![glib::subclass::Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    pub struct Settings(ObjectSubclass<imp::SettingsObject>);
}

impl Default for Settings {
    fn default() -> Self {
        glib::Object::new()
    }
}

// The GObject is only ever created and destroyed on the main thread; worker
// threads read snapshots under a lock. Refcounting is atomic in GObject, so
// sharing the wrapper across threads is safe.
unsafe impl Send for Settings {}
unsafe impl Sync for Settings {}

pub static SETTINGS: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();

pub fn settings() -> &'static Settings {
    SETTINGS.get_or_init(Settings::default)
}

impl Settings {
    pub fn connect_changed<F: Fn() + 'static>(&self, cb: F) -> glib::SignalHandlerId {
        self.connect_local("changed", false, move |_: &[glib::Value]| {
            cb();
            None
        })
    }

    pub fn disconnect_changed(&self, id: glib::SignalHandlerId) {
        self.disconnect(id);
    }

    pub fn notify_changed(&self) {
        if glib::MainContext::default().is_owner() {
            self.emit_by_name::<()>("changed", &[]);
        } else {
            let weak = self.downgrade();
            glib::MainContext::default().invoke(move || {
                if let Some(s) = weak.upgrade() {
                    s.emit_by_name::<()>("changed", &[]);
                }
            });
        }
    }

    fn data(&self) -> std::sync::MutexGuard<'_, Value> {
        self.imp().data.lock().unwrap()
    }

    fn data_mut(&self) -> std::sync::MutexGuard<'_, Value> {
        self.imp().data.lock().unwrap()
    }

    fn ensure_loaded(&self) {
        let imp = self.imp();
        if imp.loaded.load(Ordering::SeqCst) {
            return;
        }
        {
            let mut data = imp.data.lock().unwrap();
            if !data.is_object() {
                *data = defaults_ref().clone();
            }
        }
        let dir = config_dir();
        let path = config_file();
        let _ = fs::create_dir_all(&dir);
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        let mut rewrite = !path.exists();
        if !rewrite {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Value>(&content) {
                    Ok(Value::Object(map)) => {
                        let defs = defaults_ref();
                        let defs_map = defs.as_object().unwrap();
                        for (key, value) in map {
                            if defs_map.contains_key(&key) && Self::valid_value(&key, &value) {
                                imp.data
                                    .lock()
                                    .unwrap()
                                    .as_object_mut()
                                    .unwrap()
                                    .insert(key, value.clone());
                            } else if defs_map.contains_key(&key) {
                                LOGGER.warning(&format!("settings_value_invalid key={}", key));
                                rewrite = true;
                            }
                        }
                    }
                    Ok(_) => {
                        LOGGER.warning("settings root must be an object");
                        rewrite = true;
                    }
                    Err(e) => {
                        LOGGER.error(&format!("settings_load_failed error={}", e));
                        rewrite = true;
                    }
                },
                Err(e) => {
                    LOGGER.error(&format!("settings_load_failed error={}", e));
                    rewrite = true;
                }
            }
        }
        imp.loaded.store(true, Ordering::SeqCst);
        if rewrite {
            self.save();
        }
    }

    pub fn save(&self) {
        let imp = self.imp();
        if imp.batch.get() {
            return;
        }
        let _g = imp.save_lock.lock().unwrap();
        let target = config_file();
        // Write the temp file next to the final target so the atomic rename never
        // has to cross filesystems (relevant when `--config` points elsewhere).
        let target_dir = target
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(config_dir);
        let _ = fs::create_dir_all(&target_dir);
        let _ = fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o700));
        let tmp = temporary_path(&target_dir, "settings_tmp");
        let json_str = serde_json::to_string_pretty(&*imp.data.lock().unwrap());
        match json_str {
            Ok(s) => {
                if let Err(e) = fs::write(&tmp, s) {
                    LOGGER.error(&format!("settings_save_failed error={}", e));
                    return;
                }
                let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
                if target.exists() {
                    let backup = target.with_extension("json.bak");
                    if let Err(e) = fs::copy(&target, &backup) {
                        LOGGER.warning(&format!("settings_backup_failed error={}", e));
                    } else {
                        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
                    }
                }
                if let Err(e) = fs::rename(&tmp, &target) {
                    LOGGER.error(&format!("settings_save_failed error={}", e));
                }
            }
            Err(e) => LOGGER.error(&format!("settings_save_failed error={}", e)),
        }
    }

    pub fn load(&self) {
        self.ensure_loaded();
    }

    pub fn get(&self, key: &str) -> Value {
        if let Some(v) = override_value(key) {
            return v;
        }
        self.ensure_loaded();
        self.data().get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn get_default(&self, key: &str, default: Value) -> Value {
        if let Some(v) = override_value(key) {
            return v;
        }
        self.ensure_loaded();
        self.data().get(key).cloned().unwrap_or(default)
    }

    pub fn get_str(&self, key: &str) -> String {
        self.get_default(key, Value::String(String::new()))
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    pub fn get_str_default(&self, key: &str, default: &str) -> String {
        self.get_default(key, Value::String(default.to_string()))
            .as_str()
            .unwrap_or(default)
            .to_string()
    }

    pub fn get_i64(&self, key: &str) -> i64 {
        self.get_default(key, json!(0)).as_i64().unwrap_or(0)
    }

    pub fn get_f64(&self, key: &str) -> f64 {
        self.get_default(key, json!(0.0)).as_f64().unwrap_or(0.0)
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.get_default(key, json!(false))
            .as_bool()
            .unwrap_or(false)
    }

    pub fn get_obj(&self, key: &str) -> Value {
        self.get_default(key, json!({}))
    }

    pub fn set(&self, key: &str, value: Value) -> Result<(), String> {
        self.ensure_loaded();
        if defaults_ref().as_object().unwrap().contains_key(key) && !Self::valid_value(key, &value)
        {
            return Err(format!("Invalid setting value: {}", key));
        }
        self.data_mut()
            .as_object_mut()
            .unwrap()
            .insert(key.to_string(), value);
        self.save();
        Ok(())
    }

    pub fn set_bool(&self, key: &str, value: bool) {
        let _ = self.set(key, Value::Bool(value));
    }

    pub fn set_str(&self, key: &str, value: &str) {
        let _ = self.set(key, Value::String(value.to_string()));
    }

    pub fn set_i64(&self, key: &str, value: i64) {
        let _ = self.set(key, json!(value));
    }

    pub fn set_many(&self, updates: BTreeMap<String, Value>) -> Result<(), String> {
        self.ensure_loaded();
        let defs = defaults_ref();
        for (key, value) in &updates {
            if defs.as_object().unwrap().contains_key(key) && !Self::valid_value(key, value) {
                return Err(format!("Invalid setting value: {}", key));
            }
        }
        {
            let mut data = self.data_mut();
            let obj = data.as_object_mut().unwrap();
            for (key, value) in updates {
                obj.insert(key, value);
            }
        }
        self.save();
        Ok(())
    }

    pub fn begin_batch(&self) {
        self.ensure_loaded();
        self.imp().batch.set(true);
    }

    pub fn end_batch(&self) {
        self.imp().batch.set(false);
        self.save();
    }

    pub fn raw_data(&self) -> Value {
        self.ensure_loaded();
        self.data().clone()
    }

    pub fn get_color_scheme(&self) -> (String, String) {
        self.ensure_loaded();
        let data = self.data();
        let scheme = data
            .get("color_scheme")
            .and_then(|v| v.as_str())
            .unwrap_or("Dark (Default)")
            .to_string();
        let pal = color_schemes()
            .get(scheme.as_str())
            .unwrap_or_else(|| color_schemes().get("Dark (Default)").unwrap());
        (
            pal.get("foreground").unwrap_or(&"#d3d7cf").to_string(),
            pal.get("background").unwrap_or(&"#0a0a0a").to_string(),
        )
    }

    pub fn get_palette(&self) -> Value {
        self.ensure_loaded();
        let custom = self.data().get("custom_palette").cloned();
        if let Some(Value::Object(map)) = custom {
            if !map.is_empty() {
                return Value::Object(map);
            }
        }
        let data = self.data();
        let scheme = data
            .get("color_scheme")
            .and_then(|v| v.as_str())
            .unwrap_or("Dark (Default)");
        let pal = color_palettes()
            .get(scheme)
            .unwrap_or_else(|| color_palettes().get("Dark (Default)").unwrap());
        Value::Object(
            pal.iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .collect(),
        )
    }

    pub fn get_fg_color(&self) -> String {
        let explicit = self.get_str("foreground_color");
        if !explicit.is_empty() {
            return explicit;
        }
        let (fg, _) = self.get_color_scheme();
        fg
    }

    pub fn get_bg_color(&self) -> String {
        let explicit = self.get_str("background_color");
        if !explicit.is_empty() {
            return explicit;
        }
        let (_, bg) = self.get_color_scheme();
        bg
    }

    fn valid_value(key: &str, value: &Value) -> bool {
        let default = defaults_ref().get(key).cloned().unwrap_or(Value::Null);
        match &default {
            Value::Bool(_) => value.is_boolean(),
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    if !value.is_number() || value.is_f64() {
                        return false;
                    }
                    let v = value.as_i64().unwrap_or(0);
                    let limits: Option<(i64, i64)> = match key {
                        "font_size" => Some((4, 128)),
                        "scrollback_lines" => Some((-1, 1_000_000)),
                        "terminal_columns" => Some((40, 300)),
                        "terminal_rows" => Some((10, 120)),
                        "window_padding_horizontal" => Some((0, 100)),
                        "window_padding_vertical" => Some((0, 100)),
                        _ => None,
                    };
                    match limits {
                        Some((lo, hi)) => v >= lo && v <= hi,
                        None => true,
                    }
                } else {
                    value.is_number()
                        && !value.is_boolean()
                        && (value.as_f64().unwrap_or(0.0) >= 0.1)
                        && (value.as_f64().unwrap_or(0.0) <= 1.0)
                }
            }
            Value::String(_) => {
                if !value.is_string() {
                    return false;
                }
                let s = value.as_str().unwrap();
                if s.chars().count() > 4096 {
                    return false;
                }
                if (key == "foreground_color" || key == "background_color") && !s.is_empty() {
                    return is_hex_color(s);
                }
                if matches!(
                    key,
                    "cursor_color"
                        | "highlight_color"
                        | "highlight_bg_color"
                        | "tab_title_color"
                        | "tab_active_title_color"
                ) {
                    return is_hex_color(s);
                }
                true
            }
            Value::Null => match key {
                "custom_palette" => {
                    if value.is_null() {
                        return true;
                    }
                    if let Value::Object(map) = value {
                        if map.len() <= 16 {
                            return map
                                .iter()
                                .all(|(_k, v)| v.is_string() && is_hex_color(v.as_str().unwrap()));
                        }
                    }
                    false
                }
                _ => value.is_null(),
            },
            Value::Object(_) => {
                if let Value::Object(map) = value {
                    if map.len() > 100 {
                        return false;
                    }
                    return map.iter().all(|(k, v)| {
                        k.chars().count() <= 100
                            && v.is_string()
                            && v.as_str().unwrap().chars().count() <= 4096
                    });
                }
                false
            }
            Value::Array(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_schemes_have_foreground_and_background() {
        let light = color_schemes().get("Light").unwrap();
        assert_eq!(light.get("foreground"), Some(&"#2e3436"));
        assert_eq!(light.get("background"), Some(&"#ffffff"));
        assert_eq!(color_palettes().get("Light").unwrap().len(), 16);
    }

    #[test]
    fn validates_settings_limits_and_colors() {
        assert!(Settings::valid_value("scrollback_lines", &json!(1_000_000)));
        assert!(Settings::valid_value("scrollback_lines", &json!(-1)));
        assert!(!Settings::valid_value(
            "scrollback_lines",
            &json!(1_000_001)
        ));
        assert!(Settings::valid_value("opacity", &json!(0.1)));
        assert!(!Settings::valid_value("opacity", &json!(0.09)));
        assert!(Settings::valid_value("foreground_color", &json!("#abcdef")));
        assert!(!Settings::valid_value("foreground_color", &json!("#abc")));
    }
}

pub fn json_to_str_map(value: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Value::Object(map) = value {
        for (k, v) in map {
            out.insert(k.clone(), v.as_str().unwrap_or("").to_string());
        }
    }
    out
}

pub fn str_map_to_json(map: &BTreeMap<String, String>) -> Value {
    let mut obj = Map::new();
    for (k, v) in map {
        obj.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(obj)
}
