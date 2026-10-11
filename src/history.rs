use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

// use crate::logging::LOGGER;
use crate::settings;

pub fn history_db_path() -> PathBuf {
    settings::config_dir().join("history.db")
}

const TRIM_CHECK_INTERVAL: u32 = 50;
const TRIM_MAX_ROWS: i64 = 1_000_000;
const HISTORY_QUEUE_SIZE: usize = 256;

type SearchCallback = Box<dyn FnOnce(Vec<Vec<Value>>) + Send + 'static>;
type SqlCallback = Box<dyn FnOnce(Result<Vec<Vec<Value>>, String>) + Send + 'static>;
type AddManyCallback = Box<dyn FnOnce(usize) + Send + 'static>;
type LatestFailedCallback = Box<dyn FnOnce(Option<Vec<Value>>) + Send + 'static>;
type OptimizeCallback = Box<dyn FnOnce(Result<BTreeMap<String, Value>, String>) + Send + 'static>;

enum HistoryRequest {
    Add {
        ticket: Option<u64>,
        command: String,
        cwd: String,
        exit_code: i64,
        duration_ms: Option<i64>,
        git_branch: Option<String>,
        /// Resolve the git branch of `cwd` on the worker (spawns `git`).
        lookup_branch: bool,
    },
    SetResult {
        ticket: u64,
        exit_code: i64,
        duration_ms: Option<i64>,
    },
    AddMany {
        commands: Vec<String>,
        cwd: String,
        exit_code: i64,
        callback: AddManyCallback,
    },
    Search {
        terms: String,
        limit: i64,
        cwd: String,
        callback: SearchCallback,
    },
    SqlSearch {
        sql: String,
        callback: SqlCallback,
    },
    LatestFailed {
        cwd: String,
        callback: LatestFailedCallback,
    },
    Optimize {
        callback: OptimizeCallback,
    },
    Clear,
}

struct HistoryWorker {
    sender: SyncSender<HistoryRequest>,
    overflow_sender: mpsc::Sender<HistoryRequest>,
}

static HISTORY_WORKER: OnceLock<HistoryWorker> = OnceLock::new();
static HISTORY_TICKET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// Requests parked in the overflow channel. While any are pending, new
/// requests must queue behind them so the worker sees them in order.
static OVERFLOW_IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn history_worker() -> &'static HistoryWorker {
    HISTORY_WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(HISTORY_QUEUE_SIZE);
        let (overflow_sender, overflow_receiver) = mpsc::channel();
        let sender_for_overflow = sender.clone();
        std::thread::Builder::new()
            .name("oxterm-history".to_string())
            .spawn(move || history_worker_loop(receiver))
            .expect("could not start history worker");
        std::thread::Builder::new()
            .name("oxterm-history-overflow".to_string())
            .spawn(move || {
                for request in overflow_receiver {
                    let sent = sender_for_overflow.send(request).is_ok();
                    OVERFLOW_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
                    if !sent {
                        break;
                    }
                }
            })
            .expect("could not start history overflow worker");
        HistoryWorker {
            sender,
            overflow_sender,
        }
    })
}

fn history_worker_loop(receiver: mpsc::Receiver<HistoryRequest>) {
    // Opening (and, on upgrade, migrating/rebuilding the FTS index) happens
    // here so the GTK thread never touches the database.
    let dir = settings::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let path = history_db_path();
    let manager = HistoryManager::open_and_initialize(&path)
        .map(|conn| {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            HistoryManager {
                conn: Mutex::new(conn),
                inserts_since_trim: Mutex::new(0),
            }
        })
        .unwrap_or_else(|error| {
            crate::logging::Logger::log(
                "WARNING",
                "history",
                "worker using temporary in-memory history",
                Some(&error.to_string()),
            );
            HistoryManager {
                conn: Mutex::new(
                    HistoryManager::initialize_connection(
                        Connection::open_in_memory().expect("open in-memory history database"),
                    )
                    .expect("initialize in-memory history database"),
                ),
                inserts_since_trim: Mutex::new(0),
            }
        });
    let mut pending = BTreeMap::<u64, i64>::new();
    let mut completed = BTreeMap::<u64, (i64, Option<i64>)>::new();

    for request in receiver {
        match request {
            HistoryRequest::Add {
                ticket,
                command,
                cwd,
                exit_code,
                duration_ms,
                git_branch,
                lookup_branch,
            } => {
                let git_branch = git_branch.or_else(|| {
                    if lookup_branch {
                        current_git_branch(&cwd)
                    } else {
                        None
                    }
                });
                let id = manager.add_with_context(
                    &command,
                    &cwd,
                    exit_code,
                    duration_ms,
                    git_branch.as_deref(),
                );
                if let Some(ticket) = ticket {
                    if id != 0 {
                        if let Some((exit_code, duration_ms)) = completed.remove(&ticket) {
                            manager.set_command_result(Some(id), exit_code, duration_ms);
                            continue;
                        }
                        if pending.len() >= 4096 {
                            if let Some(oldest) = pending.keys().next().copied() {
                                pending.remove(&oldest);
                            }
                        }
                        pending.insert(ticket, id);
                    }
                }
            }
            HistoryRequest::SetResult {
                ticket,
                exit_code,
                duration_ms,
            } => {
                if let Some(id) = pending.remove(&ticket) {
                    manager.set_command_result(Some(id), exit_code, duration_ms);
                } else {
                    if completed.len() >= 4096 {
                        if let Some(oldest) = completed.keys().next().copied() {
                            completed.remove(&oldest);
                        }
                    }
                    completed.insert(ticket, (exit_code, duration_ms));
                }
            }
            HistoryRequest::AddMany {
                commands,
                cwd,
                exit_code,
                callback,
            } => {
                let added = manager.add_many(&commands, &cwd, exit_code);
                callback(added);
            }
            HistoryRequest::Search {
                terms,
                limit,
                cwd,
                callback,
            } => callback(manager.search(&terms, limit, &cwd)),
            HistoryRequest::SqlSearch { sql, callback } => {
                callback(manager.sql_search(&sql));
            }
            HistoryRequest::LatestFailed { cwd, callback } => {
                callback(manager.latest_failed(&cwd));
            }
            HistoryRequest::Optimize { callback } => callback(manager.optimize()),
            HistoryRequest::Clear => manager.clear(),
        }
    }
}

/// Queue a request for the worker without ever blocking the caller. Once a
/// request has spilled into the overflow channel, later ones follow it there
/// so they cannot overtake it (e.g. a Clear overtaking an earlier Add).
/// Returns false if the worker has stopped and the request was dropped.
fn send_history_request(request: HistoryRequest, what: &str) -> bool {
    let worker = history_worker();
    let request = if OVERFLOW_IN_FLIGHT.load(Ordering::Acquire) == 0 {
        match worker.sender.try_send(request) {
            Ok(()) => return true,
            Err(TrySendError::Full(request)) => request,
            Err(TrySendError::Disconnected(_)) => {
                crate::logging::log_warning(&format!("history worker stopped; {} skipped", what));
                return false;
            }
        }
    } else {
        request
    };
    OVERFLOW_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
    if worker.overflow_sender.send(request).is_err() {
        OVERFLOW_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
        crate::logging::log_warning(&format!("history worker stopped; {} dropped", what));
        return false;
    }
    true
}

/// Branch of the git repository containing `cwd`, if any. Runs `git`, so it
/// is only called from the history worker thread.
fn current_git_branch(cwd: &str) -> Option<String> {
    // A command can emit more than one OSC 133 command-start event in quick
    // succession; a short TTL avoids spawning git for each of them.
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(entries) = cache.lock() {
        if let Some((at, branch)) = entries.get(cwd) {
            if at.elapsed() < Duration::from_secs(2) {
                return branch.clone();
            }
        }
    }
    let output = std::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok();
    let branch = output.and_then(|output| {
        if !output.status.success() {
            return None;
        }
        let branch = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch)
        }
    });
    if let Ok(mut entries) = cache.lock() {
        if entries.len() >= 256 {
            entries.clear();
        }
        entries.insert(cwd.to_string(), (Instant::now(), branch.clone()));
    }
    branch
}

/// Owns the SQLite connection. Only the history worker thread (and tests)
/// create one; everything else goes through [`History`].
pub struct HistoryManager {
    conn: Mutex<Connection>,
    inserts_since_trim: Mutex<u32>,
}

/// Handle used by the UI. Every operation is queued to the worker thread, so
/// none of them can block GTK on disk I/O or a long VACUUM.
pub struct History;

static HISTORY: History = History;

pub fn history() -> &'static History {
    &HISTORY
}

impl History {
    /// Queue a history entry. Used for commands whose result is not updated.
    pub fn add_async(&self, command: &str, cwd: &str, exit_code: i64) {
        let request = HistoryRequest::Add {
            ticket: None,
            command: command.to_string(),
            cwd: cwd.to_string(),
            exit_code,
            duration_ms: None,
            git_branch: None,
            lookup_branch: false,
        };
        let _ = send_history_request(request, "history entry");
    }

    /// Queue a command-start entry and return a ticket that can later be used
    /// to update its exit code and duration. The ticket is deliberately local
    /// to the worker and is not exposed as a database row ID. The git branch
    /// of `cwd` is resolved on the worker.
    pub fn add_command_start_async(&self, command: &str, cwd: &str) -> Option<u64> {
        let ticket = HISTORY_TICKET.fetch_add(1, Ordering::Relaxed);
        let request = HistoryRequest::Add {
            ticket: Some(ticket),
            command: command.to_string(),
            cwd: cwd.to_string(),
            exit_code: -1,
            duration_ms: None,
            git_branch: None,
            lookup_branch: true,
        };
        let _ = send_history_request(request, "history entry");
        Some(ticket)
    }

    pub fn set_command_result_async(&self, ticket: u64, exit_code: i64, duration_ms: Option<i64>) {
        let request = HistoryRequest::SetResult {
            ticket,
            exit_code,
            duration_ms,
        };
        let _ = send_history_request(request, "result update");
    }

    pub fn add_many_async(
        &self,
        commands: Vec<String>,
        cwd: String,
        exit_code: i64,
        callback: AddManyCallback,
    ) {
        let request = HistoryRequest::AddMany {
            commands,
            cwd,
            exit_code,
            callback,
        };
        let _ = send_history_request(request, "import");
    }

    pub fn search_async(&self, terms: String, limit: i64, cwd: String, callback: SearchCallback) {
        let request = HistoryRequest::Search {
            terms,
            limit,
            cwd,
            callback,
        };
        let _ = send_history_request(request, "search");
    }

    pub fn sql_search_async(&self, sql: String, callback: SqlCallback) {
        let _ = send_history_request(HistoryRequest::SqlSearch { sql, callback }, "SQL search");
    }

    pub fn latest_failed_async(&self, cwd: String, callback: LatestFailedCallback) {
        let _ = send_history_request(HistoryRequest::LatestFailed { cwd, callback }, "lookup");
    }

    /// Returns false if the request could not be queued (worker stopped).
    pub fn optimize_async(&self, callback: OptimizeCallback) -> bool {
        send_history_request(HistoryRequest::Optimize { callback }, "optimize")
    }

    pub fn clear_async(&self) {
        let _ = send_history_request(HistoryRequest::Clear, "history clear");
    }
}

impl HistoryManager {
    fn open_and_initialize(path: &std::path::Path) -> rusqlite::Result<Connection> {
        let conn = Connection::open(path)?;
        Self::initialize_connection(conn)
    }

    fn initialize_connection(conn: Connection) -> rusqlite::Result<Connection> {
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL is durable against application crashes in WAL mode and
        // avoids an fsync on every single command insert.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                cwd TEXT,
                exit_code INTEGER,
                timestamp TEXT NOT NULL,
                duration_ms INTEGER,
                git_branch TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_commands_ts ON commands(timestamp);
            CREATE INDEX IF NOT EXISTS idx_commands_command_cwd ON commands(command, cwd);",
        )?;
        // Existing installations predate command duration and branch metadata.
        // SQLite has no IF NOT EXISTS form for ADD COLUMN, so only ignore the
        // expected duplicate-column error from an already migrated database.
        Self::add_column_if_missing(&conn, "ALTER TABLE commands ADD COLUMN duration_ms INTEGER")?;
        Self::add_column_if_missing(&conn, "ALTER TABLE commands ADD COLUMN git_branch TEXT")?;
        if let Err(error) = Self::initialize_fts(&conn) {
            crate::logging::Logger::log(
                "WARNING",
                "history",
                "history FTS index unavailable; using LIKE search",
                Some(&error.to_string()),
            );
        }
        Ok(conn)
    }

    fn initialize_fts(conn: &Connection) -> rusqlite::Result<()> {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'commands_fts')",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            conn.execute_batch(
                "CREATE VIRTUAL TABLE commands_fts USING fts5(
                    command,
                    content='commands',
                    content_rowid='id',
                    tokenize='trigram'
                );
                INSERT INTO commands_fts(commands_fts) VALUES ('rebuild');",
            )?;
        }
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS commands_fts_ai AFTER INSERT ON commands BEGIN
                 INSERT INTO commands_fts(rowid, command) VALUES (new.id, new.command);
             END;
             CREATE TRIGGER IF NOT EXISTS commands_fts_ad AFTER DELETE ON commands BEGIN
                 INSERT INTO commands_fts(commands_fts, rowid, command)
                 VALUES ('delete', old.id, old.command);
             END;
             CREATE TRIGGER IF NOT EXISTS commands_fts_au AFTER UPDATE OF command ON commands BEGIN
                 INSERT INTO commands_fts(commands_fts, rowid, command)
                 VALUES ('delete', old.id, old.command);
                 INSERT INTO commands_fts(rowid, command) VALUES (new.id, new.command);
             END;",
        )
    }

    fn add_column_if_missing(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
        match conn.execute(sql, []) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("duplicate column name") =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn now_ts() -> String {
        crate::logging::utc_iso_now()
    }

    fn bump_inserts(&self, n: usize) {
        let should_trim = {
            let mut c = self.inserts_since_trim.lock().unwrap();
            *c += n as u32;
            if *c >= TRIM_CHECK_INTERVAL {
                *c = 0;
                true
            } else {
                false
            }
        };
        if should_trim {
            self.trim();
        }
    }

    #[cfg(test)]
    pub fn add(&self, command: &str, cwd: &str, exit_code: i64) -> i64 {
        self.add_with_context(command, cwd, exit_code, None, None)
    }

    pub fn add_with_context(
        &self,
        command: &str,
        cwd: &str,
        exit_code: i64,
        duration_ms: Option<i64>,
        git_branch: Option<&str>,
    ) -> i64 {
        let Ok(conn) = self.conn.lock() else {
            crate::logging::log_warning("history database lock is unavailable");
            return 0;
        };
        let ts = Self::now_ts();
        let id = match conn.execute(
            "INSERT INTO commands
                 (command, cwd, exit_code, timestamp, duration_ms, git_branch)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            params![command, cwd, exit_code, ts, duration_ms, git_branch],
        ) {
            Ok(_) => conn.last_insert_rowid(),
            Err(error) => {
                crate::logging::Logger::log(
                    "WARNING",
                    "history",
                    "could not add history entry",
                    Some(&error.to_string()),
                );
                0
            }
        };
        drop(conn);
        if id != 0 {
            self.bump_inserts(1);
        }
        id
    }

    pub fn add_many(&self, commands: &[String], cwd: &str, exit_code: i64) -> usize {
        if commands.is_empty() {
            return 0;
        }
        let Ok(conn) = self.conn.lock() else {
            crate::logging::log_warning("history database lock is unavailable");
            return 0;
        };
        let mut n = 0usize;
        // One transaction for the whole import instead of a commit per row.
        let Ok(tx) = conn.unchecked_transaction() else {
            crate::logging::log_warning("could not start history import");
            return 0;
        };
        {
            let Ok(mut stmt) = tx.prepare(
                "INSERT INTO commands (command, cwd, exit_code, timestamp) VALUES (?1,?2,?3,?4)",
            ) else {
                crate::logging::log_warning("could not prepare history import");
                return 0;
            };
            let ts = Self::now_ts();
            let mut failed = false;
            for cmd in commands {
                if stmt.execute(params![cmd, cwd, exit_code, ts]).is_ok() {
                    n += 1;
                } else {
                    failed = true;
                }
            }
            if failed {
                crate::logging::log_warning("some history import entries could not be written");
            }
        }
        if let Err(error) = tx.commit() {
            crate::logging::Logger::log(
                "WARNING",
                "history",
                "could not commit history import",
                Some(&error.to_string()),
            );
            return 0;
        }
        drop(conn);
        self.bump_inserts(n);
        n
    }

    pub fn set_command_result(
        &self,
        row_id: Option<i64>,
        exit_code: i64,
        duration_ms: Option<i64>,
    ) {
        if let Some(id) = row_id {
            let Ok(conn) = self.conn.lock() else {
                crate::logging::log_warning("history database lock is unavailable");
                return;
            };
            if let Err(error) = conn.execute(
                "UPDATE commands SET exit_code = ?1, duration_ms = ?2 WHERE id = ?3",
                params![exit_code, duration_ms, id],
            ) {
                crate::logging::Logger::log(
                    "WARNING",
                    "history",
                    "could not update history entry",
                    Some(&error.to_string()),
                );
            }
        }
    }

    pub fn latest_failed(&self, cwd: &str) -> Option<Vec<Value>> {
        let Ok(conn) = self.conn.lock() else {
            crate::logging::log_warning("history database lock is unavailable");
            return None;
        };
        let sql = if cwd.is_empty() {
            "SELECT id, command, cwd, timestamp, exit_code, duration_ms, git_branch
             FROM commands
             WHERE exit_code IS NOT NULL AND exit_code != 0 AND exit_code >= 0
             ORDER BY id DESC LIMIT 1"
        } else {
            "SELECT id, command, cwd, timestamp, exit_code, duration_ms, git_branch
             FROM commands
             WHERE cwd = ?1 AND exit_code IS NOT NULL AND exit_code != 0 AND exit_code >= 0
             ORDER BY id DESC LIMIT 1"
        };
        let mut stmt = conn.prepare(sql).ok()?;
        let row = if cwd.is_empty() {
            stmt.query_row([], Self::map_row_7)
        } else {
            stmt.query_row(params![cwd], Self::map_row_7)
        };
        row.ok()
    }

    fn like_escape(term: &str) -> String {
        term.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    }

    pub fn search(&self, terms: &str, limit: i64, cwd: &str) -> Vec<Vec<Value>> {
        let Ok(conn) = self.conn.lock() else {
            crate::logging::log_warning("history database lock is unavailable");
            return Vec::new();
        };
        if terms.trim().is_empty() {
            let result = if !cwd.is_empty() {
                (|| -> rusqlite::Result<Vec<Vec<Value>>> {
                    let mut stmt = conn.prepare(
                        "SELECT MAX(id) AS id, command, cwd, timestamp, exit_code, duration_ms, git_branch FROM commands \
                         WHERE command NOT LIKE '/%' ESCAPE '\\' GROUP BY command \
                         ORDER BY CASE WHEN cwd = ?1 THEN 0 ELSE 1 END, id DESC LIMIT ?2",
                    )?;
                    let iter = stmt.query_map(params![cwd, limit], Self::map_row_7)?;
                    iter.collect()
                })()
            } else {
                (|| -> rusqlite::Result<Vec<Vec<Value>>> {
                    let mut stmt = conn.prepare(
                        "SELECT MAX(id) AS id, command, cwd, timestamp, exit_code, duration_ms, git_branch FROM commands \
                         WHERE command NOT LIKE '/%' ESCAPE '\\' GROUP BY command \
                         ORDER BY id DESC LIMIT ?1",
                    )?;
                    let iter = stmt.query_map(params![limit], Self::map_row_7)?;
                    iter.collect()
                })()
            };
            return Self::search_result(result);
        }

        let parts: Vec<&str> = terms.split_whitespace().collect();
        let mut pos: Vec<&str> = Vec::new();
        let mut neg: Vec<&str> = Vec::new();
        for p in &parts {
            if p.starts_with('-') && p.len() > 1 {
                neg.push(&p[1..]);
            } else {
                pos.push(p);
            }
        }

        if !pos.is_empty() && pos.iter().all(|term| term.chars().count() >= 3) {
            if let Some(result) = Self::search_fts(&conn, &pos, &neg, limit, cwd) {
                return result;
            }
        }

        let mut where_clauses: Vec<String> = Vec::new();
        let mut where_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if pos.len() == 1 {
            let escaped = Self::like_escape(pos[0]);
            where_clauses.push(
                "((command LIKE ? ESCAPE '\\') OR (REPLACE(command, ' ', '') LIKE ? ESCAPE '\\'))"
                    .to_string(),
            );
            where_params.push(Box::new(format!("%{}%", escaped)));
            where_params.push(Box::new(format!("%{}%", escaped)));
        } else if pos.len() > 1 {
            let conds = vec!["command LIKE ? ESCAPE '\\'"; pos.len()].join(" AND ");
            where_clauses.push(format!("({})", conds));
            for p in &pos {
                where_params.push(Box::new(format!("%{}%", Self::like_escape(p))));
            }
        }
        for n in &neg {
            where_clauses.push("command NOT LIKE ? ESCAPE '\\'".to_string());
            where_params.push(Box::new(format!("%{}%", Self::like_escape(n))));
        }
        where_clauses.push("command NOT LIKE '/%' ESCAPE '\\'".to_string());
        let where_sql = where_clauses.join(" AND ");

        let mut order_parts: Vec<String> = Vec::new();
        let mut order_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !pos.is_empty() {
            let first = pos[0];
            order_parts.push("CASE WHEN command LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END".to_string());
            order_params.push(Box::new(format!("{}%", Self::like_escape(first))));
        }
        if !cwd.is_empty() {
            order_parts.push("CASE WHEN cwd = ? THEN 0 ELSE 1 END".to_string());
            order_params.push(Box::new(cwd.to_string()));
        }
        if pos.len() > 1 {
            let sum = pos
                .iter()
                .map(|_| "INSTR(LOWER(command), ?)".to_string())
                .collect::<Vec<_>>()
                .join(" + ");
            order_parts.push(format!("({})", sum));
            for p in &pos {
                order_params.push(Box::new(p.to_lowercase()));
            }
        }
        order_parts.push("LENGTH(command)".to_string());
        order_parts.push("id DESC".to_string());
        let order_sql = order_parts.join(", ");

        let sql = format!(
            "SELECT MAX(id) AS id, command, cwd, timestamp, exit_code, duration_ms, git_branch FROM commands \
             WHERE {} GROUP BY command ORDER BY {} LIMIT ?",
            where_sql, order_sql
        );

        let mut all: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        all.extend(where_params);
        all.extend(order_params);
        all.push(Box::new(limit));
        let result = (|| -> rusqlite::Result<Vec<Vec<Value>>> {
            let mut stmt = conn.prepare(&sql)?;
            let iter = stmt.query_map(rusqlite::params_from_iter(all), Self::map_row_7)?;
            iter.collect()
        })();
        Self::search_result(result)
    }

    fn search_fts(
        conn: &Connection,
        positive: &[&str],
        negative: &[&str],
        limit: i64,
        cwd: &str,
    ) -> Option<Vec<Vec<Value>>> {
        let fts_query = positive
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut subquery = vec![
            "f.commands_fts MATCH ?".to_string(),
            "c2.command NOT LIKE '/%' ESCAPE '\\'".to_string(),
        ];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query)];
        for term in negative {
            subquery.push("c2.command NOT LIKE ? ESCAPE '\\'".to_string());
            params.push(Box::new(format!("%{}%", Self::like_escape(term))));
        }

        let mut candidates = format!(
            "SELECT MAX(c2.id) FROM commands c2 \
             JOIN commands_fts f ON f.rowid = c2.id \
             WHERE {} GROUP BY c2.command",
            subquery.join(" AND ")
        );
        // Preserve the historical compact-match behavior (`gitstatus` also
        // matched `git status`) alongside the normal FTS substring match.
        if positive.len() == 1 {
            let mut compact = vec![
                "REPLACE(c3.command, ' ', '') LIKE ? ESCAPE '\\'".to_string(),
                "c3.command NOT LIKE '/%' ESCAPE '\\'".to_string(),
            ];
            params.push(Box::new(format!("%{}%", Self::like_escape(positive[0]))));
            for term in negative {
                compact.push("c3.command NOT LIKE ? ESCAPE '\\'".to_string());
                params.push(Box::new(format!("%{}%", Self::like_escape(term))));
            }
            candidates.push_str(&format!(
                " UNION SELECT MAX(c3.id) FROM commands c3 WHERE {} GROUP BY c3.command",
                compact.join(" AND ")
            ));
        }

        let mut order_parts = Vec::new();
        let mut order_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        order_parts.push("CASE WHEN c.command LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END".to_string());
        order_params.push(Box::new(format!("{}%", Self::like_escape(positive[0]))));
        if !cwd.is_empty() {
            order_parts.push("CASE WHEN c.cwd = ? THEN 0 ELSE 1 END".to_string());
            order_params.push(Box::new(cwd.to_string()));
        }
        if positive.len() > 1 {
            let sum = positive
                .iter()
                .map(|_| "INSTR(LOWER(c.command), ?)".to_string())
                .collect::<Vec<_>>()
                .join(" + ");
            order_parts.push(format!("({})", sum));
            for term in positive {
                order_params.push(Box::new(term.to_lowercase()));
            }
        }
        order_parts.push("LENGTH(c.command)".to_string());
        order_parts.push("c.id DESC".to_string());

        let sql = format!(
            "SELECT c.id, c.command, c.cwd, c.timestamp, c.exit_code, c.duration_ms, c.git_branch \
             FROM commands c WHERE c.id IN ( \
                 {} \
             ) ORDER BY {} LIMIT ?",
            candidates,
            order_parts.join(", ")
        );
        params.extend(order_params);
        params.push(Box::new(limit));
        let result = (|| -> rusqlite::Result<Vec<Vec<Value>>> {
            let mut stmt = conn.prepare(&sql)?;
            let iter = stmt.query_map(rusqlite::params_from_iter(params), Self::map_row_7)?;
            iter.collect()
        })();
        result.ok()
    }

    fn search_result(result: rusqlite::Result<Vec<Vec<Value>>>) -> Vec<Vec<Value>> {
        result.unwrap_or_else(|error| {
            crate::logging::Logger::log(
                "WARNING",
                "history",
                "history search failed",
                Some(&error.to_string()),
            );
            Vec::new()
        })
    }

    fn map_row_7(row: &rusqlite::Row) -> rusqlite::Result<Vec<Value>> {
        Ok(vec![
            Value::Number(serde_json::Number::from(row.get::<_, i64>(0)?)),
            Value::String(row.get::<_, String>(1)?),
            Value::String(row.get::<_, Option<String>>(2)?.unwrap_or_default()),
            Value::String(row.get::<_, String>(3)?),
            row.get::<_, Option<i64>>(4)?
                .map_or(Value::Null, |value| Value::Number(value.into())),
            row.get::<_, Option<i64>>(5)?
                .map_or(Value::Null, |value| Value::Number(value.into())),
            row.get::<_, Option<String>>(6)?
                .map_or(Value::Null, Value::String),
        ])
    }

    pub fn sql_search(&self, sql: &str) -> Result<Vec<Vec<Value>>, String> {
        let sql_stripped = sql.trim();
        if !is_read_only_sql(sql_stripped) {
            return Err("Only SELECT and EXPLAIN queries are supported".to_string());
        }
        let path = history_db_path();
        let conn = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| e.to_string())?;
        conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Select | AuthAction::Read { .. } | AuthAction::Function { .. } => {
                Authorization::Allow
            }
            _ => Authorization::Deny,
        }));
        let deadline = Instant::now() + Duration::from_secs(2);
        conn.progress_handler(10_000, Some(move || Instant::now() >= deadline));
        let mut stmt = conn.prepare(sql_stripped).map_err(|e| e.to_string())?;
        let col_count = stmt.column_count();
        let mut rows = Vec::new();
        {
            let mut query = stmt.query([]).map_err(|e| e.to_string())?;
            while let Some(row) = query.next().map_err(|e| e.to_string())? {
                let mut values = Vec::new();
                for i in 0..col_count {
                    let v = row
                        .get::<_, rusqlite::types::Value>(i)
                        .unwrap_or(rusqlite::types::Value::Null);
                    values.push(rusqlite_value_to_json(v));
                }
                rows.push(values);
                if rows.len() >= 1000 {
                    break;
                }
            }
        }
        Ok(rows)
    }

    /// Keep at most TRIM_MAX_ROWS commands. Runs on the worker's own
    /// connection, so it cannot race other writes or hit busy timeouts.
    fn trim(&self) {
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        // Cheap upper bound first: ids are monotonic, so if the id span fits
        // there is nothing to trim and no index scan is needed.
        let span: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(id) - MIN(id) + 1, 0) FROM commands",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if span <= TRIM_MAX_ROWS {
            return;
        }
        let cutoff: Option<i64> = conn
            .query_row(
                "SELECT id FROM commands ORDER BY id DESC LIMIT 1 OFFSET ?1",
                params![TRIM_MAX_ROWS],
                |r| r.get(0),
            )
            .ok();
        if let Some(cutoff) = cutoff {
            if let Err(error) = conn.execute("DELETE FROM commands WHERE id <= ?1", params![cutoff])
            {
                crate::logging::Logger::log(
                    "WARNING",
                    "history",
                    "could not trim history",
                    Some(&error.to_string()),
                );
            }
        }
    }

    /// Delete all history. Commands can contain secrets, so freed pages are
    /// zeroed, the FTS index is emptied and the WAL is truncated rather than
    /// leaving the old text recoverable from the database files.
    pub fn clear(&self) {
        let Ok(conn) = self.conn.lock() else {
            crate::logging::log_warning("history database lock is unavailable");
            return;
        };
        let has_fts: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'commands_fts')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        let mut sql = String::from("PRAGMA secure_delete = ON; DELETE FROM commands;");
        if has_fts {
            sql.push_str(" INSERT INTO commands_fts(commands_fts) VALUES ('delete-all');");
        }
        sql.push_str(" PRAGMA secure_delete = OFF; PRAGMA wal_checkpoint(TRUNCATE);");
        if let Err(error) = conn.execute_batch(&sql) {
            crate::logging::Logger::log(
                "WARNING",
                "history",
                "could not clear history",
                Some(&error.to_string()),
            );
        }
    }

    pub fn optimize(&self) -> Result<BTreeMap<String, Value>, String> {
        let db_path = history_db_path();
        let mut stats = BTreeMap::new();
        // Runs on the worker, on the same connection as every other history
        // operation, so maintenance cannot race inserts or block the UI.
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "history database lock is unavailable".to_string())?;
        let rows_before = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        let size_before = std::fs::metadata(&db_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        {
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            Self::remove_duplicates(&tx).map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())?;
        }
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|error| error.to_string())?;
        conn.execute_batch("ANALYZE; VACUUM;")
            .map_err(|error| error.to_string())?;
        if let Ok(mut inserts_since_trim) = self.inserts_since_trim.lock() {
            *inserts_since_trim = 0;
        }

        let rows_after = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        let size_after = std::fs::metadata(&db_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        stats.insert("rows_before".into(), json!(rows_before));
        stats.insert("rows_after".into(), json!(rows_after));
        stats.insert("duplicates_removed".into(), json!(rows_before - rows_after));
        stats.insert("size_before".into(), json!(size_before));
        stats.insert("size_after".into(), json!(size_after));
        Ok(stats)
    }

    fn remove_duplicates(conn: &rusqlite::Transaction<'_>) -> rusqlite::Result<usize> {
        conn.execute(
            "DELETE FROM commands
             WHERE id NOT IN (
                 SELECT MAX(id) FROM commands GROUP BY command, cwd
             )",
            [],
        )
    }
}

fn is_read_only_sql(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();
    upper.starts_with("SELECT") || upper.starts_with("EXPLAIN")
}

fn rusqlite_value_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => Value::String(format!("{:?}", b)),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_read_only_sql, HistoryManager};
    use rusqlite::Connection;
    use std::sync::Mutex;

    fn test_history() -> HistoryManager {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                cwd TEXT,
                exit_code INTEGER,
                timestamp TEXT NOT NULL,
                duration_ms INTEGER,
                git_branch TEXT
            );",
        )
        .unwrap();
        HistoryManager::initialize_fts(&conn).unwrap();
        HistoryManager {
            conn: Mutex::new(conn),
            inserts_since_trim: Mutex::new(0),
        }
    }

    #[test]
    fn sql_filter_accepts_only_select_and_explain() {
        assert!(is_read_only_sql("SELECT * FROM commands"));
        assert!(is_read_only_sql("EXPLAIN SELECT 1"));
        assert!(!is_read_only_sql("PRAGMA table_info(commands)"));
        assert!(!is_read_only_sql("DELETE FROM commands"));
    }

    #[test]
    fn search_matches_every_positive_term() {
        let history = test_history();
        history.add("ssh andres@example.test", "/workspace", 0);
        history.add("ssh other@example.test", "/workspace", 0);

        let commands: Vec<_> = history
            .search("ssh andres", 50, "/workspace")
            .into_iter()
            .map(|row| row[1].as_str().unwrap().to_string())
            .collect();

        assert_eq!(commands, ["ssh andres@example.test"]);
    }

    #[test]
    fn search_preserves_compact_single_term_matching() {
        let history = test_history();
        history.add("git status", "/workspace", 0);
        history.add("git stash", "/workspace", 0);

        let commands: Vec<_> = history
            .search("gitstatus", 50, "/workspace")
            .into_iter()
            .map(|row| row[1].as_str().unwrap().to_string())
            .collect();

        assert_eq!(commands, ["git status"]);
    }

    #[test]
    fn fts_index_tracks_history_deletes() {
        let history = test_history();
        history.add("kubectl get pods", "/workspace", 0);
        assert_eq!(history.search("kubectl", 50, "/workspace").len(), 1);

        history.clear();

        assert!(history.search("kubectl", 50, "/workspace").is_empty());
    }

    #[test]
    fn deduplicate_keeps_the_latest_command_per_directory() {
        let history = test_history();
        history.add("git status", "/workspace/a", 0);
        history.add("git status", "/workspace/b", 0);
        history.add("git status", "/workspace/a", 0);

        let mut conn = history.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        assert_eq!(HistoryManager::remove_duplicates(&tx).unwrap(), 1);
        tx.commit().unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    fn count(history: &HistoryManager) -> i64 {
        history
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn add_many_imports_in_one_transaction() {
        let history = test_history();
        let commands: Vec<String> = (0..500).map(|i| format!("echo {}", i)).collect();
        assert_eq!(history.add_many(&commands, "/w", 0), 500);
        assert_eq!(count(&history), 500);
    }

    #[test]
    fn clear_removes_commands_and_fts_rows() {
        let history = test_history();
        history.add("export TOKEN=secret", "/w", 0);
        history.clear();
        assert_eq!(count(&history), 0);
        assert!(history.search("secret", 10, "/w").is_empty());
    }

    #[test]
    fn latest_failed_ignores_successes_and_other_directories() {
        let history = test_history();
        let failed = history.add("make", "/w", 2);
        history.add("ls", "/w", 0);
        history.add("cargo build", "/other", 1);
        let row = history.latest_failed("/w").unwrap();
        assert_eq!(row[0].as_i64(), Some(failed));
        assert_eq!(row[1].as_str(), Some("make"));
    }
}
