//! End-to-end checks of the connection book.
//!
//! `oxterm connections` and the keyring helper need no display, so those always
//! run. The GUI tests use a fake `ssh` that records its arguments: the point is
//! what Oxterm asks OpenSSH to do, not whether a server answers. They are
//! skipped unless `OXTERM_GUI_TESTS=1`, like the other window tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
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
        // Short: the broker and control sockets live under it.
        let path =
            std::env::temp_dir().join(format!("ox{}{:06}", std::process::id(), unique % 1_000_000));
        fs::create_dir_all(path.join("bin")).unwrap();
        fs::create_dir_all(path.join(".config/oxterm")).unwrap();
        fs::create_dir_all(path.join(".ssh")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        TempHome(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_book(&self, contents: &str) {
        fs::write(self.0.join(".config/oxterm/connections.json"), contents).unwrap();
    }

    fn write_script(&self, name: &str, body: &str) {
        let path = self.0.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{}\n", body)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake `ssh`: answers `-G` like OpenSSH, records anything else and stays
    /// alive the way an interactive session would.
    fn install_fake_ssh(&self) {
        let log = self.0.join("ssh.log");
        self.write_script(
            "ssh",
            &format!(
                r#"if [ "$1" = "-G" ]; then
    printf 'user tester\nhostname h.example\ncontrolpath none\nserveraliveinterval 0\n'
    exit 0
fi
printf '%s\n' "$*" >> '{}'
exec sleep 60"#,
                log.display()
            ),
        );
    }

    fn command(&self, binary: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .env("HOME", &self.0)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env_remove("OXTERM_CONTROL")
            .env_remove("OXTERM_PANE")
            .env_remove("OXTERM_SHELL_INTEGRATION")
            .env_remove("OXTERM_OSC133_FIFO");
        command
    }

    fn oxterm(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_oxterm"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run oxterm")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

const BOOK: &str = r#"{"version":1,"connections":[
  {"id":"c1","name":"db-prod","host":"10.0.0.1","user":"admin","port":2222,
   "proxy_jump":"bastion","group":"Customers/ACME","tags":["db"],
   "tunnels":[{"type":"local","listen":"127.0.0.1:15433","target":"db:5432"}]},
  {"id":"c2","name":"careful","host":"h","confirm_connect":true},
  {"id":"alias:web","kind":"alias","name":"web","group":"G"}
]}"#;

#[test]
fn listing_merges_the_book_with_ssh_config_and_leaves_it_alone() {
    let home = TempHome::new();
    home.write_book(BOOK);
    let config =
        "Host web\n  HostName w.example\nHost cfg-only\n  HostName c.example\nHost *\n  User x\n";
    let config_path = home.path().join(".ssh/config");
    fs::write(&config_path, config).unwrap();

    let output = home.oxterm(&["connections"]);
    assert!(output.status.success(), "{:?}", output);
    let listing = text(&output);
    assert!(listing.contains("db-prod"), "{}", listing);
    assert!(listing.contains("admin@10.0.0.1:2222"), "{}", listing);
    assert!(listing.contains("cfg-only"), "{}", listing);
    assert_eq!(listing.matches("web").count(), 1, "{}", listing);

    let output = home.oxterm(&["connections", "--json"]);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let db = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "db-prod")
        .unwrap();
    assert_eq!(
        db["command"], "ssh -l admin -J bastion -p 2222 10.0.0.1",
        "the listing shows the real command"
    );

    assert_eq!(
        fs::read_to_string(&config_path).unwrap(),
        config,
        "~/.ssh/config is never modified"
    );
}

#[test]
fn export_and_import_round_trip_without_replacing_anything() {
    let home = TempHome::new();
    home.write_book(BOOK);
    let export = home.oxterm(&["connections", "export"]);
    assert!(export.status.success(), "{:?}", export);
    let file = home.path().join("shared.json");
    let shared = text(&export).replace("\"db-prod\"", "\"db-copy\"");
    fs::write(&file, shared).unwrap();

    let output = home.oxterm(&["connections", "import", file.to_str().unwrap()]);
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(text(&output).trim(), "imported 1");
    assert!(String::from_utf8_lossy(&output.stderr).contains("careful: the name is taken"));

    let listing = text(&home.oxterm(&["connections"]));
    assert!(
        listing.contains("db-prod") && listing.contains("db-copy"),
        "{}",
        listing
    );
    let missing = home.oxterm(&["connections", "import", "/nonexistent/file.json"]);
    assert!(!missing.status.success());
}

#[test]
fn a_damaged_book_is_reported_and_not_replaced() {
    let home = TempHome::new();
    home.write_book("{ broken");
    let output = home.oxterm(&["connections"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("connections.json"));
    assert_eq!(
        fs::read_to_string(home.path().join(".config/oxterm/connections.json")).unwrap(),
        "{ broken"
    );
}

#[test]
fn connect_refuses_unknown_names_and_needs_consent_where_asked() {
    let home = TempHome::new();
    home.write_book(BOOK);
    let output = home.oxterm(&["connect", "absent"]);
    assert_eq!(output.status.code(), Some(3));
    let output = home.oxterm(&["connect", "careful"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));
    // Past the consent check, but no window is running.
    let output = home.oxterm(&["connect", "careful", "--yes"]);
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn tunnel_list_works_without_a_window() {
    let home = TempHome::new();
    home.write_book(BOOK);
    let output = home.oxterm(&["tunnel", "list"]);
    assert!(output.status.success());
    assert_eq!(
        text(&output).trim(),
        "db-prod\t0\t-L 127.0.0.1:15433:db:5432\tstopped"
    );
    assert_eq!(
        home.oxterm(&["tunnel", "start", "db-prod"]).status.code(),
        Some(4)
    );
}

// ── Keyring helper ───────────────────────────────────────────

fn askpass(home: &TempHome, prompt: &str) -> Output {
    let mut command = home.command(env!("CARGO_BIN_EXE_oxterm-askpass"));
    command
        .arg(prompt)
        .env("OXTERM_ASKPASS_ID", "c1")
        .env("OXTERM_ASKPASS_TARGET", "admin@10.0.0.1")
        .stdin(Stdio::null());
    // No controlling terminal: a prompt the helper must not answer has to fail,
    // not hang waiting for someone at the test runner's terminal.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.output().expect("run oxterm-askpass")
}

fn install_keyring(home: &TempHome) -> PathBuf {
    let log = home.path().join("keyring.log");
    home.write_script(
        "secret-tool",
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\nprintf 's3cret\\n'",
            log.display()
        ),
    );
    log
}

#[test]
fn the_helper_answers_the_target_password_once() {
    let home = TempHome::new();
    let log = install_keyring(&home);
    let output = askpass(&home, "admin@10.0.0.1's password: ");
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(text(&output), "s3cret\n");
    assert_eq!(
        fs::read_to_string(&log).unwrap().trim(),
        "lookup oxterm-connection c1"
    );
    // The same `ssh` asking again means the password was wrong. Replaying it
    // would lock the account, so the second prompt is left to the person.
    let again = askpass(&home, "admin@10.0.0.1's password: ");
    assert!(!again.status.success());
    assert_eq!(text(&again), "");
}

#[test]
fn the_helper_keeps_the_password_from_anything_else() {
    for prompt in [
        "admin@bastion's password: ",
        "Password: ",
        "Enter passphrase for key '/k': ",
        "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
    ] {
        let home = TempHome::new();
        let log = install_keyring(&home);
        let output = askpass(&home, prompt);
        assert!(!output.status.success(), "answered {:?}", prompt);
        assert_eq!(text(&output), "", "leaked to {:?}", prompt);
        assert!(!log.exists(), "read the keyring for {:?}", prompt);
    }
}

#[test]
fn the_helper_does_not_link_the_gui() {
    let output = Command::new("ldd")
        .arg(env!("CARGO_BIN_EXE_oxterm-askpass"))
        .output();
    let Ok(output) = output else {
        return;
    };
    let libraries = text(&output);
    assert!(!libraries.contains("libgtk"), "{}", libraries);
    assert!(!libraries.contains("libvte"), "{}", libraries);
}

// ── With a window ────────────────────────────────────────────

fn gui_tests_enabled() -> bool {
    if std::env::var_os("OXTERM_GUI_TESTS").is_none() {
        eprintln!("skipping: set OXTERM_GUI_TESTS=1 and run with a display");
        return false;
    }
    true
}

struct Gui(Child);

impl Gui {
    fn start(home: &TempHome) -> Gui {
        let child = home
            .command(env!("CARGO_BIN_EXE_oxterm"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start oxterm");
        let gui = Gui(child);
        wait_until("the control socket", || {
            home.oxterm(&["tunnel", "list"]).status.success()
                && home
                    .oxterm(&["new", "--title", "probe", "--", "sleep", "60"])
                    .status
                    .success()
        });
        gui
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        // SIGTERM goes through Oxterm's shutdown path, which ends the terminals
        // and their brokers. SIGKILL would leave those running after the test.
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {}", what);
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn connect_opens_a_tab_running_the_saved_command() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    home.install_fake_ssh();
    home.write_book(BOOK);
    let _gui = Gui::start(&home);

    let output = home.oxterm(&["connect", "db-prod"]);
    assert!(output.status.success(), "{:?}", output);
    assert!(!text(&output).trim().is_empty(), "prints the pane id");

    let log = home.path().join("ssh.log");
    wait_until("ssh to start", || log.exists());
    let line = fs::read_to_string(&log).unwrap();
    assert!(
        line.contains("-o ControlMaster=auto"),
        "connection sharing applies to saved connections: {}",
        line
    );
    assert!(line.contains("-o ServerAliveInterval=30"), "{}", line);
    assert!(
        line.trim_end()
            .ends_with("-l admin -J bastion -p 2222 10.0.0.1"),
        "{}",
        line
    );

    let state =
        fs::read_to_string(home.path().join(".config/oxterm/connections-state.json")).unwrap();
    assert!(
        state.contains("\"c1\""),
        "the connection counts as recent: {}",
        state
    );
}

#[test]
fn saved_options_reach_ssh_and_the_session_automation_runs() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    let seen = home.path().join("seen.log");
    let answered = home.path().join("answered.log");
    // The first run drops the connection (255), like a network failure. The
    // second prints a prompt, which a saved rule answers.
    home.write_script(
        "ssh",
        &format!(
            r#"if [ "$1" = "-G" ]; then
    printf 'user tester\nhostname h.example\nport 22\ncontrolpath none\nserveraliveinterval 0\n'
    exit 0
fi
printf '%s TERM=%s\n' "$*" "$TERM" >> '{seen}'
if [ "$(wc -l < '{seen}')" = 1 ]; then
    echo first-session-output
    exit 255
fi
echo second-session-output
printf 'Continue? '
read answer
printf '%s\n' "$answer" >> '{answered}'
exec sleep 60"#,
            seen = seen.display(),
            answered = answered.display()
        ),
    );
    let logs = home.path().join("logs");
    let marker = home.path().join("pre-ran");
    home.write_book(&format!(
        r#"{{"version":1,"connections":[{{"id":"c9","name":"full","host":"h",
          "x11":true,"compression":true,"keepalive_interval":45,"legacy_kex":true,
          "terminal_type":"vt220","auto_reconnect":true,"reconnect_delay":1,
          "log_output":true,"log_dir":"{logs}","pre_command":"touch {marker}",
          "macros":[{{"name":"m","command":"uptime"}}],
          "expect":[{{"pattern":"Continue\\?","command":"yes-from-rule","delay_ms":10}}]
        }}]}}"#,
        logs = logs.display(),
        marker = marker.display()
    ));
    let _gui = Gui::start(&home);

    let output = home.oxterm(&["connect", "full"]);
    assert!(output.status.success(), "{:?}", output);
    assert!(marker.exists(), "the pre-connect command ran first");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !fs::read_to_string(&answered).is_ok_and(|text| text.contains('\n')) {
        if Instant::now() >= deadline {
            let listing = text(&home.oxterm(&["--list"]));
            let mut screens = String::new();
            for line in listing.lines().skip(1) {
                let pane = line.split('\t').next().unwrap_or_default();
                screens.push_str(&format!(
                    "--- {}\n{}\n",
                    pane,
                    text(&home.oxterm(&["capture", "--pane", pane]))
                ));
            }
            panic!(
                "no answer from the rule\nssh runs:\n{}\nsessions:\n{}\nscreens:\n{}",
                fs::read_to_string(&seen).unwrap_or_default(),
                listing,
                screens
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        fs::read_to_string(&answered).unwrap().trim(),
        "yes-from-rule"
    );

    let lines = fs::read_to_string(&seen).unwrap();
    assert_eq!(lines.lines().count(), 2, "one reconnect: {}", lines);
    let first = lines.lines().next().unwrap();
    for option in [
        "-X",
        "-C",
        "ServerAliveInterval=45",
        "KexAlgorithms=+",
        "TERM=vt220",
    ] {
        assert!(first.contains(option), "{} missing from {}", option, first);
    }

    wait_until("the session logs", || {
        fs::read_dir(&logs).map_or(false, |files| {
            files.flatten().any(|file| {
                fs::read_to_string(file.path())
                    .is_ok_and(|text| text.contains("second-session-output"))
            })
        })
    });
    for file in fs::read_dir(&logs).unwrap().flatten() {
        let mode = file.metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{:?}", file.path());
    }
}

#[test]
fn a_clean_exit_does_not_reconnect() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    let seen = home.path().join("seen.log");
    home.write_script(
        "ssh",
        &format!(
            r#"if [ "$1" = "-G" ]; then printf 'user t\nhostname h\n'; exit 0; fi
echo run >> '{}'
exit 1"#,
            seen.display()
        ),
    );
    home.write_book(
        r#"{"version":1,"connections":[{"id":"c8","name":"quits","host":"h",
           "auto_reconnect":true,"reconnect_delay":1}]}"#,
    );
    let _gui = Gui::start(&home);
    assert!(home.oxterm(&["connect", "quits"]).status.success());
    wait_until("ssh to run", || seen.exists());
    thread::sleep(Duration::from_secs(3));
    assert_eq!(fs::read_to_string(&seen).unwrap().lines().count(), 1);
}

#[test]
fn tunnels_report_the_real_state_of_the_process() {
    if !gui_tests_enabled() {
        return;
    }
    let home = TempHome::new();
    home.install_fake_ssh();
    // Exits at once, the way ssh does when the port or the login fails.
    home.write_script("ssh", "echo 'bind: Address already in use' >&2\nexit 255");
    home.write_book(BOOK);
    let _gui = Gui::start(&home);

    let output = home.oxterm(&["tunnel", "start", "db-prod", "0"]);
    assert!(output.status.success(), "{:?}", output);
    wait_until("the failure to be reported", || {
        text(&home.oxterm(&["tunnel", "list", "db-prod"])).contains("Address already in use")
    });
    assert_eq!(
        home.oxterm(&["tunnel", "start", "db-prod", "7"])
            .status
            .code(),
        Some(3)
    );
}
