//! Note shortcuts typed at the shell prompt.
//!
//! A line starting with `:` is a note, not a shell command: `:text` appends to
//! the default notes file and `::file.md text` appends to that file (creating
//! it when needed). Anything else is left to the shell.

use serde_json::Value;

/// A parsed note line, with the text borrowed from the line it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLine<'a> {
    /// Append to the default note (`notes.md` or the configured `notes_file`).
    Default(&'a str),
    /// Append to a named note, relative to the notes directory.
    Named { file: &'a str, text: &'a str },
}

impl NoteLine<'_> {
    /// Return the note text and, when present, the target filename.
    pub fn into_parts(self) -> (String, Option<String>) {
        match self {
            Self::Default(text) => (text.to_string(), None),
            Self::Named { file, text } => (text.to_string(), Some(file.to_string())),
        }
    }
}

/// Parse a line typed at the prompt.
///
/// - `:hello` appends "hello" to the default note.
/// - `::project.md hello` appends "hello" to `project.md`.
/// - `:` and `::project.md` have no text, so they are left to the shell.
/// - Other lines are ordinary commands and are left to the shell.
pub fn parse(line: &str) -> Option<NoteLine<'_>> {
    let rest = line.trim().strip_prefix(':')?;
    let (file, text) = if let Some(named) = rest.strip_prefix(':') {
        let named = named.trim_start();
        let split = named.find(char::is_whitespace).unwrap_or(named.len());
        (&named[..split], named[split..].trim())
    } else {
        ("", rest.trim())
    };
    if text.is_empty() {
        return None;
    }
    if file.is_empty() {
        Some(NoteLine::Default(text))
    } else {
        Some(NoteLine::Named { file, text })
    }
}

/// Whether `:` lines should go to notes instead of the shell.
pub fn enabled(settings: &Value) -> bool {
    settings
        .get("note_prefix_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}
