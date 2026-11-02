//! `oxterm mcp` is a stdio server: initialize and tool discovery need no window.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_oxterm")
}

#[test]
fn mcp_answers_the_handshake_and_lists_tools() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = std::env::temp_dir().join(format!("oxterm-mcp-{}-{}", std::process::id(), unique));
    std::fs::create_dir_all(&home).expect("home");

    let mut child = Command::new(binary())
        .arg("mcp")
        .env("HOME", &home)
        .env_remove("OXTERM_CONTROL")
        .env_remove("OXTERM_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxterm mcp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        write!(
            stdin,
            "{}\n{}\n{}\n{}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        )
        .expect("write requests");
    }
    let output = child.wait_with_output().expect("wait for mcp");
    assert!(output.status.success(), "mcp failed: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("JSON response"))
        .collect();
    assert_eq!(
        responses.len(),
        3,
        "notifications must not be answered: {stdout}"
    );
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(responses[1]["id"], 2);
    let tools = responses[1]["result"]["tools"].as_array().expect("tools");
    assert!(tools.iter().any(|tool| tool["name"] == "terminal_send"));
    assert!(tools
        .iter()
        .any(|tool| tool["name"] == "terminal_workspace"));
    assert_eq!(responses[2]["result"]["isError"], true);

    let _ = std::fs::remove_dir_all(&home);
}
