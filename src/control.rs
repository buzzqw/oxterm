//! Scriptable control API: `oxterm new|split|send|capture|wait`.
//!
//! Every Oxterm GUI process publishes a private Unix socket next to the PTY
//! broker sockets. A request is a length-prefixed JSON object; the GUI thread
//! performs it and replies with JSON. Shells started by Oxterm receive
//! `OXTERM_PANE` and `OXTERM_CONTROL`, so a tool running inside a pane can
//! target that pane without knowing its id.
//!
//! The API is deliberately small and local: it never listens on a network
//! socket, the directory and socket are owned by the current user, and peers
//! are checked with SO_PEERCRED before a request is read.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Once;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use gtk::prelude::*;
use zoha_vte::traits::TerminalExt;

use crate::headless::connections::{self, Kind};
use crate::headless::control_cli::{ActionCommand, CaptureScope, CliMode, TunnelAction};
use crate::headless::framing;
use crate::headless::ssh_config;
use crate::headless::tunnels;
use crate::logging::LOGGER;
use crate::remote;
use crate::terminal::TerminalBox;
use crate::window::{DetachedWindow, MainWindow};

// The parsing half lives in the headless core so it can be tested without a
// display; re-export it for `main`.
pub use crate::headless::control_cli::parse_cli_mode;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

// ── Client ───────────────────────────────────────────────────

pub fn run_cli(mode: CliMode) -> i32 {
    match mode {
        CliMode::Help(text) => {
            print!("{}", text);
            0
        }
        CliMode::New {
            cwd,
            title,
            command,
        } => {
            let request = json!({
                "op": "new",
                "cwd": cwd,
                "title": title,
                "command": command,
            });
            match client_new(&request) {
                Ok(pane) => {
                    println!("{}", pane);
                    0
                }
                Err((code, message)) => fail(code, "new", &message),
            }
        }
        CliMode::Split {
            pane,
            vertical,
            cwd,
            title,
            command,
        } => {
            let target = match resolve_pane(pane.as_deref()) {
                Ok(target) => target,
                Err(error) => return fail(error.0, "split", &error.1),
            };
            let request = json!({
                "op": "split",
                "pane": target.id,
                "vertical": vertical,
                "cwd": cwd,
                "title": title,
                "command": command,
            });
            match request_at(&target.socket, &request) {
                Ok(reply) => {
                    println!("{}", reply["pane"].as_str().unwrap_or(""));
                    0
                }
                Err(error) => fail(error.0, "split", &error.1),
            }
        }
        CliMode::Connect {
            name,
            split,
            pane,
            yes,
        } => run_connect(&name, split, pane.as_deref(), yes),
        CliMode::Connections { json } => run_connections(json),
        CliMode::ConnectionsExport => run_connections_export(),
        CliMode::ConnectionsImport { file, allow_local } => {
            run_connections_import(&file, allow_local)
        }
        CliMode::Tunnel {
            action,
            name,
            index,
        } => run_tunnel(action, name.as_deref(), index),
        CliMode::Send { pane, enter, text } => {
            let text = match text {
                Some(text) => text,
                None => {
                    let mut buffer = String::new();
                    if let Err(error) = std::io::stdin().read_to_string(&mut buffer) {
                        return fail(1, "send", &format!("cannot read standard input: {}", error));
                    }
                    buffer
                }
            };
            let target = match resolve_pane(pane.as_deref()) {
                Ok(target) => target,
                Err(error) => return fail(error.0, "send", &error.1),
            };
            let request = json!({
                "op": "send",
                "pane": target.id,
                "text": text,
                "enter": enter,
            });
            match request_at(&target.socket, &request) {
                Ok(_) => 0,
                Err(error) => fail(error.0, "send", &error.1),
            }
        }
        CliMode::Capture { pane, scope } => {
            let target = match resolve_pane(pane.as_deref()) {
                Ok(target) => target,
                Err(error) => return fail(error.0, "capture", &error.1),
            };
            let (scope_name, lines, clear) = match scope {
                CaptureScope::Screen => ("screen", 0, false),
                CaptureScope::Lines(count) => ("lines", count, false),
                CaptureScope::All => ("all", 0, false),
                CaptureScope::Captured { clear } => ("captured", 0, clear),
            };
            let request = json!({
                "op": "capture",
                "pane": target.id,
                "scope": scope_name,
                "lines": lines,
                "clear": clear,
            });
            match request_at(&target.socket, &request) {
                Ok(reply) => {
                    print!("{}", reply["text"].as_str().unwrap_or(""));
                    0
                }
                Err(error) => fail(error.0, "capture", &error.1),
            }
        }
        CliMode::Wait { pane, timeout } => {
            let target = match resolve_pane(pane.as_deref()) {
                Ok(target) => target,
                Err(error) => return fail(error.0, "wait", &error.1),
            };
            wait_for_command(&target, timeout)
        }
        CliMode::Events {
            pane,
            events,
            output,
            json,
        } => {
            let socket = match events_socket(pane.as_deref()) {
                Ok(socket) => socket,
                Err(error) => return fail(error.0, "events", &error.1),
            };
            run_events(&socket, &events, output, json)
        }
        CliMode::Action { pane, command } => run_action(pane.as_deref(), command),
        CliMode::Workspace {
            file,
            new_window,
            replace,
        } => run_workspace(&file, new_window, replace),
    }
}

fn fail(code: i32, verb: &str, message: &str) -> i32 {
    eprintln!("oxterm {}: {}", verb, message);
    code
}

fn events_socket(pane: Option<&str>) -> Result<PathBuf, (i32, String)> {
    if pane.is_some() {
        return resolve_pane(pane).map(|target| target.socket);
    }
    live_sockets()
        .into_iter()
        .next()
        .ok_or((4, "no running Oxterm window".to_string()))
}

fn run_events(path: &Path, names: &[String], output: bool, _json: bool) -> i32 {
    use std::os::unix::net::UnixStream;
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error) => {
            return fail(
                4,
                "events",
                &format!("cannot reach the Oxterm control socket: {}", error),
            )
        }
    };
    let request = json!({
        "op": "subscribe",
        "events": if names.is_empty() { Value::Null } else { json!(names) },
        "include_output": output,
    });
    if let Err(error) = write_json(&mut stream, &request) {
        return fail(1, "events", &error);
    }
    loop {
        match read_json(&mut stream) {
            Ok(reply) => {
                if reply["ok"].as_bool() == Some(false) {
                    let code = reply["code"].as_i64().unwrap_or(1) as i32;
                    return fail(
                        code,
                        "events",
                        reply["error"].as_str().unwrap_or("subscription refused"),
                    );
                }
                // The first reply is the acknowledgement, not an event.
                if reply.get("event").is_none() {
                    continue;
                }
                if reply["event"].as_str() == Some("heartbeat") {
                    continue;
                }
                // The stream is JSON Lines so `jq` works without a flag;
                // `--json` is accepted only for symmetry with other verbs.
                println!("{}", reply);
            }
            Err(error) => {
                // A clean end of stream is not an error: the window closed.
                if error.contains("failed to fill whole buffer")
                    || error.contains("unexpected end of file")
                    || error.contains("Connection reset by peer")
                {
                    return 0;
                }
                return fail(1, "events", &error);
            }
        }
    }
}

fn run_action(pane: Option<&str>, command: ActionCommand) -> i32 {
    let needs_pane = !matches!(
        &command,
        ActionCommand::Notify { .. } | ActionCommand::OpenUrl { .. }
    );
    let target = if needs_pane || pane.is_some() {
        match resolve_pane(pane) {
            Ok(target) => Some(target),
            Err(error) => return fail(error.0, "action", &error.1),
        }
    } else {
        None
    };
    let socket = match &target {
        Some(target) => target.socket.clone(),
        None => match live_sockets().into_iter().next() {
            Some(socket) => socket,
            None => return fail(4, "action", "no running Oxterm window"),
        },
    };
    let pane_id = target.as_ref().map(|target| target.id.clone());
    let request = match command {
        ActionCommand::SetTabColor { color } => json!({
            "op": "action", "action": "set-tab-color", "pane": pane_id, "color": color,
        }),
        ActionCommand::SetTabTitle { title } => json!({
            "op": "action", "action": "set-tab-title", "pane": pane_id, "title": title,
        }),
        ActionCommand::Notify { title, body } => json!({
            "op": "action", "action": "notify", "pane": pane_id, "title": title, "body": body,
        }),
        ActionCommand::OpenUrl { url } => json!({
            "op": "action", "action": "open-url", "pane": pane_id, "url": url,
        }),
        ActionCommand::InsertText { text } => {
            let text = match text {
                Some(text) => text,
                None => {
                    let mut buffer = String::new();
                    if let Err(error) = std::io::stdin().read_to_string(&mut buffer) {
                        return fail(
                            1,
                            "action",
                            &format!("cannot read standard input: {}", error),
                        );
                    }
                    buffer
                }
            };
            json!({
                "op": "action", "action": "insert-text", "pane": pane_id, "text": text,
            })
        }
        ActionCommand::MarkFailure => json!({
            "op": "action", "action": "mark-failure", "pane": pane_id,
        }),
        ActionCommand::ClearFailure => json!({
            "op": "action", "action": "clear-failure", "pane": pane_id,
        }),
    };
    match request_at(&socket, &request) {
        Ok(_) => 0,
        Err(error) => fail(error.0, "action", &error.1),
    }
}

fn run_workspace(file: &str, new_window: bool, replace: bool) -> i32 {
    let path = PathBuf::from(file);
    let raw = match crate::headless::workspace::read(&path) {
        Ok(raw) => raw,
        Err(error) => return fail(2, "workspace", &error),
    };
    if let Err(error) = crate::headless::workspace::parse(&raw) {
        return fail(2, "workspace", &error);
    }
    let socket = if new_window || live_sockets().is_empty() {
        match spawn_workspace_window() {
            Ok(socket) => {
                // The new window creates its first terminal from an idle
                // callback. Wait for it, so `replace` can close it after the
                // workspace tabs are open.
                wait_for_window_ready(&socket);
                socket
            }
            Err(error) => return fail(error.0, "workspace", &error.1),
        }
    } else {
        match live_sockets().into_iter().next() {
            Some(socket) => socket,
            None => return fail(4, "workspace", "no running Oxterm window"),
        }
    };
    let request = json!({
        "op": "workspace",
        "document": raw,
        "replace": replace,
    });
    match request_at_with_timeout(&socket, &request, Duration::from_secs(60)) {
        Ok(_) => 0,
        Err(error) => fail(error.0, "workspace", &error.1),
    }
}

/// Start a clean Oxterm window and wait for its private control socket.
fn spawn_workspace_window() -> Result<PathBuf, (i32, String)> {
    let executable = std::env::current_exe()
        .map_err(|error| (1, format!("cannot find the oxterm binary: {}", error)))?;
    let child = std::process::Command::new(executable)
        .args(["--new-window", "--no-restore"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| (1, format!("cannot start a new Oxterm window: {}", error)))?;
    let pid = child.id() as i32;
    let wanted = format!("control-{}.sock", pid);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(socket) = live_sockets()
            .into_iter()
            .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(wanted.as_str()))
        {
            return Ok(socket);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err((
        4,
        "the new Oxterm window did not publish a control socket in time".to_string(),
    ))
}

fn wait_for_window_ready(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(reply) =
            request_at_with_timeout(socket, &json!({ "op": "list" }), Duration::from_secs(2))
        {
            if reply["panes"]
                .as_array()
                .is_some_and(|panes| !panes.is_empty())
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Target {
    socket: PathBuf,
    id: String,
}

/// The book merged with `~/.ssh/config`, for the client side of the commands.
fn client_entries() -> Result<Vec<connections::Entry>, (i32, String)> {
    let book = connections::load_book(&connections::book_path()).map_err(|error| (1, error))?;
    Ok(connections::merge(&book, &ssh_config::load_hosts()))
}

fn run_connect(name: &str, split: bool, pane: Option<&str>, yes: bool) -> i32 {
    let entries = match client_entries() {
        Ok(entries) => entries,
        Err(error) => return fail(error.0, "connect", &error.1),
    };
    let Some(entry) = connections::find_entry(&entries, name) else {
        return fail(3, "connect", &format!("no connection named {}", name));
    };
    let connection = &entry.connection;
    if let Err(error) = connection.validate() {
        return fail(1, "connect", &error);
    }
    if connection.confirm_connect && !yes {
        return fail(
            1,
            "connect",
            &format!("{} asks for confirmation; pass --yes", connection.name),
        );
    }
    // A desktop or a file browser is a window of its own, opened by the GUI.
    if connection.protocol.is_desktop() || connection.protocol.is_file_transfer() {
        let Some(socket) = live_sockets().into_iter().next() else {
            return fail(4, "connect", "no running Oxterm window");
        };
        return match request_at(
            &socket,
            &json!({ "op": "open_connection", "id": connection.id }),
        ) {
            Ok(_) => 0,
            Err(error) => fail(error.0, "connect", &error.1),
        };
    }
    // Resolved here and not by the GUI: `ssh -G` can block, and a control request
    // is answered on the GUI thread.
    for problem in crate::headless::preconnect::run(connection) {
        eprintln!("oxterm connect: {}", problem);
    }
    let prepared = connections::prepare_session(connection);
    if let Some(note) = prepared.keyring_note {
        eprintln!("oxterm connect: {}", note);
    }
    let result = if split {
        let target = match resolve_pane(pane) {
            Ok(target) => target,
            Err(error) => return fail(error.0, "connect", &error.1),
        };
        request_at(
            &target.socket,
            &json!({
                "op": "split",
                "pane": target.id,
                "vertical": true,
                "title": connection.name,
                "command": prepared.plan.argv,
                "connection_id": connection.id,
            }),
        )
        .map(|reply| reply["pane"].as_str().unwrap_or_default().to_string())
    } else {
        client_new(&json!({
            "op": "new",
            "title": connection.name,
            "command": prepared.plan.argv,
            "connection_id": connection.id,
        }))
    };
    match result {
        Ok(pane) => {
            if let Err(error) =
                connections::record_usage(&connections::usage_path(), &connection.id)
            {
                LOGGER.warning(&format!("connection_usage_failed error={}", error));
            }
            for (index, tunnel) in connection.tunnels.iter().enumerate() {
                if tunnel.autostart {
                    if let Some(socket) = live_sockets().into_iter().next() {
                        let _ = request_at(
                            &socket,
                            &json!({
                                "op": "tunnel",
                                "action": "start",
                                "id": connection.id,
                                "index": index,
                            }),
                        );
                    }
                }
            }
            println!("{}", pane);
            0
        }
        Err(error) => fail(error.0, "connect", &error.1),
    }
}

fn run_connections(as_json: bool) -> i32 {
    let entries = match client_entries() {
        Ok(entries) => entries,
        Err(error) => return fail(error.0, "connections", &error.1),
    };
    if as_json {
        let rows: Vec<Value> = entries
            .iter()
            .map(|entry| {
                let connection = &entry.connection;
                json!({
                    "id": connection.id,
                    "name": connection.name,
                    "kind": if connection.kind == Kind::Alias { "alias" } else { "manual" },
                    "destination": connection.destination(),
                    "group": connection.group,
                    "tags": connection.tags,
                    "favorite": connection.favorite,
                    "command": connection.command_preview(),
                    "tunnels": connection.tunnels.iter().map(|tunnel| tunnel.describe()).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", Value::Array(rows));
        return 0;
    }
    let width = entries
        .iter()
        .map(|entry| entry.connection.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut sorted: Vec<&connections::Entry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.connection.name.to_lowercase());
    for entry in sorted {
        let connection = &entry.connection;
        let mut line = format!("{:<width$}  {}", connection.name, connection.summary());
        if !connection.group.is_empty() {
            line.push_str(&format!("  [{}]", connection.group));
        }
        println!("{}", line.trim_end());
    }
    0
}

fn run_connections_export() -> i32 {
    match connections::load_book(&connections::book_path()) {
        Ok(book) => match serde_json::to_string_pretty(&book) {
            Ok(text) => {
                println!("{}", text);
                0
            }
            Err(error) => fail(1, "connections", &error.to_string()),
        },
        Err(error) => fail(1, "connections", &error),
    }
}

fn run_connections_import(file: &str, allow_local: bool) -> i32 {
    let incoming = match connections::load_book(std::path::Path::new(file)) {
        Ok(book) => book,
        Err(error) => return fail(1, "connections", &error),
    };
    if !std::path::Path::new(file).exists() {
        return fail(1, "connections", &format!("{} does not exist", file));
    }
    let report = connections::update_book(&connections::book_path(), |book| {
        connections::import_into(book, incoming, allow_local)
    });
    match report {
        Ok(report) => {
            println!("imported {}", report.imported);
            for reason in &report.skipped {
                eprintln!("skipped {}", reason);
            }
            0
        }
        Err(error) => fail(1, "connections", &error),
    }
}

fn run_tunnel(action: TunnelAction, name: Option<&str>, index: Option<usize>) -> i32 {
    let entries = match client_entries() {
        Ok(entries) => entries,
        Err(error) => return fail(error.0, "tunnel", &error.1),
    };
    let selected = match name {
        Some(name) => match connections::find_entry(&entries, name) {
            Some(entry) => Some(entry),
            None => return fail(3, "tunnel", &format!("no connection named {}", name)),
        },
        None => None,
    };
    let socket = live_sockets().into_iter().next();
    if action == TunnelAction::List {
        // Tunnels belong to a GUI process; without one, every tunnel is stopped.
        let live = socket.as_ref().and_then(|socket| {
            request_at(socket, &json!({ "op": "tunnel", "action": "list" })).ok()
        });
        for entry in &entries {
            if selected.is_some_and(|selected| selected.connection.id != entry.connection.id) {
                continue;
            }
            for (position, tunnel) in entry.connection.tunnels.iter().enumerate() {
                let status = live
                    .as_ref()
                    .and_then(|reply| reply["tunnels"].as_array())
                    .and_then(|rows| {
                        rows.iter().find(|row| {
                            row["name"].as_str() == Some(entry.connection.name.as_str())
                                && row["index"].as_u64() == Some(position as u64)
                        })
                    })
                    .and_then(|row| row["status"].as_str())
                    .unwrap_or("stopped");
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.connection.name,
                    position,
                    tunnel.describe(),
                    status
                );
            }
        }
        return 0;
    }
    let Some(socket) = socket else {
        return fail(4, "tunnel", "no running Oxterm window");
    };
    let Some(entry) = selected else {
        return fail(2, "tunnel", "a connection name is required");
    };
    let request = json!({
        "op": "tunnel",
        "action": if action == TunnelAction::Start { "start" } else { "stop" },
        "id": entry.connection.id,
        "index": index,
    });
    match request_at(&socket, &request) {
        Ok(_) => 0,
        Err(error) => fail(error.0, "tunnel", &error.1),
    }
}

fn client_new(request: &Value) -> Result<String, (i32, String)> {
    let socket = live_sockets()
        .into_iter()
        .next()
        .ok_or((4, "no running Oxterm window".to_string()))?;
    let reply = request_at(&socket, request)?;
    reply["pane"]
        .as_str()
        .map(str::to_string)
        .ok_or((1, "the GUI did not return a pane id".to_string()))
}

fn resolve_pane(requested: Option<&str>) -> Result<Target, (i32, String)> {
    let requested = requested.map(str::to_string).or_else(|| {
        std::env::var("OXTERM_PANE")
            .ok()
            .filter(|id| !id.is_empty())
    });
    let panes = all_panes();
    if panes.is_empty() {
        return Err((4, "no running Oxterm window".to_string()));
    }
    if let Some(id) = requested {
        return panes
            .into_iter()
            .find(|(_, pane)| pane["pane"].as_str() == Some(id.as_str()))
            .map(|(socket, pane)| Target {
                socket,
                id: pane["pane"].as_str().unwrap_or_default().to_string(),
            })
            .ok_or((3, format!("pane not found: {}", id)));
    }
    if panes.len() == 1 {
        let (socket, pane) = panes.into_iter().next().unwrap();
        return Ok(Target {
            socket,
            id: pane["pane"].as_str().unwrap_or_default().to_string(),
        });
    }
    let ids: Vec<String> = panes
        .iter()
        .filter_map(|(_, pane)| pane["pane"].as_str().map(str::to_string))
        .collect();
    Err((
        3,
        format!(
            "several panes are open; pass --pane ID (one of: {})",
            ids.join(", ")
        ),
    ))
}

fn all_panes() -> Vec<(PathBuf, Value)> {
    let mut panes = Vec::new();
    for socket in live_sockets() {
        let Ok(reply) = request_at(&socket, &json!({ "op": "list" })) else {
            continue;
        };
        if let Some(list) = reply["panes"].as_array() {
            for pane in list {
                panes.push((socket.clone(), pane.clone()));
            }
        }
    }
    panes
}

fn wait_for_command(target: &Target, timeout: Option<Duration>) -> i32 {
    let deadline = timeout.map(|limit| Instant::now() + limit);
    let baseline = match request_at(&target.socket, &json!({ "op": "gen", "pane": target.id })) {
        Ok(reply) => reply["generation"].as_u64().unwrap_or(0),
        Err(error) => return fail(error.0, "wait", &error.1),
    };
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return fail(1, "wait", "timed out");
        }
        thread::sleep(Duration::from_millis(120));
        let reply = match request_at(&target.socket, &json!({ "op": "gen", "pane": target.id })) {
            Ok(reply) => reply,
            Err(error) => return fail(error.0, "wait", &error.1),
        };
        let generation = reply["generation"].as_u64().unwrap_or(0);
        if generation > baseline {
            // A command whose status the shell did not report must not look
            // like a success to a script that chains on `oxterm wait`.
            return match reply["exit"].as_i64() {
                Some(exit) => exit.clamp(0, 255) as i32,
                None => fail(1, "wait", "the command finished without reporting a status"),
            };
        }
        if reply["alive"].as_bool() == Some(false) {
            return fail(1, "wait", "the pane closed before a command finished");
        }
    }
}

// ── Socket discovery / transport ─────────────────────────────

pub fn socket_path() -> PathBuf {
    remote::remote_dir().join(format!("control-{}.sock", std::process::id()))
}

fn live_sockets() -> Vec<PathBuf> {
    if let Ok(path) = std::env::var("OXTERM_CONTROL") {
        let path = PathBuf::from(path);
        if socket_is_alive(&path) {
            return vec![path];
        }
    }
    all_live_sockets()
}

/// Every running Oxterm on this machine, including this process.
///
/// `live_sockets` honours `OXTERM_CONTROL` because CLI verbs run inside a pane
/// must address the window that owns them; a menu that offers to move a tab
/// from another window wants the full list instead.
fn all_live_sockets() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(remote::remote_dir()) else {
        return Vec::new();
    };
    let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy();
            if !name.starts_with("control-") || path.extension()? != "sock" {
                return None;
            }
            if !socket_is_alive(&path) {
                return None;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            Some((modified, path))
        })
        .collect();
    sockets.sort_by(|a, b| b.0.cmp(&a.0));
    sockets.into_iter().map(|(_, path)| path).collect()
}

/// A plain connect (or a connect immediately dropped) leaves the server with a
/// thread waiting for a request, so probes exchange a tiny `ping` instead.
fn socket_is_alive(path: &Path) -> bool {
    request_at_with_timeout(path, &json!({ "op": "ping" }), Duration::from_secs(2)).is_ok()
}

fn request_at(path: &Path, request: &Value) -> Result<Value, (i32, String)> {
    request_at_with_timeout(path, request, CLIENT_TIMEOUT)
}

fn request_at_with_timeout(
    path: &Path,
    request: &Value,
    timeout: Duration,
) -> Result<Value, (i32, String)> {
    let mut stream = std::os::unix::net::UnixStream::connect(path)
        .map_err(|e| (4, format!("cannot reach the Oxterm control socket: {}", e)))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| (1, e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| (1, e.to_string()))?;
    write_json(&mut stream, request).map_err(|e| (1, e))?;
    let reply = read_json(&mut stream).map_err(|e| (1, e))?;
    if reply["ok"].as_bool() == Some(true) {
        Ok(reply)
    } else {
        Err((
            reply["code"].as_i64().unwrap_or(1) as i32,
            reply["error"]
                .as_str()
                .unwrap_or("unknown control error")
                .to_string(),
        ))
    }
}

fn write_json(stream: &mut impl Write, value: &Value) -> Result<(), String> {
    framing::write_json(stream, value, framing::MAX_RESPONSE_FRAME)
}

fn read_json(stream: &mut impl Read) -> Result<Value, String> {
    framing::read_json(stream, framing::MAX_REQUEST_FRAME)
}

// ── GUI server ───────────────────────────────────────────────

thread_local! {
    static APP: std::cell::RefCell<Option<gtk::Application>> =
        const { std::cell::RefCell::new(None) };
}

/// Remember the application on the GUI thread. Called once before the main
/// loop starts; requests are answered from that same thread.
pub fn init(app: gtk::Application) {
    APP.with(|slot| *slot.borrow_mut() = Some(app));
}

static ACTIVE_CONNECTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn active_connections() -> usize {
    ACTIVE_CONNECTIONS.load(std::sync::atomic::Ordering::SeqCst)
}

struct ConnectionGuard;

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        ACTIVE_CONNECTIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub fn start_server() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        if let Err(error) = remote::ensure_remote_dir() {
            LOGGER.warning(&format!("control_dir_failed error={}", error));
            return;
        }
        clean_stale_sockets();
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = match std::os::unix::net::UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                LOGGER.warning(&format!("control_bind_failed error={}", error));
                return;
            }
        };
        let _ =
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
        LOGGER.info(&format!("control_socket path={}", path.display()));
        thread::Builder::new()
            .name("oxterm-control".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    if !remote::peer_is_same_user(&stream) {
                        continue;
                    }
                    // Bound the threads a local process can make us spawn: a
                    // peer that opens many idle connections is simply refused.
                    if active_connections() >= framing::MAX_CONTROL_CLIENTS {
                        LOGGER.warning("control_client_limit_reached");
                        continue;
                    }
                    ACTIVE_CONNECTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let spawned = thread::Builder::new()
                        .name("oxterm-control-client".to_string())
                        .spawn(move || {
                            let _guard = ConnectionGuard;
                            handle_connection(stream);
                        });
                    if let Err(error) = spawned {
                        // The closure (and its guard) was never run.
                        ACTIVE_CONNECTIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        LOGGER.warning(&format!("control_thread_spawn_failed error={}", error));
                    }
                }
            })
            .ok();
    });
}

fn clean_stale_sockets() {
    let Ok(entries) = std::fs::read_dir(remote::remote_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        let Some(name) = name else { continue };
        if !name.starts_with("control-") || !name.ends_with(".sock") {
            continue;
        }
        // A control socket is named after its GUI process. No connect probe:
        // probing a live server would leave a handler thread waiting for a
        // request that never comes.
        let pid = name
            .strip_prefix("control-")
            .and_then(|rest| rest.strip_suffix(".sock"))
            .and_then(|pid| pid.parse::<i32>().ok());
        let alive = pid.is_some_and(|pid| pid > 0 && unsafe { libc::kill(pid, 0) } == 0);
        if !alive {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn handle_connection(mut stream: std::os::unix::net::UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(
        framing::CONTROL_READ_TIMEOUT_SECS,
    )));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    let reply = match read_json(&mut stream) {
        Ok(request) => {
            // Liveness must not depend on the GUI thread. A window that is
            // busy realising a terminal or drawing a frame is still running,
            // but a ping that has to wait for the main loop reports it as gone
            // once the client's short probe timeout expires.
            if request["op"].as_str() == Some("ping") {
                let _ = write_json(&mut stream, &json!({ "ok": true }));
                return;
            }
            if request["op"].as_str() == Some("subscribe") {
                serve_events(&mut stream, &request);
                return;
            }
            let (sender, receiver) = mpsc::channel();
            crate::run_on_main(move || {
                let reply = dispatch(&request);
                let _ = sender.send(reply);
            });
            match receiver.recv_timeout(CLIENT_TIMEOUT) {
                Ok(reply) => reply,
                Err(_) => error_reply("the GUI did not answer in time", 1),
            }
        }
        Err(error) => error_reply(&format!("invalid request: {}", error), 2),
    };
    let _ = write_json(&mut stream, &reply);
}

fn error_reply(message: &str, code: i32) -> Value {
    json!({ "ok": false, "error": message, "code": code })
}

/// Keep a control connection open and stream events until the peer goes away.
///
/// A heartbeat every 30 seconds both proves liveness and notices a client that
/// closed without reading: the write fails and the subscription is released.
fn serve_events(stream: &mut std::os::unix::net::UnixStream, request: &Value) {
    use crate::headless::events::{self, EventFilter};
    let filter = EventFilter {
        pane: request["pane"].as_str().map(str::to_string),
        events: request["events"].as_array().map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_str().map(str::to_string))
                .collect()
        }),
        include_output: request["include_output"].as_bool().unwrap_or(false),
    };
    let Some((id, receiver)) = events::bus().subscribe(filter) else {
        let _ = write_json(stream, &error_reply("too many event subscribers", 1));
        return;
    };
    if write_json(stream, &json!({ "ok": true, "subscription": id })).is_err() {
        events::bus().unsubscribe(id);
        return;
    }
    loop {
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(event) => {
                if write_json(stream, &event).is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let heartbeat =
                    json!({ "event": "heartbeat", "time": crate::logging::utc_iso_now() });
                if write_json(stream, &heartbeat).is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    events::bus().unsubscribe(id);
}

fn dispatch(request: &Value) -> Value {
    let app = APP.with(|slot| slot.borrow().clone());
    let Some(app) = app else {
        return error_reply("no running Oxterm window", 4);
    };
    match request["op"].as_str().unwrap_or("") {
        // Answered on the control thread, see `handle_connection`.
        "ping" => json!({ "ok": true }),
        "list" => {
            let panes: Vec<Value> = terminals(&app)
                .iter()
                .map(|(_, term)| pane_info(term))
                .collect();
            json!({ "ok": true, "panes": panes })
        }
        "has" => match find_terminal(&app, request["pane"].as_str().unwrap_or("")) {
            Some(_) => json!({ "ok": true }),
            None => error_reply("pane not found", 3),
        },
        "focus" => {
            let Some((window, term)) = find_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found", 3);
            };
            if let Some(window) = window {
                window.focus_terminal(&term);
            } else {
                if let Some(window) = term
                    .toplevel()
                    .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                {
                    window.present();
                }
                term.vte().grab_focus();
            }
            json!({ "ok": true })
        }
        "new" => {
            let Some(window) = target_window(&app) else {
                return error_reply("no running Oxterm window", 4);
            };
            let cwd = request["cwd"].as_str();
            let title = request["title"].as_str();
            let command = request_command(request);
            let term = window.add_new_tab(cwd, None, title, title, command.as_ref());
            if let Some(id) = request["connection_id"].as_str() {
                window.attach_connection_id(&term, id);
            }
            window.reload_connections_panel();
            window.present();
            json!({ "ok": true, "pane": term.remote_id() })
        }
        "split" => {
            let Some((window, _)) =
                find_main_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found in a main window", 3);
            };
            let mode = if request["vertical"].as_bool().unwrap_or(true) {
                "vertical"
            } else {
                "horizontal"
            };
            let cwd = request["cwd"].as_str();
            let title = request["title"].as_str();
            let command = request_command(request);
            let Some(term) = window.split_terminal(mode, cwd, command.as_ref(), title) else {
                return error_reply("this window already has two panes", 1);
            };
            if let Some(id) = request["connection_id"].as_str() {
                window.attach_connection_id(&term, id);
            }
            window.reload_connections_panel();
            window.present();
            json!({ "ok": true, "pane": term.remote_id() })
        }
        "tunnel" => handle_tunnel(&app, request),
        "action" => handle_action(&app, request),
        "workspace" => {
            let Some(window) = target_window(&app) else {
                return error_reply("no running Oxterm window", 4);
            };
            let workspace =
                match crate::headless::workspace::parse(request["document"].as_str().unwrap_or(""))
                {
                    Ok(workspace) => workspace,
                    Err(error) => return error_reply(&error, 2),
                };
            window.apply_workspace(&workspace, request["replace"].as_bool().unwrap_or(false));
            json!({ "ok": true, "name": workspace.name })
        }
        "open_connection" => {
            let Some(window) = target_window(&app) else {
                return error_reply("no running Oxterm window", 4);
            };
            window.connect_id(
                request["id"].as_str().unwrap_or(""),
                crate::connections_ui::Placement::Tab,
            );
            window.present();
            json!({ "ok": true })
        }
        "handover" => {
            let Some((_, term)) = find_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found", 3);
            };
            // Release first: closing the tab must take the view away, not the
            // shell. The destination already holds the OSC 133 FIFO open, so
            // stopping the reader here is safe.
            term.prepare_handover();
            let closing = term.clone();
            // Answer before the window disappears: closing the last tab ends
            // this process, and the adopting window needs the reply.
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                closing.call_window(|win| win.close_tab_signal(Some(&closing)));
            });
            json!({ "ok": true })
        }
        "send" => {
            let Some((_, term)) = find_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found", 3);
            };
            if !term.vte().is_input_enabled() {
                return error_reply("the terminal is read-only", 1);
            }
            let text = request["text"].as_str().unwrap_or("");
            term.control_send(text, request["enter"].as_bool().unwrap_or(false));
            json!({ "ok": true })
        }
        "capture" => {
            let Some((_, term)) = find_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found", 3);
            };
            let scope = request["scope"].as_str().unwrap_or("screen");
            let lines = request["lines"].as_u64().unwrap_or(0) as usize;
            let text = if scope == "captured" {
                term.captured_text(request["clear"].as_bool().unwrap_or(false))
            } else {
                term.capture_text(scope, lines)
            };
            json!({ "ok": true, "text": text })
        }
        "gen" => {
            let Some((_, term)) = find_terminal(&app, request["pane"].as_str().unwrap_or(""))
            else {
                return error_reply("pane not found", 3);
            };
            json!({
                "ok": true,
                "generation": term.command_generation(),
                "exit": term.last_exit_code(),
                "running": term.command_running(),
                "alive": term.is_alive(),
            })
        }
        other => error_reply(&format!("unknown operation: {}", other), 2),
    }
}

/// A pane owned by a different Oxterm process, for the "move tab here" menu.
#[derive(Clone, Debug)]
pub(crate) struct ForeignPane {
    pub socket: PathBuf,
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub command: String,
    pub running: bool,
    /// OSC 133 FIFO of the session, when the source window has integration on.
    pub fifo: Option<String>,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub color_override: Option<String>,
}

/// Panes of every other running Oxterm, sorted for a stable menu.
pub(crate) fn foreign_panes() -> Vec<ForeignPane> {
    let ours = socket_path();
    let mut panes = Vec::new();
    for socket in all_live_sockets() {
        if socket == ours {
            continue;
        }
        let Ok(reply) = request_at(&socket, &json!({ "op": "list" })) else {
            continue;
        };
        for pane in reply["panes"].as_array().into_iter().flatten() {
            let Some(id) = pane["pane"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let text = |key: &str| pane[key].as_str().unwrap_or("").trim().to_string();
            panes.push(ForeignPane {
                socket: socket.clone(),
                id: id.to_string(),
                title: text("title"),
                cwd: text("cwd"),
                command: text("command"),
                running: text("app_status") == "running",
                fifo: pane["osc133_fifo"]
                    .as_str()
                    .filter(|path| !path.is_empty())
                    .map(str::to_string),
                connection_id: pane["connection_id"].as_str().map(str::to_string),
                connection_name: pane["connection_name"].as_str().map(str::to_string),
                color_override: pane["color_override"].as_str().map(str::to_string),
            });
        }
    }
    panes.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.cwd.cmp(&b.cwd))
    });
    panes
}

/// Whether a control socket still answers. Used to tell "the other window
/// refused the move" from "the other window closed as part of the move".
pub(crate) fn socket_alive(path: &Path) -> bool {
    socket_is_alive(path)
}

/// Ask another Oxterm to release a pane and close its tab. The caller must
/// already hold the session's OSC 133 FIFO open, or the shell dies of SIGPIPE on
/// its next prompt (see `TerminalBox::open_osc133_fifo`).
pub(crate) fn handover(socket: &Path, id: &str) -> Result<(), String> {
    request_at(socket, &json!({ "op": "handover", "pane": id }))
        .map(|_| ())
        .map_err(|(_, error)| error)
}

/// Ask another Oxterm window to come forward and select one of its panes.
pub(crate) fn focus_pane(socket: &Path, id: &str) -> Result<(), String> {
    request_at(socket, &json!({ "op": "focus", "pane": id }))
        .map(|_| ())
        .map_err(|(_, error)| error)
}

fn handle_action(app: &gtk::Application, request: &Value) -> Value {
    let action = request["action"].as_str().unwrap_or("");
    if action == "notify" {
        crate::terminal::send_desktop_notification(
            request["title"].as_str().unwrap_or("Oxterm"),
            request["body"].as_str().unwrap_or(""),
        );
        return json!({ "ok": true });
    }
    if action == "open-url" {
        let Some((_, term)) = terminals(app).into_iter().next() else {
            return error_reply("no terminal is open", 3);
        };
        term.open_url(request["url"].as_str().unwrap_or(""));
        return json!({ "ok": true });
    }
    let Some((_, term)) = find_terminal(app, request["pane"].as_str().unwrap_or("")) else {
        return error_reply("pane not found", 3);
    };
    match action {
        "set-tab-color" => {
            let raw = request["color"].as_str().unwrap_or("");
            match crate::headless::triggers::parse_color(raw) {
                Ok(color) => {
                    term.set_tab_color_override(Some(color));
                    json!({ "ok": true })
                }
                Err(error) => error_reply(&error, 2),
            }
        }
        "set-tab-title" => {
            let title = request["title"].as_str().unwrap_or("").trim();
            if title.is_empty() {
                return error_reply("set-tab-title requires a title", 2);
            }
            let term2 = term.clone();
            let title = title.to_string();
            term.call_window(move |win| win.pin_tab_title_signal(&term2, &title));
            json!({ "ok": true })
        }
        "insert-text" => {
            if !term.vte().is_input_enabled() {
                return error_reply("the terminal is read-only", 1);
            }
            term.control_send(request["text"].as_str().unwrap_or(""), false);
            json!({ "ok": true })
        }
        "mark-failure" | "clear-failure" => {
            let term2 = term.clone();
            let failed = action == "mark-failure";
            term.call_window(move |win| {
                if failed {
                    win.set_tab_failure_signal(&term2, true);
                } else {
                    win.clear_tab_failure_signal(&term2);
                }
            });
            json!({ "ok": true })
        }
        other => error_reply(&format!("unknown action: {}", other), 2),
    }
}

fn handle_tunnel(app: &gtk::Application, request: &Value) -> Value {
    let action = request["action"].as_str().unwrap_or("");
    let states = |entries: &[connections::Entry]| -> Value {
        let mut rows = Vec::new();
        let snapshot = tunnels::snapshot();
        for entry in entries {
            for (index, tunnel) in entry.connection.tunnels.iter().enumerate() {
                let key = connections::tunnel_key(&entry.connection, tunnel);
                let status = snapshot
                    .get(&key)
                    .cloned()
                    .unwrap_or(tunnels::Status::Stopped);
                rows.push(json!({
                    "name": entry.connection.name,
                    "index": index,
                    "tunnel": tunnel.describe(),
                    "status": status.label(),
                }));
            }
        }
        json!({ "ok": true, "tunnels": rows })
    };
    let entries = match crate::connections_ui::load_entries() {
        Ok(entries) => entries,
        Err(error) => return error_reply(&error, 1),
    };
    if action == "list" {
        return states(&entries);
    }
    let Some(window) = target_window(app) else {
        return error_reply("no running Oxterm window", 4);
    };
    let id = request["id"].as_str().unwrap_or("");
    let Some(entry) = connections::find_entry_by_id(&entries, id) else {
        return error_reply("connection not found", 3);
    };
    let index = request["index"].as_u64().map(|index| index as usize);
    if index.is_some_and(|index| index >= entry.connection.tunnels.len()) {
        return error_reply("no tunnel with that index", 3);
    }
    if entry.connection.tunnels.is_empty() {
        return error_reply("this connection has no saved tunnels", 3);
    }
    for (position, tunnel) in entry.connection.tunnels.iter().enumerate() {
        if index.is_some_and(|index| index != position) {
            continue;
        }
        if action == "start" {
            window.start_tunnel(&entry.connection, tunnel);
        } else {
            tunnels::stop(&connections::tunnel_key(&entry.connection, tunnel));
        }
    }
    window.watch_tunnels();
    json!({ "ok": true })
}

fn request_command(request: &Value) -> Option<Vec<String>> {
    let command: Vec<String> = request["command"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (!command.is_empty()).then_some(command)
}

fn target_window(app: &gtk::Application) -> Option<MainWindow> {
    app.active_window()
        .and_then(|window| window.downcast::<MainWindow>().ok())
        .or_else(|| {
            app.windows()
                .into_iter()
                .find_map(|window| window.downcast::<MainWindow>().ok())
        })
}

pub(crate) fn terminals(app: &gtk::Application) -> Vec<(Option<MainWindow>, TerminalBox)> {
    let mut terminals = Vec::new();
    for window in app.windows() {
        if let Some(main) = window.downcast_ref::<MainWindow>() {
            for term in main.all_terminals() {
                terminals.push((Some(main.clone()), term));
            }
        } else if let Some(detached) = window.downcast_ref::<DetachedWindow>() {
            if let Some(term) = detached.terminal_box() {
                terminals.push((None, term));
            }
        }
    }
    terminals
}

fn find_terminal(app: &gtk::Application, id: &str) -> Option<(Option<MainWindow>, TerminalBox)> {
    terminals(app)
        .into_iter()
        .find(|(_, term)| term.remote_id() == id)
}

fn find_main_terminal(app: &gtk::Application, id: &str) -> Option<(MainWindow, TerminalBox)> {
    terminals(app)
        .into_iter()
        .find(|(window, term)| window.is_some() && term.remote_id() == id)
        .and_then(|(window, term)| window.map(|window| (window, term)))
}

fn pane_info(term: &TerminalBox) -> Value {
    json!({
        "pane": term.remote_id(),
        "title": term.vte().window_title().map(|title| title.to_string()).unwrap_or_default(),
        "cwd": term.get_cwd(),
        "command": term.last_command(),
        "connection_id": term.connection_id(),
        "connection_name": term.connection_name(),
        "color_override": term.color_override(),
        "status": term.remote_status(),
        "app_status": term.remote_command_status(),
        "exit": term.last_exit_code(),
        "generation": term.command_generation(),
        "alive": term.is_alive(),
        // The FIFO is the shell integration channel; another window opens it
        // before the handover so a moved tab keeps its blocks and history.
        "osc133_fifo": term.osc133_fifo().unwrap_or_default(),
    })
}
