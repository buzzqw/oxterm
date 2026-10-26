//! VI-style keyboard copy mode.
//!
//! The mode works on the buffer rows reported by the vertical adjustment and
//! extracts text with `text::range`, so it is independent of VTE's own
//! selection API. Motions, visual modes, text objects and bookmarks are kept
//! in small pure helpers at the top of this file; the methods below only move
//! the cursor, redraw the overlay and put the yanked text on the clipboard.

use super::*;

/// Keep at least this many lines of context above and below the VI cursor.
const VI_SCROLLOFF: f64 = 3.0;

// ── Pure helpers (unit tested without a display) ─────────────

fn is_word_char(c: char, big: bool) -> bool {
    if big {
        !c.is_whitespace()
    } else {
        c.is_alphanumeric() || c == '_'
    }
}

/// Start of the next word after `col` (as Vim's `w`), if it is on this line.
pub(crate) fn next_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col.min(chars.len());
    if i < chars.len() && is_word_char(chars[i], big) {
        while i < chars.len() && is_word_char(chars[i], big) {
            i += 1;
        }
    }
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    (i < chars.len()).then_some(i)
}

/// Start of the word before `col` (as Vim's `b`), if it is on this line.
pub(crate) fn prev_word_start(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    // Vim's `b` from the first character of a word goes to the previous word,
    // not to the current one.
    if i > 0 && is_word_char(chars[i], big) && !is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    if i > 0 && !is_word_char(chars[i], big) && is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    while i > 0 && !is_word_char(chars[i], big) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1], big) {
        i -= 1;
    }
    Some(i)
}

/// End of the current or next word (as Vim's `e`), if it is on this line.
pub(crate) fn word_end(line: &str, col: usize, big: bool) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    while i + 1 < chars.len() && is_word_char(chars[i + 1], big) {
        i += 1;
    }
    Some(i)
}

fn quote_pair(line: &str, col: usize, quote: char) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut open: Option<usize> = None;
    let mut first: Option<(usize, usize)> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c != quote {
            continue;
        }
        match open.take() {
            None => open = Some(i),
            Some(o) => {
                if o <= col && col <= i {
                    return Some((o, i));
                }
                first.get_or_insert((o, i));
            }
        }
    }
    first
}

fn bracket_pair(line: &str, col: usize, open: char, close: char) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut stack: Vec<usize> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    let mut first_after: Option<(usize, usize)> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == open {
            stack.push(i);
        } else if c == close {
            if let Some(o) = stack.pop() {
                if o <= col && col <= i {
                    if best.map_or(true, |(bo, bi)| o >= bo && i <= bi) {
                        best = Some((o, i));
                    }
                } else if o >= col && first_after.is_none() {
                    first_after = Some((o, i));
                }
            }
        }
    }
    best.or(first_after)
}

fn word_object(line: &str, col: usize, big: bool, around: bool) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = col.min(chars.len() - 1);
    while i < chars.len() && !is_word_char(chars[i], big) {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let mut start = i;
    while start > 0 && is_word_char(chars[start - 1], big) {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < chars.len() && is_word_char(chars[end + 1], big) {
        end += 1;
    }
    if around {
        if end + 1 < chars.len() && chars[end + 1].is_whitespace() {
            let mut j = end + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            end = j - 1;
        } else if start > 0 && chars[start - 1].is_whitespace() {
            let mut j = start;
            while j > 0 && chars[j - 1].is_whitespace() {
                j -= 1;
            }
            start = j;
        }
    }
    Some((start, end))
}

/// Inclusive character range of a text object on one line.
///
/// `kind` is one of `w`/`W` (word), `"`/`'`/`` ` `` (quotes), `(`/`[`/`{`/`<`
/// (brackets) or their closing counterparts.
pub(crate) fn text_object_range(
    line: &str,
    col: usize,
    kind: char,
    around: bool,
) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let col = col.min(chars.len() - 1);
    match kind {
        'w' => word_object(line, col, false, around),
        'W' => word_object(line, col, true, around),
        '"' | '\'' | '`' => {
            let (open, close) = quote_pair(line, col, kind)?;
            if around {
                Some((open, close))
            } else if close > open + 1 {
                Some((open + 1, close - 1))
            } else {
                Some((open + 1, open + 1))
            }
        }
        '(' | ')' => {
            let (open, close) = bracket_pair(line, col, '(', ')')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '[' | ']' => {
            let (open, close) = bracket_pair(line, col, '[', ']')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '{' | '}' => {
            let (open, close) = bracket_pair(line, col, '{', '}')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        '<' | '>' => {
            let (open, close) = bracket_pair(line, col, '<', '>')?;
            Some(if around {
                (open, close)
            } else {
                (open + 1, close.saturating_sub(1))
            })
        }
        _ => None,
    }
}

fn chars_slice(line: &str, start: usize, end: usize) -> String {
    line.chars()
        .skip(start)
        .take(end.saturating_sub(start) + 1)
        .collect()
}

// ── Mode ─────────────────────────────────────────────────────

impl TerminalBox {
    pub(super) fn activate_vi_copy(&self) {
        if *self.imp().vi_copy_active.borrow() {
            return;
        }
        *self.imp().vi_copy_active.borrow_mut() = true;
        self.imp().vi_visual_mode.set(ViVisualMode::None);
        self.imp().vi_pending.borrow_mut().clear();
        self.imp().vi_count.borrow_mut().clear();
        *self.imp().vi_last_key.borrow_mut() = None;
        *self.imp().vi_last_key_time.borrow_mut() = 0;

        let (col, row) = self.vte().cursor_position();
        let total = self.vi_total_rows();
        let row = row.clamp(0, (total - 1).max(0));
        let col = self.vi_clamp_col(row, col);
        self.imp().vi_cursor_row.set(row);
        self.imp().vi_cursor_col.set(col);
        self.imp().vi_anchor_row.set(row);
        self.imp().vi_anchor_col.set(col);
        *self.imp().scroll_follow.borrow_mut() = false;

        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.show();
            area.queue_draw();
        }
        self.vi_scroll_to_cursor();
    }

    pub(super) fn deactivate_vi_copy(&self) {
        *self.imp().vi_copy_active.borrow_mut() = false;
        self.imp().vi_visual_mode.set(ViVisualMode::None);
        self.imp().vi_pending.borrow_mut().clear();
        self.imp().vi_count.borrow_mut().clear();
        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.hide();
        }
    }

    // ── Buffer helpers ────────────────────────────────────────

    fn vi_total_rows(&self) -> i64 {
        self.vadjustment().upper().max(1.0) as i64
    }

    fn vi_line(&self, row: i64) -> String {
        // `end_col = -1` means "to the start of the next row", i.e. this whole
        // line: `end_col = 0` would describe an empty range.
        let text = text::range(&self.vte(), row, 0, row, -1).unwrap_or_default();
        text.trim_end_matches(['\n', '\r']).to_string()
    }

    fn vi_line_len(&self, row: i64) -> i64 {
        self.vi_line(row).chars().count().max(1) as i64
    }

    fn vi_clamp_col(&self, row: i64, col: i64) -> i64 {
        col.clamp(0, self.vi_line_len(row) - 1)
    }

    fn vi_set_cursor(&self, row: i64, col: i64) {
        let row = row.clamp(0, (self.vi_total_rows() - 1).max(0));
        let col = self.vi_clamp_col(row, col);
        self.imp().vi_cursor_row.set(row);
        self.imp().vi_cursor_col.set(col);
        self.vi_scroll_to_cursor();
        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.queue_draw();
        }
    }

    fn vi_scroll_to_cursor(&self) {
        let vadj = self.vadjustment();
        let page = vadj.page_size();
        if page <= 0.0 {
            return;
        }
        let row = self.imp().vi_cursor_row.get() as f64;
        let top = vadj.value();
        let mut target = top;
        if row < top + VI_SCROLLOFF {
            target = row - VI_SCROLLOFF;
        } else if row > top + page - VI_SCROLLOFF - 1.0 {
            target = row - page + VI_SCROLLOFF + 1.0;
        }
        let bottom = (vadj.upper() - page).max(0.0);
        let target = target.clamp(0.0, bottom);
        if (target - top).abs() > 0.01 {
            vadj.set_value(target);
        }
    }

    fn vi_redraw(&self) {
        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.queue_draw();
        }
    }

    fn vi_take_count(&self) -> usize {
        let raw = std::mem::take(&mut *self.imp().vi_count.borrow_mut());
        raw.parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(1)
    }

    fn vi_visual_range(&self) -> (i64, i64, i64, i64) {
        let (r1, c1) = (
            self.imp().vi_anchor_row.get(),
            self.imp().vi_anchor_col.get(),
        );
        let (r2, c2) = (
            self.imp().vi_cursor_row.get(),
            self.imp().vi_cursor_col.get(),
        );
        if (r2, c2) < (r1, c1) {
            (r2, c2, r1, c1)
        } else {
            (r1, c1, r2, c2)
        }
    }

    fn vi_extract(&self, mode: ViVisualMode, r1: i64, c1: i64, r2: i64, c2: i64) -> String {
        let total = self.vi_total_rows();
        let r1 = r1.clamp(0, total - 1);
        let r2 = r2.clamp(r1, total - 1);
        let mut lines = Vec::new();
        for row in r1..=r2 {
            let line = self.vi_line(row);
            let len = line.chars().count();
            if len == 0 {
                lines.push(String::new());
                continue;
            }
            match mode {
                ViVisualMode::Line => lines.push(line),
                ViVisualMode::Block => {
                    let start = c1.min(c2).max(0) as usize;
                    let end = c1.max(c2).max(0) as usize;
                    if start < len {
                        lines.push(chars_slice(&line, start, end.min(len - 1)));
                    } else {
                        lines.push(String::new());
                    }
                }
                ViVisualMode::Char | ViVisualMode::None => {
                    let start = if row == r1 { c1.max(0) as usize } else { 0 };
                    let end = if row == r2 {
                        c2.max(0) as usize
                    } else {
                        len.saturating_sub(1)
                    };
                    if start < len {
                        lines.push(chars_slice(&line, start, end.min(len - 1)));
                    } else {
                        lines.push(String::new());
                    }
                }
            }
        }
        lines.join("\n")
    }

    fn vi_copy_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        clipboard.set_text(text);
        self.deactivate_vi_copy();
    }

    // ── Marks ─────────────────────────────────────────────────

    fn vi_marks(&self) -> Vec<i64> {
        let mut marks: Vec<i64> = self
            .imp()
            .osc133_markers
            .borrow()
            .iter()
            .filter(|(_, kind, _)| kind == "prompt")
            .map(|(row, _, _)| *row)
            .collect();
        marks.extend(self.imp().vi_user_marks.borrow().iter().copied());
        marks.sort_unstable();
        marks.dedup();
        marks
    }

    fn vi_toggle_mark(&self) {
        let row = self.imp().vi_cursor_row.get();
        let mut marks = self.imp().vi_user_marks.borrow_mut();
        if let Some(index) = marks.iter().position(|mark| *mark == row) {
            marks.remove(index);
        } else {
            marks.push(row);
            marks.sort_unstable();
        }
        drop(marks);
        self.refresh_command_marks();
    }

    fn vi_jump_mark(&self, forward: bool, count: usize) {
        let marks = self.vi_marks();
        let mut row = self.imp().vi_cursor_row.get();
        for _ in 0..count {
            if forward {
                match marks.iter().find(|mark| **mark > row) {
                    Some(mark) => row = *mark,
                    None => break,
                }
            } else {
                match marks.iter().rev().find(|mark| **mark < row) {
                    Some(mark) => row = *mark,
                    None => break,
                }
            }
        }
        self.vi_set_cursor(row, 0);
    }

    // ── Keys ──────────────────────────────────────────────────

    pub(super) fn handle_vi_copy_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        let ctrl = event.state().contains(gdk::ModifierType::CONTROL_MASK);
        let visual = self.imp().vi_visual_mode.get();

        if key == K::Escape {
            self.imp().vi_pending.borrow_mut().clear();
            self.imp().vi_count.borrow_mut().clear();
            if visual != ViVisualMode::None {
                self.imp().vi_visual_mode.set(ViVisualMode::None);
                self.vi_redraw();
            } else {
                self.deactivate_vi_copy();
            }
            return glib::Propagation::Stop;
        }
        if (key == K::c || key == K::C) && ctrl {
            self.deactivate_vi_copy();
            return glib::Propagation::Stop;
        }
        if ctrl {
            let count = self.vi_take_count() as i64;
            match key {
                K::u | K::U => self.vi_page(-count),
                K::d | K::D => self.vi_page(count),
                K::f | K::F => self.vi_page(count * 2),
                K::b | K::B => self.vi_page(-count * 2),
                K::v | K::V => self.vi_set_visual(ViVisualMode::Block),
                _ => {}
            }
            return glib::Propagation::Stop;
        }
        if key == K::slash || key == K::question {
            self.imp().vi_pending.borrow_mut().clear();
            self.show_search();
            return glib::Propagation::Stop;
        }

        let typed = event_text(event);
        let Some(ch) = typed.chars().next() else {
            return glib::Propagation::Stop;
        };

        // Count prefixes: `0` is a motion unless a count is already being built.
        if ch.is_ascii_digit() {
            let mut count = self.imp().vi_count.borrow_mut();
            if ch != '0' || !count.is_empty() {
                count.push(ch);
                return glib::Propagation::Stop;
            }
        }

        let pending = self.imp().vi_pending.borrow().clone();
        if !pending.is_empty() {
            self.vi_run_pending(&pending, ch, visual);
            return glib::Propagation::Stop;
        }

        match ch {
            'h' => self.vi_move(0, -(self.vi_take_count() as i64)),
            'l' => self.vi_move(0, self.vi_take_count() as i64),
            'j' => self.vi_move(self.vi_take_count() as i64, 0),
            'k' => self.vi_move(-(self.vi_take_count() as i64), 0),
            '0' => self.vi_set_cursor(self.imp().vi_cursor_row.get(), 0),
            '^' => {
                let row = self.imp().vi_cursor_row.get();
                let line = self.vi_line(row);
                let col = line.chars().position(|c| !c.is_whitespace()).unwrap_or(0) as i64;
                self.vi_set_cursor(row, col);
            }
            '$' => {
                let row = self.imp().vi_cursor_row.get();
                let col = self.vi_line_len(row) - 1;
                self.vi_set_cursor(row, col);
            }
            '|' => {
                let count = self.vi_take_count() as i64;
                let row = self.imp().vi_cursor_row.get();
                self.vi_set_cursor(row, count - 1);
            }
            'w' => self.vi_word_forward(false),
            'W' => self.vi_word_forward(true),
            'b' => self.vi_word_backward(false),
            'B' => self.vi_word_backward(true),
            'e' => self.vi_word_end(false),
            'E' => self.vi_word_end(true),
            '{' => self.vi_jump_mark(false, self.vi_take_count()),
            '}' => self.vi_jump_mark(true, self.vi_take_count()),
            'v' => self.vi_set_visual(ViVisualMode::Char),
            'V' => self.vi_set_visual(ViVisualMode::Line),
            'g' => self.imp().vi_pending.borrow_mut().push('g'),
            '[' => self.imp().vi_pending.borrow_mut().push('['),
            ']' => self.imp().vi_pending.borrow_mut().push(']'),
            'm' => self.imp().vi_pending.borrow_mut().push('m'),
            'y' => {
                if visual == ViVisualMode::None {
                    self.imp().vi_pending.borrow_mut().push('y');
                } else {
                    let (r1, c1, r2, c2) = self.vi_visual_range();
                    let text = self.vi_extract(visual, r1, c1, r2, c2);
                    self.vi_copy_text(&text);
                }
            }
            'Y' => {
                let row = self.imp().vi_cursor_row.get();
                let text = self.vi_extract(ViVisualMode::Line, row, 0, row, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            'i' | 'a' if visual != ViVisualMode::None => {
                self.imp().vi_pending.borrow_mut().push(ch);
            }
            'q' => self.deactivate_vi_copy(),
            'G' => {
                let count = self.vi_take_count() as i64;
                let total = self.vi_total_rows();
                let row = if count > 1 {
                    (count - 1).min(total - 1)
                } else {
                    total - 1
                };
                self.vi_set_cursor(row, 0);
            }
            _ => {}
        }
        self.vi_redraw();
        glib::Propagation::Stop
    }

    fn vi_run_pending(&self, pending: &str, ch: char, visual: ViVisualMode) {
        let mut pending_borrow = self.imp().vi_pending.borrow_mut();
        pending_borrow.clear();
        drop(pending_borrow);
        match pending {
            "g" => {
                if ch == 'g' {
                    let count = self.vi_take_count() as i64;
                    let total = self.vi_total_rows();
                    let row = if count > 1 {
                        (count - 1).min(total - 1)
                    } else {
                        0
                    };
                    self.vi_set_cursor(row, 0);
                }
            }
            "m" => {
                if ch == 'm' {
                    self.vi_toggle_mark();
                }
            }
            "[" | "]" => {
                if ch == 'm' {
                    let forward = pending == "]";
                    self.vi_jump_mark(forward, self.vi_take_count());
                }
            }
            "y" => {
                if ch == 'i' || ch == 'a' {
                    self.imp().vi_pending.borrow_mut().push(ch);
                } else if ch == 'g' {
                    self.imp().vi_pending.borrow_mut().push_str("yg");
                } else {
                    self.vi_yank_operator(ch);
                }
            }
            "yg" => {
                if ch == 'g' {
                    self.vi_yank_operator('g');
                }
            }
            "yi" | "ya" => {
                if let Some((mode, r1, c1, r2, c2)) = self.vi_object_range(ch, pending == "ya") {
                    let text = self.vi_extract(mode, r1, c1, r2, c2);
                    self.vi_copy_text(&text);
                }
            }
            "i" | "a" if visual != ViVisualMode::None => {
                if let Some((mode, r1, c1, r2, c2)) = self.vi_object_range(ch, pending == "a") {
                    self.imp().vi_visual_mode.set(mode);
                    self.imp().vi_anchor_row.set(r1);
                    self.imp().vi_anchor_col.set(c1);
                    self.imp().vi_cursor_row.set(r2);
                    self.imp().vi_cursor_col.set(c2);
                    self.vi_redraw();
                }
            }
            _ => {}
        }
    }

    fn vi_object_range(
        &self,
        kind: char,
        around: bool,
    ) -> Option<(ViVisualMode, i64, i64, i64, i64)> {
        let row = self.imp().vi_cursor_row.get();
        if kind == 'm' {
            let marks = self.vi_marks();
            let current = row;
            let previous = marks.iter().rev().find(|mark| **mark < current).copied();
            let next = marks.iter().find(|mark| **mark > current).copied();
            let (Some(previous), Some(next)) = (previous, next) else {
                return None;
            };
            let (start, end) = if around {
                (previous, next)
            } else {
                (previous + 1, next - 1)
            };
            if start > end {
                return None;
            }
            return Some((ViVisualMode::Line, start, 0, end, 0));
        }
        let line = self.vi_line(row);
        let col = self.imp().vi_cursor_col.get();
        text_object_range(&line, col.max(0) as usize, kind, around)
            .map(|(start, end)| (ViVisualMode::Char, row, start as i64, row, end as i64))
    }

    /// `y` followed by a motion. Ranges are inclusive, as in Vim.
    fn vi_yank_operator(&self, motion: char) {
        let count = self.vi_take_count() as i64;
        let total = self.vi_total_rows();
        let row = self.imp().vi_cursor_row.get();
        let col = self.imp().vi_cursor_col.get();
        let line_len = self.vi_line_len(row) - 1;
        match motion {
            'y' | 'Y' => {
                let end = (row + count - 1).min(total - 1);
                let text = self.vi_extract(ViVisualMode::Line, row, 0, end, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            'j' => {
                let end = (row + count).min(total - 1);
                let text = self.vi_extract(ViVisualMode::Line, row, 0, end, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            'k' => {
                let start = (row - count).max(0);
                let text = self.vi_extract(ViVisualMode::Line, start, 0, row, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            'G' => {
                let text = self.vi_extract(ViVisualMode::Line, row, 0, total - 1, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            'g' => {
                let text = self.vi_extract(ViVisualMode::Line, 0, 0, row, 0);
                self.vi_copy_text(&(text + "\n"));
            }
            '0' | '^' => {
                let text = self.vi_extract(ViVisualMode::Char, row, 0, row, col - 1);
                self.vi_copy_text(&text);
            }
            '$' => {
                let text = self.vi_extract(ViVisualMode::Char, row, col, row, line_len);
                self.vi_copy_text(&text);
            }
            'w' | 'W' => {
                let line = self.vi_line(row);
                let big = motion == 'W';
                let end = next_word_start(&line, col.max(0) as usize, big)
                    .map(|next| next as i64 - 1)
                    .unwrap_or(line_len);
                let text = self.vi_extract(ViVisualMode::Char, row, col, row, end);
                self.vi_copy_text(&text);
            }
            'b' | 'B' => {
                let line = self.vi_line(row);
                let big = motion == 'B';
                let start = prev_word_start(&line, col.max(0) as usize, big)
                    .map(|prev| prev as i64)
                    .unwrap_or(0);
                let text = self.vi_extract(ViVisualMode::Char, row, start, row, (col - 1).max(0));
                self.vi_copy_text(&text);
            }
            'e' | 'E' => {
                let line = self.vi_line(row);
                let big = motion == 'E';
                let end = word_end(&line, col.max(0) as usize, big)
                    .map(|end| end as i64)
                    .unwrap_or(line_len);
                let text = self.vi_extract(ViVisualMode::Char, row, col, row, end);
                self.vi_copy_text(&text);
            }
            _ => {}
        }
    }

    fn vi_move(&self, drow: i64, dcol: i64) {
        let row = self.imp().vi_cursor_row.get() + drow;
        let col = self.imp().vi_cursor_col.get() + dcol;
        self.vi_set_cursor(row, col);
    }

    fn vi_page(&self, half_pages: i64) {
        let vadj = self.vadjustment();
        let page = vadj.page_size().max(1.0) as i64;
        let delta = (page / 2).max(1) * half_pages;
        let row = self.imp().vi_cursor_row.get() + delta;
        self.vi_set_cursor(row, self.imp().vi_cursor_col.get());
    }

    fn vi_word_forward(&self, big: bool) {
        let count = self.vi_take_count();
        let total = self.vi_total_rows();
        let mut row = self.imp().vi_cursor_row.get();
        let mut col = self.imp().vi_cursor_col.get();
        for _ in 0..count {
            let line = self.vi_line(row);
            if let Some(next) = next_word_start(&line, col.max(0) as usize, big) {
                col = next as i64;
            } else if row + 1 < total {
                row += 1;
                col = 0;
            } else {
                col = self.vi_line_len(row) - 1;
            }
        }
        self.vi_set_cursor(row, col);
    }

    fn vi_word_backward(&self, big: bool) {
        let count = self.vi_take_count();
        let mut row = self.imp().vi_cursor_row.get();
        let mut col = self.imp().vi_cursor_col.get();
        for _ in 0..count {
            if col == 0 {
                if row == 0 {
                    break;
                }
                row -= 1;
                let line = self.vi_line(row);
                let end = (self.vi_line_len(row) - 1) as usize;
                col = prev_word_start(&line, end, big).unwrap_or(0) as i64;
                continue;
            }
            let line = self.vi_line(row);
            if let Some(previous) = prev_word_start(&line, col.max(0) as usize, big) {
                if previous as i64 == col {
                    if let Some(previous) = prev_word_start(&line, (col - 1).max(0) as usize, big) {
                        col = previous as i64;
                    } else if row > 0 {
                        row -= 1;
                        col = self.vi_line_len(row) - 1;
                    }
                } else {
                    col = previous as i64;
                }
            } else if row > 0 {
                row -= 1;
                col = self.vi_line_len(row) - 1;
            } else {
                col = 0;
            }
        }
        self.vi_set_cursor(row, col);
    }

    fn vi_word_end(&self, big: bool) {
        let count = self.vi_take_count();
        let total = self.vi_total_rows();
        let mut row = self.imp().vi_cursor_row.get();
        let mut col = self.imp().vi_cursor_col.get();
        for _ in 0..count {
            let line = self.vi_line(row);
            match word_end(&line, col.max(0) as usize, big) {
                Some(end) => col = end as i64,
                None if row + 1 < total => {
                    row += 1;
                    col = 0;
                    let next_line = self.vi_line(row);
                    if let Some(end) = word_end(&next_line, 0, big) {
                        col = end as i64;
                    }
                }
                None => {
                    col = self.vi_line_len(row) - 1;
                }
            }
        }
        self.vi_set_cursor(row, col);
    }

    fn vi_set_visual(&self, mode: ViVisualMode) {
        let current = self.imp().vi_visual_mode.get();
        if current == mode {
            self.imp().vi_visual_mode.set(ViVisualMode::None);
        } else {
            if current == ViVisualMode::None {
                self.imp().vi_anchor_row.set(self.imp().vi_cursor_row.get());
                self.imp().vi_anchor_col.set(self.imp().vi_cursor_col.get());
            }
            self.imp().vi_visual_mode.set(mode);
        }
        self.vi_redraw();
    }

    // ── Drawing ───────────────────────────────────────────────

    fn vi_span(&self, row: i64) -> Option<(i64, i64)> {
        let mode = self.imp().vi_visual_mode.get();
        if mode == ViVisualMode::None {
            return None;
        }
        let (r1, c1, r2, c2) = self.vi_visual_range();
        if row < r1 || row > r2 {
            return None;
        }
        let len = self.vi_line_len(row) - 1;
        match mode {
            ViVisualMode::Line => Some((0, len)),
            ViVisualMode::Block => Some((c1.min(c2).clamp(0, len), c1.max(c2).clamp(0, len))),
            ViVisualMode::Char | ViVisualMode::None => {
                let start = if row == r1 { c1 } else { 0 };
                let end = if row == r2 { c2 } else { len };
                Some((start.clamp(0, len), end.clamp(0, len)))
            }
        }
    }

    pub(super) fn draw_vi_overlay(&self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        if !*self.imp().vi_copy_active.borrow() {
            return;
        }
        let vadj = self.vadjustment();
        let top = vadj.value();
        let page = vadj.page_size();
        if page <= 0.0 {
            return;
        }
        let vte = self.vte();
        let cw = vte.char_width() as f64;
        let ch = vte.char_height() as f64;
        if cw <= 0.0 || ch <= 0.0 {
            return;
        }
        let padding = settings().get_i64("window_padding_horizontal") as f64;
        let gutter = if self
            .imp()
            .osc133_margin
            .borrow()
            .as_ref()
            .is_some_and(|margin| margin.is_visible())
        {
            OSC133_GUTTER_WIDTH as f64
        } else {
            0.0
        };
        let x0 = gutter + padding + 8.0;
        let height = area.allocated_height() as f64;
        let width = area.allocated_width() as f64;

        for row in (top as i64)..(top as i64 + page as i64 + 1) {
            let Some((start, end)) = self.vi_span(row) else {
                continue;
            };
            let y = (row as f64 - top) * ch;
            if y < -ch || y > height {
                continue;
            }
            let x = x0 + start as f64 * cw;
            let w = ((end - start + 1).max(1) as f64 * cw).min(width - x);
            cr.set_source_rgba(0.30, 0.55, 0.85, 0.32);
            cr.rectangle(x, y, w, ch);
            let _ = cr.fill();
        }

        let row = self.imp().vi_cursor_row.get();
        let col = self.imp().vi_cursor_col.get();
        let y = (row as f64 - top) * ch;
        if y >= -ch && y <= height {
            let x = x0 + col as f64 * cw;
            cr.set_source_rgba(0.988, 0.816, 0.31, 0.35);
            cr.rectangle(x, y, cw, ch);
            let _ = cr.fill();
            cr.set_source_rgba(0.988, 0.816, 0.31, 0.9);
            cr.set_line_width(1.0);
            cr.rectangle(x + 0.5, y + 0.5, cw - 1.0, ch - 1.0);
            let _ = cr.stroke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_motions_follow_vim_semantics() {
        let line = "let mut value = foo(bar);";
        assert_eq!(next_word_start(line, 0, false), Some(4));
        assert_eq!(next_word_start(line, 4, false), Some(8));
        assert_eq!(prev_word_start(line, 8, false), Some(4));
        assert_eq!(prev_word_start(line, 10, false), Some(8));
        assert_eq!(word_end(line, 0, false), Some(2));
        assert_eq!(word_end(line, 4, false), Some(6));
        // Punctuation is skipped by the small-word motions.
        assert_eq!(next_word_start("foo(bar)", 0, false), Some(4));
        assert_eq!(next_word_start("foo(bar)", 3, false), Some(4));
        // BIG words only break on whitespace.
        assert_eq!(next_word_start("foo(bar) baz", 0, true), Some(9));
    }

    #[test]
    fn quote_and_bracket_objects_are_inclusive() {
        assert_eq!(
            text_object_range("say \"hello\" now", 7, '"', false),
            Some((5, 9))
        );
        assert_eq!(
            text_object_range("say \"hello\" now", 7, '"', true),
            Some((4, 10))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 7, '(', false),
            Some((6, 6))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 7, '(', true),
            Some((5, 7))
        );
        assert_eq!(
            text_object_range("f(a, (b), c)", 0, ')', false),
            Some((6, 6))
        );
    }

    #[test]
    fn word_object_includes_trailing_space_when_around() {
        assert_eq!(
            text_object_range("one two three", 5, 'w', false),
            Some((4, 6))
        );
        assert_eq!(
            text_object_range("one two three", 5, 'w', true),
            Some((4, 7))
        );
    }

    #[test]
    fn chars_slice_is_inclusive_and_safe() {
        assert_eq!(chars_slice("abcdef", 1, 3), "bcd");
        assert_eq!(chars_slice("abc", 2, 99), "c");
        assert_eq!(chars_slice("", 0, 3), "");
    }
}
