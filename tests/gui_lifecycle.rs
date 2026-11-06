//! End-to-end window lifecycle tests.
//!
//! These start the real debug binary on a display and ask it (through the
//! debug-only `OXTERM_TEST_SCENARIO` hook) to detach/close tabs and windows,
//! then assert both what the scenario saw and that the process stayed alive.
//!
//! They are skipped unless `OXTERM_GUI_TESTS=1` is set, because a normal
//! `cargo test` must not open windows on the developer's desktop. CI sets the
//! variable and runs this file under Xvfb.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCENARIOS: &[&str] = &[
    "detach-last-tab",
    "detach-multi-tab",
    "close-last-tab",
    "close-split-pane",
    "detach-from-split",
    "detach-local-view",
    "new-window",
    "split-zoom",
    "theme-switch",
    "tab-failure",
    "agent-auto-fix",
    "preferences",
];

struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> TempHome {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        // Deliberately longer than a Unix socket path allows once the config
        // directory is appended. This exercises the /tmp socket fallback
        // (regression test for the crash with a long $HOME).
        let path = std::env::temp_dir().join(format!(
            "oxterm-gui-test-{}-{}-{}",
            name,
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("create test home");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("make test home private");
        TempHome(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn gui_tests_enabled() -> bool {
    if std::env::var_os("OXTERM_GUI_TESTS").is_none() {
        eprintln!("skipping GUI lifecycle tests: set OXTERM_GUI_TESTS=1 and run with a display");
        return false;
    }
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        panic!("OXTERM_GUI_TESTS is set but DISPLAY and WAYLAND_DISPLAY are both unset");
    }
    true
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => return Err(format!("could not wait for Oxterm: {error}")),
        }
    }
    let _ = child.kill();
    Err(format!(
        "scenario did not finish within {}s",
        timeout.as_secs()
    ))
}

/// A minimal ACP agent: one streamed message per prompt, then the turn ends.
const FAKE_AGENT: &str = r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": {"name": "fake", "title": "Fake Agent"}}}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                          "result": {"sessionId": "s1"}}), flush=True)
    elif method == "session/prompt":
        print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "s1", "update": {"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "OXTERM-SCENARIO-ANSWER"}}}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                          "result": {"stopReason": "end_turn"}}), flush=True)
"#;

/// `agent-auto-fix` needs a real shell and a configured agent, not `sleep`.
fn configure_agent(home: &Path) -> Result<(), String> {
    let agent = home.join("fake-agent.py");
    fs::write(&agent, FAKE_AGENT)
        .map_err(|error| format!("could not write fake agent: {error}"))?;
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("could not make the fake agent executable: {error}"))?;
    let settings = home.join(".config").join("oxterm");
    fs::create_dir_all(&settings)
        .map_err(|error| format!("could not create config dir: {error}"))?;
    fs::write(
        settings.join("settings.json"),
        format!(
            r#"{{"agent_enabled": true, "agent_command": "{}", "shell_command": "/bin/bash", "osc133": true}}"#,
            agent.display()
        ),
    )
    .map_err(|error| format!("could not write settings: {error}"))
}

fn run_scenario(scenario: &str) -> Result<(), String> {
    let home = TempHome::new(scenario);
    let agent_scenario = scenario == "agent-auto-fix";
    if agent_scenario {
        configure_agent(home.path())?;
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxterm"));
    command.arg("--no-restore");
    if !agent_scenario {
        command.args(["-e", "sleep", "30"]);
    }
    let child = command
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_RUNTIME_DIR", home.path())
        .env("OXTERM_TEST_SCENARIO", scenario)
        .env("NO_AT_BRIDGE", "1")
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start Oxterm: {error}"))?;

    let mut child = child;
    let waited = wait_with_timeout(&mut child, Duration::from_secs(30));
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not read Oxterm output: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    waited?;
    if !output.status.success() {
        return Err(format!(
            "scenario failed with {}\nstdout:\n{}\nstderr:\n{}",
            output.status, stdout, stderr
        ));
    }
    if !stderr.contains("OXTERM TEST SCENARIO OK") {
        return Err(format!(
            "the test scenario did not report success\nstdout:\n{}\nstderr:\n{}",
            stdout, stderr
        ));
    }
    Ok(())
}

#[test]
fn window_lifecycle_scenarios() {
    if !gui_tests_enabled() {
        return;
    }
    if !cfg!(debug_assertions) {
        eprintln!("skipping GUI lifecycle tests: OXTERM_TEST_SCENARIO is debug-only");
        return;
    }

    let mut failures = Vec::new();
    for scenario in SCENARIOS {
        match run_scenario(scenario) {
            Ok(()) => eprintln!("GUI scenario passed: {scenario}"),
            Err(error) => failures.push(format!("{scenario}: {error}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} GUI scenario(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
