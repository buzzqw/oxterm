//! Connections side panel: search box and a tree of saved SSH destinations.
//!
//! Like the notes panel it is built once per main window, the first time it is
//! shown, so a window that never opens it pays nothing. It only displays what
//! `headless::connections` computes; the behaviour lives in `connections_ui`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk::gdk;
use gtk::prelude::*;

use crate::headless::connections::{Entry, Kind, Node, Protocol, UsageMap};
use crate::headless::reachability;

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
pub(crate) const ONLINE_COLOR: &str = "#2e9d4f";
pub(crate) const OFFLINE_COLOR: &str = "#c62828";
pub(crate) const UNKNOWN_COLOR: &str = "#6b7280";

/// DnD target shared with the terminal drop target. The row id is sent as raw
/// bytes; `text/plain` is offered as a fallback for other applications.
pub(crate) const DRAG_TARGET: &str = "oxterm-connection-id";

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
    /// Last reachability probe for this connection, when one has run.
    pub reach: Option<reachability::Reachability>,
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

/// Colour and glyph of a protocol's chip. Each protocol has its own colour and
/// mark, so a list of otherwise identical names still reads at a glance: the
/// colour says the family, the glyph the exact protocol.
fn protocol_style(protocol: Protocol) -> (&'static str, &'static str) {
    match protocol {
        Protocol::Ssh => ("#2b6cb0", ">_"),
        Protocol::Mosh => ("#2c7a7b", "\u{2248}"),
        Protocol::Telnet => ("#4a5568", "\u{2301}"),
        Protocol::Serial => ("#8b5e34", "\u{2393}"),
        Protocol::Sftp => ("#b7791f", "\u{21C5}"),
        Protocol::Ftp => ("#975a16", "\u{21C5}"),
        Protocol::Docker => ("#1d6fb8", "\u{25A3}"),
        Protocol::Kubectl => ("#4c51bf", "\u{2638}"),
        Protocol::AwsSsm => ("#c05621", "\u{2601}"),
        Protocol::Command => ("#4a5568", "$"),
        Protocol::Rdp => ("#6b46c1", "\u{25AD}"),
        Protocol::Vnc => ("#2c7a7b", "\u{25AD}"),
        Protocol::Spice => ("#b83232", "\u{25AD}"),
    }
}

/// A compact, coloured chip naming the protocol.
fn protocol_chip(protocol: Protocol) -> String {
    let (color, glyph) = protocol_style(protocol);
    let tag = if protocol.tag().is_empty() {
        protocol.label()
    } else {
        protocol.tag()
    };
    format!(
        "<span background=\"{}\" foreground=\"#ffffff\"><small><b>\u{2009}{} {}\u{2009}</b></small></span>",
        color, glyph, tag
    )
}

/// The section marks make Favorites, Recent and the hosts from `~/.ssh/config`
/// look different from the user's own folders.
fn group_icon(title: &str) -> Option<(&'static str, &'static str)> {
    match title {
        "Favorites" => Some(("\u{2605}", "#b7791f")),
        "Recent" => Some(("\u{21BB}", "#2b6cb0")),
        "~/.ssh/config" => Some(("\u{2261}", "#6b7280")),
        "Ungrouped" => Some(("\u{25A4}", "#6b7280")),
        _ => None,
    }
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
    /// Shows only connections that answered the last reachability probe.
    pub reachable_only: gtk::ToggleButton,
    pub status: gtk::Label,
    pub data: RefCell<Option<PanelData>>,
    /// Last probe per connection id, written by the worker in `connections_ui`.
    pub reach: RefCell<HashMap<String, reachability::Reachability>>,
    /// Timer driving the probes, removed when the panel is hidden or destroyed.
    pub reach_timer: RefCell<Option<glib::SourceId>>,
    /// Avoids overlapping batches when a probe takes longer than one interval.
    pub reach_busy: Cell<bool>,
    /// Set when the window is gone, so an in-flight batch drops its results.
    pub reach_stop: Arc<AtomicBool>,
    store: gtk::TreeStore,
    collapsed: RefCell<HashSet<String>>,
    /// True while `populate` runs: the expand signals it causes are not choices
    /// the user made.
    rebuilding: Cell<bool>,
}

impl ConnectionsPanel {
    pub fn build() -> ConnectionsPanel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(180);
        root.set_no_show_all(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.set_margin_start(8);
        header.set_margin_end(8);
        let title = gtk::Label::new(None);
        title.set_markup("<b>Connections</b>");
        title.set_halign(gtk::Align::Start);
        title.set_ellipsize(pango::EllipsizeMode::End);
        header.pack_start(&title, true, true, 0);
        let icon_button = |icon: &str, tooltip: &str| {
            let button = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Menu);
            button.set_tooltip_text(Some(tooltip));
            button
        };
        let close = icon_button("window-close-symbolic", "Hide the connections panel");
        header.pack_end(&close, false, false, 0);
        let add = icon_button("list-add-symbolic", "Create a new connection");
        header.pack_end(&add, false, false, 0);
        let tunnels = icon_button("network-workgroup-symbolic", "Open the tunnel manager");
        header.pack_end(&tunnels, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_row.set_margin_start(8);
        search_row.set_margin_end(8);
        search_row.set_margin_bottom(6);
        let search = gtk::SearchEntry::new();
        search.set_width_chars(6);
        search.set_placeholder_text(Some("Search name, host, group, tag, notes"));
        search_row.pack_start(&search, true, true, 0);
        let live_only = gtk::ToggleButton::with_label("Active");
        live_only.set_tooltip_text(Some("Show only connections with open tabs or tunnels"));
        search_row.pack_start(&live_only, false, false, 0);
        let reachable_only = gtk::ToggleButton::with_label("Reachable");
        reachable_only.set_tooltip_text(Some("Show only connections that answered the last probe"));
        search_row.pack_start(&reachable_only, false, false, 0);
        root.pack_start(&search_row, false, false, 0);

        let store =
            gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
        let tree = gtk::TreeView::with_model(&store);
        tree.set_headers_visible(false);
        tree.set_enable_search(false);
        tree.set_activate_on_single_click(false);
        tree.selection().set_mode(gtk::SelectionMode::Multiple);
        tree.set_tooltip_column(COLUMN_TOOLTIP as i32);
        // Drag a row (or several) onto a terminal to connect there. Terminals
        // advertise the same private target; plain text is a fallback.
        let row_target = gtk::TargetEntry::new(DRAG_TARGET, gtk::TargetFlags::SAME_APP, 0);
        let text_target = gtk::TargetEntry::new("text/plain", gtk::TargetFlags::SAME_APP, 1);
        tree.drag_source_set(
            gdk::ModifierType::BUTTON1_MASK,
            &[row_target, text_target],
            gdk::DragAction::MOVE | gdk::DragAction::COPY,
        );
        tree.connect_drag_data_get(|tree, _, data, _, _| {
            let Some(model) = tree.model() else {
                return;
            };
            let id = tree
                .cursor()
                .0
                .and_then(|path| model.iter(&path))
                .and_then(|iter| {
                    model
                        .value(&iter, COLUMN_ID as i32)
                        .get::<String>()
                        .ok()
                        .filter(|id| !id.is_empty())
                })
                .or_else(|| {
                    // `selected()` is None in Multiple mode: use the rows API.
                    let (paths, model) = tree.selection().selected_rows();
                    let iter = model.iter(paths.first()?)?;
                    model
                        .value(&iter, COLUMN_ID as i32)
                        .get::<String>()
                        .ok()
                        .filter(|id| !id.is_empty())
                });
            if let Some(id) = id {
                data.set(&data.target(), 8, id.as_bytes());
            }
        });
        let renderer = gtk::CellRendererText::new();
        renderer.set_ellipsize(pango::EllipsizeMode::End);
        renderer.set_padding(4, 3);
        let column = gtk::TreeViewColumn::new();
        column.set_expand(true);
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "markup", COLUMN_MARKUP as i32);
        tree.append_column(&column);

        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        // An always-visible scrollbar next to the splitter reads as a second
        // separator; the overlay shows only while the list is being scrolled.
        scroll.set_overlay_scrolling(true);
        scroll.add(&tree);
        root.pack_start(&scroll, true, true, 0);

        let status = gtk::Label::new(None);
        status.set_no_show_all(true);
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
            reachable_only,
            status,
            data: RefCell::new(None),
            reach: RefCell::new(HashMap::new()),
            reach_timer: RefCell::new(None),
            reach_busy: Cell::new(false),
            reach_stop: Arc::new(AtomicBool::new(false)),
            store,
            collapsed: RefCell::new(HashSet::new()),
            rebuilding: Cell::new(false),
        }
    }

    /// Stop periodic probes and make any in-flight batch drop its results.
    pub fn stop_reachability_timer(&self) {
        if let Some(source) = self.reach_timer.borrow_mut().take() {
            source.remove();
        }
        self.reach_stop.store(true, Ordering::SeqCst);
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
    pub fn populate<'a>(
        &self,
        nodes: &[Node],
        entries: &[Entry],
        decorate: &dyn Fn(&Entry) -> &'a RowDecoration,
    ) {
        let selected = self.selected_targets();
        self.rebuilding.set(true);
        self.tree.set_model(None::<&gtk::TreeStore>);
        self.store.clear();
        for node in nodes {
            self.append(None, node, entries, decorate);
        }
        self.tree.set_model(Some(&self.store));
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
        for (group, id) in selected {
            self.select_target(&group, &id);
        }
    }

    fn append<'a>(
        &self,
        parent: Option<&gtk::TreeIter>,
        node: &Node,
        entries: &[Entry],
        decorate: &dyn Fn(&Entry) -> &'a RowDecoration,
    ) -> GroupActivity {
        match node {
            Node::Group { title, children } => {
                let iter = self.store.append(parent);
                let mut activity = GroupActivity::default();
                for child in children {
                    activity.add(self.append(Some(&iter), child, entries, decorate));
                }
                let icon = group_icon(title)
                    .map(|(glyph, color)| {
                        format!("<span foreground=\"{}\">{}</span> ", color, glyph)
                    })
                    .unwrap_or_default();
                let markup = format!(
                    "{icon}<b>{}</b> <span alpha=\"55%\"><small>({})</small></span>{}",
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
                        (COLUMN_MARKUP, &row_markup(entry, decoration)),
                        (COLUMN_ID, &entry.connection.id),
                        (COLUMN_TOOLTIP, &row_tooltip(entry, decoration)),
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

    /// The primary selection, for a single click or a keyboard action.
    ///
    /// `TreeSelection::selected()` returns `None` in `SelectionMode::Multiple`
    /// (GTK asserts on it), so take the first selected path instead. Without
    /// this, F2/Delete/Enter silently stopped working when multi-select was
    /// introduced.
    pub fn selected_id(&self) -> Option<String> {
        let (paths, _) = self.tree.selection().selected_rows();
        paths.first().and_then(|path| self.id_at(path))
    }

    /// Ids of every selected connection row. Group rows have an empty id and
    /// are skipped; a host listed in two groups is returned only once.
    pub fn selected_ids(&self) -> Vec<String> {
        let (paths, _) = self.tree.selection().selected_rows();
        self.ids_for_paths(&paths)
    }

    /// Ids of the connections at `paths`, skipping group rows and duplicates.
    pub fn ids_for_paths(&self, paths: &[gtk::TreePath]) -> Vec<String> {
        let mut ids = Vec::new();
        for path in paths {
            if let Some(id) = self.id_at(path) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
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

    /// Group and id of every selected row, so a rebuild can restore the whole
    /// multi-selection instead of only its primary row.
    fn selected_targets(&self) -> Vec<(String, String)> {
        let (paths, model) = self.tree.selection().selected_rows();
        let mut targets = Vec::new();
        for path in paths {
            let Some(iter) = model.iter(&path) else {
                continue;
            };
            let id = model
                .value(&iter, COLUMN_ID as i32)
                .get::<String>()
                .unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let mut parent = path.clone();
            let group = if parent.up() && parent.depth() > 0 {
                self.group_key(&parent)
            } else {
                String::new()
            };
            targets.push((group, id));
        }
        targets
    }

    fn path_visible(&self, path: &gtk::TreePath) -> bool {
        let mut parent = path.clone();
        while parent.up() && parent.depth() > 0 {
            if !self.tree.row_expanded(&parent) {
                return false;
            }
        }
        true
    }

    /// Select `id` in `group` when still visible there, or the first visible
    /// copy of `id` elsewhere in the tree.
    fn select_target(&self, group: &str, id: &str) {
        let mut exact = None;
        let mut fallback = None;
        self.store.foreach(|model, path, iter| {
            let matches = model
                .value(iter, COLUMN_ID as i32)
                .get::<String>()
                .is_ok_and(|value| value == id);
            if !matches || !self.path_visible(path) {
                return false;
            }
            let mut parent = path.clone();
            let row_group = if parent.up() && parent.depth() > 0 {
                self.group_key(&parent)
            } else {
                String::new()
            };
            if row_group == group {
                exact = Some(path.clone());
                return true;
            }
            if fallback.is_none() {
                fallback = Some(path.clone());
            }
            false
        });
        if let Some(path) = exact.or(fallback) {
            self.tree.selection().select_path(&path);
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
    let connection = &entry.connection;
    let name = glib::markup_escape_text(&connection.name);
    let open = decoration.open_tabs > 0;
    // A dot in front of every row, filled and green when the connection is open:
    // the state reads down one column, whatever the length of the names.
    let mut markup = if open {
        format!("<span foreground=\"{}\">\u{25CF}</span> ", OPEN_COLOR)
    } else {
        "<span alpha=\"35%\">\u{25CB}</span> ".to_string()
    };
    // A reachability badge next to the state dot: green answered, red refused
    // or timed out, grey not probed yet.
    if let Some(reach) = &decoration.reach {
        let (color, label) = match reach.state {
            reachability::ProbeState::Online => (ONLINE_COLOR, "Online"),
            reachability::ProbeState::Offline => (OFFLINE_COLOR, "Offline"),
            reachability::ProbeState::Unknown => (UNKNOWN_COLOR, "Unknown"),
        };
        markup.push_str(&badge(color, label));
    }
    // A chip in a fixed column says what kind of connection this is before the
    // name is read, so two similarly named hosts are not mistaken for each other.
    markup.push_str(&protocol_chip(connection.protocol));
    markup.push(' ');
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
    // The address follows on the same line, dim and after the name, so it costs
    // no height and the narrow panel ellipsizes it before the name.
    if let Some(detail) = row_detail(entry) {
        markup.push_str("  ");
        markup.push_str(&detail);
    }
    markup
}

/// Where the entry actually goes, in dim text after its name.
fn row_detail(entry: &Entry) -> Option<String> {
    use crate::headless::connections::Origin;
    let connection = &entry.connection;
    let mut parts = Vec::new();
    if entry.origin == Origin::Orphan {
        parts.push("not in ~/.ssh/config".to_string());
    } else if connection.kind == Kind::Manual {
        // An alias has no address of its own here: the name is the destination,
        // and "~/.ssh/config" would only repeat the group it is filed under.
        let summary = connection.summary();
        if !summary.is_empty() {
            parts.push(glib::markup_escape_text(&summary).to_string());
        }
    }
    if !connection.tags.is_empty() {
        parts.push(
            connection
                .tags
                .iter()
                .map(|tag| format!("#{}", glib::markup_escape_text(tag)))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "<small><span alpha=\"60%\">{}</span></small>",
        parts.join("  \u{00B7}  ")
    ))
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
    if let Some(reach) = &decoration.reach {
        lines.extend(
            reachability::tooltip_lines(reach)
                .lines()
                .map(|line| line.to_string()),
        );
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
