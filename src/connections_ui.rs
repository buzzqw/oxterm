//! The connections feature on the main window: panel behaviour, the connect
//! flow and tunnel control. Kept out of `window.rs`, which only hosts the slot
//! and forwards a few calls here.

use std::rc::Rc;

use glib::subclass::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

use crate::connections_panel::{ConnectionsPanel, RowDecoration};
use crate::headless::askpass;
use crate::headless::connections::{
    self, tunnel_key, Auth, Connection, Entry, Kind, Origin, Prepared, Tunnel, UsageMap,
};
use crate::headless::ssh_config;
use crate::headless::tunnels;
use crate::logging::LOGGER;
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::{MainWindow, TerminalWindow};

/// Where a connection opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Tab,
    Split,
    Window,
}

/// The book merged with `~/.ssh/config`, or why the book could not be read.
pub fn load_entries() -> Result<Vec<Entry>, String> {
    let book = connections::load_book(&connections::book_path())?;
    Ok(connections::merge(&book, &ssh_config::load_hosts()))
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
        panel.search.connect_search_changed(move |_| {
            if let Some(window) = weak.upgrade() {
                window.reload_connections_panel();
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
                _ => glib::Propagation::Proceed,
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
            let selected = window
                .imp()
                .connections_panel
                .borrow()
                .as_ref()
                .and_then(|panel| panel.selected_id());
            match (key, selected) {
                (gdk::keys::constants::Return | gdk::keys::constants::KP_Enter, Some(id))
                    if control =>
                {
                    window.connect_id(&id, Placement::Split);
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
                return glib::Propagation::Proceed;
            };
            tree.selection().select_path(&path);
            let id = window
                .imp()
                .connections_panel
                .borrow()
                .as_ref()
                .and_then(|panel| panel.id_at(&path));
            if let Some(id) = id {
                window.show_connection_menu(&id);
            }
            glib::Propagation::Stop
        });
        slot.pack_start(&panel.root, false, true, 0);
        *self.imp().connections_panel.borrow_mut() = Some(panel);
        true
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
            // See `set_notes_panel_visible` for why `no_show_all` is toggled.
            panel.root.set_no_show_all(false);
            panel.root.show_all();
            panel.root.set_no_show_all(true);
            self.reload_connections_panel();
            panel.search.grab_focus();
        } else {
            panel.root.hide();
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
        let book_path = connections::book_path();
        let (book, status) = match connections::load_book(&book_path) {
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
        let query = panel.search.text().to_string();
        let tree = connections::build_tree(&entries, &usage, &query);
        let open = self.open_connection_counts();
        let patterns = settings().get_str("ssh_alert_patterns");
        let alert = settings().get_str("ssh_alert_color");
        let states = tunnels::snapshot();
        panel.populate(&tree, &entries, &|entry| {
            let connection = &entry.connection;
            let mut decoration = RowDecoration {
                open_tabs: open.get(&connection.id).copied().unwrap_or(0),
                ..RowDecoration::default()
            };
            for tunnel in &connection.tunnels {
                match states.get(&tunnel_key(connection, tunnel)) {
                    Some(status) if status.is_active() => decoration.active_tunnels += 1,
                    Some(tunnels::Status::Failed(_)) => decoration.failed_tunnels += 1,
                    _ => {}
                }
            }
            if ssh_config::alert_host(&patterns, connection.destination())
                || ssh_config::alert_host(&patterns, &connection.name)
            {
                decoration.alert_color = Some(alert.clone());
            }
            decoration
        });
        panel.status.set_text(status.as_deref().unwrap_or(""));
    }

    fn open_connection_counts(&self) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        for term in self.all_terminals() {
            if let Some(id) = term.connection_id() {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        counts
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

    fn show_connection_menu(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id).cloned() else {
            return;
        };
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
        {
            let id = id.to_string();
            add("Open SFTP", Box::new(move |window| window.open_sftp(&id)));
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        if !entry.connection.tunnels.is_empty() {
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
            let tunnels_item = gtk::MenuItem::with_label("Tunnels");
            tunnels_item.set_submenu(Some(&submenu));
            menu.append(&tunnels_item);
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
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    // ── Editing ──────────────────────────────────────────────

    pub fn new_connection(&self) {
        self.run_form("New Connection", Connection::new_manual());
    }

    fn edit_connection(&self, id: &str) {
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
            // The password belongs to the original's keyring entry.
            copy.auth = Auth::Default;
            self.run_form("Duplicate Connection", copy);
        }
    }

    fn run_form(&self, title: &str, initial: Connection) {
        let id = initial.id.clone();
        let was_secret = initial.auth == Auth::SecretService;
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
        let password = result.password;
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let error = match password {
                Some(password) => askpass::store_secret(&id, &label, &password).err(),
                None => {
                    if was_secret && !wants_secret {
                        askpass::clear_secret(&id);
                    }
                    None
                }
            };
            if let Some(error) = error {
                crate::run_on_main(move || {
                    if let Some(window) = weak.upgrade() {
                        window.status_message(&error, true);
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
                if connection.auth == Auth::SecretService {
                    let id = connection.id.clone();
                    std::thread::spawn(move || askpass::clear_secret(&id));
                }
                self.reload_connections_panel();
            }
            Err(error) => self.status_message(&format!("Could not save: {}", error), true),
        }
    }

    // ── Connecting ───────────────────────────────────────────

    pub fn connect_id(&self, id: &str, placement: Placement) {
        match load_entries() {
            Ok(entries) => match connections::find_entry_by_id(&entries, id) {
                Some(entry) => self.connect(entry.connection.clone(), placement),
                None => self.status_message("That connection no longer exists", true),
            },
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
        let connection = connections::find_entry(&entries, text)
            .map(|entry| entry.connection.clone())
            .unwrap_or_else(|| Connection::alias(text));
        self.connect(connection, Placement::Tab);
    }

    pub fn connect(&self, connection: Connection, placement: Placement) {
        if let Err(error) = connection.validate() {
            self.status_message(&error, true);
            return;
        }
        if crate::notes::which("ssh").is_none() {
            self.status_message("Cannot connect: ssh is not installed", true);
            return;
        }
        if connection.confirm_connect && !self.confirm_connection(&connection) {
            return;
        }
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let prepared = connections::prepare_session(&connection);
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    window.finish_connect(connection, prepared, placement);
                }
            });
        });
    }

    fn confirm_connection(&self, connection: &Connection) -> bool {
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

    fn finish_connect(&self, connection: Connection, prepared: Prepared, placement: Placement) {
        let title = connection.name.clone();
        let term = self.open_connection_terminal(&prepared.plan.argv, &title, placement);
        let Some(term) = term else {
            return;
        };
        term.set_connection(Some(connection.id.clone()), Some(connection.name.clone()));
        if placement != Placement::Window {
            self.pin_tab_title(&term, &title);
        }
        // Writes to disk with an fsync; not worth a stall on the GUI thread.
        let id = connection.id.clone();
        std::thread::spawn(move || {
            if let Err(error) = connections::record_usage(&connections::usage_path(), &id) {
                LOGGER.warning(&format!("connection_usage_failed error={}", error));
            }
        });
        let mut started = false;
        for tunnel in connection.tunnels.iter().filter(|tunnel| tunnel.autostart) {
            self.start_tunnel(&connection, tunnel);
            started = true;
        }
        if started {
            self.watch_tunnels();
        }
        if let Some(note) = prepared.keyring_note {
            self.status_message(note, true);
        } else if prepared.plan.forwards_agent {
            self.status_message(
                "Agent forwarding is enabled for this host; prefer ProxyJump instead",
                true,
            );
        }
        self.reload_connections_panel();
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
            Placement::Window => {
                let term = self.add_new_tab(None, None, Some(title), Some(title), Some(argv));
                self.detach_tab_signal(&term);
                Some(term)
            }
        }
    }

    fn open_sftp(&self, id: &str) {
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(entry) = connections::find_entry_by_id(&entries, id) else {
            return;
        };
        if crate::notes::which("sftp").is_none() {
            self.status_message("Cannot open SFTP: sftp is not installed", true);
            return;
        }
        let argv = entry.connection.sftp_argv();
        let title = format!("sftp {}", entry.connection.name);
        let term = self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
        term.set_connection(
            Some(entry.connection.id.clone()),
            Some(entry.connection.name.clone()),
        );
        self.pin_tab_title(&term, &title);
    }

    // ── Tunnels ──────────────────────────────────────────────

    pub fn start_tunnel(&self, connection: &Connection, tunnel: &Tunnel) {
        let connection = connection.clone();
        let tunnel = tunnel.clone();
        std::thread::spawn(move || {
            let spec = connections::tunnel_spec(&connection, &tunnel);
            tunnels::start(&tunnel_key(&connection, &tunnel), spec);
        });
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

/// Ask whether to reopen the SSH connections of a restored session, and open
/// them. Nothing connects by itself after a restart: a session file must not be
/// able to log in to a production host without someone saying yes.
pub fn offer_reconnect(window: &MainWindow, ids: Vec<String>) {
    let Ok(entries) = load_entries() else {
        return;
    };
    let connections_to_open: Vec<Connection> = ids
        .iter()
        .filter_map(|id| connections::find_entry_by_id(&entries, id))
        .map(|entry| entry.connection.clone())
        .collect();
    if connections_to_open.is_empty() {
        return;
    }
    let names: Vec<&str> = connections_to_open
        .iter()
        .map(|connection| connection.name.as_str())
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
    if answer == gtk::ResponseType::Yes {
        for connection in connections_to_open {
            window.connect(connection, Placement::Tab);
        }
    }
}
