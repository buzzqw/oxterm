//! Hints mode: keyboard selection of URLs, paths, hashes and IPs.

use super::*;

impl TerminalBox {
    // ── Hints mode ───────────────────────────────────────────

    pub(super) fn cell_to_overlay_coords(&self, col: i64, row: i64) -> (f64, f64) {
        let vte = self.vte();
        let cw = vte.char_width() as f64;
        let ch = vte.char_height() as f64;
        let scroll_row = {
            let scroll = self.imp().scroll.borrow().clone().unwrap();
            scroll.vadjustment().value()
        };
        let padding = settings().get_i64("window_padding_horizontal") as f64;
        let cell_x = padding + 8.0 + col as f64 * cw;
        let cell_y = (row as f64 - scroll_row) * ch;
        (cell_x, cell_y)
    }

    pub(super) fn generate_hint_labels(count: usize) -> Vec<String> {
        let chars: Vec<char> = HINT_CHARS.chars().collect();
        let mut labels: Vec<String> = Vec::new();
        for c in &chars {
            labels.push(c.to_string());
            if labels.len() >= count {
                return labels[..count].to_vec();
            }
        }
        for c1 in &chars {
            for c2 in &chars {
                labels.push(format!("{}{}", c1, c2));
                if labels.len() >= count {
                    return labels[..count].to_vec();
                }
            }
        }
        for c1 in &chars {
            for c2 in &chars {
                for c3 in &chars {
                    labels.push(format!("{}{}{}", c1, c2, c3));
                    if labels.len() >= count {
                        return labels[..count].to_vec();
                    }
                }
            }
        }
        labels[..count.min(labels.len())].to_vec()
    }

    pub(super) fn activate_hints(&self) {
        if *self.imp().hints_active.borrow() {
            return;
        }
        *self.imp().hints_active.borrow_mut() = true;
        *self.imp().hints_buffer.borrow_mut() = String::new();
        self.imp().hints_map.borrow_mut().clear();
        let vte_w = self.vte().allocated_width();
        let hints_fixed = self.imp().hints_fixed.borrow().clone().unwrap();
        hints_fixed.set_size_request(vte_w, -1);
        hints_fixed.show_all();
        let matches = self.scan_for_hints();
        let labels = Self::generate_hint_labels(matches.len());
        for (i, (mtype, text, col, row)) in matches.iter().enumerate() {
            let label = &labels[i];
            self.imp()
                .hints_map
                .borrow_mut()
                .insert(label.clone(), (mtype.clone(), text.clone()));
            let (x, y) = self.cell_to_overlay_coords(*col, *row);
            let lbl = gtk::Label::new(Some(label));
            lbl.style_context().add_class("oxterm-hint-label");
            lbl.set_halign(gtk::Align::Start);
            lbl.set_valign(gtk::Align::Start);
            lbl.show();
            hints_fixed.put(&lbl, x.round() as i32, y.round() as i32);
        }
        if !self.imp().hints_map.borrow().is_empty() {
            self.vte()
                .feed(b"\r\n\x1b[90mType hint label to select, Esc to cancel.\x1b[0m\r\n");
        }
    }

    pub(super) fn deactivate_hints(&self) {
        *self.imp().hints_active.borrow_mut() = false;
        *self.imp().hints_buffer.borrow_mut() = String::new();
        let hints_fixed = self.imp().hints_fixed.borrow().clone().unwrap();
        for child in hints_fixed.children() {
            hints_fixed.remove(&child);
        }
        hints_fixed.hide();
        self.imp().hints_map.borrow_mut().clear();
    }

    pub(super) fn handle_hint_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        if key == K::Escape {
            self.deactivate_hints();
            return glib::Propagation::Stop;
        }
        let text = event_text(event);
        if text.is_empty() {
            return glib::Propagation::Stop;
        }
        let c = text.chars().next().unwrap();
        if (c as u32) < 0x20 {
            return glib::Propagation::Stop;
        }
        self.imp().hints_buffer.borrow_mut().push(c);
        let buffer = self.imp().hints_buffer.borrow().clone();
        let map = self.imp().hints_map.borrow().clone();
        if let Some(action) = map.get(&buffer) {
            self.perform_hint_action(action.clone());
            self.deactivate_hints();
            return glib::Propagation::Stop;
        }
        let matching_prefixes = map.keys().any(|k| k.starts_with(&buffer));
        if !matching_prefixes {
            self.deactivate_hints();
        }
        glib::Propagation::Stop
    }

    pub(super) fn perform_hint_action(&self, action: (String, String)) {
        let (mtype, text) = action;
        let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        if mtype == "url" {
            self.open_url(&text);
        } else if mtype == "path" {
            clipboard.set_text(&text);
            let expanded = if let Some(rest) = text.strip_prefix("~/") {
                dirs::home_dir()
                    .map(|p| p.join(rest))
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                text.clone()
            };
            let p = std::path::Path::new(&expanded);
            if p.is_dir() || p.is_file() {
                crate::notes::spawn_detached("xdg-open", &[&expanded]);
            } else {
                self.vte()
                    .feed(format!("\r\n\x1b[32mPath copied: {}\x1b[0m\r\n", text).as_bytes());
            }
        } else if mtype == "git-sha" || mtype == "ip" {
            clipboard.set_text(&text);
            self.vte()
                .feed(format!("\r\n\x1b[32mCopied: {}\x1b[0m\r\n", text).as_bytes());
        }
    }

    pub(super) fn scan_for_hints(&self) -> Vec<(String, String, i64, i64)> {
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        let vadj = scroll.vadjustment();
        let scroll_top = vadj.value() as i64;
        let page_size = vadj.page_size() as i64;
        if page_size <= 0 {
            return Vec::new();
        }
        let first_row = (scroll_top - 1).max(0);
        let last_row = scroll_top + page_size + 1;
        let text = super::text::range(&self.vte(), first_row, 0, last_row, 0).unwrap_or_default();
        if text.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<(String, String, i64, i64)> = Vec::new();
        for (i, line) in text.split('\n').enumerate() {
            let row = first_row + i as i64;
            if row > last_row {
                break;
            }
            for m in hint_url_re().find_iter(line) {
                matches.push((
                    "url".to_string(),
                    m.as_str().to_string(),
                    char_column(line, m.start()) as i64,
                    row,
                ));
            }
            for m in hint_path_re().find_iter(line) {
                let p = m.as_str().trim();
                if p.chars().count() >= 3 {
                    matches.push((
                        "path".to_string(),
                        p.to_string(),
                        char_column(line, m.start()) as i64,
                        row,
                    ));
                }
            }
            for m in hint_git_sha_re().find_iter(line) {
                let sha = m.as_str().to_string();
                if !sha.chars().all(|c| c.is_ascii_digit()) && sha.chars().count() >= 7 {
                    matches.push((
                        "git-sha".to_string(),
                        sha,
                        char_column(line, m.start()) as i64,
                        row,
                    ));
                }
            }
            for m in hint_ip_re().find_iter(line) {
                let ip = m.as_str().to_string();
                let parts: Vec<&str> = ip.split('.').collect();
                if parts.iter().all(|p| {
                    p.parse::<i64>()
                        .map(|v| (0..=255).contains(&v))
                        .unwrap_or(false)
                }) {
                    matches.push((
                        "ip".to_string(),
                        ip,
                        char_column(line, m.start()) as i64,
                        row,
                    ));
                }
            }
        }
        let max_matches = HINT_CHARS.len() + HINT_CHARS.len() * HINT_CHARS.len();
        matches.truncate(max_matches);
        matches
    }
}
