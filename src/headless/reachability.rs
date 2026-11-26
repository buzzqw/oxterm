//! Cheap TCP reachability checks, independent of the display.
//!
//! A probe is just a DNS lookup followed by `connect_timeout`: no ICMP, no
//! external command and no toolkit. A failed lookup is reported as an offline
//! host rather than a panic, so a connection list can probe every entry safely.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime};

/// Result of the last probe. `Unknown` means the target was never checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProbeState {
    #[default]
    Unknown,
    Online,
    Offline,
}

/// The outcome of one reachability probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reachability {
    pub host: String,
    pub port: u16,
    pub state: ProbeState,
    pub last_check: Option<SystemTime>,
    pub latency_ms: Option<u128>,
}

/// The default connect timeout, used when the caller passes 0.
pub const DEFAULT_TIMEOUT_MS: u64 = 2000;

/// Try to open a TCP connection to `host:port` within the given timeout.
///
/// A timeout of 0 means [`DEFAULT_TIMEOUT_MS`]. Every address returned by the
/// resolver is tried, so both IPv4 and IPv6 targets work. DNS failures and
/// refused connections are normal results (`Offline`), not errors or panics.
pub fn probe_tcp(host: &str, port: u16, timeout_ms: u64) -> Reachability {
    let timeout_ms = if timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        timeout_ms
    };
    let timeout = Duration::from_millis(timeout_ms);
    let mut result = Reachability {
        host: host.to_string(),
        port,
        state: ProbeState::Offline,
        last_check: None,
        latency_ms: None,
    };

    let started = Instant::now();
    if let Ok(addresses) = (host, port).to_socket_addrs() {
        for address in addresses {
            if let Ok(stream) = TcpStream::connect_timeout(&address, timeout) {
                drop(stream);
                result.state = ProbeState::Online;
                result.latency_ms = Some(started.elapsed().as_millis());
                break;
            }
        }
    }
    result.last_check = Some(SystemTime::now());
    result
}

/// Split a target into host and port.
///
/// Understands `host`, `host:port`, `[::1]:port` and `user@host:port`; the user
/// part is discarded. A malformed or missing port falls back to `default_port`.
pub fn host_port(target: &str, default_port: u16) -> (String, u16) {
    let target = target.trim();
    let host_part = target.rsplit_once('@').map_or(target, |(_, host)| host);

    if let Some(inner) = host_part.strip_prefix('[') {
        if let Some(end) = inner.find(']') {
            let host = &inner[..end];
            let tail = &inner[end + 1..];
            let port = tail
                .strip_prefix(':')
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(default_port);
            return (host.to_string(), port);
        }
    }

    if let Some((host, value)) = host_part.rsplit_once(':') {
        // More than one colon without brackets is a bare IPv6 address, not a
        // `host:port` pair.
        if !host.is_empty() && !host.contains(':') {
            if let Ok(port) = value.parse::<u16>() {
                return (host.to_string(), port);
            }
        }
    }
    (host_part.to_string(), default_port)
}

/// Short status lines for tooltips and status bars.
pub fn tooltip_lines(state: &Reachability) -> String {
    match state.state {
        ProbeState::Unknown => "Not checked yet".to_string(),
        ProbeState::Offline => "Unreachable".to_string(),
        ProbeState::Online => match state.latency_ms {
            Some(ms) => format!("Reachable ({} ms)", ms),
            None => "Reachable".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn unchecked(host: &str, port: u16) -> Reachability {
        Reachability {
            host: host.to_string(),
            port,
            state: ProbeState::Unknown,
            last_check: None,
            latency_ms: None,
        }
    }

    #[test]
    fn parses_host_and_port_forms() {
        assert_eq!(
            host_port("192.168.1.2:2200", 22),
            ("192.168.1.2".to_string(), 2200)
        );
        assert_eq!(
            host_port("server.example", 22),
            ("server.example".to_string(), 22)
        );
        assert_eq!(
            host_port("server.example:2222", 22),
            ("server.example".to_string(), 2222)
        );
        // An invalid or missing port falls back to the default.
        assert_eq!(host_port("server:abc", 22), ("server:abc".to_string(), 22));
        assert_eq!(host_port("", 22), (String::new(), 22));
    }

    #[test]
    fn parses_ipv6_and_user_at_host() {
        assert_eq!(host_port("[::1]:5900", 22), ("::1".to_string(), 5900));
        assert_eq!(host_port("[::1]", 22), ("::1".to_string(), 22));
        assert_eq!(
            host_port("alice@server:2222", 22),
            ("server".to_string(), 2222)
        );
        assert_eq!(host_port("alice@[::1]:5900", 22), ("::1".to_string(), 5900));
        assert_eq!(host_port("alice@server", 22), ("server".to_string(), 22));
    }

    #[test]
    fn probes_a_local_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local listener");
        let port = listener.local_addr().unwrap().port();
        let result = probe_tcp("127.0.0.1", port, 1000);
        assert_eq!(result.host, "127.0.0.1");
        assert_eq!(result.port, port);
        assert_eq!(result.state, ProbeState::Online);
        assert!(result.last_check.is_some());
        assert!(result.latency_ms.is_some());
        assert!(tooltip_lines(&result).starts_with("Reachable"));
    }

    #[test]
    fn probes_a_closed_port_and_formats_tooltips() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local listener");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let result = probe_tcp("127.0.0.1", port, 200);
        assert_eq!(result.state, ProbeState::Offline);
        assert!(result.last_check.is_some());
        assert_eq!(result.latency_ms, None);
        assert_eq!(tooltip_lines(&result), "Unreachable");

        assert_eq!(
            tooltip_lines(&unchecked("127.0.0.1", port)),
            "Not checked yet"
        );
        let mut online = unchecked("127.0.0.1", port);
        online.state = ProbeState::Online;
        online.latency_ms = Some(23);
        assert_eq!(tooltip_lines(&online), "Reachable (23 ms)");
    }

    #[test]
    fn timeout_and_failed_dns_return_offline_without_panicking() {
        // TEST-NET-1 is reserved for documentation: a connect must fail, and
        // the short timeout keeps the test bounded even if routing is odd.
        let started = Instant::now();
        let result = probe_tcp("192.0.2.1", 81, 30);
        assert_eq!(result.state, ProbeState::Offline);
        assert!(result.last_check.is_some());
        assert!(result.latency_ms.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the probe did not honour its timeout"
        );

        // 0 means the default timeout, not "fail immediately".
        let started = Instant::now();
        let result = probe_tcp("127.0.0.1", 1, 0);
        assert_eq!(result.host, "127.0.0.1");
        assert!(started.elapsed() < Duration::from_secs(3));

        let result = probe_tcp("host-that-does-not-exist.invalid", 22, 30);
        assert_eq!(result.state, ProbeState::Offline);
        assert_eq!(result.host, "host-that-does-not-exist.invalid");
        assert!(result.last_check.is_some());
    }
}
