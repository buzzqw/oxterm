//! Catalogue of saved workspaces: named, coloured sets of connections.
//!
//! The older context-menu actions save a selection from the connections tree.
//! This manager is about the working set the person actually has open: only
//! tabs that came from a saved connection can be stored, and opening a
//! workspace replaces the current tabs instead of piling new ones on top.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use gtk::prelude::*;

use crate::headless::connections;
use crate::headless::workspaces::{self, WorkBook, Workspace, WorkspaceEntry};
use crate::window::{MainWindow, TerminalWindow};

const COL_NAME: u32 = 0;
const COL_ICON: u32 = 1;
const COL_COLOR: u32 = 2;
const COL_COUNT: u32 = 3;
const COL_TOOLTIP: u32 = 4;

thread_local! {
    static OPEN: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

struct Catalogue {
    window: gtk::Window,
    store: gtk::ListStore,
    tree: gtk::TreeView,
    owner: crate::SendWeak<MainWindow>,
}

fn load_book(owner: &MainWindow) -> Option<WorkBook> {
    let path = workspaces::default_path()?;
    match workspaces::load(&path) {
        Ok(book) => Some(book),
        Err(error) => {
            owner.status_message(&format!("Cannot load workspaces: {error}"), true);
            None
        }
    }
}

fn save_book(book: &WorkBook, owner: &MainWindow) -> bool {
    let Some(path) = workspaces::default_path() else {
        owner.status_message("Cannot find the configuration directory", true);
        return false;
    };
    match workspaces::save(&path, book) {
        Ok(()) => true,
        Err(error) => {
            owner.status_message(&format!("Cannot save workspaces: {error}"), true);
            false
        }
    }
}

impl Catalogue {
    fn refresh(&self, book: &WorkBook) {
        self.store.clear();
        for workspace in &book.workspaces {
            let tooltip = if workspace.entries.is_empty() {
                "No connections".to_string()
            } else {
                workspace
                    .entries
                    .iter()
                    .map(|entry| entry.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let row = self.store.append();
            self.store
                .set_value(&row, COL_NAME, &workspace.name.to_value());
            self.store
                .set_value(&row, COL_ICON, &workspace.icon.to_value());
            self.store
                .set_value(&row, COL_COLOR, &workspace.color.to_value());
            self.store.set_value(
                &row,
                COL_COUNT,
                &workspace.entries.len().to_string().to_value(),
            );
            self.store.set_value(&row, COL_TOOLTIP, &tooltip.to_value());
        }
        if let Some(first) = gtk::prelude::TreeModelExt::iter_first(&self.store) {
            self.tree.selection().select_iter(&first);
        }
    }

    fn selected(&self) -> Option<String> {
        let (model, iter) = self.tree.selection().selected()?;
        model.value(&iter, COL_NAME as i32).get::<String>().ok()
    }

    /// Tabs that came from the saved book: aliases typed on the fly and plain
    /// `~/.ssh/config` hosts are deliberately left out.
    fn current_entries(owner: &MainWindow) -> Vec<WorkspaceEntry> {
        let Ok(book) = connections::load_book(&connections::book_path()) else {
            return Vec::new();
        };
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for term in owner.all_terminals() {
            let Some(id) = term.connection_id() else {
                continue;
            };
            if book.find_by_id(&id).is_none() || !seen.insert(id.clone()) {
                continue;
            }
            entries.push(WorkspaceEntry {
                label: term.connection_name().unwrap_or_else(|| id.clone()),
                kind: "connection".to_string(),
                target: id,
            });
        }
        entries
    }
}

fn add_column(tree: &gtk::TreeView, title: &str, column: u32, color: bool) {
    let view_column = gtk::TreeViewColumn::new();
    view_column.set_title(title);
    let renderer = gtk::CellRendererText::new();
    gtk::prelude::TreeViewColumnExt::pack_start(&view_column, &renderer, true);
    gtk::prelude::TreeViewColumnExt::add_attribute(
        &view_column,
        &renderer,
        if color { "foreground" } else { "text" },
        column as i32,
    );
    tree.append_column(&view_column);
}

fn edit_dialog(
    owner: &MainWindow,
    existing: Option<&Workspace>,
) -> Option<(String, String, String)> {
    let dialog = gtk::Dialog::with_buttons(
        Some("Workspace"),
        Some(owner),
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Save", gtk::ResponseType::Ok),
        ],
    );
    let area = dialog.content_area();
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some("Name"));
    name.set_text(
        existing
            .map(|workspace| workspace.name.as_str())
            .unwrap_or(""),
    );
    let icon = gtk::Entry::new();
    icon.set_placeholder_text(Some("Icon name (optional), e.g. code"));
    icon.set_text(
        existing
            .map(|workspace| workspace.icon.as_str())
            .unwrap_or(""),
    );
    let color = gtk::Entry::new();
    color.set_placeholder_text(Some("Colour (optional), e.g. #39f"));
    color.set_text(
        existing
            .map(|workspace| workspace.color.as_str())
            .unwrap_or(""),
    );
    for (label, field) in [("Name", &name), ("Icon", &icon), ("Colour", &color)] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let caption = gtk::Label::new(Some(label));
        caption.set_width_chars(8);
        caption.set_xalign(0.0);
        row.pack_start(&caption, false, false, 0);
        row.pack_start(field, true, true, 0);
        area.pack_start(&row, false, false, 4);
    }
    dialog.show_all();
    name.grab_focus();
    let answer = dialog.run();
    let result = if answer == gtk::ResponseType::Ok {
        Some((
            name.text().to_string(),
            icon.text().to_string(),
            color.text().to_string(),
        ))
    } else {
        None
    };
    // SAFETY: `run` has returned and nothing else owns this dialog.
    unsafe { dialog.destroy() };
    result
}

fn confirm_delete(owner: &MainWindow, name: &str) -> bool {
    let dialog = gtk::MessageDialog::new(
        Some(owner),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Question,
        gtk::ButtonsType::YesNo,
        &format!("Delete the workspace '{name}'?"),
    );
    let answer = dialog.run();
    dialog.close();
    answer == gtk::ResponseType::Yes
}

fn open_selected(catalogue: &Catalogue) {
    let Some(owner) = catalogue.owner.upgrade() else {
        return;
    };
    let Some(name) = catalogue.selected() else {
        return;
    };
    let Some(book) = load_book(&owner) else {
        return;
    };
    let Some(workspace) = workspaces::find(&book, &name).cloned() else {
        return;
    };
    if !owner.confirm_replace_tabs() {
        return;
    }
    owner.clear_tabs_for_workspace();
    owner.open_workspace_entries(workspace.entries.clone());
    owner.status_message(&format!("Opened workspace {}", workspace.name), false);
}

fn save_current(catalogue: &Catalogue) {
    let Some(owner) = catalogue.owner.upgrade() else {
        return;
    };
    let entries = Catalogue::current_entries(&owner);
    if entries.is_empty() {
        owner.status_message("No tabs from saved connections are open", true);
        return;
    }
    let Some(book) = load_book(&owner) else {
        return;
    };
    let Some((name, icon, color)) = edit_dialog(&owner, None) else {
        return;
    };
    let mut book = book;
    if let Err(error) = workspaces::upsert_styled(&mut book, &name, entries, &icon, &color) {
        owner.status_message(&format!("Cannot save workspace: {error}"), true);
        return;
    }
    if save_book(&book, &owner) {
        catalogue.refresh(&book);
        owner.status_message(&format!("Saved workspace {name}"), false);
    }
}

fn edit_selected(catalogue: &Catalogue) {
    let Some(owner) = catalogue.owner.upgrade() else {
        return;
    };
    let Some(name) = catalogue.selected() else {
        return;
    };
    let Some(book) = load_book(&owner) else {
        return;
    };
    let Some(workspace) = workspaces::find(&book, &name).cloned() else {
        return;
    };
    let Some((new_name, icon, color)) = edit_dialog(&owner, Some(&workspace)) else {
        return;
    };
    let mut book = book;
    if new_name.to_lowercase() != workspace.name.to_lowercase() {
        workspaces::remove(&mut book, &workspace.name);
    }
    let entries = workspace.entries.clone();
    if let Err(error) = workspaces::upsert_styled(&mut book, &new_name, entries, &icon, &color) {
        owner.status_message(&format!("Cannot rename workspace: {error}"), true);
        return;
    }
    if save_book(&book, &owner) {
        catalogue.refresh(&book);
    }
}

fn delete_selected(catalogue: &Catalogue) {
    let Some(owner) = catalogue.owner.upgrade() else {
        return;
    };
    let Some(name) = catalogue.selected() else {
        return;
    };
    if !confirm_delete(&owner, &name) {
        return;
    }
    let Some(book) = load_book(&owner) else {
        return;
    };
    let mut book = book;
    if !workspaces::remove(&mut book, &name) {
        return;
    }
    if save_book(&book, &owner) {
        catalogue.refresh(&book);
    }
}

/// Open (or focus) the saved-workspace catalogue.
pub(crate) fn show(owner: &MainWindow) {
    if let Some(window) = OPEN.with(|slot| slot.borrow().clone()) {
        window.present();
        return;
    }
    let Some(book) = load_book(owner) else {
        return;
    };
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Saved workspaces");
    window.set_transient_for(Some(owner));
    window.set_modal(true);
    window.set_default_size(600, 380);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_border_width(12);
    let store = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let tree = gtk::TreeView::with_model(&store);
    tree.set_tooltip_column(COL_TOOLTIP as i32);
    add_column(&tree, "Name", COL_NAME, false);
    add_column(&tree, "Icon", COL_ICON, false);
    add_column(&tree, "Colour", COL_COLOR, true);
    add_column(&tree, "Tabs", COL_COUNT, false);
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&tree);
    root.pack_start(&scroll, true, true, 0);

    let buttons = gtk::ButtonBox::new(gtk::Orientation::Horizontal);
    buttons.set_layout(gtk::ButtonBoxStyle::End);
    buttons.set_spacing(6);
    let open = gtk::Button::with_label("Open");
    let edit = gtk::Button::with_label("Edit...");
    let save = gtk::Button::with_label("Save Current...");
    let delete = gtk::Button::with_label("Delete");
    let close = gtk::Button::with_label("Close");
    for button in [&open, &edit, &save, &delete, &close] {
        buttons.pack_start(button, false, false, 0);
    }
    root.pack_start(&buttons, false, false, 0);
    window.add(&root);

    let catalogue = Rc::new(Catalogue {
        window: window.clone(),
        store,
        tree: tree.clone(),
        owner: crate::SendWeak::new(owner),
    });
    catalogue.refresh(&book);

    {
        let catalogue = catalogue.clone();
        open.connect_clicked(move |_| open_selected(&catalogue));
    }
    {
        let catalogue = catalogue.clone();
        edit.connect_clicked(move |_| edit_selected(&catalogue));
    }
    {
        let catalogue = catalogue.clone();
        save.connect_clicked(move |_| save_current(&catalogue));
    }
    {
        let catalogue = catalogue.clone();
        delete.connect_clicked(move |_| delete_selected(&catalogue));
    }
    close.connect_clicked(move |_| window.close());
    {
        let catalogue = catalogue.clone();
        tree.connect_row_activated(move |_, _, _| open_selected(&catalogue));
    }

    catalogue.window.connect_destroy(move |_| {
        OPEN.with(|slot| *slot.borrow_mut() = None);
    });
    OPEN.with(|slot| *slot.borrow_mut() = Some(catalogue.window.clone()));
    catalogue.window.show_all();
}
