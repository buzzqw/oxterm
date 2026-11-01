//! The connection book: saved SSH destinations, independent of any display.
//!
//! OpenSSH stays the only SSH engine. This module stores what OpenSSH has no
//! place for (groups, tags, notes, saved tunnels) and turns an entry into an
//! `ssh` argument vector. It never writes `~/.ssh/config` and never stores a
//! secret, so `connections.json` holds nothing that a copy could leak.
//!
//! Hosts from `~/.ssh/config` show up without being imported. A `kind: alias`
//! record only adds metadata to one of them; a `kind: manual` record carries
//! its own parameters. The file format is public: other front ends read it, so
//! fields it does not know are preserved when the file is rewritten.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::askpass;
use super::paths;
use super::persistence::atomic_write;
use super::ssh_config;
use super::text::fuzzy_score;
use super::tunnels;

/// Newest `connections.json` format this build can rewrite. A newer file is
/// opened read-only so an older Oxterm cannot drop what it does not understand.
pub const CURRENT_VERSION: u32 = 1;

const MAX_RECENT: usize = 5;
const MAX_CONNECTIONS: usize = 5000;
const MAX_FIELD_CHARS: usize = 4096;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const ALIAS_ID_PREFIX: &str = "alias:";

pub fn book_path() -> PathBuf {
    paths::config_dir().join("connections.json")
}

pub fn usage_path() -> PathBuf {
    paths::config_dir().join("connections-state.json")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Manual,
    Alias,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// Keys, agent, or the prompt `ssh` shows in the terminal.
    #[default]
    Default,
    /// A password kept in the system keyring, answered by `oxterm-askpass`.
    SecretService,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    #[default]
    Local,
    Remote,
    Dynamic,
}

impl TunnelKind {
    pub fn flag(self) -> &'static str {
        match self {
            TunnelKind::Local => "-L",
            TunnelKind::Remote => "-R",
            TunnelKind::Dynamic => "-D",
        }
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tunnel {
    #[serde(rename = "type", default)]
    pub kind: TunnelKind,
    /// `[bind:]port` on the listening side.
    #[serde(default)]
    pub listen: String,
    /// `host:port` on the far side; unused by a dynamic (SOCKS) tunnel.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub autostart: bool,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl Tunnel {
    /// The value of the `-L`/`-R`/`-D` option.
    pub fn forward_spec(&self) -> String {
        match self.kind {
            TunnelKind::Dynamic => self.listen.clone(),
            _ => format!("{}:{}", self.listen, self.target),
        }
    }

    /// Short text for menus and logs.
    pub fn describe(&self) -> String {
        format!("{} {}", self.kind.flag(), self.forward_spec())
    }

    /// Address to probe to see that the local listener is up. A remote forward
    /// listens on the far side, where only the process state can be observed.
    pub fn probe_address(&self) -> Option<String> {
        if self.kind == TunnelKind::Remote {
            return None;
        }
        let (bind, port) = split_listen(&self.listen)?;
        let host = match bind {
            "" | "*" | "0.0.0.0" => "127.0.0.1",
            "[::]" => "[::1]",
            other => other,
        };
        Some(format!("{}:{}", host, port))
    }
}

/// Split `[bind:]port`, leaving a bracketed IPv6 bind address intact.
fn split_listen(listen: &str) -> Option<(&str, u16)> {
    let (bind, port) = listen.rsplit_once(':').unwrap_or(("", listen));
    let port: u16 = port.parse().ok()?;
    (port != 0).then_some((bind, port))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Stable identity used by tabs, restored sessions and the keyring, so a
    /// rename breaks nothing.
    pub id: String,
    #[serde(default)]
    pub kind: Kind,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub identity_file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_jump: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    /// `/`-separated path, for example `Customers/ACME`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub auth: Auth,
    /// Ask before connecting, for hosts where a stray keystroke is expensive.
    #[serde(default, skip_serializing_if = "is_default")]
    pub confirm_connect: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnels: Vec<Tunnel>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl Connection {
    /// An unannotated host from `~/.ssh/config`, or one typed by hand.
    pub fn alias(name: &str) -> Connection {
        Connection {
            id: alias_id(name),
            kind: Kind::Alias,
            name: name.to_string(),
            ..Connection::default()
        }
    }

    pub fn new_manual() -> Connection {
        Connection {
            id: new_id(),
            kind: Kind::Manual,
            ..Connection::default()
        }
    }

    /// The `[user@]host` argument of `ssh`.
    pub fn destination(&self) -> &str {
        match self.kind {
            Kind::Alias => &self.name,
            Kind::Manual => &self.host,
        }
    }

    /// One line for lists: where this entry actually goes.
    pub fn summary(&self) -> String {
        if self.kind == Kind::Alias {
            return "~/.ssh/config".to_string();
        }
        let mut text = String::new();
        if !self.user.is_empty() {
            text.push_str(&self.user);
            text.push('@');
        }
        text.push_str(&self.host);
        if let Some(port) = self.port {
            text.push_str(&format!(":{}", port));
        }
        text
    }

    /// Every option up to and including the destination. Also what `ssh -G`
    /// receives, because the remote command says nothing about configuration.
    pub fn target_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.kind == Kind::Manual {
            let mut option = |flag: &str, value: &str| {
                if !value.is_empty() {
                    args.push(flag.to_string());
                    args.push(value.to_string());
                }
            };
            option("-l", &self.user);
            option("-i", &self.identity_file);
            option("-J", &self.proxy_jump);
            if let Some(port) = self.port {
                args.push("-p".to_string());
                args.push(port.to_string());
            }
        }
        args.extend(self.extra_args.iter().cloned());
        args.push(self.destination().to_string());
        args
    }

    /// Arguments of an interactive session, without the leading `ssh`.
    pub fn session_args(&self) -> Vec<String> {
        let mut args = self.target_args();
        if !self.remote_command.is_empty() {
            let destination = args.pop().unwrap_or_default();
            args.push("-t".to_string());
            args.push(destination);
            args.push(self.remote_command.clone());
        }
        args
    }

    /// The `sftp` command for this entry. `sftp` shares few flags with `ssh`
    /// (`-P` for the port, no `-l`), so the user travels in the destination.
    pub fn sftp_argv(&self) -> Vec<String> {
        let mut argv = vec!["sftp".to_string()];
        if self.kind == Kind::Manual {
            if let Some(port) = self.port {
                argv.push("-P".to_string());
                argv.push(port.to_string());
            }
            if !self.identity_file.is_empty() {
                argv.push("-i".to_string());
                argv.push(self.identity_file.clone());
            }
            if !self.proxy_jump.is_empty() {
                argv.push("-J".to_string());
                argv.push(self.proxy_jump.clone());
            }
            if !self.user.is_empty() && !self.host.contains('@') {
                argv.push(format!("{}@{}", self.user, self.host));
                return argv;
            }
        }
        argv.push(self.destination().to_string());
        argv
    }

    /// The command shown to the user and put on the clipboard.
    pub fn command_preview(&self) -> String {
        let mut argv = vec!["ssh".to_string()];
        argv.extend(self.session_args());
        shell_words::join(argv)
    }

    /// Everything but the display name, so the form can reject bad input before
    /// it reaches a command line. The name is free text but appears in menus,
    /// so control characters are refused too.
    pub fn validate(&self) -> Result<(), String> {
        check_text("Name", &self.name)?;
        if self.name.trim().is_empty() {
            return Err("The name cannot be empty".to_string());
        }
        check_text("Group", &self.group)?;
        for tag in &self.tags {
            check_token("Tag", tag)?;
        }
        if self.notes.chars().count() > MAX_FIELD_CHARS {
            return Err("The notes are too long".to_string());
        }
        check_text("Remote command", &self.remote_command)?;
        match self.kind {
            Kind::Alias => check_argument("Host alias", &self.name)?,
            Kind::Manual => {
                if self.host.is_empty() {
                    return Err("The host cannot be empty".to_string());
                }
                check_argument("Host", &self.host)?;
                check_optional_argument("User", &self.user)?;
                check_optional_argument("ProxyJump", &self.proxy_jump)?;
                check_text("Identity file", &self.identity_file)?;
                if self.identity_file.starts_with('-') {
                    return Err("Identity file cannot start with '-'".to_string());
                }
                if self.port == Some(0) {
                    return Err("The port cannot be 0".to_string());
                }
            }
        }
        for argument in &self.extra_args {
            check_text("Extra argument", argument)?;
        }
        for tunnel in &self.tunnels {
            validate_tunnel(tunnel)?;
        }
        Ok(())
    }

    /// `extra_args` options that make `ssh` run a local program. Shown before
    /// connecting to an imported entry: the book is data, this part is not.
    pub fn local_execution_options(&self) -> Vec<String> {
        let mut found = Vec::new();
        for argument in &self.extra_args {
            let lower = argument.to_ascii_lowercase();
            if [
                "proxycommand",
                "localcommand",
                "permitlocalcommand",
                "knownhostscommand",
            ]
            .iter()
            .any(|option| lower.contains(option))
            {
                found.push(argument.clone());
            }
        }
        found
    }

    /// Text every search looks at.
    fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name,
            self.host,
            self.user,
            self.group,
            self.tags.join(" "),
            self.notes
        )
        .to_lowercase()
    }
}

fn check_text(label: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > MAX_FIELD_CHARS {
        return Err(format!("{} is too long", label));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{} contains control characters", label));
    }
    Ok(())
}

fn check_token(label: &str, value: &str) -> Result<(), String> {
    check_text(label, value)?;
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{} cannot contain spaces", label));
    }
    Ok(())
}

/// A value placed where `ssh` looks for options: a leading dash would turn data
/// into an option such as `-oProxyCommand=...`, so it is never accepted.
fn check_argument(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{} cannot be empty", label));
    }
    check_token(label, value)?;
    if value.starts_with('-') {
        return Err(format!("{} cannot start with '-'", label));
    }
    Ok(())
}

fn check_optional_argument(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        Ok(())
    } else {
        check_argument(label, value)
    }
}

pub fn validate_tunnel(tunnel: &Tunnel) -> Result<(), String> {
    check_argument("Tunnel listen address", &tunnel.listen)?;
    if split_listen(&tunnel.listen).is_none() {
        return Err(format!(
            "Tunnel listen address must be [bind:]port: {}",
            tunnel.listen
        ));
    }
    if tunnel.kind != TunnelKind::Dynamic {
        check_argument("Tunnel target", &tunnel.target)?;
        let valid = tunnel
            .target
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok());
        if !valid {
            return Err(format!(
                "Tunnel target must be host:port: {}",
                tunnel.target
            ));
        }
    }
    Ok(())
}

pub fn alias_id(name: &str) -> String {
    format!("{}{}", ALIAS_ID_PREFIX, name)
}

/// A random identifier for a manual entry.
pub fn new_id() -> String {
    let mut bytes = [0u8; 8];
    let random = File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if random.is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        bytes = (nanos ^ u64::from(std::process::id()).rotate_left(32)).to_be_bytes();
    }
    let hex: String = bytes.iter().map(|byte| format!("{:02x}", byte)).collect();
    format!("c{}", hex)
}

// ── The book on disk ─────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub version: u32,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl Default for Book {
    fn default() -> Book {
        Book {
            version: CURRENT_VERSION,
            connections: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }
}

impl Book {
    pub fn read_only(&self) -> bool {
        self.version > CURRENT_VERSION
    }

    pub fn find_by_id(&self, id: &str) -> Option<&Connection> {
        self.connections.iter().find(|entry| entry.id == id)
    }

    /// Insert or replace by id. Names stay unique, because the name is what a
    /// person types at `oxterm connect NAME`.
    pub fn upsert(&mut self, connection: Connection) -> Result<(), String> {
        connection.validate()?;
        let clash = self
            .connections
            .iter()
            .any(|entry| entry.name == connection.name && entry.id != connection.id);
        if clash {
            return Err(format!(
                "A connection named '{}' already exists",
                connection.name
            ));
        }
        if let Some(slot) = self
            .connections
            .iter_mut()
            .find(|entry| entry.id == connection.id)
        {
            *slot = connection;
            return Ok(());
        }
        if self.connections.len() >= MAX_CONNECTIONS {
            return Err("The connection book is full".to_string());
        }
        self.connections.push(connection);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.connections.len();
        self.connections.retain(|entry| entry.id != id);
        self.connections.len() != before
    }
}

/// Read the book. A missing file is an empty book; a damaged one is an error,
/// so the caller never saves over data it could not read.
pub fn load_book(path: &Path) -> Result<Book, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Book::default());
        }
        Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
    };
    serde_json::from_str(&contents).map_err(|error| format!("{}: {}", path.display(), error))
}

pub fn save_book(path: &Path, book: &Book) -> Result<(), String> {
    if book.read_only() {
        return Err(format!(
            "{} was written by a newer Oxterm and is read-only",
            path.display()
        ));
    }
    ensure_parent(path)?;
    let mut contents = serde_json::to_string_pretty(book).map_err(|error| error.to_string())?;
    contents.push('\n');
    atomic_write(path, contents.as_bytes())
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

/// Load, change and save under a lock, so two Oxterm processes editing the book
/// cannot lose each other's changes. The change returns its own value.
pub fn update_book<T>(
    path: &Path,
    change: impl FnOnce(&mut Book) -> Result<T, String>,
) -> Result<T, String> {
    ensure_parent(path)?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| format!("cannot open {}: {}", lock_path.display(), error))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() >= deadline {
            return Err("the connection book is locked by another process".to_string());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let mut book = load_book(path)?;
    let result = change(&mut book)?;
    save_book(path, &book)?;
    Ok(result)
    // Closing `lock` releases the flock.
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    /// One line per skipped entry, naming it and saying why.
    pub skipped: Vec<String>,
}

/// Add the entries of another book. A shared file is data from someone else, so
/// it can only ever add: every entry gets a new id (it can neither replace an
/// entry nor reach a stored password), a taken name is skipped, and an entry
/// that would run a local program needs explicit consent.
pub fn import_into(
    book: &mut Book,
    incoming: Book,
    allow_local: bool,
) -> Result<ImportReport, String> {
    if incoming.read_only() {
        return Err("that file was written by a newer Oxterm".to_string());
    }
    let mut report = ImportReport::default();
    for mut connection in incoming.connections {
        let name = connection.name.clone();
        let risky = connection.local_execution_options();
        if !risky.is_empty() && !allow_local {
            report.skipped.push(format!(
                "{}: runs a local program ({}); pass --allow-local-commands",
                name,
                risky.join(" ")
            ));
            continue;
        }
        connection.id = match connection.kind {
            Kind::Alias => alias_id(&connection.name),
            Kind::Manual => new_id(),
        };
        connection.auth = Auth::Default;
        if book.connections.iter().any(|entry| entry.name == name) {
            report.skipped.push(format!("{}: the name is taken", name));
            continue;
        }
        match book.upsert(connection) {
            Ok(()) => report.imported += 1,
            Err(error) => report.skipped.push(format!("{}: {}", name, error)),
        }
    }
    Ok(report)
}

// ── Usage state ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub last_used: u64,
    #[serde(default)]
    pub count: u64,
}

pub type UsageMap = HashMap<String, Usage>;

/// Usage lives apart from the book, so a connection does not rewrite a file the
/// user may keep under version control. Damage here only loses the recent list.
pub fn load_usage(path: &Path) -> UsageMap {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Record one connection. Concurrent writers may lose a count; nothing depends
/// on an exact number, and the atomic write keeps the file valid.
pub fn record_usage(path: &Path, id: &str) -> Result<(), String> {
    let mut usage = load_usage(path);
    let entry = usage.entry(id.to_string()).or_default();
    entry.count = entry.count.saturating_add(1);
    entry.last_used = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    ensure_parent(path)?;
    let contents = serde_json::to_string(&usage).map_err(|error| error.to_string())?;
    atomic_write(path, contents.as_bytes()).map_err(|error| error.to_string())
}

// ── Merging with ~/.ssh/config ───────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Created in Oxterm.
    Manual,
    /// A host from `~/.ssh/config` with Oxterm metadata.
    Annotated,
    /// A host from `~/.ssh/config`, untouched.
    SshConfig,
    /// Metadata whose host left `~/.ssh/config`. Kept, never deleted silently.
    Orphan,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub connection: Connection,
    pub origin: Origin,
}

/// The book plus every concrete host from `~/.ssh/config`.
pub fn merge(book: &Book, hosts: &[String]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = book
        .connections
        .iter()
        .map(|connection| {
            let origin = match connection.kind {
                Kind::Manual => Origin::Manual,
                Kind::Alias if hosts.contains(&connection.name) => Origin::Annotated,
                Kind::Alias => Origin::Orphan,
            };
            Entry {
                connection: connection.clone(),
                origin,
            }
        })
        .collect();
    for host in hosts {
        let annotated = book
            .connections
            .iter()
            .any(|entry| entry.kind == Kind::Alias && entry.name == *host);
        if !annotated {
            entries.push(Entry {
                connection: Connection::alias(host),
                origin: Origin::SshConfig,
            });
        }
    }
    entries
}

/// Find what a person typed: a name first, then a saved host.
pub fn find_entry<'a>(entries: &'a [Entry], text: &str) -> Option<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.connection.name == text)
        .or_else(|| {
            entries.iter().find(|entry| {
                entry.connection.kind == Kind::Manual && entry.connection.host == text
            })
        })
}

pub fn find_entry_by_id<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
    entries.iter().find(|entry| entry.connection.id == id)
}

// ── Tree shown by the panel ──────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Group {
        title: String,
        children: Vec<Node>,
    },
    /// An index into the entries the tree was built from.
    Leaf(usize),
}

/// Rank `entries` for `query`; every word has to match. Substring hits rank
/// above fuzzy ones, and a hit in the name above a hit elsewhere.
fn search(entries: &[Entry], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<(i64, usize)> = Vec::new();
    'entry: for (index, entry) in entries.iter().enumerate() {
        let name = entry.connection.name.to_lowercase();
        let haystack = entry.connection.haystack();
        let mut score = 0i64;
        for word in &words {
            if name.contains(word.as_str()) {
                score += 2000;
            } else if haystack.contains(word.as_str()) {
                score += 1000;
            } else if let Some(fuzzy) = fuzzy_score(word, &name) {
                score += i64::from(fuzzy);
            } else {
                continue 'entry;
            }
        }
        hits.push((score, index));
    }
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            sort_key(&entries[a.1].connection).cmp(&sort_key(&entries[b.1].connection))
        })
    });
    hits.into_iter().map(|(_, index)| index).collect()
}

fn sort_key(connection: &Connection) -> String {
    connection.name.to_lowercase()
}

#[derive(Default)]
struct GroupBuilder {
    groups: BTreeMap<String, (String, GroupBuilder)>,
    leaves: Vec<usize>,
}

impl GroupBuilder {
    fn insert(&mut self, path: &[&str], leaf: usize) {
        match path.split_first() {
            None => self.leaves.push(leaf),
            Some((first, rest)) => {
                self.groups
                    .entry(first.to_lowercase())
                    .or_insert_with(|| (first.to_string(), GroupBuilder::default()))
                    .1
                    .insert(rest, leaf);
            }
        }
    }

    fn into_nodes(self, entries: &[Entry]) -> Vec<Node> {
        let mut nodes: Vec<Node> = self
            .groups
            .into_values()
            .map(|(title, builder)| Node::Group {
                title,
                children: builder.into_nodes(entries),
            })
            .collect();
        nodes.extend(sorted_leaves(self.leaves, entries));
        nodes
    }
}

fn sorted_leaves(mut leaves: Vec<usize>, entries: &[Entry]) -> Vec<Node> {
    leaves.sort_by_key(|&index| sort_key(&entries[index].connection));
    leaves.into_iter().map(Node::Leaf).collect()
}

/// The panel's tree. With a query it is a flat ranked list. Without one:
/// Favorites, Recent, the user's groups, then hosts nobody filed yet.
pub fn build_tree(entries: &[Entry], usage: &UsageMap, query: &str) -> Vec<Node> {
    if !query.trim().is_empty() {
        return search(entries, query).into_iter().map(Node::Leaf).collect();
    }
    let mut nodes = Vec::new();
    let favorites: Vec<usize> = (0..entries.len())
        .filter(|&index| entries[index].connection.favorite)
        .collect();
    if !favorites.is_empty() {
        nodes.push(Node::Group {
            title: "Favorites".to_string(),
            children: sorted_leaves(favorites, entries),
        });
    }
    let mut recent: Vec<(u64, usize)> = (0..entries.len())
        .filter_map(|index| {
            let used = usage.get(&entries[index].connection.id)?.last_used;
            (used > 0).then_some((used, index))
        })
        .collect();
    recent.sort_by(|a, b| b.cmp(a));
    recent.truncate(MAX_RECENT);
    if !recent.is_empty() {
        nodes.push(Node::Group {
            title: "Recent".to_string(),
            children: recent
                .into_iter()
                .map(|(_, index)| Node::Leaf(index))
                .collect(),
        });
    }
    let mut grouped = GroupBuilder::default();
    let mut ungrouped = Vec::new();
    let mut from_config = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let path: Vec<&str> = entry
            .connection
            .group
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .collect();
        if !path.is_empty() {
            grouped.insert(&path, index);
        } else if entry.connection.kind == Kind::Alias {
            from_config.push(index);
        } else {
            ungrouped.push(index);
        }
    }
    nodes.extend(grouped.into_nodes(entries));
    if !ungrouped.is_empty() {
        nodes.push(Node::Group {
            title: "Ungrouped".to_string(),
            children: sorted_leaves(ungrouped, entries),
        });
    }
    if !from_config.is_empty() {
        nodes.push(Node::Group {
            title: "~/.ssh/config".to_string(),
            children: sorted_leaves(from_config, entries),
        });
    }
    nodes
}

// ── Commands ─────────────────────────────────────────────────

/// What it takes to open an entry in a terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectPlan {
    pub argv: Vec<String>,
    pub forwards_agent: bool,
}

/// The `ssh` command of a session. `effective` is `ssh -G` output; without it
/// the command still works, just without connection sharing and keepalives.
pub fn connect_plan(
    connection: &Connection,
    effective: Option<&str>,
    socket_dir: &Path,
) -> ConnectPlan {
    let mut argv = vec!["ssh".to_string()];
    let mut forwards_agent = false;
    if let Some(config) = effective {
        argv.extend(ssh_config::sharing_options(config, socket_dir));
        forwards_agent = ssh_config::agent_forwarding_enabled(config);
    }
    argv.extend(connection.session_args());
    ConnectPlan {
        argv,
        forwards_agent,
    }
}

/// Bound on `ssh -G`: past this a connection opens without sharing rather than
/// waiting for a slow `Match exec` rule.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Everything needed to open a session, computed off the GUI thread because
/// `ssh -G` can block.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub plan: ConnectPlan,
    /// Why a stored password cannot be used, so `ssh` will prompt instead.
    pub keyring_note: Option<&'static str>,
}

pub fn prepare_session(connection: &Connection) -> Prepared {
    let effective = ssh_config::effective_config(&connection.target_args(), PROBE_TIMEOUT);
    let mut plan = connect_plan(connection, effective.as_deref(), &paths::config_dir());
    let mut keyring_note = None;
    if connection.auth == Auth::SecretService {
        match askpass_environment(connection, effective.as_deref()) {
            Ok(environment) => {
                // `env` keeps the terminal API simple and adds nothing secret:
                // these values only say where to look.
                let mut argv = vec!["env".to_string()];
                argv.extend(
                    environment
                        .into_iter()
                        .map(|(key, value)| format!("{}={}", key, value)),
                );
                argv.append(&mut plan.argv);
                plan.argv = argv;
            }
            Err(note) => keyring_note = Some(note),
        }
    }
    Prepared { plan, keyring_note }
}

/// The environment that lets `ssh` use the keyring helper for this entry.
pub fn askpass_environment(
    connection: &Connection,
    effective: Option<&str>,
) -> Result<Vec<(String, String)>, &'static str> {
    let helper =
        askpass::helper_path().ok_or("oxterm-askpass is not installed; ssh will prompt")?;
    let target = effective
        .and_then(askpass::target_from_config)
        .ok_or("Could not resolve this host's user; ssh will prompt")?;
    Ok(askpass::ssh_environment(&helper, &connection.id, &target))
}

/// What to run for one tunnel of an entry. Blocks on `ssh -G` for an entry with
/// a stored password, so call it off the GUI thread.
pub fn tunnel_spec(connection: &Connection, tunnel: &Tunnel) -> tunnels::Spec {
    let mut env = Vec::new();
    let mut batch = true;
    if connection.auth == Auth::SecretService {
        let effective = ssh_config::effective_config(&connection.target_args(), PROBE_TIMEOUT);
        if let Ok(values) = askpass_environment(connection, effective.as_deref()) {
            env = values;
            batch = false;
        }
    }
    tunnels::Spec {
        argv: tunnel_argv(connection, tunnel, batch),
        env,
        probe: tunnel.probe_address(),
    }
}

pub fn tunnel_key(connection: &Connection, tunnel: &Tunnel) -> String {
    tunnels::key(&connection.id, &tunnel.describe())
}

/// The `ssh` command of a background tunnel. It has its own connection: a
/// forward on a shared master would die with whichever tab owns that master.
/// `batch` refuses to prompt, because nobody could answer.
pub fn tunnel_argv(connection: &Connection, tunnel: &Tunnel, batch: bool) -> Vec<String> {
    let mut argv: Vec<String> = ["ssh", "-N"].iter().map(|arg| arg.to_string()).collect();
    for option in [
        "ExitOnForwardFailure=yes",
        "ControlMaster=no",
        "ControlPath=none",
        "ConnectTimeout=15",
        "ServerAliveInterval=30",
        "ServerAliveCountMax=3",
    ] {
        argv.push("-o".to_string());
        argv.push(option.to_string());
    }
    if batch {
        argv.push("-o".to_string());
        argv.push("BatchMode=yes".to_string());
    }
    argv.push(tunnel.kind.flag().to_string());
    argv.push(tunnel.forward_spec());
    argv.extend(connection.target_args());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxterm-connections-{}-{}-{:?}",
            name,
            std::process::id(),
            thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manual(name: &str, host: &str) -> Connection {
        Connection {
            id: format!("id-{}", name),
            name: name.to_string(),
            host: host.to_string(),
            ..Connection::default()
        }
    }

    fn leaf_names(nodes: &[Node], entries: &[Entry]) -> Vec<String> {
        nodes
            .iter()
            .filter_map(|node| match node {
                Node::Leaf(index) => Some(entries[*index].connection.name.clone()),
                Node::Group { .. } => None,
            })
            .collect()
    }

    #[test]
    fn manual_arguments_keep_options_before_the_destination() {
        let connection = Connection {
            user: "admin".into(),
            port: Some(2222),
            identity_file: "~/.ssh/id".into(),
            proxy_jump: "bastion".into(),
            extra_args: vec!["-C".into()],
            ..manual("db", "10.0.0.1")
        };
        assert_eq!(
            connection.session_args(),
            [
                "-l",
                "admin",
                "-i",
                "~/.ssh/id",
                "-J",
                "bastion",
                "-p",
                "2222",
                "-C",
                "10.0.0.1"
            ]
        );
        assert_eq!(
            ssh_config::ssh_destination(&connection.session_args()).as_deref(),
            Some("10.0.0.1"),
            "the production badge and the statistics must find the host"
        );
    }

    #[test]
    fn remote_command_forces_a_terminal_and_stays_one_argument() {
        let connection = Connection {
            remote_command: "tmux new -A -s main".into(),
            ..manual("db", "h")
        };
        assert_eq!(
            connection.session_args(),
            ["-t", "h", "tmux new -A -s main"]
        );
        assert_eq!(connection.target_args(), ["h"]);
        assert_eq!(
            connection.command_preview(),
            "ssh -t h 'tmux new -A -s main'"
        );
    }

    #[test]
    fn sftp_uses_its_own_flags() {
        let connection = Connection {
            user: "admin".into(),
            port: Some(2222),
            proxy_jump: "bastion".into(),
            ..manual("db", "h")
        };
        assert_eq!(
            connection.sftp_argv(),
            ["sftp", "-P", "2222", "-J", "bastion", "admin@h"]
        );
        assert_eq!(Connection::alias("web").sftp_argv(), ["sftp", "web"]);
    }

    #[test]
    fn alias_lets_openssh_resolve_everything() {
        let mut connection = Connection::alias("web");
        // Stale manual fields must not leak into the command of an alias.
        connection.user = "ignored".into();
        connection.host = "ignored".into();
        assert_eq!(connection.session_args(), ["web"]);
    }

    #[test]
    fn values_cannot_become_ssh_options() {
        for (host, user, jump) in [
            ("-oProxyCommand=evil", "", ""),
            ("host name", "", ""),
            ("h\n", "", ""),
            ("h", "-oProxyCommand=evil", ""),
            ("h", "", "-oProxyCommand=evil"),
            ("", "", ""),
        ] {
            let connection = Connection {
                user: user.into(),
                proxy_jump: jump.into(),
                ..manual("x", host)
            };
            assert!(
                connection.validate().is_err(),
                "accepted {host:?} {user:?} {jump:?}"
            );
        }
        assert!(Connection::alias("-oProxyCommand=evil").validate().is_err());
        assert!(Connection {
            identity_file: "-oProxyCommand=evil".into(),
            ..manual("x", "h")
        }
        .validate()
        .is_err());
        assert!(manual("", "h").validate().is_err());
        assert!(manual("ok", "h").validate().is_ok());
        assert!(manual("ok", "user@[::1]").validate().is_ok());
    }

    #[test]
    fn extra_arguments_that_run_programs_are_reported() {
        let connection = Connection {
            extra_args: vec!["-C".into(), "-oProxyCommand=nc %h %p".into()],
            ..manual("x", "h")
        };
        assert_eq!(
            connection.local_execution_options(),
            ["-oProxyCommand=nc %h %p"]
        );
    }

    #[test]
    fn tunnels_are_validated_and_formatted() {
        let local = Tunnel {
            kind: TunnelKind::Local,
            listen: "127.0.0.1:5433".into(),
            target: "db:5432".into(),
            ..Tunnel::default()
        };
        assert!(validate_tunnel(&local).is_ok());
        assert_eq!(local.forward_spec(), "127.0.0.1:5433:db:5432");
        assert_eq!(local.probe_address().as_deref(), Some("127.0.0.1:5433"));

        let socks = Tunnel {
            kind: TunnelKind::Dynamic,
            listen: "1080".into(),
            ..Tunnel::default()
        };
        assert!(validate_tunnel(&socks).is_ok());
        assert_eq!(socks.forward_spec(), "1080");
        assert_eq!(socks.probe_address().as_deref(), Some("127.0.0.1:1080"));

        let remote = Tunnel {
            kind: TunnelKind::Remote,
            listen: "8080".into(),
            target: "localhost:80".into(),
            ..Tunnel::default()
        };
        assert_eq!(remote.probe_address(), None);

        let v6 = Tunnel {
            listen: "[::1]:5433".into(),
            target: "[::1]:5432".into(),
            ..Tunnel::default()
        };
        assert!(validate_tunnel(&v6).is_ok());
        assert_eq!(v6.probe_address().as_deref(), Some("[::1]:5433"));

        for (listen, target) in [
            ("abc", "h:1"),
            ("0", "h:1"),
            ("70000", "h:1"),
            ("-oX=1:1", "h:1"),
            ("1", ""),
            ("1", "noport"),
            ("1", ":5"),
            ("1", "-oX:1"),
            ("1", "h:notaport"),
        ] {
            let tunnel = Tunnel {
                listen: listen.into(),
                target: target.into(),
                ..Tunnel::default()
            };
            assert!(
                validate_tunnel(&tunnel).is_err(),
                "accepted {listen} {target}"
            );
        }
    }

    #[test]
    fn tunnel_commands_never_share_a_master() {
        let connection = Connection {
            user: "u".into(),
            remote_command: "ignored by a tunnel".into(),
            ..manual("db", "h")
        };
        let tunnel = Tunnel {
            listen: "1".into(),
            target: "t:2".into(),
            ..Tunnel::default()
        };
        let argv = tunnel_argv(&connection, &tunnel, true);
        assert_eq!(&argv[..2], ["ssh", "-N"]);
        assert!(argv
            .windows(2)
            .any(|pair| pair == ["-o", "ControlPath=none"]));
        assert!(argv.windows(2).any(|pair| pair == ["-o", "BatchMode=yes"]));
        assert!(argv.windows(2).any(|pair| pair == ["-L", "1:t:2"]));
        assert_eq!(argv.last().map(String::as_str), Some("h"));
        assert!(!tunnel_argv(&connection, &tunnel, false)
            .iter()
            .any(|arg| arg == "BatchMode=yes"));
    }

    #[test]
    fn plan_adds_sharing_only_when_the_configuration_allows_it() {
        let connection = manual("db", "h");
        let dir = Path::new("/cfg");
        let bare = connect_plan(&connection, None, dir);
        assert_eq!(bare.argv, ["ssh", "h"]);
        let config = "controlpath none\nserveraliveinterval 0\nforwardagent yes\n";
        let shared = connect_plan(&connection, Some(config), dir);
        assert!(shared.forwards_agent);
        assert!(shared.argv.iter().any(|arg| arg == "ControlMaster=auto"));
        assert_eq!(shared.argv.last().map(String::as_str), Some("h"));
        let own = "controlpath ~/.ssh/cm/%C\nserveraliveinterval 5\n";
        assert_eq!(connect_plan(&connection, Some(own), dir).argv, ["ssh", "h"]);
    }

    #[test]
    fn book_round_trips_and_keeps_unknown_fields() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("connections.json");
        fs::write(
            &path,
            r#"{"version":1,"future_top":1,"connections":[
                {"id":"a","name":"web","kind":"alias","group":"G","future":{"x":1},
                 "tunnels":[{"type":"dynamic","listen":"1080","future_t":true}]}
            ]}"#,
        )
        .unwrap();
        let mut book = load_book(&path).unwrap();
        assert_eq!(book.connections[0].tunnels[0].kind, TunnelKind::Dynamic);
        book.connections[0].favorite = true;
        save_book(&path, &book).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        for field in ["future_top", "future", "future_t", "favorite"] {
            assert!(text.contains(field), "lost {field}: {text}");
        }
        assert!(!text.contains("identity_file"), "empty fields stay out");
        let mode = fs::metadata(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_or_newer_books_are_never_overwritten() {
        let dir = temp_dir("guard");
        let path = dir.join("connections.json");
        assert_eq!(load_book(&path).unwrap(), Book::default());

        fs::write(&path, "{ not json").unwrap();
        assert!(load_book(&path).is_err());
        assert!(update_book(&path, |_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");

        let newer = r#"{"version":999,"connections":[]}"#;
        fs::write(&path, newer).unwrap();
        assert!(load_book(&path).unwrap().read_only());
        assert!(update_book(&path, |_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_serializes_concurrent_writers() {
        let dir = temp_dir("lock");
        let path = dir.join("connections.json");
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                thread::spawn(move || {
                    update_book(&path, |book| book.upsert(manual(&format!("n{}", n), "h")))
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(load_book(&path).unwrap().connections.len(), 8);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_are_unique_and_editing_keeps_identity() {
        let mut book = Book::default();
        book.upsert(manual("a", "h")).unwrap();
        book.upsert(manual("b", "h")).unwrap();
        let mut renamed = manual("b", "h");
        renamed.name = "a".into();
        assert!(book.upsert(renamed.clone()).is_err());
        renamed.name = "c".into();
        book.upsert(renamed).unwrap();
        assert_eq!(book.find_by_id("id-b").unwrap().name, "c");
        assert_eq!(book.connections.len(), 2);
        assert!(book.remove("id-a"));
        assert!(!book.remove("id-a"));
    }

    #[test]
    fn merge_shows_config_hosts_and_keeps_orphans() {
        let mut book = Book::default();
        book.connections.push(manual("own", "h"));
        let mut annotated = Connection::alias("web");
        annotated.group = "G".into();
        book.connections.push(annotated);
        book.connections.push(Connection::alias("gone"));
        let hosts = vec!["web".to_string(), "new".to_string()];
        let entries = merge(&book, &hosts);
        let origin = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.connection.name == name)
                .unwrap()
                .origin
        };
        assert_eq!(entries.len(), 4, "no duplicate for an annotated host");
        assert_eq!(origin("own"), Origin::Manual);
        assert_eq!(origin("web"), Origin::Annotated);
        assert_eq!(origin("new"), Origin::SshConfig);
        assert_eq!(origin("gone"), Origin::Orphan);
        assert_eq!(find_entry(&entries, "h").unwrap().connection.name, "own");
        assert_eq!(find_entry(&entries, "web").unwrap().connection.group, "G");
        assert!(find_entry(&entries, "absent").is_none());
    }

    #[test]
    fn tree_orders_sections_and_nests_groups() {
        let mut book = Book::default();
        let mut favorite = manual("fav", "h");
        favorite.favorite = true;
        favorite.group = "Customers/ACME".into();
        book.connections.push(favorite);
        let mut second = manual("db", "h");
        second.group = "customers / Beta".into();
        book.connections.push(second);
        book.connections.push(manual("loose", "h"));
        let entries = merge(&book, &["cfg".to_string()]);
        let mut usage = UsageMap::new();
        usage.insert(
            "id-loose".to_string(),
            Usage {
                last_used: 5,
                count: 1,
            },
        );
        let tree = build_tree(&entries, &usage, "");
        let titles: Vec<&str> = tree
            .iter()
            .filter_map(|node| match node {
                Node::Group { title, .. } => Some(title.as_str()),
                Node::Leaf(_) => None,
            })
            .collect();
        assert_eq!(
            titles,
            [
                "Favorites",
                "Recent",
                "Customers",
                "Ungrouped",
                "~/.ssh/config"
            ]
        );
        let Node::Group { children, .. } = &tree[2] else {
            panic!("expected a group");
        };
        let nested: Vec<&str> = children
            .iter()
            .filter_map(|node| match node {
                Node::Group { title, .. } => Some(title.as_str()),
                Node::Leaf(_) => None,
            })
            .collect();
        assert_eq!(nested, ["ACME", "Beta"], "case-insensitive group merge");
    }

    #[test]
    fn recent_keeps_the_newest_five() {
        let mut book = Book::default();
        let mut usage = UsageMap::new();
        for n in 0..8 {
            book.connections.push(manual(&format!("n{}", n), "h"));
            usage.insert(
                format!("id-n{}", n),
                Usage {
                    last_used: n + 1,
                    count: 1,
                },
            );
        }
        let entries = merge(&book, &[]);
        let tree = build_tree(&entries, &usage, "");
        let Node::Group { children, .. } = &tree[0] else {
            panic!("expected Recent");
        };
        assert_eq!(
            leaf_names(children, &entries),
            ["n7", "n6", "n5", "n4", "n3"]
        );
    }

    #[test]
    fn search_is_flat_and_requires_every_word() {
        let mut book = Book::default();
        let mut tagged = manual("alpha", "10.0.0.1");
        tagged.tags = vec!["prod".into()];
        tagged.group = "ACME".into();
        book.connections.push(tagged);
        let mut noted = manual("beta", "10.0.0.2");
        noted.notes = "alpha replica".into();
        book.connections.push(noted);
        let entries = merge(&book, &[]);
        let usage = UsageMap::new();
        assert_eq!(
            leaf_names(&build_tree(&entries, &usage, "alpha"), &entries),
            ["alpha", "beta"],
            "a name hit ranks above a notes hit"
        );
        assert_eq!(
            leaf_names(&build_tree(&entries, &usage, "ACME PROD"), &entries),
            ["alpha"]
        );
        assert!(build_tree(&entries, &usage, "alpha zzzz").is_empty());
    }

    #[test]
    fn usage_survives_damage_and_counts() {
        let dir = temp_dir("usage");
        let path = dir.join("state.json");
        fs::write(&path, "garbage").unwrap();
        assert!(load_usage(&path).is_empty());
        record_usage(&path, "a").unwrap();
        record_usage(&path, "a").unwrap();
        let usage = load_usage(&path);
        assert_eq!(usage["a"].count, 2);
        assert!(usage["a"].last_used > 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn import_only_adds_and_asks_about_local_programs() {
        let mut book = Book::default();
        let mut mine = manual("mine", "keep.example");
        mine.auth = Auth::SecretService;
        book.upsert(mine).unwrap();

        let mut incoming = Book::default();
        // Same id and name as an existing entry: must not replace it.
        incoming.connections.push(manual("mine", "evil.example"));
        // Same id, new name: must not reach the original's stored password.
        let mut stolen = manual("other", "evil.example");
        stolen.id = "id-mine".into();
        stolen.auth = Auth::SecretService;
        incoming.connections.push(stolen);
        incoming.connections.push(Connection {
            extra_args: vec!["-oProxyCommand=evil".into()],
            ..manual("risky", "h")
        });
        incoming.connections.push(manual("-bad name", "-oX"));

        let report = import_into(&mut book, incoming.clone(), false).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped.len(), 3, "{:?}", report.skipped);
        assert_eq!(book.find_by_id("id-mine").unwrap().host, "keep.example");
        let added = book
            .connections
            .iter()
            .find(|entry| entry.name == "other")
            .unwrap();
        assert_ne!(added.id, "id-mine");
        assert_eq!(added.auth, Auth::Default);
        assert!(!book.connections.iter().any(|entry| entry.name == "risky"));

        let report = import_into(&mut book, incoming, true).unwrap();
        assert!(book.connections.iter().any(|entry| entry.name == "risky"));
        assert_eq!(report.imported, 1, "{:?}", report.skipped);
    }

    #[test]
    fn ids_are_distinct() {
        assert_ne!(new_id(), new_id());
        assert!(new_id().starts_with('c'));
        assert_eq!(alias_id("web"), "alias:web");
    }
}
