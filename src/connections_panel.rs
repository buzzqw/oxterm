//! Connections side panel: search box and a tree of saved SSH destinations.
//!
//! Like the notes panel it is built once per main window, the first time it is
//! shown, so a window that never opens it pays nothing. It only displays what
//! `headless::connections` computes; the behaviour lives in `connections_ui`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use gtk::prelude::*;

use crate::headless::connections::{Entry, Node, UsageMap};

const COLUMN_MARKUP: u32 = 0;
const COLUMN_ID: u32 = 1;
const COLUMN_TOOLTIP: u32 = 2;

/// What the rows are drawn from, read once when the panel is reloaded. Searching
/// filters this, so a key press reads no files.
pub struct PanelData {
    pub entries: Vec<Entry>,
    pub usage: UsageMap,
    pub status: Option<String>,
}

/// Colours of the state marks. Saturated enough to read on light and dark
/// themes; the badges carry white text on them.
pub(crate) const OPEN_COLOR: &str = "#2e9d4f";
pub(crate) const TUNNEL_UP_COLOR: &str = "#1d6fb8";
pub(crate) const TUNNEL_STARTING_COLOR: &str = "#b26a00";
pub(crate) const FAILED_COLOR: &str = "#c62828";

/// What a row shows besides the entry itself.
#[derive(Clone, Debug, Default)]
pub struct RowDecoration {
    pub open_tabs: usize,
    /// Tunnels that listen or run.
    pub active_tunnels: usize,
    /// Tunnels whose `ssh` runs but whose port does not answer yet.
    pub starting_tunnels: usize,
    pub failed_tunnels: usize,
    pub alert_color: Option<String>,
    /// One line per tunnel with its state, for the tooltip.
    pub tunnel_lines: Vec<String>,
}

impl RowDecoration {
    /// Whether the connection has something running, or something that failed.
    pub fn is_live(&self) -> bool {
        self.open_tabs > 0
            || self.active_tunnels > 0
            || self.starting_tunnels > 0
            || self.failed_tunnels > 0
    }
}

/// Counts over a group, so a folded group still shows what runs inside.
#[derive(Clone, Copy, Debug, Default)]
struct GroupActivity {
    open: usize,
    tunnels: usize,
    starting: usize,
    failed: usize,
}

impl GroupActivity {
    fn add(&mut self, other: GroupActivity) {
        self.open += other.open;
        self.tunnels += other.tunnels;
        self.starting += other.starting;
        self.failed += other.failed;
    }
}

/// A coloured pill: visible at a glance, also in a long list.
fn badge(color: &str, text: &str) -> String {
    format!(
        "<span background=\"{}\" foreground=\"#ffffff\"><small><b>\u{2009}{}\u{2009}</b></small></span>",
        color, text
    )
}

fn activity_badges(open: usize, tunnels: usize, starting: usize, failed: usize) -> String {
    let mut markup = String::new();
    if open > 0 {
        markup.push_str(&format!(
            "  <span foreground=\"{}\"><small><b>\u{25CF} {}</b></small></span>",
            OPEN_COLOR, open
        ));
    }
    if tunnels > 0 {
        markup.push_str("  ");
        markup.push_str(&badge(TUNNEL_UP_COLOR, &format!("\u{21C4} {}", tunnels)));
    }
    if starting > 0 {
        markup.push_str("  ");
        markup.push_str(&badge(
            TUNNEL_STARTING_COLOR,
            &format!("\u{21C4} {}\u{2026}", starting),
        ));
    }
    if failed > 0 {
        markup.push_str("  ");
        markup.push_str(&badge(FAILED_COLOR, &format!("\u{26A0} {}", failed)));
    }
    markup
}

pub struct ConnectionsPanel {
    pub root: gtk::Box,
    pub search: gtk::SearchEntry,
    pub tree: gtk::TreeView,
    pub add: gtk::Button,
    pub close: gtk::Button,
    /// Opens the tunnel manager.
    pub tunnels: gtk::Button,
    /// Shows only connections with open tabs or tunnels.
    pub live_only: gtk::ToggleButton,
    pub status: gtk::Label,
    pub data: RefCell<Option<PanelData>>,
    store: gtk::TreeStore,
    collapsed: RefCell<HashSet<String>>,
    /// True while `populate` runs: the expand signals it causes are not choices
    /// the user made.
    rebuilding: Cell<bool>,
}

impl ConnectionsPanel {
    pub fn build() -> ConnectionsPanel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(320);
        root.set_no_show_all(true);
        root.style_context().add_class("oxterm-connections-panel");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.set_margin_start(8);
        header.set_margin_end(8);
        let title = gtk::Label::new(None);
        title.set_markup("<b>Connections</b>");
        title.set_halign(gtk::Align::Start);
        header.pack_start(&title, false, false, 0);
        let close = gtk::Button::with_label("Close");
        close.set_tooltip_text(Some("Hide the connections panel"));
        header.pack_end(&close, false, false, 0);
        let add = gtk::Button::with_label("New");
        add.set_tooltip_text(Some("Create a new connection"));
        header.pack_end(&add, false, false, 0);
        let tunnels = gtk::Button::with_label("Tunnels");
        tunnels.set_tooltip_text(Some("Open the tunnel manager"));
        header.pack_end(&tunnels, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_row.set_margin_start(8);
        search_row.set_margin_end(8);
        search_row.set_margin_bottom(6);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search name, host, group, tag, notes"));
        search_row.pack_start(&search, true, true, 0);
        let live_only = gtk::ToggleButton::with_label("Active");
        live_only.set_tooltip_text(Some("Show only connections with open tabs or tunnels"));
        search_row.pack_start(&live_only, false, false, 0);
        root.pack_start(&search_row, false, false, 0);

        let store =
            gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
        let tree = gtk::TreeView::with_model(&store);
        tree.set_headers_visible(false);
        tree.set_enable_search(false);
        tree.set_activate_on_single_click(false);
        tree.set_tooltip_column(COLUMN_TOOLTIP as i32);
        let renderer = gtk::CellRendererText::new();
        renderer.set_ellipsize(pango::EllipsizeMode::End);
        let column = gtk::TreeViewColumn::new();
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "markup", COLUMN_MARKUP as i32);
        tree.append_column(&column);

        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.add(&tree);
        root.pack_start(&scroll, true, true, 0);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_top(4);
        status.set_margin_bottom(4);
        status.set_line_wrap(true);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        ConnectionsPanel {
            root,
            search,
            tree,
            add,
            close,
            tunnels,
            live_only,
            status,
            data: RefCell::new(None),
            store,
            collapsed: RefCell::new(HashSet::new()),
            rebuilding: Cell::new(false),
        }
    }

    /// Remember which groups the user folded, so a refresh does not undo it.
    pub fn connect_tree_memory(panel: &std::rc::Rc<ConnectionsPanel>) {
        let weak = std::rc::Rc::downgrade(panel);
        panel.tree.connect_row_collapsed(move |_, _, path| {
            if let Some(panel) = weak.upgrade() {
                if !panel.rebuilding.get() {
                    let key = panel.group_key(path);
                    panel.collapsed.borrow_mut().insert(key);
                }
            }
        });
        let weak = std::rc::Rc::downgrade(panel);
        panel.tree.connect_row_expanded(move |_, _, path| {
            if let Some(panel) = weak.upgrade() {
                if !panel.rebuilding.get() {
                    let key = panel.group_key(path);
                    panel.collapsed.borrow_mut().remove(&key);
                }
            }
        });
    }

    fn group_key(&self, path: &gtk::TreePath) -> String {
        let mut titles = Vec::new();
        let mut cursor = path.clone();
        while cursor.depth() > 0 {
            if let Some(iter) = self.store.iter(&cursor) {
                titles.push(
                    self.store
                        .value(&iter, COLUMN_TOOLTIP as i32)
                        .get::<String>()
                        .unwrap_or_default(),
                );
            }
            if !cursor.up() {
                break;
            }
        }
        titles.reverse();
        titles.join("/")
    }

    /// Replace the rows. Groups stay folded the way the user left them, and the
    /// selection follows its connection through the rebuild.
    pub fn populate(
        &self,
        nodes: &[Node],
        entries: &[Entry],
        decorate: &dyn Fn(&Entry) -> RowDecoration,
    ) {
        let selected = self.selected_id();
        self.rebuilding.set(true);
        self.store.clear();
        for node in nodes {
            self.append(None, node, entries, decorate);
        }
        self.tree.expand_all();
        let mut paths = Vec::new();
        self.store.foreach(|model, path, iter| {
            if model.iter_has_child(iter) {
                paths.push(path.clone());
            }
            false
        });
        // Deepest first, or folding a parent would hide its children's state.
        paths.reverse();
        for path in paths {
            if self.collapsed.borrow().contains(&self.group_key(&path)) {
                self.tree.collapse_row(&path);
            }
        }
        self.rebuilding.set(false);
        if let Some(id) = selected {
            self.select_id(&id);
        }
    }

    fn append(
        &self,
        parent: Option<&gtk::TreeIter>,
        node: &Node,
        entries: &[Entry],
        decorate: &dyn Fn(&Entry) -> RowDecoration,
    ) -> GroupActivity {
        match node {
            Node::Group { title, children } => {
                let iter = self.store.append(parent);
                let mut activity = GroupActivity::default();
                for child in children {
                    activity.add(self.append(Some(&iter), child, entries, decorate));
                }
                let markup = format!(
                    "<b>{}</b> <small>({})</small>{}",
                    glib::markup_escape_text(title),
                    leaf_count(children),
                    activity_badges(
                        activity.open,
                        activity.tunnels,
                        activity.starting,
                        activity.failed
                    )
                );
                self.store.set(
                    &iter,
                    &[
                        (COLUMN_MARKUP, &markup),
                        (COLUMN_ID, &String::new()),
                        // Also the key that remembers the fold state.
                        (COLUMN_TOOLTIP, &glib::markup_escape_text(title).to_string()),
                    ],
                );
                activity
            }
            Node::Leaf(index) => {
                let Some(entry) = entries.get(*index) else {
                    return GroupActivity::default();
                };
                let decoration = decorate(entry);
                let iter = self.store.append(parent);
                self.store.set(
                    &iter,
                    &[
                        (COLUMN_MARKUP, &row_markup(entry, &decoration)),
                        (COLUMN_ID, &entry.connection.id),
                        (COLUMN_TOOLTIP, &row_tooltip(entry, &decoration)),
                    ],
                );
                GroupActivity {
                    open: usize::from(decoration.open_tabs > 0),
                    tunnels: decoration.active_tunnels,
                    starting: decoration.starting_tunnels,
                    failed: decoration.failed_tunnels,
                }
            }
        }
    }

    pub fn selected_id(&self) -> Option<String> {
        let (model, iter) = self.tree.selection().selected()?;
        let id = model
            .value(&iter, COLUMN_ID as i32)
            .get::<String>()
            .unwrap_or_default();
        (!id.is_empty()).then_some(id)
    }

    /// The connection at `path`; `None` for a group row.
    pub fn id_at(&self, path: &gtk::TreePath) -> Option<String> {
        let iter = self.store.iter(path)?;
        let id = self
            .store
            .value(&iter, COLUMN_ID as i32)
            .get::<String>()
            .unwrap_or_default();
        (!id.is_empty()).then_some(id)
    }

    /// Every connection below a group row, once each: a host can be listed under
    /// Favorites and its own group, and opening a group should not open it twice.
    pub fn ids_under(&self, path: &gtk::TreePath) -> Vec<String> {
        let mut ids = Vec::new();
        self.store.foreach(|model, row, iter| {
            if row.is_descendant(path) {
                let id = model
                    .value(iter, COLUMN_ID as i32)
                    .get::<String>()
                    .unwrap_or_default();
                if !id.is_empty() && !ids.contains(&id) {
                    ids.push(id);
                }
            }
            false
        });
        ids
    }

    /// Select `id`, unless it is a row the search or a fold currently hides.
    fn select_id(&self, id: &str) {
        let mut found = None;
        self.store.foreach(|model, path, iter| {
            let matches = model
                .value(iter, COLUMN_ID as i32)
                .get::<String>()
                .is_ok_and(|value| value == id);
            if matches {
                found = Some(path.clone());
            }
            matches
        });
        if let Some(path) = found {
            let visible = {
                let mut parent = path.clone();
                let mut open = true;
                while parent.up() && parent.depth() > 0 {
                    open &= self.tree.row_expanded(&parent);
                }
                open
            };
            if visible {
                self.tree.selection().select_path(&path);
            }
        }
    }

    /// Select the first connection, for Enter in the search box.
    pub fn select_first_connection(&self) -> Option<String> {
        let mut found = None;
        self.store.foreach(|model, path, iter| {
            let id = model
                .value(iter, COLUMN_ID as i32)
                .get::<String>()
                .unwrap_or_default();
            if id.is_empty() {
                return false;
            }
            found = Some((path.clone(), id));
            true
        });
        let (path, id) = found?;
        self.tree.expand_to_path(&path);
        self.tree.selection().select_path(&path);
        Some(id)
    }
}

fn leaf_count(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|node| match node {
            Node::Leaf(_) => 1,
            Node::Group { children, .. } => leaf_count(children),
        })
        .sum()
}

fn row_markup(entry: &Entry, decoration: &RowDecoration) -> String {
    use crate::headless::connections::Origin;
    let name = glib::markup_escape_text(&entry.connection.name);
    let open = decoration.open_tabs > 0;
    // A dot in front of every row, filled and green when the connection is open:
    // the state reads down one column, whatever the length of the names.
    let mut markup = if open {
        format!("<span foreground=\"{}\">\u{25CF}</span> ", OPEN_COLOR)
    } else {
        "<span alpha=\"35%\">\u{25CB}</span> ".to_string()
    };
    let name = if open {
        format!("<b>{}</b>", name)
    } else {
        name.to_string()
    };
    match &decoration.alert_color {
        Some(color) => markup.push_str(&format!(
            "<span foreground=\"{}\"><b>{}</b></span>",
            color, name
        )),
        None => markup.push_str(&name),
    }
    let tag = entry.connection.protocol.tag();
    if !tag.is_empty() {
        markup.push_str(&format!("  <small><b>{}</b></small>", tag));
    }
    // State first: the narrow panel ellipsizes the end of the row, and the
    // address is also in the tooltip.
    markup.push_str(&activity_badges(
        if decoration.open_tabs > 1 {
            decoration.open_tabs
        } else {
            0
        },
        decoration.active_tunnels,
        decoration.starting_tunnels,
        decoration.failed_tunnels,
    ));
    if entry.origin == Origin::Orphan {
        markup.push_str(" <small><i>(not in ~/.ssh/config)</i></small>");
    } else if entry.connection.kind == crate::headless::connections::Kind::Manual {
        markup.push_str(&format!(
            "  <small>{}</small>",
            glib::markup_escape_text(&entry.connection.summary())
        ));
    }
    markup
}

fn row_tooltip(entry: &Entry, decoration: &RowDecoration) -> String {
    let connection = &entry.connection;
    // A desktop's preview looks for its viewer on the PATH: too much for every row
    // of a list redrawn on each key press.
    let mut lines = vec![if connection.protocol.is_desktop() {
        format!("{}: {}", connection.protocol.label(), connection.summary())
    } else {
        connection.command_preview()
    }];
    if !connection.group.is_empty() {
        lines.push(format!("Group: {}", connection.group));
    }
    if !connection.tags.is_empty() {
        lines.push(format!("Tags: {}", connection.tags.join(", ")));
    }
    if !connection.notes.is_empty() {
        lines.push(connection.notes.clone());
    }
    match decoration.open_tabs {
        0 => {}
        1 => lines.push("Open in 1 tab".to_string()),
        count => lines.push(format!("Open in {} tabs", count)),
    }
    if !decoration.tunnel_lines.is_empty() {
        lines.push("Tunnels:".to_string());
        lines.extend(
            decoration
                .tunnel_lines
                .iter()
                .map(|line| format!("  {}", line)),
        );
    }
    glib::markup_escape_text(&lines.join("\n")).to_string()
}
