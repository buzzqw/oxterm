//! Parsing of `scheme://...` connection URIs, display-free.
//!
//! A URI is a compact way to hand a destination to Oxterm (from a terminal
//! hint, a URL handler or a command line). Only connection schemes are
//! recognised: an `http://` link must never be mistaken for one. Passwords are
//! deliberately rejected so a URI cannot become a way to smuggle a secret into
//! history, logs or the connection book.

use std::collections::BTreeMap;

/// Protocols with a URI scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UriProtocol {
    #[default]
    Ssh,
    Sftp,
    Ftp,
    Telnet,
    Mosh,
    Rdp,
    Vnc,
    Serial,
    Docker,
    Kubectl,
    AwsSsm,
}

impl UriProtocol {
    /// The canonical scheme written before `://`.
    pub fn scheme(self) -> &'static str {
        match self {
            UriProtocol::Ssh => "ssh",
            UriProtocol::Sftp => "sftp",
            UriProtocol::Ftp => "ftp",
            UriProtocol::Telnet => "telnet",
            UriProtocol::Mosh => "mosh",
            UriProtocol::Rdp => "rdp",
            UriProtocol::Vnc => "vnc",
            UriProtocol::Serial => "serial",
            UriProtocol::Docker => "docker",
            UriProtocol::Kubectl => "kubectl",
            UriProtocol::AwsSsm => "aws-ssm",
        }
    }

    /// A label for menus and tooltips.
    pub fn label(self) -> &'static str {
        match self {
            UriProtocol::Ssh => "SSH",
            UriProtocol::Sftp => "SFTP",
            UriProtocol::Ftp => "FTP",
            UriProtocol::Telnet => "Telnet",
            UriProtocol::Mosh => "Mosh",
            UriProtocol::Rdp => "RDP",
            UriProtocol::Vnc => "VNC",
            UriProtocol::Serial => "Serial port",
            UriProtocol::Docker => "Docker",
            UriProtocol::Kubectl => "Kubernetes",
            UriProtocol::AwsSsm => "AWS SSM",
        }
    }
}

/// The parts of a parsed connection URI. A missing port stays `None`; callers
/// that need one use [`default_port`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionUri {
    pub protocol: UriProtocol,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub path: Option<String>,
    pub options: BTreeMap<String, String>,
}

/// The well-known TCP port of a protocol, when it has one. Mosh uses SSH to
/// bootstrap but then picks a UDP port, so it has no fixed port of its own.
pub fn default_port(protocol: UriProtocol) -> Option<u16> {
    match protocol {
        UriProtocol::Ssh | UriProtocol::Sftp => Some(22),
        UriProtocol::Ftp => Some(21),
        UriProtocol::Telnet => Some(23),
        UriProtocol::Rdp => Some(3389),
        UriProtocol::Vnc => Some(5900),
        UriProtocol::Mosh
        | UriProtocol::Serial
        | UriProtocol::Docker
        | UriProtocol::Kubectl
        | UriProtocol::AwsSsm => None,
    }
}

fn scheme_is_well_formed(scheme: &str) -> bool {
    !scheme.is_empty()
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'+' || b == b'.')
}

fn protocol_from_scheme(scheme: &str) -> Option<UriProtocol> {
    if scheme.eq_ignore_ascii_case("ssh") {
        Some(UriProtocol::Ssh)
    } else if scheme.eq_ignore_ascii_case("sftp") {
        Some(UriProtocol::Sftp)
    } else if scheme.eq_ignore_ascii_case("ftp") {
        Some(UriProtocol::Ftp)
    } else if scheme.eq_ignore_ascii_case("telnet") {
        Some(UriProtocol::Telnet)
    } else if scheme.eq_ignore_ascii_case("mosh") {
        Some(UriProtocol::Mosh)
    } else if scheme.eq_ignore_ascii_case("rdp") {
        Some(UriProtocol::Rdp)
    } else if scheme.eq_ignore_ascii_case("vnc") {
        Some(UriProtocol::Vnc)
    } else if scheme.eq_ignore_ascii_case("serial") {
        Some(UriProtocol::Serial)
    } else if scheme.eq_ignore_ascii_case("docker") {
        Some(UriProtocol::Docker)
    } else if scheme.eq_ignore_ascii_case("kubectl") {
        Some(UriProtocol::Kubectl)
    } else if scheme.eq_ignore_ascii_case("aws-ssm")
        || scheme.eq_ignore_ascii_case("aws_ssm")
        || scheme.eq_ignore_ascii_case("awsssm")
    {
        Some(UriProtocol::AwsSsm)
    } else {
        None
    }
}

/// Cheap check for text that looks like a supported connection URI. Used to
/// decide whether a terminal hint is clickable as a connection.
pub fn is_connection_uri(text: &str) -> bool {
    let Some((scheme, _)) = text.trim().split_once("://") else {
        return false;
    };
    scheme_is_well_formed(scheme) && protocol_from_scheme(scheme).is_some()
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("Invalid port in URI: {}", value))
}

/// Split `[user@]host[:port]`, keeping bracketed IPv6 hosts intact.
fn split_authority(authority: &str) -> Result<(String, Option<u16>), String> {
    if let Some(inner) = authority.strip_prefix('[') {
        let end = inner
            .find(']')
            .ok_or_else(|| format!("Invalid IPv6 host in URI: {}", authority))?;
        let host = &inner[..end];
        if host.is_empty() {
            return Err("Missing host in URI".to_string());
        }
        let tail = &inner[end + 1..];
        let port = match tail {
            "" => None,
            rest if rest.starts_with(':') => Some(parse_port(&rest[1..])?),
            _ => return Err(format!("Invalid IPv6 host in URI: {}", authority)),
        };
        return Ok((host.to_string(), port));
    }

    if let Some((host, value)) = authority.rsplit_once(':') {
        // A bare IPv6 address has more than one colon; only bracketed forms
        // carry a port.
        if !host.contains(':') {
            if host.is_empty() {
                return Err("Missing host in URI".to_string());
            }
            return Ok((host.to_string(), Some(parse_port(value)?)));
        }
    }
    Ok((authority.to_string(), None))
}

/// Parse a supported `scheme://` URI.
///
/// Examples: `ssh://user@host:2222`, `sftp://host/path`,
/// `rdp://host:3389?user=x`, `serial:///dev/ttyUSB0?baud=115200`.
/// Query parameters land in `options`. Any `user:pass@host` userinfo is refused
/// with an explicit error.
pub fn parse(input: &str) -> Result<ConnectionUri, String> {
    let input = input.trim();
    let (scheme, rest) = input
        .split_once("://")
        .ok_or_else(|| format!("Not a connection URI (expected scheme://): {}", input))?;
    if !scheme_is_well_formed(scheme) {
        return Err(format!("Invalid URI scheme: {}", scheme));
    }
    let protocol = protocol_from_scheme(scheme)
        .ok_or_else(|| format!("Unsupported connection URI scheme: {}", scheme))?;

    let (before_query, query) = match rest.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (rest, None),
    };
    let (authority, path) = match before_query.split_once('/') {
        Some((host, tail)) => (host, Some(format!("/{}", tail))),
        None => (before_query, None),
    };

    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    if userinfo.is_some_and(|user| user.contains(':')) {
        return Err("Passwords in URIs are not accepted".to_string());
    }
    let (host, port) = split_authority(hostport)?;
    if host.is_empty() && protocol != UriProtocol::Serial {
        return Err(format!("Missing host in URI: {}", input));
    }
    if protocol == UriProtocol::Serial && path.is_none() {
        return Err("A serial URI needs a device path".to_string());
    }

    let mut options = BTreeMap::new();
    if let Some(query) = query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if !key.is_empty() {
                options.insert(key.to_string(), value.to_string());
            }
        }
    }

    Ok(ConnectionUri {
        protocol,
        host,
        port,
        user: userinfo.filter(|user| !user.is_empty()).map(str::to_string),
        path,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_protocol() {
        let cases = [
            ("ssh://host", UriProtocol::Ssh, "host", None, None),
            (
                "sftp://host/path",
                UriProtocol::Sftp,
                "host",
                None,
                Some("/path"),
            ),
            (
                "ftp://host:21/pub",
                UriProtocol::Ftp,
                "host",
                Some(21),
                Some("/pub"),
            ),
            (
                "telnet://host:23",
                UriProtocol::Telnet,
                "host",
                Some(23),
                None,
            ),
            ("mosh://host", UriProtocol::Mosh, "host", None, None),
            (
                "rdp://host:3389",
                UriProtocol::Rdp,
                "host",
                Some(3389),
                None,
            ),
            (
                "vnc://host:5900",
                UriProtocol::Vnc,
                "host",
                Some(5900),
                None,
            ),
            (
                "serial:///dev/ttyUSB0?baud=115200",
                UriProtocol::Serial,
                "",
                None,
                Some("/dev/ttyUSB0"),
            ),
            (
                "docker://host/container",
                UriProtocol::Docker,
                "host",
                None,
                Some("/container"),
            ),
            (
                "kubectl://host/pod",
                UriProtocol::Kubectl,
                "host",
                None,
                Some("/pod"),
            ),
            (
                "aws-ssm://host/i-123",
                UriProtocol::AwsSsm,
                "host",
                None,
                Some("/i-123"),
            ),
        ];
        for (text, protocol, host, port, path) in cases {
            let uri = parse(text).unwrap_or_else(|error| panic!("{}: {}", text, error));
            assert_eq!(uri.protocol, protocol, "{}", text);
            assert_eq!(uri.host, host, "{}", text);
            assert_eq!(uri.port, port, "{}", text);
            assert_eq!(uri.path, path.map(str::to_string), "{}", text);
        }
    }

    #[test]
    fn parses_user_port_query_and_ipv6() {
        let uri = parse("ssh://user@host:2222?group=prod&key=abc").unwrap();
        assert_eq!(uri.protocol, UriProtocol::Ssh);
        assert_eq!(uri.user.as_deref(), Some("user"));
        assert_eq!(uri.port, Some(2222));
        assert_eq!(uri.options.get("group").map(String::as_str), Some("prod"));
        assert_eq!(uri.options.get("key").map(String::as_str), Some("abc"));

        let uri = parse("ssh://user@host/?group=prod").unwrap();
        assert_eq!(uri.path.as_deref(), Some("/"));
        assert_eq!(uri.user.as_deref(), Some("user"));

        let uri = parse("rdp://host:3389?user=x").unwrap();
        assert_eq!(uri.port, Some(3389));
        assert_eq!(uri.options.get("user").map(String::as_str), Some("x"));
        assert_eq!(uri.user, None);

        let uri = parse("vnc://[::1]:5901").unwrap();
        assert_eq!(uri.host, "::1");
        assert_eq!(uri.port, Some(5901));

        let uri = parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(uri.options.get("baud").map(String::as_str), Some("115200"));
    }

    #[test]
    fn rejects_passwords() {
        for text in ["ssh://user:pass@host", "ssh://user:pass@host:2222/path"] {
            let error = parse(text).unwrap_err();
            assert_eq!(error, "Passwords in URIs are not accepted");
        }
    }

    #[test]
    fn rejects_missing_host_non_numeric_port_and_unknown_schemes() {
        assert!(parse("ssh://").is_err());
        assert!(parse("ssh://:22").is_err());
        assert!(parse("ssh://host:abc").is_err());
        assert!(parse("serial://host").is_err());
        assert!(parse("http://host").is_err());
        assert!(parse("ssh:host").is_err());
        assert!(parse("host:22").is_err());
        assert!(parse("://host").is_err());
    }

    #[test]
    fn recognises_only_connection_uris() {
        assert!(is_connection_uri("ssh://host"));
        assert!(is_connection_uri("SSH://host:2222"));
        assert!(is_connection_uri("aws-ssm://host/i-123"));
        assert!(!is_connection_uri("http://example.com"));
        assert!(!is_connection_uri("ssh:host"));
        assert!(!is_connection_uri("user@host:/path"));
        assert!(!is_connection_uri("just some text"));
    }

    #[test]
    fn exposes_default_ports() {
        assert_eq!(default_port(UriProtocol::Ssh), Some(22));
        assert_eq!(default_port(UriProtocol::Sftp), Some(22));
        assert_eq!(default_port(UriProtocol::Ftp), Some(21));
        assert_eq!(default_port(UriProtocol::Telnet), Some(23));
        assert_eq!(default_port(UriProtocol::Rdp), Some(3389));
        assert_eq!(default_port(UriProtocol::Vnc), Some(5900));
        assert_eq!(default_port(UriProtocol::Mosh), None);
        assert_eq!(default_port(UriProtocol::Serial), None);
        assert_eq!(default_port(UriProtocol::Docker), None);
        assert_eq!(default_port(UriProtocol::Kubectl), None);
        assert_eq!(default_port(UriProtocol::AwsSsm), None);
    }
}
