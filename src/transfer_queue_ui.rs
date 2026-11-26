//! GTK window for the parallel transfer queue.
//!
//! The queue itself is display-free (`headless::transfer_queue`): this module
//! owns the workers and is the only place that touches GTK. Every state change
//! is reported back to the main thread with `crate::run_on_main`, so the
//! `ListStore` is never mutated from a worker. The queue lives in a
//! thread-local manager that outlives the window: closing the window hides it,
//! it does not throw the jobs away.
//!
//! `sftp`/`lftp` do not report byte counts through `remote_files::transfer`, so
//! a running job with no known size is shown as an indeterminate pulse rather
//! than a made-up percentage. Pause stops the child process; resume starts that
//! transfer again from the beginning (the remote programs cannot resume a
//! partial copy).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;

use crate::headless::connections::Connection;
use crate::headless::remote_files::{self, human_size};
use crate::headless::transfer_queue::{
    TransferDirection, TransferJob, TransferQueue, TransferState,
};

const COL_ID: u32 = 0;
const COL_DIRECTION: u32 = 1;
const COL_NAME: u32 = 2;
const COL_STATE: u32 = 3;
const COL_PROGRESS: u32 = 4;
const COL_PULSE: u32 = 5;
const COL_SIZE: u32 = 6;
const COL_DETAIL: u32 = 7;

/// Number of copies allowed at the same time. The queue state machine clamps
/// zero to one; three is a useful default for separate `sftp` sessions.
const MAX_CONCURRENT: usize = 3;

#[derive(Clone)]
struct JobMeta {
    is_dir: bool,
    connection: Connection,
    /// Set to stop the child process on pause/cancel.
    cancel: Arc<AtomicBool>,
}

/// Owns the queue, the per-job worker data and the (optional) window.
pub(crate) struct TransferManager {
    /// Shared with every worker through `crate::Sendable`.
    queue: Rc<RefCell<TransferQueue>>,
    meta: RefCell<HashMap<u64, JobMeta>>,
    window: RefCell<Option<Rc<TransferWindow>>>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
}

thread_local! {
    static MANAGER: Rc<TransferManager> = Rc::new(TransferManager::new());
}

pub(crate) fn manager() -> Rc<TransferManager> {
    MANAGER.with(Rc::clone)
}

/// The legacy single-file worker is kept as a fallback. Setting this variable
/// before starting Oxterm selects it instead of the parallel queue.
pub(crate) fn enabled() -> bool {
    std::env::var_os("OXTERM_LEGACY_TRANSFERS").is_none()
}

/// Add a transfer to the process-wide queue. `None` means the parallel queue is
/// unavailable and the caller should use its older single-file worker.
pub(crate) fn try_enqueue(
    direction: TransferDirection,
    local: String,
    remote: String,
    is_dir: bool,
    connection: &Connection,
) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let manager = manager();
    let id = manager.queue.borrow_mut().enqueue(direction, local, remote);
    manager.meta.borrow_mut().insert(
        id,
        JobMeta {
            is_dir,
            connection: connection.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );
    manager.changed();
    Some(id)
}

/// Show (or raise) the transfer window, owned by `parent`.
pub(crate) fn show_transfer_queue(parent: &gtk::Window) {
    let manager = manager();
    let window = match manager.window.borrow().clone() {
        Some(window) => window,
        None => {
            let window = TransferWindow::build(Rc::downgrade(&manager));
            *manager.window.borrow_mut() = Some(window.clone());
            window
        }
    };
    window.window.set_transient_for(Some(parent));
    window.window.present();
    window.refresh();
    window.start_timer();
}

/// notified on every queue state change, used by the browser to reload panes.
pub(crate) fn subscribe<F: Fn() + 'static>(listener: F) {
    manager().listeners.borrow_mut().push(Rc::new(listener));
}

impl TransferManager {
    fn new() -> Self {
        Self {
            queue: Rc::new(RefCell::new(TransferQueue::new(MAX_CONCURRENT))),
            meta: RefCell::new(HashMap::new()),
            window: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
        }
    }

    /// Refresh the view, start as many queued jobs as there are free slots and
    /// tell the browser that files may have changed.
    fn changed(&self) {
        self.refresh();
        self.schedule();
        let listeners = self.listeners.borrow().clone();
        for listener in listeners {
            listener();
        }
    }

    fn refresh(&self) {
        if let Some(window) = self.window.borrow().clone() {
            window.refresh();
        }
    }

    fn schedule(&self) {
        let ids: Vec<u64> = self
            .queue
            .borrow()
            .next_schedulable()
            .iter()
            .map(|job| job.id)
            .collect();
        for id in ids {
            if self.queue.borrow_mut().start(id).is_err() {
                continue;
            }
            self.spawn(id);
        }
        self.refresh();
    }

    /// Run one queued transfer on its own thread. All GTK access happens in the
    /// `run_on_main` closure posted when the program ends.
    fn spawn(&self, id: u64) {
        let Some(meta) = self.meta.borrow().get(&id).cloned() else {
            return;
        };
        let (local, remote, direction) = {
            let queue = self.queue.borrow();
            let Some(job) = queue.get(id) else {
                return;
            };
            (job.local.clone(), job.remote.clone(), job.direction)
        };
        let upload = direction == TransferDirection::Upload;
        let cancel = meta.cancel.clone();
        cancel.store(false, Ordering::SeqCst);
        let connection = meta.connection;
        let is_dir = meta.is_dir;
        // The Rc<RefCell<_>> is only dereferenced after hopping back to the
        // main thread; `Sendable` carries it across without touching GTK.
        let shared_queue = crate::Sendable(self.queue.clone());
        let shared_manager = crate::Sendable(manager());
        std::thread::spawn(move || {
            let result =
                remote_files::transfer(&connection, upload, is_dir, &local, &remote, &cancel);
            crate::run_on_main(move || {
                // Keep the shared Rc<RefCell<TransferQueue>> with the callback:
                // it is only borrowed again once we are back on the main thread.
                let _queue_on_main_thread = shared_queue;
                shared_manager.worker_done(id, result);
            });
        });
    }

    fn worker_done(&self, id: u64, result: Result<(), remote_files::TransferError>) {
        let state = self.queue.borrow().get(id).map(|job| job.state.clone());
        // A paused job's child was killed on purpose; keep the last honest byte
        // counts and let Resume start a fresh copy.
        if let Some(TransferState::Running { .. }) = state {
            let outcome = result.map_err(|error| error.message());
            let _ = self.queue.borrow_mut().finish(id, outcome);
        }
        self.changed();
    }

    fn pause(&self, id: u64) -> Result<(), String> {
        self.queue.borrow_mut().pause(id)?;
        self.signal(id, true);
        self.changed();
        Ok(())
    }

    fn resume(&self, id: u64) -> Result<(), String> {
        self.queue.borrow_mut().resume(id)?;
        // `sftp` cannot continue a partial copy: restart without pretending
        // that the old byte counts still apply.
        self.queue.borrow_mut().set_progress(id, 0, 0)?;
        self.spawn(id);
        self.changed();
        Ok(())
    }

    fn cancel(&self, id: u64) -> Result<(), String> {
        self.queue.borrow_mut().cancel(id)?;
        self.signal(id, true);
        self.changed();
        Ok(())
    }

    fn cancel_all(&self) {
        let ids: Vec<u64> = self
            .queue
            .borrow()
            .pending()
            .iter()
            .map(|job| job.id)
            .collect();
        for id in ids {
            let _ = self.cancel(id);
        }
    }

    fn signal(&self, id: u64, stop: bool) {
        if let Some(meta) = self.meta.borrow().get(&id) {
            meta.cancel.store(stop, Ordering::SeqCst);
        }
    }
}

struct TransferWindow {
    window: gtk::Window,
    store: gtk::ListStore,
    tree: gtk::TreeView,
    status: gtk::Label,
    pause: gtk::Button,
    resume: gtk::Button,
    cancel: gtk::Button,
    cancel_all: gtk::Button,
    manager: Weak<TransferManager>,
    /// Drives the activity-mode progress cells; no byte count is implied.
    pulse: Cell<u32>,
    timer: Cell<bool>,
}

impl TransferWindow {
    fn build(manager: Weak<TransferManager>) -> Rc<Self> {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Transfers");
        window.set_destroy_with_parent(false);
        window.set_position(gtk::WindowPosition::CenterOnParent);
        window.set_default_size(880, 440);

        let store = gtk::ListStore::new(&[
            glib::Type::U64,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::I32,
            glib::Type::I32,
            glib::Type::STRING,
            glib::Type::STRING,
        ]);
        let tree = gtk::TreeView::with_model(&store);
        tree.set_tooltip_column(COL_DETAIL as i32);
        tree.set_enable_search(true);
        tree.set_search_column(COL_NAME as i32);
        tree.selection().set_mode(gtk::SelectionMode::Multiple);

        let column = gtk::TreeViewColumn::new();
        column.set_title("Direction");
        let renderer = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "text", COL_DIRECTION as i32);
        tree.append_column(&column);

        let column = gtk::TreeViewColumn::new();
        column.set_title("Name");
        column.set_expand(true);
        let renderer = gtk::CellRendererText::new();
        renderer.set_ellipsize(pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "text", COL_NAME as i32);
        tree.append_column(&column);

        let column = gtk::TreeViewColumn::new();
        column.set_title("Status");
        column.set_resizable(true);
        let renderer = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "text", COL_STATE as i32);
        tree.append_column(&column);

        let column = gtk::TreeViewColumn::new();
        column.set_title("Progress");
        column.set_resizable(true);
        let renderer = gtk::CellRendererProgress::new();
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "value", COL_PROGRESS as i32);
        TreeViewColumnExt::add_attribute(&column, &renderer, "pulse", COL_PULSE as i32);
        TreeViewColumnExt::add_attribute(&column, &renderer, "text", COL_STATE as i32);
        tree.append_column(&column);

        let column = gtk::TreeViewColumn::new();
        column.set_title("Size");
        let renderer = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&column, &renderer, true);
        TreeViewColumnExt::add_attribute(&column, &renderer, "text", COL_SIZE as i32);
        tree.append_column(&column);

        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_shadow_type(gtk::ShadowType::In);
        scroll.set_vexpand(true);
        scroll.add(&tree);

        let status = gtk::Label::new(Some("No transfers"));
        status.set_xalign(0.0);
        status.set_line_wrap(true);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let pause = gtk::Button::with_label("Pause");
        pause.set_tooltip_text(Some("Stop the selected copy; Resume starts it again"));
        let resume = gtk::Button::with_label("Resume");
        resume.set_tooltip_text(Some("Copy the selected file again from the beginning"));
        let cancel = gtk::Button::with_label("Cancel");
        let cancel_all = gtk::Button::with_label("Cancel all");
        let close = gtk::Button::with_label("Close");
        actions.pack_start(&pause, false, false, 0);
        actions.pack_start(&resume, false, false, 0);
        actions.pack_start(&cancel, false, false, 0);
        actions.pack_start(&cancel_all, false, false, 0);
        actions.pack_end(&close, false, false, 0);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.pack_start(&scroll, true, true, 0);
        content.pack_start(&status, false, false, 0);
        content.pack_start(&actions, false, false, 0);
        window.add(&content);

        let this = Rc::new(Self {
            window,
            store,
            tree,
            status,
            pause,
            resume,
            cancel,
            cancel_all,
            manager,
            pulse: Cell::new(0),
            timer: Cell::new(false),
        });

        let weak = Rc::downgrade(&this);
        this.tree.selection().connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.update_actions();
            }
        });
        let weak = Rc::downgrade(&this);
        this.pause.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if let (Some(id), Some(manager)) = (this.selected_id(), this.manager.upgrade()) {
                if let Err(error) = manager.pause(id) {
                    this.say(&error);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.resume.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if let (Some(id), Some(manager)) = (this.selected_id(), this.manager.upgrade()) {
                if let Err(error) = manager.resume(id) {
                    this.say(&error);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.cancel.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if let (Some(id), Some(manager)) = (this.selected_id(), this.manager.upgrade()) {
                if let Err(error) = manager.cancel(id) {
                    this.say(&error);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.cancel_all.connect_clicked(move |_| {
            if let Some(manager) = weak.upgrade().and_then(|this| this.manager.upgrade()) {
                manager.cancel_all();
            }
        });
        let weak = Rc::downgrade(&this);
        close.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.timer.set(false);
                this.window.hide();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_delete_event(move |window, _| {
            if let Some(this) = weak.upgrade() {
                this.timer.set(false);
            }
            // Hiding keeps the window (and its store) available for the next
            // opening; the queue itself lives in the manager in any case.
            window.hide();
            glib::Propagation::Stop
        });

        this.update_actions();
        this
    }

    fn start_timer(self: &Rc<Self>) {
        if self.timer.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !this.window.is_visible() {
                this.timer.set(false);
                return glib::ControlFlow::Break;
            }
            this.pulse.set(this.pulse.get().wrapping_add(1) % 101);
            this.refresh();
            glib::ControlFlow::Continue
        });
    }

    fn say(&self, message: &str) {
        self.status.set_text(message);
    }

    fn selected_id(&self) -> Option<u64> {
        let (paths, _) = self.tree.selection().selected_rows();
        let iter = self.store.iter(paths.first()?)?;
        self.store.value(&iter, COL_ID as i32).get().ok()
    }

    fn update_actions(&self) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let state = self
            .selected_id()
            .and_then(|id| manager.queue.borrow().get(id).map(|job| job.state.clone()));
        self.pause
            .set_sensitive(matches!(state, Some(TransferState::Running { .. })));
        self.resume
            .set_sensitive(matches!(state, Some(TransferState::Paused { .. })));
        self.cancel.set_sensitive(matches!(
            state,
            Some(TransferState::Queued)
                | Some(TransferState::Running { .. })
                | Some(TransferState::Paused { .. })
        ));
        self.cancel_all
            .set_sensitive(!manager.queue.borrow().pending().is_empty());
    }

    fn refresh(&self) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let jobs = manager.queue.borrow().jobs().to_vec();
        self.store.clear();
        for job in &jobs {
            let direction = if job.direction == TransferDirection::Upload {
                "Upload"
            } else {
                "Download"
            };
            let name = job_name(job);
            let (status, progress, pulse, size) = Self::describe(job, self.pulse.get());
            let detail = format!(
                "{}\nlocal: {}\nremote: {}\n{}",
                direction, job.local, job.remote, status
            );
            self.store.set(
                &self.store.append(),
                &[
                    (COL_ID, &job.id),
                    (COL_DIRECTION, &direction),
                    (COL_NAME, &name),
                    (COL_STATE, &status),
                    (COL_PROGRESS, &progress),
                    (COL_PULSE, &pulse),
                    (COL_SIZE, &size),
                    (COL_DETAIL, &detail),
                ],
            );
        }
        let active = manager.queue.borrow().active_count();
        let queued = manager.queue.borrow().queued().len();
        self.status
            .set_text(&format!("{} copying, {} waiting", active, queued));
        self.update_actions();
    }

    /// Never invent a percentage: without byte counts the cell uses GTK's
    /// activity mode (`pulse >= 0`) and the size column reports what is known.
    fn describe(job: &TransferJob, pulse: u32) -> (String, i32, i32, String) {
        let (done, total) = match &job.state {
            TransferState::Running { done, total } | TransferState::Paused { done, total } => {
                (*done, *total)
            }
            _ => (0, 0),
        };
        let percentage = || -> i32 {
            done.saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(100) as i32
        };
        let (status, value, activity) = match &job.state {
            TransferState::Queued => ("Queued".to_string(), 0, -1),
            TransferState::Running { total: 0, .. } => ("Copying".to_string(), 0, pulse as i32),
            TransferState::Running { .. } => ("Copying".to_string(), percentage(), -1),
            TransferState::Paused { total: 0, .. } => ("Paused".to_string(), 0, -1),
            TransferState::Paused { .. } => ("Paused".to_string(), percentage(), -1),
            TransferState::Done => ("Done".to_string(), 100, -1),
            TransferState::Failed(message) => (format!("Failed: {}", message), 0, -1),
            TransferState::Cancelled => ("Cancelled".to_string(), 0, -1),
        };
        let size = if total > 0 {
            format!("{} / {}", human_size(done), human_size(total))
        } else if std::fs::metadata(&job.local)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
        {
            std::fs::metadata(&job.local)
                .map(|metadata| human_size(metadata.len()))
                .unwrap_or_else(|_| "—".to_string())
        } else {
            "—".to_string()
        };
        (status, value, activity, size)
    }
}

fn job_name(job: &TransferJob) -> String {
    let path = if job.direction == TransferDirection::Upload {
        &job.local
    } else {
        &job.remote
    };
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone())
}
