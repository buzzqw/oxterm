//! `oxterm mcp` is a stdio server: initialize and tool discovery need no window.
//!
//! Agent features are opt-in, so the tests write their own settings file in a
//! throwaway HOME instead of relying on the developer's configuration.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_oxterm")
}

fn temp_home(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("oxterm-mcp-{name}-{}-{unique}", std::process::id()))
}

fn write_settings(home: &Path, settings: &str) {
    let directory = home.join(".config").join("oxterm");
    std::fs::create_dir_all(&directory).expect("config directory");
    std::fs::write(directory.join("settings.json"), settings).expect("settings file");
}

fn run_mcp(home: &Path, requests: &str) -> Vec<Value> {
    let mut child = Command::new(binary())
        .arg("mcp")
        .env("HOME", home)
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxterm mcp");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(requests.as_bytes())
        .expect("write requests");
    let output = child.wait_with_output().expect("wait for mcp");
    assert!(output.status.success(), "mcp failed: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("JSON response"))
        .collect()
}

#[test]
fn mcp_answers_the_handshake_and_lists_tools_when_enabled() {
    let home = temp_home("enabled");
    write_settings(
        &home,
        r#"{"agent_enabled": true, "agent_command": "true", "agent_audit": false}"#,
    );

    let responses = run_mcp(
        &home,
        concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            "\n",
        ),
    );
    assert_eq!(
        responses.len(),
        3,
        "notifications must not be answered: {responses:?}"
    );
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    assert!(responses[0]["result"].get("instructions").is_none());
    assert_eq!(responses[1]["id"], 2);
    let tools = responses[1]["result"]["tools"].as_array().expect("tools");
    assert!(tools.iter().any(|tool| tool["name"] == "terminal_send"));
    assert!(tools
        .iter()
        .any(|tool| tool["name"] == "terminal_workspace"));
    assert_eq!(responses[2]["result"]["isError"], true);

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn mcp_exposes_no_tools_until_the_user_enables_agents() {
    let home = temp_home("disabled");
    std::fs::create_dir_all(&home).expect("home");

    let responses = run_mcp(
        &home,
        concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
        ),
    );
    assert_eq!(responses.len(), 2);
    let instructions = responses[0]["result"]["instructions"]
        .as_str()
        .expect("instructions explain why there are no tools");
    assert!(instructions.contains("agent_enabled"));
    assert_eq!(
        responses[1]["result"]["tools"]
            .as_array()
            .expect("tools array")
            .len(),
        0,
        "disabled agent features must not advertise tools"
    );

    let _ = std::fs::remove_dir_all(&home);
}
