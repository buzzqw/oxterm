//! Preferences and information dialogs.
//!
//! The preferences window is a searchable stack of focused pages. A filter box
//! at the top hides every setting that does not match the query and switches to
//! the first page with a hit; each row shows its explanation under the label
//! instead of hiding it in a tooltip. Rows register themselves while the pages
//! are built (see [`Page`]), so a new setting is searchable as soon as it is
//! added. Every page is restored independently, and the dialog as a whole can
//! be reset to the documented defaults from `spec`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use glib::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

use crate::headless::{diagnostics, spec};
use crate::settings::{self, settings, Settings};
use crate::window::{APP_NAME, EUPL_LICENSE_TEXT, VERSION};

const PROJECT_URL: &str = "https://github.com/buzzqw/oxterm";
const MANUAL_URL: &str = "https://github.com/buzzqw/oxterm/blob/master/manual.md";

fn hex_to_rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or_else(|_| gdk::RGBA::new(0.0, 0.0, 0.0, 1.0))
}

fn rgba_to_hex(rgba: &gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8
    )
}

// -- Documented defaults ----------------------------------------------------
//
// `spec` is the single source of truth for defaults, so "Restore Defaults"
// cannot drift from `--show-config` or the generated documentation.

fn default_value(key: &str) -> serde_json::Value {
    spec::find(key)
        .map(|setting| setting.default_value())
        .unwrap_or(serde_json::Value::Null)
}

fn default_str(key: &str) -> String {
    default_value(key).as_str().unwrap_or("").to_string()
}

fn default_bool(key: &str) -> bool {
    default_value(key).as_bool().unwrap_or(false)
}

fn default_i64(key: &str) -> i64 {
    default_value(key).as_i64().unwrap_or(0)
}

fn default_f64(key: &str) -> f64 {
    default_value(key).as_f64().unwrap_or(0.0)
}

// -- Enumerated values ------------------------------------------------------
//
// Each entry is (stored value, label). Keeping them in one place means the
// combo box, the apply step and the reset step agree on the ordering.

type Choices = &'static [(&'static str, &'static str)];

const DYNAMIC_TITLE: Choices = &[
    ("replace", "Replace initial title"),
    ("before", "Goes before initial title"),
    ("after", "Goes after initial title"),
    ("hide", "Isn't displayed"),
];

const SCROLLBAR_POSITION: Choices = &[
    ("right", "Right side"),
    ("left", "Left side"),
    ("disabled", "Disabled"),
];

const CURSOR_SHAPE: Choices = &[
    ("block", "Block (filled rectangle)"),
    ("underline", "Underline"),
    ("ibeam", "I-beam"),
];

const FONT_RENDERING: Choices = &[
    ("system", "System default"),
    ("grayscale", "Grayscale"),
    ("subpixel", "Subpixel (LCD)"),
    ("none", "None"),
];

const FONT_HINTING: Choices = &[
    ("system", "System default"),
    ("none", "None"),
    ("slight", "Slight"),
    ("medium", "Medium"),
    ("full", "Full"),
];

const UNDERLINE_STYLE: Choices = &[
    ("single", "Single"),
    ("double", "Double"),
    ("curly", "Curly"),
    ("dashed", "Dashed"),
    ("dotted", "Dotted"),
];

const BACKSPACE_BINDING: Choices = &[
    ("auto", "Auto-detect"),
    ("ascii-del", "ASCII DEL (127)"),
    ("escape-sequence", "Escape sequence"),
    ("control-h", "Control-H (8)"),
];

const DELETE_BINDING: Choices = &[
    ("auto", "Auto-detect"),
    ("escape-sequence", "Escape sequence"),
    ("ascii-del", "ASCII DEL (127)"),
    ("control-h", "Control-H (8)"),
];

const ENCODINGS: &[&str] = &[
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

fn index_of(choices: Choices, value: &str) -> u32 {
    choices
        .iter()
        .position(|(stored, _)| *stored == value)
        .unwrap_or(0) as u32
}

fn scale_spin(value: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(0.8, 2.0, 0.05);
    spin.set_digits(2);
    spin.set_value(value);
    spin
}

fn combo_for(choices: Choices, current: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for (_, label) in choices {
        combo.append_text(label);
    }
    combo.set_active(Some(index_of(choices, current)));
    combo
}

fn combo_value(combo: &gtk::ComboBoxText, choices: Choices, fallback: &str) -> String {
    combo
        .active()
        .and_then(|index| choices.get(index as usize))
        .map(|(stored, _)| (*stored).to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn theme_combo(current: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    let mut names = Vec::new();
    for name in settings::color_schemes().keys() {
        combo.append_text(name);
        names.push(name.to_string());
    }
    let index = names.iter().position(|name| name == current).unwrap_or(0);
    combo.set_active(Some(index as u32));
    combo
}

fn encoding_combo(current: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for encoding in ENCODINGS {
        combo.append_text(encoding);
    }
    let index = ENCODINGS
        .iter()
        .position(|name| *name == current)
        .unwrap_or(0);
    combo.set_active(Some(index as u32));
    combo
}

// -- Search index -----------------------------------------------------------

#[derive(Default)]
struct FilterIndex {
    rows: RefCell<Vec<FilterRow>>,
    sections: RefCell<Vec<FilterSection>>,
}

struct FilterRow {
    widgets: Vec<gtk::Widget>,
    /// Lower-cased text matched against the search query.
    haystack: String,
    page: &'static str,
}

struct FilterSection {
    header: gtk::Widget,
    /// Indices into [`FilterIndex::rows`] belonging to this section.
    rows: Vec<usize>,
    /// Explanatory paragraphs that follow the section header.
    extras: Vec<gtk::Widget>,
}

fn caption_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    label.set_line_wrap(true);
    label.set_max_width_chars(46);
    label.style_context().add_class("dim-label");
    label.style_context().add_class("settings-caption");
    label
}

/// Builds one preferences page and records every row for the search filter.
struct Page {
    id: &'static str,
    title: &'static str,
    grid: gtk::Grid,
    index: Rc<FilterIndex>,
    next_row: Cell<i32>,
    section: Cell<Option<usize>>,
}

impl Page {
    fn new(id: &'static str, title: &'static str, index: Rc<FilterIndex>) -> Page {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(18);
        grid.set_row_spacing(10);
        Page {
            id,
            title,
            grid,
            index,
            next_row: Cell::new(0),
            section: Cell::new(None),
        }
    }

    fn section(&self, title: &str) {
        let row = self.next_row.get();
        let label = gtk::Label::new(None);
        label.set_markup(&format!("<b>{}</b>", title));
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        label.set_margin_top(if row == 0 { 0 } else { 14 });
        self.grid.attach(&label, 0, row, 2, 1);
        self.next_row.set(row + 1);
        self.index.sections.borrow_mut().push(FilterSection {
            header: label.upcast(),
            rows: Vec::new(),
            extras: Vec::new(),
        });
        self.section
            .set(Some(self.index.sections.borrow().len() - 1));
    }

    /// Adds a labelled setting. The explanation is the tooltip text, mirrored
    /// under the label so it is readable without hovering.
    fn row(&self, label: &str, widget: &impl IsA<gtk::Widget>) -> &Self {
        self.row_keyed(label, widget, None)
    }

    fn row_keyed(&self, label: &str, widget: &impl IsA<gtk::Widget>, key: Option<&str>) -> &Self {
        let row = self.next_row.get();
        self.next_row.set(row + 1);

        let title = gtk::Label::new(Some(label));
        title.set_halign(gtk::Align::Start);
        title.set_xalign(0.0);
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.pack_start(&title, false, false, 0);

        let mut haystack = format!("{} {}", self.title, label);
        if let Some(key) = key {
            haystack.push(' ');
            haystack.push_str(key);
            if let Some(setting) = spec::find(key) {
                haystack.push(' ');
                haystack.push_str(setting.description);
            }
        }
        if let Some(tooltip) = widget.tooltip_text() {
            cell.pack_start(&caption_label(&tooltip), false, false, 0);
            haystack.push(' ');
            haystack.push_str(&tooltip);
        }

        let widget_ref: &gtk::Widget = widget.upcast_ref();
        widget_ref.set_hexpand(true);
        widget_ref.set_valign(gtk::Align::Center);
        self.grid.attach(&cell, 0, row, 1, 1);
        self.grid.attach(widget_ref, 1, row, 1, 1);

        self.register(vec![cell.upcast(), widget_ref.clone()], &haystack);
        self
    }

    /// Adds a full-width explanatory paragraph tied to the current section.
    fn note(&self, text: &str) -> &Self {
        let row = self.next_row.get();
        self.next_row.set(row + 1);
        let label = caption_label(text);
        self.grid.attach(&label, 0, row, 2, 1);
        if let Some(section) = self.section.get() {
            if let Some(section) = self.index.sections.borrow_mut().get_mut(section) {
                section.extras.push(label.upcast());
            }
        }
        self
    }

    /// Adds a full-width widget outside the label/value layout.
    fn full(&self, widget: &impl IsA<gtk::Widget>) -> &Self {
        let row = self.next_row.get();
        self.next_row.set(row + 1);
        self.grid.attach(widget, 0, row, 2, 1);
        self
    }

    /// Adds a full-width control that belongs to the current section and is
    /// hidden with it while filtering.
    fn extra(&self, widget: &impl IsA<gtk::Widget>) -> &Self {
        self.full(widget);
        if let Some(section) = self.section.get() {
            if let Some(section) = self.index.sections.borrow_mut().get_mut(section) {
                let widget: &gtk::Widget = widget.upcast_ref();
                section.extras.push(widget.clone());
            }
        }
        self
    }

    fn register(&self, widgets: Vec<gtk::Widget>, haystack: &str) {
        let row = self.index.rows.borrow().len();
        self.index.rows.borrow_mut().push(FilterRow {
            widgets,
            haystack: haystack.to_lowercase(),
            page: self.id,
        });
        if let Some(section) = self.section.get() {
            if let Some(section) = self.index.sections.borrow_mut().get_mut(section) {
                section.rows.push(row);
            }
        }
    }

    fn finish(self) -> gtk::ScrolledWindow {
        self.grid.set_margin_start(18);
        self.grid.set_margin_end(18);
        self.grid.set_margin_top(12);
        self.grid.set_margin_bottom(16);
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_min_content_width(520);
        scroll.add(&self.grid);
        scroll
    }
}

fn make_color_button(hex_color: &str, tooltip: &str) -> gtk::ColorButton {
    let rgba = hex_to_rgba(hex_color);
    let btn = gtk::ColorButton::with_rgba(&rgba);
    btn.set_size_request(56, 24);
    btn.set_halign(gtk::Align::Start);
    btn.set_tooltip_text(Some(tooltip));
    btn
}

/// Theme picker with a live filter; the built-in list has hundreds of entries,
/// so scrolling a plain combo box would be unusable.
fn choose_theme(parent: Option<&gtk::Window>, title: &str) -> Option<String> {
    let dialog = gtk::Dialog::new();
    dialog.set_title(title);
    dialog.set_transient_for(parent);
    dialog.set_modal(true);
    dialog.set_destroy_with_parent(true);
    dialog.set_default_size(400, 560);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search themes..."));
    search.set_margin_start(8);
    search.set_margin_end(8);
    search.set_margin_top(8);

    let list = gtk::ListBox::new();
    list.set_activate_on_single_click(true);
    for name in settings::color_schemes().keys() {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(name);
        let label = gtk::Label::new(Some(name));
        label.set_halign(gtk::Align::Start);
        label.set_margin_start(8);
        label.set_margin_end(8);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        row.add(&label);
        list.add(&row);
    }

    let query = Rc::new(RefCell::new(String::new()));
    list.set_filter_func(Some(Box::new({
        let query = query.clone();
        move |row: &gtk::ListBoxRow| {
            let query = query.borrow().clone();
            query.is_empty() || row.widget_name().to_lowercase().contains(&query)
        }
    })));
    {
        let query = query.clone();
        let list = list.clone();
        search.connect_search_changed(move |entry| {
            *query.borrow_mut() = entry.text().to_lowercase();
            list.invalidate_filter();
        });
    }

    let selected: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let selected = selected.clone();
        let dialog = dialog.clone();
        list.connect_row_activated(move |_, row| {
            *selected.borrow_mut() = Some(row.widget_name().to_string());
            dialog.response(gtk::ResponseType::Ok);
        });
    }

    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.add(&list);
    dialog.content_area().pack_start(&search, false, false, 0);
    dialog.content_area().pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );
    dialog.content_area().pack_start(&scroll, true, true, 0);

    let cancel = gtk::Button::with_label("Cancel");
    let select = gtk::Button::with_label("Select");
    select.style_context().add_class("suggested-action");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.response(gtk::ResponseType::Cancel));
    }
    {
        let dialog = dialog.clone();
        select.connect_clicked(move |_| dialog.response(gtk::ResponseType::Ok));
    }
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.pack_end(&select, false, false, 0);
    actions.pack_end(&cancel, false, false, 0);
    actions.set_margin_start(12);
    actions.set_margin_end(12);
    actions.set_margin_top(8);
    actions.set_margin_bottom(10);
    dialog.content_area().pack_start(&actions, false, false, 0);

    dialog.show_all();
    // Focus the first row rather than the filter box: a focused GtkEntry
    // hides its placeholder, and arrow keys work straight away.
    if let Some(row) = list.row_at_index(0) {
        dialog.set_focus(Some(&row));
    }

    let response = dialog.run();
    dialog.close();
    if response == gtk::ResponseType::Ok {
        selected.borrow().clone()
    } else {
        None
    }
}

// -- Pages ------------------------------------------------------------------

struct GeneralPage {
    title: gtk::Entry,
    dynamic: gtk::ComboBoxText,
    login: gtk::CheckButton,
    shell: gtk::Entry,
    columns: gtk::SpinButton,
    rows: gtk::SpinButton,
    session_restore: gtk::CheckButton,
    ssh_sharing: gtk::CheckButton,
    confirm_close: gtk::CheckButton,
    auto_copy: gtk::CheckButton,
    warn_paste: gtk::CheckButton,
    file_manager: gtk::Entry,
}

fn build_general(s: &Settings, index: &Rc<FilterIndex>) -> (gtk::ScrolledWindow, GeneralPage) {
    let page = Page::new("general", "General", index.clone());

    page.section("Tab titles");
    let title = gtk::Entry::new();
    title.set_text(&s.get_str_default("tab_title", "Terminal"));
    title.set_tooltip_text(Some(
        "Name displayed on each new tab until a program changes it.",
    ));
    page.row_keyed("Initial title", &title, Some("tab_title"));

    let dynamic = combo_for(
        DYNAMIC_TITLE,
        &s.get_str_default("dynamic_title", "replace"),
    );
    dynamic.set_tooltip_text(Some(
        "How a title set by the shell (OSC escape) is combined with the initial title.",
    ));
    page.row_keyed("Dynamic title", &dynamic, Some("dynamic_title"));

    page.section("Shell");
    let login = gtk::CheckButton::new();
    login.set_active(s.get_bool("login_shell"));
    login.set_tooltip_text(Some(
        "Start the shell as a login shell, so .profile and .bash_profile are loaded.",
    ));
    page.row_keyed("Login shell", &login, Some("login_shell"));

    let shell = gtk::Entry::new();
    shell.set_text(&s.get_str_default("shell_command", "/bin/bash"));
    shell.set_placeholder_text(Some("$SHELL"));
    shell.set_tooltip_text(Some(
        "Custom shell command or path. Use $SHELL for the default shell.",
    ));
    page.row_keyed("Shell", &shell, Some("shell_command"));

    page.section("Window");
    let size_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let columns = gtk::SpinButton::with_range(40.0, 300.0, 1.0);
    columns.set_value(s.get_i64("terminal_columns") as f64);
    columns.set_tooltip_text(Some(
        "Width of a new terminal in character columns (40-300).",
    ));
    let rows = gtk::SpinButton::with_range(10.0, 120.0, 1.0);
    rows.set_value(s.get_i64("terminal_rows") as f64);
    rows.set_tooltip_text(Some("Height of a new terminal in character rows (10-120)."));
    size_box.pack_start(&columns, false, false, 0);
    size_box.pack_start(&gtk::Label::new(Some("x")), false, false, 2);
    size_box.pack_start(&rows, false, false, 0);
    size_box.set_tooltip_text(Some(
        "Default size of a new terminal window, in characters.",
    ));
    page.row_keyed("Size (cols x rows)", &size_box, Some("terminal_columns"));

    page.section("Session");
    let session_restore = gtk::CheckButton::new();
    session_restore.set_active(s.get_bool("session_restore"));
    session_restore.set_tooltip_text(Some(
        "Reopen the tabs and splits from the last session when Oxterm starts.",
    ));
    page.row_keyed(
        "Restore last session",
        &session_restore,
        Some("session_restore"),
    );

    let ssh_sharing = gtk::CheckButton::new();
    ssh_sharing.set_active(s.get_bool("ssh_connection_sharing"));
    ssh_sharing.set_tooltip_text(Some(
        "Reuse one SSH connection per server across tabs, for faster logins and live \
         remote stats. Ignored when your ssh config sets ControlPath. Applies to new tabs.",
    ));
    page.row_keyed(
        "Share SSH connections",
        &ssh_sharing,
        Some("ssh_connection_sharing"),
    );

    page.section("Behaviour");
    let confirm_close = gtk::CheckButton::new();
    confirm_close.set_active(s.get_bool("confirm_close"));
    confirm_close.set_tooltip_text(Some(
        "Ask for confirmation before closing a terminal that is still running a command.",
    ));
    page.row_keyed("Confirm close", &confirm_close, Some("confirm_close"));

    let auto_copy = gtk::CheckButton::new();
    auto_copy.set_active(s.get_bool("auto_copy_selection"));
    auto_copy.set_tooltip_text(Some("Copy selected text to the clipboard automatically."));
    page.row_keyed(
        "Auto-copy selection",
        &auto_copy,
        Some("auto_copy_selection"),
    );

    let warn_paste = gtk::CheckButton::new();
    warn_paste.set_active(s.get_bool("show_unsafe_paste_dialog"));
    warn_paste.set_tooltip_text(Some(
        "Warn before pasting multi-line text that may contain harmful commands.",
    ));
    page.row_keyed(
        "Warn on multi-line paste",
        &warn_paste,
        Some("show_unsafe_paste_dialog"),
    );

    let file_manager = gtk::Entry::new();
    file_manager.set_text(&s.get_str("file_manager"));
    file_manager.set_placeholder_text(Some("auto-detect"));
    file_manager.set_tooltip_text(Some(
        "Override the file manager used by the Open-folder action (nemo, thunar, dolphin...). \
         Leave blank to auto-detect.",
    ));
    page.row_keyed("File manager", &file_manager, Some("file_manager"));

    let state = GeneralPage {
        title,
        dynamic,
        login,
        shell,
        columns,
        rows,
        session_restore,
        ssh_sharing,
        confirm_close,
        auto_copy,
        warn_paste,
        file_manager,
    };
    (page.finish(), state)
}

struct ScrollingPage {
    scrollbar_position: gtk::ComboBoxText,
    scrollback_unlimited: gtk::CheckButton,
    scrollback: gtk::SpinButton,
    scroll_on_output: gtk::CheckButton,
    scroll_on_keystroke: gtk::CheckButton,
    smooth_scroll: gtk::CheckButton,
}

fn build_scrolling(s: &Settings, index: &Rc<FilterIndex>) -> (gtk::ScrolledWindow, ScrollingPage) {
    let page = Page::new("scrolling", "Scrolling", index.clone());

    page.section("Scrollbar");
    let scrollbar_position = combo_for(
        SCROLLBAR_POSITION,
        &s.get_str_default("scrollbar_position", "right"),
    );
    scrollbar_position.set_tooltip_text(Some(
        "Where to draw the scrollbar, or hide it entirely (the mouse wheel still works).",
    ));
    page.row_keyed(
        "Scrollbar position",
        &scrollbar_position,
        Some("scrollbar_position"),
    );

    page.section("Buffer");
    let scrollback_unlimited = gtk::CheckButton::new();
    scrollback_unlimited.set_active(s.get_i64("scrollback_lines") == -1);
    scrollback_unlimited.set_tooltip_text(Some(
        "Keep every line for the whole session. Memory use grows with the output.",
    ));
    page.row_keyed(
        "Unlimited scrollback",
        &scrollback_unlimited,
        Some("scrollback_lines"),
    );

    let scrollback = gtk::SpinButton::with_range(1.0, 1_000_000.0, 100.0);
    let saved_scrollback = s.get_i64("scrollback_lines");
    scrollback.set_value(if saved_scrollback > 0 {
        saved_scrollback as f64
    } else {
        10_000.0
    });
    scrollback.set_tooltip_text(Some(
        "Maximum number of lines kept above the visible screen.",
    ));
    page.row_keyed("Scrollback lines", &scrollback, Some("scrollback_lines"));
    scrollback.set_sensitive(!scrollback_unlimited.is_active());
    {
        let spin = scrollback.clone();
        scrollback_unlimited.connect_toggled(move |check| {
            spin.set_sensitive(!check.is_active());
        });
    }

    page.section("Auto-scroll");
    let scroll_on_output = gtk::CheckButton::new();
    scroll_on_output.set_active(s.get_bool("scroll_on_output"));
    scroll_on_output.set_tooltip_text(Some(
        "Jump to the bottom when new output arrives, even while reading older lines.",
    ));
    page.row_keyed(
        "Scroll on output",
        &scroll_on_output,
        Some("scroll_on_output"),
    );

    let scroll_on_keystroke = gtk::CheckButton::new();
    scroll_on_keystroke.set_active(s.get_bool("scroll_on_keystroke"));
    scroll_on_keystroke.set_tooltip_text(Some("Jump to the bottom as soon as you type."));
    page.row_keyed(
        "Scroll on keystroke",
        &scroll_on_keystroke,
        Some("scroll_on_keystroke"),
    );

    let smooth_scroll = gtk::CheckButton::new();
    smooth_scroll.set_active(s.get_bool("smooth_scroll"));
    smooth_scroll.set_tooltip_text(Some("Scroll by pixels instead of whole lines."));
    page.row_keyed("Smooth scrolling", &smooth_scroll, Some("smooth_scroll"));

    let state = ScrollingPage {
        scrollbar_position,
        scrollback_unlimited,
        scrollback,
        scroll_on_output,
        scroll_on_keystroke,
        smooth_scroll,
    };
    (page.finish(), state)
}

struct InputPage {
    hint_mode: gtk::CheckButton,
    vi_copy: gtk::CheckButton,
    tmux_prefix: gtk::CheckButton,
    tmux_prefix_key: gtk::Entry,
    backspace: gtk::ComboBoxText,
    delete: gtk::ComboBoxText,
    word_chars: gtk::Entry,
}

fn build_input(s: &Settings, index: &Rc<FilterIndex>) -> (gtk::ScrolledWindow, InputPage) {
    let page = Page::new("input", "Input", index.clone());

    page.section("Keyboard modes");
    let hint_mode = gtk::CheckButton::new();
    hint_mode.set_active(s.get_bool("hint_mode_enabled"));
    hint_mode.set_tooltip_text(Some(
        "Ctrl+Shift+H highlights URLs, paths and git hashes with keyboard-selectable labels.",
    ));
    page.row_keyed("Hint mode", &hint_mode, Some("hint_mode_enabled"));

    let vi_copy = gtk::CheckButton::new();
    vi_copy.set_active(s.get_bool("vi_copy_mode_enabled"));
    vi_copy.set_tooltip_text(Some(
        "Ctrl+Shift+Y opens a VI-style copy mode: hjkl to move, v to select, y to yank, Esc to exit.",
    ));
    page.row_keyed("VI copy mode", &vi_copy, Some("vi_copy_mode_enabled"));

    let tmux_prefix = gtk::CheckButton::new();
    tmux_prefix.set_active(s.get_bool("tmux_prefix_enabled"));
    tmux_prefix.set_tooltip_text(Some(
        "Use a Ctrl+letter prefix for tab and split shortcuts. Disable it to pass that key \
         through to programs such as tmux or screen.",
    ));
    page.row_keyed(
        "tmux-style prefix keys",
        &tmux_prefix,
        Some("tmux_prefix_enabled"),
    );

    let tmux_prefix_key = gtk::Entry::new();
    tmux_prefix_key.set_text(&s.get_str_default("tmux_prefix_key", "b"));
    tmux_prefix_key.set_max_length(1);
    tmux_prefix_key.set_width_chars(3);
    tmux_prefix_key.set_max_width_chars(3);
    tmux_prefix_key.set_sensitive(tmux_prefix.is_active());
    tmux_prefix_key.set_tooltip_text(Some(
        "Letter pressed with Ctrl as the prefix (default b). Change it to keep Ctrl+B for tmux.",
    ));
    page.row_keyed(
        "Prefix key (Ctrl+)",
        &tmux_prefix_key,
        Some("tmux_prefix_key"),
    );
    tmux_prefix_key.set_hexpand(false);
    {
        let entry = tmux_prefix_key.clone();
        tmux_prefix.connect_toggled(move |check| entry.set_sensitive(check.is_active()));
    }

    page.section("Key bindings");
    let backspace = combo_for(
        BACKSPACE_BINDING,
        &s.get_str_default("backspace_binding", "ascii-del"),
    );
    backspace.set_tooltip_text(Some(
        "Sequence sent when Backspace is pressed. Auto-detect matches most setups.",
    ));
    page.row_keyed("Backspace sends", &backspace, Some("backspace_binding"));

    let delete = combo_for(
        DELETE_BINDING,
        &s.get_str_default("delete_binding", "escape-sequence"),
    );
    delete.set_tooltip_text(Some("Sequence sent when the Delete key is pressed."));
    page.row_keyed("Delete sends", &delete, Some("delete_binding"));

    page.section("Selection");
    let word_chars = gtk::Entry::new();
    word_chars.set_text(&s.get_str_default("word_char_exceptions", "-./?%&#:_=+@~"));
    word_chars.set_tooltip_text(Some(
        "Extra characters treated as part of a word by double-click selection, \
         so that paths and URLs select in one go.",
    ));
    page.row_keyed("Word characters", &word_chars, Some("word_char_exceptions"));

    let state = InputPage {
        hint_mode,
        vi_copy,
        tmux_prefix,
        tmux_prefix_key,
        backspace,
        delete,
        word_chars,
    };
    (page.finish(), state)
}

struct NotificationsPage {
    notify: gtk::CheckButton,
    only_unfocused: gtk::CheckButton,
    min_seconds: gtk::SpinButton,
    notify_failure: gtk::CheckButton,
}

fn build_notifications(
    s: &Settings,
    index: &Rc<FilterIndex>,
) -> (gtk::ScrolledWindow, NotificationsPage) {
    let page = Page::new("notifications", "Notifications", index.clone());

    page.section("Command notifications");
    page.note(
        "Desktop notifications use OSC 133 shell integration, which Oxterm enables in the \
         shells it starts. Failure notifications also work with the command markers Oxterm \
         adds itself.",
    );

    let notify = gtk::CheckButton::new();
    notify.set_active(s.get_bool("bell_notification"));
    notify.set_tooltip_text(Some("Show a desktop notification when a command finishes."));
    page.row_keyed(
        "Notify when a command finishes",
        &notify,
        Some("bell_notification"),
    );

    let only_unfocused = gtk::CheckButton::new();
    only_unfocused.set_active(s.get_bool("notify_only_unfocused"));
    only_unfocused.set_tooltip_text(Some(
        "Stay quiet while you are watching the terminal; notify only when it is in the background.",
    ));
    page.row_keyed(
        "Only when unfocused",
        &only_unfocused,
        Some("notify_only_unfocused"),
    );

    let min_seconds = gtk::SpinButton::with_range(0.0, 86_400.0, 1.0);
    min_seconds.set_value(s.get_i64("notify_min_seconds") as f64);
    min_seconds.set_tooltip_text(Some(
        "Skip notifications for commands shorter than this. Zero notifies for every command; \
         failures always notify when enabled below.",
    ));
    page.row_keyed(
        "Minimum duration (s)",
        &min_seconds,
        Some("notify_min_seconds"),
    );

    let notify_failure = gtk::CheckButton::new();
    notify_failure.set_active(s.get_bool("notify_on_failure"));
    notify_failure.set_tooltip_text(Some(
        "Notify when a command exits non-zero even if it was faster than the minimum duration.",
    ));
    page.row_keyed(
        "Always notify failures",
        &notify_failure,
        Some("notify_on_failure"),
    );

    // The detailed options only make sense while notifications are enabled.
    let dependent: Vec<gtk::Widget> = vec![
        only_unfocused.clone().upcast(),
        min_seconds.clone().upcast(),
        notify_failure.clone().upcast(),
    ];
    for widget in &dependent {
        widget.set_sensitive(notify.is_active());
    }
    notify.connect_toggled(move |check| {
        for widget in &dependent {
            widget.set_sensitive(check.is_active());
        }
    });

    let state = NotificationsPage {
        notify,
        only_unfocused,
        min_seconds,
        notify_failure,
    };
    (page.finish(), state)
}

struct AppearancePage {
    font: gtk::FontButton,
    bold: gtk::CheckButton,
    font_rendering: gtk::ComboBoxText,
    font_hinting: gtk::ComboBoxText,
    cell_height: gtk::SpinButton,
    cell_width: gtk::SpinButton,
    text_shaping: gtk::CheckButton,
    scheme: gtk::ComboBoxText,
    auto_theme: gtk::CheckButton,
    light_theme: gtk::ComboBoxText,
    dark_theme: gtk::ComboBoxText,
    fg_color: gtk::ColorButton,
    bg_color: gtk::ColorButton,
    cursor_color: gtk::ColorButton,
    highlight: gtk::ColorButton,
    highlight_bg: gtk::ColorButton,
    tab_title_color: gtk::ColorButton,
    tab_active_color: gtk::ColorButton,
    cursor_shape: gtk::ComboBoxText,
    cursor_blink: gtk::CheckButton,
    transparency: gtk::CheckButton,
    opacity: gtk::SpinButton,
    pad_h: gtk::SpinButton,
    pad_v: gtk::SpinButton,
    undercurl: gtk::ComboBoxText,
}

type PaletteButtons = Rc<BTreeMap<String, (gtk::ColorButton, gtk::Label)>>;

fn apply_palette(
    buttons: &BTreeMap<String, (gtk::ColorButton, gtk::Label)>,
    palette: &BTreeMap<String, String>,
) {
    for (key, (button, label)) in buttons {
        if let Some(hex) = palette.get(key) {
            button.set_rgba(&hex_to_rgba(hex));
            label.set_text(hex);
        }
    }
}

fn build_appearance(
    s: &Settings,
    index: &Rc<FilterIndex>,
    palette_buttons: &PaletteButtons,
    scheme_name: &Rc<RefCell<String>>,
) -> (gtk::ScrolledWindow, AppearancePage) {
    let page = Page::new("appearance", "Appearance", index.clone());

    page.section("Font");
    let font_desc = format!(
        "{} {}",
        s.get_str_default("font_name", "Monospace"),
        s.get_i64("font_size")
    );
    let font = gtk::FontButton::new();
    font.set_font(&font_desc);
    font.set_tooltip_text(Some("Font family and size used by the terminal."));
    page.row("Font", &font);

    let bold = gtk::CheckButton::new();
    bold.set_active(s.get_bool("allow_bold_text"));
    bold.set_tooltip_text(Some(
        "Render bold text with a heavier weight. Disable for uniform text.",
    ));
    page.row_keyed("Allow bold text", &bold, Some("allow_bold_text"));

    let font_rendering = combo_for(
        FONT_RENDERING,
        &s.get_str_default("font_rendering", "system"),
    );
    font_rendering.set_tooltip_text(Some(
        "Antialiasing for glyphs. Subpixel is sharpest on an RGB-ordered LCD panel.",
    ));
    page.row_keyed("Antialiasing", &font_rendering, Some("font_rendering"));

    let font_hinting = combo_for(FONT_HINTING, &s.get_str_default("font_hinting", "system"));
    font_hinting.set_tooltip_text(Some(
        "How strongly glyphs snap to the pixel grid. Slight or none looks smoother at large sizes.",
    ));
    page.row_keyed("Hinting", &font_hinting, Some("font_hinting"));

    let cell_height = scale_spin(s.get_f64("cell_height_scale"));
    cell_height.set_tooltip_text(Some(
        "Vertical cell size multiplier; above 1.0 adds air between the lines.",
    ));
    page.row_keyed("Line spacing", &cell_height, Some("cell_height_scale"));

    let cell_width = scale_spin(s.get_f64("cell_width_scale"));
    cell_width.set_tooltip_text(Some(
        "Horizontal cell size multiplier; above 1.0 adds letter spacing.",
    ));
    page.row_keyed("Letter spacing", &cell_width, Some("cell_width_scale"));

    let text_shaping = gtk::CheckButton::new();
    text_shaping.set_active(s.get_bool("text_shaping"));
    text_shaping.set_tooltip_text(Some(
        "Shape text with HarfBuzz and enable ligatures. Disable it for a faster grid \
         when only ASCII output is used.",
    ));
    page.row_keyed("Text shaping", &text_shaping, Some("text_shaping"));

    page.section("Theme");
    let scheme = gtk::ComboBoxText::new();
    for name in settings::color_schemes().keys() {
        scheme.append_text(name);
    }
    let current = s.get_str_default("color_scheme", "Dark (Default)");
    let names: Vec<String> = settings::color_schemes().keys().cloned().collect();
    let scheme_index = names.iter().position(|name| *name == current).unwrap_or(0);
    scheme.set_active(Some(scheme_index as u32));
    scheme.set_tooltip_text(Some(
        "Color scheme for the terminal. Drop Ghostty or iTerm2 theme files into \
         ~/.config/oxterm/themes/ to add more.",
    ));
    page.row_keyed("Color scheme", &scheme, Some("color_scheme"));

    let auto_theme = gtk::CheckButton::new();
    auto_theme.set_active(s.get_bool("auto_theme"));
    auto_theme.set_tooltip_text(Some(
        "Switch between the light and dark theme below when the desktop changes appearance.",
    ));
    page.row_keyed("Follow system dark mode", &auto_theme, Some("auto_theme"));

    let light_theme = theme_combo(&s.get_str_default("theme_light", "Light"));
    light_theme.set_tooltip_text(Some(
        "Theme used when the desktop prefers a light appearance.",
    ));
    page.row_keyed("Light theme", &light_theme, Some("theme_light"));

    let dark_theme = theme_combo(&s.get_str_default("theme_dark", "Dark (Default)"));
    dark_theme.set_tooltip_text(Some(
        "Theme used when the desktop prefers a dark appearance.",
    ));
    page.row_keyed("Dark theme", &dark_theme, Some("theme_dark"));

    for combo in [&light_theme, &dark_theme] {
        combo.set_sensitive(auto_theme.is_active());
    }
    {
        let light = light_theme.clone();
        let dark = dark_theme.clone();
        auto_theme.connect_toggled(move |check| {
            light.set_sensitive(check.is_active());
            dark.set_sensitive(check.is_active());
        });
    }

    page.section("Colors");
    let fg_color = make_color_button(
        &s.get_fg_color(),
        "Default text color. Click to override the theme foreground.",
    );
    page.row_keyed("Foreground", &fg_color, Some("foreground_color"));
    let bg_color = make_color_button(
        &s.get_bg_color(),
        "Terminal background. Click to override the theme background.",
    );
    page.row_keyed("Background", &bg_color, Some("background_color"));
    let cursor_color = make_color_button(
        &s.get_str_default("cursor_color", "#ffffff"),
        "Color of the text cursor.",
    );
    page.row_keyed("Cursor", &cursor_color, Some("cursor_color"));
    let highlight = make_color_button(
        &s.get_str_default("highlight_color", "#ffffff"),
        "Text color of the current selection.",
    );
    page.row_keyed("Selection text", &highlight, Some("highlight_color"));
    let highlight_bg = make_color_button(
        &s.get_str_default("highlight_bg_color", "#446688"),
        "Background color of the current selection.",
    );
    page.row_keyed(
        "Selection background",
        &highlight_bg,
        Some("highlight_bg_color"),
    );

    page.section("Tabs");
    let tab_title_color = make_color_button(
        &s.get_str_default("tab_title_color", "#ffffff"),
        "Color of inactive tab titles.",
    );
    page.row_keyed("Tab title", &tab_title_color, Some("tab_title_color"));
    let tab_active_color = make_color_button(
        &s.get_str_default("tab_active_title_color", "#ffffff"),
        "Color of the active tab title.",
    );
    page.row_keyed(
        "Active tab title",
        &tab_active_color,
        Some("tab_active_title_color"),
    );

    page.section("Cursor");
    let cursor_shape = combo_for(CURSOR_SHAPE, &s.get_str_default("cursor_shape", "block"));
    cursor_shape.set_tooltip_text(Some("Shape of the text cursor."));
    page.row_keyed("Cursor shape", &cursor_shape, Some("cursor_shape"));

    let cursor_blink = gtk::CheckButton::new();
    cursor_blink.set_active(s.get_bool("cursor_blink"));
    cursor_blink.set_tooltip_text(Some("Make the cursor blink periodically."));
    page.row_keyed("Blinking cursor", &cursor_blink, Some("cursor_blink"));

    page.section("Window");
    let transparency = gtk::CheckButton::new();
    transparency.set_active(s.get_bool("enable_transparency"));
    transparency.set_tooltip_text(Some(
        "Enable RGBA transparency. Requires a compositor such as picom or a Wayland desktop.",
    ));
    page.row_keyed("Transparency", &transparency, Some("enable_transparency"));

    let opacity = gtk::SpinButton::with_range(0.1, 1.0, 0.05);
    opacity.set_digits(2);
    opacity.set_value(s.get_f64("opacity"));
    opacity.set_tooltip_text(Some(
        "Window opacity: 1.00 is fully opaque, 0.30 is almost transparent.",
    ));
    page.row_keyed("Opacity", &opacity, Some("opacity"));
    opacity.set_sensitive(transparency.is_active());
    {
        let spin = opacity.clone();
        transparency.connect_toggled(move |check| spin.set_sensitive(check.is_active()));
    }

    let pad_h = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    pad_h.set_value(s.get_i64("window_padding_horizontal") as f64);
    pad_h.set_tooltip_text(Some(
        "Horizontal padding inside the terminal window, in pixels.",
    ));
    page.row_keyed(
        "Horizontal padding",
        &pad_h,
        Some("window_padding_horizontal"),
    );
    let pad_v = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    pad_v.set_value(s.get_i64("window_padding_vertical") as f64);
    pad_v.set_tooltip_text(Some(
        "Vertical padding inside the terminal window, in pixels.",
    ));
    page.row_keyed("Vertical padding", &pad_v, Some("window_padding_vertical"));

    page.section("Underlines");
    let undercurl = combo_for(
        UNDERLINE_STYLE,
        &s.get_str_default("undercurl_style", "single"),
    );
    undercurl.set_tooltip_text(Some(
        "Style of underlined text, for example compiler errors and spell check. \
         Curly requires VTE 0.58 or newer.",
    ));
    page.row_keyed("Underline style", &undercurl, Some("undercurl_style"));

    // Live preview: picking a scheme previews its foreground, background and
    // palette without touching the settings file.
    {
        let fg = fg_color.clone();
        let bg = bg_color.clone();
        let buttons = palette_buttons.clone();
        let scheme_name = scheme_name.clone();
        scheme.connect_changed(move |combo| {
            let Some(name) = combo.active_text() else {
                return;
            };
            *scheme_name.borrow_mut() = name.to_string();
            if let Some(theme) = settings::color_schemes().get(name.as_str()) {
                fg.set_rgba(&hex_to_rgba(&theme.foreground));
                bg.set_rgba(&hex_to_rgba(&theme.background));
                apply_palette(&buttons, &theme.palette);
            }
        });
    }

    let state = AppearancePage {
        font,
        bold,
        font_rendering,
        font_hinting,
        cell_height,
        cell_width,
        text_shaping,
        scheme,
        auto_theme,
        light_theme,
        dark_theme,
        fg_color,
        bg_color,
        cursor_color,
        highlight,
        highlight_bg,
        tab_title_color,
        tab_active_color,
        cursor_shape,
        cursor_blink,
        transparency,
        opacity,
        pad_h,
        pad_v,
        undercurl,
    };
    (page.finish(), state)
}

const PALETTE_NAMES: [[(&str, &str); 4]; 4] = [
    [
        ("black", "Black"),
        ("red", "Red"),
        ("green", "Green"),
        ("yellow", "Yellow"),
    ],
    [
        ("blue", "Blue"),
        ("magenta", "Magenta"),
        ("cyan", "Cyan"),
        ("white", "White"),
    ],
    [
        ("brightblack", "Bright black"),
        ("brightred", "Bright red"),
        ("brightgreen", "Bright green"),
        ("brightyellow", "Bright yellow"),
    ],
    [
        ("brightblue", "Bright blue"),
        ("brightmagenta", "Bright magenta"),
        ("brightcyan", "Bright cyan"),
        ("brightwhite", "Bright white"),
    ],
];

fn build_colors(
    s: &Settings,
    index: &Rc<FilterIndex>,
    scheme_name: &Rc<RefCell<String>>,
) -> (gtk::ScrolledWindow, PaletteButtons) {
    let page = Page::new("colors", "Colors", index.clone());
    page.section("16-color palette");
    page.note(
        "These colors override the selected theme. Loading a preset or resetting the page only \
         changes what you see here: nothing is saved until you press OK.",
    );

    let saved = s.get("custom_palette");
    let theme = s.effective_theme();

    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(18);
    grid.set_column_homogeneous(true);
    grid.set_halign(gtk::Align::Start);
    let mut buttons: BTreeMap<String, (gtk::ColorButton, gtk::Label)> = BTreeMap::new();
    for (row_index, row) in PALETTE_NAMES.iter().enumerate() {
        for (column_index, (key, name)) in row.iter().enumerate() {
            let hex = saved
                .as_object()
                .and_then(|custom| custom.get(*key))
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .or_else(|| theme.palette.get(*key).cloned())
                .unwrap_or_else(|| "#000000".to_string());
            let button = make_color_button(&hex, &format!("ANSI color: {}", name));
            button.set_size_request(64, 26);
            let hex_label = gtk::Label::new(Some(&hex));
            hex_label.style_context().add_class("dim-label");
            hex_label.set_halign(gtk::Align::Start);
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 3);
            cell.pack_start(&gtk::Label::new(Some(name)), false, false, 0);
            cell.pack_start(&button, false, false, 0);
            cell.pack_start(&hex_label, false, false, 0);
            grid.attach(&cell, column_index as i32, row_index as i32, 1, 1);
            buttons.insert(key.to_string(), (button, hex_label));
        }
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let load = gtk::Button::with_label("Load Preset...");
    load.set_tooltip_text(Some(
        "Fill the palette from any installed or imported theme.",
    ));
    {
        let buttons = buttons.clone();
        load.connect_clicked(move |button| {
            let parent = button
                .toplevel()
                .and_then(|window| window.downcast::<gtk::Window>().ok());
            let Some(name) = choose_theme(parent.as_ref(), "Load Palette") else {
                return;
            };
            if let Some(theme) = settings::color_schemes().get(name.as_str()) {
                apply_palette(&buttons, &theme.palette);
            }
        });
    }

    let reset = gtk::Button::with_label("Reset to Theme");
    reset.set_tooltip_text(Some(
        "Discard the custom palette and use the colors of the selected theme.",
    ));
    {
        let buttons = buttons.clone();
        let scheme_name = scheme_name.clone();
        reset.connect_clicked(move |_| {
            let name = scheme_name.borrow().clone();
            if let Some(theme) = settings::color_schemes().get(name.as_str()) {
                apply_palette(&buttons, &theme.palette);
            }
        });
    }

    actions.pack_start(&load, false, false, 0);
    actions.pack_start(&reset, false, false, 0);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.pack_start(&grid, false, false, 0);
    content.pack_start(&actions, false, false, 0);
    page.full(&content);
    page.register(
        vec![content.upcast()],
        "palette colors ansi normal bright custom_palette",
    );

    (page.finish(), Rc::new(buttons))
}

struct CompatibilityPage {
    encoding: gtk::ComboBoxText,
    bold_is_bright: gtk::CheckButton,
    sixel: gtk::CheckButton,
    osc133: gtk::CheckButton,
}

fn build_compatibility(
    s: &Settings,
    index: &Rc<FilterIndex>,
) -> (gtk::ScrolledWindow, CompatibilityPage) {
    let page = Page::new("compatibility", "Compatibility", index.clone());

    page.section("Encoding");
    let encoding = encoding_combo(&s.get_str_default("encoding", "UTF-8"));
    encoding.set_tooltip_text(Some("Character encoding used by new terminal tabs."));
    page.row_keyed("Default encoding", &encoding, Some("encoding"));

    page.section("Rendering");
    let bold_is_bright = gtk::CheckButton::new();
    bold_is_bright.set_active(s.get_bool("bold_is_bright"));
    bold_is_bright.set_tooltip_text(Some(
        "Match xterm and most Linux terminals: bold text uses the bright palette color.",
    ));
    page.row_keyed(
        "Bold text uses bright colors",
        &bold_is_bright,
        Some("bold_is_bright"),
    );

    let sixel = gtk::CheckButton::new();
    sixel.set_active(s.get_bool("sixel"));
    sixel.set_tooltip_text(Some(
        "Let applications draw inline images with Sixel (lsix, chafa, img2sixel). \
         Off by default because images consume memory.",
    ));
    page.row_keyed("Sixel images", &sixel, Some("sixel"));

    page.section("Integration");
    let osc133 = gtk::CheckButton::new();
    osc133.set_active(s.get_bool("osc133"));
    osc133.set_tooltip_text(Some(
        "Enable shell integration (OSC 133) in shells started by Oxterm. Your shell \
         configuration files are never modified.",
    ));
    page.row_keyed("Shell integration (OSC 133)", &osc133, Some("osc133"));

    let reset = gtk::Button::with_label("Reset this page to defaults");
    reset.set_tooltip_text(Some(
        "Restore only the compatibility settings listed above.",
    ));
    reset.set_halign(gtk::Align::Start);
    let page_state = CompatibilityPage {
        encoding: encoding.clone(),
        bold_is_bright: bold_is_bright.clone(),
        sixel: sixel.clone(),
        osc133: osc133.clone(),
    };
    {
        let encoding = page_state.encoding.clone();
        let bold_is_bright = page_state.bold_is_bright.clone();
        let sixel = page_state.sixel.clone();
        let osc133 = page_state.osc133.clone();
        reset.connect_clicked(move |_| {
            encoding.set_active(Some(
                ENCODINGS
                    .iter()
                    .position(|name| *name == default_str("encoding"))
                    .unwrap_or(0) as u32,
            ));
            bold_is_bright.set_active(default_bool("bold_is_bright"));
            sixel.set_active(default_bool("sixel"));
            osc133.set_active(default_bool("osc133"));
        });
    }
    page.extra(&reset);

    (page.finish(), page_state)
}

struct NotesPage {
    notes_dir: gtk::Entry,
    notes_file: gtk::Entry,
    editor: gtk::Entry,
    record_context: gtk::CheckButton,
    template: gtk::Entry,
}

fn build_notes(s: &Settings, index: &Rc<FilterIndex>) -> (gtk::ScrolledWindow, NotesPage) {
    let page = Page::new("notes", "Notes", index.clone());

    page.section("Storage");
    let notes_dir = gtk::Entry::new();
    notes_dir.set_text(&s.get_str("notes_dir"));
    let default_dir = crate::notes::default_notes_dir();
    notes_dir.set_placeholder_text(Some(default_dir.to_string_lossy().as_ref()));
    notes_dir.set_tooltip_text(Some(
        "Directory where note files are stored. Leave empty to use the default data directory.",
    ));
    page.row_keyed("Notes directory", &notes_dir, Some("notes_dir"));

    let notes_file = gtk::Entry::new();
    notes_file.set_text(&s.get_str("notes_file"));
    notes_file.set_placeholder_text(Some("notes.md"));
    notes_file.set_tooltip_text(Some(
        "Default notes file, created the first time you use /wnotes.",
    ));
    page.row_keyed("Default notes file", &notes_file, Some("notes_file"));

    let editor = gtk::Entry::new();
    editor.set_text(&s.get_str_default("editor_command", "nano"));
    editor.set_tooltip_text(Some(
        "Fallback editor for /onotes, used only when xdg-open is unavailable.",
    ));
    page.row_keyed("Fallback editor", &editor, Some("editor_command"));

    page.section("New notes");
    let record_context = gtk::CheckButton::new();
    record_context.set_active(s.get_bool("notes_record_context"));
    record_context.set_tooltip_text(Some(
        "Add a small HTML comment with the source, working directory and host to each entry.",
    ));
    page.row_keyed(
        "Record context",
        &record_context,
        Some("notes_record_context"),
    );

    let template = gtk::Entry::new();
    template.set_text(&s.get_str("notes_template"));
    template.set_placeholder_text(Some("e.g. Task: {command} (cwd {cwd})"));
    template.set_tooltip_text(Some(
        "Template pre-filled in new notes. Placeholders: {date} {time} {cwd} {host} {command}.",
    ));
    page.row_keyed("Template", &template, Some("notes_template"));

    let state = NotesPage {
        notes_dir,
        notes_file,
        editor,
        record_context,
        template,
    };
    (page.finish(), state)
}

// -- Dialog -----------------------------------------------------------------

struct DialogState {
    general: GeneralPage,
    scrolling: ScrollingPage,
    input: InputPage,
    notifications: NotificationsPage,
    appearance: AppearancePage,
    compatibility: CompatibilityPage,
    notes: NotesPage,
    palette_btns: PaletteButtons,
    fg_override: Rc<Cell<bool>>,
    bg_override: Rc<Cell<bool>>,
}

pub fn show_settings_dialog(parent: Option<&gtk::Window>) {
    let s = settings();
    let dialog = gtk::Dialog::new();
    dialog.set_title("Preferences - Oxterm");
    dialog.set_transient_for(parent);
    dialog.set_modal(true);
    dialog.set_destroy_with_parent(true);
    dialog.set_default_size(960, 720);

    let index = Rc::new(FilterIndex::default());
    let scheme_name = Rc::new(RefCell::new(
        s.get_str_default("color_scheme", "Dark (Default)"),
    ));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);

    let (colors_widget, palette_btns) = build_colors(&s, &index, &scheme_name);
    let (general_widget, general) = build_general(&s, &index);
    let (scrolling_widget, scrolling) = build_scrolling(&s, &index);
    let (input_widget, input) = build_input(&s, &index);
    let (notifications_widget, notifications) = build_notifications(&s, &index);
    let (appearance_widget, appearance) = build_appearance(&s, &index, &palette_btns, &scheme_name);
    let (compat_widget, compatibility) = build_compatibility(&s, &index);
    let (notes_widget, notes) = build_notes(&s, &index);

    stack.add_titled(&general_widget, "general", "General");
    stack.add_titled(&appearance_widget, "appearance", "Appearance");
    stack.add_titled(&colors_widget, "colors", "Colors");
    stack.add_titled(&scrolling_widget, "scrolling", "Scrolling");
    stack.add_titled(&input_widget, "input", "Input");
    stack.add_titled(&notifications_widget, "notifications", "Notifications");
    stack.add_titled(&compat_widget, "compatibility", "Compatibility");
    stack.add_titled(&notes_widget, "notes", "Notes");

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(190, -1);
    // The page list takes the initial focus so the filter box keeps showing
    // its placeholder; Ctrl+F jumps to the search field.
    sidebar.set_can_focus(true);

    // Search bar: filters every registered setting and jumps to a page that
    // still has matches.
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search settings..."));
    search.set_tooltip_text(Some("Type to filter every page (Ctrl+F)."));
    search.set_hexpand(true);
    let status = gtk::Label::new(None);
    status.style_context().add_class("dim-label");
    status.set_xalign(1.0);
    let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    search_bar.pack_start(&search, true, true, 0);
    search_bar.pack_start(&status, false, false, 0);
    search_bar.set_margin_start(14);
    search_bar.set_margin_end(14);
    search_bar.set_margin_top(8);
    search_bar.set_margin_bottom(8);

    {
        let index = index.clone();
        let stack = stack.clone();
        let status = status.clone();
        search.connect_search_changed(move |entry| {
            let query = entry.text().trim().to_lowercase();
            let visible: Vec<bool> = index
                .rows
                .borrow()
                .iter()
                .map(|row| query.is_empty() || row.haystack.contains(&query))
                .collect();

            {
                let rows = index.rows.borrow();
                for (row, shown) in rows.iter().zip(&visible) {
                    for widget in &row.widgets {
                        widget.set_visible(*shown);
                    }
                }
            }
            for section in index.sections.borrow().iter() {
                let any = section
                    .rows
                    .iter()
                    .any(|index| visible.get(*index).copied().unwrap_or(false));
                section.header.set_visible(any);
                for extra in &section.extras {
                    extra.set_visible(any);
                }
            }

            if query.is_empty() {
                status.set_text("");
                return;
            }

            let mut pages: Vec<&str> = Vec::new();
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            {
                let rows = index.rows.borrow();
                for (row, shown) in rows.iter().zip(&visible) {
                    if *shown {
                        if !pages.contains(&row.page) {
                            pages.push(row.page);
                        }
                        *counts.entry(row.page).or_insert(0) += 1;
                    }
                }
            }
            let total: usize = counts.values().sum();
            status.set_text(&match total {
                0 => "No matching settings".to_string(),
                1 => "1 setting".to_string(),
                n => format!("{n} settings"),
            });

            let current = stack
                .visible_child_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            if counts.get(current.as_str()).copied().unwrap_or(0) == 0 {
                if let Some(page) = pages.first() {
                    stack.set_visible_child_name(page);
                }
            }
        });
    }
    {
        let search = search.clone();
        search.connect_stop_search(move |entry| entry.set_text(""));
    }
    {
        let search = search.clone();
        dialog.connect_key_press_event(move |_, event| {
            if event.state().contains(gdk::ModifierType::CONTROL_MASK)
                && event.keyval() == gdk::keys::constants::f
            {
                search.grab_focus();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    let content_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content_box.pack_start(&sidebar, false, false, 0);
    content_box.pack_start(
        &gtk::Separator::new(gtk::Orientation::Vertical),
        false,
        false,
        0,
    );
    content_box.pack_start(&stack, true, true, 0);
    dialog
        .content_area()
        .pack_start(&search_bar, false, false, 0);
    dialog.content_area().pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );
    dialog
        .content_area()
        .pack_start(&content_box, true, true, 8);

    dialog.content_area().pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );
    let restore = gtk::Button::with_label("Restore Defaults");
    restore.set_tooltip_text(Some("Reset every page to the documented defaults."));
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("OK");
    ok.style_context().add_class("suggested-action");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.response(gtk::ResponseType::Cancel));
    }
    {
        let dialog = dialog.clone();
        ok.connect_clicked(move |_| dialog.response(gtk::ResponseType::Ok));
    }
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.pack_start(&restore, false, false, 0);
    actions.pack_end(&ok, false, false, 0);
    actions.pack_end(&cancel, false, false, 0);
    actions.set_margin_start(14);
    actions.set_margin_end(14);
    actions.set_margin_top(10);
    actions.set_margin_bottom(10);
    dialog.content_area().pack_start(&actions, false, false, 0);

    let dialog_css = gtk::CssProvider::new();
    let _ = dialog_css.load_from_data(
        b"stacksidebar list row { padding: 10px 12px; margin: 2px 8px; border-radius: 8px; }
           stacksidebar list row label { font-size: 1.05em; }
           stacksidebar list row:selected label { font-weight: bold; }
           entry, spinbutton, combobox button, checkbutton label,
           radiobutton label { font-size: 1.03em; }
           entry, spinbutton { padding: 4px 6px; }
           .settings-caption { font-size: 0.92em; }",
    );
    dialog
        .style_context()
        .add_provider(&dialog_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let state = Rc::new(DialogState {
        general,
        scrolling,
        input,
        notifications,
        appearance,
        compatibility,
        notes,
        palette_btns,
        fg_override: Rc::new(Cell::new(!s.get_str("foreground_color").is_empty())),
        bg_override: Rc::new(Cell::new(!s.get_str("background_color").is_empty())),
    });

    {
        let override_flag = state.fg_override.clone();
        state
            .appearance
            .fg_color
            .connect_color_set(move |_| override_flag.set(true));
        let override_flag = state.bg_override.clone();
        state
            .appearance
            .bg_color
            .connect_color_set(move |_| override_flag.set(true));
    }

    // "Restore Defaults" clears the whole form but does not write anything:
    // pressing Cancel still leaves the configuration untouched.
    {
        let state = state.clone();
        restore.connect_clicked(move |_| state.reset_defaults());
    }

    {
        let state = state.clone();
        dialog.connect_response(move |dlg, response| {
            if response == gtk::ResponseType::Ok {
                let errors = state.apply();
                if !errors.is_empty() {
                    let msg = gtk::MessageDialog::new(
                        Some(dlg),
                        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                        gtk::MessageType::Warning,
                        gtk::ButtonsType::Close,
                        &format!("Some settings were not saved:\n\n{}", errors.join("\n")),
                    );
                    msg.run();
                    msg.close();
                }
            }
            dlg.close();
        });
    }

    dialog.show_all();
    // Keep the filter box empty and quiet: a focused GtkEntry hides its
    // placeholder, so the page list takes the initial focus instead.
    dialog.set_focus(Some(&sidebar));
    let _ = dialog.run();
    dialog.close();
}

pub fn show_about_dialog(parent: Option<&gtk::Window>) {
    let dialog = gtk::AboutDialog::new();
    dialog.set_transient_for(parent);
    dialog.set_modal(true);
    dialog.set_program_name(APP_NAME);
    dialog.set_version(Some(VERSION));
    dialog.set_comments(Some(
        "Native Linux terminal emulator built with Rust, GTK 3 and VTE.\n\nTabs, split panes, command history, shell integration, notes, profiles, named sessions, and SSH-friendly remote attach/detach.\n\nSee the project page for the user manual, releases, and issue tracker.",
    ));
    dialog.set_authors(&["Andres Zanzani"]);
    dialog.set_copyright(Some("Copyright (C) 2026 Andres Zanzani"));
    dialog.set_website(Some(PROJECT_URL));
    dialog.set_website_label(Some("Oxterm project and documentation"));
    dialog.set_license(Some(EUPL_LICENSE_TEXT));
    dialog.set_wrap_license(true);
    let _ = dialog.run();
    dialog.close();
}

pub fn show_help_dialog(parent: Option<&gtk::Window>) {
    let dialog = gtk::Dialog::with_buttons(
        Some("Quick Help - Oxterm"),
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(760, 560);

    let shortcuts = spec::keybinds();
    let text = format!(
        "OXTERM QUICK HELP\n\n\
Terminal workflow\n\
{shortcuts}\
\nBuilt-in commands\n\
  /help              Show the in-terminal command reference\n\
  /history [terms]   Search command history (use -term to exclude)\n\
  /wnotes TEXT       Save a timestamped Markdown note\n\
  /onotes            Open the configured notes file\n\
  /session list      List saved sessions\n\
  /session export    Export a saved session as private JSON\n\
  /snippet list      List saved command snippets\n\
  /clear             Clear the terminal screen\n\
\nRemote sessions\n\
  oxterm --list      List live terminal sessions\n\
  oxterm --info ID   Inspect one live session\n\
  oxterm -a ID       Attach from another terminal or over SSH\n\
  Ctrl+B, then d     Detach local forwarding\n\
\nInspection (no window is opened)\n\
  oxterm --show-config [--default]   Print effective settings as JSON\n\
  oxterm --validate-config [FILE]    Check a settings file\n\
  oxterm --list-settings             List keys, types and defaults\n\
  oxterm --list-themes               List color schemes\n\
  oxterm --list-keybinds             List keyboard shortcuts\n\
  oxterm --list-actions              List control actions\n\
  oxterm --diagnostics               Print a report for bug reports\n\
\nVersion {VERSION}\n\
Manual: {MANUAL_URL}\n\
Project: {PROJECT_URL}\n"
    );

    let text_view = gtk::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    text_view.set_left_margin(14);
    text_view.set_right_margin(14);
    text_view.set_top_margin(12);
    text_view.set_bottom_margin(12);
    if let Some(buffer) = text_view.buffer() {
        buffer.set_text(&text);
    }

    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&text_view);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    dialog.show_all();
    let _ = dialog.run();
    dialog.close();
}

/// Help > Diagnostics: versions, paths, limits and the running session, ready
/// to paste into a bug report. It never includes settings values.
pub fn show_diagnostics_dialog(parent: Option<&gtk::Window>, panes: usize) {
    let overrides = settings::override_keys();
    let notes_dir = crate::notes::resolve_notes_dir();
    let extra = vec![
        (
            "gtk".to_string(),
            format!(
                "{}.{}.{}",
                gtk::major_version(),
                gtk::minor_version(),
                gtk::micro_version()
            ),
        ),
        ("vte".to_string(), unsafe {
            format!(
                "{}.{}.{}",
                zoha_vte_sys::vte_get_major_version(),
                zoha_vte_sys::vte_get_minor_version(),
                zoha_vte_sys::vte_get_micro_version()
            )
        }),
        (
            "themes".to_string(),
            format!("{} installed", settings::color_schemes().len()),
        ),
        (
            "themes-dir".to_string(),
            crate::headless::themes::themes_dir().display().to_string(),
        ),
        ("panes".to_string(), panes.to_string()),
        ("pid".to_string(), std::process::id().to_string()),
        (
            "control-socket".to_string(),
            crate::control::socket_path().display().to_string(),
        ),
    ];
    let report = diagnostics::Report {
        version: VERSION,
        build: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        rustc: env!("OXTERM_BUILD_RUSTC"),
        config_file: &settings::config_file(),
        config_dir: &settings::config_dir(),
        remote_dir: Some(&crate::remote::remote_dir()),
        notes_dir: Some(&notes_dir),
        overrides: &overrides,
        extra: &extra,
    };
    let text = diagnostics::render(&report);

    let dialog = gtk::Dialog::with_buttons(
        Some("Diagnostics - Oxterm"),
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Copy", gtk::ResponseType::Other(1)),
            ("Close", gtk::ResponseType::Close),
        ],
    );
    dialog.set_default_size(760, 520);
    let text_view = gtk::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    if let Some(buffer) = text_view.buffer() {
        buffer.set_text(&text);
    }
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.add(&text_view);
    dialog.content_area().pack_start(&scroll, true, true, 0);
    dialog.show_all();
    while dialog.run() == gtk::ResponseType::Other(1) {
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&text);
    }
    dialog.close();
}

impl DialogState {
    /// Apply the dialog to the settings; returns values that were rejected.
    fn apply(&self) -> Vec<String> {
        let s = settings();
        s.begin_batch();

        s.set_str("tab_title", &self.general.title.text());
        s.set_str(
            "dynamic_title",
            &combo_value(&self.general.dynamic, DYNAMIC_TITLE, "replace"),
        );
        s.set_bool("login_shell", self.general.login.is_active());
        s.set_str("shell_command", &self.general.shell.text());
        s.set_i64("terminal_columns", self.general.columns.value() as i64);
        s.set_i64("terminal_rows", self.general.rows.value() as i64);
        s.set_bool("session_restore", self.general.session_restore.is_active());
        s.set_bool(
            "ssh_connection_sharing",
            self.general.ssh_sharing.is_active(),
        );
        s.set_bool("confirm_close", self.general.confirm_close.is_active());
        s.set_bool("auto_copy_selection", self.general.auto_copy.is_active());
        s.set_bool(
            "show_unsafe_paste_dialog",
            self.general.warn_paste.is_active(),
        );
        s.set_str("file_manager", &self.general.file_manager.text());

        s.set_str(
            "scrollbar_position",
            &combo_value(
                &self.scrolling.scrollbar_position,
                SCROLLBAR_POSITION,
                "right",
            ),
        );
        let scrollback = if self.scrolling.scrollback_unlimited.is_active() {
            -1
        } else {
            self.scrolling.scrollback.value() as i64
        };
        s.set_i64("scrollback_lines", scrollback);
        s.set_bool(
            "scroll_on_output",
            self.scrolling.scroll_on_output.is_active(),
        );
        s.set_bool(
            "scroll_on_keystroke",
            self.scrolling.scroll_on_keystroke.is_active(),
        );
        s.set_bool("smooth_scroll", self.scrolling.smooth_scroll.is_active());

        s.set_bool("hint_mode_enabled", self.input.hint_mode.is_active());
        s.set_bool("vi_copy_mode_enabled", self.input.vi_copy.is_active());
        s.set_bool("tmux_prefix_enabled", self.input.tmux_prefix.is_active());
        let prefix_key = self
            .input
            .tmux_prefix_key
            .text()
            .trim()
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_lowercase().to_string())
            .unwrap_or_else(|| "b".to_string());
        s.set_str("tmux_prefix_key", &prefix_key);
        s.set_str(
            "backspace_binding",
            &combo_value(&self.input.backspace, BACKSPACE_BINDING, "ascii-del"),
        );
        s.set_str(
            "delete_binding",
            &combo_value(&self.input.delete, DELETE_BINDING, "escape-sequence"),
        );
        s.set_str("word_char_exceptions", self.input.word_chars.text().trim());

        s.set_bool("bell_notification", self.notifications.notify.is_active());
        s.set_bool(
            "notify_only_unfocused",
            self.notifications.only_unfocused.is_active(),
        );
        s.set_i64(
            "notify_min_seconds",
            self.notifications.min_seconds.value() as i64,
        );
        s.set_bool(
            "notify_on_failure",
            self.notifications.notify_failure.is_active(),
        );

        let font_name = self
            .appearance
            .font
            .font()
            .map(|font| font.to_string())
            .unwrap_or_default();
        let mut parts = font_name.rsplitn(2, ' ');
        let size = parts.next().unwrap_or("").to_string();
        let family = parts.next().unwrap_or(font_name.as_str()).to_string();
        s.set_str("font_name", &family);
        if let Ok(size) = size.parse::<i64>() {
            s.set_i64("font_size", size);
        }
        s.set_bool("allow_bold_text", self.appearance.bold.is_active());
        s.set_str(
            "font_rendering",
            &combo_value(&self.appearance.font_rendering, FONT_RENDERING, "system"),
        );
        s.set_str(
            "font_hinting",
            &combo_value(&self.appearance.font_hinting, FONT_HINTING, "system"),
        );
        let _ = s.set(
            "cell_height_scale",
            serde_json::json!(self.appearance.cell_height.value()),
        );
        let _ = s.set(
            "cell_width_scale",
            serde_json::json!(self.appearance.cell_width.value()),
        );
        s.set_bool("text_shaping", self.appearance.text_shaping.is_active());

        if let Some(name) = self.appearance.scheme.active_text() {
            s.set_str("color_scheme", &name);
        }
        s.set_bool("auto_theme", self.appearance.auto_theme.is_active());
        if let Some(name) = self.appearance.light_theme.active_text() {
            s.set_str("theme_light", &name);
        }
        if let Some(name) = self.appearance.dark_theme.active_text() {
            s.set_str("theme_dark", &name);
        }

        let foreground = if self.fg_override.get() {
            rgba_to_hex(&self.appearance.fg_color.rgba())
        } else {
            String::new()
        };
        let background = if self.bg_override.get() {
            rgba_to_hex(&self.appearance.bg_color.rgba())
        } else {
            String::new()
        };
        s.set_str("foreground_color", &foreground);
        s.set_str("background_color", &background);
        s.set_str(
            "cursor_color",
            &rgba_to_hex(&self.appearance.cursor_color.rgba()),
        );
        s.set_str(
            "highlight_color",
            &rgba_to_hex(&self.appearance.highlight.rgba()),
        );
        s.set_str(
            "highlight_bg_color",
            &rgba_to_hex(&self.appearance.highlight_bg.rgba()),
        );
        s.set_str(
            "tab_title_color",
            &rgba_to_hex(&self.appearance.tab_title_color.rgba()),
        );
        s.set_str(
            "tab_active_title_color",
            &rgba_to_hex(&self.appearance.tab_active_color.rgba()),
        );
        s.set_str(
            "cursor_shape",
            &combo_value(&self.appearance.cursor_shape, CURSOR_SHAPE, "block"),
        );
        s.set_bool("cursor_blink", self.appearance.cursor_blink.is_active());
        let _ = s.set(
            "opacity",
            serde_json::json!(self.appearance.opacity.value()),
        );
        s.set_bool(
            "enable_transparency",
            self.appearance.transparency.is_active(),
        );
        s.set_i64(
            "window_padding_horizontal",
            self.appearance.pad_h.value() as i64,
        );
        s.set_i64(
            "window_padding_vertical",
            self.appearance.pad_v.value() as i64,
        );
        s.set_str(
            "undercurl_style",
            &combo_value(&self.appearance.undercurl, UNDERLINE_STYLE, "single"),
        );

        let mut palette = BTreeMap::new();
        for (key, (button, _)) in self.palette_btns.iter() {
            palette.insert(key.clone(), serde_json::json!(rgba_to_hex(&button.rgba())));
        }
        let scheme_name = self.appearance.scheme.active_text().unwrap_or_default();
        let preset = settings::color_palettes()
            .get(scheme_name.as_str())
            .unwrap_or_else(|| settings::color_palettes().get("Dark (Default)").unwrap());
        let preset_json: BTreeMap<String, serde_json::Value> = preset
            .palette
            .iter()
            .map(|(key, value)| (key.to_string(), serde_json::json!(value)))
            .collect();
        if palette == preset_json {
            let _ = s.set("custom_palette", serde_json::Value::Null);
        } else {
            let mut object = serde_json::Map::new();
            for (key, value) in palette {
                object.insert(key, value);
            }
            let _ = s.set("custom_palette", serde_json::Value::Object(object));
        }

        s.set_str(
            "encoding",
            &self
                .compatibility
                .encoding
                .active_text()
                .map(|text| text.to_string())
                .unwrap_or_else(|| "UTF-8".to_string()),
        );
        s.set_bool(
            "bold_is_bright",
            self.compatibility.bold_is_bright.is_active(),
        );
        s.set_bool("sixel", self.compatibility.sixel.is_active());
        s.set_bool("osc133", self.compatibility.osc133.is_active());

        s.set_str("notes_dir", self.notes.notes_dir.text().trim());
        s.set_str("notes_file", self.notes.notes_file.text().trim());
        s.set_str("editor_command", self.notes.editor.text().trim());
        s.set_bool(
            "notes_record_context",
            self.notes.record_context.is_active(),
        );
        s.set_str("notes_template", self.notes.template.text().trim());

        s.end_batch();
        s.notify_changed();
        Vec::new()
    }

    /// Put every widget back to its documented default without saving. The
    /// reset is purely visual until the user presses OK.
    fn reset_defaults(&self) {
        // General
        self.general.title.set_text(&default_str("tab_title"));
        self.general
            .dynamic
            .set_active(Some(index_of(DYNAMIC_TITLE, &default_str("dynamic_title"))));
        self.general.login.set_active(default_bool("login_shell"));
        self.general.shell.set_text(&default_str("shell_command"));
        self.general
            .columns
            .set_value(default_i64("terminal_columns") as f64);
        self.general
            .rows
            .set_value(default_i64("terminal_rows") as f64);
        self.general
            .session_restore
            .set_active(default_bool("session_restore"));
        self.general
            .ssh_sharing
            .set_active(default_bool("ssh_connection_sharing"));
        self.general
            .confirm_close
            .set_active(default_bool("confirm_close"));
        self.general
            .auto_copy
            .set_active(default_bool("auto_copy_selection"));
        self.general
            .warn_paste
            .set_active(default_bool("show_unsafe_paste_dialog"));
        self.general
            .file_manager
            .set_text(&default_str("file_manager"));

        // Scrolling
        self.scrolling.scrollbar_position.set_active(Some(index_of(
            SCROLLBAR_POSITION,
            &default_str("scrollbar_position"),
        )));
        let scrollback_default = default_i64("scrollback_lines");
        self.scrolling
            .scrollback_unlimited
            .set_active(scrollback_default == -1);
        self.scrolling
            .scrollback
            .set_value(if scrollback_default > 0 {
                scrollback_default as f64
            } else {
                10_000.0
            });
        self.scrolling
            .scrollback
            .set_sensitive(scrollback_default != -1);
        self.scrolling
            .scroll_on_output
            .set_active(default_bool("scroll_on_output"));
        self.scrolling
            .scroll_on_keystroke
            .set_active(default_bool("scroll_on_keystroke"));
        self.scrolling
            .smooth_scroll
            .set_active(default_bool("smooth_scroll"));

        // Input
        self.input
            .hint_mode
            .set_active(default_bool("hint_mode_enabled"));
        self.input
            .vi_copy
            .set_active(default_bool("vi_copy_mode_enabled"));
        self.input
            .tmux_prefix
            .set_active(default_bool("tmux_prefix_enabled"));
        self.input
            .tmux_prefix_key
            .set_text(&default_str("tmux_prefix_key"));
        self.input.backspace.set_active(Some(index_of(
            BACKSPACE_BINDING,
            &default_str("backspace_binding"),
        )));
        self.input.delete.set_active(Some(index_of(
            DELETE_BINDING,
            &default_str("delete_binding"),
        )));
        self.input
            .word_chars
            .set_text(&default_str("word_char_exceptions"));

        // Notifications
        self.notifications
            .notify
            .set_active(default_bool("bell_notification"));
        self.notifications
            .only_unfocused
            .set_active(default_bool("notify_only_unfocused"));
        self.notifications
            .min_seconds
            .set_value(default_i64("notify_min_seconds") as f64);
        self.notifications
            .notify_failure
            .set_active(default_bool("notify_on_failure"));

        // Appearance
        let font = format!("{} {}", default_str("font_name"), default_i64("font_size"));
        self.appearance.font.set_font(&font);
        self.appearance
            .bold
            .set_active(default_bool("allow_bold_text"));
        self.appearance.font_rendering.set_active(Some(index_of(
            FONT_RENDERING,
            &default_str("font_rendering"),
        )));
        self.appearance
            .font_hinting
            .set_active(Some(index_of(FONT_HINTING, &default_str("font_hinting"))));
        self.appearance
            .cell_height
            .set_value(default_f64("cell_height_scale"));
        self.appearance
            .cell_width
            .set_value(default_f64("cell_width_scale"));
        self.appearance
            .text_shaping
            .set_active(default_bool("text_shaping"));
        let default_scheme = default_str("color_scheme");
        let scheme_names: Vec<String> = settings::color_schemes().keys().cloned().collect();
        self.appearance.scheme.set_active(Some(
            scheme_names
                .iter()
                .position(|name| *name == default_scheme)
                .unwrap_or(0) as u32,
        ));
        self.appearance
            .auto_theme
            .set_active(default_bool("auto_theme"));
        for (combo, key) in [
            (&self.appearance.light_theme, "theme_light"),
            (&self.appearance.dark_theme, "theme_dark"),
        ] {
            let value = default_str(key);
            combo.set_active(Some(
                scheme_names
                    .iter()
                    .position(|name| *name == value)
                    .unwrap_or(0) as u32,
            ));
        }
        self.appearance
            .cursor_color
            .set_rgba(&hex_to_rgba(&default_str("cursor_color")));
        self.appearance
            .highlight
            .set_rgba(&hex_to_rgba(&default_str("highlight_color")));
        self.appearance
            .highlight_bg
            .set_rgba(&hex_to_rgba(&default_str("highlight_bg_color")));
        self.appearance
            .tab_title_color
            .set_rgba(&hex_to_rgba(&default_str("tab_title_color")));
        self.appearance
            .tab_active_color
            .set_rgba(&hex_to_rgba(&default_str("tab_active_title_color")));
        self.appearance
            .cursor_shape
            .set_active(Some(index_of(CURSOR_SHAPE, &default_str("cursor_shape"))));
        self.appearance
            .cursor_blink
            .set_active(default_bool("cursor_blink"));
        self.appearance
            .transparency
            .set_active(default_bool("enable_transparency"));
        self.appearance.opacity.set_value(default_f64("opacity"));
        self.appearance
            .pad_h
            .set_value(default_i64("window_padding_horizontal") as f64);
        self.appearance
            .pad_v
            .set_value(default_i64("window_padding_vertical") as f64);
        self.appearance.undercurl.set_active(Some(index_of(
            UNDERLINE_STYLE,
            &default_str("undercurl_style"),
        )));

        // Foreground, background and palette follow the default theme.
        if let Some(theme) = settings::color_schemes().get(default_scheme.as_str()) {
            self.appearance
                .fg_color
                .set_rgba(&hex_to_rgba(&theme.foreground));
            self.appearance
                .bg_color
                .set_rgba(&hex_to_rgba(&theme.background));
            apply_palette(&self.palette_btns, &theme.palette);
        }
        self.fg_override.set(false);
        self.bg_override.set(false);

        // Compatibility
        self.compatibility.encoding.set_active(Some(
            ENCODINGS
                .iter()
                .position(|name| *name == default_str("encoding"))
                .unwrap_or(0) as u32,
        ));
        self.compatibility
            .bold_is_bright
            .set_active(default_bool("bold_is_bright"));
        self.compatibility.sixel.set_active(default_bool("sixel"));
        self.compatibility.osc133.set_active(default_bool("osc133"));

        // Notes
        self.notes.notes_dir.set_text(&default_str("notes_dir"));
        self.notes.notes_file.set_text(&default_str("notes_file"));
        self.notes.editor.set_text(&default_str("editor_command"));
        self.notes
            .record_context
            .set_active(default_bool("notes_record_context"));
        self.notes.template.set_text(&default_str("notes_template"));
    }
}
