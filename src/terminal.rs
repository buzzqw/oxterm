#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::gdk;
mod commands;
mod hints;
mod history_picker;
mod scrollback;
mod shell_scripts;
pub(crate) mod text;
mod vi_copy;

pub use shell_scripts::{OSC133_FISH_SCRIPT, OSC133_SCRIPT};

#[allow(non_snake_case)]
mod K {
    pub use gdk::keys::constants::*;
}
use gtk::prelude::*;
use regex::Regex;
use zoha_vte::traits::TerminalExt;
use zoha_vte::{CursorBlinkMode, CursorShape, Format, PtyFlags, Regex as VteRegex};

use crate::history::history;
use crate::logging::LOGGER;
use crate::persistence::validate_name;
use crate::session;
use crate::settings::{self, settings};
use crate::window::MainWindow;

/// Width of the OSC 133 gutter drawn left of the terminal. The space is
/// reserved from the first allocation so the grid never changes size when the
/// first prompt marker shows up.
const OSC133_GUTTER_WIDTH: i32 = 6;

/// Selection shape while VI copy mode is active.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViVisualMode {
    #[default]
    None,
    Char,
    Line,
    Block,
}

/// One command as reported by the shell integration: the prompt it started
/// at, the command line, its exit status and how long it took. Used by the
/// sticky command header and the scrollbar marks.
#[derive(Clone, Debug, Default)]
pub struct CommandBlock {
    pub prompt_row: i64,
    pub command_row: i64,
    pub end_row: i64,
    pub command: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub started_at: Option<Instant>,
    pub finished: bool,
}

pub const OXTERM_COMMANDS: &[&str] = &[
    "history", "wnotes", "onotes", "learn", "optimize", "session", "snippet", "help", "clear",
    "cls",
];

const HINT_CHARS: &str = "asdfghjklqwertyuiopzxcvbnm";
const MAX_OSC133_BUFFER: usize = 64 * 1024;

static HINT_URL_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static HINT_PATH_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static HINT_GIT_SHA_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static HINT_IP_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
static URL_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

fn hint_url_re() -> &'static Regex {
    HINT_URL_RE.get_or_init(|| {
        Regex::new(r"(?i)(https?://|ssh://|ftp://|git@|www\.)[\w.\-_~:/?#\[\]@!$&'()*+,;=%]+")
            .unwrap()
    })
}

fn hint_path_re() -> &'static Regex {
    HINT_PATH_RE.get_or_init(|| {
        Regex::new(r"(~?/[\w.\-~+#@!]{2,}(?:/[\w.\-~+#@!]+)*/?)|(\./[\w.\-~+#@!]{1,}(?:/[\w.\-~+#@!]+)*/?)").unwrap()
    })
}

fn hint_git_sha_re() -> &'static Regex {
    HINT_GIT_SHA_RE.get_or_init(|| Regex::new(r"(?i)\b([0-9a-f]{40}|[0-9a-f]{7,39})\b").unwrap())
}

fn hint_ip_re() -> &'static Regex {
    HINT_IP_RE.get_or_init(|| Regex::new(r"\b((?:\d{1,3}\.){3}\d{1,3})\b").unwrap())
}

fn url_re() -> &'static Regex {
    URL_RE.get_or_init(|| {
        Regex::new(r"(?i)(https?://|ssh://|ftp://|git@|www\.)[\w.\-_~:/?#\[\]@!$&'()*+,;=%]+")
            .unwrap()
    })
}

fn hex_to_rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or_else(|_| gdk::RGBA::new(0.0, 0.0, 0.0, 1.0))
}

pub fn event_text(ev: &gdk::EventKey) -> String {
    match ev.keyval().to_unicode() {
        Some(c) => c.to_string(),
        None => String::new(),
    }
}

fn sanitize_terminal_text(text: &str) -> String {
    text.chars()
        .filter(|c| {
            *c == '\n'
                || *c == '\r'
                || *c == '\t'
                || (*c >= ' ' && *c != '\u{7f}' && !('\u{80}'..='\u{9f}').contains(c))
        })
        .collect()
}

/// Convert a byte offset within `line` into a terminal column (one per char).
fn char_column(line: &str, byte_offset: usize) -> usize {
    line.get(..byte_offset)
        .map(|prefix| prefix.chars().count())
        .unwrap_or(byte_offset)
}

fn safe_web_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let url = if raw.to_ascii_lowercase().starts_with("www.") {
        format!("https://{}", raw)
    } else {
        raw.to_string()
    };
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }
    let authority = url.split_once("://")?.1.split('/').next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(url)
}

/// Append a menu item that runs `action` on this terminal. The item holds only
/// a weak reference, so a menu that outlives its terminal is harmless.
fn menu_action<F>(menu: &gtk::Menu, term: &TerminalBox, label: &str, action: F) -> gtk::MenuItem
where
    F: Fn(&TerminalBox) + 'static,
{
    let item = gtk::MenuItem::with_label(label);
    let weak = crate::SendWeak::new(term);
    item.connect_activate(move |_| {
        if let Some(t) = weak.upgrade() {
            action(&t);
        }
    });
    menu.append(&item);
    item
}

/// Same as [`menu_action`], but for window-level actions (tabs, split, …).
fn menu_window_action<F>(
    menu: &gtk::Menu,
    term: &TerminalBox,
    label: &str,
    action: F,
) -> gtk::MenuItem
where
    F: Fn(&dyn crate::window::TerminalWindow) + 'static,
{
    let item = gtk::MenuItem::with_label(label);
    let weak = crate::SendWeak::new(term);
    item.connect_activate(move |_| {
        if let Some(t) = weak.upgrade() {
            t.call_window(|w| action(w));
        }
    });
    menu.append(&item);
    item
}

/// Add a submenu and return the (empty) menu to fill.
fn menu_submenu(menu: &gtk::Menu, label: &str) -> gtk::Menu {
    let item = gtk::MenuItem::with_label(label);
    let submenu = gtk::Menu::new();
    item.set_submenu(Some(&submenu));
    menu.append(&item);
    submenu
}

/// Shorten long labels (URLs, commands) so the menu stays readable.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}...", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

fn open_pty_slave(master: RawFd) -> Result<RawFd, String> {
    let name = unsafe { libc::ptsname(master) };
    if name.is_null() {
        return Err("ptsname failed".to_string());
    }
    let cname = unsafe { std::ffi::CStr::from_ptr(name) };
    let slave = unsafe { libc::open(cname.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if slave < 0 {
        Err("cannot open slave pty".to_string())
    } else {
        Ok(slave)
    }
}

fn set_gui_slave_raw(fd: RawFd) -> Result<(), String> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } != 0 {
        unsafe { libc::close(fd) };
        return Err("cannot configure GUI PTY".to_string());
    }
    let mut termios = unsafe { termios.assume_init() };
    unsafe { libc::cfmakeraw(&mut termios) };
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
        unsafe { libc::close(fd) };
        return Err("cannot configure GUI PTY".to_string());
    }
    Ok(())
}

fn format_duration_ms(duration_ms: i64) -> String {
    if duration_ms < 1_000 {
        format!("{}ms", duration_ms)
    } else if duration_ms < 60_000 {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    } else {
        format!("{}m {}s", duration_ms / 60_000, (duration_ms / 1_000) % 60)
    }
}

fn osc133_event_parts(line: &str) -> Option<(char, &str)> {
    let mut chars = line.chars();
    Some((chars.next()?, chars.as_str()))
}

fn child_status_message(status: i32) -> String {
    let signal = status & 0x7f;
    if signal != 0 && signal != 0x7f {
        format!("[Process terminated by signal {}]", signal)
    } else {
        format!("[Process exited with code {}]", (status >> 8) & 0xff)
    }
}

fn expand_snippet(template: &str, args: &[&str]) -> String {
    let mut expanded = template.replace("{*}", &args.join(" "));
    for (index, arg) in args.iter().enumerate() {
        expanded = expanded.replace(&format!("{{{}}}", index + 1), arg);
    }
    expanded
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn only_web_urls_are_openable() {
        assert!(safe_web_url("https://example.test/path").is_some());
        assert!(safe_web_url("www.example.test").is_some());
        assert!(safe_web_url("file:///etc/passwd").is_none());
        assert!(safe_web_url("ssh://example.test").is_none());
    }

    #[test]
    fn snippets_expand_positional_and_rest_arguments() {
        assert_eq!(
            expand_snippet(
                "git commit -m '{1}' && git push {2} {*}",
                &["message", "origin", "main"]
            ),
            "git commit -m 'message' && git push origin message origin main"
        );
    }

    #[test]
    fn osc133_event_parser_does_not_slice_utf8_boundaries() {
        assert_eq!(osc133_event_parts("Cecho hello"), Some(('C', "echo hello")));
        assert_eq!(osc133_event_parts("€stats"), Some(('€', "stats")));
    }

    #[test]
    fn char_column_counts_characters_not_bytes() {
        let line = "età → https://example.test";
        let start = line.find("https").unwrap();
        assert_eq!(char_column(line, start), 6);
        assert_eq!(char_column(line, 0), 0);
    }

    #[test]
    fn control_socket_is_resolved_through_ssh_config() {
        if std::process::Command::new("ssh")
            .arg("-V")
            .output()
            .is_err()
        {
            return; // no OpenSSH client available
        }
        let dir = std::env::temp_dir().join(format!("oxterm-ssh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("cm.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let args = |path: &str| {
            vec![
                "-o".to_string(),
                format!("ControlPath={}", path),
                "-p".to_string(),
                "2222".to_string(),
                "bob@example.test".to_string(),
                "uptime".to_string(),
            ]
        };
        assert_eq!(
            TerminalBox::ssh_control_socket(&args(&socket.to_string_lossy())),
            Some((
                socket.to_string_lossy().into_owned(),
                "example.test".to_string()
            ))
        );
        // A missing socket means no shared connection: never probe.
        assert_eq!(
            TerminalBox::ssh_control_socket(&args(&dir.join("absent").to_string_lossy())),
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Manual check against a real server with a live shared connection:
    /// OXTERM_TEST_SSH_ARGS="-o ControlPath=... -p 2222 user@host" \
    ///     cargo test remote_stats_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn remote_stats_live() {
        let args: Vec<String> = std::env::var("OXTERM_TEST_SSH_ARGS")
            .expect("set OXTERM_TEST_SSH_ARGS")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let stats = TerminalBox::collect_remote_stats(&args);
        println!("remote stats: {:?}", stats);
        assert!(stats.contains("CPU"));
    }

    #[test]
    fn rebuilds_ssh_arguments_for_mosh_and_autossh() {
        let argv = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        // Exactly as observed in /proc for
        // `mosh --ssh='ssh -p 2223' andres@host -- sleep 9`.
        let mosh = vec![
            "mosh-client".to_string(),
            "-# --ssh=ssh -p 2223 andres@host -- sleep 9 |".to_string(),
            "10.0.0.1".to_string(),
            "60002".to_string(),
        ];
        assert_eq!(mosh_ssh_args(&mosh), Some(argv("-p 2223 andres@host")));
        let mosh_udp_port = vec![
            "mosh-client".to_string(),
            "-# -p 60010 host2 |".to_string(),
            "10.0.0.2".to_string(),
            "60010".to_string(),
        ];
        assert_eq!(mosh_ssh_args(&mosh_udp_port), Some(argv("host2")));
        assert_eq!(mosh_ssh_args(&argv("mosh-client 10.0.0.1 60001")), None);
        assert_eq!(
            autossh_ssh_args(&argv("autossh -M 0 -f -N -L 8080:localhost:80 host")),
            argv("-N -L 8080:localhost:80 host")
        );
        assert!(is_remote_client("mosh-client") && !is_remote_client("bash"));
    }

    /// Manual check: OXTERM_TEST_SHELL_PID=<pid of a shell running ssh,
    /// autossh or mosh> cargo test remote_session_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn remote_session_live() {
        let pid: i32 = std::env::var("OXTERM_TEST_SHELL_PID")
            .expect("set OXTERM_TEST_SHELL_PID")
            .parse()
            .unwrap();
        let args = remote_session_args(pid).expect("no remote session found");
        println!("ssh args: {:?}", args);
        let stats = TerminalBox::collect_remote_stats(&args);
        println!("remote stats: {:?}", stats);
        assert!(stats.contains("CPU"));
    }

    #[test]
    fn child_status_reports_signal_termination() {
        assert_eq!(child_status_message(0), "[Process exited with code 0]");
        assert_eq!(
            child_status_message(libc::SIGTERM),
            "[Process terminated by signal 15]"
        );
    }
}

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct TerminalBox {
        pub window: RefCell<Option<glib::WeakRef<gtk::Window>>>,
        pub settings_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub vte: RefCell<Option<zoha_vte::Terminal>>,
        pub scroll: RefCell<Option<gtk::ScrolledWindow>>,
        pub overlay: RefCell<Option<gtk::Overlay>>,
        pub osc133_margin: RefCell<Option<gtk::DrawingArea>>,
        pub cmd_bar_revealer: RefCell<Option<gtk::Revealer>>,
        pub cmd_bar: RefCell<Option<gtk::Box>>,
        pub cmd_entry: RefCell<Option<gtk::Entry>>,
        pub cmd_list: RefCell<Option<gtk::ListBox>>,
        pub cmd_row_map: RefCell<std::collections::HashMap<usize, String>>,
        pub search_revealer: RefCell<Option<gtk::Revealer>>,
        pub search_entry: RefCell<Option<gtk::SearchEntry>>,
        pub search_label: RefCell<Option<gtk::Label>>,
        pub search_case_btn: RefCell<Option<gtk::ToggleButton>>,
        pub search_regex_btn: RefCell<Option<gtk::ToggleButton>>,
        pub hints_fixed: RefCell<Option<gtk::Fixed>>,
        pub vi_overlay_area: RefCell<Option<gtk::DrawingArea>>,
        pub undercurl_provider: RefCell<Option<gtk::CssProvider>>,

        pub pid: RefCell<i32>,
        pub pty_fd: RefCell<i32>,
        pub shell_integration_path: RefCell<Option<std::path::PathBuf>>,
        pub remote_id: RefCell<String>,
        pub remote_command: RefCell<String>,
        pub remote_command_running: RefCell<bool>,
        pub remote_handle: RefCell<Option<crate::remote::BrokerHandle>>,
        pub remote_pty: RefCell<Option<zoha_vte::Pty>>,
        pub tmux_prefix: RefCell<bool>,
        pub scroll_follow: RefCell<bool>,
        /// When true (`--hold`), keep this terminal open after its child exits
        /// instead of asking the window to close the tab.
        pub hold: RefCell<bool>,

        pub resize_settle_source: RefCell<Option<glib::SourceId>>,
        pub resize_nudge_pending: RefCell<bool>,
        pub skip_next_resize_nudge: RefCell<bool>,
        pub last_alloc_size: RefCell<(i32, i32)>,

        pub input_shadow: RefCell<String>,
        pub shadow_anchor: RefCell<Option<(i64, i64)>>,

        pub history_optimizing: RefCell<bool>,
        pub history_search_mode: RefCell<bool>,
        pub history_search_query: RefCell<String>,
        pub history_search_generation: RefCell<u64>,
        pub history_list_display: RefCell<bool>,
        pub history_list_loading: RefCell<bool>,
        pub history_list_results: RefCell<Vec<Vec<serde_json::Value>>>,
        pub history_list_index: RefCell<usize>,
        pub history_list_nlines: RefCell<usize>,
        pub history_sql_mode: RefCell<bool>,
        pub history_tab_mode: RefCell<bool>,
        pub history_tab_original: RefCell<String>,
        pub tab_fallback_pending_before: RefCell<Option<String>>,
        pub tab_fallback_pending_time: RefCell<i64>,
        /// While the history picker is closing, swallow extra Esc presses so a
        /// user tapping Esc to "get out" does not feed lone Esc characters to
        /// the shell (readline interprets them as a pending meta prefix).
        pub history_esc_deadline: RefCell<i64>,

        pub comando_corrente: RefCell<String>,

        pub remote_stats_cache: RefCell<String>,
        pub remote_stats_ts: RefCell<f64>,
        pub remote_stats_running: RefCell<bool>,
        pub ssh_target_cache: RefCell<Option<(Instant, Option<Vec<String>>)>>,

        pub osc133_markers: RefCell<Vec<(i64, String, i64)>>,
        pub osc133_rfd: RefCell<i32>,
        pub osc133_fifo_path: RefCell<String>,
        pub osc133_source_id: RefCell<Option<glib::SourceId>>,
        pub osc133_buf: RefCell<Vec<u8>>,
        pub osc133_last_exit: RefCell<i64>,
        pub osc133_cmd_start_row: RefCell<i64>,
        pub osc133_integration_active: RefCell<bool>,
        pub osc133_last_history_id: RefCell<Option<u64>>,
        pub osc133_command_started_at: RefCell<Option<Instant>>,
        pub bell_notify_cmd_running: RefCell<bool>,

        pub search_results: RefCell<Vec<serde_json::Value>>,
        pub search_index: RefCell<usize>,
        pub search_tags: RefCell<Vec<i32>>,
        pub quickmarks: RefCell<Vec<i64>>,
        pub quickmark_index: RefCell<i64>,

        pub hints_active: RefCell<bool>,
        pub hints_buffer: RefCell<String>,
        pub hints_map: RefCell<std::collections::BTreeMap<String, (String, String)>>,

        pub vi_copy_active: RefCell<bool>,
        pub vi_cursor_row: Cell<i64>,
        pub vi_cursor_col: Cell<i64>,
        pub vi_anchor_row: Cell<i64>,
        pub vi_anchor_col: Cell<i64>,
        pub vi_visual_mode: Cell<ViVisualMode>,
        pub vi_pending: RefCell<String>,
        pub vi_count: RefCell<String>,
        pub vi_user_marks: RefCell<Vec<i64>>,
        pub vi_last_key: RefCell<Option<gdk::keys::Key>>,
        pub vi_last_key_time: RefCell<i64>,

        /// Command blocks seen through OSC 133, newest last.
        pub command_blocks: RefCell<Vec<CommandBlock>>,
        /// Row of the most recent prompt (OSC 133 ; A).
        pub last_prompt_row: Cell<i64>,
        /// Incremented every time a command finishes; `wait` watches it.
        pub command_generation: Cell<u64>,
        /// Row where the output of the command currently being submitted
        /// starts, captured on Enter. Negative when no command was submitted
        /// through a keystroke (for example a shell hook).
        pub pending_command_row: Cell<i64>,
        pub sticky_header: RefCell<Option<gtk::EventBox>>,
        pub sticky_label: RefCell<Option<gtk::Label>>,
        pub scrollbar_marks: RefCell<Option<gtk::DrawingArea>>,

        pub cached_backspace_binding: RefCell<String>,
        pub cached_delete_binding: RefCell<String>,
        pub cached_broadcast_input: RefCell<bool>,
        pub cached_scroll_on_output: RefCell<bool>,
        pub cmd_bar_visible: RefCell<bool>,

        pub interactive_shell: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TerminalBox {
        const NAME: &'static str = "TpgkTerminalBox";
        type Type = super::TerminalBox;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TerminalBox {}

    impl WidgetImpl for TerminalBox {}

    impl ContainerImpl for TerminalBox {}

    impl BoxImpl for TerminalBox {}
}

pub type ValueRow = serde_json::Value;

glib::wrapper! {
    pub struct TerminalBox(ObjectSubclass<imp::TerminalBox>)
        @extends gtk::Box, gtk::Container, gtk::Widget,
        @implements gtk::Orientable;
}

impl TerminalBox {
    pub fn new(window: &MainWindow) -> TerminalBox {
        let this: TerminalBox = glib::Object::new();
        this.set_orientation(gtk::Orientation::Vertical);
        this.set_hexpand(true);
        this.set_vexpand(true);
        this.init(window);
        this
    }

    #[allow(dead_code)]
    fn imp_state(&self) -> &imp::TerminalBox {
        self.imp()
    }

    fn window(&self) -> Option<gtk::Window> {
        self.imp()
            .window
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    fn call_window<R>(&self, f: impl FnOnce(&dyn crate::window::TerminalWindow) -> R) -> Option<R> {
        let win = self.window()?;
        if let Ok(mw) = win.clone().downcast::<crate::window::MainWindow>() {
            return Some(f(&mw));
        }
        if let Ok(dw) = win.downcast::<crate::window::DetachedWindow>() {
            return Some(f(&dw));
        }
        None
    }

    pub fn vte(&self) -> zoha_vte::Terminal {
        self.imp().vte.borrow().clone().unwrap()
    }

    /// Scroll adjustment of the terminal (absolute buffer rows).
    pub fn vadjustment(&self) -> gtk::Adjustment {
        self.imp().scroll.borrow().as_ref().unwrap().vadjustment()
    }

    pub fn set_window_ref(&self, win: &impl IsA<gtk::Window>) {
        let mut wref = self.imp().window.borrow_mut();
        *wref = Some(win.upcast_ref::<gtk::Window>().downgrade());
    }

    fn init(&self, window: &MainWindow) {
        let s = settings();
        let vte = zoha_vte::Terminal::new();
        vte.set_scrollback_lines(s.get_i64("scrollback_lines"));
        vte.set_mouse_autohide(true);
        vte.set_scroll_on_output(false);
        vte.set_scroll_on_keystroke(s.get_bool("scroll_on_keystroke"));

        *self.imp().vte.borrow_mut() = Some(vte.clone());
        self.imp()
            .window
            .borrow_mut()
            .replace(window.upcast_ref::<gtk::Window>().downgrade());

        self.apply_font();
        self.apply_colors();

        let remote_id = crate::remote::new_session_id();
        *self.imp().remote_id.borrow_mut() = remote_id;

        if s.get_bool("cursor_blink") {
            vte.set_cursor_blink_mode(CursorBlinkMode::On);
        } else {
            vte.set_cursor_blink_mode(CursorBlinkMode::Off);
        }
        self.apply_cursor_shape();
        self.apply_palette();
        vte.set_audible_bell(false);
        vte.set_allow_bold(s.get_bool("allow_bold_text"));
        // Enable OSC 8 hyperlinks so programs (ls --hyperlink, gcc, etc.) can
        // emit explicit clickable links, matching modern terminals. Regex-based
        // URL detection already works independently of this.
        vte.set_allow_hyperlink(true);

        let encoding = s.get_str("encoding");
        let _ = vte.set_encoding(Some(&encoding));

        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.add(&vte);
        *self.imp().scroll.borrow_mut() = Some(scroll.clone());
        self.apply_padding();
        // New adjustments have not emitted a value-changed signal yet, so they
        // must start in follow mode for initial output to reach the bottom.
        *self.imp().scroll_follow.borrow_mut() = true;

        let osc133_margin = gtk::DrawingArea::new();
        osc133_margin.set_size_request(OSC133_GUTTER_WIDTH, -1);
        osc133_margin.set_no_show_all(true);
        // Reserve the gutter up front when the shell integration is enabled.
        // Showing it lazily on the first marker (the first prompt that is not
        // on row 0, i.e. after the first command) shrank the grid by up to a
        // column mid-session: VTE then re-wrapped every full-width line that
        // was already on screen, moving the last character of each one to the
        // next row.
        osc133_margin.set_visible(s.get_bool("osc133"));
        {
            let weak = crate::SendWeak::new(self);
            osc133_margin.connect_draw(move |area, cr| {
                if let Some(t) = weak.upgrade() {
                    t.draw_osc133_margin(area, cr);
                }
                glib::Propagation::Proceed
            });
        }
        *self.imp().osc133_margin.borrow_mut() = Some(osc133_margin.clone());

        let vadj = scroll.vadjustment();
        {
            let weak = crate::SendWeak::new(self);
            vadj.connect_value_changed(move |adj| {
                if let Some(t) = weak.upgrade() {
                    t.on_vadj_value_changed(adj);
                }
            });
            let weak = crate::SendWeak::new(self);
            vadj.connect_changed(move |adj| {
                if let Some(t) = weak.upgrade() {
                    t.on_vadj_changed(adj);
                }
            });
        }

        let term_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        term_box.set_hexpand(true);
        term_box.set_vexpand(true);
        term_box.pack_start(&osc133_margin, false, false, 0);
        term_box.pack_start(&scroll, true, true, 0);

        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.add(&term_box);
        self.pack_start(&overlay, true, true, 0);
        *self.imp().overlay.borrow_mut() = Some(overlay.clone());

        // The command bar and the scrollback search bar are only needed when
        // the user asks for them. Building them lazily keeps each new tab (and
        // session restore) much cheaper while saving a sizeable widget tree
        // when they are never used. See `ensure_command_bar`/`ensure_search_bar`.
        let hints_fixed = gtk::Fixed::new();
        hints_fixed.set_no_show_all(true);
        hints_fixed.hide();
        overlay.add_overlay(&hints_fixed);
        *self.imp().hints_fixed.borrow_mut() = Some(hints_fixed);

        let vi_overlay_area = gtk::DrawingArea::new();
        vi_overlay_area.set_no_show_all(true);
        vi_overlay_area.set_hexpand(true);
        vi_overlay_area.set_vexpand(true);
        vi_overlay_area.set_halign(gtk::Align::Fill);
        vi_overlay_area.set_valign(gtk::Align::Fill);
        vi_overlay_area.hide();
        let weak = crate::SendWeak::new(self);
        vi_overlay_area.connect_draw(move |area, cr| {
            if let Some(t) = weak.upgrade() {
                t.draw_vi_overlay(area, cr);
            }
            glib::Propagation::Proceed
        });
        overlay.add_overlay(&vi_overlay_area);
        *self.imp().vi_overlay_area.borrow_mut() = Some(vi_overlay_area);

        // Sticky header: pinned over the top of the page while the user is
        // scrolled back into a command's output. It is an EventBox so a click
        // can jump to the command that produced the visible lines.
        let sticky = gtk::EventBox::new();
        sticky.set_halign(gtk::Align::Fill);
        sticky.set_valign(gtk::Align::Start);
        sticky.set_hexpand(true);
        sticky.set_no_show_all(true);
        sticky.hide();
        sticky.style_context().add_class("oxterm-sticky-header");
        let sticky_label = gtk::Label::new(None);
        sticky_label.set_xalign(0.0);
        sticky_label.set_ellipsize(pango::EllipsizeMode::End);
        sticky_label.set_single_line_mode(true);
        sticky.add(&sticky_label);
        {
            let weak = crate::SendWeak::new(self);
            sticky.connect_button_press_event(move |_w, ev| {
                if ev.button() == 1 {
                    if let Some(t) = weak.upgrade() {
                        t.scroll_to_sticky_block();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        overlay.add_overlay(&sticky);
        *self.imp().sticky_header.borrow_mut() = Some(sticky);
        *self.imp().sticky_label.borrow_mut() = Some(sticky_label);

        // Scrollbar marks: a thin pass-through strip over the scrollbar that
        // shows where commands (and their failures) are. Pass-through keeps
        // the real scrollbar draggable underneath.
        let scrollbar_marks = gtk::DrawingArea::new();
        scrollbar_marks.set_size_request(10, 1);
        scrollbar_marks.set_hexpand(false);
        scrollbar_marks.set_vexpand(true);
        scrollbar_marks.set_halign(gtk::Align::End);
        scrollbar_marks.set_valign(gtk::Align::Fill);
        scrollbar_marks.set_no_show_all(true);
        scrollbar_marks.hide();
        {
            let weak = crate::SendWeak::new(self);
            scrollbar_marks.connect_draw(move |area, cr| {
                if let Some(t) = weak.upgrade() {
                    t.draw_scrollbar_marks(area, cr);
                }
                glib::Propagation::Proceed
            });
        }
        overlay.add_overlay(&scrollbar_marks);
        overlay.set_overlay_pass_through(&scrollbar_marks, true);
        *self.imp().scrollbar_marks.borrow_mut() = Some(scrollbar_marks);

        // Configure the scrollbar (and the marks strip drawn over it) now that
        // both exist. Doing this before the strip was created left it hidden.
        self.apply_scrollbar_position();

        // VTE signals
        {
            let weak = crate::SendWeak::new(self);
            vte.connect_child_exited(move |_t, status| {
                if let Some(t) = weak.upgrade() {
                    t.on_child_exited(status);
                }
            });
        }
        {
            let weak = crate::SendWeak::new(self);
            vte.connect_selection_changed(move |t| {
                if let Some(t2) = weak.upgrade() {
                    t2.on_selection_changed(t);
                }
            });
        }
        {
            let weak = crate::SendWeak::new(self);
            vte.connect_button_press_event(move |_w, ev| {
                if let Some(t) = weak.upgrade() {
                    return t.on_button_press(ev);
                }
                glib::Propagation::Proceed
            });
        }
        {
            let weak = crate::SendWeak::new(self);
            vte.connect_key_press_event(move |_w, ev| {
                if let Some(t) = weak.upgrade() {
                    return t.on_key_press(ev);
                }
                glib::Propagation::Proceed
            });
        }
        {
            let weak = crate::SendWeak::new(self);
            vte.connect_window_title_changed(move |t| {
                if let Some(t2) = weak.upgrade() {
                    t2.on_title_changed(t);
                }
            });
        }
        vte.add_events(gdk::EventMask::SCROLL_MASK);

        let url_regex = VteRegex::for_match(
            r"(https?://|ssh://|ftp://|git@|www\.)[\w\.\-_~:/?#\[\]@!$&'()*+,;=%]+",
            0x400,
        )
        .ok();
        if let Some(regex) = url_regex {
            let tag = vte.match_add_regex(&regex, 0x400);
            vte.match_set_cursor_type(tag, gdk::CursorType::Hand2);
        }
        vte.set_allow_hyperlink(true);

        // NOTE: the old "resize nudge" (set_size(rows-1) then set_size(rows) on
        // every size-allocate) has been removed. It forced the child to redraw
        // twice for every allocation change, which made full-screen apps like
        // top/htop look like they refreshed two or more times at once (stuttering).
        // VTE already sends the correct winsize/SIGWINCH to the child when the
        // widget is really resized, so the nudge was both redundant and harmful.

        let undercurl_provider = gtk::CssProvider::new();
        let ctx = vte.style_context();
        ctx.add_provider(
            &undercurl_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        *self.imp().undercurl_provider.borrow_mut() = Some(undercurl_provider);
        self.apply_undercurl();

        *self.imp().cached_backspace_binding.borrow_mut() = s.get_str("backspace_binding");
        *self.imp().cached_delete_binding.borrow_mut() = s.get_str("delete_binding");
        *self.imp().cached_broadcast_input.borrow_mut() = s.get_bool("broadcast_input");
        *self.imp().cached_scroll_on_output.borrow_mut() = s.get_bool("scroll_on_output");

        // Settings callback
        let weak = crate::SendWeak::new(self);
        let handler = s.connect_changed(move || {
            if let Some(t) = weak.upgrade() {
                t.apply_settings();
            }
        });
        *self.imp().settings_handler.borrow_mut() = Some(handler);

        *self.imp().cmd_bar_visible.borrow_mut() = false;
        self.imp().pending_command_row.set(-1);

        self.show_all();
    }

    // ── Settings / apply ─────────────────────────────────────

    pub fn apply_settings(&self) {
        self.apply_font();
        self.apply_colors();
        self.apply_cursor_shape();
        self.apply_palette();
        self.apply_scrollbar_position();
        self.apply_padding();
        self.apply_undercurl();
        let s = settings();
        let vte = self.vte();
        vte.set_scrollback_lines(s.get_i64("scrollback_lines"));
        vte.set_scroll_on_keystroke(s.get_bool("scroll_on_keystroke"));
        if s.get_bool("cursor_blink") {
            vte.set_cursor_blink_mode(CursorBlinkMode::On);
        } else {
            vte.set_cursor_blink_mode(CursorBlinkMode::Off);
        }
        vte.set_allow_bold(s.get_bool("allow_bold_text"));
        let encoding = s.get_str("encoding");
        let _ = vte.set_encoding(Some(&encoding));
        *self.imp().cached_backspace_binding.borrow_mut() = s.get_str("backspace_binding");
        *self.imp().cached_delete_binding.borrow_mut() = s.get_str("delete_binding");
        *self.imp().cached_broadcast_input.borrow_mut() = s.get_bool("broadcast_input");
        *self.imp().cached_scroll_on_output.borrow_mut() = s.get_bool("scroll_on_output");
        // Toggling the shell integration in the settings dialog adds or
        // removes the gutter; keep the grid consistent with it.
        self.set_osc133_gutter_visible(
            *self.imp().interactive_shell.borrow() && s.get_bool("osc133"),
        );
    }

    fn apply_font(&self) {
        let s = settings();
        let family = s.get_str("font_name");
        let size = s.get_i64("font_size");
        let mut fd = pango::FontDescription::new();
        fd.set_family(&family);
        fd.set_size(size as i32 * pango::SCALE);
        self.vte().set_font(Some(&fd));
    }

    fn apply_padding(&self) {
        let s = settings();
        let h = s.get_i64("window_padding_horizontal") as i32;
        let v = s.get_i64("window_padding_vertical") as i32;
        let vte = self.vte();
        vte.set_margin_start(h);
        vte.set_margin_end(h);
        vte.set_margin_top(v);
        vte.set_margin_bottom(v);
    }

    fn apply_undercurl(&self) {
        let style = settings().get_str("undercurl_style");
        let css_map = [
            (
                "single",
                "vte-terminal { text-decoration-line: underline; text-decoration-style: solid; }",
            ),
            (
                "double",
                "vte-terminal { text-decoration-line: underline; text-decoration-style: double; }",
            ),
            (
                "curly",
                "vte-terminal { text-decoration-line: underline; text-decoration-style: wavy; }",
            ),
            (
                "dashed",
                "vte-terminal { text-decoration-line: underline; text-decoration-style: dashed; }",
            ),
            (
                "dotted",
                "vte-terminal { text-decoration-line: underline; text-decoration-style: dotted; }",
            ),
        ];
        let css = css_map
            .iter()
            .find(|(k, _)| *k == style)
            .map(|(_, v)| *v)
            .unwrap_or(css_map[0].1);
        if let Some(provider) = self.imp().undercurl_provider.borrow().clone() {
            let _ = provider.load_from_data(css.as_bytes());
        }
    }

    fn bg_rgba(&self) -> gdk::RGBA {
        let s = settings();
        let mut bg = hex_to_rgba(&s.get_bg_color());
        if s.get_bool("enable_transparency") {
            bg.set_alpha(s.get_f64("opacity"));
        }
        bg
    }

    fn apply_colors(&self) {
        let s = settings();
        let fg = hex_to_rgba(&s.get_fg_color());
        let bg = self.bg_rgba();
        self.vte().set_colors(Some(&fg), Some(&bg), &[]);
        let cursor = hex_to_rgba(&s.get_str("cursor_color"));
        self.vte().set_color_cursor(Some(&cursor));
    }

    fn apply_cursor_shape(&self) {
        let shape = settings().get_str("cursor_shape");
        let vte = self.vte();
        if shape == "underline" {
            vte.set_cursor_shape(CursorShape::Underline);
        } else if shape == "ibeam" {
            vte.set_cursor_shape(CursorShape::Ibeam);
        } else {
            vte.set_cursor_shape(CursorShape::Block);
        }
    }

    fn apply_palette(&self) {
        let s = settings();
        let fg = hex_to_rgba(&s.get_fg_color());
        let bg = self.bg_rgba();
        let palette = s.get_palette();
        let keys = [
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
        let mut colors = Vec::new();
        for key in keys {
            let hex = palette
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("#000000");
            colors.push(hex_to_rgba(hex));
        }
        self.vte().set_colors(Some(&fg), Some(&bg), &colors);
        let hl_fg = hex_to_rgba(&s.get_str("highlight_color"));
        self.vte().set_color_highlight_foreground(Some(&hl_fg));
        let hl_bg = hex_to_rgba(&s.get_str("highlight_bg_color"));
        self.vte().set_color_highlight(Some(&hl_bg));
    }

    fn apply_scrollbar_position(&self) {
        let pos = settings().get_str("scrollbar_position");
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        if pos == "left" {
            scroll.set_placement(gtk::CornerType::TopRight);
        } else if pos == "disabled" {
            if let Some(vsb) = scroll.vscrollbar() {
                vsb.hide();
            }
        } else {
            scroll.set_placement(gtk::CornerType::TopLeft);
        }
        if pos != "disabled" {
            if let Some(vsb) = scroll.vscrollbar() {
                vsb.show();
            }
        }
        if let Some(marks) = self.imp().scrollbar_marks.borrow().clone() {
            if pos == "left" {
                marks.set_halign(gtk::Align::Start);
                marks.set_margin_start(OSC133_GUTTER_WIDTH);
            } else {
                marks.set_halign(gtk::Align::End);
                marks.set_margin_start(0);
            }
            if pos == "disabled" {
                marks.hide();
            } else {
                marks.set_visible(true);
            }
        }
    }

    pub fn update_font(&self) {
        self.apply_font();
    }

    pub fn update_colors(&self) {
        self.apply_colors();
        self.apply_cursor_shape();
        self.apply_palette();
    }

    pub fn set_scrollbar_visible(&self, visible: bool) {
        if let Some(vsb) = self.imp().scroll.borrow().clone().unwrap().vscrollbar() {
            vsb.set_visible(visible);
        }
    }

    // ── Launch / pty ─────────────────────────────────────────

    /// Enable/disable "hold" mode: when enabled, the terminal is kept open
    /// after its child exits instead of closing the tab (`--hold`).
    pub fn set_hold(&self, hold: bool) {
        *self.imp().hold.borrow_mut() = hold;
    }

    pub fn launch(&self, cwd: Option<&str>, command: Option<&Vec<String>>) {
        let s = settings();
        *self.imp().interactive_shell.borrow_mut() = command.is_none();
        // Shell integration (and therefore the gutter markers) only exists for
        // an interactive shell; a terminal started with an explicit command
        // keeps the full width for text.
        self.set_osc133_gutter_visible(command.is_none() && s.get_bool("osc133"));
        let mut argv: Vec<String> = if let Some(cmd) = command {
            cmd.clone()
        } else {
            let shell = s.get_str("shell_command");
            if s.get_bool("login_shell") {
                vec![shell, "-l".to_string()]
            } else {
                vec![shell]
            }
        };
        if argv.is_empty() {
            return;
        }

        let mut env: Vec<(String, String)> = std::env::vars().collect();
        if command.is_none() && s.get_bool("osc133") {
            self.write_shell_scripts_once();
            self.prepare_shell_integration(&mut argv, &mut env);
        }
        env.push(("TERM".to_string(), "xterm-256color".to_string()));
        env.push(("COLORTERM".to_string(), "truecolor".to_string()));
        // A tool running in this pane can address it without knowing the
        // session id, and can reach its GUI's control socket directly.
        env.push(("OXTERM_PANE".to_string(), self.remote_id()));
        env.push((
            "OXTERM_CONTROL".to_string(),
            crate::control::socket_path().to_string_lossy().into_owned(),
        ));
        if s.get_bool("osc133") {
            env.push(("OXTERM_SHELL_INTEGRATION".to_string(), "1".to_string()));
            let fifo_path = settings::config_dir().join(format!(
                "osc133_{}_{}.fifo",
                std::process::id(),
                self.as_ptr() as usize
            ));
            let _ = std::fs::remove_file(&fifo_path);
            unsafe {
                let cpath = std::ffi::CString::new(fifo_path.to_string_lossy().as_bytes()).unwrap();
                libc::mkfifo(cpath.as_ptr(), 0o600);
            }
            let _ = std::fs::set_permissions(&fifo_path, std::fs::Permissions::from_mode(0o600));
            *self.imp().osc133_fifo_path.borrow_mut() = fifo_path.to_string_lossy().to_string();
            env.push((
                "OXTERM_OSC133_FIFO".to_string(),
                fifo_path.to_string_lossy().to_string(),
            ));
            // Read by the shell's ssh wrapper (see shell_scripts.rs).
            env.push((
                "OXTERM_SSH_SHARING".to_string(),
                if settings().get_bool("ssh_connection_sharing") {
                    "1"
                } else {
                    "0"
                }
                .to_string(),
            ));
            let fd = unsafe {
                let cpath = std::ffi::CString::new(fifo_path.to_string_lossy().as_bytes()).unwrap();
                // O_RDWR keeps a writer on the FIFO for as long as we hold it,
                // so a shell that opens it per event (fish) or reopens it never
                // produces EOF/HUP and the watch stays alive until child exit.
                libc::open(
                    cpath.as_ptr(),
                    libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            *self.imp().osc133_rfd.borrow_mut() = fd;
        }

        let wd = if let Some(c) = cwd {
            if c.is_empty() {
                dirs::home_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                c.to_string()
            }
        } else {
            dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        };

        match self.spawn_in_pty(&argv, &env, &wd, command.is_none()) {
            Ok(pid) => {
                *self.imp().pid.borrow_mut() = pid;
                let vte = self.vte();
                if let Some(pty) = vte.pty() {
                    *self.imp().pty_fd.borrow_mut() = pty.fd();
                }
                self.watch_osc133_fifo();
                vte.watch_child(glib::Pid(pid));
            }
            Err(e) => {
                self.cleanup_shell_integration();
                self.close_osc133_fifo();
                LOGGER.error(&format!("shell_spawn_failed error={}", e));
                self.vte().feed(
                    format!("\r\n\x1b[31m[Failed to start shell: {}]\x1b[0m\r\n", e).as_bytes(),
                );
            }
        }
    }

    fn watch_osc133_fifo(&self) {
        let fd = *self.imp().osc133_rfd.borrow();
        if fd < 0 || self.imp().osc133_source_id.borrow().is_some() {
            return;
        }
        let weak = crate::SendWeak::new(self);
        let source = glib::source::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_fd, condition| {
                let Some(t) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let keep_source = t.on_osc133_pipe_data()
                    && !(condition.contains(glib::IOCondition::HUP)
                        && !condition.contains(glib::IOCondition::IN));
                if !keep_source {
                    // Returning Break removes the source in GLib. Clear the
                    // handle without calling SourceId::remove(), which would
                    // panic because the source is already gone.
                    t.imp().osc133_source_id.borrow_mut().take();
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            },
        );
        *self.imp().osc133_source_id.borrow_mut() = Some(source);
    }

    fn prepare_shell_integration(&self, argv: &mut Vec<String>, env: &mut Vec<(String, String)>) {
        let Some(shell) = argv
            .first()
            .and_then(|path| std::path::Path::new(path).file_name())
            .and_then(|name| name.to_str())
        else {
            return;
        };
        let dir = settings::config_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let id = format!("{}-{}", std::process::id(), self.as_ptr() as usize);
        let osc_path = dir.join("osc133.sh");

        if shell == "bash" {
            let path = dir.join(format!("session-{}.bashrc", id));
            let login = argv.iter().any(|arg| arg == "-l" || arg == "--login");
            let startup = if login {
                "[ -r /etc/profile ] && . /etc/profile\nif [ -r \"$HOME/.bash_profile\" ]; then . \"$HOME/.bash_profile\"; elif [ -r \"$HOME/.bash_login\" ]; then . \"$HOME/.bash_login\"; elif [ -r \"$HOME/.profile\" ]; then . \"$HOME/.profile\"; fi\n[ -r \"$OXTERM_OSC133_SCRIPT\" ] && . \"$OXTERM_OSC133_SCRIPT\"\n".to_string()
            } else {
                "[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n[ -r \"$OXTERM_OSC133_SCRIPT\" ] && . \"$OXTERM_OSC133_SCRIPT\"\n"
                    .to_string()
            };
            if std::fs::write(&path, startup).is_err() {
                return;
            }
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            let original = std::mem::take(argv);
            argv.push(original[0].clone());
            for arg in &original[1..] {
                if arg != "-l" && arg != "--login" {
                    argv.push(arg.clone());
                }
            }
            argv.push("--rcfile".to_string());
            argv.push(path.to_string_lossy().to_string());
            argv.push("-i".to_string());
            env.push((
                "OXTERM_OSC133_SCRIPT".to_string(),
                osc_path.to_string_lossy().to_string(),
            ));
            *self.imp().shell_integration_path.borrow_mut() = Some(path);
        } else if shell == "zsh" {
            let path = dir.join(format!("session-{}-zsh", id));
            if std::fs::create_dir_all(&path).is_err() {
                return;
            }
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
            let original_zdotdir = std::env::var_os("ZDOTDIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(home);
            for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
                let original = original_zdotdir.join(name);
                let original = shell_quote(&original);
                let mut startup =
                    format!("if [[ -r {} ]]; then source {}; fi\n", original, original);
                if name == ".zshrc" {
                    startup.push_str(
                        "[[ -r \"$OXTERM_OSC133_SCRIPT\" ]] && source \"$OXTERM_OSC133_SCRIPT\"\n",
                    );
                }
                let _ = std::fs::write(path.join(name), startup);
                let _ = std::fs::set_permissions(
                    path.join(name),
                    std::fs::Permissions::from_mode(0o600),
                );
            }
            env.push(("ZDOTDIR".to_string(), path.to_string_lossy().to_string()));
            env.push((
                "OXTERM_OSC133_SCRIPT".to_string(),
                osc_path.to_string_lossy().to_string(),
            ));
            *self.imp().shell_integration_path.borrow_mut() = Some(path);
        } else if shell == "fish" {
            let path = dir.join(format!("session-{}-fish", id));
            let fish_path = dir.join("osc133.fish");
            let startup = format!("source {}\n", shell_quote(&fish_path));
            if std::fs::write(&path, startup).is_err() {
                return;
            }
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            argv.push("--init-command".to_string());
            argv.push(format!("source {}", shell_quote(&path)));
            env.push((
                "OXTERM_OSC133_SCRIPT".to_string(),
                fish_path.to_string_lossy().to_string(),
            ));
            *self.imp().shell_integration_path.borrow_mut() = Some(path);
        }
    }

    fn cleanup_shell_integration(&self) {
        let Some(path) = self.imp().shell_integration_path.borrow_mut().take() else {
            return;
        };
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }

    fn close_osc133_fifo(&self) {
        if let Some(source) = self.imp().osc133_source_id.borrow_mut().take() {
            source.remove();
        }
        if *self.imp().osc133_rfd.borrow() >= 0 {
            unsafe { libc::close(*self.imp().osc133_rfd.borrow()) };
            *self.imp().osc133_rfd.borrow_mut() = -1;
        }
        let fifo = self.imp().osc133_fifo_path.borrow().clone();
        if !fifo.is_empty() {
            let _ = std::fs::remove_file(&fifo);
            *self.imp().osc133_fifo_path.borrow_mut() = String::new();
        }
    }

    fn spawn_in_pty(
        &self,
        argv: &[String],
        env: &[(String, String)],
        cwd: &str,
        interactive_shell: bool,
    ) -> Result<i32, String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let vte = self.vte();
        let child_pty = vte
            .pty_new_sync(PtyFlags::DEFAULT, None::<&gio::Cancellable>)
            .map_err(|e| e.to_string())?;
        let child_master = child_pty.fd();
        let child_slave = open_pty_slave(child_master)?;
        let gui_pty = match vte.pty_new_sync(PtyFlags::DEFAULT, None::<&gio::Cancellable>) {
            Ok(pty) => pty,
            Err(error) => {
                unsafe { libc::close(child_slave) };
                return Err(error.to_string());
            }
        };
        let gui_slave = match open_pty_slave(gui_pty.fd()) {
            Ok(fd) => fd,
            Err(error) => {
                unsafe { libc::close(child_slave) };
                return Err(error);
            }
        };
        if let Err(error) = set_gui_slave_raw(gui_slave) {
            unsafe { libc::close(child_slave) };
            return Err(error);
        }

        // VTE owns the GUI master. The broker owns the GUI slave and child
        // master, so closing the widget does not close the shell session.
        vte.set_pty(Some(&gui_pty));

        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.current_dir(cwd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child_master_for_child = child_master;
        let gui_slave_for_child = gui_slave;
        unsafe {
            cmd.pre_exec(move || {
                libc::setsid();
                libc::ioctl(child_slave, libc::TIOCSCTTY as libc::c_ulong, 0);
                libc::dup2(child_slave, 0);
                libc::dup2(child_slave, 1);
                libc::dup2(child_slave, 2);
                if child_slave > 2 {
                    libc::close(child_slave);
                }
                libc::close(child_master_for_child);
                libc::close(gui_slave_for_child);
                Ok(())
            });
        }
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                unsafe {
                    libc::close(child_slave);
                    libc::close(gui_slave);
                }
                // Do not reset the PTY to NULL: libvte 0.84 dereferences the
                // null PTY here. Leaving the GUI PTY attached is harmless
                // because this terminal has no child and is about to show the
                // spawn error.
                return Err(error.to_string());
            }
        };
        unsafe { libc::close(child_slave) };
        let child_pid = child.id() as i32;
        let broker = match crate::remote::spawn_broker(
            &self.imp().remote_id.borrow(),
            child_master,
            gui_slave,
            child_pid,
            "Terminal",
            cwd,
            interactive_shell,
        ) {
            Ok(broker) => broker,
            Err(error) => {
                unsafe {
                    libc::killpg(child_pid, libc::SIGTERM);
                    libc::close(gui_slave);
                }
                // See the comment above: set_pty(NULL) crashes libvte 0.84.
                return Err(error);
            }
        };
        unsafe { libc::close(gui_slave) };
        *self.imp().remote_handle.borrow_mut() = Some(broker);
        Ok(child_pid)
    }

    fn write_shell_scripts_once(&self) {
        // The scripts are identical for every tab and only depend on the
        // config directory, so write them once per process instead of once
        // per new terminal/restored tab.
        static WRITTEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if WRITTEN.get().is_some() {
            return;
        }
        self.write_osc133_script();
        self.write_osc133_fish_script();
        let _ = WRITTEN.set(());
    }

    fn write_osc133_script(&self) {
        let dir = settings::config_dir();
        let path = dir.join("osc133.sh");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            LOGGER.error(&format!("osc133_dir_failed error={}", e));
            return;
        }
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        if let Err(e) = std::fs::write(&path, OSC133_SCRIPT) {
            LOGGER.error(&format!("osc133_write_failed error={}", e));
            return;
        }
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            LOGGER.error(&format!("osc133_permissions_failed error={}", e));
        }
    }

    fn write_osc133_fish_script(&self) {
        let dir = settings::config_dir();
        let path = dir.join("osc133.fish");
        if let Err(e) = std::fs::write(&path, OSC133_FISH_SCRIPT) {
            LOGGER.error(&format!("osc133_fish_write_failed error={}", e));
            return;
        }
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    pub fn terminate(&self) {
        self.cleanup_shell_integration();
        self.close_osc133_fifo();
        if let Some(handler) = self.imp().settings_handler.borrow_mut().take() {
            settings().disconnect_changed(handler);
        }
        self.imp().remote_pty.borrow_mut().take();
        if let Some(handle) = self.imp().remote_handle.borrow_mut().take() {
            // A session whose local view is off was detached with `Ctrl+B`, `d`
            // or taken over elsewhere with `oxterm -M`. Its broker owns the
            // shell now, so closing this window must leave it running instead
            // of killing a session that is still in use in another terminal.
            if handle.local_is_on() {
                handle.kill();
            } else {
                LOGGER.warning("terminal_close_preserves_detached_session");
            }
        } else {
            let pid = *self.imp().pid.borrow();
            if pid > 0 {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }

    pub fn set_remote_info(&self, title: &str, cwd: &str) {
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.update_info("", title, cwd);
        }
    }

    pub fn set_remote_name(&self, name: &str) {
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.set_name(name);
        }
    }

    fn set_remote_command(&self, command: &str, running: bool) {
        let command = command.trim();
        if command.is_empty() || self.is_oxterm_command(command) {
            return;
        }
        *self.imp().remote_command.borrow_mut() = command.to_string();
        *self.imp().remote_command_running.borrow_mut() = running;
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.update_command(command, running);
        }
    }

    fn set_remote_command_state(&self, running: bool) {
        let command = self.imp().remote_command.borrow().clone();
        if !command.is_empty() {
            *self.imp().remote_command_running.borrow_mut() = running;
            if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
                handle.update_command(&command, running);
            }
        }
    }

    pub fn remote_id(&self) -> String {
        self.imp().remote_id.borrow().clone()
    }

    pub fn remote_command(&self) -> String {
        self.imp().remote_command.borrow().clone()
    }

    pub fn remote_command_status(&self) -> &'static str {
        if self.imp().remote_command.borrow().is_empty() {
            "-"
        } else if *self.imp().remote_command_running.borrow() {
            "running"
        } else {
            "last"
        }
    }

    pub fn remote_status(&self) -> &'static str {
        if self.imp().remote_pty.borrow().is_some() {
            "detached"
        } else if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            // A terminal moved away with `oxterm --move` keeps its PTY in this
            // VTE but has local forwarding turned off in the broker, so the
            // tab shows no output until it is reattached.
            if handle.local_is_on() {
                "local"
            } else {
                "detached"
            }
        } else {
            "local"
        }
    }

    pub fn has_active_process(&self) -> bool {
        let pid = *self.imp().pid.borrow();
        if pid <= 0 {
            return false;
        }
        if *self.imp().osc133_integration_active.borrow()
            && *self.imp().remote_command_running.borrow()
        {
            return true;
        }
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            return handle.foreground_process_active();
        }

        let fd = *self.imp().pty_fd.borrow();
        if fd < 0 || !*self.imp().interactive_shell.borrow() {
            return true;
        }
        let foreground = unsafe { libc::tcgetpgrp(fd) };
        let shell_group = unsafe { libc::getpgid(pid) };
        if foreground <= 0 || shell_group <= 0 {
            return true;
        }
        foreground != shell_group
    }

    pub fn reattach_local(&self) {
        if let Some(pty) = self.imp().remote_pty.borrow_mut().take() {
            self.vte().set_pty(Some(&pty));
        }
        // A terminal moved away by `oxterm --move` keeps its PTY attached to
        // this VTE; re-enabling local forwarding brings its output back.
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.local_on();
        }
        self.vte().grab_focus();
    }

    pub(crate) fn detach_local_terminal(&self) {
        if self.imp().remote_pty.borrow().is_some() {
            return;
        }
        let vte = self.vte();
        let Some(pty) = vte.pty() else {
            return;
        };
        // libvte 0.84 crashes when the terminal's PTY is set to NULL, which is
        // what "detaching" the local view used to do. Park an idle PTY in the
        // widget instead and keep the real one for the broker/reattach.
        let idle = match vte.pty_new_sync(PtyFlags::DEFAULT, None::<&gio::Cancellable>) {
            Ok(pty) => pty,
            Err(error) => {
                LOGGER.error(&format!("remote_pty_replace_failed error={}", error));
                return;
            }
        };
        vte.set_pty(Some(&idle));
        *self.imp().remote_pty.borrow_mut() = Some(pty);
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.local_off();
        }
    }

    fn foreground_process_is_ssh(&self) -> bool {
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            return handle.foreground_is_ssh();
        }
        let fd = *self.imp().pty_fd.borrow();
        if fd < 0 {
            return false;
        }
        let foreground = unsafe { libc::tcgetpgrp(fd) };
        if foreground <= 0 {
            return false;
        }
        std::fs::read_to_string(format!("/proc/{foreground}/comm"))
            .map(|name| is_remote_client(name.trim()))
            .unwrap_or(false)
    }

    pub fn kill(&self, sig: i32) {
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            handle.signal(sig);
        } else {
            let pid = *self.imp().pid.borrow();
            if pid > 0 {
                unsafe { libc::kill(pid, sig) };
            }
        }
    }

    pub fn set_encoding(&self, encoding: &str) {
        let _ = self.vte().set_encoding(Some(encoding));
    }

    pub fn copy(&self) {
        self.vte().copy_clipboard_format(Format::Text);
    }

    pub fn paste(&self) {
        // Request the text asynchronously (no nested main loop) and paste
        // exactly the text that was checked, so the clipboard owner cannot
        // swap in different content after the multi-line warning.
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        let weak = self.downgrade();
        clipboard.request_text(move |_, text| {
            let (Some(t), Some(text)) = (weak.upgrade(), text) else {
                return;
            };
            t.paste_checked_text(text);
        });
    }

    fn paste_checked_text(&self, text: &str) {
        if (text.contains('\n') || text.contains('\r'))
            && settings().get_bool("show_unsafe_paste_dialog")
        {
            let parent = self
                .toplevel()
                .and_then(|w| w.downcast::<gtk::Window>().ok());
            let dialog = gtk::MessageDialog::new(
                parent.as_ref(),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Warning,
                gtk::ButtonsType::YesNo,
                "The clipboard contains multiple lines.\nPasting could run commands unintentionally.\n\nPaste anyway?",
            );
            let resp = dialog.run();
            dialog.close();
            if resp != gtk::ResponseType::Yes {
                return;
            }
        }
        self.shadow_paste(text);
        self.vte().paste_text(text);
    }

    pub fn paste_selection(&self) {
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_PRIMARY);
        let weak = self.downgrade();
        clipboard.request_text(move |_, text| {
            let (Some(t), Some(text)) = (weak.upgrade(), text) else {
                return;
            };
            t.shadow_paste(text);
            t.vte().paste_text(text);
        });
    }

    pub fn select_all(&self) {
        self.vte().select_all();
    }

    pub fn reset(&self, clear: bool) {
        self.vte().reset(true, clear);
    }

    pub fn set_read_only(&self, ro: bool) {
        self.vte().set_input_enabled(!ro);
    }

    pub fn get_cwd(&self) -> String {
        let pid = *self.imp().pid.borrow();
        if pid > 0 {
            if let Ok(target) = std::fs::read_link(format!("/proc/{}/cwd", pid)) {
                return target.to_string_lossy().to_string();
            }
        }
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Text of the last command reported through OSC 133, if any.
    pub fn last_command(&self) -> String {
        self.imp().remote_command.borrow().clone()
    }

    /// Exit status of the last command reported through OSC 133.
    pub fn last_exit_code(&self) -> i64 {
        *self.imp().osc133_last_exit.borrow()
    }

    pub fn is_ssh(&self) -> bool {
        let pid = *self.imp().pid.borrow();
        if pid <= 0 {
            return false;
        }
        if let Ok(content) = std::fs::read(format!("/proc/{}/environ", pid)) {
            for needle in [
                b"SSH_CONNECTION".as_slice(),
                b"SSH_TTY".as_slice(),
                b"SSH_CLIENT".as_slice(),
            ] {
                if content.windows(needle.len()).any(|w| w == needle) {
                    return true;
                }
            }
        }
        self.get_ssh_target().is_some()
    }

    pub fn is_ssh_client(&self) -> bool {
        self.get_ssh_target().is_some()
    }

    /// ssh arguments (without the program name) describing the remote
    /// session running as a direct child of this terminal's shell: a plain
    /// `ssh`, the `ssh` supervised by `autossh`, or the host of a `mosh`
    /// session (whose ssh bootstrap has already exited).
    fn get_ssh_target(&self) -> Option<Vec<String>> {
        if let Some((at, target)) = self.imp().ssh_target_cache.borrow().as_ref() {
            if at.elapsed() < Duration::from_secs(2) {
                return target.clone();
            }
        }
        let pid = *self.imp().pid.borrow();
        let result = (pid > 0).then(|| remote_session_args(pid)).flatten();
        *self.imp().ssh_target_cache.borrow_mut() = Some((Instant::now(), result.clone()));
        result
    }

    /// The running ssh client's arguments plus any still-fresh cached stats.
    /// A failed probe is remembered for a minute so an unreachable or
    /// unshared connection is not retried every few seconds.
    pub fn remote_stats_request(&self) -> Option<(Vec<String>, Option<String>)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        let args = self.get_ssh_target()?;
        let cache = self.imp().remote_stats_cache.borrow().clone();
        let cached_at = *self.imp().remote_stats_ts.borrow();
        let age = now - cached_at;
        let cached = if cached_at <= 0.0 {
            None
        } else if !cache.is_empty() && age < 15.0 {
            Some(cache)
        } else if cache.is_empty() && age < 60.0 {
            Some(crate::system_stats::ssh_placeholder())
        } else {
            None
        };
        Some((args, cached))
    }

    pub fn cache_remote_stats(&self, stats: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        *self.imp().remote_stats_cache.borrow_mut() = stats.to_string();
        *self.imp().remote_stats_ts.borrow_mut() = now;
    }

    /// Path of the live connection-sharing (ControlMaster) socket for the
    /// given ssh arguments, resolved by ssh itself (`ssh -G`), so the user's
    /// config, `-p`, `-l`, `-J` and our wrapper's options are all honoured.
    fn ssh_control_socket(args: &[String]) -> Option<(String, String)> {
        Self::ssh_control_socket_with(args, &[]).or_else(|| {
            // Sessions started outside the wrapper (mosh, a plain autossh)
            // can still use a connection another tab shares for this host.
            let shared = format!(
                "ControlPath={}/cm-%C",
                settings::config_dir().to_string_lossy()
            );
            Self::ssh_control_socket_with(args, &["-o".to_string(), shared])
        })
    }

    fn ssh_control_socket_with(args: &[String], extra: &[String]) -> Option<(String, String)> {
        let output = std::process::Command::new("ssh")
            .arg("-G")
            .args(extra)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let config = String::from_utf8_lossy(&output.stdout);
        let value = |key: &str| {
            config.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(' '))
                    .map(str::to_string)
            })
        };
        let socket = value("controlpath").filter(|p| p != "none")?;
        let host = value("hostname").unwrap_or_else(|| "localhost".to_string());
        let socket_path = if let Some(rest) = socket.strip_prefix("~/") {
            dirs::home_dir()?.join(rest).to_string_lossy().into_owned()
        } else {
            socket
        };
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(&socket_path)
            .ok()
            .filter(|m| m.file_type().is_socket())?;
        Some((socket_path, host))
    }

    /// Collect CPU/RAM/disk of the remote host through the session's shared
    /// ssh connection. Never opens a new connection: without a live master
    /// socket it returns nothing (no extra logins, no fail2ban hits).
    pub fn collect_remote_stats(args: &[String]) -> String {
        let Some((socket, host)) = Self::ssh_control_socket(args) else {
            return String::new();
        };
        let mux_args = |extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = [
                "-S",
                socket.as_str(),
                "-o",
                "ControlMaster=no",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=2",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        // Make sure the master is alive first; otherwise ssh would fall back
        // to opening a brand-new connection.
        let alive = std::process::Command::new("ssh")
            .args(mux_args(&["-O", "check", &host]))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return String::new();
        }
        // LC_ALL=C: ssh forwards LANG/LC_*, and in e.g. it_IT awk prints
        // "3,2" which would not parse as a number.
        let cmd = "export LC_ALL=C; cpu1=$(awk '/^cpu /{idle=$5+$6; total=$2+$3+$4+$5+$6+$7+$8+$9; printf \"%s %s\",idle,total; exit}' /proc/stat 2>/dev/null); \
sleep 0.2; \
cpu2=$(awk '/^cpu /{idle=$5+$6; total=$2+$3+$4+$5+$6+$7+$8+$9; printf \"%s %s\",idle,total; exit}' /proc/stat 2>/dev/null); \
awk -v first=\"$cpu1\" -v second=\"$cpu2\" 'BEGIN{split(first,a);split(second,b);d=b[2]-a[2];u=d>0?(1-(b[1]-a[1])/d)*100:0;if(u<0)u=0;if(u>100)u=100;printf \"%.1f\\n\",u}'; \
awk '/^MemTotal/{t=$2}/^MemAvailable/{a=$2}END{printf \"%d %d\\n\",(t-a)*1024,t*1024}' /proc/meminfo 2>/dev/null; \
df -B1 / 2>/dev/null | awk 'NR==2{printf \"%d %d\\n\",$3,$2}'";
        let ssh_args = mux_args(&[&host, cmd]);
        let output = std::process::Command::new("ssh")
            .args(&ssh_args)
            .env("SSH_ASKPASS", "true")
            .env("DISPLAY", "")
            .output();
        let Ok(output) = output else {
            return String::new();
        };
        if !output.status.success() {
            return String::new();
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() < 3 {
            return String::new();
        }
        let Ok(cpu) = lines[0].parse::<f64>() else {
            return String::new();
        };
        let mem: Vec<&str> = lines[1].split_whitespace().collect();
        if mem.len() < 2 {
            return String::new();
        }
        let mem_used = mem[0].parse::<i64>().unwrap_or(0);
        let mem_total = mem[1].parse::<i64>().unwrap_or(0);
        let disk: Vec<&str> = lines[2].split_whitespace().collect();
        if disk.len() < 2 {
            return String::new();
        }
        let disk_used = disk[0].parse::<i64>().unwrap_or(0);
        let disk_total = disk[1].parse::<i64>().unwrap_or(0);
        let mem_pct = if mem_total > 0 {
            (mem_used as f64 / mem_total as f64 * 100.0) as i64
        } else {
            0
        };
        let disk_pct = if disk_total > 0 {
            (disk_used as f64 / disk_total as f64 * 100.0) as i64
        } else {
            0
        };
        let result = format!(
            "  [SSH] CPU {:5.1}%  RAM {}/{} ({}%)  Disk {}/{} ({}%)",
            cpu,
            Self::format_bytes(mem_used),
            Self::format_bytes(mem_total),
            mem_pct,
            Self::format_bytes(disk_used),
            Self::format_bytes(disk_total),
            disk_pct
        );
        result
    }

    fn is_echo_on(&self) -> bool {
        if let Some(handle) = self.imp().remote_handle.borrow().as_ref() {
            return handle.child_echo_on();
        }
        let fd = *self.imp().pty_fd.borrow();
        if fd < 0 {
            return false;
        }
        unsafe {
            let mut attr = std::mem::MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(fd, attr.as_mut_ptr()) == 0 {
                let attr = attr.assume_init();
                return attr.c_lflag & libc::ECHO != 0;
            }
        }
        false
    }

    fn format_bytes(val: i64) -> String {
        let gb = 1024 * 1024 * 1024;
        let mb = 1024 * 1024;
        if val >= gb {
            format!("{:.1}G", val as f64 / gb as f64)
        } else {
            format!("{}M", val / mb)
        }
    }

    pub fn zoom_in(&self) {
        let vte = self.vte();
        vte.set_font_scale(vte.font_scale() * 1.1);
    }

    pub fn zoom_out(&self) {
        let vte = self.vte();
        vte.set_font_scale((vte.font_scale() / 1.1).max(0.25));
    }

    pub fn zoom_reset(&self) {
        self.vte().set_font_scale(1.0);
    }

    pub fn feed_command(&self, text: &str) {
        self.vte().feed_child(text.as_bytes());
    }

    pub fn feed_command_bytes(&self, data: &[u8]) {
        self.vte().feed_child(data);
        // cached_broadcast_input is refreshed on every settings change.
        if *self.imp().cached_broadcast_input.borrow() {
            self.broadcast_to_others(data);
        }
    }

    /// Type into the pane from the control API. Unlike keystrokes this does
    /// not go through broadcast input: a script targets exactly one pane.
    pub fn control_send(&self, text: &str, enter: bool) {
        if enter && !text.contains(['\n', '\r']) {
            // The shell integration reports the command start without a row;
            // remember where the output will begin so the block is accurate
            // even when the command finishes before the GUI reads its output.
            let (_, row) = self.vte().cursor_position();
            self.imp().pending_command_row.set(row + 1);
        }
        let mut bytes = text.as_bytes().to_vec();
        if enter {
            bytes.push(b'\r');
        }
        self.vte().feed_child(&bytes);
    }

    /// Text for `oxterm capture`: visible screen, N lines of history, or all.
    pub fn capture_text(&self, scope: &str, lines: usize) -> String {
        let vadj = self.vadjustment();
        let lower = vadj.lower();
        let upper = vadj.upper();
        let page = vadj.page_size();
        let start = match scope {
            "all" => lower,
            "lines" => (upper - page - lines as f64).max(lower),
            _ => (upper - page).max(lower),
        };
        text::range(&self.vte(), start as i64, 0, upper as i64, 0).unwrap_or_default()
    }

    pub fn command_generation(&self) -> u64 {
        self.imp().command_generation.get()
    }

    pub fn command_running(&self) -> bool {
        *self.imp().remote_command_running.borrow()
    }

    pub fn is_alive(&self) -> bool {
        *self.imp().pid.borrow() > 0
    }

    fn broadcast_to_others(&self, data: &[u8]) {
        self.call_window(|win| win.broadcast_feed(self, data));
    }

    // ── Scroll / resize ──────────────────────────────────────

    fn on_vadj_value_changed(&self, adj: &gtk::Adjustment) {
        if let Some(margin) = self.imp().osc133_margin.borrow().clone() {
            margin.queue_draw();
        }
        if let Some(marks) = self.imp().scrollbar_marks.borrow().clone() {
            marks.queue_draw();
        }
        self.update_sticky_header();
        let bottom = (adj.upper() - adj.page_size()).max(0.0);
        *self.imp().scroll_follow.borrow_mut() = adj.value() >= bottom - 0.5;
    }

    fn on_vadj_changed(&self, adj: &gtk::Adjustment) {
        // Runs for every line of output: avoid redrawing a hidden margin and
        // use the cached setting instead of a settings lookup.
        if let Some(margin) = self.imp().osc133_margin.borrow().as_ref() {
            if margin.is_visible() {
                margin.queue_draw();
            }
        }
        if let Some(marks) = self.imp().scrollbar_marks.borrow().as_ref() {
            if marks.is_visible() {
                marks.queue_draw();
            }
        }
        self.update_sticky_header();
        if !*self.imp().cached_scroll_on_output.borrow() {
            return;
        }
        if !*self.imp().scroll_follow.borrow() {
            return;
        }
        let bottom = (adj.upper() - adj.page_size()).max(0.0);
        if adj.value() != bottom {
            adj.set_value(bottom);
        }
    }

    // ── OSC 133 ──────────────────────────────────────────────

    fn on_osc133_pipe_data(&self) -> bool {
        let mut data = vec![0u8; 4096];
        let n = unsafe {
            libc::read(
                *self.imp().osc133_rfd.borrow(),
                data.as_mut_ptr() as *mut _,
                4096,
            )
        };
        if n > 0 {
            data.truncate(n as usize);
            let mut buf = self.imp().osc133_buf.borrow_mut();
            buf.extend_from_slice(&data);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line_str = String::from_utf8_lossy(&line);
                self.process_osc133_line(line_str.trim());
            }
            if buf.len() > MAX_OSC133_BUFFER {
                buf.clear();
                LOGGER.warning("osc133_buffer_limit_reached");
            }
            true
        } else if n == 0 {
            // No writer remains. Keeping a GLib source on this FIFO would make
            // the main loop spin forever on repeated HUP/IN notifications.
            false
        } else {
            let error = std::io::Error::last_os_error();
            error.kind() == std::io::ErrorKind::WouldBlock
        }
    }

    fn process_osc133_line(&self, line: &str) {
        if line.is_empty() {
            return;
        }
        // Handle each event as soon as it arrives. Batching these for even a
        // few milliseconds moved prompt/command rows to where the cursor had
        // already scrolled by the time the batch ran, which made fast commands
        // report the wrong block range.
        self.osc133_handle_event(line);
    }

    fn osc133_handle_event(&self, line: &str) {
        // Prompt boundaries and command ends are only meaningful once VTE has
        // consumed the output that precedes them. Handle them after a short
        // delay; the command start (`C`) itself must be handled immediately so
        // its row comes from the pending keystroke, not from the output.
        let command = line.chars().next();
        if matches!(command, Some('A') | Some('D')) {
            // Duration must be measured when the event arrives, not when the
            // deferred handler runs, or every fast command gains the delay.
            // `None` means the shell reported a prompt without having started
            // a command first, such as the marker emitted when the integration
            // script loads. Such an event must not complete an `oxterm wait`.
            let duration = if command == Some('D') {
                self.imp()
                    .osc133_command_started_at
                    .borrow_mut()
                    .take()
                    .map(|started| started.elapsed().as_millis().min(i64::MAX as u128) as i64)
            } else {
                None
            };
            let line = line.to_string();
            let weak = crate::SendWeak::new(self);
            // An idle callback can run before VTE has read the command's output
            // from the PTY, which would put the prompt on the command's first
            // row. A short timeout reliably covers the hand-off for local
            // output while staying far below human perception.
            glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
                if let Some(term) = weak.upgrade() {
                    term.osc133_handle_event_now(&line, duration);
                }
            });
            return;
        }
        self.osc133_handle_event_now(line, None);
    }

    fn osc133_handle_event_now(&self, line: &str, duration_ms: Option<i64>) {
        if *self.imp().pid.borrow() <= 0 {
            return;
        }
        *self.imp().osc133_integration_active.borrow_mut() = true;
        let (col, cursor_row) = self.vte().cursor_position();
        let _ = col;
        let Some((cmd, rest)) = osc133_event_parts(line) else {
            return;
        };
        if cmd == 'C' {
            let pending_row = self.imp().pending_command_row.replace(-1);
            let row = if pending_row >= 0 {
                pending_row
            } else {
                cursor_row
            };
            *self.imp().osc133_cmd_start_row.borrow_mut() = row;
            *self.imp().osc133_command_started_at.borrow_mut() = Some(Instant::now());
            self.imp()
                .osc133_markers
                .borrow_mut()
                .push((row, "cmd_start".into(), 0));
            *self.imp().bell_notify_cmd_running.borrow_mut() = true;
            let command_text = rest.trim().to_string();
            self.set_remote_command(&command_text, true);
            *self.imp().osc133_last_history_id.borrow_mut() = None;
            {
                let prompt_row = self.imp().last_prompt_row.get();
                let mut blocks = self.imp().command_blocks.borrow_mut();
                blocks.push(CommandBlock {
                    prompt_row: if prompt_row >= 0 { prompt_row } else { row },
                    command_row: row,
                    end_row: row,
                    command: command_text.clone(),
                    exit_code: None,
                    duration_ms: None,
                    started_at: Some(Instant::now()),
                    finished: false,
                });
                if blocks.len() > 1000 {
                    let drain = blocks.len() - 1000;
                    blocks.drain(0..drain);
                }
            }
            self.refresh_command_marks();
            if !command_text.is_empty()
                && !self.is_oxterm_command(&command_text)
                && settings().get_bool("history_enabled")
            {
                let ticket = history().add_command_start_async(&command_text, &self.get_cwd());
                *self.imp().osc133_last_history_id.borrow_mut() = ticket;
                *self.imp().input_shadow.borrow_mut() = String::new();
            }
        } else if cmd == 'D' {
            self.imp().pending_command_row.set(-1);
            let exit_code = rest.trim().parse::<i64>().unwrap_or(0);
            *self.imp().osc133_last_exit.borrow_mut() = exit_code;
            self.set_remote_command_state(false);
            {
                let (_, end_row) = self.vte().cursor_position();
                if let Some(block) = self
                    .imp()
                    .command_blocks
                    .borrow_mut()
                    .iter_mut()
                    .rev()
                    .find(|block| !block.finished)
                {
                    block.exit_code = Some(exit_code);
                    block.end_row = end_row;
                    block.duration_ms = duration_ms;
                    block.finished = true;
                }
            }
            // Only a command that actually started (an OSC 133 `C`) completes
            // a `wait`. The prompt marker for the initial shell prompt has no
            // preceding command and must stay invisible to it.
            if duration_ms.is_some() {
                self.imp()
                    .command_generation
                    .set(self.imp().command_generation.get() + 1);
            }
            self.refresh_command_marks();
            if let Some(id) = self.imp().osc133_last_history_id.borrow_mut().take() {
                history().set_command_result_async(id, exit_code, duration_ms);
            }
            if *self.imp().bell_notify_cmd_running.borrow() {
                *self.imp().bell_notify_cmd_running.borrow_mut() = false;
                self.trigger_bell_notification(exit_code);
            }
        } else if cmd == 'A' {
            *self.imp().osc133_cmd_start_row.borrow_mut() = -1;
            self.imp().pending_command_row.set(-1);
            self.imp().last_prompt_row.set(cursor_row);
            let last_exit = *self.imp().osc133_last_exit.borrow();
            if cursor_row > 0 {
                let mut markers = self.imp().osc133_markers.borrow_mut();
                markers.push((cursor_row, "prompt".into(), last_exit));
                if markers.len() > 1000 {
                    let drain = markers.len() - 1000;
                    markers.drain(0..drain);
                }
                drop(markers);
                self.update_margin_visibility();
            }
            self.refresh_command_marks();
        }
    }

    fn update_margin_visibility(&self) {
        if !self.imp().osc133_markers.borrow().is_empty() {
            if let Some(margin) = self.imp().osc133_margin.borrow().clone() {
                margin.set_visible(true);
                margin.queue_draw();
            }
        }
    }

    /// Show or hide the OSC 133 gutter. Hiding it after it has been reserved
    /// gives the width back to the terminal, so this is only used for
    /// terminals that never receive prompt markers (see `launch`).
    fn set_osc133_gutter_visible(&self, visible: bool) {
        if let Some(margin) = self.imp().osc133_margin.borrow().clone() {
            if margin.is_visible() != visible {
                margin.set_visible(visible);
            }
        }
    }

    fn draw_osc133_margin(&self, widget: &gtk::DrawingArea, cr: &cairo::Context) {
        let width = widget.allocated_width() as f64;
        let height = widget.allocated_height() as f64;
        if width < 2.0 {
            return;
        }
        // Match the terminal background so unmarked rows blend in.
        let bg = hex_to_rgba(&settings().get_bg_color());
        cr.set_source_rgba(bg.red(), bg.green(), bg.blue(), 1.0);
        cr.rectangle(0.0, 0.0, width, height);
        let _ = cr.fill();

        // Borrow rather than clone: this runs on every redraw and the list
        // holds up to 1000 markers.
        let markers = self.imp().osc133_markers.borrow();
        if markers.is_empty() {
            return;
        }
        let vadj = self.vadjustment();
        let top = vadj.value();
        let page = vadj.page_size();
        if page <= 0.0 {
            return;
        }
        let char_height = height / page;
        for (row, mtype, exit_code) in markers.iter() {
            if mtype != "prompt" {
                continue;
            }
            let rel_row = *row as f64 - top;
            if rel_row < -1.0 || rel_row > page {
                continue;
            }
            let y = rel_row * char_height;
            if *exit_code == 0 {
                cr.set_source_rgba(0.2, 0.75, 0.2, 0.7);
            } else {
                cr.set_source_rgba(0.85, 0.25, 0.25, 0.7);
            }
            cr.rectangle(1.0, y, width - 2.0, char_height);
            let _ = cr.fill();
        }
        drop(markers);

        // User bookmarks set from VI copy mode (`mm`) are drawn as a small
        // diamond in the same gutter, so prompts and bookmarks can be told
        // apart at a glance.
        for row in self.imp().vi_user_marks.borrow().iter() {
            let rel_row = *row as f64 - top;
            if rel_row < -1.0 || rel_row > page {
                continue;
            }
            let y = rel_row * char_height + char_height / 2.0;
            let cx = width / 2.0;
            cr.set_source_rgba(0.30, 0.78, 0.95, 0.9);
            cr.move_to(cx, y - 3.0);
            cr.line_to(cx + 3.0, y);
            cr.line_to(cx, y + 3.0);
            cr.line_to(cx - 3.0, y);
            cr.close_path();
            let _ = cr.fill();
        }
    }

    /// Refresh the two chrome pieces driven by OSC 133 command blocks.
    fn refresh_command_marks(&self) {
        if let Some(margin) = self.imp().osc133_margin.borrow().clone() {
            margin.queue_draw();
        }
        if let Some(marks) = self.imp().scrollbar_marks.borrow().clone() {
            marks.queue_draw();
        }
        self.update_sticky_header();
    }

    /// Text for the pinned header, kept free of terminal control characters so
    /// a command line cannot smuggle markup into the label.
    fn sticky_header_text(block: &CommandBlock) -> String {
        let mut command: String = block
            .command
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        command = command.split_whitespace().collect::<Vec<_>>().join(" ");
        if command.is_empty() {
            command = "(shell)".to_string();
        }
        if command.chars().count() > 160 {
            command = command.chars().take(159).collect::<String>() + "…";
        }
        if !block.finished {
            let elapsed = block
                .started_at
                .map(|started| started.elapsed().as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            return format!("● {}  —  running {}", command, format_duration_ms(elapsed));
        }
        let exit = block.exit_code.unwrap_or(0);
        if exit == 0 {
            match block.duration_ms {
                Some(ms) => format!("✓ {}  —  {}", command, format_duration_ms(ms)),
                None => format!("✓ {}", command),
            }
        } else {
            match block.duration_ms {
                Some(ms) => format!(
                    "✗ {}  —  exit {} · {}",
                    command,
                    exit,
                    format_duration_ms(ms)
                ),
                None => format!("✗ {}  —  exit {}", command, exit),
            }
        }
    }

    /// The command block whose output is at the top of the viewport. When the
    /// viewport is above the oldest block, the oldest one is used so the header
    /// still says where the visible output came from.
    fn sticky_block(&self) -> Option<CommandBlock> {
        let blocks = self.imp().command_blocks.borrow();
        if blocks.is_empty() {
            return None;
        }
        let top = self.vadjustment().value() as i64;
        blocks
            .iter()
            .rev()
            .find(|block| block.prompt_row <= top || block.command_row <= top)
            .or_else(|| blocks.first())
            .cloned()
    }

    fn update_sticky_header(&self) {
        let (Some(header), Some(label)) = (
            self.imp().sticky_header.borrow().clone(),
            self.imp().sticky_label.borrow().clone(),
        ) else {
            return;
        };
        let vadj = self.vadjustment();
        let page = vadj.page_size();
        let upper = vadj.upper();
        let top = vadj.value();
        let block = {
            let blocks = self.imp().command_blocks.borrow();
            if blocks.is_empty() {
                if header.is_visible() {
                    header.hide();
                }
                return;
            }
            // Show only while scrolled back from the live end of the buffer,
            // and never on the alternate screen (where there is no scrollback).
            let follow_start = (upper - page).max(0.0);
            if upper <= page + 0.5 || top + 0.5 >= follow_start {
                if header.is_visible() {
                    header.hide();
                }
                return;
            }
            let row = top as i64;
            blocks
                .iter()
                .rev()
                .find(|block| block.prompt_row <= row || block.command_row <= row)
                .or_else(|| blocks.first())
                .cloned()
        };
        let Some(block) = block else {
            header.hide();
            return;
        };
        let ok = block.finished && block.exit_code.unwrap_or(0) == 0;
        for class in [
            "oxterm-sticky-ok",
            "oxterm-sticky-fail",
            "oxterm-sticky-run",
        ] {
            label.style_context().remove_class(class);
        }
        label.style_context().add_class(if !block.finished {
            "oxterm-sticky-run"
        } else if ok {
            "oxterm-sticky-ok"
        } else {
            "oxterm-sticky-fail"
        });
        label.set_text(&Self::sticky_header_text(&block));
        if !header.is_visible() {
            header.show();
            label.show();
        }
        header.queue_draw();
    }

    fn scroll_to_sticky_block(&self) {
        if let Some(block) = self.sticky_block() {
            let vadj = self.vadjustment();
            let bottom = (vadj.upper() - vadj.page_size()).max(0.0);
            let target = block.command_row.max(block.prompt_row) as f64;
            vadj.set_value(target.clamp(0.0, bottom));
        }
    }

    fn draw_scrollbar_marks(&self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        let width = area.allocated_width() as f64;
        let height = area.allocated_height() as f64;
        if width < 2.0 || height < 4.0 {
            return;
        }
        let vadj = self.vadjustment();
        let lower = vadj.lower();
        let upper = vadj.upper();
        let page = vadj.page_size();
        let total = upper - lower;
        if total <= page + 0.5 {
            return;
        }
        let y_of = |row: i64| {
            let rel = (row as f64 - lower) / total;
            (rel * height).clamp(0.0, (height - 2.0).max(0.0))
        };
        let x = if settings().get_str("scrollbar_position") == "left" {
            1.0
        } else {
            (width - 5.0).max(0.0)
        };
        let draw = |row: i64, color: (f64, f64, f64, f64)| {
            let y = y_of(row);
            cr.set_source_rgba(color.0, color.1, color.2, color.3);
            cr.rectangle(x, y, 4.0, 3.0);
            let _ = cr.fill();
        };
        for block in self.imp().command_blocks.borrow().iter() {
            if !block.finished {
                draw(block.prompt_row, (0.95, 0.78, 0.25, 0.95));
            } else if block.exit_code.unwrap_or(0) != 0 {
                draw(block.prompt_row, (0.90, 0.30, 0.30, 0.95));
            }
        }
        for row in self.imp().vi_user_marks.borrow().iter() {
            draw(*row, (0.30, 0.78, 0.95, 0.95));
        }
    }

    fn on_child_exited(&self, status: i32) {
        *self.imp().pid.borrow_mut() = -1;
        self.close_osc133_fifo();
        self.cleanup_shell_integration();
        self.vte()
            .feed(format!("\r\n\x1b[33m{}\x1b[0m\r\n", child_status_message(status)).as_bytes());
        // With `--hold`, keep the terminal on screen after the command exits so
        // its final output stays visible (as in kitty/xterm --hold).
        if *self.imp().hold.borrow() {
            self.vte()
                .feed(b"\x1b[33m[Oxterm: --hold active, close this tab manually]\x1b[0m\r\n");
            return;
        }
        let weak = crate::SendWeak::new(self);
        glib::idle_add_local(move || {
            if let Some(t) = weak.upgrade() {
                t.call_window(|win| win.close_tab_signal(Some(&t)));
            }
            glib::ControlFlow::Break
        });
    }

    fn on_selection_changed(&self, term: &zoha_vte::Terminal) {
        if settings().get_bool("auto_copy_selection") && term.has_selection() {
            term.copy_clipboard_format(Format::Text);
        }
    }

    fn on_title_changed(&self, term: &zoha_vte::Terminal) {
        if let Some(title) = term.window_title() {
            let title = title.to_string();
            if !title.is_empty() {
                self.call_window(|win| win.set_tab_title_from_terminal(self, &title));
            }
        }
    }

    // ── Context menu & URL ───────────────────────────────────

    fn pixel_to_cell(&self, x: f64, y: f64) -> (i64, i64) {
        let vte = self.vte();
        let cw = vte.char_width();
        let ch = vte.char_height();
        if cw <= 0 || ch <= 0 {
            return (0, 0);
        }
        // Button events are relative to the VTE widget, so its GTK margin is
        // already excluded. Only the terminal's CSS content padding remains.
        let col = (((x - 8.0) / cw as f64).floor() as i64).max(0);
        let row = (y / ch as f64).floor() as i64;
        (col, row)
    }

    fn url_at_position(&self, x: f64, y: f64) -> Option<String> {
        let (col, row) = self.pixel_to_cell(x, y);
        let (matched, _tag) = self.vte().match_check(col, row);
        if let Some(text) = matched {
            return safe_web_url(text.as_str());
        }
        None
    }

    fn url_from_text_at(&self, x: f64, y: f64) -> Option<String> {
        let (col, row) = self.pixel_to_cell(x, y);
        // pixel_to_cell yields a viewport row; VTE text ranges use absolute
        // buffer rows, so offset by the current scroll position.
        let scroll_top = self.vadjustment().value() as i64;
        let row = row + scroll_top;
        let vte = self.vte();
        let text = text::range(&vte, row, 0, row, -1).unwrap_or_default();
        if text.is_empty() {
            return None;
        }
        let line = text.trim_end_matches('\n');
        let col = col as usize;
        for m in url_re().find_iter(line) {
            let start = char_column(line, m.start());
            let end = char_column(line, m.end());
            if start <= col && col <= end {
                return safe_web_url(m.as_str());
            }
        }
        None
    }

    fn open_url(&self, url: &str) {
        let Some(url) = safe_web_url(url) else {
            self.vte().feed(
                b"\r\n\x1b[31mCannot open URL: only HTTP and HTTPS links are allowed.\x1b[0m\r\n",
            );
            return;
        };
        if crate::notes::which("xdg-open").is_none() {
            self.vte()
                .feed(b"\r\n\x1b[31mCannot open URL: xdg-open is not installed.\x1b[0m\r\n");
            return;
        }
        crate::notes::spawn_detached("xdg-open", &[&url]);
    }

    fn copy_url(&self, url: &str) {
        if url.is_empty() {
            return;
        }
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        clipboard.set_text(url);
    }

    fn show_context_menu(&self, px: f64, py: f64) {
        let menu = gtk::Menu::new();
        let vte = self.vte();
        let has_selection = vte.has_selection();
        let read_only = !vte.is_input_enabled();
        let last_command = self.last_command();
        let has_command_output = self.get_command_output_range().0.is_some();
        let url = self
            .url_at_position(px, py)
            .or_else(|| self.url_from_text_at(px, py));

        // ── Clipboard ────────────────────────────────────────
        // The few actions used all the time stay at the top level; the rest is
        // grouped into submenus so the menu never turns into a long list.
        let copy_item = menu_action(&menu, self, "Copy", |t| t.copy());
        copy_item.set_sensitive(has_selection);
        copy_item.set_tooltip_text(Some("Copy the selection (Ctrl+Shift+C)"));

        menu_action(&menu, self, "Paste", |t| t.paste())
            .set_tooltip_text(Some("Paste the clipboard contents (Ctrl+Shift+V)"));
        menu_action(&menu, self, "Paste Selection", |t| t.paste_selection())
            .set_tooltip_text(Some("Paste the primary selection (middle-click paste)"));
        menu_action(&menu, self, "Select All", |t| t.select_all())
            .set_tooltip_text(Some("Select the whole scrollback (Ctrl+Shift+A)"));

        // ── Clipboard variants ───────────────────────────────
        let copy_menu = menu_submenu(&menu, "Copy As");
        let copy_html_item = menu_action(&copy_menu, self, "HTML", |t| {
            t.vte().copy_clipboard_format(Format::Html)
        });
        copy_html_item.set_sensitive(has_selection);
        copy_html_item.set_tooltip_text(Some("Copy the selection with its colours and styles"));

        menu_action(&copy_menu, self, "Whole Scrollback", |t| {
            t.select_all();
            t.copy();
        })
        .set_tooltip_text(Some("Copy the whole scrollback to the clipboard"));

        if !last_command.is_empty() || has_command_output {
            copy_menu.append(&gtk::SeparatorMenuItem::new());
        }
        if !last_command.is_empty() {
            let last = last_command.clone();
            menu_action(&copy_menu, self, "Last Command", move |_t| {
                gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&last);
            })
            .set_tooltip_text(Some("Copy the last command line to the clipboard"));
        }
        if has_command_output {
            menu_action(&copy_menu, self, "Last Command Output", |t| {
                t.copy_command_output()
            })
            .set_tooltip_text(Some(
                "Copy the output of the last command (shell integration)",
            ));
            menu_action(&copy_menu, self, "Open Last Output in Pager", |t| {
                t.open_command_output_in_pager()
            })
            .set_tooltip_text(Some(
                "Open the last command's output in a new tab with the configured pager (Ctrl+Shift+G)",
            ));
        }

        // ── Links (contextual) ───────────────────────────────
        if let Some(url) = url {
            menu.append(&gtk::SeparatorMenuItem::new());
            let url_copy = url.clone();
            let copy_url_item =
                menu_action(&menu, self, "Copy Link", move |t| t.copy_url(&url_copy));
            copy_url_item.set_tooltip_text(Some(&format!("Copy this URL: {}", url)));

            menu_action(&menu, self, "Open Link in Browser", move |t| {
                t.open_url(&url)
            });
        }

        // ── Notes ────────────────────────────────────────────
        let notes_menu = menu_submenu(&menu, "Notes");
        let add_note_item = menu_action(&notes_menu, self, "Add Selection to Note...", |t| {
            t.add_selection_to_note()
        });
        add_note_item.set_sensitive(has_selection);
        add_note_item.set_tooltip_text(Some("Append the selected text to a notes file"));
        let output_item = menu_window_action(&notes_menu, self, "Note Command Output...", |w| {
            w.note_add_command_output()
        });
        output_item.set_sensitive(has_command_output);
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        menu_window_action(&notes_menu, self, "New Note...", |w| w.note_new());
        menu_window_action(&notes_menu, self, "Toggle Notes Panel", |w| {
            w.note_toggle_panel()
        });
        menu_window_action(&notes_menu, self, "Open Notes Folder", |w| {
            w.note_open_folder()
        });

        // ── Tabs and panes ───────────────────────────────────
        let tab_menu = menu_submenu(&menu, "Tab");
        menu_window_action(&tab_menu, self, "New Tab", |w| w.new_tab_signal());

        let rename_tab_item = gtk::MenuItem::with_label("Rename Tab...");
        let weak = crate::SendWeak::new(self);
        rename_tab_item.connect_activate(move |_| {
            if let Some(t) = weak.upgrade() {
                t.call_window(|w| w.rename_tab_signal(&t));
            }
        });
        tab_menu.append(&rename_tab_item);

        let detach_tab_item = gtk::MenuItem::with_label("Detach Tab");
        detach_tab_item.set_tooltip_text(Some("Move this tab into its own window"));
        let weak = crate::SendWeak::new(self);
        detach_tab_item.connect_activate(move |_| {
            if let Some(t) = weak.upgrade() {
                t.call_window(|w| w.detach_tab_signal(&t));
            }
        });
        tab_menu.append(&detach_tab_item);

        let close_tab_item = gtk::MenuItem::with_label("Close Tab");
        let weak = crate::SendWeak::new(self);
        close_tab_item.connect_activate(move |_| {
            if let Some(t) = weak.upgrade() {
                t.call_window(|w| w.close_tab_signal(Some(&t)));
            }
        });
        tab_menu.append(&close_tab_item);

        tab_menu.append(&gtk::SeparatorMenuItem::new());
        let split_menu = menu_submenu(&tab_menu, "Split");
        menu_window_action(&split_menu, self, "Vertically", |w| {
            w.split_signal("vertical")
        });
        menu_window_action(&split_menu, self, "Horizontally", |w| {
            w.split_signal("horizontal")
        });
        menu_window_action(&split_menu, self, "Unsplit", |w| w.split_signal("single"));

        // ── Tools ────────────────────────────────────────────
        let tools_menu = menu_submenu(&menu, "Tools");
        menu_action(&tools_menu, self, "Search Scrollback...", |t| {
            t.show_search()
        })
        .set_tooltip_text(Some("Search in the scrollback buffer (Ctrl+Shift+F)"));
        menu_action(&tools_menu, self, "Command Bar...", |t| {
            t.show_command_bar()
        })
        .set_tooltip_text(Some("Run an Oxterm /command (Ctrl+Shift+P)"));
        menu_action(&tools_menu, self, "History Search...", |t| {
            t.start_history_search()
        })
        .set_tooltip_text(Some("Search command history (Ctrl+R)"));

        let quickmarks = menu_submenu(&tools_menu, "Quickmarks");
        menu_action(&quickmarks, self, "Set Quickmark", |t| t.set_quickmark())
            .set_tooltip_text(Some("Bookmark this position (Ctrl+Shift+M)"));
        let mark_count = self.imp().quickmarks.borrow().len();
        if mark_count > 0 {
            menu_action(&quickmarks, self, "Jump to Next Quickmark", |t| {
                t.jump_next_quickmark()
            });
            let clear_label = format!("Clear All Quickmarks ({})", mark_count);
            menu_action(&quickmarks, self, &clear_label, |t| {
                t.remove_all_quickmarks()
            });
        }

        let snippets = crate::settings::json_to_str_map(&settings().get_obj("snippets"));
        if !snippets.is_empty() {
            let snippet_menu = menu_submenu(&tools_menu, "Insert Snippet");
            for (name, command) in snippets {
                let label = format!("{}  —  {}", name, ellipsize(&command, 42));
                let item = gtk::MenuItem::with_label(&label);
                item.set_tooltip_text(Some(&command));
                let weak = crate::SendWeak::new(self);
                item.connect_activate(move |_| {
                    if let Some(t) = weak.upgrade() {
                        // Replace whatever is on the prompt, like /snippet does.
                        t.feed_command_bytes(b"\x15");
                        *t.imp().input_shadow.borrow_mut() = command.clone();
                        t.vte().feed_child(command.as_bytes());
                        t.vte().grab_focus();
                    }
                });
                snippet_menu.append(&item);
            }
        }

        tools_menu.append(&gtk::SeparatorMenuItem::new());
        menu_action(&tools_menu, self, "Open File Manager Here", |t| t.open_fm())
            .set_tooltip_text(Some("Open the working directory in the file manager"));
        menu_action(&tools_menu, self, "Reset Terminal", |t| t.reset(false))
            .set_tooltip_text(Some("Reset the terminal state (Ctrl+Shift+R)"));
        menu_action(&tools_menu, self, "Clear Scrollback", |t| t.reset(true)).set_tooltip_text(
            Some("Reset the terminal and clear the scrollback (Ctrl+Shift+X)"),
        );

        // ── View ─────────────────────────────────────────────
        let view_menu = menu_submenu(&menu, "View");
        menu_action(&view_menu, self, "Zoom In", |t| t.zoom_in())
            .set_tooltip_text(Some("Increase the font size (Ctrl++)"));
        menu_action(&view_menu, self, "Zoom Out", |t| t.zoom_out())
            .set_tooltip_text(Some("Decrease the font size (Ctrl+-)"));
        menu_action(&view_menu, self, "Reset Zoom", |t| t.zoom_reset())
            .set_tooltip_text(Some("Reset the font scale to 100% (Ctrl+0)"));
        view_menu.append(&gtk::SeparatorMenuItem::new());
        menu_action(&view_menu, self, "Full Screen", |t| {
            if let Some(win) = t.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok()) {
                let fullscreen = win
                    .window()
                    .map(|gw| gw.state().contains(gdk::WindowState::FULLSCREEN))
                    .unwrap_or(false);
                if fullscreen {
                    win.unfullscreen();
                } else {
                    win.fullscreen();
                }
            }
        })
        .set_tooltip_text(Some("Toggle full-screen mode (F11)"));

        // ── Terminal modes ───────────────────────────────────
        menu.append(&gtk::SeparatorMenuItem::new());
        let cast_item = gtk::CheckMenuItem::with_label("Broadcast Input");
        cast_item.set_active(settings().get_bool("broadcast_input"));
        cast_item.set_tooltip_text(Some("Send typed input to every terminal in this window"));
        let weak = crate::SendWeak::new(self);
        cast_item.connect_activate(move |b| {
            if weak.upgrade().is_some() {
                let s = settings();
                let _ = s.set("broadcast_input", serde_json::Value::Bool(b.is_active()));
                // Refresh every terminal's cached broadcast flag.
                s.notify_changed();
            }
        });
        menu.append(&cast_item);

        let ro_item = gtk::CheckMenuItem::with_label("Read-Only");
        ro_item.set_active(read_only);
        ro_item.set_tooltip_text(Some("Block keyboard input to this terminal"));
        let weak = crate::SendWeak::new(self);
        ro_item.connect_activate(move |b| {
            if let Some(t) = weak.upgrade() {
                t.set_read_only(b.is_active());
            }
        });
        menu.append(&ro_item);

        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    fn add_selection_to_note(&self) {
        let vte = self.vte();
        vte.copy_clipboard_format(Format::Text);
        // Read the selection directly instead of round-tripping through the
        // clipboard, which would spin a nested main loop.
        self.call_window(|w| w.note_add_selection());
    }

    fn open_fm(&self) {
        if let Some(fm) = crate::window::detect_file_manager() {
            let cwd = self.get_cwd();
            crate::notes::spawn_detached(&fm, &[&cwd]);
        }
    }

    fn is_oxterm_command(&self, shadow: &str) -> bool {
        let value = shadow.trim();
        OXTERM_COMMANDS.iter().any(|cmd| {
            value == format!("/{}", cmd)
                || value.starts_with(&format!("/{} ", cmd))
                || value.starts_with(&format!("/{}\t", cmd))
        })
    }

    fn shadow_paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.is_echo_on() {
            *self.imp().input_shadow.borrow_mut() = String::new();
            *self.imp().shadow_anchor.borrow_mut() = None;
            return;
        }
        let input_empty = self.imp().input_shadow.borrow().is_empty()
            && self.imp().shadow_anchor.borrow().is_none();
        if input_empty {
            let pos = self.vte().cursor_position();
            *self.imp().shadow_anchor.borrow_mut() = Some(pos);
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized.split('\n').collect();
        {
            let mut shadow = self.imp().input_shadow.borrow_mut();
            shadow.push_str(lines[0]);
        }
        if lines.len() > 1 {
            let mut shadow = self.imp().input_shadow.borrow_mut();
            let first = shadow.clone();
            shadow.clear();
            let mut to_add = vec![first];
            to_add.extend(lines[1..lines.len() - 1].iter().map(|l| l.to_string()));
            for cmd in to_add {
                let cmd = cmd.trim().to_string();
                if !cmd.is_empty()
                    && !self.is_oxterm_command(&cmd)
                    && settings().get_bool("history_enabled")
                {
                    history().add_async(&cmd, &self.get_cwd(), -1);
                }
            }
            *shadow = lines[lines.len() - 1].to_string();
        }
    }

    fn get_real_command_text(&self) -> String {
        let anchor = *self.imp().shadow_anchor.borrow();
        let fallback = self.imp().input_shadow.borrow().clone();
        let Some((start_col, start_row)) = anchor else {
            return fallback.trim().to_string();
        };
        let (end_col, end_row) = self.vte().cursor_position();
        let text = text::range(&self.vte(), start_row, start_col, end_row, end_col);
        match text {
            Some(t) if !t.is_empty() => t.trim().to_string(),
            _ => fallback.trim().to_string(),
        }
    }

    // ── Callbacks: button / key ──────────────────────────────

    fn on_button_press(&self, event: &gdk::EventButton) -> glib::Propagation {
        if event.button() == 2 {
            self.paste_selection();
            return glib::Propagation::Stop;
        }
        if event.button() != 1 && event.button() != 3 {
            return glib::Propagation::Proceed;
        }
        if event.button() == 3 {
            let (px, py) = event.position();
            self.show_context_menu(px, py);
            return glib::Propagation::Stop;
        }
        let (px, py) = event.position();
        let url = self
            .url_at_position(px, py)
            .or_else(|| self.url_from_text_at(px, py));
        if let Some(url) = url {
            self.open_url(&url);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn on_key_press(&self, event: &gdk::EventKey) -> glib::Propagation {
        let state = event.state();
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::MOD1_MASK);
        let key = event.keyval();

        // A second Tab is meaningful only when it immediately follows the
        // first one. Any other key starts a new completion interaction.
        if key != K::Tab {
            *self.imp().tab_fallback_pending_before.borrow_mut() = None;
        }

        // Any key other than Esc cancels the short post-picker window during
        // which extra Esc presses are swallowed.
        if key != K::Escape {
            *self.imp().history_esc_deadline.borrow_mut() = 0;
        }

        // The picker owns every key while open. Letting global terminal
        // shortcuts through would send invisible control characters to the shell.
        if *self.imp().history_search_mode.borrow() {
            return self.handle_history_search_key(event);
        }

        // Just after the history picker was dismissed, absorb further Esc
        // presses instead of forwarding them to the shell (a lone Esc starts
        // a readline meta sequence and can leave the prompt in a weird state).
        if key == K::Escape && *self.imp().history_esc_deadline.borrow() > mono_us() {
            return glib::Propagation::Stop;
        }

        if *self.imp().tmux_prefix.borrow() {
            // Shift (for % and ") arrives as its own key event; keep waiting
            // for the actual command key.
            if event.is_modifier() {
                return glib::Propagation::Stop;
            }
            *self.imp().tmux_prefix.borrow_mut() = false;
            return self.handle_tmux_prefix_key(event);
        }
        if ctrl
            && !shift
            && (key == K::b || key == K::B)
            && settings().get_bool("tmux_prefix_enabled")
        {
            *self.imp().tmux_prefix.borrow_mut() = true;
            return glib::Propagation::Stop;
        }

        let idle_state = self.imp().input_shadow.borrow().is_empty()
            && self.imp().shadow_anchor.borrow().is_none()
            && !*self.imp().cmd_bar_visible.borrow()
            && !*self.imp().history_search_mode.borrow()
            && !*self.imp().hints_active.borrow()
            && !*self.imp().vi_copy_active.borrow();
        if idle_state {
            *self.imp().shadow_anchor.borrow_mut() = Some(self.vte().cursor_position());
        }

        if ctrl && shift {
            if *self.imp().hints_active.borrow() {
                if key == K::h || key == K::H {
                    self.deactivate_hints();
                }
                return glib::Propagation::Stop;
            }
            if *self.imp().vi_copy_active.borrow() {
                if key == K::y || key == K::Y {
                    self.deactivate_vi_copy();
                }
                return glib::Propagation::Stop;
            }
            if key == K::c || key == K::C {
                self.copy();
                return glib::Propagation::Stop;
            }
            if key == K::v || key == K::V {
                self.paste();
                return glib::Propagation::Stop;
            }
            if key == K::w || key == K::W {
                self.call_window(|win| win.close_tab_signal(None));
                return glib::Propagation::Stop;
            }
            if key == K::n || key == K::N {
                self.call_window(|win| win.new_window_signal());
                return glib::Propagation::Stop;
            }
            if key == K::t || key == K::T {
                self.call_window(|win| win.new_tab_signal());
                return glib::Propagation::Stop;
            }
            if key == K::q || key == K::Q {
                self.call_window(|win| win.close_window_signal());
                return glib::Propagation::Stop;
            }
            if key == K::s || key == K::S {
                self.call_window(|win| win.set_title_dialog());
                return glib::Propagation::Stop;
            }
            if key == K::r || key == K::R {
                self.call_window(|win| win.reset_terminal());
                return glib::Propagation::Stop;
            }
            if key == K::x || key == K::X {
                self.call_window(|win| win.reset_and_clear());
                return glib::Propagation::Stop;
            }
            if key == K::a || key == K::A {
                self.select_all();
                return glib::Propagation::Stop;
            }
            if key == K::e || key == K::E {
                self.call_window(|win| win.split_signal("vertical"));
                return glib::Propagation::Stop;
            }
            if key == K::d || key == K::D {
                self.call_window(|win| win.split_signal("horizontal"));
                return glib::Propagation::Stop;
            }
            if key == K::Up {
                self.scroll_to_osc133_prompt(true);
                return glib::Propagation::Stop;
            }
            if key == K::Down {
                self.scroll_to_osc133_prompt(false);
                return glib::Propagation::Stop;
            }
            if key == K::f || key == K::F {
                self.show_search();
                return glib::Propagation::Stop;
            }
            if key == K::m || key == K::M {
                self.set_quickmark();
                return glib::Propagation::Stop;
            }
            if key == K::g || key == K::G {
                self.open_command_output_in_pager();
                return glib::Propagation::Stop;
            }
            if key == K::b || key == K::B {
                let current = settings().get_bool("broadcast_input");
                let s = settings();
                let _ = s.set("broadcast_input", serde_json::Value::Bool(!current));
                s.notify_changed();
                let status = if !current { "ON" } else { "OFF" };
                self.vte()
                    .feed(format!("\r\n\x1b[33mBroadcast input: {}\x1b[0m\r\n", status).as_bytes());
                return glib::Propagation::Stop;
            }
            if key == K::p || key == K::P {
                self.show_command_bar();
                return glib::Propagation::Stop;
            }
            if key == K::h || key == K::H {
                if settings().get_bool("hint_mode_enabled") {
                    self.activate_hints();
                }
                return glib::Propagation::Stop;
            }
            if key == K::y || key == K::Y {
                if settings().get_bool("vi_copy_mode_enabled") {
                    self.activate_vi_copy();
                }
                return glib::Propagation::Stop;
            }
        }

        if *self.imp().hints_active.borrow() {
            self.handle_hint_key(event);
            return glib::Propagation::Stop;
        }

        if *self.imp().vi_copy_active.borrow() {
            self.handle_vi_copy_key(event);
            return glib::Propagation::Stop;
        }

        if ctrl && !shift {
            if key == K::r || key == K::R {
                // Do not steal application shortcuts from an interactive
                // foreground process (for example opencode's Ctrl+R reload).
                if self.has_active_process() {
                    return glib::Propagation::Proceed;
                }
                self.start_history_search();
                return glib::Propagation::Stop;
            }
            if key == K::d || key == K::D {
                if *self.imp().pid.borrow() == -1 {
                    self.call_window(|win| win.close_tab_signal(None));
                } else if *self.imp().pty_fd.borrow() >= 0 {
                    unsafe {
                        libc::write(*self.imp().pty_fd.borrow(), b"\x04".as_ptr() as *const _, 1)
                    };
                } else {
                    self.feed_command_bytes(b"\x04");
                }
                return glib::Propagation::Stop;
            }
            if key == K::l || key == K::L {
                self.feed_command_bytes(b"\x0c");
                return glib::Propagation::Stop;
            }
            if key == K::u || key == K::U {
                self.feed_command_bytes(b"\x15");
                *self.imp().input_shadow.borrow_mut() = String::new();
                self.exit_history_search_mode();
                return glib::Propagation::Stop;
            }
            if key == K::w || key == K::W {
                self.feed_command_bytes(b"\x17");
                {
                    let mut shadow = self.imp().input_shadow.borrow_mut();
                    if let Some(pos) = shadow.rfind(' ') {
                        shadow.truncate(pos);
                    } else {
                        shadow.clear();
                    }
                }
                return glib::Propagation::Stop;
            }
            if key == K::c || key == K::C {
                let real_text = self.get_real_command_text();
                if !real_text.is_empty()
                    && !self.is_oxterm_command(&real_text)
                    && settings().get_bool("history_enabled")
                {
                    history().add_async(&real_text, &self.get_cwd(), -1);
                }
                self.feed_command_bytes(b"\x03");
                *self.imp().input_shadow.borrow_mut() = String::new();
                *self.imp().shadow_anchor.borrow_mut() = None;
                self.exit_history_search_mode();
                return glib::Propagation::Stop;
            }
            if key == K::plus || key == K::equal {
                self.zoom_in();
                return glib::Propagation::Stop;
            }
            if key == K::minus {
                self.zoom_out();
                return glib::Propagation::Stop;
            }
            if key == K::_0 {
                self.zoom_reset();
                return glib::Propagation::Stop;
            }
            if key == K::m || key == K::M {
                self.jump_next_quickmark();
                return glib::Propagation::Stop;
            }
        }

        if ctrl && alt && (key == K::o || key == K::O) {
            self.call_window(|win| win.focus_other_pane_signal());
            return glib::Propagation::Stop;
        }

        if alt && !ctrl {
            let num = (*key as i64) - (*K::_0 as i64);
            // Only take Alt+digit at the shell prompt; readline, editors and
            // other programs use it themselves.
            if (1..=9).contains(&num) && !self.has_active_process() {
                self.replay_history_number(num);
                return glib::Propagation::Stop;
            }
        }

        // Password prompts disable terminal echo. Never mirror that input in
        // the command shadow, which is used by history and session metadata.
        let echo_on = self.is_echo_on();
        if !echo_on {
            *self.imp().input_shadow.borrow_mut() = String::new();
            *self.imp().shadow_anchor.borrow_mut() = None;
        }

        if key == K::Tab && self.imp().input_shadow.borrow().starts_with('/') {
            self.autocomplete_oxterm();
            return glib::Propagation::Stop;
        }

        if key == K::Tab {
            let shadow = self.imp().input_shadow.borrow().clone();
            if shadow.ends_with(' ') && shadow.trim_end() == "ssh" {
                self.start_history_tab_complete(true);
                return glib::Propagation::Stop;
            }
            if !shadow.trim().is_empty() {
                let now = mono_us();
                let double_tab = {
                    let pending = self.imp().tab_fallback_pending_before.borrow().clone();
                    let time = *self.imp().tab_fallback_pending_time.borrow();
                    pending.is_some()
                        && now - time < 600_000
                        && shadow.trim_end_matches('\t') == pending.as_deref().unwrap_or("")
                };
                if double_tab {
                    *self.imp().tab_fallback_pending_before.borrow_mut() = None;
                    self.start_history_tab_complete(true);
                    return glib::Propagation::Stop;
                } else {
                    *self.imp().tab_fallback_pending_before.borrow_mut() = Some(shadow.clone());
                    *self.imp().tab_fallback_pending_time.borrow_mut() = now;
                }
            }
            // Tab is a completion request, not text in the shell prompt. Keep
            // the shadow aligned with the line that is actually editable.
            return glib::Propagation::Proceed;
        }

        if key == K::Return || key == K::KP_Enter {
            let mut shadow = self.imp().input_shadow.borrow().trim().to_string();
            let real_text = self.get_real_command_text();
            if !self.is_oxterm_command(&shadow) && self.is_oxterm_command(&real_text) {
                shadow = real_text.clone();
            }
            if !shadow.is_empty() {
                let is_oxterm_cmd = self.is_oxterm_command(&shadow);
                if !is_oxterm_cmd {
                    let command = if real_text.is_empty() {
                        shadow.as_str()
                    } else {
                        real_text.as_str()
                    };
                    self.set_remote_command(command, false);
                }
                if settings().get_bool("history_enabled") {
                    let cwd = self.get_cwd();
                    if is_oxterm_cmd {
                        history().add_async(&shadow, &cwd, -1);
                    } else if !*self.imp().osc133_integration_active.borrow() {
                        let command = if real_text.is_empty() {
                            shadow.as_str()
                        } else {
                            real_text.as_str()
                        };
                        history().add_async(command, &cwd, -1);
                    }
                }
                *self.imp().shadow_anchor.borrow_mut() = None;
                if is_oxterm_cmd {
                    self.feed_command_bytes(b"\x15");
                }
                if is_oxterm_cmd {
                    if self.dispatch_oxterm_command(&shadow) {
                        // Let the shell redraw its prompt below the output.
                        self.vte().feed_child(b"\r");
                    }
                    *self.imp().input_shadow.borrow_mut() = String::new();
                    return glib::Propagation::Stop;
                }
            }
            *self.imp().input_shadow.borrow_mut() = String::new();
            let (_, row) = self.vte().cursor_position();
            self.imp().pending_command_row.set(row + 1);
        }

        if key == K::BackSpace {
            let bs = if self.imp().cached_backspace_binding.borrow().is_empty() {
                settings().get_str("backspace_binding")
            } else {
                self.imp().cached_backspace_binding.borrow().clone()
            };
            if bs == "control-h" {
                self.feed_command_bytes(b"\x08");
            } else if bs == "escape-sequence" {
                self.feed_command_bytes(b"\x1b\x7f");
            } else {
                self.feed_command_bytes(b"\x7f");
            }
            let mut shadow = self.imp().input_shadow.borrow_mut();
            if !shadow.is_empty() {
                shadow.pop();
            }
            return glib::Propagation::Stop;
        }

        if key == K::Delete {
            let dl = if self.imp().cached_delete_binding.borrow().is_empty() {
                settings().get_str("delete_binding")
            } else {
                self.imp().cached_delete_binding.borrow().clone()
            };
            if dl == "ascii-del" {
                self.feed_command_bytes(b"\x7f");
            } else if dl == "control-h" {
                self.feed_command_bytes(b"\x08");
            } else {
                self.feed_command_bytes(b"\x1b[3~");
            }
            return glib::Propagation::Stop;
        }

        let text = event_text(event);
        if echo_on && !text.is_empty() {
            let c = text.chars().next().unwrap();
            if c as u32 >= 0x20 {
                self.imp().input_shadow.borrow_mut().push(c);
            }
        }

        glib::Propagation::Proceed
    }

    fn handle_tmux_prefix_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        let text = event_text(event);
        if key == K::Escape {
            return glib::Propagation::Stop;
        }
        if key == K::b || key == K::B {
            self.feed_command_bytes(b"\x02");
        } else if key == K::c || key == K::C {
            self.call_window(|win| win.new_tab_signal());
        } else if key == K::n || key == K::N {
            self.call_window(|win| win.next_tab_signal());
        } else if key == K::p || key == K::P {
            self.call_window(|win| win.prev_tab_signal());
        } else if key == K::o || key == K::O {
            self.call_window(|win| win.focus_other_pane_signal());
        } else if key == K::s || key == K::S || key == K::w || key == K::W {
            self.call_window(|win| win.session_list_signal());
        } else if key == K::x || key == K::X {
            self.call_window(|win| win.close_tab_signal(None));
        } else if text == "%" {
            self.call_window(|win| win.split_signal("vertical"));
        } else if text == "\"" {
            self.call_window(|win| win.split_signal("horizontal"));
        } else if (key == K::d || key == K::D) && self.foreground_process_is_ssh() {
            if key == K::D {
                self.vte().feed_child(b"\x02D");
            } else {
                self.vte().feed_child(b"\x02d");
            }
        } else if key == K::d || key == K::D {
            self.detach_local_terminal();
        } else if key == K::question {
            self.vte().feed(
                b"\r\n\x1b[36mCtrl+B commands: c new tab, n/p next/previous tab, %/\" split, o other pane, d detach, x close, ? help\x1b[0m\r\n",
            );
        } else {
            // Not one of ours: hand Ctrl+B and this key to the program (emacs,
            // readline, a nested tmux) instead of silently eating both.
            self.feed_command_bytes(b"\x02");
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    }

    // ── Lazily built overlays ────────────────────────────────

    /// Build the command bar on first use. It is hidden by default, so there
    /// is no reason to pay for its widgets (entry, list, scroller, frame) in
    /// every terminal tab at startup.
    fn ensure_command_bar(&self) -> bool {
        if self.imp().cmd_entry.borrow().is_some() {
            return true;
        }
        let Some(overlay) = self.imp().overlay.borrow().clone() else {
            return false;
        };

        let cmd_bar_revealer = gtk::Revealer::new();
        cmd_bar_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        cmd_bar_revealer.set_transition_duration(150);
        cmd_bar_revealer.set_halign(gtk::Align::Fill);
        cmd_bar_revealer.set_valign(gtk::Align::End);
        cmd_bar_revealer.set_vexpand(false);
        cmd_bar_revealer.set_reveal_child(false);
        overlay.add_overlay(&cmd_bar_revealer);
        *self.imp().cmd_bar_revealer.borrow_mut() = Some(cmd_bar_revealer.clone());

        let cmd_bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cmd_bar.set_hexpand(true);
        cmd_bar.set_vexpand(false);
        cmd_bar_revealer.add(&cmd_bar);
        *self.imp().cmd_bar.borrow_mut() = Some(cmd_bar.clone());

        let outer_frame = gtk::Frame::new(None);
        outer_frame.set_shadow_type(gtk::ShadowType::EtchedOut);
        outer_frame.set_hexpand(true);
        outer_frame.set_vexpand(false);
        outer_frame.style_context().add_class("command-bar-frame");
        cmd_bar.pack_start(&outer_frame, true, true, 0);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer_frame.add(&inner);

        let cmd_entry = gtk::Entry::new();
        cmd_entry.set_has_frame(false);
        cmd_entry.set_hexpand(true);
        cmd_entry.set_size_request(-1, 36);
        cmd_entry.set_placeholder_text(Some("/command [args]…"));
        let weak = crate::SendWeak::new(self);
        cmd_entry.connect_changed(move |_e| {
            if let Some(t) = weak.upgrade() {
                t.on_cmd_bar_changed();
            }
        });
        let weak = crate::SendWeak::new(self);
        cmd_entry.connect_activate(move |_e| {
            if let Some(t) = weak.upgrade() {
                t.execute_from_bar();
            }
        });
        let weak = crate::SendWeak::new(self);
        cmd_entry.connect_key_press_event(move |_w, ev| {
            if let Some(t) = weak.upgrade() {
                return t.on_cmd_bar_key(ev);
            }
            glib::Propagation::Proceed
        });
        inner.pack_start(&cmd_entry, false, false, 0);
        *self.imp().cmd_entry.borrow_mut() = Some(cmd_entry);

        let cmd_list = gtk::ListBox::new();
        cmd_list.set_selection_mode(gtk::SelectionMode::Single);
        cmd_list.set_vexpand(false);
        let weak = crate::SendWeak::new(self);
        cmd_list.connect_row_activated(move |_l, row| {
            if let Some(t) = weak.upgrade() {
                t.on_cmd_bar_row_activated(row);
            }
        });
        let cmd_scroll =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        cmd_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        cmd_scroll.set_hexpand(true);
        cmd_scroll.set_vexpand(false);
        cmd_scroll.set_propagate_natural_height(true);
        cmd_scroll.set_max_content_height(200);
        cmd_scroll.add(&cmd_list);
        inner.pack_start(&cmd_scroll, true, true, 0);
        *self.imp().cmd_list.borrow_mut() = Some(cmd_list);

        cmd_bar_revealer.show_all();
        cmd_bar_revealer.set_reveal_child(false);
        true
    }

    /// Build the scrollback search bar on first use (Ctrl+Shift+F / menu).
    fn ensure_search_bar(&self) -> bool {
        if self.imp().search_entry.borrow().is_some() {
            return true;
        }
        let Some(overlay) = self.imp().overlay.borrow().clone() else {
            return false;
        };

        let search_revealer = gtk::Revealer::new();
        search_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        search_revealer.set_transition_duration(150);
        search_revealer.set_halign(gtk::Align::Fill);
        search_revealer.set_valign(gtk::Align::Start);
        search_revealer.set_reveal_child(false);
        overlay.add_overlay(&search_revealer);
        *self.imp().search_revealer.borrow_mut() = Some(search_revealer.clone());

        let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        search_box.set_size_request(-1, 32);
        let search_frame = gtk::Frame::new(None);
        search_frame.set_shadow_type(gtk::ShadowType::EtchedOut);
        search_frame.style_context().add_class("command-bar-frame");
        search_frame.add(&search_box);
        search_revealer.add(&search_frame);

        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some(
            "Search scrollback (Enter=next, Shift+Enter=prev, Esc=close)...",
        ));
        let weak = crate::SendWeak::new(self);
        search_entry.connect_search_changed(move |_e| {
            if let Some(t) = weak.upgrade() {
                t.do_search();
            }
        });
        let weak = crate::SendWeak::new(self);
        search_entry.connect_key_press_event(move |_w, ev| {
            if let Some(t) = weak.upgrade() {
                return t.on_search_key(ev);
            }
            glib::Propagation::Proceed
        });
        search_box.pack_start(&search_entry, true, true, 0);
        *self.imp().search_entry.borrow_mut() = Some(search_entry);

        let search_label = gtk::Label::new(Some(""));
        search_label.set_halign(gtk::Align::Start);
        search_box.pack_start(&search_label, false, false, 6);
        *self.imp().search_label.borrow_mut() = Some(search_label);

        let search_case_btn = gtk::ToggleButton::with_label("Aa");
        search_case_btn.set_tooltip_text(Some("Match case"));
        let weak = crate::SendWeak::new(self);
        search_case_btn.connect_clicked(move |_b| {
            if let Some(t) = weak.upgrade() {
                t.do_search();
            }
        });
        search_box.pack_start(&search_case_btn, false, false, 0);
        *self.imp().search_case_btn.borrow_mut() = Some(search_case_btn);

        let search_regex_btn = gtk::ToggleButton::with_label(".*");
        search_regex_btn.set_tooltip_text(Some("Regular expression"));
        let weak = crate::SendWeak::new(self);
        search_regex_btn.connect_clicked(move |_b| {
            if let Some(t) = weak.upgrade() {
                t.do_search();
            }
        });
        search_box.pack_start(&search_regex_btn, false, false, 0);
        *self.imp().search_regex_btn.borrow_mut() = Some(search_regex_btn);

        search_revealer.show_all();
        search_revealer.set_reveal_child(false);
        true
    }

    // ── Command bar ──────────────────────────────────────────

    pub fn show_command_bar(&self) {
        if *self.imp().cmd_bar_visible.borrow() {
            return;
        }
        if !self.ensure_command_bar() {
            return;
        }
        *self.imp().cmd_bar_visible.borrow_mut() = true;
        let revealer = self.imp().cmd_bar_revealer.borrow().clone().unwrap();
        revealer.set_reveal_child(true);
        let entry = self.imp().cmd_entry.borrow().clone().unwrap();
        entry.set_text("/");
        self.build_cmd_list("");
        entry.grab_focus();
        entry.select_region(-1, -1);
    }

    fn hide_command_bar(&self) {
        *self.imp().cmd_bar_visible.borrow_mut() = false;
        if let Some(revealer) = self.imp().cmd_bar_revealer.borrow().clone() {
            revealer.set_reveal_child(false);
        }
        *self.imp().input_shadow.borrow_mut() = String::new();
        self.vte().grab_focus();
    }

    fn on_cmd_bar_changed(&self) {
        if !self.ensure_command_bar() {
            return;
        }
        let entry = self.imp().cmd_entry.borrow().clone().unwrap();
        self.build_cmd_list(&entry.text());
    }

    fn build_cmd_list(&self, query: &str) {
        if !self.ensure_command_bar() {
            return;
        }
        let list = self.imp().cmd_list.borrow().clone().unwrap();
        for child in list.children() {
            list.remove(&child);
        }
        self.imp().cmd_row_map.borrow_mut().clear();
        let q = query.to_lowercase().trim_start_matches('/').to_string();
        let commands: &[(&str, &str)] = &[
            ("/history [terms | :sql SQL]", "Search command history"),
            (
                "/session export NAME [FILE]",
                "Export a saved session as JSON",
            ),
            ("/session name NAME", "Name the live terminal session"),
            (
                "/snippet [NAME args...]",
                "Expand a saved parameterized snippet",
            ),
            ("/wnotes [-file.md] <text>", "Save timestamped note"),
            ("/onotes [-file.md]", "Open notes in editor"),
            ("/help", "Show all commands and shortcuts"),
            ("/clear", "Clear the terminal screen"),
            ("/cls", "Clear the terminal screen"),
        ];
        let mut first: Option<gtk::ListBoxRow> = None;
        for (cmd, desc) in commands {
            if !q.is_empty() && !cmd.to_lowercase().contains(&q) {
                continue;
            }
            let row = gtk::ListBoxRow::new();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let lbl = gtk::Label::new(Some(&format!("{}  ", cmd)));
            lbl.set_xalign(0.0);
            lbl.set_halign(gtk::Align::Start);
            lbl.set_markup(&format!("<b>{}</b>", cmd));
            let desc_lbl = gtk::Label::new(Some(desc));
            desc_lbl.set_xalign(0.0);
            desc_lbl.set_halign(gtk::Align::Start);
            desc_lbl.style_context().add_class("dim-label");
            hbox.pack_start(&lbl, false, false, 0);
            hbox.pack_start(&desc_lbl, true, true, 0);
            row.add(&hbox);
            row.show_all();
            self.imp().cmd_row_map.borrow_mut().insert(
                row.as_ptr() as usize,
                cmd.split_whitespace().next().unwrap_or("").to_string(),
            );
            list.add(&row);
            if first.is_none() {
                first = Some(row);
            }
        }
        if let Some(row) = first {
            list.select_row(Some(&row));
        }
    }

    fn on_cmd_bar_row_activated(&self, row: &gtk::ListBoxRow) {
        let cmd_label = self
            .imp()
            .cmd_row_map
            .borrow()
            .get(&(row.as_ptr() as usize))
            .cloned()
            .unwrap_or_default();
        let entry = self.imp().cmd_entry.borrow().clone().unwrap();
        entry.set_text(&format!("{} ", cmd_label));
        entry.set_position(-1);
        entry.grab_focus();
    }

    fn execute_from_bar(&self) {
        let entry = self.imp().cmd_entry.borrow().clone().unwrap();
        let shadow = entry.text().trim().to_string();
        if shadow.is_empty() {
            self.hide_command_bar();
            return;
        }
        self.hide_command_bar();
        *self.imp().input_shadow.borrow_mut() = shadow.clone();
        self.execute_oxterm_command(&shadow);
        *self.imp().input_shadow.borrow_mut() = String::new();
    }

    fn on_cmd_bar_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        if key == K::Escape {
            self.hide_command_bar();
            return glib::Propagation::Stop;
        }
        if key == K::Return || key == K::KP_Enter {
            return glib::Propagation::Proceed;
        }
        let list = self.imp().cmd_list.borrow().clone().unwrap();
        if key == K::Down || key == K::Up {
            let children: Vec<gtk::ListBoxRow> = list
                .children()
                .into_iter()
                .filter_map(|c| c.downcast::<gtk::ListBoxRow>().ok())
                .collect();
            if !children.is_empty() {
                let sel = list.selected_row();
                let idx = sel.map(|r| r.index()).unwrap_or(if key == K::Down {
                    -1
                } else {
                    children.len() as i32
                });
                let n = children.len() as i32;
                let nxt = if key == K::Down {
                    (idx + 1).rem_euclid(n)
                } else {
                    (idx - 1).rem_euclid(n)
                };
                if let Some(row) = children.get(nxt as usize) {
                    list.select_row(Some(row));
                }
            }
            return glib::Propagation::Stop;
        }
        if key == K::Tab {
            let children: Vec<gtk::ListBoxRow> = list
                .children()
                .into_iter()
                .filter_map(|c| c.downcast::<gtk::ListBoxRow>().ok())
                .collect();
            if !children.is_empty() {
                let sel = list.selected_row().unwrap_or_else(|| children[0].clone());
                if let Some(cmd) = self
                    .imp()
                    .cmd_row_map
                    .borrow()
                    .get(&(sel.as_ptr() as usize))
                {
                    let entry = self.imp().cmd_entry.borrow().clone().unwrap();
                    entry.set_text(&format!("{} ", cmd));
                    entry.set_position(-1);
                }
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn execute_oxterm_command(&self, shadow: &str) {
        if !self.is_oxterm_command(shadow) {
            self.vte()
                .feed(b"\r\n\x1b[33mUnknown Oxterm command. Use /help.\x1b[0m\r\n");
            return;
        }
        if settings().get_bool("history_enabled") {
            history().add_async(shadow, &self.get_cwd(), -1);
        }
        self.feed_command_bytes(b"\x15");
        self.dispatch_oxterm_command(shadow);
    }

    /// Run an Oxterm `/command` (shared by the prompt and the command bar).
    /// Returns true when the shell prompt should be redrawn afterwards, i.e.
    /// the command did not hand the terminal to the history picker.
    fn dispatch_oxterm_command(&self, shadow: &str) -> bool {
        let args = || {
            shadow
                .split_once(char::is_whitespace)
                .map_or("", |(_, a)| a)
                .to_string()
        };
        if shadow.starts_with("/history") {
            let args = args();
            if args.trim().eq_ignore_ascii_case("clear") {
                history().clear_async();
                self.vte().feed(b"\r\n\x1b[32mHistory cleared.\x1b[0m\r\n");
                true
            } else {
                self.cmd_history(&args, true);
                false
            }
        } else if shadow.starts_with("/wnotes") {
            self.cmd_wnotes(&args());
            true
        } else if shadow.starts_with("/onotes") {
            self.cmd_onotes(&args());
            true
        } else if shadow.starts_with("/learn") {
            self.cmd_learn(&args());
            true
        } else if shadow.starts_with("/optimize") {
            self.cmd_optimize(&args());
            true
        } else if shadow.starts_with("/session") {
            self.cmd_session(&args());
            true
        } else if shadow.starts_with("/snippet") {
            self.cmd_snippet(&args());
            true
        } else if shadow == "/help" {
            self.cmd_help();
            true
        } else if shadow == "/clear" || shadow == "/cls" {
            self.vte().feed(b"\x1b[H\x1b[2J");
            true
        } else {
            true
        }
    }

    fn autocomplete_oxterm(&self) {
        let shadow = self.imp().input_shadow.borrow().clone();
        if !shadow.starts_with('/') {
            return;
        }
        let rest = &shadow[1..];
        let matches: Vec<&str> = OXTERM_COMMANDS
            .iter()
            .filter(|c| c.starts_with(rest))
            .copied()
            .collect();
        if matches.len() == 1 {
            self.feed_command_bytes(b"\x15");
            let completed = format!("/{} ", matches[0]);
            *self.imp().input_shadow.borrow_mut() = completed.clone();
            self.vte().feed_child(completed.as_bytes());
        } else if matches.len() > 1 {
            let common = common_prefix(&matches);
            if common.len() > rest.len() {
                self.feed_command_bytes(b"\x15");
                let completed = format!("/{}", common);
                *self.imp().input_shadow.borrow_mut() = completed.clone();
                self.vte().feed_child(completed.as_bytes());
            } else {
                self.feed_command_bytes(b"\x15");
                let list = matches
                    .iter()
                    .map(|m| format!("/{}", m))
                    .collect::<Vec<_>>()
                    .join("  ");
                self.vte()
                    .feed(format!("\r\n\x1b[90m{}\x1b[0m\r\n", list).as_bytes());
                let current = self.imp().input_shadow.borrow().clone();
                self.vte().feed_child(current.as_bytes());
            }
        }
    }
}

/// Process names that represent an interactive session on a remote host.
pub(crate) fn is_remote_client(comm: &str) -> bool {
    matches!(comm, "ssh" | "autossh" | "mosh-client")
}

fn proc_children(pid: impl std::fmt::Display) -> Vec<String> {
    std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", pid))
        .map(|c| c.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn proc_comm(pid: &str) -> String {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|c| c.trim().to_string())
        .unwrap_or_default()
}

fn proc_cmdline(pid: &str) -> Vec<String> {
    let raw = std::fs::read(format!("/proc/{}/cmdline", pid)).unwrap_or_default();
    let mut args: Vec<String> = raw
        .split(|&b| b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    if args.last().is_some_and(String::is_empty) {
        args.pop();
    }
    args
}

/// See `TerminalBox::get_ssh_target`.
fn remote_session_args(shell_pid: i32) -> Option<Vec<String>> {
    for child in proc_children(shell_pid) {
        match proc_comm(&child).as_str() {
            "ssh" => {
                let args = proc_cmdline(&child);
                if args.len() > 1 {
                    return Some(args[1..].to_vec());
                }
            }
            "autossh" => {
                // Prefer the live ssh child; while autossh is restarting it,
                // fall back to autossh's own arguments minus its options.
                if let Some(ssh) = proc_children(&child)
                    .into_iter()
                    .find(|c| proc_comm(c) == "ssh")
                {
                    let args = proc_cmdline(&ssh);
                    if args.len() > 1 {
                        return Some(args[1..].to_vec());
                    }
                }
                let args = autossh_ssh_args(&proc_cmdline(&child));
                if !args.is_empty() {
                    return Some(args);
                }
            }
            "mosh-client" => {
                if let Some(args) = mosh_ssh_args(&proc_cmdline(&child)) {
                    return Some(args);
                }
            }
            _ => {}
        }
    }
    None
}

/// Strip autossh's own leading options (-M port, -f, -V).
fn autossh_ssh_args(argv: &[String]) -> Vec<String> {
    let mut rest = argv.iter().skip(1).peekable();
    while let Some(arg) = rest.peek() {
        if arg.as_str() == "-M" {
            rest.next();
            rest.next();
        } else if arg.starts_with("-M") || arg.as_str() == "-f" || arg.as_str() == "-V" {
            rest.next();
        } else {
            break;
        }
    }
    rest.cloned().collect()
}

/// mosh execs `mosh-client "-# <original mosh arguments joined by spaces> |"
/// IP PORT`. Rebuild, best effort, the ssh arguments that reach that host:
/// the `--ssh=` command plus the ssh options that follow it, then the host.
fn mosh_ssh_args(argv: &[String]) -> Option<Vec<String>> {
    let joined = argv.iter().find_map(|a| a.strip_prefix("-# "))?;
    let joined = joined.strip_suffix(" |").unwrap_or(joined);
    // ssh options that take a separate value.
    const SSH_VALUE_OPTS: &[&str] = &["-p", "-l", "-i", "-J", "-o", "-F", "-b", "-c", "-m"];
    let mut ssh_args: Vec<String> = Vec::new();
    let mut in_ssh = false;
    let mut tokens = joined.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.starts_with("--ssh=") {
            // The value's first word is the program ("ssh"); its options
            // follow as separate tokens.
            in_ssh = true;
        } else if token == "--" {
            return None;
        } else if token.starts_with('-') {
            let takes_value = SSH_VALUE_OPTS.contains(&token);
            if in_ssh {
                ssh_args.push(token.to_string());
                if takes_value {
                    ssh_args.push(tokens.next()?.to_string());
                }
            } else if token == "-p" || token == "--port" {
                tokens.next(); // mosh's own UDP port
            }
        } else {
            ssh_args.push(token.to_string());
            return Some(ssh_args);
        }
    }
    None
}

fn common_prefix(strings: &[&str]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = strings[0];
    let mut prefix = first.to_string();
    for s in &strings[1..] {
        while !s.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn mono_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod command_block_tests {
    use super::*;

    fn block(command: &str, finished: bool, exit: i64, duration: i64) -> CommandBlock {
        CommandBlock {
            command: command.to_string(),
            finished,
            exit_code: Some(exit),
            duration_ms: Some(duration),
            ..CommandBlock::default()
        }
    }

    #[test]
    fn sticky_header_summarises_success_and_failure() {
        assert_eq!(
            TerminalBox::sticky_header_text(&block("make -j8", true, 0, 1234)),
            "✓ make -j8  —  1.2s"
        );
        assert_eq!(
            TerminalBox::sticky_header_text(&block("make -j8", true, 2, 65_000)),
            "✗ make -j8  —  exit 2 · 1m 5s"
        );
        assert_eq!(
            TerminalBox::sticky_header_text(&block("", true, 0, 0)),
            "✓ (shell)  —  0ms"
        );
    }

    #[test]
    fn sticky_header_flattens_control_characters() {
        assert_eq!(
            TerminalBox::sticky_header_text(&block("echo\u{1b}[31mred\nnow", true, 0, 10)),
            "✓ echo [31mred now  —  10ms"
        );
    }

    #[test]
    fn sticky_header_marks_a_running_command() {
        let mut running = block("cargo build", false, 0, 0);
        running.started_at = Some(Instant::now());
        let text = TerminalBox::sticky_header_text(&running);
        assert!(text.starts_with("● cargo build  —  running"), "{text}");
    }
}
