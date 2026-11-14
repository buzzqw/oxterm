//! Two-pane file browser for SFTP and FTP: this computer on the left, the server
//! on the right. Transfers run one at a time on a worker thread, so a large file
//! never freezes the window, and the panes stay usable meanwhile.
//!
//! Everything remote goes through `headless::remote_files`, which drives the
//! system's `sftp` and `lftp`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::gdk;
use gtk::prelude::*;

use crate::headless::askpass;
use crate::headless::bookmarks::{self, Bookmark, Side};
use crate::headless::connections::{Auth, Connection};
use crate::headless::remote_files::{self, human_size, Operation, RemoteEntry};

const COLUMN_DISPLAY: u32 = 0;
const COLUMN_NAME: u32 = 1;
const COLUMN_SIZE: u32 = 2;
const COLUMN_MODIFIED: u32 = 3;
const COLUMN_IS_DIR: u32 = 4;
const COLUMN_SORT_SIZE: u32 = 5;
const DRAG_TARGET: &str = "application/x-oxterm-browser-rows";

/// Work for the thread that owns the remote side.
enum Job {
    Home,
    List(String),
    Transfer {
        upload: bool,
        is_dir: bool,
        from: String,
        to: String,
        label: String,
    },
    MakeDir(String),
    Rename {
        from: String,
        to: String,
    },
    Delete {
        path: String,
        is_dir: bool,
    },
    EditDownload {
        remote: String,
        local: String,
        name: String,
    },
    EditUpload {
        local: String,
        remote: String,
        name: String,
    },
    Sync {
        upload: bool,
        from: String,
        to: String,
    },
    /// Switch the worker's copy of the connection to the keyring helper.
    UseKeyring,
}

enum Outcome {
    Home(Result<String, String>),
    Listed(String, Result<Vec<RemoteEntry>, String>),
    Transferred {
        upload: bool,
        label: String,
        result: Result<(), String>,
    },
    Changed(Result<(), String>),
    EditReady {
        local: PathBuf,
        remote: String,
        name: String,
        result: Result<(), String>,
    },
    EditUploaded {
        remote: String,
        name: String,
        result: Result<(), String>,
    },
    Synced {
        upload: bool,
        from: String,
        to: String,
        result: Result<(), String>,
    },
    AuthStored,
}

fn worker(mut connection: Connection, jobs: mpsc::Receiver<Job>, results: mpsc::Sender<Outcome>) {
    let protocol = connection.protocol;
    let simple = |connection: &Connection, operation: Operation| -> Result<(), String> {
        let text = remote_files::script(protocol, &operation)?;
        let result = remote_files::run(connection, &text).map(|_| ());
        if result.is_ok() {
            remote_files::invalidate(connection);
        }
        result
    };
    for job in jobs {
        let job = match job {
            Job::UseKeyring => {
                connection.auth = Auth::SecretService;
                let _ = results.send(Outcome::AuthStored);
                continue;
            }
            other => other,
        };
        let outcome = match job {
            Job::Home => Outcome::Home(remote_files::home_directory(&connection)),
            Job::List(path) => {
                let listing = remote_files::list(&connection, &path);
                Outcome::Listed(path, listing)
            }
            Job::Transfer {
                upload,
                is_dir,
                from,
                to,
                label,
            } => Outcome::Transferred {
                upload,
                label,
                result: remote_files::transfer(&connection, upload, is_dir, &from, &to),
            },
            Job::MakeDir(path) => Outcome::Changed(simple(&connection, Operation::MakeDir(path))),
            Job::Rename { from, to } => {
                Outcome::Changed(simple(&connection, Operation::Rename { from, to }))
            }
            Job::Delete { path, is_dir } => {
                Outcome::Changed(remote_files::delete(&connection, &path, is_dir))
            }
            Job::EditDownload {
                remote,
                local,
                name,
            } => Outcome::EditReady {
                local: PathBuf::from(local.clone()),
                result: remote_files::transfer(&connection, false, false, &remote, &local),
                remote,
                name,
            },
            Job::EditUpload {
                local,
                remote,
                name,
            } => Outcome::EditUploaded {
                result: remote_files::transfer(&connection, true, false, &local, &remote),
                remote,
                name,
            },
            Job::Sync { upload, from, to } => Outcome::Synced {
                upload,
                result: remote_files::sync_directory(&connection, upload, &from, &to),
                from,
                to,
            },
            Job::UseKeyring => unreachable!("handled before the job match"),
        };
        if results.send(outcome).is_err() {
            return;
        }
    }
}

struct Pane {
    root: gtk::Box,
    path: gtk::Entry,
    tree: gtk::TreeView,
    store: gtk::ListStore,
    up: gtk::Button,
    refresh: gtk::Button,
    new_folder: gtk::Button,
    rename: gtk::Button,
    delete: gtk::Button,
    bookmark: gtk::Button,
    bookmarks: gtk::Button,
    sync: gtk::Button,
    edit: Option<gtk::Button>,
}

impl Pane {
    fn build(title: &str, remote: bool) -> Pane {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let caption = gtk::Label::new(None);
        caption.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
        caption.set_xalign(0.0);
        root.pack_start(&caption, false, false, 0);

        let path_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let up = gtk::Button::with_label("Up");
        up.set_tooltip_text(Some("Go to the parent folder (Backspace)"));
        let path = gtk::Entry::new();
        path.set_hexpand(true);
        let refresh = gtk::Button::with_label("Refresh");
        refresh.set_tooltip_text(Some("Read this folder again (F5)"));
        path_row.pack_start(&up, false, false, 0);
        path_row.pack_start(&path, true, true, 0);
        path_row.pack_start(&refresh, false, false, 0);
        root.pack_start(&path_row, false, false, 0);

        let store = gtk::ListStore::new(&[
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::BOOL,
            glib::Type::U64,
        ]);
        let tree = gtk::TreeView::with_model(&store);
        tree.set_enable_search(true);
        tree.set_search_column(COLUMN_NAME as i32);
        tree.selection().set_mode(gtk::SelectionMode::Multiple);
        for (title, text_column, sort_column, expand) in [
            ("Name", COLUMN_DISPLAY, COLUMN_NAME, true),
            ("Size", COLUMN_SIZE, COLUMN_SORT_SIZE, false),
            ("Modified", COLUMN_MODIFIED, COLUMN_MODIFIED, false),
        ] {
            let renderer = gtk::CellRendererText::new();
            if expand {
                renderer.set_ellipsize(pango::EllipsizeMode::End);
            }
            let column = gtk::TreeViewColumn::new();
            column.set_title(title);
            column.set_expand(expand);
            column.set_resizable(true);
            column.set_sort_column_id(sort_column as i32);
            TreeViewColumnExt::pack_start(&column, &renderer, true);
            TreeViewColumnExt::add_attribute(&column, &renderer, "text", text_column as i32);
            tree.append_column(&column);
        }
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_shadow_type(gtk::ShadowType::In);
        scroll.set_vexpand(true);
        scroll.add(&tree);
        root.pack_start(&scroll, true, true, 0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let new_folder = gtk::Button::with_label("New Folder");
        let rename = gtk::Button::with_label("Rename");
        rename.set_tooltip_text(Some("Rename the selected item (F2)"));
        let delete = gtk::Button::with_label("Delete");
        delete.set_tooltip_text(Some("Delete the selected items (Delete)"));
        actions.pack_start(&new_folder, false, false, 0);
        actions.pack_start(&rename, false, false, 0);
        actions.pack_start(&delete, false, false, 0);
        root.pack_start(&actions, false, false, 0);

        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let bookmark = gtk::Button::with_label("Bookmark");
        bookmark.set_tooltip_text(Some("Save this folder as a bookmark"));
        let bookmarks = gtk::Button::with_label("Bookmarks");
        bookmarks.set_tooltip_text(Some("Jump to a saved folder"));
        let sync = gtk::Button::with_label(if remote { "← Sync" } else { "Sync →" });
        sync.set_tooltip_text(Some(if remote {
            "Copy every item in this folder to this computer"
        } else {
            "Copy every item in this folder to the server"
        }));
        tools.pack_start(&bookmark, false, false, 0);
        tools.pack_start(&bookmarks, false, false, 0);
        tools.pack_start(&sync, false, false, 0);
        let edit = if remote {
            let edit = gtk::Button::with_label("Edit");
            edit.set_tooltip_text(Some("Edit the selected remote file in place"));
            tools.pack_start(&edit, false, false, 0);
            Some(edit)
        } else {
            None
        };
        root.pack_start(&tools, false, false, 0);

        Pane {
            root,
            path,
            tree,
            store,
            up,
            refresh,
            new_folder,
            rename,
            delete,
            bookmark,
            bookmarks,
            sync,
            edit,
        }
    }

    fn fill(&self, entries: &[RemoteEntry]) {
        self.store.clear();
        for entry in entries {
            let display = if entry.is_dir {
                format!("{}/", entry.name)
            } else if entry.is_link {
                format!("{} @", entry.name)
            } else {
                entry.name.clone()
            };
            let size = if entry.is_dir {
                String::new()
            } else {
                human_size(entry.size)
            };
            self.store.set(
                &self.store.append(),
                &[
                    (COLUMN_DISPLAY, &display),
                    (COLUMN_NAME, &entry.name),
                    (COLUMN_SIZE, &size),
                    (COLUMN_MODIFIED, &entry.modified),
                    (COLUMN_IS_DIR, &entry.is_dir),
                    (COLUMN_SORT_SIZE, &entry.size),
                ],
            );
        }
    }

    /// Selected rows as (name, is_dir).
    fn selected(&self) -> Vec<(String, bool)> {
        let (paths, model) = self.tree.selection().selected_rows();
        paths
            .iter()
            .filter_map(|path| {
                let iter = model.iter(path)?;
                Some((
                    model.value(&iter, COLUMN_NAME as i32).get().ok()?,
                    model.value(&iter, COLUMN_IS_DIR as i32).get().ok()?,
                ))
            })
            .collect()
    }

    /// Every row as (name, is_dir), in display order.
    fn all(&self) -> Vec<(String, bool)> {
        let mut rows = Vec::new();
        self.store.foreach(|model, _, iter| {
            let name = model
                .value(iter, COLUMN_NAME as i32)
                .get::<String>()
                .unwrap_or_default();
            let is_dir = model
                .value(iter, COLUMN_IS_DIR as i32)
                .get::<bool>()
                .unwrap_or(false);
            rows.push((name, is_dir));
            false
        });
        rows
    }

    fn contains(&self, name: &str) -> bool {
        let mut found = false;
        self.store.foreach(|model, _, iter| {
            found = model
                .value(iter, COLUMN_NAME as i32)
                .get::<String>()
                .is_ok_and(|value| value == name);
            found
        });
        found
    }

    fn current(&self) -> String {
        self.path.text().to_string()
    }
}

struct EditSession {
    remote: String,
    last_modified: Option<std::time::SystemTime>,
    uploading: Cell<bool>,
}

struct Browser {
    window: gtk::Window,
    connection: Connection,
    local: Pane,
    remote: Pane,
    status: gtk::Label,
    spinner: gtk::Spinner,
    jobs: mpsc::Sender<Job>,
    results: mpsc::Receiver<Outcome>,
    pending: Cell<usize>,
    polling: Cell<bool>,
    /// Discards a local directory listing that finished after a newer one.
    local_generation: Cell<u64>,
    queued: RefCell<VecDeque<String>>,
    edits: RefCell<Vec<EditSession>>,
    /// For the polling timer. Weak: the timer must not keep a closed browser alive.
    me: RefCell<std::rc::Weak<Browser>>,
}

/// A stable temporary path for editing one remote file. The hash keeps two
/// files with the same name in different remote folders apart.
fn edit_local_path(connection_id: &str, remote: &str, name: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    remote.hash(&mut hasher);
    let safe = |text: &str| -> String {
        text.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || "._-".contains(character) {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    };
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("oxterm")
        .join("remote-edit")
        .join(safe(connection_id))
        .join(format!("{:016x}-{}", hasher.finish(), safe(name)))
}

fn local_entries(path: &Path) -> Result<Vec<RemoteEntry>, String> {
    let reader =
        std::fs::read_dir(path).map_err(|error| format!("{}: {}", path.display(), error))?;
    let mut entries: Vec<RemoteEntry> = reader
        .flatten()
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().to_string();
            let link = item.file_type().ok()?.is_symlink();
            // A broken link has no target to describe, but it is still listed.
            let metadata = item
                .metadata()
                .ok()
                .or_else(|| std::fs::symlink_metadata(item.path()).ok())?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| {
                    let local: glib::DateTime = glib::DateTime::from_unix_local(
                        time.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                    )
                    .ok()?;
                    local
                        .format("%b %e %H:%M")
                        .ok()
                        .map(|text| text.to_string())
                })
                .unwrap_or_default();
            Some(RemoteEntry {
                name,
                is_dir: metadata.is_dir(),
                is_link: link,
                size: metadata.len(),
                modified,
                permissions: String::new(),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

impl Browser {
    fn say(&self, message: &str) {
        self.status.set_text(message);
    }

    fn submit(&self, job: Job) {
        self.pending.set(self.pending.get() + 1);
        self.spinner.start();
        let _ = self.jobs.send(job);
        self.start_polling();
    }

    /// Collect results only while work is out. A browser left open on a folder for
    /// hours then costs no wakeups at all.
    fn start_polling(&self) {
        if self.polling.replace(true) {
            return;
        }
        let Some(browser) = self.me.borrow().upgrade() else {
            self.polling.set(false);
            return;
        };
        glib::timeout_add_local(Duration::from_millis(100), move || {
            while let Ok(outcome) = browser.results.try_recv() {
                browser.apply(outcome);
            }
            if browser.pending.get() > 0 {
                return glib::ControlFlow::Continue;
            }
            browser.polling.set(false);
            glib::ControlFlow::Break
        });
    }

    fn finished(&self) {
        let left = self.pending.get().saturating_sub(1);
        self.pending.set(left);
        if left == 0 {
            self.spinner.stop();
        }
    }

    fn load_local(&self, path: &str) {
        // Directory traversal and `stat` calls can block for a long time on a
        // huge folder or a network mount, so list on a worker and apply the
        // result on the main thread.
        let generation = self.local_generation.get().wrapping_add(1);
        self.local_generation.set(generation);
        self.say(&format!("Reading {}...", path));
        let Some(browser) = self.me.borrow().upgrade() else {
            return;
        };
        let browser = crate::Sendable(browser);
        let path = path.to_string();
        std::thread::spawn(move || {
            let result = local_entries(Path::new(&path));
            crate::run_on_main(move || {
                let browser = browser;
                if browser.0.local_generation.get() != generation {
                    return;
                }
                match result {
                    Ok(entries) => {
                        browser.0.local.path.set_text(&path);
                        browser.0.local.fill(&entries);
                        browser.0.say(&format!("{} entries", entries.len()));
                    }
                    Err(error) => browser.0.say(&error),
                }
            });
        });
    }

    fn load_remote(&self, path: &str) {
        self.say(&format!("Reading {}...", path));
        self.submit(Job::List(path.to_string()));
    }

    fn reload_remote(&self) {
        self.load_remote(&self.remote.current());
    }

    fn reload_local(&self) {
        self.load_local(&self.local.current());
    }

    fn confirm(&self, question: &str) -> bool {
        let dialog = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::YesNo,
            question,
        );
        let answer = dialog.run();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        answer == gtk::ResponseType::Yes
    }

    fn ask_name(&self, title: &str, initial: &str) -> Option<String> {
        let dialog = gtk::Dialog::with_buttons(
            Some(title),
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("OK", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Accept);
        let entry = gtk::Entry::new();
        entry.set_text(initial);
        entry.set_activates_default(true);
        entry.set_margin_start(12);
        entry.set_margin_end(12);
        entry.set_margin_top(12);
        entry.set_margin_bottom(12);
        dialog.content_area().pack_start(&entry, true, true, 0);
        dialog.show_all();
        let answer = dialog.run();
        let text = entry.text().trim().to_string();
        // SAFETY of the GTK contract: `run` has returned and nothing else owns it.
        unsafe {
            dialog.destroy();
        }
        if answer != gtk::ResponseType::Accept {
            return None;
        }
        match remote_files::check_name(&text) {
            Ok(()) => Some(text),
            Err(message) => {
                self.say(&message);
                None
            }
        }
    }

    /// Copy the selection of one pane into the folder shown by the other.
    fn transfer_selected(&self, upload: bool) {
        let (source, target) = if upload {
            (&self.local, &self.remote)
        } else {
            (&self.remote, &self.local)
        };
        let selected = source.selected();
        if selected.is_empty() {
            self.say("Select something to copy first");
            return;
        }
        let from_dir = source.current();
        let to_dir = target.current();
        for (name, is_dir) in selected {
            self.enqueue_transfer(
                upload,
                is_dir,
                remote_files::join(&from_dir, &name),
                &to_dir,
                &name,
            );
        }
    }

    fn enqueue_transfer(&self, upload: bool, is_dir: bool, from: String, to_dir: &str, name: &str) {
        let target = if upload { &self.remote } else { &self.local };
        if target.contains(name)
            && !self.confirm(&format!("'{}' already exists there. Replace it?", name))
        {
            return;
        }
        self.queued.borrow_mut().push_back(name.to_string());
        self.update_queue_text();
        self.submit(Job::Transfer {
            upload,
            is_dir,
            from,
            to: remote_files::join(to_dir, name),
            label: name.to_string(),
        });
    }

    fn update_queue_text(&self) {
        let queued = self.queued.borrow();
        if let Some(first) = queued.front() {
            let more = queued.len() - 1;
            if more > 0 {
                self.say(&format!("Copying {} ({} more waiting)...", first, more));
            } else {
                self.say(&format!("Copying {}...", first));
            }
        }
    }

    fn bookmarks_for(&self, remote: bool) -> Vec<Bookmark> {
        let file = bookmarks::bookmark_path();
        let list = match bookmarks::load(&file) {
            Ok(list) => list,
            Err(error) => {
                self.say(&error);
                return Vec::new();
            }
        };
        let connection = if remote {
            Some(self.connection.id.as_str())
        } else {
            None
        };
        bookmarks::visible(
            &list,
            if remote { Side::Remote } else { Side::Local },
            connection,
        )
        .into_iter()
        .cloned()
        .collect()
    }

    fn add_bookmark(&self, remote: bool) {
        let pane = if remote { &self.remote } else { &self.local };
        let path = pane.current();
        let default = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("Folder")
            .to_string();
        let Some(name) = self.ask_name("Bookmark Name", &default) else {
            return;
        };
        let file = bookmarks::bookmark_path();
        let mut list = match bookmarks::load(&file) {
            Ok(list) => list,
            Err(error) => {
                self.say(&error);
                return;
            }
        };
        let bookmark = Bookmark {
            name,
            path: path.clone(),
            side: if remote { Side::Remote } else { Side::Local },
            connection: if remote {
                Some(self.connection.id.clone())
            } else {
                None
            },
        };
        match bookmarks::add(&mut list, bookmark).and_then(|_| bookmarks::save(&file, &list)) {
            Ok(()) => self.say(&format!("Bookmarked {}", path)),
            Err(error) => self.say(&error),
        }
    }

    fn remove_bookmark(&self, remote: bool) {
        let pane = if remote { &self.remote } else { &self.local };
        let path = pane.current();
        let file = bookmarks::bookmark_path();
        let mut list = match bookmarks::load(&file) {
            Ok(list) => list,
            Err(error) => {
                self.say(&error);
                return;
            }
        };
        let connection = if remote {
            Some(self.connection.id.as_str())
        } else {
            None
        };
        let side = if remote { Side::Remote } else { Side::Local };
        if bookmarks::remove(&mut list, side, connection, &path) {
            match bookmarks::save(&file, &list) {
                Ok(()) => self.say(&format!("Removed bookmark for {}", path)),
                Err(error) => self.say(&error),
            }
        } else {
            self.say("This folder has no bookmark");
        }
    }

    /// Copy the whole current folder tree to the other pane.
    fn sync_current(&self, upload: bool) {
        let (source, target) = if upload {
            (&self.local, &self.remote)
        } else {
            (&self.remote, &self.local)
        };
        if source.all().is_empty() {
            self.say("This folder is empty");
            return;
        }
        let question = format!(
            "Copy every item from\n{}\nto\n{}\n\nExisting files are overwritten. \
             Extra files in the destination are kept.",
            source.current(),
            target.current()
        );
        if !self.confirm(&question) {
            return;
        }
        let from = source.current();
        let to = target.current();
        self.say(&format!("Syncing {}...", from));
        self.submit(Job::Sync { upload, from, to });
    }

    /// Download the selected remote file to a temporary copy and open it; every
    /// save is uploaded back. The polling timer stops when the browser closes or
    /// the temporary file disappears.
    fn edit_selected(self: &Rc<Self>) {
        let selected = self.remote.selected();
        let [(name, false)] = selected.as_slice() else {
            self.say("Select one remote file to edit");
            return;
        };
        let remote = remote_files::join(&self.remote.current(), name);
        let local = edit_local_path(&self.connection.id, &remote, name);
        if self
            .edits
            .borrow()
            .iter()
            .any(|session| session.remote == remote)
        {
            self.open_editor(&local);
            return;
        }
        if let Some(parent) = local.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.say(&format!("{}: {}", parent.display(), error));
                return;
            }
        }
        if local.exists()
            && !self.confirm(&format!(
                "An edited copy of '{}' already exists. Download the server version and replace it?",
                name
            ))
        {
            self.watch_edit(local.clone(), remote, name.clone());
            self.open_editor(&local);
            return;
        }
        self.say(&format!("Downloading {}...", name));
        self.submit(Job::EditDownload {
            remote,
            local: local.display().to_string(),
            name: name.clone(),
        });
    }

    /// Ask for this host's password, put it in the keyring, and reconnect.
    /// Useful for hosts that do not accept a key: sftp cannot prompt by itself.
    fn prompt_password(self: &Rc<Self>) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Connect with a password"),
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Connect", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Accept);
        let content = dialog.content_area();
        let label = gtk::Label::new(Some(&format!(
            "Store a password for {} in the system keyring.\n\n\
             It is used by sftp and is never written to the connection file.",
            self.connection.name
        )));
        label.set_xalign(0.0);
        label.set_line_wrap(true);
        let entry = gtk::Entry::new();
        entry.set_visibility(false);
        entry.set_activates_default(true);
        entry.set_margin_top(8);
        content.pack_start(&label, false, false, 0);
        content.pack_start(&entry, false, false, 0);
        dialog.show_all();
        let answer = dialog.run();
        let password = entry.text().to_string();
        unsafe { dialog.destroy() };
        if answer != gtk::ResponseType::Accept {
            return;
        }
        if password.is_empty() {
            self.say("The password is empty");
            return;
        }
        match askpass::store_secret(&self.connection.id, &self.connection.name, &password) {
            Ok(()) => {
                self.submit(Job::UseKeyring);
                self.submit(Job::Home);
                self.say("Password stored. Reconnecting...");
            }
            Err(error) => self.say(&format!("Cannot store the password: {}", error)),
        }
    }

    fn open_editor(&self, local: &Path) {
        if crate::notes::which("xdg-open").is_none() {
            self.say("Cannot open the editor: xdg-open is not installed");
            return;
        }
        let path = local.to_string_lossy().to_string();
        if !crate::notes::spawn_detached("xdg-open", &[&path]) {
            self.say("Could not start the editor");
        }
    }

    fn start_editing(self: &Rc<Self>, local: PathBuf, remote: String, name: String) {
        if !self
            .edits
            .borrow()
            .iter()
            .any(|session| session.remote == remote)
        {
            self.watch_edit(local.clone(), remote, name.clone());
        }
        self.open_editor(&local);
        self.say(&format!("Editing {}; changes upload automatically", name));
    }

    fn watch_edit(self: &Rc<Self>, local: PathBuf, remote: String, name: String) {
        let last_modified = std::fs::metadata(&local)
            .and_then(|metadata| metadata.modified())
            .ok();
        self.edits.borrow_mut().push(EditSession {
            remote: remote.clone(),
            last_modified,
            uploading: Cell::new(false),
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(2, move || {
            let Some(browser) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Ok(metadata) = std::fs::metadata(&local) else {
                browser
                    .edits
                    .borrow_mut()
                    .retain(|session| session.remote != remote);
                return glib::ControlFlow::Break;
            };
            let modified = metadata.modified().ok();
            let (uploading, changed) = {
                let edits = browser.edits.borrow();
                let Some(session) = edits.iter().find(|session| session.remote == remote) else {
                    return glib::ControlFlow::Break;
                };
                (session.uploading.get(), session.last_modified != modified)
            };
            if uploading || !changed {
                return glib::ControlFlow::Continue;
            }
            if let Some(session) = browser
                .edits
                .borrow_mut()
                .iter_mut()
                .find(|session| session.remote == remote)
            {
                session.last_modified = modified;
                session.uploading.set(true);
            }
            browser.submit(Job::EditUpload {
                local: local.display().to_string(),
                remote: remote.clone(),
                name: name.clone(),
            });
            glib::ControlFlow::Continue
        });
    }

    fn make_folder(&self, remote: bool) {
        let Some(name) = self.ask_name("New Folder", "") else {
            return;
        };
        if remote {
            self.submit(Job::MakeDir(remote_files::join(
                &self.remote.current(),
                &name,
            )));
        } else {
            let path = PathBuf::from(self.local.current()).join(&name);
            match std::fs::create_dir(&path) {
                Ok(()) => self.reload_local(),
                Err(error) => self.say(&format!("{}: {}", path.display(), error)),
            }
        }
    }

    fn rename_selected(&self, remote: bool) {
        let pane = if remote { &self.remote } else { &self.local };
        let selected = pane.selected();
        let [(name, _)] = selected.as_slice() else {
            self.say("Select one item to rename");
            return;
        };
        let Some(new_name) = self.ask_name("Rename", name) else {
            return;
        };
        if new_name == *name {
            return;
        }
        let directory = pane.current();
        let from = remote_files::join(&directory, name);
        let to = remote_files::join(&directory, &new_name);
        if remote {
            self.submit(Job::Rename { from, to });
        } else {
            match std::fs::rename(&from, &to) {
                Ok(()) => self.reload_local(),
                Err(error) => self.say(&format!("{}: {}", from, error)),
            }
        }
    }

    fn delete_selected(&self, remote: bool) {
        let pane = if remote { &self.remote } else { &self.local };
        let selected = pane.selected();
        if selected.is_empty() {
            self.say("Select something to delete");
            return;
        }
        let question = match selected.as_slice() {
            [(name, true)] => format!("Delete the folder '{}' and everything in it?", name),
            [(name, false)] => format!("Delete '{}'?", name),
            many => format!(
                "Delete these {} items, with everything in them?",
                many.len()
            ),
        };
        if !self.confirm(&question) {
            return;
        }
        let directory = pane.current();
        for (name, is_dir) in selected {
            let path = remote_files::join(&directory, &name);
            if remote {
                self.submit(Job::Delete { path, is_dir });
            } else {
                let result = if is_dir {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(error) = result {
                    self.say(&format!("{}: {}", path, error));
                }
            }
        }
        if !remote {
            self.reload_local();
        }
    }

    fn enter(&self, remote: bool, path: &gtk::TreePath) {
        let pane = if remote { &self.remote } else { &self.local };
        let Some(iter) = pane.store.iter(path) else {
            return;
        };
        let name: String = pane
            .store
            .value(&iter, COLUMN_NAME as i32)
            .get()
            .unwrap_or_default();
        let is_dir: bool = pane
            .store
            .value(&iter, COLUMN_IS_DIR as i32)
            .get()
            .unwrap_or(false);
        let full = remote_files::join(&pane.current(), &name);
        if is_dir {
            if remote {
                self.load_remote(&full);
            } else {
                self.load_local(&full);
            }
            return;
        }
        // Activating a file copies it to the other side, the quickest gesture.
        let to_dir = if remote {
            self.local.current()
        } else {
            self.remote.current()
        };
        self.enqueue_transfer(!remote, false, full, &to_dir, &name);
    }

    fn go_up(&self, remote: bool) {
        if remote {
            self.load_remote(&remote_files::parent(&self.remote.current()));
        } else {
            self.load_local(&remote_files::parent(&self.local.current()));
        }
    }

    fn apply(self: &Rc<Self>, outcome: Outcome) {
        self.finished();
        match outcome {
            Outcome::AuthStored => {}
            Outcome::Home(Ok(path)) => self.load_remote(&path),
            Outcome::Home(Err(error)) => {
                self.say(&format!(
                    "Could not connect: {}. A key, an agent, a shared connection or a \
                     password in the keyring is needed; use Password... for a password.",
                    error
                ));
            }
            Outcome::Listed(path, Ok(entries)) => {
                self.remote.path.set_text(&path);
                self.remote.fill(&entries);
                if self.queued.borrow().is_empty() {
                    self.say(&format!("{} items", entries.len()));
                }
            }
            Outcome::Listed(path, Err(error)) => self.say(&format!("{}: {}", path, error)),
            Outcome::Transferred {
                upload,
                label,
                result,
            } => {
                self.queued.borrow_mut().pop_front();
                match result {
                    Ok(()) => {
                        if self.queued.borrow().is_empty() {
                            self.say(&format!("Copied {}", label));
                        } else {
                            self.update_queue_text();
                        }
                        if upload {
                            self.reload_remote();
                        } else {
                            self.reload_local();
                        }
                    }
                    Err(error) => self.say(&format!("{}: {}", label, error)),
                }
            }
            Outcome::Synced {
                upload,
                from,
                to,
                result,
            } => match result {
                Ok(()) => {
                    self.say(&format!(
                        "Synced {} {} {}",
                        from,
                        if upload { "→" } else { "←" },
                        to
                    ));
                    self.reload_remote();
                    self.reload_local();
                }
                Err(error) => self.say(&format!("Sync failed: {}", error)),
            },
            Outcome::EditReady {
                local,
                remote,
                name,
                result,
            } => match result {
                Ok(()) => self.start_editing(local, remote, name),
                Err(error) => self.say(&format!("Could not download {}: {}", name, error)),
            },
            Outcome::EditUploaded {
                remote,
                name,
                result,
            } => {
                if let Some(session) = self
                    .edits
                    .borrow()
                    .iter()
                    .find(|session| session.remote == remote)
                {
                    session.uploading.set(false);
                }
                match result {
                    Ok(()) => {
                        self.say(&format!("Uploaded {}", name));
                        if self.remote.current() == remote_files::parent(&remote) {
                            self.reload_remote();
                        }
                    }
                    Err(error) => self.say(&format!("{}: {}", name, error)),
                }
            }
            Outcome::Changed(Ok(())) => self.reload_remote(),
            Outcome::Changed(Err(error)) => self.say(&error),
        }
    }
}

fn connect_pane(browser: &Rc<Browser>, remote: bool) {
    let pane = if remote {
        &browser.remote
    } else {
        &browser.local
    };
    let weak = Rc::downgrade(browser);
    let on = move |action: fn(&Browser, bool)| {
        let weak = weak.clone();
        move |_: &gtk::Button| {
            if let Some(browser) = weak.upgrade() {
                action(&browser, remote);
            }
        }
    };
    pane.up.connect_clicked(on(Browser::go_up));
    pane.new_folder.connect_clicked(on(Browser::make_folder));
    pane.rename.connect_clicked(on(Browser::rename_selected));
    pane.delete.connect_clicked(on(Browser::delete_selected));
    let weak = Rc::downgrade(browser);
    pane.bookmark.connect_clicked(move |_| {
        if let Some(browser) = weak.upgrade() {
            browser.add_bookmark(remote);
        }
    });
    let weak = Rc::downgrade(browser);
    pane.bookmarks.connect_clicked(move |button| {
        if let Some(browser) = weak.upgrade() {
            show_bookmarks_menu(&browser, remote, button);
        }
    });
    let weak = Rc::downgrade(browser);
    let upload = !remote;
    pane.sync.connect_clicked(move |_| {
        if let Some(browser) = weak.upgrade() {
            browser.sync_current(upload);
        }
    });
    if let Some(edit) = pane.edit.clone() {
        let weak = Rc::downgrade(browser);
        edit.connect_clicked(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.edit_selected();
            }
        });
    }
    let weak = Rc::downgrade(browser);
    pane.refresh.connect_clicked(move |_| {
        if let Some(browser) = weak.upgrade() {
            if remote {
                browser.reload_remote();
            } else {
                browser.reload_local();
            }
        }
    });
    let weak = Rc::downgrade(browser);
    pane.path.connect_activate(move |entry| {
        if let Some(browser) = weak.upgrade() {
            let path = entry.text().to_string();
            if remote {
                browser.load_remote(&path);
            } else {
                browser.load_local(&path);
            }
        }
    });
    let weak = Rc::downgrade(browser);
    pane.tree.connect_row_activated(move |_, path, _| {
        if let Some(browser) = weak.upgrade() {
            browser.enter(remote, path);
        }
    });
    let weak = Rc::downgrade(browser);
    pane.tree.connect_key_press_event(move |_, event| {
        use crate::keys::constants as K;
        let Some(browser) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let key = event.keyval();
        if key == K::Delete {
            browser.delete_selected(remote);
        } else if key == K::F2 {
            browser.rename_selected(remote);
        } else if key == K::F5 {
            if remote {
                browser.reload_remote();
            } else {
                browser.reload_local();
            }
        } else if key == K::BackSpace {
            browser.go_up(remote);
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    });

    // Drag between the panes, and from a file manager onto the server.
    let row_target = gtk::TargetEntry::new(DRAG_TARGET, gtk::TargetFlags::SAME_APP, 0);
    let uri_target = gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP, 1);
    pane.tree.drag_source_set(
        gdk::ModifierType::BUTTON1_MASK,
        std::slice::from_ref(&row_target),
        gdk::DragAction::COPY,
    );
    pane.tree.drag_dest_set(
        gtk::DestDefaults::ALL,
        &[row_target, uri_target],
        gdk::DragAction::COPY,
    );
    pane.tree.connect_drag_data_get(|_, _, data, _, _| {
        data.set(&data.target(), 8, b"rows");
    });
    let weak = Rc::downgrade(browser);
    pane.tree
        .connect_drag_data_received(move |tree, context, _, _, data, info, time| {
            tree.stop_signal_emission_by_name("drag-data-received");
            let Some(browser) = weak.upgrade() else {
                return;
            };
            if info == 0 {
                // From the other pane: dropping on the right uploads.
                if context.drag_get_source_widget().as_ref()
                    != Some(tree.upcast_ref::<gtk::Widget>())
                {
                    browser.transfer_selected(remote);
                }
            } else if remote {
                let to_dir = browser.remote.current();
                for uri in data.uris() {
                    let Ok((path, _)) = glib::filename_from_uri(&uri) else {
                        continue;
                    };
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let is_dir = path.is_dir();
                    browser.enqueue_transfer(
                        true,
                        is_dir,
                        path.display().to_string(),
                        &to_dir,
                        &name,
                    );
                }
            }
            context.drag_finish(true, false, time);
        });
}

/// The bookmarks menu: saved folders plus add/remove for the current one.
fn show_bookmarks_menu(browser: &Rc<Browser>, remote: bool, button: &gtk::Button) {
    let menu = gtk::Menu::new();
    let saved = browser.bookmarks_for(remote);
    if saved.is_empty() {
        let empty = gtk::MenuItem::with_label("No bookmarks yet");
        empty.set_sensitive(false);
        menu.append(&empty);
    } else {
        for bookmark in saved {
            let item =
                gtk::MenuItem::with_label(&format!("{}  —  {}", bookmark.name, bookmark.path));
            let weak = Rc::downgrade(browser);
            let path = bookmark.path.clone();
            item.connect_activate(move |_| {
                if let Some(browser) = weak.upgrade() {
                    if remote {
                        browser.load_remote(&path);
                    } else {
                        browser.load_local(&path);
                    }
                }
            });
            menu.append(&item);
        }
    }
    menu.append(&gtk::SeparatorMenuItem::new());

    let add = gtk::MenuItem::with_label("Add Bookmark for This Folder...");
    {
        let weak = Rc::downgrade(browser);
        add.connect_activate(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.add_bookmark(remote);
            }
        });
    }
    menu.append(&add);

    let remove = gtk::MenuItem::with_label("Remove Bookmark for This Folder");
    {
        let weak = Rc::downgrade(browser);
        remove.connect_activate(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.remove_bookmark(remote);
            }
        });
    }
    menu.append(&remove);

    menu.show_all();
    let menu_weak = menu.clone();
    menu.connect_deactivate(move |_| {
        let menu = menu_weak.clone();
        glib::idle_add_local_once(move || unsafe {
            menu.destroy();
        });
    });
    menu.popup_at_widget(
        button,
        gdk::Gravity::SouthWest,
        gdk::Gravity::NorthWest,
        None,
    );
}

/// Open a browser window for `connection`.
pub fn open(parent: &gtk::Window, connection: Connection) {
    let window = gtk::Window::new();
    window.set_title(&format!("Files: {}", connection.name));
    // Keep the browser above its terminal: without a transient parent some
    // window managers open it behind and it looks as if nothing happened.
    window.set_transient_for(Some(parent));
    window.set_destroy_with_parent(false);
    window.set_position(gtk::WindowPosition::CenterOnParent);
    window.set_default_size(1000, 620);
    if let Some(application) = parent.application() {
        window.set_application(Some(&application));
    }

    let (job_sender, job_receiver) = mpsc::channel();
    let (result_sender, result_receiver) = mpsc::channel();
    let worker_connection = connection.clone();
    std::thread::spawn(move || worker(worker_connection, job_receiver, result_sender));

    let local = Pane::build("This computer", false);
    let remote = Pane::build(&connection.name, true);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    let panes = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    panes.set_homogeneous(false);
    local.root.set_hexpand(true);
    remote.root.set_hexpand(true);
    panes.pack_start(&local.root, true, true, 0);
    let middle = gtk::Box::new(gtk::Orientation::Vertical, 6);
    middle.set_valign(gtk::Align::Center);
    let upload = gtk::Button::with_label("Upload →");
    upload.set_tooltip_text(Some("Copy the selection to the server"));
    let download = gtk::Button::with_label("← Download");
    download.set_tooltip_text(Some("Copy the selection to this computer"));
    middle.pack_start(&upload, false, false, 0);
    middle.pack_start(&download, false, false, 0);
    panes.pack_start(&middle, false, false, 0);
    panes.pack_start(&remote.root, true, true, 0);
    content.pack_start(&panes, true, true, 0);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    let status = gtk::Label::new(Some(if connection.no_sharing {
        "Connection sharing is off for this entry: every folder and transfer opens a new SSH session."
    } else {
        "Connecting..."
    }));
    status.set_xalign(0.0);
    status.set_line_wrap(true);
    let password = gtk::Button::with_label("Password...");
    password.set_tooltip_text(Some(
        "Store this host's password in the keyring and reconnect",
    ));
    footer.pack_start(&spinner, false, false, 0);
    footer.pack_start(&status, true, true, 0);
    footer.pack_end(&password, false, false, 0);
    content.pack_start(&footer, false, false, 0);
    window.add(&content);

    let browser = Rc::new(Browser {
        window: window.clone(),
        connection: connection.clone(),
        local,
        remote,
        status,
        spinner,
        jobs: job_sender,
        results: result_receiver,
        pending: Cell::new(0),
        polling: Cell::new(false),
        local_generation: Cell::new(0),
        queued: RefCell::new(VecDeque::new()),
        edits: RefCell::new(Vec::new()),
        me: RefCell::new(std::rc::Weak::new()),
    });
    *browser.me.borrow_mut() = Rc::downgrade(&browser);
    connect_pane(&browser, false);
    connect_pane(&browser, true);
    {
        let weak = Rc::downgrade(&browser);
        upload.connect_clicked(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.transfer_selected(true);
            }
        });
        let weak = Rc::downgrade(&browser);
        download.connect_clicked(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.transfer_selected(false);
            }
        });
        let weak = Rc::downgrade(&browser);
        password.connect_clicked(move |_| {
            if let Some(browser) = weak.upgrade() {
                browser.prompt_password();
            }
        });
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    browser.load_local(&home.to_string_lossy());
    browser.submit(Job::Home);

    // The window's destroy handler keeps the browser alive until it closes. Dropping
    // the browser drops the job sender, which ends the worker after its current job.
    {
        let keep_alive = RefCell::new(Some(browser.clone()));
        window.connect_destroy(move |_| {
            keep_alive.borrow_mut().take();
        });
    }
    browser.start_polling();
    window.show_all();
    window.present();
}
