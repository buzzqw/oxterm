//! End-to-end tests for the `oxterm new|split|send|capture|wait` control API.
//!
//! Like the window lifecycle tests, these open a real window and are skipped
//! unless `OXTERM_GUI_TESTS=1`; CI runs them under Xvfb.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TempHome(PathBuf);

impl TempHome {
    fn new() -> TempHome {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        // Keep the path short enough that Oxterm's broker sockets (whose id can
        // be 49 characters) stay under the Unix socket limit, so both the
        // broker and the control socket live in the temporary HOME.
        let path =
            std::env::temp_dir().join(format!("oc{}{:06}", std::process::id(), unique % 1_000_000));
        std::fs::create_dir_all(&path).expect("create test home");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make test home private");
        TempHome(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn gui_tests_enabled() -> bool {
    if std::env::var_os("OXTERM_GUI_TESTS").is_none() {
        eprintln!("skipping control API tests: set OXTERM_GUI_TESTS=1 and run with a display");
        return false;
    }
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        panic!("OXTERM_GUI_TESTS is set but DISPLAY and WAYLAND_DISPLAY are both unset");
    }
    true
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_oxterm")
}

fn socket_dir(home: &Path) -> PathBuf {
    let configured = home.join(".config/oxterm/remote");
    let probe = configured.join(format!("control-{}.sock", "9".repeat(20)));
    if probe.as_os_str().len() <= 107 {
        configured
    } else {
        PathBuf::from("/tmp")
            .join(format!("oxterm-{}", unsafe { libc::geteuid() }))
            .join("remote")
    }
}

fn wait_for_control_socket(home: &Path, child: &mut Child) -> PathBuf {
    let directory = socket_dir(home);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("control-"))
                {
                    return path;
                }
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("Oxterm exited before creating its control socket: {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "Oxterm did not create a control socket in {}",
        directory.display()
    );
}

fn run_cli(home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env("NO_AT_BRIDGE", "1")
        // A developer running the suite from inside Oxterm inherits these;
        // leaving them set makes the CLI talk to the outer window instead of
        // the test instance.
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::null())
        .output()
        .expect("run oxterm")
}

fn cli_stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn first_pane(home: &Path) -> String {
    // The broker socket appears only after the GUI has spawned its shell, and
    // startup can take a while on a loaded CI runner. Poll instead of relying
    // on a fixed sleep.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let output = run_cli(home, &["--list"]);
        let text = cli_stdout(&output);
        if let Some(id) = text
            .lines()
            .nth(1)
            .and_then(|line| line.split('\t').next())
            .filter(|id| !id.is_empty())
        {
            return id.to_string();
        }
        if Instant::now() >= deadline {
            eprintln!(
                "remote directory: {:?}",
                std::fs::read_dir(socket_dir(home))
                    .map(|entries| entries.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
            );
            eprintln!(
                "GUI log:\n{}",
                std::fs::read_to_string(home.join("gui.log")).unwrap_or_default()
            );
            eprintln!("--list output:\n{text}");
            panic!("no pane in --list output: {text}");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_wait(home: &Path, pane: &str, timeout: &str) -> Child {
    Command::new(binary())
        .args(["wait", "--pane", pane, "--timeout", timeout])
        .env("HOME", home)
        .env("NO_AT_BRIDGE", "1")
        // A developer running the suite from inside Oxterm inherits these;
        // leaving them set makes the CLI talk to the outer window instead of
        // the test instance.
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxterm wait")
}

fn wait_status(child: Child) -> i32 {
    let output = child.wait_with_output().expect("wait for oxterm wait");
    output.status.code().unwrap_or(-1)
}

struct Fixture {
    child: Child,
    home: TempHome,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SIGTERM goes through Oxterm's shutdown path, which closes the
        // terminals and their persistent PTY brokers. SIGKILL would leave the
        // brokers running after the test.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = self.child.kill();
                    break;
                }
            }
        }
        let _ = self.child.wait();
        // Give the brokers a moment to notice the closed sessions.
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn control_api_drives_tabs_panes_and_commands() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    let log = std::fs::File::create(home.path().join("gui.log")).expect("create gui log");
    let log_err = log.try_clone().expect("clone gui log");
    let child = Command::new(binary())
        .args([
            "--no-restore",
            "-o",
            "osc133=true",
            "-o",
            "login_shell=false",
            "-o",
            "shell_command=/bin/bash",
        ])
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .env("NO_AT_BRIDGE", "1")
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .expect("start Oxterm");
    let mut fixture = Fixture { child, home };
    wait_for_control_socket(fixture.home.path(), &mut fixture.child);
    // Let the shell print its first prompt and initialise the integration.
    thread::sleep(Duration::from_millis(800));

    let pane = first_pane(fixture.home.path());

    // send --enter + wait: the command's own status is the wait exit status.
    let waiting = spawn_wait(fixture.home.path(), &pane, "10");
    thread::sleep(Duration::from_millis(300));
    cli_stdout(&run_cli(
        fixture.home.path(),
        &["send", "--pane", &pane, "--enter", "printf ctl-ok"],
    ));
    assert_eq!(wait_status(waiting), 0, "successful command should exit 0");

    let capture = cli_stdout(&run_cli(
        fixture.home.path(),
        &["capture", "--pane", &pane, "--scrollback", "50"],
    ));
    assert!(capture.contains("ctl-ok"), "capture: {capture:?}");

    // A failing command makes wait exit with its status.
    let waiting = spawn_wait(fixture.home.path(), &pane, "10");
    thread::sleep(Duration::from_millis(300));
    cli_stdout(&run_cli(
        fixture.home.path(),
        &["send", "--pane", &pane, "--enter", "false"],
    ));
    assert_eq!(wait_status(waiting), 1, "failed command should exit 1");

    // stdin is a supported send source.
    let mut sender = Command::new(binary())
        .args(["send", "--pane", &pane, "--enter", "-"])
        .env("HOME", fixture.home.path())
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxterm send");
    sender
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"printf from-stdin")
        .expect("write stdin");
    let output = sender.wait_with_output().expect("wait for send");
    assert!(
        output.status.success(),
        "send - failed: {:?}",
        output.status
    );
    thread::sleep(Duration::from_millis(400));
    let capture = cli_stdout(&run_cli(
        fixture.home.path(),
        &["capture", "--pane", &pane, "--scrollback", "50"],
    ));
    assert!(capture.contains("from-stdin"), "capture: {capture:?}");

    // new runs a command in its own tab.
    let new_pane = cli_stdout(&run_cli(
        fixture.home.path(),
        &[
            "new",
            "--title",
            "ctl-new",
            "--",
            "/bin/sh",
            "-c",
            "printf new-tab-ok; sleep 10",
        ],
    ));
    thread::sleep(Duration::from_millis(400));
    let capture = cli_stdout(&run_cli(
        fixture.home.path(),
        &["capture", "--pane", &new_pane],
    ));
    assert!(capture.contains("new-tab-ok"), "capture: {capture:?}");

    // split only works while the window still has a single pane.
    let split_pane = cli_stdout(&run_cli(
        fixture.home.path(),
        &["split", "--pane", &pane, "--vertical", "--title", "monitor"],
    ));
    assert_ne!(split_pane, pane);
    // The requested title must stick: the shell's OSC title updates and the
    // numbered default must not overwrite it.
    let listing = cli_stdout(&run_cli(fixture.home.path(), &["--list"]));
    let row = listing
        .lines()
        .find(|line| line.starts_with(&format!("{split_pane}\t")));
    assert!(
        row.is_some_and(|row| row.contains("\tmonitor\t")),
        "split title missing from listing: {listing}"
    );
    let again = run_cli(fixture.home.path(), &["split", "--pane", &pane]);
    assert_eq!(again.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already has two panes"),
        "stderr: {}",
        String::from_utf8_lossy(&again.stderr)
    );

    // Unknown pane and usage errors have dedicated exit statuses.
    let missing = run_cli(
        fixture.home.path(),
        &["capture", "--pane", "does-not-exist"],
    );
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(
        run_cli(fixture.home.path(), &["capture", "--bogus"])
            .status
            .code(),
        Some(2)
    );
}

/// `Tabs > Move Tab from Another Oxterm` is a `handover`: the source window
/// closes the tab but must leave the session (and its shell) alive for the
/// window that adopts it. The VTE child watch used to signal the child when the
/// widget was disposed, which killed the moved session.
#[test]
fn handover_keeps_the_moved_session_alive() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    let log = std::fs::File::create(home.path().join("gui.log")).expect("create gui log");
    let log_err = log.try_clone().expect("clone gui log");
    let child = Command::new(binary())
        .args([
            "--no-restore",
            "-o",
            "osc133=true",
            "-o",
            "login_shell=false",
            "-o",
            "shell_command=/bin/bash",
        ])
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path())
        .env("NO_AT_BRIDGE", "1")
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .expect("start Oxterm");
    let mut fixture = Fixture { child, home };
    let socket = wait_for_control_socket(fixture.home.path(), &mut fixture.child);
    thread::sleep(Duration::from_millis(800));

    let first = first_pane(fixture.home.path());
    let moved = cli_stdout(&run_cli(fixture.home.path(), &["new"]));
    assert_ne!(moved, first, "the new tab must be a different pane");
    thread::sleep(Duration::from_millis(500));

    let listing = control_call(&socket, serde_json::json!({ "op": "list" }));
    let panes = listing["panes"].as_array().expect("pane list").clone();
    let moved_row = panes
        .iter()
        .find(|pane| pane["pane"].as_str() == Some(moved.as_str()))
        .expect("the new tab is not listed");
    let fifo = moved_row["osc133_fifo"]
        .as_str()
        .filter(|path| !path.is_empty())
        .expect("the new tab has no shell integration FIFO")
        .to_string();

    // The adopter holds the FIFO open before the source lets go, or the shell
    // dies of SIGPIPE at its next prompt.
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .expect("open the OSC 133 FIFO");

    let reply = control_call(
        &socket,
        serde_json::json!({ "op": "handover", "pane": moved }),
    );
    assert_eq!(reply["ok"], true, "handover failed: {reply}");
    // The source answers before it closes the tab (closing the last window ends
    // the process), so give the deferred close a moment to run.
    thread::sleep(Duration::from_millis(400));

    let listing = control_call(&socket, serde_json::json!({ "op": "list" }));
    let panes = listing["panes"].as_array().expect("pane list");
    assert!(
        panes
            .iter()
            .all(|pane| pane["pane"].as_str() != Some(moved.as_str())),
        "the moved tab is still open: {listing}"
    );
    assert!(
        panes
            .iter()
            .any(|pane| pane["pane"].as_str() == Some(first.as_str())),
        "the source window lost its other tab: {listing}"
    );

    // The broker must still be running the shell: `--list` reads the brokers.
    let sessions = cli_stdout(&run_cli(fixture.home.path(), &["--list"]));
    let row = sessions
        .lines()
        .find(|line| line.starts_with(&format!("{moved}\t")));
    let row = row.unwrap_or_else(|| panic!("the moved session is gone: {sessions}"));
    assert!(
        row.contains("\tlocal-off\t"),
        "the moved session should no longer be attached to the source window: {row}"
    );
    drop(reader);
}

fn control_call(socket: &Path, request: serde_json::Value) -> serde_json::Value {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket).expect("connect to the control socket");
    oxterm::headless::framing::write_json(&mut stream, &request, 1 << 20).expect("write request");
    oxterm::headless::framing::read_json(&mut stream, 1 << 20).expect("read reply")
}
