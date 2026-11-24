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
    pub open_file: gtk::Button,
    pub save: gtk::Button,
    pub undo: gtk::Button,
    pub redo: gtk::Button,
    pub find: gtk::Button,
    pub close_document: gtk::Button,
    pub hide_panel: gtk::Button,
    pub search: gtk::SearchEntry,
    search_bar: gtk::Box,
    path: RefCell<Option<PathBuf>>,
    loading: Cell<bool>,
    dirty: Cell<bool>,
    saved: RefCell<Option<Rc<dyn Fn()>>>,
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

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.set_margin_start(8);
        header.set_margin_end(8);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        let path_label = gtk::Label::new(Some("No file open"));
        path_label.set_halign(gtk::Align::Start);
        path_label.set_hexpand(true);
        path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        header.pack_start(&path_label, true, true, 0);

        let open_file = icon_button("document-open", "Open a file in the editor");
        let save = icon_button("document-save", "Save this file (Ctrl+S)");
        let undo = icon_button("edit-undo", "Undo (Ctrl+Z)");
        let redo = icon_button("edit-redo", "Redo (Ctrl+Shift+Z)");
        let find = icon_button("edit-find", "Find in this file (Ctrl+F)");
        let close_document = icon_button("window-close", "Close document (Ctrl+W)");
        let hide_panel = icon_button("go-next", "Hide the Notes/Editor dock");
        save.set_sensitive(false);
        undo.set_sensitive(false);
        redo.set_sensitive(false);
        close_document.set_sensitive(false);
        for button in [
            &open_file,
            &hide_panel,
            &close_document,
            &find,
            &redo,
            &undo,
            &save,
        ] {
            header.pack_end(button, false, false, 0);
        }
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

        let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        search_bar.set_no_show_all(true);
        search_bar.hide();
        search_bar.set_margin_start(8);
        search_bar.set_margin_end(8);
        search_bar.set_margin_top(4);
        let search = gtk::SearchEntry::new();
        search.set_hexpand(true);
        search.set_placeholder_text(Some("Find in file (Enter = next)"));
        search_bar.pack_start(&search, true, true, 0);
        let prev = icon_button("go-up", "Find previous (Shift+Enter)");
        let next = icon_button("go-down", "Find next (Enter)");
        let dismiss = icon_button("window-close", "Close search bar (Escape)");
        search_bar.pack_end(&dismiss, false, false, 0);
        search_bar.pack_end(&next, false, false, 0);
        search_bar.pack_end(&prev, false, false, 0);
        root.pack_start(&search_bar, false, false, 0);

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
            open_file,
            save,
            undo,
            redo,
            find,
            close_document,
            hide_panel,
            search,
            search_bar,
            path: RefCell::new(None),
            loading: Cell::new(false),
            dirty: Cell::new(false),
            saved: RefCell::new(None),
        });

        let weak = Rc::downgrade(&panel);
        panel.buffer.connect_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                if !panel.loading.get() {
                    panel.set_dirty(true);
                }
                panel.update_history_sensitivity();
            }
        });
        // Ctrl+S belongs to the editor only while it has focus: the terminal
        // keeps its own Ctrl+S handling for the shell.
        let weak = Rc::downgrade(&panel);
        panel.view.connect_key_press_event(move |_, event| {
            let state = event.state();
            let control = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let key = event.keyval();
            if control && key == gdk::keys::constants::s {
                if let Some(panel) = weak.upgrade() {
                    if let Err(error) = panel.save() {
                        panel.set_status(&error);
                    }
                }
                return glib::Propagation::Stop;
            }
            if control && key == gdk::keys::constants::f {
                if let Some(panel) = weak.upgrade() {
                    panel.show_find(true);
                }
                return glib::Propagation::Stop;
            }
            if control && key == gdk::keys::constants::w {
                if let Some(panel) = weak.upgrade() {
                    panel.close_document();
                }
                return glib::Propagation::Stop;
            }
            if control && key == gdk::keys::constants::z {
                if let Some(panel) = weak.upgrade() {
                    if shift {
                        panel.buffer.redo();
                    } else {
                        panel.buffer.undo();
                    }
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(&panel);
        panel.save.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                if let Err(error) = panel.save() {
                    panel.set_status(&error);
                }
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.undo.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.buffer.undo();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.redo.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.buffer.redo();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.find.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.show_find(true);
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.close_document.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.close_document();
            }
        });

        let weak = Rc::downgrade(&panel);
        panel.search.connect_activate(move |entry| {
            if let Some(panel) = weak.upgrade() {
                let text = entry.text().to_string();
                if !panel.find_in_buffer(&text, true) {
                    panel.set_status(&format!("No match for '{}'", text));
                }
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.search.connect_search_changed(move |entry| {
            if let Some(panel) = weak.upgrade() {
                let text = entry.text().to_string();
                if !text.is_empty() {
                    panel.find_in_buffer(&text, true);
                }
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.search.connect_key_press_event(move |_, event| {
            let backwards = event.state().contains(gdk::ModifierType::SHIFT_MASK);
            if event.keyval() == gdk::keys::constants::Escape {
                if let Some(panel) = weak.upgrade() {
                    panel.show_find(false);
                }
                return glib::Propagation::Stop;
            }
            if event.keyval() == gdk::keys::constants::Return {
                if let Some(panel) = weak.upgrade() {
                    let text = panel.search.text().to_string();
                    panel.find_in_buffer(&text, !backwards);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&panel);
        prev.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                let text = panel.search.text().to_string();
                panel.find_in_buffer(&text, false);
            }
        });
        let weak = Rc::downgrade(&panel);
        next.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                let text = panel.search.text().to_string();
                panel.find_in_buffer(&text, true);
            }
        });
        let weak = Rc::downgrade(&panel);
        dismiss.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.show_find(false);
            }
        });

        panel
    }

    fn update_history_sensitivity(&self) {
        self.undo.set_sensitive(self.buffer.can_undo());
        self.redo.set_sensitive(self.buffer.can_redo());
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
        let has_path = self.path.borrow().is_some();
        self.save.set_sensitive(has_path);
        self.close_document.set_sensitive(has_path);
        self.find.set_sensitive(has_path);
        self.update_history_sensitivity();
    }

    /// Register a callback fired after every successful save. The window uses
    /// it to refresh dependent views (the notes index) no matter whether the
    /// save came from the toolbar, Ctrl+S or the close-document dialog.
    pub fn connect_saved(&self, callback: impl Fn() + 'static) {
        *self.saved.borrow_mut() = Some(Rc::new(callback));
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

    fn show_find(&self, show: bool) {
        if show {
            self.search_bar.set_no_show_all(false);
            self.search_bar.show_all();
            self.search.grab_focus();
        } else {
            self.search_bar.hide();
            self.search_bar.set_no_show_all(true);
            self.view.grab_focus();
        }
    }

    /// Search from the cursor, wrapping around at the end of the buffer.
    fn find_in_buffer(&self, text: &str, forward: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        let flags = gtk::TextSearchFlags::CASE_INSENSITIVE;
        let (start, end) = self.buffer.bounds();
        let cursor = self
            .buffer
            .get_insert()
            .map(|mark| self.buffer.iter_at_mark(&mark))
            .unwrap_or_else(|| self.buffer.start_iter());
        let found = if forward {
            cursor
                .forward_search(text, flags, Some(&end))
                .or_else(|| start.forward_search(text, flags, Some(&end)))
        } else {
            cursor
                .backward_search(text, flags, Some(&start))
                .or_else(|| end.backward_search(text, flags, Some(&start)))
        };
        if let Some((match_start, match_end)) = found {
            self.buffer.select_range(&match_start, &match_end);
            let mut iter = match_start;
            self.view.scroll_to_iter(&mut iter, 0.0, false, 0.0, 0.0);
            true
        } else {
            false
        }
    }

    /// Close the current document, keeping the panel open. Unsaved changes are
    /// confirmed with an explicit Save / Discard / Cancel choice.
    pub fn close_document(&self) -> bool {
        if self.path.borrow().is_none() {
            return false;
        }
        if self.dirty.get() {
            let parent = self
                .root
                .toplevel()
                .and_then(|window| window.downcast::<gtk::Window>().ok());
            let name = self.path_label.text().trim_end_matches(" *").to_string();
            let dialog = gtk::MessageDialog::new(
                parent.as_ref(),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Question,
                gtk::ButtonsType::None,
                &format!("Discard unsaved changes to {}?", name),
            );
            dialog.add_button("Cancel", gtk::ResponseType::Cancel);
            dialog.add_button("Discard", gtk::ResponseType::Reject);
            dialog.add_button("Save", gtk::ResponseType::Accept);
            let response = dialog.run();
            unsafe { dialog.destroy() };
            match response {
                gtk::ResponseType::Accept => {
                    if let Err(error) = self.save() {
                        self.set_status(&error);
                        return false;
                    }
                }
                gtk::ResponseType::Cancel => return false,
                _ => {}
            }
        }
        self.clear();
        true
    }

    fn clear(&self) {
        self.show_find(false);
        self.loading.set(true);
        self.buffer.begin_not_undoable_action();
        self.buffer.set_text("");
        self.buffer.end_not_undoable_action();
        self.loading.set(false);
        self.buffer.set_language(None::<&sourceview4::Language>);
        *self.path.borrow_mut() = None;
        self.set_dirty(false);
        self.set_status("No file open");
        self.view.grab_focus();
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
        let callback = self.saved.borrow().clone();
        if let Some(callback) = callback {
            callback();
        }
        Ok(path)
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
    button.set_tooltip_text(Some(tooltip));
    button
}
