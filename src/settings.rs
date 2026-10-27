use std::cell::{Cell, OnceCell};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use serde_json::{json, Map, Value};

use crate::logging::LOGGER;
use crate::persistence::{sync_parent_directory, write_private_temp};

use crate::headless::{settings as headless_settings, spec, themes};

pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("oxterm")
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
pub fn set_override(key: &str, value: Value) -> Result<(), String> {
    if defaults_ref().as_object().unwrap().contains_key(key) && !Settings::valid_value(key, &value)
    {
        return Err(format!("Invalid setting value: {}", key));
    }
    overrides()
        .lock()
        .map_err(|_| "settings overrides lock is unavailable".to_string())?
        .insert(key.to_string(), value);
    Ok(())
}

fn override_value(key: &str) -> Option<Value> {
    overrides().lock().unwrap().get(key).cloned()
}

/// Keys overridden for this session, sorted. Used by the diagnostics report so
/// a bug report shows *which* settings were changed without leaking values.
pub fn override_keys() -> Vec<String> {
    let mut keys: Vec<String> = overrides()
        .lock()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
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
    // Every key, default and description lives in the headless spec so the
    // GUI, --show-config and the generated documentation cannot drift apart.
    spec::defaults()
}

/// All available themes (built-in plus files from `~/.config/oxterm/themes/`).
pub fn color_schemes() -> &'static BTreeMap<String, themes::Theme> {
    themes::all()
}

/// Alias kept for the palette editor: a theme carries both foreground/background
/// and the 16 ANSI colors.
pub fn color_palettes() -> &'static BTreeMap<String, themes::Theme> {
    themes::all()
}

/// Whether the desktop currently asks for a dark theme. GTK exposes this for
/// GNOME, XFCE, Cinnamon and most other desktops.
pub fn system_prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|settings| settings.is_gtk_application_prefer_dark_theme())
        .unwrap_or(false)
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

thread_local! {
    // GObjects are thread-affine. Each thread gets its own settings facade, so
    // GTK state is never sent across threads; disk writes remain serialized.
    static SETTINGS: OnceCell<Settings> = const { OnceCell::new() };
}

static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// Set when an unreadable settings file could not be moved aside; saving would
/// otherwise replace the user's settings (including API keys) with defaults.
static SAVE_DISABLED: AtomicBool = AtomicBool::new(false);

/// Move a settings file that failed to load out of the way instead of
/// overwriting it with defaults, so the user can recover it by hand.
fn preserve_unreadable_settings(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".corrupt-{}", stamp));
    let dest = path.with_file_name(name);
    match fs::rename(path, &dest) {
        Ok(()) => LOGGER.warning(&format!(
            "settings_preserved path={}",
            dest.to_string_lossy()
        )),
        Err(e) => {
            LOGGER.error(&format!("settings_preserve_failed error={}", e));
            SAVE_DISABLED.store(true, Ordering::SeqCst);
        }
    }
}

pub fn settings() -> Settings {
    SETTINGS.with(|slot| slot.get_or_init(Settings::default).clone())
}

/// The letter used with Ctrl as Oxterm's tmux-style prefix. Falls back to `b`
/// when the setting is missing or invalid.
pub fn tmux_prefix_char() -> char {
    settings()
        .get_str_default("tmux_prefix_key", "b")
        .chars()
        .next()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_alphabetic())
        .unwrap_or('b')
}

/// The control byte sent by the prefix key (Ctrl+A = 0x01, Ctrl+B = 0x02...).
pub fn tmux_prefix_byte() -> u8 {
    (tmux_prefix_char() as u8) & 0x1f
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
        // Signals and their GTK listeners must run on the owning main context.
        glib::MainContext::default().invoke_local({
            let weak = self.downgrade();
            move || {
                if let Some(s) = weak.upgrade() {
                    s.emit_by_name::<()>("changed", &[]);
                }
            }
        });
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
            let loaded = headless_settings::read_document(&path)
                .and_then(|raw| headless_settings::load_document(&raw));
            match loaded {
                Ok(loaded) => {
                    for issue in &loaded.issues {
                        if issue.fatal {
                            LOGGER.warning(&format!(
                                "settings_value_invalid key={} error={}",
                                issue.key, issue.message
                            ));
                        } else {
                            LOGGER.warning(&format!("settings_key_unknown key={}", issue.key));
                        }
                    }
                    *imp.data.lock().unwrap() = loaded.settings;
                    rewrite = loaded.rewrite;
                }
                Err(e) => {
                    LOGGER.error(&format!("settings_load_failed error={}", e));
                    preserve_unreadable_settings(&path);
                }
            }
        }
        imp.loaded.store(true, Ordering::SeqCst);
        if rewrite {
            if let Err(e) = self.save() {
                LOGGER.error(&format!("settings_save_failed error={}", e));
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let imp = self.imp();
        if imp.batch.get() {
            return Ok(());
        }
        if SAVE_DISABLED.load(Ordering::SeqCst) {
            return Err("settings file could not be read; refusing to overwrite it".to_string());
        }
        let _g = SAVE_LOCK.lock().unwrap();
        let target = config_file();
        // Write the temp file next to the final target so the atomic rename never
        // has to cross filesystems (relevant when `--config` points elsewhere).
        let target_dir = target
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(config_dir);
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
        // Only lock down oxterm's own config directory. A `--config` override
        // may live in a shared directory whose permissions are not ours.
        if target_dir == config_dir() {
            fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let json_str = serde_json::to_string_pretty(&*imp.data.lock().unwrap());
        match json_str {
            Ok(s) => {
                let tmp = match write_private_temp(&target_dir, "settings_tmp", s.as_bytes()) {
                    Ok(tmp) => tmp,
                    Err(e) => return Err(e.to_string()),
                };
                if let Err(e) = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e.to_string());
                }
                if target.exists() {
                    let backup = target.with_extension("json.bak");
                    if backup
                        .symlink_metadata()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false)
                    {
                        LOGGER.warning("settings_backup_failed error=backup is a symbolic link");
                    } else if let Err(e) = fs::copy(&target, &backup) {
                        LOGGER.warning(&format!("settings_backup_failed error={}", e));
                    } else {
                        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
                    }
                }
                if let Err(e) = fs::rename(&tmp, &target) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e.to_string());
                }
                sync_parent_directory(&target).map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
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
        self.save()
    }

    pub fn set_bool(&self, key: &str, value: bool) {
        if let Err(e) = self.set(key, Value::Bool(value)) {
            LOGGER.error(&format!("settings_save_failed error={}", e));
        }
    }

    pub fn set_str(&self, key: &str, value: &str) {
        if let Err(e) = self.set(key, Value::String(value.to_string())) {
            LOGGER.error(&format!("settings_save_failed error={}", e));
        }
    }

    pub fn set_i64(&self, key: &str, value: i64) {
        if let Err(e) = self.set(key, json!(value)) {
            LOGGER.error(&format!("settings_save_failed error={}", e));
        }
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
        self.save()
    }

    pub fn begin_batch(&self) {
        self.ensure_loaded();
        self.imp().batch.set(true);
    }

    pub fn end_batch(&self) {
        self.imp().batch.set(false);
        if let Err(e) = self.save() {
            LOGGER.error(&format!("settings_save_failed error={}", e));
        }
    }

    pub fn raw_data(&self) -> Value {
        self.ensure_loaded();
        self.data().clone()
    }

    /// Theme actually in use, resolving `auto_theme` against the desktop's
    /// dark-mode preference. Falls back to `color_scheme` when the chosen
    /// light/dark theme no longer exists (for example a removed theme file).
    pub fn effective_theme(&self) -> &'static themes::Theme {
        let mut name = self.get_str_default("color_scheme", "Dark (Default)");
        if self.get_bool("auto_theme") {
            let preferred = if system_prefers_dark() {
                self.get_str_default("theme_dark", "Dark (Default)")
            } else {
                self.get_str_default("theme_light", "Light")
            };
            if themes::get(&preferred).is_some() {
                name = preferred;
            }
        }
        themes::get(&name).unwrap_or_else(themes::default_theme)
    }

    pub fn get_color_scheme(&self) -> (String, String) {
        let theme = self.effective_theme();
        (theme.foreground.clone(), theme.background.clone())
    }

    pub fn get_palette(&self) -> Value {
        // `get` applies session overrides, so `-o color_scheme=...` and colour
        // schemes loaded from a profile change the palette as well.
        let custom = self.get("custom_palette");
        if let Value::Object(map) = custom {
            if !map.is_empty() {
                return Value::Object(map);
            }
        }
        Value::Object(
            self.effective_theme()
                .palette
                .iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.clone())))
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

    pub(crate) fn valid_value(key: &str, value: &Value) -> bool {
        match spec::find(key) {
            Some(setting) => spec::validate(setting, value).is_ok(),
            // Unknown keys are accepted for forward compatibility; `--validate-config`
            // reports them separately as warnings.
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_schemes_have_foreground_and_background() {
        let light = color_schemes().get("Light").unwrap();
        assert_eq!(light.foreground, "#2e3436");
        assert_eq!(light.background, "#ffffff");
        assert_eq!(color_palettes().get("Light").unwrap().palette.len(), 16);
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

    #[test]
    fn validates_tmux_prefix_key() {
        assert!(Settings::valid_value("tmux_prefix_key", &json!("a")));
        assert!(Settings::valid_value("tmux_prefix_key", &json!("B")));
        assert!(!Settings::valid_value("tmux_prefix_key", &json!("")));
        assert!(!Settings::valid_value("tmux_prefix_key", &json!("ab")));
        assert!(!Settings::valid_value("tmux_prefix_key", &json!("1")));
    }

    #[test]
    fn unreadable_settings_file_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!(
            "oxterm-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        preserve_unreadable_settings(&path);
        assert!(!path.exists());
        let preserved: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(preserved.len(), 1);
        assert!(preserved[0].starts_with("settings.json.corrupt-"));
        assert_eq!(
            fs::read_to_string(dir.join(&preserved[0])).unwrap(),
            "{ not json"
        );
        assert!(!SAVE_DISABLED.load(Ordering::SeqCst));
        fs::remove_dir_all(dir).unwrap();
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
