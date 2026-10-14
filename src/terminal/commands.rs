//! Oxterm `/commands` typed at the prompt or in the command bar.

use super::*;
use crate::notes::NotesManager;

impl TerminalBox {
    // ── Oxterm commands ───────────────────────────────────────

    pub(super) fn cmd_history(&self, args: &str, clear_prompt: bool) {
        if clear_prompt {
            self.feed_command_bytes(b"\x15");
            *self.imp().input_shadow.borrow_mut() = String::new();
            *self.imp().shadow_anchor.borrow_mut() = None;
        }
        *self.imp().history_list_display.borrow_mut() = true;
        *self.imp().history_search_mode.borrow_mut() = true;
        *self.imp().history_search_query.borrow_mut() = args.trim().to_string();
        *self.imp().history_list_index.borrow_mut() = 0;
        *self.imp().history_list_nlines.borrow_mut() = 0;
        *self.imp().history_sql_mode.borrow_mut() = false;
        *self.imp().history_list_loading.borrow_mut() = false;
        self.vte().feed(b"\x1b[?1049h");
        let query = args.trim().to_string();
        let upper = query.to_uppercase();
        let mut is_sql = false;
        let mut sql = query.clone();
        if upper.starts_with(":SQL ") || upper.starts_with(":SQL\t") {
            is_sql = true;
            sql = query[5..].trim().to_string();
        } else if upper.starts_with("SELECT") || upper.starts_with("EXPLAIN") {
            is_sql = true;
        }
        if is_sql {
            *self.imp().history_sql_mode.borrow_mut() = true;
            *self.imp().history_list_loading.borrow_mut() = true;
            let generation = {
                let mut generation = self.imp().history_search_generation.borrow_mut();
                *generation += 1;
                *generation
            };
            self.show_history_list();
            let weak = crate::SendWeak::new(self);
            history().sql_search_async(
                sql,
                Box::new(move |result| {
                    crate::run_on_main(move || {
                        let Some(t) = weak.upgrade() else {
                            return;
                        };
                        if !*t.imp().history_search_mode.borrow()
                            || generation != *t.imp().history_search_generation.borrow()
                        {
                            return;
                        }
                        *t.imp().history_list_loading.borrow_mut() = false;
                        match result {
                            Ok(rows) => {
                                *t.imp().history_list_results.borrow_mut() = rows;
                                t.show_history_list();
                            }
                            Err(error) => {
                                t.imp().history_list_results.borrow_mut().clear();
                                t.vte().feed(
                                    format!(
                                        "\x1b[2J\x1b[H\x1b[31mSQL Error: {}\x1b[0m\r\n\x1b[90mPress Esc to exit.\x1b[0m\r\n",
                                        error
                                    )
                                    .as_bytes(),
                                );
                            }
                        }
                    });
                }),
            );
        } else {
            self.request_history_list_results();
        }
    }

    pub(super) fn cmd_wnotes(&self, args: &str) {
        if args.is_empty() {
            self.vte()
                .feed(b"\r\n\x1b[33mUsage: /wnotes [-filename.md] <note text>\x1b[0m\r\n");
            return;
        }
        let mut filename = None;
        let mut text = args.to_string();
        if let Some(rest) = args.strip_prefix('-') {
            if let Some(pos) = rest.find(' ') {
                filename = Some(rest[..pos].to_string());
                text = rest[pos + 1..].trim().to_string();
            } else {
                filename = Some(rest.to_string());
                text = String::new();
            }
            if text.is_empty() {
                self.vte()
                    .feed(b"\r\n\x1b[33mUsage: /wnotes [-filename.md] <note text>\x1b[0m\r\n");
                return;
            }
        }
        let vte = self.vte();
        NotesManager::new().write_note_async(
            text,
            filename.as_deref(),
            crate::notes::NoteContext::manual(),
            move |result| match result {
                Ok(path) => vte.feed(
                    format!(
                        "\r\n\x1b[32mNote saved to: {}\x1b[0m\r\n",
                        path.to_string_lossy()
                    )
                    .as_bytes(),
                ),
                Err(e) => vte.feed(format!("\r\n\x1b[31m/wnotes: {}\x1b[0m\r\n", e).as_bytes()),
            },
        );
    }

    pub(super) fn cmd_onotes(&self, args: &str) {
        let mut filename = None;
        if !args.trim().is_empty() {
            let a = args.trim();
            filename = Some(if let Some(stripped) = a.strip_prefix('-') {
                stripped.to_string()
            } else {
                a.to_string()
            });
        }
        let notes = NotesManager::new();
        match notes.open_notes(filename.as_deref()) {
            Ok(path) => {
                self.vte().feed(
                    format!(
                        "\r\n\x1b[32mOpening notes: {}\x1b[0m\r\n",
                        path.to_string_lossy()
                    )
                    .as_bytes(),
                );
            }
            Err(e) => {
                self.vte()
                    .feed(format!("\r\n\x1b[31m/onotes: {}\x1b[0m\r\n", e).as_bytes());
            }
        }
    }

    pub(super) fn cmd_learn(&self, args: &str) {
        const MAX_LINES: usize = 5000;
        const MAX_LINE_LEN: usize = 1000;
        let path = args.trim();
        if path.is_empty() {
            self.vte()
                .feed(b"\r\n\x1b[33mUsage: /learn <file>\x1b[0m\r\n");
            return;
        }
        let expanded = if let Some(rest) = path.strip_prefix("~/") {
            dirs::home_dir()
                .map(|p| p.join(rest).to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            path.to_string()
        };
        let full_path = if std::path::Path::new(&expanded).is_absolute() {
            expanded
        } else {
            format!("{}/{}", self.get_cwd(), expanded)
        };
        let cwd = self.get_cwd();
        self.vte()
            .feed(b"\r\n\x1b[90m/learn: importing commands...\x1b[0m\r\n");
        let weak = crate::SendWeak::new(self);
        std::thread::spawn(move || {
            let result = std::fs::read_to_string(&full_path).map(|content| {
                let mut commands = Vec::new();
                let mut skipped_long = 0;
                let mut lines = content.lines();
                for line in lines.by_ref().take(MAX_LINES) {
                    let cmd = line.trim();
                    if cmd.is_empty() || cmd.starts_with('#') {
                        continue;
                    }
                    if cmd.chars().count() > MAX_LINE_LEN {
                        skipped_long += 1;
                        continue;
                    }
                    commands.push(cmd.to_string());
                }
                let truncated = lines.next().is_some();
                (commands, skipped_long, truncated)
            });
            let (commands, skipped_long, truncated) = match result {
                Ok(parsed) => parsed,
                Err(error) => {
                    crate::run_on_main(move || {
                        if let Some(t) = weak.upgrade() {
                            t.vte().feed(
                                format!("\r\n\x1b[31m/learn: {}\x1b[0m\r\n", error).as_bytes(),
                            );
                        }
                    });
                    return;
                }
            };
            let report_path = full_path.clone();
            history().add_many_async(
                commands,
                cwd,
                -1,
                Box::new(move |added| {
                    crate::run_on_main(move || {
                        if let Some(t) = weak.upgrade() {
                            t.vte().feed(
                                format!(
                                    "\r\n\x1b[32m/learn: {} command(s) added to history from {}\x1b[0m\r\n",
                                    added, report_path
                                )
                                .as_bytes(),
                            );
                            if skipped_long > 0 {
                                t.vte().feed(
                                    format!(
                                        "\x1b[33m/learn: {} line(s) skipped (too long, not a command)\x1b[0m\r\n",
                                        skipped_long
                                    )
                                    .as_bytes(),
                                );
                            }
                            if truncated {
                                t.vte().feed(
                                    format!(
                                        "\x1b[33m/learn: file has more than {} lines, only the first {} were read\x1b[0m\r\n",
                                        MAX_LINES, MAX_LINES
                                    )
                                    .as_bytes(),
                                );
                            }
                        }
                    });
                }),
            );
        });
    }

    pub(super) fn human_size(n: i64) -> String {
        let mut size = n as f64;
        for (unit, divisor) in [
            ("B", 1024.0f64),
            ("KB", 1024.0),
            ("MB", 1024.0),
            ("GB", 1024.0),
        ] {
            if size < divisor {
                return if unit == "B" {
                    format!("{:.0}{}", size, unit)
                } else {
                    format!("{:.1}{}", size, unit)
                };
            }
            size /= divisor;
        }
        format!("{:.1}TB", size)
    }

    pub(super) fn cmd_optimize(&self, args: &str) {
        if args.trim().to_lowercase() != "history" {
            self.vte()
                .feed(b"\r\n\x1b[33mUsage: /optimize history\x1b[0m\r\n");
            return;
        }
        if *self.imp().history_optimizing.borrow() {
            self.vte()
                .feed(b"\r\n\x1b[33mHistory optimization is already running.\x1b[0m\r\n");
            return;
        }
        self.vte()
            .feed(b"\r\n\x1b[90mOptimizing history database...\x1b[0m\r\n");
        *self.imp().history_optimizing.borrow_mut() = true;
        let weak = crate::SendWeak::new(self);
        let queued = history().optimize_async(Box::new(move |result| {
            crate::run_on_main(move || {
                if let Some(t) = weak.upgrade() {
                    *t.imp().history_optimizing.borrow_mut() = false;
                    let stats = match result {
                        Ok(stats) => stats,
                        Err(error) => {
                            t.vte().feed(
                                format!("\x1b[31m/optimize: {}\x1b[0m\r\n", error).as_bytes(),
                            );
                            return;
                        }
                    };
                    let dup = stats
                        .get("duplicates_removed")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let before = stats
                        .get("rows_before")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let after = stats
                        .get("rows_after")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let size_before = stats
                        .get("size_before")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let size_after = stats
                        .get("size_after")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    t.vte().feed(
                        format!(
                            "\x1b[32m/optimize: removed {} duplicate(s) ({} -> {} rows)\x1b[0m\r\n",
                            dup, before, after
                        )
                        .as_bytes(),
                    );
                    t.vte().feed(
                        format!(
                            "\x1b[32m/optimize: db size {} -> {}\x1b[0m\r\n",
                            Self::human_size(size_before),
                            Self::human_size(size_after)
                        )
                        .as_bytes(),
                    );
                }
            });
        }));
        if !queued {
            *self.imp().history_optimizing.borrow_mut() = false;
            self.vte()
                .feed(b"\x1b[31m/optimize: history database is unavailable\x1b[0m\r\n");
        }
    }

    pub(super) fn cmd_session(&self, args: &str) {
        let mut parts = args.split_whitespace();
        match parts.next().unwrap_or("").to_lowercase().as_str() {
            "name" => {
                let Some(name) = parts.next() else {
                    self.vte()
                        .feed(b"\r\n\x1b[33mUsage: /session name NAME\x1b[0m\r\n");
                    return;
                };
                if parts.next().is_some() {
                    self.vte()
                        .feed(b"\r\n\x1b[33mUsage: /session name NAME\x1b[0m\r\n");
                    return;
                }
                if let Err(error) = validate_name(name, "Remote session") {
                    self.vte().feed(
                        format!("\r\n\x1b[31m/session: {}\x1b[0m\r\n", error).as_bytes(),
                    );
                    return;
                }
                self.set_remote_name(name);
                self.vte().feed(
                    format!("\r\n\x1b[32mRemote session named '{}'.\x1b[0m\r\n", name)
                        .as_bytes(),
                );
            }
            "export" => {
                let Some(name) = parts.next() else {
                    self.vte()
                        .feed(b"\r\n\x1b[33mUsage: /session export NAME [FILE]\x1b[0m\r\n");
                    return;
                };
                let destination = parts
                    .next()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}.json", name));
                // Resolve relative paths against the shell's directory, not
                // oxterm's own working directory.
                let destination = if destination.starts_with("~/") {
                    dirs::home_dir()
                        .map(|h| h.join(&destination[2..]).to_string_lossy().into_owned())
                        .unwrap_or(destination)
                } else if std::path::Path::new(&destination).is_absolute() {
                    destination
                } else {
                    std::path::Path::new(&self.get_cwd())
                        .join(&destination)
                        .to_string_lossy()
                        .into_owned()
                };
                match session::export_session(name, &destination) {
                    Ok(()) => self.vte().feed(
                        format!(
                            "\r\n\x1b[32mSession '{}' exported to {}.\x1b[0m\r\n",
                            name, destination
                        )
                        .as_bytes(),
                    ),
                    Err(error) => self
                        .vte()
                        .feed(format!("\r\n\x1b[31m/session: {}\x1b[0m\r\n", error).as_bytes()),
                }
            }
            "list" => {
                let sessions = session::list_sessions();
                let output = if sessions.is_empty() {
                    "\r\n\x1b[90mNo saved sessions.\x1b[0m\r\n".to_string()
                } else {
                    format!("\r\nSaved sessions:\r\n  {}\r\n", sessions.join("\r\n  "))
                };
                self.vte().feed(output.as_bytes());
            }
            _ => self
                .vte()
                .feed(b"\r\n\x1b[33mUsage: /session name NAME | /session export NAME [FILE] | /session list\x1b[0m\r\n"),
        }
    }

    pub(super) fn cmd_snippet(&self, args: &str) {
        let mut parts = args.trim().splitn(3, char::is_whitespace);
        let action = parts.next().unwrap_or("");
        if action.eq_ignore_ascii_case("list") || action.is_empty() {
            let snippets = settings::json_to_str_map(&settings().get_obj("snippets"));
            if snippets.is_empty() {
                self.vte()
                    .feed(b"\r\n\x1b[90mNo snippets saved.\x1b[0m\r\n");
            } else {
                let mut output = "\r\nSaved snippets:\r\n".to_string();
                for (name, command) in snippets {
                    output.push_str(&format!("  {:<16} {}\r\n", name, command));
                }
                self.vte().feed(output.as_bytes());
            }
            return;
        }
        if action.eq_ignore_ascii_case("add") {
            let Some(name) = parts.next() else {
                self.vte()
                    .feed(b"\r\n\x1b[33mUsage: /snippet add NAME COMMAND\x1b[0m\r\n");
                return;
            };
            let command = parts.next().unwrap_or("").trim();
            if command.is_empty() {
                self.vte()
                    .feed(b"\r\n\x1b[33mUsage: /snippet add NAME COMMAND\x1b[0m\r\n");
                return;
            }
            if let Err(error) = validate_name(name, "Snippet") {
                self.vte()
                    .feed(format!("\r\n\x1b[31m/snippet: {}\x1b[0m\r\n", error).as_bytes());
                return;
            }
            let mut snippets = settings::json_to_str_map(&settings().get_obj("snippets"));
            snippets.insert(name.to_string(), command.to_string());
            let mut updates = std::collections::BTreeMap::new();
            updates.insert("snippets".to_string(), settings::str_map_to_json(&snippets));
            match settings().set_many(updates) {
                Ok(()) => self
                    .vte()
                    .feed(format!("\r\n\x1b[32mSnippet '{}' saved.\x1b[0m\r\n", name).as_bytes()),
                Err(error) => self
                    .vte()
                    .feed(format!("\r\n\x1b[31m/snippet: {}\x1b[0m\r\n", error).as_bytes()),
            }
            return;
        }
        if action.eq_ignore_ascii_case("delete") || action.eq_ignore_ascii_case("rm") {
            let Some(name) = parts.next() else {
                self.vte()
                    .feed(b"\r\n\x1b[33mUsage: /snippet delete NAME\x1b[0m\r\n");
                return;
            };
            let mut snippets = settings::json_to_str_map(&settings().get_obj("snippets"));
            if snippets.remove(name).is_none() {
                self.vte().feed(
                    format!("\r\n\x1b[33mSnippet '{}' not found.\x1b[0m\r\n", name).as_bytes(),
                );
                return;
            }
            let mut updates = std::collections::BTreeMap::new();
            updates.insert("snippets".to_string(), settings::str_map_to_json(&snippets));
            let _ = settings().set_many(updates);
            self.vte()
                .feed(format!("\r\n\x1b[32mSnippet '{}' deleted.\x1b[0m\r\n", name).as_bytes());
            return;
        }

        let snippets = settings::json_to_str_map(&settings().get_obj("snippets"));
        let Some(template) = snippets.get(action) else {
            self.vte().feed(
                format!(
                    "\r\n\x1b[33mUnknown snippet '{}'. Use /snippet list.\x1b[0m\r\n",
                    action
                )
                .as_bytes(),
            );
            return;
        };
        let arguments: Vec<&str> = parts.next().unwrap_or("").split_whitespace().collect();
        let expanded = expand_snippet(template, &arguments);
        self.feed_command_bytes(b"\x15");
        *self.imp().input_shadow.borrow_mut() = expanded.clone();
        self.vte().feed_child(expanded.as_bytes());
    }

    pub(super) fn cmd_help(&self) {
        let help_text = "\r\n\x1b[36m--- Oxterm Commands ---\x1b[0m\r\n\
  \x1b[33m/history [terms]\x1b[0m      Search history; prefix a term with - to exclude\r\n\
  \x1b[33m/history :sql SELECT\x1b[0m  Run a read-only history query\r\n\
  \x1b[33m/wnotes TEXT\x1b[0m          Save a timestamped Markdown note\r\n\
  \x1b[33m/onotes\x1b[0m               Open the configured notes file\r\n\
  \x1b[33m/learn FILE\x1b[0m           Import commands into history (never execute)\r\n\
  \x1b[33m/optimize history\x1b[0m     Deduplicate, vacuum and analyze history\r\n\
  \x1b[33m/session list\x1b[0m          List saved sessions\r\n\
  \x1b[33m/session name NAME\x1b[0m    Name the live terminal session\r\n\
  \x1b[33m/session export NAME\x1b[0m  Export a saved session as private JSON\r\n\
  \x1b[33m/snippet add NAME CMD\x1b[0m  Save a parameterized command snippet\r\n\
  \x1b[33m/snippet NAME [ARGS]\x1b[0m   Expand a saved snippet into the prompt\r\n\
  \x1b[33m/help\x1b[0m / \x1b[33m/clear\x1b[0m      Show this help / clear the screen\r\n\r\n\
  \x1b[90mTab\x1b[0m                    Complete /commands; press twice for history\r\n\
  \x1b[90mCtrl+R\x1b[0m                 History search; repeat to select next match\r\n\
  \x1b[90mCtrl+W/U\x1b[0m               Delete last history filter term / clear filter\r\n\
  \x1b[90mCtrl+Shift+F\x1b[0m          Search terminal scrollback\r\n\
  \x1b[90mCtrl+Shift+P\x1b[0m          Open command palette\r\n\
  \x1b[90mCtrl+Shift+T/N/W\x1b[0m      New tab / window / close tab\r\n\
  \x1b[90mCtrl+Shift+C/V\x1b[0m        Copy / paste\r\n\
  \x1b[90mCtrl+Plus/Minus/0\x1b[0m     Zoom in / out / reset\r\n\
  \x1b[90mCtrl+Shift+M, Ctrl+M\x1b[0m  Set / jump to quickmark\r\n\
  \x1b[90mCtrl+Shift+H\x1b[0m          Hint mode for URLs, paths and commits\r\n\
  \x1b[90mCtrl+Shift+Y\x1b[0m          VI copy mode\r\n\
\x1b[36m------------------------\x1b[0m\r\n";
        self.vte().feed(help_text.as_bytes());
    }

    pub(super) fn trigger_bell_notification(&self, exit_code: i64) {
        if !settings().get_bool("bell_notification") {
            return;
        }
        if crate::notes::which("notify-send").is_none() {
            return;
        }
        let cwd = self.get_cwd();
        let status = if exit_code == 0 {
            "succeeded".to_string()
        } else {
            format!("failed (exit {})", exit_code)
        };
        crate::notes::spawn_detached(
            "notify-send",
            &[
                "-a",
                "Oxterm",
                "-i",
                "terminal",
                "Command finished",
                &format!("Command {} in {}", status, cwd),
            ],
        );
    }
}
