//! Integrated Notes panel: search, tag filter, entry list, preview and a quick
//! append box. It is built once per main window and shown on demand from the
//! Notes menu.

use std::cell::RefCell;

use gtk::prelude::*;

use crate::notes_index::NoteEntry;

pub struct NotesPanel {
    pub root: gtk::Box,
    pub search: gtk::SearchEntry,
    pub tag: gtk::ComboBoxText,
    pub list: gtk::ListBox,
    pub preview: gtk::TextView,
    pub target: gtk::ComboBoxText,
    pub entry: gtk::Entry,
    pub status: gtk::Label,
    pub refresh: gtk::Button,
    pub close: gtk::Button,
    pub append: gtk::Button,
    entries: RefCell<Vec<NoteEntry>>,
}

impl NotesPanel {
    pub fn build() -> NotesPanel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(380);
        root.set_no_show_all(true);
        root.style_context().add_class("oxterm-notes-panel");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        header.set_margin_start(8);
        header.set_margin_end(8);
        let title = gtk::Label::new(None);
        title.set_markup("<b>Notes</b>");
        title.set_halign(gtk::Align::Start);
        header.pack_start(&title, false, false, 0);
        let refresh = gtk::Button::with_label("Refresh");
        refresh.set_tooltip_text(Some("Rescan the notes directory and rebuild the index"));
        header.pack_end(&refresh, false, false, 0);
        let close = gtk::Button::with_label("Close");
        close.set_tooltip_text(Some("Hide the notes panel"));
        header.pack_end(&close, false, false, 0);
        root.pack_start(&header, false, false, 0);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search notes..."));
        search.set_margin_start(8);
        search.set_margin_end(8);
        root.pack_start(&search, false, false, 0);

        let tag = gtk::ComboBoxText::new();
        tag.set_margin_start(8);
        tag.set_margin_end(8);
        tag.set_margin_top(4);
        tag.set_tooltip_text(Some("Filter results by tag"));
        root.pack_start(&tag, false, false, 0);

        let scroller = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_height(220);
        scroller.set_margin_top(6);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        scroller.add(&list);
        root.pack_start(&scroller, true, true, 0);

        let preview = gtk::TextView::new();
        preview.set_editable(false);
        preview.set_cursor_visible(false);
        preview.set_monospace(true);
        preview.set_wrap_mode(gtk::WrapMode::WordChar);
        preview.set_left_margin(8);
        preview.set_right_margin(8);
        let preview_scroller =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        preview_scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        preview_scroller.set_min_content_height(140);
        preview_scroller.add(&preview);
        root.pack_start(&preview_scroller, true, true, 0);

        let footer = gtk::Box::new(gtk::Orientation::Vertical, 4);
        footer.set_margin_start(8);
        footer.set_margin_end(8);
        footer.set_margin_top(6);
        footer.set_margin_bottom(8);
        let target = gtk::ComboBoxText::new();
        target.set_tooltip_text(Some("Note to append new entries to"));
        footer.pack_start(&target, false, false, 0);
        let entry_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("New note entry..."));
        entry_row.pack_start(&entry, true, true, 0);
        let append = gtk::Button::with_label("Add");
        entry_row.pack_end(&append, false, false, 0);
        footer.pack_start(&entry_row, false, false, 0);
        root.pack_start(&footer, false, false, 0);

        let status = gtk::Label::new(None);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_xalign(0.0);
        status.set_margin_start(8);
        status.set_margin_end(8);
        status.set_margin_bottom(6);
        status.style_context().add_class("dim-label");
        root.pack_start(&status, false, false, 0);

        NotesPanel {
            root,
            search,
            tag,
            list,
            preview,
            target,
            entry,
            status,
            refresh,
            close,
            append,
            entries: RefCell::new(Vec::new()),
        }
    }

    pub fn set_status(&self, message: &str) {
        self.status.set_text(message);
    }

    pub fn set_entries(&self, entries: Vec<NoteEntry>) {
        for child in self.list.children() {
            self.list.remove(&child);
        }
        for entry in &entries {
            let row = gtk::ListBoxRow::new();
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
            cell.set_margin_top(4);
            cell.set_margin_bottom(4);
            cell.set_margin_start(6);
            cell.set_margin_end(6);
            let title = gtk::Label::new(None);
            title.set_markup(&format!(
                "<b>{}</b>  <small>{}</small>",
                escape_markup(&entry.note),
                escape_markup(&entry.timestamp)
            ));
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let snippet = gtk::Label::new(Some(&entry.snippet()));
            snippet.set_xalign(0.0);
            snippet.set_ellipsize(gtk::pango::EllipsizeMode::End);
            snippet.style_context().add_class("dim-label");
            cell.pack_start(&title, false, false, 0);
            cell.pack_start(&snippet, false, false, 0);
            row.add(&cell);
            self.list.add(&row);
        }
        self.list.show_all();
        *self.entries.borrow_mut() = entries;
    }

    pub fn set_tags(&self, tags: &[String]) {
        let current = self.tag.active_id();
        self.tag.remove_all();
        self.tag.append(Some(""), "All tags");
        for tag in tags {
            self.tag.append(Some(tag), &format!("#{}", tag));
        }
        let restore = current
            .as_ref()
            .map(|c| tags.iter().any(|t| t == c.as_str()))
            .unwrap_or(false);
        if restore {
            self.tag.set_active_id(current.as_ref().map(|c| c.as_str()));
        } else {
            self.tag.set_active_id(Some(""));
        }
    }

    pub fn set_targets(&self, notes: &[String]) {
        let current = self.target.active_id();
        self.target.remove_all();
        self.target.append(Some(""), "Default note");
        for note in notes {
            self.target.append(Some(note), note);
        }
        let restore = current
            .as_ref()
            .map(|c| c.is_empty() || notes.iter().any(|n| n == c.as_str()))
            .unwrap_or(true);
        if restore {
            self.target
                .set_active_id(current.as_ref().map(|c| c.as_str()));
        }
        if self.target.active_id().is_none() {
            self.target.set_active_id(Some(""));
        }
    }

    pub fn search_query(&self) -> String {
        self.search.text().to_string()
    }

    pub fn active_tag(&self) -> String {
        self.tag
            .active_id()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    pub fn selected_target(&self) -> String {
        self.target
            .active_id()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    pub fn take_entry_text(&self) -> String {
        let text = self.entry.text().to_string();
        self.entry.set_text("");
        text
    }

    /// Show the entry at `index` in the preview and preselect its note as the
    /// append target.
    pub fn preview_index(&self, index: i32) {
        let entries = self.entries.borrow();
        let Some(entry) = usize::try_from(index).ok().and_then(|i| entries.get(i)) else {
            return;
        };
        let buffer = self.preview.buffer().expect("preview buffer");
        buffer.set_text(&entry.body);
        self.target.set_active_id(Some(&entry.note));
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
