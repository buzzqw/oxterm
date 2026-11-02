//! User-defined triggers on terminal output.
//!
//! A trigger is a regular expression plus one or more actions. The file lives
//! next to `settings.json`:
//!
//! ```json
//! {
//!   "version": 1,
//!   "triggers": [
//!     {
//!       "id": "errors",
//!       "match": "(?i)\\berror\\b",
//!       "actions": [{ "type": "highlight_line", "fg": "#ff5555" }]
//!     }
//!   ]
//! }
//! ```
//!
//! Parsing, validation and matching live here so they can be tested without a
//! display; `src/terminal.rs` only extracts text from VTE and applies the
//! returned [`Effect`]s.

use std::path::Path;

use regex::{Captures, Regex};
use serde_json::Value;

/// Largest accepted `triggers.json`. The file is a few rules, not a database.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Accepted trigger file version. A newer file is refused, not half-read.
pub const CURRENT_VERSION: u64 = 1;

/// Bound on the number of rules, so a hostile file cannot make every line of
/// output run thousands of regular expressions.
pub const MAX_TRIGGERS: usize = 256;

/// Bound on a single pattern.
pub const MAX_PATTERN: usize = 4096;

/// Bound on the text produced by `{n}` expansion (a notification body, a tab
/// title, a captured line).
pub const MAX_EXPANSION: usize = 4096;

/// Where a trigger applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every line of output (the default).
    Line,
    /// Only lines produced while an OSC 133 command is running.
    CommandOutput,
}

/// One parsed action, with templates still unexpanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    HighlightLine {
        fg: Option<String>,
        bg: Option<String>,
    },
    HighlightText {
        fg: Option<String>,
        bg: Option<String>,
    },
    Notify {
        title: String,
        body: String,
    },
    Capture {
        list: String,
        label: String,
        open: Option<String>,
    },
    SetTabColor {
        color: String,
    },
    SetTabTitle {
        title: String,
    },
    Hyperlink {
        url: String,
    },
    MarkFailure,
}

/// One action after `{n}` expansion: what the GUI should actually do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    HighlightLine {
        fg: Option<String>,
        bg: Option<String>,
    },
    HighlightText {
        fg: Option<String>,
        bg: Option<String>,
    },
    Notify {
        title: String,
        body: String,
    },
    Capture {
        list: String,
        label: String,
        open: Option<String>,
    },
    SetTabColor {
        color: String,
    },
    SetTabTitle {
        title: String,
    },
    Hyperlink {
        url: String,
    },
    MarkFailure,
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub id: String,
    pub enabled: bool,
    pub pattern: Regex,
    pub actions: Vec<Action>,
    pub profiles: Vec<String>,
    pub scope: Scope,
    /// Fire at most once per terminal session.
    pub once: bool,
}

/// A rule that matched one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerHit {
    pub id: String,
    pub once: bool,
    pub effects: Vec<Effect>,
}

/// Everything a trigger can depend on besides the line itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    pub profile: Option<&'a str>,
    pub in_command_output: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TriggerSet {
    triggers: Vec<Trigger>,
}

impl TriggerSet {
    pub fn len(&self) -> usize {
        self.triggers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triggers.is_empty()
    }

    /// Read a trigger file. A missing file is an empty set, not an error.
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
        let document: Value =
            serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {}", error))?;
        let object = document
            .as_object()
            .ok_or("the trigger file must contain a JSON object")?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("the trigger file has no \"version\" (expected 1)")?;
        if version > CURRENT_VERSION {
            return Err(format!(
                "the trigger file was written by a newer Oxterm (version {}; this build reads {})",
                version, CURRENT_VERSION
            ));
        }
        let list = object
            .get("triggers")
            .and_then(Value::as_array)
            .ok_or("the trigger file has no \"triggers\" array")?;
        if list.len() > MAX_TRIGGERS {
            return Err(format!(
                "too many triggers: {} (limit {})",
                list.len(),
                MAX_TRIGGERS
            ));
        }
        let mut triggers = Vec::with_capacity(list.len());
        for (index, value) in list.iter().enumerate() {
            let trigger = parse_trigger(value, index)?;
            if triggers
                .iter()
                .any(|other: &Trigger| other.id == trigger.id)
            {
                return Err(format!("duplicate trigger id: {}", trigger.id));
            }
            triggers.push(trigger);
        }
        Ok(Self { triggers })
    }

    /// Match one line and resolve every action of every matching rule.
    ///
    /// Rules are evaluated in file order, so a specific rule can come before a
    /// broad one. A disabled rule or one that does not apply to the current
    /// profile or scope is skipped.
    pub fn evaluate(&self, line: &str, context: Context<'_>) -> Vec<TriggerHit> {
        if self.triggers.is_empty() || line.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for trigger in &self.triggers {
            if !trigger.enabled {
                continue;
            }
            if trigger.scope == Scope::CommandOutput && !context.in_command_output {
                continue;
            }
            if !trigger.profiles.is_empty()
                && !context
                    .profile
                    .is_some_and(|profile| trigger.profiles.iter().any(|entry| entry == profile))
            {
                continue;
            }
            let Some(captures) = trigger.pattern.captures(line) else {
                continue;
            };
            let effects = trigger
                .actions
                .iter()
                .map(|action| resolve(action, &captures))
                .collect();
            hits.push(TriggerHit {
                id: trigger.id.clone(),
                once: trigger.once,
                effects,
            });
        }
        hits
    }
}

fn parse_trigger(value: &Value, index: usize) -> Result<Trigger, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("trigger {} is not an object", index + 1))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("trigger-{}", index + 1));
    if id.len() > 128 {
        return Err(format!("trigger id is too long: {}", id));
    }
    let pattern_text = object
        .get("match")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("trigger {} has no \"match\"", id))?;
    if pattern_text.is_empty() {
        return Err(format!("trigger {} has an empty \"match\"", id));
    }
    if pattern_text.len() > MAX_PATTERN {
        return Err(format!(
            "trigger {} has a pattern longer than {} bytes",
            id, MAX_PATTERN
        ));
    }
    let pattern = Regex::new(pattern_text)
        .map_err(|error| format!("trigger {} has an invalid regex: {}", id, error))?;
    let list = object
        .get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("trigger {} has no \"actions\" array", id))?;
    if list.is_empty() {
        return Err(format!("trigger {} has no actions", id));
    }
    let mut actions = Vec::with_capacity(list.len());
    for action in list {
        actions.push(parse_action(action, &id)?);
    }
    let profiles = object
        .get("profiles")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("trigger {}: \"profiles\" must be an array", id))?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("trigger {}: profiles must be strings", id))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let scope = match object
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("line")
    {
        "line" => Scope::Line,
        "command_output" => Scope::CommandOutput,
        other => {
            return Err(format!(
                "trigger {}: unknown scope '{}' (use line or command_output)",
                id, other
            ))
        }
    };
    Ok(Trigger {
        id,
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        pattern,
        actions,
        profiles,
        scope,
        once: object.get("once").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn parse_action(value: &Value, trigger: &str) -> Result<Action, String> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("trigger {}: action without a \"type\"", trigger))?;
    let color = |key: &str| -> Result<Option<String>, String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(raw)) if raw.is_empty() => Ok(None),
            Some(Value::String(raw)) => parse_color(raw)
                .map(Some)
                .map_err(|error| format!("trigger {}: {}: {}", trigger, key, error)),
            Some(_) => Err(format!("trigger {}: {} must be a string", trigger, key)),
        }
    };
    let text = |key: &str| -> Result<String, String> {
        match value.get(key) {
            Some(Value::String(raw)) => Ok(raw.clone()),
            None | Some(Value::Null) => Ok(String::new()),
            Some(_) => Err(format!("trigger {}: {} must be a string", trigger, key)),
        }
    };
    Ok(match kind {
        "highlight_line" => Action::HighlightLine {
            fg: color("fg")?,
            bg: color("bg")?,
        },
        "highlight_text" => Action::HighlightText {
            fg: color("fg")?,
            bg: color("bg")?,
        },
        "notify" => Action::Notify {
            title: text("title")?,
            body: text("body")?,
        },
        "capture" => Action::Capture {
            list: text("list")?,
            label: text("label")?,
            open: value
                .get("open")
                .and_then(Value::as_str)
                .filter(|open| !open.is_empty())
                .map(str::to_string),
        },
        "set_tab_color" => Action::SetTabColor {
            color: color("color")?
                .ok_or_else(|| format!("trigger {}: set_tab_color needs \"color\"", trigger))?,
        },
        "set_tab_title" => Action::SetTabTitle {
            title: text("title")?,
        },
        "hyperlink" => Action::Hyperlink { url: text("url")? },
        "mark_failure" => Action::MarkFailure,
        other => {
            return Err(format!(
                "trigger {}: unknown action type '{}' (execute and inject are not supported)",
                trigger, other
            ))
        }
    })
}

/// Accept `#rgb` and `#rrggbb`, lower-cased. Markup colours must not carry
/// quotes or other syntax, so only hex digits are allowed.
pub fn parse_color(raw: &str) -> Result<String, String> {
    let hex = raw
        .strip_prefix('#')
        .ok_or_else(|| format!("'{}' is not a #rrggbb colour", raw))?;
    let expanded = match hex.len() {
        3 if hex.chars().all(|c| c.is_ascii_hexdigit()) => {
            hex.chars().flat_map(|c| [c, c]).collect::<String>()
        }
        6 if hex.chars().all(|c| c.is_ascii_hexdigit()) => hex.to_string(),
        _ => return Err(format!("'{}' is not a #rrggbb colour", raw)),
    };
    Ok(format!("#{}", expanded.to_ascii_lowercase()))
}

fn resolve(action: &Action, captures: &Captures<'_>) -> Effect {
    match action {
        Action::HighlightLine { fg, bg } => Effect::HighlightLine {
            fg: fg.clone(),
            bg: bg.clone(),
        },
        Action::HighlightText { fg, bg } => Effect::HighlightText {
            fg: fg.clone(),
            bg: bg.clone(),
        },
        Action::Notify { title, body } => Effect::Notify {
            title: expand(title, captures),
            body: expand(body, captures),
        },
        Action::Capture { list, label, open } => Effect::Capture {
            list: list.clone(),
            label: expand(label, captures),
            open: open.clone(),
        },
        Action::SetTabColor { color } => Effect::SetTabColor {
            color: color.clone(),
        },
        Action::SetTabTitle { title } => Effect::SetTabTitle {
            title: expand(title, captures),
        },
        Action::Hyperlink { url } => Effect::Hyperlink {
            url: expand(url, captures),
        },
        Action::MarkFailure => Effect::MarkFailure,
    }
}

/// Replace `{0}`..`{9}` with capture groups, `{{` and `}}` with literal
/// braces, and leave everything else alone. The result is capped at
/// [`MAX_EXPANSION`] characters.
pub fn expand(template: &str, captures: &Captures<'_>) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if out.chars().count() >= MAX_EXPANSION {
            break;
        }
        let current = chars[index];
        if current == '{' && index + 1 < chars.len() && chars[index + 1] == '{' {
            out.push('{');
            index += 2;
            continue;
        }
        if current == '}' && index + 1 < chars.len() && chars[index + 1] == '}' {
            out.push('}');
            index += 2;
            continue;
        }
        if current == '{' {
            let mut end = index + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end > index + 1 && end < chars.len() && chars[end] == '}' {
                let digits: String = chars[index + 1..end].iter().collect();
                if let Ok(group) = digits.parse::<usize>() {
                    if let Some(found) = captures.get(group) {
                        for ch in found.as_str().chars() {
                            if out.chars().count() >= MAX_EXPANSION {
                                break;
                            }
                            out.push(ch);
                        }
                    }
                }
                index = end + 1;
                continue;
            }
        }
        out.push(current);
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(raw_trigger: &str) -> TriggerSet {
        let document = format!("{{\"version\":1,\"triggers\":[{}]}}", raw_trigger);
        TriggerSet::parse(&document).unwrap()
    }

    #[test]
    fn parses_defaults_and_expands_capture_groups() {
        let triggers = one(r#"{"match":"^([^:]+):(\\d+): (error|warning): (.*)$",
                "actions":[{"type":"capture","list":"errors","label":"{1}:{2} {4}"},
                           {"type":"notify","title":"Build problem","body":"{3}: {4}"}]}"#);
        let hits = triggers.evaluate("src/main.rs:42: error: missing value", Context::default());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "trigger-1");
        assert_eq!(
            hits[0].effects[0],
            Effect::Capture {
                list: "errors".to_string(),
                label: "src/main.rs:42 missing value".to_string(),
                open: None,
            }
        );
        assert_eq!(
            hits[0].effects[1],
            Effect::Notify {
                title: "Build problem".to_string(),
                body: "error: missing value".to_string(),
            }
        );
    }

    #[test]
    fn disabled_and_scope_and_profile_rules_are_skipped() {
        let triggers = one(r#"{"enabled":false,"match":"x","actions":[{"type":"mark_failure"}]}"#);
        assert!(triggers.evaluate("x", Context::default()).is_empty());

        let triggers =
            one(r#"{"scope":"command_output","match":"x","actions":[{"type":"mark_failure"}]}"#);
        assert!(triggers
            .evaluate(
                "x",
                Context {
                    in_command_output: false,
                    ..Context::default()
                }
            )
            .is_empty());
        assert_eq!(
            triggers
                .evaluate(
                    "x",
                    Context {
                        in_command_output: true,
                        ..Context::default()
                    }
                )
                .len(),
            1
        );

        let triggers =
            one(r#"{"profiles":["work"],"match":"x","actions":[{"type":"mark_failure"}]}"#);
        assert!(triggers
            .evaluate(
                "x",
                Context {
                    profile: Some("home"),
                    ..Context::default()
                }
            )
            .is_empty());
        assert_eq!(
            triggers
                .evaluate(
                    "x",
                    Context {
                        profile: Some("work"),
                        ..Context::default()
                    }
                )
                .len(),
            1
        );
    }

    #[test]
    fn rejects_invalid_documents() {
        assert!(TriggerSet::parse("{}").is_err());
        assert!(TriggerSet::parse(r#"{"triggers":{}}"#).is_err());
        assert!(TriggerSet::parse(r#"{"version":99,"triggers":[]}"#).is_err());
        assert!(TriggerSet::parse(
            r#"{"triggers":[{"match":"(","actions":[{"type":"mark_failure"}]}]}"#
        )
        .is_err());
        assert!(TriggerSet::parse(
            r#"{"triggers":[{"match":"x","actions":[{"type":"execute"}]}]}"#
        )
        .is_err());
        assert!(TriggerSet::parse(r#"{"triggers":[{"match":"x","actions":[]}]}"#).is_err());
        assert!(TriggerSet::parse(
            r#"{"triggers":[{"id":"same","match":"x","actions":[{"type":"mark_failure"}]},
                            {"id":"same","match":"y","actions":[{"type":"mark_failure"}]}]}"#
        )
        .is_err());
    }

    #[test]
    fn validates_colours() {
        assert_eq!(parse_color("#abc").unwrap(), "#aabbcc");
        assert_eq!(parse_color("#AABBCC").unwrap(), "#aabbcc");
        assert!(parse_color("red").is_err());
        assert!(parse_color("#gggggg").is_err());
        assert!(TriggerSet::parse(
            r#"{"triggers":[{"match":"x","actions":[{"type":"set_tab_color","color":"red"}]}]}"#
        )
        .is_err());
    }

    #[test]
    fn expand_handles_braces_and_missing_groups() {
        let regex = Regex::new(r"^(a)(b)?(c)$").unwrap();
        let captures = regex.captures("ac").unwrap();
        assert_eq!(expand("{0}-{1}-{2}-{3}", &captures), "ac-a--c");
        assert_eq!(expand("{{x}} {1}", &captures), "{x} a");
        assert_eq!(expand("{", &captures), "{");
        assert_eq!(expand("{99}", &captures), "");
    }

    #[test]
    fn missing_file_is_an_empty_set() {
        let path = std::env::temp_dir().join("oxterm-no-such-triggers.json");
        let _ = std::fs::remove_file(&path);
        assert!(TriggerSet::load(&path).unwrap().is_empty());
    }
}
