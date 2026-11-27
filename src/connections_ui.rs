//! The connections feature on the main window: panel behaviour, the connect
//! flow and tunnel control. Kept out of `window.rs`, which only hosts the slot
//! and forwards a few calls here.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glib::subclass::prelude::*;
use gtk::gdk;
use gtk::prelude::*;
use zoha_vte::traits::TerminalExt;

use crate::connections_panel::{ConnectionsPanel, PanelData, RowDecoration};
use crate::headless::askpass;
use crate::headless::connections::{
    self, tunnel_key, Auth, Connection, Entry, ExpectRule, Kind, Origin, Prepared, Protocol,
    Tunnel, UsageMap,
};
use crate::headless::desktop;
use crate::headless::preconnect;
use crate::headless::reachability::{self, ProbeState};
use crate::headless::remote_files;
use crate::headless::ssh_config;
use crate::headless::tunnels;
use crate::headless::workspaces::{self, WorkspaceEntry};
use crate::logging::LOGGER;
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::{MainWindow, TerminalWindow};

const DEFAULT_RECONNECT_DELAY: u32 = 5;
/// A virtual desktop can take a while to come up, and someone may be typing a
/// password in the tab first.
const DESKTOP_WAIT: Duration = Duration::from_secs(180);
const MENU_SHORTCUTS: usize = 10;

/// Where a connection opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Tab,
    Split,
    Window,
    /// The right or lower side, as in a restored session: splits the window if
    /// it is not split yet, or adds a tab to that side.
    OtherSide {
        vertical: bool,
    },
}

/// The book merged with `~/.ssh/config`, or why the book could not be read.
pub fn load_entries() -> Result<Vec<Entry>, String> {
    let book = connections::load_book(&connections::book_path())?;
    Ok(connections::merge(&book, &ssh_config::load_hosts()))
}

/// Whether a terminal tab is the SSH session behind a Monitor target.
/// `exact` restricts the comparison to identical spellings; the second pass
/// also accepts the same host with a different user/port spelling.
fn term_matches_target(term: &TerminalBox, target: &str, entries: &[Entry], exact: bool) -> bool {
    let live = term.ssh_destination();
    if live.as_deref() == Some(target) {
        return true;
    }
    let saved_match = term.connection_id().and_then(|id| {
        let entry = connections::find_entry_by_id(entries, &id)?;
        if entry.connection.protocol != Protocol::Ssh {
            return None;
        }
        let saved = if entry.connection.kind == Kind::Alias {
            entry.connection.name.clone()
        } else {
            entry.connection.summary()
        };
        if saved == target || entry.connection.destination() == target {
            return Some(true);
        }
        if exact {
            return Some(false);
        }
        Some(crate::headless::monitor::same_ssh_host(&saved, target))
    });
    if saved_match == Some(true) {
        return true;
    }
    if exact {
        return false;
    }
    live.as_deref()
        .is_some_and(|value| crate::headless::monitor::same_ssh_host(value, target))
}

impl MainWindow {
    // ── Panel ────────────────────────────────────────────────

    fn ensure_connections_panel(&self) -> bool {
        if self.imp().connections_panel.borrow().is_some() {
            return true;
        }
        let Some(slot) = self.imp().connections_slot.borrow().clone() else {
            return false;
        };
        let panel = Rc::new(ConnectionsPanel::build());
        ConnectionsPanel::connect_tree_memory(&panel);
        let weak = crate::SendWeak::new(self);
        panel.close.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.set_connections_panel_visible(false);
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.add.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.new_connection();
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.tunnels.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.show_tunnel_manager(None);
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.live_only.connect_toggled(move |_| {
            if let Some(window) = weak.upgrade() {
                window.refilter_connections_panel();
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.reachable_only.connect_toggled(move |_| {
            if let Some(window) = weak.upgrade() {
                window.refilter_connections_panel();
            }
        });
        // A destroy can arrive while a probe batch is still running: the flag
        // makes the worker discard its results instead of touching dead widgets.
        let weak = crate::SendWeak::new(self);
        self.connect_destroy(move |_| {
            if let Some(window) = weak.upgrade() {
                if let Some(panel) = window.imp().connections_panel.borrow().as_ref() {
                    panel.stop_reachability_timer();
                }
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.search.connect_search_changed(move |_| {
            if let Some(window) = weak.upgrade() {
                // Each key press only filters: no files to read again.
                window.refilter_connections_panel();
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.search.connect_activate(move |_| {
            if let Some(window) = weak.upgrade() {
                window.connect_first_result();
            }
        });
        let tree = panel.tree.clone();
        let weak = crate::SendWeak::new(self);
        panel
            .search
            .connect_key_press_event(move |_, event| match event.keyval() {
                gdk::keys::constants::Down => {
                    tree.grab_focus();
                    glib::Propagation::Stop
                }
                gdk::keys::constants::Escape => {
                    if let Some(window) = weak.upgrade() {
                        window.set_connections_panel_visible(false);
                    }
                    glib::Propagation::Stop
                }
                key => match weak.upgrade() {
                    Some(window) if window.panel_shortcut(key, event.state()) => {
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                },
            });
        let weak = crate::SendWeak::new(self);
        panel.tree.connect_row_activated(move |tree, path, _| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let panel = window.imp().connections_panel.borrow().clone();
            let Some(panel) = panel else {
                return;
            };
            // A double click on one of several selected rows opens the whole
            // selection; a click on a group still just folds it.
            let ids = panel.selected_ids();
            if ids.len() > 1 {
                window.connect_ids_staggered(ids, Placement::Tab);
                return;
            }
            match panel.id_at(path) {
                Some(id) => window.connect_id(&id, Placement::Tab),
                None => {
                    if tree.row_expanded(path) {
                        tree.collapse_row(path);
                    } else {
                        tree.expand_row(path, false);
                    }
                }
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.tree.connect_key_press_event(move |_, event| {
            let Some(window) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let key = event.keyval();
            let control = event.state().contains(gdk::ModifierType::CONTROL_MASK);
            if window.panel_shortcut(key, event.state()) {
                return glib::Propagation::Stop;
            }
            let panel = window.imp().connections_panel.borrow().clone();
            let selected = panel.as_ref().and_then(|panel| panel.selected_id());
            match (key, selected) {
                (gdk::keys::constants::Return | gdk::keys::constants::KP_Enter, Some(_)) => {
                    // Handle Enter here instead of letting GTK activate the
                    // row, so a multi-selection is opened exactly once.
                    let ids = panel
                        .as_ref()
                        .map(|panel| panel.selected_ids())
                        .unwrap_or_default();
                    window.connect_ids_staggered(
                        ids,
                        if control {
                            Placement::Split
                        } else {
                            Placement::Tab
                        },
                    );
                    glib::Propagation::Stop
                }
                (gdk::keys::constants::Delete, Some(id)) => {
                    window.delete_connection(&id);
                    glib::Propagation::Stop
                }
                (gdk::keys::constants::F2, Some(id)) => {
                    window.edit_connection(&id);
                    glib::Propagation::Stop
                }
                (gdk::keys::constants::Escape, _) => {
                    window.set_connections_panel_visible(false);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        let weak = crate::SendWeak::new(self);
        panel.tree.connect_button_press_event(move |tree, event| {
            if event.button() != 3 {
                return glib::Propagation::Proceed;
            }
            let Some(window) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let (x, y) = event.position();
            let Some((Some(path), _, _, _)) = tree.path_at_pos(x as i32, y as i32) else {
                // The empty part of the panel is the natural place for the
                // workspace actions: they act on the whole selection, or on
                // every connection when nothing is selected.
                window.show_panel_menu();
                return glib::Propagation::Stop;
            };
            tree.selection().select_path(&path);
            let id = window
                .imp()
                .connections_panel
                .borrow()
                .as_ref()
                .and_then(|panel| panel.id_at(&path));
            match id {
                Some(id) => window.show_connection_menu(&id),
                None => window.show_group_menu(&path),
            }
            glib::Propagation::Stop
        });
        slot.pack_start(&panel.root, true, true, 0);
        slot.show();
        // The slot was hidden while the panel did not exist, so the splitter's
        // position was clamped. Restore the stored width now that it has a
        // child, and save whatever width the user drags to afterwards.
        if let Some(pane) = slot.parent().and_then(|p| p.downcast::<gtk::Paned>().ok()) {
            let width = settings()
                .get_i64("connections_panel_width")
                .clamp(180, 1200) as i32;
            self.imp().connections_positioning.set(true);
            pane.set_position(width);
            self.imp().connections_positioning.set(false);
            let weak = crate::SendWeak::new(self);
            pane.connect_notify_local(Some("position"), move |pane, _| {
                if let Some(window) = weak.upgrade() {
                    window.remember_connections_panel_width(pane);
                }
            });
        }
        *self.imp().connections_panel.borrow_mut() = Some(panel);
        true
    }

    fn remember_connections_panel_width(&self, pane: &gtk::Paned) {
        if !self.connections_panel_visible() || self.imp().connections_positioning.get() {
            return;
        }
        let width = (pane.position() as i64).clamp(180, 1200);
        let generation = self
            .imp()
            .connections_width_generation
            .get()
            .wrapping_add(1);
        self.imp().connections_width_generation.set(generation);
        let weak = self.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if window.imp().connections_width_generation.get() != generation {
                return;
            }
            settings().set_i64("connections_panel_width", width);
        });
    }

    /// Shortcuts that work from anywhere in the panel. `Ctrl+Shift+J` is normally
    /// handled by the terminal, which does not see keys while the panel has focus.
    fn panel_shortcut(&self, key: gdk::keys::Key, state: gdk::ModifierType) -> bool {
        use gdk::keys::constants as K;
        if !state.contains(gdk::ModifierType::CONTROL_MASK) {
            return false;
        }
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        if shift && (key == K::j || key == K::J) {
            self.set_connections_panel_visible(false);
            return true;
        }
        if !shift && key == K::n {
            self.new_connection();
            return true;
        }
        false
    }

    pub fn connections_panel_visible(&self) -> bool {
        self.imp()
            .connections_panel
            .borrow()
            .as_ref()
            .map(|panel| panel.root.is_visible())
            .unwrap_or(false)
    }

    pub fn set_connections_panel_visible(&self, visible: bool) {
        if visible && !self.ensure_connections_panel() {
            return;
        }
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        if visible {
            // See `set_workspace_visible` for why `no_show_all` is toggled.
            if let Some(slot) = self.imp().connections_slot.borrow().clone() {
                slot.show();
            }
            panel.root.set_no_show_all(false);
            panel.root.show_all();
            panel.root.set_no_show_all(true);
            self.reload_connections_panel();
            self.start_reachability_worker();
            panel.search.grab_focus();
        } else {
            panel.stop_reachability_timer();
            panel.root.hide();
            // A hidden slot also hides the splitter's handle and gives the room
            // back to the terminals.
            if let Some(slot) = self.imp().connections_slot.borrow().clone() {
                slot.hide();
            }
            if let Some(term) = self.current_terminal() {
                term.vte().grab_focus();
            }
        }
    }

    pub fn toggle_connections_panel(&self) {
        self.set_connections_panel_visible(!self.connections_panel_visible());
    }

    /// Rebuild the rows from disk. `~/.ssh/config` may have changed behind our
    /// back, and another Oxterm may have edited the book.
    pub fn reload_connections_panel(&self) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        if !panel.root.is_visible() {
            return;
        }
        let (book, status) = match connections::load_book(&connections::book_path()) {
            Ok(book) => {
                let status = book.read_only().then(|| {
                    "connections.json comes from a newer Oxterm and is read-only.".to_string()
                });
                (book, status)
            }
            Err(error) => {
                // Hosts from ~/.ssh/config still work; editing is refused by
                // `update_book`, so the damaged file is not overwritten.
                (
                    connections::Book::default(),
                    Some(format!("Could not read the book: {}", error)),
                )
            }
        };
        let entries = connections::merge(&book, &ssh_config::load_hosts());
        let usage: UsageMap = connections::load_usage(&connections::usage_path());
        *panel.data.borrow_mut() = Some(PanelData {
            entries,
            usage,
            status,
        });
        self.refilter_connections_panel();
    }

    // ── Reachability ─────────────────────────────────────────

    /// Probe immediately, then every minute while the panel is visible. A
    /// timer on the main thread snapshots the rows; the actual TCP work runs
    /// on a worker, so DNS and connect timeouts never block the UI.
    fn start_reachability_worker(&self) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        if panel.reach_timer.borrow().is_some() {
            return;
        }
        panel.reach_stop.store(false, Ordering::SeqCst);
        self.probe_reachability_once();
        let weak = crate::SendWeak::new(self);
        let source = glib::timeout_add_seconds_local(60, move || {
            let Some(window) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            window.probe_reachability_once();
            glib::ControlFlow::Continue
        });
        *panel.reach_timer.borrow_mut() = Some(source);
    }

    fn probe_reachability_once(&self) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        if !panel.root.is_visible() || panel.reach_busy.replace(true) {
            return;
        }
        // One probe per unique host: a host can appear in several groups, and
        // the book may list both an alias and its address.
        let mut targets: HashMap<(String, u16), Vec<String>> = HashMap::new();
        {
            let data = panel.data.borrow();
            if let Some(data) = data.as_ref() {
                for entry in &data.entries {
                    let (host, port) = reachability::connection_target(&entry.connection);
                    if host.is_empty() {
                        continue;
                    }
                    targets
                        .entry((host, port))
                        .or_default()
                        .push(entry.connection.id.clone());
                }
            }
        }
        if targets.is_empty() {
            panel.reach_busy.set(false);
            return;
        }
        let stop = panel.reach_stop.clone();
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let mut results = Vec::new();
            for ((host, port), ids) in targets {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let probe = reachability::probe_tcp(&host, port, 2000);
                for id in ids {
                    results.push((id, probe.clone()));
                }
            }
            if stop.load(Ordering::SeqCst) {
                return;
            }
            // `_local` would try to acquire the default main context on this
            // worker thread and panic; `run_on_main` posts from any thread.
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    window.finish_reachability_probe(results);
                }
            });
        });
    }

    fn finish_reachability_probe(&self, results: Vec<(String, reachability::Reachability)>) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        panel.reach_busy.set(false);
        if panel.reach_stop.load(Ordering::SeqCst) {
            return;
        }
        panel.reach.borrow_mut().extend(results);
        self.refilter_connections_panel();
    }

    /// Redraw the rows from what `reload_connections_panel` last read.
    fn refilter_connections_panel(&self) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        let data = panel.data.borrow();
        let Some(PanelData {
            entries,
            usage,
            status,
        }) = data.as_ref()
        else {
            return;
        };
        let query = panel.search.text().to_string();
        let reach = panel.reach.borrow();
        let open = self.open_connection_counts();
        let patterns = settings().get_str("ssh_alert_patterns");
        let alert = settings().get_str("ssh_alert_color");
        let states = tunnels::snapshot();
        let decorations: std::collections::HashMap<String, RowDecoration> = entries
            .iter()
            .map(|entry| {
                let connection = &entry.connection;
                let mut decoration = RowDecoration {
                    open_tabs: open.get(&connection.id).copied().unwrap_or(0),
                    reach: reach.get(&connection.id).cloned(),
                    ..RowDecoration::default()
                };
                let mut count = |status: &tunnels::Status, label: String| {
                    match status {
                        tunnels::Status::Starting => decoration.starting_tunnels += 1,
                        tunnels::Status::Up | tunnels::Status::Running => {
                            decoration.active_tunnels += 1
                        }
                        tunnels::Status::Failed(_) => decoration.failed_tunnels += 1,
                        tunnels::Status::Stopped => {}
                    }
                    decoration
                        .tunnel_lines
                        .push(format!("{}: {}", label, status.label()));
                };
                if let Some(status) = states.get(&desktop_key(connection)) {
                    if *status != tunnels::Status::Stopped {
                        count(status, "desktop".to_string());
                    }
                }
                for tunnel in &connection.tunnels {
                    let status = states
                        .get(&tunnel_key(connection, tunnel))
                        .cloned()
                        .unwrap_or(tunnels::Status::Stopped);
                    count(&status, tunnel.describe());
                }
                if ssh_config::alert_host(&patterns, connection.destination())
                    || ssh_config::alert_host(&patterns, &connection.name)
                {
                    decoration.alert_color = Some(alert.clone());
                }
                (connection.id.clone(), decoration)
            })
            .collect();
        let empty_decoration = RowDecoration::default();
        let decorate = |entry: &Entry| -> &RowDecoration {
            decorations
                .get(&entry.connection.id)
                .unwrap_or(&empty_decoration)
        };
        let live_only = panel.live_only.is_active();
        let reachable_only = panel.reachable_only.is_active();
        let filtered: Vec<Entry>;
        let shown: &[Entry] = if live_only || reachable_only {
            filtered = entries
                .iter()
                .filter(|entry| {
                    (!live_only || decorate(entry).is_live())
                        && (!reachable_only
                            || reach
                                .get(&entry.connection.id)
                                .is_some_and(|probe| probe.state == ProbeState::Online))
                })
                .cloned()
                .collect();
            &filtered
        } else {
            entries
        };
        let tree = connections::build_tree(shown, usage, &query);
        panel.populate(&tree, shown, &decorate);
        let connected = decorations.values().filter(|d| d.open_tabs > 0).count();
        let up: usize = decorations.values().map(|d| d.active_tunnels).sum();
        let starting: usize = decorations.values().map(|d| d.starting_tunnels).sum();
        let failed: usize = decorations.values().map(|d| d.failed_tunnels).sum();
        let mut summary = Vec::new();
        if connected > 0 {
            summary.push(format!("{} connected", connected));
        }
        if up > 0 {
            summary.push(format!(
                "{} tunnel{} up",
                up,
                if up == 1 { "" } else { "s" }
            ));
        }
        if starting > 0 {
            summary.push(format!("{} starting", starting));
        }
        if failed > 0 {
            summary.push(format!("{} failed", failed));
        }
        let summary = summary.join(" \u{00B7} ");
        let empty_hint = if entries.is_empty() && query.is_empty() {
            Some("No connections yet. Click + to add one. Hosts in ~/.ssh/config appear here by themselves.")
        } else if live_only && reachable_only && shown.is_empty() {
            Some("No open and reachable connections.")
        } else if reachable_only && shown.is_empty() {
            Some("No reachable connections yet. Hosts are probed every minute.")
        } else if live_only && shown.is_empty() {
            Some("No open connections or tunnels.")
        } else {
            None
        };
        let text = status.as_deref().or(empty_hint).unwrap_or(summary.as_str());
        panel.status.set_text(text);
        panel.status.set_visible(!text.is_empty());
    }

    fn open_connection_counts(&self) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        // Every window of this process: a detached tab still counts.
        let terminals = match self.application() {
            Some(app) => crate::control::terminals(&app)
                .into_iter()
                .map(|(_, term)| term)
                .collect(),
            None => self.all_terminals(),
        };
        for term in terminals {
            if let Some(id) = term.connection_id() {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Targets for the Monitor dropdown as `(target, label, is_active)`.
    /// SSH connections with an open tab come first, the others keep book order.
    pub(crate) fn monitor_connection_choices(&self) -> Vec<(String, String, bool)> {
        let active_ids = self.open_connection_counts();
        let terminals: Vec<TerminalBox> = match self.application() {
            Some(app) => crate::control::terminals(&app)
                .into_iter()
                .map(|(_, term)| term)
                .collect(),
            None => self.all_terminals(),
        };
        let live: Vec<String> = terminals
            .iter()
            .filter_map(|term| term.ssh_destination())
            .collect();

        let mut choices = Vec::new();
        let mut seen = HashSet::new();
        for entry in load_entries().unwrap_or_default() {
            if entry.connection.protocol != Protocol::Ssh {
                continue;
            }
            // An alias is a real ssh destination; `summary()` would only say
            // "~/.ssh/config" and could not be used by the monitor probe.
            let target = if entry.connection.kind == Kind::Alias {
                entry.connection.name.clone()
            } else {
                entry.connection.summary()
            };
            if target.trim().is_empty() || !seen.insert(target.clone()) {
                continue;
            }
            let label = if entry.connection.name.is_empty() || entry.connection.name == target {
                target.clone()
            } else {
                format!("{} ({target})", entry.connection.name)
            };
            let active = active_ids.contains_key(&entry.connection.id)
                || live
                    .iter()
                    .any(|value| crate::headless::monitor::same_ssh_host(value, &target));
            choices.push((target, label, active));
        }
        // Ad-hoc ssh sessions are not in the book but are active right now.
        for target in live {
            if seen.insert(target.clone()) {
                choices.push((target.clone(), target, true));
            }
        }
        crate::headless::monitor::order_connection_choices(choices)
    }

    /// Close the SSH session shown by the Monitor panel, using the same path
    /// as the tab's X button so the remote process is really terminated.
    /// An empty target means localhost: there is no session to close.
    pub(crate) fn close_monitored_connection(&self, target: &str) {
        if target.trim().is_empty() {
            return;
        }
        let entries = load_entries().unwrap_or_default();
        let terminals: Vec<(Option<MainWindow>, TerminalBox)> = match self.application() {
            Some(app) => crate::control::terminals(&app),
            None => self
                .all_terminals()
                .into_iter()
                .map(|term| (Some(self.clone()), term))
                .collect(),
        };

        // Prefer an exact destination match: a host-only match could close a
        // different user's session on the same machine.
        for exact in [true, false] {
            for (window, term) in &terminals {
                let matches = term_matches_target(&term, target, &entries, exact);
                if !matches {
                    continue;
                }
                match window {
                    Some(window) => window.close_tab_signal(Some(term)),
                    // A detached window owns no MainWindow: terminating the
                    // terminal still kills its shell, as closing its tab does.
                    None => term.terminate(),
                }
                return;
            }
        }
    }

    fn connect_first_result(&self) {
        let id = self
            .imp()
            .connections_panel
            .borrow()
            .as_ref()
            .and_then(|panel| panel.select_first_connection());
        if let Some(id) = id {
            self.connect_id(&id, Placement::Tab);
        }
    }

    // ── Workspaces ───────────────────────────────────────────

    /// The panel's background menu: both workspace actions work on the current
    /// selection, so they belong where the tree has no row of its own.
    fn show_panel_menu(&self) {
        let menu = gtk::Menu::new();
        self.append_workspace_items(&menu);
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    fn append_workspace_items(&self, menu: &gtk::Menu) {
        if !menu.children().is_empty() {
            menu.append(&gtk::SeparatorMenuItem::new());
        }
        let add = |menu: &gtk::Menu, label: &str, action: fn(&MainWindow)| {
            let item = gtk::MenuItem::with_label(label);
            let weak = crate::SendWeak::new(self);
            item.connect_activate(move |_| {
                if let Some(window) = weak.upgrade() {
                    action(&window);
                }
            });
            menu.append(&item);
        };
        add(menu, "Save Workspace...", MainWindow::save_workspace);
        add(menu, "Load Workspace...", MainWindow::load_workspace);
    }

    /// Save the selected connections (or all of them) under a name. The name
    /// is validated by `workspaces::upsert`, so no extra parsing is needed.
    fn save_workspace(&self) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        let selected = panel.selected_ids();
        let entries: Vec<WorkspaceEntry> = {
            let data = panel.data.borrow();
            let Some(data) = data.as_ref() else {
                self.status_message("The connection list is not loaded yet", true);
                return;
            };
            let mut seen = HashSet::new();
            data.entries
                .iter()
                .filter(|entry| selected.is_empty() || selected.contains(&entry.connection.id))
                .filter(|entry| seen.insert(entry.connection.id.clone()))
                .map(|entry| WorkspaceEntry {
                    label: entry.connection.name.clone(),
                    kind: "connection".to_string(),
                    target: entry.connection.id.clone(),
                })
                .collect()
        };
        if entries.is_empty() {
            self.status_message("No connections to save", true);
            return;
        }
        let dialog = gtk::Dialog::with_buttons(
            Some("Save Workspace"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Save", gtk::ResponseType::Ok),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Ok);
        let area = dialog.content_area();
        let label = gtk::Label::new(Some(&format!(
            "Name for these {} connection(s):",
            entries.len()
        )));
        label.set_xalign(0.0);
        area.pack_start(&label, false, false, 4);
        let entry = gtk::Entry::new();
        entry.set_activates_default(true);
        area.pack_start(&entry, false, false, 4);
        dialog.show_all();
        entry.grab_focus();
        let answer = dialog.run();
        let name = entry.text().to_string();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        if answer != gtk::ResponseType::Ok {
            return;
        }
        let Some(path) = workspaces::default_path() else {
            self.status_message("Cannot locate the Oxterm configuration directory", true);
            return;
        };
        let mut book = match workspaces::load(&path) {
            Ok(book) => book,
            Err(error) => {
                self.status_message(&format!("Cannot read workspaces: {}", error), true);
                return;
            }
        };
        if let Err(error) = workspaces::upsert(&mut book, &name, entries) {
            self.status_message(&error, true);
            return;
        }
        match workspaces::save(&path, &book) {
            Ok(()) => self.status_message(&format!("Workspace '{}' saved", name), false),
            Err(error) => self.status_message(&format!("Cannot save workspace: {}", error), true),
        }
    }

    fn load_workspace(&self) {
        let Some(path) = workspaces::default_path() else {
            self.status_message("Cannot locate the Oxterm configuration directory", true);
            return;
        };
        let book = match workspaces::load(&path) {
            Ok(book) => book,
            Err(error) => {
                self.status_message(&format!("Cannot read workspaces: {}", error), true);
                return;
            }
        };
        if book.workspaces.is_empty() {
            self.status_message("No saved workspaces yet", false);
            return;
        }
        let menu = gtk::Menu::new();
        for workspace in book.workspaces {
            let label = format!("{} ({} tabs)", workspace.name, workspace.entries.len());
            let item = gtk::MenuItem::with_label(&label);
            item.set_tooltip_text(Some(
                &workspace
                    .entries
                    .iter()
                    .map(|e| e.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
            let entries = workspace.entries;
            let weak = crate::SendWeak::new(self);
            item.connect_activate(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.open_workspace_entries(entries.clone());
                }
            });
            menu.append(&item);
        }
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    /// Open each saved entry in its own tab, a quarter second apart. The delay
    /// is scheduled on the main loop: a modal confirmation in one tab must not
    /// keep the others from opening later.
    pub(crate) fn open_workspace_entries(&self, entries: Vec<WorkspaceEntry>) {
        Self::schedule_workspace_entry(crate::SendWeak::new(self), entries, 0);
    }

    fn schedule_workspace_entry(
        weak: crate::SendWeak<MainWindow>,
        entries: Vec<WorkspaceEntry>,
        index: usize,
    ) {
        let Some(entry) = entries.get(index) else {
            return;
        };
        if let Some(window) = weak.upgrade() {
            match entry.kind.as_str() {
                "connection" => match Self::connection_for_id(&entry.target) {
                    Ok(connection) => window.connect(connection, Placement::Tab),
                    Err(error) => {
                        window.status_message(&format!("{}: {}", entry.label, error), true)
                    }
                },
                "local" => {
                    if Path::new(&entry.target).is_dir() {
                        window.add_new_tab(
                            Some(&entry.target),
                            None,
                            Some(&entry.label),
                            Some(&entry.label),
                            None,
                        );
                    } else {
                        window.status_message(
                            &format!("Workspace folder not found: {}", entry.target),
                            true,
                        );
                    }
                }
                other => {
                    window.status_message(&format!("Unknown workspace entry type: {}", other), true)
                }
            }
        }
        if index + 1 < entries.len() {
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                Self::schedule_workspace_entry(weak, entries, index + 1);
            });
        }
    }

    fn show_group_menu(&self, path: &gtk::TreePath) {
        let panel = self.imp().connections_panel.borrow().clone();
        let Some(panel) = panel else {
            return;
        };
        let ids = panel.ids_under(path);
        if ids.is_empty() {
            return;
        }
        let menu = gtk::Menu::new();
        let item = gtk::MenuItem::with_label(&format!("Open All ({})", ids.len()));
        let weak = crate::SendWeak::new(self);
        item.connect_activate(move |_| {
            if let Some(window) = weak.upgrade() {
                for id in &ids {
                    window.connect_id(id, Placement::Tab);
                }
            }
        });
        menu.append(&item);
        self.append_workspace_items(&menu);
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    fn show_connection_menu(&self, id: &str) {
        let cached = self
            .imp()
            .connections_panel
            .borrow()
            .as_ref()
            .and_then(|panel| {
                panel
                    .data
                    .borrow()
                    .as_ref()
                    .and_then(|data| connections::find_entry_by_id(&data.entries, id).cloned())
            });
        let Some(mut entry) = cached.or_else(|| {
            let entries = load_entries().ok()?;
            connections::find_entry_by_id(&entries, id).cloned()
        }) else {
            return;
        };
        // Menu actions open the connection too: inherit template fields before
        // building command previews, browser arguments and tunnel keys.
        if let Ok(resolved) = Self::connection_for_id(id) {
            entry.connection = resolved;
        }
        let menu = gtk::Menu::new();
        let add = |label: &str, action: Box<dyn Fn(&MainWindow)>| {
            let item = gtk::MenuItem::with_label(label);
            let weak = crate::SendWeak::new(self);
            item.connect_activate(move |_| {
                if let Some(window) = weak.upgrade() {
                    action(&window);
                }
            });
            menu.append(&item);
        };
        for (label, placement) in [
            ("Open in New Tab", Placement::Tab),
            ("Open in Split", Placement::Split),
            ("Open in New Window", Placement::Window),
        ] {
            let id = id.to_string();
            add(
                label,
                Box::new(move |window| window.connect_id(&id, placement)),
            );
        }
        let protocol = entry.connection.protocol;
        if protocol == Protocol::Ssh || protocol.is_file_transfer() {
            let connection = entry.connection.clone();
            add(
                "Browse Files...",
                Box::new(move |window| window.open_file_browser(connection.clone())),
            );
            let id = id.to_string();
            add(
                if protocol == Protocol::Ftp {
                    "Open FTP Prompt"
                } else {
                    "Open SFTP Prompt"
                },
                Box::new(move |window| window.open_sftp(&id)),
            );
        }
        if protocol.is_desktop() && tunnels::status(&desktop_key(&entry.connection)).is_active() {
            let key = desktop_key(&entry.connection);
            add(
                "Disconnect",
                Box::new(move |window| {
                    tunnels::stop(&key);
                    window.watch_tunnels();
                }),
            );
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        if protocol == Protocol::Ssh || !entry.connection.tunnels.is_empty() {
            let submenu = gtk::Menu::new();
            for tunnel in &entry.connection.tunnels {
                let key = tunnel_key(&entry.connection, tunnel);
                let status = tunnels::status(&key);
                let active = status.is_active();
                let label = format!(
                    "{} {} ({})",
                    if active { "Stop" } else { "Start" },
                    tunnel.describe(),
                    status.label()
                );
                let item = gtk::MenuItem::with_label(&label);
                let connection = entry.connection.clone();
                let tunnel = tunnel.clone();
                let weak = crate::SendWeak::new(self);
                item.connect_activate(move |_| {
                    if let Some(window) = weak.upgrade() {
                        if active {
                            tunnels::stop(&tunnel_key(&connection, &tunnel));
                        } else {
                            window.start_tunnel(&connection, &tunnel);
                        }
                        window.watch_tunnels();
                    }
                });
                submenu.append(&item);
            }
            if !entry.connection.tunnels.is_empty() {
                submenu.append(&gtk::SeparatorMenuItem::new());
            }
            if protocol == Protocol::Ssh {
                let item = gtk::MenuItem::with_label("Add Tunnel...");
                let id = id.to_string();
                let weak = crate::SendWeak::new(self);
                item.connect_activate(move |_| {
                    if let Some(window) = weak.upgrade() {
                        window.show_tunnel_manager(None);
                        crate::tunnel_manager::add_tunnel_for(&window, &id);
                    }
                });
                submenu.append(&item);
            }
            let item = gtk::MenuItem::with_label("Tunnel Manager...");
            let id = id.to_string();
            let weak = crate::SendWeak::new(self);
            item.connect_activate(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.show_tunnel_manager(Some(&id));
                }
            });
            submenu.append(&item);
            let tunnels_item = gtk::MenuItem::with_label("Tunnels");
            tunnels_item.set_submenu(Some(&submenu));
            menu.append(&tunnels_item);
        }
        if protocol == Protocol::Ssh {
            let connection = entry.connection.clone();
            add(
                "SSH Details...",
                Box::new(move |window| window.show_ssh_details(connection.clone())),
            );
            let connection = entry.connection.clone();
            add(
                "Copy Public Key to Server",
                Box::new(move |window| window.copy_key_to_server(&connection)),
            );
        }
        {
            let command = entry.connection.command_preview();
            add(
                "Copy ssh Command",
                Box::new(move |_| {
                    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&command);
                }),
            );
        }
        if entry.connection.uses_keyring() {
            let id = id.to_string();
            add(
                "Forget Stored Password",
                Box::new(move |window| window.forget_stored_password(&id)),
            );
        }
        {
            let id = id.to_string();
            let label = if entry.connection.favorite {
                "Remove from Favorites"
            } else {
                "Add to Favorites"
            };
            add(label, Box::new(move |window| window.toggle_favorite(&id)));
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        {
            let id = id.to_string();
            add(
                "Edit...",
                Box::new(move |window| window.edit_connection(&id)),
            );
        }
        if entry.connection.kind == Kind::Manual {
            let id = id.to_string();
            add(
                "Duplicate...",
                Box::new(move |window| window.duplicate_connection(&id)),
            );
        }
        // A plain ~/.ssh/config host has nothing of ours to delete.
        if entry.origin != Origin::SshConfig {
            let id = id.to_string();
            let label = if entry.origin == Origin::Annotated {
                "Remove Oxterm Metadata"
            } else {
                "Delete"
            };
            add(label, Box::new(move |window| window.delete_connection(&id)));
        }
        self.append_workspace_items(&menu);
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    // ── Editing ──────────────────────────────────────────────

    pub fn new_connection(&self) {
        self.run_form("New Connection", Connection::new_manual());
    }

    pub(crate) fn edit_connection(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            self.status_message("Could not read the connection book", true);
            return;
        };
        match connections::find_entry_by_id(&entries, id) {
            Some(entry) => self.run_form("Edit Connection", entry.connection.clone()),
            None => self.status_message("That connection no longer exists", true),
        }
    }

    fn duplicate_connection(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        if let Some(entry) = connections::find_entry_by_id(&entries, id) {
            let mut copy = entry.connection.clone();
            copy.id = connections::new_id();
            copy.name = format!("{} copy", copy.name);
            // The secrets belong to the original's keyring entries.
            copy.auth = Auth::Default;
            copy.totp = false;
            self.run_form("Duplicate Connection", copy);
        }
    }

    fn run_form(&self, title: &str, initial: Connection) {
        let id = initial.id.clone();
        let was_secret = initial.auth == Auth::SecretService;
        let had_totp = initial.totp;
        let available = askpass::keyring_available();
        let check = |connection: &Connection| -> Result<(), String> {
            let book = connections::load_book(&connections::book_path())?;
            if book.read_only() {
                return Err("connections.json comes from a newer Oxterm".to_string());
            }
            let mut probe = book;
            probe.upsert(connection.clone())
        };
        let window: gtk::Window = self.clone().upcast();
        let Some(result) =
            crate::connections_form::edit(&window, title, initial, available, &check)
        else {
            return;
        };
        let connection = result.connection;
        let saved = connections::update_book(&connections::book_path(), |book| {
            book.upsert(connection.clone())
        });
        if let Err(error) = saved {
            self.status_message(&format!("Could not save: {}", error), true);
            return;
        }
        // Keyring tools can wait on an unlock prompt: never on the GUI thread.
        let label = connection.name.clone();
        let wants_secret = connection.auth == Auth::SecretService;
        let wants_totp = connection.totp;
        let password = result.password;
        let totp_secret = result.totp_secret;
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let mut errors = Vec::new();
            match password {
                Some(password) => {
                    errors.extend(askpass::store_secret(&id, &label, &password).err())
                }
                None if was_secret && !wants_secret => askpass::clear_secret(&id),
                None => {}
            }
            match totp_secret {
                Some(secret) => {
                    errors.extend(askpass::store_totp_secret(&id, &label, &secret).err())
                }
                None if had_totp && !wants_totp => askpass::clear_totp_secret(&id),
                None => {}
            }
            if !errors.is_empty() {
                crate::run_on_main(move || {
                    if let Some(window) = weak.upgrade() {
                        window.status_message(&errors.join("; "), true);
                    }
                });
            }
        });
        self.reload_connections_panel();
    }

    fn toggle_favorite(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id) else {
            return;
        };
        let mut connection = entry.connection.clone();
        connection.favorite = !connection.favorite;
        match connections::update_book(&connections::book_path(), |book| {
            book.upsert(connection.clone())
        }) {
            Ok(()) => self.reload_connections_panel(),
            Err(error) => self.status_message(&format!("Could not save: {}", error), true),
        }
    }

    fn delete_connection(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id) else {
            return;
        };
        if entry.origin == Origin::SshConfig {
            return;
        }
        let dialog = gtk::MessageDialog::new(
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::YesNo,
            &format!(
                "Delete '{}'? Its saved tunnels and stored password are removed too.",
                entry.connection.name
            ),
        );
        let answer = dialog.run();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        if answer != gtk::ResponseType::Yes {
            return;
        }
        let connection = entry.connection.clone();
        let removed = connections::update_book(&connections::book_path(), |book| {
            Ok(book.remove(&connection.id))
        });
        match removed {
            Ok(_) => {
                for tunnel in &connection.tunnels {
                    tunnels::stop(&tunnel_key(&connection, tunnel));
                }
                if connection.uses_keyring() {
                    let id = connection.id.clone();
                    std::thread::spawn(move || {
                        askpass::clear_secret(&id);
                        askpass::clear_totp_secret(&id);
                    });
                }
                self.reload_connections_panel();
            }
            Err(error) => self.status_message(&format!("Could not save: {}", error), true),
        }
    }

    /// Forget the keyring entries of a connection and stop asking for them.
    /// The book keeps its identity, tunnels and metadata.
    pub(crate) fn forget_stored_password(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id) else {
            return;
        };
        if !entry.connection.uses_keyring() {
            return;
        }
        let dialog = gtk::MessageDialog::new(
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::YesNo,
            &format!(
                "Forget the stored password and one-time code for '{}'?",
                entry.connection.name
            ),
        );
        let answer = dialog.run();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        if answer != gtk::ResponseType::Yes {
            return;
        }
        let mut connection = entry.connection.clone();
        connection.auth = Auth::Default;
        connection.totp = false;
        let label = connection.name.clone();
        match connections::update_book(&connections::book_path(), |book| {
            book.upsert(connection.clone())
        }) {
            Ok(()) => {
                let id = connection.id.clone();
                std::thread::spawn(move || {
                    askpass::clear_secret(&id);
                    askpass::clear_totp_secret(&id);
                });
                self.status_message(&format!("Stored password forgotten for {}", label), false);
                self.reload_connections_panel();
            }
            Err(error) => self.status_message(&format!("Could not save: {}", error), true),
        }
    }

    /// Show what OpenSSH will actually use for this entry. `ssh -G` can block on
    /// `Match exec` rules, so it runs on a worker and the dialog opens after.
    pub fn show_ssh_details(&self, connection: Connection) {
        let title = connection.name.clone();
        self.status_message(
            &format!("Reading SSH configuration for {}...", title),
            false,
        );
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let effective =
                ssh_config::effective_config(&connection.target_args(), connections::PROBE_TIMEOUT);
            let command = connection.command_preview();
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    window.present_ssh_details(&connection, &command, effective.as_deref());
                }
            });
        });
    }

    fn present_ssh_details(&self, connection: &Connection, command: &str, effective: Option<&str>) {
        let text = ssh_details_text(connection, command, effective);
        let dialog = gtk::Dialog::with_buttons(
            Some(&format!("SSH Details: {}", connection.name)),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Copy", gtk::ResponseType::Apply),
                ("Close", gtk::ResponseType::Close),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Close);
        dialog.set_default_size(760, 560);
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        if let Some(buffer) = view.buffer() {
            buffer.set_text(&text);
        }
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_shadow_type(gtk::ShadowType::In);
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.add(&view);
        dialog.content_area().pack_start(&scroll, true, true, 0);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Apply {
            gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&text);
        }
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
    }

    // ── Connecting ───────────────────────────────────────────

    /// Look up a connection with its template chain resolved. Hosts from
    /// `~/.ssh/config` are not in our book, so they still come from the merged
    /// entries.
    fn connection_for_id(id: &str) -> Result<Connection, String> {
        if let Ok(book) = connections::load_book(&connections::book_path()) {
            if let Ok(connection) = book.resolve_connection(id) {
                return Ok(connection);
            }
        }
        let entries = load_entries()?;
        connections::find_entry_by_id(&entries, id)
            .map(|entry| entry.connection.clone())
            .ok_or_else(|| "That connection no longer exists".to_string())
    }

    pub fn connect_id(&self, id: &str, placement: Placement) {
        match Self::connection_for_id(id) {
            Ok(connection) => self.connect(connection, placement),
            Err(error) => self.status_message(&error, true),
        }
    }

    /// Open what the host picker returned: a saved name, or a typed destination
    /// that goes to `ssh` as it is.
    pub fn connect_text(&self, text: &str) {
        let entries = load_entries().unwrap_or_else(|error| {
            LOGGER.warning(&format!("connections_book_unreadable error={}", error));
            Vec::new()
        });
        let connection = Self::connection_for_id(text)
            .ok()
            .or_else(|| {
                connections::find_entry(&entries, text)
                    .or_else(|| connections::find_entry_by_id(&entries, text))
                    .map(|entry| entry.connection.clone())
            })
            .unwrap_or_else(|| Connection::alias(text));
        self.connect(connection, Placement::Tab);
    }

    /// Open several connections one after another, leaving a moment between
    /// tabs. The delay runs on the main loop, never as `thread::sleep`.
    fn connect_ids_staggered(&self, ids: Vec<String>, placement: Placement) {
        Self::schedule_connect_ids(crate::SendWeak::new(self), ids.into(), placement);
    }

    fn schedule_connect_ids(
        weak: crate::SendWeak<MainWindow>,
        mut queue: VecDeque<String>,
        placement: Placement,
    ) {
        let Some(id) = queue.pop_front() else {
            return;
        };
        if let Some(window) = weak.upgrade() {
            window.connect_id(&id, placement);
        }
        if !queue.is_empty() {
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                Self::schedule_connect_ids(weak, queue, placement);
            });
        }
    }

    pub fn connect(&self, connection: Connection, placement: Placement) {
        if let Err(error) = connection.validate() {
            self.status_message(&error, true);
            return;
        }
        if connection.confirm_connect && !self.confirm_connection(&connection) {
            return;
        }
        if connection.protocol.is_file_transfer() {
            self.open_file_browser(connection);
            return;
        }
        if connection.protocol.is_desktop() {
            self.start_desktop(connection);
            return;
        }
        let program = connection
            .terminal_argv()
            .and_then(|argv| argv.first().cloned())
            .unwrap_or_default();
        if crate::notes::which(&program).is_none() {
            self.status_message(
                &format!("Cannot connect: {} is not installed", program),
                true,
            );
            return;
        }
        if preconnect::has_work(&connection) {
            self.status_message(&format!("Preparing {}...", connection.name), false);
        }
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let problems = preconnect::run(&connection);
            let prepared = connections::prepare_session(&connection);
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    for problem in &problems {
                        window.status_message(problem, true);
                    }
                    window.finish_connect(connection, prepared, placement);
                }
            });
        });
    }

    pub(crate) fn confirm_connection(&self, connection: &Connection) -> bool {
        let dialog = gtk::MessageDialog::new(
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Warning,
            gtk::ButtonsType::YesNo,
            &format!(
                "Connect to '{}'?\n\n{}",
                connection.name,
                connection.command_preview()
            ),
        );
        let answer = dialog.run();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        answer == gtk::ResponseType::Yes
    }

    pub(crate) fn finish_connect(
        &self,
        connection: Connection,
        prepared: Prepared,
        placement: Placement,
    ) {
        let title = connection.name.clone();
        let term = self.open_connection_terminal(&prepared.plan.argv, &title, placement);
        let Some(term) = term else {
            return;
        };
        self.attach_connection(&term, &connection, placement != Placement::Window);
        // Writes to disk with an fsync; not worth a stall on the GUI thread.
        let id = connection.id.clone();
        std::thread::spawn(move || {
            if let Err(error) = connections::record_usage(&connections::usage_path(), &id) {
                LOGGER.warning(&format!("connection_usage_failed error={}", error));
            }
        });
        let mut started = false;
        for tunnel in connection
            .tunnels
            .iter()
            .filter(|tunnel| tunnel.autostart && connection.protocol == Protocol::Ssh)
        {
            self.start_tunnel(&connection, tunnel);
            started = true;
        }
        if started {
            self.watch_tunnels();
        }
        if let Some(note) = prepared.keyring_note {
            self.status_message(note, true);
        } else if let Some(note) = prepared.probe_note {
            self.status_message(note, true);
        } else if prepared.plan.forwards_agent {
            self.status_message(
                "Agent forwarding is enabled for this host; prefer ProxyJump instead",
                true,
            );
        }
        self.reload_connections_panel();
    }

    /// Everything a saved connection means for its terminal.
    pub fn attach_connection(&self, term: &TerminalBox, connection: &Connection, pin_title: bool) {
        term.set_connection(Some(connection.id.clone()), Some(connection.name.clone()));
        term.set_color_override(Some(connection.color_scheme.clone()));
        term.set_session_overrides(
            Some(connection.font.clone()),
            connection.scrollback,
            Some(connection.encoding.clone()),
        );
        term.set_connection_macros(connection.macros.clone());
        term.set_expect_rules(&connection.expect);
        if connection.auto_reconnect {
            term.enable_reconnect(
                connection
                    .reconnect_delay
                    .unwrap_or(DEFAULT_RECONNECT_DELAY),
                self.imp().pending_reconnect_attempts.take(),
            );
        }
        if connection.log_output {
            if let Err(error) = term.start_session_log(&connection.log_dir, &connection.name) {
                self.status_message(&format!("Session log: {}", error), true);
            }
        }
        if pin_title {
            self.pin_tab_title(term, &connection.name);
        }
    }

    /// The same, for a terminal opened through the control API.
    pub fn attach_connection_id(&self, term: &TerminalBox, id: &str) {
        match Self::connection_for_id(id) {
            Ok(connection) => self.attach_connection(term, &connection, true),
            Err(_) => term.set_connection(Some(id.to_string()), None),
        }
    }

    /// Replace a terminal whose connection dropped with a new one in the same
    /// place. A new tab rather than a respawn: everything that opens a connection
    /// (options, stored password, log) then runs the one tested way.
    pub fn reconnect_terminal(&self, old: &TerminalBox) {
        let Some(id) = old.connection_id() else {
            return;
        };
        let Some((notebook, position)) = self.find_terminal(old) else {
            return;
        };
        let Ok(connection) = Self::connection_for_id(&id) else {
            return;
        };
        let attempts = old.reconnect_attempts();
        let weak = crate::SendWeak::new(self);
        let weak_old = crate::SendWeak::new(old);
        let weak_notebook = crate::SendWeak::new(&notebook);
        std::thread::spawn(move || {
            let prepared = connections::prepare_session(&connection);
            crate::run_on_main(move || {
                let (Some(window), Some(old), Some(notebook)) =
                    (weak.upgrade(), weak_old.upgrade(), weak_notebook.upgrade())
                else {
                    return;
                };
                // The tab may have been closed during the wait: that means stop.
                if window.find_terminal(&old).is_none() {
                    return;
                }
                window.imp().pending_reconnect_attempts.set(attempts);
                let title = connection.name.clone();
                let term = window.add_new_tab(
                    None,
                    Some(&notebook),
                    Some(&title),
                    Some(&title),
                    Some(&prepared.plan.argv),
                );
                window.attach_connection(&term, &connection, true);
                notebook.reorder_child(&term, Some(position));
                window.close_tab_signal(Some(&old));
                window.reload_connections_panel();
            });
        });
    }

    fn open_connection_terminal(
        &self,
        argv: &Vec<String>,
        title: &str,
        placement: Placement,
    ) -> Option<TerminalBox> {
        match placement {
            Placement::Tab => {
                Some(self.add_new_tab(None, None, Some(title), Some(title), Some(argv)))
            }
            Placement::Split => {
                let split = self.split_terminal("vertical", None, Some(argv), Some(title));
                if split.is_none() {
                    self.status_message(
                        "This window is already split; opening a tab instead",
                        false,
                    );
                    return Some(self.add_new_tab(
                        None,
                        None,
                        Some(title),
                        Some(title),
                        Some(argv),
                    ));
                }
                split
            }
            Placement::OtherSide { vertical } => {
                if *self.imp().split_mode.borrow() == "single" {
                    let mode = if vertical { "vertical" } else { "horizontal" };
                    return self.split_terminal(mode, None, Some(argv), Some(title));
                }
                let other = self.imp().notebook2.borrow().clone();
                Some(self.add_new_tab(None, other.as_ref(), Some(title), Some(title), Some(argv)))
            }
            Placement::Window => {
                let term = self.add_new_tab(None, None, Some(title), Some(title), Some(argv));
                self.detach_tab_signal(&term);
                Some(term)
            }
        }
    }

    /// Open the two-pane file browser on this entry's server.
    pub fn open_file_browser(&self, connection: Connection) {
        self.open_file_browser_at(connection, None);
    }

    /// Open the file browser, optionally in a folder the terminal already shows.
    pub fn open_file_browser_at(&self, connection: Connection, start: Option<String>) {
        let program = if connection.protocol == Protocol::Ftp {
            "lftp"
        } else {
            "sftp"
        };
        if crate::notes::which(program).is_none() {
            self.status_message(&format!("{} is not installed", program), true);
            return;
        }
        let window: gtk::Window = self.clone().upcast();
        let weak = self.downgrade();
        let open_editor: Rc<dyn Fn(&std::path::Path)> = Rc::new(move |path| {
            if let Some(window) = weak.upgrade() {
                window.open_file_in_editor(path, None, None);
            }
        });
        crate::file_browser::open_at(&window, connection, start.as_deref(), Some(open_editor));
    }

    /// Show a remote desktop. Its viewer runs in a tab, like ssh, so its logs and
    /// questions (a changed certificate, a password) stay visible and answerable.
    /// With a tunnel a first tab carries the ssh, and the viewer starts once the
    /// service answers.
    pub fn start_desktop(&self, connection: Connection) {
        if self
            .open_connection_counts()
            .get(&connection.id)
            .is_some_and(|count| *count > 0)
        {
            self.status_message(&format!("{} is already open", connection.name), false);
            return;
        }
        if desktop::uses_tunnel(&connection) {
            self.start_tunnelled_desktop(connection);
            return;
        }
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let problems = preconnect::run(&connection);
            let password = stored_password(&connection);
            let port = desktop::remote_port(&connection);
            let viewer = desktop::viewer(&connection, &connection.host, port, password);
            crate::run_on_main(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                for problem in &problems {
                    window.status_message(problem, true);
                }
                match viewer {
                    Ok(viewer) => {
                        window.open_viewer_tab(&connection, viewer, None);
                    }
                    Err(error) => window.status_message(&error, true),
                }
                window.reload_connections_panel();
            });
        });
    }

    fn start_tunnelled_desktop(&self, connection: Connection) {
        let Some(local_port) = desktop::free_local_port() else {
            self.status_message("No free local port for the tunnel", true);
            return;
        };
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let problems = preconnect::run(&connection);
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    for problem in &problems {
                        window.status_message(problem, true);
                    }
                    window.open_desktop_tab(connection, local_port);
                }
            });
        });
    }

    fn open_desktop_tab(&self, connection: Connection, local_port: u16) {
        // A password invented for this session. The server reads it with echo off
        // from the tab, so it reaches neither a command line nor the scrollback.
        let one_time = (connection.protocol == Protocol::Vnc
            && connection.vnc_server.uses_one_time_password())
        .then(desktop::one_time_password);
        let mut attached = connection.clone();
        if let Some(password) = &one_time {
            attached.expect.push(ExpectRule {
                pattern: regex::escape(desktop::OTP_PROMPT),
                command: password.clone(),
                max_trigger: 1,
                delay_ms: 0,
            });
        }
        // The tab has its own lifetime: dropping the connection must not reconnect
        // behind a viewer that is gone.
        attached.auto_reconnect = false;
        // The viewer gets its own tab, so the two are told apart by name.
        attached.name = format!("{} (tunnel)", connection.name);
        let argv = desktop::tunnel_argv(&connection, local_port);
        let title = attached.name.clone();
        let term = self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
        self.attach_connection(&term, &attached, true);

        let key = desktop_key(&connection);
        let started = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        // The viewer's tab is created on the main thread once the service answers.
        let viewer_term = ViewerTab::new();
        let viewer_term_from_thread = crate::Sendable(viewer_term.clone());
        {
            let started = started.clone();
            let cancelled = cancelled.clone();
            let key = key.clone();
            let connection = connection.clone();
            let one_time = one_time.clone();
            let weak = crate::SendWeak::new(self);
            std::thread::spawn(move || {
                let deadline = Instant::now() + DESKTOP_WAIT;
                while !desktop::service_is_ready(connection.protocol, local_port) {
                    if cancelled.load(Ordering::SeqCst) {
                        return;
                    }
                    if Instant::now() >= deadline {
                        tunnels::stop(&key);
                        crate::run_on_main(move || {
                            if let Some(window) = weak.upgrade() {
                                window.status_message(
                                    "The remote desktop did not start: see the tab",
                                    true,
                                );
                            }
                        });
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                let password = one_time.clone().or_else(|| stored_password(&connection));
                match desktop::viewer(&connection, "127.0.0.1", local_port, password) {
                    Ok(viewer) => crate::run_on_main(move || {
                        if cancelled.load(Ordering::SeqCst) {
                            return;
                        }
                        let Some(window) = weak.upgrade() else {
                            return;
                        };
                        let term = window.open_viewer_tab(&connection, viewer, one_time.as_deref());
                        viewer_term_from_thread.set(term);
                        started.store(true, Ordering::SeqCst);
                    }),
                    Err(error) => {
                        tunnels::stop(&key);
                        crate::run_on_main(move || {
                            if let Some(window) = weak.upgrade() {
                                window.status_message(&error, true);
                            }
                        })
                    }
                }
            });
        }
        // The two tabs and the processes are one session: closing either end ends
        // the other, and a failed tunnel takes the viewer down with it.
        let weak = crate::SendWeak::new(self);
        let weak_term = crate::SendWeak::new(&term);
        glib::timeout_add_seconds_local(1, move || {
            let ssh_term = weak_term.upgrade().filter(|term| term.parent().is_some());
            let Some(ssh_term) = ssh_term else {
                cancelled.store(true, Ordering::SeqCst);
                tunnels::stop(&key);
                if let Some(window) = weak.upgrade() {
                    viewer_term.close(&window);
                }
                return glib::ControlFlow::Break;
            };
            if !started.load(Ordering::SeqCst) {
                return glib::ControlFlow::Continue;
            }
            if !viewer_term.is_open() {
                // The person closed the viewer: the tunnel has nothing left to carry.
                cancelled.store(true, Ordering::SeqCst);
                tunnels::stop(&key);
                if let Some(window) = weak.upgrade() {
                    window.close_tab_signal(Some(&ssh_term));
                }
                return glib::ControlFlow::Break;
            }
            let status = tunnels::status(&key);
            if status.is_active() {
                return glib::ControlFlow::Continue;
            }
            if let Some(window) = weak.upgrade() {
                viewer_term.close(&window);
                match status {
                    // Keep the tab: what went wrong may be written there.
                    tunnels::Status::Failed(reason) => {
                        window.status_message(&format!("Viewer: {}", reason), true);
                    }
                    _ => window.close_tab_signal(Some(&ssh_term)),
                }
                window.reload_connections_panel();
            }
            glib::ControlFlow::Break
        });
    }

    /// Run a desktop viewer in its own tab. A password from the keyring is typed
    /// at the viewer's prompt, with echo off, rather than piped: piping it also
    /// makes FreeRDP answer a certificate question with "no" without showing it.
    fn open_viewer_tab(
        &self,
        connection: &Connection,
        viewer: desktop::Viewer,
        one_time: Option<&str>,
    ) -> TerminalBox {
        let mut attached = connection.clone();
        attached.auto_reconnect = false;
        if let Some(password) = viewer.prompt_password {
            attached.expect.push(ExpectRule {
                pattern: desktop::PASSWORD_PROMPT.to_string(),
                command: password,
                max_trigger: 1,
                delay_ms: 0,
            });
        }
        let title = connection.name.clone();
        let term = self.add_new_tab_with_env(
            None,
            None,
            Some(&title),
            Some(&title),
            Some(&viewer.argv),
            &viewer.env,
        );
        self.attach_connection(&term, &attached, true);
        // A viewer that takes no password from a program: the person types it,
        // so it has to be shown, here on their own screen.
        if let (false, Some(password)) = (viewer.delivered, one_time) {
            term.vte()
                .feed(format!("\r\n[Oxterm] VNC password: {}\r\n", password).as_bytes());
        }
        term
    }

    fn copy_key_to_server(&self, connection: &Connection) {
        if crate::notes::which("ssh-copy-id").is_none() {
            self.status_message("ssh-copy-id is not installed", true);
            return;
        }
        let title = format!("copy key to {}", connection.name);
        let argv = connection.copy_id_argv();
        self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
    }

    fn generate_ssh_key(&self) {
        if crate::notes::which("ssh-keygen").is_none() {
            self.status_message("ssh-keygen is not installed", true);
            return;
        }
        // In a terminal, so the file name and passphrase prompts stay ssh-keygen's
        // own and a passphrase never appears on a command line.
        let argv = vec![
            "ssh-keygen".to_string(),
            "-t".to_string(),
            "ed25519".to_string(),
        ];
        let title = "ssh-keygen".to_string();
        self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
    }

    /// The Connections menu, rebuilt each time it opens so favorites, recent
    /// connections and the current tab's macros are up to date.
    pub fn populate_connections_menu(&self, menu: &gtk::Menu, with_saved: bool) {
        for child in menu.children() {
            menu.remove(&child);
        }
        let add = |menu: &gtk::Menu, label: &str, action: Box<dyn Fn(&MainWindow)>| {
            let item = gtk::MenuItem::with_label(label);
            let weak = crate::SendWeak::new(self);
            item.connect_activate(move |_| {
                if let Some(window) = weak.upgrade() {
                    action(&window);
                }
            });
            menu.append(&item);
        };
        add(
            menu,
            "Connections Panel\tCtrl+Shift+J",
            Box::new(|window| window.toggle_connections_panel()),
        );
        add(
            menu,
            "New Connection...",
            Box::new(|window| window.new_connection()),
        );
        add(
            menu,
            "Tunnel Manager...",
            Box::new(|window| window.show_tunnel_manager(None)),
        );
        add(menu, "Workspaces...", Box::new(crate::workspaces_ui::show));
        add(
            menu,
            "Import Connections...",
            Box::new(crate::import_sync_ui::show_import_dialog),
        );
        add(
            menu,
            "Sync ~/.ssh/config...",
            Box::new(crate::import_sync_ui::show_ssh_sync_dialog),
        );
        add(
            menu,
            "Monitor Panel",
            Box::new(|window| {
                window.show_workspace_tab(crate::workspace_ui::WorkspaceTab::Monitor)
            }),
        );
        add(
            menu,
            "Quick Connect...\tCtrl+Shift+K",
            Box::new(|window| window.ssh_picker_signal()),
        );
        // Files are read only when the menu really opens. The menus are built at every
        // start, when nothing may read ~/.ssh/config, which can sit on a slow disk.
        let (entries, usage) = if with_saved {
            (
                load_entries().unwrap_or_default(),
                connections::load_usage(&connections::usage_path()),
            )
        } else {
            (Vec::new(), UsageMap::new())
        };
        if let Some(term) = self.current_terminal() {
            if let Some(id) = term.connection_id() {
                menu.append(&gtk::SeparatorMenuItem::new());
                let name = term
                    .connection_name()
                    .unwrap_or_else(|| "this connection".to_string());
                if let Some(entry) = connections::find_entry_by_id(&entries, &id) {
                    if remote_files::supports_browsing(&entry.connection) {
                        let connection = entry.connection.clone();
                        add(
                            menu,
                            "Browse Files...",
                            Box::new(move |window| window.open_file_browser(connection.clone())),
                        );
                    }
                }
                let edit_id = id.clone();
                add(
                    menu,
                    &format!("Edit {}...", name),
                    Box::new(move |window| window.edit_connection(&edit_id)),
                );
                let macros = term.connection_macros();
                if !macros.is_empty() {
                    let submenu = gtk::Menu::new();
                    for item in macros {
                        let entry = gtk::MenuItem::with_label(&item.name);
                        entry.set_tooltip_text(Some(&item.command));
                        let weak = crate::SendWeak::new(&term);
                        entry.connect_activate(move |_| {
                            if let Some(term) = weak.upgrade() {
                                term.control_send(&item.command, true);
                            }
                        });
                        submenu.append(&entry);
                    }
                    let macros_item = gtk::MenuItem::with_label("Macros");
                    macros_item.set_submenu(Some(&submenu));
                    menu.append(&macros_item);
                }
            }
        }
        let shortcuts = connections::menu_shortcuts(&entries, &usage, MENU_SHORTCUTS);
        if !shortcuts.is_empty() {
            menu.append(&gtk::SeparatorMenuItem::new());
            for index in shortcuts {
                let connection = entries[index].connection.clone();
                let id = connection.id.clone();
                add(
                    menu,
                    &connection.name,
                    Box::new(move |window| window.connect_id(&id, Placement::Tab)),
                );
            }
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        add(
            menu,
            "Generate SSH Key...",
            Box::new(|window| window.generate_ssh_key()),
        );
        menu.show_all();
    }

    fn open_sftp(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id) else {
            return;
        };
        let program = if entry.connection.protocol == Protocol::Ftp {
            "lftp"
        } else {
            "sftp"
        };
        if crate::notes::which(program).is_none() {
            self.status_message(&format!("{} is not installed", program), true);
            return;
        }
        let mut argv = match entry.connection.protocol {
            Protocol::Ftp => entry.connection.terminal_argv().unwrap_or_default(),
            _ => entry.connection.sftp_argv(),
        };
        if entry.connection.protocol != Protocol::Ftp && !entry.connection.no_sharing {
            // Without a master every sftp start pays a full SSH handshake.
            let path = crate::headless::paths::ssh_socket_dir().join("cm-%C");
            argv.splice(
                1..1,
                [
                    "-o".to_string(),
                    "ControlMaster=auto".to_string(),
                    "-o".to_string(),
                    format!("ControlPath={}", path.display()),
                    "-o".to_string(),
                    "ControlPersist=10m".to_string(),
                ],
            );
        }
        let title = format!("files {}", entry.connection.name);
        let term = self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
        term.set_connection(
            Some(entry.connection.id.clone()),
            Some(entry.connection.name.clone()),
        );
        term.set_color_override(Some(entry.connection.color_scheme.clone()));
        self.pin_tab_title(&term, &title);
    }

    // ── Tunnels ──────────────────────────────────────────────

    /// Open the tunnel manager, or bring it forward; `connection_id` filters it
    /// to one connection.
    pub fn show_tunnel_manager(&self, connection_id: Option<&str>) {
        crate::tunnel_manager::show(self, connection_id);
    }

    pub fn start_tunnel(&self, connection: &Connection, tunnel: &Tunnel) {
        spawn_tunnel(connection, tunnel);
    }

    /// Follow tunnel states while any is active: refresh the rows on a change
    /// and report a failure once. Stops itself when nothing is left to watch.
    pub fn watch_tunnels(&self) {
        if self.imp().tunnel_watch_active.replace(true) {
            return;
        }
        let weak = crate::SendWeak::new(self);
        let mut last = tunnels::snapshot();
        // A start or stop may still be on its way to the registry when the watch
        // begins; give it a moment before concluding that nothing runs.
        let mut idle_ticks = 0;
        glib::timeout_add_seconds_local(1, move || {
            let Some(window) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let current = tunnels::snapshot();
            if current != last {
                for (key, status) in &current {
                    if let tunnels::Status::Failed(reason) = status {
                        if last.get(key) != Some(status) {
                            window.status_message(&format!("Tunnel failed: {}", reason), true);
                        }
                    }
                }
                last = current.clone();
                window.reload_connections_panel();
            }
            if current.values().any(tunnels::Status::is_active) {
                idle_ticks = 0;
                return glib::ControlFlow::Continue;
            }
            idle_ticks += 1;
            if idle_ticks < 3 {
                return glib::ControlFlow::Continue;
            }
            window.imp().tunnel_watch_active.set(false);
            glib::ControlFlow::Break
        });
    }
}

/// First value of a key in `ssh -G` output, which uses lowercase names.
fn ssh_config_value<'a>(config: &'a str, key: &str) -> Option<&'a str> {
    config.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(' '))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// The text of the "SSH Details" dialog: what OpenSSH resolved, then the full
/// effective configuration for the cases where something does not work.
fn ssh_details_text(connection: &Connection, command: &str, effective: Option<&str>) -> String {
    let mut text = format!(
        "Connection\n  {}\n\nCommand\n  {}\n\n",
        connection.name, command
    );
    match effective {
        Some(config) => {
            text.push_str("Resolved by OpenSSH\n");
            for (label, key) in [
                ("Host name", "hostname"),
                ("User", "user"),
                ("Port", "port"),
                ("ProxyJump", "proxyjump"),
                ("Identity file", "identityfile"),
                ("Known hosts", "userknownhostsfile"),
                ("Host key policy", "stricthostkeychecking"),
            ] {
                if let Some(value) = ssh_config_value(config, key) {
                    text.push_str(&format!("  {}: {}\n", label, value));
                }
            }
            let control = ssh_config_value(config, "controlpath").unwrap_or("none");
            text.push_str(&format!(
                "  Connection sharing: {}\n",
                if control == "none" { "off" } else { control }
            ));
            let keepalive = ssh_config_value(config, "serveraliveinterval").unwrap_or("0");
            text.push_str(&format!(
                "  Keepalive: {}\n",
                if keepalive == "0" {
                    "off".to_string()
                } else {
                    format!("{} s", keepalive)
                }
            ));
            let agent = ssh_config_value(config, "forwardagent").unwrap_or("no");
            text.push_str(&format!("  Agent forwarding: {}\n", agent));
            let mut notes = Vec::new();
            if let Some("no" | "off" | "false") = ssh_config_value(config, "stricthostkeychecking")
            {
                notes.push("host-key verification is disabled");
            }
            if ssh_config_value(config, "userknownhostsfile")
                .is_some_and(|paths| paths.split_whitespace().all(|path| path == "/dev/null"))
            {
                notes.push("host keys are not saved (known_hosts is /dev/null)");
            }
            if agent == "yes" {
                notes.push("the SSH agent is forwarded to the server");
            }
            if ssh_config_value(config, "forwardx11").is_some_and(|value| value == "yes")
                || ssh_config_value(config, "forwardx11trusted").is_some_and(|value| value == "yes")
            {
                notes.push("X11 is forwarded to the server");
            }
            if !notes.is_empty() {
                text.push_str("\nSecurity notes\n");
                for note in notes {
                    text.push_str(&format!("  - {}\n", note));
                }
            }
            text.push_str("\nEffective configuration (ssh -G)\n");
            text.push_str(config.trim_end());
            text.push('\n');
        }
        None => text.push_str(
            "OpenSSH configuration could not be read.\n\n\
             `ssh -G` failed or timed out. The connection still works, but Oxterm cannot add \
             connection sharing, keepalives, or the keyring helper for this entry. Check the \
             host and the Include/Match exec rules in ~/.ssh/config.\n",
        ),
    }
    text
}

/// Ask whether to reopen the SSH connections of a restored session, and open
/// them where they were, in their old order. Nothing connects by itself after a
/// restart: a session file must not be able to log in to a production host
/// without someone saying yes.
pub fn offer_reconnect(window: &MainWindow, tabs: Vec<(String, bool)>, vertical: bool) {
    let Ok(entries) = load_entries() else {
        return;
    };
    let wanted: Vec<(Connection, bool)> = tabs
        .iter()
        .filter_map(|(id, right)| {
            connections::find_entry_by_id(&entries, id)
                .map(|entry| (entry.connection.clone(), *right))
        })
        .collect();
    if wanted.is_empty() {
        return;
    }
    let names: Vec<&str> = wanted
        .iter()
        .map(|(connection, _)| connection.name.as_str())
        .collect();
    let dialog = gtk::MessageDialog::new(
        Some(window),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Question,
        gtk::ButtonsType::YesNo,
        &format!(
            "The last session had these SSH connections open. Reconnect?\n\n{}",
            names.join("\n")
        ),
    );
    let answer = dialog.run();
    // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
    unsafe {
        dialog.destroy();
    }
    if answer != gtk::ResponseType::Yes {
        return;
    }
    // An entry that asks before connecting still asks: "reconnect all" was said
    // to a list, not to that host in particular.
    let wanted: Vec<(Connection, bool)> = wanted
        .into_iter()
        .filter(|(connection, _)| {
            !connection.confirm_connect || window.confirm_connection(connection)
        })
        .collect();
    let weak = crate::SendWeak::new(window);
    std::thread::spawn(move || {
        let prepared: Vec<_> = wanted
            .into_iter()
            .map(|(connection, right)| {
                for problem in preconnect::run(&connection) {
                    LOGGER.warning(&format!("connection_preconnect_failed {}", problem));
                }
                let plan = connections::prepare_session(&connection);
                (connection, plan, right)
            })
            .collect();
        crate::run_on_main(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            for (connection, plan, right) in prepared {
                let placement = if right {
                    Placement::OtherSide { vertical }
                } else {
                    Placement::Tab
                };
                window.finish_connect(connection, plan, placement);
            }
        });
    });
}

/// Start a saved tunnel. Preparing it may read the keyring and run `ssh -G`, so
/// that happens on a thread of its own.
pub fn spawn_tunnel(connection: &Connection, tunnel: &Tunnel) {
    let connection = connection.clone();
    let tunnel = tunnel.clone();
    std::thread::spawn(move || {
        let spec = connections::tunnel_spec(&connection, &tunnel);
        tunnels::start(&tunnel_key(&connection, &tunnel), spec);
    });
}

fn desktop_key(connection: &Connection) -> String {
    tunnels::key(&connection.id, "desktop")
}

/// The viewer tab of a tunnelled desktop. Created and used on the main thread;
/// the thread that waits for the service only hands the tab over through
/// `run_on_main`, so the `Rc` never crosses a thread on its own.
#[derive(Clone)]
struct ViewerTab(Rc<RefCell<Option<TerminalBox>>>);

impl ViewerTab {
    fn new() -> ViewerTab {
        ViewerTab(Rc::new(RefCell::new(None)))
    }

    fn set(&self, term: TerminalBox) {
        *self.0.borrow_mut() = Some(term);
    }

    fn is_open(&self) -> bool {
        self.0
            .borrow()
            .as_ref()
            .is_some_and(|term| term.parent().is_some())
    }

    fn close(&self, window: &MainWindow) {
        if let Some(term) = self.0.borrow_mut().take() {
            if term.parent().is_some() {
                window.close_tab_signal(Some(&term));
            }
        }
    }
}

/// Reads the keyring, which may wait for an unlock: not on the GUI thread.
fn stored_password(connection: &Connection) -> Option<String> {
    (connection.auth == Auth::SecretService)
        .then(|| askpass::lookup_secret(&connection.id))
        .flatten()
}
