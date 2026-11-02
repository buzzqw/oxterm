//! Installed plugins: manifests, the lock file and install/update/remove.
//!
//! A plugin is a directory under `$XDG_DATA_HOME/oxterm/plugins/<id>` with a
//! `plugin.json` manifest:
//!
//! ```json
//! {
//!   "id": "acme.git-status",
//!   "name": "Git status",
//!   "version": "1.0.0",
//!   "api_version": 1,
//!   "permissions": ["notify", "workspace"],
//!   "events": ["command_finished"],
//!   "command": ["./run.sh"]
//! }
//! ```
//!
//! The manifest is validated here. Installing from a local directory copies it;
//! installing from a git URL clones it. The lock file records where each plugin
//! came from so `plugin update` knows what to do.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{events, paths, persistence};

/// Manifest API version this build understands.
pub const CURRENT_API: u64 = 1;

/// Lock file version.
pub const LOCK_VERSION: u64 = 1;

/// Largest accepted manifest.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Bound on a plugin command line.
pub const MAX_COMMAND: usize = 8192;

/// Permissions a manifest may request. Some are enforced for actions that
/// identify the plugin; the rest are declarative and shown by `plugin list`.
pub const PERMISSIONS: &[&str] = &[
    "events",
    "read-screen",
    "send-input",
    "spawn",
    "open-files",
    "network",
    "clipboard",
    "notify",
    "workspace",
    "status-bar",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u64,
    pub permissions: Vec<String>,
    pub events: Vec<String>,
    pub command: Vec<String>,
    pub enabled: bool,
}

impl Manifest {
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|entry| entry == permission)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub source: String,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockEntry {
    pub id: String,
    pub version: String,
    pub source: String,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    pub plugins: Vec<LockEntry>,
}

impl Lock {
    fn find(&self, id: &str) -> Option<&LockEntry> {
        self.plugins.iter().find(|entry| entry.id == id)
    }

    fn upsert(&mut self, entry: LockEntry) {
        if let Some(existing) = self.plugins.iter_mut().find(|item| item.id == entry.id) {
            *existing = entry;
        } else {
            self.plugins.push(entry);
        }
    }
}

/// A plugin store rooted at a data directory. Tests use a temporary root.
#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_store() -> Self {
        Self::new(paths::data_dir())
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins")
    }

    pub fn plugin_dir(&self, id: &str) -> PathBuf {
        self.plugins_dir().join(id)
    }

    pub fn state_dir(&self, id: &str) -> PathBuf {
        self.root.join("state").join(id)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("plugins.lock.json")
    }

    pub fn load_lock(&self) -> Result<Lock, String> {
        let path = self.lock_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Lock::default())
            }
            Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
        };
        let value: Value =
            serde_json::from_str(&raw).map_err(|error| format!("invalid JSON: {}", error))?;
        let object = value
            .as_object()
            .ok_or("the plugin lock must contain a JSON object")?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("the plugin lock has no \"version\"")?;
        if version > LOCK_VERSION {
            return Err(format!(
                "the plugin lock was written by a newer Oxterm (version {})",
                version
            ));
        }
        let mut plugins = Vec::new();
        for row in object
            .get("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = row
                .get("id")
                .and_then(Value::as_str)
                .ok_or("a plugin lock entry has no id")?;
            validate_id(id)?;
            plugins.push(LockEntry {
                id: id.to_string(),
                version: row
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                source: row
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                commit: row
                    .get("commit")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        Ok(Lock { plugins })
    }

    fn save_lock(&self, lock: &Lock) -> Result<(), String> {
        let plugins: Vec<Value> = lock
            .plugins
            .iter()
            .map(|entry| {
                json!({
                    "id": entry.id,
                    "version": entry.version,
                    "source": entry.source,
                    "commit": entry.commit,
                })
            })
            .collect();
        let document = serde_json::to_vec_pretty(&json!({
            "version": LOCK_VERSION,
            "plugins": plugins,
        }))
        .map_err(|error| error.to_string())?;
        if let Some(parent) = self.lock_path().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {}", parent.display(), error))?;
        }
        persistence::atomic_write(&self.lock_path(), &document)
            .map_err(|error| format!("cannot write the plugin lock: {}", error))
    }

    /// Every valid plugin directory, sorted by id.
    pub fn list(&self) -> Result<Vec<Installed>, String> {
        let lock = self.load_lock()?;
        let directory = self.plugins_dir();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("cannot read {}: {}", directory.display(), error)),
        };
        let mut plugins = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let manifest = match read_manifest(&dir) {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };
            if manifest.id != name {
                continue;
            }
            let locked = lock.find(&manifest.id);
            plugins.push(Installed {
                manifest,
                dir,
                source: locked.map(|entry| entry.source.clone()).unwrap_or_default(),
                commit: locked.and_then(|entry| entry.commit.clone()),
            });
        }
        plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(plugins)
    }

    pub fn find(&self, id: &str) -> Result<Installed, String> {
        validate_id(id)?;
        self.list()?
            .into_iter()
            .find(|entry| entry.manifest.id == id)
            .ok_or_else(|| format!("no plugin named {}", id))
    }

    /// Install from a local directory or a git URL.
    pub fn install(&self, source: &str, force: bool) -> Result<Installed, String> {
        let source = source.trim();
        if source.is_empty() {
            return Err("plugin install requires a directory or a git URL".to_string());
        }
        if Path::new(source).is_dir() {
            self.install_local(Path::new(source), force)
        } else if is_git_source(source) {
            self.install_git(source, force)
        } else {
            Err(format!(
                "{} is neither a directory nor a git URL (https://, ssh://, git@ or file://)",
                source
            ))
        }
    }

    fn install_local(&self, source: &Path, force: bool) -> Result<Installed, String> {
        let source = std::fs::canonicalize(source)
            .map_err(|error| format!("cannot resolve {}: {}", source.display(), error))?;
        let manifest = read_manifest(&source)?;
        self.check_target(&manifest.id, force)?;
        let temp = self.temp_dir(&manifest.id)?;
        let result = (|| {
            copy_dir(&source, &temp)?;
            let copied = read_manifest(&temp)?;
            if copied.id != manifest.id {
                return Err("the copied manifest has a different id".to_string());
            }
            self.finish_install(&manifest.id, &temp, force)?;
            Ok(Installed {
                manifest: copied,
                dir: self.plugin_dir(&manifest.id),
                source: source.to_string_lossy().to_string(),
                commit: None,
            })
        })();
        let _ = std::fs::remove_dir_all(&temp);
        let installed = result?;
        self.record(&installed)?;
        Ok(installed)
    }

    fn install_git(&self, source: &str, force: bool) -> Result<Installed, String> {
        let temp = self.temp_dir("clone")?;
        let result = (|| {
            let output = std::process::Command::new("git")
                .args(["clone", "--depth", "1", "--", source])
                .arg(&temp)
                .output()
                .map_err(|error| format!("cannot run git: {}", error))?;
            if !output.status.success() {
                return Err(format!(
                    "git clone failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let manifest = read_manifest(&temp)?;
            self.check_target(&manifest.id, force)?;
            let commit = std::process::Command::new("git")
                .args(["-C"])
                .arg(&temp)
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|commit| !commit.is_empty());
            self.finish_install(&manifest.id, &temp, force)?;
            let dir = self.plugin_dir(&manifest.id);
            Ok(Installed {
                manifest,
                dir,
                source: source.to_string(),
                commit,
            })
        })();
        let _ = std::fs::remove_dir_all(&temp);
        let installed = result?;
        self.record(&installed)?;
        Ok(installed)
    }

    fn temp_dir(&self, name: &str) -> Result<PathBuf, String> {
        let directory = self.plugins_dir();
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {}", directory.display(), error))?;
        std::fs::set_permissions(
            &directory,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .ok();
        let temp = directory.join(format!(
            ".install-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&temp);
        Ok(temp)
    }

    fn check_target(&self, id: &str, force: bool) -> Result<(), String> {
        validate_id(id)?;
        let target = self.plugin_dir(id);
        if target.exists() && !force {
            return Err(format!(
                "plugin {} is already installed; use --force to replace it",
                id
            ));
        }
        Ok(())
    }

    fn finish_install(&self, id: &str, temp: &Path, force: bool) -> Result<(), String> {
        let target = self.plugin_dir(id);
        if target.exists() {
            if !force {
                return Err(format!("plugin {} is already installed", id));
            }
            std::fs::remove_dir_all(&target)
                .map_err(|error| format!("cannot replace {}: {}", target.display(), error))?;
        }
        std::fs::rename(temp, &target)
            .map_err(|error| format!("cannot install {}: {}", target.display(), error))
    }

    fn record(&self, installed: &Installed) -> Result<(), String> {
        let mut lock = self.load_lock()?;
        lock.upsert(LockEntry {
            id: installed.manifest.id.clone(),
            version: installed.manifest.version.clone(),
            source: installed.source.clone(),
            commit: installed.commit.clone(),
        });
        self.save_lock(&lock)
    }

    pub fn remove(&self, id: &str, purge: bool) -> Result<(), String> {
        validate_id(id)?;
        let dir = self.plugin_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|error| format!("cannot remove {}: {}", dir.display(), error))?;
        }
        let mut lock = self.load_lock()?;
        lock.plugins.retain(|entry| entry.id != id);
        self.save_lock(&lock)?;
        if purge {
            let state = self.state_dir(id);
            if state.exists() {
                std::fs::remove_dir_all(&state)
                    .map_err(|error| format!("cannot remove {}: {}", state.display(), error))?;
            }
        }
        Ok(())
    }

    /// Update git-installed plugins. Returns one human-readable line per plugin.
    pub fn update(&self, only: Option<&str>) -> Result<Vec<String>, String> {
        let plugins = self.list()?;
        let mut messages = Vec::new();
        for installed in plugins {
            if let Some(id) = only {
                if installed.manifest.id != id {
                    continue;
                }
            }
            let id = &installed.manifest.id;
            if !is_git_source(&installed.source) {
                messages.push(format!(
                    "{}: installed from a local directory; reinstall",
                    id
                ));
                continue;
            }
            let fetch = std::process::Command::new("git")
                .args(["-C"])
                .arg(&installed.dir)
                .args(["fetch", "--depth", "1", "origin"])
                .output()
                .map_err(|error| format!("cannot run git: {}", error))?;
            if !fetch.status.success() {
                messages.push(format!(
                    "{}: git fetch failed: {}",
                    id,
                    String::from_utf8_lossy(&fetch.stderr).trim()
                ));
                continue;
            }
            let reset = std::process::Command::new("git")
                .args(["-C"])
                .arg(&installed.dir)
                .args(["reset", "--hard", "FETCH_HEAD"])
                .output()
                .map_err(|error| format!("cannot run git: {}", error))?;
            if !reset.status.success() {
                messages.push(format!(
                    "{}: git reset failed: {}",
                    id,
                    String::from_utf8_lossy(&reset.stderr).trim()
                ));
                continue;
            }
            let manifest = read_manifest(&installed.dir)?;
            let commit = std::process::Command::new("git")
                .args(["-C"])
                .arg(&installed.dir)
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|commit| !commit.is_empty());
            let mut lock = self.load_lock()?;
            lock.upsert(LockEntry {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                source: installed.source.clone(),
                commit,
            });
            self.save_lock(&lock)?;
            messages.push(format!("{}: updated to {}", manifest.id, manifest.version));
        }
        if let Some(id) = only {
            if !messages.iter().any(|message| message.starts_with(id)) {
                return Err(format!("no plugin named {}", id));
            }
        }
        Ok(messages)
    }
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("plugin.json");
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("cannot read {}: {}", path.display(), error))?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{} is larger than {} KiB",
            path.display(),
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {}", path.display(), error))?;
    parse_manifest(&raw)
}

pub fn parse_manifest(raw: &str) -> Result<Manifest, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {}", error))?;
    let object = value
        .as_object()
        .ok_or("the plugin manifest must contain a JSON object")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or("the plugin manifest has no \"id\"")?;
    validate_id(id)?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .trim()
        .to_string();
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .ok_or("the plugin manifest has no \"version\"")?
        .to_string();
    let api_version = object
        .get("api_version")
        .and_then(Value::as_u64)
        .ok_or("the plugin manifest has no \"api_version\"")?;
    if api_version != CURRENT_API {
        return Err(format!(
            "plugin {} uses API {}; this build supports {}",
            id, api_version, CURRENT_API
        ));
    }
    let permissions = string_list(object.get("permissions"), "permissions")?;
    for permission in &permissions {
        if !PERMISSIONS.contains(&permission.as_str()) {
            return Err(format!(
                "plugin {} requests unknown permission '{}' (known: {})",
                id,
                permission,
                PERMISSIONS.join(", ")
            ));
        }
    }
    let events = string_list(object.get("events"), "events")?;
    for event in &events {
        if !events::EVENT_NAMES.contains(&event.as_str()) {
            return Err(format!(
                "plugin {} subscribes to unknown event '{}'",
                id, event
            ));
        }
    }
    let command = match object.get("command").and_then(Value::as_array) {
        Some(values) => {
            let mut command = Vec::with_capacity(values.len());
            for value in values {
                let part = value
                    .as_str()
                    .ok_or_else(|| format!("plugin {}: command must be an array of strings", id))?;
                command.push(part.to_string());
            }
            if command.is_empty() || command[0].trim().is_empty() {
                return Err(format!("plugin {} has an empty command", id));
            }
            if command.iter().map(String::len).sum::<usize>() > MAX_COMMAND {
                return Err(format!("plugin {}: the command is too long", id));
            }
            command
        }
        None => return Err(format!("plugin {} has no \"command\" array", id)),
    };
    Ok(Manifest {
        id: id.to_string(),
        name,
        version,
        api_version,
        permissions,
        events,
        command,
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

fn string_list(value: Option<&Value>, key: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| format!("\"{}\" must be an array", key))?;
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let text = entry
            .as_str()
            .ok_or_else(|| format!("\"{}\" must contain strings", key))?
            .to_string();
        if !out.contains(&text) {
            out.push(text);
        }
    }
    Ok(out)
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id == "." || id == ".." {
        return Err("plugin id cannot be empty".to_string());
    }
    if id.chars().count() > 64 {
        return Err("plugin id is longer than 64 characters".to_string());
    }
    if !id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return Err(format!(
            "plugin id '{}' may only contain letters, digits, '.', '-' and '_'",
            id
        ));
    }
    Ok(())
}

fn is_git_source(source: &str) -> bool {
    source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("ssh://")
        || source.starts_with("git@")
        || source.starts_with("file://")
        || source.ends_with(".git")
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target)
        .map_err(|error| format!("cannot create {}: {}", target.display(), error))?;
    for entry in std::fs::read_dir(source)
        .map_err(|error| format!("cannot read {}: {}", source.display(), error))?
        .flatten()
    {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {}", from.display(), error))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|error| format!("cannot copy {}: {}", from.display(), error))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "oxterm-plugins-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_plugin(dir: &Path, id: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            format!(
                r##"{{
                    "id": "{}",
                    "name": "Test",
                    "version": "1.2.3",
                    "api_version": 1,
                    "permissions": ["notify", "workspace"],
                    "events": ["command_finished"],
                    "command": ["./run.sh"]
                }}"##,
                id
            ),
        )
        .unwrap();
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    }

    #[test]
    fn parses_and_validates_manifests() {
        let manifest = parse_manifest(
            r##"{
                "id": "acme.git-status",
                "version": "1.0.0",
                "api_version": 1,
                "permissions": ["notify"],
                "events": ["command_finished"],
                "command": ["./run.sh"]
            }"##,
        )
        .unwrap();
        assert_eq!(manifest.name, "acme.git-status");
        assert!(manifest.enabled);
        assert!(manifest.has_permission("notify"));

        assert!(parse_manifest("{}").is_err());
        assert!(
            parse_manifest(r#"{"id":"bad/id","version":"1","api_version":1,"command":["x"]}"#)
                .is_err()
        );
        assert!(
            parse_manifest(r#"{"id":"a","version":"1","api_version":2,"command":["x"]}"#).is_err()
        );
        assert!(
            parse_manifest(r#"{"id":"a","version":"1","api_version":1,"command":[]}"#).is_err()
        );
        assert!(parse_manifest(
            r#"{"id":"a","version":"1","api_version":1,"permissions":["root"],
                "command":["x"]}"#
        )
        .is_err());
        assert!(parse_manifest(
            r#"{"id":"a","version":"1","api_version":1,"events":["nope"],
                "command":["x"]}"#
        )
        .is_err());
    }

    #[test]
    fn installs_lists_and_removes_a_local_plugin() {
        let root = temp_root("local");
        let source = root.join("source");
        write_plugin(&source, "acme.test");
        let store = Store::new(root.join("data"));
        let installed = store.install(source.to_str().unwrap(), false).unwrap();
        assert_eq!(installed.manifest.id, "acme.test");
        assert!(store.plugin_dir("acme.test").join("run.sh").is_file());
        assert!(!store.plugin_dir("acme.test").join(".git").exists());

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].manifest.version, "1.2.3");

        assert!(store.install(source.to_str().unwrap(), false).is_err());
        assert!(store.install(source.to_str().unwrap(), true).is_ok());

        let messages = store.update(Some("acme.test")).unwrap();
        assert!(messages[0].contains("local directory"));

        std::fs::create_dir_all(store.state_dir("acme.test")).unwrap();
        store.remove("acme.test", true).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(!store.state_dir("acme.test").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_round_trips() {
        let root = temp_root("lock");
        let store = Store::new(root.clone());
        let mut lock = Lock::default();
        lock.upsert(LockEntry {
            id: "a".to_string(),
            version: "1".to_string(),
            source: "https://example.com/a.git".to_string(),
            commit: Some("abc".to_string()),
        });
        store.save_lock(&lock).unwrap();
        assert_eq!(store.load_lock().unwrap(), lock);
        let _ = std::fs::remove_dir_all(&root);
    }
}
