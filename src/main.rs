#![recursion_limit = "256"]

mod control;
mod history;
mod icons;
mod logging;
mod notes;
mod notes_index;
mod notes_panel;
mod notes_ui;
mod persistence;
mod profiles;
mod remote;
mod session;
mod settings;
mod settings_dialog;
mod system_stats;
mod terminal;
mod window;

// The headless core lives in the library target so the docgen tool, the tests
// and the GUI share one implementation. Re-export it at the crate root so the
// GUI modules can keep using `crate::headless::...`.
pub use oxterm::headless;

use glib::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

use oxterm::headless::{cli, control_cli, diagnostics, settings as headless_settings, spec};

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

/// Schedule `f` on the GTK main loop from any thread. Unlike
/// `MainContext::invoke`, this never runs `f` synchronously on the calling
/// thread, so GTK objects are only touched from the main thread.
pub fn run_on_main<F: FnOnce() + Send + 'static>(f: F) {
    glib::timeout_add_once(std::time::Duration::ZERO, f);
}

/// Pop up a menu built for a single use and destroy it once it closes, so it
/// (and the widgets its item closures capture) is not leaked.
pub fn popup_transient_menu(menu: &gtk::Menu) {
    menu.connect_deactivate(|menu| {
        // Item "activate" handlers run after "deactivate"; destroy afterwards.
        let menu = menu.clone();
        glib::idle_add_local_once(move || unsafe { menu.destroy() });
    });
    menu.popup_at_pointer(None);
}

/// Move a non-Send value into a worker thread; it is only ever touched again
/// on the main thread.
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}

struct App {
    app: gtk::Application,
}

impl App {
    fn new() -> App {
        let app = gtk::Application::new(
            Some("com.buzzqw.oxterm"),
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE,
        );
        glib::set_prgname(Some(window::APP_NAME));
        glib::set_application_name(window::APP_TITLE);

        let _settings = settings();
        notes::migrate_legacy_notes_dir();

        app.connect_startup(|_app| {
            // Follow the desktop's light/dark preference. Emitting the settings
            // signal makes every terminal recompute its theme when the desktop
            // switches appearance at sunset (or from the control centre).
            if let Some(gtk_settings) = gtk::Settings::default() {
                gtk_settings.connect_gtk_application_prefer_dark_theme_notify(|_| {
                    settings().notify_changed();
                });
            }

            let css_provider = gtk::CssProvider::new();
            let css = b"
                vte-terminal { padding-left: 8px; padding-right: 4px; }
                .oxterm-notes-panel { border-left: 1px solid alpha(currentColor, 0.2); }
                .oxterm-menu-row { background: alpha(@theme_fg_color, 0.05); padding: 1px 4px; }                .oxterm-menu-row button { padding: 3px 10px; }
                .command-bar-frame { border: 1px solid alpha(currentColor, 0.3); background: alpha(@theme_bg_color, 0.95); }
                .command-bar-frame entry { padding: 6px 10px; font-family: Monospace; }
                .command-bar-frame list row { padding: 2px 10px; }
                .command-bar-frame list row:selected { background: @theme_selected_bg_color; }
                popover { background-color: @theme_bg_color; }
                popover contents modelbutton { color: @theme_fg_color; padding: 8px 12px; min-height: 24px; }
                .oxterm-tab-menu { min-width: 220px; }
                .oxterm-tab-menu menuitem { padding: 6px 12px; min-height: 24px; }
                .oxterm-stats-label { font-size: 0.85em; font-family: Monospace; color: alpha(@theme_fg_color, 0.6); background: alpha(@theme_bg_color, 0.5); padding: 2px 12px; }
                .oxterm-hint-label { background: #fce94f; color: #000000; font-family: Monospace; font-weight: bold; font-size: 0.85em; padding: 1px 3px; border-radius: 2px; }
                .oxterm-sticky-header { background: alpha(@theme_bg_color, 0.94); border-bottom: 1px solid alpha(currentColor, 0.25); padding: 2px 8px; font-family: Monospace; font-size: 0.88em; }
                .oxterm-sticky-ok { color: #4faa5a; }
                .oxterm-sticky-fail { color: #cc4b4b; }
                .oxterm-sticky-run { color: #c9971f; }
            ";
            let _ = css_provider.load_from_data(css);
            if let Some(screen) = gdk::Screen::default() {
                gtk::StyleContext::add_provider_for_screen(
                    &screen,
                    &css_provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        });

        App { app }
    }

    fn run(&mut self, args: Vec<String>) -> i32 {
        let app = self.app.clone();
        app.connect_command_line(move |app, cmdline| {
            let args: Vec<String> = cmdline
                .arguments()
                .iter()
                .skip(1)
                .map(|s| s.to_string_lossy().to_string())
                .collect();
            let opts = match cli::parse_cli(&args) {
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
                print!("{}", cli::usage());
                return 0;
            }
            if let Some(inspection) = opts.inspection() {
                return run_inspection(&opts, inspection);
            }
            // Apply command-line configuration before any window (and therefore
            // any settings read) is created: alternative config file first, then
            // the session-only overrides (`-o`, `--font`, `--font-size`) and the
            // requested profile. None of these are written back to disk.
            if let Some(cfg) = opts.config.as_ref() {
                settings::set_config_file_override(std::path::PathBuf::from(cfg));
            }
            for (k, v) in &opts.options {
                if let Err(error) = settings::set_override(k, v.clone()) {
                    eprintln!("{}: {}", window::APP_NAME, error);
                    return 2;
                }
            }
            if let Some(name) = opts.profile.as_ref() {
                match crate::profiles::load_profile(name) {
                    Some(serde_json::Value::Object(map)) => {
                        for (k, v) in map {
                            if let Err(error) = settings::set_override(&k, v) {
                                eprintln!(
                                    "{}: invalid profile {}: {}",
                                    window::APP_NAME,
                                    name,
                                    error
                                );
                                return 2;
                            }
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
            #[cfg(debug_assertions)]
            if let Some(scenario) = std::env::var_os("OXTERM_TEST_SCENARIO") {
                window::test_scenario::schedule(app, &win, scenario.to_string_lossy().into_owned());
            }
            0
        });
        // With HANDLES_COMMAND_LINE every launch goes through the
        // command-line handler above; "activate" is never emitted.

        // Logout and `kill` send SIGTERM/SIGHUP: close windows cleanly so the
        // session is saved and this process's temporary files are removed.
        for signal in [libc::SIGTERM, libc::SIGHUP] {
            let app = app.clone();
            glib::unix_signal_add_local(signal, move || {
                for window in app.windows() {
                    if let Some(win) = window.downcast_ref::<MainWindow>() {
                        win.shutdown();
                    } else if let Some(win) = window.downcast_ref::<window::DetachedWindow>() {
                        win.shutdown();
                    }
                }
                app.quit();
                glib::ControlFlow::Break
            });
        }
        control::init(app.clone());
        control::start_server();
        let code = self.app.run_with_args(&args).value();
        #[cfg(debug_assertions)]
        if window::test_scenario::failed() {
            return 1;
        }
        code
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    logging::configure_logging();
    if let Some(mode) = remote::parse_cli_mode(&args) {
        let code = match mode {
            Ok(mode) => remote::run_cli(mode),
            Err(error) => {
                eprintln!("oxterm: {}", error);
                2
            }
        };
        std::process::exit(code);
    }
    if let Some(mode) = control::parse_cli_mode(&args) {
        let code = match mode {
            Ok(mode) => control::run_cli(mode),
            Err(error) => {
                eprintln!("oxterm: {}", error);
                2
            }
        };
        std::process::exit(code);
    }
    // Help, version and the inspection commands are answered before GTK is
    // even initialised, so they work without a display and never touch the
    // user's settings file unless asked to.
    let parsed = match cli::parse_cli(&args[1..]) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{}: {}", window::APP_NAME, error);
            std::process::exit(2);
        }
    };
    if parsed.help {
        print!("{}", cli::usage());
        std::process::exit(0);
    }
    if parsed.version {
        println!("{} {}", window::APP_NAME, window::VERSION);
        std::process::exit(0);
    }
    if let Some(inspection) = parsed.inspection() {
        std::process::exit(run_inspection(&parsed, inspection));
    }
    let mut app = App::new();
    let code = app.run(args);
    std::process::exit(code);
}

/// Settings file that `--show-config` / `--validate-config` should use.
fn inspection_config_path(opts: &cli::CliOptions) -> std::path::PathBuf {
    opts.config
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(settings::config_file)
}

fn run_inspection(opts: &cli::CliOptions, inspection: cli::Inspection) -> i32 {
    match inspection {
        cli::Inspection::ShowConfig { defaults } => show_config(opts, defaults),
        cli::Inspection::ValidateConfig(file) => validate_config(opts, file),
        cli::Inspection::ListSettings => list_settings(),
        cli::Inspection::ListThemes => list_themes(),
        cli::Inspection::ListKeybinds => {
            print!("{}", spec::keybinds());
            0
        }
        cli::Inspection::ListActions => list_actions(),
        cli::Inspection::Diagnostics => diagnostics_report(opts),
        cli::Inspection::EditConfig => edit_config(opts),
        cli::Inspection::ListFonts => list_fonts(),
    }
}

fn show_config(opts: &cli::CliOptions, defaults_only: bool) -> i32 {
    let mut effective = if defaults_only {
        spec::defaults()
    } else {
        let path = inspection_config_path(opts);
        if !path.exists() {
            spec::defaults()
        } else {
            match headless_settings::load_file(&path) {
                Ok(loaded) => {
                    for issue in &loaded.issues {
                        eprintln!(
                            "{}: {}: {}: {}",
                            window::APP_NAME,
                            path.display(),
                            issue.key,
                            issue.message
                        );
                    }
                    loaded.settings
                }
                Err(error) => {
                    eprintln!("{}: {}: {}", window::APP_NAME, path.display(), error);
                    return 2;
                }
            }
        }
    };
    let issues = headless_settings::apply_overrides(&mut effective, &opts.options);
    for issue in &issues {
        eprintln!("{}: -o {}: {}", window::APP_NAME, issue.key, issue.message);
    }
    match serde_json::to_string_pretty(&effective) {
        Ok(text) => {
            println!("{}", text);
            0
        }
        Err(error) => {
            eprintln!("{}: {}", window::APP_NAME, error);
            2
        }
    }
}

fn validate_config(opts: &cli::CliOptions, file: Option<String>) -> i32 {
    let path = file
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| inspection_config_path(opts));
    let raw = match headless_settings::read_document(&path) {
        Ok(raw) => raw,
        Err(error) => {
            eprintln!("{}: {}: {}", window::APP_NAME, path.display(), error);
            return 2;
        }
    };
    match headless_settings::load_document(&raw) {
        Ok(loaded) => {
            let mut fatal = 0;
            for issue in &loaded.issues {
                if issue.fatal {
                    fatal += 1;
                }
                println!(
                    "{}: {}: {}: {}",
                    path.display(),
                    if issue.fatal { "error" } else { "warning" },
                    issue.key,
                    issue.message
                );
            }
            if fatal > 0 {
                eprintln!(
                    "{}: {}: {} invalid setting(s)",
                    window::APP_NAME,
                    path.display(),
                    fatal
                );
                2
            } else {
                println!("{}: ok", path.display());
                0
            }
        }
        Err(error) => {
            eprintln!("{}: {}: {}", window::APP_NAME, path.display(), error);
            2
        }
    }
}

fn list_settings() -> i32 {
    println!("{:<28} {:<22} {:<22} DESCRIPTION", "KEY", "TYPE", "DEFAULT");
    for setting in spec::SETTINGS {
        let default = setting.default_value();
        let default = match &default {
            serde_json::Value::String(value) => format!("\"{}\"", value),
            other => other.to_string(),
        };
        println!(
            "{:<28} {:<22} {:<22} {}",
            setting.key,
            setting.kind.type_name(),
            default,
            setting.description
        );
        if !setting.choices.is_empty() {
            println!(
                "{:<28} {:<22} {:<22} values: {}",
                "",
                "",
                "",
                setting.choices.join(", ")
            );
        }
    }
    0
}

fn list_themes() -> i32 {
    let width = settings::color_schemes()
        .keys()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(20)
        .clamp(20, 40);
    println!("{:<width$} {:<9} {}", "THEME", "FOREGROUND", "BACKGROUND");
    for (name, theme) in settings::color_schemes() {
        println!(
            "{:<width$} {:<9} {}",
            name, theme.foreground, theme.background
        );
    }
    println!(
        "\n{} themes built in. Extra Ghostty/iTerm2 files in {} are loaded as well.",
        settings::color_schemes().len(),
        oxterm::headless::themes::themes_dir().display()
    );
    0
}

fn list_actions() -> i32 {
    println!("Control API (run inside a pane, or pass --pane ID):");
    print!("{}", control_cli::summary());
    println!("\nInspection actions (no window is opened):");
    let options: Vec<&spec::CliOption> = spec::CLI
        .iter()
        .filter(|option| option.group == spec::CliGroup::Inspect)
        .collect();
    let width = options
        .iter()
        .map(|option| option.names().len())
        .max()
        .unwrap_or(0);
    for option in options {
        println!("  {:<width$}  {}", option.names(), option.description);
    }
    println!("\nRun `oxterm <action> --help` for the details of a control action.");
    0
}

fn edit_config(opts: &cli::CliOptions) -> i32 {
    let path = inspection_config_path(opts);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::set_permissions(
                parent,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            );
        }
        let defaults =
            serde_json::to_string_pretty(&spec::defaults()).unwrap_or_else(|_| "{}\n".to_string());
        if let Err(error) = std::fs::write(&path, defaults + "\n") {
            eprintln!("{}: {}: {}", window::APP_NAME, path.display(), error);
            return 2;
        }
        let _ =
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    }
    let editor = configured_editor(&path);
    let parts = shell_words::split(&editor).unwrap_or_default();
    let Some((program, arguments)) = parts.split_first() else {
        eprintln!("{}: editor command is empty", window::APP_NAME);
        return 2;
    };
    match std::process::Command::new(program)
        .args(arguments)
        .arg(&path)
        .status()
    {
        Ok(status) => {
            if status.success() {
                0
            } else {
                status.code().unwrap_or(1)
            }
        }
        Err(error) => {
            eprintln!(
                "{}: cannot start editor {:?}: {}",
                window::APP_NAME,
                program,
                error
            );
            2
        }
    }
}

/// `editor_command` from the settings file, then `$EDITOR`, then a safe default.
fn configured_editor(path: &std::path::Path) -> String {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&text) {
            if let Some(command) = map.get("editor_command").and_then(|value| value.as_str()) {
                if !command.trim().is_empty() {
                    return command.to_string();
                }
            }
        }
    }
    std::env::var("EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "nano".to_string())
}

fn list_fonts() -> i32 {
    let output = std::process::Command::new("fc-list")
        .args([":", "family"])
        .output();
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "{}: fc-list failed: {}",
                window::APP_NAME,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return 1;
        }
        Err(error) => {
            eprintln!(
                "{}: cannot run fc-list (is fontconfig installed?): {}",
                window::APP_NAME,
                error
            );
            return 1;
        }
    };
    let mut families = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        for family in line.split(',') {
            let family = family.trim();
            if !family.is_empty() {
                families.insert(family.to_string());
            }
        }
    }
    for family in families {
        println!("{}", family);
    }
    0
}

fn diagnostics_report(opts: &cli::CliOptions) -> i32 {
    let mut effective = spec::defaults();
    let issues = headless_settings::apply_overrides(&mut effective, &opts.options);
    for issue in &issues {
        eprintln!("{}: -o {}: {}", window::APP_NAME, issue.key, issue.message);
    }
    let configured_notes = effective
        .get("notes_dir")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let notes_dir = if configured_notes.is_empty() {
        notes::default_notes_dir()
    } else {
        std::path::PathBuf::from(configured_notes)
    };
    let overrides: Vec<String> = opts.options.iter().map(|(key, _)| key.clone()).collect();
    let mut extra = Vec::new();
    if let Some(profile) = &opts.profile {
        extra.push(("profile".to_string(), profile.clone()));
    }
    if let Some(config) = &opts.config {
        extra.push(("config-override".to_string(), config.clone()));
    }
    extra.push((
        "themes".to_string(),
        format!("{} installed", settings::color_schemes().len()),
    ));
    extra.push((
        "themes-dir".to_string(),
        oxterm::headless::themes::themes_dir().display().to_string(),
    ));
    let report = diagnostics::Report {
        version: window::VERSION,
        build: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        rustc: env!("OXTERM_BUILD_RUSTC"),
        config_file: &settings::config_file(),
        config_dir: &settings::config_dir(),
        remote_dir: Some(&remote::remote_dir()),
        notes_dir: Some(&notes_dir),
        overrides: &overrides,
        extra: &extra,
    };
    print!("{}", diagnostics::render(&report));
    if issues.is_empty() {
        0
    } else {
        2
    }
}
