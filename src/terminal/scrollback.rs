//! Scrollback search, quickmarks and OSC 133 prompt navigation.

use super::*;

impl TerminalBox {
    // ── Scrollback search ────────────────────────────────────

    pub(super) fn show_search(&self) {
        let revealer = self.imp().search_revealer.borrow().clone().unwrap();
        if revealer.property::<bool>("reveal-child") {
            return;
        }
        revealer.set_reveal_child(true);
        let entry = self.imp().search_entry.borrow().clone().unwrap();
        entry.set_text("");
        entry.grab_focus();
        self.clear_search_highlights();
    }

    pub(super) fn hide_search(&self) {
        if let Some(revealer) = self.imp().search_revealer.borrow().clone() {
            revealer.set_reveal_child(false);
        }
        self.clear_search_highlights();
        self.imp().search_results.borrow_mut().clear();
        *self.imp().search_index.borrow_mut() = 0;
        if let Some(label) = self.imp().search_label.borrow().clone() {
            label.set_text("");
        }
        self.vte().grab_focus();
    }

    pub(super) fn do_search(&self) {
        self.clear_search_highlights();
        self.imp().search_results.borrow_mut().clear();
        *self.imp().search_index.borrow_mut() = 0;
        let entry = self.imp().search_entry.borrow().clone().unwrap();
        let query = entry.text();
        let label = self.imp().search_label.borrow().clone().unwrap();
        if query.is_empty() || query.chars().count() < 2 {
            label.set_text("type at least 2 characters");
            return;
        }
        let use_regex = self
            .imp()
            .search_regex_btn
            .borrow()
            .as_ref()
            .map(|b| b.is_active())
            .unwrap_or(false);
        let case_sensitive = self
            .imp()
            .search_case_btn
            .borrow()
            .as_ref()
            .map(|b| b.is_active())
            .unwrap_or(false);
        let search_query = if use_regex {
            query.to_string()
        } else {
            regex::escape(&query).to_string()
        };
        // VTE only accepts search regexes built with for_search and
        // PCRE2_MULTILINE; anything else is silently ignored.
        const PCRE2_CASELESS: u32 = 0x0000_0008;
        const PCRE2_MULTILINE: u32 = 0x0000_0400;
        let mut flags = PCRE2_MULTILINE;
        if !case_sensitive {
            flags |= PCRE2_CASELESS;
        }
        let vte_regex = VteRegex::for_search(&search_query, flags);
        match vte_regex {
            Ok(regex) => {
                let vte = self.vte();
                vte.search_set_regex(Some(&regex), 0);
                vte.search_set_wrap_around(true);
                if vte.search_find_next() {
                    label.set_text("Match");
                } else {
                    label.set_text("No matches");
                }
            }
            Err(_) => {
                label.set_text("Invalid regex");
            }
        }
    }

    pub(super) fn clear_search_highlights(&self) {
        let vte = self.vte();
        let tags = std::mem::take(&mut *self.imp().search_tags.borrow_mut());
        for tag in tags {
            vte.match_remove(tag);
        }
        vte.search_set_regex(None, 0);
    }

    pub(super) fn on_search_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        let shift = event.state().contains(gdk::ModifierType::SHIFT_MASK);
        if key == K::Escape {
            self.hide_search();
            return glib::Propagation::Stop;
        }
        if key == K::Return || key == K::KP_Enter {
            if shift {
                self.vte().search_find_previous();
            } else {
                self.vte().search_find_next();
            }
            return glib::Propagation::Stop;
        }
        if key == K::Up {
            self.vte().search_find_previous();
            return glib::Propagation::Stop;
        }
        if key == K::Down {
            self.vte().search_find_next();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    // ── Quickmarks ───────────────────────────────────────────

    pub(super) fn set_quickmark(&self) {
        let (_, row) = self.vte().cursor_position();
        if row < 0 {
            return;
        }
        {
            let marks = self.imp().quickmarks.borrow();
            if marks.iter().any(|r| (r - row).abs() < 3) {
                return;
            }
        }
        let mut marks = self.imp().quickmarks.borrow_mut();
        marks.push(row);
        marks.sort();
        *self.imp().quickmark_index.borrow_mut() = -1;
        self.vte()
            .feed(format!("\r\n\x1b[32m+ Quickmark set at line {}\x1b[0m\r\n", row).as_bytes());
    }

    pub(super) fn jump_next_quickmark(&self) {
        let marks = self.imp().quickmarks.borrow().clone();
        if marks.is_empty() {
            return;
        }
        let mut idx = *self.imp().quickmark_index.borrow() + 1;
        idx %= marks.len() as i64;
        *self.imp().quickmark_index.borrow_mut() = idx;
        let target = marks[idx as usize];
        if let Some(scroll) = self.imp().scroll.borrow().clone() {
            {
                let vadj = scroll.vadjustment();
                vadj.set_value(target as f64);
            }
        }
    }

    pub(super) fn remove_all_quickmarks(&self) {
        self.imp().quickmarks.borrow_mut().clear();
        *self.imp().quickmark_index.borrow_mut() = -1;
    }

    // ── OSC133 prompt jump ───────────────────────────────────

    pub(super) fn scroll_to_osc133_prompt(&self, direction_up: bool) {
        let markers = self.imp().osc133_markers.borrow().clone();
        if markers.is_empty() {
            return;
        }
        let (_, cur_row) = self.vte().cursor_position();
        let prompts: Vec<(i64, i64)> = markers
            .iter()
            .filter(|(_, t, _)| t == "prompt")
            .map(|(r, _, e)| (*r, *e))
            .collect();
        if prompts.is_empty() {
            return;
        }
        let mut target: Option<i64> = None;
        if direction_up {
            for (r, _) in prompts.iter().rev() {
                if *r < cur_row - 1 {
                    target = Some(*r);
                    break;
                }
            }
        } else {
            for (r, _) in &prompts {
                if *r > cur_row {
                    target = Some(*r);
                    break;
                }
            }
        }
        if let Some(target) = target {
            if let Some(scroll) = self.imp().scroll.borrow().clone() {
                {
                    let vadj = scroll.vadjustment();
                    vadj.set_value(target as f64);
                }
            }
        }
    }

    pub(crate) fn get_command_output_range(&self) -> (Option<i64>, Option<i64>) {
        let (_, cur_row) = self.vte().cursor_position();
        let markers = self.imp().osc133_markers.borrow().clone();
        let mut cmd_start: Option<i64> = None;
        let mut prompt_end: Option<i64> = None;
        for (row, mtype, _e) in markers {
            if mtype == "prompt" && row <= cur_row {
                prompt_end = Some(row);
            }
            if mtype == "cmd_start" && row <= cur_row {
                cmd_start = Some(row);
            }
        }
        match (cmd_start, prompt_end) {
            (Some(s), Some(e)) if s < e => (Some(s), Some(e)),
            _ => (None, None),
        }
    }

    pub(super) fn copy_command_output(&self) {
        if let (Some(start_row), Some(mut end_row)) = self.get_command_output_range() {
            end_row = end_row.min(start_row + 500);
            let (text, _) = self
                .vte()
                .text_range_format(Format::Text, start_row, 0, end_row, 0);
            if let Some(text) = text {
                let text = text.to_string();
                let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
                clipboard.set_text(&text);
                self.vte()
                    .feed(b"\r\n\x1b[32m+ Output copied to clipboard\x1b[0m\r\n");
                self.vte().feed_child(b"\r");
            }
        }
    }
}
