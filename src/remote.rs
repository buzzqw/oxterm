use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::settings;

const CHILD_FD: RawFd = 3;
const GUI_FD: RawFd = 4;
const MAX_FRAME: usize = 1024 * 1024;
const MAX_PENDING: usize = 4 * 1024 * 1024;

/// Last winsize applied to the child PTY, packed as `(columns << 32) | rows`.
/// `set_child_size` becomes a no-op when the size is unchanged so that the
/// broker does not emit a `SIGWINCH` (and, for `ssh`, a window-change message)
/// on every loop iteration.
static LAST_CHILD_SIZE: AtomicU64 = AtomicU64::new(u64::MAX);

// Frames sent after ATTACH contain one of these tags followed by terminal data.
pub const FRAME_OUTPUT: u8 = 0;
pub const FRAME_INPUT: u8 = 1;
pub const FRAME_DETACH: u8 = 2;
pub const FRAME_RESIZE: u8 = 3;

#[derive(Debug)]
pub struct BrokerHandle {
    path: PathBuf,
    id: String,
    pid: i32,
}

impl BrokerHandle {
    pub fn update_info(&self, name: &str, title: &str, cwd: &str) {
        let _ = control_request(
            &self.path,
            &format!(
                "UPDATE\t{}\t{}\t{}\t{}",
                self.id,
                field(name),
                field(title),
                field(cwd)
            ),
        );
    }

    pub fn set_name(&self, name: &str) {
        let _ = control_request(&self.path, &format!("RENAME {} {}", self.id, field(name)));
    }

    pub fn update_command(&self, command: &str, running: bool) {
        let _ = control_request(
            &self.path,
            &format!(
                "COMMAND\t{}\t{}\t{}",
                self.id,
                if running { "running" } else { "last" },
                field(command)
            ),
        );
    }

    pub fn foreground_is_ssh(&self) -> bool {
        matches!(
            control_request(&self.path, &format!("IS_SSH {}", self.id)),
            Ok(response) if response == "1"
        )
    }

    pub fn local_off(&self) {
        let _ = control_request(&self.path, &format!("LOCAL_OFF {}", self.id));
    }

    pub fn local_on(&self) {
        let _ = control_request(&self.path, &format!("LOCAL_ON {}", self.id));
    }

    pub fn signal(&self, signal: i32) {
        let _ = control_request(&self.path, &format!("SIGNAL {} {}", self.id, signal));
    }

    pub fn kill(&self) {
        if control_request(&self.path, &format!("KILL {}", self.id)).is_err() {
            unsafe {
                libc::kill(self.pid, libc::SIGTERM);
            }
        }
    }
}

#[derive(Debug)]
pub enum CliMode {
    List,
    Attach(Option<String>),
    Info(String),
    Detach(String),
    Broker(PathBuf, String),
}

pub fn parse_cli_mode(args: &[String]) -> Option<Result<CliMode, String>> {
    let first = args.get(1)?.as_str();
    match first {
        "--broker" => {
            if args.len() != 4 {
                Some(Err("usage: oxterm --broker SOCKET SESSION_ID".to_string()))
            } else if args[2].is_empty() || invalid_id(&args[3]) {
                Some(Err("broker socket and session ID are required".to_string()))
            } else {
                Some(Ok(CliMode::Broker(
                    PathBuf::from(&args[2]),
                    args[3].clone(),
                )))
            }
        }
        "--list" => {
            if args.len() != 2 {
                Some(Err("--list does not accept arguments".to_string()))
            } else {
                Some(Ok(CliMode::List))
            }
        }
        "-a" | "--attach" => {
            if args.len() > 3 {
                Some(Err("usage: oxterm -a [SESSION_ID]".to_string()))
            } else if args.len() == 2 {
                Some(Ok(CliMode::Attach(None)))
            } else if invalid_id(&args[2]) {
                Some(Err("invalid session ID".to_string()))
            } else {
                Some(Ok(CliMode::Attach(Some(args[2].clone()))))
            }
        }
        "--info" | "--detach" => {
            if args.len() != 3 {
                Some(Err(format!("usage: oxterm {} SESSION_ID", first)))
            } else if invalid_id(&args[2]) {
                Some(Err("invalid session ID".to_string()))
            } else if first == "--info" {
                Some(Ok(CliMode::Info(args[2].clone())))
            } else {
                Some(Ok(CliMode::Detach(args[2].clone())))
            }
        }
        _ => None,
    }
}

fn invalid_id(value: &str) -> bool {
    value.is_empty() || value.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn remote_dir() -> PathBuf {
    settings::config_dir().join("remote")
}

fn ensure_remote_dir() -> io::Result<()> {
    std::fs::create_dir_all(remote_dir())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(remote_dir(), std::fs::Permissions::from_mode(0o700))
}

fn field(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c != '\r' && *c != '\n' && *c != '\t')
        .take(200)
        .collect()
}

pub fn new_session_id() -> String {
    format!("{}-{}", std::process::id(), monotonic_id())
}

fn monotonic_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn session_socket(session_id: &str) -> PathBuf {
    remote_dir().join(format!("trust-{}.sock", session_id))
}

/// Start the broker with the PTY ends that it must own. The GUI keeps the master
/// of the GUI PTY; the broker owns its slave and the child PTY master.
pub fn spawn_broker(
    session_id: &str,
    child_master: RawFd,
    gui_slave: RawFd,
    child_pid: i32,
    title: &str,
    cwd: &str,
) -> Result<BrokerHandle, String> {
    ensure_remote_dir().map_err(|e| format!("cannot create remote session directory: {}", e))?;
    let path = session_socket(session_id);
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Err("remote session socket is already in use".to_string());
        }
        let _ = std::fs::remove_file(&path);
    }

    let child_dup = duplicate_fd(child_master)?;
    let gui_dup = duplicate_fd(gui_slave)?;
    if unsafe { libc::dup2(child_dup, CHILD_FD) } < 0 {
        close_fd(child_dup);
        close_fd(gui_dup);
        return Err(io::Error::last_os_error().to_string());
    }
    if unsafe { libc::dup2(gui_dup, GUI_FD) } < 0 {
        close_fd(child_dup);
        close_fd(gui_dup);
        close_fd(CHILD_FD);
        return Err(io::Error::last_os_error().to_string());
    }
    close_fd(child_dup);
    close_fd(gui_dup);
    clear_cloexec(CHILD_FD);
    clear_cloexec(GUI_FD);

    let exe =
        std::env::current_exe().map_err(|e| format!("cannot find oxterm executable: {}", e))?;
    let child = std::process::Command::new(exe)
        .arg("--broker")
        .arg(&path)
        .arg(session_id)
        .env("TRUST_BROKER_CHILD_PID", child_pid.to_string())
        .env("TRUST_BROKER_TITLE", field(title))
        .env("TRUST_BROKER_CWD", field(cwd))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start PTY broker: {}", e));
    close_fd(CHILD_FD);
    close_fd(GUI_FD);
    let child = child?;
    let mut ready = false;
    for _ in 0..200 {
        if UnixStream::connect(&path).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if !ready {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        return Err("PTY broker did not create its session socket".to_string());
    }
    Ok(BrokerHandle {
        path,
        id: session_id.to_string(),
        pid: child.id() as i32,
    })
}

fn duplicate_fd(fd: RawFd) -> Result<RawFd, String> {
    let result = unsafe { libc::fcntl(fd, libc::F_DUPFD, 5) };
    if result < 0 {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(result)
    }
}

fn clear_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }
}

fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

pub fn run_cli(mode: CliMode) -> i32 {
    match mode {
        CliMode::Broker(path, id) => run_broker(&path, &id),
        CliMode::List => {
            let sessions = list_sessions();
            if sessions.is_empty() {
                println!("No active Oxterm terminals.");
            } else {
                println!("ID\tNAME\tTITLE\tDIRECTORY\tSTATUS\tAPPLICATION\tAPP_STATUS");
                for session in sessions {
                    println!("{}", session);
                }
            }
            0
        }
        CliMode::Attach(session_id) => match choose_session(session_id) {
            Ok(id) => match find_socket(&id) {
                Ok(path) => relay_terminal(&path, &id),
                Err(error) => {
                    eprintln!("oxterm attach: {}", error);
                    1
                }
            },
            Err(error) => {
                eprintln!("oxterm attach: {}", error);
                1
            }
        },
        CliMode::Info(session_id) => match find_socket(&session_id)
            .and_then(|path| control_request(&path, &format!("INFO {}", session_id)))
        {
            Ok(info) => {
                println!("ID\tNAME\tTITLE\tDIRECTORY\tSTATUS\tAPPLICATION\tAPP_STATUS");
                println!("{}", info);
                0
            }
            Err(error) => {
                eprintln!("oxterm info: {}", error);
                1
            }
        },
        CliMode::Detach(session_id) => match find_socket(&session_id)
            .and_then(|path| control_request(&path, &format!("DETACH {}", session_id)))
        {
            Ok(_) => 0,
            Err(error) => {
                eprintln!("oxterm detach: {}", error);
                1
            }
        },
    }
}

fn socket_paths() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(remote_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy().starts_with("trust-")
                    && path.extension().is_some_and(|e| e == "sock")
            })
        })
        .collect()
}

fn find_socket(session_id: &str) -> Result<PathBuf, String> {
    let expected = session_socket(session_id);
    if expected.exists() && UnixStream::connect(&expected).is_ok() {
        return Ok(expected);
    }
    for path in socket_paths() {
        let Ok(response) = control_request(&path, &format!("INFO {}", session_id)) else {
            continue;
        };
        if response.split('\t').next() == Some(session_id) {
            return Ok(path);
        }
    }
    Err("session not found".to_string())
}

pub fn list_sessions() -> Vec<String> {
    let mut sessions = Vec::new();
    for path in socket_paths() {
        if let Ok(response) = control_request(&path, "LIST") {
            sessions.extend(
                response
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_string),
            );
        }
    }
    sessions.sort();
    sessions
}

fn choose_session(requested: Option<String>) -> Result<String, String> {
    if let Some(id) = requested {
        return Ok(id);
    }
    let sessions = list_sessions();
    match sessions.len() {
        0 => Err("no active Oxterm terminals".to_string()),
        1 => sessions[0]
            .split('\t')
            .next()
            .map(str::to_string)
            .ok_or_else(|| "invalid Oxterm session listing".to_string()),
        _ => {
            println!("Active Oxterm terminals:");
            for (index, session) in sessions.iter().enumerate() {
                let fields: Vec<&str> = session.split('\t').collect();
                println!(
                    "  {:>2}) {}  {}  {}  {} [{}]  |  {} [{}]",
                    index + 1,
                    fields.first().copied().unwrap_or(""),
                    fields
                        .get(1)
                        .filter(|v| !v.is_empty())
                        .copied()
                        .unwrap_or("-"),
                    fields.get(2).copied().unwrap_or(""),
                    fields.get(3).copied().unwrap_or(""),
                    fields.get(4).copied().unwrap_or(""),
                    fields.get(5).copied().unwrap_or(""),
                    fields.get(6).copied().unwrap_or("")
                );
            }
            print!("Select terminal [1-{}]: ", sessions.len());
            io::stdout()
                .flush()
                .map_err(|e| format!("could not display selection prompt: {}", e))?;
            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .map_err(|e| format!("could not read terminal selection: {}", e))?;
            let selected = answer
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid terminal selection".to_string())?;
            if !(1..=sessions.len()).contains(&selected) {
                return Err("terminal selection is out of range".to_string());
            }
            sessions[selected - 1]
                .split('\t')
                .next()
                .map(str::to_string)
                .ok_or_else(|| "invalid Oxterm session listing".to_string())
        }
    }
}

fn control_request(path: &Path, command: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    write_frame(&mut stream, command.as_bytes())?;
    let response = read_frame(&mut stream)?;
    let text = String::from_utf8_lossy(&response);
    if let Some(error) = text.strip_prefix("ERR ") {
        Err(error.trim().to_string())
    } else if let Some(ok) = text.strip_prefix("OK\n") {
        Ok(ok.trim_end().to_string())
    } else if text.trim() == "OK" {
        Ok(String::new())
    } else {
        Err(text.trim().to_string())
    }
}

pub fn write_frame(stream: &mut impl Write, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_FRAME {
        return Err("frame is too large".to_string());
    }
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(payload))
        .map_err(|e| e.to_string())
}

pub fn read_frame(stream: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err("frame is too large".to_string());
    }
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).map_err(|e| e.to_string())?;
    Ok(payload)
}

fn relay_terminal(path: &Path, session_id: &str) -> i32 {
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("could not connect to broker: {}", error);
            return 1;
        }
    };
    if let Err(error) = write_frame(&mut stream, format!("ATTACH {}", session_id).as_bytes()) {
        eprintln!("oxterm attach: {}", error);
        return 1;
    }
    match read_frame(&mut stream) {
        Ok(response) if response == b"OK\nATTACH" => {}
        Ok(response) => {
            eprintln!("oxterm attach: {}", String::from_utf8_lossy(&response));
            return 1;
        }
        Err(error) => {
            eprintln!("oxterm attach: {}", error);
            return 1;
        }
    }

    let stdin_fd = libc::STDIN_FILENO;
    let stdout_fd = libc::STDOUT_FILENO;
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(stdin_fd, original.as_mut_ptr()) } != 0 {
        eprintln!("oxterm attach: stdin is not a terminal");
        return 1;
    }
    let original = unsafe { original.assume_init() };
    let mut raw = original;
    unsafe { libc::cfmakeraw(&mut raw) };
    if unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) } != 0 {
        return 1;
    }
    let _ = stream.set_nonblocking(true);
    let mut last_size: Option<(u16, u16)> = None;
    forward_resize(&mut stream, stdin_fd, &mut last_size);
    let mut input_buffer = Vec::new();
    let mut tmux_prefix = false;
    let mut detached = false;
    let result = 'relay: loop {
        let mut poll_fds = [
            libc::pollfd {
                fd: stdin_fd,
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
            libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
        ];
        let status = unsafe { libc::poll(poll_fds.as_mut_ptr(), 2, 250) };
        if status < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break 1;
        }
        if poll_fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut bytes = [0u8; 8192];
            let count = unsafe { libc::read(stdin_fd, bytes.as_mut_ptr() as *mut _, bytes.len()) };
            if count <= 0 {
                let _ = write_frame(&mut stream, &[FRAME_DETACH]);
                break 0;
            }
            match relay_input(&bytes[..count as usize], &mut tmux_prefix) {
                RelayAction::Forward(forwarded) => {
                    if !forwarded.is_empty() && send_input(&mut stream, &forwarded).is_err() {
                        break 0;
                    }
                }
                RelayAction::Detach(forwarded) => {
                    if !forwarded.is_empty() {
                        let _ = send_input(&mut stream, &forwarded);
                    }
                    let _ = write_frame(&mut stream, &[FRAME_DETACH]);
                    detached = true;
                    break 0;
                }
                RelayAction::PrefixDetach { upper, forwarded } => {
                    if !forwarded.is_empty() && send_input(&mut stream, &forwarded).is_err() {
                        break 0;
                    }
                    if is_child_ssh(path, session_id) {
                        let seq = if upper { b"\x02D" } else { b"\x02d" };
                        if send_input(&mut stream, seq).is_err() {
                            break 0;
                        }
                    } else {
                        let _ = write_frame(&mut stream, &[FRAME_DETACH]);
                        detached = true;
                        break 0;
                    }
                }
            }
        }
        if poll_fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut bytes = [0u8; 8192];
            loop {
                match stream.read(&mut bytes) {
                    Ok(0) => break 'relay 0,
                    Ok(count) => input_buffer.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break 'relay 0,
                }
            }
            while input_buffer.len() >= 4 {
                let length = u32::from_be_bytes(input_buffer[..4].try_into().unwrap()) as usize;
                if length > MAX_FRAME {
                    break 'relay 1;
                }
                if input_buffer.len() < 4 + length {
                    break;
                }
                let frame = input_buffer.drain(..4 + length).skip(4).collect::<Vec<_>>();
                if frame.first() == Some(&FRAME_OUTPUT) {
                    if write_all_fd(stdout_fd, &frame[1..]).is_err() {
                        break 'relay 1;
                    }
                } else if frame.first() == Some(&FRAME_DETACH) {
                    detached = true;
                    break 'relay 0;
                }
            }
        }
        forward_resize(&mut stream, stdin_fd, &mut last_size);
    };
    unsafe {
        libc::tcsetattr(stdin_fd, libc::TCSAFLUSH, &original);
    }
    if detached {
        let _ = write_all_fd(stdout_fd, b"\r\n");
    }
    result
}

enum RelayAction {
    Forward(Vec<u8>),
    Detach(Vec<u8>),
    /// The user pressed `Ctrl+B` then `d`/`D`. Pass the sequence through to
    /// the child when it is an `ssh` session (so a remote tmux detaches),
    /// otherwise detach the local client.
    PrefixDetach {
        upper: bool,
        forwarded: Vec<u8>,
    },
}

fn relay_input(bytes: &[u8], prefix: &mut bool) -> RelayAction {
    let mut forwarded = Vec::with_capacity(bytes.len());
    for byte in bytes {
        if *prefix {
            *prefix = false;
            if *byte == b'd' {
                return RelayAction::PrefixDetach {
                    upper: false,
                    forwarded,
                };
            }
            if *byte == b'D' {
                return RelayAction::PrefixDetach {
                    upper: true,
                    forwarded,
                };
            }
            if *byte == 0x04 {
                return RelayAction::Detach(forwarded);
            }
            if *byte == 0x02 || *byte == b'b' || *byte == b'B' {
                forwarded.push(0x02);
            }
        } else if *byte == 0x02 {
            *prefix = true;
        } else {
            forwarded.push(*byte);
        }
    }
    RelayAction::Forward(forwarded)
}

/// Ask the broker whether the session's child is currently `ssh`, so the relay
/// can decide whether to detach or pass `Ctrl+B`/`d` through.
fn is_child_ssh(path: &Path, session_id: &str) -> bool {
    matches!(
        control_request(path, &format!("IS_SSH {}", session_id)),
        Ok(response) if response == "1"
    )
}

/// Wrap `data` in a `FRAME_INPUT` frame and write it to the broker.
fn send_input(stream: &mut UnixStream, data: &[u8]) -> Result<(), String> {
    let mut frame = Vec::with_capacity(data.len() + 1);
    frame.push(FRAME_INPUT);
    frame.extend_from_slice(data);
    write_frame(stream, &frame)
}

fn terminal_size(fd: RawFd) -> Option<libc::winsize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
        Some(unsafe { size.assume_init() })
    } else {
        None
    }
}

fn resize_frame(columns: u16, rows: u16) -> [u8; 5] {
    let mut frame = [FRAME_RESIZE, 0, 0, 0, 0];
    frame[1..3].copy_from_slice(&columns.to_be_bytes());
    frame[3..5].copy_from_slice(&rows.to_be_bytes());
    frame
}

/// Forward the attach terminal's size to the broker, but only when it has
/// actually changed since the last time. Sending a resize frame after every
/// keystroke used to spam the child with `SIGWINCH`, which made full-screen
/// programs redraw on every key press.
fn forward_resize(stream: &mut UnixStream, fd: RawFd, last: &mut Option<(u16, u16)>) {
    let Some(size) = terminal_size(fd) else {
        return;
    };
    let current = (size.ws_col as u16, size.ws_row as u16);
    if *last == Some(current) {
        return;
    }
    *last = Some(current);
    let _ = write_frame(stream, &resize_frame(current.0, current.1));
}

fn write_all_fd(fd: RawFd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let written = unsafe { libc::write(fd, data.as_ptr() as *const _, data.len()) };
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        data = &data[written as usize..];
    }
    Ok(())
}

struct Client {
    stream: UnixStream,
    attached: bool,
    input: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    output_offset: usize,
    output_pending: usize,
}

impl Client {
    fn queue(&mut self, payload: &[u8]) -> bool {
        let frame_len = payload.len().saturating_add(4);
        if self.output_pending.saturating_add(frame_len) > MAX_PENDING {
            return false;
        }
        let mut frame = Vec::with_capacity(frame_len);
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        self.output_pending += frame_len;
        self.output.push_back(frame);
        true
    }

    fn flush(&mut self) -> bool {
        while let Some(frame) = self.output.front() {
            match self.stream.write(&frame[self.output_offset..]) {
                Ok(0) => return false,
                Ok(count) => {
                    self.output_offset += count;
                    self.output_pending = self.output_pending.saturating_sub(count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
            if self.output_offset == frame.len() {
                self.output.pop_front();
                self.output_offset = 0;
            }
        }
        true
    }
}

fn run_broker(path: &Path, id: &str) -> i32 {
    let listener = match bind_broker_socket(path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("oxterm broker: {}", error);
            return 1;
        }
    };
    let _ = listener.set_nonblocking(true);
    let child_pid = std::env::var("TRUST_BROKER_CHILD_PID")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let mut state = BrokerState {
        id: id.to_string(),
        name: std::env::var("TRUST_BROKER_NAME").unwrap_or_default(),
        title: std::env::var("TRUST_BROKER_TITLE").unwrap_or_else(|_| "Terminal".to_string()),
        cwd: std::env::var("TRUST_BROKER_CWD").unwrap_or_default(),
        last_command: String::new(),
        command_running: false,
        local_on: true,
        child_closed: false,
        child_pid,
        kill_requested: false,
        child_input: VecDeque::new(),
        child_input_offset: 0,
        child_input_pending: 0,
    };
    let mut clients: Vec<Client> = Vec::new();
    let mut gui_output = VecDeque::new();
    let mut gui_offset = 0usize;
    let mut gui_pending = 0usize;
    let mut poll_fds = Vec::with_capacity(3);
    set_nonblocking(CHILD_FD);
    set_nonblocking(GUI_FD);

    loop {
        // Only the GUI drives the child size while no remote client is
        // attached. Once a remote client attaches it sends its own
        // FRAME_RESIZE frames, which would otherwise be reverted on the next
        // iteration by the GUI's size.
        if !clients.iter().any(|client| client.attached) {
            sync_gui_size();
        }
        poll_fds.clear();
        poll_fds.reserve(3 + clients.len());
        poll_fds.push(libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        poll_fds.push(libc::pollfd {
            fd: CHILD_FD,
            events: if state.child_closed {
                0
            } else {
                libc::POLLIN
                    | if state.child_input.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    }
            },
            revents: 0,
        });
        poll_fds.push(libc::pollfd {
            fd: GUI_FD,
            events: libc::POLLIN
                | if gui_output.is_empty() {
                    0
                } else {
                    libc::POLLOUT
                },
            revents: 0,
        });
        for client in &clients {
            poll_fds.push(libc::pollfd {
                fd: client.stream.as_raw_fd(),
                events: libc::POLLIN
                    | if client.output.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
                revents: 0,
            });
        }
        // Every broker input is represented by a file descriptor. Sleeping
        // until readiness avoids ten needless wakeups per second when idle.
        let status = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as _, -1) };
        if status < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }

        while poll_fds[0].revents & libc::POLLIN != 0 {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(true);
                    clients.push(Client {
                        stream,
                        attached: false,
                        input: Vec::new(),
                        output: VecDeque::new(),
                        output_offset: 0,
                        output_pending: 0,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        if poll_fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut bytes = [0u8; 8192];
            loop {
                let count =
                    unsafe { libc::read(CHILD_FD, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                if count > 0 {
                    broadcast_output(
                        &mut clients,
                        &mut gui_output,
                        &mut gui_offset,
                        &mut gui_pending,
                        state.local_on,
                        &bytes[..count as usize],
                    );
                } else if count == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EIO)
                {
                    state.child_closed = true;
                    break;
                } else if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                    break;
                } else {
                    state.child_closed = true;
                    break;
                }
            }
        }

        if poll_fds[2].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut bytes = [0u8; 8192];
            loop {
                let count =
                    unsafe { libc::read(GUI_FD, bytes.as_mut_ptr() as *mut _, bytes.len()) };
                if count > 0 {
                    if state.local_on {
                        queue_child_input(&mut state, &bytes[..count as usize]);
                    }
                } else if count == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EIO)
                {
                    state.local_on = false;
                    break;
                } else if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                    break;
                } else {
                    state.local_on = false;
                    break;
                }
            }
        }
        if !state.child_closed && !flush_child_input(&mut state) {
            state.child_closed = true;
        }
        if !flush_gui(&mut gui_output, &mut gui_offset, &mut gui_pending) {
            state.local_on = false;
        }

        let mut remove = Vec::new();
        for index in 0..clients.len() {
            let poll = poll_fds.get(index + 3).map(|p| p.revents).unwrap_or(0);
            if poll & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
                && !read_client(index, &mut clients, &mut state)
            {
                remove.push(index);
            }
            if !clients[index].flush() {
                remove.push(index);
            }
        }
        remove.sort_unstable();
        remove.dedup();
        for index in remove.into_iter().rev() {
            clients.remove(index);
        }
        if state.kill_requested {
            break;
        }
        if state.child_closed
            && gui_output.is_empty()
            && clients.iter().all(|client| client.output.is_empty())
        {
            break;
        }
    }
    close_fd(CHILD_FD);
    close_fd(GUI_FD);
    let _ = std::fs::remove_file(path);
    0
}

struct BrokerState {
    id: String,
    name: String,
    title: String,
    cwd: String,
    last_command: String,
    command_running: bool,
    local_on: bool,
    child_closed: bool,
    child_pid: i32,
    kill_requested: bool,
    child_input: VecDeque<Vec<u8>>,
    child_input_offset: usize,
    child_input_pending: usize,
}

fn bind_broker_socket(path: &Path) -> Result<UnixListener, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let listener = UnixListener::bind(path).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(listener)
}

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

fn read_client(index: usize, clients: &mut [Client], state: &mut BrokerState) -> bool {
    let mut frames = Vec::new();
    {
        let client = &mut clients[index];
        let mut bytes = [0u8; 8192];
        loop {
            match client.stream.read(&mut bytes) {
                Ok(0) => return false,
                Ok(count) => client.input.extend_from_slice(&bytes[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
        loop {
            if client.input.len() < 4 {
                break;
            }
            let length = u32::from_be_bytes(client.input[..4].try_into().unwrap()) as usize;
            if length > MAX_FRAME {
                return false;
            }
            if client.input.len() < length + 4 {
                break;
            }
            frames.push(client.input.drain(..length + 4).skip(4).collect::<Vec<_>>());
        }
    }
    for frame in frames {
        if !clients[index].attached {
            if !handle_command(index, clients, state, &frame) {
                return false;
            }
        } else if frame.first() == Some(&FRAME_INPUT) {
            if !queue_child_input(state, &frame[1..]) {
                return false;
            }
        } else if frame.first() == Some(&FRAME_RESIZE) && frame.len() == 5 {
            let columns = u16::from_be_bytes([frame[1], frame[2]]);
            let rows = u16::from_be_bytes([frame[3], frame[4]]);
            set_child_size(columns, rows);
        } else if frame.first() == Some(&FRAME_DETACH) {
            clients[index].attached = false;
            let _ = clients[index].queue(b"OK\nDETACH");
        }
    }
    true
}

fn handle_command(
    index: usize,
    clients: &mut [Client],
    state: &mut BrokerState,
    frame: &[u8],
) -> bool {
    let command = String::from_utf8_lossy(frame);
    if let Some(update) = command.strip_prefix("UPDATE\t") {
        let mut fields = update.splitn(4, '\t');
        if fields.next() != Some(state.id.as_str()) {
            return clients[index].queue(b"ERR session not found");
        }
        state.name = field(fields.next().unwrap_or(""));
        state.title = field(fields.next().unwrap_or(""));
        state.cwd = field(fields.next().unwrap_or(""));
        return clients[index].queue(b"OK");
    }
    if let Some(update) = command.strip_prefix("COMMAND\t") {
        let mut fields = update.splitn(3, '\t');
        if fields.next() != Some(state.id.as_str()) {
            return clients[index].queue(b"ERR session not found");
        }
        state.command_running = fields.next() == Some("running");
        state.last_command = field(fields.next().unwrap_or(""));
        return clients[index].queue(b"OK");
    }
    let mut parts = command.splitn(4, ' ');
    let verb = parts.next().unwrap_or("");
    let id = parts.next().unwrap_or("");
    if matches!(
        verb,
        "LIST"
            | "INFO"
            | "ATTACH"
            | "DETACH"
            | "RENAME"
            | "LOCAL_ON"
            | "LOCAL_OFF"
            | "UPDATE"
            | "COMMAND"
            | "SIGNAL"
            | "KILL"
            | "IS_SSH"
    ) && id != state.id
        && verb != "LIST"
    {
        return clients[index].queue(b"ERR session not found");
    }
    let line = state.line();
    match verb {
        "LIST" | "INFO" => clients[index].queue(format!("OK\n{}", line).as_bytes()),
        "IS_SSH" => clients[index].queue(if child_foreground_is_ssh() {
            b"OK\n1"
        } else {
            b"OK\n0"
        }),
        "ATTACH" => {
            clients[index].attached = true;
            // A new client starts from an empty screen (the broker does not
            // replay scrollback). Nudge the child with SIGWINCH so full-screen
            // programs redraw and shells re-print their prompt, otherwise the
            // freshly attached client looks frozen until new output arrives.
            signal_child(state.child_pid, libc::SIGWINCH);
            clients[index].queue(b"OK\nATTACH")
        }
        "DETACH" => {
            for client in clients.iter_mut() {
                client.attached = false;
            }
            clients[index].queue(b"OK\nDETACH")
        }
        "RENAME" => {
            state.name = field(parts.next().unwrap_or(""));
            clients[index].queue(b"OK")
        }
        "LOCAL_ON" => {
            state.local_on = true;
            clients[index].queue(b"OK")
        }
        "LOCAL_OFF" => {
            state.local_on = false;
            clients[index].queue(b"OK")
        }
        "SIGNAL" => {
            let signal = parts
                .next()
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(libc::SIGTERM);
            signal_child(state.child_pid, signal);
            clients[index].queue(b"OK")
        }
        "KILL" => {
            signal_child(state.child_pid, libc::SIGTERM);
            state.kill_requested = true;
            clients[index].queue(b"OK")
        }
        _ => clients[index].queue(b"ERR invalid request"),
    }
}

impl BrokerState {
    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id,
            self.name,
            self.title,
            self.cwd,
            if self.local_on { "local" } else { "local-off" },
            self.last_command,
            if self.command_running {
                "running"
            } else {
                "last"
            }
        )
    }
}

fn signal_child(pid: i32, signal: i32) {
    if pid > 0 {
        unsafe {
            if libc::killpg(pid, signal) != 0 {
                libc::kill(pid, signal);
            }
        }
    }
}

/// Whether the child's foreground process group is `ssh`, used to pass the
/// `Ctrl+B`/`d` detach sequence through to a remote tmux instead of detaching
/// the local client.
fn child_foreground_is_ssh() -> bool {
    let foreground = unsafe { libc::tcgetpgrp(CHILD_FD) };
    if foreground <= 0 {
        return false;
    }
    std::fs::read_to_string(format!("/proc/{foreground}/comm"))
        .map(|name| name.trim() == "ssh")
        .unwrap_or(false)
}

fn queue_child_input(state: &mut BrokerState, data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    if state.child_input_pending.saturating_add(data.len()) > MAX_PENDING {
        return false;
    }
    state.child_input_pending += data.len();
    state.child_input.push_back(data.to_vec());
    true
}

fn flush_child_input(state: &mut BrokerState) -> bool {
    loop {
        let Some(data) = state.child_input.front() else {
            state.child_input_offset = 0;
            return true;
        };
        let offset = state.child_input_offset;
        let written = unsafe {
            libc::write(
                CHILD_FD,
                data[offset..].as_ptr() as *const _,
                data.len() - offset,
            )
        };
        if written > 0 {
            state.child_input_offset += written as usize;
            state.child_input_pending = state.child_input_pending.saturating_sub(written as usize);
            if state.child_input_offset == data.len() {
                state.child_input.pop_front();
                state.child_input_offset = 0;
            }
            continue;
        }
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock
                || error.kind() == io::ErrorKind::Interrupted
            {
                return true;
            }
        }
        return false;
    }
}

fn set_child_size(columns: u16, rows: u16) {
    let packed = ((columns as u64) << 32) | rows as u64;
    if LAST_CHILD_SIZE.swap(packed, Ordering::Relaxed) == packed {
        return;
    }
    let size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(CHILD_FD, libc::TIOCSWINSZ, &size);
    }
}

fn sync_gui_size() {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    if unsafe { libc::ioctl(GUI_FD, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
        let size = unsafe { size.assume_init() };
        set_child_size(size.ws_col, size.ws_row);
    }
}

fn broadcast_output(
    clients: &mut [Client],
    gui_output: &mut VecDeque<Vec<u8>>,
    gui_offset: &mut usize,
    gui_pending: &mut usize,
    local_on: bool,
    data: &[u8],
) {
    if local_on {
        *gui_pending = gui_pending.saturating_add(data.len());
        gui_output.push_back(data.to_vec());
        while *gui_pending > MAX_PENDING {
            if let Some(old) = gui_output.pop_front() {
                let remaining = old.len().saturating_sub(*gui_offset);
                *gui_pending = gui_pending.saturating_sub(remaining);
                *gui_offset = 0;
            } else {
                break;
            }
        }
    }
    let mut dropped = Vec::new();
    let mut frame = Vec::with_capacity(data.len() + 1);
    frame.push(FRAME_OUTPUT);
    frame.extend_from_slice(data);
    for (index, client) in clients.iter_mut().enumerate() {
        if client.attached && !client.queue(&frame) {
            dropped.push(index);
        }
    }
    for index in dropped.into_iter().rev() {
        clients[index].attached = false;
    }
}

fn flush_gui(queue: &mut VecDeque<Vec<u8>>, offset: &mut usize, pending: &mut usize) -> bool {
    while let Some(data) = queue.front() {
        let written = unsafe {
            libc::write(
                GUI_FD,
                data[*offset..].as_ptr() as *const _,
                data.len() - *offset,
            )
        };
        if written <= 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                return true;
            }
            queue.clear();
            *offset = 0;
            *pending = 0;
            return false;
        }
        *pending = pending.saturating_sub(written as usize);
        *offset += written as usize;
        if *offset == data.len() {
            queue.pop_front();
            *offset = 0;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_headless_and_broker_modes() {
        let args = vec!["oxterm".into(), "--list".into()];
        assert!(matches!(parse_cli_mode(&args), Some(Ok(CliMode::List))));
        let args = vec!["oxterm".into(), "-a".into(), "1234-2".into()];
        assert!(
            matches!(parse_cli_mode(&args), Some(Ok(CliMode::Attach(Some(id)))) if id == "1234-2")
        );
        let args = vec!["oxterm".into(), "--info".into(), "1234-2".into()];
        assert!(matches!(parse_cli_mode(&args), Some(Ok(CliMode::Info(id))) if id == "1234-2"));
        let args = vec!["oxterm".into(), "--detach".into(), "1234-2".into()];
        assert!(matches!(parse_cli_mode(&args), Some(Ok(CliMode::Detach(id))) if id == "1234-2"));
        let args = vec![
            "oxterm".into(),
            "--broker".into(),
            "/tmp/x.sock".into(),
            "1234-2".into(),
        ];
        assert!(
            matches!(parse_cli_mode(&args), Some(Ok(CliMode::Broker(path, id))) if path == PathBuf::from("/tmp/x.sock") && id == "1234-2")
        );
    }

    #[test]
    fn frames_are_length_prefixed_and_bounded() {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, b"ATTACH 1").unwrap();
        assert_eq!(&encoded[..4], &(8u32.to_be_bytes()));
        assert_eq!(read_frame(&mut Cursor::new(encoded)).unwrap(), b"ATTACH 1");
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME + 1]).is_err());

        let oversized = (MAX_FRAME as u32 + 1).to_be_bytes();
        assert!(read_frame(&mut Cursor::new(oversized)).is_err());
        assert!(read_frame(&mut Cursor::new([0, 0, 0])).is_err());
    }

    #[test]
    fn rejects_invalid_headless_arguments() {
        let args = vec!["oxterm".into(), "--list".into(), "extra".into()];
        assert!(parse_cli_mode(&args).unwrap().is_err());
        let args = vec!["oxterm".into(), "--attach".into(), "bad id".into()];
        assert!(parse_cli_mode(&args).unwrap().is_err());
    }

    #[test]
    fn session_ids_and_metadata_are_safely_bounded() {
        assert!(invalid_id(""));
        assert!(invalid_id("has whitespace"));
        assert!(invalid_id("has\nnewline"));
        assert!(!invalid_id("1234-2"));

        let metadata = format!("a\tb\nc\r{}", "x".repeat(300));
        let sanitized = field(&metadata);
        assert!(!sanitized.chars().any(|c| matches!(c, '\t' | '\n' | '\r')));
        assert_eq!(sanitized.chars().count(), 200);
    }

    #[test]
    fn client_output_queue_has_a_hard_limit() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut client = Client {
            stream,
            attached: false,
            input: Vec::new(),
            output: VecDeque::new(),
            output_offset: 0,
            output_pending: 0,
        };
        assert!(client.queue(&vec![0; MAX_PENDING - 4]));
        assert!(!client.queue(b"overflow"));
    }

    #[test]
    fn relay_input_forwards_keys_and_handles_detach_prefix() {
        let mut prefix = false;
        assert!(matches!(
            relay_input(b"nano", &mut prefix),
            RelayAction::Forward(ref v) if v == b"nano"
        ));
        assert!(!prefix);

        assert!(matches!(
            relay_input(&[0x02], &mut prefix),
            RelayAction::Forward(ref v) if v.is_empty()
        ));
        assert!(prefix);
        assert!(matches!(
            relay_input(b"d", &mut prefix),
            RelayAction::PrefixDetach { upper: false, ref forwarded } if forwarded.is_empty()
        ));
        assert!(!prefix);

        relay_input(&[0x02], &mut prefix);
        assert!(prefix);
        assert!(matches!(
            relay_input(b"D", &mut prefix),
            RelayAction::PrefixDetach { upper: true, ref forwarded } if forwarded.is_empty()
        ));
        assert!(!prefix);

        assert!(matches!(
            relay_input(&[0x02, 0x02], &mut prefix),
            RelayAction::Forward(ref v) if v == &[0x02]
        ));
        assert!(!prefix);

        relay_input(&[0x02], &mut prefix);
        assert!(matches!(
            relay_input(&[0x04], &mut prefix),
            RelayAction::Detach(ref v) if v.is_empty()
        ));

        // Bytes typed before the Ctrl+B prefix are preserved and forwarded.
        assert!(matches!(
            relay_input(b"ls\x02d", &mut prefix),
            RelayAction::PrefixDetach { upper: false, ref forwarded } if forwarded == b"ls"
        ));
    }
}
