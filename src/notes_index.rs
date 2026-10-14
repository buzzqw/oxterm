//! Full-text index for Markdown notes, backed by SQLite + FTS5.
//!
//! The index mirrors the approach used for command history: a small SQLite
//! database under the config directory with an external-content FTS5 table and
//! triggers keeping it in sync. Every operation runs on a worker thread and
//! reports back on the GTK main loop.

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::notes;
use crate::settings;

pub fn notes_db_path() -> PathBuf {
    settings::config_dir().join("notes.db")
}

/// One indexed note entry (a `## timestamp` section).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteEntry {
    pub note: String,
    pub timestamp: String,
    pub body: String,
    pub tags: String,
}

impl NoteEntry {
    pub fn snippet(&self) -> String {
        let text = self.body.trim();
        let first = text
            .lines()
            .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("<!--"))
            .unwrap_or("")
            .trim();
        let mut snippet = first.to_string();
        if snippet.chars().count() > 120 {
            snippet = snippet.chars().take(117).collect::<String>() + "...";
        }
        snippet
    }

    pub fn tag_list(&self) -> Vec<String> {
        self.tags
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }
}

/// Rebuild the whole index from the notes directory. Returns the number of
/// indexed entries.
pub fn refresh_async<F>(done: F)
where
    F: FnOnce(Result<usize, String>) + 'static,
{
    let done = crate::Sendable(done);
    std::thread::spawn(move || {
        let result = refresh();
        crate::run_on_main(move || {
            let done = done;
            (done.0)(result)
        });
    });
}

fn refresh() -> Result<usize, String> {
    let dir = notes::resolve_notes_dir();
    let mut entries: Vec<NoteEntry> = Vec::new();
    if let Ok(read) = std::fs::read_dir(&dir) {
        for file in read.flatten() {
            let path = file.path();
            let is_md = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("md"))
                .unwrap_or(false);
            if !is_md || !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            entries.extend(parse_entries(name, &content));
        }
    }

    let conn = open()?;
    conn.execute("DELETE FROM note_entries", [])
        .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO note_entries (note, timestamp, body, tags) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| e.to_string())?;
        for entry in &entries {
            stmt.execute(params![entry.note, entry.timestamp, entry.body, entry.tags])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(entries.len())
}

/// Full-text search across all notes. `tag`, when non-empty, restricts results
/// to entries carrying that tag.
pub fn search_async<F>(query: String, tag: String, limit: i64, done: F)
where
    F: FnOnce(Result<Vec<NoteEntry>, String>) + 'static,
{
    let done = crate::Sendable(done);
    std::thread::spawn(move || {
        let result = search(&query, &tag, limit);
        crate::run_on_main(move || {
            let done = done;
            (done.0)(result)
        });
    });
}

fn search(query: &str, tag: &str, limit: i64) -> Result<Vec<NoteEntry>, String> {
    let conn = open()?;
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_string()).collect();
    let tag_filter = tag.trim();
    let mut rows: Vec<NoteEntry> = Vec::new();

    if !terms.is_empty() {
        // Trigram FTS supports substring matches; quote each term to avoid FTS
        // syntax errors from punctuation.
        let fts: String = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        rows = run_fts(&conn, &fts, tag_filter, limit).unwrap_or_default();
    }
    if rows.is_empty() {
        rows = run_like(&conn, &terms, tag_filter, limit)?;
    }
    Ok(rows)
}

fn run_fts(conn: &Connection, fts: &str, tag: &str, limit: i64) -> Result<Vec<NoteEntry>, String> {
    let tag_clause = if tag.is_empty() {
        "".to_string()
    } else {
        format!("AND e.tags LIKE '% {} %'", escape_like(tag))
    };
    let sql = format!(
        "SELECT e.note, e.timestamp, e.body, e.tags \
         FROM note_entries e JOIN note_entries_fts f ON f.rowid = e.id \
         WHERE f.note_entries_fts MATCH ?1 {tag_clause} \
         ORDER BY e.id DESC LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map(params![fts, limit], map_entry)
        .map_err(|e| e.to_string())?;
    iter.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn run_like(
    conn: &Connection,
    terms: &[String],
    tag: &str,
    limit: i64,
) -> Result<Vec<NoteEntry>, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for term in terms {
        where_parts.push("e.body LIKE ? ESCAPE '\\'".to_string());
        args.push(Box::new(format!("%{}%", escape_like(term))));
    }
    if !tag.is_empty() {
        where_parts.push("e.tags LIKE ? ESCAPE '\\'".to_string());
        args.push(Box::new(format!("% {} %", escape_like(tag))));
    }
    let where_sql = if where_parts.is_empty() {
        "1".to_string()
    } else {
        where_parts.join(" AND ")
    };
    let sql = format!(
        "SELECT e.note, e.timestamp, e.body, e.tags FROM note_entries e \
         WHERE {where_sql} ORDER BY e.id DESC LIMIT ?"
    );
    args.push(Box::new(limit));
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map(rusqlite::params_from_iter(args), map_entry)
        .map_err(|e| e.to_string())?;
    iter.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

/// List the distinct tags present in the index.
pub fn tags_async<F>(done: F)
where
    F: FnOnce(Result<Vec<String>, String>) + 'static,
{
    let done = crate::Sendable(done);
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<String>, String> {
            let conn = open()?;
            let mut stmt = conn
                .prepare("SELECT tags FROM note_entries")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            let mut tags = std::collections::BTreeSet::new();
            for value in rows.flatten() {
                for tag in value.split_whitespace() {
                    tags.insert(tag.to_string());
                }
            }
            Ok(tags.into_iter().collect())
        })();
        crate::run_on_main(move || {
            let done = done;
            (done.0)(result)
        });
    });
}

fn map_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteEntry> {
    Ok(NoteEntry {
        note: row.get(0)?,
        timestamp: row.get(1)?,
        body: row.get(2)?,
        tags: row.get(3)?,
    })
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn open() -> Result<Connection, String> {
    let dir = settings::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let conn = Connection::open(notes_db_path()).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS note_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            note TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            body TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_note_entries_note ON note_entries(note);",
    )
    .map_err(|e| e.to_string())?;

    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'note_entries_fts')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE note_entries_fts USING fts5(
                body, note, tags,
                content='note_entries',
                content_rowid='id',
                tokenize='trigram'
            );
            INSERT INTO note_entries_fts(note_entries_fts) VALUES ('rebuild');",
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS note_entries_ai AFTER INSERT ON note_entries BEGIN
             INSERT INTO note_entries_fts(rowid, body, note, tags)
             VALUES (new.id, new.body, new.note, new.tags);
         END;
         CREATE TRIGGER IF NOT EXISTS note_entries_ad AFTER DELETE ON note_entries BEGIN
             INSERT INTO note_entries_fts(note_entries_fts, rowid, body, note, tags)
             VALUES ('delete', old.id, old.body, old.note, old.tags);
         END;
         CREATE TRIGGER IF NOT EXISTS note_entries_au AFTER UPDATE ON note_entries BEGIN
             INSERT INTO note_entries_fts(note_entries_fts, rowid, body, note, tags)
             VALUES ('delete', old.id, old.body, old.note, old.tags);
             INSERT INTO note_entries_fts(rowid, body, note, tags)
             VALUES (new.id, new.body, new.note, new.tags);
         END;",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Split the content of a note into entries and extract tags.
fn parse_entries(note: &str, content: &str) -> Vec<NoteEntry> {
    let tag_re = regex::Regex::new(r"(?:^|\s)#([A-Za-z0-9][A-Za-z0-9_\-/]*)").unwrap();
    let mut entries = Vec::new();
    let mut timestamp: Option<String> = None;
    let mut body = String::new();

    let mut flush = |timestamp: &mut Option<String>, body: &mut String| {
        if let Some(ts) = timestamp.take() {
            let tags: std::collections::BTreeSet<String> = tag_re
                .captures_iter(body)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect();
            let joined = if tags.is_empty() {
                String::new()
            } else {
                format!(" {} ", tags.into_iter().collect::<Vec<_>>().join(" "))
            };
            entries.push(NoteEntry {
                note: note.to_string(),
                timestamp: ts,
                body: body.trim_end().to_string(),
                tags: joined,
            });
        }
        body.clear();
    };

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            flush(&mut timestamp, &mut body);
            timestamp = Some(rest.trim().to_string());
        } else if timestamp.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&mut timestamp, &mut body);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_tags() {
        let content = "# Oxterm Notes\n\n\
            ## 2026-10-14 09:00:00\n\n\
            fixed the build #ci #rust\n\n\
            ## 2026-10-14 10:00:00\n\n\
            <!-- source: selection | cwd: /tmp -->\n\
            note without tags\n";
        let entries = parse_entries("notes.md", content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp, "2026-10-14 09:00:00");
        assert_eq!(entries[0].tag_list(), vec!["ci", "rust"]);
        assert!(entries[1].tags.trim().is_empty());
        assert!(entries[1].body.contains("note without tags"));
    }

    #[test]
    fn snippet_skips_context_comment() {
        let entry = NoteEntry {
            note: "n.md".into(),
            timestamp: "t".into(),
            body: "<!-- source: selection -->\nreal content here".into(),
            tags: String::new(),
        };
        assert_eq!(entry.snippet(), "real content here");
    }

    #[test]
    fn indexes_and_searches() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO note_entries (note, timestamp, body, tags) VALUES (?1, ?2, ?3, ?4)",
            params!["a.md", "t1", "deploy staging server #ops", " ops "],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO note_entries (note, timestamp, body, tags) VALUES (?1, ?2, ?3, ?4)",
            params!["b.md", "t2", "unrelated text", ""],
        )
        .unwrap();

        let all = run_fts(&conn, "\"staging\"", "", 10).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].note, "a.md");

        let tagged = run_fts(&conn, "\"deploy\"", "ops", 10).unwrap();
        assert_eq!(tagged.len(), 1);

        let excluded = run_fts(&conn, "\"deploy\"", "missing", 10).unwrap();
        assert!(excluded.is_empty());

        // LIKE fallback finds the same row.
        let like = run_like(&conn, &["staging".to_string()], "", 10).unwrap();
        assert_eq!(like.len(), 1);
    }
}
