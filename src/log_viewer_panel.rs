//! Remote log viewer tab of the shared right-hand dock.
//!
//! File discovery and tailing happen over `ssh` with arguments (never a local
//! shell), with every user-supplied path single-quoted by `shell_quote`. The
//! regex filter is applied on the main thread with the `regex` crate, and
//! lines are coloured by level in a monospace `gtk::TextView`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gtk::prelude::*;

use crate::headless::commands::{output_cancellable, CommandError};

/// Guard against a huge directory turning into minutes of sequential ssh
/// calls and an unbounded GTK buffer rebuild.
const MAX_LOG_FILES: usize = 40;
const MAX_RENDER_LINES: usize = 2000;

/// Common log destinations offered by the Logs dropdown. The combo has an
/// entry, so any other path or `journal:<unit>` can still be typed.
pub const LOG_PATHS: [&str; 9] = [
    "/var/log",
    "/var/log/syslog",
    "/var/log/messages",
    "/var/log/auth.log",
    "/var/log/secure",
    "/var/log/journal",
    "journal",
    "journal:ssh",
    "journal:systemd-journald",
];

/// A POSIX-shell-safe single-quoted argument. An embedded `'` is written as
/// the conventional `'\\''` sequence.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// What the path entry means: a file tree to scan, or the systemd journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogSource {
    Files(String),
    Journal(Option<String>),
}

/// Text of an editable combo (a preset or whatever the user typed).
fn combo_text(combo: &gtk::ComboBoxText) -> String {
    combo
        .active_text()
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// `journal` / `journal:<unit>` select the journal; everything else is a path.
pub fn parse_source(path: &str) -> LogSource {
    let value = path.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("journal") {
        return LogSource::Journal(None);
    }
    if let Some(unit) = value.strip_prefix("journal:") {
        let unit = unit.trim();
        return LogSource::Journal((!unit.is_empty()).then(|| unit.to_string()));
    }
    LogSource::Files(value.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Other,
}

impl LogLevel {
    /// Name of the `gtk::TextTag` created in `build()`.
    pub fn tag(self) -> &'static str {
        match self {
            LogLevel::Error => "log-error",
            LogLevel::Warn => "log-warn",
            LogLevel::Info => "log-info",
            LogLevel::Other => "log-other",
        }
    }
}

/// Classify a log line by the first level word it contains.
pub fn parse_log_level(line: &str) -> LogLevel {
    let upper = line.to_ascii_uppercase();
    if upper.contains("ERROR") || upper.contains("FATAL") || upper.contains("CRITICAL") {
        LogLevel::Error
    } else if upper.contains("WARN") {
        LogLevel::Warn
    } else if upper.contains("INFO") {
        LogLevel::Info
    } else {
        LogLevel::Other
    }
}

/// Normalise remote output into lines: drop CR and blank lines.
pub fn parse_remote_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn log_command(host: &str, command: &str) -> Result<std::process::Command, String> {
    if host.trim().is_empty() {
        // The entry says an empty target means localhost: run the same script
        // here rather than needing sshd on this machine. `command` is built by
        // `remote_script` from quoted, fixed bodies.
        let mut process = std::process::Command::new("sh");
        process.arg("-c").arg(command);
        return Ok(process);
    }
    // A typed `user@host:port` must become -l/-p arguments: ssh reads the port
    // in the destination as part of the hostname.
    let args = crate::headless::connections::ssh_target_args(host)
        .map_err(|error| format!("{error} (es. user@server:2222)"))?;
    let mut process = std::process::Command::new("ssh");
    process
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .args(args)
        .arg(command);
    Ok(process)
}

fn ssh_run(host: &str, command: &str, cancel: &AtomicBool) -> Result<String, String> {
    let mut process = log_command(host, command)?;
    let output = match output_cancellable(&mut process, cancel) {
        Ok(output) => output,
        Err(CommandError::Cancelled) => return Err("interrotto".to_string()),
        Err(CommandError::Failed(message)) => return Err(message),
    };
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("comando terminato con {status}", status = output.status)
        } else {
            detail
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Only hand a pattern to `grep -E` when its syntax is also POSIX ERE. Rust
/// extras such as `\\d` or `(?i)` stay local, where the `regex` crate handles
/// them. The local regex is applied to every result in either case.
fn remote_grep_pattern(pattern: Option<&regex::Regex>) -> Option<String> {
    let text = pattern.map(|regex| regex.as_str())?;
    if text.contains('\\') || text.contains("(?") || text.contains('[') {
        return None;
    }
    Some(text.to_string())
}

fn grep_suffix(pattern: Option<&str>) -> String {
    pattern
        .map(|pattern| format!(" | grep -E -n {}", shell_quote(pattern)))
        .unwrap_or_default()
}

/// Wrap a script body in `timeout 5 sh -c <quoted body>`. Quoting the whole
/// body (not just its arguments) keeps inner quotes from breaking the outer
/// one, even when the user path or regex contains `'`.
fn remote_script(body: &str) -> String {
    format!("timeout 5 sh -c {}", shell_quote(body))
}

/// Run the remote commands for a source and concatenate their output. Each
/// file is preceded by a `==> path <==` header, like `tail -f` does.
fn collect_remote(
    host: &str,
    source: &LogSource,
    grep: Option<&str>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    match source {
        LogSource::Journal(unit) => {
            let command = match unit {
                Some(unit) => remote_script(&format!(
                    "journalctl --no-pager -n 200 -u {} 2>&1{}",
                    shell_quote(unit),
                    grep_suffix(grep)
                )),
                None => remote_script(&format!(
                    "journalctl --no-pager -n 200 2>&1{}",
                    grep_suffix(grep)
                )),
            };
            ssh_run(host, &command, cancel)
        }
        LogSource::Files(path) => {
            let find = remote_script(&format!(
                "find {} -maxdepth 2 -type f 2>/dev/null | head -{}",
                shell_quote(path),
                MAX_LOG_FILES
            ));
            let listing = ssh_run(host, &find, cancel)?;
            let files: Vec<&str> = listing
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with('/'))
                .collect();
            if files.is_empty() {
                return Ok(format!("Nessun file trovato in {path}"));
            }
            let mut output = String::new();
            for file in files {
                let command = remote_script(&format!(
                    "tail -n 200 {} 2>/dev/null{}",
                    shell_quote(file),
                    grep_suffix(grep)
                ));
                output.push_str(&format!("==> {file} <==\n"));
                match ssh_run(host, &command, cancel) {
                    Ok(text) => output.push_str(&text),
                    Err(error) => output.push_str(&format!("[errore: {error}]\n")),
                }
            }
            Ok(output)
        }
    }
}

struct LogState {
    stop: Arc<AtomicBool>,
    generation: u64,
    paused: bool,
}

#[derive(Clone)]
struct Ui {
    status: crate::SendWeak<gtk::Label>,
    view: crate::SendWeak<gtk::TextView>,
}

impl Ui {
    fn new(status: &gtk::Label, view: &gtk::TextView) -> Ui {
        Ui {
            status: crate::SendWeak::new(status),
            view: crate::SendWeak::new(view),
        }
    }

    fn upgrade(&self) -> Option<(gtk::Label, gtk::TextView)> {
        Some((self.status.upgrade()?, self.view.upgrade()?))
    }
}

fn spawn_load(
    state: &Arc<Mutex<LogState>>,
    ui: &Ui,
    host: String,
    source: LogSource,
    pattern: Option<regex::Regex>,
) {
    let (generation, stop) = {
        let mut state = state.lock().expect("log viewer state lock poisoned");
        state.generation = state.generation.wrapping_add(1);
        state.stop.store(false, Ordering::SeqCst);
        (state.generation, state.stop.clone())
    };
    let shared = state.clone();
    let worker_ui = ui.clone();
    std::thread::spawn(move || {
        if stop.load(Ordering::SeqCst)
            || shared
                .lock()
                .expect("log viewer state lock poisoned")
                .generation
                != generation
        {
            return;
        }
        let grep = remote_grep_pattern(pattern.as_ref());
        let result = collect_remote(&host, &source, grep.as_deref(), &stop);
        let state = shared.clone();
        let ui = worker_ui.clone();
        crate::run_on_main(move || {
            let state = state.lock().expect("log viewer state lock poisoned");
            if state.generation != generation {
                return; // a newer Refresh/Pause superseded this result
            }
            let Some((status, view)) = ui.upgrade() else {
                return;
            };
            if state.paused {
                status.set_text("In pausa — risultati ignorati");
                return;
            }
            match result {
                Ok(output) => {
                    let lines = parse_remote_output(&output);
                    let shown = render(&view, &lines, pattern.as_ref());
                    status.set_text(&format!(
                        "{} righe mostrate su {} · {}",
                        shown,
                        lines.len(),
                        match &source {
                            LogSource::Files(path) => path.clone(),
                            LogSource::Journal(None) => "journal".to_string(),
                            LogSource::Journal(Some(unit)) => format!("journal:{unit}"),
                        }
                    ));
                }
                Err(message) => status.set_text(&format!("Errore: {message}")),
            }
        });
    });
}

/// Build the whole text first and insert it once. Applying a tag per line
/// through its own `TextIter` used to keep the GTK main loop busy for seconds
/// with large logs, which looked like a frozen monitor panel.
fn render(view: &gtk::TextView, lines: &[String], pattern: Option<&regex::Regex>) -> usize {
    let buffer = view.buffer().expect("text view always has a buffer");
    buffer.set_text("");
    let mut text = String::new();
    let mut spans: Vec<(i32, i32, &'static str)> = Vec::new();
    let mut offset = 0usize;
    let mut shown = 0usize;

    for line in lines.iter().take(MAX_RENDER_LINES) {
        let tag = if line.starts_with("==> ") {
            "log-header"
        } else if pattern.is_some_and(|regex| !regex.is_match(line)) {
            continue;
        } else if line.starts_with('[') {
            "log-header"
        } else {
            parse_log_level(line).tag()
        };
        let start = offset as i32;
        text.push_str(line);
        text.push('\n');
        offset += line.chars().count() + 1;
        spans.push((start, offset as i32, tag));
        shown += 1;
    }
    if lines.len() > MAX_RENDER_LINES {
        text.push_str(&format!(
            "... {} righe non mostrate\n",
            lines.len() - MAX_RENDER_LINES
        ));
    }
    if !text.is_empty() {
        let mut cursor = buffer.end_iter();
        buffer.insert(&mut cursor, &text);
        for (start, end, tag) in spans {
            let from = buffer.iter_at_offset(start);
            let to = buffer.iter_at_offset(end);
            buffer.apply_tag_by_name(tag, &from, &to);
        }
    }
    let mut end = buffer.end_iter();
    view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
    shown
}

/// Public state of the Logs tab. `build()` wires all signals; callers only
/// keep the value alive and connect `close` if they want to hide the dock.
pub struct LogViewerPanel {
    pub root: gtk::Box,
    /// Opt-in gate: nothing is read until Logs is enabled.
    pub enabled: gtk::CheckButton,
    pub host: gtk::Entry,
    /// Editable combo: pick a canonical path or type a custom one.
    pub path: gtk::ComboBoxText,
    pub filter: gtk::Entry,
    pub refresh: gtk::Button,
    pub pause: gtk::Button,
    pub close: gtk::Button,
    pub view: gtk::TextView,
    pub status: gtk::Label,
    state: Arc<Mutex<LogState>>,
}

impl LogViewerPanel {
    pub fn build() -> LogViewerPanel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(220);
        root.set_no_show_all(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(6);
        header.set_margin_bottom(4);
        header.set_margin_start(8);
        header.set_margin_end(8);
        let title = gtk::Label::new(None);
        title.set_markup("<b>Logs</b>");
        title.set_halign(gtk::Align::Start);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.pack_start(&title, true, true, 0);
        let close = gtk::Button::with_label("Close");
        close.set_tooltip_text(Some("Hide the shared dock"));
        header.pack_end(&close, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let enabled = gtk::CheckButton::with_label("Logs");
        enabled.set_active(false);
        enabled.set_margin_start(8);
        enabled.set_margin_top(2);
        enabled.set_tooltip_text(Some(
            "Abilita la lettura dei log del target impostato in Monitor",
        ));
        root.pack_start(&enabled, false, false, 0);

        let host = gtk::Entry::new();
        host.set_margin_start(8);
        host.set_margin_end(8);
        host.set_placeholder_text(Some("user@host[:port] (vuoto = localhost)"));
        let path = gtk::ComboBoxText::with_entry();
        path.set_margin_start(8);
        path.set_margin_end(8);
        for preset in LOG_PATHS {
            path.append_text(preset);
        }
        path.set_active(Some(0));
        path.set_tooltip_text(Some(
            "Percorso remoto o journal / journal:<unit>; scrivi pure un percorso personalizzato",
        ));
        let filter = gtk::Entry::new();
        filter.set_margin_start(8);
        filter.set_margin_end(8);
        filter.set_placeholder_text(Some("Filtro regex (vuoto = tutto)"));
        root.pack_start(&host, false, false, 0);
        root.pack_start(&path, false, false, 0);
        root.pack_start(&filter, false, false, 0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_margin_start(8);
        actions.set_margin_end(8);
        actions.set_margin_top(4);
        let refresh = gtk::Button::with_label("Refresh");
        let pause = gtk::Button::with_label("Pause");
        actions.pack_start(&refresh, false, false, 0);
        actions.pack_start(&pause, false, false, 0);
        root.pack_start(&actions, false, false, 0);

        let table = gtk::TextTagTable::new();
        for (name, color) in [
            ("log-error", "#ff6b6b"),
            ("log-warn", "#f0c674"),
            ("log-info", "#8abeb7"),
            ("log-other", "#c0c0c0"),
            ("log-header", "#7f9fbf"),
        ] {
            let tag = gtk::TextTag::new(Some(name));
            tag.set_property("foreground", color);
            table.add(&tag);
        }
        let buffer = gtk::TextBuffer::new(Some(&table));
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_wrap_mode(gtk::WrapMode::Word);
        let scroller = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_overlay_scrolling(true);
        scroller.set_min_content_height(180);
        scroller.set_margin_top(6);
        scroller.set_margin_start(8);
        scroller.set_margin_end(8);
        scroller.add(&view);
        root.pack_start(&scroller, true, true, 0);

        let status = gtk::Label::new(Some("Premi Refresh per leggere i log remoti"));
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_top(4);
        status.set_margin_bottom(6);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        let state = Arc::new(Mutex::new(LogState {
            stop: Arc::new(AtomicBool::new(false)),
            generation: 0,
            paused: false,
        }));
        let ui = Ui::new(&status, &view);

        let refresh_state = state.clone();
        let refresh_ui = ui.clone();
        let refresh_host = host.clone();
        let refresh_path = path.clone();
        let refresh_filter = filter.clone();
        let refresh_status = status.clone();
        let refresh_enabled = enabled.clone();
        refresh.connect_clicked(move |_| {
            if !refresh_enabled.is_active() {
                return;
            }
            let pattern_text = refresh_filter.text().to_string();
            let pattern = if pattern_text.trim().is_empty() {
                None
            } else {
                match regex::Regex::new(&pattern_text) {
                    Ok(regex) => Some(regex),
                    Err(error) => {
                        refresh_status.set_text(&format!("Regex non valida: {error}"));
                        return;
                    }
                }
            };
            refresh_status.set_text("Lettura in corso...");
            spawn_load(
                &refresh_state,
                &refresh_ui,
                refresh_host.text().to_string(),
                parse_source(&combo_text(&refresh_path)),
                pattern,
            );
        });

        let activate_state = state.clone();
        let activate_ui = ui.clone();
        let activate_host = host.clone();
        let activate_path = path.clone();
        let activate_filter = filter.clone();
        let activate_status = status.clone();
        let activate_enabled = enabled.clone();
        filter.connect_activate(move |_| {
            if !activate_enabled.is_active() {
                return;
            }
            let pattern_text = activate_filter.text().to_string();
            let pattern = if pattern_text.trim().is_empty() {
                None
            } else {
                match regex::Regex::new(&pattern_text) {
                    Ok(regex) => Some(regex),
                    Err(error) => {
                        activate_status.set_text(&format!("Regex non valida: {error}"));
                        return;
                    }
                }
            };
            activate_status.set_text("Lettura in corso...");
            spawn_load(
                &activate_state,
                &activate_ui,
                activate_host.text().to_string(),
                parse_source(&combo_text(&activate_path)),
                pattern,
            );
        });

        let pause_state = state.clone();
        let pause_status = status.clone();
        pause.connect_clicked(move |button| {
            let mut state = pause_state.lock().expect("log viewer state lock poisoned");
            state.paused = !state.paused;
            state.generation = state.generation.wrapping_add(1);
            let paused = state.paused;
            if paused {
                // Pausing must interrupt a read in flight, not just hide it.
                state.stop.store(true, Ordering::SeqCst);
            }
            button.set_label(if paused { "Resume" } else { "Pause" });
            pause_status.set_text(if paused {
                "In pausa — la vista non viene aggiornata"
            } else {
                "Ripreso — premi Refresh per rileggere"
            });
        });

        let toggle_state = state.clone();
        let toggle_status = status.clone();
        let toggle_view = view.clone();
        let toggle_refresh = refresh.clone();
        let toggle_pause = pause.clone();
        let toggle_refresh_action = refresh.downgrade();
        enabled.connect_toggled(move |button| {
            if button.is_active() {
                toggle_refresh.set_sensitive(true);
                toggle_pause.set_sensitive(true);
                toggle_status.set_text("Logs attivi — premi Refresh per rileggere");
                if let Some(refresh) = toggle_refresh_action.upgrade() {
                    refresh.activate();
                }
            } else {
                // Cancel any in-flight read and drop what was on screen: Logs
                // is opt-in, so an unchecked box must not show or fetch data.
                let mut state = toggle_state.lock().expect("log viewer state lock poisoned");
                state.stop.store(true, Ordering::SeqCst);
                state.generation = state.generation.wrapping_add(1);
                state.paused = false;
                drop(state);
                if let Some(buffer) = toggle_view.buffer() {
                    buffer.set_text("");
                }
                toggle_refresh.set_sensitive(false);
                toggle_pause.set_sensitive(false);
                toggle_pause.set_label("Pause");
                toggle_status.set_text("Logs disabilitati — spunta Logs per leggere");
            }
        });
        refresh.set_sensitive(false);
        pause.set_sensitive(false);
        status.set_text("Spunta Logs per leggere i log del target in Monitor");

        LogViewerPanel {
            root,
            enabled,
            host,
            path,
            filter,
            refresh,
            pause,
            close,
            view,
            status,
            state,
        }
    }

    /// Prefill the target from the active SSH connection, if any.
    pub fn set_target(&self, target: Option<String>) {
        if let Some(target) = target.filter(|target| !target.is_empty()) {
            self.host.set_text(&target);
        }
    }

    /// Release the worker when the dock is hidden.
    pub fn pause(&self) {
        let mut state = self.state.lock().expect("log viewer state lock poisoned");
        state.stop.store(true, Ordering::SeqCst);
        state.generation = state.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(command: &std::process::Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn log_command_splits_the_port_and_runs_locally_when_empty() {
        let script = remote_script("journalctl --no-pager -n 200");
        assert_eq!(
            argv(&log_command("andres@192.168.1.161:2223", &script).unwrap()),
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-l",
                "andres",
                "-p",
                "2223",
                "192.168.1.161",
                script.as_str()
            ]
        );
        assert_eq!(
            argv(&log_command("", &script).unwrap()),
            ["-c", script.as_str()]
        );
        assert!(log_command("-oProxyCommand=evil", &script).is_err());
    }

    #[test]
    fn quoting_protects_spaces_and_quotes() {
        assert_eq!(shell_quote("/var/log"), "'/var/log'");
        assert_eq!(shell_quote("a b'c"), "'a b'\\''c'");
    }

    #[test]
    fn remote_script_escapes_nested_quotes() {
        assert_eq!(
            grep_suffix(Some("ERROR|WARN")),
            " | grep -E -n 'ERROR|WARN'"
        );
        let body = format!("tail -n 200 {}", shell_quote("/var/log/a'b.log"));
        let script = remote_script(&body);
        assert!(script.starts_with("timeout 5 sh -c '"));
        assert!(script.contains("'\\''"), "inner quotes must be escaped");
        assert_eq!(remote_grep_pattern(None), None);
        let regex = regex::Regex::new(r"\d+").unwrap();
        assert_eq!(remote_grep_pattern(Some(&regex)), None);
        let regex = regex::Regex::new("ERROR|WARN").unwrap();
        assert_eq!(remote_grep_pattern(Some(&regex)), Some("ERROR|WARN".into()));
    }

    #[test]
    fn canonical_log_paths_cover_files_and_journal() {
        assert!(LOG_PATHS.contains(&"/var/log"));
        assert!(LOG_PATHS.contains(&"journal"));
        assert!(LOG_PATHS.iter().all(|path| !path.trim().is_empty()));
        assert_eq!(parse_source("journal"), LogSource::Journal(None));
        assert_eq!(
            parse_source("/var/log/syslog"),
            LogSource::Files("/var/log/syslog".into())
        );
    }

    #[test]
    fn source_selects_files_or_journal() {
        assert_eq!(
            parse_source("/var/log"),
            LogSource::Files("/var/log".into())
        );
        assert_eq!(parse_source("journal"), LogSource::Journal(None));
        assert_eq!(
            parse_source(" journal:ssh "),
            LogSource::Journal(Some("ssh".into()))
        );
    }

    #[test]
    fn levels_are_detected_case_insensitively() {
        assert_eq!(
            parse_log_level("2026-01-01 ERROR disk full"),
            LogLevel::Error
        );
        assert_eq!(parse_log_level("warn: retry"), LogLevel::Warn);
        assert_eq!(parse_log_level("INFO started"), LogLevel::Info);
        assert_eq!(parse_log_level("noise"), LogLevel::Other);
        assert_eq!(LogLevel::Error.tag(), "log-error");
    }

    #[test]
    fn remote_output_drops_empty_lines_and_carriage_returns() {
        assert_eq!(
            parse_remote_output("==> a <==\r\nfirst\r\n\r\nsecond\r\n"),
            vec!["==> a <==", "first", "second"]
        );
    }

    #[test]
    fn regex_filter_is_applied_to_plain_lines() {
        let lines = parse_remote_output("ERROR bad\nINFO ok\nWARN bad\n");
        let regex = regex::Regex::new("bad").unwrap();
        let matched: Vec<&String> = lines.iter().filter(|line| regex.is_match(line)).collect();
        assert_eq!(
            matched,
            vec![&"ERROR bad".to_string(), &"WARN bad".to_string()]
        );
    }
}
