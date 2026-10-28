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

use crate::headless::control_cli::{CaptureScope, CliMode};
use crate::headless::framing;
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
        // The FIFO is the shell integration channel; another window opens it
        // before the handover so a moved tab keeps its blocks and history.
        "osc133_fifo": term.osc133_fifo().unwrap_or_default(),
    })
}
