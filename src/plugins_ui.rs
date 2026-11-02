//! A small manager for installed plugins.
//!
//! Plugins are external processes, so the natural place to manage them is a
//! dialog: list what is installed, install from a folder or a git URL, open the
//! folder, read a plugin's log, and remove one. A newly installed plugin starts
//! with the next Oxterm window, not immediately.

use gtk::prelude::*;

use crate::headless::plugins::{self, Store};

const COLUMNS: usize = 4;

pub fn show(parent: &gtk::Window) {
    let dialog = gtk::Dialog::with_buttons(
        Some("Plugins"),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(780, 440);
    let content = dialog.content_area();
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);

    let hint = gtk::Label::new(Some(
        "Plugins run in the background. After installing one, press Start Plugins. \
         After updating or removing one, restart every Oxterm window.",
    ));
    hint.set_xalign(0.0);
    hint.set_line_wrap(true);
    content.pack_start(&hint, false, false, 0);

    let model = gtk::ListStore::new(&[glib::Type::STRING; COLUMNS]);
    let tree = gtk::TreeView::with_model(&model);
    tree.set_headers_visible(true);
    for (index, title) in ["ID", "Version", "Status", "Permissions"]
        .iter()
        .enumerate()
    {
        let renderer = gtk::CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.set_title(title);
        column.set_expand(index == 3);
        column.set_resizable(true);
        gtk::prelude::CellLayoutExt::pack_start(&column, &renderer, true);
        gtk::prelude::CellLayoutExt::add_attribute(&column, &renderer, "text", index as i32);
        tree.append_column(&column);
    }
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_shadow_type(gtk::ShadowType::In);
    scroll.set_vexpand(true);
    scroll.add(&tree);
    content.pack_start(&scroll, true, true, 0);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_line_wrap(true);
    content.pack_start(&status, false, false, 0);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let open_folder = gtk::Button::with_label("Open Folder");
    let open_state = gtk::Button::with_label("Open State");
    let log = gtk::Button::with_label("Log");
    let remove = gtk::Button::with_label("Remove");
    let install_folder = gtk::Button::with_label("Install Folder...");
    let install_git = gtk::Button::with_label("Install Git URL...");
    let start = gtk::Button::with_label("Start Plugins");
    buttons.pack_start(&open_folder, false, false, 0);
    buttons.pack_start(&open_state, false, false, 0);
    buttons.pack_start(&log, false, false, 0);
    buttons.pack_start(&remove, false, false, 0);
    buttons.pack_end(&install_git, false, false, 0);
    buttons.pack_end(&install_folder, false, false, 0);
    buttons.pack_end(&start, false, false, 0);
    content.pack_start(&buttons, false, false, 0);

    let store = Store::default_store();
    reload(&store, &model, &status);

    {
        let store = store.clone();
        let model = model.clone();
        let status = status.clone();
        start.connect_clicked(move |_| {
            crate::plugins::start_missing();
            reload(&store, &model, &status);
            status.set_text("Started installed plugins. They run in the background.");
        });
    }

    {
        let store = store.clone();
        let status = status.clone();
        open_folder.connect_clicked(move |_| {
            let directory = store.plugins_dir();
            if let Err(error) = std::fs::create_dir_all(&directory) {
                status.set_text(&format!("{}: {}", directory.display(), error));
                return;
            }
            if !crate::notes::spawn_detached("xdg-open", &[&directory.to_string_lossy()]) {
                status.set_text("Cannot open the plugins folder");
            }
        });
    }
    {
        let store = store.clone();
        let status = status.clone();
        open_state.connect_clicked(move |_| {
            let directory = store.root.join("state");
            if let Err(error) = std::fs::create_dir_all(&directory) {
                status.set_text(&format!("{}: {}", directory.display(), error));
                return;
            }
            if !crate::notes::spawn_detached("xdg-open", &[&directory.to_string_lossy()]) {
                status.set_text("Cannot open the plugin state folder");
            }
        });
    }
    {
        let store = store.clone();
        let tree = tree.clone();
        let status = status.clone();
        log.connect_clicked(move |_| {
            let Some(id) = selected_id(&tree) else {
                status.set_text("Select a plugin first");
                return;
            };
            let path = store.state_dir(&id).join("plugin.log");
            if !path.exists() {
                status.set_text(&format!("{} has no log yet", id));
                return;
            }
            if !crate::notes::spawn_detached("xdg-open", &[&path.to_string_lossy()]) {
                status.set_text("Cannot open the plugin log");
            }
        });
    }
    {
        let store = store.clone();
        let tree = tree.clone();
        let model = model.clone();
        let status = status.clone();
        let dialog = dialog.clone();
        remove.connect_clicked(move |_| {
            let Some(id) = selected_id(&tree) else {
                status.set_text("Select a plugin first");
                return;
            };
            let question = gtk::MessageDialog::new(
                Some(&dialog),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Question,
                gtk::ButtonsType::YesNo,
                &format!("Remove plugin {}?", id),
            );
            let answer = question.run();
            unsafe { question.destroy() };
            if answer != gtk::ResponseType::Yes {
                return;
            }
            match store.remove(&id, false) {
                Ok(()) => {
                    status.set_text(&format!("Removed {}. Restart Oxterm.", id));
                    reload(&store, &model, &status);
                }
                Err(error) => status.set_text(&error),
            }
        });
    }
    {
        let store = store.clone();
        let model = model.clone();
        let status = status.clone();
        let dialog = dialog.clone();
        install_folder.connect_clicked(move |_| {
            let chooser = gtk::FileChooserDialog::with_buttons(
                Some("Install Plugin from Folder"),
                Some(&dialog),
                gtk::FileChooserAction::SelectFolder,
                &[
                    ("Cancel", gtk::ResponseType::Cancel),
                    ("Install", gtk::ResponseType::Accept),
                ],
            );
            let answer = chooser.run();
            let source = chooser.filename();
            unsafe { chooser.destroy() };
            if answer != gtk::ResponseType::Accept {
                return;
            }
            let Some(source) = source else {
                return;
            };
            start_install(
                store.clone(),
                source.to_string_lossy().to_string(),
                model.clone(),
                status.clone(),
            );
        });
    }
    {
        let store = store.clone();
        let model = model.clone();
        let status = status.clone();
        let dialog = dialog.clone();
        install_git.connect_clicked(move |_| {
            let ask = gtk::Dialog::with_buttons(
                Some("Install Plugin from Git"),
                Some(&dialog),
                gtk::DialogFlags::MODAL,
                &[
                    ("Cancel", gtk::ResponseType::Cancel),
                    ("Install", gtk::ResponseType::Accept),
                ],
            );
            ask.set_default_response(gtk::ResponseType::Accept);
            let entry = gtk::Entry::new();
            entry.set_placeholder_text(Some("https://host/plugin.git"));
            entry.set_activates_default(true);
            entry.set_margin_start(12);
            entry.set_margin_end(12);
            entry.set_margin_top(12);
            entry.set_margin_bottom(12);
            ask.content_area().pack_start(&entry, true, true, 0);
            ask.show_all();
            let answer = ask.run();
            let source = entry.text().trim().to_string();
            unsafe { ask.destroy() };
            if answer != gtk::ResponseType::Accept || source.is_empty() {
                return;
            }
            start_install(store.clone(), source, model.clone(), status.clone());
        });
    }

    dialog.show_all();
    dialog.run();
    unsafe { dialog.destroy() };
}

fn reload(store: &Store, model: &gtk::ListStore, status: &gtk::Label) {
    model.clear();
    match store.list() {
        Ok(list) => {
            for installed in list {
                let manifest = &installed.manifest;
                let status = if manifest.enabled {
                    if crate::plugins::is_running(&manifest.id) {
                        "running".to_string()
                    } else {
                        "installed".to_string()
                    }
                } else {
                    "disabled".to_string()
                };
                let permissions = manifest.permissions.join(", ");
                model.set(
                    &model.append(),
                    &[
                        (0, &manifest.id),
                        (1, &manifest.version),
                        (2, &status),
                        (3, &permissions),
                    ],
                );
            }
            status.set_text(&format!(
                "{} plugin(s). Data: {}",
                model.iter_n_children(None),
                store.plugins_dir().display()
            ));
        }
        Err(error) => status.set_text(&error),
    }
}

fn selected_id(tree: &gtk::TreeView) -> Option<String> {
    let (paths, model) = tree.selection().selected_rows();
    let path = paths.first()?;
    let iter = model.iter(path)?;
    model.value(&iter, 0).get::<String>().ok()
}

fn start_install(store: Store, source: String, model: gtk::ListStore, status: gtk::Label) {
    status.set_text(&format!("Installing {}...", source));
    let model_weak = crate::SendWeak::new(&model);
    let status_weak = crate::SendWeak::new(&status);
    std::thread::spawn(move || {
        let result = store.install(&source, false);
        crate::run_on_main(move || {
            if let Some(model) = model_weak.upgrade() {
                if let Some(status) = status_weak.upgrade() {
                    match result {
                        Ok(installed) => {
                            status.set_text(&format!(
                                "Installed {} {}. Restart Oxterm.",
                                installed.manifest.id, installed.manifest.version
                            ));
                            reload(&store, &model, &status);
                        }
                        Err(error) => status.set_text(&error),
                    }
                }
            }
        });
    });
}

/// Open the plugins directory from the menu without a dialog.
pub fn open_folder() {
    let directory = plugins::Store::default_store().plugins_dir();
    let _ = std::fs::create_dir_all(&directory);
    crate::notes::spawn_detached("xdg-open", &[&directory.to_string_lossy()]);
}

pub fn open_state() {
    let directory = plugins::Store::default_store().root.join("state");
    let _ = std::fs::create_dir_all(&directory);
    crate::notes::spawn_detached("xdg-open", &[&directory.to_string_lossy()]);
}
