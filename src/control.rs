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

use crate::logging::LOGGER;
use crate::remote;
use crate::terminal::TerminalBox;
use crate::window::{DetachedWindow, MainWindow};

/// Capture may legitimately return a few megabytes of scrollback, so the
/// control protocol has its own, larger frame limit than the PTY broker.
const MAX_FRAME: usize = 64 * 1024 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq)]
pub enum CaptureScope {
    Screen,
    Lines(usize),
    All,
}

#[derive(Debug, PartialEq)]
pub enum CliMode {
    Help(&'static str),
    New {
        cwd: Option<String>,
        title: Option<String>,
        command: Vec<String>,
    },
    Split {
        pane: Option<String>,
        vertical: bool,
        cwd: Option<String>,
        title: Option<String>,
        command: Vec<String>,
    },
    Send {
        pane: Option<String>,
        enter: bool,
        text: String,
    },
    Capture {
        pane: Option<String>,
        scope: CaptureScope,
    },
    Wait {
        pane: Option<String>,
        timeout: Option<Duration>,
    },
}

// ── CLI parsing ──────────────────────────────────────────────

pub fn parse_cli_mode(args: &[String]) -> Option<Result<CliMode, String>> {
    let verb = args.get(1)?.as_str();
    if !matches!(verb, "new" | "split" | "send" | "capture" | "wait") {
        return None;
    }
    Some(parse_verb(&args[1..]))
}

fn parse_verb(args: &[String]) -> Result<CliMode, String> {
    let verb = &args[0];
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    let mut pane: Option<String> = None;
    let mut vertical = true;
    let mut enter = false;
    let mut scope = CaptureScope::Screen;
    let mut timeout: Option<Duration> = None;
    let mut command: Vec<String> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    let mut read_stdin = false;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(verb_usage(verb))),
            "--cwd" if verb == "new" || verb == "split" => {
                i += 1;
                let value = args.get(i).ok_or("--cwd requires a directory")?;
                cwd = Some(expand_dir(value)?);
            }
            "--title" if verb == "new" || verb == "split" => {
                i += 1;
                title = Some(args.get(i).ok_or("--title requires a value")?.to_string());
            }
            "--pane" if verb != "new" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            "--vertical" if verb == "split" => vertical = true,
            "--horizontal" if verb == "split" => vertical = false,
            "--enter" if verb == "send" => enter = true,
            "--all" if verb == "capture" => scope = CaptureScope::All,
            "--scrollback" if verb == "capture" => {
                i += 1;
                let value = args.get(i).ok_or("--scrollback requires a line count")?;
                let lines = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --scrollback '{}'", value))?;
                scope = CaptureScope::Lines(lines);
            }
            "--timeout" if verb == "wait" => {
                i += 1;
                let value = args.get(i).ok_or("--timeout requires seconds")?;
                let secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout '{}': expected seconds", value))?;
                timeout = Some(Duration::from_secs(secs));
            }
            "--" if verb == "new" || verb == "split" => {
                command = args[i + 1..].to_vec();
                if command.is_empty() {
                    return Err("-- must be followed by a command".to_string());
                }
                break;
            }
            "--" if verb == "send" => {
                text_parts = args[i + 1..].to_vec();
                break;
            }
            _ if verb == "send" => {
                if arg == "-" {
                    read_stdin = true;
                } else if arg.starts_with('-') {
                    return Err(format!("unknown option: {}", arg));
                } else {
                    text_parts.push(arg.clone());
                }
            }
            _ if verb == "new" && !arg.starts_with('-') => {
                // `oxterm new DIR` is a convenient shorthand for --cwd.
                if cwd.is_some() {
                    return Err(format!("unexpected argument: {}", arg));
                }
                cwd = Some(expand_dir(arg)?);
            }
            _ => return Err(format!("unknown option: {}", arg)),
        }
        i += 1;
    }

    match verb.as_str() {
        "new" => Ok(CliMode::New {
            cwd,
            title,
            command,
        }),
        "split" => Ok(CliMode::Split {
            pane,
            vertical,
            cwd,
            title,
            command,
        }),
        "send" => {
            let text = if read_stdin {
                let mut buffer = String::new();
                std::io::stdin()
                    .read_to_string(&mut buffer)
                    .map_err(|e| format!("cannot read standard input: {}", e))?;
                buffer
            } else {
                text_parts.join(" ")
            };
            if text.is_empty() && !enter {
                return Err("nothing to send".to_string());
            }
            Ok(CliMode::Send { pane, enter, text })
        }
        "capture" => Ok(CliMode::Capture { pane, scope }),
        "wait" => Ok(CliMode::Wait { pane, timeout }),
        _ => unreachable!(),
    }
}

fn expand_dir(raw: &str) -> Result<String, String> {
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_default()
    } else if raw == "~" {
        dirs::home_dir().unwrap_or_default()
    } else {
        PathBuf::from(raw)
    };
    if !expanded.is_dir() {
        return Err(format!("not a directory: {}", raw));
    }
    std::fs::canonicalize(&expanded)
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|e| format!("cannot resolve {}: {}", raw, e))
}

fn verb_usage(verb: &str) -> &'static str {
    match verb {
        "new" => {
            "Usage: oxterm new [DIR] [--cwd DIR] [--title TITLE] [-- COMMAND...]\n\n\
             Open a new tab in the running Oxterm and print its pane id. DIR is a\n\
             shorthand for --cwd.\n"
        }
        "split" => {
            "Usage: oxterm split [--pane ID] [--vertical|--horizontal] [--cwd DIR]\n\
             \x20            [--title TITLE] [-- COMMAND...]\n\n\
             Split the target pane's window (which must be unsplit) and print the\n\
             new pane id. Defaults to the pane in $OXTERM_PANE and to a vertical split.\n"
        }
        "send" => {
            "Usage: oxterm send [--pane ID] [--enter] TEXT...\n\
             \x20      oxterm send [--pane ID] [--enter] -\n\n\
             Type TEXT into the target pane, or standard input with '-'. --enter\n\
             appends a carriage return, as if Enter had been pressed.\n"
        }
        "capture" => {
            "Usage: oxterm capture [--pane ID] [--scrollback N | --all]\n\n\
             Print the target pane's text: the visible screen by default, N lines\n\
             of history with --scrollback, or the whole buffer with --all.\n"
        }
        "wait" => {
            "Usage: oxterm wait [--pane ID] [--timeout SECONDS]\n\n\
             Wait for the next command in the pane to finish and exit with its\n\
             status. Without a timeout it waits indefinitely.\n"
        }
        _ => "Usage: oxterm new|split|send|capture|wait\n",
    }
}

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
        CliMode::Send { pane, enter, text } => {
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
            let (scope_name, lines) = match scope {
                CaptureScope::Screen => ("screen", 0),
                CaptureScope::Lines(count) => ("lines", count),
                CaptureScope::All => ("all", 0),
            };
            let request = json!({
                "op": "capture",
                "pane": target.id,
                "scope": scope_name,
                "lines": lines,
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
    }
}

fn fail(code: i32, verb: &str, message: &str) -> i32 {
    eprintln!("oxterm {}: {}", verb, message);
    code
}

struct Target {
    socket: PathBuf,
    id: String,
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
            let exit = reply["exit"].as_i64().unwrap_or(0);
            return exit.clamp(0, 255) as i32;
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
    let payload = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if payload.len() > MAX_FRAME {
        return Err("control request is too large".to_string());
    }
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(&payload))
        .map_err(|e| e.to_string())
}

fn read_json(stream: &mut impl Read) -> Result<Value, String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err("control frame is too large".to_string());
    }
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).map_err(|e| e.to_string())?;
    serde_json::from_slice(&payload).map_err(|e| e.to_string())
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
                    thread::spawn(move || handle_connection(stream));
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
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    let reply = match read_json(&mut stream) {
        Ok(request) => {
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

fn dispatch(request: &Value) -> Value {
    let app = APP.with(|slot| slot.borrow().clone());
    let Some(app) = app else {
        return error_reply("no running Oxterm window", 4);
    };
    match request["op"].as_str().unwrap_or("") {
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
        "new" => {
            let Some(window) = target_window(&app) else {
                return error_reply("no running Oxterm window", 4);
            };
            let cwd = request["cwd"].as_str();
            let title = request["title"].as_str();
            let command = request_command(request);
            let term = window.add_new_tab(cwd, None, title, title, command.as_ref());
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
            window.present();
            json!({ "ok": true, "pane": term.remote_id() })
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
            let text = term.capture_text(scope, lines);
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

fn terminals(app: &gtk::Application) -> Vec<(Option<MainWindow>, TerminalBox)> {
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
        "status": term.remote_status(),
        "app_status": term.remote_command_status(),
        "exit": term.last_exit_code(),
        "generation": term.command_generation(),
        "alive": term.is_alive(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_new_with_cwd_title_and_command() {
        let mode = parse_verb(&args(&[
            "new", "--cwd", "/tmp", "--title", "Build", "--", "make", "test",
        ]))
        .unwrap();
        assert_eq!(
            mode,
            CliMode::New {
                cwd: Some("/tmp".to_string()),
                title: Some("Build".to_string()),
                command: args(&["make", "test"]),
            }
        );
    }

    #[test]
    fn parses_split_defaults_and_orientation() {
        let mode = parse_verb(&args(&["split", "--horizontal", "--pane", "123"])).unwrap();
        assert_eq!(
            mode,
            CliMode::Split {
                pane: Some("123".to_string()),
                vertical: false,
                cwd: None,
                title: None,
                command: Vec::new(),
            }
        );
        assert!(matches!(
            parse_verb(&args(&["split"])).unwrap(),
            CliMode::Split { vertical: true, .. }
        ));
    }

    #[test]
    fn parses_send_text_and_options() {
        let mode = parse_verb(&args(&["send", "--pane", "1", "--enter", "make", "test"])).unwrap();
        assert_eq!(
            mode,
            CliMode::Send {
                pane: Some("1".to_string()),
                enter: true,
                text: "make test".to_string(),
            }
        );
        assert!(parse_verb(&args(&["send"])).is_err());
    }

    #[test]
    fn parses_capture_scopes_and_wait_timeout() {
        assert_eq!(
            parse_verb(&args(&["capture", "--scrollback", "500"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::Lines(500),
            }
        );
        assert_eq!(
            parse_verb(&args(&["capture", "--all"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::All,
            }
        );
        assert_eq!(
            parse_verb(&args(&["wait", "--timeout", "5"])).unwrap(),
            CliMode::Wait {
                pane: None,
                timeout: Some(Duration::from_secs(5)),
            }
        );
    }

    #[test]
    fn rejects_unknown_options_and_missing_values() {
        assert!(parse_verb(&args(&["split", "--bogus"])).is_err());
        assert!(parse_verb(&args(&["send", "--pane"])).is_err());
        assert!(parse_verb(&args(&["capture", "--scrollback", "many"])).is_err());
        assert!(parse_verb(&args(&["new", "--cwd", "/definitely/not/a/dir"])).is_err());
    }
}
