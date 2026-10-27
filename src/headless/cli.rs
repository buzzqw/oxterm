//! Command line parsing for the main `oxterm` binary.
//!
//! Kept free of GTK so the parsing rules, `--help` text and the inspection
//! commands (`--show-config`, `--validate-config`, `--list-*`, `--diagnostics`)
//! can run without a display and be tested headlessly.

use serde_json::Value;

use super::spec;

#[derive(Debug, Clone, PartialEq)]
pub enum Inspection {
    ShowConfig { defaults: bool },
    ValidateConfig(Option<String>),
    ListSettings,
    ListThemes,
    ListKeybinds,
    ListActions,
    Diagnostics,
    EditConfig,
    ListFonts,
}

#[derive(Debug, Clone)]
pub struct CliOptions {
    pub start_dir: Option<String>,
    pub requested_dir: bool,
    pub new_window: bool,
    pub no_restore: bool,
    pub execute: Option<Vec<String>>,
    pub title: Option<String>,
    pub hold: bool,
    pub fullscreen: bool,
    pub maximize: bool,
    pub geometry: Option<(i32, i32)>,
    /// `--class` / `--name`: WM_CLASS class and instance name for the window,
    /// used by window managers for tiling rules, icons and .desktop matching.
    pub wm_class: Option<String>,
    pub wm_name: Option<String>,
    /// `-p/--profile`: profile to apply on startup (session-only).
    pub profile: Option<String>,
    /// `--config`: alternative settings file.
    pub config: Option<String>,
    /// `-o/--option key=value`: session-only settings overrides.
    pub options: Vec<(String, Value)>,
    pub show_config: bool,
    pub show_default: bool,
    pub validate_config: Option<Option<String>>,
    pub list_settings: bool,
    pub list_themes: bool,
    pub list_keybinds: bool,
    pub list_actions: bool,
    pub diagnostics: bool,
    pub edit_config: bool,
    pub list_fonts: bool,
    pub version: bool,
    pub help: bool,
}

impl CliOptions {
    /// The requested inspection command, if any. At most one may be given.
    pub fn inspection(&self) -> Option<Inspection> {
        let mut found = Vec::new();
        if self.show_config {
            found.push(Inspection::ShowConfig {
                defaults: self.show_default,
            });
        }
        if let Some(file) = &self.validate_config {
            found.push(Inspection::ValidateConfig(file.clone()));
        }
        if self.list_settings {
            found.push(Inspection::ListSettings);
        }
        if self.list_themes {
            found.push(Inspection::ListThemes);
        }
        if self.list_keybinds {
            found.push(Inspection::ListKeybinds);
        }
        if self.list_actions {
            found.push(Inspection::ListActions);
        }
        if self.diagnostics {
            found.push(Inspection::Diagnostics);
        }
        if self.edit_config {
            found.push(Inspection::EditConfig);
        }
        if self.list_fonts {
            found.push(Inspection::ListFonts);
        }
        (found.len() == 1).then(|| found.remove(0))
    }
}

/// Turn the right-hand side of `-o key=value` into a JSON value, matching the
/// type users expect: booleans, integers and floats are parsed as such, and
/// anything else stays a string (e.g. `opacity=0.9`, `login_shell=false`).
pub fn parse_override_value(raw: &str) -> Value {
    match raw {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = raw.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return Value::from(f);
    }
    Value::String(raw.to_string())
}

/// Parse a `key=value` override string used by `-o/--option`, `--font` and
/// `--font-size`.
pub fn parse_option_kv(s: &str) -> Result<(String, Value), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid option '{}': expected key=value", s))?;
    let key = key.trim();
    if key.is_empty() {
        return Err(format!("invalid option '{}': empty key", s));
    }
    Ok((key.to_string(), parse_override_value(value)))
}

/// Parse a `COLSxROWS` geometry string (e.g. `120x40`), as used by many
/// modern terminals. Columns and rows must both be positive integers.
pub fn parse_geometry(s: &str) -> Result<(i32, i32), String> {
    const MAX_GEOMETRY: i32 = 1_000;
    let lower = s.to_ascii_lowercase();
    let (c, r) = lower
        .split_once('x')
        .ok_or_else(|| format!("invalid geometry '{}': expected COLSxROWS (e.g. 120x40)", s))?;
    let cols: i32 = c
        .trim()
        .parse()
        .map_err(|_| format!("invalid geometry '{}': columns must be a number", s))?;
    let rows: i32 = r
        .trim()
        .parse()
        .map_err(|_| format!("invalid geometry '{}': rows must be a number", s))?;
    if cols < 1 || rows < 1 || cols > MAX_GEOMETRY || rows > MAX_GEOMETRY {
        return Err(format!(
            "invalid geometry '{}': columns and rows must be between 1 and {}",
            s, MAX_GEOMETRY
        ));
    }
    Ok((cols, rows))
}

pub fn expand_dir(d: String) -> Result<String, String> {
    let expanded = if let Some(rest) = d.strip_prefix("~/") {
        dirs::home_dir()
            .map(|p| p.join(rest).to_string_lossy().to_string())
            .unwrap_or_default()
    } else if d == "~" {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        d
    };
    if std::path::Path::new(&expanded).is_dir() {
        std::fs::canonicalize(&expanded)
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| format!("cannot resolve directory {}: {}", expanded, e))
    } else {
        Err(format!("not a directory: {}", expanded))
    }
}

pub fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut start_dir: Option<String> = None;
    let mut requested_dir = false;
    let mut new_window = false;
    let mut no_restore = false;
    let mut execute: Option<Vec<String>> = None;
    let mut title: Option<String> = None;
    let mut hold = false;
    let mut fullscreen = false;
    let mut maximize = false;
    let mut geometry: Option<(i32, i32)> = None;
    let mut wm_class: Option<String> = None;
    let mut wm_name: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut config: Option<String> = None;
    let mut options: Vec<(String, Value)> = Vec::new();
    let mut show_config = false;
    let mut show_default = false;
    let mut validate_config: Option<Option<String>> = None;
    let mut list_settings = false;
    let mut list_themes = false;
    let mut list_keybinds = false;
    let mut list_actions = false;
    let mut diagnostics = false;
    let mut edit_config = false;
    let mut list_fonts = false;
    let mut version = false;
    let mut help = false;
    let mut no_more_opts = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if no_more_opts {
            if start_dir.is_some() {
                return Err("directory and --working-directory cannot be used together".into());
            }
            start_dir = Some(arg.clone());
            requested_dir = true;
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--" => {
                no_more_opts = true;
            }
            "-w" | "--working-directory" => {
                i += 1;
                if i >= args.len() {
                    return Err("--working-directory requires an argument".into());
                }
                if start_dir.is_some() {
                    return Err("directory and --working-directory cannot be used together".into());
                }
                start_dir = Some(args[i].clone());
                requested_dir = true;
            }
            "--new-window" => new_window = true,
            "--no-restore" => no_restore = true,
            "--hold" => hold = true,
            "--fullscreen" | "-F" => fullscreen = true,
            "--maximize" | "-m" => maximize = true,
            "-T" | "--title" => {
                i += 1;
                if i >= args.len() {
                    return Err("--title requires an argument".into());
                }
                title = Some(args[i].clone());
            }
            "-g" | "--geometry" => {
                i += 1;
                if i >= args.len() {
                    return Err("--geometry requires an argument".into());
                }
                geometry = Some(parse_geometry(&args[i])?);
            }
            "--class" => {
                i += 1;
                if i >= args.len() {
                    return Err("--class requires an argument".into());
                }
                wm_class = Some(args[i].clone());
            }
            "--name" => {
                i += 1;
                if i >= args.len() {
                    return Err("--name requires an argument".into());
                }
                wm_name = Some(args[i].clone());
            }
            "-p" | "--profile" => {
                i += 1;
                if i >= args.len() {
                    return Err("--profile requires an argument".into());
                }
                profile = Some(args[i].clone());
            }
            "--config" => {
                i += 1;
                if i >= args.len() {
                    return Err("--config requires an argument".into());
                }
                config = Some(args[i].clone());
            }
            "-o" | "--option" => {
                i += 1;
                if i >= args.len() {
                    return Err("--option requires an argument".into());
                }
                options.push(parse_option_kv(&args[i])?);
            }
            "--font" => {
                i += 1;
                if i >= args.len() {
                    return Err("--font requires an argument".into());
                }
                options.push(("font_name".to_string(), Value::String(args[i].clone())));
            }
            "--font-size" => {
                i += 1;
                if i >= args.len() {
                    return Err("--font-size requires an argument".into());
                }
                let sz: i64 = args[i]
                    .parse()
                    .map_err(|_| format!("invalid --font-size '{}': expected a number", args[i]))?;
                if !(4..=128).contains(&sz) {
                    return Err(format!(
                        "invalid --font-size '{}': must be between 4 and 128",
                        args[i]
                    ));
                }
                options.push(("font_size".to_string(), Value::from(sz)));
            }
            "-e" | "--execute" => {
                i += 1;
                let mut cmd = Vec::new();
                while i < args.len() {
                    cmd.push(args[i].clone());
                    i += 1;
                }
                if cmd.is_empty() {
                    return Err("--execute requires a command".into());
                }
                execute = Some(cmd);
            }
            "--show-config" => show_config = true,
            "--default" => show_default = true,
            "--validate-config" => {
                validate_config = Some(None);
                // The file is optional; take the next argument unless it is an
                // option itself.
                if let Some(next) = args.get(i + 1) {
                    if !next.starts_with('-') {
                        validate_config = Some(Some(next.clone()));
                        i += 1;
                    }
                }
            }
            "--list-settings" => list_settings = true,
            "--list-themes" => list_themes = true,
            "--list-keybinds" => list_keybinds = true,
            "--list-actions" => list_actions = true,
            "--diagnostics" => diagnostics = true,
            "--edit-config" => edit_config = true,
            "--list-fonts" => list_fonts = true,
            "--version" | "-V" => version = true,
            "--help" | "-h" => help = true,
            _ => {
                if arg.starts_with('-') {
                    return Err(format!("Unknown option: {}", arg));
                }
                if start_dir.is_some() {
                    return Err("directory and --working-directory cannot be used together".into());
                }
                start_dir = Some(arg.clone());
                requested_dir = true;
            }
        }
        i += 1;
    }
    if fullscreen && maximize {
        return Err("--fullscreen and --maximize cannot be used together".into());
    }
    if show_default && !show_config {
        return Err("--default can only be used with --show-config".into());
    }
    let inspections = usize::from(show_config)
        + usize::from(validate_config.is_some())
        + usize::from(list_settings)
        + usize::from(list_themes)
        + usize::from(list_keybinds)
        + usize::from(list_actions)
        + usize::from(diagnostics)
        + usize::from(edit_config)
        + usize::from(list_fonts);
    if inspections > 1 {
        return Err("choose one inspection option at a time".into());
    }
    let mut parsed = CliOptions {
        start_dir: None,
        requested_dir,
        new_window,
        no_restore,
        execute,
        title,
        hold,
        fullscreen,
        maximize,
        geometry,
        wm_class,
        wm_name,
        profile,
        config,
        options,
        show_config,
        show_default,
        validate_config,
        list_settings,
        list_themes,
        list_keybinds,
        list_actions,
        diagnostics,
        edit_config,
        list_fonts,
        version,
        help,
    };
    if parsed.inspection().is_some() || help || version {
        return Ok(parsed);
    }
    parsed.start_dir = match start_dir {
        Some(dir) => Some(expand_dir(dir)?),
        None => Some(
            std::env::current_dir()
                .map_err(|e| format!("cannot determine current directory: {}", e))?
                .to_string_lossy()
                .to_string(),
        ),
    };
    Ok(parsed)
}

/// `--help` output.
pub fn usage() -> String {
    spec::usage()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::spec;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn cli_defaults_to_current_directory_without_explicit_open() {
        let options = parse_cli(&[]).unwrap();
        assert!(!options.requested_dir);
        assert_eq!(
            options.start_dir,
            Some(
                std::env::current_dir()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            )
        );
    }

    #[test]
    fn cli_rejects_missing_directories_and_supports_help() {
        assert!(parse_cli(&args(&["/definitely/not/a/dir"])).is_err());
        assert!(parse_cli(&args(&["--help"])).unwrap().help);
    }

    #[test]
    fn cli_parses_modern_options() {
        let o = parse_cli(&args(&[
            "-T",
            "My Term",
            "--geometry",
            "120x40",
            "--fullscreen",
            "--hold",
        ]))
        .unwrap();
        assert_eq!(o.title.as_deref(), Some("My Term"));
        assert_eq!(o.geometry, Some((120, 40)));
        assert!(o.fullscreen);
        assert!(o.hold);
        assert!(!o.maximize);
        assert!(parse_cli(&args(&["-V"])).unwrap().version);
    }

    #[test]
    fn cli_parses_all_window_and_command_options() {
        let o = parse_cli(&args(&[
            "-w",
            "/tmp",
            "--new-window",
            "--no-restore",
            "--hold",
            "--execute",
            "sh",
            "-c",
            "printf '%s' ok",
        ]))
        .unwrap();
        assert_eq!(o.start_dir.as_deref(), Some("/tmp"));
        assert!(o.requested_dir);
        assert!(o.new_window);
        assert!(o.no_restore);
        assert!(o.hold);
        assert_eq!(o.execute, Some(args(&["sh", "-c", "printf '%s' ok"])));

        let o = parse_cli(&args(&[
            "--working-directory",
            "/tmp",
            "-e",
            "printf",
            "%s",
            "ok",
        ]))
        .unwrap();
        assert_eq!(o.start_dir.as_deref(), Some("/tmp"));
        assert_eq!(o.execute, Some(args(&["printf", "%s", "ok"])));
    }

    #[test]
    fn cli_parses_short_aliases() {
        let o = parse_cli(&args(&["-F"])).unwrap();
        assert!(o.fullscreen);
        assert!(!o.maximize);

        let o = parse_cli(&args(&["-m"])).unwrap();
        assert!(!o.fullscreen);
        assert!(o.maximize);

        let o = parse_cli(&args(&["-g", "120X40"])).unwrap();
        assert_eq!(o.geometry, Some((120, 40)));
    }

    #[test]
    fn cli_requires_arguments_for_value_options() {
        for option in [
            "-w",
            "--working-directory",
            "-T",
            "--title",
            "-g",
            "--geometry",
            "--class",
            "--name",
            "-p",
            "--profile",
            "--config",
            "-o",
            "--option",
            "--font",
            "--font-size",
            "-e",
            "--execute",
        ] {
            assert!(
                parse_cli(&args(&[option])).is_err(),
                "{} accepts no value",
                option
            );
        }
    }

    #[test]
    fn every_documented_window_option_is_understood() {
        // Session options are handled by `remote::parse_cli_mode` before this
        // parser, so only the Window group is checked here.
        for option in spec::CLI
            .iter()
            .filter(|option| option.group == spec::CliGroup::Window)
        {
            if matches!(option.long, "help" | "version") {
                continue;
            }
            let mut argv = vec![format!("--{}", option.long)];
            if option.value.is_some() {
                // Missing value must be an error rather than a silent flag.
                let missing = parse_cli(&argv);
                if option.long != "execute" {
                    assert!(
                        missing.is_err(),
                        "--{} swallowed a missing value",
                        option.long
                    );
                }
                argv.push(match (option.long, option.kind) {
                    ("geometry", _) => "120x40".to_string(),
                    (_, spec::ValueKind::Number) => "12".to_string(),
                    (_, spec::ValueKind::KeyValue) => "font_size=12".to_string(),
                    (_, spec::ValueKind::Dir) => "/tmp".to_string(),
                    _ => "value".to_string(),
                });
            }
            let parsed = parse_cli(&argv);
            assert!(
                parsed.is_ok(),
                "--{} is documented but rejected: {:?}",
                option.long,
                parsed.err()
            );
        }
    }

    #[test]
    fn cli_geometry_validation() {
        assert!(parse_cli(&args(&["--geometry", "abc"])).is_err());
        assert!(parse_cli(&args(&["--geometry", "0x10"])).is_err());
        assert!(parse_cli(&args(&["--geometry", "1001x40"])).is_err());
        assert!(parse_cli(&args(&["--geometry", "80"])).is_err());
        assert_eq!(parse_geometry("100X30").unwrap(), (100, 30));
    }

    #[test]
    fn cli_fullscreen_and_maximize_conflict() {
        assert!(parse_cli(&args(&["--fullscreen", "--maximize"])).is_err());
    }

    #[test]
    fn cli_double_dash_forces_directory() {
        let o = parse_cli(&args(&["--", "/tmp"])).unwrap();
        assert!(o.requested_dir);
        assert_eq!(o.start_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn cli_parses_class_name_profile_config() {
        let o = parse_cli(&args(&[
            "--class",
            "MyClass",
            "--name",
            "inst",
            "--profile",
            "work",
            "--config",
            "/tmp/alt.json",
        ]))
        .unwrap();
        assert_eq!(o.wm_class.as_deref(), Some("MyClass"));
        assert_eq!(o.wm_name.as_deref(), Some("inst"));
        assert_eq!(o.profile.as_deref(), Some("work"));
        assert_eq!(o.config.as_deref(), Some("/tmp/alt.json"));
    }

    #[test]
    fn cli_parses_option_overrides_with_types() {
        let o = parse_cli(&args(&[
            "-o",
            "opacity=0.9",
            "-o",
            "login_shell=false",
            "--font",
            "Fira Code",
            "--font-size",
            "16",
        ]))
        .unwrap();
        assert_eq!(o.options.len(), 4);
        assert_eq!(o.options[0].0, "opacity");
        assert_eq!(o.options[0].1, serde_json::json!(0.9));
        assert_eq!(o.options[1].1, Value::Bool(false));
        assert_eq!(
            o.options[2],
            (
                "font_name".to_string(),
                Value::String("Fira Code".to_string())
            )
        );
        assert_eq!(o.options[3], ("font_size".to_string(), Value::from(16)));
    }

    #[test]
    fn cli_option_and_font_size_validation() {
        assert!(parse_cli(&args(&["-o", "noequals"])).is_err());
        assert!(parse_cli(&args(&["-o", "=novalue"])).is_err());
        assert!(parse_cli(&args(&["--font-size", "big"])).is_err());
        assert!(parse_cli(&args(&["--font-size", "3"])).is_err());
        assert!(parse_cli(&args(&["--font-size", "129"])).is_err());
        assert_eq!(parse_override_value("42"), Value::from(42));
        assert_eq!(parse_override_value("true"), Value::Bool(true));
        assert_eq!(
            parse_override_value("hello"),
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn inspection_commands_do_not_need_a_directory() {
        let o = parse_cli(&args(&["--list-themes"])).unwrap();
        assert_eq!(o.inspection(), Some(Inspection::ListThemes));
        assert_eq!(o.start_dir, None);
        assert_eq!(
            parse_cli(&args(&["--show-config"])).unwrap().inspection(),
            Some(Inspection::ShowConfig { defaults: false })
        );
        assert_eq!(
            parse_cli(&args(&["--show-config", "--default"]))
                .unwrap()
                .inspection(),
            Some(Inspection::ShowConfig { defaults: true })
        );
        assert_eq!(
            parse_cli(&args(&["--validate-config", "/tmp/x.json"]))
                .unwrap()
                .inspection(),
            Some(Inspection::ValidateConfig(Some("/tmp/x.json".to_string())))
        );
        assert_eq!(
            parse_cli(&args(&["--validate-config"]))
                .unwrap()
                .inspection(),
            Some(Inspection::ValidateConfig(None))
        );
    }

    #[test]
    fn inspection_commands_are_exclusive_and_default_needs_show_config() {
        let error = parse_cli(&args(&["--list-themes", "--list-settings"])).unwrap_err();
        assert!(error.contains("one inspection option"));
        assert!(parse_cli(&args(&["--default"])).is_err());
    }
}
