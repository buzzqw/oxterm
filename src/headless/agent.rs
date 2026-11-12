//! Opt-in agent integration: permissions, auditing and fix context.
//!
//! Nothing here talks to a model or starts a process. The module holds the
//! parts that must be identical for the GUI, the MCP server and the command
//! line: which class of action a tool belongs to, whether the user's policy
//! allows it, how an audit record is written, how an agent command is parsed and
//! how the context of a failed command is turned into a prompt.
//!
//! Everything is off by default. `agent_enabled` must be true *and* the user
//! must have configured `agent_command`; a terminal nobody has configured never
//! starts an agent, never advertises a tool and never reads context.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use super::paths;

/// Longest command output included in a fix prompt. The tail is what matters,
/// so it is the head that gets dropped.
pub const MAX_OUTPUT_CHARS: usize = 8_000;

/// Longest hint accepted from the command line.
pub const MAX_HINT_CHARS: usize = 1_000;

/// What kind of change an agent request makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionClass {
    /// Look at the terminal without changing it.
    Read,
    /// Open a tab, split a pane or load a workspace.
    Create,
    /// Type into a shell or start a program: this runs code as the user.
    Input,
}

impl ActionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionClass::Read => "read",
            ActionClass::Create => "create",
            ActionClass::Input => "input",
        }
    }

    pub fn parse(text: &str) -> Option<ActionClass> {
        match text {
            "read" => Some(ActionClass::Read),
            "create" => Some(ActionClass::Create),
            "input" => Some(ActionClass::Input),
            _ => None,
        }
    }
}

/// A per-class permission setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Perform the action without asking.
    Auto,
    /// Ask the user in a dialog before the action.
    Ask,
    /// Refuse the action.
    Deny,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Auto => "auto",
            Policy::Ask => "ask",
            Policy::Deny => "deny",
        }
    }

    pub fn parse(text: &str) -> Option<Policy> {
        match text {
            "auto" => Some(Policy::Auto),
            "ask" => Some(Policy::Ask),
            "deny" => Some(Policy::Deny),
            _ => None,
        }
    }
}

/// The decision for one request, after the policy has been applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        }
    }

    pub fn allowed(self) -> bool {
        self != Decision::Deny
    }
}

/// Effective permissions for the three action classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policies {
    pub read: Policy,
    pub create: Policy,
    pub input: Policy,
}

impl Default for Policies {
    /// Reading a pane is harmless, creating a tab is visible and typing is code
    /// execution, so the default gets stricter as the impact grows. The master
    /// switch (`agent_enabled`) is off anyway.
    fn default() -> Self {
        Policies {
            read: Policy::Auto,
            create: Policy::Ask,
            input: Policy::Ask,
        }
    }
}

impl Policies {
    /// Read the three `agent_policy_*` settings from an effective document.
    /// Unknown values fall back to the safe default for that class instead of
    /// silently becoming "auto".
    pub fn from_settings(settings: &Value) -> Policies {
        let default = Policies::default();
        let policy = |key: &str, fallback: Policy| {
            settings
                .get(key)
                .and_then(Value::as_str)
                .and_then(Policy::parse)
                .unwrap_or(fallback)
        };
        Policies {
            read: policy("agent_policy_read", default.read),
            create: policy("agent_policy_create", default.create),
            input: policy("agent_policy_input", default.input),
        }
    }

    pub fn for_class(&self, class: ActionClass) -> Policy {
        match class {
            ActionClass::Read => self.read,
            ActionClass::Create => self.create,
            ActionClass::Input => self.input,
        }
    }

    pub fn decide(&self, class: ActionClass) -> Decision {
        match self.for_class(class) {
            Policy::Auto => Decision::Allow,
            Policy::Ask => Decision::Ask,
            Policy::Deny => Decision::Deny,
        }
    }
}

/// The master switch. Off unless the user turned it on explicitly.
pub fn enabled(settings: &Value) -> bool {
    settings
        .get("agent_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether an agent command is configured, i.e. whether the agent pane and
/// `oxterm fix` have something to start.
pub fn configured(settings: &Value) -> bool {
    enabled(settings) && !settings_string(settings, "agent_command").trim().is_empty()
}

/// A line typed at the shell prompt that starts with `?` is a question for the
/// agent, not a command: `?explain this` returns `Some("explain this")`.
///
/// `Some("")` means the user typed only `?` and wants the pane focused, while
/// `None` means the line is an ordinary command and must reach the shell.
pub fn prompt_from_line(line: &str) -> Option<&str> {
    line.trim().strip_prefix('?').map(str::trim)
}

/// Whether a `?` line should go to the agent rather than the shell.
///
/// This is the only way a line typed at the prompt starts the agent: a failed
/// command is never handed over automatically. The preference is on by default
/// and only applies when the agent features themselves are enabled.
pub fn question_prefix_enabled(settings: &Value) -> bool {
    enabled(settings)
        && settings
            .get("agent_question_prefix")
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

/// Whether the agent may drive the terminal through the MCP server.
pub fn mcp_available(settings: &Value) -> bool {
    enabled(settings)
}

/// Whether the agent session should be handed Oxterm's own MCP server.
pub fn mcp_shared(settings: &Value) -> bool {
    enabled(settings)
        && settings
            .get("agent_mcp")
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

fn settings_string(settings: &Value, key: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Which action class a tool name belongs to. `None` means the tool is not one
/// of ours and must not be executed at all.
///
/// The arguments matter: `terminal_new` and `terminal_split` are harmless when
/// they open a shell and become code execution when they carry a command, and a
/// workspace file can contain commands too.
pub fn tool_class(name: &str, arguments: &Value) -> Option<ActionClass> {
    match name {
        "terminal_list" | "terminal_capture" | "terminal_wait" => Some(ActionClass::Read),
        "terminal_send" => Some(ActionClass::Input),
        "terminal_new" | "terminal_split" => {
            let has_command = arguments
                .get("command")
                .and_then(Value::as_array)
                .is_some_and(|command| !command.is_empty());
            Some(if has_command {
                ActionClass::Input
            } else {
                ActionClass::Create
            })
        }
        // A workspace can start arbitrary commands, so opening one is input.
        "terminal_workspace" => Some(ActionClass::Input),
        _ => None,
    }
}

/// Human description of a tool call for the confirmation dialog and the audit
/// log. Secrets typed into a pane are shown to the user who is approving them,
/// but never written to the audit log.
pub fn describe_tool(name: &str, arguments: &Value) -> String {
    match name {
        "terminal_send" => {
            let text = arguments.get("text").and_then(Value::as_str).unwrap_or("");
            let enter = arguments
                .get("enter")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!(
                "type into the shell{}:\n\n{}",
                if enter { " and press Enter" } else { "" },
                truncate_head(text, 600)
            )
        }
        "terminal_new" | "terminal_split" => {
            match arguments.get("command").and_then(Value::as_array) {
                Some(command) if !command.is_empty() => format!(
                    "run this command in a new {}:\n\n{}",
                    if name == "terminal_new" {
                        "tab"
                    } else {
                        "pane"
                    },
                    command
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                _ => format!(
                    "open a new {}",
                    if name == "terminal_new" {
                        "tab"
                    } else {
                        "pane"
                    }
                ),
            }
        }
        "terminal_workspace" => format!(
            "open the workspace file {}",
            arguments
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or("(missing)")
        ),
        "terminal_capture" => "read the terminal contents".to_string(),
        "terminal_list" => "list the open panes".to_string(),
        "terminal_wait" => "wait for a command to finish".to_string(),
        _ => name.to_string(),
    }
}

/// Details for the audit log. Deliberately excludes the text typed into a
/// shell, which may contain passwords and tokens.
pub fn audit_details(name: &str, arguments: &Value) -> String {
    match name {
        "terminal_send" => {
            let length = arguments
                .get("text")
                .and_then(Value::as_str)
                .map(|text| text.chars().count())
                .unwrap_or(0);
            format!("{} characters", length)
        }
        "terminal_new" | "terminal_split" => {
            let has_command = arguments
                .get("command")
                .and_then(Value::as_array)
                .is_some_and(|command| !command.is_empty());
            if has_command {
                "with a command".to_string()
            } else {
                "shell only".to_string()
            }
        }
        "terminal_workspace" => "workspace file".to_string(),
        "terminal_capture" => {
            let scope = if arguments.get("all").and_then(Value::as_bool) == Some(true) {
                "whole buffer"
            } else if let Some(lines) = arguments.get("scrollback").and_then(Value::as_u64) {
                return format!("{lines} lines");
            } else {
                "visible screen"
            };
            scope.to_string()
        }
        _ => String::new(),
    }
}

/// Path of the append-only audit log.
pub fn audit_file() -> std::path::PathBuf {
    paths::state_dir().join("agent-audit.log")
}

/// One JSON line describing a decision. `allowed` is the final outcome, which
/// differs from the policy decision when the user answered a prompt.
pub fn audit_line(
    time: &str,
    source: &str,
    tool: &str,
    class: ActionClass,
    decision: Decision,
    allowed: bool,
    pane: Option<&str>,
    details: &str,
) -> String {
    json!({
        "time": time,
        "source": source,
        "tool": tool,
        "class": class.as_str(),
        "decision": decision.as_str(),
        "allowed": allowed,
        "pane": pane,
        "details": details,
    })
    .to_string()
}

/// Append `line` to `path`, creating the parent directory with owner-only
/// permissions. A failure is reported to the caller; callers log and continue,
/// because a broken audit file must not break the terminal.
pub fn append_audit(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Record a decision when auditing is enabled. Never fails the caller.
pub fn audit(
    settings: &Value,
    source: &str,
    tool: &str,
    class: ActionClass,
    decision: Decision,
    allowed: bool,
    pane: Option<&str>,
    details: &str,
) {
    if settings
        .get("agent_audit")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        let line = audit_line(
            &timestamp(),
            source,
            tool,
            class,
            decision,
            allowed,
            pane,
            details,
        );
        let _ = append_audit(&audit_file(), &line);
    }
}

/// A snapshot of the last command, used to build a fix prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixContext {
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub output: String,
    pub shell: Option<String>,
}

impl FixContext {
    pub fn is_empty(&self) -> bool {
        self.command.trim().is_empty()
    }
}

/// The prompt sent to the agent by `oxterm fix` (and the `/fix` command).
///
/// It asks for an explanation and a minimal fix, and it states that the agent
/// must not run anything: suggestions are for the user to approve, matching the
/// confirmation policy used by the control API.
pub fn fix_prompt(context: &FixContext, hint: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(
        "A command failed in my terminal. Explain what went wrong and propose the smallest fix.\n",
    );
    out.push_str(
        "Do not run commands yourself: show the commands to run and I will approve them.\n\n",
    );
    if !context.command.trim().is_empty() {
        out.push_str("Command:\n```\n");
        out.push_str(context.command.trim_end());
        out.push_str("\n```\n\n");
    }
    if !context.cwd.trim().is_empty() {
        out.push_str(&format!("Working directory: `{}`\n\n", context.cwd));
    }
    match context.exit_code {
        Some(code) => out.push_str(&format!("Exit status: {code}\n")),
        None => out.push_str("Exit status: not reported by the shell\n"),
    }
    if let Some(ms) = context.duration_ms {
        out.push_str(&format!("Duration: {} ms\n", ms));
    }
    if let Some(shell) = context.shell.as_ref().filter(|shell| !shell.is_empty()) {
        out.push_str(&format!("Shell: {}\n", shell));
    }
    if let Some(hint) = hint.map(str::trim).filter(|hint| !hint.is_empty()) {
        out.push_str(&format!(
            "\nHint from me: {}\n",
            truncate_head(hint, MAX_HINT_CHARS)
        ));
    }
    let output = context.output.trim_end();
    if !output.is_empty() {
        out.push_str("\nRecent terminal output (tail):\n```\n");
        out.push_str(&truncate_tail(output, MAX_OUTPUT_CHARS));
        out.push_str("\n```\n");
    }
    out
}

/// Keep the first `max` characters, marking what was dropped.
pub fn truncate_head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n… (truncated)");
    out
}

/// Keep the last `max` characters, marking what was dropped.
pub fn truncate_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let skipped = count - max;
    let mut out = format!("… ({skipped} earlier characters omitted)\n");
    out.extend(text.chars().skip(skipped));
    out
}

/// Split a configured agent command into an argv, without a shell.
pub fn command_argv(raw: &str) -> Result<Vec<String>, String> {
    let argv =
        shell_words::split(raw).map_err(|error| format!("invalid agent command: {error}"))?;
    if argv.is_empty() {
        return Err("agent_command is empty".to_string());
    }
    if argv[0].trim().is_empty() {
        return Err("agent_command has no program".to_string());
    }
    Ok(argv)
}

/// An ACP-capable agent CLI Oxterm knows how to suggest. Oxterm never installs
/// or starts one of these on its own: `oxterm agent --list` only reports which
/// programs the user already has, and the user copies the command into
/// `agent_command`.
pub struct KnownAgent {
    pub program: &'static str,
    pub label: &'static str,
    /// Command line to put in `agent_command`, when the CLI's ACP mode is known.
    pub command: &'static str,
}

impl KnownAgent {
    /// The suggested command with the discovered binary path substituted, so a
    /// GUI started from the desktop does not depend on its own `PATH`.
    pub fn command_with_path(&self, path: &Path) -> String {
        let mut argv = command_argv(self.command).unwrap_or_default();
        if argv.is_empty() {
            return path.to_string_lossy().to_string();
        }
        argv[0] = path.to_string_lossy().to_string();
        shell_words::join(&argv)
    }
}

pub const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        program: "opencode",
        label: "OpenCode",
        command: "opencode acp",
    },
    KnownAgent {
        program: "claude-code-acp",
        label: "Claude Code (ACP adapter)",
        command: "claude-code-acp",
    },
    KnownAgent {
        program: "agy",
        label: "Antigravity (agy)",
        command: "agy acp",
    },
    KnownAgent {
        program: "gemini",
        label: "Gemini CLI",
        command: "gemini --experimental-acp",
    },
    KnownAgent {
        program: "copilot",
        label: "GitHub Copilot CLI",
        command: "copilot --acp --stdio",
    },
    KnownAgent {
        program: "codex-acp",
        label: "Codex (ACP adapter)",
        command: "codex-acp",
    },
];

/// Absolute path of `program` on `path_var`, if it is an executable file.
pub fn find_in_path(path_var: &str, program: &str) -> Option<std::path::PathBuf> {
    for directory in path_var.split(':').filter(|entry| !entry.is_empty()) {
        let candidate = Path::new(directory).join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Directories searched after `PATH`.
///
/// A terminal started from the desktop menu inherits a much smaller `PATH` than
/// an interactive shell: per-user tool directories are often missing. Looking
/// in the usual install locations keeps `opencode acp` working without asking
/// the user to hard-code a path.
pub fn fallback_dirs() -> Vec<std::path::PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for relative in [
            ".opencode/bin",
            ".local/bin",
            ".bun/bin",
            ".cargo/bin",
            ".npm-global/bin",
        ] {
            directories.push(home.join(relative));
        }
    }
    directories.push(std::path::PathBuf::from("/usr/local/bin"));
    directories.push(std::path::PathBuf::from("/opt/homebrew/bin"));
    directories
}

fn find_in_dirs(directories: &[std::path::PathBuf], program: &str) -> Option<std::path::PathBuf> {
    directories
        .iter()
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// Testable form of the lookup: `PATH` first, then `extra`.
pub fn find_in_path_with(
    path_var: &str,
    extra: &[std::path::PathBuf],
    program: &str,
) -> Option<std::path::PathBuf> {
    find_in_path(path_var, program).or_else(|| find_in_dirs(extra, program))
}

/// `PATH`, then [`fallback_dirs`].
pub fn find_program(program: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    find_in_path_with(&path_var, &fallback_dirs(), program)
}

/// Absolute path for `program`, or the name unchanged when it cannot be found.
/// `PATH` and the common per-user bin directories are both searched.
pub fn resolve_program(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    find_program(program)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| program.to_string())
}

/// Known agent CLIs installed on this machine, in the order of [`KNOWN_AGENTS`].
pub fn detected_agents() -> Vec<(&'static KnownAgent, std::path::PathBuf)> {
    KNOWN_AGENTS
        .iter()
        .filter_map(|agent| find_program(agent.program).map(|path| (agent, path)))
        .collect()
}

/// Short message for the command line and the pane when the switch is off.
pub fn disabled_message() -> &'static str {
    "Agent features are disabled. Set \"agent_enabled\": true and \"agent_command\" in \
     ~/.config/oxterm/settings.json; `oxterm agent --list` shows the agent CLIs \
     already installed."
}

/// Instructions advertised to MCP clients when agent features are disabled, so
/// an agent that finds `oxterm mcp` sees why it has no tools.
pub fn mcp_disabled_message() -> &'static str {
    "Oxterm's agent integration is disabled. The user must set \"agent_enabled\": true in \
     ~/.config/oxterm/settings.json, configure agent_command and choose the permission \
     policies before this server exposes any tool."
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`, implemented without a dependency
/// so the headless core stays free of platform crates.
pub fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_timestamp(seconds)
}

/// Convert Unix seconds to the audit timestamp format.
pub fn format_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        remainder / 3600,
        (remainder % 3600) / 60,
        remainder % 60
    )
}

/// Howard Hinnant's days-from-civil inverse, valid for the whole Unix range.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, Value)]) -> Value {
        let mut map = serde_json::Map::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        Value::Object(map)
    }

    #[test]
    fn defaults_are_safe_and_off() {
        let empty = json!({});
        assert!(!enabled(&empty));
        assert!(!configured(&empty));
        let policies = Policies::from_settings(&empty);
        assert_eq!(policies.decide(ActionClass::Read), Decision::Allow);
        assert_eq!(policies.decide(ActionClass::Create), Decision::Ask);
        assert_eq!(policies.decide(ActionClass::Input), Decision::Ask);
    }

    #[test]
    fn configuration_requires_the_switch_and_a_command() {
        let on = settings(&[("agent_enabled", json!(true))]);
        assert!(enabled(&on));
        assert!(!configured(&on));
        let both = settings(&[
            ("agent_enabled", json!(true)),
            ("agent_command", json!("opencode acp")),
        ]);
        assert!(configured(&both));
        // A command without the switch is still disabled.
        let command_only = settings(&[("agent_command", json!("opencode acp"))]);
        assert!(!configured(&command_only));
        assert!(!mcp_available(&command_only));
    }

    #[test]
    fn a_leading_question_mark_is_a_prompt_for_the_agent() {
        assert_eq!(
            prompt_from_line("? analizza il mio ~/.bashrc"),
            Some("analizza il mio ~/.bashrc")
        );
        assert_eq!(
            prompt_from_line("  ?perche' e' lento?  "),
            Some("perche' e' lento?")
        );
        // A bare `?` only asks for the pane, it has nothing to send.
        assert_eq!(prompt_from_line("?"), Some(""));
        assert_eq!(prompt_from_line("?   "), Some(""));
        // Everything else belongs to the shell, including a `?` that is not
        // the first character or is inside quotes.
        assert_eq!(prompt_from_line("ls -la"), None);
        assert_eq!(prompt_from_line("echo '?x'"), None);
        assert_eq!(prompt_from_line(""), None);
    }

    #[test]
    fn a_question_prefix_needs_the_agent_and_the_preference() {
        let ready = settings(&[
            ("agent_enabled", json!(true)),
            ("agent_command", json!("opencode acp")),
        ]);
        assert!(question_prefix_enabled(&ready));

        let off = settings(&[
            ("agent_enabled", json!(true)),
            ("agent_command", json!("opencode acp")),
            ("agent_question_prefix", json!(false)),
        ]);
        assert!(!question_prefix_enabled(&off));

        // The master switch still wins, while an enabled agent without a
        // command keeps the prefix so the pane can offer its setup.
        let disabled = settings(&[
            ("agent_enabled", json!(false)),
            ("agent_command", json!("opencode acp")),
        ]);
        assert!(!question_prefix_enabled(&disabled));
        let no_command = settings(&[("agent_enabled", json!(true))]);
        assert!(question_prefix_enabled(&no_command));
    }

    #[test]
    fn unknown_policies_fall_back_to_the_strict_default() {
        let value = settings(&[
            ("agent_policy_read", json!("nonsense")),
            ("agent_policy_create", json!("deny")),
            ("agent_policy_input", json!("auto")),
        ]);
        let policies = Policies::from_settings(&value);
        assert_eq!(policies.read, Policy::Auto);
        assert_eq!(policies.create, Policy::Deny);
        assert_eq!(policies.input, Policy::Auto);
        assert_eq!(policies.decide(ActionClass::Create), Decision::Deny);
    }

    #[test]
    fn tools_are_classified_by_what_they_can_change() {
        assert_eq!(
            tool_class("terminal_list", &json!({})),
            Some(ActionClass::Read)
        );
        assert_eq!(
            tool_class("terminal_capture", &json!({})),
            Some(ActionClass::Read)
        );
        assert_eq!(
            tool_class("terminal_new", &json!({"cwd": "/tmp"})),
            Some(ActionClass::Create)
        );
        assert_eq!(
            tool_class("terminal_new", &json!({"command": ["make"]})),
            Some(ActionClass::Input)
        );
        assert_eq!(
            tool_class("terminal_split", &json!({"command": []})),
            Some(ActionClass::Create)
        );
        assert_eq!(
            tool_class("terminal_send", &json!({"text": "rm -rf /"})),
            Some(ActionClass::Input)
        );
        assert_eq!(
            tool_class("terminal_workspace", &json!({"file": "w.json"})),
            Some(ActionClass::Input)
        );
        assert_eq!(tool_class("nope", &json!({})), None);
    }

    #[test]
    fn a_fix_prompt_carries_the_failure_and_the_hint() {
        let context = FixContext {
            command: "cargo build".to_string(),
            cwd: "/tmp/project".to_string(),
            exit_code: Some(101),
            duration_ms: Some(1234),
            output: "error[E0425]: cannot find value `x`".to_string(),
            shell: Some("/bin/bash".to_string()),
        };
        let prompt = fix_prompt(&context, Some("the import looks wrong"));
        assert!(prompt.contains("cargo build"));
        assert!(prompt.contains("Exit status: 101"));
        assert!(prompt.contains("E0425"));
        assert!(prompt.contains("the import looks wrong"));
        assert!(prompt.contains("Do not run commands"));
    }

    #[test]
    fn long_output_keeps_the_tail() {
        let output: String = std::iter::repeat('x').take(MAX_OUTPUT_CHARS + 50).collect();
        let context = FixContext {
            command: "cmd".to_string(),
            output: format!("HEAD{}TAIL", output),
            ..FixContext::default()
        };
        let prompt = fix_prompt(&context, None);
        assert!(!prompt.contains("HEAD"));
        assert!(prompt.contains("TAIL"));
        assert!(prompt.contains("earlier characters omitted"));
    }

    #[test]
    fn audit_lines_never_include_typed_text() {
        let line = audit_line(
            "2026-01-02T03:04:05Z",
            "mcp",
            "terminal_send",
            ActionClass::Input,
            Decision::Ask,
            true,
            Some("pane-1"),
            &audit_details("terminal_send", &json!({"text": "hunter2"})),
        );
        assert!(!line.contains("hunter2"));
        assert!(line.contains("\"class\":\"input\""));
        assert!(line.contains("\"allowed\":true"));
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_767_312_245), "2026-01-02T00:04:05Z");
    }

    #[test]
    fn audit_records_are_appended_to_a_private_file() {
        let directory = std::env::temp_dir().join(format!(
            "oxterm-agent-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("nested").join("agent-audit.log");
        append_audit(&path, "{\"one\":1}").unwrap();
        append_audit(&path, "{\"two\":2}").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\"one\":1}\n{\"two\":2}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn known_agents_are_well_formed_and_detected_on_a_path() {
        for agent in KNOWN_AGENTS {
            assert!(!agent.program.is_empty());
            assert!(
                command_argv(agent.command).is_ok(),
                "{} has an invalid command",
                agent.program
            );
        }
        let directory = std::env::temp_dir().join(format!(
            "oxterm-agent-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let program = directory.join("opencode");
        std::fs::write(&program, b"#!/bin/sh\n").unwrap();
        let path_var = format!("{}:/nonexistent", directory.display());
        assert_eq!(find_in_path(&path_var, "opencode"), Some(program.clone()));
        assert_eq!(find_in_path(&path_var, "definitely-not-here"), None);
        assert_eq!(find_in_path("", "opencode"), None);

        // Fallback directories cover the minimal PATH of a desktop launcher.
        let empty_path = "";
        assert_eq!(
            find_in_path(empty_path, "opencode"),
            None,
            "the temporary program is not in the fallback directories"
        );
        let extra = vec![directory.clone()];
        assert_eq!(
            find_in_path_with(empty_path, &extra, "opencode"),
            Some(program.clone())
        );
        assert_eq!(
            KNOWN_AGENTS[0].command_with_path(&program),
            format!("{} acp", program.display())
        );
        assert_eq!(resolve_program("/already/absolute"), "/already/absolute");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn agent_commands_are_split_without_a_shell() {
        assert_eq!(
            command_argv("opencode acp --foo bar").unwrap(),
            vec!["opencode", "acp", "--foo", "bar"]
        );
        assert_eq!(
            command_argv("claude-code-acp").unwrap(),
            vec!["claude-code-acp"]
        );
        assert!(command_argv("   ").is_err());
        assert!(command_argv("\"unterminated").is_err());
    }
}
