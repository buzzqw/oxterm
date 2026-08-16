#![allow(dead_code)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

// use crate::logging::LOGGER;
use crate::settings;

pub fn history_db_path() -> PathBuf {
    settings::config_dir().join("history.db")
}

const TRIM_CHECK_INTERVAL: u32 = 50;
const TRIM_MAX_ROWS: i64 = 1_000_000;

pub struct HistoryManager {
    conn: Mutex<Connection>,
    inserts_since_trim: Mutex<u32>,
}

static HISTORY: OnceLock<HistoryManager> = OnceLock::new();

pub fn history() -> &'static HistoryManager {
    HISTORY.get_or_init(|| HistoryManager::new())
}

impl HistoryManager {
    fn new() -> HistoryManager {
        let dir = settings::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let db_path = history_db_path();
        let conn = Connection::open(&db_path).expect("open history db");
        let _ = std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600));
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                cwd TEXT,
                exit_code INTEGER,
                timestamp TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_commands_ts ON commands(timestamp);",
        )
        .expect("create history tables");
        HistoryManager {
            conn: Mutex::new(conn),
            inserts_since_trim: Mutex::new(0),
        }
    }

    fn now_ts() -> String {
        crate::logging::utc_iso_now()
    }

    fn bump_inserts(&self, n: usize) {
        let mut c = self.inserts_since_trim.lock().unwrap();
        *c += n as u32;
        if *c >= TRIM_CHECK_INTERVAL {
            *c = 0;
            self.trim();
        }
    }

    pub fn add(&self, command: &str, cwd: &str, exit_code: i64) -> i64 {
        let conn = self.conn.lock().unwrap();
        let ts = Self::now_ts();
        let id = conn
            .execute(
                "INSERT INTO commands (command, cwd, exit_code, timestamp) VALUES (?1,?2,?3,?4)",
                params![command, cwd, exit_code, ts],
            )
            .map(|_| conn.last_insert_rowid())
            .unwrap_or(0);
        drop(conn);
        self.bump_inserts(1);
        id
    }

    pub fn add_many(&self, commands: &[String], cwd: &str, exit_code: i64) -> usize {
        if commands.is_empty() {
            return 0;
        }
        let conn = self.conn.lock().unwrap();
        let mut n = 0usize;
        {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO commands (command, cwd, exit_code, timestamp) VALUES (?1,?2,?3,?4)",
                )
                .unwrap();
            let ts = Self::now_ts();
            for cmd in commands {
                if stmt.execute(params![cmd, cwd, exit_code, ts]).is_ok() {
                    n += 1;
                }
            }
        }
        drop(conn);
        self.bump_inserts(n);
        n
    }

    pub fn set_exit_code(&self, row_id: Option<i64>, exit_code: i64) {
        if let Some(id) = row_id {
            let conn = self.conn.lock().unwrap();
            let _ = conn.execute(
                "UPDATE commands SET exit_code = ?1 WHERE id = ?2",
                params![exit_code, id],
            );
        }
    }

    fn like_escape(term: &str) -> String {
        term.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    }

    pub fn search(&self, terms: &str, limit: i64, cwd: &str) -> Vec<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        if terms.trim().is_empty() {
            let rows = if !cwd.is_empty() {
                let mut stmt = conn
                    .prepare(
                        "SELECT MAX(id) AS id, command, cwd, timestamp FROM commands \
                         WHERE command NOT LIKE '/%' ESCAPE '\\' GROUP BY command \
                         ORDER BY CASE WHEN cwd = ?1 THEN 0 ELSE 1 END, id DESC LIMIT ?2",
                    )
                    .unwrap();
                let iter = stmt
                    .query_map(params![cwd, limit], Self::map_row_4)
                    .unwrap();
                iter.filter_map(|r| r.ok()).collect::<Vec<_>>()
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT MAX(id) AS id, command, cwd, timestamp FROM commands \
                         WHERE command NOT LIKE '/%' ESCAPE '\\' GROUP BY command \
                         ORDER BY id DESC LIMIT ?1",
                    )
                    .unwrap();
                let iter = stmt.query_map(params![limit], Self::map_row_4).unwrap();
                iter.filter_map(|r| r.ok()).collect::<Vec<_>>()
            };
            return rows;
        }

        let parts: Vec<&str> = terms.trim().split_whitespace().collect();
        let mut pos: Vec<&str> = Vec::new();
        let mut neg: Vec<&str> = Vec::new();
        for p in &parts {
            if p.starts_with('-') && p.len() > 1 {
                neg.push(&p[1..]);
            } else {
                pos.push(p);
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
            "SELECT MAX(id) AS id, command, cwd, timestamp FROM commands \
             WHERE {} GROUP BY command ORDER BY {} LIMIT ?",
            where_sql, order_sql
        );

        let mut stmt = conn.prepare(&sql).unwrap();
        let mut all: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        all.extend(where_params.into_iter().map(|b| b));
        all.extend(order_params.into_iter().map(|b| b));
        all.push(Box::new(limit));
        let iter = stmt
            .query_map(rusqlite::params_from_iter(all.into_iter()), |row| {
                Self::map_row_4(row)
            })
            .unwrap();
        iter.filter_map(|r| r.ok()).collect()
    }

    fn map_row_4(row: &rusqlite::Row) -> rusqlite::Result<Vec<Value>> {
        Ok(vec![
            Value::Number(serde_json::Number::from(row.get::<_, i64>(0)?)),
            Value::String(row.get::<_, String>(1)?),
            Value::String(row.get::<_, Option<String>>(2)?.unwrap_or_default()),
            Value::String(row.get::<_, String>(3)?),
        ])
    }

    fn map_row_1(row: &rusqlite::Row) -> rusqlite::Result<Value> {
        Ok(Value::String(row.get::<_, String>(0)?))
    }

    pub fn sql_search(&self, sql: &str) -> Result<Vec<Vec<Value>>, String> {
        let sql_stripped = sql.trim();
        let upper = sql_stripped.to_uppercase();
        if !(upper.starts_with("SELECT") || upper.starts_with("EXPLAIN")) {
            return Err("Only SELECT and EXPLAIN queries are supported".to_string());
        }
        let path = history_db_path();
        let conn = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(sql_stripped).map_err(|e| e.to_string())?;
        let col_count = stmt.column_count();
        let mut rows = Vec::new();
        {
            let mut query = stmt.query([]).map_err(|e| e.to_string())?;
            while let Ok(Some(row)) = query.next() {
                let mut values = Vec::new();
                for i in 0..col_count {
                    let v = row
                        .get::<_, rusqlite::types::Value>(i)
                        .unwrap_or(rusqlite::types::Value::Null);
                    values.push(rusqlite_value_to_json(v));
                }
                rows.push(values);
                if rows.len() > 1000 {
                    break;
                }
            }
        }
        Ok(rows)
    }

    pub fn interactive_search(&self, query: &str, limit: i64) -> Vec<Value> {
        let conn = self.conn.lock().unwrap();
        if query.trim().is_empty() {
            let mut stmt = conn
                .prepare("SELECT DISTINCT command FROM commands ORDER BY id DESC LIMIT ?1")
                .unwrap();
            let iter = stmt.query_map(params![limit], Self::map_row_1).unwrap();
            return iter.filter_map(|r| r.ok()).collect();
        }
        let parts: Vec<&str> = query.trim().split_whitespace().collect();
        let where_sql = parts
            .iter()
            .map(|_| "command LIKE ? ESCAPE '\\'")
            .collect::<Vec<_>>()
            .join(" AND ");
        let where_params: Vec<String> = parts
            .iter()
            .map(|p| format!("%{}%", Self::like_escape(p)))
            .collect();

        let first_term = parts[0];
        let mut order_parts = vec![
            "CASE WHEN command LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END".to_string(),
        ];
        let mut order_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(format!(
            "{}%",
            Self::like_escape(first_term)
        ))];
        if parts.len() > 1 {
            let sum = parts
                .iter()
                .map(|_| "INSTR(LOWER(command), ?)".to_string())
                .collect::<Vec<_>>()
                .join(" + ");
            order_parts.push(format!("({})", sum));
            for p in &parts {
                order_params.push(Box::new(p.to_lowercase()));
            }
        }
        order_parts.push("LENGTH(command)".to_string());
        order_parts.push("max_id DESC".to_string());
        let order_sql = order_parts.join(", ");

        let sql = format!(
            "SELECT command FROM (
                SELECT command, MAX(id) as max_id FROM commands WHERE {}
                GROUP BY command LIMIT ?)
             ORDER BY {} LIMIT ?",
            where_sql, order_sql
        );

        let mut stmt = conn.prepare(&sql).unwrap();
        let mut all: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        for p in where_params {
            all.push(Box::new(p));
        }
        all.push(Box::new(limit));
        all.extend(order_params.into_iter().map(|b| b));
        all.push(Box::new(limit));
        let iter = stmt
            .query_map(rusqlite::params_from_iter(all.into_iter()), |row| {
                Self::map_row_1(row)
            })
            .unwrap();
        iter.filter_map(|r| r.ok()).collect()
    }

    fn trim(&self) {
        let db_path = history_db_path();
        std::thread::spawn(move || {
            if let Ok(conn) = Connection::open(&db_path) {
                if let Ok(count) = conn.query_row("SELECT COUNT(*) FROM commands", [], |r| {
                    r.get::<_, i64>(0)
                }) {
                    if count > TRIM_MAX_ROWS {
                        let _ = conn.execute(
                            "DELETE FROM commands WHERE id NOT IN \
                             (SELECT id FROM commands ORDER BY id DESC LIMIT ?1)",
                            params![TRIM_MAX_ROWS],
                        );
                    }
                }
            }
        });
    }

    pub fn search_latest(&self, prefix: &str, limit: i64) -> Vec<Value> {
        let conn = self.conn.lock().unwrap();
        if !prefix.is_empty() {
            let mut stmt = conn
                .prepare(
                    "SELECT command FROM commands WHERE command LIKE ?1 \
                     GROUP BY command ORDER BY MAX(id) DESC LIMIT ?2",
                )
                .unwrap();
            let iter = stmt
                .query_map(params![format!("{}%", prefix), limit], Self::map_row_1)
                .unwrap();
            iter.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT command FROM commands GROUP BY command ORDER BY MAX(id) DESC LIMIT ?1",
                )
                .unwrap();
            let iter = stmt.query_map(params![limit], Self::map_row_1).unwrap();
            iter.filter_map(|r| r.ok()).collect()
        }
    }

    pub fn get_all(&self, limit: i64) -> Vec<Value> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT command FROM commands ORDER BY id DESC LIMIT ?1")
            .unwrap();
        let iter = stmt.query_map(params![limit], Self::map_row_1).unwrap();
        iter.filter_map(|r| r.ok()).collect()
    }

    pub fn clear(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM commands", []);
    }

    pub fn optimize(&self) -> BTreeMap<String, Value> {
        let db_path = history_db_path();
        let mut stats = BTreeMap::new();
        let rows_before = {
            let conn = self.conn.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap_or(0)
        };
        let size_before = std::fs::metadata(&db_path).map(|m| m.len() as i64).unwrap_or(0);

        if let Ok(conn) = Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        ) {
            let _ = conn.pragma_update(None, "wal_checkpoint(TRUNCATE)", rusqlite::types::Value::Null);
            let _ = conn.execute_batch("ANALYZE; VACUUM;");
        }
        let mut g = self.inserts_since_trim.lock().unwrap();
        *g = 0;

        let rows_after = {
            let conn = self.conn.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap_or(0)
        };
        let size_after = std::fs::metadata(&db_path).map(|m| m.len() as i64).unwrap_or(0);

        stats.insert("rows_before".into(), json!(rows_before));
        stats.insert("rows_after".into(), json!(rows_after));
        stats.insert("duplicates_removed".into(), json!(rows_before - rows_after));
        stats.insert("size_before".into(), json!(size_before));
        stats.insert("size_after".into(), json!(size_after));
        stats
    }
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
