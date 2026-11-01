//! Background `ssh -N` processes for saved tunnels.
//!
//! The state shown to the user comes from the process and the listening port,
//! never from the configuration: a tunnel is `Up` only after its local port
//! accepted a connection, and it leaves that state when `ssh` exits.
//!
//! One thread owns each child. `PR_SET_PDEATHSIG` is bound to the creating
//! thread, so it stays alive for as long as the child does; that way a crash of
//! Oxterm (the release profile aborts on panic) cannot leave a port forwarded
//! with nobody to stop it.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);
/// Once settled a tunnel or viewer can run for days: a stop request or an exit
/// noticed half a second later costs nothing, ten wakeups a second would.
const SETTLED_POLL: Duration = Duration::from_millis(500);
const PROBE_EVERY: Duration = Duration::from_millis(250);
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const STOP_GRACE: Duration = Duration::from_secs(2);
const ERROR_LINES: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// `ssh` runs and the local port does not answer yet.
    Starting,
    /// The local port accepted a connection.
    Up,
    /// `ssh` runs; a remote forward offers no port to check from here.
    Running,
    /// `ssh` exited unexpectedly, with its last error lines.
    Failed(String),
    Stopped,
}

impl Status {
    pub fn is_active(&self) -> bool {
        matches!(self, Status::Starting | Status::Up | Status::Running)
    }

    pub fn label(&self) -> String {
        match self {
            Status::Starting => "starting".to_string(),
            Status::Up => "listening".to_string(),
            Status::Running => "running".to_string(),
            Status::Failed(reason) => format!("failed: {}", reason),
            Status::Stopped => "stopped".to_string(),
        }
    }
}

/// A tunnel to start. `argv[0]` is the program.
#[derive(Clone, Debug)]
pub struct Spec {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub probe: Option<String>,
    /// Written to standard input: how a viewer receives a password.
    pub stdin: Option<String>,
}

struct Slot {
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Slot>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, Slot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn set(status: &Mutex<Status>, value: Status) {
    if let Ok(mut guard) = status.lock() {
        *guard = value;
    }
}

/// Key of one tunnel of one connection.
pub fn key(connection_id: &str, tunnel: &str) -> String {
    format!("{}\n{}", connection_id, tunnel)
}

/// Start a tunnel unless it already runs. Returns immediately.
pub fn start(key: &str, spec: Spec) {
    let mut slots = match registry().lock() {
        Ok(slots) => slots,
        Err(_) => return,
    };
    if let Some(slot) = slots.get(key) {
        if slot.status.lock().is_ok_and(|status| status.is_active()) {
            return;
        }
    }
    let status = Arc::new(Mutex::new(Status::Starting));
    let stop = Arc::new(AtomicBool::new(false));
    slots.insert(
        key.to_string(),
        Slot {
            status: Arc::clone(&status),
            stop: Arc::clone(&stop),
        },
    );
    let spawned = thread::Builder::new()
        .name("oxterm-tunnel".to_string())
        .spawn({
            let status = Arc::clone(&status);
            move || supervise(spec, &status, &stop)
        });
    if let Err(error) = spawned {
        set(&status, Status::Failed(error.to_string()));
    }
}

/// Ask a tunnel to stop. The owning thread does the waiting.
pub fn stop(key: &str) {
    if let Ok(slots) = registry().lock() {
        if let Some(slot) = slots.get(key) {
            slot.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// Stop everything and wait, within reason, for the children to go. Called when
/// the application shuts down.
pub fn stop_all() {
    let statuses: Vec<Arc<Mutex<Status>>> = match registry().lock() {
        Ok(slots) => slots
            .values()
            .map(|slot| {
                slot.stop.store(true, Ordering::SeqCst);
                Arc::clone(&slot.status)
            })
            .collect(),
        Err(_) => return,
    };
    let deadline = Instant::now() + STOP_GRACE + Duration::from_millis(500);
    while Instant::now() < deadline {
        let busy = statuses
            .iter()
            .any(|status| status.lock().is_ok_and(|status| status.is_active()));
        if !busy {
            return;
        }
        thread::sleep(POLL);
    }
}

pub fn status(key: &str) -> Status {
    registry()
        .lock()
        .ok()
        .and_then(|slots| {
            slots
                .get(key)
                .and_then(|slot| slot.status.lock().ok().map(|status| status.clone()))
        })
        .unwrap_or(Status::Stopped)
}

/// Every known tunnel with its current state.
pub fn snapshot() -> BTreeMap<String, Status> {
    registry()
        .lock()
        .map(|slots| {
            slots
                .iter()
                .filter_map(|(key, slot)| {
                    slot.status
                        .lock()
                        .ok()
                        .map(|status| (key.clone(), status.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn port_answers(address: &str) -> bool {
    let Ok(mut addresses) = address.to_socket_addrs() else {
        return false;
    };
    addresses.any(|socket| TcpStream::connect_timeout(&socket, PROBE_TIMEOUT).is_ok())
}

fn supervise(spec: Spec, status: &Mutex<Status>, stop: &AtomicBool) {
    let Some((program, args)) = spec.argv.split_first() else {
        set(status, Status::Failed("empty command".to_string()));
        return;
    };
    // A port already in use would make a stale listener look like our tunnel.
    // Refuse early, with a clearer message than ExitOnForwardFailure's.
    if let Some(address) = spec.probe.as_deref() {
        if port_answers(address) {
            set(
                status,
                Status::Failed(format!("{} is already in use", address)),
            );
            return;
        }
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(spec.env.iter().map(|(key, value)| (key, value)))
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe calls between fork and exec. A new session
    // gives `ssh` no controlling terminal, so nothing can block on a prompt that
    // nobody sees.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            set(
                status,
                Status::Failed(format!("cannot run {}: {}", program, error)),
            );
            return;
        }
    };
    if let (Some(text), Some(mut stdin)) = (spec.stdin.as_deref(), child.stdin.take()) {
        // A viewer that exits without reading must not take this thread down;
        // its exit is reported below like any other.
        let _ = writeln!(stdin, "{}", text);
    }
    let errors = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let reader = child.stderr.take().map(|stderr| {
        let errors = Arc::clone(&errors);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut errors) = errors.lock() {
                    if errors.len() == ERROR_LINES {
                        errors.pop_front();
                    }
                    errors.push_back(line);
                }
            }
        })
    });
    if spec.probe.is_none() {
        set(status, Status::Running);
    }
    let mut last_probe: Option<Instant> = None;
    let mut up = spec.probe.is_none();
    let outcome = loop {
        if stop.load(Ordering::SeqCst) {
            terminate(&mut child);
            break Status::Stopped;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
                let text = errors
                    .lock()
                    .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("; "))
                    .unwrap_or_default();
                // A viewer closed by the person is not a failure.
                if exit.success() {
                    break Status::Stopped;
                }
                break Status::Failed(if text.is_empty() {
                    format!("exited: {}", exit)
                } else {
                    text
                });
            }
            Ok(None) => {}
            Err(error) => break Status::Failed(error.to_string()),
        }
        // Probe only until the first success: each probe opens a forwarded
        // connection on the far side, which is noise on a healthy tunnel. From
        // then on the process state is the signal, and keepalives make `ssh`
        // exit when the network is gone.
        if !up && last_probe.is_none_or(|at| at.elapsed() >= PROBE_EVERY) {
            last_probe = Some(Instant::now());
            if spec.probe.as_deref().is_some_and(port_answers) {
                up = true;
                set(status, Status::Up);
            }
        }
        thread::sleep(if up { SETTLED_POLL } else { POLL });
    };
    set(status, outcome);
}

fn terminate(child: &mut std::process::Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn wait_for(key: &str, wanted: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let current = status(key);
            if wanted(&current) {
                return current;
            }
            assert!(Instant::now() < deadline, "stuck in {:?}", current);
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn sh(script: &str, probe: Option<String>) -> Spec {
        Spec {
            argv: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            env: Vec::new(),
            probe,
            stdin: None,
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn failure_reports_the_error_output() {
        let key = key("t", "fail");
        start(
            &key,
            sh("echo first >&2; echo 'bind: denied' >&2; exit 3", None),
        );
        let result = wait_for(&key, |status| matches!(status, Status::Failed(_)));
        let Status::Failed(reason) = result else {
            unreachable!()
        };
        assert!(reason.contains("first; bind: denied"), "{}", reason);
    }

    #[test]
    fn remote_forward_runs_until_stopped() {
        let key = key("t", "remote");
        start(&key, sh("exec sleep 60", None));
        wait_for(&key, |status| *status == Status::Running);
        stop(&key);
        wait_for(&key, |status| *status == Status::Stopped);
    }

    #[test]
    fn local_forward_is_up_only_once_the_port_answers() {
        let port = free_port();
        let key = key("t", "local");
        let address = format!("127.0.0.1:{}", port);
        start(&key, sh("exec sleep 60", Some(address.clone())));
        thread::sleep(Duration::from_millis(400));
        assert_eq!(status(&key), Status::Starting, "nothing listens yet");
        let _listener = TcpListener::bind(&address).unwrap();
        wait_for(&key, |status| *status == Status::Up);
        stop(&key);
        wait_for(&key, |status| *status == Status::Stopped);
    }

    #[test]
    fn a_port_in_use_fails_without_starting_ssh() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let key = key("t", "busy");
        start(&key, sh("exec sleep 60", Some(address)));
        let result = wait_for(&key, |status| matches!(status, Status::Failed(_)));
        assert!(result.label().contains("already in use"), "{:?}", result);
    }

    #[test]
    fn starting_twice_keeps_one_process_and_restarts_after_a_stop() {
        let key = key("t", "twice");
        start(&key, sh("exec sleep 60", None));
        wait_for(&key, |status| *status == Status::Running);
        start(&key, sh("exit 9", None));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            status(&key),
            Status::Running,
            "second start must be a no-op"
        );
        stop(&key);
        wait_for(&key, |status| *status == Status::Stopped);
        start(&key, sh("exec sleep 60", None));
        wait_for(&key, |status| *status == Status::Running);
        stop(&key);
        wait_for(&key, |status| *status == Status::Stopped);
    }

    #[test]
    fn a_missing_program_fails_cleanly() {
        let key = key("t", "missing");
        start(
            &key,
            Spec {
                argv: vec!["/nonexistent/oxterm-test".to_string()],
                env: Vec::new(),
                probe: None,
                stdin: None,
            },
        );
        wait_for(&key, |status| matches!(status, Status::Failed(_)));
    }

    #[test]
    fn a_clean_exit_is_stopped_and_stdin_reaches_the_program() {
        let dir = std::env::temp_dir().join(format!("oxterm-tunnel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("got");
        let key = key("t", "stdin");
        let mut spec = sh(&format!("cat > {}", file.display()), None);
        spec.stdin = Some("secret".to_string());
        start(&key, spec);
        wait_for(&key, |status| *status == Status::Stopped);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "secret\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_tunnels_read_as_stopped() {
        assert_eq!(status("no such tunnel"), Status::Stopped);
        assert!(!Status::Stopped.is_active());
        assert!(Status::Up.is_active());
    }
}
