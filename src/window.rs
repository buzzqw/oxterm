use std::cell::{Cell, RefCell};

use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::gdk;
#[allow(non_snake_case)]
mod K {
    pub use gdk::keys::constants::*;
}
use gtk::prelude::*;

use zoha_vte::traits::TerminalExt;

use crate::notes::{NoteContext, NotesManager};
use crate::settings::settings;
use crate::terminal::TerminalBox;
pub const SIGNALS: &[(&str, i32)] = &[
    ("SIGHUP (1)", 1),
    ("SIGINT (2)", 2),
    ("SIGQUIT (3)", 3),
    ("SIGILL (4)", 4),
    ("SIGABRT (6)", 6),
    ("SIGKILL (9)", 9),
    ("SIGUSR1 (10)", 10),
    ("SIGSEGV (11)", 11),
    ("SIGUSR2 (12)", 12),
    ("SIGPIPE (13)", 13),
    ("SIGTERM (15)", 15),
    ("SIGSTOP (19)", 19),
    ("SIGTSTP (20)", 20),
    ("SIGCONT (18)", 18),
];

pub const ENCODINGS: &[&str] = &[
    "UTF-8",
    "ISO-8859-1",
    "ISO-8859-15",
    "UTF-16",
    "UTF-16BE",
    "UTF-16LE",
    "CP1252",
    "CP850",
    "ASCII",
    "KOI8-R",
    "Shift_JIS",
    "EUC-JP",
    "GBK",
];

pub const EUPL_LICENSE_TEXT: &str = "Licensed under the European Union Public Licence (EUPL) v. 1.2.\n\nCopyright (c) 2026 Andres Zanzani.\n\nThe complete licence text is distributed in the LICENSE file and is available at:\nhttps://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12\n";

pub const VERSION: &str = env!("OXTERM_VERSION");
pub const APP_NAME: &str = "Oxterm";
pub const APP_TITLE: &str = "Oxterm Terminal";

pub fn detect_file_manager() -> Option<String> {
    let s = settings();
    let fm = s.get_str("file_manager");
    if !fm.is_empty() && crate::notes::which(&fm).is_some() {
        return Some(fm);
    }
    for candidate in [
        "nemo",
        "thunar",
        "dolphin",
        "nautilus",
        "pcmanfm",
        "caja",
        "nemo-desktop",
        "spacefm",
    ] {
        if crate::notes::which(candidate).is_some() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Build the in-app information bar used to report note actions without
/// polluting the terminal scrollback with escape sequences.
fn build_info_bar() -> (gtk::InfoBar, gtk::Label) {
    let bar = gtk::InfoBar::new();
    bar.set_no_show_all(true);
    bar.set_show_close_button(true);
    let label = gtk::Label::new(None);
    label.set_line_wrap(true);
    label.set_halign(gtk::Align::Start);
    bar.content_area().pack_start(&label, true, true, 0);
    bar.connect_response(|bar, _| bar.hide());
    (bar, label)
}

/// Show a transient message in a window's info bar, hiding it after a few
/// seconds. No-op when the window has no info bar.
fn paned_size(paned: &gtk::Paned) -> i32 {
    if paned.orientation() == gtk::Orientation::Horizontal {
        paned.allocated_width()
    } else {
        paned.allocated_height()
    }
}

fn show_note_feedback(
    bar: &RefCell<Option<gtk::InfoBar>>,
    label: &RefCell<Option<gtk::Label>>,
    message: &str,
    is_error: bool,
) {
    let (Some(bar_widget), Some(label_widget)) = (bar.borrow().clone(), label.borrow().clone())
    else {
        return;
    };
    label_widget.set_text(message);
    bar_widget.set_message_type(if is_error {
        gtk::MessageType::Error
    } else {
        gtk::MessageType::Info
    });
    bar_widget.show_all();
    glib::timeout_add_seconds_local(5, move || {
        bar_widget.hide();
        glib::ControlFlow::Break
    });
}

/// Widgets that make up one tab: the title, its close button container and the
/// small red dot shown when the last command failed.
pub struct TabLabel {
    pub event_box: gtk::EventBox,
    pub label: gtk::Label,
    pub indicator: gtk::Label,
}

/// Interface implemented by both window types so a terminal can drive its
/// host window without knowing which concrete kind it is.
pub trait TerminalWindow {
    fn new_tab_signal(&self);
    fn new_window_signal(&self);
    fn close_tab_signal(&self, term: Option<&TerminalBox>);
    fn close_window_signal(&self);
    fn set_title_dialog(&self);
    fn reset_terminal(&self);
    fn reset_and_clear(&self);
    fn split_signal(&self, mode: &str);
    fn toggle_split_zoom_signal(&self) {}
    fn equalize_splits_signal(&self) {}
    /// Mark (or clear) the failed-command dot on the tab that owns `term`.
    fn set_tab_failure_signal(&self, term: &TerminalBox, failed: bool) {
        let _ = (term, failed);
    }
    fn clear_tab_failure_signal(&self, term: &TerminalBox) {
        let _ = term;
    }
    fn focus_other_pane_signal(&self);
    fn prev_tab_signal(&self);
    fn next_tab_signal(&self);
    fn session_list_signal(&self);
    /// Rename the tab that owns `term`. Windows without tabs (a detached
    /// terminal) leave this as a no-op.
    fn rename_tab_signal(&self, term: &TerminalBox) {
        let _ = term;
    }
    /// Move `term` into a separate window. Only the tabbed main window
    /// implements this.
    fn detach_tab_signal(&self, term: &TerminalBox) {
        let _ = term;
    }
    fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str);
    /// The shell changed the directory it reports through OSC 7. Display-only:
    /// the tooltip and the status bar follow it, nothing else does.
    fn refresh_cwd_display(&self, _term: &TerminalBox) {}
    fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]);

    // ── Notes ────────────────────────────────────────────────
    // Required hooks implemented by each window type.
    fn note_feedback(&self, message: &str, is_error: bool);
    fn note_window(&self) -> gtk::Window;
    fn note_terminal(&self) -> Option<TerminalBox>;
    fn note_write(&self, text: String, filename: Option<String>, context: NoteContext);

    /// Show a transient message in the window's info bar.
    fn status_message(&self, message: &str, is_error: bool);

    /// Open a new tab running `command` in `cwd`. Used by the command-output
    /// pager; windows without tabs simply ignore it.
    fn open_command_tab(&self, _cwd: &str, _command: Vec<String>, _title: &str) {}

    /// Toggle the integrated notes panel (only the main window has one).
    fn note_toggle_panel(&self) {
        self.note_feedback("The Notes panel is available in the main window", true);
    }

    /// Refresh the notes panel after a note is written, when it is open.
    fn note_refresh_panel_if_open(&self) {}

    /// Append the last command output (OSC 133) to a note.
    fn note_add_command_output(&self) {
        let Some(term) = self.note_terminal() else {
            return;
        };
        let (Some(start_row), Some(end_row)) = term.get_command_output_range() else {
            self.note_feedback(
                "No command output available (shell integration required)",
                true,
            );
            return;
        };
        let end_row = end_row.min(start_row + 2000);
        let text =
            crate::terminal::text::range(&term.vte(), start_row, 0, end_row, 0).unwrap_or_default();
        if text.trim().is_empty() {
            self.note_feedback("No command output available", true);
            return;
        }
        let window = self.note_window();
        let Some(name) = crate::notes_ui::pick_note_file(&window, "Note command output") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        let command = term.last_command();
        let exit_code = term.last_exit_code();
        let context = NoteContext::from_command(&term.get_cwd(), &command, exit_code);
        self.note_write(text, filename, context);
    }

    /// Compose a new note from scratch, pre-filled with the configured template.
    fn note_new(&self) {
        let window = self.note_window();
        let (cwd, command) = match self.note_terminal() {
            Some(term) => (term.get_cwd(), term.last_command()),
            None => (String::new(), String::new()),
        };
        let template =
            crate::notes::expand_template(&settings().get_str("notes_template"), &cwd, &command);
        if let Some((name, text)) = crate::notes_ui::compose_note(&window, "New Note", &template) {
            let filename = if name.is_empty() { None } else { Some(name) };
            self.note_write(text, filename, NoteContext::manual());
        }
    }

    /// Append the current terminal selection to a chosen note.
    fn note_add_selection(&self) {
        let Some(term) = self.note_terminal() else {
            return;
        };
        let text = term
            .vte()
            .text_selected(zoha_vte::Format::Text)
            .map(|t| t.to_string())
            .unwrap_or_default();
        if text.trim().is_empty() {
            self.note_feedback("No text selected in the terminal", true);
            return;
        }
        let window = self.note_window();
        let Some(name) = crate::notes_ui::pick_note_file(&window, "Add selection to note") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        let context = NoteContext::from_terminal("selection", &term.get_cwd());
        self.note_write(text, filename, context);
    }

    /// Append the whole terminal buffer to a chosen note, after confirmation.
    fn note_add_all(&self) {
        let Some(term) = self.note_terminal() else {
            return;
        };
        let vte = term.vte();
        let vadj = term.vadjustment();
        let first_row = vadj.lower() as i64;
        let last_row = (vadj.upper() as i64).max(first_row);
        let text =
            crate::terminal::text::range(&vte, first_row, 0, last_row, 0).unwrap_or_default();
        if text.trim().is_empty() {
            self.note_feedback("Nothing to copy from the terminal", true);
            return;
        }
        let window = self.note_window();
        let lines = text.lines().count();
        let chars = text.chars().count();
        if !crate::notes_ui::confirm_large_append(&window, "all terminal text", lines, chars) {
            return;
        }
        let Some(name) = crate::notes_ui::pick_note_file(&window, "Copy all to note") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        let context = NoteContext::from_terminal("scrollback", &term.get_cwd());
        self.note_write(text, filename, context);
    }

    /// Open the default note in the configured external editor.
    fn note_open_default(&self) {
        match NotesManager::new().open_notes(None) {
            Ok(path) => self.note_feedback(&format!("Opening note: {}", path.display()), false),
            Err(e) => self.note_feedback(&format!("Could not open note: {}", e), true),
        }
    }

    /// Open the notes directory in the file manager (or `xdg-open`).
    fn note_open_folder(&self) {
        match crate::notes::open_notes_folder() {
            Ok(path) => self.note_feedback(&format!("Notes folder: {}", path.display()), false),
            Err(e) => self.note_feedback(&format!("Could not open notes folder: {}", e), true),
        }
    }

    /// Pick a note and open it in the external editor.
    fn note_choose_open(&self) {
        let window = self.note_window();
        let Some(name) = crate::notes_ui::pick_note_file(&window, "Open note") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        match NotesManager::new().open_notes(filename.as_deref()) {
            Ok(path) => self.note_feedback(&format!("Opening note: {}", path.display()), false),
            Err(e) => self.note_feedback(&format!("Could not open note: {}", e), true),
        }
    }
}

// ── DetachedWindow ───────────────────────────────────────────

mod det_imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct DetachedWindow {
        pub terminal: RefCell<Option<TerminalBox>>,
        pub settings_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub headerbar: RefCell<Option<gtk::HeaderBar>>,
        pub toolbar: RefCell<Option<gtk::Box>>,
        pub menubar: RefCell<Option<gtk::Box>>,
        pub menu_buttons: RefCell<Vec<gtk::MenuButton>>,
        pub accel_group: RefCell<Option<gtk::AccelGroup>>,
        pub stats_sys_label: RefCell<Option<gtk::Label>>,
        pub stats_self_label: RefCell<Option<gtk::Label>>,
        pub stats_box: RefCell<Option<gtk::Box>>,
        pub cwd_label: RefCell<Option<gtk::Label>>,
        pub info_bar: RefCell<Option<gtk::InfoBar>>,
        pub info_label: RefCell<Option<gtk::Label>>,
        pub stats_source_id: RefCell<Option<glib::SourceId>>,
        pub remote_stats_pending: RefCell<bool>,
        pub remote_stats_generation: RefCell<u64>,
        pub closing: RefCell<bool>,
        pub encoding_actions: RefCell<Vec<(gtk::CheckMenuItem, String)>>,
        /// (columns, rows, font size) last used to size the window; settings
        /// changes only resize when one of these actually changes.
        pub applied_size_key: Cell<Option<WindowSizeKey>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DetachedWindow {
        const NAME: &'static str = "TpgkDetachedWindow";
        type Type = super::DetachedWindow;
        type ParentType = gtk::Window;
    }

    impl ObjectImpl for DetachedWindow {}

    impl WidgetImpl for DetachedWindow {}

    impl ContainerImpl for DetachedWindow {}

    impl BinImpl for DetachedWindow {}

    impl WindowImpl for DetachedWindow {}
}

glib::wrapper! {
    pub struct DetachedWindow(ObjectSubclass<det_imp::DetachedWindow>)
        @extends gtk::Window, gtk::Bin, gtk::Container, gtk::Widget;
}

impl DetachedWindow {
    fn update_cwd_badge(&self, term: &TerminalBox) {
        let Some(label) = self.imp().cwd_label.borrow().clone() else {
            return;
        };
        match term.cwd_badge() {
            Some(path) => {
                label.set_text(&path);
                label.show();
            }
            None => label.hide(),
        }
    }

    fn refresh_active_cwd_badge(&self) {
        if let Some(term) = self.terminal_box() {
            self.update_cwd_badge(&term);
        }
    }

    pub fn terminal_box(&self) -> Option<TerminalBox> {
        self.imp().terminal.borrow().clone()
    }

    pub fn new(terminal: &TerminalBox, title: &str) -> DetachedWindow {
        let this: DetachedWindow = glib::Object::new();
        this.update_title(&format!("{} - {}", APP_NAME, title));
        *this.imp().terminal.borrow_mut() = Some(terminal.clone());
        this.apply_window_visuals();
        this.apply_window_size();

        // Directory the shell reports through OSC 7 when it differs from the
        // process directory: an SSH tab, or a shell that moved itself. Hidden
        // when both agree, so a local tab keeps the plain stats bar.
        let cwd_label = gtk::Label::new(None);
        cwd_label.set_halign(gtk::Align::Start);
        cwd_label.set_ellipsize(pango::EllipsizeMode::Start);
        cwd_label.set_max_width_chars(48);
        cwd_label.set_single_line_mode(true);
        cwd_label.set_margin_end(12);
        cwd_label.style_context().add_class("oxterm-stats-label");
        cwd_label.set_tooltip_text(Some("Directory reported by the shell (OSC 7)"));
        cwd_label.set_no_show_all(true);
        cwd_label.hide();
        let stats_sys_label = gtk::Label::new(Some(""));
        stats_sys_label.set_halign(gtk::Align::Start);
        stats_sys_label
            .style_context()
            .add_class("oxterm-stats-label");
        let stats_self_label = gtk::Label::new(Some(""));
        stats_self_label.set_halign(gtk::Align::End);
        stats_self_label
            .style_context()
            .add_class("oxterm-stats-label");
        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        stats_box.pack_start(&cwd_label, false, false, 0);
        stats_box.pack_start(&stats_sys_label, true, true, 0);
        stats_box.pack_end(&stats_self_label, false, false, 0);
        stats_box.set_no_show_all(true);
        *this.imp().stats_sys_label.borrow_mut() = Some(stats_sys_label);
        *this.imp().stats_self_label.borrow_mut() = Some(stats_self_label);
        *this.imp().stats_box.borrow_mut() = Some(stats_box.clone());
        *this.imp().cwd_label.borrow_mut() = Some(cwd_label);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (info_bar, info_label) = build_info_bar();
        *this.imp().info_bar.borrow_mut() = Some(info_bar.clone());
        *this.imp().info_label.borrow_mut() = Some(info_label);
        vbox.pack_start(&info_bar, false, false, 0);
        this.add(&vbox);

        let accel_group = gtk::AccelGroup::new();
        this.add_accel_group(&accel_group);
        *this.imp().accel_group.borrow_mut() = Some(accel_group);

        this.build_headerbar(title);

        let menubar = this.imp().menubar.borrow().clone().unwrap();
        let toolbar = this.imp().toolbar.borrow().clone().unwrap();
        vbox.pack_start(&menubar, false, false, 0);
        vbox.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        vbox.pack_start(terminal, true, true, 0);
        vbox.pack_end(&stats_box, false, false, 0);

        toolbar.set_visible(settings().get_bool("show_toolbar"));
        menubar.set_visible(settings().get_bool("show_menubar"));
        this.apply_stats_visibility();

        // One handler per window: each settings change re-applies everything
        // this window derives from settings.
        let handler = settings().connect_changed({
            let w = this.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_window_size();
                    w.apply_window_visuals();
                    w.apply_ui_visibility();
                }
            }
        });
        this.imp().settings_handlers.borrow_mut().push(handler);
        // Settings is a process-wide object: drop our handlers however the
        // window goes away, not only through on_close.
        this.connect_destroy(|w| {
            for handler in std::mem::take(&mut *w.imp().settings_handlers.borrow_mut()) {
                settings().disconnect_changed(handler);
            }
        });

        let weak = this.downgrade();
        this.connect_delete_event(move |w, _| {
            if let Some(w2) = weak.upgrade() {
                return w2.on_close();
            }
            let _ = w;
            glib::Propagation::Proceed
        });
        let weak = this.downgrade();
        this.connect_key_press_event(move |w, ev| {
            if let Some(w2) = weak.upgrade() {
                return w2.on_window_key(w.upcast_ref(), ev);
            }
            glib::Propagation::Proceed
        });

        this.show_all();
        let weak = this.downgrade();
        glib::idle_add_local(move || {
            if let Some(w) = weak.upgrade() {
                w.populate_menus();
                if let Some(t) = w.imp().terminal.borrow().clone() {
                    t.vte().grab_focus();
                }
            }
            glib::ControlFlow::Break
        });
        this
    }

    fn apply_window_size(&self) {
        let key = window_size_key();
        if self.imp().applied_size_key.replace(Some(key)) == Some(key) {
            return;
        }
        let (w, h) = window_size_for(key);
        self.resize(w, h);
    }

    fn apply_window_visuals(&self) {
        let s = settings();
        let opacity = (s.get_f64("opacity") * 100.0).round() / 100.0;
        if s.get_bool("enable_transparency") {
            self.set_opacity(opacity);
            if let Some(screen) = gtk::prelude::WidgetExt::screen(self) {
                if let Some(visual) = screen.rgba_visual() {
                    self.set_app_paintable(true);
                    self.set_visual(Some(&visual));
                }
            }
        } else {
            self.set_opacity(1.0);
            self.set_app_paintable(false);
        }
    }

    fn update_title(&self, title: &str) {
        gtk::Window::set_title(self.upcast_ref::<gtk::Window>(), title);
        if let Some(header) = self.imp().headerbar.borrow().clone() {
            header.set_title(Some(title));
        }
    }

    fn build_headerbar(&self, title: &str) {
        let header = gtk::HeaderBar::new();
        header.set_show_close_button(true);
        header.set_title(Some(&format!("{} - {}", APP_NAME, title)));
        *self.imp().headerbar.borrow_mut() = Some(header.clone());

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        *self.imp().toolbar.borrow_mut() = Some(toolbar.clone());

        let new_win_btn = crate::icons::icon_button(
            Some("window-new-symbolic"),
            None,
            Some("Open a new terminal window (Ctrl+Shift+N)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        new_win_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.on_new_window();
            }
        });
        toolbar.pack_start(&new_win_btn, false, false, 0);
        toolbar.pack_start(
            &gtk::Separator::new(gtk::Orientation::Vertical),
            false,
            false,
            4,
        );

        let copy_btn = crate::icons::icon_button(
            Some("edit-copy-symbolic"),
            None,
            Some("Copy selected text (Ctrl+Shift+C)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        copy_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(t) = w.imp().terminal.borrow().clone() {
                    t.copy();
                }
            }
        });
        toolbar.pack_start(&copy_btn, false, false, 0);

        let paste_btn = crate::icons::icon_button(
            Some("edit-paste-symbolic"),
            None,
            Some("Paste from clipboard (Ctrl+Shift+V)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        paste_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(t) = w.imp().terminal.borrow().clone() {
                    t.paste();
                }
            }
        });
        toolbar.pack_start(&paste_btn, false, false, 0);

        header.pack_start(&toolbar);

        let menubar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        menubar.style_context().add_class("oxterm-menu-row");
        let mut buttons = Vec::new();
        for label in ["File", "Edit", "View", "Terminal", "Notes", "Help"] {
            let btn = gtk::MenuButton::new();
            btn.set_label(label);
            btn.set_relief(gtk::ReliefStyle::None);
            menubar.pack_start(&btn, false, false, 0);
            buttons.push(btn);
        }
        *self.imp().menubar.borrow_mut() = Some(menubar);
        *self.imp().menu_buttons.borrow_mut() = buttons;

        self.set_titlebar(Some(&header));
    }

    fn menu_item(
        &self,
        menu: &gtk::Menu,
        label: &str,
        cb: Box<dyn Fn() + 'static>,
        accel: Option<&str>,
        tooltip: Option<&str>,
    ) {
        let item = gtk::MenuItem::with_label(label);
        item.connect_activate(move |_| cb());
        if let Some(accel) = accel {
            if let Some(accel_group) = self.imp().accel_group.borrow().clone() {
                let (raw_key, mods) = gtk::accelerator_parse(accel);
                if raw_key != 0 {
                    item.add_accelerator(
                        "activate",
                        &accel_group,
                        raw_key,
                        mods,
                        gtk::AccelFlags::VISIBLE,
                    );
                }
            }
        }
        if let Some(tip) = tooltip {
            item.set_tooltip_text(Some(tip));
        }
        menu.append(&item);
    }

    fn check_menu_item(
        &self,
        menu: &gtk::Menu,
        label: &str,
        cb: Box<dyn Fn(bool) + 'static>,
        active: bool,
        tooltip: Option<&str>,
    ) -> gtk::CheckMenuItem {
        let item = gtk::CheckMenuItem::with_label(label);
        item.set_active(active);
        item.connect_activate(move |i| cb(i.is_active()));
        if let Some(tip) = tooltip {
            item.set_tooltip_text(Some(tip));
        }
        menu.append(&item);
        item
    }

    fn build_menu(&self) -> Vec<(String, gtk::Menu)> {
        let mut menus = Vec::new();

        let file_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "New Window",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_new_window();
                }
            }),
            Some("<Primary><Shift>N"),
            Some("Open a new Oxterm terminal window"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Open File Manager Here",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.open_fm();
                }
            }),
            None,
            Some("Open the file manager in the current terminal working directory"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Close Window",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_close();
                }
            }),
            Some("<Primary><Shift>Q"),
            Some("Close this window"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Quit",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_close();
                }
            }),
            Some("<Primary>Q"),
            Some("Quit – close this window"),
        );
        menus.push(("File".into(), file_menu));

        let edit_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Copy",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.copy();
                    }
                }
            }),
            Some("<Primary><Shift>C"),
            Some("Copy selected text to clipboard"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Paste",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.paste();
                    }
                }
            }),
            Some("<Primary><Shift>V"),
            Some("Paste clipboard content into the terminal"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Paste Selection",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.paste_selection();
                    }
                }
            }),
            None,
            Some("Paste the primary selection (middle-click paste)"),
        );
        edit_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Select All",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.select_all();
                    }
                }
            }),
            Some("<Primary><Shift>A"),
            Some("Select all text in the terminal"),
        );
        edit_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Preferences...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.open_settings();
                }
            }),
            None,
            Some("Open the Oxterm settings dialog"),
        );
        menus.push(("Edit".into(), edit_menu));

        let view_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Menus",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_menubar", active);
                    if let Some(mb) = w.imp().menubar.borrow().clone() {
                        mb.set_visible(active);
                    }
                }
            }),
            settings().get_bool("show_menubar"),
            Some("Keep the File/Edit/View/... menu buttons always visible in the header bar"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Quick Actions",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_toolbar", active);
                    if let Some(tb) = w.imp().toolbar.borrow().clone() {
                        tb.set_visible(active);
                    }
                }
            }),
            settings().get_bool("show_toolbar"),
            Some("Always show the quick-action buttons in the header bar"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Show System Stats",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_stats", active);
                    w.apply_stats_visibility();
                }
            }),
            settings().get_bool("show_stats"),
            Some("Show CPU, RAM and Disk usage at the bottom of the window"),
        );
        view_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Full Screen",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.toggle_fullscreen();
                }
            }),
            Some("F11"),
            Some("Toggle full-screen mode"),
        );
        view_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom In",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.zoom_in();
                    }
                }
            }),
            Some("<Primary>plus"),
            Some("Increase terminal font size"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom Out",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.zoom_out();
                    }
                }
            }),
            Some("<Primary>minus"),
            Some("Decrease terminal font size"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom Reset",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.zoom_reset();
                    }
                }
            }),
            Some("<Primary>0"),
            Some("Reset terminal font size to default"),
        );
        menus.push(("View".into(), view_menu));

        let term_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Set Title...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.set_title_dialog();
                }
            }),
            Some("<Primary><Shift>S"),
            Some("Set a custom title for this window"),
        );
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let encoding_menu = gtk::Menu::new();
        let enc_item = gtk::MenuItem::with_label("Set Encoding");
        enc_item.set_submenu(Some(&encoding_menu));
        let mut encoding_actions = Vec::new();
        for label in ENCODINGS {
            let enc_name = label.split(' ').next().unwrap_or("").to_string();
            let weak = crate::SendWeak::new(self);
            let enc_name2 = enc_name.clone();
            let action = self.check_menu_item(
                &encoding_menu,
                label,
                Box::new(move |active| {
                    if active {
                        if let Some(w) = weak.upgrade() {
                            w.set_encoding(&enc_name2);
                        }
                    }
                }),
                settings().get_str("encoding") == enc_name,
                Some(&format!("Set terminal character encoding to {}", enc_name)),
            );
            encoding_actions.push((action, enc_name));
        }
        *self.imp().encoding_actions.borrow_mut() = encoding_actions;
        term_menu.append(&enc_item);

        term_menu.append(&gtk::SeparatorMenuItem::new());

        let signal_menu = gtk::Menu::new();
        let sig_item = gtk::MenuItem::with_label("Send Signal");
        sig_item.set_submenu(Some(&signal_menu));
        for (label, sig) in SIGNALS {
            let weak = crate::SendWeak::new(self);
            let sig = *sig;
            self.menu_item(
                &signal_menu,
                label,
                Box::new(move || {
                    if let Some(w) = weak.upgrade() {
                        if let Some(t) = w.imp().terminal.borrow().clone() {
                            t.kill(sig);
                        }
                    }
                }),
                None,
                Some(&format!("Send {} to the foreground process", label)),
            );
        }
        term_menu.append(&sig_item);

        term_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Reset",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.reset(false);
                    }
                }
            }),
            Some("<Primary><Shift>R"),
            Some("Soft reset the terminal emulator state"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Reset and Clear",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.reset(true);
                    }
                }
            }),
            Some("<Primary><Shift>X"),
            Some("Reset the terminal and clear the scrollback buffer"),
        );
        term_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &term_menu,
            "Read-Only",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.imp().terminal.borrow().clone() {
                        t.set_read_only(active);
                    }
                }
            }),
            false,
            Some("Toggle read-only mode (blocks keyboard input)"),
        );
        menus.push(("Terminal".into(), term_menu));

        let notes_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Notes Panel",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_toggle_panel();
                }
            }),
            None,
            Some("Show or hide the integrated notes panel"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "New Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_new();
                }
            }),
            None,
            Some("Compose a new note and save it to a Markdown file"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Add Selection to Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_selection();
                }
            }),
            None,
            Some("Append the selected terminal text to a note"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Copy All to Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_all();
                }
            }),
            None,
            Some("Append the whole terminal buffer to a note after confirmation"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Note Command Output...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_command_output();
                }
            }),
            None,
            Some("Append the last command output (needs shell integration) to a note"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Default Note",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_open_default();
                }
            }),
            None,
            Some("Open the default notes file in the editor"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_choose_open();
                }
            }),
            None,
            Some("Choose a note to open in the editor"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Notes Folder",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_open_folder();
                }
            }),
            None,
            Some("Open the notes directory in the file manager"),
        );
        menus.push(("Notes".into(), notes_menu));

        let help_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "Quick Help",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    crate::settings_dialog::show_help_dialog(Some(w.upcast_ref::<gtk::Window>()));
                }
            }),
            None,
            Some("Show Oxterm commands, shortcuts and remote session help"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "Diagnostics",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    let panes = usize::from(w.terminal_box().is_some());
                    crate::settings_dialog::show_diagnostics_dialog(
                        Some(w.upcast_ref::<gtk::Window>()),
                        panes,
                    );
                }
            }),
            None,
            Some("Show versions, paths, limits and session details for bug reports"),
        );
        help_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "About",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.show_about();
                }
            }),
            None,
            Some("Show information about Oxterm"),
        );
        menus.push(("Help".into(), help_menu));

        for (_, menu) in &menus {
            menu.show_all();
        }
        menus
    }

    fn populate_menus(&self) {
        let menus = self.build_menu();
        let buttons = self.imp().menu_buttons.borrow().clone();
        for (btn, (_label, menu)) in buttons.iter().zip(menus.iter()) {
            btn.set_popup(Some(menu));
        }
        self.imp().menu_buttons.borrow_mut().clear();
    }

    fn on_new_window(&self) {
        match self.application() {
            Some(app) => open_new_main_window(&app),
            None => spawn_new_process(true),
        }
    }

    fn open_fm(&self) {
        if let Some(fm) = detect_file_manager() {
            if let Some(t) = self.imp().terminal.borrow().clone() {
                let cwd = t.get_cwd();
                crate::notes::spawn_detached(&fm, &[&cwd]);
            }
        }
    }

    fn open_settings(&self) {
        let parent = self.clone();
        crate::settings_dialog::show_settings_dialog(Some(parent.upcast_ref()));
    }

    fn toggle_fullscreen(&self) {
        if let Some(w) = self.window() {
            if w.state() & gdk::WindowState::FULLSCREEN != gdk::WindowState::empty() {
                self.unfullscreen();
            } else {
                self.fullscreen();
            }
        }
    }

    fn set_title_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Set Title"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Set", gtk::ResponseType::Ok),
            ],
        );
        let entry = gtk::Entry::new();
        let current = self
            .title()
            .unwrap_or_default()
            .replace(&format!("{} - ", APP_NAME), "");
        entry.set_text(&current);
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let title = entry.text().trim().to_string();
            if !title.is_empty() {
                self.update_title(&format!("{} - {}", APP_NAME, title));
            }
        }
        dialog.close();
    }

    fn set_encoding(&self, encoding: &str) {
        if let Some(t) = self.imp().terminal.borrow().clone() {
            t.set_encoding(encoding);
        }
        let title = self.title().unwrap_or_default();
        let base = title.split(" [").next().unwrap_or("").to_string();
        self.update_title(&format!("{} [{}]", base, encoding));
        settings().set_str("encoding", encoding);
        for (action, enc_name) in self.imp().encoding_actions.borrow().iter() {
            action.set_active(*enc_name == encoding);
        }
    }

    fn show_about(&self) {
        crate::settings_dialog::show_about_dialog(Some(self.upcast_ref::<gtk::Window>()));
    }

    fn on_close(&self) -> glib::Propagation {
        if *self.imp().closing.borrow() {
            return glib::Propagation::Stop;
        }
        let has_active_process = self
            .imp()
            .terminal
            .borrow()
            .as_ref()
            .is_some_and(|term| term.has_active_process());
        if settings().get_bool("confirm_close") && has_active_process {
            let dialog = gtk::MessageDialog::new(
                Some(self),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Question,
                gtk::ButtonsType::YesNo,
                "Close all tabs and exit?",
            );
            let resp = dialog.run();
            dialog.close();
            if resp != gtk::ResponseType::Yes {
                return glib::Propagation::Stop;
            }
        }
        self.shutdown();
        glib::Propagation::Stop
    }

    /// Close without asking (used after confirmation and on SIGTERM/SIGHUP).
    pub fn shutdown(&self) {
        if self.imp().closing.replace(true) {
            return;
        }
        self.stop_stats_timer();
        for handler in std::mem::take(&mut *self.imp().settings_handlers.borrow_mut()) {
            settings().disconnect_changed(handler);
        }
        if let Some(t) = self.imp().terminal.borrow().clone() {
            t.terminate();
        }
        unsafe { self.destroy() };
    }

    fn on_window_key(&self, _widget: &gtk::Window, event: &gdk::EventKey) -> glib::Propagation {
        let ctrl = event.state().contains(gdk::ModifierType::CONTROL_MASK);
        let key = event.keyval();
        if (ctrl && key == K::F11) || key == K::F11 {
            self.toggle_fullscreen();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn apply_stats_visibility(&self) {
        let visible = settings().get_bool("show_stats");
        if let Some(box_) = self.imp().stats_box.borrow().clone() {
            box_.set_visible(visible);
        }
        if let Some(l) = self.imp().stats_sys_label.borrow().clone() {
            l.set_visible(visible);
        }
        if let Some(l) = self.imp().stats_self_label.borrow().clone() {
            l.set_visible(visible);
        }
        if visible {
            self.refresh_active_cwd_badge();
            self.start_stats_timer();
        } else {
            self.stop_stats_timer();
        }
    }

    fn start_stats_timer(&self) {
        if self.imp().stats_source_id.borrow().is_some() {
            return;
        }
        self.refresh_stats();
        let weak = crate::SendWeak::new(self);
        let source = glib::timeout_add_seconds_local(3, move || {
            if let Some(w) = weak.upgrade() {
                // Nobody can see the stats of a minimized window.
                let iconified = gtk::prelude::WidgetExt::window(&w)
                    .is_some_and(|gw| gw.state().contains(gdk::WindowState::ICONIFIED));
                if !iconified {
                    w.refresh_stats();
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        *self.imp().stats_source_id.borrow_mut() = Some(source);
    }

    fn stop_stats_timer(&self) {
        if let Some(id) = self.imp().stats_source_id.borrow_mut().take() {
            id.remove();
        }
        // Leave remote_stats_pending set while a probe is still running: it
        // is cleared when that probe reports back, so toggling the stats bar
        // cannot stack up concurrent ssh probes.
        *self.imp().remote_stats_generation.borrow_mut() += 1;
    }

    fn refresh_stats(&self) {
        let Some(term) = self.imp().terminal.borrow().clone() else {
            return;
        };
        // Focus can move between split panes without a tab switch; the badge
        // follows the active terminal on every stats tick as well.
        self.update_cwd_badge(&term);
        if term.is_ssh_client() {
            if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                label.set_text(&crate::system_stats::ssh_placeholder());
            }
            if !*self.imp().remote_stats_pending.borrow() {
                *self.imp().remote_stats_pending.borrow_mut() = true;
                *self.imp().remote_stats_generation.borrow_mut() += 1;
                let gen = *self.imp().remote_stats_generation.borrow();
                let weak = crate::SendWeak::new(self);
                let term_id = term.remote_id();
                let Some((ssh_args, cached)) = term.remote_stats_request() else {
                    *self.imp().remote_stats_pending.borrow_mut() = false;
                    return;
                };
                if let Some(stats) = cached {
                    *self.imp().remote_stats_pending.borrow_mut() = false;
                    if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                        label.set_text(&stats);
                    }
                    return;
                }
                std::thread::spawn(move || {
                    let stats = TerminalBox::collect_remote_stats(&ssh_args);
                    crate::run_on_main(move || {
                        if let Some(w) = weak.upgrade() {
                            *w.imp().remote_stats_pending.borrow_mut() = false;
                            if gen == *w.imp().remote_stats_generation.borrow() {
                                let is_active = w
                                    .imp()
                                    .terminal
                                    .borrow()
                                    .as_ref()
                                    .is_some_and(|term| term.remote_id() == term_id);
                                if is_active {
                                    if let Some(term) = w.imp().terminal.borrow().clone() {
                                        term.cache_remote_stats(&stats);
                                    }
                                    if let Some(label) = w.imp().stats_sys_label.borrow().clone() {
                                        if stats.is_empty() {
                                            label.set_text(&crate::system_stats::ssh_placeholder());
                                        } else {
                                            label.set_text(&stats);
                                        }
                                    }
                                }
                            }
                        }
                    });
                });
            }
            if let Some(label) = self.imp().stats_self_label.borrow().clone() {
                label.set_text(&crate::system_stats::collect_self());
            }
            return;
        }
        let stats = if term.is_ssh() {
            format!("  [SSH] {}", crate::system_stats::collect(false).trim())
        } else {
            crate::system_stats::collect(false)
        };
        if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
            label.set_text(&stats);
        }
        if let Some(label) = self.imp().stats_self_label.borrow().clone() {
            label.set_text(&crate::system_stats::collect_self());
        }
    }

    fn apply_ui_visibility(&self) {
        let s = settings();
        if let Some(menubar) = self.imp().menubar.borrow().clone() {
            menubar.set_visible(s.get_bool("show_menubar"));
        }
        if let Some(toolbar) = self.imp().toolbar.borrow().clone() {
            toolbar.set_visible(s.get_bool("show_toolbar"));
        }
        self.apply_stats_visibility();
    }
}

impl TerminalWindow for DetachedWindow {
    fn new_tab_signal(&self) {
        self.on_new_window();
    }
    fn new_window_signal(&self) {
        self.on_new_window();
    }
    fn close_tab_signal(&self, _term: Option<&TerminalBox>) {
        self.on_close();
    }
    fn close_window_signal(&self) {
        self.on_close();
    }
    fn set_title_dialog(&self) {
        self.set_title_dialog();
    }
    fn reset_terminal(&self) {
        if let Some(t) = self.imp().terminal.borrow().clone() {
            t.reset(false);
        }
    }
    fn reset_and_clear(&self) {
        if let Some(t) = self.imp().terminal.borrow().clone() {
            t.reset(true);
        }
    }
    fn split_signal(&self, _mode: &str) {}
    fn focus_other_pane_signal(&self) {}
    fn prev_tab_signal(&self) {}
    fn next_tab_signal(&self) {}
    fn session_list_signal(&self) {}
    fn set_tab_title_from_terminal(&self, _term: &TerminalBox, title: &str) {
        self.update_title(&format!("{} - {}", APP_NAME, title));
    }
    fn refresh_cwd_display(&self, _term: &TerminalBox) {
        self.refresh_active_cwd_badge();
    }
    fn broadcast_feed(&self, _source: &TerminalBox, _data: &[u8]) {}

    fn note_feedback(&self, message: &str, is_error: bool) {
        show_note_feedback(
            &self.imp().info_bar,
            &self.imp().info_label,
            message,
            is_error,
        );
    }
    fn status_message(&self, message: &str, is_error: bool) {
        self.note_feedback(message, is_error);
    }
    fn note_window(&self) -> gtk::Window {
        self.clone().upcast()
    }
    fn note_terminal(&self) -> Option<TerminalBox> {
        self.imp().terminal.borrow().clone()
    }
    fn note_write(&self, text: String, filename: Option<String>, context: NoteContext) {
        let weak = crate::SendWeak::new(self);
        NotesManager::new().write_note_async(text, filename.as_deref(), context, move |result| {
            if let Some(w) = weak.upgrade() {
                match result {
                    Ok(path) => w.note_feedback(&format!("Saved to {}", path.display()), false),
                    Err(e) => w.note_feedback(&format!("Could not write note: {}", e), true),
                }
            }
        });
    }
}

// ── MainWindow ───────────────────────────────────────────────

mod main_imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct MainWindow {
        pub settings_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub paned: RefCell<Option<gtk::Paned>>,
        pub notebook: RefCell<Option<gtk::Notebook>>,
        pub notebook2: RefCell<Option<gtk::Notebook>>,
        pub headerbar: RefCell<Option<gtk::HeaderBar>>,
        pub toolbar: RefCell<Option<gtk::Box>>,
        pub menubar: RefCell<Option<gtk::Box>>,
        pub menu_buttons: RefCell<Vec<gtk::MenuButton>>,
        pub accel_group: RefCell<Option<gtk::AccelGroup>>,
        pub stats_sys_label: RefCell<Option<gtk::Label>>,
        pub stats_self_label: RefCell<Option<gtk::Label>>,
        pub stats_box: RefCell<Option<gtk::Box>>,
        pub cwd_label: RefCell<Option<gtk::Label>>,
        pub info_bar: RefCell<Option<gtk::InfoBar>>,
        pub info_label: RefCell<Option<gtk::Label>>,
        pub notes_panel: RefCell<Option<crate::notes_panel::NotesPanel>>,
        /// Container that receives the notes panel the first time it is shown.
        pub notes_slot: RefCell<Option<gtk::Box>>,
        /// Menu popups are built once, after the first terminal is up.
        pub menus_built: Cell<bool>,
        pub stats_source_id: RefCell<Option<glib::SourceId>>,
        pub remote_stats_pending: RefCell<bool>,
        pub remote_stats_generation: RefCell<u64>,
        pub split_mode: RefCell<String>,
        /// True while one half of a split is zoomed to fill the window.
        pub split_zoomed: Cell<bool>,
        /// Position of the splitter as a fraction of the window before zooming.
        pub split_zoom_ratio: Cell<f64>,
        /// True while `move_tab_between` is detaching a page from its old
        /// notebook. The page is appended to the destination only afterwards,
        /// so `page-removed` must not mistake the temporary empty state for
        /// "every tab is gone" and close the window.
        pub moving_tab: RefCell<bool>,
        pub tab_base_titles: RefCell<std::collections::HashMap<TerminalBox, String>>,
        /// Tabs explicitly renamed by the user. Their title is kept as-is and
        /// is not replaced by OSC titles coming from the running program.
        pub tab_custom_titles: RefCell<std::collections::HashMap<TerminalBox, String>>,
        pub tab_labels: RefCell<std::collections::HashMap<TerminalBox, TabLabel>>,
        pub next_tab_number: RefCell<i64>,
        pub current_pages: RefCell<std::collections::HashMap<gtk::Notebook, u32>>,
        pub encoding_actions: RefCell<Vec<(gtk::CheckMenuItem, String)>>,
        pub split_single_act: RefCell<Option<gtk::CheckMenuItem>>,
        pub split_v_act: RefCell<Option<gtk::CheckMenuItem>>,
        pub split_h_act: RefCell<Option<gtk::CheckMenuItem>>,
        pub read_only_action: RefCell<Option<gtk::CheckMenuItem>>,
        pub profiles_menu: RefCell<Option<gtk::Menu>>,
        pub sessions_menu: RefCell<Option<gtk::Menu>>,
        pub tabs_menu: RefCell<Option<gtk::Menu>>,
        pub tab_list_button: RefCell<Option<gtk::MenuButton>>,
        pub splitting: RefCell<bool>,
        pub closing: RefCell<bool>,
        pub skip_close_confirm: RefCell<bool>,
        pub restoring_session: RefCell<bool>,
        /// Fixed window title requested via `--title`; when set, title updates
        /// coming from the running program (OSC) are ignored.
        pub forced_title: RefCell<Option<String>>,
        /// When true (`--hold`), the initial terminal is kept open after its
        /// command exits instead of closing the tab/window.
        pub hold: RefCell<bool>,
        /// Periodic "last" session autosave, so logout/SIGTERM/crash do not
        /// lose the open tabs.
        pub autosave_source: RefCell<Option<glib::SourceId>>,
        pub last_saved_session: RefCell<Option<crate::session::SessionData>>,
        /// (columns, rows, font size) last used to size the window; settings
        /// changes only resize when one of these actually changes.
        pub applied_size_key: Cell<Option<WindowSizeKey>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "TpgkMainWindow";
        type Type = super::MainWindow;
        type ParentType = gtk::ApplicationWindow;
    }

    impl ObjectImpl for MainWindow {}

    impl WidgetImpl for MainWindow {}

    impl ContainerImpl for MainWindow {}

    impl BinImpl for MainWindow {}

    impl WindowImpl for MainWindow {}

    impl ApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<main_imp::MainWindow>)
        @extends gtk::ApplicationWindow, gtk::Window, gtk::Bin, gtk::Container, gtk::Widget;
}

/// Extra window-level options coming from the command line, inspired by modern
/// terminals (kitty, alacritty, gnome-terminal): a fixed title, a hold-open
/// flag, fullscreen/maximize startup states and an initial cell geometry.
#[derive(Default)]
pub struct WindowOptions {
    pub title: Option<String>,
    pub hold: bool,
    pub fullscreen: bool,
    pub maximize: bool,
    pub geometry: Option<(i32, i32)>,
    /// WM_CLASS class part (`--class`), used by window managers for tiling
    /// rules, icons and `.desktop` association.
    pub wm_class: Option<String>,
    /// WM_CLASS instance name (`--name`).
    pub wm_name: Option<String>,
}

struct SplitGuard<'a>(&'a std::cell::RefCell<bool>);

impl Drop for SplitGuard<'_> {
    fn drop(&mut self) {
        *self.0.borrow_mut() = false;
    }
}

impl MainWindow {
    pub fn new(
        app: Option<&gtk::Application>,
        start_dir: Option<String>,
        command: Option<Vec<String>>,
        restore_session: bool,
        options: WindowOptions,
    ) -> MainWindow {
        let this: MainWindow = glib::Object::new();
        if let Some(app) = app {
            this.set_application(Some(app));
        }
        this.init(start_dir, command, restore_session, options);
        this.start_session_autosave();
        this
    }

    fn start_session_autosave(&self) {
        let weak = self.downgrade();
        let source = glib::timeout_add_seconds_local(30, move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // on_close owns removing this source (SourceId::remove panics on
            // an already destroyed source), and a cancelled close dialog must
            // leave autosave running, so never Break here.
            if *win.imp().closing.borrow() {
                return glib::ControlFlow::Continue;
            }
            // With several windows in one process each would overwrite the
            // others' "last" session; only autosave a lone main window.
            if win.main_window_count() == 1 {
                win.save_session("last");
            }
            glib::ControlFlow::Continue
        });
        *self.imp().autosave_source.borrow_mut() = Some(source);
    }

    fn main_window_count(&self) -> usize {
        self.application()
            .map(|a| a.windows().iter().filter(|w| w.is::<MainWindow>()).count())
            .unwrap_or(1)
    }

    fn init(
        &self,
        start_dir: Option<String>,
        command: Option<Vec<String>>,
        restore_session: bool,
        options: WindowOptions,
    ) {
        *self.imp().hold.borrow_mut() = options.hold;
        if let Some(t) = options.title.as_ref() {
            *self.imp().forced_title.borrow_mut() = Some(t.clone());
        }
        self.set_title(options.title.as_deref().unwrap_or(APP_TITLE));
        self.set_icon_name(Some("oxterm"));
        // Set WM_CLASS (`--class` / `--name`) so tiling window managers (i3,
        // sway, hyprland, …) can match the window for rules, icons and the
        // `.desktop` entry. GTK3 builds WM_CLASS from the GDK program class
        // (res_class) and the program name (res_name), so set both globally
        // before the window is realized.
        if let Some(class) = options.wm_class.as_ref() {
            gdk::set_program_class(class);
        }
        if let Some(name) = options.wm_name.as_ref() {
            glib::set_prgname(Some(name));
        }
        let s = settings();
        let (cols, rows) = options.geometry.unwrap_or_else(|| {
            (
                s.get_i64("terminal_columns") as i32,
                s.get_i64("terminal_rows") as i32,
            )
        });
        let font_size = s.get_i64("font_size");
        let (w, h) = window_size_for(window_size_key_with(cols as i64, rows as i64, font_size));
        self.set_default_size(w, h);
        // The initial size (possibly from --geometry) is already applied;
        // don't snap back to the configured size on the next settings change.
        self.imp().applied_size_key.set(Some(window_size_key()));

        if options.fullscreen {
            self.fullscreen();
        } else if options.maximize {
            self.maximize();
        }

        self.apply_window_visuals();

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_wide_handle(true);
        *self.imp().paned.borrow_mut() = Some(paned.clone());

        let notebook = gtk::Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_show_border(false);
        let weak = crate::SendWeak::new(self);
        notebook.connect_switch_page(move |_nb, page, num| {
            if let Some(w) = weak.upgrade() {
                w.on_switch_tab(page, num);
            }
        });
        let weak = crate::SendWeak::new(self);
        notebook.connect_page_reordered(move |_nb, _c, _i| {
            if let Some(w) = weak.upgrade() {
                w.update_tabs_menu();
            }
        });
        let weak = crate::SendWeak::new(self);
        notebook.connect_page_removed(move |_nb, _c, _i| {
            if let Some(w) = weak.upgrade() {
                w.on_page_removed();
            }
        });
        *self.imp().notebook.borrow_mut() = Some(notebook.clone());

        let notebook2 = gtk::Notebook::new();
        notebook2.set_scrollable(true);
        notebook2.set_show_border(false);
        notebook2.set_no_show_all(true);
        notebook2.hide();
        let weak = crate::SendWeak::new(self);
        notebook2.connect_switch_page(move |_nb, page, num| {
            if let Some(w) = weak.upgrade() {
                w.on_switch_tab(page, num);
            }
        });
        let weak = crate::SendWeak::new(self);
        notebook2.connect_page_removed(move |_nb, _c, _i| {
            if let Some(w) = weak.upgrade() {
                w.on_page_removed();
            }
        });
        *self.imp().notebook2.borrow_mut() = Some(notebook2.clone());

        paned.pack1(&notebook, true, true);
        paned.pack2(&notebook2, true, true);

        *self.imp().split_mode.borrow_mut() = "single".to_string();

        let accel_group = gtk::AccelGroup::new();
        self.add_accel_group(&accel_group);
        *self.imp().accel_group.borrow_mut() = Some(accel_group);

        // Start the shell before the rest of the window is built: creating the
        // headerbar, the toolbar icons and realizing the widget tree costs tens
        // of milliseconds, and the shell boots meanwhile. The terminal is only
        // attached to the notebook once the window exists, so the first frame
        // still shows it.
        let prepared = if restore_session {
            None
        } else {
            let term = TerminalBox::new(self);
            term.launch(start_dir.as_deref(), command.as_ref());
            Some(term)
        };

        self.build_headerbar();

        // Directory the shell reports through OSC 7 when it differs from the
        // process directory: an SSH tab, or a shell that moved itself. Hidden
        // when both agree, so a local tab keeps the plain stats bar.
        let cwd_label = gtk::Label::new(None);
        cwd_label.set_halign(gtk::Align::Start);
        cwd_label.set_ellipsize(pango::EllipsizeMode::Start);
        cwd_label.set_max_width_chars(48);
        cwd_label.set_single_line_mode(true);
        cwd_label.set_margin_end(12);
        cwd_label.style_context().add_class("oxterm-stats-label");
        cwd_label.set_tooltip_text(Some("Directory reported by the shell (OSC 7)"));
        cwd_label.set_no_show_all(true);
        cwd_label.hide();
        let stats_sys_label = gtk::Label::new(Some(""));
        stats_sys_label.set_halign(gtk::Align::Start);
        stats_sys_label
            .style_context()
            .add_class("oxterm-stats-label");
        let stats_self_label = gtk::Label::new(Some(""));
        stats_self_label.set_halign(gtk::Align::End);
        stats_self_label
            .style_context()
            .add_class("oxterm-stats-label");
        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        stats_box.pack_start(&cwd_label, false, false, 0);
        stats_box.pack_start(&stats_sys_label, true, true, 0);
        stats_box.pack_end(&stats_self_label, false, false, 0);
        stats_box.set_no_show_all(true);
        *self.imp().stats_sys_label.borrow_mut() = Some(stats_sys_label);
        *self.imp().stats_self_label.borrow_mut() = Some(stats_self_label);
        *self.imp().stats_box.borrow_mut() = Some(stats_box.clone());
        *self.imp().cwd_label.borrow_mut() = Some(cwd_label);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (info_bar, info_label) = build_info_bar();
        *self.imp().info_bar.borrow_mut() = Some(info_bar.clone());
        *self.imp().info_label.borrow_mut() = Some(info_label);
        vbox.pack_start(&info_bar, false, false, 0);
        let menubar = self.imp().menubar.borrow().clone().unwrap();
        vbox.pack_start(&menubar, false, false, 0);
        vbox.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        // The notes panel is lazy: its (fairly large) widget tree is created
        // the first time it is shown. The empty slot keeps its place in the
        // layout so the paned terminal area keeps the same size.
        let content_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content_row.pack_start(&paned, true, true, 0);
        let notes_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content_row.pack_start(&notes_slot, false, true, 0);
        *self.imp().notes_slot.borrow_mut() = Some(notes_slot);
        vbox.pack_start(&content_row, true, true, 0);
        vbox.pack_end(&stats_box, false, false, 0);
        self.add(&vbox);

        let weak = crate::SendWeak::new(self);
        self.connect_delete_event(move |w, _| {
            if let Some(w2) = weak.upgrade() {
                return w2.on_close(w);
            }
            glib::Propagation::Proceed
        });
        let weak = crate::SendWeak::new(self);
        self.connect_key_press_event(move |w, ev| {
            if let Some(w2) = weak.upgrade() {
                return w2.on_window_key(w.upcast_ref(), ev);
            }
            glib::Propagation::Proceed
        });

        self.show_all();

        self.apply_ui_visibility();
        self.apply_tab_colors();

        // Apply a `--title` after the headerbar exists so the fixed title shows
        // both in the window title (WM) and in the client-side header.
        if let Some(forced) = self.imp().forced_title.borrow().clone() {
            self.update_title(&forced);
        }

        // One handler per window: each settings change re-applies everything
        // this window derives from settings.
        let handler = settings().connect_changed({
            let w = self.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_tab_colors();
                    w.apply_window_size();
                    w.apply_window_visuals();
                    w.apply_ui_visibility();
                    w.apply_dynamic_titles();
                }
            }
        });
        self.imp().settings_handlers.borrow_mut().push(handler);
        self.connect_destroy(|w| {
            for handler in std::mem::take(&mut *w.imp().settings_handlers.borrow_mut()) {
                settings().disconnect_changed(handler);
            }
        });

        let weak = crate::SendWeak::new(self);
        let sd = start_dir.clone();
        let cmd = command.clone();
        // The idle source is `FnMut`, so the already started terminal is taken
        // out of the cell on the first (and only) run.
        let prepared = std::cell::RefCell::new(prepared);
        glib::idle_add_local(move || {
            if let Some(w) = weak.upgrade() {
                w.fix_paned_position();
                w.initialize_terminal(sd.as_deref(), &cmd, restore_session, prepared.take());
            }
            glib::ControlFlow::Break
        });

        // Menus are 70+ items with closures, tooltips and accelerators. Build
        // them only after the first terminal has been started (and the first
        // frame is on its way), not as the very first idle callback. Clicking
        // a menu button earlier calls `ensure_menus` synchronously, so the
        // popup is never empty.
        let weak = crate::SendWeak::new(self);
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            if let Some(w) = weak.upgrade() {
                w.ensure_menus();
            }
            glib::ControlFlow::Break
        });
    }

    fn initialize_terminal(
        &self,
        start_dir: Option<&str>,
        command: &Option<Vec<String>>,
        restore: bool,
        prepared: Option<TerminalBox>,
    ) {
        if restore && self.restore_session() {
            return;
        }
        let term = match prepared {
            Some(term) => self.attach_tab(term, None, None, None),
            None => self.add_new_tab(start_dir, None, None, None, command.as_ref()),
        };
        if *self.imp().hold.borrow() {
            term.set_hold(true);
        }
    }

    fn fix_paned_position(&self) {
        if let Some(paned) = self.imp().paned.borrow().clone() {
            paned.set_position(99999);
        }
    }

    /// Toggle a tmux-style split zoom: the focused half fills the window, and
    /// toggling again restores the previous splitter position.
    fn toggle_split_zoom(&self) {
        if *self.imp().split_mode.borrow() == "single" {
            return;
        }
        let Some(paned) = self.imp().paned.borrow().clone() else {
            return;
        };
        if self.imp().split_zoomed.get() {
            let size = paned_size(&paned);
            let ratio = self.imp().split_zoom_ratio.get().clamp(0.15, 0.85);
            paned.set_position((ratio * size as f64).round() as i32);
            self.imp().split_zoomed.set(false);
            return;
        }
        let size = paned_size(&paned);
        if size <= 2 {
            return;
        }
        let position = paned.position().clamp(1, size - 1);
        self.imp()
            .split_zoom_ratio
            .set(position as f64 / size as f64);
        let first = self.imp().notebook.borrow().clone().unwrap();
        let zoom_first = self.focused_notebook() == first;
        // A huge position is clamped by GtkPaned and stays clamped when the
        // window is resized, unlike an absolute pixel value.
        paned.set_position(if zoom_first { i32::MAX / 4 } else { 0 });
        self.imp().split_zoomed.set(true);
    }

    fn equalize_splits(&self) {
        if *self.imp().split_mode.borrow() == "single" {
            return;
        }
        let Some(paned) = self.imp().paned.borrow().clone() else {
            return;
        };
        self.imp().split_zoomed.set(false);
        let size = paned_size(&paned);
        if size > 0 {
            paned.set_position(size / 2);
        }
    }

    fn apply_window_size(&self) {
        let key = window_size_key();
        if self.imp().applied_size_key.replace(Some(key)) == Some(key) {
            return;
        }
        let (w, h) = window_size_for(key);
        self.resize(w, h);
    }

    fn apply_window_visuals(&self) {
        let s = settings();
        let opacity = (s.get_f64("opacity") * 100.0).round() / 100.0;
        if s.get_bool("enable_transparency") {
            self.set_opacity(opacity);
            if let Some(screen) = gtk::prelude::WidgetExt::screen(self) {
                if let Some(visual) = screen.rgba_visual() {
                    self.set_app_paintable(true);
                    self.set_visual(Some(&visual));
                }
            }
        } else {
            self.set_opacity(1.0);
            self.set_app_paintable(false);
        }
    }

    fn apply_ui_visibility(&self) {
        let s = settings();
        if let Some(nb) = self.imp().notebook.borrow().clone() {
            nb.set_show_tabs(s.get_bool("show_tabs"));
        }
        if let Some(nb) = self.imp().notebook2.borrow().clone() {
            nb.set_show_tabs(s.get_bool("show_tabs"));
        }
        if let Some(menubar) = self.imp().menubar.borrow().clone() {
            menubar.set_visible(s.get_bool("show_menubar"));
        }
        if let Some(toolbar) = self.imp().toolbar.borrow().clone() {
            toolbar.set_visible(s.get_bool("show_toolbar"));
        }
        self.apply_stats_visibility();
    }

    fn update_title(&self, title: &str) {
        gtk::Window::set_title(self.upcast_ref::<gtk::Window>(), title);
        if let Some(header) = self.imp().headerbar.borrow().clone() {
            header.set_title(Some(title));
        }
    }

    pub fn active_notebook(&self) -> gtk::Notebook {
        if *self.imp().split_mode.borrow() == "single" {
            return self.imp().notebook.borrow().clone().unwrap();
        }
        self.focused_notebook()
    }

    fn focused_notebook(&self) -> gtk::Notebook {
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for n in [&nb, &nb2] {
            for i in 0..n.n_pages() {
                if let Some(page) = n.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        if term.vte().is_focus() {
                            return n.clone();
                        }
                    }
                }
            }
        }
        nb
    }

    pub fn current_terminal(&self) -> Option<TerminalBox> {
        let nb = self.active_notebook();
        nb.current_page()
            .and_then(|idx| nb.nth_page(Some(idx)))
            .and_then(|w| w.downcast::<TerminalBox>().ok())
    }

    /// Every pane of this window, in both halves of a split.
    pub fn all_terminals(&self) -> Vec<TerminalBox> {
        let mut terminals = Vec::new();
        for notebook in [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ]
        .into_iter()
        .flatten()
        {
            for index in 0..notebook.n_pages() {
                if let Some(page) = notebook.nth_page(Some(index)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        terminals.push(term);
                    }
                }
            }
        }
        terminals
    }

    fn find_terminal(&self, term: &TerminalBox) -> Option<(gtk::Notebook, u32)> {
        for nb in [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(idx) = nb.page_num(term.upcast_ref::<gtk::Widget>()) {
                return Some((nb, idx));
            }
        }
        None
    }

    fn total_tabs(&self) -> u32 {
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        nb.n_pages() + nb2.n_pages()
    }

    fn any_terminal_has_active_process(&self) -> bool {
        let notebooks = [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ];
        notebooks.into_iter().flatten().any(|notebook| {
            (0..notebook.n_pages()).any(|index| {
                notebook
                    .nth_page(Some(index))
                    .and_then(|page| page.downcast::<TerminalBox>().ok())
                    .is_some_and(|term| term.has_active_process())
            })
        })
    }

    fn update_tab_list_button(&self) {
        if let Some(button) = self.imp().tab_list_button.borrow().clone() {
            button.set_sensitive(self.total_tabs() > 1);
        }
    }

    fn tab_tooltip(&self, title: &str, term: &TerminalBox) -> String {
        let cwd = term.display_cwd();
        if cwd.is_empty() {
            title.to_string()
        } else {
            format!("{title}\n{cwd}")
        }
    }

    fn update_cwd_badge(&self, term: &TerminalBox) {
        let Some(label) = self.imp().cwd_label.borrow().clone() else {
            return;
        };
        match term.cwd_badge() {
            Some(path) => {
                label.set_text(&path);
                label.show();
            }
            None => label.hide(),
        }
    }

    fn refresh_active_cwd_badge(&self) {
        if let Some(term) = self.current_terminal() {
            self.update_cwd_badge(&term);
        }
    }

    fn get_tab_text(&self, term: &TerminalBox) -> String {
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            return tab.label.text().to_string();
        }
        String::new()
    }

    fn set_tab_text(&self, term: &TerminalBox, text: &str) {
        // The tooltip shows the directory the shell reports, which in an SSH
        // tab is the remote one; the label itself stays the title.
        let tooltip = self.tab_tooltip(text, term);
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            let lbl = &tab.label;
            lbl.set_tooltip_text(Some(&tooltip));
            lbl.set_width_chars(Self::tab_label_width(text));
            let s = settings();
            let active_color = s.get_str("tab_active_title_color");
            let title_color = s.get_str("tab_title_color");
            let color = if self.is_active_tab(term) {
                active_color
            } else {
                title_color
            };
            if color.is_empty() {
                lbl.set_text(text);
            } else {
                lbl.set_markup(&format!(
                    "<span foreground='{}'>{}</span>",
                    color,
                    escape_markup(text)
                ));
            }
        }
    }

    fn is_active_tab(&self, term: &TerminalBox) -> bool {
        for nb in [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ]
        .into_iter()
        .flatten()
        {
            let cur = *self
                .imp()
                .current_pages
                .borrow()
                .get(&nb)
                .unwrap_or(&nb.current_page().unwrap_or(0));
            if let Some(idx) = nb.page_num(term.upcast_ref::<gtk::Widget>()) {
                if idx == cur {
                    return true;
                }
            }
        }
        false
    }

    fn apply_tab_colors(&self) {
        for nb in [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ]
        .into_iter()
        .flatten()
        {
            let cur = *self
                .imp()
                .current_pages
                .borrow()
                .get(&nb)
                .unwrap_or(&nb.current_page().unwrap_or(0));
            for i in 0..nb.n_pages() {
                if let Some(page) = nb.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        let is_active = i == cur;
                        self.recolor_tab_label(&term, is_active);
                    }
                }
            }
        }
    }

    fn apply_dynamic_titles(&self) {
        for nb in [
            self.imp().notebook.borrow().clone(),
            self.imp().notebook2.borrow().clone(),
        ]
        .into_iter()
        .flatten()
        {
            for i in 0..nb.n_pages() {
                if let Some(page) = nb.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        if let Some(title) = term.vte().window_title() {
                            self.set_tab_title_from_terminal(&term, title.as_str());
                        }
                    }
                }
            }
        }
    }

    fn recolor_tab_label(&self, term: &TerminalBox, is_active: bool) {
        let s = settings();
        let color = if is_active {
            s.get_str("tab_active_title_color")
        } else {
            s.get_str("tab_title_color")
        };
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            let lbl = &tab.label;
            let text = lbl.text().to_string();
            if color.is_empty() {
                lbl.set_text(&text);
            } else {
                lbl.set_markup(&format!(
                    "<span foreground='{}'>{}</span>",
                    color,
                    escape_markup(&text)
                ));
            }
        }
    }

    // ── Header bar ───────────────────────────────────────────

    fn build_headerbar(&self) {
        let header = gtk::HeaderBar::new();
        header.set_show_close_button(true);
        header.set_title(Some(APP_TITLE));
        *self.imp().headerbar.borrow_mut() = Some(header.clone());

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        *self.imp().toolbar.borrow_mut() = Some(toolbar.clone());

        let new_tab_btn = crate::icons::icon_button(
            Some("tab-new-symbolic"),
            None,
            Some("Open a new terminal tab (Ctrl+Shift+T)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        new_tab_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.on_new_tab();
            }
        });
        toolbar.pack_start(&new_tab_btn, false, false, 0);

        let new_win_btn = crate::icons::icon_button(
            Some("window-new-symbolic"),
            None,
            Some("Open a new terminal window (Ctrl+Shift+N)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        new_win_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.on_new_window();
            }
        });
        toolbar.pack_start(&new_win_btn, false, false, 0);

        toolbar.pack_start(
            &gtk::Separator::new(gtk::Orientation::Vertical),
            false,
            false,
            4,
        );

        let split_v_img = crate::icons::split_view_image(true, crate::icons::ICON_SIZE);
        let split_v_btn = crate::icons::icon_button(
            None,
            None,
            Some("Split vertically – left/right panels (Ctrl+Shift+E)"),
            crate::icons::ICON_SIZE,
            Some(&split_v_img),
        );
        let weak = crate::SendWeak::new(self);
        split_v_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let mode = w.imp().split_mode.borrow().clone();
                if mode == "vertical" {
                    w.set_split("single", true);
                } else {
                    w.set_split("vertical", true);
                }
            }
        });
        toolbar.pack_start(&split_v_btn, false, false, 0);

        let split_h_img = crate::icons::split_view_image(false, crate::icons::ICON_SIZE);
        let split_h_btn = crate::icons::icon_button(
            None,
            None,
            Some("Split horizontally – top/bottom panels (Ctrl+Shift+D)"),
            crate::icons::ICON_SIZE,
            Some(&split_h_img),
        );
        let weak = crate::SendWeak::new(self);
        split_h_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                let mode = w.imp().split_mode.borrow().clone();
                if mode == "horizontal" {
                    w.set_split("single", true);
                } else {
                    w.set_split("horizontal", true);
                }
            }
        });
        toolbar.pack_start(&split_h_btn, false, false, 0);

        toolbar.pack_start(
            &gtk::Separator::new(gtk::Orientation::Vertical),
            false,
            false,
            4,
        );

        let copy_btn = crate::icons::icon_button(
            Some("edit-copy-symbolic"),
            None,
            Some("Copy selected text (Ctrl+Shift+C)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        copy_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(t) = w.current_terminal() {
                    t.copy();
                }
            }
        });
        toolbar.pack_start(&copy_btn, false, false, 0);

        let paste_btn = crate::icons::icon_button(
            Some("edit-paste-symbolic"),
            None,
            Some("Paste from clipboard (Ctrl+Shift+V)"),
            crate::icons::ICON_SIZE,
            None,
        );
        let weak = crate::SendWeak::new(self);
        paste_btn.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                if let Some(t) = w.current_terminal() {
                    t.paste();
                }
            }
        });
        toolbar.pack_start(&paste_btn, false, false, 0);

        header.pack_start(&toolbar);

        let menubar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        menubar.style_context().add_class("oxterm-menu-row");
        let mut buttons = Vec::new();
        for label in ["File", "Edit", "View", "Terminal", "Tabs", "Notes", "Help"] {
            let btn = gtk::MenuButton::new();
            btn.set_label(label);
            btn.set_relief(gtk::ReliefStyle::None);
            // The menus are built lazily after startup; make sure they exist
            // before the button's own handler tries to show the popup.
            let weak = crate::SendWeak::new(self);
            btn.connect_button_press_event(move |_, _| {
                if let Some(w) = weak.upgrade() {
                    w.ensure_menus();
                }
                glib::Propagation::Proceed
            });
            let weak = crate::SendWeak::new(self);
            btn.connect_key_press_event(move |_, _| {
                if let Some(w) = weak.upgrade() {
                    w.ensure_menus();
                }
                glib::Propagation::Proceed
            });
            menubar.pack_start(&btn, false, false, 0);
            buttons.push(btn);
        }
        *self.imp().menubar.borrow_mut() = Some(menubar);
        *self.imp().menu_buttons.borrow_mut() = buttons;

        let tab_list_btn = self.make_tab_list_button();
        *self.imp().tab_list_button.borrow_mut() = Some(tab_list_btn.clone());
        header.pack_end(&tab_list_btn);

        self.set_titlebar(Some(&header));
    }

    fn make_tab_list_button(&self) -> gtk::MenuButton {
        let btn = gtk::MenuButton::new();
        btn.set_image(Some(&crate::icons::symbolic_image("pan-down-symbolic", 16)));
        btn.set_relief(gtk::ReliefStyle::None);
        btn.set_size_request(32, 32);
        btn.set_margin_start(8);
        btn.set_tooltip_text(Some("Show all open tabs"));
        btn.set_sensitive(self.total_tabs() > 1);
        let menu = gtk::Menu::new();
        menu.style_context().add_class("oxterm-tab-menu");
        menu.set_reserve_toggle_size(false);
        btn.set_popup(Some(&menu));
        let weak = crate::SendWeak::new(self);
        btn.connect_toggled(move |b| {
            if b.is_active() {
                if let Some(w) = weak.upgrade() {
                    if w.total_tabs() > 1 {
                        w.populate_tab_menu(&menu);
                    }
                }
            }
        });
        btn
    }

    // ── Menus ────────────────────────────────────────────────

    fn menu_item(
        &self,
        menu: &gtk::Menu,
        label: &str,
        cb: Box<dyn Fn() + 'static>,
        accel: Option<&str>,
        tooltip: Option<&str>,
    ) {
        let item = gtk::MenuItem::with_label(label);
        item.connect_activate(move |_| cb());
        if let Some(accel) = accel {
            if let Some(accel_group) = self.imp().accel_group.borrow().clone() {
                let (raw_key, mods) = gtk::accelerator_parse(accel);
                if raw_key != 0 {
                    item.add_accelerator(
                        "activate",
                        &accel_group,
                        raw_key,
                        mods,
                        gtk::AccelFlags::VISIBLE,
                    );
                }
            }
        }
        if let Some(tip) = tooltip {
            item.set_tooltip_text(Some(tip));
        }
        menu.append(&item);
    }

    fn check_menu_item(
        &self,
        menu: &gtk::Menu,
        label: &str,
        cb: Box<dyn Fn(bool) + 'static>,
        active: bool,
        tooltip: Option<&str>,
    ) -> gtk::CheckMenuItem {
        let item = gtk::CheckMenuItem::with_label(label);
        item.set_active(active);
        item.connect_activate(move |i| cb(i.is_active()));
        if let Some(tip) = tooltip {
            item.set_tooltip_text(Some(tip));
        }
        menu.append(&item);
        item
    }

    fn build_menu(&self) -> Vec<(String, gtk::Menu)> {
        let mut menus = Vec::new();

        // File
        let file_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "New Tab",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_new_tab();
                }
            }),
            Some("<Primary><Shift>T"),
            Some("Open a new terminal tab in the current window"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "New Window",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_new_window();
                }
            }),
            Some("<Primary><Shift>N"),
            Some("Open a new Oxterm terminal window"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Open File Manager Here",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_open_fm();
                }
            }),
            None,
            Some("Open the file manager in the current terminal working directory"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Close Tab",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.close_tab(None);
                }
            }),
            Some("<Primary><Shift>W"),
            Some("Close the current terminal tab"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Close Window",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_close(w.upcast_ref());
                }
            }),
            Some("<Primary><Shift>Q"),
            Some("Close the current window"),
        );
        file_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &file_menu,
            "Quit",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.on_close(w.upcast_ref());
                }
            }),
            Some("<Primary>Q"),
            Some("Quit Oxterm (close all windows)"),
        );
        menus.push(("File".into(), file_menu));

        // Edit
        let edit_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Copy",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.copy();
                    }
                }
            }),
            Some("<Primary><Shift>C"),
            Some("Copy selected text to clipboard"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Paste",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.paste();
                    }
                }
            }),
            Some("<Primary><Shift>V"),
            Some("Paste clipboard content into the terminal"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Paste Selection",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.paste_selection();
                    }
                }
            }),
            None,
            Some("Paste the primary selection (middle-click paste)"),
        );
        edit_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Select All",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.select_all();
                    }
                }
            }),
            Some("<Primary><Shift>A"),
            Some("Select all text in the terminal"),
        );
        edit_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &edit_menu,
            "Preferences...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.open_settings();
                }
            }),
            None,
            Some("Open the Oxterm settings dialog"),
        );
        menus.push(("Edit".into(), edit_menu));

        // View
        let view_menu = gtk::Menu::new();
        let split_menu = gtk::Menu::new();
        let split_item = gtk::MenuItem::with_label("Split");
        split_item.set_submenu(Some(&split_menu));
        let weak = crate::SendWeak::new(self);
        let split_single = self.check_menu_item(
            &split_menu,
            "Single Panel",
            Box::new(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_split("single", true);
                }
            }),
            true,
            Some("Single terminal panel (no split)"),
        );
        let weak = crate::SendWeak::new(self);
        let split_v = self.check_menu_item(
            &split_menu,
            "Split Vertical",
            Box::new(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_split("vertical", true);
                }
            }),
            false,
            Some("Split the window vertically (left/right panels)"),
        );
        let weak = crate::SendWeak::new(self);
        let split_h = self.check_menu_item(
            &split_menu,
            "Split Horizontal",
            Box::new(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_split("horizontal", true);
                }
            }),
            false,
            Some("Split the window horizontally (top/bottom panels)"),
        );
        *self.imp().split_single_act.borrow_mut() = Some(split_single);
        *self.imp().split_v_act.borrow_mut() = Some(split_v);
        *self.imp().split_h_act.borrow_mut() = Some(split_h);
        view_menu.append(&split_item);

        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom Split Pane",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.toggle_split_zoom_signal();
                }
            }),
            None,
            Some("Let the focused pane fill the window; repeat to restore the split"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Equalize Splits",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.equalize_splits_signal();
                }
            }),
            None,
            Some("Reset the splitter to the middle"),
        );
        view_menu.append(&gtk::SeparatorMenuItem::new());

        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Tabs",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_tabs", active);
                    if let Some(nb) = w.imp().notebook.borrow().clone() {
                        nb.set_show_tabs(active);
                    }
                    if let Some(nb) = w.imp().notebook2.borrow().clone() {
                        nb.set_show_tabs(active);
                    }
                }
            }),
            settings().get_bool("show_tabs"),
            Some("Show the tab bar even when only one tab is open"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Menus",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_menubar", active);
                    if let Some(mb) = w.imp().menubar.borrow().clone() {
                        mb.set_visible(active);
                    }
                }
            }),
            settings().get_bool("show_menubar"),
            Some("Keep the File/Edit/View/... menu buttons always visible in the header bar"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Scrollbar",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    let pos = if active { "right" } else { "disabled" };
                    settings().set_str("scrollbar_position", pos);
                    for nb in [
                        w.imp().notebook.borrow().clone(),
                        w.imp().notebook2.borrow().clone(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        for i in 0..nb.n_pages() {
                            if let Some(page) = nb.nth_page(Some(i)) {
                                if let Ok(term) = page.downcast::<TerminalBox>() {
                                    term.set_scrollbar_visible(active);
                                }
                            }
                        }
                    }
                }
            }),
            settings().get_str("scrollbar_position") != "disabled",
            Some("Always show the vertical scrollbar"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Always Show Quick Actions",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_toolbar", active);
                    if let Some(tb) = w.imp().toolbar.borrow().clone() {
                        tb.set_visible(active);
                    }
                }
            }),
            settings().get_bool("show_toolbar"),
            Some("Always show the quick-action buttons in the header bar"),
        );
        let weak = crate::SendWeak::new(self);
        self.check_menu_item(
            &view_menu,
            "Show System Stats",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    settings().set_bool("show_stats", active);
                    w.apply_stats_visibility();
                }
            }),
            settings().get_bool("show_stats"),
            Some("Show CPU, RAM and Disk usage at the bottom of the window"),
        );
        view_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Full Screen",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.toggle_fullscreen();
                }
            }),
            Some("F11"),
            Some("Toggle full-screen mode"),
        );
        view_menu.append(&gtk::SeparatorMenuItem::new());

        let profiles_menu = gtk::Menu::new();
        let prof_item = gtk::MenuItem::with_label("Profiles");
        prof_item.set_submenu(Some(&profiles_menu));
        let weak = crate::SendWeak::new(self);
        profiles_menu.connect_show(move |menu| {
            if let Some(w) = weak.upgrade() {
                w.populate_profiles_menu(menu);
            }
        });
        *self.imp().profiles_menu.borrow_mut() = Some(profiles_menu.clone());
        view_menu.append(&prof_item);

        let sessions_menu = gtk::Menu::new();
        let sess_item = gtk::MenuItem::with_label("Sessions");
        sess_item.set_submenu(Some(&sessions_menu));
        let weak = crate::SendWeak::new(self);
        sessions_menu.connect_show(move |menu| {
            if let Some(w) = weak.upgrade() {
                w.populate_sessions_menu(menu);
            }
        });
        *self.imp().sessions_menu.borrow_mut() = Some(sessions_menu.clone());
        view_menu.append(&sess_item);

        view_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom In",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.zoom_all(|t| t.zoom_in());
                }
            }),
            Some("<Primary>plus"),
            Some("Increase terminal font size"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom Out",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.zoom_all(|t| t.zoom_out());
                }
            }),
            Some("<Primary>minus"),
            Some("Decrease terminal font size"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &view_menu,
            "Zoom Reset",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.zoom_all(|t| t.zoom_reset());
                }
            }),
            Some("<Primary>0"),
            Some("Reset terminal font size to default"),
        );
        menus.push(("View".into(), view_menu));

        // Terminal
        let term_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Rename Tab...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.set_title_dialog();
                }
            }),
            Some("<Primary><Shift>S"),
            Some("Rename the current tab (or double-click a tab)"),
        );
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let encoding_menu = gtk::Menu::new();
        let enc_item = gtk::MenuItem::with_label("Set Encoding");
        enc_item.set_submenu(Some(&encoding_menu));
        let mut encoding_actions = Vec::new();
        for label in ENCODINGS {
            let enc_name = label.split(' ').next().unwrap_or("").to_string();
            let weak = crate::SendWeak::new(self);
            let enc_name2 = enc_name.clone();
            let action = self.check_menu_item(
                &encoding_menu,
                label,
                Box::new(move |active| {
                    if active {
                        if let Some(w) = weak.upgrade() {
                            w.set_encoding(&enc_name2);
                        }
                    }
                }),
                settings().get_str("encoding") == enc_name,
                Some(&format!("Set terminal character encoding to {}", enc_name)),
            );
            encoding_actions.push((action, enc_name));
        }
        *self.imp().encoding_actions.borrow_mut() = encoding_actions;
        term_menu.append(&enc_item);
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let signal_menu = gtk::Menu::new();
        let sig_item = gtk::MenuItem::with_label("Send Signal");
        sig_item.set_submenu(Some(&signal_menu));
        for (label, sig) in SIGNALS {
            let weak = crate::SendWeak::new(self);
            let sig = *sig;
            self.menu_item(
                &signal_menu,
                label,
                Box::new(move || {
                    if let Some(w) = weak.upgrade() {
                        if let Some(t) = w.current_terminal() {
                            t.kill(sig);
                        }
                    }
                }),
                None,
                Some(&format!("Send {} to the foreground process", label)),
            );
        }
        term_menu.append(&sig_item);
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Reset",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.reset(false);
                    }
                }
            }),
            Some("<Primary><Shift>R"),
            Some("Soft reset the terminal emulator state"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Reset and Clear",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.reset(true);
                    }
                }
            }),
            Some("<Primary><Shift>X"),
            Some("Reset the terminal and clear the scrollback buffer"),
        );
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let weak = crate::SendWeak::new(self);
        let ro = self.check_menu_item(
            &term_menu,
            "Read-Only",
            Box::new(move |active| {
                if let Some(w) = weak.upgrade() {
                    if let Some(t) = w.current_terminal() {
                        t.set_read_only(active);
                    }
                }
            }),
            false,
            Some("Toggle read-only mode (blocks keyboard input)"),
        );
        *self.imp().read_only_action.borrow_mut() = Some(ro);
        term_menu.append(&gtk::SeparatorMenuItem::new());

        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Previous Pane",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.focus_other_pane();
                }
            }),
            Some("<Primary><Alt>Page_Up"),
            Some("Switch focus to the other split pane"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Previous Tab",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.prev_tab();
                }
            }),
            Some("<Primary>Page_Up"),
            Some("Switch to the previous tab"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Next Tab",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.next_tab();
                }
            }),
            Some("<Primary>Page_Down"),
            Some("Switch to the next tab"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Move Tab Left",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.move_tab(-1);
                }
            }),
            Some("<Primary><Shift>Page_Up"),
            Some("Move the current tab one position to the left"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Move Tab Right",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.move_tab(1);
                }
            }),
            Some("<Primary><Shift>Page_Down"),
            Some("Move the current tab one position to the right"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &term_menu,
            "Detach Tab",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.detach_tab(None);
                }
            }),
            None,
            Some("Detach the current tab into a separate window with full menu"),
        );
        menus.push(("Terminal".into(), term_menu));

        // Tabs
        let tabs_menu = gtk::Menu::new();
        *self.imp().tabs_menu.borrow_mut() = Some(tabs_menu.clone());
        menus.push(("Tabs".into(), tabs_menu));

        let notes_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Notes Panel",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_toggle_panel();
                }
            }),
            None,
            Some("Show or hide the integrated notes panel"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "New Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_new();
                }
            }),
            None,
            Some("Compose a new note and save it to a Markdown file"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Add Selection to Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_selection();
                }
            }),
            None,
            Some("Append the selected terminal text to a note"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Copy All to Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_all();
                }
            }),
            None,
            Some("Append the whole terminal buffer to a note after confirmation"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Note Command Output...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_add_command_output();
                }
            }),
            None,
            Some("Append the last command output (needs shell integration) to a note"),
        );
        notes_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Default Note",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_open_default();
                }
            }),
            None,
            Some("Open the default notes file in the editor"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Note...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_choose_open();
                }
            }),
            None,
            Some("Choose a note to open in the editor"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &notes_menu,
            "Open Notes Folder",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.note_open_folder();
                }
            }),
            None,
            Some("Open the notes directory in the file manager"),
        );
        menus.push(("Notes".into(), notes_menu));

        // Help
        let help_menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "Quick Help",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    crate::settings_dialog::show_help_dialog(Some(w.upcast_ref::<gtk::Window>()));
                }
            }),
            None,
            Some("Show Oxterm commands, shortcuts and remote session help"),
        );
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "Diagnostics",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    let panes = w.all_terminals().len();
                    crate::settings_dialog::show_diagnostics_dialog(
                        Some(w.upcast_ref::<gtk::Window>()),
                        panes,
                    );
                }
            }),
            None,
            Some("Show versions, paths, limits and session details for bug reports"),
        );
        help_menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        self.menu_item(
            &help_menu,
            "About",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.show_about();
                }
            }),
            None,
            Some("Show information about Oxterm"),
        );
        menus.push(("Help".into(), help_menu));

        for (_, menu) in &menus {
            menu.show_all();
        }
        menus
    }

    /// Build the header-bar menus once. Callers may reach this either from the
    /// delayed startup timeout or directly when a menu button is pressed
    /// before that timeout fired.
    fn ensure_menus(&self) {
        if self.imp().menus_built.get() {
            return;
        }
        self.imp().menus_built.set(true);
        let menus = self.build_menu();
        let buttons = self.imp().menu_buttons.borrow().clone();
        for btn in &buttons {
            let label = btn.label().unwrap_or_default().to_string();
            if let Some((_, menu)) = menus.iter().find(|(name, _)| *name == label) {
                btn.set_popup(Some(menu));
            }
        }
        // The Tabs menu is generated from the live notebook; fill it now that
        // it exists (tab changes before this point had no menu to update).
        if let Some(menu) = self.imp().tabs_menu.borrow().clone() {
            self.populate_tab_menu(&menu);
        }
    }

    // ── Split modes ──────────────────────────────────────────

    fn set_split(&self, mode: &str, create_tab: bool) {
        let _ = self.set_split_with(mode, create_tab, None, None, None);
    }

    /// Split this window and return the terminal for the new pane. `mode` must
    /// be `vertical` or `horizontal`; callers that need a single pane use
    /// `set_split`. Used by the menu, the control API and session restore.
    fn set_split_with(
        &self,
        mode: &str,
        create_tab: bool,
        cwd: Option<&str>,
        command: Option<&Vec<String>>,
        title: Option<&str>,
    ) -> Option<TerminalBox> {
        if *self.imp().splitting.borrow() {
            return None;
        }
        let _split_guard = SplitGuard(&self.imp().splitting);
        *self.imp().splitting.borrow_mut() = true;
        self.imp().split_zoomed.set(false);
        *self.imp().split_mode.borrow_mut() = mode.to_string();
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let paned = self.imp().paned.borrow().clone().unwrap();
        let mut created = None;
        if mode == "single" {
            while nb2.n_pages() > 0 {
                self.move_tab_between(&nb2, &nb, 0u32);
            }
            nb2.hide();
            paned.set_position(99999);
            if let Some(a) = self.imp().split_single_act.borrow().clone() {
                a.set_active(true);
            }
            if let Some(a) = self.imp().split_v_act.borrow().clone() {
                a.set_active(false);
            }
            if let Some(a) = self.imp().split_h_act.borrow().clone() {
                a.set_active(false);
            }
        } else if mode == "vertical" {
            paned.show();
            paned.set_orientation(gtk::Orientation::Horizontal);
            if nb2.n_pages() == 0 && create_tab {
                nb2.show();
                created = Some(self.add_new_tab(cwd, Some(&nb2), title, title, command));
            } else {
                nb2.show();
            }
            if let Some(a) = self.imp().split_single_act.borrow().clone() {
                a.set_active(false);
            }
            if let Some(a) = self.imp().split_v_act.borrow().clone() {
                a.set_active(true);
            }
            if let Some(a) = self.imp().split_h_act.borrow().clone() {
                a.set_active(false);
            }
            let (w, _h) = self.size();
            paned.set_position((w / 2).max(200));
        } else if mode == "horizontal" {
            paned.show();
            paned.set_orientation(gtk::Orientation::Vertical);
            if nb2.n_pages() == 0 && create_tab {
                nb2.show();
                created = Some(self.add_new_tab(cwd, Some(&nb2), title, title, command));
            } else {
                nb2.show();
            }
            if let Some(a) = self.imp().split_single_act.borrow().clone() {
                a.set_active(false);
            }
            if let Some(a) = self.imp().split_v_act.borrow().clone() {
                a.set_active(false);
            }
            if let Some(a) = self.imp().split_h_act.borrow().clone() {
                a.set_active(true);
            }
            let (_w, h) = self.size();
            paned.set_position((h / 2).max(100));
        }
        let result = created.or_else(|| {
            if mode == "single" {
                None
            } else {
                nb2.current_page()
                    .and_then(|index| nb2.nth_page(Some(index)))
                    .and_then(|page| page.downcast::<TerminalBox>().ok())
            }
        });
        self.update_tabs_menu();
        result
    }

    /// Public entry point for the control API: split only when the window is
    /// still showing a single pane.
    pub fn split_terminal(
        &self,
        mode: &str,
        cwd: Option<&str>,
        command: Option<&Vec<String>>,
        title: Option<&str>,
    ) -> Option<TerminalBox> {
        if *self.imp().split_mode.borrow() != "single" {
            return None;
        }
        self.set_split_with(mode, true, cwd, command, title)
    }

    fn move_tab_between(&self, src: &gtk::Notebook, dst: &gtk::Notebook, idx: u32) {
        if idx >= src.n_pages() {
            return;
        }
        let Some(page) = src.nth_page(Some(idx)) else {
            return;
        };
        let Ok(term) = page.downcast::<TerminalBox>() else {
            return;
        };
        let title = self.get_tab_text(&term);
        // Keep `page-removed` from closing the window in the short window
        // between removing the page here and appending it to `dst` below.
        *self.imp().moving_tab.borrow_mut() = true;
        let _moving = SplitGuard(&self.imp().moving_tab);
        src.remove_page(Some(idx));
        let (lbl_box, lbl, indicator) = self.make_tab_label(&title, &term);
        self.imp().tab_labels.borrow_mut().insert(
            term.clone(),
            TabLabel {
                event_box: lbl_box.clone(),
                label: lbl,
                indicator,
            },
        );
        dst.append_page(&term, Some(&lbl_box));
        dst.set_tab_reorderable(&term, true);
        dst.show_all();
        dst.set_current_page(Some(dst.n_pages() - 1));
        let nb2_empty = {
            let nb2 = self.imp().notebook2.borrow().clone().unwrap();
            src == &nb2 && nb2.n_pages() == 0
        };
        if nb2_empty {
            self.set_split("single", false);
        }
    }

    fn focus_other_pane(&self) {
        if *self.imp().split_mode.borrow() == "single" {
            return;
        }
        if self.imp().split_zoomed.get() {
            self.toggle_split_zoom();
        }
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let focused = self.focused_notebook();
        let target = if focused == nb { nb2 } else { nb };
        if let Some(idx) = target.current_page() {
            if let Some(page) = target.nth_page(Some(idx)) {
                if let Ok(term) = page.downcast::<TerminalBox>() {
                    term.vte().grab_focus();
                }
            }
        }
    }

    // ── Tab management ───────────────────────────────────────

    pub fn add_new_tab(
        &self,
        cwd: Option<&str>,
        target_notebook: Option<&gtk::Notebook>,
        base_title: Option<&str>,
        display_title: Option<&str>,
        command: Option<&Vec<String>>,
    ) -> TerminalBox {
        let term = TerminalBox::new(self);
        // Start the shell before the tab is wired into the notebook. The
        // bookkeeping below (page, label, tab menu, realize) costs tens of
        // milliseconds, and the shell can boot meanwhile.
        term.launch(cwd, command);
        self.attach_tab(term, target_notebook, base_title, display_title)
    }

    /// Wire an already started terminal into the notebook: title, tab label,
    /// page, tab menu and the first draw.
    ///
    /// Split from spawning so a shell can be started before the window is
    /// built; `add_new_tab` is the two steps in a row.
    fn attach_tab(
        &self,
        term: TerminalBox,
        target_notebook: Option<&gtk::Notebook>,
        base_title: Option<&str>,
        display_title: Option<&str>,
    ) -> TerminalBox {
        let base = if let Some(bt) = base_title {
            bt.to_string()
        } else {
            let base_name = settings().get_str("tab_title");
            if base_name.is_empty() {
                format!("Terminal {}", *self.imp().next_tab_number.borrow())
            } else {
                format!("{} {}", base_name, *self.imp().next_tab_number.borrow())
            }
        };
        *self.imp().next_tab_number.borrow_mut() += 1;
        self.imp()
            .tab_base_titles
            .borrow_mut()
            .insert(term.clone(), base.clone());

        let nb = match target_notebook {
            Some(nb) => nb.clone(),
            None => self.active_notebook(),
        };
        let (lbl_box, lbl, indicator) = self.make_tab_label(display_title.unwrap_or(&base), &term);
        self.imp().tab_labels.borrow_mut().insert(
            term.clone(),
            TabLabel {
                event_box: lbl_box.clone(),
                label: lbl,
                indicator,
            },
        );
        let idx = nb.append_page(&term, Some(&lbl_box));
        nb.set_tab_reorderable(&term, true);
        nb.set_show_tabs(settings().get_bool("show_tabs"));
        nb.show_all();
        nb.set_current_page(Some(idx));
        self.update_tabs_menu();

        term.set_remote_info(&base, &term.get_cwd());
        term.show_all();
        let weak = crate::SendWeak::new(&term);
        glib::idle_add_local(move || {
            if let Some(t) = weak.upgrade() {
                t.vte().grab_focus();
            }
            glib::ControlFlow::Break
        });
        term
    }

    /// Minimum character width for a tab label. `GtkNotebook` sizes a tab from
    /// the label's *minimum* width, so an ellipsized label with no explicit
    /// width collapses to a bare ellipsis. The cap matches `max_width_chars`
    /// above so long titles still ellipsize instead of widening the tab.
    fn tab_label_width(text: &str) -> i32 {
        text.chars().count().clamp(4, 28) as i32
    }

    fn make_tab_label(
        &self,
        name: &str,
        term: &TerminalBox,
    ) -> (gtk::EventBox, gtk::Label, gtk::Label) {
        let eb = gtk::EventBox::new();
        let term2 = term.clone();
        let weak = crate::SendWeak::new(self);
        eb.connect_button_press_event(move |_w, ev| {
            if let Some(win) = weak.upgrade() {
                if ev.button() == 3 {
                    win.on_tab_button_press(&term2);
                    return glib::Propagation::Stop;
                }
                if ev.button() == 1 && ev.event_type() == gdk::EventType::DoubleButtonPress {
                    win.rename_tab_dialog(&term2);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let lbl = gtk::Label::new(Some(name));
        // Long terminal titles (user@host:/deep/path) must not become the
        // notebook's minimum width: a split pane would then refuse to shrink
        // and the older terminal would be clipped instead of reflowing.
        lbl.set_ellipsize(pango::EllipsizeMode::End);
        lbl.set_max_width_chars(28);
        lbl.set_width_chars(Self::tab_label_width(name));
        lbl.set_single_line_mode(true);
        lbl.set_tooltip_text(Some(name));
        let btn = gtk::Button::new();
        btn.set_relief(gtk::ReliefStyle::None);
        btn.set_focus_on_click(false);
        btn.set_tooltip_text(Some("Close tab"));
        btn.add(&gtk::Image::from_icon_name(
            Some("window-close-symbolic"),
            gtk::IconSize::Menu,
        ));
        let term3 = term.clone();
        let weak = crate::SendWeak::new(self);
        btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.close_tab(Some(&term3));
            }
        });
        let indicator = gtk::Label::new(Some("●"));
        indicator.style_context().add_class("oxterm-tab-failure");
        indicator.set_tooltip_text(Some("The last command in this tab failed"));
        indicator.set_no_show_all(true);
        indicator.hide();
        box_.pack_start(&indicator, false, false, 0);
        box_.pack_start(&lbl, true, true, 0);
        box_.pack_start(&btn, false, false, 0);
        eb.add(&box_);
        eb.show_all();
        (eb, lbl, indicator)
    }

    /// Show or hide the failed-command dot. The tab the user is currently
    /// looking at stays clean: they can already see the error.
    fn set_tab_failure(&self, term: &TerminalBox, failed: bool) {
        let watching = self.is_active()
            && self.current_terminal().as_ref() == Some(term)
            && term.vte().is_focus();
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            tab.indicator.set_visible(failed && !watching);
        }
    }

    fn clear_tab_failure(&self, term: &TerminalBox) {
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            tab.indicator.hide();
        }
    }

    fn on_tab_button_press(&self, term: &TerminalBox) {
        let menu = gtk::Menu::new();
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_rename = gtk::MenuItem::with_label("Rename Tab...");
        item_rename.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.rename_tab_dialog(&term_c);
            }
        });
        menu.append(&item_rename);
        if self.imp().tab_custom_titles.borrow().contains_key(term) {
            let weak = crate::SendWeak::new(self);
            let term_c = term.clone();
            let item_reset = gtk::MenuItem::with_label("Reset Tab Title");
            item_reset.connect_activate(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.reset_tab_title(&term_c);
                }
            });
            menu.append(&item_reset);
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_close = gtk::MenuItem::with_label("Close Tab");
        item_close.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.close_tab(Some(&term_c));
            }
        });
        menu.append(&item_close);
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_detach = gtk::MenuItem::with_label("Detach Tab");
        item_detach.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.detach_tab(Some(&term_c));
            }
        });
        menu.append(&item_detach);
        menu.append(&gtk::SeparatorMenuItem::new());
        let term_c = term.clone();
        let item_copy = gtk::MenuItem::with_label("Copy All Text");
        item_copy.connect_activate(move |_| {
            term_c.select_all();
        });
        menu.append(&item_copy);
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_copy_notes = gtk::MenuItem::with_label("Copy All to Note...");
        item_copy_notes.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.copy_all_to_notes(&term_c);
            }
        });
        menu.append(&item_copy_notes);
        menu.append(&gtk::SeparatorMenuItem::new());
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_v = gtk::MenuItem::with_label("Move to Vertical Split");
        item_v.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.move_tab_to_split(&term_c, "vertical");
            }
        });
        menu.append(&item_v);
        let weak = crate::SendWeak::new(self);
        let term_c = term.clone();
        let item_h = gtk::MenuItem::with_label("Move to Horizontal Split");
        item_h.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.move_tab_to_split(&term_c, "horizontal");
            }
        });
        menu.append(&item_h);
        menu.show_all();
        crate::popup_transient_menu(&menu);
    }

    fn move_tab_to_split(&self, term: &TerminalBox, mode: &str) {
        let Some((src_nb, src_idx)) = self.find_terminal(term) else {
            return;
        };
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let total = nb.n_pages() + nb2.n_pages();
        if total < 2 {
            self.set_split(mode, true);
            return;
        }
        if src_nb == nb2 {
            self.move_tab_between(&nb2, &nb, src_idx);
            self.set_split(mode, false);
        } else {
            self.move_tab_between(&nb, &nb2, src_idx);
            self.set_split(mode, false);
        }
    }

    pub fn close_tab(&self, term: Option<&TerminalBox>) {
        let term = match term {
            Some(t) => t.clone(),
            None => match self.current_terminal() {
                Some(t) => t,
                None => return,
            },
        };
        if let Some((nb, idx)) = self.find_terminal(&term) {
            term.terminate();
            self.imp().tab_labels.borrow_mut().remove(&term);
            self.imp().tab_base_titles.borrow_mut().remove(&term);
            self.imp().tab_custom_titles.borrow_mut().remove(&term);
            nb.remove_page(Some(idx));
            self.update_tabs_menu();
        }
    }

    fn on_page_removed(&self) {
        if *self.imp().restoring_session.borrow() || *self.imp().moving_tab.borrow() {
            return;
        }
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        if nb.n_pages() == 0 && nb2.n_pages() == 0 {
            self.on_close(self);
        } else if *self.imp().split_mode.borrow() != "single"
            && (nb.n_pages() == 0 || nb2.n_pages() == 0)
        {
            // Collapse the split when either pane runs out of tabs. Without
            // this, closing one side leaves an empty pane and collapsing it
            // later would briefly make both notebooks empty.
            self.set_split("single", false);
        }
    }

    pub fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str) {
        // A fixed title requested via `--title` overrides any title the running
        // program tries to set through OSC escapes.
        if let Some(forced) = self.imp().forced_title.borrow().clone() {
            self.set_tab_text(term, &forced);
            if self.total_tabs() == 1 {
                self.update_title(&forced);
            }
            return;
        }
        // Once the user has renamed a tab, keep that name: shell prompts and
        // other OSC title updates must not overwrite an explicit choice.
        if self.imp().tab_custom_titles.borrow().contains_key(term) {
            return;
        }
        let base = self
            .imp()
            .tab_base_titles
            .borrow()
            .get(term)
            .cloned()
            .unwrap_or_else(|| settings().get_str("tab_title"));
        let mode = settings().get_str("dynamic_title");
        let mut num = String::new();
        if let Some(last) = base.split_whitespace().next_back() {
            if last.chars().all(|c| c.is_ascii_digit()) {
                num = last.to_string();
            }
        }
        let display = if mode == "replace" {
            if num.is_empty() {
                title.to_string()
            } else {
                format!("{}. {}", num, title)
            }
        } else if mode == "before" {
            if num.is_empty() {
                format!("{} | {}", title, base)
            } else {
                format!("{}. {} | {}", num, title, base)
            }
        } else if mode == "after" {
            if num.is_empty() {
                format!("{} | {}", base, title)
            } else {
                format!("{}. {} | {}", num, base, title)
            }
        } else {
            base
        };
        self.set_tab_text(term, &display);
        if self.total_tabs() == 1 {
            self.update_title(&format!("{} - {}", APP_NAME, title));
        }
    }

    fn update_tabs_menu(&self) {
        self.update_tab_list_button();
        let tabs_menu = self.imp().tabs_menu.borrow().clone();
        if let Some(menu) = tabs_menu {
            self.populate_tab_menu(&menu);
        }
    }

    fn populate_tab_menu(&self, menu: &gtk::Menu) {
        for child in menu.children() {
            menu.remove(&child);
        }
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let mut idx = 0;
        for (notebook, prefix) in [(&nb, ""), (&nb2, "[R] ")] {
            let cur = *self
                .imp()
                .current_pages
                .borrow()
                .get(notebook)
                .unwrap_or(&notebook.current_page().unwrap_or(0));
            let cur_page = if cur < notebook.n_pages() {
                notebook.nth_page(Some(cur))
            } else {
                None
            };
            for i in 0..notebook.n_pages() {
                let Some(page) = notebook.nth_page(Some(i)) else {
                    continue;
                };
                let is_cur = Some(page.clone()) == cur_page;
                let Ok(term) = page.clone().downcast::<TerminalBox>() else {
                    continue;
                };
                let text = self.get_tab_text(&term);
                let label = format!("{}{}. {}", prefix, idx + 1, text);
                let weak = crate::SendWeak::new(self);
                let nb_c = notebook.clone();
                let item = gtk::MenuItem::with_label(&label);
                item.connect_activate(move |_| {
                    if let Some(w) = weak.upgrade() {
                        w.jump_to_tab(&nb_c, i);
                    }
                });
                if is_cur {
                    item.set_sensitive(false);
                }
                menu.append(&item);
                item.show();
                idx += 1;
            }
        }
        if idx == 0 {
            let item = gtk::MenuItem::with_label("(no tabs)");
            item.set_sensitive(false);
            menu.append(&item);
            item.show();
        }

        let separator = gtk::SeparatorMenuItem::new();
        menu.append(&separator);
        separator.show();
        let weak = crate::SendWeak::new(self);
        let item = gtk::MenuItem::with_label("Move Tab from Another Oxterm…");
        item.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.move_tab_from_another_oxterm();
            }
        });
        menu.append(&item);
        item.show();
    }

    /// Offer the tabs of the other running Oxterm windows and adopt one here.
    fn move_tab_from_another_oxterm(&self) {
        let weak = crate::SendWeak::new(self);
        // Discovery talks to other processes: never block the GTK thread on a
        // window that is busy or wedged.
        std::thread::spawn(move || {
            let panes = crate::control::foreign_panes();
            crate::run_on_main(move || {
                if let Some(w) = weak.upgrade() {
                    w.show_move_pane_dialog(panes);
                }
            });
        });
    }

    fn show_move_pane_dialog(&self, panes: Vec<crate::control::ForeignPane>) {
        if panes.is_empty() {
            self.status_message("No other Oxterm window is running", false);
            return;
        }
        let dialog = gtk::Dialog::with_buttons(
            Some("Move a Tab from Another Oxterm"),
            Some(self),
            gtk::DialogFlags::MODAL,
            &[
                ("Move Here", gtk::ResponseType::Accept),
                ("Cancel", gtk::ResponseType::Cancel),
            ],
        );
        dialog.set_default_size(640, 380);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for pane in &panes {
            let heading = if pane.title.is_empty() {
                "(untitled)"
            } else {
                pane.title.as_str()
            };
            let detail = if pane.running {
                format!("{}  ·  running: {}", pane.cwd, pane.command)
            } else if pane.command.is_empty() {
                pane.cwd.clone()
            } else {
                format!("{}  ·  last: {}", pane.cwd, pane.command)
            };
            let title = gtk::Label::new(None);
            title.set_text(heading);
            title.set_xalign(0.0);
            title.set_ellipsize(pango::EllipsizeMode::End);
            let subtitle = gtk::Label::new(None);
            subtitle.set_text(&detail);
            subtitle.set_xalign(0.0);
            subtitle.set_ellipsize(pango::EllipsizeMode::Middle);
            subtitle.style_context().add_class("oxterm-stats-label");
            let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
            column.set_margin_start(10);
            column.set_margin_end(10);
            column.set_margin_top(6);
            column.set_margin_bottom(6);
            column.pack_start(&title, false, false, 0);
            column.pack_start(&subtitle, false, false, 0);
            let row = gtk::ListBoxRow::new();
            row.add(&column);
            // The whole session id is long; the tooltip carries it for bug
            // reports without eating the row.
            row.set_tooltip_text(Some(&format!("{}  ({})", heading, pane.id)));
            list.add(&row);
        }
        // Double-click or Enter picks the row and accepts the dialog.
        let weak_dialog = dialog.clone();
        list.connect_row_activated(move |_, _| {
            weak_dialog.response(gtk::ResponseType::Accept);
        });
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.add(&list);
        dialog.content_area().pack_start(&scroll, true, true, 0);
        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }
        dialog.show_all();

        let response = dialog.run();
        let selected = list
            .selected_row()
            .and_then(|row| usize::try_from(row.index()).ok());
        dialog.close();
        if response != gtk::ResponseType::Accept {
            return;
        }
        let Some(pane) = selected.and_then(|index| panes.get(index)).cloned() else {
            return;
        };
        self.move_pane_here(pane);
    }

    /// Attach this window to `pane`'s session, then ask the other Oxterm to let
    /// go of it. The relay tab is created first so the session is never without
    /// a view, and is dropped again if the other window refuses.
    fn move_pane_here(&self, pane: crate::control::ForeignPane) {
        let binary = std::env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "oxterm".to_string());
        let command = vec![binary, "-a".to_string(), pane.id.clone()];
        let title = if pane.title.is_empty() {
            "Terminal".to_string()
        } else {
            pane.title.clone()
        };
        let cwd = (!pane.cwd.is_empty()).then_some(pane.cwd.as_str());
        let term = self.add_new_tab(cwd, None, Some(&title), Some(&title), Some(&command));

        let weak = crate::SendWeak::new(self);
        let weak_term = crate::SendWeak::new(&term);
        let source = pane.socket.clone();
        let id = pane.id.clone();
        let fifo = pane.fifo.clone();
        std::thread::spawn(move || {
            // Become a reader of the OSC 133 FIFO *before* the source lets go:
            // a shell whose last reader disappears dies of SIGPIPE at the next
            // prompt, and that would kill the session we are adopting.
            let reader = fifo
                .as_deref()
                .map(|path| TerminalBox::open_osc133_fifo(path).map(|fd| (path.to_string(), fd)));
            let mut result = if matches!(reader, Some(None)) {
                Err("cannot open the session's shell integration FIFO".to_string())
            } else {
                crate::control::handover(&source, &id)
            };
            // The source closes its last window as part of a successful move,
            // so it may never answer; a dead socket after a failure is still a
            // success for us.
            if result.is_err() && !crate::control::socket_alive(&source) {
                result = Ok(());
            }
            crate::run_on_main(move || {
                let Some(w) = weak.upgrade() else {
                    if let Some(Some((_, fd))) = reader {
                        unsafe { libc::close(fd) };
                    }
                    return;
                };
                match result {
                    Ok(()) => {
                        if let (Some(term), Some(Some((path, fd)))) = (weak_term.upgrade(), reader)
                        {
                            term.adopt_osc133_fifo(&path, fd);
                        }
                        w.status_message(&format!("Moved \"{title}\" from another Oxterm"), false);
                    }
                    Err(error) => {
                        if let Some(Some((_, fd))) = reader {
                            unsafe { libc::close(fd) };
                        }
                        if let Some(term) = weak_term.upgrade() {
                            w.close_tab(Some(&term));
                        }
                        w.status_message(&format!("Could not move the tab: {}", error), true);
                    }
                }
            });
        });
    }

    fn populate_profiles_menu(&self, menu: &gtk::Menu) {
        for child in menu.children() {
            menu.remove(&child);
        }
        let active = settings().get_str("active_profile");
        let profiles = crate::profiles::list_profiles();
        if !profiles.is_empty() {
            for name in &profiles {
                let marker = if name == &active { " \u{2713}" } else { "" };
                let item = gtk::MenuItem::with_label(&format!("{}{}", name, marker));
                let name_c = name.clone();
                item.connect_activate(move |_| {
                    crate::profiles::apply_profile(settings(), &name_c);
                });
                menu.append(&item);
                item.show();
            }
            menu.append(&gtk::SeparatorMenuItem::new());
        }
        let weak = crate::SendWeak::new(self);
        let item = gtk::MenuItem::with_label("Save Current as Profile...");
        item.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.save_profile_dialog();
            }
        });
        menu.append(&item);
        if !profiles.is_empty() {
            let del_menu = gtk::Menu::new();
            for name in &profiles {
                let name_c = name.clone();
                let ditem = gtk::MenuItem::with_label(name);
                ditem.connect_activate(move |_| {
                    crate::profiles::delete_profile(&name_c);
                });
                del_menu.append(&ditem);
            }
            let ditem_item = gtk::MenuItem::with_label("Delete Profile");
            ditem_item.set_submenu(Some(&del_menu));
            menu.append(&ditem_item);
        }
        menu.show_all();
    }

    fn populate_sessions_menu(&self, menu: &gtk::Menu) {
        for child in menu.children() {
            menu.remove(&child);
        }
        let sessions = crate::session::list_sessions();
        if !sessions.is_empty() {
            for name in &sessions {
                let item = gtk::MenuItem::with_label(&format!("Restore: {}", name));
                let name_c = name.clone();
                let weak = crate::SendWeak::new(self);
                item.connect_activate(move |_| {
                    if let Some(w) = weak.upgrade() {
                        w.load_session_named(&name_c);
                    }
                });
                menu.append(&item);
                item.show();
            }
            menu.append(&gtk::SeparatorMenuItem::new());
        }
        let weak = crate::SendWeak::new(self);
        let item = gtk::MenuItem::with_label("Save Current Session As...");
        item.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.save_session_dialog();
            }
        });
        menu.append(&item);
        if !sessions.is_empty() {
            let del_menu = gtk::Menu::new();
            for name in &sessions {
                let name_c = name.clone();
                let ditem = gtk::MenuItem::with_label(name);
                ditem.connect_activate(move |_| {
                    crate::session::delete_session(&name_c);
                });
                del_menu.append(&ditem);
            }
            let ditem_item = gtk::MenuItem::with_label("Delete Session");
            ditem_item.set_submenu(Some(&del_menu));
            menu.append(&ditem_item);
        }
        menu.show_all();
    }

    fn jump_to_tab(&self, nb: &gtk::Notebook, page_idx: u32) {
        nb.set_current_page(Some(page_idx));
        nb.show_all();
        if let Some(page) = nb.nth_page(Some(page_idx)) {
            if let Ok(term) = page.downcast::<TerminalBox>() {
                term.vte().grab_focus();
            }
        }
    }

    fn show_tmux_session_list(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Oxterm Sessions"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Close", gtk::ResponseType::Close)],
        );
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let mut choices = Vec::new();
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in 0..notebook.n_pages() {
                let Some(page) = notebook.nth_page(Some(i)) else {
                    continue;
                };
                let Ok(term) = page.downcast::<TerminalBox>() else {
                    continue;
                };
                let label = gtk::Label::new(Some(&format!(
                    "{}  |  {}  |  {}  |  {}  |  {} [{}]",
                    self.get_tab_text(&term),
                    term.remote_id(),
                    term.remote_status(),
                    term.get_cwd(),
                    if term.remote_command().is_empty() {
                        "-".to_string()
                    } else {
                        term.remote_command()
                    },
                    term.remote_command_status()
                )));
                label.set_xalign(0.0);
                label.set_margin_start(8);
                label.set_margin_end(8);
                label.set_margin_top(6);
                label.set_margin_bottom(6);
                let row = gtk::ListBoxRow::new();
                row.add(&label);
                list.add(&row);
                choices.push(term);
            }
        }
        if choices.is_empty() {
            let label = gtk::Label::new(Some("No active Oxterm terminals."));
            label.set_margin_top(12);
            label.set_margin_bottom(12);
            list.add(&label);
        }
        let weak = crate::SendWeak::new(self);
        let dialog_for_rows = dialog.clone();
        list.connect_row_activated(move |_list, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            if let Some(term) = choices.get(index as usize) {
                term.reattach_local();
                if let Some(w) = weak.upgrade() {
                    if let Some((notebook, page)) = w.find_terminal(term) {
                        w.jump_to_tab(&notebook, page);
                    }
                }
                dialog_for_rows.close();
            }
        });
        list.show_all();
        dialog.content_area().pack_start(&list, true, true, 0);
        dialog.set_default_size(760, 320);
        dialog.show_all();
        dialog.run();
        dialog.close();
    }

    fn on_switch_tab(&self, page: &gtk::Widget, page_num: u32) {
        let nb = page
            .parent()
            .and_then(|p| p.downcast::<gtk::Notebook>().ok());
        if let Some(nb) = nb {
            self.imp().current_pages.borrow_mut().insert(nb, page_num);
        }
        if let Ok(term) = page.clone().downcast::<TerminalBox>() {
            self.clear_tab_failure(&term);
            self.update_cwd_badge(&term);
            let weak = crate::SendWeak::new(&term);
            glib::idle_add_local(move || {
                if let Some(t) = weak.upgrade() {
                    t.vte().grab_focus();
                }
                glib::ControlFlow::Break
            });
            if let Some(ro) = self.imp().read_only_action.borrow().clone() {
                ro.set_active(!term.vte().is_input_enabled());
            }
        }
        self.update_tabs_menu();
        let weak = crate::SendWeak::new(self);
        glib::idle_add_local(move || {
            if let Some(w) = weak.upgrade() {
                w.apply_tab_colors();
            }
            glib::ControlFlow::Break
        });
    }

    // ── Menu callbacks ───────────────────────────────────────

    fn on_new_tab(&self) {
        self.add_new_tab(None, None, None, None, None);
    }

    fn on_new_window(&self) {
        match self.application() {
            Some(app) => open_new_main_window(&app),
            None => spawn_new_process(true),
        }
    }

    fn on_open_fm(&self) {
        match detect_file_manager() {
            Some(fm) => {
                if let Some(term) = self.current_terminal() {
                    let cwd = term.get_cwd();
                    crate::notes::spawn_detached(&fm, &[&cwd]);
                }
            }
            None => {
                let dialog = gtk::MessageDialog::new(
                    Some(self),
                    gtk::DialogFlags::MODAL,
                    gtk::MessageType::Warning,
                    gtk::ButtonsType::Ok,
                    "No file manager found.",
                );
                dialog.run();
                dialog.close();
            }
        }
    }

    fn open_settings(&self) {
        crate::settings_dialog::show_settings_dialog(Some(self.upcast_ref::<gtk::Window>()));
    }

    fn toggle_fullscreen(&self) {
        if let Some(w) = self.window() {
            if w.state() & gdk::WindowState::FULLSCREEN != gdk::WindowState::empty() {
                self.unfullscreen();
            } else {
                self.fullscreen();
            }
        }
    }

    fn zoom_all<F: Fn(&TerminalBox)>(&self, f: F) {
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in 0..notebook.n_pages() {
                if let Some(page) = notebook.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        f(&term);
                    }
                }
            }
        }
    }

    fn open_set_title_dialog(&self) {
        if let Some(term) = self.current_terminal() {
            self.rename_tab_dialog(&term);
        }
    }

    fn rename_tab_dialog(&self, term: &TerminalBox) {
        // Renaming an inactive tab is allowed from its context menu: bring it
        // to the front first so the dialog refers to a visible page.
        if let Some((notebook, page)) = self.find_terminal(term) {
            self.jump_to_tab(&notebook, page);
        }
        let dialog = gtk::Dialog::with_buttons(
            Some("Rename Tab"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Rename", gtk::ResponseType::Ok),
            ],
        );
        let entry = gtk::Entry::new();
        entry.set_text(&self.get_tab_text(term));
        entry.set_position(-1);
        entry.select_region(0, -1);
        entry.set_activates_default(true);
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.set_default_response(gtk::ResponseType::Ok);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            self.rename_tab(term, entry.text().as_str());
        }
        dialog.close();
    }

    fn rename_tab(&self, term: &TerminalBox, title: &str) {
        let title: String = title.trim().chars().take(200).collect();
        if title.is_empty() {
            return;
        }
        self.imp()
            .tab_custom_titles
            .borrow_mut()
            .insert(term.clone(), title.clone());
        self.set_tab_text(term, &title);
        if self
            .current_terminal()
            .is_some_and(|current| &current == term)
        {
            self.update_title(&format!("{} - {}", APP_NAME, title));
        }
        self.update_tabs_menu();
    }

    fn reset_tab_title(&self, term: &TerminalBox) {
        if self
            .imp()
            .tab_custom_titles
            .borrow_mut()
            .remove(term)
            .is_none()
        {
            return;
        }
        let base = self
            .imp()
            .tab_base_titles
            .borrow()
            .get(term)
            .cloned()
            .unwrap_or_else(|| settings().get_str("tab_title"));
        let title = term
            .vte()
            .window_title()
            .map(|t| t.to_string())
            .unwrap_or(base);
        self.set_tab_title_from_terminal(term, &title);
        if self
            .current_terminal()
            .is_some_and(|current| &current == term)
        {
            self.update_title(&format!("{} - {}", APP_NAME, self.get_tab_text(term)));
        }
        self.update_tabs_menu();
    }

    fn save_profile_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Save Profile"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Save", gtk::ResponseType::Ok),
            ],
        );
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Profile name..."));
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let name = entry.text().trim().to_string();
            if !name.is_empty() {
                let data = settings().raw_data();
                match crate::profiles::save_profile(&name, &data) {
                    true => {
                        settings().set_str("active_profile", &name);
                    }
                    false => {
                        self.show_error(
                            "Could not save the profile. See the application log for details.",
                        );
                    }
                }
            }
        }
        dialog.close();
    }

    fn save_session_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Save Session"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Save", gtk::ResponseType::Ok),
            ],
        );
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Session name..."));
        let now = crate::logging::utc_iso_now();
        entry.set_text(&now[..10]);
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let name = entry.text().trim().to_string();
            if !name.is_empty() && !self.save_session_named(&name) {
                self.show_error("Could not save the session. See the application log for details.");
            }
        }
        dialog.close();
    }

    fn show_error(&self, message: &str) {
        let dialog = gtk::MessageDialog::new(
            Some(self),
            gtk::DialogFlags::MODAL,
            gtk::MessageType::Error,
            gtk::ButtonsType::Ok,
            message,
        );
        dialog.run();
        dialog.close();
    }

    fn set_encoding(&self, encoding: &str) {
        if let Some(term) = self.current_terminal() {
            term.set_encoding(encoding);
            let text = self.get_tab_text(&term);
            let base = text.split(" [").next().unwrap_or("").to_string();
            self.set_tab_text(&term, &format!("{} [{}]", base, encoding));
            settings().set_str("encoding", encoding);
        }
        for (action, enc_name) in self.imp().encoding_actions.borrow().iter() {
            action.set_active(*enc_name == encoding);
        }
    }

    fn prev_tab(&self) {
        let nb = self.focused_notebook();
        let n = nb.n_pages();
        if n > 1 {
            let idx = nb.current_page().unwrap_or(0);
            nb.set_current_page(Some(idx.checked_sub(1).unwrap_or(n - 1)));
        }
    }

    fn next_tab(&self) {
        let nb = self.focused_notebook();
        let n = nb.n_pages();
        if n > 1 {
            let idx = nb.current_page().unwrap_or(0);
            nb.set_current_page(Some((idx + 1) % n));
        }
    }

    fn move_tab(&self, delta: i32) {
        let nb = self.focused_notebook();
        let n = nb.n_pages();
        let idx = nb.current_page().unwrap_or(0);
        let new_idx = (idx as i32 + delta).max(0) as u32;
        if new_idx < n {
            if let Some(page) = nb.nth_page(Some(idx)) {
                nb.reorder_child(&page, Some(new_idx));
            }
            self.update_tabs_menu();
        }
    }

    fn detach_tab(&self, term: Option<&TerminalBox>) {
        let term = match term {
            Some(t) => t.clone(),
            None => match self.current_terminal() {
                Some(t) => t,
                None => return,
            },
        };
        let Some((nb, idx)) = self.find_terminal(&term) else {
            return;
        };
        let title = self.get_tab_text(&term);
        // Removing the final page closes the now-empty source window. That is
        // a consequence of detaching, not a user-requested window close.
        //
        // Keep the application alive across that gap: destroying the last
        // window drops GtkApplication's hold, and without this extra hold the
        // main loop would exit before the detached window is registered,
        // taking every tab with it.
        let app = self.application();
        let _app_hold = app.as_ref().map(|app| app.hold());
        *self.imp().skip_close_confirm.borrow_mut() = true;
        nb.remove_page(Some(idx));
        *self.imp().skip_close_confirm.borrow_mut() = false;
        self.imp().tab_labels.borrow_mut().remove(&term);
        self.imp().tab_base_titles.borrow_mut().remove(&term);
        self.imp().tab_custom_titles.borrow_mut().remove(&term);
        self.update_tabs_menu();

        let new_win = DetachedWindow::new(&term, &title);
        // Register with the application so the process (and this tab's
        // shell) keeps running when the last main window goes away. The
        // source window may already be destroyed here, which clears its
        // `application` pointer, so use the reference captured above.
        if let Some(app) = app.as_ref() {
            app.add_window(&new_win);
        }
        term.set_window_ref(&new_win);
        let weak = crate::SendWeak::new(&term);
        new_win.connect_destroy(move |_| {
            if let Some(t) = weak.upgrade() {
                t.terminate();
            }
        });
        new_win.show_all();
        let weak = crate::SendWeak::new(&term);
        glib::idle_add_local(move || {
            if let Some(t) = weak.upgrade() {
                t.vte().grab_focus();
            }
            glib::ControlFlow::Break
        });
    }

    fn copy_all_to_notes(&self, term: &TerminalBox) {
        let vte = term.vte();
        // VTE rows are absolute: the oldest retained line is at the
        // adjustment's lower bound, not necessarily row 0.
        let vadj = term.vadjustment();
        let first_row = vadj.lower() as i64;
        let last_row = (vadj.upper() as i64).max(first_row);
        let text =
            crate::terminal::text::range(&vte, first_row, 0, last_row, 0).unwrap_or_default();
        if text.trim().is_empty() {
            self.note_feedback("Nothing to copy from the terminal", true);
            return;
        }
        let window = self.clone().upcast::<gtk::Window>();
        if !crate::notes_ui::confirm_large_append(
            &window,
            "all terminal text",
            text.lines().count(),
            text.chars().count(),
        ) {
            return;
        }
        let Some(name) = crate::notes_ui::pick_note_file(&window, "Copy all to note") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        let context = NoteContext::from_terminal("scrollback", &term.get_cwd());
        self.note_write(text, filename, context);
    }

    // ── Notes panel ──────────────────────────────────────────

    /// Build the notes panel the first time it is needed. Keeping this out of
    /// `init` avoids creating a dozen widgets (and an empty list/text view) in
    /// every window that never opens the panel.
    fn ensure_notes_panel(&self) -> bool {
        if self.imp().notes_panel.borrow().is_some() {
            return true;
        }
        let Some(slot) = self.imp().notes_slot.borrow().clone() else {
            return false;
        };
        let panel = crate::notes_panel::NotesPanel::build();
        {
            let weak = crate::SendWeak::new(self);
            panel.close.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.set_notes_panel_visible(false);
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.refresh.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.reload_notes_panel();
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.search.connect_search_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.run_notes_search();
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.tag.connect_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.run_notes_search();
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.list.connect_row_selected(move |_, row| {
                if let (Some(w), Some(row)) = (weak.upgrade(), row) {
                    w.notes_panel_preview(row.index());
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.append.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.notes_panel_append();
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.entry.connect_activate(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.notes_panel_append();
                }
            });
        }
        slot.pack_start(&panel.root, false, true, 0);
        *self.imp().notes_panel.borrow_mut() = Some(panel);
        true
    }

    fn notes_panel_visible(&self) -> bool {
        self.imp()
            .notes_panel
            .borrow()
            .as_ref()
            .map(|p| p.root.is_visible())
            .unwrap_or(false)
    }

    fn set_notes_panel_visible(&self, visible: bool) {
        if visible && !self.ensure_notes_panel() {
            return;
        }
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            if visible {
                // The panel carries `no_show_all` so the window's own
                // `show_all()` keeps it hidden; clear it just long enough to
                // show the panel and its children, then restore it.
                panel.root.set_no_show_all(false);
                panel.root.show_all();
                panel.root.set_no_show_all(true);
            } else {
                panel.root.hide();
            }
        }
    }

    fn refresh_note_targets(&self) {
        let notes = crate::notes::list_note_files();
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.set_targets(&notes);
        }
    }

    fn reload_notes_panel(&self) {
        self.refresh_note_targets();
        self.set_panel_status("Indexing notes...");
        let weak = crate::SendWeak::new(self);
        crate::notes_index::refresh_async(move |result| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(count) => {
                    w.set_panel_status(&format!("{} entries indexed", count));
                    w.run_notes_search();
                    let weak = crate::SendWeak::new(&w);
                    crate::notes_index::tags_async(move |tags| {
                        if let (Some(w), Ok(tags)) = (weak.upgrade(), tags) {
                            w.set_panel_tags(&tags);
                        }
                    });
                }
                Err(e) => w.set_panel_status(&format!("Index error: {}", e)),
            }
        });
    }

    fn run_notes_search(&self) {
        let (query, tag) = match self.imp().notes_panel.borrow().as_ref() {
            Some(panel) => (panel.search_query(), panel.active_tag()),
            None => return,
        };
        let weak = crate::SendWeak::new(self);
        crate::notes_index::search_async(query, tag, 200, move |result| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(entries) => {
                    let count = entries.len();
                    w.set_panel_entries(entries);
                    w.set_panel_status(&format!("{} results", count));
                }
                Err(e) => w.set_panel_status(&format!("Search error: {}", e)),
            }
        });
    }

    fn set_panel_status(&self, message: &str) {
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.set_status(message);
        }
    }

    fn set_panel_entries(&self, entries: Vec<crate::notes_index::NoteEntry>) {
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.set_entries(entries);
        }
    }

    fn set_panel_tags(&self, tags: &[String]) {
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.set_tags(tags);
        }
    }

    fn notes_panel_preview(&self, index: i32) {
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.preview_index(index);
        }
    }

    fn notes_panel_append(&self) {
        let (text, target) = match self.imp().notes_panel.borrow().as_ref() {
            Some(panel) => (panel.take_entry_text(), panel.selected_target()),
            None => return,
        };
        if text.trim().is_empty() {
            return;
        }
        let filename = if target.is_empty() {
            None
        } else {
            Some(target)
        };
        let cwd = self
            .current_terminal()
            .map(|t| t.get_cwd())
            .unwrap_or_default();
        let context = if cwd.is_empty() {
            NoteContext {
                source: Some("panel".to_string()),
                ..NoteContext::default()
            }
        } else {
            NoteContext::from_terminal("panel", &cwd)
        };
        self.note_write(text, filename, context);
    }

    fn show_about(&self) {
        crate::settings_dialog::show_about_dialog(Some(self.upcast_ref::<gtk::Window>()));
    }

    // ── Window signals ───────────────────────────────────────

    /// Close without confirmation, saving the session as on a normal close
    /// (used on SIGTERM/SIGHUP, e.g. at logout).
    pub fn shutdown(&self) {
        *self.imp().skip_close_confirm.borrow_mut() = true;
        self.on_close(self);
    }

    fn on_close(&self, _widget: &MainWindow) -> glib::Propagation {
        if *self.imp().closing.borrow() {
            return glib::Propagation::Stop;
        }
        let has_active_process = self.any_terminal_has_active_process();
        if settings().get_bool("confirm_close")
            && has_active_process
            && !*self.imp().skip_close_confirm.borrow()
        {
            *self.imp().closing.borrow_mut() = true;
            let dialog = gtk::MessageDialog::new(
                Some(self),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Question,
                gtk::ButtonsType::YesNo,
                "Close all tabs and exit?",
            );
            let resp = dialog.run();
            dialog.close();
            if resp != gtk::ResponseType::Yes {
                *self.imp().closing.borrow_mut() = false;
                return glib::Propagation::Stop;
            }
        }

        *self.imp().closing.borrow_mut() = true;
        self.stop_stats_timer();
        if let Some(id) = self.imp().autosave_source.borrow_mut().take() {
            id.remove();
        }

        // Detached tab windows are not MainWindows and do not own a session.
        if self.main_window_count() <= 1 {
            self.save_session("last");
        }
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in (0..notebook.n_pages()).rev() {
                if let Some(page) = notebook.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        term.terminate();
                    }
                }
            }
        }
        for handler in std::mem::take(&mut *self.imp().settings_handlers.borrow_mut()) {
            settings().disconnect_changed(handler);
        }
        unsafe { self.destroy() };
        glib::Propagation::Stop
    }

    fn on_window_key(
        &self,
        _widget: &gtk::ApplicationWindow,
        event: &gdk::EventKey,
    ) -> glib::Propagation {
        let ctrl = event.state().contains(gdk::ModifierType::CONTROL_MASK);
        let key = event.keyval();
        if (ctrl && key == K::F11) || key == K::F11 {
            self.toggle_fullscreen();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    // ── Public signals for TerminalBox ───────────────────────

    pub fn new_tab_signal(&self) {
        self.add_new_tab(None, None, None, None, None);
    }

    pub fn new_window_signal(&self) {
        self.on_new_window();
    }

    pub fn close_tab_signal(&self, term: Option<&TerminalBox>) {
        *self.imp().skip_close_confirm.borrow_mut() = true;
        self.close_tab(term);
        *self.imp().skip_close_confirm.borrow_mut() = false;
    }

    pub fn close_window_signal(&self) {
        self.on_close(self);
    }

    pub fn set_title_dialog(&self) {
        self.open_set_title_dialog();
    }

    pub fn reset_terminal(&self) {
        if let Some(term) = self.current_terminal() {
            term.reset(false);
        }
    }

    pub fn reset_and_clear(&self) {
        if let Some(term) = self.current_terminal() {
            term.reset(true);
        }
    }

    pub fn split_signal(&self, mode: &str) {
        let current = self.imp().split_mode.borrow().clone();
        if current == mode {
            self.set_split("single", true);
        } else {
            self.set_split(mode, true);
        }
    }

    pub fn focus_other_pane_signal(&self) {
        if *self.imp().split_mode.borrow() != "single" {
            self.focus_other_pane();
        }
    }

    pub fn toggle_split_zoom_signal(&self) {
        self.toggle_split_zoom();
    }

    pub fn equalize_splits_signal(&self) {
        self.equalize_splits();
    }

    pub fn set_tab_failure_signal(&self, term: &TerminalBox, failed: bool) {
        self.set_tab_failure(term, failed);
    }

    pub fn clear_tab_failure_signal(&self, term: &TerminalBox) {
        self.clear_tab_failure(term);
    }

    pub fn prev_tab_signal(&self) {
        self.prev_tab();
    }

    pub fn next_tab_signal(&self) {
        self.next_tab();
    }

    pub fn session_list_signal(&self) {
        self.show_tmux_session_list();
    }

    pub fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]) {
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in 0..notebook.n_pages() {
                if let Some(page) = notebook.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        if term != *source && term.vte().is_input_enabled() {
                            term.vte().feed_child(data);
                        }
                    }
                }
            }
        }
    }

    // ── Stats ────────────────────────────────────────────────

    fn apply_stats_visibility(&self) {
        let visible = settings().get_bool("show_stats");
        if let Some(box_) = self.imp().stats_box.borrow().clone() {
            box_.set_visible(visible);
        }
        if let Some(l) = self.imp().stats_sys_label.borrow().clone() {
            l.set_visible(visible);
        }
        if let Some(l) = self.imp().stats_self_label.borrow().clone() {
            l.set_visible(visible);
        }
        if visible {
            self.refresh_active_cwd_badge();
            self.start_stats_timer();
        } else {
            self.stop_stats_timer();
        }
    }

    fn start_stats_timer(&self) {
        if self.imp().stats_source_id.borrow().is_some() {
            return;
        }
        self.refresh_stats();
        let weak = crate::SendWeak::new(self);
        let source = glib::timeout_add_seconds_local(3, move || {
            if let Some(w) = weak.upgrade() {
                // Nobody can see the stats of a minimized window.
                let iconified = gtk::prelude::WidgetExt::window(&w)
                    .is_some_and(|gw| gw.state().contains(gdk::WindowState::ICONIFIED));
                if !iconified {
                    w.refresh_stats();
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        *self.imp().stats_source_id.borrow_mut() = Some(source);
    }

    fn stop_stats_timer(&self) {
        if let Some(id) = self.imp().stats_source_id.borrow_mut().take() {
            id.remove();
        }
        // Leave remote_stats_pending set while a probe is still running: it
        // is cleared when that probe reports back, so toggling the stats bar
        // cannot stack up concurrent ssh probes.
        *self.imp().remote_stats_generation.borrow_mut() += 1;
    }

    fn refresh_stats(&self) {
        let Some(term) = self.current_terminal() else {
            return;
        };
        // Focus can move between split panes without a tab switch; the badge
        // follows the active terminal on every stats tick as well.
        self.update_cwd_badge(&term);
        if term.is_ssh_client() {
            if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                label.set_text(&crate::system_stats::ssh_placeholder());
            }
            if !*self.imp().remote_stats_pending.borrow() {
                *self.imp().remote_stats_pending.borrow_mut() = true;
                *self.imp().remote_stats_generation.borrow_mut() += 1;
                let gen = *self.imp().remote_stats_generation.borrow();
                let weak = crate::SendWeak::new(self);
                let term_id = term.remote_id();
                let Some((ssh_args, cached)) = term.remote_stats_request() else {
                    *self.imp().remote_stats_pending.borrow_mut() = false;
                    return;
                };
                if let Some(stats) = cached {
                    *self.imp().remote_stats_pending.borrow_mut() = false;
                    if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                        label.set_text(&stats);
                    }
                    return;
                }
                std::thread::spawn(move || {
                    let stats = TerminalBox::collect_remote_stats(&ssh_args);
                    crate::run_on_main(move || {
                        if let Some(w) = weak.upgrade() {
                            *w.imp().remote_stats_pending.borrow_mut() = false;
                            if gen == *w.imp().remote_stats_generation.borrow() {
                                let is_active = w
                                    .current_terminal()
                                    .is_some_and(|term| term.remote_id() == term_id);
                                if is_active {
                                    if let Some(term) = w.current_terminal() {
                                        term.cache_remote_stats(&stats);
                                    }
                                    if let Some(label) = w.imp().stats_sys_label.borrow().clone() {
                                        if stats.is_empty() {
                                            label.set_text(&crate::system_stats::ssh_placeholder());
                                        } else {
                                            label.set_text(&stats);
                                        }
                                    }
                                }
                            }
                        }
                    });
                });
            }
            if let Some(label) = self.imp().stats_self_label.borrow().clone() {
                label.set_text(&crate::system_stats::collect_self());
            }
            return;
        }
        let stats = if term.is_ssh() {
            format!("  [SSH] {}", crate::system_stats::collect(false).trim())
        } else {
            crate::system_stats::collect(false)
        };
        if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
            label.set_text(&stats);
        }
        if let Some(label) = self.imp().stats_self_label.borrow().clone() {
            label.set_text(&crate::system_stats::collect_self());
        }
    }

    // ── Sessions ─────────────────────────────────────────────

    fn build_session_data(&self) -> crate::session::SessionData {
        let mut data = crate::session::SessionData {
            split_mode: self.imp().split_mode.borrow().clone(),
            ..Default::default()
        };
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for (notebook, target) in [(&nb, &mut data.tabs_left), (&nb2, &mut data.tabs_right)] {
            for i in 0..notebook.n_pages() {
                let Some(page) = notebook.nth_page(Some(i)) else {
                    continue;
                };
                let Ok(term) = page.downcast::<TerminalBox>() else {
                    continue;
                };
                let base_title = self
                    .imp()
                    .tab_base_titles
                    .borrow()
                    .get(&term)
                    .cloned()
                    .unwrap_or_else(|| self.get_tab_text(&term));
                let title = self.get_tab_text(&term);
                let cwd = term.get_cwd();
                let custom_title = self.imp().tab_custom_titles.borrow().contains_key(&term);
                target.push(crate::session::TabEntry {
                    base_title,
                    title,
                    cwd,
                    custom_title,
                });
            }
        }
        data
    }

    fn save_session(&self, name: &str) -> bool {
        if !settings().get_bool("session_restore") {
            return true;
        }
        let mut data = self.build_session_data();
        if data.tabs_left.is_empty() {
            data.tabs_left = std::mem::take(&mut data.tabs_right);
            data.split_mode = "single".to_string();
        }
        if data.tabs_left.is_empty() {
            // Every tab was closed: there is nothing to restore. Writing an
            // empty session would be rejected on load anyway.
            crate::session::delete_session(name);
            *self.imp().last_saved_session.borrow_mut() = None;
            return true;
        }
        if self.imp().last_saved_session.borrow().as_ref() == Some(&data) {
            return true;
        }
        let saved = crate::session::save_session(name, &data);
        if saved {
            *self.imp().last_saved_session.borrow_mut() = Some(data);
        }
        saved
    }

    fn save_session_named(&self, name: &str) -> bool {
        let data = self.build_session_data();
        crate::session::save_session(name, &data)
    }

    fn restore_session(&self) -> bool {
        if !settings().get_bool("session_restore") {
            return false;
        }
        if let Some(data) = crate::session::load_session("last") {
            self.restore_window(data);
            return true;
        }
        false
    }

    fn restore_window(&self, data: crate::session::SessionData) {
        self.prepare_session_restore();
        let split = data.split_mode.clone();
        let tabs_left = data.tabs_left.clone();
        let tabs_right = data.tabs_right.clone();
        if !tabs_left.is_empty() {
            let first = &tabs_left[0];
            let term = self.add_new_tab(
                Some(&first.cwd),
                None,
                Some(&first.base_title),
                Some(&first.title),
                None,
            );
            self.remember_restored_custom_title(&term, first);
            for tab in &tabs_left[1..] {
                let term = self.add_new_tab(
                    Some(&tab.cwd),
                    None,
                    Some(&tab.base_title),
                    Some(&tab.title),
                    None,
                );
                self.remember_restored_custom_title(&term, tab);
            }
        }
        if !tabs_right.is_empty() && split != "single" {
            let nb2 = self.imp().notebook2.borrow().clone().unwrap();
            self.set_split(&split, false);
            for tab in &tabs_right {
                let term = self.add_new_tab(
                    Some(&tab.cwd),
                    Some(&nb2),
                    Some(&tab.base_title),
                    Some(&tab.title),
                    None,
                );
                self.remember_restored_custom_title(&term, tab);
            }
        } else if split == "single" {
            self.set_split("single", false);
        }
    }

    fn remember_restored_custom_title(&self, term: &TerminalBox, tab: &crate::session::TabEntry) {
        if tab.custom_title {
            self.imp()
                .tab_custom_titles
                .borrow_mut()
                .insert(term.clone(), tab.title.clone());
            self.set_tab_text(term, &tab.title);
        }
    }

    fn prepare_session_restore(&self) {
        *self.imp().restoring_session.borrow_mut() = true;
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in (0..notebook.n_pages()).rev() {
                if let Some(page) = notebook.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        term.terminate();
                    }
                    notebook.remove_page(Some(i));
                }
            }
        }
        self.imp().tab_labels.borrow_mut().clear();
        self.imp().tab_base_titles.borrow_mut().clear();
        self.imp().tab_custom_titles.borrow_mut().clear();
        self.imp().current_pages.borrow_mut().clear();
        *self.imp().split_mode.borrow_mut() = "single".to_string();
        nb2.hide();
        nb.set_show_tabs(settings().get_bool("show_tabs"));
        *self.imp().restoring_session.borrow_mut() = false;
    }

    fn load_session_named(&self, name: &str) {
        let Some(data) = crate::session::load_session(name) else {
            self.show_error("Could not load the session. See the application log for details.");
            return;
        };
        // Restoring replaces every open tab; don't kill running programs
        // without asking.
        if self.any_terminal_has_active_process() {
            let dialog = gtk::MessageDialog::new(
                Some(self),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Question,
                gtk::ButtonsType::YesNo,
                "Restoring this session closes all open tabs, including running programs.\n\nContinue?",
            );
            let resp = dialog.run();
            dialog.close();
            if resp != gtk::ResponseType::Yes {
                return;
            }
        }
        self.restore_window(data);
    }
}

impl TerminalWindow for MainWindow {
    fn new_tab_signal(&self) {
        MainWindow::new_tab_signal(self);
    }
    fn new_window_signal(&self) {
        MainWindow::new_window_signal(self);
    }
    fn close_tab_signal(&self, term: Option<&TerminalBox>) {
        MainWindow::close_tab_signal(self, term);
    }
    fn close_window_signal(&self) {
        MainWindow::close_window_signal(self);
    }
    fn set_title_dialog(&self) {
        MainWindow::set_title_dialog(self);
    }
    fn reset_terminal(&self) {
        MainWindow::reset_terminal(self);
    }
    fn reset_and_clear(&self) {
        MainWindow::reset_and_clear(self);
    }
    fn split_signal(&self, mode: &str) {
        MainWindow::split_signal(self, mode);
    }
    fn focus_other_pane_signal(&self) {
        MainWindow::focus_other_pane_signal(self);
    }
    fn toggle_split_zoom_signal(&self) {
        MainWindow::toggle_split_zoom_signal(self);
    }
    fn equalize_splits_signal(&self) {
        MainWindow::equalize_splits_signal(self);
    }
    fn set_tab_failure_signal(&self, term: &TerminalBox, failed: bool) {
        MainWindow::set_tab_failure_signal(self, term, failed);
    }
    fn clear_tab_failure_signal(&self, term: &TerminalBox) {
        MainWindow::clear_tab_failure_signal(self, term);
    }
    fn prev_tab_signal(&self) {
        MainWindow::prev_tab_signal(self);
    }
    fn next_tab_signal(&self) {
        MainWindow::next_tab_signal(self);
    }
    fn session_list_signal(&self) {
        MainWindow::session_list_signal(self);
    }
    fn open_command_tab(&self, cwd: &str, command: Vec<String>, title: &str) {
        let term = self.add_new_tab(Some(cwd), None, Some(title), Some(title), Some(&command));
        // Keep the tab after the pager exits so its output is still readable.
        term.set_hold(true);
    }
    fn rename_tab_signal(&self, term: &TerminalBox) {
        self.rename_tab_dialog(term);
    }
    fn detach_tab_signal(&self, term: &TerminalBox) {
        self.detach_tab(Some(term));
    }
    fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str) {
        MainWindow::set_tab_title_from_terminal(self, term, title);
    }
    fn refresh_cwd_display(&self, term: &TerminalBox) {
        let title = self.get_tab_text(term);
        let tooltip = self.tab_tooltip(&title, term);
        if let Some(tab) = self.imp().tab_labels.borrow().get(term) {
            tab.label.set_tooltip_text(Some(&tooltip));
        }
        if self.current_terminal().as_ref() == Some(term) {
            self.update_cwd_badge(term);
        }
    }

    fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]) {
        MainWindow::broadcast_feed(self, source, data);
    }

    fn note_feedback(&self, message: &str, is_error: bool) {
        show_note_feedback(
            &self.imp().info_bar,
            &self.imp().info_label,
            message,
            is_error,
        );
    }
    fn status_message(&self, message: &str, is_error: bool) {
        self.note_feedback(message, is_error);
    }
    fn note_window(&self) -> gtk::Window {
        self.clone().upcast()
    }
    fn note_terminal(&self) -> Option<TerminalBox> {
        self.current_terminal()
    }
    fn note_toggle_panel(&self) {
        let visible = self.notes_panel_visible();
        self.set_notes_panel_visible(!visible);
        if !visible {
            self.reload_notes_panel();
        }
    }
    fn note_refresh_panel_if_open(&self) {
        if self.notes_panel_visible() {
            self.reload_notes_panel();
        }
    }
    fn note_write(&self, text: String, filename: Option<String>, context: NoteContext) {
        let weak = crate::SendWeak::new(self);
        NotesManager::new().write_note_async(text, filename.as_deref(), context, move |result| {
            if let Some(w) = weak.upgrade() {
                match result {
                    Ok(path) => {
                        w.note_feedback(&format!("Saved to {}", path.display()), false);
                        w.note_refresh_panel_if_open();
                    }
                    Err(e) => w.note_feedback(&format!("Could not write note: {}", e), true),
                }
            }
        });
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Everything that determines the default window size: columns, rows, font
/// size and the two cell scale percentages.
type WindowSizeKey = (i64, i64, i64, i64, i64);

fn window_size_key_with(cols: i64, rows: i64, font_size: i64) -> WindowSizeKey {
    let s = settings();
    (
        cols,
        rows,
        font_size,
        (s.get_f64("cell_width_scale") * 100.0).round() as i64,
        (s.get_f64("cell_height_scale") * 100.0).round() as i64,
    )
}

/// Settings that determine the default window size.
fn window_size_key() -> WindowSizeKey {
    let s = settings();
    window_size_key_with(
        s.get_i64("terminal_columns"),
        s.get_i64("terminal_rows"),
        s.get_i64("font_size"),
    )
}

/// Approximate window size in pixels for a [`WindowSizeKey`]. The cell scales
/// grow the window instead of shrinking the grid, so a scaled terminal keeps
/// the configured rows and columns fully visible.
fn window_size_for((cols, rows, font_size, width_pct, height_pct): WindowSizeKey) -> (i32, i32) {
    let cw = (font_size as i32 * 6 / 10).max(5);
    let ch = (font_size as i32 * 145 / 100).max(10);
    let width = cols as i32 * cw * width_pct as i32 / 100 + 60;
    let height = rows as i32 * ch * height_pct as i32 / 100 + 120;
    (width, height)
}

/// Open a new main window inside the current process.
///
/// Spawning a second process re-executes the binary from disk; if the
/// executable was rebuilt or replaced in the meantime (a normal occurrence
/// while working on Oxterm), the new process can map a half-written file and
/// crash right after showing its window. Creating the window in-process avoids
/// that race and is also noticeably faster.
fn open_new_main_window(app: &gtk::Application) {
    let start_dir = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    let win = MainWindow::new(Some(app), start_dir, None, false, WindowOptions::default());
    app.add_window(&win);
    win.present();
}

fn spawn_new_process(new_window: bool) {
    // `/proc/self/exe` always points at the running image, even when the file
    // on disk has since been replaced (for example by a rebuild). Re-executing
    // that inode is safer than resolving the old path again.
    let current_exe = std::path::PathBuf::from("/proc/self/exe");
    let mut cmd = std::process::Command::new(&current_exe);
    if new_window {
        cmd.arg("--new-window");
    }
    if let Ok(child) = cmd.spawn() {
        crate::notes::reap_in_background(child);
    }
}

/// Debug-only scenarios used by `tests/gui_lifecycle.rs`.
///
/// The integration test starts the real binary with `OXTERM_TEST_SCENARIO`
/// set, so the scenario runs on the actual GTK main thread with the real
/// application event loop. Release builds do not contain this module.
#[cfg(debug_assertions)]
pub mod test_scenario {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    static FAILED: AtomicBool = AtomicBool::new(false);

    pub fn failed() -> bool {
        FAILED.load(Ordering::SeqCst)
    }

    pub fn schedule(app: &gtk::Application, win: &MainWindow, scenario: String) {
        let app = app.clone();
        let win = win.clone();
        let mut attempts = 0u32;
        glib::timeout_add_local(Duration::from_millis(50), move || {
            if win.total_tabs() == 0 {
                attempts += 1;
                if attempts < 100 {
                    return glib::ControlFlow::Continue;
                }
                finish(Err("the initial terminal tab was not created".into()), &app);
                return glib::ControlFlow::Break;
            }
            finish(run(&app, &win, &scenario), &app);
            glib::ControlFlow::Break
        });
    }

    fn finish(result: Result<(), String>, app: &gtk::Application) {
        match result {
            Ok(()) => eprintln!("OXTERM TEST SCENARIO OK"),
            Err(error) => {
                FAILED.store(true, Ordering::SeqCst);
                eprintln!("OXTERM TEST SCENARIO FAILED: {error}");
            }
        }
        close_all(app);
        app.quit();
    }

    fn close_all(app: &gtk::Application) {
        for window in app.windows() {
            if let Some(win) = window.downcast_ref::<MainWindow>() {
                win.shutdown();
            } else if let Some(win) = window.downcast_ref::<DetachedWindow>() {
                win.shutdown();
            }
        }
    }

    fn run(app: &gtk::Application, win: &MainWindow, scenario: &str) -> Result<(), String> {
        match scenario {
            "detach-last-tab" => detach_last_tab(app, win),
            "detach-multi-tab" => detach_multi_tab(app, win),
            "close-last-tab" => close_last_tab(app, win),
            "close-split-pane" => close_split_pane(app, win),
            "detach-from-split" => detach_from_split(app, win),
            "detach-local-view" => detach_local_view(app, win),
            "new-window" => new_window(app, win),
            "split-zoom" => split_zoom(win),
            "theme-switch" => theme_switch(win),
            "tab-failure" => tab_failure(win),
            "preferences" => preferences(win),
            other => Err(format!("unknown test scenario: {other}")),
        }
    }

    /// Building the preferences dialog exercises every page, which is where a
    /// settings widget with a bad type or a missing row would panic. Close it
    /// from a timeout so the modal `run()` loop returns.
    fn preferences(win: &MainWindow) -> Result<(), String> {
        glib::timeout_add_local(Duration::from_millis(600), move || {
            // The dialog is not in `app.windows()`: GtkDialog toplevels are
            // only registered with the application when added explicitly.
            for window in gtk::Window::list_toplevels() {
                if let Ok(dialog) = window.downcast::<gtk::Dialog>() {
                    if dialog
                        .title()
                        .map(|title| title.starts_with("Preferences"))
                        .unwrap_or(false)
                    {
                        dialog.response(gtk::ResponseType::Close);
                    }
                }
            }
            glib::ControlFlow::Break
        });
        win.open_settings();
        Ok(())
    }

    fn notebook(win: &MainWindow, right: bool) -> Result<gtk::Notebook, String> {
        if right {
            win.imp()
                .notebook2
                .borrow()
                .clone()
                .ok_or_else(|| "right notebook is missing".to_string())
        } else {
            win.imp()
                .notebook
                .borrow()
                .clone()
                .ok_or_else(|| "left notebook is missing".to_string())
        }
    }

    fn term_at(notebook: &gtk::Notebook, index: u32) -> Result<TerminalBox, String> {
        notebook
            .nth_page(Some(index))
            .ok_or_else(|| format!("no notebook page at index {index}"))?
            .downcast::<TerminalBox>()
            .map_err(|_| format!("page at index {index} is not a terminal"))
    }

    fn add_sleep_tab(
        win: &MainWindow,
        target: Option<&gtk::Notebook>,
        title: &str,
    ) -> Result<TerminalBox, String> {
        let command = vec!["sleep".to_string(), "30".to_string()];
        if win.total_tabs() >= 100 {
            return Err("too many tabs".to_string());
        }
        Ok(win.add_new_tab(
            Some("/tmp"),
            target,
            Some(title),
            Some(title),
            Some(&command),
        ))
    }

    fn split_zoom(win: &MainWindow) -> Result<(), String> {
        win.set_split("vertical", true);
        let paned = win
            .imp()
            .paned
            .borrow()
            .clone()
            .ok_or_else(|| "the split paned is missing".to_string())?;
        let size = paned.allocated_width();
        if size < 50 {
            return Err(format!("window is too small for the split test: {size}px"));
        }
        if !win.imp().split_zoomed.get() {
            // The new pane (right) has focus, so zooming must hide the left.
            win.toggle_split_zoom_signal();
        }
        if !win.imp().split_zoomed.get() {
            return Err("zoom did not engage".to_string());
        }
        // The position may still be the requested (huge) value until the next
        // allocation; 0 and >= size both mean one pane is fully hidden.
        let zoomed_position = paned.position();
        if zoomed_position > 1 && zoomed_position < size {
            return Err(format!(
                "zoomed pane did not fill the window: position {zoomed_position}/{size}"
            ));
        }
        win.toggle_split_zoom_signal();
        if win.imp().split_zoomed.get() {
            return Err("zoom did not disengage".to_string());
        }
        let restored = paned.position();
        if restored <= 0 || restored >= size {
            return Err(format!("splitter was not restored: {restored}/{size}"));
        }
        win.equalize_splits_signal();
        let middle = size / 2;
        if (paned.position() - middle).abs() > 4 {
            return Err(format!(
                "equalize left position {} instead of {middle}",
                paned.position()
            ));
        }
        win.set_split("single", false);
        Ok(())
    }

    fn tab_failure(win: &MainWindow) -> Result<(), String> {
        let background = win
            .current_terminal()
            .ok_or_else(|| "no current terminal".to_string())?;
        // A second tab makes the first one a background tab.
        let foreground = add_sleep_tab(win, None, "foreground")?;
        win.set_tab_failure_signal(&background, true);
        let visible = win
            .imp()
            .tab_labels
            .borrow()
            .get(&background)
            .map(|tab| tab.indicator.is_visible())
            .unwrap_or(false);
        if !visible {
            return Err("the failed-command marker is not visible on a background tab".into());
        }
        win.clear_tab_failure_signal(&background);
        let still_visible = win
            .imp()
            .tab_labels
            .borrow()
            .get(&background)
            .map(|tab| tab.indicator.is_visible())
            .unwrap_or(false);
        if still_visible {
            return Err("the failed-command marker was not cleared".into());
        }
        // A successful command clears a previous failure marker.
        win.set_tab_failure_signal(&background, true);
        win.set_tab_failure_signal(&background, false);
        if win
            .imp()
            .tab_labels
            .borrow()
            .get(&background)
            .map(|tab| tab.indicator.is_visible())
            .unwrap_or(true)
        {
            return Err("success did not clear the marker".into());
        }
        let _ = foreground;
        Ok(())
    }

    fn theme_switch(win: &MainWindow) -> Result<(), String> {
        let term = win
            .current_terminal()
            .ok_or_else(|| "no current terminal".to_string())?;
        let settings = settings();
        let auto_before = settings.get_bool("auto_theme");
        let light_before = settings.get_str("theme_light");
        let dark_before = settings.get_str("theme_dark");
        let gtk_settings =
            gtk::Settings::default().ok_or_else(|| "GTK settings are unavailable".to_string())?;
        let dark_before_gtk = gtk_settings.is_gtk_application_prefer_dark_theme();

        settings.set_bool("auto_theme", true);
        settings.set_str("theme_light", "Light");
        settings.set_str("theme_dark", "Dracula");

        gtk_settings.set_gtk_application_prefer_dark_theme(true);
        settings.notify_changed();
        let dark_bg = term.vte().color_background_for_draw();
        // Dracula background is #282a36.
        if (dark_bg.red() - 0.157).abs() > 0.03 || (dark_bg.blue() - 0.212).abs() > 0.03 {
            settings.set_bool("auto_theme", auto_before);
            settings.set_str("theme_light", &light_before);
            settings.set_str("theme_dark", &dark_before);
            gtk_settings.set_gtk_application_prefer_dark_theme(dark_before_gtk);
            return Err(format!(
                "dark theme not applied: {:.3},{:.3},{:.3}",
                dark_bg.red(),
                dark_bg.green(),
                dark_bg.blue()
            ));
        }

        gtk_settings.set_gtk_application_prefer_dark_theme(false);
        settings.notify_changed();
        let light_bg = term.vte().color_background_for_draw();
        if light_bg.red() < 0.9 || light_bg.blue() < 0.9 {
            settings.set_bool("auto_theme", auto_before);
            settings.set_str("theme_light", &light_before);
            settings.set_str("theme_dark", &dark_before);
            gtk_settings.set_gtk_application_prefer_dark_theme(dark_before_gtk);
            return Err(format!(
                "light theme not applied: {:.3},{:.3},{:.3}",
                light_bg.red(),
                light_bg.green(),
                light_bg.blue()
            ));
        }

        settings.set_bool("auto_theme", auto_before);
        settings.set_str("theme_light", &light_before);
        settings.set_str("theme_dark", &dark_before);
        gtk_settings.set_gtk_application_prefer_dark_theme(dark_before_gtk);
        settings.notify_changed();
        Ok(())
    }

    fn detach_last_tab(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        let term = win
            .current_terminal()
            .ok_or_else(|| "no current terminal".to_string())?;
        win.detach_tab(Some(&term));

        let windows = app.windows();
        if windows.len() != 1 {
            return Err(format!(
                "detaching the last tab left {} window(s), expected 1",
                windows.len()
            ));
        }
        let detached = windows[0]
            .downcast_ref::<DetachedWindow>()
            .ok_or_else(|| "the remaining window is not detached".to_string())?;
        let attached = detached.imp().terminal.borrow().clone();
        if attached.as_ref() != Some(&term) {
            return Err("the detached window does not own the moved tab".to_string());
        }
        if !term.has_active_process() {
            return Err("the shell did not survive the detach".to_string());
        }
        Ok(())
    }

    fn detach_multi_tab(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        let left = notebook(win, false)?;
        let first = term_at(&left, 0)?;
        add_sleep_tab(win, None, "second")?;
        win.detach_tab(Some(&first));

        let windows = app.windows();
        if windows.len() != 2 {
            return Err(format!(
                "expected the source and detached windows, found {}",
                windows.len()
            ));
        }
        if windows.iter().filter(|w| w.is::<MainWindow>()).count() != 1 {
            return Err("the source MainWindow disappeared".to_string());
        }
        if windows.iter().filter(|w| w.is::<DetachedWindow>()).count() != 1 {
            return Err("the DetachedWindow was not registered".to_string());
        }
        if win.total_tabs() != 1 {
            return Err(format!(
                "source window should keep one tab, has {}",
                win.total_tabs()
            ));
        }
        Ok(())
    }

    fn close_last_tab(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        settings()
            .set("confirm_close", serde_json::json!(false))
            .map_err(|e| format!("disable confirm_close: {e}"))?;
        let term = win
            .current_terminal()
            .ok_or_else(|| "no current terminal".to_string())?;
        win.close_tab(Some(&term));
        if !app.windows().is_empty() {
            return Err("closing the last tab did not close the window".to_string());
        }
        Ok(())
    }

    fn close_split_pane(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        let left = notebook(win, false)?;
        let first = term_at(&left, 0)?;
        win.set_split("vertical", false);
        let right = notebook(win, true)?;
        add_sleep_tab(win, Some(&right), "right")?;

        win.close_tab(Some(&first));

        if app.windows().len() != 1 {
            return Err("closing one split pane closed the whole window".to_string());
        }
        if win.total_tabs() != 1 {
            return Err(format!(
                "expected one surviving tab, found {}",
                win.total_tabs()
            ));
        }
        if left.n_pages() != 1 || right.n_pages() != 0 {
            return Err(format!(
                "tabs were not moved out of the split: left={}, right={}",
                left.n_pages(),
                right.n_pages()
            ));
        }
        if *win.imp().split_mode.borrow() != "single" {
            return Err("the split was not collapsed".to_string());
        }
        Ok(())
    }

    fn detach_from_split(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        win.set_split("vertical", false);
        let right = notebook(win, true)?;
        let term = add_sleep_tab(win, Some(&right), "right")?;
        win.detach_tab(Some(&term));

        if app.windows().len() != 2 {
            return Err(format!(
                "expected the source and detached windows, found {}",
                app.windows().len()
            ));
        }
        if win.total_tabs() != 1 {
            return Err(format!(
                "the source window should keep one tab, has {}",
                win.total_tabs()
            ));
        }
        if right.n_pages() != 0 {
            return Err("the detached tab is still in the right notebook".to_string());
        }
        if *win.imp().split_mode.borrow() != "single" {
            return Err("the now-empty split was not collapsed".to_string());
        }
        Ok(())
    }

    fn detach_local_view(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        let term = win
            .current_terminal()
            .ok_or_else(|| "no current terminal".to_string())?;
        term.detach_local_terminal();
        term.reattach_local();
        if app.windows().len() != 1 {
            return Err(
                "detaching/reattaching the local view changed the window count".to_string(),
            );
        }
        if !term.has_active_process() {
            return Err("the shell did not survive detaching the local view".to_string());
        }
        Ok(())
    }

    fn new_window(app: &gtk::Application, win: &MainWindow) -> Result<(), String> {
        win.on_new_window();
        if app.windows().len() != 2 {
            return Err(format!(
                "new window was not registered, found {} window(s)",
                app.windows().len()
            ));
        }
        if app
            .windows()
            .iter()
            .filter(|w| w.is::<MainWindow>())
            .count()
            != 2
        {
            return Err("the new window is not a MainWindow".to_string());
        }
        Ok(())
    }
}
