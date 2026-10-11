//! VI-style keyboard copy mode.

use super::*;

impl TerminalBox {
    // ── VI copy mode ─────────────────────────────────────────

    pub(super) fn activate_vi_copy(&self) {
        if *self.imp().vi_copy_active.borrow() {
            return;
        }
        *self.imp().vi_copy_active.borrow_mut() = true;
        *self.imp().vi_visual_active.borrow_mut() = false;
        *self.imp().vi_selection_start.borrow_mut() = -1;
        *self.imp().vi_selection_end.borrow_mut() = -1;
        *self.imp().vi_last_key.borrow_mut() = None;
        *self.imp().vi_last_key_time.borrow_mut() = 0;
        let area = self.imp().vi_overlay_area.borrow().clone().unwrap();
        area.set_size_request(self.vte().allocated_width(), self.vte().allocated_height());
        area.show_all();
        self.vte()
            .feed(b"\r\n\x1b[90mVI Copy Mode: hjkl scroll, v select, y yank, / search, Esc exit\x1b[0m\r\n");
    }

    pub(super) fn deactivate_vi_copy(&self) {
        *self.imp().vi_copy_active.borrow_mut() = false;
        *self.imp().vi_visual_active.borrow_mut() = false;
        *self.imp().vi_selection_start.borrow_mut() = -1;
        *self.imp().vi_selection_end.borrow_mut() = -1;
        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.hide();
        }
    }

    pub(super) fn handle_vi_copy_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        let ctrl = event.state().contains(gdk::ModifierType::CONTROL_MASK);
        if key == K::Escape {
            self.deactivate_vi_copy();
            return glib::Propagation::Stop;
        }
        if (key == K::c || key == K::C) && ctrl {
            self.deactivate_vi_copy();
            return glib::Propagation::Stop;
        }
        if ctrl {
            if key == K::u || key == K::U {
                self.vi_scroll_page(true);
            } else if key == K::d || key == K::D {
                self.vi_scroll_page(false);
            }
            return glib::Propagation::Stop;
        }
        if key == K::j {
            self.vi_scroll(1);
        } else if key == K::k {
            self.vi_scroll(-1);
        } else if key == K::h {
            self.vi_scroll_h(-3);
        } else if key == K::l {
            self.vi_scroll_h(3);
        } else if key == K::w {
            self.vi_scroll(5);
        } else if key == K::b {
            self.vi_scroll(-5);
        } else if key == K::v || key == K::V {
            self.vi_toggle_visual();
        } else if key == K::y || key == K::Y {
            self.vi_yank_selection();
            return glib::Propagation::Stop;
        } else if key == K::slash || key == K::question {
            self.show_search();
            return glib::Propagation::Stop;
        } else if key == K::g {
            let now = mono_us();
            let double_g = {
                let last = *self.imp().vi_last_key.borrow();
                let last_time = *self.imp().vi_last_key_time.borrow();
                last == Some(K::g) && now - last_time < 1_000_000
            };
            if double_g {
                self.vi_scroll_to_top();
                *self.imp().vi_last_key.borrow_mut() = None;
            } else {
                *self.imp().vi_last_key.borrow_mut() = Some(K::g);
                *self.imp().vi_last_key_time.borrow_mut() = now;
            }
        } else if key == K::G {
            self.vi_scroll_to_bottom();
        } else {
            return glib::Propagation::Stop;
        }
        if *self.imp().vi_visual_active.borrow() {
            if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
                area.queue_draw();
            }
        }
        glib::Propagation::Stop
    }

    pub(super) fn vi_scroll(&self, lines: i64) {
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        let vadj = scroll.vadjustment();
        let bottom = (vadj.upper() - vadj.page_size()).max(0.0);
        let new_val = (vadj.value() + lines as f64).clamp(0.0, bottom);
        vadj.set_value(new_val);
        if *self.imp().vi_visual_active.borrow() {
            let row = vadj.value() as i64;
            if *self.imp().vi_selection_start.borrow() < 0 {
                *self.imp().vi_selection_start.borrow_mut() = row;
            }
            *self.imp().vi_selection_end.borrow_mut() = row;
        }
    }

    pub(super) fn vi_scroll_h(&self, cols: i64) {
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        let hadj = scroll.hadjustment();
        let cw = self.vte().char_width();
        let cw = if cw <= 0 { 8 } else { cw };
        let new_val = (hadj.value() + cols as f64 * cw as f64)
            .clamp(0.0, (hadj.upper() - hadj.page_size()).max(0.0));
        hadj.set_value(new_val);
    }

    pub(super) fn vi_scroll_to_top(&self) {
        if let Some(scroll) = self.imp().scroll.borrow().clone() {
            {
                let vadj = scroll.vadjustment();
                vadj.set_value(0.0);
            }
        }
    }

    pub(super) fn vi_scroll_to_bottom(&self) {
        if let Some(scroll) = self.imp().scroll.borrow().clone() {
            {
                let vadj = scroll.vadjustment();
                let bottom = (vadj.upper() - vadj.page_size()).max(0.0);
                vadj.set_value(bottom);
            }
        }
    }

    pub(super) fn vi_scroll_page(&self, up: bool) {
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        let vadj = scroll.vadjustment();
        let page = vadj.page_size() * 0.5;
        let delta = if up { -page } else { page };
        let bottom = (vadj.upper() - vadj.page_size()).max(0.0);
        let new_val = (vadj.value() + delta).clamp(0.0, bottom);
        vadj.set_value(new_val);
    }

    pub(super) fn vi_toggle_visual(&self) {
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        if *self.imp().vi_visual_active.borrow() {
            *self.imp().vi_visual_active.borrow_mut() = false;
            *self.imp().vi_selection_start.borrow_mut() = -1;
            *self.imp().vi_selection_end.borrow_mut() = -1;
            if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
                area.queue_draw();
            }
            return;
        }
        *self.imp().vi_visual_active.borrow_mut() = true;
        {
            let vadj = scroll.vadjustment();
            let v = vadj.value() as i64;
            *self.imp().vi_selection_start.borrow_mut() = v;
            *self.imp().vi_selection_end.borrow_mut() = v;
        }
        if let Some(area) = self.imp().vi_overlay_area.borrow().clone() {
            area.queue_draw();
        }
    }

    pub(super) fn vi_yank_selection(&self) {
        if *self.imp().vi_visual_active.borrow() && *self.imp().vi_selection_start.borrow() >= 0 {
            let mut start = *self.imp().vi_selection_start.borrow();
            let mut end = *self.imp().vi_selection_end.borrow();
            if start > end {
                std::mem::swap(&mut start, &mut end);
            }
            if end > start + 500 {
                end = start + 500;
            }
            let (text, _) = self
                .vte()
                .text_range_format(Format::Text, start, 0, end + 1, 0);
            if let Some(text) = text {
                let text = text.to_string();
                let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
                clipboard.set_text(&text);
                let n = text.lines().count();
                self.vte()
                    .feed(format!("\r\n\x1b[32m{} lines copied\x1b[0m\r\n", n).as_bytes());
            } else {
                self.vte().feed(b"\r\n\x1b[33mNo text selected.\x1b[0m\r\n");
            }
            self.deactivate_vi_copy();
        } else {
            self.deactivate_vi_copy();
        }
    }

    pub(super) fn draw_vi_overlay(&self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        if !*self.imp().vi_visual_active.borrow() {
            return;
        }
        let start = *self.imp().vi_selection_start.borrow();
        let end = *self.imp().vi_selection_end.borrow();
        if start < 0 || end < 0 {
            return;
        }
        let scroll = self.imp().scroll.borrow().clone().unwrap();
        let vadj = scroll.vadjustment();
        let scroll_row = vadj.value();
        let ch = self.vte().char_height();
        let ch = if ch <= 0 { 16 } else { ch };
        let width = area.allocated_width() as f64;
        let height = area.allocated_height() as f64;
        let padding = settings().get_i64("window_padding_horizontal") as f64;
        let x = padding + 8.0;
        let s = start.min(end);
        let e = start.max(end);
        for row in s..=e {
            let y = (row as f64 - scroll_row) * ch as f64;
            if (0.0..=height).contains(&y) {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
                cr.rectangle(x, y, width - x, ch as f64);
                let _ = cr.fill();
            }
        }
        let cur_y = (vadj.value().floor() - scroll_row) * ch as f64;
        cr.set_source_rgba(0.988, 0.816, 0.31, 0.8);
        cr.rectangle(0.0, cur_y, width, 2.0);
        let _ = cr.fill();
    }
}
