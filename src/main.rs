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
    version: bool,
    help: bool,
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut start_dir: Option<String> = None;
    let mut requested_dir = false;
    let mut new_window = false;
    let mut no_restore = false;
    let mut execute: Option<Vec<String>> = None;
    let mut version = false;
    let mut help = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
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
            "--version" => version = true,
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
    Ok(CliOptions {
        start_dir,
        requested_dir,
        new_window,
        no_restore,
        execute,
        version,
        help,
    })
}

fn usage() -> &'static str {
    "Usage: terust [DIRECTORY] [OPTIONS]\n\nOptions:\n  -w, --working-directory DIR  Working directory for the new terminal\n      --new-window             Open a new window\n      --no-restore              Do not restore the last session\n  -e, --execute CMD...          Run a command instead of the configured shell\n      --version                 Show the terust version\n  -h, --help                   Show this help\n"
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
            let explicit = opts.requested_dir || opts.execute.is_some() || opts.new_window;
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
            let win = MainWindow::new(Some(app), first_win.clone(), None, true);
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
}
fn main() {
    logging::configure_logging();
    let args: Vec<String> = std::env::args().collect();
    let mut app = App::new();
    let code = app.run(args);
    std::process::exit(code);
}
