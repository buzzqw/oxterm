//! Import connection entries from other tools, without touching secrets.
//!
//! All parsers are small and tolerant: the data comes from text files written
//! by other programs, so an unrecognised line is ignored rather than rejecting
//! the whole file. Entries are checked with `Connection::validate` before they
//! reach the book, and options that run programs are reported, never enabled.

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::Value;

use super::connections::{alias_id, new_id, Auth, Book, Connection, Kind, Protocol};
use super::ssh_sync;

/// Formats understood by `parse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportFormat {
    /// Remmina's `.remmina` INI files.
    Remmina,
    /// Remote Desktop Manager XML/JSON exports.
    RemoteDesktopManager,
    /// PuTTY registry exports (`.reg`).
    Putty,
    /// MobaXterm `.mxtsessions` XML exports.
    MobaXterm,
    /// OpenSSH `config` files.
    OpenSsh,
    /// Oxterm books and generic JSON entry lists.
    Json,
    /// Guess from the text and the file name, then try every parser.
    Auto,
}

/// Guess the format from the file name first, then from distinctive text.
pub fn detect_format(text: &str, filename: &str) -> ImportFormat {
    let name = filename.to_ascii_lowercase();
    let head = text.trim_start();
    let lowered = text.to_ascii_lowercase();
    if name.ends_with(".reg")
        || head.starts_with("regedit4")
        || head.starts_with("windows registry editor")
        || lowered.contains("simontatham\\putty")
    {
        return ImportFormat::Putty;
    }
    if name.contains("mobaxterm") || lowered.contains("<mobaxterm") || lowered.contains("<session ")
    {
        return ImportFormat::MobaXterm;
    }
    if name.ends_with(".rdm")
        || lowered.contains("remotedesktopmanager")
        || lowered.contains("<connection")
    {
        return ImportFormat::RemoteDesktopManager;
    }
    if name.ends_with(".remmina") || lowered.contains("[remmina") {
        return ImportFormat::Remmina;
    }
    if name == "config"
        || name.ends_with("ssh_config")
        || name.ends_with("sshconfig")
        || text
            .lines()
            .any(|line| line.trim_start().to_ascii_lowercase().starts_with("host "))
    {
        return ImportFormat::OpenSsh;
    }
    if name.ends_with(".json") || head.starts_with('{') || head.starts_with('[') {
        return ImportFormat::Json;
    }
    ImportFormat::Auto
}

/// Convert an exported file into entries. `Auto` uses `detect_format` and
/// falls back to trying each parser in turn.
pub fn parse(text: &str, format: ImportFormat) -> Result<Vec<Connection>, String> {
    match format {
        ImportFormat::Auto => {
            let detected = detect_format(text, "");
            if detected != ImportFormat::Auto {
                return parse(text, detected);
            }
            for candidate in [
                ImportFormat::Json,
                ImportFormat::Remmina,
                ImportFormat::RemoteDesktopManager,
                ImportFormat::Putty,
                ImportFormat::MobaXterm,
                ImportFormat::OpenSsh,
            ] {
                if let Ok(entries) = parse(text, candidate) {
                    if !entries.is_empty() {
                        return Ok(entries);
                    }
                }
            }
            Err("Cannot detect the import format".to_string())
        }
        ImportFormat::Remmina => Ok(parse_remmina(text)),
        ImportFormat::RemoteDesktopManager => {
            if text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
                parse_json(text)
            } else {
                Ok(parse_rdm_xml(text))
            }
        }
        ImportFormat::Putty => Ok(parse_putty(text)),
        ImportFormat::MobaXterm => Ok(parse_mobaxterm(text)),
        ImportFormat::OpenSsh => Ok(parse_openssh(text)),
        ImportFormat::Json => parse_json(text),
    }
}

/// The report shared by `preview` and `merge`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportPreview {
    /// Entries that are safe and free of name clashes.
    pub entries: Vec<Connection>,
    /// `(name, reason)` for everything that was left out.
    pub skipped: Vec<(String, String)>,
}

/// Filter entries before changing the book: invalid names, clashes and options
/// that act on the local machine are reported with a reason.
pub fn preview(book: &Book, entries: Vec<Connection>) -> ImportPreview {
    let mut report = ImportPreview::default();
    for entry in entries {
        let name = entry.name.clone();
        if name.trim().is_empty() {
            report.skipped.push((name, "missing name".to_string()));
            continue;
        }
        if let Err(error) = entry.validate() {
            report
                .skipped
                .push((name, format!("invalid entry: {error}")));
            continue;
        }
        if book
            .connections
            .iter()
            .any(|known| known.name.eq_ignore_ascii_case(&name))
            || report
                .entries
                .iter()
                .any(|known| known.name.eq_ignore_ascii_case(&name))
        {
            report
                .skipped
                .push((name, "the name is already in the book".to_string()));
            continue;
        }
        let risky = entry.local_execution_options();
        if !risky.is_empty() {
            report.skipped.push((
                name,
                format!("runs local commands or automation: {}", risky.join(", ")),
            ));
            continue;
        }
        report.entries.push(entry);
    }
    report
}

/// Add the accepted entries. Like `import_into`, this never replaces an entry
/// or a secret: every accepted entry gets a fresh id and no keyring flags.
pub fn merge(book: &mut Book, entries: Vec<Connection>) -> ImportPreview {
    let mut report = preview(book, entries);
    let accepted = report.entries.clone();
    for mut entry in accepted {
        let name = entry.name.clone();
        entry.id = match entry.kind {
            Kind::Alias => alias_id(&entry.name),
            Kind::Manual => new_id(),
        };
        entry.auth = Auth::Default;
        entry.totp = false;
        entry.template = None; // a template from another book would dangle
                               // Unknown fields are kept when Oxterm reads its own book, but a file
                               // from another manager can hide secrets or settings in them. Never
                               // carry them into our book: import copies known fields only.
        entry.unknown.clear();
        if book.find_by_id(&entry.id).is_some() {
            report
                .skipped
                .push((name, "the id is already in the book".to_string()));
            continue;
        }
        if let Err(error) = book.upsert(entry) {
            report.skipped.push((name, error));
        }
    }
    report
}

// ── Shared parsing helpers ────────────────────────────────────

fn connection(name: &str) -> Connection {
    Connection {
        id: new_id(),
        name: name.trim().to_string(),
        kind: Kind::Manual,
        ..Connection::default()
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.chars().next().unwrap_or(' ');
        let last = value.chars().last().unwrap_or(' ');
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn parse_port(value: &str) -> Option<u16> {
    value.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

/// Split `host[:port]`, keeping bracketed IPv6 addresses intact.
fn split_host_port(server: &str) -> (String, Option<u16>) {
    let server = server.trim();
    if let Some(rest) = server.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            return (
                format!("[{host}]"),
                tail.strip_prefix(':').and_then(parse_port),
            );
        }
    }
    if let Some((host, port)) = server.rsplit_once(':') {
        if let Some(port) = parse_port(port) {
            return (host.trim().to_string(), Some(port));
        }
    }
    (server.to_string(), None)
}

fn protocol(text: &str) -> Protocol {
    match text.trim().to_ascii_lowercase().as_str() {
        "mosh" => Protocol::Mosh,
        "telnet" => Protocol::Telnet,
        "serial" | "com" => Protocol::Serial,
        "sftp" => Protocol::Sftp,
        "ftp" => Protocol::Ftp,
        "docker" => Protocol::Docker,
        "kube" | "kubernetes" | "kubectl" => Protocol::Kubectl,
        "aws" | "ssm" | "aws_ssm" => Protocol::AwsSsm,
        "command" | "cmd" => Protocol::Command,
        "rdp" => Protocol::Rdp,
        "vnc" => Protocol::Vnc,
        "spice" => Protocol::Spice,
        _ => Protocol::Ssh,
    }
}

fn parse_ini(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut sections: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut current: Option<(String, BTreeMap<String, String>)> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with(';')
            || line.starts_with('#')
            || line.starts_with("//")
        {
            continue;
        }
        if let Some(title) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if let Some(previous) = current.take() {
                sections.push(previous);
            }
            current = Some((title.trim().to_string(), BTreeMap::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if current.is_none() {
            current = Some((String::new(), BTreeMap::new()));
        }
        let map = &mut current.as_mut().unwrap().1;
        map.insert(key.trim().to_ascii_lowercase(), unquote(value));
    }
    if let Some(last) = current {
        sections.push(last);
    }
    sections
}

// ── Remmina ───────────────────────────────────────────────────

fn parse_remmina(text: &str) -> Vec<Connection> {
    let mut entries = Vec::new();
    for (section, fields) in parse_ini(text) {
        let name = fields
            .get("name")
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| section.clone());
        if name.is_empty() {
            continue;
        }
        let mut entry = connection(&name);
        entry.protocol = protocol(fields.get("protocol").map_or("ssh", String::as_str));
        let (host, port) = split_host_port(fields.get("server").map_or("", String::as_str));
        entry.host = host;
        entry.port = port;
        entry.user = fields.get("username").cloned().unwrap_or_default();
        entry.group = fields
            .get("group")
            .map(|group| group.replace('\\', "/"))
            .unwrap_or_default();
        entry.proxy_jump = fields.get("ssh_server").cloned().unwrap_or_default();
        if let Some(command) = fields.get("precommand") {
            entry.pre_command = command.clone();
        }
        if let Some(domain) = fields.get("domain") {
            entry.rdp_domain = domain.clone();
        }
        entries.push(entry);
    }
    entries
}

// ── Minimal XML reader (Remote Desktop Manager, MobaXterm) ─────

#[derive(Debug, Clone, Default)]
struct XmlNode {
    attrs: BTreeMap<String, String>,
    body: String,
}

impl XmlNode {
    fn attr(&self, key: &str) -> Option<String> {
        self.attrs.get(&key.to_ascii_lowercase()).cloned()
    }
}

fn local_name(raw: &str) -> &str {
    raw.rsplit(':').next().unwrap_or(raw)
}

fn parse_attrs(content: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let Some(position) = content.find(char::is_whitespace) else {
        return attrs;
    };
    let chars: Vec<char> = content[position..].chars().collect();
    let mut index = 0;
    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let start = index;
        while index < chars.len() && !chars[index].is_whitespace() && chars[index] != '=' {
            index += 1;
        }
        let key: String = chars[start..index].iter().collect();
        if key.is_empty() {
            break;
        }
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index >= chars.len() || chars[index] != '=' {
            continue;
        }
        index += 1;
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if index < chars.len() && (chars[index] == '"' || chars[index] == '\'') {
            let quote = chars[index];
            index += 1;
            while index < chars.len() && chars[index] != quote {
                value.push(chars[index]);
                index += 1;
            }
            index += 1;
        } else {
            while index < chars.len() && !chars[index].is_whitespace() {
                value.push(chars[index]);
                index += 1;
            }
        }
        attrs.insert(key.to_ascii_lowercase(), value);
    }
    attrs
}

/// Find the body of a tag, accounting for nested tags of the same name.
fn find_closing<'a>(text: &'a str, from: usize, tag: &str) -> Option<(&'a str, usize)> {
    let mut cursor = from;
    let mut depth = 1usize;
    while let Some(relative) = text[cursor..].find('<') {
        let start = cursor + relative;
        let end = start + text[start..].find('>')?;
        let raw = text[start + 1..end].trim();
        if raw.starts_with('!') || raw.starts_with('?') {
            cursor = end + 1;
            continue;
        }
        let closing = raw.starts_with('/');
        let self_closing = raw.ends_with('/');
        let content = raw.trim_start_matches('/').trim_end_matches('/').trim();
        let name = local_name(content.split_whitespace().next().unwrap_or(""));
        if name.eq_ignore_ascii_case(tag) && !self_closing {
            if closing {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[from..start], end + 1));
                }
            } else {
                depth += 1;
            }
        }
        cursor = end + 1;
    }
    None
}

fn xml_nodes(text: &str, tag: &str) -> Vec<XmlNode> {
    let mut nodes = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find('<') {
        let start = cursor + relative;
        let Some(end_relative) = text[start..].find('>') else {
            break;
        };
        let end = start + end_relative;
        let raw = text[start + 1..end].trim();
        if raw.starts_with('/') || raw.starts_with('!') || raw.starts_with('?') {
            cursor = end + 1;
            continue;
        }
        let self_closing = raw.ends_with('/');
        let content = raw.trim_end_matches('/').trim();
        let name = local_name(content.split_whitespace().next().unwrap_or(""));
        if name.eq_ignore_ascii_case(tag) {
            let attrs = parse_attrs(content);
            if self_closing {
                nodes.push(XmlNode {
                    attrs,
                    body: String::new(),
                });
                cursor = end + 1;
            } else if let Some((body, after)) = find_closing(text, end + 1, tag) {
                nodes.push(XmlNode {
                    attrs,
                    body: body.to_string(),
                });
                cursor = after;
            } else {
                cursor = end + 1;
            }
        } else {
            cursor = end + 1;
        }
    }
    nodes
}

fn xml_texts(text: &str, tag: &str) -> Vec<String> {
    let Ok(pattern) = Regex::new(&format!(r"(?is)<{tag}(?:\s[^>]*)?>(.*?)</{tag}\s*>")) else {
        return Vec::new();
    };
    pattern
        .captures_iter(text)
        .map(|captures| {
            captures[1]
                .trim()
                .trim_start_matches("<![CDATA[")
                .trim_end_matches("]]>")
                .trim()
                .to_string()
        })
        .filter(|text| !text.is_empty())
        .collect()
}

fn node_value(node: &XmlNode, key: &str) -> Option<String> {
    node.attr(key)
        .or_else(|| xml_texts(&node.body, key).into_iter().next())
}

fn parse_rdm_xml(text: &str) -> Vec<Connection> {
    let mut entries = Vec::new();
    for node in xml_nodes(text, "Connection") {
        let name = node_value(&node, "Name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let mut entry = connection(&name);
        entry.protocol = protocol(&node_value(&node, "Protocol").unwrap_or_default());
        let (host, port) = split_host_port(&node_value(&node, "Host").unwrap_or_default());
        entry.host = host;
        entry.port =
            port.or_else(|| node_value(&node, "Port").and_then(|value| parse_port(&value)));
        entry.user = node_value(&node, "Username").unwrap_or_default();
        entry.group = node_value(&node, "Group").unwrap_or_default();
        entry.notes = node_value(&node, "Description").unwrap_or_default();
        entries.push(entry);
    }
    entries
}

// ── PuTTY registry exports ────────────────────────────────────

fn reg_value(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix("dword:") {
        return u32::from_str_radix(hex.trim(), 16)
            .map(|number| number.to_string())
            .unwrap_or_default();
    }
    unquote(raw).replace("\\\"", "\"").replace("\\\\", "\\")
}

fn parse_putty(text: &str) -> Vec<Connection> {
    let section = Regex::new(r"(?m)^\s*\[([^\]]+)\]\s*$").unwrap();
    let key_value = Regex::new(r#"^\s*"([^"]+)"\s*=\s*(.*?)\s*$"#).unwrap();
    let mut sessions: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut current: Option<(String, BTreeMap<String, String>)> = None;
    for line in text.lines() {
        if let Some(captures) = section.captures(line) {
            if let Some(previous) = current.take() {
                sessions.push(previous);
            }
            current = Some((captures[1].trim().to_string(), BTreeMap::new()));
            continue;
        }
        if let Some(captures) = key_value.captures(line) {
            if let Some((_, fields)) = current.as_mut() {
                fields.insert(captures[1].to_ascii_lowercase(), reg_value(&captures[2]));
            }
        }
    }
    if let Some(last) = current {
        sessions.push(last);
    }
    let mut entries = Vec::new();
    for (path, fields) in sessions {
        if !path.to_ascii_lowercase().contains("sessions") {
            continue;
        }
        let name = path.rsplit('\\').next().unwrap_or("").replace("%20", " ");
        if name.is_empty() {
            continue;
        }
        let mut entry = connection(&name);
        entry.protocol = protocol(
            &fields
                .get("protocol")
                .map_or("ssh".to_string(), |v| v.clone()),
        );
        entry.host = fields.get("hostname").cloned().unwrap_or_default();
        entry.user = fields.get("username").cloned().unwrap_or_default();
        entry.port = fields.get("portnumber").and_then(|value| parse_port(value));
        entries.push(entry);
    }
    entries
}

// ── MobaXterm XML ─────────────────────────────────────────────

fn parse_mobaxterm(text: &str) -> Vec<Connection> {
    let mut entries = Vec::new();
    for node in xml_nodes(text, "Session") {
        let name = node_value(&node, "Name").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let mut entry = connection(&name);
        entry.protocol = protocol(&node_value(&node, "Protocol").unwrap_or_default());
        entry.host = node_value(&node, "Host").unwrap_or_default();
        entry.user = node_value(&node, "Username").unwrap_or_default();
        entry.port = node_value(&node, "Port").and_then(|value| parse_port(&value));
        entry.group = node_value(&node, "Group").unwrap_or_default();
        entries.push(entry);
    }
    entries
}

// ── OpenSSH config ────────────────────────────────────────────

fn parse_openssh(text: &str) -> Vec<Connection> {
    ssh_sync::parse_hosts(text)
        .into_iter()
        .map(|host| Connection {
            id: alias_id(&host.alias),
            kind: Kind::Alias,
            name: host.alias,
            host: host.host_name,
            user: host.user,
            port: host.port,
            identity_file: host.identity_file,
            proxy_jump: host.proxy_jump,
            ..Connection::default()
        })
        .collect()
}

// ── JSON ──────────────────────────────────────────────────────

fn json_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn json_get(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = object.get(*key).and_then(json_string) {
            return value;
        }
    }
    String::new()
}

/// A value encrypted by another manager is not usable here: never copy an
/// `ENC:...` token into a field that Oxterm will use as data (a user name, for
/// instance). The dedicated PCM importer can unlock those values.
fn json_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    let value = json_get(object, keys);
    if value.starts_with("ENC:") {
        String::new()
    } else {
        value
    }
}

/// Notes are checked as single-line text. Other managers allow newlines and
/// tabs there, so keep the words but drop the control characters.
fn json_notes(object: &serde_json::Map<String, Value>) -> String {
    json_field(object, &["notes", "Notes", "description"])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Some managers keep the book as `{"<display name>": {...}}`: the name is the
/// key, not a field of the entry. Only treat objects that look like
/// connections as entries, so a map of unrelated settings is not imported.
fn json_keyed_entries(object: &serde_json::Map<String, Value>) -> Vec<(String, Value)> {
    object
        .iter()
        .filter_map(|(name, value)| {
            let entry = value.as_object()?;
            let looks_like_connection = [
                "host", "Host", "hostname", "HostName", "server", "protocol", "Protocol", "port",
                "Port", "group", "Group",
            ]
            .iter()
            .any(|key| entry.contains_key(*key));
            if name.trim().is_empty() || !looks_like_connection {
                return None;
            }
            Some((name.clone(), value.clone()))
        })
        .collect()
}

fn parse_json(text: &str) -> Result<Vec<Connection>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    // The first item is a name to fall back on when the entry has no name of
    // its own, as in a book keyed by display name.
    let values: Vec<(String, Value)> = match value {
        Value::Array(items) => items
            .into_iter()
            .map(|value| (String::new(), value))
            .collect(),
        Value::Object(object) => {
            let nested = object
                .get("connections")
                .or_else(|| object.get("Connections"))
                .or_else(|| object.get("Entries"));
            match nested {
                Some(Value::Array(items)) => items
                    .iter()
                    .cloned()
                    .map(|value| (String::new(), value))
                    .collect(),
                Some(Value::Object(entries)) => json_keyed_entries(entries),
                _ if object.contains_key("name") || object.contains_key("host") => {
                    vec![(String::new(), Value::Object(object))]
                }
                _ => json_keyed_entries(&object),
            }
        }
        _ => Vec::new(),
    };
    let mut entries = Vec::new();
    for (key, value) in values {
        // A full Oxterm entry needs no translation.
        if let Ok(entry) = serde_json::from_value::<Connection>(value.clone()) {
            if !entry.name.is_empty() {
                entries.push(entry);
                continue;
            }
        }
        let Value::Object(object) = value else {
            continue;
        };
        let name = json_field(&object, &["name", "Name", "title", "alias", "Alias"]);
        let name = if name.is_empty() { key } else { name };
        if name.is_empty() {
            continue;
        }
        let mut entry = connection(&name);
        entry.host = json_field(
            &object,
            &["host", "Host", "hostname", "HostName", "server", "Server"],
        );
        entry.user = json_field(&object, &["user", "User", "username", "UserName", "login"]);
        let port = json_field(&object, &["port", "Port"]);
        entry.port = parse_port(&port);
        let raw_protocol = json_field(&object, &["protocol", "Protocol", "type", "Type"]);
        entry.protocol = protocol(&raw_protocol);
        if raw_protocol.eq_ignore_ascii_case("file_transfer") {
            entry.protocol = protocol(&json_field(&object, &["ft_protocol", "file_protocol"]));
        }
        entry.group = json_field(&object, &["group", "Group", "folder"]);
        entry.notes = json_notes(&object);
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_format_uses_the_name_and_the_text() {
        assert_eq!(detect_format("", "sessions.reg"), ImportFormat::Putty);
        assert_eq!(
            detect_format("[remmina]\nname=a\n", ""),
            ImportFormat::Remmina
        );
        assert_eq!(
            detect_format("Host web\n  HostName h\n", "config"),
            ImportFormat::OpenSsh
        );
        assert_eq!(
            detect_format("{\"connections\":[]}", "in.json"),
            ImportFormat::Json
        );
        assert_eq!(
            detect_format("<MobaXterm><Session/></MobaXterm>", ""),
            ImportFormat::MobaXterm
        );
        assert_eq!(
            detect_format("<Root><Connection></Connection></Root>", ""),
            ImportFormat::RemoteDesktopManager
        );
        assert_eq!(
            detect_format("nothing special", "mystery.txt"),
            ImportFormat::Auto
        );
    }

    #[test]
    fn remmina_sections_become_entries() {
        let text = "[remmina]\n\
name=Web desktop\n\
protocol=RDP\n\
server=10.0.0.5:3389\n\
username=admin\n\
group=Customers/ACME\n\
domain=EXAMPLE\n\
\n\
[remmina1]\n\
name=Shell\n\
protocol=SSH\n\
server=shell.example:2222\n\
username=root\n";
        let entries = parse(text, ImportFormat::Remmina).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Web desktop");
        assert_eq!(entries[0].protocol, Protocol::Rdp);
        assert_eq!(entries[0].host, "10.0.0.5");
        assert_eq!(entries[0].port, Some(3389));
        assert_eq!(entries[0].user, "admin");
        assert_eq!(entries[0].group, "Customers/ACME");
        assert_eq!(entries[0].rdp_domain, "EXAMPLE");
        assert_eq!(entries[1].protocol, Protocol::Ssh);
        assert_eq!(entries[1].port, Some(2222));
    }

    #[test]
    fn remote_desktop_manager_xml_becomes_entries() {
        let text = r#"<?xml version="1.0"?>
<Root>
  <Connection>
    <Name>Prod DB</Name>
    <Protocol>SSH</Protocol>
    <Host>db.example</Host>
    <Port>2222</Port>
    <Username>dba</Username>
    <Group>Prod</Group>
  </Connection>
  <Connection Name="Jump" Protocol="SSH" Host="jump.example" Username="ops" />
</Root>"#;
        let entries = parse(text, ImportFormat::RemoteDesktopManager).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Prod DB");
        assert_eq!(entries[0].host, "db.example");
        assert_eq!(entries[0].port, Some(2222));
        assert_eq!(entries[0].user, "dba");
        assert_eq!(entries[0].group, "Prod");
        assert_eq!(entries[1].name, "Jump");
        assert_eq!(entries[1].host, "jump.example");
        assert_eq!(entries[1].user, "ops");
    }

    #[test]
    fn putty_registry_export_decodes_dwords() {
        let text = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\web%20prod]
"HostName"="web.example"
"UserName"="deploy"
"PortNumber"=dword:00002258
"Protocol"="ssh"

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\legacy]
"HostName"="legacy.example"
"PortNumber"=dword:00000017
"Protocol"="telnet"
"Protocol "=-"#;
        let entries = parse(text, ImportFormat::Putty).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "web prod");
        assert_eq!(entries[0].host, "web.example");
        assert_eq!(entries[0].user, "deploy");
        assert_eq!(entries[0].port, Some(8792));
        assert_eq!(entries[1].port, Some(23));
        assert_eq!(entries[1].protocol, Protocol::Telnet);
    }

    #[test]
    fn mobaxterm_sessions_become_entries() {
        let text = r#"<MobaXterm>
  <Session Name="Prod" Protocol="SSH" Host="10.1.0.2" Username="root" Port="2222" Group="Servers"/>
  <Session Name="Console" Protocol="VNC" Host="10.1.0.3" Port="5900"/>
</MobaXterm>"#;
        let entries = parse(text, ImportFormat::MobaXterm).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Prod");
        assert_eq!(entries[0].host, "10.1.0.2");
        assert_eq!(entries[0].user, "root");
        assert_eq!(entries[0].port, Some(2222));
        assert_eq!(entries[0].group, "Servers");
        assert_eq!(entries[1].protocol, Protocol::Vnc);
        assert_eq!(entries[1].port, Some(5900));
    }

    #[test]
    fn openssh_config_entries_keep_their_alias() {
        let text = "Host *\n    User root\n\
Host web prod\n\
    HostName web.example\n\
    User deploy\n\
    Port 2222\n\
    IdentityFile ~/.ssh/web\n";
        let entries = parse(text, ImportFormat::OpenSsh).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, Kind::Alias);
        assert_eq!(entries[0].name, "web");
        assert_eq!(entries[0].host, "web.example");
        assert_eq!(entries[0].user, "deploy");
        assert_eq!(entries[0].port, Some(2222));
        assert_eq!(entries[0].identity_file, "~/.ssh/web");
        assert_eq!(entries[1].name, "prod");
    }

    #[test]
    fn json_accepts_generic_entries_and_oxterm_books() {
        let text = r#"{"connections":[
            {"name":"web","host":"web.example","user":"deploy","port":"2201","protocol":"ssh","group":"Prod"},
            {"name":"desk","HostName":"desk.example","Protocol":"RDP","Port":3389}
        ]}"#;
        let entries = parse(text, ImportFormat::Json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].host, "web.example");
        assert_eq!(entries[0].port, Some(2201));
        assert_eq!(entries[0].group, "Prod");
        assert_eq!(entries[1].protocol, Protocol::Rdp);
        assert_eq!(entries[1].port, Some(3389));

        let book =
            r#"{"version":1,"connections":[{"id":"x","name":"full","host":"h","favorite":true}]}"#;
        let entries = parse(book, ImportFormat::Auto).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].favorite);
        assert!(parse("not json", ImportFormat::Json).is_err());
    }

    #[test]
    fn json_accepts_books_keyed_by_display_name() {
        // Python Connection Manager stores the display name as the key and
        // encrypts credentials with an `ENC:` prefix. Those tokens are not
        // user names and must never reach Oxterm's fields.
        let text = r#"{
            "web prod": {
                "protocol": "ssh", "group": "Prod", "host": "web.example",
                "port": "2223", "user": "ENC:unusable", "password": "ENC:secret",
                "notes": "first step\t\nsecond step", "x11": true,
                "pre_cmd": "touch /tmp/unsafe", "macros": [{"nome":"m","cmd":"id"}]
            },
            "transfer": {"protocol": "file_transfer", "ft_protocol": "SFTP", "host": "ftp.example"},
            "unnamed template": {"protocol": "ssh", "group": "Templates"},
            "": {"protocol": "ssh", "host": "empty.example"}
        }"#;
        let entries = parse(text, ImportFormat::Json).unwrap();
        assert_eq!(entries.len(), 3);
        let find = |name: &str| entries.iter().find(|entry| entry.name == name).unwrap();
        let web = find("web prod");
        assert_eq!(web.host, "web.example");
        assert_eq!(web.port, Some(2223));
        assert_eq!(web.user, "");
        assert_eq!(web.notes, "first step second step");
        // Nothing that runs a local command was carried over.
        assert!(web.local_execution_options().is_empty());
        assert_eq!(find("transfer").protocol, Protocol::Sftp);
        // An entry with no host is invalid, but still parsed so that `preview`
        // can report why it was skipped rather than dropping it silently.
        assert_eq!(find("unnamed template").host, "");
    }

    #[test]
    fn importing_never_copies_secrets_or_program_options() {
        // A full Oxterm-looking entry may carry fields from another manager in
        // its forward-compatible map. They are data from an untrusted file,
        // not settings to copy into our book.
        let text = r#"{"connections":[
            {"id":"old","name":"risky","host":"h","password":"secret",
             "pre_command":"touch /tmp/unsafe","extra_args":["-oProxyCommand=evil"]},
            {"id":"other","name":"clean","host":"h2","auth":"secret_service",
             "totp":true,"template":"gone","password":"secret","totp_secret":"JBSWY3DPEHPK3PXP"}
        ]}"#;
        let entries = parse(text, ImportFormat::Json).unwrap();
        let mut book = Book::default();
        let report = merge(&mut book, entries);
        assert_eq!(report.entries.len(), 1);
        assert!(report
            .skipped
            .iter()
            .any(|(name, reason)| name == "risky" && reason.contains("runs local commands")));
        let saved = &book.connections[0];
        assert_ne!(saved.id, "other");
        assert_eq!(saved.auth, Auth::Default);
        assert!(!saved.totp);
        assert!(saved.template.is_none());
        assert!(saved.unknown.is_empty());
        let serialized = serde_json::to_string(saved).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("JBSWY3"));
    }

    #[test]
    fn preview_skips_duplicates_invalid_and_risky_entries() {
        let mut book = Book::default();
        book.connections.push(Connection {
            name: "Web".into(),
            host: "old.example".into(),
            ..Connection::default()
        });
        let entries = vec![
            Connection {
                name: "web".into(),
                host: "new.example".into(),
                ..Connection::default()
            },
            Connection {
                name: "".into(),
                ..Connection::default()
            },
            Connection {
                name: "bad host".into(),
                host: "-oX".into(),
                ..Connection::default()
            },
            Connection {
                name: "risky".into(),
                host: "h".into(),
                pre_command: "vpn up".into(),
                ..Connection::default()
            },
            Connection {
                name: "good".into(),
                host: "good.example".into(),
                ..Connection::default()
            },
        ];
        let report = preview(&book, entries);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].name, "good");
        assert_eq!(report.skipped.len(), 4);
        assert!(report.skipped[0].1.contains("already in the book"));
        assert!(report.skipped[3].1.contains("vpn up"));
    }

    #[test]
    fn merge_only_adds_and_never_touches_existing_entries() {
        let mut book = Book::default();
        let mut mine = Connection {
            name: "web".into(),
            host: "keep.example".into(),
            auth: Auth::SecretService,
            totp: true,
            ..Connection::default()
        };
        mine.id = "id-mine".into();
        book.connections.push(mine.clone());

        let incoming = vec![
            Connection {
                id: "id-mine".into(),
                name: "web".into(),
                host: "evil.example".into(),
                ..Connection::default()
            },
            Connection {
                name: "new".into(),
                host: "new.example".into(),
                auth: Auth::SecretService,
                totp: true,
                template: Some("web".into()),
                ..Connection::default()
            },
        ];
        let report = merge(&mut book, incoming);
        assert_eq!(book.connections[0], mine, "the original was not touched");
        assert_eq!(book.connections.len(), 2);
        let added = &book.connections[1];
        assert_eq!(added.host, "new.example");
        assert_ne!(added.id, "id-mine");
        assert_eq!(added.auth, Auth::Default);
        assert!(!added.totp);
        assert_eq!(added.template, None);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn risky_options_from_remmina_are_reported_not_imported() {
        let text = "[remmina]\nname=scripted\nprotocol=SSH\nserver=h\nprecommand=/usr/bin/vpn up\n";
        let entries = parse(text, ImportFormat::Remmina).unwrap();
        let mut book = Book::default();
        let report = merge(&mut book, entries);
        assert!(book.connections.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].1.contains("pre-connect command"));
    }
}
