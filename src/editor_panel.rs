//! Integrated text editor pane, built on GtkSourceView 4.
//!
//! The widget owns one buffer at a time and keeps the editor concerns that are
//! already solved by GtkSourceView (syntax, undo/redo, line numbers,
//! auto-indent) inside the library. Everything specific to Oxterm -- the
//! settings, the terminal font and the private atomic save -- is wired here.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use sourceview4::prelude::*;

use crate::settings::settings;

pub struct EditorPanel {
    pub root: gtk::Box,
    pub view: sourceview4::View,
    pub buffer: sourceview4::Buffer,
    pub path_label: gtk::Label,
    pub status: gtk::Label,
    pub save: gtk::Button,
    pub close: gtk::Button,
    path: RefCell<Option<PathBuf>>,
    loading: Cell<bool>,
    dirty: Cell<bool>,
}

/// Pick a syntax style scheme: the explicit setting wins, otherwise follow the
/// light/dark nature of the active terminal theme. All the 236 shipped themes
/// keep working because this only reads their background colour.
fn style_scheme() -> Option<sourceview4::StyleScheme> {
    let manager = sourceview4::StyleSchemeManager::new();
    let configured = settings().get_str("editor_style_scheme");
    if !configured.trim().is_empty() {
        if let Some(scheme) = manager.scheme(configured.trim()) {
            return Some(scheme);
        }
    }
    let theme_name = settings().get_str("color_scheme");
    let dark = crate::headless::themes::get(&theme_name)
        .map(|theme| crate::headless::themes::is_dark_color(&theme.background))
        .unwrap_or(true);
    let candidates: &[&str] = if dark {
        &["oblivion", "solarized-dark", "cobalt", "classic"]
    } else {
        &["classic", "tango", "solarized-light", "kate"]
    };
    candidates.iter().find_map(|id| manager.scheme(id))
}

impl EditorPanel {
    pub fn build() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(320);
        root.set_no_show_all(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(8);
        header.set_margin_end(8);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        let path_label = gtk::Label::new(Some("No file open"));
        path_label.set_halign(gtk::Align::Start);
        path_label.set_hexpand(true);
        path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        header.pack_start(&path_label, true, true, 0);
        let save = gtk::Button::with_label("Save");
        save.set_tooltip_text(Some("Save this file (Ctrl+S)"));
        save.set_sensitive(false);
        let close = gtk::Button::with_label("Close");
        close.set_tooltip_text(Some("Hide the editor panel"));
        header.pack_end(&close, false, false, 0);
        header.pack_end(&save, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let buffer = sourceview4::Buffer::new(None::<&gtk::TextTagTable>);
        buffer.set_max_undo_levels(-1);
        let view = sourceview4::View::with_buffer(&buffer);
        view.set_show_line_numbers(true);
        view.set_highlight_current_line(true);
        view.set_insert_spaces_instead_of_tabs(true);
        view.set_tab_width(4);
        view.set_auto_indent(true);
        view.set_monospace(true);
        view.style_context().add_class("oxterm-editor");
        // Reuse the terminal's typography so switching to the editor does not
        // feel like switching applications.
        if let Some(screen) = gdk::Screen::default() {
            let css = format!(
                ".oxterm-editor {{ font-family: '{}'; font-size: {}pt; }}",
                settings().get_str("font_name"),
                settings().get_i64("font_size")
            );
            let provider = gtk::CssProvider::new();
            if provider.load_from_data(css.as_bytes()).is_ok() {
                gtk::StyleContext::add_provider_for_screen(
                    &screen,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }

        let scroller = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.add(&view);
        root.pack_start(&scroller, true, true, 0);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_top(4);
        status.set_margin_bottom(6);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        let panel = Rc::new(Self {
            root,
            view,
            buffer,
            path_label,
            status,
            save,
            close,
            path: RefCell::new(None),
            loading: Cell::new(false),
            dirty: Cell::new(false),
        });

        let weak = Rc::downgrade(&panel);
        panel.buffer.connect_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                if !panel.loading.get() {
                    panel.set_dirty(true);
                }
            }
        });
        // Ctrl+S belongs to the editor only while it has focus: the terminal
        // keeps its own Ctrl+S handling for the shell.
        let weak = Rc::downgrade(&panel);
        panel.view.connect_key_press_event(move |_, event| {
            if event.state().contains(gdk::ModifierType::CONTROL_MASK)
                && event.keyval() == gdk::keys::constants::s
            {
                if let Some(panel) = weak.upgrade() {
                    if let Err(error) = panel.save() {
                        panel.set_status(&error);
                    }
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        panel
    }

    fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
        let name = self
            .path
            .borrow()
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No file open".to_string());
        self.path_label
            .set_label(&if dirty { format!("{} *", name) } else { name });
        self.save.set_sensitive(self.path.borrow().is_some());
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.path.borrow().clone()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    pub fn set_status(&self, message: &str) {
        self.status.set_label(message);
    }

    /// Load one file into the buffer. The load is not an undoable action, so
    /// Ctrl+Z never resurrects the previous document.
    pub fn open(&self, path: &Path, line: Option<u32>) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {}", path.display(), error))?;
        self.loading.set(true);
        self.buffer.begin_not_undoable_action();
        self.buffer.set_text(&text);
        self.buffer.end_not_undoable_action();
        self.loading.set(false);

        let manager = sourceview4::LanguageManager::new();
        let language = manager.guess_language(Some(path), None);
        self.buffer.set_language(language.as_ref());
        if let Some(scheme) = style_scheme() {
            self.buffer.set_style_scheme(Some(&scheme));
        }

        *self.path.borrow_mut() = Some(path.to_path_buf());
        self.set_dirty(false);
        self.set_status(&format!(
            "{} · {} lines",
            path.display(),
            self.buffer.line_count()
        ));

        if let Some(line) = line {
            let mut iter = self.buffer.iter_at_line(line.saturating_sub(1) as i32);
            iter.set_line_offset(0);
            self.buffer.place_cursor(&iter);
            self.view.scroll_to_iter(&mut iter, 0.0, false, 0.0, 0.0);
        }
        self.view.grab_focus();
        Ok(())
    }

    /// Atomically replace the file on disk, preserving the private permissions
    /// that the remote-edit cache relies on.
    pub fn save(&self) -> Result<PathBuf, String> {
        let path = self
            .path
            .borrow()
            .clone()
            .ok_or_else(|| "No file is open in the editor".to_string())?;
        let (start, end) = self.buffer.bounds();
        let text = self
            .buffer
            .text(&start, &end, false)
            .unwrap_or_default()
            .to_string();
        crate::persistence::atomic_write(&path, text.as_bytes())
            .map_err(|error| format!("{}: {}", path.display(), error))?;
        self.set_dirty(false);
        self.set_status(&format!("Saved {}", path.display()));
        Ok(path)
    }
}
