//! Small GTK dialogs backing the Notes menu: a composer for new notes and a
//! picker for choosing (or naming) the file an entry is appended to.

use gtk::prelude::*;

use crate::notes;

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
