//! The connections feature on the main window: panel behaviour, the connect
//! flow and tunnel control. Kept out of `window.rs`, which only hosts the slot
//! and forwards a few calls here.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glib::subclass::prelude::*;
use gtk::gdk;
use gtk::prelude::*;
use zoha_vte::traits::TerminalExt;

use crate::connections_panel::{ConnectionsPanel, RowDecoration};
use crate::headless::askpass;
use crate::headless::connections::{
    self, tunnel_key, Auth, Connection, Entry, ExpectRule, Kind, Origin, Prepared, Protocol,
    Tunnel, UsageMap,
};
use crate::headless::desktop;
use crate::headless::preconnect;
use crate::headless::remote_files;
use crate::headless::ssh_config;
use crate::headless::tunnels;
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
            match id {
                Some(id) => window.show_connection_menu(&id),
                None => window.show_group_menu(&path),
            }
            glib::Propagation::Stop
        });
        slot.pack_start(&panel.root, false, true, 0);
        *self.imp().connections_panel.borrow_mut() = Some(panel);
        true
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
            match states.get(&desktop_key(connection)) {
                Some(status) if status.is_active() => decoration.open_tabs += 1,
                Some(tunnels::Status::Failed(_)) => decoration.failed_tunnels += 1,
                _ => {}
            }
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
        let empty_hint = (entries.is_empty() && query.is_empty()).then_some(
            "No connections yet. Click New to add one. Hosts in ~/.ssh/config appear here by themselves.",
        );
        panel
            .status
            .set_text(status.as_deref().or(empty_hint).unwrap_or(""));
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
        menu.show_all();
        crate::popup_transient_menu(&menu);
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
        if protocol == Protocol::Ssh {
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
        match load_entries() {
            Ok(entries) => match connections::find_entry_by_id(&entries, id) {
                Some(entry) => self.attach_connection(term, &entry.connection, true),
                None => term.set_connection(Some(id.to_string()), None),
            },
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
        let Ok(entries) = load_entries() else {
            return;
        };
        let Some(connection) =
            connections::find_entry_by_id(&entries, &id).map(|entry| entry.connection.clone())
        else {
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
        crate::file_browser::open(&window, connection);
    }

    /// Show a remote desktop. Without a tunnel the viewer starts directly; with one
    /// a tab carries the ssh, and the viewer starts once the service answers.
    pub fn start_desktop(&self, connection: Connection) {
        if desktop::uses_tunnel(&connection) {
            self.start_tunnelled_desktop(connection);
            return;
        }
        let key = desktop_key(&connection);
        if tunnels::status(&key).is_active() {
            self.status_message(&format!("{} is already open", connection.name), false);
            return;
        }
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let problems = preconnect::run(&connection);
            let password = stored_password(&connection);
            let port = desktop::remote_port(&connection);
            let viewer = desktop::viewer(&connection, &connection.host, port, password);
            let error = match viewer {
                Ok((spec, _)) => {
                    tunnels::start(&key, spec);
                    None
                }
                Err(error) => Some(error),
            };
            crate::run_on_main(move || {
                if let Some(window) = weak.upgrade() {
                    for problem in problems.iter().chain(error.iter()) {
                        window.status_message(problem, true);
                    }
                    window.watch_tunnels();
                    window.reload_connections_panel();
                }
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
        let argv = desktop::tunnel_argv(&connection, local_port);
        let title = connection.name.clone();
        let term = self.add_new_tab(None, None, Some(&title), Some(&title), Some(&argv));
        self.attach_connection(&term, &attached, true);

        let key = desktop_key(&connection);
        let started = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let weak = crate::SendWeak::new(self);
        let weak_term = crate::SendWeak::new(&term);
        {
            let started = started.clone();
            let cancelled = cancelled.clone();
            let key = key.clone();
            let connection = connection.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + DESKTOP_WAIT;
                while !desktop::service_is_ready(connection.protocol, local_port) {
                    if cancelled.load(Ordering::SeqCst) {
                        return;
                    }
                    if Instant::now() >= deadline {
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
                    Ok((spec, delivered)) => {
                        tunnels::start(&key, spec);
                        started.store(true, Ordering::SeqCst);
                        // A viewer that takes no password from a program: the person
                        // types it, so it has to be shown, here on their own screen.
                        if let (false, Some(password)) = (delivered, one_time) {
                            crate::run_on_main(move || {
                                if let Some(term) = weak_term.upgrade() {
                                    term.vte().feed(
                                        format!("\r\n[Oxterm] VNC password: {}\r\n", password)
                                            .as_bytes(),
                                    );
                                }
                            });
                        }
                    }
                    Err(error) => crate::run_on_main(move || {
                        if let Some(window) = weak.upgrade() {
                            window.status_message(&error, true);
                        }
                    }),
                }
            });
        }
        // The tab and the viewer are one session: closing either ends the other.
        let weak = crate::SendWeak::new(self);
        let weak_term = crate::SendWeak::new(&term);
        glib::timeout_add_seconds_local(1, move || {
            let term = weak_term.upgrade().filter(|term| term.parent().is_some());
            let Some(term) = term else {
                cancelled.store(true, Ordering::SeqCst);
                tunnels::stop(&key);
                return glib::ControlFlow::Break;
            };
            if !started.load(Ordering::SeqCst) {
                return glib::ControlFlow::Continue;
            }
            let status = tunnels::status(&key);
            if status.is_active() {
                return glib::ControlFlow::Continue;
            }
            if let Some(window) = weak.upgrade() {
                match status {
                    // Keep the tab: what went wrong may be written there.
                    tunnels::Status::Failed(reason) => {
                        window.status_message(&format!("Viewer: {}", reason), true);
                    }
                    _ => window.close_tab_signal(Some(&term)),
                }
                window.reload_connections_panel();
            }
            glib::ControlFlow::Break
        });
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

    /// The SSH menu, rebuilt each time it opens so favorites, recent connections
    /// and the current tab's macros are up to date.
    pub fn populate_ssh_menu(&self, menu: &gtk::Menu) {
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
            "Quick Connect...\tCtrl+Shift+K",
            Box::new(|window| window.ssh_picker_signal()),
        );
        if let Some(term) = self.current_terminal() {
            if let Some(id) = term.connection_id() {
                menu.append(&gtk::SeparatorMenuItem::new());
                let name = term
                    .connection_name()
                    .unwrap_or_else(|| "this connection".to_string());
                if let Some(entry) =
                    connections::find_entry_by_id(&load_entries().unwrap_or_default(), &id)
                {
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
        let entries = load_entries().unwrap_or_default();
        let usage = connections::load_usage(&connections::usage_path());
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
        let argv = match entry.connection.protocol {
            Protocol::Ftp => entry.connection.terminal_argv().unwrap_or_default(),
            _ => entry.connection.sftp_argv(),
        };
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

fn desktop_key(connection: &Connection) -> String {
    tunnels::key(&connection.id, "desktop")
}

/// Reads the keyring, which may wait for an unlock: not on the GUI thread.
fn stored_password(connection: &Connection) -> Option<String> {
    (connection.auth == Auth::SecretService)
        .then(|| askpass::lookup_secret(&connection.id))
        .flatten()
}
