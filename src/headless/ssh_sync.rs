//! Pure book ⇄ `~/.ssh/config` synchronisation logic.
//!
//! The GUI reads the config file, asks the user which changes to apply and
//! writes the result with an atomic write. Nothing here touches the filesystem:
//! `diff` compares data and `render` only rewrites text.

use super::connections::{Book, Connection, Kind, Protocol};

/// One proposed change to an ssh config block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncChange {
    /// The book has an entry that ssh config does not know yet.
    Add,
    /// The alias exists, but SSH-owned fields have different values.
    Update,
    /// The config host has no counterpart in the book.
    Remove,
    /// Same values on both sides; not selected by default.
    Unchanged,
}

/// A row of the sync dialog. For `Add`/`Update`, `fields` holds config lines
/// such as `HostName example.com` that `render` writes into the block; the
/// other changes carry no fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshSyncEntry {
    pub alias: String,
    pub host: String,
    pub change: SyncChange,
    pub fields: Vec<String>,
    pub selected: bool,
}

/// A concrete host parsed from an OpenSSH configuration file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub host_name: String,
    pub user: String,
    pub port: Option<u16>,
    pub identity_file: String,
    pub proxy_jump: String,
}

/// What `diff` needs to know about a host from ssh config.
///
/// `ssh_config::load_hosts()` returns aliases only, so `String` implements this
/// without field information: a matching alias is then reported as `Unchanged`
/// because its values cannot be compared. Pass `parse_hosts(text)` for a
/// field-aware diff.
pub trait SshHostInfo {
    fn alias(&self) -> &str;
    fn fields(&self) -> Option<&SshHost>;
}

impl SshHostInfo for str {
    fn alias(&self) -> &str {
        self
    }
    fn fields(&self) -> Option<&SshHost> {
        None
    }
}

impl SshHostInfo for String {
    fn alias(&self) -> &str {
        self
    }
    fn fields(&self) -> Option<&SshHost> {
        None
    }
}

impl SshHostInfo for SshHost {
    fn alias(&self) -> &str {
        &self.alias
    }
    fn fields(&self) -> Option<&SshHost> {
        Some(self)
    }
}

fn concrete_alias(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.starts_with('!')
        && !pattern.starts_with('-')
        && !pattern.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}

/// Parse concrete `Host` blocks of an ssh config text. Wildcards, negated
/// patterns and unsupported syntax are ignored, as in `ssh_config::load_hosts`.
pub fn parse_hosts(text: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    // Indices of the hosts declared by the current `Host` line. Options apply
    // to that block only, never to blocks seen earlier in the file.
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Ok(mut fields) = shell_words::split(line) else {
            continue;
        };
        if let Some(first) = fields.first().cloned() {
            if let Some((keyword, value)) = first.split_once('=') {
                fields[0] = keyword.to_string();
                fields.insert(1, value.to_string());
            }
        }
        let Some(keyword) = fields.first() else {
            continue;
        };
        let value = fields.get(1).map(String::as_str).unwrap_or("");
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                current.clear();
                for pattern in &fields[1..] {
                    if concrete_alias(pattern) && !hosts.iter().any(|host| host.alias == *pattern) {
                        current.push(hosts.len());
                        hosts.push(SshHost {
                            alias: pattern.clone(),
                            ..SshHost::default()
                        });
                    }
                }
            }
            "hostname" | "user" | "port" | "identityfile" | "proxyjump" => {
                for &index in &current {
                    let host = &mut hosts[index];
                    match keyword.to_ascii_lowercase().as_str() {
                        "hostname" => host.host_name = value.to_string(),
                        "user" => host.user = value.to_string(),
                        "port" => host.port = value.parse().ok(),
                        "identityfile" => host.identity_file = value.to_string(),
                        "proxyjump" => host.proxy_jump = value.to_string(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    hosts
}

/// The alias a book entry would use in ssh config. Names with spaces are not
/// valid aliases; a manual entry can fall back to its host name.
fn alias_for(connection: &Connection) -> Option<String> {
    let candidate = if concrete_alias(&connection.name) {
        connection.name.clone()
    } else if connection.kind == Kind::Manual && concrete_alias(&connection.host) {
        connection.host.clone()
    } else {
        String::new()
    };
    (!candidate.is_empty()).then_some(candidate)
}

/// The SSH-owned fields of a book entry, as config lines.
fn desired_fields(connection: &Connection) -> Vec<String> {
    let mut fields = Vec::new();
    if !connection.host.is_empty() {
        fields.push(format!("HostName {}", connection.host));
    }
    if !connection.user.is_empty() {
        fields.push(format!("User {}", connection.user));
    }
    if let Some(port) = connection.port {
        fields.push(format!("Port {port}"));
    }
    if !connection.identity_file.is_empty() {
        fields.push(format!("IdentityFile {}", connection.identity_file));
    }
    if !connection.proxy_jump.is_empty() {
        fields.push(format!("ProxyJump {}", connection.proxy_jump));
    }
    fields
}

/// Config lines whose value differs from the book. Empty book fields are left
/// alone so an annotation does not delete settings it does not manage.
fn changed_fields(connection: &Connection, host: &SshHost) -> Vec<String> {
    let mut changed = Vec::new();
    if !connection.host.is_empty() && !host.host_name.eq_ignore_ascii_case(&connection.host) {
        changed.push(format!("HostName {}", connection.host));
    }
    if !connection.user.is_empty() && host.user != connection.user {
        changed.push(format!("User {}", connection.user));
    }
    if let Some(port) = connection.port {
        if host.port != Some(port) {
            changed.push(format!("Port {port}"));
        }
    }
    if !connection.identity_file.is_empty() && host.identity_file != connection.identity_file {
        changed.push(format!("IdentityFile {}", connection.identity_file));
    }
    if !connection.proxy_jump.is_empty() && host.proxy_jump != connection.proxy_jump {
        changed.push(format!("ProxyJump {}", connection.proxy_jump));
    }
    changed
}

/// Compare the book with hosts from ssh config.
///
/// Works both with `ssh_config::load_hosts()` (`&[String]`, aliases only) and
/// with `parse_hosts(text)` (`&[SshHost]`, field-aware). Every change is
/// selected unless it is `Unchanged`.
pub fn diff<H: SshHostInfo>(book: &Book, hosts: &[H]) -> Vec<SshSyncEntry> {
    let mut entries = Vec::new();
    for connection in &book.connections {
        if connection.kind == Kind::Manual && connection.protocol != Protocol::Ssh {
            continue;
        }
        let Some(alias) = alias_for(connection) else {
            continue;
        };
        let found = hosts
            .iter()
            .find(|host| host.alias().eq_ignore_ascii_case(&alias));
        let (change, fields) = match found {
            None => (SyncChange::Add, desired_fields(connection)),
            Some(host) => match host.fields() {
                Some(parsed) => {
                    let changed = changed_fields(connection, parsed);
                    if changed.is_empty() {
                        (SyncChange::Unchanged, changed)
                    } else {
                        (SyncChange::Update, changed)
                    }
                }
                None => (SyncChange::Unchanged, Vec::new()),
            },
        };
        entries.push(SshSyncEntry {
            alias,
            host: connection.host.clone(),
            change,
            fields,
            selected: change != SyncChange::Unchanged,
        });
    }
    for host in hosts {
        let alias = host.alias();
        if alias.is_empty()
            || book
                .connections
                .iter()
                .filter_map(alias_for)
                .any(|name| name.eq_ignore_ascii_case(alias))
            || entries
                .iter()
                .any(|entry| entry.alias.eq_ignore_ascii_case(alias))
        {
            continue;
        }
        entries.push(SshSyncEntry {
            alias: alias.to_string(),
            host: host
                .fields()
                .map_or_else(String::new, |parsed| parsed.host_name.clone()),
            change: SyncChange::Remove,
            fields: Vec::new(),
            selected: true,
        });
    }
    entries
}

#[derive(Clone)]
struct Block {
    aliases: Vec<String>,
    text: String,
}

fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

fn header_aliases(line: &str) -> Vec<String> {
    let words = shell_words::split(line.trim()).unwrap_or_else(|_| {
        line.split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    words
        .into_iter()
        .skip(1)
        .filter(|word| concrete_alias(word))
        .collect()
}

fn parse_config(original: &str) -> (String, Vec<Block>) {
    let lines = split_lines(original);
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.split_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("Host"))
        })
        .map(|(index, _)| index)
        .collect();
    let preamble_end = starts.first().copied().unwrap_or(lines.len());
    let preamble = lines[..preamble_end].concat();
    let blocks = starts
        .iter()
        .enumerate()
        .map(|(position, &start)| {
            let end = starts.get(position + 1).copied().unwrap_or(lines.len());
            let text = lines[start..end].concat();
            let aliases = split_lines(&text)
                .first()
                .map_or_else(Vec::new, |line| header_aliases(line));
            Block { aliases, text }
        })
        .collect();
    (preamble, blocks)
}

fn field_parts(field: &str, entry: &SshSyncEntry) -> (String, String) {
    let mut parts = field.splitn(2, char::is_whitespace);
    let key = parts.next().unwrap_or("").trim().to_string();
    let mut value = parts.next().unwrap_or("").trim().to_string();
    if value.is_empty() && key.eq_ignore_ascii_case("HostName") {
        value = entry.host.clone();
    }
    (key, value)
}

fn update_block(block: &mut Block, entry: &SshSyncEntry) {
    let mut lines = split_lines(&block.text);
    for field in &entry.fields {
        let (key, value) = field_parts(field, entry);
        if key.is_empty() || value.is_empty() {
            continue;
        }
        let mut replaced = false;
        for line in lines.iter_mut().skip(1) {
            let trimmed = line.trim_start();
            let word = trimmed
                .split(|c: char| c.is_whitespace() || c == '=')
                .next()
                .unwrap_or("");
            if !word.eq_ignore_ascii_case(&key) {
                continue;
            }
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];
            let ending = if line.ends_with('\n') { "\n" } else { "" };
            *line = format!("{indent}{key} {value}{ending}");
            replaced = true;
            break;
        }
        if !replaced {
            if let Some(last) = lines.last_mut() {
                if !last.ends_with('\n') {
                    last.push('\n');
                }
            }
            lines.push(format!("    {key} {value}\n"));
        }
    }
    block.text = lines.concat();
}

fn added_block(entry: &SshSyncEntry) -> Block {
    let mut text = format!("Host {}\n", entry.alias);
    for field in &entry.fields {
        let (key, value) = field_parts(field, entry);
        if !key.is_empty() && !value.is_empty() {
            text.push_str(&format!("    {key} {value}\n"));
        }
    }
    Block {
        aliases: vec![entry.alias.clone()],
        text,
    }
}

/// Apply the selected changes to ssh config text. Unselected entries and
/// unrelated lines, comments and blocks are preserved verbatim.
pub fn render(original: &str, entries: &[SshSyncEntry]) -> String {
    let (preamble, mut blocks) = parse_config(original);

    for entry in entries
        .iter()
        .filter(|entry| entry.selected && entry.change == SyncChange::Remove)
    {
        for block in &mut blocks {
            if !block
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&entry.alias))
            {
                continue;
            }
            if block.aliases.len() > 1 {
                block
                    .aliases
                    .retain(|alias| !alias.eq_ignore_ascii_case(&entry.alias));
                let mut lines = split_lines(&block.text);
                if let Some(first) = lines.first_mut() {
                    let ending = if first.ends_with('\n') { "\n" } else { "" };
                    *first = format!("Host {}{ending}", block.aliases.join(" "));
                }
                block.text = lines.concat();
            } else {
                block.aliases.clear();
                block.text.clear();
            }
        }
    }
    for entry in entries
        .iter()
        .filter(|entry| entry.selected && entry.change == SyncChange::Update)
    {
        if let Some(block) = blocks.iter_mut().find(|block| {
            block
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&entry.alias))
        }) {
            update_block(block, entry);
        }
    }
    for entry in entries
        .iter()
        .filter(|entry| entry.selected && entry.change == SyncChange::Add)
    {
        if blocks.iter().any(|block| {
            block
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&entry.alias))
        }) {
            continue;
        }
        blocks.push(added_block(entry));
    }

    let mut output = preamble;
    for block in blocks {
        if block.text.is_empty() {
            continue;
        }
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&block.text);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::connections::Connection;

    const CONFIG: &str = r#"# kept by hand
Host wiki
    HostName wiki.example
    User alice

Host router
    HostName router.example
    User admin
    Port 2200

Host oldbox
    HostName old.example
"#;

    fn manual(name: &str, host: &str) -> Connection {
        Connection {
            id: format!("id-{name}"),
            name: name.to_string(),
            host: host.to_string(),
            ..Connection::default()
        }
    }

    #[test]
    fn parse_hosts_keeps_concrete_hosts_and_their_fields() {
        let hosts = parse_hosts(CONFIG);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "wiki");
        assert_eq!(hosts[0].host_name, "wiki.example");
        assert_eq!(hosts[0].user, "alice");
        assert_eq!(hosts[1].port, Some(2200));
        assert_eq!(hosts[2].host_name, "old.example");
        let wildcard = parse_hosts("Host *\n    User root\nHost !bad\nHost good\n  Port=22\n");
        assert_eq!(wildcard.len(), 1);
        assert_eq!(wildcard[0].alias, "good");
        assert_eq!(wildcard[0].port, Some(22));
    }

    #[test]
    fn diff_reports_add_update_remove_and_unchanged() {
        let mut book = Book::default();
        book.connections.push(Connection::alias("wiki"));
        book.connections.push(Connection {
            user: "admin".into(),
            port: Some(2022),
            ..manual("router", "router.example")
        });
        book.connections.push(Connection {
            user: "bob".into(),
            port: Some(2222),
            ..manual("newhost", "new.example")
        });
        let entries = diff(&book, &parse_hosts(CONFIG));
        let change = |alias: &str| {
            entries
                .iter()
                .find(|entry| entry.alias == alias)
                .unwrap_or_else(|| panic!("missing {alias}"))
        };
        assert_eq!(change("wiki").change, SyncChange::Unchanged);
        assert!(!change("wiki").selected);
        assert_eq!(change("router").change, SyncChange::Update);
        assert_eq!(change("router").fields, vec!["Port 2022"]);
        assert!(change("router").selected);
        assert_eq!(change("newhost").change, SyncChange::Add);
        assert_eq!(
            change("newhost").fields,
            ["HostName new.example", "User bob", "Port 2222"]
        );
        assert_eq!(change("oldbox").change, SyncChange::Remove);
        assert!(change("oldbox").fields.is_empty());
    }

    #[test]
    fn diff_accepts_alias_only_host_lists() {
        let mut book = Book::default();
        book.connections.push(Connection {
            port: Some(2200),
            ..manual("router", "router.example")
        });
        let aliases = vec!["wiki".to_string(), "router".to_string()];
        let entries = diff(&book, &aliases);
        assert!(entries
            .iter()
            .any(|entry| entry.alias == "wiki" && entry.change == SyncChange::Remove));
        assert!(entries
            .iter()
            .any(|entry| entry.alias == "router" && entry.change == SyncChange::Unchanged));
    }

    #[test]
    fn render_applies_only_the_selected_changes() {
        let mut book = Book::default();
        book.connections.push(Connection::alias("wiki"));
        book.connections.push(Connection {
            user: "admin".into(),
            port: Some(2022),
            ..manual("router", "router.example")
        });
        book.connections.push(Connection {
            user: "bob".into(),
            port: Some(2222),
            ..manual("newhost", "new.example")
        });
        let mut entries = diff(&book, &parse_hosts(CONFIG));

        let partial: Vec<SshSyncEntry> = entries
            .iter()
            .map(|entry| SshSyncEntry {
                selected: entry.change == SyncChange::Update,
                ..entry.clone()
            })
            .collect();
        let unchanged_text = render(CONFIG, &partial);
        assert!(unchanged_text.contains("Port 2022"));
        assert!(!unchanged_text.contains("Port 2200"));
        assert!(unchanged_text.contains("Host oldbox"));
        assert!(!unchanged_text.contains("newhost"));

        let rendered = render(CONFIG, &entries);
        assert!(rendered.starts_with("# kept by hand\n"));
        assert!(rendered.contains("Host wiki\n    HostName wiki.example\n    User alice"));
        assert!(rendered.contains("Port 2022"));
        assert!(!rendered.contains("Port 2200"));
        assert!(rendered
            .contains("Host newhost\n    HostName new.example\n    User bob\n    Port 2222\n"));
        assert!(!rendered.contains("oldbox"));
        assert_eq!(
            render(&rendered, &entries),
            rendered,
            "applying twice is stable"
        );
        for entry in &mut entries {
            entry.selected = entry.change == SyncChange::Remove;
        }
        let removed = render(CONFIG, &entries);
        assert!(!removed.contains("oldbox"));
        assert!(removed.contains("Port 2200"), "unselected updates stay put");
    }

    #[test]
    fn render_removes_one_alias_of_a_shared_block() {
        let original = "Host one two\n    HostName shared.example\n    User alice\n";
        let entry = SshSyncEntry {
            alias: "one".into(),
            host: "shared.example".into(),
            change: SyncChange::Remove,
            fields: Vec::new(),
            selected: true,
        };
        assert_eq!(
            render(original, &[entry]),
            "Host two\n    HostName shared.example\n    User alice\n"
        );
    }
}
