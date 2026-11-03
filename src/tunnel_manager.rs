//! Tunnel manager: one window with the saved tunnels of every connection, their
//! live state, and a log of what each background `ssh` did.
//!
//! Tunnels stay part of their connection in `connections.json`; this window only
//! gives them a place of their own, like the tunnel manager of PCM. The state
//! comes from `headless::tunnels`, the rows from `headless::connections`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::gdk;
use gtk::prelude::*;

use crate::connections_panel::{FAILED_COLOR, TUNNEL_STARTING_COLOR, TUNNEL_UP_COLOR};
use crate::connections_ui::load_entries;
use crate::headless::connections::{
    self, find_entry_by_id, tunnel_key, validate_tunnel, Entry, Protocol, Tunnel, TunnelKind,
    TunnelRow,
};
use crate::headless::tunnels::{self, Status};
use crate::window::{MainWindow, TerminalWindow};

const COLUMN_STATUS: u32 = 0;
const COLUMN_CONNECTION: u32 = 1;
const COLUMN_KIND: u32 = 2;
const COLUMN_LISTEN: u32 = 3;
const COLUMN_TARGET: u32 = 4;
const COLUMN_AUTOSTART: u32 = 5;
const COLUMN_UPTIME: u32 = 6;
const COLUMN_KEY: u32 = 7;
const COLUMN_TOOLTIP: u32 = 8;

thread_local! {
    static MANAGER: RefCell<Option<Rc<Manager>>> = const { RefCell::new(None) };
}

struct Manager {
    window: gtk::Window,
    owner: RefCell<crate::SendWeak<MainWindow>>,
    store: gtk::ListStore,
    tree: gtk::TreeView,
    search: gtk::SearchEntry,
    log: gtk::TextBuffer,
    log_view: gtk::TextView,
    summary: gtk::Label,
    start: gtk::Button,
    stop: gtk::Button,
    edit: gtk::Button,
    duplicate: gtk::Button,
    delete: gtk::Button,
    edit_connection: gtk::Button,
    entries: RefCell<Vec<Entry>>,
    /// The rows on screen, in store order.
    rows: RefCell<Vec<TunnelRow>>,
    log_seq: Cell<u64>,
}

/// Open the manager, or bring it forward. With a connection, the list is
/// filtered to it.
pub fn show(owner: &MainWindow, connection_id: Option<&str>) {
    let manager = MANAGER
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| {
            let manager = Manager::build(owner);
            MANAGER.with(|slot| *slot.borrow_mut() = Some(manager.clone()));
            manager
        });
    *manager.owner.borrow_mut() = crate::SendWeak::new(owner);
    manager.reload();
    if let Some(id) = connection_id {
        let name = manager
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.connection.id == id)
            .map(|entry| entry.connection.name.clone());
        if let Some(name) = name {
            manager.search.set_text(&name);
        }
    }
    manager.window.present();
}

/// Ask for a new tunnel of `connection_id`, in the manager that `show` opened.
pub fn add_tunnel_for(owner: &MainWindow, connection_id: &str) {
    if let Some(manager) = MANAGER.with(|slot| slot.borrow().clone()) {
        *manager.owner.borrow_mut() = crate::SendWeak::new(owner);
        manager.add(Some(connection_id));
    }
}

impl Manager {
    fn build(owner: &MainWindow) -> Rc<Manager> {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Tunnels");
        window.set_default_size(900, 560);
        window.set_icon_name(Some("network-wired"));
        if let Some(app) = owner.application() {
            window.set_application(Some(&app));
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        toolbar.set_margin_start(8);
        toolbar.set_margin_end(8);
        toolbar.set_margin_top(8);
        toolbar.set_margin_bottom(8);
        let button = |label: &str, tooltip: &str| {
            let button = gtk::Button::with_label(label);
            button.set_tooltip_text(Some(tooltip));
            button
        };
        let add = button("Add...", "Add a tunnel to a connection");
        let edit = button("Edit...", "Change the selected tunnel (F2)");
        let duplicate = button(
            "Duplicate...",
            "Copy the selected tunnel on the next free local port",
        );
        let delete = button("Delete", "Remove the selected tunnel (Delete)");
        let start = button("Start", "Start the selected tunnel (Enter)");
        let stop = button("Stop", "Stop the selected tunnel (Enter)");
        let start_all = button("Start All", "Start every tunnel in the list");
        let stop_all = button("Stop All", "Stop every tunnel in the list");
        let edit_connection = button(
            "Edit Connection...",
            "Open the connection the selected tunnel belongs to",
        );
        for widget in [&add, &edit, &duplicate, &delete] {
            toolbar.pack_start(widget, false, false, 0);
        }
        toolbar.pack_start(
            &gtk::Separator::new(gtk::Orientation::Vertical),
            false,
            false,
            4,
        );
        for widget in [&start, &stop, &start_all, &stop_all] {
            toolbar.pack_start(widget, false, false, 0);
        }
        toolbar.pack_end(&edit_connection, false, false, 0);
        root.pack_start(&toolbar, false, false, 0);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Filter by connection, type or port"));
        search.set_margin_start(8);
        search.set_margin_end(8);
        search.set_margin_bottom(6);
        root.pack_start(&search, false, false, 0);

        let store = gtk::ListStore::new(&[glib::Type::STRING; 9]);
        let tree = gtk::TreeView::with_model(&store);
        tree.set_tooltip_column(COLUMN_TOOLTIP as i32);
        tree.set_enable_search(false);
        for (title, column, markup, expand) in [
            ("Status", COLUMN_STATUS, true, false),
            ("Connection", COLUMN_CONNECTION, false, true),
            ("Type", COLUMN_KIND, false, false),
            ("Listen", COLUMN_LISTEN, false, false),
            ("Target", COLUMN_TARGET, false, true),
            ("Autostart", COLUMN_AUTOSTART, false, false),
            ("Up for", COLUMN_UPTIME, false, false),
        ] {
            let renderer = gtk::CellRendererText::new();
            if expand {
                renderer.set_ellipsize(pango::EllipsizeMode::End);
            }
            let view_column = gtk::TreeViewColumn::new();
            view_column.set_title(title);
            view_column.set_resizable(true);
            view_column.set_expand(expand);
            TreeViewColumnExt::pack_start(&view_column, &renderer, true);
            TreeViewColumnExt::add_attribute(
                &view_column,
                &renderer,
                if markup { "markup" } else { "text" },
                column as i32,
            );
            tree.append_column(&view_column);
        }
        let list_scroll =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        list_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        list_scroll.add(&tree);

        let log = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
        let log_view = gtk::TextView::with_buffer(&log);
        log_view.set_editable(false);
        log_view.set_cursor_visible(false);
        log_view.set_monospace(true);
        log_view.set_wrap_mode(gtk::WrapMode::WordChar);
        log_view.set_left_margin(6);
        let log_scroll =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        log_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        log_scroll.add(&log_view);
        let log_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let log_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        log_header.set_margin_start(8);
        log_header.set_margin_end(8);
        log_header.set_margin_top(4);
        log_header.set_margin_bottom(4);
        let log_title = gtk::Label::new(None);
        log_title.set_markup("<b>Log</b>");
        log_header.pack_start(&log_title, false, false, 0);
        let clear = gtk::Button::with_label("Clear");
        clear.set_relief(gtk::ReliefStyle::None);
        log_header.pack_end(&clear, false, false, 0);
        log_box.pack_start(&log_header, false, false, 0);
        log_box.pack_start(&log_scroll, true, true, 0);

        let paned = gtk::Paned::new(gtk::Orientation::Vertical);
        paned.pack1(&list_scroll, true, false);
        paned.pack2(&log_box, false, false);
        paned.set_position(300);
        root.pack_start(&paned, true, true, 0);

        let summary = gtk::Label::new(None);
        summary.set_xalign(0.0);
        summary.set_margin_start(8);
        summary.set_margin_end(8);
        summary.set_margin_top(4);
        summary.set_margin_bottom(6);
        summary.style_context().add_class("dim-label");
        root.pack_start(&summary, false, false, 0);
        window.add(&root);

        let manager = Rc::new(Manager {
            window,
            owner: RefCell::new(crate::SendWeak::new(owner)),
            store,
            tree,
            search,
            log,
            log_view,
            summary,
            start,
            stop,
            edit,
            duplicate,
            delete,
            edit_connection,
            entries: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            log_seq: Cell::new(0),
        });

        let on = |button: &gtk::Button, action: fn(&Manager)| {
            let weak = Rc::downgrade(&manager);
            button.connect_clicked(move |_| {
                if let Some(manager) = weak.upgrade() {
                    action(&manager);
                }
            });
        };
        on(&add, |manager| manager.add(None));
        on(&manager.edit, Manager::edit_selected);
        on(&manager.duplicate, Manager::duplicate_selected);
        on(&manager.delete, Manager::delete_selected);
        on(&manager.start, |manager| manager.set_selected_running(true));
        on(&manager.stop, |manager| manager.set_selected_running(false));
        on(&start_all, |manager| manager.set_all_running(true));
        on(&stop_all, |manager| manager.set_all_running(false));
        on(&manager.edit_connection, Manager::edit_selected_connection);
        on(&clear, |manager| manager.log.set_text(""));

        let weak = Rc::downgrade(&manager);
        manager.search.connect_search_changed(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.refresh();
            }
        });
        let weak = Rc::downgrade(&manager);
        manager.tree.selection().connect_changed(move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.update_buttons();
            }
        });
        let weak = Rc::downgrade(&manager);
        manager.tree.connect_row_activated(move |_, _, _| {
            if let Some(manager) = weak.upgrade() {
                manager.toggle_selected();
            }
        });
        let weak = Rc::downgrade(&manager);
        manager.tree.connect_key_press_event(move |_, event| {
            let Some(manager) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match event.keyval() {
                gdk::keys::constants::Delete => manager.delete_selected(),
                gdk::keys::constants::F2 => manager.edit_selected(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&manager);
        manager.tree.connect_button_press_event(move |tree, event| {
            if event.button() != 3 {
                return glib::Propagation::Proceed;
            }
            let Some(manager) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let (x, y) = event.position();
            if let Some((Some(path), _, _, _)) = tree.path_at_pos(x as i32, y as i32) {
                tree.selection().select_path(&path);
                manager.show_menu();
            }
            glib::Propagation::Stop
        });
        manager.window.connect_key_press_event(|window, event| {
            if event.keyval() == gdk::keys::constants::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        // Another Oxterm or the connection form may have changed the book.
        let weak = Rc::downgrade(&manager);
        manager.window.connect_focus_in_event(move |_, _| {
            if let Some(manager) = weak.upgrade() {
                manager.reload();
            }
            glib::Propagation::Proceed
        });
        manager.window.connect_destroy(|_| {
            MANAGER.with(|slot| *slot.borrow_mut() = None);
        });
        Manager::tick(Rc::downgrade(&manager));
        manager.window.show_all();
        manager
    }

    /// Follow the states and the log while the window is open.
    fn tick(weak: Weak<Manager>) {
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(manager) => {
                manager.refresh();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    fn owner(&self) -> Option<MainWindow> {
        if let Some(window) = self.owner.borrow().upgrade() {
            return Some(window);
        }
        self.window
            .application()?
            .windows()
            .into_iter()
            .find_map(|window| window.downcast::<MainWindow>().ok())
    }

    /// Have every main window follow the tunnels, so its connections panel
    /// shows the new states.
    fn notify_windows(&self) {
        let Some(app) = self.window.application() else {
            return;
        };
        for window in app.windows() {
            if let Ok(main) = window.downcast::<MainWindow>() {
                main.watch_tunnels();
            }
        }
    }

    fn message(&self, text: &str, is_error: bool) {
        if is_error {
            let dialog = gtk::MessageDialog::new(
                Some(&self.window),
                gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                gtk::MessageType::Error,
                gtk::ButtonsType::Close,
                text,
            );
            dialog.run();
            // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
            unsafe {
                dialog.destroy();
            }
        } else if let Some(owner) = self.owner() {
            owner.status_message(text, false);
        }
    }

    /// Read the book and `~/.ssh/config` again, then redraw.
    fn reload(&self) {
        match load_entries() {
            Ok(entries) => *self.entries.borrow_mut() = entries,
            Err(error) => self
                .summary
                .set_text(&format!("Could not read the connection book: {}", error)),
        }
        self.refresh();
    }

    /// Redraw from the entries already read and the live states. Rows are only
    /// rebuilt when the set changes, so a refresh keeps the scroll position.
    fn refresh(&self) {
        let query = self.search.text().to_string();
        let rows: Vec<TunnelRow> =
            connections::tunnel_rows(&self.entries.borrow(), &tunnels::snapshot())
                .into_iter()
                .filter(|row| row.matches(&query))
                .collect();
        let same = {
            let current = self.rows.borrow();
            current.len() == rows.len()
                && current
                    .iter()
                    .zip(&rows)
                    .all(|(old, new)| old.key == new.key && old.tunnel == new.tunnel)
        };
        let now = SystemTime::now();
        if same {
            let mut index = 0;
            self.store.foreach(|model, _, iter| {
                if let Some(row) = rows.get(index) {
                    let store = model.downcast_ref::<gtk::ListStore>().unwrap();
                    store.set(
                        iter,
                        &[
                            (COLUMN_STATUS, &status_markup(&row.status)),
                            (COLUMN_UPTIME, &uptime(row, now)),
                            (COLUMN_TOOLTIP, &self.tooltip(row)),
                        ],
                    );
                }
                index += 1;
                false
            });
        } else {
            let selected = self.selected_key();
            self.store.clear();
            for row in &rows {
                let (listen, target, autostart) = match &row.tunnel {
                    Some(tunnel) => (
                        tunnel.listen.clone(),
                        if tunnel.kind == TunnelKind::Dynamic {
                            "(SOCKS proxy)".to_string()
                        } else {
                            tunnel.target.clone()
                        },
                        if tunnel.autostart { "yes" } else { "" }.to_string(),
                    ),
                    None => (String::new(), String::new(), String::new()),
                };
                let iter = self.store.append();
                self.store.set(
                    &iter,
                    &[
                        (COLUMN_STATUS, &status_markup(&row.status)),
                        (COLUMN_CONNECTION, &row.connection_name),
                        (COLUMN_KIND, &row.kind_label()),
                        (COLUMN_LISTEN, &listen),
                        (COLUMN_TARGET, &target),
                        (COLUMN_AUTOSTART, &autostart),
                        (COLUMN_UPTIME, &uptime(row, now)),
                        (COLUMN_KEY, &row.key),
                        (COLUMN_TOOLTIP, &self.tooltip(row)),
                    ],
                );
                if selected.as_deref() == Some(row.key.as_str()) {
                    self.tree.selection().select_iter(&iter);
                }
            }
        }
        let total = rows.len();
        let active = rows.iter().filter(|row| row.status.is_active()).count();
        let failed = rows
            .iter()
            .filter(|row| matches!(row.status, Status::Failed(_)))
            .count();
        let mut summary = if total == 0 {
            if query.trim().is_empty() {
                "No tunnels yet. Click Add to forward a port through one of your SSH connections."
                    .to_string()
            } else {
                "No tunnel matches the filter.".to_string()
            }
        } else {
            format!(
                "{} tunnel{}, {} active",
                total,
                if total == 1 { "" } else { "s" },
                active
            )
        };
        if failed > 0 {
            summary.push_str(&format!(", {} failed", failed));
        }
        self.summary.set_text(&summary);
        *self.rows.borrow_mut() = rows;
        self.update_buttons();
        self.append_log();
    }

    fn tooltip(&self, row: &TunnelRow) -> String {
        let mut lines = vec![format!("{}: {}", row.connection_name, row.status.label())];
        if let (Some(tunnel), Some(entry)) = (
            &row.tunnel,
            find_entry_by_id(&self.entries.borrow(), &row.connection_id),
        ) {
            lines.push(shell_words::join(connections::tunnel_argv(
                &entry.connection,
                tunnel,
                true,
            )));
        }
        glib::markup_escape_text(&lines.join("\n")).to_string()
    }

    fn append_log(&self) {
        let lines = tunnels::log_since(self.log_seq.get());
        let Some(last) = lines.last() else {
            return;
        };
        self.log_seq.set(last.seq);
        let entries = self.entries.borrow();
        let mut text = String::new();
        for line in &lines {
            let (id, what) = line.key.split_once('\n').unwrap_or(("", line.key.as_str()));
            let name = find_entry_by_id(&entries, id)
                .map(|entry| entry.connection.name.as_str())
                .unwrap_or(id);
            let seconds = line
                .at
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(0);
            let clock = glib::DateTime::from_unix_local(seconds)
                .and_then(|time| time.format("%H:%M:%S"))
                .map(|text| text.to_string())
                .unwrap_or_default();
            text.push_str(&format!("{}  {} [{}]  {}\n", clock, name, what, line.text));
        }
        let mut end = self.log.end_iter();
        self.log.insert(&mut end, &text);
        // After the new lines are laid out, or the view stops short of them.
        let view = self.log_view.clone();
        glib::idle_add_local_once(move || {
            let buffer = view.buffer().unwrap();
            if let Some(mark) = buffer.create_mark(None, &buffer.end_iter(), false) {
                view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
                buffer.delete_mark(&mark);
            }
        });
    }

    fn selected_key(&self) -> Option<String> {
        let (model, iter) = self.tree.selection().selected()?;
        model.value(&iter, COLUMN_KEY as i32).get::<String>().ok()
    }

    fn selected(&self) -> Option<TunnelRow> {
        let key = self.selected_key()?;
        self.rows
            .borrow()
            .iter()
            .find(|row| row.key == key)
            .cloned()
    }

    fn update_buttons(&self) {
        let row = self.selected();
        let saved = row.as_ref().is_some_and(|row| row.tunnel.is_some());
        let active = row.as_ref().is_some_and(|row| row.status.is_active());
        self.start.set_sensitive(saved && !active);
        self.stop.set_sensitive(active);
        self.edit.set_sensitive(saved);
        self.duplicate.set_sensitive(saved);
        self.delete.set_sensitive(saved);
        self.edit_connection.set_sensitive(row.is_some());
    }

    fn connection_of(&self, row: &TunnelRow) -> Option<connections::Connection> {
        find_entry_by_id(&self.entries.borrow(), &row.connection_id)
            .map(|entry| entry.connection.clone())
    }

    fn start_row(&self, row: &TunnelRow) {
        let (Some(tunnel), Some(connection)) = (&row.tunnel, self.connection_of(row)) else {
            return;
        };
        if row.status.is_active() {
            return;
        }
        crate::connections_ui::spawn_tunnel(&connection, tunnel);
    }

    fn set_selected_running(&self, running: bool) {
        let Some(row) = self.selected() else {
            return;
        };
        if running {
            self.start_row(&row);
        } else {
            tunnels::stop(&row.key);
        }
        self.notify_windows();
    }

    fn toggle_selected(&self) {
        if let Some(row) = self.selected() {
            self.set_selected_running(!row.status.is_active() && row.tunnel.is_some());
        }
    }

    fn set_all_running(&self, running: bool) {
        let rows = self.rows.borrow().clone();
        for row in &rows {
            if running {
                self.start_row(row);
            } else if row.status.is_active() {
                tunnels::stop(&row.key);
            }
        }
        self.notify_windows();
    }

    fn edit_selected_connection(&self) {
        if let (Some(row), Some(owner)) = (self.selected(), self.owner()) {
            owner.edit_connection(&row.connection_id);
            self.reload();
        }
    }

    fn add(&self, connection_id: Option<&str>) {
        let entries = self.entries.borrow().clone();
        let selected = self.selected().map(|row| row.connection_id);
        let preset = connection_id.map(str::to_string).or(selected);
        let initial = Tunnel {
            listen: next_free_listen(&entries, "8080").unwrap_or_else(|| "8080".to_string()),
            ..Tunnel::default()
        };
        if let Some((id, tunnel)) = tunnel_dialog(
            &self.window,
            "Add Tunnel",
            &entries,
            preset.as_deref(),
            initial,
        ) {
            self.save(None, &id, tunnel, false);
        }
    }

    fn edit_selected(&self) {
        let Some(row) = self.selected() else {
            return;
        };
        let (Some(original), Some(index)) = (row.tunnel.clone(), row.index) else {
            return;
        };
        let entries = self.entries.borrow().clone();
        if let Some((id, tunnel)) = tunnel_dialog(
            &self.window,
            "Edit Tunnel",
            &entries,
            Some(&row.connection_id),
            original.clone(),
        ) {
            let was_running = row.status.is_active();
            self.save(
                Some((row.connection_id.clone(), index, original)),
                &id,
                tunnel,
                was_running,
            );
        }
    }

    fn duplicate_selected(&self) {
        let Some(row) = self.selected() else {
            return;
        };
        let Some(mut copy) = row.tunnel.clone() else {
            return;
        };
        let entries = self.entries.borrow().clone();
        copy.autostart = false;
        if let Some(listen) = next_free_listen(&entries, &copy.listen) {
            copy.listen = listen;
        }
        if let Some((id, tunnel)) = tunnel_dialog(
            &self.window,
            "Duplicate Tunnel",
            &entries,
            Some(&row.connection_id),
            copy,
        ) {
            self.save(None, &id, tunnel, false);
        }
    }

    fn delete_selected(&self) {
        let Some(row) = self.selected() else {
            return;
        };
        let (Some(tunnel), Some(index)) = (row.tunnel.clone(), row.index) else {
            return;
        };
        let dialog = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::YesNo,
            &format!(
                "Delete the tunnel {} of '{}'?{}",
                tunnel.describe(),
                row.connection_name,
                if row.status.is_active() {
                    "\n\nIt is running and will be stopped."
                } else {
                    ""
                }
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
        let entries = self.entries.borrow().clone();
        let id = row.connection_id.clone();
        let result = connections::update_book(&connections::book_path(), |book| {
            let mut connection = base_connection(book, &entries, &id)?;
            take_tunnel(&mut connection, index, &tunnel)?;
            book.upsert(connection)
        });
        match result {
            Ok(()) => {
                tunnels::stop(&row.key);
                self.after_change();
            }
            Err(error) => self.message(&format!("Could not delete the tunnel: {}", error), true),
        }
    }

    /// Store a new or changed tunnel. `old` is where it was, with what it was,
    /// so a tunnel changed meanwhile by someone else is not overwritten.
    fn save(
        &self,
        old: Option<(String, usize, Tunnel)>,
        connection_id: &str,
        tunnel: Tunnel,
        restart: bool,
    ) {
        let entries = self.entries.borrow().clone();
        let result = connections::update_book(&connections::book_path(), |book| {
            if book.read_only() {
                return Err("connections.json comes from a newer Oxterm".to_string());
            }
            if let Some((old_id, index, original)) = &old {
                let mut connection = base_connection(book, &entries, old_id)?;
                take_tunnel(&mut connection, *index, original)?;
                if old_id == connection_id {
                    connection.tunnels.insert(*index, tunnel.clone());
                    return book.upsert(connection);
                }
                book.upsert(connection)?;
            }
            let mut connection = base_connection(book, &entries, connection_id)?;
            connection.tunnels.push(tunnel.clone());
            book.upsert(connection)
        });
        if let Err(error) = result {
            self.message(&format!("Could not save the tunnel: {}", error), true);
            return;
        }
        if let Some((old_id, _, original)) = &old {
            if let Some(entry) = find_entry_by_id(&entries, old_id) {
                let old_key = tunnel_key(&entry.connection, original);
                let new_key = tunnels::key(connection_id, &tunnel.describe());
                if old_key != new_key || old_id != connection_id {
                    tunnels::stop(&old_key);
                }
            }
        }
        self.after_change();
        if restart {
            if let Some(entry) = find_entry_by_id(&self.entries.borrow(), connection_id) {
                // The old process needs a moment to let go of the port.
                let connection = entry.connection.clone();
                let tunnel = tunnel.clone();
                glib::timeout_add_seconds_local_once(1, move || {
                    crate::connections_ui::spawn_tunnel(&connection, &tunnel);
                });
            }
        }
        self.notify_windows();
    }

    fn after_change(&self) {
        self.reload();
        if let Some(app) = self.window.application() {
            for window in app.windows() {
                if let Ok(main) = window.downcast::<MainWindow>() {
                    main.reload_connections_panel();
                }
            }
        }
    }

    fn show_menu(&self) {
        let Some(row) = self.selected() else {
            return;
        };
        let manager = MANAGER.with(|slot| slot.borrow().clone());
        let Some(manager) = manager else {
            return;
        };
        let menu = gtk::Menu::new();
        let add = |label: &str, sensitive: bool, action: fn(&Manager)| {
            let item = gtk::MenuItem::with_label(label);
            item.set_sensitive(sensitive);
            let weak = Rc::downgrade(&manager);
            item.connect_activate(move |_| {
                if let Some(manager) = weak.upgrade() {
                    action(&manager);
                }
            });
            menu.append(&item);
        };
        let saved = row.tunnel.is_some();
        let active = row.status.is_active();
        add("Start", saved && !active, |manager| {
            manager.set_selected_running(true)
        });
        add("Stop", active, |manager| {
            manager.set_selected_running(false)
        });
        menu.append(&gtk::SeparatorMenuItem::new());
        add("Edit...", saved, Manager::edit_selected);
        add("Duplicate...", saved, Manager::duplicate_selected);
        add("Copy ssh Command", saved, |manager| {
            if let Some(row) = manager.selected() {
                if let (Some(tunnel), Some(connection)) = (&row.tunnel, manager.connection_of(&row))
                {
                    let command =
                        shell_words::join(connections::tunnel_argv(&connection, tunnel, true));
                    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&command);
                }
            }
        });
        add(
            "Edit Connection...",
            true,
            Manager::edit_selected_connection,
        );
        menu.append(&gtk::SeparatorMenuItem::new());
        add("Delete", saved, Manager::delete_selected);
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }
}

/// The connection as the book has it, or as `~/.ssh/config` lists it when the
/// book has nothing for it yet: saving then adds Oxterm metadata to that host.
fn base_connection(
    book: &connections::Book,
    entries: &[Entry],
    id: &str,
) -> Result<connections::Connection, String> {
    book.find_by_id(id)
        .cloned()
        .or_else(|| find_entry_by_id(entries, id).map(|entry| entry.connection.clone()))
        .ok_or_else(|| "that connection no longer exists".to_string())
}

fn take_tunnel(
    connection: &mut connections::Connection,
    index: usize,
    original: &Tunnel,
) -> Result<(), String> {
    if connection.tunnels.get(index) != Some(original) {
        return Err("the tunnel was changed elsewhere; reopen the manager and try again".into());
    }
    connection.tunnels.remove(index);
    Ok(())
}

/// A listen address like `listen` on a port no saved tunnel uses and nothing
/// listens on.
fn next_free_listen(entries: &[Entry], listen: &str) -> Option<String> {
    let used: Vec<u16> = entries
        .iter()
        .flat_map(|entry| entry.connection.tunnels.iter())
        .filter_map(connections::local_port)
        .collect();
    if let Ok(port) = listen.parse::<u16>() {
        if !used.contains(&port) && connections::port_is_free(port) {
            return Some(listen.to_string());
        }
    }
    connections::next_listen(listen, &used, connections::port_is_free)
}

fn status_markup(status: &Status) -> String {
    let (color, mark) = match status {
        Status::Up | Status::Running => (Some(TUNNEL_UP_COLOR), "\u{25CF}"),
        Status::Starting => (Some(TUNNEL_STARTING_COLOR), "\u{25CC}"),
        Status::Failed(_) => (Some(FAILED_COLOR), "\u{26A0}"),
        Status::Stopped => (None, "\u{25CB}"),
    };
    let label = match status {
        Status::Failed(_) => "failed".to_string(),
        other => other.label(),
    };
    match color {
        Some(color) => format!(
            "<span foreground=\"{}\"><b>{} {}</b></span>",
            color, mark, label
        ),
        None => format!("<span alpha=\"55%\">{} {}</span>", mark, label),
    }
}

fn uptime(row: &TunnelRow, now: SystemTime) -> String {
    tunnels::started_at(&row.key)
        .and_then(|started| now.duration_since(started).ok())
        .map(tunnels::format_uptime)
        .unwrap_or_default()
}

/// The form of one tunnel. Returns the connection it belongs to and the tunnel,
/// once valid, or `None` when cancelled.
fn tunnel_dialog(
    parent: &gtk::Window,
    title: &str,
    entries: &[Entry],
    connection_id: Option<&str>,
    initial: Tunnel,
) -> Option<(String, Tunnel)> {
    let mut candidates: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.connection.protocol == Protocol::Ssh)
        .collect();
    candidates.sort_by_key(|entry| entry.connection.name.to_lowercase());
    if candidates.is_empty() {
        let dialog = gtk::MessageDialog::new(
            Some(parent),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Info,
            gtk::ButtonsType::Close,
            "A tunnel runs through an SSH connection. Add one in the connections panel first.",
        );
        dialog.run();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        return None;
    }
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("_Cancel", gtk::ResponseType::Cancel),
            ("_Save", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    dialog.set_default_width(520);
    let area = dialog.content_area();
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    area.pack_start(&grid, true, true, 0);
    let mut row = 0;
    let mut add_row = |label: &str, widget: &gtk::Widget| {
        let text = gtk::Label::with_mnemonic(label);
        text.set_xalign(1.0);
        text.set_mnemonic_widget(Some(widget));
        widget.set_hexpand(true);
        grid.attach(&text, 0, row, 1, 1);
        grid.attach(widget, 1, row, 1, 1);
        row += 1;
    };

    let connection = gtk::ComboBoxText::new();
    for entry in &candidates {
        connection.append(Some(&entry.connection.id), &entry.connection.name);
    }
    let preset = connection_id
        .filter(|id| candidates.iter().any(|entry| entry.connection.id == *id))
        .unwrap_or(&candidates[0].connection.id);
    connection.set_active_id(Some(preset));
    add_row("_Connection", connection.upcast_ref());

    let kind = gtk::ComboBoxText::new();
    kind.append(
        Some("local"),
        "Local (-L): a local port reaches a host behind the server",
    );
    kind.append(
        Some("remote"),
        "Remote (-R): a port on the server reaches this side",
    );
    kind.append(
        Some("dynamic"),
        "SOCKS proxy (-D): a local port proxies anything",
    );
    kind.set_active_id(Some(match initial.kind {
        TunnelKind::Local => "local",
        TunnelKind::Remote => "remote",
        TunnelKind::Dynamic => "dynamic",
    }));
    add_row("_Type", kind.upcast_ref());

    let listen = gtk::Entry::new();
    listen.set_text(&initial.listen);
    listen.set_placeholder_text(Some("[bind:]port, e.g. 8080 or 127.0.0.1:8080"));
    listen.set_activates_default(true);
    add_row("_Listen on", listen.upcast_ref());

    let target = gtk::Entry::new();
    target.set_text(&initial.target);
    target.set_placeholder_text(Some("host:port, e.g. localhost:5432"));
    target.set_activates_default(true);
    add_row("_Forward to", target.upcast_ref());

    let autostart =
        gtk::CheckButton::with_mnemonic("Start _automatically when the connection opens");
    autostart.set_active(initial.autostart);
    grid.attach(&autostart, 1, row, 1, 1);
    row += 1;

    let preview = gtk::Label::new(None);
    preview.set_xalign(0.0);
    preview.set_line_wrap(true);
    preview.set_line_wrap_mode(pango::WrapMode::WordChar);
    preview.set_selectable(true);
    preview.style_context().add_class("dim-label");
    grid.attach(&preview, 0, row, 2, 1);
    row += 1;
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.set_line_wrap(true);
    grid.attach(&error, 0, row, 2, 1);

    let read = {
        let kind = kind.clone();
        let listen = listen.clone();
        let target = target.clone();
        let autostart = autostart.clone();
        let initial = initial.clone();
        move || -> Tunnel {
            let kind = match kind.active_id().as_deref() {
                Some("remote") => TunnelKind::Remote,
                Some("dynamic") => TunnelKind::Dynamic,
                _ => TunnelKind::Local,
            };
            Tunnel {
                kind,
                listen: listen.text().trim().to_string(),
                target: if kind == TunnelKind::Dynamic {
                    String::new()
                } else {
                    target.text().trim().to_string()
                },
                autostart: autostart.is_active(),
                ..initial.clone()
            }
        }
    };
    let candidates_owned: Vec<connections::Connection> = candidates
        .iter()
        .map(|entry| entry.connection.clone())
        .collect();
    let update: Rc<dyn Fn()> = {
        let read = read.clone();
        let connection = connection.clone();
        let target = target.clone();
        let kind = kind.clone();
        let preview = preview.clone();
        let candidates = candidates_owned.clone();
        Rc::new(move || {
            target.set_sensitive(kind.active_id().as_deref() != Some("dynamic"));
            let tunnel = read();
            let chosen = connection.active_id();
            let text = candidates
                .iter()
                .find(|candidate| Some(candidate.id.as_str()) == chosen.as_deref())
                .filter(|_| validate_tunnel(&tunnel).is_ok())
                .map(|candidate| {
                    shell_words::join(connections::tunnel_argv(candidate, &tunnel, true))
                })
                .unwrap_or_default();
            preview.set_text(&text);
        })
    };
    {
        let update = update.clone();
        connection.connect_changed(move |_| update());
    }
    {
        let update = update.clone();
        kind.connect_changed(move |_| update());
    }
    for field in [&listen, &target] {
        let update = update.clone();
        field.connect_changed(move |_| update());
    }
    update();
    dialog.show_all();
    listen.grab_focus();

    let original_key = connection_id.map(|id| tunnels::key(id, &initial.describe()));
    let result = loop {
        if dialog.run() != gtk::ResponseType::Ok {
            break None;
        }
        let tunnel = read();
        let Some(id) = connection.active_id().map(|id| id.to_string()) else {
            error.set_markup(&error_markup("Choose a connection."));
            continue;
        };
        if let Err(message) = validate_tunnel(&tunnel) {
            error.set_markup(&error_markup(&message));
            continue;
        }
        let key = tunnels::key(&id, &tunnel.describe());
        let clash = candidates_owned
            .iter()
            .find(|candidate| candidate.id == id)
            .is_some_and(|candidate| {
                candidate.tunnels.iter().any(|other| {
                    tunnels::key(&id, &other.describe()) == key
                        && original_key.as_deref() != Some(key.as_str())
                })
            });
        if clash {
            error.set_markup(&error_markup(
                "This connection already has the same tunnel.",
            ));
            continue;
        }
        break Some((id, tunnel));
    };
    // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
    unsafe {
        dialog.destroy();
    }
    result
}

fn error_markup(text: &str) -> String {
    format!(
        "<span foreground=\"{}\">{}</span>",
        FAILED_COLOR,
        glib::markup_escape_text(text)
    )
}
