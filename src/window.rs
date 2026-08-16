use std::cell::RefCell;

use glib::prelude::*;
use glib::translate::*;
use glib::subclass::prelude::*;
use gtk::gdk;
#[allow(non_snake_case)]
mod K {
    pub use gdk::keys::constants::*;
}
use gtk::prelude::*;

use zoha_vte::traits::TerminalExt;

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
    "UTF-8", "ISO-8859-1", "ISO-8859-15", "UTF-16", "UTF-16BE", "UTF-16LE", "CP1252", "CP850",
    "ASCII", "KOI8-R", "Shift_JIS", "EUC-JP", "GBK",
];

pub const EUPL_LICENSE_TEXT: &str = "Licensed under the European Union Public Licence (EUPL) v. 1.2.\n\nCopyright (c) 2026 Andres Zanzani.\n\nThe complete licence text is distributed in the LICENSE file and is available at:\nhttps://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12\n";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn detect_file_manager() -> Option<String> {
    let s = settings();
    let fm = s.get_str("file_manager");
    if !fm.is_empty() && crate::notes::which(&fm).is_some() {
        return Some(fm);
    }
    for candidate in [
        "nemo", "thunar", "dolphin", "nautilus", "pcmanfm", "caja", "nemo-desktop", "spacefm",
    ] {
        if crate::notes::which(candidate).is_some() {
            return Some(candidate.to_string());
        }
    }
    None
}


/// Interface implemented by both window types so a terminal can drive its
/// host window without knowing which concrete kind it is.
pub trait TerminalWindow {
    fn new_tab_signal(&self);
    fn close_tab_signal(&self, term: Option<&TerminalBox>);
    fn close_window_signal(&self);
    fn set_title_dialog(&self);
    fn reset_terminal(&self);
    fn reset_and_clear(&self);
    fn split_signal(&self, mode: &str);
    fn focus_other_pane_signal(&self);
    fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str);
    fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]);
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
        pub stats_source_id: RefCell<Option<glib::SourceId>>,
        pub remote_stats_pending: RefCell<bool>,
        pub remote_stats_generation: RefCell<u64>,
        pub closing: RefCell<bool>,
        pub encoding_actions: RefCell<Vec<(gtk::CheckMenuItem, String)>>,
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
    pub fn new(terminal: &TerminalBox, title: &str) -> DetachedWindow {
        let this: DetachedWindow = glib::Object::new();
        this.update_title(&format!("TPGK - {}", title));
        *this.imp().terminal.borrow_mut() = Some(terminal.clone());
        this.apply_window_visuals();
        this.apply_window_size();

        let stats_sys_label = gtk::Label::new(Some(""));
        stats_sys_label.set_halign(gtk::Align::Start);
        stats_sys_label.style_context().add_class("tpgk-stats-label");
        let stats_self_label = gtk::Label::new(Some(""));
        stats_self_label.set_halign(gtk::Align::End);
        stats_self_label.style_context().add_class("tpgk-stats-label");
        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        stats_box.pack_start(&stats_sys_label, true, true, 0);
        stats_box.pack_end(&stats_self_label, false, false, 0);
        stats_box.set_no_show_all(true);
        *this.imp().stats_sys_label.borrow_mut() = Some(stats_sys_label);
        *this.imp().stats_self_label.borrow_mut() = Some(stats_self_label);
        *this.imp().stats_box.borrow_mut() = Some(stats_box.clone());

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
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

        let h1 = settings().connect_changed({
            let w = this.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_window_size();
                }
            }
        });
        let h2 = settings().connect_changed({
            let w = this.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_window_visuals();
                }
            }
        });
        this.imp().settings_handlers.borrow_mut().push(h1);
        this.imp().settings_handlers.borrow_mut().push(h2);

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
        let s = settings();
        let cols = s.get_i64("terminal_columns") as i32;
        let rows = s.get_i64("terminal_rows") as i32;
        let font_size = s.get_i64("font_size");
        let cw = (font_size as i32 * 6 / 10).max(5);
        let ch = (font_size as i32 * 145 / 100).max(10);
        self.resize(cols * cw + 60, rows * ch + 120);
    }

    fn apply_window_visuals(&self) {
        let s = settings();
        let opacity = (s.get_f64("opacity") * 100.0).round() / 100.0;
        self.set_opacity(opacity);
        if s.get_bool("enable_transparency") {
            if let Some(screen) = gtk::prelude::WidgetExt::screen(self) {
                if let Some(visual) = screen.rgba_visual() {
                    self.set_app_paintable(true);
                    self.set_visual(Some(&visual));
                }
            }
        } else {
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
        header.set_title(Some(&format!("TPGK - {}", title)));
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
        menubar.style_context().add_class("tpgk-menu-row");
        let mut buttons = Vec::new();
        for label in ["File", "Edit", "View", "Terminal", "Help"] {
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
                    item.add_accelerator("activate", &accel_group, raw_key, mods, gtk::AccelFlags::VISIBLE);
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
            Some("Open a new TPGK terminal window"),
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
            Some("Open the TPGK settings dialog"),
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

        let help_menu = gtk::Menu::new();
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
            Some("Show information about TPGK"),
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
        spawn_new_process(false);
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
            &[("Cancel", gtk::ResponseType::Cancel), ("Set", gtk::ResponseType::Ok)],
        );
        let entry = gtk::Entry::new();
        let current = self.title().unwrap_or_default().replace("TPGK - ", "");
        entry.set_text(&current);
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let title = entry.text().trim().to_string();
            if !title.is_empty() {
                self.update_title(&format!("TPGK - {}", title));
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
        if settings().get_bool("confirm_close") {
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
        *self.imp().closing.borrow_mut() = true;
        for handler in std::mem::take(&mut *self.imp().settings_handlers.borrow_mut()) {
            settings().disconnect_changed(handler);
        }
        if let Some(t) = self.imp().terminal.borrow().clone() {
            t.terminate();
        }
        unsafe { self.destroy() };
        glib::Propagation::Stop
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
                w.refresh_stats();
            }
            glib::ControlFlow::Continue
        });
        *self.imp().stats_source_id.borrow_mut() = Some(source);
    }

    fn stop_stats_timer(&self) {
        if let Some(id) = self.imp().stats_source_id.borrow_mut().take() {
            id.remove();
        }
    }

    fn refresh_stats(&self) {
        let Some(term) = self.imp().terminal.borrow().clone() else {
            return;
        };
        if term.is_ssh_client() {
            if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                label.set_text(&crate::system_stats::ssh_placeholder());
            }
            if !*self.imp().remote_stats_pending.borrow() {
                *self.imp().remote_stats_pending.borrow_mut() = true;
                *self.imp().remote_stats_generation.borrow_mut() += 1;
                let gen = *self.imp().remote_stats_generation.borrow();
                let weak = crate::SendWeak::new(self);
                let term_ptr = term.as_ptr() as usize;
                std::thread::spawn(move || {
                    let obj: glib::Object = unsafe {
                        glib::Object::from_glib_none(term_ptr as *mut glib::gobject_ffi::GObject)
                    };
                    let term: TerminalBox = unsafe { obj.unsafe_cast() };
                    let stats = term.get_remote_stats();
                    glib::MainContext::default().invoke(move || {
                        if let Some(w) = weak.upgrade() {
                            if gen == *w.imp().remote_stats_generation.borrow() {
                                *w.imp().remote_stats_pending.borrow_mut() = false;
                                if let Some(label) = w.imp().stats_sys_label.borrow().clone() {
                                    if stats.is_empty() {
                                        label.set_text(&crate::system_stats::ssh_placeholder());
                                    } else {
                                        label.set_text(&stats);
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
            let osc = term.get_osc133_stats();
            if osc.is_empty() {
                crate::system_stats::collect(false)
            } else {
                osc
            }
        };
        if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
            label.set_text(&stats);
        }
        if let Some(label) = self.imp().stats_self_label.borrow().clone() {
            label.set_text(&crate::system_stats::collect_self());
        }
    }
}

impl TerminalWindow for DetachedWindow {
    fn new_tab_signal(&self) {
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
    fn set_tab_title_from_terminal(&self, _term: &TerminalBox, title: &str) {
        self.update_title(&format!("TPGK - {}", title));
    }
    fn broadcast_feed(&self, _source: &TerminalBox, _data: &[u8]) {}
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
        pub stats_source_id: RefCell<Option<glib::SourceId>>,
        pub remote_stats_pending: RefCell<bool>,
        pub remote_stats_generation: RefCell<u64>,
        pub split_mode: RefCell<String>,
        pub tab_base_titles: RefCell<std::collections::HashMap<TerminalBox, String>>,
        pub tab_labels: RefCell<std::collections::HashMap<TerminalBox, (gtk::EventBox, gtk::Label)>>,
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
        pub splitting: RefCell<bool>,
        pub closing: RefCell<bool>,
        pub skip_close_confirm: RefCell<bool>,
        pub restoring_session: RefCell<bool>,
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

impl MainWindow {
    pub fn new(
        app: Option<&gtk::Application>,
        start_dir: Option<String>,
        command: Option<Vec<String>>,
        restore_session: bool,
    ) -> MainWindow {
        let this: MainWindow = glib::Object::new();
        if let Some(app) = app {
            this.set_application(Some(app));
        }
        this.init(start_dir, command, restore_session);
        this
    }

    fn init(&self, start_dir: Option<String>, command: Option<Vec<String>>, restore_session: bool) {
        self.set_title("TPGK Terminal");
        let s = settings();
        let cols = s.get_i64("terminal_columns") as i32;
        let rows = s.get_i64("terminal_rows") as i32;
        let font_size = s.get_i64("font_size");
        let cw = (font_size as i32 * 6 / 10).max(5);
        let ch = (font_size as i32 * 145 / 100).max(10);
        self.set_default_size(cols * cw + 60, rows * ch + 120);

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

        self.build_headerbar();

        let stats_sys_label = gtk::Label::new(Some(""));
        stats_sys_label.set_halign(gtk::Align::Start);
        stats_sys_label.style_context().add_class("tpgk-stats-label");
        let stats_self_label = gtk::Label::new(Some(""));
        stats_self_label.set_halign(gtk::Align::End);
        stats_self_label.style_context().add_class("tpgk-stats-label");
        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        stats_box.pack_start(&stats_sys_label, true, true, 0);
        stats_box.pack_end(&stats_self_label, false, false, 0);
        stats_box.set_no_show_all(true);
        *self.imp().stats_sys_label.borrow_mut() = Some(stats_sys_label);
        *self.imp().stats_self_label.borrow_mut() = Some(stats_self_label);
        *self.imp().stats_box.borrow_mut() = Some(stats_box.clone());

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let menubar = self.imp().menubar.borrow().clone().unwrap();
        vbox.pack_start(&menubar, false, false, 0);
        vbox.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        vbox.pack_start(&paned, true, true, 0);
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
        let weak = crate::SendWeak::new(self);
        glib::idle_add_local(move || {
            if let Some(w) = weak.upgrade() {
                w.populate_menus();
            }
            glib::ControlFlow::Break
        });

        let toolbar = self.imp().toolbar.borrow().clone().unwrap();
        toolbar.set_visible(settings().get_bool("show_toolbar"));
        menubar.set_visible(settings().get_bool("show_menubar"));
        self.apply_stats_visibility();
        self.apply_tab_colors();

        let h1 = settings().connect_changed({
            let w = self.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_tab_colors();
                }
            }
        });
        let h2 = settings().connect_changed({
            let w = self.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_window_size();
                }
            }
        });
        let h3 = settings().connect_changed({
            let w = self.downgrade();
            move || {
                if let Some(w) = w.upgrade() {
                    w.apply_window_visuals();
                }
            }
        });
        self.imp().settings_handlers.borrow_mut().push(h1);
        self.imp().settings_handlers.borrow_mut().push(h2);
        self.imp().settings_handlers.borrow_mut().push(h3);

        let weak = crate::SendWeak::new(self);
        let sd = start_dir.clone();
        let cmd = command.clone();
        glib::idle_add_local(move || {
            if let Some(w) = weak.upgrade() {
                w.fix_paned_position();
                w.initialize_terminal(sd.as_deref(), &cmd, restore_session);
            }
            glib::ControlFlow::Break
        });
    }

    fn initialize_terminal(&self, start_dir: Option<&str>, command: &Option<Vec<String>>, restore: bool) {
        if restore && self.restore_session() {
            return;
        }
        self.add_new_tab(
            start_dir,
            None,
            None,
            None,
            command.as_ref(),
        );
    }

    fn fix_paned_position(&self) {
        if let Some(paned) = self.imp().paned.borrow().clone() {
            paned.set_position(99999);
        }
    }

    fn apply_window_size(&self) {
        let s = settings();
        let cols = s.get_i64("terminal_columns") as i32;
        let rows = s.get_i64("terminal_rows") as i32;
        let font_size = s.get_i64("font_size");
        let cw = (font_size as i32 * 6 / 10).max(5);
        let ch = (font_size as i32 * 145 / 100).max(10);
        let w = cols * cw + 60;
        let h = rows * ch + 120;
        self.resize(w, h);
    }

    fn apply_window_visuals(&self) {
        let s = settings();
        let opacity = (s.get_f64("opacity") * 100.0).round() / 100.0;
        self.set_opacity(opacity);
        if s.get_bool("enable_transparency") {
            if let Some(screen) = gtk::prelude::WidgetExt::screen(self) {
                if let Some(visual) = screen.rgba_visual() {
                    self.set_app_paintable(true);
                    self.set_visual(Some(&visual));
                }
            }
        } else {
            self.set_app_paintable(false);
        }
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

    fn get_tab_text(&self, term: &TerminalBox) -> String {
        if let Some((_box, lbl)) = self.imp().tab_labels.borrow().get(term) {
            return lbl.text().to_string();
        }
        String::new()
    }

    fn set_tab_text(&self, term: &TerminalBox, text: &str) {
        if let Some((_box, lbl)) = self.imp().tab_labels.borrow().get(term) {
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
            let cur = *self.imp().current_pages.borrow().get(&nb).unwrap_or(&nb.current_page().unwrap_or(0));
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
            let cur = *self.imp().current_pages.borrow().get(&nb).unwrap_or(&nb.current_page().unwrap_or(0));
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

    fn recolor_tab_label(&self, term: &TerminalBox, is_active: bool) {
        let s = settings();
        let color = if is_active {
            s.get_str("tab_active_title_color")
        } else {
            s.get_str("tab_title_color")
        };
        if let Some((_box, lbl)) = self.imp().tab_labels.borrow().get(term) {
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
        header.set_title(Some("TPGK Terminal"));
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
        menubar.style_context().add_class("tpgk-menu-row");
        let mut buttons = Vec::new();
        for label in ["File", "Edit", "View", "Terminal", "Tabs", "Help"] {
            let btn = gtk::MenuButton::new();
            btn.set_label(label);
            btn.set_relief(gtk::ReliefStyle::None);
            menubar.pack_start(&btn, false, false, 0);
            buttons.push(btn);
        }
        *self.imp().menubar.borrow_mut() = Some(menubar);
        *self.imp().menu_buttons.borrow_mut() = buttons;

        let tab_list_btn = self.make_tab_list_button();
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
        let menu = gtk::Menu::new();
        menu.style_context().add_class("tpgk-tab-menu");
        menu.set_reserve_toggle_size(false);
        btn.set_popup(Some(&menu));
        let weak = crate::SendWeak::new(self);
        btn.connect_toggled(move |b| {
            if b.is_active() {
                if let Some(w) = weak.upgrade() {
                    w.populate_tab_menu(&menu);
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
                    item.add_accelerator("activate", &accel_group, raw_key, mods, gtk::AccelFlags::VISIBLE);
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
            Some("Open a new TPGK terminal window"),
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
            Some("Quit TPGK (close all windows)"),
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
            Some("Open the TPGK settings dialog"),
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
            "Set Title...",
            Box::new(move || {
                if let Some(w) = weak.upgrade() {
                    w.set_title_dialog();
                }
            }),
            Some("<Primary><Shift>S"),
            Some("Set a custom title for the current tab"),
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

        // Help
        let help_menu = gtk::Menu::new();
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
            Some("Show information about TPGK"),
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

    // ── Split modes ──────────────────────────────────────────

    fn set_split(&self, mode: &str, create_tab: bool) {
        if *self.imp().splitting.borrow() {
            return;
        }
        *self.imp().splitting.borrow_mut() = true;
        *self.imp().split_mode.borrow_mut() = mode.to_string();
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let paned = self.imp().paned.borrow().clone().unwrap();
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
            paned.set_orientation(gtk::Orientation::Horizontal);
            if nb2.n_pages() == 0 && create_tab {
                nb2.show();
                self.add_new_tab(None, Some(&nb2), None, None, None);
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
            paned.set_orientation(gtk::Orientation::Vertical);
            if nb2.n_pages() == 0 && create_tab {
                nb2.show();
                self.add_new_tab(None, Some(&nb2), None, None, None);
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
        self.update_tabs_menu();
        *self.imp().splitting.borrow_mut() = false;
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
        src.remove_page(Some(idx));
        let (lbl_box, lbl) = self.make_tab_label(&title, &term);
        self.imp()
            .tab_labels
            .borrow_mut()
            .insert(term.clone(), (lbl_box.clone(), lbl));
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
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        let focused = self.focused_notebook();
        let target = if focused == nb { nb } else { nb2 };
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
    ) {
        let term = TerminalBox::new(self);
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
        let (lbl_box, lbl) = self.make_tab_label(display_title.unwrap_or(&base), &term);
        self.imp()
            .tab_labels
            .borrow_mut()
            .insert(term.clone(), (lbl_box.clone(), lbl));
        let idx = nb.append_page(&term, Some(&lbl_box));
        nb.set_tab_reorderable(&term, true);
        nb.set_show_tabs(true);
        nb.show_all();
        nb.set_current_page(Some(idx));
        self.update_tabs_menu();

        term.launch(cwd, command);
        term.show_all();
        let weak = crate::SendWeak::new(&term);
        glib::idle_add_local(move || {
            if let Some(t) = weak.upgrade() {
                t.vte().grab_focus();
            }
            glib::ControlFlow::Break
        });
    }

    fn make_tab_label(&self, name: &str, term: &TerminalBox) -> (gtk::EventBox, gtk::Label) {
        let eb = gtk::EventBox::new();
        let term2 = term.clone();
        let weak = crate::SendWeak::new(self);
        eb.connect_button_press_event(move |_w, ev| {
            if let Some(win) = weak.upgrade() {
                if ev.button() == 3 {
                    win.on_tab_button_press(&term2);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let lbl = gtk::Label::new(Some(name));
        let btn = gtk::Button::new();
        btn.set_relief(gtk::ReliefStyle::None);
        btn.set_focus_on_click(false);
        btn.set_tooltip_text(Some("Close tab"));
        btn.add(
            &gtk::Image::from_icon_name(Some("window-close-symbolic"), gtk::IconSize::Menu),
        );
        let term3 = term.clone();
        let weak = crate::SendWeak::new(self);
        btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.close_tab(Some(&term3));
            }
        });
        box_.pack_start(&lbl, true, true, 0);
        box_.pack_start(&btn, false, false, 0);
        eb.add(&box_);
        eb.show_all();
        (eb, lbl)
    }

    fn on_tab_button_press(&self, term: &TerminalBox) {
        let menu = gtk::Menu::new();
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
        let item_copy_notes = gtk::MenuItem::with_label("Copy All to Notes");
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
        menu.popup_at_pointer(None);
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
            nb.remove_page(Some(idx));
            self.update_tabs_menu();
        }
    }

    fn on_page_removed(&self) {
        if *self.imp().restoring_session.borrow() {
            return;
        }
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        if nb.n_pages() == 0 && nb2.n_pages() == 0 {
            self.on_close(self);
        } else if nb2.n_pages() == 0 && *self.imp().split_mode.borrow() != "single" {
            self.set_split("single", false);
        }
    }

    pub fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str) {
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
            self.update_title(&format!("TPGK - {}", title));
        }
    }

    fn update_tabs_menu(&self) {
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
            let cur = *self.imp().current_pages.borrow().get(notebook).unwrap_or(&notebook.current_page().unwrap_or(0));
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

    fn on_switch_tab(&self, page: &gtk::Widget, page_num: u32) {
        let nb = page.parent().and_then(|p| p.downcast::<gtk::Notebook>().ok());
        if let Some(nb) = nb {
            self.imp().current_pages.borrow_mut().insert(nb, page_num as u32);
        }
        if let Ok(term) = page.clone().downcast::<TerminalBox>() {
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
        spawn_new_process(true);
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
        let dialog = gtk::Dialog::with_buttons(
            Some("Set Tab Title"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Cancel", gtk::ResponseType::Cancel), ("Set", gtk::ResponseType::Ok)],
        );
        let entry = gtk::Entry::new();
        if let Some(term) = self.current_terminal() {
            entry.set_text(&self.get_tab_text(&term));
        }
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let title = entry.text().trim().to_string();
            if !title.is_empty() {
                self.update_title(&format!("TPGK - {}", title));
                if let Some(term) = self.current_terminal() {
                    self.set_tab_text(&term, &title);
                    self.imp()
                        .tab_base_titles
                        .borrow_mut()
                        .insert(term, title);
                }
            }
        }
        dialog.close();
    }

    fn save_profile_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Save Profile"),
            Some(self),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Ok)],
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
                        self.show_error("Could not save the profile. See the application log for details.");
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
            &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Ok)],
        );
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Session name..."));
        let now = crate::logging::utc_iso_now();
        entry.set_text(&now[..10]);
        dialog.content_area().pack_start(&entry, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let name = entry.text().trim().to_string();
            if !name.is_empty() {
                if !self.save_session_named(&name) {
                    self.show_error("Could not save the session. See the application log for details.");
                }
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
        nb.remove_page(Some(idx));
        self.imp().tab_labels.borrow_mut().remove(&term);
        self.imp().tab_base_titles.borrow_mut().remove(&term);
        self.update_tabs_menu();

        let new_win = DetachedWindow::new(&term, &title);
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
        let max_lines = settings().get_i64("scrollback_lines");
        let bound = if max_lines <= 0 { 1_000_000 } else { max_lines };
        let (text, _) = vte.text_range_format(zoha_vte::Format::Text, 0, 0, bound, 0);
        let text = text.map(|t| t.to_string()).unwrap_or_default();
        if text.is_empty() {
            return;
        }
        let notes = crate::notes::NotesManager::new();
        match notes.write_note(&text, None) {
            Ok(path) => {
                vte.feed(
                    format!(
                        "\r\n\x1b[32m+ Added all text to note: {}\x1b[0m\r\n",
                        path.to_string_lossy()
                    )
                    .as_bytes(),
                );
                vte.feed_child(b"\r");
            }
            Err(e) => {
                vte.feed(format!("\r\n\x1b[31mCould not write note: {}\x1b[0m\r\n", e).as_bytes());
            }
        }
    }

    fn show_about(&self) {
        crate::settings_dialog::show_about_dialog(Some(self.upcast_ref::<gtk::Window>()));
    }

    // ── Window signals ───────────────────────────────────────

    fn on_close(&self, _widget: &MainWindow) -> glib::Propagation {
        if *self.imp().closing.borrow() {
            return glib::Propagation::Stop;
        }
        if settings().get_bool("confirm_close") && !*self.imp().skip_close_confirm.borrow() {
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

        let app = self.application();
        let windows = app.map(|a| a.windows().len()).unwrap_or(1);
        if windows <= 1 {
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

    fn on_window_key(&self, _widget: &gtk::ApplicationWindow, event: &gdk::EventKey) -> glib::Propagation {
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

    pub fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]) {
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for notebook in [&nb, &nb2] {
            for i in 0..notebook.n_pages() {
                if let Some(page) = notebook.nth_page(Some(i)) {
                    if let Ok(term) = page.downcast::<TerminalBox>() {
                        if term != *source {
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
                w.refresh_stats();
            }
            glib::ControlFlow::Continue
        });
        *self.imp().stats_source_id.borrow_mut() = Some(source);
    }

    fn stop_stats_timer(&self) {
        if let Some(id) = self.imp().stats_source_id.borrow_mut().take() {
            id.remove();
        }
    }

    fn refresh_stats(&self) {
        let Some(term) = self.current_terminal() else {
            return;
        };
        if term.is_ssh_client() {
            if let Some(label) = self.imp().stats_sys_label.borrow().clone() {
                label.set_text(&crate::system_stats::ssh_placeholder());
            }
            if !*self.imp().remote_stats_pending.borrow() {
                *self.imp().remote_stats_pending.borrow_mut() = true;
                *self.imp().remote_stats_generation.borrow_mut() += 1;
                let gen = *self.imp().remote_stats_generation.borrow();
                let weak = crate::SendWeak::new(self);
                let term_ptr = term.as_ptr() as usize;
                std::thread::spawn(move || {
                    let obj: glib::Object = unsafe {
                        glib::Object::from_glib_none(term_ptr as *mut glib::gobject_ffi::GObject)
                    };
                    let term: TerminalBox = unsafe { obj.unsafe_cast() };
                    let stats = term.get_remote_stats();
                    glib::MainContext::default().invoke(move || {
                        if let Some(w) = weak.upgrade() {
                            if gen == *w.imp().remote_stats_generation.borrow() {
                                *w.imp().remote_stats_pending.borrow_mut() = false;
                                if let Some(label) = w.imp().stats_sys_label.borrow().clone() {
                                    if stats.is_empty() {
                                        label.set_text(&crate::system_stats::ssh_placeholder());
                                    } else {
                                        label.set_text(&stats);
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
            let osc = term.get_osc133_stats();
            if osc.is_empty() {
                crate::system_stats::collect(false)
            } else {
                osc
            }
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
        let mut data = crate::session::SessionData::default();
        data.split_mode = self.imp().split_mode.borrow().clone();
        let nb = self.imp().notebook.borrow().clone().unwrap();
        let nb2 = self.imp().notebook2.borrow().clone().unwrap();
        for (notebook, target) in [
            (&nb, &mut data.tabs_left),
            (&nb2, &mut data.tabs_right),
        ] {
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
                target.push(crate::session::TabEntry {
                    base_title,
                    title,
                    cwd,
                });
            }
        }
        data
    }

    fn save_session(&self, name: &str) -> bool {
        if settings().get_bool("session_restore") {
            let data = self.build_session_data();
            return crate::session::save_session(name, &data);
        }
        true
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
            self.add_new_tab(Some(&first.cwd), None, Some(&first.base_title), Some(&first.title), None);
            for tab in &tabs_left[1..] {
                self.add_new_tab(Some(&tab.cwd), None, Some(&tab.base_title), Some(&tab.title), None);
            }
        }
        if !tabs_right.is_empty() && split != "single" {
            let nb2 = self.imp().notebook2.borrow().clone().unwrap();
            self.set_split(&split, false);
            for tab in &tabs_right {
                self.add_new_tab(Some(&tab.cwd), Some(&nb2), Some(&tab.base_title), Some(&tab.title), None);
            }
        } else if split == "single" {
            self.set_split("single", false);
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
        self.imp().current_pages.borrow_mut().clear();
        *self.imp().split_mode.borrow_mut() = "single".to_string();
        nb2.hide();
        nb.set_show_tabs(settings().get_bool("show_tabs"));
        *self.imp().restoring_session.borrow_mut() = false;
    }

    fn load_session_named(&self, name: &str) {
        if let Some(data) = crate::session::load_session(name) {
            self.restore_window(data);
        }
    }
}

impl TerminalWindow for MainWindow {
    fn new_tab_signal(&self) {
        MainWindow::new_tab_signal(self);
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
    fn set_tab_title_from_terminal(&self, term: &TerminalBox, title: &str) {
        MainWindow::set_tab_title_from_terminal(self, term, title);
    }
    fn broadcast_feed(&self, source: &TerminalBox, data: &[u8]) {
        MainWindow::broadcast_feed(self, source, data);
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}


fn spawn_new_process(_new_window: bool) {
    let current_exe = std::env::current_exe().unwrap_or_default();
    let mut cmd = std::process::Command::new(&current_exe);
    if _new_window {
        cmd.arg("--new-window");
    }
    cmd.env("TPGK_RELOAD_MODULES", "1");
    let _ = cmd.spawn();
}
