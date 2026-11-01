//! Concrete host aliases from OpenSSH configuration files.
//!
//! This is display-free so the parser can be tested without GTK. Only aliases
//! are collected: `ssh <alias>` lets OpenSSH itself apply `HostName`, `User`,
//! `Port`, `ProxyJump`, `Include` and everything else. Wildcard and negated
//! `Host` patterns are skipped because they are defaults, not destinations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Path to the user's OpenSSH client configuration.
pub fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".ssh")
        .join("config")
}

/// Concrete aliases in `~/.ssh/config`, sorted.
pub fn load_hosts() -> Vec<String> {
    hosts_from_config(&default_config_path())
}

/// Concrete aliases in one configuration file, including `Include` files.
pub fn hosts_from_config(path: &Path) -> Vec<String> {
    let mut hosts = BTreeSet::new();
    let mut visited = BTreeSet::new();
    parse_file(path, &mut visited, &mut hosts, 0);
    hosts.into_iter().collect()
}

fn parse_file(
    path: &Path,
    visited: &mut BTreeSet<PathBuf>,
    hosts: &mut BTreeSet<String>,
    depth: usize,
) {
    // Include chains are normally shallow; the limit and the canonical-path
    // set make a mistaken recursive Include harmless.
    if depth > 8 {
        return;
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim_start();
        // OpenSSH only treats a line starting with '#' as a comment.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Ok(mut fields) = shell_words::split(line) else {
            continue;
        };
        // OpenSSH accepts both "Keyword value" and "Keyword=value".
        if let Some(first) = fields.first_mut() {
            if let Some((keyword, value)) = first.split_once('=') {
                let keyword = keyword.to_string();
                let value = value.to_string();
                *first = keyword;
                fields.insert(1, value);
            }
        }
        let Some(keyword) = fields.first() else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                for pattern in &fields[1..] {
                    if is_concrete_host(pattern) {
                        hosts.insert(pattern.clone());
                    }
                }
            }
            "include" => {
                for pattern in &fields[1..] {
                    for included in expand_include(pattern) {
                        parse_file(&included, visited, hosts, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_concrete_host(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.starts_with('!')
        && !pattern.starts_with('-')
        && !pattern.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}

fn expand_include(pattern: &str) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut pattern = pattern
        .replace("%d", &home.to_string_lossy())
        .replace("%u", &std::env::var("USER").unwrap_or_default());
    if pattern == "~" {
        pattern = home.to_string_lossy().into_owned();
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        pattern = home.join(rest).to_string_lossy().into_owned();
    }
    let path = PathBuf::from(&pattern);
    let path = if path.is_absolute() {
        path
    } else {
        home.join(".ssh").join(path)
    };
    expand_glob(&path)
}

fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
    let text = pattern.to_string_lossy();
    if !text.contains('*') && !text.contains('?') {
        return pattern
            .exists()
            .then(|| pattern.to_path_buf())
            .into_iter()
            .collect();
    }
    let mut current = if text.starts_with('/') {
        vec![PathBuf::from("/")]
    } else {
        vec![PathBuf::from(".")]
    };
    for part in text.split('/').filter(|part| !part.is_empty()) {
        let mut next = Vec::new();
        if part.contains('*') || part.contains('?') {
            for base in &current {
                let Ok(entries) = std::fs::read_dir(base) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if wildcard_match(part, &name) {
                        next.push(base.join(name.as_ref()));
                    }
                }
            }
        } else {
            for base in &current {
                next.push(base.join(part));
            }
        }
        current = next;
    }
    current.into_iter().filter(|path| path.exists()).collect()
}

/// `ssh` options that consume the following argument.
fn option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-B" | "-b"
            | "-c"
            | "-D"
            | "-E"
            | "-e"
            | "-F"
            | "-I"
            | "-i"
            | "-J"
            | "-L"
            | "-l"
            | "-m"
            | "-O"
            | "-o"
            | "-P"
            | "-p"
            | "-Q"
            | "-R"
            | "-S"
            | "-W"
            | "-w"
    )
}

/// First non-option argument, i.e. the `[user@]host` destination.
pub fn ssh_destination(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "--" {
            return args.get(index + 1).cloned();
        }
        if argument.starts_with('-') {
            let flag = argument.get(..2).unwrap_or(argument);
            if option_takes_value(flag) && argument.len() == 2 {
                index += 1;
            }
            index += 1;
            continue;
        }
        return Some(argument.clone());
    }
    None
}

/// Strip a `user@` prefix from a destination.
pub fn ssh_host_name(destination: &str) -> &str {
    destination.rsplit('@').next().unwrap_or(destination)
}

/// Whether `host` matches the configured production-host regex.
///
/// An invalid or empty pattern simply disables the badge.
pub fn alert_host(patterns: &str, host: &str) -> bool {
    if patterns.trim().is_empty() {
        return false;
    }
    regex::Regex::new(patterns)
        .map(|pattern| pattern.is_match(ssh_host_name(host)))
        .unwrap_or(false)
}

/// Command-line `-L`, `-R` and `-D` forwards, formatted for display.
///
/// This deliberately reports what was passed to ssh, not the live health of
/// each tunnel: `ssh -O check` validates the master connection, not the
/// individual ports.
pub fn ssh_forwardings(args: &[String]) -> Vec<String> {
    let mut forwards = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "--" {
            break;
        }
        let flag = argument.get(..2).unwrap_or(argument);
        if matches!(flag, "-L" | "-R" | "-D") {
            let attached = (argument.len() > 2).then(|| argument[2..].to_string());
            let value = attached.or_else(|| args.get(index + 1).cloned());
            if let Some(value) = value {
                forwards.push(format!("{} {}", flag, value));
                if argument.len() == 2 {
                    index += 1;
                }
            }
        } else if argument.starts_with('-') {
            if option_takes_value(flag) && argument.len() == 2 {
                index += 1;
            }
        } else {
            break;
        }
        index += 1;
    }
    forwards
}

/// Whether the effective configuration enables agent forwarding.
pub fn agent_forwarding_enabled(effective_config: &str) -> bool {
    effective_config
        .lines()
        .any(|line| line.trim() == "forwardagent yes")
}

/// Extra `ssh` options that mirror the shell-integration wrapper.
///
/// `ssh -G <host>` prints the effective configuration; when the user has not
/// configured connection sharing or keepalives, add the same defaults the
/// shell wrapper uses. This keeps host-picker tabs as fast as tabs where `ssh`
/// was typed at an integrated prompt.
pub fn sharing_options(effective_config: &str, socket_dir: &Path) -> Vec<String> {
    let value = |key: &str| {
        effective_config.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(str::trim)
        })
    };
    let mut options = Vec::new();
    if value("controlpath").map_or(true, |path| path == "none") {
        options.extend([
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", socket_dir.join("cm-%C").display()),
            "-o".to_string(),
            "ControlPersist=10m".to_string(),
        ]);
    }
    if value("serveraliveinterval") == Some("0") {
        options.extend([
            "-o".to_string(),
            "ServerAliveInterval=30".to_string(),
            "-o".to_string(),
            "ServerAliveCountMax=4".to_string(),
        ]);
    }
    options
}

/// Standard base64, used to paste the shell-integration script through a
/// single-line SSH command without here-docs or local quoting surprises.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(b0 >> 2) as usize] as char);
        encoded.push(ALPHABET[((b0 & 0x03) << 4 | b1 >> 4) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[((b1 & 0x0f) << 2 | b2 >> 6) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut match_at) = (None, 0usize);
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            pi += 1;
            match_at = ti;
        } else if let Some(star_index) = star {
            pi = star_index + 1;
            match_at += 1;
            ti = match_at;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("oxterm-sshconfig-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collects_concrete_hosts_and_includes() {
        let dir = temp_dir();
        let nested = dir.join("conf.d");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("more.conf"),
            "Host delta\n  HostName delta.example\n# Host commented\n",
        )
        .unwrap();
        let config = dir.join("config");
        // OpenSSH resolves a relative Include against ~/.ssh, not the file
        // that contains it, so the test uses an absolute pattern.
        let include = format!(
            "# comment\n\
             Host alpha beta\n  HostName example.test\n\
             Host *.example\n\
             Host !blocked gamma\n\
             Include {}/conf.d/*.conf\n\
             Host epsilon\n\
             Host=zeta\n",
            dir.display()
        );
        std::fs::write(&config, include).unwrap();
        assert_eq!(
            hosts_from_config(&config),
            ["alpha", "beta", "delta", "epsilon", "gamma", "zeta"]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wildcard_matching_is_anchored() {
        assert!(wildcard_match("*.conf", "base.conf"));
        assert!(!wildcard_match("*.conf", "base.conf.bak"));
        assert!(wildcard_match("host?", "host1"));
        assert!(wildcard_match("*", "anything"));
        assert!(!wildcard_match("a*b", "abx"));
    }

    #[test]
    fn finds_the_destination_past_ssh_options() {
        let args = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert_eq!(
            ssh_destination(&args(&["-p", "2222", "bob@example.test", "uptime"])),
            Some("bob@example.test".to_string())
        );
        assert_eq!(
            ssh_destination(&args(&["-L", "8080:localhost:80", "-J", "bastion", "db"])),
            Some("db".to_string())
        );
        assert_eq!(
            ssh_destination(&args(&["-v", "-oControlPath=/tmp/x", "host"])),
            Some("host".to_string())
        );
        assert_eq!(
            ssh_destination(&args(&["--", "-odd-host"])),
            Some("-odd-host".to_string())
        );
        assert_eq!(ssh_destination(&args(&[])), None);
    }

    #[test]
    fn detects_production_hosts_and_tunnels() {
        let pattern = r"(?i)(^|[-_.])(prod|production|prd|live)([-_.]|$)";
        assert!(alert_host(pattern, "prod-db-02"));
        assert!(alert_host(pattern, "user@production"));
        assert!(alert_host(pattern, "live-eu"));
        assert!(!alert_host(pattern, "productivity"));
        assert!(!alert_host(pattern, "dev-01"));
        assert!(!alert_host("", "prod-01"));
        assert!(!alert_host("(", "prod-01"));

        let args = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert_eq!(
            ssh_forwardings(&args(&[
                "-L",
                "8080:localhost:80",
                "-R9000:localhost:90",
                "-D",
                "1080",
                "host",
            ])),
            vec![
                "-L 8080:localhost:80".to_string(),
                "-R 9000:localhost:90".to_string(),
                "-D 1080".to_string(),
            ]
        );
    }

    #[test]
    fn detects_agent_forwarding() {
        assert!(agent_forwarding_enabled("forwardagent yes\n"));
        assert!(!agent_forwarding_enabled("forwardagent no\n"));
        assert!(!agent_forwarding_enabled(""));
    }

    #[test]
    fn sharing_options_respect_the_effective_config() {
        let dir = Path::new("/home/ana/.config/oxterm");
        let options = sharing_options("serveraliveinterval 0\n", dir);
        assert!(options.iter().any(|option| option == "ControlMaster=auto"));
        assert!(options
            .iter()
            .any(|option| option == "ControlPath=/home/ana/.config/oxterm/cm-%C"));
        assert!(options
            .iter()
            .any(|option| option == "ServerAliveInterval=30"));

        let configured = "controlpath /tmp/master\nserveraliveinterval 15\n";
        assert!(sharing_options(configured, dir).is_empty());

        let disabled = "controlpath none\nserveraliveinterval 15\n";
        let options = sharing_options(disabled, dir);
        assert!(options.iter().any(|option| option == "ControlMaster=auto"));
        assert!(!options
            .iter()
            .any(|option| option == "ServerAliveInterval=30"));
    }

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn missing_config_is_empty() {
        let dir = temp_dir();
        assert!(hosts_from_config(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
