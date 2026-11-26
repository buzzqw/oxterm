//! GUI for importing other managers' books and synchronising `~/.ssh/config`.
//!
//! The parsers and the diff live in `headless`; this module only asks for files,
//! shows what would change and applies it with the same atomic write as the
//! rest of Oxterm. No secret is ever read from an imported file.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::headless::connection_import;
use crate::headless::connections;
use crate::headless::persistence::atomic_write;
use crate::headless::ssh_config;
use crate::headless::ssh_sync;
use crate::window::{MainWindow, TerminalWindow};

/// Ask for an exported book, preview it, then merge the non-conflicting entries.
pub(crate) fn show_import_dialog(window: &MainWindow) {
    let chooser = gtk::FileChooserDialog::with_buttons(
        Some("Import connections"),
        None::<&gtk::Window>,
        gtk::FileChooserAction::Open,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Import", gtk::ResponseType::Accept),
        ],
    );
    chooser.set_modal(true);
    chooser.set_transient_for(Some(window));
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(
        "Connection exports (*.remmina, *.rdm, *.reg, *.json, config)",
    ));
    for pattern in [
        "*.remmina",
        "*.rdm",
        "*.reg",
        "*.json",
        "*.xml",
        "*.mxtsessions",
        "config",
    ] {
        filter.add_pattern(pattern);
    }
    chooser.add_filter(filter);
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    chooser.add_filter(all);

    if chooser.run() != gtk::ResponseType::Accept {
        // SAFETY: `run` has returned and nothing else owns this dialog.
        unsafe { chooser.destroy() };
        return;
    }
    let Some(path) = chooser.filename() else {
        // SAFETY: `run` has returned and nothing else owns this dialog.
        unsafe { chooser.destroy() };
        return;
    };
    // SAFETY: `run` has returned and nothing else owns this dialog.
    unsafe { chooser.destroy() };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            window.status_message(&format!("Cannot read {}: {}", path.display(), error), true);
            return;
        }
    };
    let format = connection_import::detect_format(&text, &path.to_string_lossy());
    let entries = match connection_import::parse(&text, format) {
        Ok(entries) => entries,
        Err(error) => {
            window.status_message(
                &format!("Cannot import {}: {}", path.display(), error),
                true,
            );
            return;
        }
    };
    let book = match connections::load_book(&connections::book_path()) {
        Ok(book) => book,
        Err(error) => {
            window.status_message(&format!("Cannot open the connection book: {}", error), true);
            return;
        }
    };
    let preview = connection_import::preview(&book, entries);
    if preview.entries.is_empty() && preview.skipped.is_empty() {
        window.status_message("No connections found in that file", true);
        return;
    }

    let dialog = gtk::Dialog::new();
    dialog.set_title("Import connections");
    dialog.set_transient_for(Some(window));
    dialog.set_modal(true);
    dialog.set_default_size(560, 420);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Import", gtk::ResponseType::Accept);

    let area = dialog.content_area();
    let heading = gtk::Label::new(Some(&format!(
        "<b>{} connection(s) ready</b>\nExisting names and invalid entries are skipped. Passwords and one-time secrets are never imported.",
        preview.entries.len()
    )));
    heading.set_use_markup(true);
    heading.set_xalign(0.0);
    heading.set_line_wrap(true);
    area.pack_start(&heading, false, false, 6);
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    let mut summary = String::new();
    for entry in &preview.entries {
        summary.push_str(&format!("  {}  —  {}\n", entry.name, entry.destination()));
    }
    for (name, reason) in &preview.skipped {
        summary.push_str(&format!("  skipped {}: {}\n", name, reason));
    }
    if let Some(buffer) = view.buffer() {
        buffer.set_text(&summary);
    }
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&view);
    area.pack_start(&scroll, true, true, 6);
    dialog.show_all();

    if dialog.run() == gtk::ResponseType::Accept {
        let mut book = book;
        // `preview` already filtered the list: rebuild the accepted entries so
        // the merge applies exactly what was shown, not the raw file.
        let accepted: Vec<connections::Connection> = preview
            .entries
            .iter()
            .cloned()
            .map(|mut connection| {
                connection.auth = crate::headless::connections::Auth::Default;
                connection.totp = false;
                connection
            })
            .collect();
        let result = connection_import::merge(&mut book, accepted);
        match connections::save_book(&connections::book_path(), &book) {
            Ok(()) => {
                window.reload_connections_panel();
                let skipped = result.skipped.len();
                window.status_message(
                    &format!(
                        "Imported {} connection(s){}",
                        result.entries.len(),
                        if skipped == 0 {
                            String::new()
                        } else {
                            format!(", skipped {}", skipped)
                        }
                    ),
                    false,
                );
            }
            Err(error) => window.status_message(&format!("Cannot save the book: {}", error), true),
        }
    }
    // SAFETY: `run` has returned and nothing else owns this dialog.
    unsafe { dialog.destroy() };
}

/// Show the book ⇄ `~/.ssh/config` differences and apply only selected rows.
pub(crate) fn show_ssh_sync_dialog(window: &MainWindow) {
    let path = ssh_config::default_config_path();
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let book = match connections::load_book(&connections::book_path()) {
        Ok(book) => book,
        Err(error) => {
            window.status_message(&format!("Cannot open the connection book: {}", error), true);
            return;
        }
    };
    let hosts = ssh_sync::parse_hosts(&original);
    let entries = ssh_sync::diff(&book, &hosts);
    if !entries
        .iter()
        .any(|entry| entry.change != ssh_sync::SyncChange::Unchanged)
    {
        window.status_message(
            "The connection book and ~/.ssh/config are already in sync",
            false,
        );
        return;
    }

    const COL_SELECTED: u32 = 0;
    const COL_ACTION: u32 = 1;
    const COL_ALIAS: u32 = 2;
    const COL_HOST: u32 = 3;
    const COL_FIELDS: u32 = 4;

    let store = gtk::ListStore::new(&[
        glib::Type::BOOL,
        glib::Type::STRING,
        glib::Type::STRING,
        glib::Type::STRING,
        glib::Type::STRING,
    ]);
    for entry in &entries {
        let action = match entry.change {
            ssh_sync::SyncChange::Add => "Add",
            ssh_sync::SyncChange::Update => "Update",
            ssh_sync::SyncChange::Remove => "Remove",
            ssh_sync::SyncChange::Unchanged => "Unchanged",
        };
        let fields = if entry.fields.is_empty() {
            String::new()
        } else {
            entry.fields.join("\n")
        };
        let row = store.append();
        store.set_value(&row, COL_SELECTED, &entry.selected.to_value());
        store.set_value(&row, COL_ACTION, &action.to_value());
        store.set_value(&row, COL_ALIAS, &entry.alias.to_value());
        store.set_value(&row, COL_HOST, &entry.host.to_value());
        store.set_value(&row, COL_FIELDS, &fields.to_value());
    }

    let dialog = gtk::Dialog::new();
    dialog.set_title("Synchronise ~/.ssh/config");
    dialog.set_transient_for(Some(window));
    dialog.set_modal(true);
    dialog.set_default_size(640, 420);
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Apply", gtk::ResponseType::Accept);

    let area = dialog.content_area();
    let tree = gtk::TreeView::with_model(&store);
    let toggle = gtk::CellRendererToggle::new();
    toggle.set_activatable(true);
    let column = gtk::TreeViewColumn::new();
    column.set_title("Apply");
    gtk::prelude::CellLayoutExt::pack_start(&column, &toggle, false);
    gtk::prelude::CellLayoutExt::add_attribute(&column, &toggle, "active", COL_SELECTED as i32);
    tree.append_column(&column);
    for (title, id) in [
        ("Change", COL_ACTION),
        ("Alias", COL_ALIAS),
        ("Host", COL_HOST),
        ("Fields", COL_FIELDS),
    ] {
        let renderer = gtk::CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        gtk::prelude::CellLayoutExt::pack_start(&column, &renderer, true);
        gtk::prelude::CellLayoutExt::add_attribute(&column, &renderer, "text", id as i32);
        tree.append_column(&column);
    }
    let rows = Rc::new(RefCell::new(entries));
    {
        let rows = rows.clone();
        toggle.connect_toggled(move |_, path| {
            let Some(iter) = store.iter(&path) else {
                return;
            };
            let mut rows = rows.borrow_mut();
            if let Ok(index) = path.to_string().parse::<usize>() {
                if let Some(entry) = rows.get_mut(index) {
                    entry.selected = !entry.selected;
                }
            }
            let selected: bool = store
                .value(&iter, COL_SELECTED as i32)
                .get()
                .unwrap_or(false);
            store.set_value(&iter, COL_SELECTED, &(!selected).to_value());
        });
    }
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&tree);
    area.pack_start(&scroll, true, true, 6);
    let hint = gtk::Label::new(Some(
        "Only selected changes are written. Commands and other risky options are never enabled.",
    ));
    hint.set_xalign(0.0);
    area.pack_start(&hint, false, false, 4);
    dialog.show_all();

    if dialog.run() == gtk::ResponseType::Accept {
        let selected: Vec<ssh_sync::SshSyncEntry> = rows
            .borrow()
            .iter()
            .filter(|entry| entry.selected)
            .cloned()
            .collect();
        let updated = ssh_sync::render(&original, &selected);
        match atomic_write(&path, updated.as_bytes()) {
            Ok(()) => {
                window.reload_connections_panel();
                window.status_message(
                    &format!("Updated ~/.ssh/config ({} change(s))", selected.len()),
                    false,
                );
            }
            Err(error) => {
                window.status_message(&format!("Cannot write ~/.ssh/config: {}", error), true)
            }
        }
    }
    // SAFETY: `run` has returned and nothing else owns this dialog.
    unsafe { dialog.destroy() };
}
