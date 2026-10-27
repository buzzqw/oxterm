//! Color themes: built-in palettes plus drop-in files.
//!
//! Built-ins cover the popular schemes; additional themes are read from
//! `~/.config/oxterm/themes/`. Both Ghostty-style theme files and iTerm2
//! `.itermcolors` files are understood, so themes can be copied from either
//! ecosystem without conversion.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// The 16 ANSI color slots, in palette order.
pub const ANSI_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightblack",
    "brightred",
    "brightgreen",
    "brightyellow",
    "brightblue",
    "brightmagenta",
    "brightcyan",
    "brightwhite",
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Theme {
    pub name: String,
    pub foreground: String,
    pub background: String,
    pub cursor: String,
    pub palette: BTreeMap<String, String>,
}

impl Theme {
    pub fn color(&self, key: &str) -> Option<&str> {
        self.palette.get(key).map(String::as_str)
    }

    /// Palette colors in ANSI order, ready for `vte_terminal_set_colors`.
    pub fn ansi(&self) -> Vec<String> {
        ANSI_KEYS
            .iter()
            .map(|key| {
                self.palette
                    .get(*key)
                    .cloned()
                    .unwrap_or_else(|| "#000000".to_string())
            })
            .collect()
    }
}

pub fn themes_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".config")
        .join("oxterm")
        .join("themes")
}

/// Every available theme: built-ins first, then files from [`themes_dir`].
/// External themes with the same name win and are suffixed with `(user)`.
pub fn all() -> &'static BTreeMap<String, Theme> {
    static THEMES: OnceLock<BTreeMap<String, Theme>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut themes = builtin();
        for theme in load_dir(&themes_dir()) {
            let mut name = theme.name.clone();
            if themes.contains_key(&name) {
                name.push_str(" (user)");
            }
            let mut theme = theme;
            theme.name = name.clone();
            themes.insert(name, theme);
        }
        themes
    })
}

pub fn get(name: &str) -> Option<&'static Theme> {
    all().get(name)
}

pub fn default_theme() -> &'static Theme {
    get("Dark (Default)")
        .or_else(|| all().values().next())
        .expect("at least one built-in theme")
}

/// Every theme embedded in the binary.
///
/// Oxterm's own palettes come first so historical names such as
/// "Dark (Default)" and "Light" keep their exact colors; the generated
/// iTerm2-Color-Schemes bundle fills the rest. Regenerate the bundle with
/// `scripts/update-themes.py`; `themes/licenses.md` records each licence.
pub fn builtin() -> BTreeMap<String, Theme> {
    let mut themes = originals();
    for theme in serde_json::from_str::<Vec<Theme>>(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/themes.json"
    )))
    .unwrap_or_default()
    {
        themes.entry(theme.name.clone()).or_insert(theme);
    }
    themes
}

/// Oxterm's hand-written palettes, kept stable across releases.
fn originals() -> BTreeMap<String, Theme> {
    let mut themes = BTreeMap::new();
    for theme in [
        theme(
            "Dark (Default)",
            "#d3d7cf",
            "#0a0a0a",
            "#ffffff",
            [
                "#0a0a0a", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a",
                "#d3d7cf", "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8",
                "#34e2e2", "#eeeeec",
            ],
        ),
        theme(
            "Light",
            "#2e3436",
            "#ffffff",
            "#000000",
            [
                "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a",
                "#d3d7cf", "#555753", "#ef2929", "#8ae234", "#fce94f", "#729fcf", "#ad7fa8",
                "#34e2e2", "#eeeeec",
            ],
        ),
        theme(
            "Solarized Dark",
            "#839496",
            "#002b36",
            "#839496",
            [
                "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                "#93a1a1", "#fdf6e3",
            ],
        ),
        theme(
            "Solarized Light",
            "#657b83",
            "#fdf6e3",
            "#586e75",
            [
                "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                "#93a1a1", "#fdf6e3",
            ],
        ),
        theme(
            "Gruvbox Dark",
            "#ebdbb2",
            "#282828",
            "#ebdbb2",
            [
                "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a",
                "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b",
                "#8ec07c", "#ebdbb2",
            ],
        ),
        theme(
            "Gruvbox Light",
            "#3c3836",
            "#fbf1c7",
            "#282828",
            [
                "#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a",
                "#7c6f64", "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71",
                "#427b58", "#3c3836",
            ],
        ),
        theme(
            "Monokai",
            "#f8f8f2",
            "#272822",
            "#f8f8f0",
            [
                "#272822", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff", "#a1efe4",
                "#f8f8f2", "#75715e", "#f92672", "#a6e22e", "#f4bf75", "#66d9ef", "#ae81ff",
                "#a1efe4", "#f9f8f5",
            ],
        ),
        theme(
            "Nord",
            "#e5e9f0",
            "#3b4252",
            "#e5e9f0",
            [
                "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead",
                "#8fbcbb", "#eceff4",
            ],
        ),
        theme(
            "Matrix",
            "#00ff41",
            "#0d0208",
            "#00ff41",
            [
                "#0d0208", "#008f11", "#00ff41", "#00ff41", "#008f11", "#00ff41", "#00ff41",
                "#00ff41", "#003b00", "#00ff41", "#00ff41", "#00ff41", "#00ff41", "#00ff41",
                "#00ff41", "#00ff41",
            ],
        ),
        theme(
            "Catppuccin Mocha",
            "#cdd6f4",
            "#1e1e2e",
            "#f5e0dc",
            [
                "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5",
                "#bac2de", "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7",
                "#94e2d5", "#a6adc8",
            ],
        ),
        theme(
            "Catppuccin Macchiato",
            "#cad3f5",
            "#24273a",
            "#f4dbd6",
            [
                "#494d64", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca",
                "#b8c0e0", "#5b6078", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6",
                "#8bd5ca", "#a5adcb",
            ],
        ),
        theme(
            "Catppuccin Frappe",
            "#c6d0f5",
            "#303446",
            "#f2d5cf",
            [
                "#51576d", "#e78284", "#a6d189", "#e5c890", "#8caaee", "#f4b8e4", "#81c8be",
                "#b5bfe2", "#626880", "#e78284", "#a6d189", "#e5c890", "#8caaee", "#f4b8e4",
                "#81c8be", "#a5adce",
            ],
        ),
        theme(
            "Catppuccin Latte",
            "#4c4f69",
            "#eff1f5",
            "#dc8a78",
            [
                "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299",
                "#acb0be", "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb",
                "#179299", "#bcc0cc",
            ],
        ),
        theme(
            "Dracula",
            "#f8f8f2",
            "#282a36",
            "#f8f8f2",
            [
                "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd",
                "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df",
                "#a4ffff", "#ffffff",
            ],
        ),
        theme(
            "Tokyo Night",
            "#c0caf5",
            "#1a1b26",
            "#c0caf5",
            [
                "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff",
                "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7",
                "#7dcfff", "#c0caf5",
            ],
        ),
        theme(
            "One Dark",
            "#abb2bf",
            "#282c34",
            "#528bff",
            [
                "#1e2127", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2",
                "#abb2bf", "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
                "#56b6c2", "#ffffff",
            ],
        ),
        theme(
            "Rose Pine",
            "#e0def4",
            "#191724",
            "#524f67",
            [
                "#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba",
                "#e0def4", "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7",
                "#ebbcba", "#e0def4",
            ],
        ),
        theme(
            "Rose Pine Moon",
            "#e0def4",
            "#232136",
            "#524f67",
            [
                "#393552", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97",
                "#e0def4", "#6e6a86", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7",
                "#ea9a97", "#e0def4",
            ],
        ),
        theme(
            "Kanagawa",
            "#dcd7ba",
            "#1f1f28",
            "#c8c093",
            [
                "#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589",
                "#c8c093", "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9",
                "#7aa89f", "#dcd7ba",
            ],
        ),
        theme(
            "Everforest",
            "#d3c6aa",
            "#2d353b",
            "#d3c6aa",
            [
                "#475258", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092",
                "#d3c6aa", "#475258", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6",
                "#83c092", "#d3c6aa",
            ],
        ),
        theme(
            "GitHub Dark",
            "#c9d1d9",
            "#0d1117",
            "#58a6ff",
            [
                "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf",
                "#b1bac4", "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff",
                "#56d4dd", "#f0f6fc",
            ],
        ),
        theme(
            "GitHub Light",
            "#24292f",
            "#ffffff",
            "#24292f",
            [
                "#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83",
                "#6e7781", "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9",
                "#3192aa", "#8c959f",
            ],
        ),
        theme(
            "Ayu Mirage",
            "#cbccc6",
            "#1f2430",
            "#ffcc66",
            [
                "#191e2a", "#f28779", "#bae67e", "#ffd580", "#73d0ff", "#d4bfff", "#95e6cb",
                "#c7c7c7", "#686868", "#f28779", "#bae67e", "#ffd580", "#73d0ff", "#d4bfff",
                "#95e6cb", "#ffffff",
            ],
        ),
        theme(
            "Material",
            "#eeffff",
            "#263238",
            "#ffcc00",
            [
                "#000000", "#ff5370", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff",
                "#ffffff", "#546e7a", "#ff5370", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea",
                "#89ddff", "#ffffff",
            ],
        ),
        theme(
            "Nightfox",
            "#cdcecf",
            "#192330",
            "#cdcecf",
            [
                "#393b44", "#c94f6d", "#81b29a", "#dbc074", "#719cd6", "#9d79d6", "#63cdcf",
                "#dfdfe0", "#575860", "#d16983", "#8ebaa4", "#e0c989", "#86abdc", "#baa1e2",
                "#7ad5d6", "#e4e4e5",
            ],
        ),
    ] {
        themes.insert(theme.name.clone(), theme);
    }
    themes
}

fn theme(name: &str, foreground: &str, background: &str, cursor: &str, ansi: [&str; 16]) -> Theme {
    let palette = ANSI_KEYS
        .iter()
        .zip(ansi)
        .map(|(key, color)| ((*key).to_string(), color.to_string()))
        .collect();
    Theme {
        name: name.to_string(),
        foreground: foreground.to_string(),
        background: background.to_string(),
        cursor: cursor.to_string(),
        palette,
    }
}

/// Read every theme file in `dir`. Unreadable or invalid files are skipped:
/// one broken theme must not break the whole palette list.
pub fn load_dir(dir: &Path) -> Vec<Theme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut themes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match load_file(&path) {
            Ok(theme) => themes.push(theme),
            Err(error) => eprintln!("oxterm: ignoring theme {}: {}", path.display(), error),
        }
    }
    themes.sort_by(|a, b| a.name.cmp(&b.name));
    themes
}

pub fn load_file(path: &Path) -> Result<Theme, String> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unnamed".to_string());
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse(&name, &text)
}

/// Parse a theme by content: iTerm2 XML plist or Ghostty `key = value` lines.
pub fn parse(name: &str, text: &str) -> Result<Theme, String> {
    if text.contains("<plist") || text.contains("<key>Ansi 0 Color</key>") {
        parse_iterm2(name, text)
    } else {
        parse_ghostty(name, text)
    }
}

fn parse_ghostty(name: &str, text: &str) -> Result<Theme, String> {
    let mut foreground = None;
    let mut background = None;
    let mut cursor = None;
    let mut palette = BTreeMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "background" => background = Some(color(value)?),
            "foreground" => foreground = Some(color(value)?),
            "cursor" | "cursor-color" | "cursor_color" => cursor = Some(color(value)?),
            "palette" => {
                let Some((index, value)) = value.split_once('=') else {
                    return Err(format!("line {}: expected palette = N=#rrggbb", number + 1));
                };
                let index: usize = index
                    .trim()
                    .parse()
                    .map_err(|_| format!("line {}: invalid palette index", number + 1))?;
                let Some(key) = ANSI_KEYS.get(index) else {
                    return Err(format!("line {}: palette index must be 0-15", number + 1));
                };
                palette.insert((*key).to_string(), color(value)?);
            }
            _ => {}
        }
    }
    let foreground = foreground.ok_or("missing foreground")?;
    let background = background.ok_or("missing background")?;
    if palette.len() != 16 {
        return Err(format!(
            "expected 16 palette entries, found {}",
            palette.len()
        ));
    }
    Ok(Theme {
        name: name.to_string(),
        cursor: cursor.unwrap_or_else(|| foreground.clone()),
        foreground,
        background,
        palette,
    })
}

fn parse_iterm2(name: &str, text: &str) -> Result<Theme, String> {
    static DICT: OnceLock<Regex> = OnceLock::new();
    static COMPONENT: OnceLock<Regex> = OnceLock::new();
    let dict = DICT.get_or_init(|| {
        Regex::new(r"(?s)<key>([^<]+)</key>\s*<dict>(.*?)</dict>").expect("valid regex")
    });
    let component = COMPONENT.get_or_init(|| {
        Regex::new(
            r"<key>(Red|Green|Blue) Component</key>\s*<(?:real|integer)>([^<]+)</(?:real|integer)>",
        )
        .expect("valid regex")
    });

    let mut foreground = None;
    let mut background = None;
    let mut cursor = None;
    let mut palette = BTreeMap::new();
    for captures in dict.captures_iter(text) {
        let key = captures[1].trim();
        let body = &captures[2];
        let mut red = None;
        let mut green = None;
        let mut blue = None;
        for part in component.captures_iter(body) {
            let value: f64 = part[2]
                .trim()
                .parse()
                .map_err(|_| format!("invalid {key} color component"))?;
            let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            match &part[1] {
                "Red" => red = Some(byte),
                "Green" => green = Some(byte),
                "Blue" => blue = Some(byte),
                _ => {}
            }
        }
        let (Some(red), Some(green), Some(blue)) = (red, green, blue) else {
            continue;
        };
        let color = format!("#{red:02x}{green:02x}{blue:02x}");
        match key {
            "Foreground Color" => foreground = Some(color),
            "Background Color" => background = Some(color),
            "Cursor Color" | "Cursor Guide Color" => cursor = Some(color),
            _ => {
                if let Some(index) = key
                    .strip_prefix("Ansi ")
                    .and_then(|rest| rest.strip_suffix(" Color"))
                    .and_then(|number| number.parse::<usize>().ok())
                {
                    if let Some(slot) = ANSI_KEYS.get(index) {
                        palette.insert((*slot).to_string(), color);
                    }
                }
            }
        }
    }
    let foreground = foreground.ok_or("missing Foreground Color")?;
    let background = background.ok_or("missing Background Color")?;
    if palette.len() != 16 {
        return Err(format!("expected 16 Ansi colors, found {}", palette.len()));
    }
    Ok(Theme {
        name: name.to_string(),
        cursor: cursor.unwrap_or_else(|| foreground.clone()),
        foreground,
        background,
        palette,
    })
}

fn color(value: &str) -> Result<String, String> {
    let value = value.trim();
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid color {value:?}"));
    }
    Ok(format!("#{}", value.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_complete_and_valid() {
        let themes = builtin();
        assert!(themes.len() >= 200, "expected the bundled theme pack");
        for (name, theme) in &themes {
            assert_eq!(name, &theme.name);
            assert_eq!(theme.palette.len(), 16, "{name} palette is incomplete");
            for color in
                theme
                    .palette
                    .values()
                    .chain([&theme.foreground, &theme.background, &theme.cursor])
            {
                assert!(
                    color.len() == 7 && color.starts_with('#'),
                    "{name}: bad color {color}"
                );
            }
        }
        assert!(themes.contains_key("Catppuccin Mocha"));
        assert!(themes.contains_key("Tokyo Night"));
        assert!(themes.contains_key("Dracula"));
        assert!(themes.contains_key("0x96f"));
    }

    #[test]
    fn parses_ghostty_theme_files() {
        let theme = parse(
            "Mine",
            "# comment\nbackground = #101010\nforeground = #eeeeee\ncursor-color = #ff0000\n\
             palette = 0=#000000\npalette = 1=#cc0000\npalette = 2=#00cc00\npalette = 3=#cccc00\n\
             palette = 4=#0000cc\npalette = 5=#cc00cc\npalette = 6=#00cccc\npalette = 7=#cccccc\n\
             palette = 8=#555555\npalette = 9=#ff5555\npalette = 10=#55ff55\npalette = 11=#ffff55\n\
             palette = 12=#5555ff\npalette = 13=#ff55ff\npalette = 14=#55ffff\npalette = 15=#ffffff\n",
        )
        .unwrap();
        assert_eq!(theme.name, "Mine");
        assert_eq!(theme.background, "#101010");
        assert_eq!(theme.cursor, "#ff0000");
        assert_eq!(theme.color("brightwhite"), Some("#ffffff"));
    }

    #[test]
    fn parses_iterm2_theme_files() {
        let mut xml = String::from(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>Background Color</key><dict>\
             <key>Red Component</key><real>0.0627</real>\
             <key>Green Component</key><real>0.0627</real>\
             <key>Blue Component</key><real>0.0627</real></dict>\
             <key>Foreground Color</key><dict>\
             <key>Red Component</key><real>0.9</real>\
             <key>Green Component</key><real>0.9</real>\
             <key>Blue Component</key><real>0.9</real></dict>",
        );
        for index in 0..16 {
            let value = index as f64 / 16.0;
            xml.push_str(&format!(
                "<key>Ansi {index} Color</key><dict>\
                 <key>Red Component</key><real>{value}</real>\
                 <key>Green Component</key><real>{value}</real>\
                 <key>Blue Component</key><real>{value}</real></dict>"
            ));
        }
        xml.push_str("</dict></plist>");
        let theme = parse("Imported", &xml).unwrap();
        assert_eq!(theme.background, "#101010");
        assert_eq!(theme.foreground, "#e6e6e6");
        assert_eq!(theme.palette.len(), 16);
    }

    #[test]
    fn invalid_themes_are_rejected_clearly() {
        assert!(parse("bad", "background = #fff")
            .unwrap_err()
            .contains("invalid color"));
        assert!(parse("bad", "foreground = #ffffff\nbackground = #000000")
            .unwrap_err()
            .contains("expected 16"));
    }

    #[test]
    fn loads_a_directory_and_renames_collisions() {
        let dir = std::env::temp_dir().join(format!(
            "oxterm-themes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::from("background = #000000\nforeground = #ffffff\n");
        for index in 0..16 {
            text.push_str(&format!("palette = {index}=#010101\n"));
        }
        std::fs::write(dir.join("Mine"), &text).unwrap();
        std::fs::write(dir.join("broken"), "nonsense").unwrap();
        let themes = load_dir(&dir);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Mine");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
