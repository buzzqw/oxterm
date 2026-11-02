//! Graphical remote desktops: which viewer to run, and how a VNC server is
//! started on the far side.
//!
//! Oxterm embeds no RDP or VNC client: a viewer is an external program, as ssh is.
//! It runs in a tab, so its logs and questions (a changed certificate, a password)
//! stay visible and answerable, and closing the tab ends it. A server started on
//! demand listens on the remote loopback only and is reached through an ssh tunnel,
//! so nothing new is exposed on the network. That ssh runs in its own tab: it can
//! ask for a password or a host key there, the server's messages stay readable, and
//! closing the tab ends the session, because a terminal hangup takes the server with
//! it.
//!
//! Secrets never appear in an argument vector, which any local user can read. A
//! stored RDP password is typed at the viewer's own prompt, with echo off, and
//! TigerVNC's viewer reads it from its environment, readable only by the same user.
//! A server started for one session gets a one-time password typed into the tab with
//! echo off.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::connections::{Connection, Protocol};

/// What the server asks for, and what the tab's automation answers.
pub const OTP_PROMPT: &str = "OXTERM-VNC-PASSWORD:";

/// FreeRDP and rdesktop ask for the password on their terminal. Anchored to the
/// start of a line so a gateway's `GatewayPassword:` is not answered with it.
pub const PASSWORD_PROMPT: &str = r"(?m)^Password:\s*$";

const VNC_BASE_PORT: u16 = 5900;
const RDP_PORT: u16 = 3389;
const SPICE_PORT: u16 = 5900;
/// VNC authentication uses the first eight characters only.
const OTP_LENGTH: usize = 8;

/// Which VNC server to use on the remote side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VncServer {
    /// Connect straight to `host:port`, with no ssh.
    #[default]
    Direct,
    /// A server already running, reached through ssh.
    Running,
    /// Share the existing screen with the first of wayvnc, x11vnc, x0vncserver.
    Auto,
    /// Share an X11 screen.
    X11vnc,
    /// Share an X11 screen with TigerVNC's scraping server.
    TigerScraping,
    /// Share a Wayland (wlroots) screen.
    Wayvnc,
    /// A separate virtual desktop that outlives the connection.
    TigerVirtual,
    TurboVirtual,
    TightVirtual,
    /// The entry's own command, with `{port}` for the port.
    Custom,
}

impl VncServer {
    pub const ALL: [(VncServer, &'static str, &'static str); 10] = [
        (
            VncServer::Direct,
            "direct",
            "None: connect directly, without ssh",
        ),
        (
            VncServer::Running,
            "running",
            "Already running on the server",
        ),
        (
            VncServer::Auto,
            "auto",
            "Share the screen: detect the server",
        ),
        (
            VncServer::X11vnc,
            "x11vnc",
            "Share the screen: x11vnc (X11)",
        ),
        (
            VncServer::TigerScraping,
            "x0vncserver",
            "Share the screen: TigerVNC x0vncserver (X11)",
        ),
        (
            VncServer::Wayvnc,
            "wayvnc",
            "Share the screen: wayvnc (Wayland)",
        ),
        (
            VncServer::TigerVirtual,
            "tigervnc",
            "Virtual desktop: TigerVNC",
        ),
        (
            VncServer::TurboVirtual,
            "turbovnc",
            "Virtual desktop: TurboVNC",
        ),
        (
            VncServer::TightVirtual,
            "tightvnc",
            "Virtual desktop: TightVNC",
        ),
        (VncServer::Custom, "custom", "My own command"),
    ];

    /// A virtual desktop has its own password, set with `vncpasswd` on the server.
    pub fn is_virtual(self) -> bool {
        matches!(
            self,
            VncServer::TigerVirtual | VncServer::TurboVirtual | VncServer::TightVirtual
        )
    }

    /// Whether this server gets a password made up for one session.
    pub fn uses_one_time_password(self) -> bool {
        matches!(
            self,
            VncServer::Auto | VncServer::X11vnc | VncServer::TigerScraping
        )
    }
}

fn vnc_display_number(connection: &Connection) -> u16 {
    connection
        .vnc_display
        .trim()
        .trim_start_matches(':')
        .parse()
        .unwrap_or(1)
}

/// The port the service listens on, as seen from the far end of the tunnel.
pub fn remote_port(connection: &Connection) -> u16 {
    match connection.protocol {
        Protocol::Rdp => connection
            .service_port
            .or(connection.port)
            .unwrap_or(RDP_PORT),
        Protocol::Spice => connection
            .service_port
            .or(connection.port)
            .unwrap_or(SPICE_PORT),
        _ => {
            if connection.vnc_server.is_virtual() {
                return VNC_BASE_PORT + vnc_display_number(connection);
            }
            if connection.vnc_server == VncServer::Direct {
                return connection
                    .port
                    .or(connection.service_port)
                    .unwrap_or(VNC_BASE_PORT);
            }
            connection.service_port.unwrap_or(VNC_BASE_PORT)
        }
    }
}

fn quote(text: &str) -> String {
    shell_words::quote(text).into_owned()
}

const READ_PASSWORD: &str = "stty -echo 2>/dev/null; \
printf '%s ' 'OXTERM-VNC-PASSWORD:'; IFS= read -r pw; stty echo 2>/dev/null; printf '\\n'; \
umask 077; f=$(mktemp) || exit 1; trap 'rm -f \"$f\"' EXIT";

fn start_x11vnc(display: &str, port: u16) -> String {
    format!(
        "printf '%s\\n' \"$pw\" > \"$f\"; x11vnc -display {} -auth guess -localhost -rfbport {} \
         -passwdfile \"rm:$f\" -forever -noxdamage -quiet",
        quote(display),
        port
    )
}

fn start_x0vncserver(display: &str, port: u16) -> String {
    format!(
        "printf '%s\\n' \"$pw\" | vncpasswd -f > \"$f\"; x0vncserver -display {} -localhost=1 \
         -rfbport={} -PasswordFile=\"$f\"",
        quote(display),
        port
    )
}

fn start_wayvnc(port: u16) -> String {
    format!(
        "export XDG_RUNTIME_DIR=\"${{XDG_RUNTIME_DIR:-/run/user/$(id -u)}}\"; \
         for s in \"$XDG_RUNTIME_DIR\"/wayland-*; do case \"$s\" in *.lock) ;; \
         *) export WAYLAND_DISPLAY=\"${{s##*/}}\"; break ;; esac; done; \
         wayvnc 127.0.0.1 {}",
        port
    )
}

/// Find a Wayland session the way wayvnc will, to prefer it over X11 tools that
/// would only see an XWayland surface.
const HAS_WAYLAND: &str = "ls \"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}\"/wayland-* >/dev/null 2>&1";

fn start_virtual(program: &str, flags: &str, connection: &Connection) -> String {
    let number = vnc_display_number(connection);
    let geometry = match connection.vnc_geometry.trim() {
        "" => "1920x1080",
        value => value,
    };
    // Reuse a desktop left running: that is the point of a virtual one.
    format!(
        "if ! [ -e /tmp/.X11-unix/X{n} ]; then {program} :{n} {flags} -geometry {geometry} || exit 1; fi; \
         printf 'Desktop :{n} is running. It stays up after this tab closes; stop it with: {program} -kill :{n}\\n'; \
         while :; do sleep 3600; done",
        n = number,
        program = program,
        flags = flags,
        geometry = quote(geometry)
    )
}

/// What runs on the server, wrapped in `sh` because the login shell may be fish
/// or csh. `None` when a server is already running and only the tunnel is needed.
pub fn remote_script(connection: &Connection) -> Option<String> {
    let port = remote_port(connection);
    let display = match connection.vnc_display.trim() {
        "" => ":0",
        value => value,
    };
    let script = match connection.vnc_server {
        VncServer::Direct | VncServer::Running => return None,
        VncServer::X11vnc => format!("{}; {}", READ_PASSWORD, start_x11vnc(display, port)),
        VncServer::TigerScraping => format!("{}; {}", READ_PASSWORD, start_x0vncserver(display, port)),
        VncServer::Wayvnc => start_wayvnc(port),
        VncServer::Auto => format!(
            "{read}; if command -v wayvnc >/dev/null 2>&1 && {wayland}; then {wayvnc}; \
             elif command -v x11vnc >/dev/null 2>&1; then {x11vnc}; \
             elif command -v x0vncserver >/dev/null 2>&1; then {x0}; \
             else echo 'No VNC server found: install x11vnc, tigervnc-scraping-server or wayvnc.'; sleep 86400; fi",
            read = READ_PASSWORD,
            wayland = HAS_WAYLAND,
            wayvnc = start_wayvnc(port),
            x11vnc = start_x11vnc(display, port),
            x0 = start_x0vncserver(display, port),
        ),
        VncServer::TigerVirtual => {
            let start = start_virtual("\"$v\"", "-localhost yes", connection);
            format!(
                "v=$(command -v tigervncserver || command -v vncserver) || \
                 {{ echo 'TigerVNC is not installed on this server.'; sleep 86400; }}; {}",
                start
            )
        }
        VncServer::TurboVirtual => start_virtual("/opt/TurboVNC/bin/vncserver", "-localhost", connection),
        VncServer::TightVirtual => start_virtual("tightvncserver", "-localhost", connection),
        VncServer::Custom => connection.vnc_command.replace("{port}", &port.to_string()),
    };
    Some(format!("sh -c {}", quote(&script)))
}

/// Whether this desktop needs an ssh tunnel, and so a tab.
pub fn uses_tunnel(connection: &Connection) -> bool {
    !connection.proxy_jump.is_empty()
        || (connection.protocol == Protocol::Vnc && connection.vnc_server != VncServer::Direct)
}

/// The ssh command that carries the tunnel. A started server goes through the
/// entry's own ssh target; a gateway forwards to the host behind it.
pub fn tunnel_argv(connection: &Connection, local_port: u16) -> Vec<String> {
    let mut argv = vec!["ssh".to_string()];
    let script = (connection.protocol == Protocol::Vnc)
        .then(|| remote_script(connection))
        .flatten();
    // Its own connection: a forward on a shared master dies with that master.
    argv.extend(
        [
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
        ]
        .map(str::to_string),
    );
    let through_target =
        connection.protocol == Protocol::Vnc && connection.vnc_server != VncServer::Direct;
    let far_host = if through_target {
        "127.0.0.1"
    } else {
        connection.host.as_str()
    };
    argv.push("-L".to_string());
    argv.push(format!(
        "127.0.0.1:{}:{}:{}",
        local_port,
        far_host,
        remote_port(connection)
    ));
    match &script {
        Some(_) => argv.push("-t".to_string()),
        None => argv.push("-N".to_string()),
    }
    if through_target {
        argv.extend(connection.target_args());
    } else {
        argv.push(connection.proxy_jump.clone());
    }
    argv.extend(script);
    argv
}

/// A free local port for a tunnel. Another program could take it before ssh does;
/// `ExitOnForwardFailure` then reports that in the tab.
pub fn free_local_port() -> Option<u16> {
    Some(
        TcpListener::bind(("127.0.0.1", 0))
            .ok()?
            .local_addr()
            .ok()?
            .port(),
    )
}

/// A VNC server greets first with `RFB`, so this sees the server itself and not
/// just ssh's listener, which accepts connections before anything is behind it.
pub fn service_is_ready(protocol: Protocol, port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    if protocol != Protocol::Vnc {
        return true;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut banner = [0u8; 3];
    stream.read_exact(&mut banner).is_ok() && &banner == b"RFB"
}

/// A password for one session.
pub fn one_time_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; OTP_LENGTH];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        let _ = random.read_exact(&mut bytes);
    }
    bytes
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect()
}

pub fn program_exists(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

fn first_available(names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|name| program_exists(name))
}

/// TigerVNC's viewer takes a password from `VNC_PASSWORD`. Other viewers ask for it.
fn vnc_viewer_takes_env_password(program: &str) -> bool {
    Command::new(program)
        .arg("-help")
        .stdin(Stdio::null())
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).contains("TigerVNC")
                || String::from_utf8_lossy(&output.stderr).contains("TigerVNC")
        })
        .unwrap_or(false)
}

/// A viewer ready to run in a tab. Its output, logs and questions stay visible
/// and answerable there, as they would from a shell.
pub struct Viewer {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The password the viewer asks for on its terminal: the tab types it.
    pub prompt_password: Option<String>,
    /// True when the password reached the viewer without asking.
    pub delivered: bool,
}

/// What to run to show a desktop at `host:port`, and whether a password reached
/// it. `password` comes from the keyring, or was made up for this session.
pub fn viewer(
    connection: &Connection,
    host: &str,
    port: u16,
    password: Option<String>,
) -> Result<Viewer, String> {
    let mut viewer = Viewer {
        argv: Vec::new(),
        env: Vec::new(),
        prompt_password: None,
        delivered: false,
    };
    match connection.protocol {
        Protocol::Rdp => {
            let program = first_available(&[
                "xfreerdp3",
                "xfreerdp",
                "wlfreerdp3",
                "wlfreerdp",
                "rdesktop",
            ])
            .ok_or("No RDP client found: install freerdp or rdesktop")?;
            if program == "rdesktop" {
                rdesktop_args(
                    connection,
                    host,
                    port,
                    &mut viewer.argv,
                    &mut viewer.prompt_password,
                    password,
                );
                viewer.argv.insert(0, program.to_string());
            } else {
                viewer.argv.push(program.to_string());
                freerdp_args(
                    connection,
                    host,
                    port,
                    &mut viewer.argv,
                    &mut viewer.prompt_password,
                    password,
                );
            }
        }
        Protocol::Spice => {
            if !program_exists("remote-viewer") {
                return Err("No SPICE client found: install virt-viewer".to_string());
            }
            let argv = &mut viewer.argv;
            argv.push("remote-viewer".to_string());
            argv.push("--title".to_string());
            argv.push(connection.name.clone());
            if connection.desktop_fullscreen {
                argv.push("--full-screen".to_string());
            }
            let scheme = if connection.spice_tls {
                "spice+tls"
            } else {
                "spice"
            };
            argv.push(format!("{}://{}:{}", scheme, host, port));
        }
        _ => {
            let program =
                first_available(&["vncviewer", "xtigervncviewer", "tigervnc", "remote-viewer"])
                    .ok_or("No VNC viewer found: install tigervnc-viewer or virt-viewer")?;
            let argv = &mut viewer.argv;
            argv.push(program.to_string());
            if program == "remote-viewer" {
                if connection.desktop_fullscreen {
                    argv.push("--full-screen".to_string());
                }
                argv.push(format!("vnc://{}:{}", host, port));
            } else {
                if connection.desktop_fullscreen {
                    argv.push("-FullScreen".to_string());
                }
                if connection.view_only {
                    argv.push("-ViewOnly".to_string());
                }
                if let Some(secret) = password.filter(|_| vnc_viewer_takes_env_password(program)) {
                    viewer.env.push(("VNC_PASSWORD".to_string(), secret));
                    viewer.delivered = true;
                }
                argv.push(format!("{}::{}", host, port));
            }
        }
    }
    Ok(viewer)
}

fn freerdp_args(
    connection: &Connection,
    host: &str,
    port: u16,
    argv: &mut Vec<String>,
    prompt_password: &mut Option<String>,
    password: Option<String>,
) {
    argv.push(format!("/v:{}:{}", host, port));
    if !connection.user.is_empty() {
        argv.push(format!("/u:{}", connection.user));
    }
    if !connection.rdp_domain.is_empty() {
        argv.push(format!("/d:{}", connection.rdp_domain));
    }
    argv.push(format!("/t:{}", connection.name));
    if connection.desktop_fullscreen {
        argv.push("/f".to_string());
    } else if !connection.vnc_geometry.is_empty() {
        argv.push(format!("/size:{}", connection.vnc_geometry));
    } else {
        argv.push("/dynamic-resolution".to_string());
    }
    if connection.rdp_multimon {
        argv.push("/multimon".to_string());
    }
    if connection.rdp_clipboard {
        argv.push("+clipboard".to_string());
    }
    if !connection.rdp_drive.is_empty() {
        argv.push(format!("/drive:shared,{}", connection.rdp_drive));
    }
    *prompt_password = password;
}

fn rdesktop_args(
    connection: &Connection,
    host: &str,
    port: u16,
    argv: &mut Vec<String>,
    prompt_password: &mut Option<String>,
    password: Option<String>,
) {
    argv.extend(["-T".to_string(), connection.name.clone()]);
    if !connection.user.is_empty() {
        argv.extend(["-u".to_string(), connection.user.clone()]);
    }
    if !connection.rdp_domain.is_empty() {
        argv.extend(["-d".to_string(), connection.rdp_domain.clone()]);
    }
    if connection.desktop_fullscreen {
        argv.push("-f".to_string());
    } else if !connection.vnc_geometry.is_empty() {
        argv.extend(["-g".to_string(), connection.vnc_geometry.clone()]);
    }
    if connection.rdp_clipboard {
        argv.extend(["-r".to_string(), "clipboard:CLIPBOARD".to_string()]);
    }
    if !connection.rdp_drive.is_empty() {
        argv.extend([
            "-r".to_string(),
            format!("disk:shared={}", connection.rdp_drive),
        ]);
    }
    *prompt_password = password;
    argv.push(format!("{}:{}", host, port));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vnc(server: VncServer) -> Connection {
        Connection {
            name: "box".into(),
            protocol: Protocol::Vnc,
            host: "h".into(),
            user: "u".into(),
            vnc_server: server,
            ..Connection::default()
        }
    }

    fn fake_path() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn a_started_server_is_tunnelled_and_listens_on_loopback() {
        let argv = tunnel_argv(&vnc(VncServer::X11vnc), 15900);
        assert!(argv
            .windows(2)
            .any(|pair| pair == ["-L", "127.0.0.1:15900:127.0.0.1:5900"]));
        assert!(
            argv.iter().any(|arg| arg == "-t"),
            "a hangup must stop the server"
        );
        assert!(argv
            .windows(2)
            .any(|pair| pair == ["-o", "ControlPath=none"]));
        let script = argv.last().unwrap();
        assert!(
            script.starts_with("sh -c "),
            "the login shell may not be POSIX"
        );
        assert!(script.contains("-localhost"));
        assert!(script.contains("-passwdfile"));
        assert!(!script.contains("-nopw"));
        assert!(script.contains(OTP_PROMPT));
    }

    #[test]
    fn every_server_stays_on_the_loopback() {
        let x0 = remote_script(&vnc(VncServer::TigerScraping)).unwrap();
        assert!(
            x0.contains("-localhost=1") && x0.contains("-PasswordFile"),
            "{}",
            x0
        );
        assert!(x0.contains("vncpasswd -f"), "{}", x0);
        let wayvnc = remote_script(&vnc(VncServer::Wayvnc)).unwrap();
        assert!(wayvnc.contains("wayvnc 127.0.0.1 5900"), "{}", wayvnc);
        for server in [
            VncServer::TigerVirtual,
            VncServer::TurboVirtual,
            VncServer::TightVirtual,
        ] {
            let script = remote_script(&vnc(server)).unwrap();
            assert!(script.contains("-localhost"), "{}", script);
        }
    }

    #[test]
    fn automatic_detection_prefers_wayland_then_x11() {
        let script = remote_script(&vnc(VncServer::Auto)).unwrap();
        let wayvnc = script.find("command -v wayvnc").unwrap();
        let x11vnc = script.find("command -v x11vnc").unwrap();
        let x0 = script.find("command -v x0vncserver").unwrap();
        assert!(wayvnc < x11vnc && x11vnc < x0);
        assert!(script.contains("install x11vnc"));
    }

    #[test]
    fn a_virtual_desktop_uses_its_display_port_and_survives() {
        let mut connection = vnc(VncServer::TigerVirtual);
        connection.vnc_display = ":3".into();
        connection.vnc_geometry = "1280x800".into();
        assert_eq!(remote_port(&connection), 5903);
        let script = remote_script(&connection).unwrap();
        assert!(
            script.contains(":3") && script.contains("1280x800"),
            "{}",
            script
        );
        assert!(
            script.contains("-kill :3"),
            "says how to stop it: {}",
            script
        );
    }

    #[test]
    fn hostile_values_stay_one_shell_word() {
        let mut connection = vnc(VncServer::X11vnc);
        connection.vnc_display = ":0; touch /tmp/owned".into();
        let inner = shell_words::split(&remote_script(&connection).unwrap()).unwrap();
        assert_eq!(inner[..2], ["sh".to_string(), "-c".to_string()]);
        assert!(
            inner[2].contains("-display ':0; touch /tmp/owned'"),
            "{}",
            inner[2]
        );
    }

    #[test]
    fn custom_and_running_servers() {
        let mut connection = vnc(VncServer::Custom);
        connection.vnc_command = "myvnc --port {port}".into();
        connection.service_port = Some(5999);
        assert!(remote_script(&connection)
            .unwrap()
            .contains("myvnc --port 5999"));
        let running = vnc(VncServer::Running);
        assert_eq!(remote_script(&running), None);
        assert!(tunnel_argv(&running, 1).iter().any(|arg| arg == "-N"));
    }

    #[test]
    fn a_gateway_tunnels_to_the_host_behind_it() {
        let connection = Connection {
            name: "win".into(),
            protocol: Protocol::Rdp,
            host: "10.0.0.5".into(),
            proxy_jump: "bastion".into(),
            ..Connection::default()
        };
        assert!(uses_tunnel(&connection));
        let argv = tunnel_argv(&connection, 13389);
        assert!(argv
            .windows(2)
            .any(|pair| pair == ["-L", "127.0.0.1:13389:10.0.0.5:3389"]));
        assert_eq!(argv.last().map(String::as_str), Some("bastion"));
        assert!(!uses_tunnel(&Connection {
            protocol: Protocol::Rdp,
            ..Connection::default()
        }));
        assert!(!uses_tunnel(&vnc(VncServer::Direct)));
    }

    #[test]
    fn freerdp_asks_for_the_password_on_its_terminal() {
        let connection = Connection {
            name: "win".into(),
            protocol: Protocol::Rdp,
            user: "admin".into(),
            rdp_domain: "CORP".into(),
            rdp_clipboard: true,
            rdp_drive: "/home/a".into(),
            desktop_fullscreen: true,
            ..Connection::default()
        };
        let mut argv = vec!["xfreerdp3".to_string()];
        let mut prompt = None;
        freerdp_args(
            &connection,
            "h",
            3389,
            &mut argv,
            &mut prompt,
            Some("s3cret".into()),
        );
        for expected in ["/v:h:3389", "/u:admin", "/d:CORP", "/f", "+clipboard"] {
            assert!(
                argv.iter().any(|arg| arg == expected),
                "{}: {:?}",
                expected,
                argv
            );
        }
        assert!(
            !argv.iter().any(|arg| arg == "/from-stdin"),
            "the password is typed at the prompt, not piped: {:?}",
            argv
        );
        assert!(
            !argv.iter().any(|arg| arg == "/cert:tofu"),
            "the certificate question must reach the terminal: {:?}",
            argv
        );
        assert!(!argv.iter().any(|arg| arg.contains("s3cret")));
        assert_eq!(prompt.as_deref(), Some("s3cret"));
    }

    #[test]
    fn rdesktop_asks_for_the_password_on_its_terminal() {
        let connection = Connection {
            name: "win".into(),
            protocol: Protocol::Rdp,
            user: "admin".into(),
            ..Connection::default()
        };
        let mut argv = Vec::new();
        let mut prompt = None;
        rdesktop_args(
            &connection,
            "h",
            3389,
            &mut argv,
            &mut prompt,
            Some("s3cret".into()),
        );
        assert!(!argv.windows(2).any(|pair| pair == ["-p", "-"]));
        assert!(!argv.iter().any(|arg| arg.contains("s3cret")));
        assert_eq!(prompt.as_deref(), Some("s3cret"));
        assert_eq!(argv.last().map(String::as_str), Some("h:3389"));
    }

    #[test]
    fn one_time_passwords_fit_vnc_and_differ() {
        let _guard = fake_path();
        let first = one_time_password();
        assert_eq!(first.len(), 8);
        assert_ne!(first, one_time_password());
    }

    #[test]
    fn the_password_prompt_is_answered_but_a_gateway_prompt_is_not() {
        let prompt = regex::Regex::new(PASSWORD_PROMPT).unwrap();
        assert!(prompt.is_match("connecting\nPassword:        "));
        assert!(prompt.is_match("Password: "));
        assert!(
            !prompt.is_match("GatewayPassword:        "),
            "the gateway has its own password"
        );
    }

    #[test]
    fn only_a_real_vnc_greeting_counts_as_ready() {
        use std::io::Write;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let silent = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            first.write_all(b"SSH").unwrap();
            let (mut second, _) = listener.accept().unwrap();
            second.write_all(b"RFB 003.008\n").unwrap();
        });
        assert!(!service_is_ready(Protocol::Vnc, port));
        assert!(service_is_ready(Protocol::Vnc, port));
        silent.join().unwrap();
        assert!(
            !service_is_ready(Protocol::Rdp, 1),
            "nothing listens on port 1"
        );
    }
}
