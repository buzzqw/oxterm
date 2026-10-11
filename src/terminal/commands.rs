//! Oxterm `/commands` typed at the prompt or in the command bar.

use super::*;

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
        if args.starts_with('-') {
            let rest = &args[1..];
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
        history().optimize_async(Box::new(move |result| {
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

    pub(super) fn cmd_connect(&self, args: &str) {
        if args.is_empty() {
            self.show_provider_list();
            return;
        }
        let parts: Vec<&str> = args.split_whitespace().collect();
        let provider = parts[0].to_lowercase();
        if ai_client::provider_info(&provider).is_none() {
            let valid = ai_client::provider_keys().join("|");
            self.vte()
                .feed(format!("\r\n\x1b[31mInvalid provider: {}\x1b[0m\r\n", provider).as_bytes());
            self.vte()
                .feed(format!("\x1b[90mUse: /connect [{}]\x1b[0m\r\n", valid).as_bytes());
            return;
        }
        self.connect_to_provider(&provider);
    }

    pub(super) fn show_provider_list(&self) {
        self.vte()
            .feed(b"\r\n\x1b[90mChecking configured providers...\x1b[0m\r\n");
        *self.imp().async_pending.borrow_mut() = true;
        let weak = crate::SendWeak::new(self);
        let gen = {
            let mut generation = self.imp().async_generation.borrow_mut();
            *generation += 1;
            *generation
        };
        std::thread::spawn(move || {
            let s = settings();
            let keys = settings::json_to_str_map(&s.get_obj("ai_keys"));
            let models = settings::json_to_str_map(&s.get_obj("ai_models"));
            let urls = settings::json_to_str_map(&s.get_obj("ai_urls"));
            let mut available: Vec<(String, String, bool)> = Vec::new();
            for provider in ["openai", "claude", "gemini", "deepseek", "ollama", "custom"] {
                let Some((name, url, default_model, _)) = ai_client::provider_info(provider) else {
                    continue;
                };
                let info_url = url.to_string();
                if provider == "ollama" || provider == "custom" {
                    let url = urls
                        .get(provider)
                        .cloned()
                        .filter(|u| !u.is_empty())
                        .unwrap_or(info_url);
                    if ai_client::ping_provider(provider, &url) {
                        let fetched = ai_client::fetch_models(provider, "", &url);
                        let model = models
                            .get(provider)
                            .cloned()
                            .filter(|m| !m.is_empty())
                            .unwrap_or_else(|| {
                                fetched
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| default_model.to_string())
                            });
                        let mut label = format!("{} ({})", name, model);
                        if !fetched.is_empty() {
                            label.push_str(&format!(" [{} models]", fetched.len()));
                        }
                        available.push((provider.to_string(), label, true));
                    }
                } else {
                    let key = keys.get(provider).cloned().unwrap_or_default();
                    if !key.is_empty() {
                        let model = models
                            .get(provider)
                            .cloned()
                            .filter(|m| !m.is_empty())
                            .unwrap_or_else(|| default_model.to_string());
                        let label = format!("{} ({})", name, model);
                        available.push((provider.to_string(), label, false));
                    }
                }
            }
            crate::run_on_main(move || {
                if let Some(t) = weak.upgrade() {
                    if gen == *t.imp().async_generation.borrow() {
                        t.on_provider_list_ready(available);
                    }
                }
            });
        });
    }

    pub(super) fn on_provider_list_ready(&self, available: Vec<(String, String, bool)>) {
        *self.imp().async_pending.borrow_mut() = false;
        if available.is_empty() {
            self.vte()
                .feed(b"\x1b[33mNo providers configured.\x1b[0m\r\n");
            self.vte()
                .feed(b"\x1b[90mSet API keys in Preferences > AI.\x1b[0m\r\n");
            self.abort_pending_ai_connection();
            *self.imp().input_shadow.borrow_mut() = String::new();
            return;
        }
        if (*self.imp().ai_activate_after_connect.borrow()
            || *self.imp().ai_one_shot_after_connect.borrow())
            && available.len() == 1
        {
            self.connect_to_provider(&available[0].0);
            return;
        }
        let mut out = "\r\n\x1b[36mAvailable providers:\x1b[0m\r\n".to_string();
        let mut list: Vec<(usize, String, bool)> = Vec::new();
        for (i, (prov, label, fetched)) in available.iter().take(9).enumerate() {
            let num = i + 1;
            let icon = if *fetched {
                "\x1b[32m\u{25cf}\x1b[0m"
            } else {
                "\x1b[33m\u{25cf}\x1b[0m"
            };
            out.push_str(&format!(
                "  \x1b[33m[{}]\x1b[0m {} {}\r\n",
                num, icon, label
            ));
            list.push((num, prov.clone(), *fetched));
        }
        out.push_str("\x1b[90mPress 1..9 to select a provider, Esc to cancel.\x1b[0m\r\n");
        self.vte().feed(out.as_bytes());
        *self.imp().provider_list.borrow_mut() = list;
    }

    pub(super) fn cancel_async_wait(&self) {
        *self.imp().async_pending.borrow_mut() = false;
        *self.imp().async_generation.borrow_mut() += 1;
        self.abort_pending_ai_connection();
        self.imp().provider_list.borrow_mut().clear();
        self.imp().model_list.borrow_mut().clear();
        self.vte().feed(b"\r\n\x1b[37mCancelled.\x1b[0m\r\n");
        *self.imp().input_shadow.borrow_mut() = String::new();
        self.exit_history_search_mode();
    }

    pub(super) fn select_provider_number(&self, num: usize) {
        let providers = self.imp().provider_list.borrow().clone();
        for (n, prov, _) in providers {
            if n == num {
                self.imp().provider_list.borrow_mut().clear();
                self.connect_to_provider(&prov);
                return;
            }
        }
        self.imp().provider_list.borrow_mut().clear();
    }

    pub(super) fn connect_to_provider(&self, provider: &str) {
        let s = settings();
        let keys = settings::json_to_str_map(&s.get_obj("ai_keys"));
        let models = settings::json_to_str_map(&s.get_obj("ai_models"));
        let urls = settings::json_to_str_map(&s.get_obj("ai_urls"));
        let key = keys.get(provider).cloned().unwrap_or_default();
        let model = models.get(provider).cloned().unwrap_or_default();
        let base_url = urls.get(provider).cloned().unwrap_or_default();
        let Some((name, _url, _dm, _proto)) = ai_client::provider_info(provider) else {
            return;
        };

        if provider != "ollama" && provider != "custom" && key.is_empty() {
            self.vte().feed(
                format!("\r\n\x1b[33mNo API key set for {}.\x1b[0m\r\n", provider).as_bytes(),
            );
            self.abort_pending_ai_connection();
            return;
        }

        *self.imp().connect_provider.borrow_mut() = provider.to_string();
        *self.imp().connect_key.borrow_mut() = key.clone();
        *self.imp().connect_url.borrow_mut() = base_url.clone();
        *self.imp().connect_model.borrow_mut() = model.clone();

        self.vte()
            .feed(format!("\r\n\x1b[90mConnecting to {}...\x1b[0m\r\n", name).as_bytes());
        if !model.is_empty() {
            *self.imp().async_pending.borrow_mut() = false;
            self.do_connect(provider, &key, &model, &base_url, true);
            return;
        }
        *self.imp().async_pending.borrow_mut() = true;
        let gen = *self.imp().async_generation.borrow();
        let weak = crate::SendWeak::new(self);
        let provider = provider.to_string();
        std::thread::spawn(move || {
            let models = ai_client::fetch_models(&provider, &key, &base_url);
            crate::run_on_main(move || {
                if let Some(t) = weak.upgrade() {
                    if gen == *t.imp().async_generation.borrow() {
                        t.on_models_fetched(provider.to_string(), key, model, base_url, models);
                    }
                }
            });
        });
    }

    pub(super) fn on_models_fetched(
        &self,
        provider: String,
        key: String,
        model: String,
        base_url: String,
        models: Vec<String>,
    ) {
        *self.imp().async_pending.borrow_mut() = false;
        if models.is_empty() {
            self.vte().feed(
                format!(
                    "\r\n\x1b[31mCould not retrieve the models supported by {}. Configure a model in Preferences > AI or check the API key.\x1b[0m\r\n",
                    provider
                )
                .as_bytes(),
            );
            self.abort_pending_ai_connection();
        } else if model.is_empty() {
            let Some((name, _u, _d, _p)) = ai_client::provider_info(&provider) else {
                return;
            };
            let mut out = format!(
                "\r\n\x1b[36m{} \u{2014} {} models:\x1b[0m\r\n",
                name,
                models.len()
            );
            let mut list = Vec::new();
            for (i, m) in models.iter().take(9).enumerate() {
                let num = i + 1;
                let marker = if *m == model {
                    " \x1b[32m(current)\x1b[0m"
                } else {
                    ""
                };
                out.push_str(&format!("  \x1b[33m[{}]\x1b[0m {}{}\r\n", num, m, marker));
                list.push((num, m.clone()));
            }
            out.push_str("\x1b[90mChoose the model with 1..9, or press Esc to cancel.\x1b[0m\r\n");
            self.vte().feed(out.as_bytes());
            *self.imp().model_list.borrow_mut() = list;
        } else {
            self.do_connect(&provider, &key, &model, &base_url, true);
        }
    }

    pub(super) fn select_model_number(&self, num: usize) {
        let models = self.imp().model_list.borrow().clone();
        for (n, m) in models {
            if n == num {
                self.imp().model_list.borrow_mut().clear();
                let provider = self.imp().connect_provider.borrow().clone();
                let key = self.imp().connect_key.borrow().clone();
                let url = self.imp().connect_url.borrow().clone();
                self.do_connect(&provider, &key, &m, &url, true);
                return;
            }
        }
        self.imp().model_list.borrow_mut().clear();
    }

    pub(super) fn do_connect(
        &self,
        provider: &str,
        api_key: &str,
        model: &str,
        base_url: &str,
        feed_prompt: bool,
    ) {
        let model_opt = if model.is_empty() { None } else { Some(model) };
        match AIClient::new(provider, api_key, model_opt, base_url) {
            Ok(mut client) => {
                let sys_prompts =
                    settings::json_to_str_map(&settings().get_obj("ai_system_prompts"));
                if let Some(prompt) = sys_prompts.get(provider) {
                    client.set_system_prompt(prompt);
                }
                let mut updates = std::collections::BTreeMap::new();
                updates.insert("ai_last_provider".to_string(), serde_json::json!(provider));
                updates.insert("ai_provider".to_string(), serde_json::json!(provider));
                if !model.is_empty() {
                    let mut models = settings::json_to_str_map(&settings().get_obj("ai_models"));
                    models.insert(provider.to_string(), model.to_string());
                    updates.insert("ai_models".to_string(), settings::str_map_to_json(&models));
                }
                if !base_url.is_empty() {
                    let mut urls = settings::json_to_str_map(&settings().get_obj("ai_urls"));
                    urls.insert(provider.to_string(), base_url.to_string());
                    updates.insert("ai_urls".to_string(), settings::str_map_to_json(&urls));
                }
                let _ = settings().set_many(updates);
                let model_name = client.model.clone();
                *self.imp().ai_client.borrow_mut() = Some(Arc::new(client));
                if let Some((name, _u, _d, _p)) = ai_client::provider_info(provider) {
                    self.vte().feed(
                        format!(
                            "\r\n\x1b[32m\u{2713} Connected to {} ({})\x1b[0m\r\n",
                            name, model_name
                        )
                        .as_bytes(),
                    );
                }
                let activate_ai = *self.imp().ai_activate_after_connect.borrow();
                let one_shot = *self.imp().ai_one_shot_after_connect.borrow();
                if activate_ai {
                    *self.imp().ai_activate_after_connect.borrow_mut() = false;
                    let prompt = std::mem::take(&mut *self.imp().ai_pending_prompt.borrow_mut());
                    self.finish_ai_start(&prompt);
                } else if one_shot {
                    *self.imp().ai_one_shot_after_connect.borrow_mut() = false;
                    let prompt = std::mem::take(&mut *self.imp().ai_pending_prompt.borrow_mut());
                    self.begin_ai_one_shot(&prompt);
                } else {
                    self.vte()
                        .feed(b"\x1b[90mType /ai to start chatting.\x1b[0m\r\n");
                }
                if feed_prompt && !one_shot {
                    self.vte().feed_child(b"\r");
                }
            }
            Err(e) => {
                self.abort_pending_ai_connection();
                self.vte()
                    .feed(format!("\r\n\x1b[31mFailed to connect: {}\x1b[0m\r\n", e).as_bytes());
            }
        }
    }

    pub(super) fn cmd_help(&self) {
        let help_text = "\r\n\x1b[36m--- Oxterm Commands ---\x1b[0m\r\n\
  \x1b[33m/history [terms]\x1b[0m      Search history; prefix a term with - to exclude\r\n\
  \x1b[33m/history :sql SELECT\x1b[0m  Run a read-only history query\r\n\
  \x1b[33m/ai [on]\x1b[0m               Start optional AI chat\r\n\
  \x1b[33m? QUESTION\x1b[0m              Ask AI once and return to the shell prompt\r\n\
  \x1b[33m/ai explain\x1b[0m           Explain the latest failed command\r\n\
  \x1b[33m/ai repair\x1b[0m            Suggest a safe repair for the latest failure\r\n\
  \x1b[33m/ai context N q\x1b[0m       Include the last N terminal lines as context\r\n\
  \x1b[33m/ai off\x1b[0m               Exit AI chat mode\r\n\
  \x1b[33m/connect [provider]\x1b[0m   Select and test an AI provider\r\n\
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
