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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use futures_channel::mpsc::UnboundedSender;

use gtk::prelude::*;
use serde_json::{json, Value};

use crate::headless::{acp, agent};
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::MainWindow;

/// The id of the handshake request that opens or resumes the ACP session.
const SESSION_REQUEST_ID: u64 = 2;
/// A `session/new` retry after the agent refused `session/load`. Below
/// `acp::CONFIG_ID_BASE` on purpose, and far above the prompt ids the panel
/// uses.
const SESSION_NEW_FALLBACK_ID: u64 = 900_000_000;

/// The messages the reader thread sends to the GTK loop.
enum UiEvent {
    Ready {
        pane: String,
        agent: String,
        model: Option<String>,
        models: Vec<acp::ConfigChoice>,
        model_api: Option<acp::ModelApi>,
        session: String,
        /// True when the process attached to an existing session with
        /// `session/load` instead of starting a new one.
        resumed: bool,
    },
    /// The agent accepted a configuration change (the model, for now).
    ModelChanged {
        pane: String,
        model: Option<String>,
        models: Vec<acp::ConfigChoice>,
    },
    /// The agent refused it. The session stays usable: only the choice failed.
    ModelRejected {
        pane: String,
        message: String,
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
    model: Option<String>,
    /// The models the agent offered, and how to switch between them.
    models: Vec<acp::ConfigChoice>,
    model_api: Option<acp::ModelApi>,
    /// Model shown before the change in flight, restored if the agent refuses.
    model_previous: Option<String>,
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
    /// Model picker for the active tab's session. Hidden until the agent
    /// reports the choices it supports.
    model_combo: gtk::ComboBoxText,
    /// Set while the combo is filled programmatically, so filling it does not
    /// send a configuration change back to the agent.
    combo_syncing: Cell<bool>,
    combo_values: RefCell<Vec<(String, String)>>,
    usage: gtk::Label,
    view: gtk::TextView,
    entry: gtk::Entry,
    sessions: RefCell<HashMap<String, Session>>,
    active: RefCell<Option<String>>,
    setup: gtk::Box,
    setup_combo: gtk::ComboBoxText,
    setup_note: gtk::Label,
    setup_enable: gtk::Button,
    setup_commands: RefCell<Vec<String>>,
    pending_prompt: RefCell<Option<String>>,
    events_tx: UnboundedSender<UiEvent>,
    /// The Settings "changed" handler; disconnected when the pane goes away.
    settings_handler: RefCell<Option<glib::SignalHandlerId>>,
    window: crate::SendWeak<MainWindow>,
}

impl AgentPanel {
    pub fn new(window: &MainWindow) -> Rc<AgentPanel> {
        let (tx, mut rx) = futures_channel::mpsc::unbounded();

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
        let model_combo = gtk::ComboBoxText::new();
        model_combo.set_tooltip_text(Some(
            "Model used by this tab's agent session; the choice is remembered for new sessions",
        ));
        model_combo.set_size_request(220, -1);
        // The pane is revealed with show_all(); the picker appears only once
        // the agent reports which models it offers.
        model_combo.set_no_show_all(true);
        model_combo.set_visible(false);
        let usage = gtk::Label::new(Some(""));
        usage.set_xalign(1.0);
        usage.style_context().add_class("dim-label");
        let settings_button = gtk::Button::with_label("⚙");
        settings_button.set_relief(gtk::ReliefStyle::None);
        settings_button.set_tooltip_text(Some("Agent settings"));
        let fix_button = gtk::Button::with_label("Fix");
        fix_button.set_relief(gtk::ReliefStyle::None);
        fix_button.set_tooltip_text(Some("Ask the agent to fix the last failed command"));
        let close_button = gtk::Button::with_label("×");
        close_button.set_relief(gtk::ReliefStyle::None);
        close_button.set_tooltip_text(Some("Hide the agent pane (the session is kept)"));
        header_row.pack_start(&header, true, true, 0);
        header_row.pack_start(&model_combo, false, false, 0);
        header_row.pack_start(&usage, false, false, 0);
        header_row.pack_start(&settings_button, false, false, 0);
        header_row.pack_start(&fix_button, false, false, 0);
        header_row.pack_start(&close_button, false, false, 0);
        body.pack_start(&header_row, false, false, 0);

        // Inline first-run setup. It is part of the pane so the user never has
        // to edit settings.json: pick an installed agent, press Enable, and the
        // pending prompt is sent immediately.
        let setup = gtk::Box::new(gtk::Orientation::Vertical, 6);
        setup.style_context().add_class("oxterm-agent-setup");
        let setup_title = gtk::Label::new(None);
        setup_title.set_markup("<b>Agent features are not configured</b>");
        setup_title.set_xalign(0.0);
        let setup_combo = gtk::ComboBoxText::new();
        setup_combo.set_hexpand(true);
        let setup_enable = gtk::Button::with_label("Enable");
        let setup_prefs = gtk::Button::with_label("Agent settings…");
        let setup_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        setup_row.pack_start(&setup_combo, true, true, 0);
        setup_row.pack_start(&setup_enable, false, false, 0);
        setup_row.pack_start(&setup_prefs, false, false, 0);
        let setup_note = gtk::Label::new(None);
        setup_note.set_xalign(0.0);
        setup_note.set_line_wrap(true);
        setup_note.set_max_width_chars(90);
        setup_note.style_context().add_class("dim-label");
        setup.pack_start(&setup_title, false, false, 0);
        setup.pack_start(&setup_row, false, false, 0);
        setup.pack_start(&setup_note, false, false, 0);
        setup.show_all();
        setup.set_no_show_all(true);
        setup.hide();
        body.pack_start(&setup, false, false, 0);

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
        // Show the contents but not the Revealer: the pane is hidden until it
        // is opened, and a shown Revealer reserves room even when it is not
        // revealing its child.
        body.show_all();

        let panel = Rc::new(AgentPanel {
            root,
            header,
            model_combo,
            combo_syncing: Cell::new(false),
            combo_values: RefCell::new(Vec::new()),
            usage,
            view,
            entry,
            sessions: RefCell::new(HashMap::new()),
            active: RefCell::new(None),
            setup,
            setup_combo,
            setup_note,
            setup_enable,
            setup_commands: RefCell::new(Vec::new()),
            pending_prompt: RefCell::new(None),
            events_tx: tx,
            settings_handler: RefCell::new(None),
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
        {
            let panel = panel.clone();
            let combo = panel.model_combo.clone();
            combo.connect_changed(move |combo| panel.select_model(combo));
        }
        {
            let panel = panel.clone();
            settings_button.connect_clicked(move |_| panel.open_agent_settings());
        }
        {
            let panel = panel.clone();
            setup_prefs.connect_clicked(move |_| panel.open_agent_settings());
        }
        {
            let enable = panel.setup_enable.clone();
            let panel = panel.clone();
            enable.connect_clicked(move |_| panel.enable_selected_agent());
        }

        // Wake the UI only when a session thread has something to report. The
        // async channel is drained by a main-context future, so the `Rc` to
        // the panel never leaves the main thread (the old worker thread cloned
        // a non-atomic `Rc::Weak` from another thread, which is undefined
        // behavior).
        {
            let weak = Rc::downgrade(&panel);
            glib::spawn_future_local(async move {
                while let Ok(event) = rx.recv().await {
                    if let Some(panel) = weak.upgrade() {
                        panel.handle_event(event);
                    }
                }
            });
        }
        // The setup panel must disappear as soon as the agent is configured in
        // Preferences; there is no polling tick to notice it any more.
        {
            let weak = Rc::downgrade(&panel);
            let handler = settings().connect_changed(move || {
                if let Some(panel) = weak.upgrade() {
                    panel.refresh_setup();
                }
            });
            *panel.settings_handler.borrow_mut() = Some(handler);
        }

        panel
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.root
    }

    /// The rendered transcript, used by the GUI scenario tests.
    pub fn transcript_text(&self) -> String {
        let buffer = self.view.buffer().expect("text view has a buffer");
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .map(|text| text.to_string())
            .unwrap_or_default()
    }

    pub fn is_visible(&self) -> bool {
        self.root.reveals_child()
    }

    pub fn set_visible(&self, visible: bool) {
        if visible {
            self.root.show();
            if let Some(window) = self.window.upgrade() {
                window.set_agent_dock_visible(true);
            }
            self.root.set_reveal_child(true);
            if agent::configured(&settings().raw_data()) {
                self.setup.hide();
            }
            self.render_active();
            self.entry.grab_focus();
        } else {
            self.root.set_reveal_child(false);
            // A Revealer that is merely not revealing still asks for its
            // child's height, which left the terminal short and an empty band
            // below it. Hiding the widget after the slide-out gives the space
            // back; showing it again before this runs keeps the pane open.
            let root = self.root.clone();
            let window = self.window.clone();
            glib::timeout_add_local_once(Duration::from_millis(220), move || {
                if !root.reveals_child() {
                    root.hide();
                    if let Some(window) = window.upgrade() {
                        window.set_agent_dock_visible(false);
                    }
                }
            });
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

    /// Show the inline first-run setup. `prompt` is sent automatically once the
    /// user enables an agent; an empty prompt just configures.
    pub fn show_setup(&self, prompt: &str) {
        if !prompt.trim().is_empty() {
            *self.pending_prompt.borrow_mut() = Some(prompt.to_string());
        }
        let detected = agent::detected_agents();
        self.setup_combo.remove_all();
        let mut commands = Vec::new();
        for (agent, path) in &detected {
            let command = agent.command_with_path(path);
            self.setup_combo
                .append_text(&format!("{} — {}", agent.label, command));
            commands.push(command);
        }
        *self.setup_commands.borrow_mut() = commands;
        if detected.is_empty() {
            self.setup_combo.set_sensitive(false);
            self.setup_enable.set_sensitive(false);
            self.setup_note.set_text(
                "No ACP agent was found. Install OpenCode, Claude Code, Antigravity, Gemini, \
                 Copilot or Codex, or open Agent settings to enter a command.",
            );
        } else {
            self.setup_combo.set_sensitive(true);
            self.setup_enable.set_sensitive(true);
            self.setup_combo.set_active(Some(0));
            self.setup_note.set_text(
                "Choose an installed agent and press Enable; the prompt is sent right after. \
                 Nothing is installed or started before that.",
            );
        }
        self.set_visible(true);
        self.setup.show();
        if self.setup_combo.is_sensitive() {
            self.setup_combo.grab_focus();
        }
    }

    fn enable_selected_agent(&self) {
        let index = self.setup_combo.active().unwrap_or(0) as usize;
        let Some(command) = self.setup_commands.borrow().get(index).cloned() else {
            self.setup_note.set_text("Choose an agent first.");
            return;
        };
        let s = settings();
        s.begin_batch();
        s.set_bool("agent_enabled", true);
        s.set_str("agent_command", &command);
        s.end_batch();
        s.notify_changed();
        self.setup.hide();
        self.update_header();
        let prompt = self.pending_prompt.borrow_mut().take();
        if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
            if let Some(window) = self.window.upgrade() {
                if let Some(term) = window.current_terminal() {
                    self.ensure_session(&term, &prompt);
                }
            }
        }
    }

    fn open_agent_settings(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let parent: gtk::Window = window.clone().upcast();
        crate::settings_dialog::show_settings_dialog_at(Some(&parent), Some("agents"));
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
        let settings = settings().raw_data();
        if !agent::enabled(&settings) {
            self.show_setup(first_prompt);
            return;
        }
        let mut previous: Option<Session> = None;
        let state = self.sessions.borrow_mut().get_mut(&pane).map(|session| {
            let alive = session.child.try_wait().ok().flatten().is_none();
            (session.ready, session.status, alive)
        });
        if let Some((ready, status, alive)) = state {
            if ready && alive {
                self.send_prompt(&pane, first_prompt);
                self.update_header();
                return;
            }
            if alive && status != "error" && status != "closed" {
                if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                    session.queue.push(first_prompt.to_string());
                    session.status = "starting";
                }
                self.update_header();
                return;
            }
            // A process that exited or failed the handshake is replaced on the
            // next prompt instead of swallowing it in a dead queue. Keep the
            // old session so its transcript, history and ACP id can be reused:
            // the replacement asks `session/load` and the agent continues the
            // same conversation.
            previous = self.sessions.borrow_mut().remove(&pane);
        }

        if !agent::configured(&settings) {
            self.show_setup(first_prompt);
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
            // ssh joins the command arguments with spaces and hands the result
            // to the remote shell, so keep the user's string intact instead of
            // splitting and re-joining (which would drop quoting).
            args.push(remote_command);
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

        let resume = previous
            .as_ref()
            .and_then(|session| session.session_id.clone());
        let resumed = resume.is_some();
        let message = if resumed {
            format!("\nResuming {} in {}…\n", argv.join(" "), cwd)
        } else {
            format!("\nStarting {} in {}…\n", argv.join(" "), cwd)
        };
        match spawn_session(
            pane.clone(),
            &argv,
            &cwd,
            servers,
            self.events_tx.clone(),
            resume.clone(),
        ) {
            Ok(mut session) => {
                if let Some(mut old) = previous {
                    // Keep the conversation on screen and in the input history;
                    // only the child process is new.
                    session.transcript = std::mem::take(&mut old.transcript);
                    session.history = std::mem::take(&mut old.history);
                    session.history_index = old.history_index;
                    session.usage = std::mem::take(&mut old.usage);
                    session.agent_name = std::mem::take(&mut old.agent_name);
                    session.model = old.model.take();
                    session.models = std::mem::take(&mut old.models);
                    session.model_api = old.model_api;
                    session.model_previous = old.model_previous.take();
                    session.session_id = resume;
                }
                self.sessions.borrow_mut().insert(pane.clone(), session);
                self.render_to(&pane, &message);
                // The session is not ready until the handshake completes; queue
                // the first prompt so the user does not have to repeat it.
                if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                    session.queue.push(first_prompt.to_string());
                    session.status = "starting";
                }
                self.update_header();
            }
            Err(error) => {
                self.append_text(&format!(
                    "\nCannot start the agent: {}\n\
                     If the command works in your shell but not here, use the absolute path \
                     printed by `oxterm agent --list`.\n",
                    error
                ));
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
        if session.child.try_wait().ok().flatten().is_some() {
            // The process is gone but the reader has not reported it yet. Do
            // not write into a dead pipe: the next prompt restarts and resumes
            // the session.
            session.queue.push(text.to_string());
            session.ready = false;
            session.status = "closed";
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

    /// Hide the inline setup once the agent becomes configured, including when
    /// the user enables it from the Preferences window.
    fn refresh_setup(&self) {
        if self.setup.is_visible() && agent::configured(&settings().raw_data()) {
            self.setup.hide();
            self.update_header();
        }
    }

    fn handle_event(&self, event: UiEvent) {
        match event {
            UiEvent::Ready {
                pane,
                agent,
                model,
                models,
                model_api,
                session,
                resumed,
            } => {
                let had_transcript = self
                    .sessions
                    .borrow()
                    .get(&pane)
                    .is_some_and(|state| !state.transcript.is_empty());
                let mut sessions = self.sessions.borrow_mut();
                if let Some(state) = sessions.get_mut(&pane) {
                    state.ready = true;
                    state.status = "ready";
                    if !agent.is_empty() {
                        state.agent_name = agent;
                    }
                    if model.is_some() {
                        state.model = model;
                    }
                    if !models.is_empty() {
                        state.models = models;
                    }
                    if model_api.is_some() {
                        state.model_api = model_api;
                    }
                    state.session_id = Some(session);
                }
                let queued: Vec<String> = sessions
                    .get_mut(&pane)
                    .map(|state| std::mem::take(&mut state.queue))
                    .unwrap_or_default();
                drop(sessions);
                if resumed && had_transcript {
                    self.render_to(&pane, "\n[agent session resumed]\n");
                }
                // Before the queued prompt: the writer channel keeps the
                // order, so the model change reaches the agent first.
                self.apply_preferred_model(&pane);
                self.publish_status(&pane, "ready");
                for text in queued {
                    self.send_prompt(&pane, &text);
                }
                self.update_header();
            }
            UiEvent::ModelChanged {
                pane,
                model,
                models,
            } => {
                if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                    if model.is_some() {
                        state.model = model;
                    }
                    if !models.is_empty() {
                        state.models = models;
                    }
                }
                if self.active_pane().as_deref() == Some(pane.as_str()) {
                    self.update_header();
                }
            }
            UiEvent::ModelRejected { pane, message } => {
                // A refused choice is not a failed session: keep the agent
                // usable and put the picker back on the model in use.
                self.render_to(&pane, &format!("\n[model not changed] {}\n", message));
                if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                    state.model = state.model_previous.clone();
                }
                if self.active_pane().as_deref() == Some(pane.as_str()) {
                    self.update_header();
                }
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
                if let Some(text) = next {
                    self.send_prompt(&pane, &text);
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
                let mut retry: Vec<String> = Vec::new();
                let mut sessions = self.sessions.borrow_mut();
                if let Some(state) = sessions.get_mut(&pane) {
                    let never_connected = state.agent_name.is_empty();
                    state.status = "closed";
                    state.ready = false;
                    state.busy = false;
                    state.append("\n[the agent process exited]\n");
                    if never_connected {
                        state.append(
                            "If it exited immediately, check that the command speaks ACP. \
                                 `oxterm agent --list` shows the known agent CLIs.\n",
                        );
                    } else if state.session_id.is_some() {
                        state.append(
                            "The next prompt restarts the agent and resumes this session.\n",
                        );
                    }
                    if !state.queue.is_empty() {
                        // Prompts that arrived while the process was dying
                        // were never sent. Restart and resume now instead of
                        // losing them.
                        retry = std::mem::take(&mut state.queue);
                    }
                }
                drop(sessions);
                self.publish_status(&pane, "closed");
                if let Some(first) = retry.first().cloned() {
                    if let Some(window) = self.window.upgrade() {
                        if let Some(term) = window.terminal_by_remote_id(&pane) {
                            self.ensure_session(&term, &first);
                            retry.remove(0);
                            if !retry.is_empty() {
                                if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                                    state.queue.extend(retry);
                                }
                            }
                        }
                    }
                }
                self.render_active();
                self.update_header();
            }
        }
    }

    fn render_update(&self, pane: &str, update: acp::Update) {
        match update {
            acp::Update::AgentMessage(text) => {
                self.render_to(pane, &text);
            }
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
                let line = format!("\n[{}] {}\n", severity, title);
                self.render_to(pane, &line);
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
        // The request belongs in the conversation, not only in a dialog that
        // can sit behind the window: the transcript explains why the turn
        // stopped when the tool is refused.
        self.render_to(pane, &format!("\n[permission] {}\n", title));
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
        // No default button on purpose: Enter is how a prompt is submitted, and
        // a dialog that appears while someone is typing must not turn that key
        // into a silent "deny" of the tool. The answer has to be a click.
        dialog.set_default_response(gtk::ResponseType::None);
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
        self.render_to(
            pane,
            if accepted {
                "[permission] allowed\n"
            } else {
                "[permission] denied\n"
            },
        );
        if self.active_pane().as_deref() == Some(pane) {
            self.update_header();
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

    /// The user picked a model in the header. It is remembered, so the next
    /// session starts on it too, including the one started by `?prompt`.
    fn select_model(&self, combo: &gtk::ComboBoxText) {
        if self.combo_syncing.get() {
            return;
        }
        let Some(value) = combo.active_id().map(|value| value.to_string()) else {
            return;
        };
        let Some(pane) = self.active_pane() else {
            return;
        };
        settings().set_str("agent_model", &value);
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&pane) else {
            return;
        };
        if session.model.as_deref() == Some(value.as_str()) {
            return;
        }
        let Some(session_id) = session.session_id.clone() else {
            return;
        };
        let id = acp::CONFIG_ID_BASE + session.next_id;
        session.next_id += 1;
        session.model_previous = session.model.clone();
        session.model = Some(value.clone());
        let request = model_request(id, &session_id, session.model_api, &value);
        let _ = session.writer.send(request);
    }

    /// A fresh session starts on the model remembered in the settings. A value
    /// the agent does not list is ignored, so a stale setting cannot keep every
    /// session from working.
    fn apply_preferred_model(&self, pane: &str) {
        let preferred = settings().get_str("agent_model").trim().to_string();
        if preferred.is_empty() {
            return;
        }
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(pane) else {
            return;
        };
        if session.model.as_deref() == Some(preferred.as_str()) {
            return;
        }
        if !session.models.is_empty()
            && !session
                .models
                .iter()
                .any(|choice| choice.value == preferred)
        {
            return;
        }
        let Some(session_id) = session.session_id.clone() else {
            return;
        };
        let id = acp::CONFIG_ID_BASE + session.next_id;
        session.next_id += 1;
        session.model_previous = session.model.clone();
        session.model = Some(preferred.clone());
        let request = model_request(id, &session_id, session.model_api, &preferred);
        let _ = session.writer.send(request);
    }

    /// Fill the picker without waking the `changed` handler.
    fn sync_model_combo(&self, current: Option<&str>, models: &[acp::ConfigChoice]) {
        let mut values: Vec<(String, String)> = models
            .iter()
            .map(|choice| (choice.value.clone(), choice.name.clone()))
            .collect();
        // A current value the agent did not list still has to be selectable,
        // otherwise the picker would show nothing.
        if let Some(current) = current {
            if !values.iter().any(|(value, _)| value == current) {
                values.push((current.to_string(), current.to_string()));
            }
        }
        if *self.combo_values.borrow() != values {
            self.combo_syncing.set(true);
            self.model_combo.remove_all();
            for (value, name) in &values {
                self.model_combo.append(Some(value), name);
            }
            self.combo_syncing.set(false);
            *self.combo_values.borrow_mut() = values;
        }
        if let Some(current) = current {
            let active = self.model_combo.active_id().map(|id| id.to_string());
            if active.as_deref() != Some(current) {
                self.combo_syncing.set(true);
                self.model_combo.set_active_id(Some(current));
                self.combo_syncing.set(false);
            }
        }
        self.model_combo.set_visible(!models.is_empty());
    }

    fn update_header(&self) {
        let (name, model, models, status, usage) = self
            .active_pane()
            .and_then(|pane| {
                self.sessions.borrow().get(&pane).map(|session| {
                    (
                        session.agent_name.clone(),
                        session.model.clone(),
                        session.models.clone(),
                        session.status,
                        session.usage.clone(),
                    )
                })
            })
            .unwrap_or_else(|| {
                (
                    "not started".to_string(),
                    None,
                    Vec::new(),
                    "",
                    String::new(),
                )
            });
        let mut text = String::from("Agent");
        if !name.is_empty() {
            text.push_str(" · ");
            text.push_str(&name);
        }
        if let Some(model) = model.as_ref().filter(|model| !model.is_empty()) {
            text.push_str(" · ");
            text.push_str(model);
        }
        if !status.is_empty() {
            text.push_str(" · ");
            text.push_str(status);
        }
        if !settings().get_bool("agent_enabled") {
            text.push_str(" · disabled");
        }
        self.header.set_text(&text);
        self.sync_model_combo(model.as_deref(), &models);
        self.usage.set_text(&usage);
        self.usage
            .set_visible(settings().get_bool("agent_show_usage") && !usage.is_empty());
    }
}

impl Drop for AgentPanel {
    fn drop(&mut self) {
        if let Some(handler) = self.settings_handler.borrow_mut().take() {
            settings().disconnect_changed(handler);
        }
    }
}

/// The request that switches models for the API this session announced.
fn model_request(id: u64, session_id: &str, api: Option<acp::ModelApi>, value: &str) -> Value {
    match api {
        Some(acp::ModelApi::SetModel) => acp::set_model(id, session_id, value),
        _ => acp::set_config_option(id, session_id, "model", value),
    }
}

fn spawn_session(
    pane: String,
    argv: &[String],
    cwd: &str,
    servers: Vec<acp::McpServer>,
    events: UnboundedSender<UiEvent>,
    resume: Option<String>,
) -> Result<Session, String> {
    let program = agent::resolve_program(argv.first().ok_or("no agent command")?);
    let mut child = Command::new(&program)
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
        .spawn(move || {
            read_agent(
                pane_for_reader,
                stdout,
                writer_thread,
                events,
                cwd,
                servers,
                resume,
            )
        })
        .map_err(|error| format!("cannot start the agent reader: {}", error))?;

    Ok(Session {
        child,
        writer,
        session_id: None,
        agent_name: String::new(),
        model: None,
        models: Vec::new(),
        model_api: None,
        model_previous: None,
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
    events: UnboundedSender<UiEvent>,
    cwd: String,
    servers: Vec<acp::McpServer>,
    resume: Option<String>,
) {
    let _ = writer.send(acp::initialize(1));
    let mut agent_name = String::new();
    // True while the pending handshake request is `session/load` rather than
    // `session/new`.
    let mut loading = false;
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
                let _ = events.unbounded_send(UiEvent::Error {
                    pane: pane.clone(),
                    message: error,
                });
                continue;
            }
        };
        match incoming {
            acp::Incoming::Response { id, result, error } => {
                let number = id.as_u64();
                let is_config = number.is_some_and(|number| number >= acp::CONFIG_ID_BASE);
                if let Some(error) = error {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the agent returned an error")
                        .to_string();
                    if is_config {
                        // The turn is untouched: only the choice was refused.
                        let _ = events.unbounded_send(UiEvent::ModelRejected {
                            pane: pane.clone(),
                            message,
                        });
                        continue;
                    }
                    if loading && number == Some(SESSION_REQUEST_ID) {
                        // The agent advertised `session/load` but refused this
                        // one. Do not leave the user without an agent: start a
                        // fresh session and keep the transcript on screen.
                        let _ = events.unbounded_send(UiEvent::Error {
                            pane: pane.clone(),
                            message: format!("could not resume the agent session: {message}"),
                        });
                        loading = false;
                        let _ =
                            writer.send(acp::session_new(SESSION_NEW_FALLBACK_ID, &cwd, &servers));
                        continue;
                    }
                    let _ = events.unbounded_send(UiEvent::Error {
                        pane: pane.clone(),
                        message,
                    });
                    if number == Some(1)
                        || number == Some(SESSION_REQUEST_ID)
                        || number == Some(SESSION_NEW_FALLBACK_ID)
                    {
                        break;
                    }
                    continue;
                }
                if is_config {
                    let result = result.unwrap_or(Value::Null);
                    let models = acp::session_models(&result);
                    // `session/set_model` may answer with an empty result: the
                    // panel already shows the requested value.
                    let (model, choices) = models
                        .map(|models| (models.current, models.choices))
                        .unwrap_or((None, Vec::new()));
                    let _ = events.unbounded_send(UiEvent::ModelChanged {
                        pane: pane.clone(),
                        model,
                        models: choices,
                    });
                    continue;
                }
                match number {
                    Some(1) => {
                        let result = result.unwrap_or(Value::Null);
                        agent_name = acp::agent_title(&result);
                        if let Some(session) =
                            resume.as_deref().filter(|_| acp::supports_load(&result))
                        {
                            loading = true;
                            let _ = writer.send(acp::session_load(
                                SESSION_REQUEST_ID,
                                session,
                                &cwd,
                                &servers,
                            ));
                        } else {
                            let _ =
                                writer.send(acp::session_new(SESSION_REQUEST_ID, &cwd, &servers));
                        }
                    }
                    Some(SESSION_REQUEST_ID) | Some(SESSION_NEW_FALLBACK_ID) => {
                        let result = result.unwrap_or(Value::Null);
                        let resumed = loading && number == Some(SESSION_REQUEST_ID);
                        loading = false;
                        let session = result
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let session = if session.is_empty() && resumed {
                            resume.clone().unwrap_or_default()
                        } else {
                            session
                        };
                        if session.is_empty() {
                            let _ = events.unbounded_send(UiEvent::Error {
                                pane: pane.clone(),
                                message: "the agent did not return a session id".to_string(),
                            });
                            break;
                        }
                        let models = acp::session_models(&result);
                        let (model, choices, model_api) = models
                            .map(|models| (models.current, models.choices, Some(models.api)))
                            .unwrap_or((None, Vec::new(), None));
                        let _ = events.unbounded_send(UiEvent::Ready {
                            pane: pane.clone(),
                            agent: agent_name.clone(),
                            model,
                            models: choices,
                            model_api,
                            session,
                            resumed,
                        });
                    }
                    _ => {
                        let reason = result
                            .as_ref()
                            .map(acp::stop_reason)
                            .unwrap_or_else(|| "error".to_string());
                        let _ = events.unbounded_send(UiEvent::TurnDone {
                            pane: pane.clone(),
                            reason,
                        });
                    }
                }
            }
            acp::Incoming::Notification { method, params } => {
                if method == "session/update" {
                    // `session/load` replays the stored conversation as
                    // notifications before it answers. The pane already has
                    // that transcript, so rendering the replay would duplicate
                    // it. Live updates start after the load response.
                    if loading {
                        continue;
                    }
                    if let Some(update) = acp::parse_update(&params) {
                        let _ = events.unbounded_send(UiEvent::Update {
                            pane: pane.clone(),
                            update,
                        });
                    }
                }
            }
            acp::Incoming::Request { id, method, params } => {
                if method == "session/request_permission" {
                    let _ = events.unbounded_send(UiEvent::Permission {
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
    let _ = events.unbounded_send(UiEvent::Closed { pane });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Blocking receive with a timeout for the panel's async event channel;
    /// the unit tests do not run a main context.
    fn recv_timeout(
        rx: &mut futures_channel::mpsc::UnboundedReceiver<UiEvent>,
        timeout: std::time::Duration,
    ) -> Result<UiEvent, ()> {
        let deadline = Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(event) => return Ok(event),
                Err(futures_channel::mpsc::TryRecvError::Closed) => return Err(()),
                Err(futures_channel::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => return Err(()),
            }
        }
    }

    fn fake_agent() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oxterm-fake-agent-{}-{}.py",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": {"name": "fake", "title": "Fake Agent"}}}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "sessionId": "s1",
            "configOptions": [{"id": "model", "currentValue": "one",
                "options": [{"value": "one", "name": "Model One"},
                            {"value": "two", "name": "Model Two"}]}]}}), flush=True)
    elif method == "session/set_config_option":
        value = message["params"]["value"]
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "configOptions": [{"id": "model", "currentValue": value,
                "options": [{"value": "one", "name": "Model One"},
                            {"value": "two", "name": "Model Two"}]}]}}), flush=True)
    elif method == "session/prompt":
        print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "s1", "update": {"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello from fake"}}}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                          "result": {"stopReason": "end_turn"}}), flush=True)
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// Spawning a script that was just written can race with the kernel and
    /// fail with ETXTBSY ("Text file busy"). Retry briefly instead of turning a
    /// test-only timing artifact into a failure.
    fn spawn_retrying(
        pane: &str,
        argv: &[String],
        events: UnboundedSender<UiEvent>,
        resume: Option<String>,
    ) -> Session {
        let mut attempt = 0;
        loop {
            match spawn_session(
                pane.to_string(),
                argv,
                "/tmp",
                Vec::new(),
                events.clone(),
                resume.clone(),
            ) {
                Ok(session) => return session,
                Err(error) if error.contains("Text file busy") && attempt < 40 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => panic!("spawn fake agent: {error}"),
            }
        }
    }

    #[test]
    fn a_session_resumes_with_load_when_the_agent_supports_it() {
        let log = std::env::temp_dir().join(format!(
            "oxterm-load-agent-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script = std::env::temp_dir().join(format!(
            "oxterm-load-agent-{}-{}.py",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &script,
            format!(
                r#"#!/usr/bin/env python3
import json, sys
log = {log:?}
def options(value):
    return [{{"id": "model", "name": "Model", "currentValue": value,
             "options": [{{"value": "m", "name": "M"}}]}}]
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        print(json.dumps({{"jsonrpc": "2.0", "id": message["id"], "result": {{
            "protocolVersion": 1,
            "agentCapabilities": {{"loadSession": True}},
            "agentInfo": {{"name": "fake", "title": "Fake Agent"}}}}}}), flush=True)
    elif method == "session/load":
        with open(log, "a") as handle:
            handle.write("load " + message["params"]["sessionId"] + "\n")
        # Replay the stored conversation before answering, exactly like a real
        # agent. The pane must not render this again.
        print(json.dumps({{"jsonrpc": "2.0", "method": "session/update", "params": {{
            "sessionId": message["params"]["sessionId"],
            "update": {{"sessionUpdate": "agent_message_chunk",
            "content": {{"type": "text", "text": "OLD ANSWER"}}}}}}}}), flush=True)
        print(json.dumps({{"jsonrpc": "2.0", "id": message["id"], "result": {{
            "sessionId": message["params"]["sessionId"],
            "configOptions": options("m")}}}}), flush=True)
    elif method == "session/new":
        with open(log, "a") as handle:
            handle.write("new\n")
        print(json.dumps({{"jsonrpc": "2.0", "id": message["id"], "result": {{
            "sessionId": "new-id", "configOptions": options("m")}}}}), flush=True)
"#,
                log = log
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        let argv = vec![script.to_string_lossy().to_string()];
        let session = spawn_retrying("pane-load", &argv, tx, Some("s-old".to_string()));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            match recv_timeout(&mut rx, Duration::from_millis(200)) {
                Ok(UiEvent::Ready {
                    session,
                    resumed: true,
                    ..
                }) => {
                    assert_eq!(session, "s-old");
                    ready = true;
                }
                Ok(UiEvent::Update { update, .. }) => {
                    panic!("replayed an update while loading: {update:?}")
                }
                Ok(UiEvent::Error { message, .. }) => panic!("handshake failed: {message}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(ready, "the resumed session never became ready");
        drop(session);
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(logged, "load s-old\n");
        std::fs::remove_file(&log).ok();
        std::fs::remove_file(&script).ok();
    }

    #[test]
    fn a_session_handshakes_streams_and_finishes_a_turn() {
        let script = fake_agent();
        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        let argv = vec![script.to_string_lossy().to_string()];
        let session = spawn_retrying("pane-1", &argv, tx, None);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            match recv_timeout(&mut rx, Duration::from_millis(200)) {
                Ok(UiEvent::Ready {
                    agent,
                    model,
                    models,
                    session,
                    ..
                }) => {
                    assert_eq!(agent, "Fake Agent");
                    assert_eq!(session, "s1");
                    assert_eq!(model.as_deref(), Some("one"));
                    assert_eq!(
                        models
                            .iter()
                            .map(|choice| choice.value.as_str())
                            .collect::<Vec<_>>(),
                        vec!["one", "two"]
                    );
                    ready = true;
                }
                Ok(UiEvent::Error { message, .. }) => panic!("handshake failed: {message}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(ready, "the session never became ready");

        // A configuration change must answer with ModelChanged, never with a
        // TurnDone: the two share the response shape and only the id tells them
        // apart.
        session
            .writer
            .send(acp::set_config_option(
                acp::CONFIG_ID_BASE + 9,
                "s1",
                "model",
                "two",
            ))
            .unwrap();
        let mut switched = false;
        while Instant::now() < deadline && !switched {
            match recv_timeout(&mut rx, Duration::from_millis(200)) {
                Ok(UiEvent::ModelChanged { model, models, .. }) => {
                    assert_eq!(model.as_deref(), Some("two"));
                    assert_eq!(models.len(), 2);
                    switched = true;
                }
                Ok(UiEvent::TurnDone { .. }) => panic!("a config change completed a turn"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(switched, "the model change was not reported");

        session
            .writer
            .send(acp::session_prompt(3, "s1", "hi"))
            .unwrap();
        let mut said_hello = false;
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            match recv_timeout(&mut rx, Duration::from_millis(200)) {
                Ok(UiEvent::Update {
                    update: acp::Update::AgentMessage(text),
                    ..
                }) => {
                    assert_eq!(text, "hello from fake");
                    said_hello = true;
                }
                Ok(UiEvent::TurnDone { reason, .. }) => {
                    assert_eq!(reason, "end_turn");
                    finished = true;
                }
                Ok(UiEvent::Error { message, .. }) => panic!("agent error: {message}"),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        assert!(said_hello, "no streamed message arrived");
        assert!(finished, "the turn never finished");
        std::fs::remove_file(script).ok();
    }
}
