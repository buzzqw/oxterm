//! Model Context Protocol (MCP) server definitions and JSON-RPC helpers.
//!
//! `oxterm mcp` speaks MCP over stdin/stdout: one JSON object per line. The
//! protocol half lives here so the tool schemas and request classification can
//! be tested without a display. The tool implementations live in `control.rs`
//! because they drive the running window through the control socket.

use serde_json::{json, Value};

/// MCP protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Initialize,
    ToolsList,
    ToolsCall {
        name: String,
        arguments: Value,
    },
    Ping,
    /// A JSON-RPC notification (`notifications/initialized`, ...).
    Notification,
    Unknown(String),
}

pub fn classify(value: &Value) -> Request {
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => Request::Initialize,
        "tools/list" => Request::ToolsList,
        "tools/call" => Request::ToolsCall {
            name: value
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            arguments: value
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({})),
        },
        "ping" => Request::Ping,
        other if other.starts_with("notifications/") => Request::Notification,
        other => Request::Unknown(other.to_string()),
    }
}

pub fn initialize_result() -> Value {
    initialize_result_with(None)
}

/// The same response, with `instructions` shown to the client. Oxterm uses it
/// to explain that agent features are disabled instead of silently exposing an
/// empty tool list.
pub fn initialize_result_with(instructions: Option<&str>) -> Value {
    let mut result = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "oxterm",
            "version": env!("CARGO_PKG_VERSION"),
        }
    });
    if let Some(instructions) = instructions {
        result["instructions"] = json!(instructions);
    }
    result
}

pub fn tools() -> Value {
    json!([
        {
            "name": "terminal_list",
            "description": "List the open Oxterm panes with their id, title, directory and status.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "terminal_new",
            "description": "Open a new tab in the running Oxterm and return its pane id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Working directory." },
                    "title": { "type": "string", "description": "Tab title." },
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command argv to run instead of the shell."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "terminal_split",
            "description": "Split an unsplit Oxterm window and return the new pane id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "string", "description": "Target pane id." },
                    "direction": { "type": "string", "enum": ["vertical", "horizontal"] },
                    "cwd": { "type": "string" },
                    "title": { "type": "string" },
                    "command": { "type": "array", "items": { "type": "string" } }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "terminal_send",
            "description": "Type text into a pane, optionally pressing Enter.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "string" },
                    "text": { "type": "string" },
                    "enter": { "type": "boolean" }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "terminal_capture",
            "description": "Read a pane's visible screen, scrollback or whole buffer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "string" },
                    "scrollback": { "type": "integer", "minimum": 0 },
                    "all": { "type": "boolean" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "terminal_wait",
            "description": "Wait for the next command in a pane and return its exit status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "string" },
                    "timeout": { "type": "number", "description": "Seconds." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "terminal_workspace",
            "description": "Open a workspace JSON file in the running Oxterm.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "replace": { "type": "boolean" }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        }
    ])
}

pub fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

pub fn tool_ok(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": false
    })
}

pub fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_requests() {
        assert_eq!(
            classify(&json!({"method":"initialize"})),
            Request::Initialize
        );
        assert_eq!(
            classify(&json!({"method":"tools/list"})),
            Request::ToolsList
        );
        assert_eq!(classify(&json!({"method":"ping"})), Request::Ping);
        assert_eq!(
            classify(&json!({"method":"notifications/initialized"})),
            Request::Notification
        );
        assert_eq!(
            classify(
                &json!({"method":"tools/call","params":{"name":"terminal_send",
                "arguments":{"text":"ls"}}})
            ),
            Request::ToolsCall {
                name: "terminal_send".to_string(),
                arguments: json!({"text":"ls"}),
            }
        );
        assert_eq!(
            classify(&json!({"method":"nope"})),
            Request::Unknown("nope".to_string())
        );
    }

    #[test]
    fn exposes_every_tool_with_a_schema() {
        let tools = tools();
        let list = tools.as_array().expect("tool list");
        assert!(list.len() >= 7);
        for tool in list {
            assert!(tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("terminal_")));
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()));
        }
    }

    #[test]
    fn wraps_results_and_errors() {
        assert_eq!(result(&json!(1), json!({"ok":true}))["jsonrpc"], "2.0");
        assert_eq!(error(&json!(1), -32601, "nope")["error"]["code"], -32601);
        assert_eq!(tool_ok("hi")["content"][0]["text"], "hi");
        assert_eq!(tool_error("bad")["isError"], true);
        assert_eq!(initialize_result()["protocolVersion"], PROTOCOL_VERSION);
    }
}
