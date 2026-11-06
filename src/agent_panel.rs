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
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::{json, Value};
use zoha_vte::traits::TerminalExt;

use crate::headless::{acp, agent};
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::MainWindow;

/// The messages the reader thread sends to the GTK loop.
enum UiEvent {
    Ready {
        pane: String,
        agent: String,
        model: Option<String>,
        models: Vec<acp::ConfigChoice>,
        session: String,
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
    /// The models the agent offered in `configOptions`, if it reports any.
    models: Vec<acp::ConfigChoice>,
    transcript: String,
    ready: bool,
    busy: bool,
    next_id: u64,
    queue: Vec<(String, bool)>,
    history: Vec<String>,
    history_index: usize,
    usage: String,
    status: &'static str,
    /// True while the turn being answered came from a command-not-found
    /// hand-off; its answer is echoed into the pane's terminal.
    mirror_current: bool,
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
            if agent::configured(&settings().raw_data()) {
                self.setup.hide();
            }
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
        self.ensure_session(term, text, false);
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

    /// `/fix` without the user asking: the shell could not find the command
    /// (exit 127). The answer is echoed into the terminal and the pane keeps
    /// its normal state, so the flow stays like running a command.
    pub fn fix_for_auto(&self, term: &TerminalBox) {
        if !agent::configured(&settings().raw_data()) {
            return;
        }
        let context = term.last_command_context();
        if context.is_empty() {
            return;
        }
        let pane = term.remote_id();
        let command = crate::headless::text::truncate_chars(context.command.trim(), 80);
        let header = format!(
            "\r\n\x1b[90m[agent]\x1b[0m {}: command not found, asking…\r\n",
            agent::plain_for_terminal(&command)
        );
        term.vte().feed(header.as_bytes());
        let prompt = agent::fix_prompt(&context, None);
        let shown = format!("/fix  —  {} (not found)", command);
        self.set_active_pane(Some(pane.clone()));
        self.ensure_session(term, &prompt, true);
        self.render_to(&pane, &format!("\n▶ {}\n", shown));
        self.record_prompt(&shown);
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
                "No ACP agent was found. Install OpenCode, Claude Code, Gemini, Copilot or Codex, \
                 or open Agent settings to enter a command.",
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
                    self.ensure_session(&term, &prompt, false);
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

    fn ensure_session(&self, term: &TerminalBox, first_prompt: &str, mirror: bool) {
        let pane = term.remote_id();
        let settings = settings().raw_data();
        if !agent::enabled(&settings) {
            self.show_setup(first_prompt);
            return;
        }
        let state = self
            .sessions
            .borrow()
            .get(&pane)
            .map(|session| (session.ready, session.status));
        if let Some((ready, status)) = state {
            if ready {
                self.send_prompt(&pane, first_prompt, mirror);
                self.update_header();
                return;
            }
            if status != "error" && status != "closed" {
                if let Some(session) = self.sessions.borrow_mut().get_mut(&pane) {
                    session.queue.push((first_prompt.to_string(), mirror));
                    session.status = "starting";
                }
                self.update_header();
                return;
            }
            // A process that exited or failed the handshake is replaced on the
            // next prompt instead of swallowing it in a dead queue.
            self.sessions.borrow_mut().remove(&pane);
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
                    session.queue.push((first_prompt.to_string(), mirror));
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

    fn send_prompt(&self, pane: &str, text: &str, mirror: bool) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(pane) else {
            return;
        };
        if !session.ready || session.busy {
            session.queue.push((text.to_string(), mirror));
            return;
        }
        let Some(session_id) = session.session_id.clone() else {
            session.queue.push((text.to_string(), mirror));
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
            session.mirror_current = mirror;
        }
        let status = session.status;
        drop(sessions);
        self.publish_status(pane, status);
        self.render_active();
    }

    fn drain_events(&self) {
        // Configuration can also arrive from the Preferences window while this
        // pane is open; hide the inline setup as soon as it is valid.
        if self.setup.is_visible() && agent::configured(&settings().raw_data()) {
            self.setup.hide();
            self.update_header();
        }
        loop {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(_) => break,
            };
            match event {
                UiEvent::Ready {
                    pane,
                    agent,
                    model,
                    models,
                    session,
                } => {
                    let mut sessions = self.sessions.borrow_mut();
                    if let Some(state) = sessions.get_mut(&pane) {
                        state.ready = true;
                        state.status = "ready";
                        state.agent_name = agent;
                        state.model = model;
                        state.models = models;
                        state.session_id = Some(session);
                    }
                    let queued: Vec<(String, bool)> = sessions
                        .get_mut(&pane)
                        .map(|state| std::mem::take(&mut state.queue))
                        .unwrap_or_default();
                    drop(sessions);
                    // Before the queued prompt: the writer channel keeps the
                    // order, so the model change reaches the agent first.
                    self.apply_preferred_model(&pane);
                    self.publish_status(&pane, "ready");
                    for (text, mirror) in queued {
                        self.send_prompt(&pane, &text, mirror);
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
                    // usable and let the picker fall back to the real model.
                    self.render_to(&pane, &format!("\n[model not changed] {}\n", message));
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
                    if self.take_mirrored_turn(&pane) {
                        if let Some(window) = self.window.upgrade() {
                            if let Some(term) = window.terminal_by_remote_id(&pane) {
                                term.vte().feed(b"\r\n");
                            }
                        }
                    }
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
                    if let Some((text, mirror)) = next {
                        self.send_prompt(&pane, &text, mirror);
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
                    if self.mirroring(&pane) {
                        self.mirror_to_terminal(
                            &pane,
                            &format!("\r\n[agent error] {}\r\n", message),
                        );
                        if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                            state.mirror_current = false;
                        }
                    }
                    if let Some(state) = self.sessions.borrow_mut().get_mut(&pane) {
                        state.status = "error";
                        state.busy = false;
                        state.ready = false;
                    }
                    self.publish_status(&pane, "error");
                    self.update_header();
                }
                UiEvent::Closed { pane } => {
                    if self.take_mirrored_turn(&pane) {
                        if let Some(window) = self.window.upgrade() {
                            if let Some(term) = window.terminal_by_remote_id(&pane) {
                                term.vte().feed(b"\r\n[agent process exited]\r\n");
                            }
                        }
                    }
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
            acp::Update::AgentMessage(text) => {
                self.render_to(pane, &text);
                self.mirror_to_terminal(pane, &text);
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
                self.mirror_to_terminal(pane, &line);
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

    /// True while the turn being answered is a command-not-found hand-off.
    fn mirroring(&self, pane: &str) -> bool {
        self.sessions
            .borrow()
            .get(pane)
            .is_some_and(|session| session.mirror_current)
    }

    /// Echo agent text into the pane's terminal. The text is sanitized first:
    /// it comes from a model and must not be able to drive the terminal parser.
    fn mirror_to_terminal(&self, pane: &str, text: &str) {
        if text.is_empty() || !self.mirroring(pane) {
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if let Some(term) = window.terminal_by_remote_id(pane) {
            term.vte().feed(agent::plain_for_terminal(text).as_bytes());
        }
    }

    /// Clear the mirror flag of the turn that just finished. Returns true when
    /// that turn was a command-not-found hand-off.
    fn take_mirrored_turn(&self, pane: &str) -> bool {
        self.sessions
            .borrow_mut()
            .get_mut(pane)
            .map(|session| std::mem::take(&mut session.mirror_current))
            .unwrap_or(false)
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
    /// session starts on it too: that is what makes the choice apply to a
    /// command-not-found hand-off, whose pane never opens.
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
        let _ = session
            .writer
            .send(acp::set_config_option(id, &session_id, "model", &value));
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
        let _ = session
            .writer
            .send(acp::set_config_option(id, &session_id, "model", &preferred));
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

fn spawn_session(
    pane: String,
    argv: &[String],
    cwd: &str,
    servers: Vec<acp::McpServer>,
    events: Sender<UiEvent>,
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
        .spawn(move || read_agent(pane_for_reader, stdout, writer_thread, events, cwd, servers))
        .map_err(|error| format!("cannot start the agent reader: {}", error))?;

    Ok(Session {
        child,
        writer,
        session_id: None,
        agent_name: String::new(),
        model: None,
        models: Vec::new(),
        transcript: String::new(),
        ready: false,
        busy: false,
        next_id: 3,
        queue: Vec::new(),
        history: Vec::new(),
        history_index: 0,
        usage: String::new(),
        status: "starting",
        mirror_current: false,
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
                let is_config = number.is_some_and(|number| number >= acp::CONFIG_ID_BASE);
                if let Some(error) = error {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the agent returned an error")
                        .to_string();
                    if is_config {
                        // The turn is untouched: only the choice was refused.
                        let _ = events.send(UiEvent::ModelRejected {
                            pane: pane.clone(),
                            message,
                        });
                        continue;
                    }
                    let _ = events.send(UiEvent::Error {
                        pane: pane.clone(),
                        message,
                    });
                    if number == Some(1) || number == Some(2) {
                        break;
                    }
                    continue;
                }
                if is_config {
                    let result = result.unwrap_or(Value::Null);
                    let (model, models) = acp::config_choices(&result, "model");
                    let _ = events.send(UiEvent::ModelChanged {
                        pane: pane.clone(),
                        model,
                        models,
                    });
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
                        let (model, models) = acp::config_choices(&result, "model");
                        let _ = events.send(UiEvent::Ready {
                            pane: pane.clone(),
                            agent: agent_name.clone(),
                            model,
                            models,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    #[test]
    fn a_session_handshakes_streams_and_finishes_a_turn() {
        let script = fake_agent();
        let (tx, rx) = mpsc::channel();
        let session = spawn_session(
            "pane-1".to_string(),
            &[script.to_string_lossy().to_string()],
            "/tmp",
            Vec::new(),
            tx,
        )
        .expect("spawn fake agent");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            match rx.recv_timeout(Duration::from_millis(200)) {
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
            match rx.recv_timeout(Duration::from_millis(200)) {
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
            match rx.recv_timeout(Duration::from_millis(200)) {
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
