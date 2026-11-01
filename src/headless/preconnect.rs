//! What runs before a connection opens: a local command, and Wake-on-LAN.
//!
//! Both may take seconds, so callers run this off the GUI thread. A failure is
//! reported and the connection is still attempted: a VPN that was already up, or
//! a host that was already awake, must not stop someone from logging in.

use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::connections::{Connection, PROBE_TIMEOUT};
use super::ssh_config;

const DEFAULT_PRE_TIMEOUT: u32 = 15;
const WOL_PORT: u16 = 9;

/// Parse `aa:bb:cc:dd:ee:ff` or `aa-bb-…`.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = text.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

pub fn magic_packet(mac: &[u8; 6]) -> Vec<u8> {
    let mut packet = vec![0xFF; 6];
    for _ in 0..16 {
        packet.extend_from_slice(mac);
    }
    packet
}

/// Run `command` without a shell, as PCM does: quoting works, `;` and `$()` are
/// plain text, so an entry cannot smuggle a second command past the form.
fn run_local(command: &str, timeout: Duration) -> Result<(), String> {
    let argv = shell_words::split(command).map_err(|_| "unbalanced quotes".to_string())?;
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {}: {}", program, error))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{} failed: {}", program, status)),
            Ok(None) if timeout.is_zero() || Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} timed out", program));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn send_wol(mac: &[u8; 6]) -> Result<(), String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).map_err(|error| error.to_string())?;
    socket
        .set_broadcast(true)
        .map_err(|error| error.to_string())?;
    socket
        .send_to(&magic_packet(mac), ("255.255.255.255", WOL_PORT))
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// `host port` as ssh resolves them, so an alias works too.
fn ssh_address(connection: &Connection) -> Option<String> {
    let config = ssh_config::effective_config(&connection.target_args(), PROBE_TIMEOUT)?;
    let value = |key: &str| {
        config.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(str::trim)
        })
    };
    Some(format!("{}:{}", value("hostname")?, value("port")?))
}

/// Wait until the host accepts connections, for at most `wait`. Returning early
/// is the point: a machine that is already up should not cost the full wait.
fn wait_for_port(address: &str, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if let Ok(mut resolved) = address.to_socket_addrs() {
            if resolved
                .any(|socket| TcpStream::connect_timeout(&socket, Duration::from_secs(1)).is_ok())
            {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Whether anything will run, so the caller can say so while it waits.
pub fn has_work(connection: &Connection) -> bool {
    !connection.pre_command.trim().is_empty() || !connection.wol_mac.trim().is_empty()
}

/// Run the entry's steps. Returns one line per problem.
pub fn run(connection: &Connection) -> Vec<String> {
    let mut problems = Vec::new();
    let command = connection.pre_command.trim();
    if !command.is_empty() {
        let seconds = connection
            .pre_command_timeout
            .unwrap_or(DEFAULT_PRE_TIMEOUT);
        if let Err(error) = run_local(command, Duration::from_secs(u64::from(seconds))) {
            problems.push(format!("Pre-connect command: {}", error));
        }
    }
    let mac = connection.wol_mac.trim();
    if !mac.is_empty() {
        match parse_mac(mac) {
            None => problems.push(format!("Invalid MAC address: {}", mac)),
            Some(mac) => match send_wol(&mac) {
                Err(error) => problems.push(format!("Wake-on-LAN: {}", error)),
                Ok(()) if connection.wol_wait > 0 => {
                    let wait = Duration::from_secs(u64::from(connection.wol_wait));
                    match ssh_address(connection) {
                        Some(address) if wait_for_port(&address, wait) => {}
                        Some(address) => {
                            problems.push(format!("Wake-on-LAN: {} did not answer", address));
                        }
                        None => thread::sleep(wait),
                    }
                }
                Ok(()) => {}
            },
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses_are_parsed_strictly() {
        assert_eq!(
            parse_mac("aa:BB:cc:00:11:22"),
            Some([0xaa, 0xbb, 0xcc, 0x00, 0x11, 0x22])
        );
        assert_eq!(parse_mac("aa-bb-cc-00-11-22").map(|mac| mac[1]), Some(0xbb));
        for bad in [
            "",
            "aa:bb",
            "aa:bb:cc:00:11:2g",
            "aaa:bb:cc:00:11:22",
            "aa:bb:cc:00:11:22:33",
        ] {
            assert_eq!(parse_mac(bad), None, "{}", bad);
        }
    }

    #[test]
    fn the_magic_packet_has_the_standard_shape() {
        let mac = [1, 2, 3, 4, 5, 6];
        let packet = magic_packet(&mac);
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        assert_eq!(&packet[96..], &mac);
    }

    #[test]
    fn local_commands_report_failure_and_time_out() {
        assert!(run_local("true", Duration::from_secs(5)).is_ok());
        assert!(run_local("false", Duration::from_secs(5)).is_err());
        assert!(run_local("/nonexistent/oxterm", Duration::from_secs(5)).is_err());
        assert!(run_local("'unbalanced", Duration::from_secs(5)).is_err());
        let error = run_local("sleep 30", Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("timed out"), "{}", error);
    }

    #[test]
    fn local_commands_have_no_shell() {
        let dir = std::env::temp_dir().join(format!("oxterm-pre-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("ran");
        let command = format!("true; touch {}", marker.display());
        let _ = run_local(&command, Duration::from_secs(5));
        assert!(!marker.exists(), "a ; must not start a second command");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_step_is_reported_and_does_not_stop_the_others() {
        let connection = Connection {
            pre_command: "false".into(),
            wol_mac: "bad".into(),
            ..Connection::default()
        };
        assert!(has_work(&connection));
        assert_eq!(run(&connection).len(), 2);
        assert!(!has_work(&Connection::default()));
    }
}
