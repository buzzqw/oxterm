//! The optional agent pane: an ACP client rendered as a docked conversation.
//!
//! The pane belongs to the window, not to a tab. Each tab gets its own ACP
//! session, keyed by pane id, so hiding the pane or switching tabs stashes the
//! conversation instead of destroying it. Nothing starts until the user submits
//! a prompt, and nothing starts at all unless `agent_enabled` is true and
//! `agent_command` names an ACP agent the user has installed.
//!
//! The protocol half lives in `headless::acp`; this file is the transport (two
//! small threads around the child's stdio) and the widgets.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::{json, Value};

use crate::headless::{acp, agent};
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::MainWindow;

/// The messages the reader thread sends to the GTK loop.
enum UiEvent {
    Ready {
        pane: String,
        agent: String,
        session: String,
    },
    Update {
        pane: String,
        update: acp::Update,
    },
    Permission {
        pane: String,
        id: Value,
        title: String,
        options: Vec<acp::PermissionOption>,
        writer: Sender<Value>,
    },
    TurnDone {
        pane: String,
        reason: String,
    },
    Error {
        pane: String,
        message: String,
    },
    Closed {
        pane: String,
    },
}

struct Session {
    child: Child,
    writer: Sender<Value>,
    session_id: Option<String>,
    agent_name: String,
    transcript: String,
    ready: bool,
    busy: bool,
    next_id: u64,
    queue: Vec<String>,
    history: Vec<String>,
    history_index: usize,
    usage: String,
    status: &'static str,
}

impl Session {
    fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.transcript.push_str(text);
        // A long conversation is still useful, but an unbounded buffer is not:
        // drop whole characters from the front once past the cap.
        let count = self.transcript.chars().count();
        if count > 300_000 {
            let skip = count - 250_000;
            self.transcript = self.transcript.chars().skip(skip).collect();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct AgentPanel {
    root: gtk::Revealer,
    header: gtk::Label,
    usage: gtk::Label,
    view: gtk::TextView,
    entry: gtk::Entry,
    sessions: RefCell<HashMap<String, Session>>,
    active: RefCell<Option<String>>,
    events: Receiver<UiEvent>,
    events_tx: Sender<UiEvent>,
    window: crate::SendWeak<MainWindow>,
}

impl AgentPanel {
    pub fn new(window: &MainWindow) -> Rc<AgentPanel> {
        let (tx, rx) = mpsc::channel();

        let root = gtk::Revealer::new();
        root.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        root.set_transition_duration(150);
        root.set_reveal_child(false);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.style_context().add_class("oxterm-agent-panel");
        root.add(&body);

        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header_row.style_context().add_class("oxterm-agent-header");
        let header = gtk::Label::new(Some("Agent"));
        header.set_xalign(0.0);
        header.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let usage = gtk::Label::new(Some(""));
        usage.set_xalign(1.0);
        usage.style_context().add_class("dim-label");
        let fix_button = gtk::Button::with_label("Fix");
        fix_button.set_relief(gtk::ReliefStyle::None);
        fix_button.set_tooltip_text(Some("Ask the agent to fix the last failed command"));
        let close_button = gtk::Button::with_label("×");
        close_button.set_relief(gtk::ReliefStyle::None);
        close_button.set_tooltip_text(Some("Hide the agent pane (the session is kept)"));
        header_row.pack_start(&header, true, true, 0);
        header_row.pack_start(&usage, false, false, 0);
        header_row.pack_start(&fix_button, false, false, 0);
        header_row.pack_start(&close_button, false, false, 0);
        body.pack_start(&header_row, false, false, 0);

        let scroller = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_height(140);
        scroller.set_vexpand(true);
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_left_margin(10);
        view.set_right_margin(10);
        view.set_top_margin(6);
        view.set_bottom_margin(6);
        view.style_context().add_class("oxterm-agent-transcript");
        scroller.add(&view);
        body.pack_start(&scroller, true, true, 0);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Ask the agent…  (/help, /fix, /stop)"));
        entry.set_has_frame(false);
        entry.set_margin_start(10);
        entry.set_margin_end(10);
        entry.set_margin_top(4);
        entry.set_margin_bottom(8);
        entry.style_context().add_class("oxterm-agent-entry");
        body.pack_start(&entry, false, false, 0);

        let panel = Rc::new(AgentPanel {
            root,
            header,
            usage,
            view,
            entry,
            sessions: RefCell::new(HashMap::new()),
            active: RefCell::new(None),
            events: rx,
            events_tx: tx,
            window: crate::SendWeak::new(window),
        });

        {
            let entry = panel.entry.clone();
            let panel = panel.clone();
            entry.connect_activate(move |entry| {
                let text = entry.text().to_string();
                entry.set_text("");
                panel.submit(&text);
            });
        }
        {
            let entry = panel.entry.clone();
            let panel = panel.clone();
            entry.connect_key_press_event(move |_entry, event| panel.on_entry_key(event));
        }
        {
            let panel = panel.clone();
            fix_button.connect_clicked(move |_| {
                panel.fix_active(None);
            });
        }
        {
            let panel = panel.clone();
            close_button.connect_clicked(move |_| {
                panel.hide();
            });
        }

        let poll = Rc::downgrade(&panel);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if let Some(panel) = poll.upgrade() {
                panel.drain_events();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        panel
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.root
    }

    pub fn is_visible(&self) -> bool {
        self.root.reveals_child()
    }

    pub fn set_visible(&self, visible: bool) {
        self.root.set_reveal_child(visible);
        if visible {
            self.render_active();
            self.entry.grab_focus();
        }
    }

    pub fn toggle(&self) {
        self.set_visible(!self.is_visible());
    }

    pub fn hide(&self) {
        self.set_visible(false);
    }

    /// Point the pane at a tab. The session keeps running while hidden.
    pub fn set_active_pane(&self, pane: Option<String>) {
        if *self.active.borrow() != pane {
            *self.active.borrow_mut() = pane;
            if self.is_visible() {
                self.render_active();
            } else {
                self.update_header();
            }
        }
    }

    /// Ask for the context of `term`'s last command and send it. `visible` is
    /// what the transcript shows; the prompt sent to the agent is `prompt`.
    pub fn ask_for(&self, term: &TerminalBox, prompt: &str, visible: Option<&str>) {
        self.set_active_pane(Some(term.remote_id()));
        let text = prompt.trim();
        if text.is_empty() {
            return;
        }
        let shown = visible.unwrap_or(text).to_string();
        let pane = term.remote_id();
        self.ensure_session(term, text);
        self.render_to(&pane, &format!("\n▶ {}\n", shown));
        self.record_prompt(&shown);
    }

    /// `/fix` and the Fix button: build the context from the active terminal.
    pub fn fix_active(&self, hint: Option<&str>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(term) = window.current_terminal() else {
            self.append_text("\nNo terminal is active.\n");
            return;
        };
        self.fix_for(&term, hint);
    }

    pub fn fix_for(&self, term: &TerminalBox, hint: Option<&str>) {
        if !agent::enabled(&settings().raw_data()) {
            self.append_text(&format!(
                "\nAgent features are disabled. Set \"agent_enabled\": true and \
                 \"agent_command\" in {}.\n",
                crate::settings::config_file().display()
            ));
            self.set_visible(true);
            return;
        }
        let context = term.last_command_context();
        if context.is_empty() {
            self.set_visible(true);
            self.append_text(
                "\nThere is no finished command to look at. The shell integration (OSC 133) \
                 must be active; see Settings > Shell integration.\n",
            );
            return;
        }
        let prompt = agent::fix_prompt(&context, hint);
        let mut shown = String::from("/fix");
        if let Some(hint) = hint.map(str::trim).filter(|hint| !hint.is_empty()) {
            shown.push(' ');
            shown.push_str(hint);
        }
        if !context.command.trim().is_empty() {
            shown.push_str(&format!(
                "  —  {}",
                crate::headless::text::truncate_chars(context.command.trim(), 80)
            ));
        }
        self.set_visible(true);
        self.ask_for(term, &prompt, Some(&shown));
    }

    /// Called when a prompt arrives from the control API or the `?` palette.
    pub fn prompt_for(&self, term: &TerminalBox, text: &str) {
        self.set_visible(true);
        self.ask_for(term, text, None);
    }

    /// Drop sessions of tabs that no longer exist.
    pub fn prune(&self, alive: &[TerminalBox]) {
        let ids: Vec<String> = alive.iter().map(|term| term.remote_id()).collect();
        let mut sessions = self.sessions.borrow_mut();
        sessions.retain(|pane, _| ids.iter().any(|id| id == pane));
        if self
            .active
            .borrow()
            .as_ref()
            .is_some_and(|pane| !ids.iter().any(|id| id == pane))
        {
            *self.active.borrow_mut() = None;
            self.render_active();
        }
    }

    // ── Prompt handling ──────────────────────────────────────

    fn submit(&self, raw: &str) {
        let text = raw.trim();
        if text.is_empty() {
            return;
        }
        if let Some(command) = text.strip_prefix('/') {
            self.run_command(command);
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(term) = window.current_terminal() else {
            self.append_text("\nNo terminal is active.\n");
            return;
        };
        self.prompt_for(&term, text);
    }

    fn run_command(&self, command: &str) {
        let (name, rest) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, rest)| (name, rest));
        match name {
            "help" => {
                self.append_text(
                    "\nCommands:\n\
                     \x20 /help          show this list\n\
                     \x20 /fix [hint]    diagnose the last failed command\n\
                     \x20 /stop          cancel the current turn\n\
                     \x20 /new           start a fresh agent session\n\
                     \x20 /clear         clear this transcript\n\
                     \x20 /agent         show the configured agent\n\
                     \x20 /copy          copy the transcript to the clipboard\n",
                );
            }
            "fix" => self.fix_active(Some(rest).filter(|rest| !rest.trim().is_empty())),
            "stop" => self.cancel_active(),
            "new" => self.restart_active(),
            "clear" => self.clear_active(),
            "copy" => self.copy_active(),
            "agent" => {
                let settings = settings().raw_data();
                let command = settings
                    .get("agent_command")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let policies = agent::Policies::from_settings(&settings);
                self.append_text(&format!(
                    "\nAgent: {}\nCommand: {}\nPolicy: read={} create={} input={}\n",
                    if agent::enabled(&settings) {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    if command.trim().is_empty() {
                        "(none)"
                    } else {
                        command
                    },
                    policies.read.as_str(),
                    policies.create.as_str(),
                    policies.input.as_str()
                ));
            }
            other => self.append_text(&format!("\nUnknown command: /{}\n", other)),
        }
    }

    fn active_pane(&self) -> Option<String> {
        self.active.borrow().clone()
    }

    fn with_active<F: FnOnce(&mut Session, &str)>(&self, f: F) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        let mut sessions = self.sessions.borrow_mut();
        if let Some(session) = sessions.get_mut(&pane) {
            f(session, &pane);
        }
    }

    fn cancel_active(&self) {
        let mut message = None;
        self.with_active(|session, _| {
            if let Some(id) = session.session_id.as_deref() {
                let _ = session.writer.send(acp::session_cancel(id));
                message = Some("cancelling…");
            }
        });
        if let Some(message) = message {
            self.append_text(&format!("\n{}\n", message));
        }
    }

    fn restart_active(&self) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        self.sessions.borrow_mut().remove(&pane);
        self.view
            .buffer()
            .expect("text view has a buffer")
            .set_text("");
        self.append_text("Session restarted; the next prompt starts a new one.\n");
        self.update_header();
    }

    fn clear_active(&self) {
        self.with_active(|session, _| session.transcript.clear());
        self.view
            .buffer()
            .expect("text view has a buffer")
            .set_text("");
    }

    fn copy_active(&self) {
        let text = self
            .active_pane()
            .and_then(|pane| {
                self.sessions
                    .borrow()
                    .get(&pane)
                    .map(|s| s.transcript.clone())
            })
            .unwrap_or_default();
        gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text(&text);
        self.append_text("\nTranscript copied to the clipboard.\n");
    }

    fn record_prompt(&self, text: &str) {
        if let Some(pane) = self.active_pane() {
            if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                session.history.push(text.to_string());
                session.history_index = session.history.len();
            }
        }
    }

    fn on_entry_key(&self, event: &gtk::gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        if key != gtk::gdk::keys::constants::Up && key != gtk::gdk::keys::constants::Down {
            return glib::Propagation::Proceed;
        }
        let Some(pane) = self.active_pane() else {
            return glib::Propagation::Proceed;
        };
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&pane) else {
            return glib::Propagation::Proceed;
        };
        if session.history.is_empty() {
            return glib::Propagation::Proceed;
        }
        if key == gtk::gdk::keys::constants::Up {
            session.history_index = session.history_index.saturating_sub(1);
        } else {
            session.history_index = (session.history_index + 1).min(session.history.len());
        }
        let text = session
            .history
            .get(session.history_index)
            .cloned()
            .unwrap_or_default();
        self.entry.set_text(&text);
        self.entry.set_position(-1);
        glib::Propagation::Stop
    }

    // ── Sessions ─────────────────────────────────────────────

    fn ensure_session(&self, term: &TerminalBox, first_prompt: &str) {
        let pane = term.remote_id();
        let state = self
            .sessions
            .borrow()
            .get(&pane)
            .map(|session| (session.ready, session.status));
        if let Some((ready, status)) = state {
            if ready {
                self.send_prompt(&pane, first_prompt);
                self.update_header();
                return;
            }
            if status != "error" && status != "closed" {
                if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                    session.queue.push(first_prompt.to_string());
                    session.status = "starting";
                }
                self.update_header();
                return;
            }
            // A process that exited or failed the handshake is replaced on the
            // next prompt instead of swallowing it in a dead queue.
            self.sessions.borrow_mut().remove(&pane);
        }

        let settings = settings().raw_data();
        if !agent::configured(&settings) {
            self.append_text(&format!(
                "\nAgent features are not configured. Set \"agent_enabled\": true and \
                 \"agent_command\" (for example \"opencode acp\") in {}.\n\
                 `oxterm agent --list` shows the agent CLIs already installed.\n",
                crate::settings::config_file().display()
            ));
            return;
        }

        let remote_args = term.remote_session_args();
        let remote_command = settings
            .get("agent_remote_command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let use_remote = remote_args.is_some() && !remote_command.is_empty();

        let argv = if use_remote {
            let mut args = vec!["ssh".to_string(), "-T".to_string()];
            // `-T` wins over an inherited `-t`, and the command goes last.
            args.extend(
                remote_args
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|arg| arg != "-t" && arg != "-T"),
            );
            match agent::command_argv(&remote_command) {
                Ok(command) => args.extend(command),
                Err(error) => {
                    self.append_text(&format!("\n{}\n", error));
                    return;
                }
            }
            args
        } else {
            match agent::command_argv(
                settings
                    .get("agent_command")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            ) {
                Ok(argv) => argv,
                Err(error) => {
                    self.append_text(&format!("\n{}\n", error));
                    return;
                }
            }
        };
        if !use_remote && argv.is_empty() {
            self.append_text("\nagent_command is empty.\n");
            return;
        }
        if argv.is_empty() {
            return;
        }

        let cwd = if use_remote {
            term.reported_cwd()
                .filter(|cwd| cwd.starts_with('/'))
                .unwrap_or_else(|| "/".to_string())
        } else {
            let cwd = term.get_cwd();
            if cwd.starts_with('/') {
                cwd
            } else {
                dirs::home_dir()
                    .map(|home| home.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".to_string())
            }
        };

        let servers = if !use_remote && agent::mcp_shared(&settings) {
            std::env::current_exe()
                .ok()
                .map(|exe| {
                    vec![acp::McpServer {
                        name: "oxterm".to_string(),
                        command: exe.to_string_lossy().to_string(),
                        args: vec!["mcp".to_string()],
                    }]
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        match spawn_session(pane.clone(), &argv, &cwd, servers, self.events_tx.clone()) {
            Ok(session) => {
                self.sessions.borrow_mut().insert(pane.clone(), session);
                self.render_to(
                    &pane,
                    &format!("\nStarting {} in {}…\n", argv.join(" "), cwd),
                );
                // The session is not ready until the handshake completes; queue
                // the first prompt so the user does not have to repeat it.
                if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                    session.queue.push(first_prompt.to_string());
                    session.status = "starting";
                }
                self.update_header();
            }
            Err(error) => {
                self.append_text(&format!("\nCannot start the agent: {}\n", error));
                self.update_header();
            }
        }
    }

    fn send_prompt(&self, pane: &str, text: &str) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(pane) else {
            return;
        };
        if !session.ready || session.busy {
            session.queue.push(text.to_string());
            return;
        }
        let Some(session_id) = session.session_id.clone() else {
            session.queue.push(text.to_string());
            return;
        };
        let id = session.next_id;
        session.next_id += 1;
        let value = acp::session_prompt(id, &session_id, text);
        if session.writer.send(value).is_err() {
            session.status = "error";
            session.busy = false;
            session.append("\n[error] the agent process is gone\n");
        } else {
            session.busy = true;
            session.status = "working";
        }
        let status = session.status;
        drop(sessions);
        self.publish_status(pane, status);
        self.render_active();
    }

    fn drain_events(&self) {
        loop {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            };
            match event {
                UiEvent::Ready {
                    pane,
                    agent,
                    session,
                } => {
                    let mut sessions = self.sessions.borrow_mut();
                    if let Some(state) = sessions.get_mut(&pane) {
                        state.ready = true;
                        state.status = "ready";
                        state.agent_name = agent;
                        state.session_id = Some(session);
                    }
                    let queued: Vec<String> = sessions
                        .get_mut(&pane)
                        .map(|state| state.queue.drain(..).collect())
                        .unwrap_or_default();
                    drop(sessions);
                    self.publish_status(&pane, "ready");
                    for text in queued {
                        self.send_prompt(&pane, &text);
                    }
                    self.update_header();
                }
                UiEvent::Update { pane, update } => {
                    self.render_update(&pane, update);
                }
                UiEvent::Permission {
                    pane,
                    id,
                    title,
                    options,
                    writer,
                } => {
                    self.ask_permission(&pane, &id, &title, &options, &writer);
                }
                UiEvent::TurnDone { pane, reason } => {
                    if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                        state.busy = false;
                        state.status = "ready";
                    }
                    self.publish_status(&pane, "ready");
                    // A prompt that arrived while the agent was busy runs now,
                    // one turn at a time.
                    let next = self.sessions.borrow_mut().get_mut(&pane).and_then(|state| {
                        if state.ready && !state.queue.is_empty() {
                            Some(state.queue.remove(0))
                        } else {
                            None
                        }
                    });
                    if let Some(next) = next {
                        self.send_prompt(&pane, &next);
                    }
                    if reason != "end_turn" && reason != "cancelled" {
                        self.render_to(&pane, &format!("\n[turn stopped: {}]\n", reason));
                    }
                    if self.active_pane().as_deref() == Some(pane.as_str()) {
                        self.update_header();
                    } else if let Some(state) = self.sessions.borrow().get(&pane) {
                        if !state.ready {
                            self.update_header();
                        }
                    }
                }
                UiEvent::Error { pane, message } => {
                    self.render_to(&pane, &format!("\n[error] {}\n", message));
                    if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                        state.status = "error";
                        state.busy = false;
                        state.ready = false;
                    }
                    self.publish_status(&pane, "error");
                    self.update_header();
                }
                UiEvent::Closed { pane } => {
                    let mut sessions = self.sessions.borrow_mut();
                    if let Some(state) = sessions.get_mut(&pane) {
                        let never_connected = state.session_id.is_none();
                        state.status = "closed";
                        state.ready = false;
                        state.busy = false;
                        state.session_id = None;
                        state.append("\n[the agent process exited]\n");
                        if never_connected {
                            state.append(
                                "If it exited immediately, check that the command speaks ACP. \
                                 `oxterm agent --list` shows the known agent CLIs.\n",
                            );
                        }
                    }
                    drop(sessions);
                    self.publish_status(&pane, "closed");
                    self.render_active();
                    self.update_header();
                }
            }
        }
    }

    fn render_update(&self, pane: &str, update: acp::Update) {
        match update {
            acp::Update::AgentMessage(text) => self.render_to(pane, &text),
            acp::Update::Thought(text) => {
                // Keep reasoning visibly secondary and compact.
                for line in text.lines() {
                    if !line.trim().is_empty() {
                        self.render_to(pane, &format!("· {}\n", line));
                    }
                }
            }
            acp::Update::UserMessage(_) => {}
            acp::Update::ToolCall {
                title,
                status,
                kind,
                ..
            } => {
                self.render_to(pane, &format!("\n[tool {}] {} ({})\n", kind, title, status));
            }
            acp::Update::ToolCallUpdate {
                title,
                status,
                detail,
                ..
            } => {
                let mut line = String::from("[tool]");
                if let Some(status) = status {
                    line.push(' ');
                    line.push_str(&status);
                }
                if let Some(title) = title {
                    line.push_str(": ");
                    line.push_str(&title);
                }
                line.push('\n');
                if let Some(detail) = detail {
                    line.push_str(&detail);
                    line.push('\n');
                }
                self.render_to(pane, &format!("\n{}", line));
            }
            acp::Update::Plan(entries) => {
                let mut text = String::from("\nPlan:\n");
                for entry in entries {
                    text.push_str(&format!("  · {}\n", entry));
                }
                self.render_to(pane, &text);
            }
            acp::Update::Usage { used, size, cost } => {
                let usage = match cost {
                    Some(cost) => format!("{}k/{}k · {}", used / 1000, size / 1000, cost),
                    None => format!("{}k/{}k", used / 1000, size / 1000),
                };
                if let Some(state) = self.sessions.borrow_mut().get_mut(pane) {
                    state.usage = usage;
                }
                if self.active_pane().as_deref() == Some(pane) {
                    self.update_header();
                }
            }
            acp::Update::Notice { severity, title } => {
                self.render_to(pane, &format!("\n[{}] {}\n", severity, title));
            }
            acp::Update::Other => {}
        }
    }

    fn ask_permission(
        &self,
        pane: &str,
        id: &Value,
        title: &str,
        options: &[acp::PermissionOption],
        writer: &Sender<Value>,
    ) {
        let allow = acp::default_allow_option(options);
        let reject = acp::default_reject_option(options);
        let names: Vec<&str> = options.iter().map(|option| option.name.as_str()).collect();
        let dialog = gtk::MessageDialog::new(
            self.root
                .toplevel()
                .and_then(|w| w.downcast::<gtk::Window>().ok())
                .as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::YesNo,
            &format!("Allow the agent to run this tool?\n\n{}", title),
        );
        dialog.set_title("Agent permission");
        if !names.is_empty() {
            dialog.set_secondary_text(Some(&format!("Options: {}", names.join(", "))));
        }
        dialog.set_default_response(gtk::ResponseType::No);
        let accepted = dialog.run() == gtk::ResponseType::Yes;
        unsafe { dialog.destroy() };
        let option = if accepted {
            allow.map(|option| option.id.as_str())
        } else {
            reject.map(|option| option.id.as_str())
        };
        let response = acp::permission_response(id, option);
        if writer.send(response).is_err() {
            self.render_to(pane, "\n[error] cannot answer the permission request\n");
        }
    }

    // ── Rendering ────────────────────────────────────────────

    /// Let scripts and triggers observe the session state. The event carries no
    /// conversation content, only the pane and the status.
    fn publish_status(&self, pane: &str, status: &str) {
        crate::headless::events::bus().publish(&json!({
            "event": "agent_status",
            "pane": pane,
            "status": status,
        }));
    }

    fn render_to(&self, pane: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(state) = self.sessions.borrow_mut().get_mut(pane) {
            state.append(text);
        }
        if self.active_pane().as_deref() == Some(pane) {
            self.append_text(text);
        }
    }

    fn append_text(&self, text: &str) {
        let buffer = self.view.buffer().expect("text view has a buffer");
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, text);
        self.scroll_to_end();
    }

    fn scroll_to_end(&self) {
        let buffer = self.view.buffer().expect("text view has a buffer");
        let mut end = buffer.end_iter();
        self.view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
    }

    fn render_active(&self) {
        let text = self
            .active_pane()
            .and_then(|pane| {
                self.sessions
                    .borrow()
                    .get(&pane)
                    .map(|session| session.transcript.clone())
            })
            .unwrap_or_default();
        self.view
            .buffer()
            .expect("text view has a buffer")
            .set_text(&text);
        self.scroll_to_end();
        self.update_header();
    }

    fn update_header(&self) {
        let (name, status, usage) = self
            .active_pane()
            .and_then(|pane| {
                self.sessions.borrow().get(&pane).map(|session| {
                    (
                        session.agent_name.clone(),
                        session.status,
                        session.usage.clone(),
                    )
                })
            })
            .unwrap_or_else(|| ("not started".to_string(), "", String::new()));
        let mut text = String::from("Agent");
        if !name.is_empty() {
            text.push_str(" · ");
            text.push_str(&name);
        }
        if !status.is_empty() {
            text.push_str(" · ");
            text.push_str(status);
        }
        if !settings().get_bool("agent_enabled") {
            text.push_str(" · disabled");
        }
        self.header.set_text(&text);
        self.usage.set_text(&usage);
        self.usage
            .set_visible(settings().get_bool("agent_show_usage") && !usage.is_empty());
    }
}

fn spawn_session(
    pane: String,
    argv: &[String],
    cwd: &str,
    servers: Vec<acp::McpServer>,
    events: Sender<UiEvent>,
) -> Result<Session, String> {
    let program = argv.first().ok_or("no agent command")?;
    let mut child = Command::new(program)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{}: {}", program, error))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;

    let (writer, write_rx) = mpsc::channel::<Value>();
    let writer_thread = writer.clone();
    thread::Builder::new()
        .name("oxterm-agent-writer".to_string())
        .spawn(move || {
            let mut stdin = stdin;
            for value in write_rx {
                if writeln!(stdin, "{}", value).is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        })
        .map_err(|error| format!("cannot start the agent writer: {}", error))?;

    let cwd = cwd.to_string();
    let pane_for_reader = pane.clone();
    thread::Builder::new()
        .name("oxterm-agent-reader".to_string())
        .spawn(move || read_agent(pane_for_reader, stdout, writer_thread, events, cwd, servers))
        .map_err(|error| format!("cannot start the agent reader: {}", error))?;

    Ok(Session {
        child,
        writer,
        session_id: None,
        agent_name: String::new(),
        transcript: String::new(),
        ready: false,
        busy: false,
        next_id: 3,
        queue: Vec::new(),
        history: Vec::new(),
        history_index: 0,
        usage: String::new(),
        status: "starting",
    })
}

fn read_agent(
    pane: String,
    stdout: std::process::ChildStdout,
    writer: Sender<Value>,
    events: Sender<UiEvent>,
    cwd: String,
    servers: Vec<acp::McpServer>,
) {
    let _ = writer.send(acp::initialize(1));
    let mut agent_name = String::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let incoming = match acp::parse(&line) {
            Ok(incoming) => incoming,
            Err(error) => {
                let _ = events.send(UiEvent::Error {
                    pane: pane.clone(),
                    message: error,
                });
                continue;
            }
        };
        match incoming {
            acp::Incoming::Response { id, result, error } => {
                let number = id.as_u64();
                if let Some(error) = error {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the agent returned an error")
                        .to_string();
                    let _ = events.send(UiEvent::Error {
                        pane: pane.clone(),
                        message,
                    });
                    if number == Some(1) || number == Some(2) {
                        break;
                    }
                    continue;
                }
                match number {
                    Some(1) => {
                        let result = result.unwrap_or(Value::Null);
                        agent_name = acp::agent_title(&result);
                        let _ = writer.send(acp::session_new(2, &cwd, &servers));
                    }
                    Some(2) => {
                        let result = result.unwrap_or(Value::Null);
                        let session = result
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if session.is_empty() {
                            let _ = events.send(UiEvent::Error {
                                pane: pane.clone(),
                                message: "the agent did not return a session id".to_string(),
                            });
                            break;
                        }
                        let _ = events.send(UiEvent::Ready {
                            pane: pane.clone(),
                            agent: agent_name.clone(),
                            session,
                        });
                    }
                    _ => {
                        let reason = result
                            .as_ref()
                            .map(acp::stop_reason)
                            .unwrap_or_else(|| "error".to_string());
                        let _ = events.send(UiEvent::TurnDone {
                            pane: pane.clone(),
                            reason,
                        });
                    }
                }
            }
            acp::Incoming::Notification { method, params } => {
                if method == "session/update" {
                    if let Some(update) = acp::parse_update(&params) {
                        let _ = events.send(UiEvent::Update {
                            pane: pane.clone(),
                            update,
                        });
                    }
                }
            }
            acp::Incoming::Request { id, method, params } => {
                if method == "session/request_permission" {
                    let _ = events.send(UiEvent::Permission {
                        pane: pane.clone(),
                        id,
                        title: acp::permission_title(&params),
                        options: acp::permission_options(&params),
                        writer: writer.clone(),
                    });
                } else {
                    // Not a method this client implements. Answer with an error
                    // instead of leaving the agent waiting forever.
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("unsupported method: {}", method) },
                    });
                    let _ = writer.send(response);
                }
            }
        }
    }
    let _ = events.send(UiEvent::Closed { pane });
}
