//! The single source of truth for Oxterm's command line and settings.
//!
//! `--help`, the man page, the shell completions, `--show-config`,
//! `--validate-config` and `docs/settings.md` are all generated from the tables
//! in this module, so a new option or setting cannot be documented in one place
//! and forgotten in another. Regenerate the committed artefacts with
//! `cargo run --bin oxterm-docgen`; CI fails when they are out of date.

use serde_json::{Map, Value};

// ── Command line ─────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CliGroup {
    Window,
    Session,
    Inspect,
    Utility,
}

/// How a shell completion should complete the value of an option.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    None,
    Dir,
    File,
    Text,
    Number,
    Command,
    Session,
    Font,
    Profile,
    KeyValue,
}

pub struct CliOption {
    pub short: Option<char>,
    pub long: &'static str,
    /// Metavariable shown in help, `None` for flags.
    pub value: Option<&'static str>,
    pub description: &'static str,
    pub group: CliGroup,
    pub kind: ValueKind,
}

impl CliOption {
    /// `-w, --working-directory DIR`, `--hold` and so on.
    pub fn names(&self) -> String {
        let mut out = String::new();
        match self.short {
            Some(short) => {
                out.push('-');
                out.push(short);
                out.push_str(", --");
            }
            None => out.push_str("    --"),
        }
        out.push_str(self.long);
        if let Some(value) = self.value {
            out.push(' ');
            out.push_str(value);
        }
        out
    }
}

const fn opt(
    short: Option<char>,
    long: &'static str,
    value: Option<&'static str>,
    description: &'static str,
    group: CliGroup,
    kind: ValueKind,
) -> CliOption {
    CliOption {
        short,
        long,
        value,
        description,
        group,
        kind,
    }
}

pub const CLI: &[CliOption] = &[
    // Window and terminal options.
    opt(
        Some('w'),
        "working-directory",
        Some("DIR"),
        "Working directory for the new terminal",
        CliGroup::Window,
        ValueKind::Dir,
    ),
    opt(
        None,
        "new-window",
        None,
        "Open a new window instead of reusing a running instance",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        None,
        "no-restore",
        None,
        "Do not restore the last session",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        Some('T'),
        "title",
        Some("TITLE"),
        "Set a fixed window title (apps cannot override it)",
        CliGroup::Window,
        ValueKind::Text,
    ),
    opt(
        Some('g'),
        "geometry",
        Some("COLSxROWS"),
        "Initial size in character cells (e.g. 120x40)",
        CliGroup::Window,
        ValueKind::Text,
    ),
    opt(
        Some('F'),
        "fullscreen",
        None,
        "Start in fullscreen mode",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        Some('m'),
        "maximize",
        None,
        "Start maximized",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        None,
        "class",
        Some("CLASS"),
        "Set the WM_CLASS class part (window manager rules)",
        CliGroup::Window,
        ValueKind::Text,
    ),
    opt(
        None,
        "name",
        Some("NAME"),
        "Set the WM_CLASS instance name",
        CliGroup::Window,
        ValueKind::Text,
    ),
    opt(
        Some('p'),
        "profile",
        Some("NAME"),
        "Start with a saved profile (session only)",
        CliGroup::Window,
        ValueKind::Profile,
    ),
    opt(
        None,
        "config",
        Some("FILE"),
        "Use an alternative settings file",
        CliGroup::Window,
        ValueKind::File,
    ),
    opt(
        Some('o'),
        "option",
        Some("KEY=VALUE"),
        "Override a setting for this session (repeatable)",
        CliGroup::Window,
        ValueKind::KeyValue,
    ),
    opt(
        None,
        "font",
        Some("FAMILY"),
        "Override the font family for this session",
        CliGroup::Window,
        ValueKind::Font,
    ),
    opt(
        None,
        "font-size",
        Some("N"),
        "Override the font size for this session",
        CliGroup::Window,
        ValueKind::Number,
    ),
    opt(
        None,
        "hold",
        None,
        "Keep the terminal open after the command exits",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        Some('e'),
        "execute",
        Some("CMD..."),
        "Run a command instead of the configured shell",
        CliGroup::Window,
        ValueKind::Command,
    ),
    // Remote sessions and live queries.
    opt(
        None,
        "list",
        None,
        "List active Oxterm terminals (first column is the session ID)",
        CliGroup::Session,
        ValueKind::None,
    ),
    opt(
        None,
        "info",
        Some("SESSION_ID"),
        "Show one active terminal",
        CliGroup::Session,
        ValueKind::Session,
    ),
    opt(
        Some('a'),
        "attach",
        Some("[SESSION_ID]"),
        "Share a live session in this terminal",
        CliGroup::Session,
        ValueKind::Session,
    ),
    opt(
        Some('M'),
        "move",
        Some("[SESSION_ID]"),
        "Take a live session over in this terminal",
        CliGroup::Session,
        ValueKind::Session,
    ),
    opt(
        None,
        "detach",
        Some("SESSION_ID"),
        "Disconnect every remote client of a session",
        CliGroup::Session,
        ValueKind::Session,
    ),
    opt(
        None,
        "broker",
        Some("SOCKET ID"),
        "Run the persistent PTY broker (internal)",
        CliGroup::Session,
        ValueKind::File,
    ),
    // Introspection: these never open a window.
    opt(
        None,
        "show-config",
        None,
        "Print the effective settings as JSON and exit",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "default",
        None,
        "With --show-config, print the built-in defaults",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "validate-config",
        Some("[FILE]"),
        "Validate a settings file and exit",
        CliGroup::Inspect,
        ValueKind::File,
    ),
    opt(
        None,
        "list-settings",
        None,
        "List every setting with its type and default",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "list-themes",
        None,
        "List the built-in color schemes",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "list-keybinds",
        None,
        "List the keyboard shortcuts",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "list-actions",
        None,
        "List the scriptable control actions",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "diagnostics",
        None,
        "Print environment, paths and limits for bug reports",
        CliGroup::Inspect,
        ValueKind::None,
    ),
    opt(
        None,
        "edit-config",
        None,
        "Open the settings file in the configured editor",
        CliGroup::Utility,
        ValueKind::None,
    ),
    opt(
        None,
        "list-fonts",
        None,
        "List installed font families for font_name",
        CliGroup::Utility,
        ValueKind::None,
    ),
    opt(
        Some('V'),
        "version",
        None,
        "Show the Oxterm version",
        CliGroup::Window,
        ValueKind::None,
    ),
    opt(
        Some('h'),
        "help",
        None,
        "Show this help",
        CliGroup::Window,
        ValueKind::None,
    ),
];

/// `--help` text. Option rows come from [`CLI`]; prose that is not a list of
/// options stays here.
pub fn usage() -> String {
    let mut out = String::from("Usage: oxterm [DIRECTORY] [OPTIONS] [-e CMD...]\n\n");
    // One width for every group keeps the columns stable when options move
    // between sections.
    let width = CLI
        .iter()
        .map(|option| option.names().len())
        .max()
        .unwrap_or(0);
    for (group, title) in [
        (CliGroup::Window, "Options"),
        (CliGroup::Session, "Remote sessions"),
        (CliGroup::Inspect, "Inspection (no window is opened)"),
        (CliGroup::Utility, "Utilities (no window is opened)"),
    ] {
        out.push_str(title);
        out.push_str(":\n");
        let rows: Vec<(String, &str)> = CLI
            .iter()
            .filter(|option| option.group == group)
            .map(|option| (option.names(), option.description))
            .collect();
        for (names, description) in rows {
            out.push_str(&format!("  {:<width$}  {}\n", names, description));
        }
        if group == CliGroup::Window {
            out.push_str(&format!(
                "  {:<width$}  {}\n",
                "    --", "Treat the next argument as the directory"
            ));
        }
        out.push('\n');
    }
    out.push_str("Scriptable control (acts on a running Oxterm, opens no window):\n");
    out.push_str(&super::control_cli::summary());
    out.push('\n');
    out.push_str(
        "  These headless commands act on a running Oxterm terminal on the same\n\
         \x20 host and open no window, so they are safe to use over SSH. Without a\n\
         \x20 SESSION_ID, -a and -M use the only session or show a list to choose from.\n\
         \n\
         \x20 attach  Adds this terminal as another view of the session, repainting\n\
         \x20         its current screen. The shell keeps running and its original\n\
         \x20         window stays live; every view shares the same input and output.\n\
         \x20 move    Like attach but takes control: it disconnects every other remote\n\
         \x20         client and switches off the original window's view, so the\n\
         \x20         session is shown only here. In that window, Ctrl+B then s or w\n\
         \x20         brings the view back.\n\
         \x20 detach  Releases every remote client (attach or move) without stopping\n\
         \x20         the shell, leaving the original window as the only view again.\n",
    );
    out
}

// ── Settings ─────────────────────────────────────────────────

/// Default value of a setting. Environment-dependent defaults are resolved when
/// [`defaults`] is called, not at compile time.
#[derive(Clone, Copy)]
pub enum Default {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(&'static str),
    Env(&'static str, &'static str),
    Null,
    Map,
}

impl Default {
    pub fn value(self) -> Value {
        match self {
            Default::Bool(value) => Value::Bool(value),
            Default::Int(value) => Value::from(value),
            Default::Float(value) => Value::from(value),
            Default::Text(value) => Value::String(value.to_string()),
            Default::Env(key, fallback) => {
                Value::String(std::env::var(key).unwrap_or_else(|_| fallback.to_string()))
            }
            Default::Null => Value::Null,
            Default::Map => Value::Object(Map::new()),
        }
    }

    /// The value `settings_markdown` prints.
    ///
    /// Environment-derived defaults are rendered as their documented fallback
    /// so the generated reference does not depend on the machine or the CI
    /// runner that produced it.
    pub fn documented_value(self) -> Value {
        match self {
            Default::Env(_, fallback) => Value::String(fallback.to_string()),
            other => other.value(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    UnitFloat { min: f64, max: f64 },
    Text,
    HexColor,
    HexColorOrEmpty,
    Letter,
    Palette,
    StringMap,
}

impl Kind {
    pub fn type_name(&self) -> String {
        match self {
            Kind::Bool => "bool".to_string(),
            Kind::Int { min, max } => format!("int ({min}..{max})"),
            Kind::UnitFloat { min, max } => format!("float ({min}..{max})"),
            Kind::Text => "string".to_string(),
            Kind::HexColor => "color (#rrggbb)".to_string(),
            Kind::HexColorOrEmpty => "color or empty".to_string(),
            Kind::Letter => "letter".to_string(),
            Kind::Palette => "palette or null".to_string(),
            Kind::StringMap => "string map".to_string(),
        }
    }
}

pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub default: Default,
    pub description: &'static str,
    /// Allowed values for enumerations. Documentation only: the settings file
    /// keeps accepting any string so an older or newer config never breaks.
    pub choices: &'static [&'static str],
}

impl Setting {
    pub fn default_value(&self) -> Value {
        self.default.value()
    }
}

const fn setting(
    key: &'static str,
    kind: Kind,
    default: Default,
    description: &'static str,
) -> Setting {
    Setting {
        key,
        kind,
        default,
        description,
        choices: &[],
    }
}

pub const SETTINGS: &[Setting] = &[
    setting(
        "font_name",
        Kind::Text,
        Default::Text("Monospace"),
        "Font family used by new terminals.",
    ),
    setting(
        "font_size",
        Kind::Int { min: 4, max: 128 },
        Default::Int(12),
        "Font size in points.",
    ),
    Setting {
        key: "font_rendering",
        kind: Kind::Text,
        default: Default::Text("system"),
        description: "Text antialiasing: system, grayscale, subpixel or none (needs VTE 0.74+).",
        choices: &["system", "grayscale", "subpixel", "none"],
    },
    Setting {
        key: "font_hinting",
        kind: Kind::Text,
        default: Default::Text("system"),
        description:
            "Font hinting strength: system, none, slight, medium or full (needs VTE 0.74+).",
        choices: &["system", "none", "slight", "medium", "full"],
    },
    setting(
        "cell_height_scale",
        Kind::UnitFloat { min: 0.8, max: 2.0 },
        Default::Float(1.0),
        "Vertical cell size multiplier; above 1.0 adds air between the lines.",
    ),
    setting(
        "cell_width_scale",
        Kind::UnitFloat { min: 0.8, max: 2.0 },
        Default::Float(1.0),
        "Horizontal cell size multiplier; above 1.0 adds letter spacing.",
    ),
    setting(
        "text_shaping",
        Kind::Bool,
        Default::Bool(true),
        "Shape text with HarfBuzz; disabling it is faster for plain ASCII output.",
    ),
    Setting {
        key: "color_scheme",
        kind: Kind::Text,
        default: Default::Text("Dark (Default)"),
        description: "Built-in color scheme; see --list-themes.",
        choices: &[],
    },
    setting(
        "auto_theme",
        Kind::Bool,
        Default::Bool(false),
        "Follow the desktop's light/dark preference, using theme_light or theme_dark.",
    ),
    setting(
        "theme_light",
        Kind::Text,
        Default::Text("Light"),
        "Theme used when the desktop prefers a light appearance.",
    ),
    setting(
        "theme_dark",
        Kind::Text,
        Default::Text("Dark (Default)"),
        "Theme used when the desktop prefers a dark appearance.",
    ),
    setting(
        "foreground_color",
        Kind::HexColorOrEmpty,
        Default::Text(""),
        "Foreground override; empty uses the color scheme.",
    ),
    setting(
        "background_color",
        Kind::HexColorOrEmpty,
        Default::Text(""),
        "Background override; empty uses the color scheme.",
    ),
    setting(
        "cursor_color",
        Kind::HexColor,
        Default::Text("#ffffff"),
        "Cursor color.",
    ),
    Setting {
        key: "cursor_shape",
        kind: Kind::Text,
        default: Default::Text("block"),
        description: "Cursor shape for new terminals.",
        choices: &["block", "underline", "ibeam"],
    },
    setting(
        "highlight_color",
        Kind::HexColor,
        Default::Text("#ffffff"),
        "Foreground of selected text.",
    ),
    setting(
        "highlight_bg_color",
        Kind::HexColor,
        Default::Text("#446688"),
        "Background of selected text.",
    ),
    setting(
        "scrollback_lines",
        Kind::Int {
            min: -1,
            max: 1_000_000,
        },
        Default::Int(10_000),
        "Lines kept above the screen; -1 means unlimited.",
    ),
    Setting {
        key: "scrollbar_position",
        kind: Kind::Text,
        default: Default::Text("right"),
        description: "Where to show the scrollbar.",
        choices: &["right", "left", "disabled"],
    },
    setting(
        "opacity",
        Kind::UnitFloat { min: 0.1, max: 1.0 },
        Default::Float(1.0),
        "Window opacity when transparency is enabled.",
    ),
    setting(
        "shell_command",
        Kind::Text,
        Default::Env("SHELL", "/bin/bash"),
        "Shell started in new terminals.",
    ),
    setting(
        "notes_file",
        Kind::Text,
        Default::Text(""),
        "Markdown file used by the notes commands; empty uses notes_dir.",
    ),
    setting(
        "notes_dir",
        Kind::Text,
        Default::Text(""),
        "Directory for Markdown notes; empty uses the data directory.",
    ),
    setting(
        "notes_record_context",
        Kind::Bool,
        Default::Bool(true),
        "Record the command context next to new notes.",
    ),
    setting(
        "notes_template",
        Kind::Text,
        Default::Text(""),
        "Template prepended to new notes.",
    ),
    setting(
        "editor_command",
        Kind::Text,
        Default::Env("EDITOR", "nano"),
        "Editor used for notes and configuration files.",
    ),
    setting(
        "snippets",
        Kind::StringMap,
        Default::Map,
        "Named command snippets available to /snippet.",
    ),
    setting(
        "osc133",
        Kind::Bool,
        Default::Bool(false),
        "Enable OSC 133 shell integration (prompt, command and exit markers).",
    ),
    setting(
        "enable_transparency",
        Kind::Bool,
        Default::Bool(false),
        "Allow window transparency using the opacity setting.",
    ),
    setting(
        "confirm_close",
        Kind::Bool,
        Default::Bool(true),
        "Ask before closing a tab with a running command.",
    ),
    setting(
        "tab_title",
        Kind::Text,
        Default::Text("Terminal"),
        "Initial title of new tabs.",
    ),
    setting(
        "tab_title_color",
        Kind::HexColor,
        Default::Text("#ffffff"),
        "Color of inactive tab titles.",
    ),
    setting(
        "tab_active_title_color",
        Kind::HexColor,
        Default::Text("#ffffff"),
        "Color of the active tab title.",
    ),
    Setting {
        key: "encoding",
        kind: Kind::Text,
        default: Default::Text("UTF-8"),
        description: "Default character encoding for new terminals.",
        choices: &[
            "UTF-8",
            "ISO-8859-1",
            "ISO-8859-15",
            "UTF-16",
            "UTF-16BE",
            "UTF-16LE",
            "CP1252",
            "CP850",
            "ASCII",
            "KOI8-R",
            "Shift_JIS",
            "EUC-JP",
            "GBK",
        ],
    },
    setting(
        "auto_copy_selection",
        Kind::Bool,
        Default::Bool(true),
        "Copy a selection to the clipboard as soon as it is made.",
    ),
    setting(
        "file_manager",
        Kind::Text,
        Default::Text(""),
        "File manager used to open the notes directory.",
    ),
    Setting {
        key: "backspace_binding",
        kind: Kind::Text,
        default: Default::Text("ascii-del"),
        description: "Bytes sent by Backspace.",
        choices: &["auto", "ascii-del", "escape-sequence", "control-h"],
    },
    Setting {
        key: "delete_binding",
        kind: Kind::Text,
        default: Default::Text("escape-sequence"),
        description: "Bytes sent by Delete.",
        choices: &["auto", "escape-sequence", "ascii-del", "control-h"],
    },
    setting(
        "custom_palette",
        Kind::Palette,
        Default::Null,
        "Per-color overrides for the 16 ANSI colors.",
    ),
    setting(
        "show_unsafe_paste_dialog",
        Kind::Bool,
        Default::Bool(true),
        "Warn before pasting text that contains control characters.",
    ),
    setting(
        "show_tabs",
        Kind::Bool,
        Default::Bool(true),
        "Show the tab bar.",
    ),
    setting(
        "show_menubar",
        Kind::Bool,
        Default::Bool(true),
        "Show the menu bar.",
    ),
    setting(
        "show_toolbar",
        Kind::Bool,
        Default::Bool(true),
        "Show the window toolbar.",
    ),
    setting(
        "show_stats",
        Kind::Bool,
        Default::Bool(false),
        "Show CPU, memory and disk statistics in the status bar.",
    ),
    Setting {
        key: "dynamic_title",
        kind: Kind::Text,
        default: Default::Text("replace"),
        description: "How application titles combine with the tab title.",
        choices: &["replace", "before", "after", "hide"],
    },
    setting(
        "login_shell",
        Kind::Bool,
        Default::Bool(true),
        "Start the shell as a login shell.",
    ),
    setting(
        "cursor_blink",
        Kind::Bool,
        Default::Bool(true),
        "Blink the cursor.",
    ),
    setting(
        "scroll_on_output",
        Kind::Bool,
        Default::Bool(true),
        "Scroll to the bottom when a command produces output.",
    ),
    setting(
        "scroll_on_keystroke",
        Kind::Bool,
        Default::Bool(true),
        "Scroll to the bottom when typing.",
    ),
    setting(
        "allow_bold_text",
        Kind::Bool,
        Default::Bool(true),
        "Render bold text with a bold font.",
    ),
    setting(
        "bold_is_bright",
        Kind::Bool,
        Default::Bool(false),
        "Render bold text with the bright variant of its color.",
    ),
    setting(
        "sixel",
        Kind::Bool,
        Default::Bool(false),
        "Allow applications to draw inline Sixel images.",
    ),
    setting(
        "smooth_scroll",
        Kind::Bool,
        Default::Bool(false),
        "Scroll by pixels instead of whole lines.",
    ),
    setting(
        "word_char_exceptions",
        Kind::Text,
        Default::Text("-./?%&#:_=+@~"),
        "Extra characters treated as part of a word for double-click selection.",
    ),
    setting(
        "terminal_columns",
        Kind::Int { min: 40, max: 300 },
        Default::Int(80),
        "Initial width in character cells.",
    ),
    setting(
        "terminal_rows",
        Kind::Int { min: 10, max: 120 },
        Default::Int(24),
        "Initial height in character cells.",
    ),
    setting(
        "window_padding_horizontal",
        Kind::Int { min: 0, max: 100 },
        Default::Int(2),
        "Horizontal padding around the terminal, in pixels.",
    ),
    setting(
        "window_padding_vertical",
        Kind::Int { min: 0, max: 100 },
        Default::Int(2),
        "Vertical padding around the terminal, in pixels.",
    ),
    setting(
        "bell_notification",
        Kind::Bool,
        Default::Bool(false),
        "Enable desktop notifications when a command finishes (requires OSC 133).",
    ),
    setting(
        "notify_only_unfocused",
        Kind::Bool,
        Default::Bool(true),
        "Only notify when the terminal does not have keyboard focus.",
    ),
    setting(
        "notify_min_seconds",
        Kind::Int {
            min: 0,
            max: 86_400,
        },
        Default::Int(10),
        "Skip notifications for commands shorter than this; 0 notifies for every command.",
    ),
    setting(
        "notify_on_failure",
        Kind::Bool,
        Default::Bool(true),
        "Notify when a command fails even if it was faster than notify_min_seconds.",
    ),
    setting(
        "pager_command",
        Kind::Text,
        Default::Text("less -R"),
        "Pager used to open command output.",
    ),
    Setting {
        key: "undercurl_style",
        kind: Kind::Text,
        default: Default::Text("single"),
        description: "Shape used to draw curly underlines.",
        choices: &["single", "double", "curly", "dashed", "dotted"],
    },
    setting(
        "broadcast_input",
        Kind::Bool,
        Default::Bool(false),
        "Send keyboard input to every pane in the window.",
    ),
    setting(
        "active_profile",
        Kind::Text,
        Default::Text(""),
        "Profile applied at startup; empty means none.",
    ),
    setting(
        "session_restore",
        Kind::Bool,
        Default::Bool(true),
        "Restore tabs, splits and working directories on startup.",
    ),
    setting(
        "history_enabled",
        Kind::Bool,
        Default::Bool(true),
        "Record commands in the SQLite history database.",
    ),
    setting(
        "hint_mode_enabled",
        Kind::Bool,
        Default::Bool(true),
        "Enable hint mode for opening URLs, paths and hashes.",
    ),
    setting(
        "vi_copy_mode_enabled",
        Kind::Bool,
        Default::Bool(false),
        "Enable VI-style keyboard copy mode.",
    ),
    setting(
        "tmux_prefix_enabled",
        Kind::Bool,
        Default::Bool(true),
        "Enable the tmux-style Ctrl+B prefix in panes.",
    ),
    setting(
        "tmux_prefix_key",
        Kind::Letter,
        Default::Text("b"),
        "Letter used with Ctrl as the tmux-style prefix.",
    ),
    setting(
        "ssh_connection_sharing",
        Kind::Bool,
        Default::Bool(true),
        "Share SSH connections and enable keepalives through the bundled wrapper.",
    ),
];

pub fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| setting.key == key)
}

/// Every default setting, resolved (environment-dependent values included).
pub fn defaults() -> Value {
    let mut map = Map::new();
    for setting in SETTINGS {
        map.insert(setting.key.to_string(), setting.default_value());
    }
    Value::Object(map)
}

fn is_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// Check a value against the documented type and range of `setting`.
///
/// The error message is shown by `--validate-config` and by the settings
/// dialog, so it names the expectation instead of only saying "invalid".
pub fn validate(setting: &Setting, value: &Value) -> Result<(), String> {
    match setting.kind {
        Kind::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err("expected true or false".to_string())
            }
        }
        Kind::Int { min, max } => {
            let Some(number) = value.as_i64() else {
                return Err(format!("expected an integer between {min} and {max}"));
            };
            if value.is_f64() {
                return Err(format!("expected an integer between {min} and {max}"));
            }
            if number < min || number > max {
                return Err(format!("must be between {min} and {max}"));
            }
            Ok(())
        }
        Kind::UnitFloat { min, max } => {
            let Some(number) = value.as_f64() else {
                return Err(format!("expected a number between {min} and {max}"));
            };
            if value.is_boolean() || number < min || number > max {
                return Err(format!("must be between {min} and {max}"));
            }
            Ok(())
        }
        Kind::Text => text(value),
        Kind::HexColor => {
            let text = string(value)?;
            if is_hex_color(text) {
                Ok(())
            } else {
                Err("expected a color in #rrggbb form".to_string())
            }
        }
        Kind::HexColorOrEmpty => {
            let text = string(value)?;
            if text.is_empty() || is_hex_color(text) {
                Ok(())
            } else {
                Err("expected a color in #rrggbb form or an empty string".to_string())
            }
        }
        Kind::Letter => {
            let text = string(value)?;
            if text.chars().count() == 1
                && text.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            {
                Ok(())
            } else {
                Err("expected a single ASCII letter".to_string())
            }
        }
        Kind::Palette => match value {
            Value::Null => Ok(()),
            Value::Object(map) if map.len() <= 16 => {
                for (key, color) in map {
                    match color.as_str() {
                        Some(color) if is_hex_color(color) => {}
                        _ => {
                            return Err(format!(
                                "palette entry {key} must be a color in #rrggbb form"
                            ))
                        }
                    }
                }
                Ok(())
            }
            _ => Err("expected null or an object of up to 16 #rrggbb colors".to_string()),
        },
        Kind::StringMap => match value {
            Value::Object(map) if map.len() <= 100 => {
                for (key, item) in map {
                    if key.chars().count() > 100 {
                        return Err("map keys must be at most 100 characters".to_string());
                    }
                    match item.as_str() {
                        Some(item) if item.chars().count() <= 4096 => {}
                        Some(_) => {
                            return Err(format!("value of {key} must be at most 4096 characters"))
                        }
                        None => return Err(format!("value of {key} must be a string")),
                    }
                }
                Ok(())
            }
            _ => Err("expected an object of at most 100 string values".to_string()),
        },
    }
}

fn string(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| "expected a string".to_string())
}

fn text(value: &Value) -> Result<(), String> {
    let text = string(value)?;
    if text.chars().count() > 4096 {
        return Err("must be at most 4096 characters".to_string());
    }
    Ok(())
}

// ── Documentation generators ─────────────────────────────────

/// The block between the generated markers in `packaging/oxterm.1`.
pub fn man_options() -> String {
    let mut out = String::new();
    for (group, title) in [
        (CliGroup::Window, "Window and terminal options"),
        (CliGroup::Session, "Remote sessions and live queries"),
        (CliGroup::Inspect, "Inspection commands"),
        (CliGroup::Utility, "Utilities"),
    ] {
        out.push_str(&format!(".SS {}\n", title));
        for option in CLI.iter().filter(|option| option.group == group) {
            out.push_str(".TP\n");
            out.push_str(&man_option_name(option));
            out.push('\n');
            out.push_str(man_detail(option));
            out.push('\n');
        }
        if group == CliGroup::Window {
            out.push_str(".TP\n.B \\-\\-\n");
            out.push_str("Treat the next argument as a directory even if it starts with a dash.\n");
        }
    }
    out
}

/// Longer wording for the man page, where there is room for the details that
/// would make `--help` unreadable.
fn man_detail(option: &CliOption) -> &'static str {
    match option.long {
        "config" => {
            "Use an alternative settings file instead of \\fI~/.config/oxterm/settings.json\\fR."
        }
        "option" => {
            "Override a setting for this session. May be repeated. Values are not written back to the settings file."
        }
        "execute" => {
            "Run a command instead of the configured shell. All remaining arguments are passed to the command."
        }
        "list" => {
            "List active Oxterm terminals. The first column is the session ID used by \\fB-a\\fR, \\fB-M\\fR, \\fB--info\\fR and \\fB--detach\\fR."
        }
        "info" => "Show details about one active terminal.",
        "broker" => {
            "Internal entry point that runs the persistent PTY broker. Not meant to be called directly."
        }
        "version" => {
            "Print the version, including the build commit count as SemVer build metadata, and exit."
        }
        "help" => "Print this summary and exit.",
        "attach" => {
            "Add this terminal as another view of the session. The shell keeps running and every view shares input and output."
        }
        "move" => {
            "Like --attach, but take control: other remote clients are disconnected and the original window's view is switched off."
        }
        "detach" => {
            "Release every remote client without stopping the shell, leaving the original window as the only view."
        }
        "show-config" => {
            "Print the effective settings as JSON and exit. Reads the file given with --config, if any."
        }
        "validate-config" => {
            "Check a settings file for invalid or unknown keys and exit. Without FILE, validates the file Oxterm would load."
        }
        "diagnostics" => {
            "Print versions, paths, permissions and enforced limits, without settings values, for bug reports."
        }
        _ => option.description,
    }
}

fn man_option_name(option: &CliOption) -> String {
    let long = format!("\\-\\-{}", option.long.replace('-', "\\-"));
    let value = option
        .value
        .map(|value| format!(" \" \" {}", value.replace(' ', " ")))
        .unwrap_or_default();
    match option.short {
        Some(short) => format!(".BR \\-{short} \", \" {long}{value}"),
        None if option.value.is_some() => format!(".BR {long}{value}"),
        None => format!(".B {long}"),
    }
}

/// `docs/settings.md`: the full settings reference.
pub fn settings_markdown() -> String {
    let mut out = String::from(
        "# Settings reference\n\n\
         <!-- Generated by `cargo run --bin oxterm-docgen`; do not edit. -->\n\n\
         Oxterm reads `~/.config/oxterm/settings.json` (or the file given with\n\
         `--config FILE`). Keys not listed here are ignored. Use\n\
         `oxterm --show-config` to print the effective values, `oxterm\n\
         --show-config --default` for the built-in defaults, and `oxterm\n\
         --validate-config [FILE]` to check a file without starting a window.\n\n\
         Values can be overridden for one session with `-o KEY=VALUE` (repeatable)\n\
         or loaded from a profile with `-p NAME`. Command line overrides are\n\
         never written back to disk.\n\n\
         | Setting | Type | Default | Description |\n\
         | --- | --- | --- | --- |\n",
    );
    for setting in SETTINGS {
        let default = setting.default.documented_value();
        let default = match &default {
            Value::String(value) if value.is_empty() => "`\"\"`".to_string(),
            Value::String(value) => format!("`{value}`"),
            other => format!("`{other}`"),
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            setting.key,
            setting.kind.type_name(),
            default,
            setting.description
        ));
        if !setting.choices.is_empty() {
            let mut choices = String::new();
            for (index, choice) in setting.choices.iter().enumerate() {
                if index > 0 {
                    choices.push_str(", ");
                }
                choices.push('`');
                choices.push_str(choice);
                choices.push('`');
            }
            out.push_str(&format!("|  |  | Values: {choices} |  |\n"));
        }
    }
    out.push_str(
        "\n## Limits\n\n\
         Limits are enforced before untrusted input reaches the UI:\n\n\
         | Limit | Value |\n\
         | --- | --- |\n\
         | Settings file / `-o` value | 4096 characters |\n\
         | Snippet map entries | 100 entries, 4096 characters each |\n\
         | Custom palette entries | 16 |\n",
    );
    out.push_str(&format!(
        "| OSC 133 FIFO line buffer | {} KiB |\n\
         | OSC 133 command text | {} KiB |\n",
        super::osc133::MAX_BUFFER / 1024,
        super::osc133::MAX_COMMAND / 1024,
    ));
    out.push_str("| Control API request frame | ");
    out.push_str(&format!(
        "{} MiB |\n| Control API response frame | {} MiB |\n| PTY broker frame | {} KiB |\n| PTY broker pending output | {} MiB |\n| PTY broker clients | {} |\n",
        super::framing::MAX_REQUEST_FRAME / (1024 * 1024),
        super::framing::MAX_RESPONSE_FRAME / (1024 * 1024),
        super::framing::MAX_BROKER_FRAME / 1024,
        super::framing::MAX_BROKER_PENDING / (1024 * 1024),
        super::framing::MAX_BROKER_CLIENTS,
    ));
    out
}

// ── Keyboard shortcuts ───────────────────────────────────────

pub struct Keybind {
    pub keys: &'static str,
    pub description: &'static str,
}

pub const KEYBINDS: &[Keybind] = &[
    Keybind {
        keys: "Ctrl+Shift+T",
        description: "New tab",
    },
    Keybind {
        keys: "Ctrl+Shift+N",
        description: "New window",
    },
    Keybind {
        keys: "Ctrl+Shift+W",
        description: "Close tab",
    },
    Keybind {
        keys: "Ctrl+Shift+Q",
        description: "Close window",
    },
    Keybind {
        keys: "Ctrl+Shift+C",
        description: "Copy",
    },
    Keybind {
        keys: "Ctrl+Shift+V",
        description: "Paste",
    },
    Keybind {
        keys: "Ctrl+Shift+A",
        description: "Select all",
    },
    Keybind {
        keys: "Ctrl+Shift+S",
        description: "Rename tab",
    },
    Keybind {
        keys: "Ctrl+Shift+R",
        description: "Reset terminal",
    },
    Keybind {
        keys: "Ctrl+Shift+X",
        description: "Reset and clear",
    },
    Keybind {
        keys: "Ctrl+Shift+F",
        description: "Search the scrollback (Enter/Shift+Enter = next/prev)",
    },
    Keybind {
        keys: "Ctrl++ / Ctrl+- / Ctrl+0",
        description: "Zoom font in / out / reset",
    },
    Keybind {
        keys: "Ctrl+Shift+E / Ctrl+Shift+D",
        description: "Vertical / horizontal split",
    },
    Keybind {
        keys: "Ctrl+Shift+M / Ctrl+M",
        description: "Set / jump to next quickmark",
    },
    Keybind {
        keys: "Ctrl+Shift+H",
        description: "Hint mode for URLs, paths and Git SHAs",
    },
    Keybind {
        keys: "Ctrl+Shift+Y",
        description: "VI-style copy mode",
    },
    Keybind {
        keys: "Ctrl+Shift+G",
        description: "Open the last command's output in a pager",
    },
    Keybind {
        keys: "Ctrl+Shift+B",
        description: "Toggle broadcast input",
    },
    Keybind {
        keys: "Ctrl+R",
        description: "Interactive history search; repeat to cycle matches",
    },
    Keybind {
        keys: "Ctrl+U",
        description: "Kill line; clear history filter while its picker is open",
    },
    Keybind {
        keys: "Ctrl+W",
        description: "Kill word; remove last history filter term while its picker is open",
    },
    Keybind {
        keys: "Ctrl+L",
        description: "Clear screen",
    },
    Keybind {
        keys: "Ctrl+C",
        description: "Interrupt or cancel history mode",
    },
    Keybind {
        keys: "Ctrl+D",
        description: "EOF; closes an empty shell tab on exit",
    },
    Keybind {
        keys: "Ctrl+PageUp / Ctrl+PageDown",
        description: "Previous / next tab",
    },
    Keybind {
        keys: "Ctrl+Shift+PageUp / Ctrl+Shift+PageDown",
        description: "Move tab",
    },
    Keybind {
        keys: "Ctrl+Alt+PageUp",
        description: "Switch split pane",
    },
    Keybind {
        keys: "Ctrl+Shift+Up / Ctrl+Shift+Down",
        description: "Previous / next prompt",
    },
    Keybind {
        keys: "Ctrl+Shift+P",
        description: "Command palette",
    },
    Keybind {
        keys: "Ctrl+B, then c",
        description: "New tab (tmux-compatible)",
    },
    Keybind {
        keys: "Ctrl+B, then n / p",
        description: "Next / previous tab",
    },
    Keybind {
        keys: "Ctrl+B, then % / \"",
        description: "Vertical / horizontal split",
    },
    Keybind {
        keys: "Ctrl+B, then o",
        description: "Switch split pane",
    },
    Keybind {
        keys: "Ctrl+B, then z",
        description: "Zoom the focused pane to fill the window, repeat to restore",
    },
    Keybind {
        keys: "Ctrl+B, then =",
        description: "Equalize the split panes",
    },
    Keybind {
        keys: "Ctrl+B, then s / w",
        description: "Show the interactive terminal list",
    },
    Keybind {
        keys: "Ctrl+B, then d",
        description: "Detach the local terminal PTY; passed through when ssh is foreground",
    },
    Keybind {
        keys: "Ctrl+B, then x",
        description: "Close tab",
    },
    Keybind {
        keys: "Alt+1 ... Alt+9",
        description: "Replay history entry",
    },
    Keybind {
        keys: "F11",
        description: "Fullscreen",
    },
    Keybind {
        keys: "Click",
        description: "Open a detected URL",
    },
];

/// The keyboard shortcut table as Markdown, for the manual.
pub fn keybinds_markdown() -> String {
    let mut out = String::from("| Shortcut | Action |\n| --- | --- |\n");
    for keybind in KEYBINDS {
        out.push_str(&format!(
            "| `{}` | {} |\n",
            keybind.keys, keybind.description
        ));
    }
    out
}

pub fn keybinds() -> String {
    let width = KEYBINDS
        .iter()
        .map(|keybind| keybind.keys.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for keybind in KEYBINDS {
        out.push_str(&format!(
            "{:<width$}  {}\n",
            keybind.keys, keybind.description
        ));
    }
    out
}

// ── Completion generators ────────────────────────────────────

fn option_words() -> Vec<String> {
    let mut words = Vec::new();
    for option in CLI {
        if let Some(short) = option.short {
            words.push(format!("-{short}"));
        }
        words.push(format!("--{}", option.long));
    }
    words.push("--".to_string());
    words
}

/// The `opts="..."` assignment used by the bash completion.
pub fn bash_option_words() -> String {
    let mut out = String::from("opts=\"");
    for (index, word) in option_words().iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push_str("\"");
    out
}

/// The `case "$prev"` block used by the bash completion.
pub fn bash_value_cases() -> String {
    let mut groups: Vec<(ValueKind, Vec<String>)> = Vec::new();
    for option in CLI {
        if option.value.is_none() {
            continue;
        }
        let mut names = Vec::new();
        if let Some(short) = option.short {
            names.push(format!("-{short}"));
        }
        names.push(format!("--{}", option.long));
        match groups.iter_mut().find(|(kind, _)| *kind == option.kind) {
            Some((_, existing)) => existing.extend(names),
            None => groups.push((option.kind, names)),
        }
    }
    let mut out = String::from("    case \"$prev\" in\n");
    for (kind, names) in groups {
        let action = match kind {
            ValueKind::Dir => "COMPREPLY=($(compgen -d -- \"$cur\"))",
            ValueKind::File => "COMPREPLY=($(compgen -f -- \"$cur\"))",
            ValueKind::Session => "COMPREPLY=($(compgen -W \"$(_oxterm_sessions)\" -- \"$cur\"))",
            ValueKind::Font => {
                "COMPREPLY=($(compgen -W \"$(fc-list : family 2>/dev/null | tr ',' '\\n' | sort -u)\" -- \"$cur\"))"
            }
            ValueKind::Command => "COMPREPLY=($(compgen -c -- \"$cur\"))",
            _ => "return 0",
        };
        out.push_str(&format!("        {} )\n", names.join(" | ")));
        if action == "return 0" {
            out.push_str("            return 0\n");
        } else {
            out.push_str(&format!("            {action}\n            return 0\n"));
        }
        out.push_str("            ;;\n");
    }
    out.push_str("    esac\n");
    out
}

/// Global `complete -c oxterm` lines for the fish completion.
pub fn fish_options() -> String {
    let mut out = String::new();
    for option in CLI {
        let mut line = String::from("complete -c oxterm -n '__oxterm_global_option'");
        if let Some(short) = option.short {
            line.push_str(&format!(" -s {short}"));
        }
        line.push_str(&format!(" -l {}", option.long));
        line.push_str(&format!(
            " -d '{}'",
            option.description.replace('\'', "\\'")
        ));
        match option.kind {
            ValueKind::None => {}
            ValueKind::Dir => line.push_str(" -r -a '(__fish_complete_directories)'"),
            ValueKind::File => line.push_str(" -r -F"),
            ValueKind::Command => line.push_str(" -r -a '(__fish_complete_command)'"),
            ValueKind::Session => line.push_str(
                " -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \\t)'",
            ),
            _ => line.push_str(" -r"),
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Option specifications for `_arguments` in the zsh completion.
pub fn zsh_options() -> String {
    let mut out = String::new();
    for option in CLI {
        let description = option
            .description
            .replace('[', "(")
            .replace(']', ")")
            .replace(':', " -")
            .replace('\'', "");
        let action = match option.kind {
            ValueKind::None => String::new(),
            ValueKind::Dir => ":directory:_directories".to_string(),
            ValueKind::File => ":file:_files".to_string(),
            ValueKind::Session => ":session:_oxterm_sessions".to_string(),
            ValueKind::Command => ":command:_command_names -e".to_string(),
            ValueKind::Font => ":family:".to_string(),
            ValueKind::Number => ":number:".to_string(),
            ValueKind::Profile => ":profile:".to_string(),
            ValueKind::KeyValue => ":key=value:".to_string(),
            ValueKind::Text => format!(":{}:", option.value.unwrap_or("value").to_lowercase()),
        };
        let spec = match option.short {
            Some(short) => format!(
                "'(-{short} --{long})'{{-{short},--{long}}}'[{description}]{action}'",
                short = short,
                long = option.long
            ),
            None => format!("'--{long}[{description}]{action}'", long = option.long),
        };
        out.push_str(&format!("    {spec}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn settings_defaults_are_unique_and_documented() {
        let mut keys = BTreeSet::new();
        for setting in SETTINGS {
            assert!(keys.insert(setting.key), "duplicate key {}", setting.key);
            assert!(
                !setting.description.is_empty(),
                "{} has no description",
                setting.key
            );
            let value = setting.default_value();
            assert!(
                validate(setting, &value).is_ok(),
                "default of {} does not validate: {value}",
                setting.key
            );
        }
    }

    #[test]
    fn settings_markdown_documents_fallbacks_not_the_environment() {
        // `docs/settings.md` is committed and checked in CI, so the generated
        // reference must not depend on the shell that ran docgen.
        let markdown = settings_markdown();
        for setting in SETTINGS {
            let Default::Env(key, fallback) = setting.default else {
                continue;
            };
            let row = format!("| `{}` |", setting.key);
            let line = markdown
                .lines()
                .find(|line| line.starts_with(&row))
                .unwrap_or_else(|| panic!("{} is missing from the markdown", setting.key));
            assert!(
                line.contains(&format!("`{fallback}`")),
                "{} should document ${key}'s fallback {fallback}: {line}",
                setting.key
            );
        }
    }

    #[test]
    fn defaults_match_the_setting_table() {
        let defaults = defaults();
        let map = defaults.as_object().unwrap();
        assert_eq!(map.len(), SETTINGS.len());
        for setting in SETTINGS {
            assert_eq!(
                map.get(setting.key),
                Some(&setting.default_value()),
                "missing default for {}",
                setting.key
            );
        }
    }

    #[test]
    fn validation_reports_the_expectation() {
        let setting = find("font_size").unwrap();
        assert!(validate(setting, &Value::from(12)).is_ok());
        assert!(validate(setting, &Value::from(3)).is_err());
        assert!(validate(setting, &Value::from(12.5)).is_err());
        assert!(validate(setting, &Value::from("12")).is_err());
        let color = find("foreground_color").unwrap();
        assert!(validate(color, &Value::from("#abcdef")).is_ok());
        assert!(validate(color, &Value::from("")).is_ok());
        assert!(validate(color, &Value::from("#abc")).is_err());
        let palette = find("custom_palette").unwrap();
        assert!(validate(palette, &Value::Null).is_ok());
        assert!(validate(palette, &serde_json::json!({"black": "#000000"})).is_ok());
        assert!(validate(palette, &serde_json::json!({"black": "bad"})).is_err());
        let snippets = find("snippets").unwrap();
        assert!(validate(snippets, &serde_json::json!({"t": "make test"})).is_ok());
        assert!(validate(snippets, &serde_json::json!({"t": 1})).is_err());
    }

    #[test]
    fn help_lists_every_option_once() {
        let help = usage();
        for option in CLI {
            assert!(
                help.contains(&format!("--{}", option.long)),
                "help is missing --{}",
                option.long
            );
            assert!(
                help.contains(option.description),
                "help is missing the description of --{}",
                option.long
            );
        }
    }

    #[test]
    fn generated_blocks_are_non_empty() {
        assert!(man_options().contains(".TP"));
        assert!(bash_option_words().contains("--working-directory"));
        assert!(bash_value_cases().contains("--config"));
        assert!(fish_options().contains("-l working-directory"));
        assert!(zsh_options().contains("--working-directory"));
        assert!(settings_markdown().contains("`font_size`"));
        assert!(keybinds().contains("Ctrl+Shift+T"));
    }
}
