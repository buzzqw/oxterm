//! Text extraction that works on the whole range of supported VTE versions.
//!
//! `vte_terminal_get_text_range_format()` was introduced in VTE 0.72 and is the
//! preferred API, but Oxterm also builds against VTE 0.70 (Debian 12). The
//! older `vte_terminal_get_text_range()` is deprecated upstream yet exported by
//! every VTE release since 0.40 (including current ones), so it is used here as
//! the single code path and only plain text is requested.

use glib::translate::{from_glib_full, ToGlibPtr};
use zoha_vte::Terminal;

/// Return the text between two buffer positions, or `None` if there is none.
pub(crate) fn range(
    terminal: &Terminal,
    start_row: i64,
    start_col: i64,
    end_row: i64,
    end_col: i64,
) -> Option<String> {
    // The last argument is a nullable GArray of VteCharAttributes, not a
    // length: pass NULL because only the text is needed.
    // The deprecated API reads `end_col = -1` as "start of the range end", not
    // "end of line" like the 0.72 API does, so extend the range to the next row.
    let (end_row, end_col) = if end_col < 0 {
        (end_row + 1, 0)
    } else {
        (end_row, end_col)
    };
    let ptr = unsafe {
        zoha_vte_sys::vte_terminal_get_text_range(
            terminal.to_glib_none().0,
            start_row as _,
            start_col as _,
            end_row as _,
            end_col as _,
            None,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ptr.is_null() {
        return None;
    }
    let text: Option<glib::GString> = unsafe { from_glib_full(ptr) };
    text.map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    use zoha_vte::traits::TerminalExt;

    #[test]
    fn extracts_text_from_fed_output() {
        if gtk::init().is_err() {
            return;
        }
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let terminal = Terminal::new();
        window.add(&terminal);
        window.show_all();
        terminal.feed(b"hello from vte\r\n");
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut text = String::new();
        while std::time::Instant::now() < deadline {
            while context.iteration(false) {}
            text = range(&terminal, 0, 0, 0, -1).unwrap_or_default();
            if !text.trim().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(text.contains("hello from vte"), "unexpected text: {text:?}");
    }
}
