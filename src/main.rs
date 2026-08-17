#![recursion_limit = "256"]

mod ai_client;
mod history;
mod icons;
mod logging;
mod notes;
mod persistence;
mod profiles;
mod session;
mod settings;
mod settings_dialog;
mod system_stats;
mod terminal;
mod window;

use glib::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

use crate::settings::settings;
use crate::window::MainWindow;

/// A weak reference wrapper that can cross threads. GTK objects are only
/// ever upgraded and used on the main thread, so carrying the `WeakRef`
/// across to be re-upgraded there is safe.
#[derive(Clone)]
pub struct SendWeak<T: glib::IsA<glib::Object>>(glib::WeakRef<T>);

impl<T: glib::IsA<glib::Object>> SendWeak<T> {
    pub fn new(w: &T) -> Self {
        SendWeak(w.downgrade())
    }
    pub fn upgrade(&self) -> Option<T> {
        self.0.upgrade()
    }
}

unsafe impl<T: glib::IsA<glib::Object>> Send for SendWeak<T> {}
unsafe impl<T: glib::IsA<glib::Object>> Sync for SendWeak<T> {}

/// Move a non-Send value into a worker thread; it is only ever touched again
/// on the main thread.
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}

struct CliOptions {
    start_dir: Option<String>,
    requested_dir: bool,
    new_window: bool,
    no_restore: bool,
    execute: Option<Vec<String>>,
    title: Option<String>,
    hold: bool,
    fullscreen: bool,
    maximize: bool,
    geometry: Option<(i32, i32)>,
    /// `--class` / `--name`: WM_CLASS class and instance name for the window,
    /// used by window managers for tiling rules, icons and .desktop matching.
    wm_class: Option<String>,
    wm_name: Option<String>,
    /// `-p/--profile`: profile to apply on startup (session-only).
    profile: Option<String>,
    /// `--config`: alternative settings file.
    config: Option<String>,
    /// `-o/--option key=value`: session-only settings overrides.
    options: Vec<(String, serde_json::Value)>,
    version: bool,
    help: bool,
}

/// Turn the right-hand side of `-o key=value` into a JSON value, matching the
/// type users expect: booleans, integers and floats are parsed as such, and
/// anything else stays a string (e.g. `opacity=0.9`, `login_shell=false`).
fn parse_override_value(raw: &str) -> serde_json::Value {
    match raw {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = raw.parse::<i64>() {
        return serde_json::json!(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return serde_json::json!(f);
    }
    serde_json::Value::String(raw.to_string())
}

/// Parse a `key=value` override string used by `-o/--option`, `--font` and
/// `--font-size`.
fn parse_option_kv(s: &str) -> Result<(String, serde_json::Value), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid option '{}': expected key=value", s))?;
    let key = k.trim();
    if key.is_empty() {
        return Err(format!("invalid option '{}': empty key", s));
    }
    Ok((key.to_string(), parse_override_value(v)))
}

/// Parse a `COLSxROWS` geometry string (e.g. `120x40`), as used by many
/// modern terminals. Columns and rows must both be positive integers.
fn parse_geometry(s: &str) -> Result<(i32, i32), String> {
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
    if cols < 1 || rows < 1 {
        return Err(format!("invalid geometry '{}': columns and rows must be positive", s));
    }
    Ok((cols, rows))
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
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
    let mut options: Vec<(String, serde_json::Value)> = Vec::new();
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
                options.push(("font_name".to_string(), serde_json::Value::String(args[i].clone())));
            }
            "--font-size" => {
                i += 1;
                if i >= args.len() {
                    return Err("--font-size requires an argument".into());
                }
                let sz: i64 = args[i]
                    .parse()
                    .map_err(|_| format!("invalid --font-size '{}': expected a number", args[i]))?;
                options.push(("font_size".to_string(), serde_json::json!(sz)));
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
    start_dir = match start_dir {
        Some(dir) => Some(expand_dir(dir)?),
        None => Some(
            std::env::current_dir()
                .map_err(|e| format!("cannot determine current directory: {}", e))?
                .to_string_lossy()
                .to_string(),
        ),
    };
    if fullscreen && maximize {
        return Err("--fullscreen and --maximize cannot be used together".into());
    }
    Ok(CliOptions {
        start_dir,
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
        version,
        help,
    })
}

fn usage() -> &'static str {
    concat!(
        "Usage: terust [DIRECTORY] [OPTIONS] [-e CMD...]\n\n",
        "Options:\n",
        "  -w, --working-directory DIR  Working directory for the new terminal\n",
        "  -T, --title TITLE            Set a fixed window title (apps cannot override it)\n",
        "  -g, --geometry COLSxROWS     Initial size in character cells (e.g. 120x40)\n",
        "  -F, --fullscreen             Start in fullscreen mode\n",
        "  -m, --maximize               Start maximized\n",
        "      --class CLASS            Set the WM_CLASS class part (window manager rules)\n",
        "      --name NAME             Set the WM_CLASS instance name\n",
        "  -p, --profile NAME          Start with a saved profile (session only)\n",
        "      --config FILE           Use an alternative settings file\n",
        "  -o, --option KEY=VALUE      Override a setting for this session (repeatable)\n",
        "      --font FAMILY           Override the font family for this session\n",
        "      --font-size N           Override the font size for this session\n",
        "      --new-window             Open a new window\n",
        "      --no-restore             Do not restore the last session\n",
        "      --hold                   Keep the terminal open after the command exits\n",
        "  -e, --execute CMD...         Run a command instead of the configured shell\n",
        "      --                       Treat every following argument as the directory\n",
        "  -V, --version                Show the terust version\n",
        "  -h, --help                   Show this help\n",
    )
}

struct App {
    app: gtk::Application,
    current_dir: Option<String>,
}

impl App {
    fn new() -> App {
        let app = gtk::Application::new(
            Some("com.buzzqw.terust"),
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE,
        );
        let _ = glib::set_prgname(Some(window::APP_NAME));
        let _ = glib::set_application_name(window::APP_TITLE);

        let _settings = settings();

        app.connect_startup(|app| {
            let css_provider = gtk::CssProvider::new();
            let css = b"
                vte-terminal { padding-left: 8px; padding-right: 4px; }
                .tpgk-menu-row { background: alpha(@theme_fg_color, 0.05); padding: 1px 4px; }
                .tpgk-menu-row button { padding: 3px 10px; }
                .command-bar-frame { border: 1px solid alpha(currentColor, 0.3); background: alpha(@theme_bg_color, 0.95); }
                .command-bar-frame entry { padding: 6px 10px; font-family: Monospace; }
                .command-bar-frame list row { padding: 2px 10px; }
                .command-bar-frame list row:selected { background: @theme_selected_bg_color; }
                popover { background-color: @theme_bg_color; }
                popover contents modelbutton { color: @theme_fg_color; padding: 8px 12px; min-height: 24px; }
                .tpgk-tab-menu { min-width: 220px; }
                .tpgk-tab-menu menuitem { padding: 6px 12px; min-height: 24px; }
                .tpgk-stats-label { font-size: 0.85em; font-family: Monospace; color: alpha(@theme_fg_color, 0.6); background: alpha(@theme_bg_color, 0.5); padding: 2px 12px; }
                .tpgk-hint-label { background: #fce94f; color: #000000; font-family: Monospace; font-weight: bold; font-size: 0.85em; padding: 1px 3px; border-radius: 2px; }
            ";
            let _ = css_provider.load_from_data(css);
            if let Some(screen) = gdk::Screen::default() {
                gtk::StyleContext::add_provider_for_screen(
                    &screen,
                    &css_provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            app.connect_activate(|_app| {
                // handled via command_line
            });
        });

        App {
            app,
            current_dir: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
        }
    }

    fn run(&mut self, args: Vec<String>) -> i32 {
        let app = self.app.clone();
        if let Ok(options) = parse_cli(&args[1..]) {
            if options.help {
                print!("{}", usage());
                return 0;
            }
            if options.version {
                println!("{} {}", window::APP_NAME, window::VERSION);
                return 0;
            }
        }
        app.connect_command_line(move |app, cmdline| {
            let args: Vec<String> = cmdline
                .arguments()
                .iter()
                .skip(1)
                .map(|s| s.to_string_lossy().to_string())
                .collect();
            let opts = match parse_cli(&args) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("{}: {}", window::APP_NAME, e);
                    return 2;
                }
            };
            if opts.version {
                println!("{} {}", window::APP_NAME, window::VERSION);
                return 0;
            }
            if opts.help {
                print!("{}", usage());
                return 0;
            }
            // Apply command-line configuration before any window (and therefore
            // any settings read) is created: alternative config file first, then
            // the session-only overrides (`-o`, `--font`, `--font-size`) and the
            // requested profile. None of these are written back to disk.
            if let Some(cfg) = opts.config.as_ref() {
                settings::set_config_file_override(std::path::PathBuf::from(cfg));
            }
            for (k, v) in &opts.options {
                settings::set_override(k, v.clone());
            }
            if let Some(name) = opts.profile.as_ref() {
                match crate::profiles::load_profile(name) {
                    Some(serde_json::Value::Object(map)) => {
                        for (k, v) in map {
                            settings::set_override(&k, v);
                        }
                    }
                    _ => eprintln!("{}: profile not found: {}", window::APP_NAME, name),
                }
            }
            let explicit = opts.requested_dir
                || opts.execute.is_some()
                || opts.new_window
                || opts.title.is_some()
                || opts.geometry.is_some()
                || opts.fullscreen
                || opts.maximize
                || opts.hold;
            let windows = app.windows();
            if !windows.is_empty() && !explicit {
                windows[0].present();
                return 0;
            }
            let win = MainWindow::new(
                Some(app),
                opts.start_dir,
                opts.execute,
                !(opts.no_restore || explicit),
                window::WindowOptions {
                    title: opts.title,
                    hold: opts.hold,
                    fullscreen: opts.fullscreen,
                    maximize: opts.maximize,
                    geometry: opts.geometry,
                    wm_class: opts.wm_class,
                    wm_name: opts.wm_name,
                },
            );
            app.add_window(&win);
            win.present();
            0
        });
        let first_win = self.current_dir.clone();
        app.connect_activate(move |app| {
            let windows = app.windows();
            if !windows.is_empty() {
                windows[0].present();
                return;
            }
            let win = MainWindow::new(
                Some(app),
                first_win.clone(),
                None,
                true,
                window::WindowOptions::default(),
            );
            app.add_window(&win);
            win.present();
        });
        self.app.run_with_args(&args).value()
    }
}

fn expand_dir(d: String) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn cli_geometry_validation() {
        assert!(parse_cli(&args(&["--geometry", "abc"])).is_err());
        assert!(parse_cli(&args(&["--geometry", "0x10"])).is_err());
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
        assert_eq!(o.options[1].1, serde_json::Value::Bool(false));
        assert_eq!(o.options[2], ("font_name".to_string(), serde_json::json!("Fira Code")));
        assert_eq!(o.options[3], ("font_size".to_string(), serde_json::json!(16)));
    }

    #[test]
    fn cli_option_and_font_size_validation() {
        assert!(parse_cli(&args(&["-o", "noequals"])).is_err());
        assert!(parse_cli(&args(&["-o", "=novalue"])).is_err());
        assert!(parse_cli(&args(&["--font-size", "big"])).is_err());
        assert_eq!(parse_override_value("42"), serde_json::json!(42));
        assert_eq!(parse_override_value("true"), serde_json::Value::Bool(true));
        assert_eq!(
            parse_override_value("hello"),
            serde_json::Value::String("hello".to_string())
        );
    }
}
fn main() {
    logging::configure_logging();
    let args: Vec<String> = std::env::args().collect();
    let mut app = App::new();
    let code = app.run(args);
    std::process::exit(code);
}
