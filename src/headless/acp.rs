//! The Agent Client Protocol (ACP) as Oxterm's client half.
//!
//! Oxterm *hosts* an agent in its agent pane: it starts a configured ACP agent
//! CLI, performs the `initialize` / `session/new` handshake, sends prompts and
//! renders the streamed updates. Everything on this page is display-free: JSON
//! shapes, message classification and update extraction, so the protocol can be
//! tested without a window and the GTK panel only deals with widgets.
//!
//! ACP runs over stdio with one JSON-RPC object per line, like the MCP server in
//! `mcp.rs`. The transport itself lives in `agent_panel.rs`; Oxterm is only the
//! client, never the agent.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// ACP major protocol version supported by this client.
pub const PROTOCOL_VERSION: i64 = 1;

/// An MCP server handed to the agent when the session is created. Oxterm passes
/// itself (`oxterm mcp`) so the agent can drive tabs, subject to the same
/// permission policy as any other MCP client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

impl McpServer {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "command": self.command,
            "args": self.args,
            "env": [],
        })
    }
}

/// `initialize` request. Read-only client: no filesystem or terminal methods are
/// advertised, so the agent uses its own tools (and asks for permission).
pub fn initialize(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "oxterm",
                "title": "Oxterm",
                "version": env!("CARGO_PKG_VERSION"),
            },
        },
    })
}

/// `session/new` request, with the servers the agent may connect to.
pub fn session_new(id: u64, cwd: &str, servers: &[McpServer]) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": servers.iter().map(McpServer::to_json).collect::<Vec<_>>(),
        },
    })
}

/// Request ids from this value up belong to session configuration
/// (`session/set_config_option`) instead of prompts.
///
/// A JSON-RPC response does not repeat the method, so the reader thread has to
/// tell the two apart by id: a configuration response updates the model picker
/// and must never complete a turn.
pub const CONFIG_ID_BASE: u64 = 1_000_000_000;

/// `session/set_config_option` request: choose one of the values the agent
/// listed in `configOptions`, such as the model.
pub fn set_config_option(id: u64, session_id: &str, config_id: &str, value: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": config_id,
            "value": value,
        },
    })
}

/// One selectable value of a `configOptions` entry reported by the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigChoice {
    pub value: String,
    /// Human label, falling back to the value when the agent omits it.
    pub name: String,
}

/// Which request switches models.
///
/// Newer agents report a `model` entry in `configOptions` and take
/// `session/set_config_option`; older ones (Antigravity, for example) report a
/// top-level `models` object and take `session/set_model`. Both are in use, so
/// the client speaks whichever the agent announced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelApi {
    ConfigOption,
    SetModel,
}

/// The model list a session reported and the request that changes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Models {
    pub current: Option<String>,
    pub choices: Vec<ConfigChoice>,
    pub api: ModelApi,
}

/// Read the model list from either ACP shape.
pub fn session_models(result: &Value) -> Option<Models> {
    let (current, choices) = config_choices(result, "model");
    if current.is_some() || !choices.is_empty() {
        return Some(Models {
            current,
            choices,
            api: ModelApi::ConfigOption,
        });
    }
    let models = result.get("models")?;
    let current = models
        .get("currentModelId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let choices: Vec<ConfigChoice> = models
        .get("availableModels")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|model| {
                    let value = model.get("modelId").and_then(Value::as_str)?;
                    if value.is_empty() {
                        return None;
                    }
                    let name = model
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(value);
                    Some(ConfigChoice {
                        value: value.to_string(),
                        name: name.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if current.is_none() && choices.is_empty() {
        return None;
    }
    Some(Models {
        current,
        choices,
        api: ModelApi::SetModel,
    })
}

/// `session/set_model` request, the older way to choose a model.
pub fn set_model(id: u64, session_id: &str, model_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_model",
        "params": { "sessionId": session_id, "modelId": model_id },
    })
}

/// The value in use and the choices offered for `config` (for example
/// `"model"`) in a `session/new` or `session/set_config_option` result.
///
/// An empty choice list means the agent does not report that config at all,
/// which is not an error: older agents only support prompts.
pub fn config_choices(result: &Value, config: &str) -> (Option<String>, Vec<ConfigChoice>) {
    let Some(entry) = result
        .get("configOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .find(|option| option.get("id").and_then(Value::as_str) == Some(config))
        })
    else {
        return (None, Vec::new());
    };
    let current = entry
        .get("currentValue")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let choices = entry
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let value = option.get("value").and_then(Value::as_str)?;
                    if value.is_empty() {
                        return None;
                    }
                    let name = option
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(value);
                    Some(ConfigChoice {
                        value: value.to_string(),
                        name: name.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (current, choices)
}

/// `session/prompt` request with one text block.
pub fn session_prompt(id: u64, session_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": text }],
        },
    })
}

/// `session/cancel` notification (no id, no response).
pub fn session_cancel(session_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id },
    })
}

/// Answer a `session/request_permission`. `None` means cancelled, which is the
/// required answer when the turn was cancelled.
pub fn permission_response(id: &Value, option_id: Option<&str>) -> Value {
    let outcome = match option_id {
        Some(option) => json!({ "outcome": "selected", "optionId": option }),
        None => json!({ "outcome": "cancelled" }),
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } })
}

/// One message read from the agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// Parse one JSON-RPC line from the agent.
pub fn parse(line: &str) -> Result<Incoming, String> {
    let value: Value =
        serde_json::from_str(line).map_err(|error| format!("invalid JSON: {error}"))?;
    if !value.is_object() {
        return Err("a JSON-RPC message must be an object".to_string());
    }
    let method = value.get("method").and_then(Value::as_str);
    let id = value.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) if !method.is_empty() => Ok(Incoming::Request {
            id,
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(method), None) if !method.is_empty() => Ok(Incoming::Notification {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(_), _) => Err("empty method".to_string()),
        (None, Some(id)) => Ok(Incoming::Response {
            id,
            result: value.get("result").cloned(),
            error: value.get("error").cloned().filter(|error| !error.is_null()),
        }),
        (None, None) => Err("message has neither method nor id".to_string()),
    }
}

/// A permission option offered by the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}

/// Options of a `session/request_permission` request.
pub fn permission_options(params: &Value) -> Vec<PermissionOption> {
    params
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    Some(PermissionOption {
                        id: option.get("optionId")?.as_str()?.to_string(),
                        name: option.get("name")?.as_str()?.to_string(),
                        kind: option
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Title of the tool call a permission request is about, shown in the dialog.
pub fn permission_title(params: &Value) -> String {
    params
        .pointer("/toolCall/title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("run a tool")
        .to_string()
}

/// First `allow_once` option, used as the default the user can approve.
pub fn default_allow_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == "allow_once")
        .or_else(|| {
            options
                .iter()
                .find(|option| option.kind.starts_with("allow"))
        })
}

/// First rejection option, used as the safe default.
pub fn default_reject_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == "reject_once")
        .or_else(|| {
            options
                .iter()
                .find(|option| option.kind.starts_with("reject"))
        })
}

/// A rendered piece of a `session/update` notification.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    UserMessage(String),
    AgentMessage(String),
    Thought(String),
    ToolCall {
        id: String,
        title: String,
        kind: String,
        status: String,
    },
    ToolCallUpdate {
        id: String,
        title: Option<String>,
        status: Option<String>,
        detail: Option<String>,
    },
    Plan(Vec<String>),
    Usage {
        used: u64,
        size: u64,
        cost: Option<String>,
    },
    Notice {
        severity: String,
        title: String,
    },
    /// A valid update this client does not render (mode changes, compaction…).
    Other,
}

/// Extract the text of a content block (text or a resource with text).
pub fn content_text(content: &Value) -> Option<String> {
    match content.get("type").and_then(Value::as_str)? {
        "text" => content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        "resource" => content
            .pointer("/resource/text")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn text_of(value: &Value) -> Option<String> {
    content_text(value)
}

/// Extract a displayable update from `session/update` params.
pub fn parse_update(params: &Value) -> Option<Update> {
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate").and_then(Value::as_str)?;
    match kind {
        "user_message_chunk" => Some(Update::UserMessage(
            update.get("content").and_then(text_of).unwrap_or_default(),
        )),
        "agent_message_chunk" => Some(Update::AgentMessage(
            update.get("content").and_then(text_of).unwrap_or_default(),
        )),
        "agent_thought_chunk" => Some(Update::Thought(
            update.get("content").and_then(text_of).unwrap_or_default(),
        )),
        "tool_call" => Some(Update::ToolCall {
            id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            title: update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string(),
            kind: update
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("other")
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
        }),
        "tool_call_update" => Some(Update::ToolCallUpdate {
            id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            title: update
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string),
            detail: tool_update_text(update),
        }),
        "plan" => Some(Update::Plan(
            update
                .get("entries")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| entry.get("content").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        )),
        "usage_update" => Some(Update::Usage {
            used: update.get("used").and_then(Value::as_u64).unwrap_or(0),
            size: update.get("size").and_then(Value::as_u64).unwrap_or(0),
            cost: update.get("cost").map(|cost| {
                let amount = cost.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
                let currency = cost
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("USD");
                format!("{:.2} {}", amount, currency)
            }),
        }),
        "notice" => Some(Update::Notice {
            severity: update
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("info")
                .to_string(),
            title: update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        _ => Some(Update::Other),
    }
}

fn tool_update_text(update: &Value) -> Option<String> {
    let content = update.get("content")?.as_array()?;
    let mut text = String::new();
    for block in content {
        if let Some(chunk) = block.get("content").and_then(text_of) {
            text.push_str(&chunk);
        } else if block.get("type").and_then(Value::as_str) == Some("diff") {
            let path = block.get("path").and_then(Value::as_str).unwrap_or("file");
            text.push_str(&format!("[diff {path}]"));
        }
    }
    (!text.is_empty()).then_some(text)
}

/// What a successful [`probe`] learned about an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub agent: String,
    pub model: Option<String>,
    pub protocol: i64,
}

/// Start `argv`, perform the ACP handshake and report the agent it finds.
///
/// The Preferences page uses this to verify a command before saving it: it
/// proves the program exists, speaks ACP, finishes `session/new` and (when the
/// agent reports it) names the model. The child is killed before returning.
/// This is display-free so it can also be tested with a fake agent.
pub fn probe(argv: &[String], cwd: &str, timeout: Duration) -> Result<Probe, String> {
    let program = argv.first().ok_or("agent_command is empty")?;
    // The preferences dialog may run with the desktop's minimal PATH; resolve
    // the same way the agent pane does.
    let program = super::agent::resolve_program(program);
    let mut child = Command::new(&program)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{}: {}", program, error))?;
    let mut stdin = child.stdin.take().ok_or("the agent has no stdin")?;
    let stdout = child.stdout.take().ok_or("the agent has no stdout")?;
    let (lines, received) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("oxterm-acp-probe".to_string())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() && lines.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| format!("cannot read from the agent: {}", error))?;

    let stop = |child: &mut Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    if let Err(error) = write_message(&mut stdin, &initialize(1)) {
        stop(&mut child);
        return Err(format!("cannot write to the agent: {}", error));
    }

    let deadline = Instant::now() + timeout;
    let mut agent = String::from("agent");
    let mut protocol = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            stop(&mut child);
            return Err("the agent did not answer in time".to_string());
        }
        let line = match received.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                stop(&mut child);
                return Err("the agent did not answer in time".to_string());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop(&mut child);
                return Err("the agent exited before answering".to_string());
            }
        };
        match parse(&line) {
            Ok(Incoming::Response { id, result, error }) => {
                if let Some(error) = error {
                    stop(&mut child);
                    return Err(error_message(&error));
                }
                match id.as_u64() {
                    Some(1) => {
                        let result = result.unwrap_or(Value::Null);
                        protocol = result
                            .get("protocolVersion")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        agent = agent_title(&result);
                        if let Err(error) = write_message(&mut stdin, &session_new(2, cwd, &[])) {
                            stop(&mut child);
                            return Err(format!("cannot write to the agent: {}", error));
                        }
                    }
                    Some(2) => {
                        let result = result.unwrap_or(Value::Null);
                        let session = result
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if session.is_empty() {
                            stop(&mut child);
                            return Err("the agent did not return a session id".to_string());
                        }
                        let model = session_model(&result);
                        stop(&mut child);
                        return Ok(Probe {
                            agent,
                            model,
                            protocol,
                        });
                    }
                    _ => {}
                }
            }
            Ok(Incoming::Request { id, method, .. }) => {
                // Never leave the probe waiting on a method we do not implement.
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("unsupported method: {}", method) },
                });
                let _ = write_message(&mut stdin, &response);
            }
            Ok(Incoming::Notification { .. }) => {}
            Err(_) => {}
        }
    }
}

fn write_message(stdin: &mut ChildStdin, value: &Value) -> std::io::Result<()> {
    writeln!(stdin, "{}", value)?;
    stdin.flush()
}

fn error_message(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the agent returned an error");
    let data = error
        .get("data")
        .and_then(Value::as_str)
        .filter(|data| !data.is_empty());
    match data {
        Some(data) => format!("{}: {}", message, data),
        None => message.to_string(),
    }
}

/// Model selected for the session, when the agent reports it in the
/// `configOptions` of `session/new` or `session/set_config_option`.
pub fn session_model(result: &Value) -> Option<String> {
    session_models(result).and_then(|models| models.current)
}

/// Stop reason of a completed `session/prompt` response.
pub fn stop_reason(result: &Value) -> String {
    result
        .get("stopReason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn")
        .to_string()
}

/// Model label reported by `initialize`/`session/new`, when present. This is
/// display metadata only.
pub fn agent_title(result: &Value) -> String {
    result
        .pointer("/agentInfo/title")
        .and_then(Value::as_str)
        .or_else(|| result.pointer("/agentInfo/name").and_then(Value::as_str))
        .unwrap_or("agent")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_documented_shape() {
        let init = initialize(1);
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["protocolVersion"], 1);
        assert_eq!(init["params"]["clientInfo"]["name"], "oxterm");
        assert!(init["params"]["clientCapabilities"].is_object());

        let servers = [McpServer {
            name: "oxterm".to_string(),
            command: "/usr/bin/oxterm".to_string(),
            args: vec!["mcp".to_string()],
        }];
        let new = session_new(2, "/tmp", &servers);
        assert_eq!(new["params"]["cwd"], "/tmp");
        assert_eq!(new["params"]["mcpServers"][0]["name"], "oxterm");
        assert_eq!(new["params"]["mcpServers"][0]["args"][0], "mcp");

        let prompt = session_prompt(3, "s1", "hello");
        assert_eq!(prompt["params"]["sessionId"], "s1");
        assert_eq!(prompt["params"]["prompt"][0]["type"], "text");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "hello");

        let cancel = session_cancel("s1");
        assert!(cancel.get("id").is_none());
        assert_eq!(cancel["method"], "session/cancel");

        let allow = permission_response(&json!(7), Some("allow-once"));
        assert_eq!(allow["result"]["outcome"]["outcome"], "selected");
        assert_eq!(allow["result"]["outcome"]["optionId"], "allow-once");
        let cancelled = permission_response(&json!(7), None);
        assert_eq!(cancelled["result"]["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn parses_responses_requests_and_notifications() {
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap(),
            Incoming::Response {
                id: json!(1),
                result: Some(json!({"ok": true})),
                error: None,
            }
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":"a","error":{"code":-1,"message":"no"}}"#).unwrap(),
            Incoming::Response {
                id: json!("a"),
                result: None,
                error: Some(json!({"code": -1, "message": "no"})),
            }
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":5,"method":"session/request_permission","params":{}}"#)
                .unwrap(),
            Incoming::Request {
                id: json!(5),
                method: "session/request_permission".to_string(),
                params: json!({}),
            }
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#).unwrap(),
            Incoming::Notification {
                method: "session/update".to_string(),
                params: json!({}),
            }
        );
        assert!(parse("[]").is_err());
        assert!(parse("{}").is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn extracts_streamed_text_and_thoughts() {
        let message = json!({"update": {
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello"},
        }});
        assert_eq!(
            parse_update(&message),
            Some(Update::AgentMessage("hello".to_string()))
        );
        let thought = json!({"update": {
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "hmm"},
        }});
        assert_eq!(
            parse_update(&thought),
            Some(Update::Thought("hmm".to_string()))
        );
        let user = json!({"update": {
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "hi"},
        }});
        assert_eq!(
            parse_update(&user),
            Some(Update::UserMessage("hi".to_string()))
        );
    }

    #[test]
    fn extracts_tools_plans_usage_and_notices() {
        let tool = json!({"update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "call-1",
            "title": "Reading src/main.rs",
            "kind": "read",
            "status": "pending",
        }});
        assert_eq!(
            parse_update(&tool),
            Some(Update::ToolCall {
                id: "call-1".to_string(),
                title: "Reading src/main.rs".to_string(),
                kind: "read".to_string(),
                status: "pending".to_string(),
            })
        );
        let finished = json!({"update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed",
            "content": [
                {"type": "content", "content": {"type": "text", "text": "done"}},
                {"type": "diff", "path": "src/main.rs"},
            ],
        }});
        assert_eq!(
            parse_update(&finished),
            Some(Update::ToolCallUpdate {
                id: "call-1".to_string(),
                title: None,
                status: Some("completed".to_string()),
                detail: Some("done[diff src/main.rs]".to_string()),
            })
        );
        let plan = json!({"update": {
            "sessionUpdate": "plan",
            "entries": [{"content": "read"}, {"content": "fix"}],
        }});
        assert_eq!(
            parse_update(&plan),
            Some(Update::Plan(vec!["read".to_string(), "fix".to_string()]))
        );
        let usage = json!({"update": {
            "sessionUpdate": "usage_update",
            "used": 5300,
            "size": 200000,
            "cost": {"amount": 0.045, "currency": "USD"},
        }});
        assert_eq!(
            parse_update(&usage),
            Some(Update::Usage {
                used: 5300,
                size: 200000,
                cost: Some("0.04 USD".to_string()),
            })
        );
        let notice = json!({"update": {
            "sessionUpdate": "notice",
            "severity": "warning",
            "title": "MCP server unavailable",
        }});
        assert_eq!(
            parse_update(&notice),
            Some(Update::Notice {
                severity: "warning".to_string(),
                title: "MCP server unavailable".to_string(),
            })
        );
        let compaction = json!({"update": {"sessionUpdate": "compaction_update"}});
        assert_eq!(parse_update(&compaction), Some(Update::Other));
        assert_eq!(parse_update(&json!({})), None);
    }

    #[test]
    fn reads_permission_options_and_picks_a_safe_default() {
        let params = json!({
            "toolCall": {"title": "Run make test"},
            "options": [
                {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
            ],
        });
        let options = permission_options(&params);
        assert_eq!(options.len(), 2);
        assert_eq!(permission_title(&params), "Run make test");
        assert_eq!(
            default_allow_option(&options).map(|option| option.id.as_str()),
            Some("allow-once")
        );
        assert_eq!(
            default_reject_option(&options).map(|option| option.id.as_str()),
            Some("reject-once")
        );
        assert_eq!(permission_title(&json!({})), "run a tool");
    }

    #[test]
    fn a_probe_completes_the_handshake_and_reports_the_model() {
        let path = std::env::temp_dir().join(format!(
            "oxterm-acp-probe-{}-{}.py",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": {"name": "fake", "title": "Fake Probe"}}}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
            "sessionId": "s1",
            "configOptions": [{"id": "model", "currentValue": "test/model"}]}}), flush=True)
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let argv = vec![path.to_string_lossy().to_string()];
        let report = probe(&argv, "/tmp", Duration::from_secs(5)).expect("probe");
        assert_eq!(
            report,
            Probe {
                agent: "Fake Probe".to_string(),
                model: Some("test/model".to_string()),
                protocol: 1,
            }
        );
        let missing = probe(
            &["/definitely/not/an/agent".to_string()],
            "/tmp",
            Duration::from_secs(2),
        );
        assert!(missing.is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_the_session_model_when_reported() {
        let result = json!({"sessionId": "s", "configOptions": [
            {"id": "model", "currentValue": "deepseek/deepseek-v4-pro"},
            {"id": "mode", "currentValue": "agent"},
        ]});
        assert_eq!(
            session_model(&result),
            Some("deepseek/deepseek-v4-pro".to_string())
        );
        assert_eq!(session_model(&json!({"sessionId": "s"})), None);
        assert_eq!(session_model(&json!({"configOptions": []})), None);
    }

    #[test]
    fn reads_the_model_choices_and_current_value() {
        let result = json!({"sessionId": "s", "configOptions": [
            {"id": "model", "name": "Model", "currentValue": "deepseek/deepseek-flash",
             "options": [
                {"value": "deepseek/deepseek-v4-pro", "name": "DeepSeek V4 Pro"},
                {"value": "deepseek/deepseek-flash"},
             ]},
            {"id": "mode", "currentValue": "build"},
        ]});
        let (current, choices) = config_choices(&result, "model");
        assert_eq!(current.as_deref(), Some("deepseek/deepseek-flash"));
        assert_eq!(
            choices,
            vec![
                ConfigChoice {
                    value: "deepseek/deepseek-v4-pro".to_string(),
                    name: "DeepSeek V4 Pro".to_string(),
                },
                ConfigChoice {
                    value: "deepseek/deepseek-flash".to_string(),
                    name: "deepseek/deepseek-flash".to_string(),
                },
            ]
        );
        // A config the agent does not report, and an agent with no options.
        assert_eq!(config_choices(&result, "effort"), (None, Vec::new()));
        assert_eq!(
            config_choices(&json!({"sessionId": "s"}), "model"),
            (None, Vec::new())
        );
    }

    #[test]
    fn reads_the_legacy_model_list_and_its_request() {
        let result = json!({"sessionId": "s", "models": {
            "currentModelId": "gemini-3.8-flash-high",
            "availableModels": [
                {"modelId": "gemini-3.8-flash-high", "name": "Gemini 3.8 Flash (High)"},
                {"modelId": "gemini-3.8-flash-low"},
            ],
        }});
        let models = session_models(&result).expect("legacy models");
        assert_eq!(models.api, ModelApi::SetModel);
        assert_eq!(models.current.as_deref(), Some("gemini-3.8-flash-high"));
        assert_eq!(
            models.choices,
            vec![
                ConfigChoice {
                    value: "gemini-3.8-flash-high".to_string(),
                    name: "Gemini 3.8 Flash (High)".to_string(),
                },
                ConfigChoice {
                    value: "gemini-3.8-flash-low".to_string(),
                    name: "gemini-3.8-flash-low".to_string(),
                },
            ]
        );
        // configOptions win when both shapes are present.
        let modern = json!({"configOptions": [
            {"id": "model", "currentValue": "a/b", "options": [{"value": "a/b", "name": "A"}]}
        ], "models": {"currentModelId": "x/y"}});
        assert_eq!(session_models(&modern).unwrap().api, ModelApi::ConfigOption);
        assert_eq!(session_model(&modern).as_deref(), Some("a/b"));
        assert_eq!(session_models(&json!({"sessionId": "s"})), None);

        let request = set_model(CONFIG_ID_BASE + 4, "s1", "c/d");
        assert_eq!(request["method"], "session/set_model");
        assert_eq!(request["params"]["modelId"], "c/d");
        assert!(request["id"].as_u64().unwrap() >= CONFIG_ID_BASE);
    }

    #[test]
    fn builds_a_config_change_that_does_not_look_like_a_prompt() {
        let request = set_config_option(CONFIG_ID_BASE + 3, "s1", "model", "a/b");
        assert_eq!(request["method"], "session/set_config_option");
        assert_eq!(request["params"]["sessionId"], "s1");
        assert_eq!(request["params"]["configId"], "model");
        assert_eq!(request["params"]["value"], "a/b");
        assert!(request["id"].as_u64().unwrap() >= CONFIG_ID_BASE);
    }

    #[test]
    fn reads_agent_identity_and_stop_reason() {
        assert_eq!(
            agent_title(&json!({"agentInfo": {"name": "copilot", "title": "Copilot"}})),
            "Copilot"
        );
        assert_eq!(agent_title(&json!({})), "agent");
        assert_eq!(
            stop_reason(&json!({"stopReason": "cancelled"})),
            "cancelled"
        );
        assert_eq!(stop_reason(&json!({})), "end_turn");
    }
}
