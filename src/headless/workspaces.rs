//! Saved workspaces: named sets of connections and local directories.
//!
//! The file is plain JSON under the private Oxterm config directory. As with
//! the connection book, a missing file is an empty workbook while a damaged one
//! is an error, so no save can silently replace data Oxterm could not read.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Newest `workspaces.json` format this build can rewrite.
pub const CURRENT_VERSION: u32 = 1;
const MAX_NAME_CHARS: usize = 64;
const MAX_WORKSPACES: usize = 32;
const MAX_ENTRIES: usize = 200;

/// One tab in a workspace. `kind` is `connection` or `local`; `target` is a
/// connection name or id for the former and a working directory for the latter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceEntry {
    pub label: String,
    pub kind: String,
    pub target: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub entries: Vec<WorkspaceEntry>,
    /// Seconds since the Unix epoch, for display and conflict resolution.
    pub saved_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkBook {
    pub version: u32,
    pub workspaces: Vec<Workspace>,
}

impl Default for WorkBook {
    fn default() -> WorkBook {
        WorkBook {
            version: CURRENT_VERSION,
            workspaces: Vec::new(),
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// Validate a workspace name the same way persisted names are validated
/// elsewhere: no path separators, no control characters and a short length.
fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let length = name.chars().count();
    if length == 0 {
        return Err("The workspace name cannot be empty".to_string());
    }
    if length > MAX_NAME_CHARS {
        return Err(format!(
            "The workspace name cannot be longer than {} characters",
            MAX_NAME_CHARS
        ));
    }
    if name.chars().any(|c| c == '/' || c.is_control()) {
        return Err("The workspace name contains invalid characters".to_string());
    }
    Ok(name.to_string())
}

/// Insert or replace a workspace. Names are unique case-insensitively, so
/// `Work` and `work` cannot both be saved.
pub fn upsert(book: &mut WorkBook, name: &str, entries: Vec<WorkspaceEntry>) -> Result<(), String> {
    let name = validate_name(name)?;
    if entries.len() > MAX_ENTRIES {
        return Err(format!(
            "A workspace cannot have more than {} entries",
            MAX_ENTRIES
        ));
    }
    if let Some(slot) = book
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.name == name)
    {
        *slot = Workspace {
            name,
            entries,
            saved_at: now_secs(),
        };
        return Ok(());
    }
    if let Some(other) = book
        .workspaces
        .iter()
        .find(|workspace| same_name(&workspace.name, &name))
    {
        return Err(format!("A workspace named '{}' already exists", other.name));
    }
    if book.workspaces.len() >= MAX_WORKSPACES {
        return Err(format!(
            "The workspace book is full ({} workspaces)",
            MAX_WORKSPACES
        ));
    }
    book.workspaces.push(Workspace {
        name,
        entries,
        saved_at: now_secs(),
    });
    Ok(())
}

/// Remove a workspace by name (case-insensitively). `true` if one was removed.
pub fn remove(book: &mut WorkBook, name: &str) -> bool {
    let Some(index) = book
        .workspaces
        .iter()
        .position(|workspace| same_name(&workspace.name, name))
    else {
        return false;
    };
    book.workspaces.remove(index);
    true
}

/// Look up a workspace by name, case-insensitively.
pub fn find<'a>(book: &'a WorkBook, name: &str) -> Option<&'a Workspace> {
    book.workspaces
        .iter()
        .find(|workspace| same_name(&workspace.name, name))
}

/// `~/.config/oxterm/workspaces.json`. `None` if the home directory is unknown.
pub fn default_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("oxterm").join("workspaces.json"))
}

/// Read the workbook. A missing file yields an empty one; a corrupt file is an
/// error and is left untouched on disk.
pub fn load(path: &Path) -> Result<WorkBook, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkBook::default());
        }
        Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
    };
    serde_json::from_str(&contents).map_err(|error| format!("{}: {}", path.display(), error))
}

/// Write the workbook atomically with mode 0600, creating a private (0700)
/// parent directory when needed.
pub fn save(path: &Path, book: &WorkBook) -> Result<(), String> {
    ensure_parent(path)?;
    let mut contents = serde_json::to_string_pretty(book).map_err(|error| error.to_string())?;
    contents.push('\n');
    crate::headless::persistence::atomic_write(path, contents.as_bytes())
        .map_err(|error| format!("cannot write {}: {}", path.display(), error))
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
    {
        Ok(()) => Ok(()),
        Err(error) => Err(format!("cannot create {}: {}", parent.display(), error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique file under /tmp/opencode, removed even if a test fails.
    struct TestPath(PathBuf);

    impl TestPath {
        fn new(label: &str) -> TestPath {
            let dir = Path::new("/tmp/opencode");
            fs::create_dir_all(dir).expect("create /tmp/opencode");
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            TestPath(dir.join(format!(
                "oxterm-workspaces-{}-{}-{}.json",
                std::process::id(),
                n,
                label
            )))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn entry(label: &str, kind: &str, target: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            label: label.to_string(),
            kind: kind.to_string(),
            target: target.to_string(),
        }
    }

    #[test]
    fn save_and_load_round_trip_with_private_permissions() {
        let file = TestPath::new("roundtrip");
        let mut book = WorkBook::default();
        assert_eq!(book.version, CURRENT_VERSION);
        upsert(
            &mut book,
            "Deploy",
            vec![
                entry("prod", "connection", "prod-web"),
                entry("logs", "local", "/tmp/opencode"),
            ],
        )
        .unwrap();

        save(file.path(), &book).unwrap();
        let loaded = load(file.path()).unwrap();
        assert_eq!(loaded, book);
        assert_eq!(find(&loaded, "DEPLOY").unwrap().entries.len(), 2);

        let mode = fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn missing_file_is_an_empty_book() {
        let file = TestPath::new("missing");
        let _ = fs::remove_file(file.path());
        let book = load(file.path()).unwrap();
        assert!(book.workspaces.is_empty());
        assert_eq!(book.version, CURRENT_VERSION);
    }

    #[test]
    fn upsert_validates_names_duplicates_and_limits() {
        let mut book = WorkBook::default();
        assert!(upsert(&mut book, "", vec![]).is_err());
        assert!(upsert(&mut book, "   ", vec![]).is_err());
        assert!(upsert(&mut book, "a/b", vec![]).is_err());
        assert!(upsert(&mut book, "a\nb", vec![]).is_err());
        assert!(upsert(&mut book, &"x".repeat(65), vec![]).is_err());
        assert!(upsert(&mut book, &"x".repeat(64), vec![]).is_ok());
        assert_eq!(book.workspaces.len(), 1);

        assert!(upsert(&mut book, "Work", vec![]).is_ok());
        let error = upsert(&mut book, "work", vec![]).unwrap_err();
        assert!(
            error.to_lowercase().contains("already") || error.to_lowercase().contains("exists")
        );
        assert_eq!(book.workspaces.len(), 2);

        let many = (0..=MAX_ENTRIES)
            .map(|n| entry(&format!("e{}", n), "local", "/tmp"))
            .collect();
        assert!(upsert(&mut book, "Too many", many).is_err());
        assert_eq!(find(&book, "Too many"), None);
    }

    #[test]
    fn upsert_enforces_the_workspace_limit() {
        let mut book = WorkBook::default();
        for n in 0..MAX_WORKSPACES {
            upsert(&mut book, &format!("w{}", n), vec![]).unwrap();
        }
        assert_eq!(book.workspaces.len(), MAX_WORKSPACES);
        assert!(upsert(&mut book, "one too many", vec![]).is_err());
        // A full book may still replace one of its workspaces by its exact name.
        assert!(upsert(&mut book, "w0", vec![entry("x", "local", "/tmp")]).is_ok());
        assert_eq!(book.workspaces.len(), MAX_WORKSPACES);
    }

    #[test]
    fn upsert_replace_and_remove_are_case_insensitive() {
        let mut book = WorkBook::default();
        upsert(&mut book, "Release", vec![entry("one", "local", "/tmp")]).unwrap();
        assert!(upsert(&mut book, "release", vec![]).is_err());
        upsert(
            &mut book,
            "Release",
            vec![entry("two", "local", "/tmp/opencode")],
        )
        .unwrap();
        assert_eq!(book.workspaces.len(), 1);
        assert_eq!(find(&book, "RELEASE").unwrap().entries[0].label, "two");

        assert!(remove(&mut book, "ReLeAsE"));
        assert!(!remove(&mut book, "release"));
        assert!(find(&book, "Release").is_none());
    }

    #[test]
    fn corrupt_file_is_an_error_and_is_never_overwritten() {
        let file = TestPath::new("corrupt");
        fs::write(file.path(), b"{ this is not json").unwrap();
        let error = load(file.path()).unwrap_err();
        assert!(error.contains("corrupt") || error.contains("json") || !error.is_empty());
        assert_eq!(fs::read(file.path()).unwrap(), b"{ this is not json");
    }

    #[test]
    fn save_creates_a_private_parent_directory() {
        let dir = Path::new("/tmp/opencode").join(format!(
            "oxterm-workspaces-dir-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let nested = dir.join("nested").join("workspaces.json");
        let book = WorkBook::default();
        save(&nested, &book).unwrap();
        let mode = fs::metadata(nested.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(load(&nested).unwrap(), book);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn default_path_ends_with_the_expected_file() {
        let path = default_path().expect("a home directory in this environment");
        assert!(path.ends_with("oxterm/workspaces.json"));
    }
}
