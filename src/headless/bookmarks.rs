//! Saved folders for the two-pane file browser.
//!
//! Local bookmarks are available in every browser; remote ones are tied to a
//! saved connection id (or to an SSH alias), because `/var/log` means different
//! things on different hosts.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{paths, persistence};

pub const CURRENT_VERSION: u64 = 1;
pub const MAX_BOOKMARKS: usize = 200;
pub const MAX_NAME: usize = 100;
pub const MAX_PATH: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Local => "local",
            Side::Remote => "remote",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: String,
    pub side: Side,
    /// `Some` for remote bookmarks; `None` for local ones.
    pub connection: Option<String>,
}

pub fn bookmark_path() -> PathBuf {
    paths::config_dir().join("file_bookmarks.json")
}

/// A missing file is an empty list, not an error.
pub fn load(path: &Path) -> Result<Vec<Bookmark>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
    };
    let value: Value =
        serde_json::from_str(&raw).map_err(|error| format!("invalid JSON: {}", error))?;
    let object = value
        .as_object()
        .ok_or("the bookmark file must contain a JSON object")?;
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .ok_or("the bookmark file has no \"version\"")?;
    if version > CURRENT_VERSION {
        return Err(format!(
            "the bookmark file was written by a newer Oxterm (version {})",
            version
        ));
    }
    let mut bookmarks = Vec::new();
    for row in object
        .get("bookmarks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("a bookmark has no name")?;
        let path = row
            .get("path")
            .and_then(Value::as_str)
            .ok_or("a bookmark has no path")?;
        let side = match row.get("side").and_then(Value::as_str) {
            Some("local") => Side::Local,
            Some("remote") => Side::Remote,
            _ => return Err("a bookmark has an invalid side".to_string()),
        };
        validate(name, path)?;
        bookmarks.push(Bookmark {
            name: name.to_string(),
            path: path.to_string(),
            side,
            connection: row
                .get("connection")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        });
        if bookmarks.len() > MAX_BOOKMARKS {
            return Err(format!("too many bookmarks (limit {})", MAX_BOOKMARKS));
        }
    }
    Ok(bookmarks)
}

pub fn save(path: &Path, bookmarks: &[Bookmark]) -> Result<(), String> {
    let rows: Vec<Value> = bookmarks
        .iter()
        .map(|bookmark| {
            json!({
                "name": bookmark.name,
                "path": bookmark.path,
                "side": bookmark.side.as_str(),
                "connection": bookmark.connection,
            })
        })
        .collect();
    let body = serde_json::to_vec_pretty(&json!({
        "version": CURRENT_VERSION,
        "bookmarks": rows,
    }))
    .map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {}", parent.display(), error))?;
    }
    persistence::atomic_write(path, &body)
        .map_err(|error| format!("cannot write {}: {}", path.display(), error))
}

/// Add or rename a bookmark for the same side, connection and path.
pub fn add(bookmarks: &mut Vec<Bookmark>, bookmark: Bookmark) -> Result<(), String> {
    validate(&bookmark.name, &bookmark.path)?;
    if let Some(existing) = bookmarks.iter_mut().find(|existing| {
        existing.side == bookmark.side
            && existing.connection == bookmark.connection
            && existing.path == bookmark.path
    }) {
        existing.name = bookmark.name;
        return Ok(());
    }
    if bookmarks.len() >= MAX_BOOKMARKS {
        return Err(format!("too many bookmarks (limit {})", MAX_BOOKMARKS));
    }
    bookmarks.push(bookmark);
    Ok(())
}

/// Remove the bookmark for a folder. Returns whether one was found.
pub fn remove(
    bookmarks: &mut Vec<Bookmark>,
    side: Side,
    connection: Option<&str>,
    path: &str,
) -> bool {
    let before = bookmarks.len();
    bookmarks.retain(|bookmark| {
        !(bookmark.side == side
            && bookmark.connection.as_deref() == connection
            && bookmark.path == path)
    });
    bookmarks.len() != before
}

/// Bookmarks that belong in this browser, in file order.
pub fn visible<'a>(
    bookmarks: &'a [Bookmark],
    side: Side,
    connection: Option<&'a str>,
) -> Vec<&'a Bookmark> {
    bookmarks
        .iter()
        .filter(|bookmark| {
            bookmark.side == side
                && match side {
                    Side::Local => true,
                    Side::Remote => bookmark.connection.as_deref() == connection,
                }
        })
        .collect()
}

fn validate(name: &str, path: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a bookmark needs a name".to_string());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!(
            "the bookmark name is longer than {} characters",
            MAX_NAME
        ));
    }
    if path.trim().is_empty() {
        return Err("a bookmark needs a path".to_string());
    }
    if path.len() > MAX_PATH {
        return Err(format!(
            "the bookmark path is longer than {} bytes",
            MAX_PATH
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmark(name: &str, path: &str, side: Side, connection: Option<&str>) -> Bookmark {
        Bookmark {
            name: name.to_string(),
            path: path.to_string(),
            side,
            connection: connection.map(str::to_string),
        }
    }

    #[test]
    fn adds_updates_and_removes() {
        let mut list = Vec::new();
        add(
            &mut list,
            bookmark("Logs", "/var/log", Side::Remote, Some("db")),
        )
        .unwrap();
        add(
            &mut list,
            bookmark("Logs again", "/var/log", Side::Remote, Some("db")),
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Logs again");
        assert!(remove(&mut list, Side::Remote, Some("db"), "/var/log"));
        assert!(!remove(&mut list, Side::Remote, Some("db"), "/var/log"));
    }

    #[test]
    fn filters_by_side_and_connection() {
        let list = vec![
            bookmark("src", "/src", Side::Local, None),
            bookmark("logs", "/var/log", Side::Remote, Some("db")),
            bookmark("web", "/var/www", Side::Remote, Some("web")),
        ];
        assert_eq!(visible(&list, Side::Local, Some("db")).len(), 1);
        assert_eq!(visible(&list, Side::Remote, Some("db")).len(), 1);
        assert_eq!(visible(&list, Side::Remote, Some("db"))[0].name, "logs");
        assert!(visible(&list, Side::Remote, None).is_empty());
    }

    #[test]
    fn round_trips_and_rejects_bad_files() {
        let path = std::env::temp_dir().join(format!(
            "oxterm-bookmarks-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let list = vec![
            bookmark("src", "/src", Side::Local, None),
            bookmark("logs", "/var/log", Side::Remote, Some("db")),
        ];
        save(&path, &list).unwrap();
        assert_eq!(load(&path).unwrap(), list);
        let _ = std::fs::remove_file(&path);

        std::fs::write(&path, r#"{"version":99,"bookmarks":[]}"#).unwrap();
        assert!(load(&path).is_err());
        std::fs::write(
            &path,
            r#"{"version":1,"bookmarks":[{"name":"","path":"/","side":"local"}]}"#,
        )
        .unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
