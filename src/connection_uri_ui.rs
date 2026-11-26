//! Open a `scheme://...` connection URI as a one-off connection.
//!
//! A URI from a terminal hint, a URL handler or the command line is a
//! destination, not an entry in the book: it is parsed, validated and opened in
//! a new tab, and nothing is written to `connections.json`.

use crate::connections_ui::Placement;
use crate::headless::connection_uri::{self, ConnectionUri, UriProtocol};
use crate::headless::connections::{Connection, HostKeyCheck, Kind, Protocol};
use crate::headless::desktop::VncServer;
use crate::window::MainWindow;

/// The path of a URI without the leading slash, when it names a thing (a serial
/// device, a container, a pod or an instance) rather than a directory.
fn path_argument(uri: &ConnectionUri) -> Option<String> {
    uri.path
        .as_deref()
        .map(|path| path.trim().trim_matches('/').to_string())
        .filter(|path| !path.is_empty())
}

fn path_basename(uri: &ConnectionUri) -> Option<String> {
    uri.path
        .as_deref()
        .and_then(|path| {
            path.trim_end_matches('/')
                .rsplit('/')
                .find(|part| !part.is_empty())
        })
        .map(str::to_string)
}

/// A stable, readable tab title for a URI. The user part is included so two
/// accounts on one host do not share a name; protocol-only targets use the path.
pub(crate) fn uri_connection_name(uri: &ConnectionUri) -> String {
    let target = match uri.protocol {
        UriProtocol::Serial | UriProtocol::Docker | UriProtocol::Kubectl | UriProtocol::AwsSsm => {
            path_basename(uri).or_else(|| {
                if uri.host.is_empty() {
                    None
                } else {
                    Some(uri.host.clone())
                }
            })
        }
        _ => {
            if uri.host.is_empty() {
                None
            } else {
                Some(uri.host.clone())
            }
        }
    }
    .unwrap_or_else(|| uri.protocol.label().to_string());
    match uri.user.as_deref().filter(|_| !target.contains('@')) {
        Some(user) => format!("{}@{}", user, target),
        None => target,
    }
}

fn option<'a>(uri: &'a ConnectionUri, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| uri.options.get(*key))
        .map(String::as_str)
}

fn number<T: std::str::FromStr>(
    uri: &ConnectionUri,
    keys: &[&str],
    label: &str,
) -> Result<Option<T>, String> {
    let Some(value) = option(uri, keys) else {
        return Ok(None);
    };
    value
        .parse()
        .map(Some)
        .map_err(|_| format!("Invalid {} in URI: {}", label, value))
}

fn flag(uri: &ConnectionUri, key: &str) -> Option<bool> {
    option(uri, &[key]).map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        )
    })
}

/// Turn a parsed URI into a connection. Pure: the caller decides whether and
/// where to open it. Query options that change identity, run a program or set
/// up a tunnel are deliberately ignored: a URI describes a destination.
pub(crate) fn connection_from_uri(uri: &ConnectionUri) -> Result<Connection, String> {
    let protocol = match uri.protocol {
        UriProtocol::Ssh => Protocol::Ssh,
        UriProtocol::Sftp => Protocol::Sftp,
        UriProtocol::Ftp => Protocol::Ftp,
        UriProtocol::Telnet => Protocol::Telnet,
        UriProtocol::Mosh => Protocol::Mosh,
        UriProtocol::Rdp => Protocol::Rdp,
        UriProtocol::Vnc => Protocol::Vnc,
        UriProtocol::Serial => Protocol::Serial,
        UriProtocol::Docker => Protocol::Docker,
        UriProtocol::Kubectl => Protocol::Kubectl,
        UriProtocol::AwsSsm => Protocol::AwsSsm,
    };
    let name = uri_connection_name(uri);
    let host = uri.host.clone();
    // `alias()` supplies a stable id. A manual kind is then needed because the
    // URI carries an explicit host, user and port: an alias would be passed to
    // ssh as a name from ~/.ssh/config and would drop all three.
    let seed = if host.is_empty() { &name } else { &host };
    let mut connection = Connection::alias(seed);
    connection.kind = Kind::Manual;
    connection.name = name;
    connection.protocol = protocol;
    connection.host = host;
    connection.user = uri
        .user
        .clone()
        .or_else(|| option(uri, &["user"]).map(str::to_string))
        .unwrap_or_default();
    // A missing port stays unset: that lets ssh use ~/.ssh/config, and keeps
    // desktop protocols from mistaking a VNC/RDP port for the ssh port.
    connection.port = uri.port.or(number(uri, &["port"], "port")?);

    if let Some(group) = option(uri, &["group"]) {
        connection.group = group.to_string();
    }
    if let Some(identity) = option(uri, &["identity_file", "identity", "key"]) {
        connection.identity_file = identity.to_string();
    }
    if let Some(jump) = option(uri, &["proxy_jump", "jump"]) {
        connection.proxy_jump = jump.to_string();
    }

    match protocol {
        Protocol::Serial => {
            connection.device = option(uri, &["device"])
                .map(str::to_string)
                .or_else(|| uri.path.clone().map(|path| path.trim().to_string()))
                .unwrap_or_default();
            connection.baud = number(uri, &["baud"], "baud")?;
        }
        Protocol::Docker => {
            connection.container = option(uri, &["container"])
                .map(str::to_string)
                .or_else(|| path_argument(uri))
                .unwrap_or_default();
            if let Some(shell) = option(uri, &["shell"]) {
                connection.shell = shell.to_string();
            }
        }
        Protocol::Kubectl => {
            connection.pod = option(uri, &["pod"])
                .map(str::to_string)
                .or_else(|| path_argument(uri))
                .unwrap_or_default();
            if let Some(container) = option(uri, &["container", "kube_container"]) {
                connection.container = container.to_string();
            }
            if let Some(namespace) = option(uri, &["namespace"]) {
                connection.namespace = namespace.to_string();
            }
            if let Some(context) = option(uri, &["context", "kube_context"]) {
                connection.kube_context = context.to_string();
            }
            if let Some(shell) = option(uri, &["shell"]) {
                connection.shell = shell.to_string();
            }
        }
        Protocol::AwsSsm => {
            connection.instance_id = option(uri, &["instance_id", "instance"])
                .map(str::to_string)
                .or_else(|| path_argument(uri))
                .unwrap_or_default();
            if let Some(region) = option(uri, &["aws_region", "region"]) {
                connection.aws_region = region.to_string();
            }
            if let Some(profile) = option(uri, &["aws_profile", "profile"]) {
                connection.aws_profile = profile.to_string();
            }
        }
        Protocol::Ftp => {
            if let Some(tls) = flag(uri, "ftp_tls").or_else(|| flag(uri, "tls")) {
                connection.ftp_tls = tls;
            }
        }
        Protocol::Rdp => {
            if let Some(domain) = option(uri, &["rdp_domain", "domain"]) {
                connection.rdp_domain = domain.to_string();
            }
            if let Some(drive) = option(uri, &["rdp_drive", "drive"]) {
                connection.rdp_drive = drive.to_string();
            }
            connection.desktop_fullscreen = flag(uri, "fullscreen").unwrap_or(false);
            connection.rdp_clipboard = flag(uri, "rdp_clipboard").unwrap_or(false);
            connection.rdp_multimon = flag(uri, "rdp_multimon").unwrap_or(false);
        }
        Protocol::Vnc => {
            if let Some(server) = option(uri, &["vnc_server"]) {
                connection.vnc_server = VncServer::ALL
                    .iter()
                    .find(|(_, id, _)| id.eq_ignore_ascii_case(server))
                    .map_or(VncServer::Direct, |(server, _, _)| *server);
            }
            if let Some(display) = option(uri, &["vnc_display", "display"]) {
                connection.vnc_display = display.to_string();
            }
            if let Some(geometry) = option(uri, &["vnc_geometry", "size"]) {
                connection.vnc_geometry = geometry.to_string();
            }
            if let Some(command) = option(uri, &["vnc_command"]) {
                connection.vnc_command = command.to_string();
            }
            connection.service_port = number(uri, &["service_port"], "service port")?;
            if connection.vnc_server != VncServer::Direct {
                // With a server behind ssh, `port` means the ssh port, so the
                // URI's port belongs to the VNC service instead.
                let uri_port = connection.port.take();
                if connection.service_port.is_none() {
                    connection.service_port = uri_port;
                }
            }
            connection.desktop_fullscreen = flag(uri, "fullscreen").unwrap_or(false);
            connection.view_only = flag(uri, "view_only").unwrap_or(false);
        }
        Protocol::Spice => {
            connection.spice_tls = flag(uri, "spice_tls").unwrap_or(false);
            connection.desktop_fullscreen = flag(uri, "fullscreen").unwrap_or(false);
        }
        Protocol::Ssh | Protocol::Mosh | Protocol::Telnet | Protocol::Sftp | Protocol::Command => {}
    }

    connection.x11 = flag(uri, "x11").unwrap_or(false);
    connection.compression = flag(uri, "compression").unwrap_or(false);
    connection.agent_forward = flag(uri, "agent_forward").unwrap_or(false);
    connection.no_sharing = flag(uri, "no_sharing").unwrap_or(false);
    connection.keepalive_interval = number(uri, &["keepalive"], "keepalive")?;
    connection.connect_timeout = number(uri, &["connect_timeout"], "connect timeout")?;
    connection.host_key_check = match option(uri, &["host_key_check"]) {
        Some("ask") => HostKeyCheck::Ask,
        Some("accept_new" | "accept-new") => HostKeyCheck::AcceptNew,
        Some("strict") => HostKeyCheck::Strict,
        _ => HostKeyCheck::Default,
    };
    if let Some(terminal) = option(uri, &["terminal_type", "term"]) {
        connection.terminal_type = terminal.to_string();
    }
    if let Some(color) = option(uri, &["color_scheme"]) {
        connection.color_scheme = color.to_string();
    }
    if let Some(font) = option(uri, &["font"]) {
        connection.font = font.to_string();
    }
    if let Some(encoding) = option(uri, &["encoding"]) {
        connection.encoding = encoding.to_string();
    }
    connection.scrollback = number(uri, &["scrollback"], "scrollback")?;
    Ok(connection)
}

impl MainWindow {
    /// Parse and open a connection URI in a new tab. Validation failures are
    /// returned to the caller, which can show them in its status bar.
    pub(crate) fn open_connection_uri(&self, input: &str) -> Result<(), String> {
        let uri = connection_uri::parse(input)?;
        let connection = connection_from_uri(&uri)?;
        connection
            .validate()
            .map_err(|error| format!("Invalid connection URI: {}", error))?;
        self.connect(connection, Placement::Tab);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_uri_uses_the_authority_and_query() {
        let uri =
            connection_uri::parse("ssh://admin@db.example:2222?group=Prod&proxy_jump=bastion")
                .unwrap();
        let connection = connection_from_uri(&uri).unwrap();
        assert_eq!(connection.name, "admin@db.example");
        assert_eq!(connection.kind, Kind::Manual);
        assert_eq!(connection.host, "db.example");
        assert_eq!(connection.user, "admin");
        assert_eq!(connection.port, Some(2222));
        assert_eq!(connection.group, "Prod");
        assert_eq!(connection.proxy_jump, "bastion");
        assert_eq!(connection.protocol, Protocol::Ssh);
        connection.validate().unwrap();
    }

    #[test]
    fn paths_name_the_service_target() {
        let uri = connection_uri::parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        let connection = connection_from_uri(&uri).unwrap();
        assert_eq!(connection.name, "ttyUSB0");
        assert_eq!(connection.device, "/dev/ttyUSB0");
        assert_eq!(connection.baud, Some(115_200));

        let uri = connection_uri::parse("kubectl://host/pod-1?namespace=default").unwrap();
        let connection = connection_from_uri(&uri).unwrap();
        assert_eq!(connection.name, "pod-1");
        assert_eq!(connection.pod, "pod-1");
        assert_eq!(connection.namespace, "default");
    }

    #[test]
    fn bad_numbers_are_refused() {
        let uri = connection_uri::parse("ssh://host?port=abc").unwrap();
        assert!(connection_from_uri(&uri).is_err());
        let uri = connection_uri::parse("ssh://host?port=0").unwrap();
        let connection = connection_from_uri(&uri).unwrap();
        assert!(connection.validate().is_err());
    }
}
