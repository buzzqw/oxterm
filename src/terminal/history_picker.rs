//! Interactive history picker (Ctrl+R, Tab completion, SQL mode).

use super::*;

impl TerminalBox {
    // ── History search ───────────────────────────────────────

    pub(super) fn exit_history_search_mode(&self) -> bool {
        let was_list_display = *self.imp().history_list_display.borrow();
        *self.imp().history_search_mode.borrow_mut() = false;
        *self.imp().history_list_display.borrow_mut() = false;
        *self.imp().history_sql_mode.borrow_mut() = false;
        *self.imp().history_tab_mode.borrow_mut() = false;
        *self.imp().history_search_generation.borrow_mut() += 1;
        *self.imp().history_search_query.borrow_mut() = String::new();
        *self.imp().history_list_loading.borrow_mut() = false;
        self.imp().history_list_results.borrow_mut().clear();
        *self.imp().history_list_index.borrow_mut() = 0;
        *self.imp().history_list_nlines.borrow_mut() = 0;
        *self.imp().history_tab_original.borrow_mut() = String::new();
        *self.imp().tab_fallback_pending_before.borrow_mut() = None;
        if was_list_display {
            self.vte().feed(b"\x1b[?1049l");
        }
        was_list_display
    }

    pub(super) fn start_history_search(&self) {
        // Ctrl+R shares the /history picker and its filtering behavior.
        self.cmd_history("", false);
    }

    pub(super) fn request_history_list_results(&self) {
        let query = self.imp().history_search_query.borrow().clone();
        let cwd = self.get_cwd();
        let generation = {
            let mut generation = self.imp().history_search_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        *self.imp().history_list_loading.borrow_mut() = true;
        self.imp().history_list_results.borrow_mut().clear();
        self.show_history_list();

        let weak = crate::SendWeak::new(self);
        history().search_async(
            query,
            50,
            cwd,
            Box::new(move |results| {
                crate::run_on_main(move || {
                    let Some(t) = weak.upgrade() else {
                        return;
                    };
                    if !*t.imp().history_search_mode.borrow()
                        || !*t.imp().history_list_display.borrow()
                        || *t.imp().history_sql_mode.borrow()
                        || generation != *t.imp().history_search_generation.borrow()
                    {
                        return;
                    }
                    *t.imp().history_list_loading.borrow_mut() = false;
                    *t.imp().history_list_results.borrow_mut() = results.clone();
                    t.show_history_list();
                });
            }),
        );
    }

    pub(super) fn handle_history_search_key(&self, event: &gdk::EventKey) -> glib::Propagation {
        let key = event.keyval();
        let text = event_text(event);
        let ctrl = event.state().contains(gdk::ModifierType::CONTROL_MASK);

        if key == K::Escape || (ctrl && (key == K::c || key == K::C)) {
            let tab_mode = *self.imp().history_tab_mode.borrow();
            let original = if tab_mode {
                self.imp().history_tab_original.borrow().clone()
            } else {
                String::new()
            };
            let was_list = self.exit_history_search_mode();
            if !was_list {
                self.vte().feed(b"\r\n");
            }
            if tab_mode {
                // The shell line was never cleared while the picker was open,
                // so leaving the alternate screen restores the original
                // command exactly as the user typed it. Do not feed anything
                // back to the shell here: sending Ctrl-U/re-typed text while
                // readline is still holding its own completion state is what
                // leaves later Esc/Tab presses stuck in a stale menu.
                *self.imp().input_shadow.borrow_mut() = original;
            }
            // Users often keep pressing Esc to make sure the picker is gone.
            // Swallow those for a moment instead of forwarding them to the
            // shell where readline may treat a lone Esc as a meta prefix.
            *self.imp().history_esc_deadline.borrow_mut() = mono_us() + 800_000;
            return glib::Propagation::Stop;
        }

        if key == K::Return || key == K::KP_Enter {
            let results = self.imp().history_list_results.borrow().clone();
            let sel_idx = *self.imp().history_list_index.borrow();
            let tab_mode = *self.imp().history_tab_mode.borrow();
            let original = self.imp().history_tab_original.borrow().clone();
            self.exit_history_search_mode();
            if sel_idx < results.len() {
                let row = &results[sel_idx];
                let cmd = row
                    .get(1)
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                if tab_mode {
                    // The shell still holds the original line (e.g.
                    // "alias " with the space readline inserted on the
                    // first Tab). Discard it before typing the match.
                    self.feed_command_bytes(b"\x15");
                    *self.imp().input_shadow.borrow_mut() = cmd.clone();
                    self.vte().feed_child(cmd.as_bytes());
                } else {
                    self.feed_command_bytes(b"\x15");
                    *self.imp().input_shadow.borrow_mut() = String::new();
                    *self.imp().shadow_anchor.borrow_mut() = None;
                    self.vte().feed_child(format!("{}\n", cmd).as_bytes());
                    history().add_async(&cmd, &self.get_cwd(), -1);
                }
            } else if tab_mode {
                // Nothing selected: the shell line still holds the typed
                // command untouched, so just keep it.
                *self.imp().input_shadow.borrow_mut() = original;
            }
            *self.imp().history_list_index.borrow_mut() = 0;
            return glib::Propagation::Stop;
        }

        // The picker is always shown as a list (history_list_display).
        if ctrl && (key == K::r || key == K::R) {
            let idx = *self.imp().history_list_index.borrow();
            let len = self.imp().history_list_results.borrow().len();
            if len > 0 {
                *self.imp().history_list_index.borrow_mut() = (idx + 1) % len;
                self.show_history_list();
            }
            return glib::Propagation::Stop;
        }
        if key == K::Up {
            let idx = *self.imp().history_list_index.borrow();
            if idx > 0 {
                *self.imp().history_list_index.borrow_mut() = idx - 1;
                self.show_history_list();
            }
            return glib::Propagation::Stop;
        }
        if key == K::Down {
            let idx = *self.imp().history_list_index.borrow();
            let len = self.imp().history_list_results.borrow().len();
            if idx + 1 < len {
                *self.imp().history_list_index.borrow_mut() = idx + 1;
                self.show_history_list();
            }
            return glib::Propagation::Stop;
        }
        if ctrl && (key == K::u || key == K::U) {
            self.imp().history_search_query.borrow_mut().clear();
        } else if ctrl && (key == K::w || key == K::W) {
            let mut q = self.imp().history_search_query.borrow_mut();
            *q = q
                .trim_end()
                .rsplit_once(char::is_whitespace)
                .map_or_else(String::new, |(before, _)| format!("{} ", before));
        } else if key == K::BackSpace {
            let mut q = self.imp().history_search_query.borrow_mut();
            if !q.is_empty() {
                q.pop();
            }
        } else if !text.is_empty() {
            let ch = text.chars().next().unwrap();
            if ch as u32 >= 0x20 {
                self.imp().history_search_query.borrow_mut().push(ch);
            }
        }
        *self.imp().history_list_index.borrow_mut() = 0;
        if !*self.imp().history_sql_mode.borrow() {
            self.request_history_list_results();
        }
        glib::Propagation::Stop
    }

    pub(super) fn show_history_list(&self) {
        let results = self.imp().history_list_results.borrow().clone();
        let q = self.imp().history_search_query.borrow().clone();
        let sql_mode = *self.imp().history_sql_mode.borrow();
        let tab_mode = *self.imp().history_tab_mode.borrow();
        let idx = *self.imp().history_list_index.borrow();

        if results.is_empty() {
            let mut out = String::new();
            if *self.imp().history_list_loading.borrow() {
                out.push_str("\x1b[90mSearching history...\x1b[0m\r\n");
            } else if !q.is_empty() {
                out.push_str(&format!("\x1b[33mNo results for: {}\x1b[0m\r\n", q));
            } else {
                out.push_str("\x1b[33mNo history found.\x1b[0m\r\n");
            }
            let enter_label = if tab_mode { "fill" } else { "execute" };
            if !sql_mode {
                out.push_str("\x1b[90mType to filter, Esc to cancel.\x1b[0m\r\n");
            } else {
                out.push_str(&format!(
                    "\x1b[90m\u{2191}\u{2193} select, Enter {}, Esc cancel\x1b[0m\r\n",
                    enter_label
                ));
            }
            let nlines = out.matches('\n').count();
            *self.imp().history_list_nlines.borrow_mut() = nlines;
            self.vte().feed(format!("\x1b[2J\x1b[H{}", out).as_bytes());
            return;
        }

        let total = results.len();
        let per_page = 10;
        let page_start = (idx / per_page) * per_page;
        let page_end = total.min(page_start + per_page);

        let mut out =
            "\x1b[36m\u{2500}\u{2500}\u{2500} History \u{2500}\u{2500}\u{2500}\x1b[0m\r\n"
                .to_string();
        for i in page_start..page_end {
            let row = &results[i];
            let (raw_cmd, time_str) = if row.len() >= 4 && !sql_mode {
                let cmd = row[1].as_str().unwrap_or("").to_string();
                let ts = row[3].as_str().unwrap_or("").to_string();
                let n = ts.chars().count();
                let time_str = if n >= 8 {
                    ts.chars().skip(n - 8).collect()
                } else {
                    String::new()
                };
                let duration = row
                    .get(5)
                    .and_then(serde_json::Value::as_i64)
                    .map(format_duration_ms);
                let branch = row.get(6).and_then(serde_json::Value::as_str);
                let mut metadata = Vec::new();
                if let Some(duration) = duration {
                    metadata.push(duration);
                }
                if let Some(branch) = branch.filter(|branch| !branch.is_empty()) {
                    metadata.push(branch.to_string());
                }
                let time_str = if metadata.is_empty() {
                    time_str
                } else {
                    format!("{}  {}", time_str, metadata.join(" · "))
                };
                (cmd, time_str)
            } else if sql_mode {
                let parts: Vec<String> = row
                    .iter()
                    .map(|v| match v {
                        serde_json::Value::Null => "".to_string(),
                        other => other.to_string(),
                    })
                    .collect();
                (parts.join(" | "), String::new())
            } else {
                (format!("{:?}", row), String::new())
            };
            // History rows can come from /learn imports or anything that
            // writes to the shell-integration FIFO; never let their escape
            // sequences reach the terminal.
            let time_str = sanitize_terminal_text(&time_str);
            let display_cmd = sanitize_terminal_text(&raw_cmd)
                .replace('\n', "\x1b[90m\u{23ce}\x1b[0m ")
                .replace('\r', "");
            let display = if display_cmd.chars().count() > 100 {
                format!("{}...", display_cmd.chars().take(100).collect::<String>())
            } else {
                display_cmd
            };
            if i == idx {
                out.push_str(&format!(
                    "\x1b[92m \u{25b6} \x1b[0m{}  \x1b[90m{}\x1b[0m\r\n",
                    display, time_str
                ));
            } else {
                out.push_str(&format!("  {}  \x1b[90m{}\x1b[0m\r\n", display, time_str));
            }
        }

        let query_disp = if q.is_empty() {
            "all".to_string()
        } else {
            format!("'{}'", q)
        };
        let enter_label = if tab_mode { "fill" } else { "execute" };
        let footer = if !sql_mode {
            format!(
                "\x1b[90m\u{2500}\u{2500}\u{2500} {} matches for \x1b[1;97m{}\x1b[0m\x1b[90m \u{2014} \u{2191}\u{2193} select, type to filter, Enter {}, Esc cancel\x1b[0m",
                total, query_disp, enter_label
            )
        } else {
            format!(
                "\x1b[90m\u{2500}\u{2500}\u{2500} {} results \u{2014} \u{2191}\u{2193} select, Enter {}, Esc cancel\x1b[0m",
                total, enter_label
            )
        };
        out.push_str(&footer);
        let nlines = out.matches('\n').count();
        *self.imp().history_list_nlines.borrow_mut() = nlines;
        self.vte().feed(format!("\x1b[2J\x1b[H{}", out).as_bytes());
    }

    pub(super) fn replay_history_number(&self, num: i64) {
        if num < 1 {
            return;
        }
        let cwd = self.get_cwd();
        let expected_shadow = self.imp().input_shadow.borrow().clone();
        let generation = {
            let mut generation = self.imp().history_search_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        let weak = crate::SendWeak::new(self);
        history().search_async(
            String::new(),
            20,
            cwd,
            Box::new(move |rows| {
                let command = rows
                    .get((num - 1) as usize)
                    .and_then(|row| row.get(1))
                    .and_then(ValueRow::as_str)
                    .map(str::to_string);
                crate::run_on_main(move || {
                    let Some(t) = weak.upgrade() else {
                        return;
                    };
                    if generation != *t.imp().history_search_generation.borrow() {
                        return;
                    }
                    if *t.imp().input_shadow.borrow() != expected_shadow {
                        return;
                    }
                    if let Some(command) = command {
                        t.vte().feed_child(b"\x15");
                        t.vte().feed_child(command.as_bytes());
                    }
                });
            }),
        );
    }

    pub(super) fn start_history_tab_complete(&self, _allow_list: bool) {
        // The shell only ever saw the first Tab, and it keeps holding the
        // typed command untouched while the picker is on screen. Cancelling
        // (Esc) therefore just leaves the alternate screen and the original
        // command is still there, with no control bytes sent to the shell.
        let shadow = self.imp().input_shadow.borrow().clone();
        let shadow_query = shadow.trim_end_matches('\t').to_string();
        let mut query = if shadow_query.trim().is_empty() {
            self.get_real_command_text()
        } else {
            shadow_query
        };
        query = query.trim_end_matches('\t').to_string();
        let cwd = self.get_cwd();
        let expected_shadow = shadow.clone();
        let generation = {
            let mut generation = self.imp().history_search_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        let weak = crate::SendWeak::new(self);
        history().search_async(
            query.clone(),
            50,
            cwd,
            Box::new(move |results| {
                crate::run_on_main(move || {
                    let Some(t) = weak.upgrade() else {
                        return;
                    };
                    if generation != *t.imp().history_search_generation.borrow() {
                        return;
                    }
                    if *t.imp().input_shadow.borrow() != expected_shadow {
                        return;
                    }
                    if results.is_empty() {
                        // The caller consumed Tab while the asynchronous
                        // history lookup was pending. Preserve the shell's
                        // normal completion behavior when nothing matched.
                        t.vte().feed_child(b"\t");
                        return;
                    }
                    if results.len() == 1 {
                        let cmd = results[0][1].as_str().unwrap_or("").to_string();
                        t.fill_history_match(&cmd);
                        return;
                    }
                    *t.imp().history_tab_mode.borrow_mut() = true;
                    let original = t
                        .imp()
                        .input_shadow
                        .borrow()
                        .trim_end_matches('\t')
                        .to_string();
                    *t.imp().history_tab_original.borrow_mut() = if original.is_empty() {
                        query.clone()
                    } else {
                        original
                    };
                    *t.imp().history_list_display.borrow_mut() = true;
                    *t.imp().history_search_mode.borrow_mut() = true;
                    *t.imp().history_search_query.borrow_mut() = query.clone();
                    *t.imp().history_list_index.borrow_mut() = 0;
                    *t.imp().history_list_nlines.borrow_mut() = 0;
                    *t.imp().history_sql_mode.borrow_mut() = false;
                    *t.imp().history_list_loading.borrow_mut() = false;
                    t.vte().feed(b"\x1b[?1049h");
                    *t.imp().history_list_results.borrow_mut() = results.clone();
                    t.show_history_list();
                });
            }),
        );
    }

    pub(super) fn fill_history_match(&self, cmd: &str) {
        self.feed_command_bytes(b"\x15");
        *self.imp().input_shadow.borrow_mut() = cmd.to_string();
        self.vte().feed_child(cmd.as_bytes());
    }
}
