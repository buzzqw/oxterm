//! Reading, merging and validating the settings document.
//!
//! This is the only place that knows how a `settings.json` becomes an effective
//! configuration, so the GUI, `--show-config` and `--validate-config` can never
//! disagree about what a key means or which values are accepted.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

use super::spec;

/// A settings file is tiny; refusing huge files keeps a mistaken `--config`
/// from reading a whole disk image into memory.
pub const MAX_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub key: String,
    pub message: String,
    /// Unknown keys are warnings (they may belong to a newer Oxterm); invalid
    /// values are errors because they would silently change behaviour.
    pub fatal: bool,
}

#[derive(Debug)]
pub struct Loaded {
    pub settings: Value,
    pub issues: Vec<Issue>,
    /// The file contains values that were rejected or dropped; rewriting it
    /// keeps it in sync with what is actually in use.
    pub rewrite: bool,
}

/// Merge a parsed settings document over the defaults.
///
/// Unknown keys are reported but ignored, invalid known keys are reported and
/// replaced by their default.
pub fn load_document(raw: &str) -> Result<Loaded, String> {
    if raw.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(format!(
            "the settings file is larger than {} MiB",
            MAX_SETTINGS_BYTES / (1024 * 1024)
        ));
    }
    let parsed: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {error}"))?;
    let Value::Object(stored) = parsed else {
        return Err("the settings file must contain a JSON object".to_string());
    };
    Ok(merge(stored))
}

pub fn merge(stored: Map<String, Value>) -> Loaded {
    let mut settings = spec::defaults();
    let effective = settings.as_object_mut().expect("defaults are an object");
    let mut issues = Vec::new();
    let mut rewrite = false;
    for (key, value) in stored {
        match spec::find(&key) {
            Some(setting) => match spec::validate(setting, &value) {
                Ok(()) => {
                    effective.insert(key, value);
                }
                Err(message) => {
                    issues.push(Issue {
                        key: key.clone(),
                        message,
                        fatal: true,
                    });
                    rewrite = true;
                }
            },
            None => issues.push(Issue {
                key,
                message: "unknown setting; it is ignored".to_string(),
                fatal: false,
            }),
        }
    }
    Loaded {
        settings,
        issues,
        rewrite,
    }
}

/// Read a settings file without letting it grow past [`MAX_SETTINGS_BYTES`].
pub fn read_document(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut buffer = String::new();
    file.by_ref()
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_string(&mut buffer)
        .map_err(|error| error.to_string())?;
    if buffer.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(format!(
            "the settings file is larger than {} MiB",
            MAX_SETTINGS_BYTES / (1024 * 1024)
        ));
    }
    Ok(buffer)
}

pub fn load_file(path: &Path) -> Result<Loaded, String> {
    load_document(&read_document(path)?)
}

/// Apply `-o KEY=VALUE` (and profile) overrides to an effective document.
///
/// Unknown keys are accepted, matching the running application: `-o` is also
/// used to set keys that only a newer build knows about. Invalid values for
/// known keys are reported and left out.
pub fn apply_overrides(settings: &mut Value, overrides: &[(String, Value)]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let effective = settings
        .as_object_mut()
        .expect("effective settings are an object");
    for (key, value) in overrides {
        match spec::find(key) {
            Some(setting) => match spec::validate(setting, value) {
                Ok(()) => {
                    effective.insert(key.clone(), value.clone());
                }
                Err(message) => issues.push(Issue {
                    key: key.clone(),
                    message,
                    fatal: true,
                }),
            },
            None => {
                effective.insert(key.clone(), value.clone());
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_valid_values_and_keeps_defaults_for_the_rest() {
        let loaded = load_document(r#"{"font_size": 18, "opacity": 0.8}"#).unwrap();
        let settings = loaded.settings.as_object().unwrap();
        assert_eq!(settings.get("font_size"), Some(&json!(18)));
        assert_eq!(settings.get("opacity"), Some(&json!(0.8)));
        assert_eq!(settings.get("terminal_rows"), Some(&json!(24)));
        assert!(loaded.issues.is_empty());
        assert!(!loaded.rewrite);
    }

    #[test]
    fn reports_unknown_and_invalid_keys() {
        let loaded = load_document(r#"{"font_sise": 18, "font_size": 900}"#).unwrap();
        assert!(loaded.rewrite);
        assert_eq!(loaded.issues.len(), 2);
        let unknown = loaded.issues.iter().find(|i| i.key == "font_sise").unwrap();
        assert!(!unknown.fatal);
        let invalid = loaded.issues.iter().find(|i| i.key == "font_size").unwrap();
        assert!(invalid.fatal);
        assert!(invalid.message.contains("between"));
        assert_eq!(
            loaded.settings.as_object().unwrap().get("font_size"),
            Some(&json!(12))
        );
    }

    #[test]
    fn rejects_broken_documents_without_panicking() {
        assert!(load_document("{").unwrap_err().contains("invalid JSON"));
        assert!(load_document("[]").unwrap_err().contains("JSON object"));
        assert!(load_document("null").is_err());
        assert!(load_document(&" ".repeat(MAX_SETTINGS_BYTES as usize + 1)).is_err());
    }

    #[test]
    fn overrides_are_applied_and_validated() {
        let mut settings = spec::defaults();
        let issues = apply_overrides(
            &mut settings,
            &[
                ("font_size".to_string(), json!(20)),
                ("font_size".to_string(), json!(0)),
                ("future_key".to_string(), json!(true)),
            ],
        );
        assert_eq!(settings["font_size"], json!(20));
        assert_eq!(settings["future_key"], json!(true));
        assert_eq!(issues.len(), 1);
        assert!(issues[0].fatal);
    }

    #[test]
    fn reads_files_up_to_the_limit() {
        let path = std::env::temp_dir().join(format!(
            "oxterm-settings-load-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, r#"{"font_size": 20}"#).unwrap();
        let loaded = load_file(&path).unwrap();
        assert_eq!(loaded.settings["font_size"], json!(20));
        std::fs::write(&path, vec![b'x'; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        assert!(load_file(&path).unwrap_err().contains("larger than"));
        std::fs::remove_file(&path).unwrap();
    }
}
