//! Small GTK dialogs backing the Notes menu: a composer for new notes and a
//! picker for choosing (or naming) the file an entry is appended to.

use glib::subclass::prelude::*;
use gtk::prelude::*;

use crate::notes::{self, NoteContext};
use crate::settings::settings;
use crate::terminal::TerminalBox;
use crate::window::{MainWindow, TerminalWindow};

/// Ask for a note composer (text + target file). Returns `(filename, text)`
/// where `filename` is empty for the default note, or `None` when cancelled.
pub fn compose_note(parent: &gtk::Window, title: &str, initial: &str) -> Option<(String, String)> {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Save", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_size(560, 420);

    let content = dialog.content_area();
    content.set_spacing(6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let (combo, entry) = target_controls();
    content.pack_start(&gtk::Label::new(Some("Note file")), false, false, 0);
    content.pack_start(&combo, false, false, 0);
    entry.set_placeholder_text(Some("New note name (optional)"));
    content.pack_start(&entry, false, false, 0);

    content.pack_start(&gtk::Label::new(Some("Note text")), false, false, 0);
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    if !initial.is_empty() {
        view.buffer().expect("text view buffer").set_text(initial);
    }
    let scroller = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroller.add(&view);
    content.pack_start(&scroller, true, true, 0);

    dialog.show_all();
    let response = dialog.run();
    if response != gtk::ResponseType::Ok {
        unsafe { dialog.destroy() };
        return None;
    }

    let buffer = view.buffer().expect("text view buffer");
    let text = buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .map(|t| t.to_string())
        .unwrap_or_default();
    let name = resolve_target_name(&combo, &entry);
    unsafe { dialog.destroy() };

    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((name, text))
}

/// Ask which note to use. Returns `Some("")` for the default note, `Some(name)`
/// for a named file, or `None` when cancelled or the name is invalid.
pub fn pick_note_file(parent: &gtk::Window, title: &str) -> Option<String> {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Choose", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_size(420, 180);

    let content = dialog.content_area();
    content.set_spacing(6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let (combo, entry) = target_controls();
    content.pack_start(
        &gtk::Label::new(Some("Choose an existing note or type a new name")),
        false,
        false,
        0,
    );
    content.pack_start(&combo, false, false, 0);
    entry.set_placeholder_text(Some("New note name (optional)"));
    content.pack_start(&entry, false, false, 0);

    dialog.show_all();
    let response = dialog.run();
    if response != gtk::ResponseType::Ok {
        unsafe { dialog.destroy() };
        return None;
    }
    let name = resolve_target_name(&combo, &entry);
    unsafe { dialog.destroy() };

    if name.is_empty() {
        return Some(String::new());
    }
    match notes::validate_note_name(&name) {
        Ok(valid) => Some(valid),
        Err(e) => {
            error_dialog(parent, &e);
            None
        }
    }
}

/// Confirm appending a potentially large amount of text. Returns false when the
/// user declines.
pub fn confirm_large_append(parent: &gtk::Window, kind: &str, lines: usize, chars: usize) -> bool {
    let dialog = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Question,
        gtk::ButtonsType::YesNo,
        &format!(
            "Append {} to a note?\n\n{} lines, {} characters.",
            kind, lines, chars
        ),
    );
    dialog.set_title("Append to note");
    let response = dialog.run();
    let accepted = response == gtk::ResponseType::Yes;
    unsafe { dialog.destroy() };
    accepted
}

fn target_controls() -> (gtk::ComboBoxText, gtk::Entry) {
    let combo = gtk::ComboBoxText::new();
    combo.append(Some(""), "Default note");
    for name in notes::list_note_files() {
        combo.append(Some(&name), &name);
    }
    combo.set_active_id(Some(""));
    let entry = gtk::Entry::new();
    (combo, entry)
}

fn resolve_target_name(combo: &gtk::ComboBoxText, entry: &gtk::Entry) -> String {
    let typed = entry.text().trim().to_string();
    if !typed.is_empty() {
        return typed;
    }
    combo.active_id().map(|s| s.to_string()).unwrap_or_default()
}

fn error_dialog(parent: &gtk::Window, message: &str) {
    let dialog = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Error,
        gtk::ButtonsType::Ok,
        message,
    );
    dialog.run();
    unsafe { dialog.destroy() };
}

impl MainWindow {
    pub(crate) fn copy_all_to_notes(&self, term: &TerminalBox) {
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
        if !confirm_large_append(
            &window,
            "all terminal text",
            text.lines().count(),
            text.chars().count(),
        ) {
            return;
        }
        let Some(name) = pick_note_file(&window, "Copy all to note") else {
            return;
        };
        let filename = if name.is_empty() { None } else { Some(name) };
        let context = NoteContext::from_terminal("scrollback", &term.get_cwd());
        self.note_write(text, filename, context);
    }

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
                    w.schedule_notes_search();
                }
            });
            let weak = crate::SendWeak::new(self);
            panel.tag.connect_changed(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.schedule_notes_search();
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
        slot.pack_start(&panel.root, true, true, 0);
        slot.show();
        if let Some(pane) = slot.parent().and_then(|p| p.downcast::<gtk::Paned>().ok()) {
            let width = settings().get_i64("notes_panel_width").clamp(220, 1200) as i32;
            let extent = pane.allocated_width();
            if extent > width {
                self.imp().notes_positioning.set(true);
                pane.set_position(extent - width);
                self.imp().notes_positioning.set(false);
            }
            let weak = crate::SendWeak::new(self);
            pane.connect_notify_local(Some("position"), move |pane, _| {
                if let Some(window) = weak.upgrade() {
                    window.remember_notes_panel_width(pane);
                }
            });
        }
        *self.imp().notes_panel.borrow_mut() = Some(panel);
        true
    }

    fn remember_notes_panel_width(&self, pane: &gtk::Paned) {
        if !self.notes_panel_visible() || self.imp().notes_positioning.get() {
            return;
        }
        let extent = pane.allocated_width();
        if extent <= 1 {
            return;
        }
        let width = ((extent - pane.position()) as i64).clamp(220, 1200);
        if (width - settings().get_i64("notes_panel_width")).abs() <= 1 {
            return;
        }
        let generation = self.imp().notes_width_generation.get().wrapping_add(1);
        self.imp().notes_width_generation.set(generation);
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if window.imp().notes_width_generation.get() != generation {
                return;
            }
            settings().set_i64("notes_panel_width", width);
        });
    }

    pub(crate) fn notes_panel_visible(&self) -> bool {
        self.imp()
            .notes_panel
            .borrow()
            .as_ref()
            .map(|p| p.root.is_visible())
            .unwrap_or(false)
    }

    pub(crate) fn set_notes_panel_visible(&self, visible: bool) {
        if visible && !self.ensure_notes_panel() {
            return;
        }
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            if visible {
                // The panel carries `no_show_all` so the window's own
                // `show_all()` keeps it hidden; clear it just long enough to
                // show the panel and its children, then restore it.
                if let Some(slot) = self.imp().notes_slot.borrow().clone() {
                    slot.show();
                }
                panel.root.set_no_show_all(false);
                panel.root.show_all();
                panel.root.set_no_show_all(true);
            } else {
                panel.root.hide();
                if let Some(slot) = self.imp().notes_slot.borrow().clone() {
                    slot.hide();
                }
                if let Some(term) = self.current_terminal() {
                    term.vte().grab_focus();
                }
            }
        }
    }

    fn refresh_note_targets(&self) {
        let notes = crate::notes::list_note_files();
        if let Some(panel) = self.imp().notes_panel.borrow().as_ref() {
            panel.set_targets(&notes);
        }
    }

    pub(crate) fn reload_notes_panel(&self) {
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

    /// Debounce typing in the notes search box. Every keystroke used to spawn
    /// a thread that opened the database and re-ran the schema DDL.
    fn schedule_notes_search(&self) {
        self.imp()
            .notes_search_generation
            .set(self.imp().notes_search_generation.get() + 1);
        if let Some(source) = self.imp().notes_search_source.borrow_mut().take() {
            source.remove();
        }
        let weak = self.downgrade();
        let source =
            glib::timeout_add_local_once(std::time::Duration::from_millis(180), move || {
                if let Some(w) = weak.upgrade() {
                    *w.imp().notes_search_source.borrow_mut() = None;
                    w.run_notes_search();
                }
            });
        *self.imp().notes_search_source.borrow_mut() = Some(source);
    }

    fn run_notes_search(&self) {
        let (query, tag) = match self.imp().notes_panel.borrow().as_ref() {
            Some(panel) => (panel.search_query(), panel.active_tag()),
            None => return,
        };
        let generation = self.imp().notes_search_generation.get().wrapping_add(1);
        self.imp().notes_search_generation.set(generation);
        let weak = crate::SendWeak::new(self);
        crate::notes_index::search_async(query, tag, 200, move |result| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            // A newer search started while this one was running.
            if w.imp().notes_search_generation.get() != generation {
                return;
            }
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
}
