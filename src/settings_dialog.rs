use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use glib::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

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

struct DialogState {
    #[allow(dead_code)]
    dialog: gtk::Dialog,
    palette_btns: RefCell<BTreeMap<String, (gtk::ColorButton, String)>>,
    _fg_btn: gtk::ColorButton,
    _bg_btn: gtk::ColorButton,
    fg_override: Rc<Cell<bool>>,
    bg_override: Rc<Cell<bool>>,
    // General
    entry_title: gtk::Entry,
    combo_dynamic: gtk::ComboBoxText,
    chk_login: gtk::CheckButton,
    entry_shell: gtk::Entry,
    spin_columns: gtk::SpinButton,
    spin_rows: gtk::SpinButton,
    combo_scrollbar_pos: gtk::ComboBoxText,
    chk_scrollback_unlimited: gtk::CheckButton,
    spin_scrollback: gtk::SpinButton,
    chk_scroll_output: gtk::CheckButton,
    chk_scroll_keystroke: gtk::CheckButton,
    chk_confirm_close: gtk::CheckButton,
    chk_auto_copy: gtk::CheckButton,
    chk_warn_paste: gtk::CheckButton,
    entry_fm: gtk::Entry,
    chk_session_restore: gtk::CheckButton,
    chk_bell_notify: gtk::CheckButton,
    chk_hint_mode: gtk::CheckButton,
    chk_vi_copy: gtk::CheckButton,
    // Appearance
    font_btn: gtk::FontButton,
    chk_bold: gtk::CheckButton,
    combo_scheme: gtk::ComboBoxText,
    fg_color_btn: gtk::ColorButton,
    bg_color_btn: gtk::ColorButton,
    cursor_color_btn: gtk::ColorButton,
    highlight_btn: gtk::ColorButton,
    highlight_bg_btn: gtk::ColorButton,
    tab_title_color_btn: gtk::ColorButton,
    tab_active_color_btn: gtk::ColorButton,
    combo_cursor_shape: gtk::ComboBoxText,
    chk_cursor_blink: gtk::CheckButton,
    spin_opacity: gtk::SpinButton,
    chk_transparency: gtk::CheckButton,
    spin_pad_h: gtk::SpinButton,
    spin_pad_v: gtk::SpinButton,
    combo_undercurl: gtk::ComboBoxText,
    // Compatibility
    combo_backspace: gtk::ComboBoxText,
    combo_delete: gtk::ComboBoxText,
    combo_encoding: gtk::ComboBoxText,
    chk_osc133: gtk::CheckButton,
    // AI
    ai_entries: BTreeMap<String, (gtk::Entry, gtk::Entry, gtk::TextBuffer)>,
    ai_url_entries: BTreeMap<String, gtk::Entry>,
    // Notes
    entry_notes_dir: gtk::Entry,
    entry_notes_file: gtk::Entry,
    entry_editor: gtk::Entry,
}

pub fn show_settings_dialog(parent: Option<&gtk::Window>) {
    let s = settings();
    let dialog = gtk::Dialog::with_buttons(
        Some("Preferences - Oxterm"),
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("OK", gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_size(880, 700);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(160, -1);

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
        .pack_start(&content_box, true, true, 8);

    // Build pages
    let (general_widget, general) = build_general(&s);
    let (appearance_widget, appearance) = build_appearance(&s);
    let (colors_widget, colors) = build_colors(&s);
    let (compat_widget, compat) = build_compatibility(&s);
    let (ai_widget, ai_entries, ai_url_entries) = build_ai(&s);
    let (notes_widget, notes) = build_notes(&s);

    stack.add_titled(&general_widget, "general", "General");
    stack.add_titled(&appearance_widget, "appearance", "Appearance");
    stack.add_titled(&colors_widget, "colors", "Colors");
    stack.add_titled(&compat_widget, "compatibility", "Compatibility");
    stack.add_titled(&ai_widget, "ai", "AI");
    stack.add_titled(&notes_widget, "notes", "Notes");

    let readability_css = gtk::CssProvider::new();
    let _ = readability_css.load_from_data(
        b"stacksidebar row { padding: 8px 10px; }
           stacksidebar row label { font-size: 1.05em; }
           entry, spinbutton, combobox button, checkbutton label,
           radiobutton label { font-size: 1.03em; }
           entry, spinbutton { padding: 4px 6px; }",
    );
    dialog
        .style_context()
        .add_provider(&readability_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let ok_btn = dialog
        .widget_for_response(gtk::ResponseType::Ok)
        .and_then(|w| w.downcast::<gtk::Button>().ok());
    if let Some(btn) = ok_btn {
        btn.style_context().add_class("suggested-action");
    }

    let state = DialogState {
        dialog: dialog.clone(),
        palette_btns: RefCell::new(colors),
        _fg_btn: appearance.4.clone(),
        _bg_btn: appearance.5.clone(),
        fg_override: Rc::new(Cell::new(!s.get_str("foreground_color").is_empty())),
        bg_override: Rc::new(Cell::new(!s.get_str("background_color").is_empty())),
        entry_title: general.0,
        combo_dynamic: general.1,
        chk_login: general.2,
        entry_shell: general.3,
        spin_columns: general.4,
        spin_rows: general.5,
        combo_scrollbar_pos: general.6,
        chk_scrollback_unlimited: general.7,
        spin_scrollback: general.8,
        chk_scroll_output: general.9,
        chk_scroll_keystroke: general.10,
        chk_confirm_close: general.11,
        chk_auto_copy: general.12,
        chk_warn_paste: general.13,
        entry_fm: general.14,
        chk_session_restore: general.15,
        chk_bell_notify: general.16,
        chk_hint_mode: general.17,
        chk_vi_copy: general.18,
        font_btn: appearance.0,
        chk_bold: appearance.1,
        combo_scheme: appearance.2,
        fg_color_btn: appearance.4.clone(),
        bg_color_btn: appearance.5.clone(),
        cursor_color_btn: appearance.6,
        highlight_btn: appearance.7,
        highlight_bg_btn: appearance.8,
        tab_title_color_btn: appearance.9,
        tab_active_color_btn: appearance.10,
        combo_cursor_shape: appearance.11,
        chk_cursor_blink: appearance.12,
        spin_opacity: appearance.13,
        chk_transparency: appearance.14,
        spin_pad_h: appearance.15,
        spin_pad_v: appearance.16,
        combo_undercurl: appearance.17,
        combo_backspace: compat.0,
        combo_delete: compat.1,
        combo_encoding: compat.2,
        chk_osc133: compat.3,
        ai_entries: ai_entries,
        ai_url_entries: ai_url_entries,
        entry_notes_dir: notes.0,
        entry_notes_file: notes.1,
        entry_editor: notes.2,
    };

    {
        let override_flag = state.fg_override.clone();
        state
            .fg_color_btn
            .connect_color_set(move |_| override_flag.set(true));
        let override_flag = state.bg_override.clone();
        state
            .bg_color_btn
            .connect_color_set(move |_| override_flag.set(true));
    }

    // Live preview: switching the scheme updates fg/bg and palette buttons.
    {
        let combo = state.combo_scheme.clone();
        let fg_btn = state.fg_color_btn.clone();
        let bg_btn = state.bg_color_btn.clone();
        let palette_btns = state.palette_btns.clone();
        combo.connect_changed(move |combo| {
            let Some(name) = combo.active_text() else {
                return;
            };
            if let Some(scheme) = settings::color_schemes().get(name.as_str()) {
                if let Some(fg) = scheme.get("foreground") {
                    fg_btn.set_rgba(&hex_to_rgba(fg));
                }
                if let Some(bg) = scheme.get("background") {
                    bg_btn.set_rgba(&hex_to_rgba(bg));
                }
            }
            if let Some(pal) = settings::color_palettes().get(name.as_str()) {
                for (key, (btn, _)) in palette_btns.borrow().iter() {
                    if let Some(hex) = pal.get(key.as_str()) {
                        btn.set_rgba(&hex_to_rgba(hex));
                    }
                }
            }
        });
    }

    dialog.connect_response(move |_dlg, response| {
        if response == gtk::ResponseType::Ok {
            state.apply();
        }
        // destroy handled by response default
    });

    dialog.show_all();
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
        "Native Linux terminal emulator built with Rust, GTK 3 and VTE.\n\nTabs, split panes, command history, shell integration, notes, profiles, named sessions, optional AI chat, and SSH-friendly remote attach/detach.\n\nSee the project page for the user manual, releases, and issue tracker.",
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

    let text = format!(
        "OXTERM QUICK HELP\n\n\
Terminal workflow\n\
  Ctrl+Shift+T       New tab\n\
  Ctrl+Shift+N       New window\n\
  Ctrl+Shift+W       Close tab\n\
  Ctrl+PageUp/Down   Previous / next tab\n\
  Ctrl+Alt+PageUp    Switch split pane\n\
  Ctrl+Shift+F       Search terminal scrollback\n\
  Ctrl+Shift+P       Open command palette\n\
  Ctrl+Shift+C/V     Copy / paste\n\
  Ctrl+Plus/Minus    Zoom in / out\n\
  Ctrl+0             Reset zoom\n\
\nBuilt-in commands\n\
  /help              Show the in-terminal command reference\n\
  /history [terms]   Search command history (use -term to exclude)\n\
  /ai                Start optional AI chat\n\
  /ai explain        Explain the latest failed command\n\
  /ai repair         Suggest a safe repair for the latest failure\n\
  /connect           Select and test an AI provider\n\
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

fn section(grid: &gtk::Grid, row: i32, title: &str) -> i32 {
    let label = gtk::Label::new(Some(title));
    label.set_use_markup(true);
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    grid.attach(&label, 0, row, 2, 1);
    row + 1
}

fn row(grid: &gtk::Grid, row: i32, label_text: &str, widget: &impl IsA<gtk::Widget>) -> i32 {
    let lbl = gtk::Label::new(Some(label_text));
    lbl.set_halign(gtk::Align::End);
    lbl.set_margin_end(8);
    grid.attach(&lbl, 0, row, 1, 1);
    grid.attach(widget, 1, row, 1, 1);
    row + 1
}

fn make_color_button(hex_color: &str, tooltip: &str) -> gtk::ColorButton {
    let rgba = hex_to_rgba(hex_color);
    let btn = gtk::ColorButton::with_rgba(&rgba);
    btn.set_size_request(48, 22);
    btn.set_halign(gtk::Align::Start);
    btn.set_tooltip_text(Some(tooltip));
    btn
}

// Returns (widget, individual fields)
fn build_general(
    s: &Settings,
) -> (
    gtk::ScrolledWindow,
    (
        gtk::Entry,
        gtk::ComboBoxText,
        gtk::CheckButton,
        gtk::Entry,
        gtk::SpinButton,
        gtk::SpinButton,
        gtk::ComboBoxText,
        gtk::CheckButton,
        gtk::SpinButton,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::Entry,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::CheckButton,
        gtk::CheckButton,
    ),
) {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let grid = gtk::Grid::new();
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_row_spacing(9);
    grid.set_column_spacing(12);
    let mut r = 0;

    r = section(&grid, r, "Title");
    let entry_title = gtk::Entry::new();
    entry_title.set_text(&s.get_str_default("tab_title", "Terminal"));
    entry_title.set_tooltip_text(Some("Default name displayed on each new tab"));
    r = row(&grid, r, "Initial title:", &entry_title);

    let combo_dynamic = gtk::ComboBoxText::new();
    combo_dynamic.append_text("Replace initial title");
    combo_dynamic.append_text("Goes before initial title");
    combo_dynamic.append_text("Goes after initial title");
    combo_dynamic.append_text("Isn't displayed");
    let dyn_map = [("replace", 0), ("before", 1), ("after", 2), ("hide", 3)];
    let current = s.get_str_default("dynamic_title", "replace");
    let idx = dyn_map
        .iter()
        .find(|(k, _)| *k == current)
        .map(|(_, v)| *v)
        .unwrap_or(0);
    combo_dynamic.set_active(Some(idx));
    combo_dynamic.set_tooltip_text(Some(
        "How to combine a title set by the shell (OSC escape) with the initial title",
    ));
    r = row(&grid, r, "Dynamic title:", &combo_dynamic);

    r += 1;
    r = section(&grid, r, "Command");
    let chk_login = gtk::CheckButton::with_label("Run as login shell");
    chk_login.set_active(s.get_bool("login_shell"));
    chk_login.set_tooltip_text(Some(
        "Start the shell as a login shell (loads .profile / .bash_profile)",
    ));
    r = row(&grid, r, "Login shell:", &chk_login);

    let entry_shell = gtk::Entry::new();
    entry_shell.set_text(&s.get_str_default("shell_command", "/bin/bash"));
    entry_shell.set_tooltip_text(Some(
        "Custom shell command or path. Use $SHELL for the default shell",
    ));
    r = row(&grid, r, "Shell:", &entry_shell);

    let size_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let spin_columns = gtk::SpinButton::with_range(40.0, 300.0, 1.0);
    spin_columns.set_value(s.get_i64("terminal_columns") as f64);
    spin_columns.set_tooltip_text(Some("Default terminal width in character columns (40-300)"));
    let spin_rows = gtk::SpinButton::with_range(10.0, 120.0, 1.0);
    spin_rows.set_value(s.get_i64("terminal_rows") as f64);
    spin_rows.set_tooltip_text(Some("Default terminal height in character rows (10-120)"));
    size_box.pack_start(&spin_columns, false, false, 0);
    size_box.pack_start(&gtk::Label::new(Some("x")), false, false, 2);
    size_box.pack_start(&spin_rows, false, false, 0);
    size_box.pack_start(&gtk::Label::new(Some("(cols x rows)")), false, false, 6);
    r = row(&grid, r, "Size:", &size_box);

    r += 1;
    r = section(&grid, r, "Scrolling");
    let combo_scrollbar_pos = gtk::ComboBoxText::new();
    combo_scrollbar_pos.append_text("Right side");
    combo_scrollbar_pos.append_text("Left side");
    combo_scrollbar_pos.append_text("Disabled");
    let pos_map = [("right", 0), ("left", 1), ("disabled", 2)];
    let current = s.get_str_default("scrollbar_position", "right");
    combo_scrollbar_pos.set_active(Some(
        pos_map
            .iter()
            .find(|(k, _)| *k == current)
            .map(|(_, v)| *v)
            .unwrap_or(0),
    ));
    combo_scrollbar_pos.set_tooltip_text(Some("Position of the scrollbar or disable it entirely"));
    r = row(&grid, r, "Scrollbar is:", &combo_scrollbar_pos);

    let chk_scrollback_unlimited = gtk::CheckButton::new();
    chk_scrollback_unlimited.set_active(s.get_i64("scrollback_lines") == -1);
    chk_scrollback_unlimited.set_tooltip_text(Some("Unlimited scrollback buffer"));
    let spin_scrollback = gtk::SpinButton::with_range(1.0, 1_000_000.0, 1.0);
    {
        let val = s.get_i64("scrollback_lines");
        spin_scrollback.set_value(if val > 0 { val as f64 } else { 10000.0 });
        spin_scrollback.set_sensitive(val > 0);
    }
    spin_scrollback.set_tooltip_text(Some(
        "Maximum number of lines kept in the scrollback buffer",
    ));
    {
        let spin = spin_scrollback.clone();
        chk_scrollback_unlimited.connect_toggled(move |chk| {
            spin.set_sensitive(!chk.is_active());
        });
    }
    r = row(&grid, r, "Unlimited scrollback:", &chk_scrollback_unlimited);
    r = row(&grid, r, "Scrollback lines:", &spin_scrollback);

    let chk_scroll_output = gtk::CheckButton::new();
    chk_scroll_output.set_active(s.get_bool("scroll_on_output"));
    chk_scroll_output.set_tooltip_text(Some(
        "Automatically scroll to the bottom when new output appears",
    ));
    r = row(&grid, r, "Scroll on output:", &chk_scroll_output);

    let chk_scroll_keystroke = gtk::CheckButton::new();
    chk_scroll_keystroke.set_active(s.get_bool("scroll_on_keystroke"));
    chk_scroll_keystroke.set_tooltip_text(Some("Automatically scroll to the bottom when typing"));
    r = row(&grid, r, "Scroll on keystroke:", &chk_scroll_keystroke);

    r += 1;
    r = section(&grid, r, "Other");
    let chk_confirm_close = gtk::CheckButton::new();
    chk_confirm_close.set_active(s.get_bool("confirm_close"));
    chk_confirm_close.set_tooltip_text(Some("Ask for confirmation before closing the window"));
    r = row(&grid, r, "Confirm before closing:", &chk_confirm_close);

    let chk_auto_copy = gtk::CheckButton::new();
    chk_auto_copy.set_active(s.get_bool("auto_copy_selection"));
    chk_auto_copy.set_tooltip_text(Some("Automatically copy selected text to the clipboard"));
    r = row(&grid, r, "Auto-copy selection:", &chk_auto_copy);

    let chk_warn_paste = gtk::CheckButton::new();
    chk_warn_paste.set_active(s.get_bool("show_unsafe_paste_dialog"));
    chk_warn_paste.set_tooltip_text(Some(
        "Warn before pasting multi-line text that may contain harmful commands",
    ));
    r = row(&grid, r, "Warn multi-line paste:", &chk_warn_paste);

    let entry_fm = gtk::Entry::new();
    entry_fm.set_text(&s.get_str("file_manager"));
    entry_fm.set_placeholder_text(Some("auto-detect"));
    entry_fm.set_tooltip_text(Some(
        "Override the file manager (e.g. nemo, thunar, dolphin). Leave blank to auto-detect",
    ));
    r = row(&grid, r, "File manager:", &entry_fm);

    r += 1;
    r = section(&grid, r, "Session");
    let chk_session_restore = gtk::CheckButton::new();
    chk_session_restore.set_active(s.get_bool("session_restore"));
    chk_session_restore
        .set_tooltip_text(Some("Restore tabs and splits from last session on startup"));
    r = row(&grid, r, "Restore last session:", &chk_session_restore);

    let chk_bell_notify = gtk::CheckButton::new();
    chk_bell_notify.set_active(s.get_bool("bell_notification"));
    chk_bell_notify.set_tooltip_text(Some(
        "Show desktop notification when a command completes (requires OSC 133 shell integration)",
    ));
    r = row(&grid, r, "Notify on command completion:", &chk_bell_notify);

    r += 1;
    r = section(&grid, r, "Keyboard Modes");
    let chk_hint_mode = gtk::CheckButton::new();
    chk_hint_mode.set_active(s.get_bool("hint_mode_enabled"));
    chk_hint_mode.set_tooltip_text(Some(
        "Ctrl+Shift+H: highlight URLs, paths and git SHAs with keyboard-selectable labels",
    ));
    r = row(&grid, r, "Hint mode:", &chk_hint_mode);

    let chk_vi_copy = gtk::CheckButton::new();
    chk_vi_copy.set_active(s.get_bool("vi_copy_mode_enabled"));
    chk_vi_copy.set_tooltip_text(Some(
        "Ctrl+Shift+Y: VI-style copy mode (hjkl scroll, v select, y yank, Esc exit)",
    ));
    row(&grid, r, "VI copy mode:", &chk_vi_copy);

    sw.add(&grid);
    (
        sw,
        (
            entry_title,
            combo_dynamic,
            chk_login,
            entry_shell,
            spin_columns,
            spin_rows,
            combo_scrollbar_pos,
            chk_scrollback_unlimited,
            spin_scrollback,
            chk_scroll_output,
            chk_scroll_keystroke,
            chk_confirm_close,
            chk_auto_copy,
            chk_warn_paste,
            entry_fm,
            chk_session_restore,
            chk_bell_notify,
            chk_hint_mode,
            chk_vi_copy,
        ),
    )
}

fn build_appearance(
    s: &Settings,
) -> (
    gtk::ScrolledWindow,
    (
        gtk::FontButton,
        gtk::CheckButton,
        gtk::ComboBoxText,
        gtk::ScrolledWindow,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ColorButton,
        gtk::ComboBoxText,
        gtk::CheckButton,
        gtk::SpinButton,
        gtk::CheckButton,
        gtk::SpinButton,
        gtk::SpinButton,
        gtk::ComboBoxText,
    ),
) {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let grid = gtk::Grid::new();
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_row_spacing(9);
    grid.set_column_spacing(12);
    let mut r = 0;

    r = section(&grid, r, "Font");
    let font_desc = format!(
        "{} {}",
        s.get_str_default("font_name", "Monospace"),
        s.get_i64("font_size")
    );
    let font_btn = gtk::FontButton::new();
    font_btn.set_font(&font_desc);
    font_btn.set_tooltip_text(Some("Choose a font family and size"));
    r = row(&grid, r, "Font:", &font_btn);

    let chk_bold = gtk::CheckButton::new();
    chk_bold.set_active(s.get_bool("allow_bold_text"));
    chk_bold.set_tooltip_text(Some(
        "Render bold text as bold. Disable for uniform text weight",
    ));
    r = row(&grid, r, "Allow bold text:", &chk_bold);

    r += 1;
    r = section(&grid, r, "Color Scheme");
    let combo_scheme = gtk::ComboBoxText::new();
    let mut scheme_names: Vec<String> = Vec::new();
    for name in settings::color_schemes().keys() {
        combo_scheme.append_text(name);
        scheme_names.push(name.to_string());
    }
    let current = s.get_str_default("color_scheme", "Dark (Default)");
    let idx = scheme_names.iter().position(|n| *n == current).unwrap_or(0);
    combo_scheme.set_active(Some(idx as u32));
    combo_scheme.set_tooltip_text(Some(
        "Choose a built-in color scheme. Customize individual colors below",
    ));
    r = row(&grid, r, "Scheme:", &combo_scheme);

    r += 1;
    r = section(&grid, r, "Individual Colors");
    let fg_color_btn = make_color_button(
        &s.get_fg_color(),
        "Default text foreground color – click to change",
    );
    let bg_color_btn = make_color_button(
        &s.get_bg_color(),
        "Terminal background color – click to change",
    );
    let cursor_color_btn = make_color_button(
        &s.get_str_default("cursor_color", "#ffffff"),
        "Cursor color – click to change",
    );
    let highlight_btn = make_color_button(
        &s.get_str_default("highlight_color", "#ffffff"),
        "Selected text color – click to change",
    );
    let highlight_bg_btn = make_color_button(
        &s.get_str_default("highlight_bg_color", "#446688"),
        "Selection background color – click to change",
    );

    let color_grid = gtk::Grid::new();
    color_grid.set_column_spacing(12);
    color_grid.set_row_spacing(4);
    color_grid.attach(&gtk::Label::new(Some("Foreground:")), 0, 0, 1, 1);
    color_grid.attach(&fg_color_btn, 1, 0, 1, 1);
    color_grid.attach(&gtk::Label::new(Some("Background:")), 0, 1, 1, 1);
    color_grid.attach(&bg_color_btn, 1, 1, 1, 1);
    color_grid.attach(&gtk::Label::new(Some("Cursor:")), 0, 2, 1, 1);
    color_grid.attach(&cursor_color_btn, 1, 2, 1, 1);
    color_grid.attach(&gtk::Label::new(Some("Highlight text:")), 2, 0, 1, 1);
    color_grid.attach(&highlight_btn, 3, 0, 1, 1);
    color_grid.attach(&gtk::Label::new(Some("Highlight bg:")), 2, 1, 1, 1);
    color_grid.attach(&highlight_bg_btn, 3, 1, 1, 1);
    r = row(&grid, r, "", &color_grid);

    r += 1;
    r = section(&grid, r, "Tab Colors");
    let tab_title_color_btn = make_color_button(
        &s.get_str_default("tab_title_color", "#ffffff"),
        "Color for inactive tab titles – click to change",
    );
    r = row(&grid, r, "Tab title:", &tab_title_color_btn);
    let tab_active_color_btn = make_color_button(
        &s.get_str_default("tab_active_title_color", "#ffffff"),
        "Color for the active tab title – click to change",
    );
    r = row(&grid, r, "Active tab:", &tab_active_color_btn);

    r += 1;
    r = section(&grid, r, "Cursor");
    let combo_cursor_shape = gtk::ComboBoxText::new();
    combo_cursor_shape.append_text("block");
    combo_cursor_shape.append_text("underline");
    combo_cursor_shape.append_text("ibeam");
    let shape = s.get_str_default("cursor_shape", "block");
    combo_cursor_shape.set_active(Some(match shape.as_str() {
        "underline" => 1,
        "ibeam" => 2,
        _ => 0,
    }));
    combo_cursor_shape.set_tooltip_text(Some(
        "Cursor appearance: block (filled rectangle), underline (_), ibeam (|)",
    ));
    r = row(&grid, r, "Cursor shape:", &combo_cursor_shape);

    let chk_cursor_blink = gtk::CheckButton::new();
    chk_cursor_blink.set_active(s.get_bool("cursor_blink"));
    chk_cursor_blink.set_tooltip_text(Some("Make the cursor blink periodically"));
    r = row(&grid, r, "Cursor blinking:", &chk_cursor_blink);

    r += 1;
    r = section(&grid, r, "Transparency");
    let spin_opacity = gtk::SpinButton::with_range(0.1, 1.0, 0.05);
    spin_opacity.set_value(s.get_f64("opacity"));
    spin_opacity.set_tooltip_text(Some(
        "Window opacity (1.0 = fully opaque, 0.3 = almost transparent)",
    ));
    r = row(&grid, r, "Opacity:", &spin_opacity);
    let chk_transparency = gtk::CheckButton::new();
    chk_transparency.set_active(s.get_bool("enable_transparency"));
    chk_transparency.set_tooltip_text(Some(
        "Enable RGBA compositing transparency. Requires a compositor (e.g. picom, Wayland)",
    ));
    r = row(&grid, r, "Enable transparency:", &chk_transparency);

    r += 1;
    r = section(&grid, r, "Padding");
    let pad_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let spin_pad_h = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    spin_pad_h.set_value(s.get_i64("window_padding_horizontal") as f64);
    spin_pad_h.set_tooltip_text(Some(
        "Horizontal padding inside the terminal window (pixels)",
    ));
    pad_box.pack_start(&gtk::Label::new(Some("H:")), false, false, 0);
    pad_box.pack_start(&spin_pad_h, false, false, 0);
    let spin_pad_v = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    spin_pad_v.set_value(s.get_i64("window_padding_vertical") as f64);
    spin_pad_v.set_tooltip_text(Some("Vertical padding inside the terminal window (pixels)"));
    pad_box.pack_start(&gtk::Label::new(Some("V:")), false, false, 0);
    pad_box.pack_start(&spin_pad_v, false, false, 0);
    r = row(&grid, r, "Border:", &pad_box);

    r += 1;
    r = section(&grid, r, "Underlines");
    let combo_undercurl = gtk::ComboBoxText::new();
    for style in ["single", "double", "curly", "dashed", "dotted"] {
        combo_undercurl.append_text(style);
    }
    let curl_style = s.get_str_default("undercurl_style", "single");
    let idx = ["single", "double", "curly", "dashed", "dotted"]
        .iter()
        .position(|x| *x == curl_style)
        .unwrap_or(0);
    combo_undercurl.set_active(Some(idx as u32));
    combo_undercurl.set_tooltip_text(Some(
        "Style for underlined text (e.g. compiler errors, spelling). curly requires VTE >= 0.58",
    ));
    row(&grid, r, "Underline style:", &combo_undercurl);

    sw.add(&grid);
    let sw_ret = sw.clone();
    (
        sw,
        (
            font_btn,
            chk_bold,
            combo_scheme,
            sw_ret,
            fg_color_btn,
            bg_color_btn,
            cursor_color_btn,
            highlight_btn,
            highlight_bg_btn,
            tab_title_color_btn,
            tab_active_color_btn,
            combo_cursor_shape,
            chk_cursor_blink,
            spin_opacity,
            chk_transparency,
            spin_pad_h,
            spin_pad_v,
            combo_undercurl,
        ),
    )
}

fn build_colors(
    s: &Settings,
) -> (
    gtk::ScrolledWindow,
    BTreeMap<String, (gtk::ColorButton, String)>,
) {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);

    let label = gtk::Label::new(Some("16-Color Palette"));
    label.set_use_markup(true);
    label.set_halign(gtk::Align::Start);
    vbox.pack_start(&label, false, false, 4);

    let saved = s.get("custom_palette");
    let scheme = s.get_str_default("color_scheme", "Dark (Default)");
    let default_palette = settings::color_palettes()
        .get(scheme.as_str())
        .unwrap_or_else(|| settings::color_palettes().get("Dark (Default)").unwrap());

    let labels_16: [[&str; 4]; 4] = [
        ["Black", "Red", "Green", "Yellow"],
        ["Blue", "Magenta", "Cyan", "White"],
        ["B.Black", "B.Red", "B.Green", "B.Yellow"],
        ["B.Blue", "B.Magenta", "B.Cyan", "B.White"],
    ];
    let keys_16: [[&str; 4]; 4] = [
        ["black", "red", "green", "yellow"],
        ["blue", "magenta", "cyan", "white"],
        ["brightblack", "brightred", "brightgreen", "brightyellow"],
        ["brightblue", "brightmagenta", "brightcyan", "brightwhite"],
    ];
    let mut palette_btns: BTreeMap<String, (gtk::ColorButton, String)> = BTreeMap::new();
    let palette_grid = gtk::Grid::new();
    palette_grid.set_row_spacing(2);
    palette_grid.set_column_spacing(12);
    for ri in 0..4 {
        for ci in 0..4 {
            let key = keys_16[ri][ci];
            let name = labels_16[ri][ci];
            let hex = if let Some(custom) = saved.as_object() {
                custom
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| default_palette.get(key).unwrap_or(&"#000000").to_string())
            } else {
                default_palette.get(key).unwrap_or(&"#000000").to_string()
            };
            let btn = make_color_button(&hex, name);
            btn.set_size_request(48, 22);
            palette_btns.insert(key.to_string(), (btn.clone(), hex));
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let lbl = gtk::Label::new(Some(name));
            lbl.set_margin_start(4);
            cell.pack_start(&lbl, false, false, 0);
            cell.pack_start(&btn, false, false, 0);
            palette_grid.attach(&cell, ci as i32, ri as i32, 1, 1);
        }
    }
    vbox.pack_start(&palette_grid, false, false, 8);

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let load_btn = gtk::Button::with_label("Load Preset...");
    let btns = palette_btns.clone();
    load_btn.connect_clicked(move |_| {
        let dialog = gtk::Dialog::with_buttons(
            Some("Load Palette"),
            None::<&gtk::Window>,
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Load", gtk::ResponseType::Ok),
            ],
        );
        let combo = gtk::ComboBoxText::new();
        let presets = [
            "Dark (Default)",
            "Light",
            "Solarized Dark",
            "Solarized Light",
            "Monokai",
            "Gruvbox Dark",
            "Nord",
            "Matrix",
        ];
        for p in presets {
            combo.append_text(p);
        }
        combo.set_active(Some(0));
        dialog.content_area().pack_start(&combo, true, true, 8);
        dialog.show_all();
        if dialog.run() == gtk::ResponseType::Ok {
            let name = combo.active_text().unwrap_or_default();
            if let Some(pal) = settings::color_palettes().get(name.as_str()) {
                for (key, (btn, _)) in &btns {
                    if let Some(hex) = pal.get(key.as_str()) {
                        btn.set_rgba(&hex_to_rgba(hex));
                    }
                }
            }
        }
        dialog.close();
    });
    load_btn.set_tooltip_text(Some(
        "Load a built-in color palette preset (Dark, Light, Solarized, Gruvbox, Monokai, Nord)",
    ));
    let save_btn = gtk::Button::with_label("Save As Custom");
    let btns = palette_btns.clone();
    save_btn.connect_clicked(move |_| {
        let mut palette = serde_json::Map::new();
        for (key, (btn, _)) in &btns {
            palette.insert(key.clone(), serde_json::json!(rgba_to_hex(&btn.rgba())));
        }
        let _ = settings().set("custom_palette", serde_json::Value::Object(palette));
    });
    let reset_btn = gtk::Button::with_label("Reset to Default");
    let btns2 = palette_btns.clone();
    reset_btn.connect_clicked(move |_| {
        if let Some(pal) = settings::color_palettes().get("Dark (Default)") {
            for (key, (btn, _)) in &btns2 {
                if let Some(hex) = pal.get(key.as_str()) {
                    btn.set_rgba(&hex_to_rgba(hex));
                }
            }
        }
    });
    btn_box.pack_start(&load_btn, false, false, 0);
    btn_box.pack_start(&save_btn, false, false, 0);
    btn_box.pack_start(&reset_btn, false, false, 0);
    vbox.pack_start(&btn_box, false, false, 0);

    sw.add(&vbox);
    (sw, palette_btns)
}

fn build_compatibility(
    s: &Settings,
) -> (
    gtk::Grid,
    (
        gtk::ComboBoxText,
        gtk::ComboBoxText,
        gtk::ComboBoxText,
        gtk::CheckButton,
    ),
) {
    let grid = gtk::Grid::new();
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_row_spacing(9);
    grid.set_column_spacing(12);
    let mut r = 0;

    r = section(&grid, r, "Keyboard");
    let combo_backspace = gtk::ComboBoxText::new();
    combo_backspace.append_text("Auto-detect");
    combo_backspace.append_text("ASCII DEL (127)");
    combo_backspace.append_text("Escape sequence");
    combo_backspace.append_text("Control-H (8)");
    let bs_map = [
        ("auto", 0),
        ("ascii-del", 1),
        ("escape-sequence", 2),
        ("control-h", 3),
    ];
    let bs = s.get_str_default("backspace_binding", "ascii-del");
    combo_backspace.set_active(Some(
        bs_map
            .iter()
            .find(|(k, _)| *k == bs)
            .map(|(_, v)| *v)
            .unwrap_or(1),
    ));
    combo_backspace.set_tooltip_text(Some(
        "The character sequence sent when Backspace is pressed.",
    ));
    r = row(&grid, r, "Backspace key:", &combo_backspace);

    let combo_delete = gtk::ComboBoxText::new();
    combo_delete.append_text("Auto-detect");
    combo_delete.append_text("Escape sequence");
    combo_delete.append_text("ASCII DEL (127)");
    combo_delete.append_text("Control-H (8)");
    let del_map = [
        ("auto", 0),
        ("escape-sequence", 1),
        ("ascii-del", 2),
        ("control-h", 3),
    ];
    let dl = s.get_str_default("delete_binding", "escape-sequence");
    combo_delete.set_active(Some(
        del_map
            .iter()
            .find(|(k, _)| *k == dl)
            .map(|(_, v)| *v)
            .unwrap_or(1),
    ));
    combo_delete.set_tooltip_text(Some("The character sequence sent when Delete is pressed."));
    r = row(&grid, r, "Delete key:", &combo_delete);

    r += 1;
    r = section(&grid, r, "Encoding");
    let combo_encoding = gtk::ComboBoxText::new();
    let encodings = [
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
    for enc in encodings {
        combo_encoding.append_text(enc);
    }
    let cur_enc = s.get_str_default("encoding", "UTF-8");
    let idx = encodings.iter().position(|e| *e == cur_enc).unwrap_or(0);
    combo_encoding.set_active(Some(idx as u32));
    combo_encoding.set_tooltip_text(Some("Default character encoding for new terminal tabs."));
    r = row(&grid, r, "Default encoding:", &combo_encoding);

    r += 1;
    r = section(&grid, r, "Integration");
    let chk_osc133 = gtk::CheckButton::with_label("Enable shell integration (OSC 133)");
    chk_osc133.set_active(s.get_bool("osc133"));
    chk_osc133.set_tooltip_text(Some(
        "Enables OSC 133 escape sequences for shell integration (bash/zsh).",
    ));
    r = row(&grid, r, "OSC 133:", &chk_osc133);

    r += 1;
    let reset_btn = gtk::Button::with_label("Reset Compatibility Options to Defaults");
    reset_btn.set_tooltip_text(Some(
        "Reset backspace/delete bindings, encoding, and OSC 133 to their default values",
    ));
    let bs = combo_backspace.clone();
    let dl = combo_delete.clone();
    let enc = combo_encoding.clone();
    let osc = chk_osc133.clone();
    reset_btn.connect_clicked(move |_| {
        bs.set_active(Some(1));
        dl.set_active(Some(1));
        enc.set_active(Some(0));
        osc.set_active(false);
    });
    grid.attach(&reset_btn, 0, r, 2, 1);

    (
        grid,
        (combo_backspace, combo_delete, combo_encoding, chk_osc133),
    )
}

fn build_ai(
    s: &Settings,
) -> (
    gtk::ScrolledWindow,
    BTreeMap<String, (gtk::Entry, gtk::Entry, gtk::TextBuffer)>,
    BTreeMap<String, gtk::Entry>,
) {
    let grid = gtk::Grid::new();
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_row_spacing(9);
    grid.set_column_spacing(12);

    let keys = settings::json_to_str_map(&s.get_obj("ai_keys"));
    let models = settings::json_to_str_map(&s.get_obj("ai_models"));
    let urls = settings::json_to_str_map(&s.get_obj("ai_urls"));
    let sys_prompts = settings::json_to_str_map(&s.get_obj("ai_system_prompts"));

    let providers = ["openai", "claude", "gemini", "deepseek", "ollama", "custom"];
    let mut ai_entries: BTreeMap<String, (gtk::Entry, gtk::Entry, gtk::TextBuffer)> =
        BTreeMap::new();
    let mut ai_url_entries: BTreeMap<String, gtk::Entry> = BTreeMap::new();
    let mut rr = 0;

    for provider in providers {
        let title = provider
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + &provider[1..])
            .unwrap_or_default();
        rr = section(&grid, rr, &title);

        let key_entry = gtk::Entry::new();
        key_entry.set_text(keys.get(provider).cloned().unwrap_or_default().as_str());
        if provider != "ollama" && provider != "custom" {
            key_entry.set_visibility(false);
        }
        let ph = if provider == "ollama" || provider == "custom" {
            "(optional)".to_string()
        } else {
            format!("{}_API_KEY", provider.to_uppercase())
        };
        key_entry.set_placeholder_text(Some(&ph));
        rr = row(&grid, rr, "API Key:", &key_entry);

        let model_entry = gtk::Entry::new();
        model_entry.set_text(models.get(provider).cloned().unwrap_or_default().as_str());
        model_entry.set_placeholder_text(Some("default model"));
        rr = row(&grid, rr, "Model:", &model_entry);

        let sys_prompt_buf = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
        sys_prompt_buf.set_text(
            sys_prompts
                .get(provider)
                .cloned()
                .unwrap_or_default()
                .as_str(),
        );
        let sys_prompt_view = gtk::TextView::with_buffer(&sys_prompt_buf);
        sys_prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
        sys_prompt_view.set_size_request(-1, 60);
        let sys_prompt_scroll =
            gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sys_prompt_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sys_prompt_scroll.set_min_content_height(40);
        sys_prompt_scroll.add(&sys_prompt_view);
        let sys_prompt_frame = gtk::Frame::new(None);
        sys_prompt_frame.set_shadow_type(gtk::ShadowType::In);
        sys_prompt_frame.add(&sys_prompt_scroll);
        let lbl = gtk::Label::new(Some("Sys. prompt:"));
        lbl.set_halign(gtk::Align::End);
        lbl.set_margin_end(8);
        lbl.set_valign(gtk::Align::Start);
        lbl.set_margin_top(4);
        grid.attach(&lbl, 0, rr, 1, 1);
        grid.attach(&sys_prompt_frame, 1, rr, 1, 1);
        rr += 1;

        ai_entries.insert(
            provider.to_string(),
            (key_entry, model_entry, sys_prompt_buf),
        );

        if provider == "ollama" || provider == "custom" {
            let url_entry = gtk::Entry::new();
            url_entry.set_text(urls.get(provider).cloned().unwrap_or_default().as_str());
            url_entry.set_placeholder_text(Some(if provider == "ollama" {
                "http://localhost:11434/v1/chat/completions"
            } else {
                "http://localhost:8080/v1/chat/completions"
            }));
            rr = row(&grid, rr, "URL:", &url_entry);
            ai_url_entries.insert(provider.to_string(), url_entry);
        }
        rr += 1;
    }

    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.add(&grid);
    (sw, ai_entries, ai_url_entries)
}

fn build_notes(s: &Settings) -> (gtk::Grid, (gtk::Entry, gtk::Entry, gtk::Entry)) {
    let grid = gtk::Grid::new();
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_row_spacing(9);
    grid.set_column_spacing(12);
    let mut rr = 0;

    rr = section(&grid, rr, "Notes");
    let entry_notes_dir = gtk::Entry::new();
    entry_notes_dir.set_text(&s.get_str("notes_dir"));
    entry_notes_dir.set_placeholder_text(Some(
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
            .as_str(),
    ));
    entry_notes_dir.set_tooltip_text(Some(
        "Directory where note files are stored. Default: home directory",
    ));
    rr = row(&grid, rr, "Notes directory:", &entry_notes_dir);

    let entry_notes_file = gtk::Entry::new();
    entry_notes_file.set_text(&s.get_str("notes_file"));
    entry_notes_file.set_placeholder_text(Some("notes.md"));
    entry_notes_file.set_tooltip_text(Some(
        "Default notes filename. Created automatically when you use /wnotes",
    ));
    rr = row(&grid, rr, "Default notes file:", &entry_notes_file);

    let entry_editor = gtk::Entry::new();
    entry_editor.set_text(&s.get_str_default("editor_command", "nano"));
    entry_editor.set_tooltip_text(Some(
        "Fallback editor for /onotes, used only if xdg-open is unavailable.",
    ));
    row(&grid, rr, "Fallback editor:", &entry_editor);

    (grid, (entry_notes_dir, entry_notes_file, entry_editor))
}

impl DialogState {
    fn apply(&self) {
        let s = settings();
        s.begin_batch();

        s.set_str("tab_title", &self.entry_title.text());
        let idx = self.combo_dynamic.active().unwrap_or(0) as i32;
        let dynamic = match idx {
            1 => "before",
            2 => "after",
            3 => "hide",
            _ => "replace",
        };
        s.set_str("dynamic_title", dynamic);

        s.set_bool("login_shell", self.chk_login.is_active());
        s.set_str("shell_command", &self.entry_shell.text());
        s.set_i64("terminal_columns", self.spin_columns.value() as i64);
        s.set_i64("terminal_rows", self.spin_rows.value() as i64);

        let pos_map = ["right", "left", "disabled"];
        let idx = self.combo_scrollbar_pos.active().unwrap_or(0) as usize;
        s.set_str(
            "scrollbar_position",
            pos_map.get(idx).copied().unwrap_or("right"),
        );
        let scrollback = if self.chk_scrollback_unlimited.is_active() {
            -1
        } else {
            self.spin_scrollback.value() as i64
        };
        s.set_i64("scrollback_lines", scrollback);
        s.set_bool("scroll_on_output", self.chk_scroll_output.is_active());
        s.set_bool("scroll_on_keystroke", self.chk_scroll_keystroke.is_active());
        s.set_bool("confirm_close", self.chk_confirm_close.is_active());
        s.set_bool("auto_copy_selection", self.chk_auto_copy.is_active());
        s.set_bool("show_unsafe_paste_dialog", self.chk_warn_paste.is_active());
        s.set_str("file_manager", &self.entry_fm.text());
        s.set_bool("session_restore", self.chk_session_restore.is_active());
        s.set_bool("bell_notification", self.chk_bell_notify.is_active());
        s.set_bool("hint_mode_enabled", self.chk_hint_mode.is_active());
        s.set_bool("vi_copy_mode_enabled", self.chk_vi_copy.is_active());

        let font_name = self
            .font_btn
            .font()
            .map(|f| f.to_string())
            .unwrap_or_default();
        let mut parts = font_name.rsplitn(2, ' ');
        let size = parts.next().unwrap_or("").to_string();
        let family = parts.next().unwrap_or(font_name.as_str()).to_string();
        s.set_str("font_name", &family);
        if let Ok(size) = size.parse::<i64>() {
            s.set_i64("font_size", size);
        }
        s.set_bool("allow_bold_text", self.chk_bold.is_active());

        if let Some(name) = self.combo_scheme.active_text() {
            s.set_str("color_scheme", &name);
        }

        let foreground = if self.fg_override.get() {
            rgba_to_hex(&self.fg_color_btn.rgba())
        } else {
            String::new()
        };
        let background = if self.bg_override.get() {
            rgba_to_hex(&self.bg_color_btn.rgba())
        } else {
            String::new()
        };
        s.set_str("foreground_color", &foreground);
        s.set_str("background_color", &background);
        s.set_str("cursor_color", &rgba_to_hex(&self.cursor_color_btn.rgba()));
        s.set_str("highlight_color", &rgba_to_hex(&self.highlight_btn.rgba()));
        s.set_str(
            "highlight_bg_color",
            &rgba_to_hex(&self.highlight_bg_btn.rgba()),
        );
        s.set_str(
            "tab_title_color",
            &rgba_to_hex(&self.tab_title_color_btn.rgba()),
        );
        s.set_str(
            "tab_active_title_color",
            &rgba_to_hex(&self.tab_active_color_btn.rgba()),
        );

        let shapes = ["block", "underline", "ibeam"];
        let idx = self.combo_cursor_shape.active().unwrap_or(0) as usize;
        s.set_str("cursor_shape", shapes.get(idx).copied().unwrap_or("block"));
        s.set_bool("cursor_blink", self.chk_cursor_blink.is_active());
        let _ = s.set("opacity", serde_json::json!(self.spin_opacity.value()));
        s.set_bool("enable_transparency", self.chk_transparency.is_active());
        s.set_i64("window_padding_horizontal", self.spin_pad_h.value() as i64);
        s.set_i64("window_padding_vertical", self.spin_pad_v.value() as i64);
        let curl_styles = ["single", "double", "curly", "dashed", "dotted"];
        let idx = self.combo_undercurl.active().unwrap_or(0) as usize;
        s.set_str(
            "undercurl_style",
            curl_styles.get(idx).copied().unwrap_or("single"),
        );

        let mut palette = BTreeMap::new();
        for (key, (btn, _)) in self.palette_btns.borrow().iter() {
            palette.insert(key.clone(), serde_json::json!(rgba_to_hex(&btn.rgba())));
        }
        let scheme_name = self.combo_scheme.active_text().unwrap_or_default();
        let preset = settings::color_palettes()
            .get(scheme_name.as_str())
            .unwrap_or_else(|| settings::color_palettes().get("Dark (Default)").unwrap());
        let preset_json: BTreeMap<String, serde_json::Value> = preset
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        if palette == preset_json {
            let _ = s.set("custom_palette", serde_json::Value::Null);
        } else {
            let mut obj = serde_json::Map::new();
            for (k, v) in palette {
                obj.insert(k, v);
            }
            let _ = s.set("custom_palette", serde_json::Value::Object(obj));
        }

        let bs_map = ["auto", "ascii-del", "escape-sequence", "control-h"];
        let idx = self.combo_backspace.active().unwrap_or(0) as usize;
        s.set_str(
            "backspace_binding",
            bs_map.get(idx).copied().unwrap_or("ascii-del"),
        );
        let del_map = ["auto", "escape-sequence", "ascii-del", "control-h"];
        let idx = self.combo_delete.active().unwrap_or(0) as usize;
        s.set_str(
            "delete_binding",
            del_map.get(idx).copied().unwrap_or("escape-sequence"),
        );
        let _ = s.set_str(
            "encoding",
            &self
                .combo_encoding
                .active_text()
                .map(|t| t.to_string())
                .unwrap_or_else(|| "UTF-8".to_string()),
        );

        let osc133_was_enabled = s.get_bool("osc133");
        let osc133_now_enabled = self.chk_osc133.is_active();
        s.set_bool("osc133", osc133_now_enabled);

        let mut new_keys = BTreeMap::new();
        let mut new_models = BTreeMap::new();
        let mut new_urls = BTreeMap::new();
        let mut new_sys_prompts = BTreeMap::new();
        for (provider, (key_entry, model_entry, buf)) in &self.ai_entries {
            new_keys.insert(provider.clone(), key_entry.text().to_string());
            new_models.insert(provider.clone(), model_entry.text().to_string());
            let prompt = buf
                .text(&buf.start_iter(), &buf.end_iter(), true)
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if !prompt.is_empty() {
                new_sys_prompts.insert(provider.clone(), prompt);
            }
        }
        for (provider, url_entry) in &self.ai_url_entries {
            new_urls.insert(provider.clone(), url_entry.text().to_string());
        }
        let _ = s.set("ai_keys", settings::str_map_to_json(&new_keys));
        let _ = s.set("ai_models", settings::str_map_to_json(&new_models));
        let _ = s.set("ai_urls", settings::str_map_to_json(&new_urls));
        let _ = s.set(
            "ai_system_prompts",
            settings::str_map_to_json(&new_sys_prompts),
        );

        s.set_str("notes_dir", &self.entry_notes_dir.text());
        s.set_str("notes_file", &self.entry_notes_file.text());
        s.set_str("editor_command", &self.entry_editor.text());

        s.end_batch();
        s.notify_changed();

        if osc133_now_enabled && !osc133_was_enabled {
            write_osc_setup_script();
        }
    }
}

fn write_osc_setup_script() {
    let dir = settings::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("osc-setup.sh");
    let script = r#"#!/bin/bash
# TPGK OSC 133 Shell Integration Setup
# Run this script to add OSC 133 integration to your shell config.
#   bash ~/.config/tpgk/osc-setup.sh

OSC133_LINE='[ -f ~/.config/tpgk/osc133.sh ] && source ~/.config/tpgk/osc133.sh'

for rc in ~/.bashrc ~/.zshrc; do
    if [ -f "$rc" ]; then
        if ! grep -qF "osc133.sh" "$rc" 2>/dev/null; then
            printf '\n# TPGK OSC 133 Shell Integration\n%s\n' "$OSC133_LINE" >> "$rc"
            echo "Added OSC 133 integration to $rc"
        else
            echo "OSC 133 already configured in $rc"
        fi
    fi
done

echo "Done. Restart your shell or run: source ~/.bashrc"
"#;
    let _ = std::fs::write(&path, script);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
}
