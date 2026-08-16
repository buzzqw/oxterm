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
    new_window: bool,
    no_restore: bool,
    execute: Option<Vec<String>>,
    version: bool,
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut start_dir: Option<String> = None;
    let mut new_window = false;
    let mut no_restore = false;
    let mut execute: Option<Vec<String>> = None;
    let mut version = false;
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
            _ => {
                if arg.starts_with('-') {
                    return Err(format!("Unknown option: {}", arg));
                }
                if start_dir.is_some() {
                    return Err("directory and --working-directory cannot be used together".into());
                }
                start_dir = Some(arg.clone());
            }
        }
        i += 1;
    }
    Ok(CliOptions {
        start_dir,
        new_window,
        no_restore,
        execute,
        version,
    })
}

struct App {
    app: gtk::Application,
    current_dir: Option<String>,
}

impl App {
    fn new() -> App {
        let app = gtk::Application::new(
            Some("com.buzzqw.tpgk"),
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE,
        );
        let _ = glib::set_prgname(Some("tpgk"));
        let _ = glib::set_application_name("TPGK Terminal");

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
            current_dir: None,
        }
    }

    fn run(&mut self, args: Vec<String>) -> i32 {
        let app = self.app.clone();
        if let Ok(options) = parse_cli(&args[1..]) {
            if options.version {
                println!("TPGK {}", window::VERSION);
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
                    eprintln!("tpgk: {}", e);
                    return 2;
                }
            };
            if opts.version {
                println!("TPGK {}", window::VERSION);
                return 0;
            }
            let explicit = opts.start_dir.is_some() || opts.execute.is_some() || opts.new_window;
            let windows = app.windows();
            if !windows.is_empty() && !explicit {
                windows[0].present();
                return 0;
            }
            let start_dir = opts.start_dir.map(expand_dir);
            let win = MainWindow::new(
                Some(app),
                start_dir,
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

fn expand_dir(d: String) -> String {
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
            .unwrap_or(expanded)
    } else {
        eprintln!("tpgk: not a directory: {}", expanded);
        expanded
    }
}
fn main() {
    logging::configure_logging();
    let args: Vec<String> = std::env::args().collect();
    let mut app = App::new();
    let code = app.run(args);
    std::process::exit(code);
}
