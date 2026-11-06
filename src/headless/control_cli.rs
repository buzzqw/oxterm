//! Parsing and help text for the scriptable control commands
//! (`oxterm new|split|send|capture|wait|connect|connections|tunnel|events|action`).
//!
//! The execution side lives in `control.rs` and needs GTK; this half does not,
//! so every verb, option and error message is unit tested without a window.

use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum CaptureScope {
    Screen,
    Lines(usize),
    All,
    /// Lines collected by `capture` trigger actions.
    Captured {
        clear: bool,
    },
}

#[derive(Debug, PartialEq)]
pub enum CliMode {
    Help(&'static str),
    New {
        cwd: Option<String>,
        title: Option<String>,
        command: Vec<String>,
    },
    Split {
        pane: Option<String>,
        vertical: bool,
        cwd: Option<String>,
        title: Option<String>,
        command: Vec<String>,
    },
    Send {
        pane: Option<String>,
        enter: bool,
        /// `None` means: read the text from standard input (`send ... -`).
        text: Option<String>,
    },
    Capture {
        pane: Option<String>,
        scope: CaptureScope,
    },
    Wait {
        pane: Option<String>,
        timeout: Option<Duration>,
    },
    Connect {
        name: String,
        split: bool,
        pane: Option<String>,
        yes: bool,
    },
    Connections {
        json: bool,
    },
    ConnectionsExport,
    ConnectionsImport {
        file: String,
        allow_local: bool,
    },
    Tunnel {
        action: TunnelAction,
        name: Option<String>,
        index: Option<usize>,
    },
    Events {
        pane: Option<String>,
        events: Vec<String>,
        output: bool,
        json: bool,
    },
    Action {
        pane: Option<String>,
        command: ActionCommand,
    },
    Workspace {
        file: String,
        new_window: bool,
        replace: bool,
    },
    Mcp,
    Browse {
        name: String,
    },
    /// Diagnose the last failed command with the configured agent.
    Fix {
        pane: Option<String>,
        print_only: bool,
        hint: Option<String>,
    },
    /// Open the agent pane, optionally with a prompt.
    Agent {
        pane: Option<String>,
        action: AgentAction,
    },
}

#[derive(Debug, PartialEq)]
pub enum AgentAction {
    /// Report the agent CLIs found on `PATH`.
    List,
    /// Print the effective agent configuration without secrets.
    Show,
    /// Show the pane and send this prompt, when there is one.
    Prompt(Option<String>),
}

/// The actions `oxterm action` can ask a running window to perform.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ActionCommand {
    SetTabColor {
        color: String,
    },
    SetTabTitle {
        title: String,
    },
    Notify {
        title: String,
        body: String,
    },
    OpenUrl {
        url: String,
    },
    /// `None` means: read the text from standard input (`insert-text ... -`).
    InsertText {
        text: Option<String>,
    },
    MarkFailure,
    ClearFailure,
    ClearTabColor,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TunnelAction {
    Start,
    Stop,
    List,
}

pub struct Verb {
    pub name: &'static str,
    pub synopsis: &'static str,
    pub summary: &'static str,
    pub help: &'static str,
}

pub const VERBS: &[Verb] = &[
    Verb {
        name: "new",
        synopsis: "new [DIR] [--cwd DIR] [--title TITLE] [-- CMD...]",
        summary: "Open a tab, print its pane id",
        help: "Usage: oxterm new [DIR] [--cwd DIR] [--title TITLE] [-- COMMAND...]\n\n\
               Open a new tab in the running Oxterm and print its pane id. DIR is a\n\
               shorthand for --cwd.\n",
    },
    Verb {
        name: "split",
        synopsis:
            "split [--pane ID] [--vertical|--horizontal] [--cwd DIR] [--title TITLE] [-- CMD...]",
        summary: "Split a single-pane window",
        help: "Usage: oxterm split [--pane ID] [--vertical|--horizontal] [--cwd DIR]\n\
               \x20            [--title TITLE] [-- COMMAND...]\n\n\
               Split the target pane's window (which must be unsplit) and print the\n\
               new pane id. Defaults to the pane in $OXTERM_PANE and to a vertical split.\n",
    },
    Verb {
        name: "send",
        synopsis: "send [--pane ID] [--enter] TEXT...|-",
        summary: "Type into a pane",
        help: "Usage: oxterm send [--pane ID] [--enter] TEXT...\n\
               \x20      oxterm send [--pane ID] [--enter] -\n\n\
               Type TEXT into the target pane, or standard input with '-'. --enter\n\
               appends a carriage return, as if Enter had been pressed.\n",
    },
    Verb {
        name: "capture",
        synopsis: "capture [--pane ID] [--scrollback N|--all|--captured [--clear]]",
        summary: "Print a pane's text",
        help: "Usage: oxterm capture [--pane ID]\n\
               \x20            [--scrollback N | --all | --captured [--clear]]\n\n\
               Print the target pane's text: the visible screen by default, N lines\n\
               of history with --scrollback, or the whole buffer with --all.\n\
               --captured prints the lines collected by triggers, and --clear\n\
               empties that list.\n",
    },
    Verb {
        name: "wait",
        synopsis: "wait [--pane ID] [--timeout SECONDS]",
        summary: "Wait for the next command to finish",
        help: "Usage: oxterm wait [--pane ID] [--timeout SECONDS]\n\n\
               Wait for the next command in the pane to finish and exit with its\n\
               status. Without a timeout it waits indefinitely. Exits 1 with an\n\
               error when the shell reports no status for the command.\n",
    },
    Verb {
        name: "connect",
        synopsis: "connect NAME [--split [--pane ID]] [--yes]",
        summary: "Open a saved connection or host",
        help: "Usage: oxterm connect NAME [--split [--pane ID]] [--yes]\n\n\
               Open a saved connection, or a host from ~/.ssh/config, in the running\n\
               Oxterm and print its pane id. --split opens it in a split of the target\n\
               pane's window. An entry marked 'Ask before connecting' needs --yes here.\n",
    },
    Verb {
        name: "connections",
        synopsis: "connections [--json|export|import FILE [--allow-local-commands]]",
        summary: "List, export or import saved connections",
        help: "Usage: oxterm connections [--json]\n\
               \x20      oxterm connections export\n\
               \x20      oxterm connections import FILE [--allow-local-commands]\n\n\
               List saved connections and the hosts from ~/.ssh/config, print the book\n\
               as JSON, or add the entries of an exported file. None of these opens a\n\
               window or needs a running Oxterm, and the book holds no secrets.\n\n\
               An import never replaces an entry and always gets new ids. Entries that\n\
               would act by themselves (ProxyCommand, a pre-connect command, automation\n\
               rules, a log folder) are skipped unless --allow-local-commands is given.\n",
    },
    Verb {
        name: "tunnel",
        synopsis: "tunnel list|start|stop [NAME [INDEX]]",
        summary: "Control a connection's saved tunnels",
        help: "Usage: oxterm tunnel list [NAME]\n\
               \x20      oxterm tunnel start|stop NAME [INDEX]\n\n\
               Show or change the saved tunnels of a connection in the running Oxterm.\n\
               INDEX is the number 'tunnel list' prints; without it the command\n\
               applies to all of that connection's tunnels.\n",
    },
    Verb {
        name: "browse",
        synopsis: "browse NAME",
        summary: "Open the file browser for a connection or host",
        help: "Usage: oxterm browse NAME\n\n\
               Open the SFTP/FTP file browser for a saved connection, a host from\n\
               ~/.ssh/config, or a [user@]host[:port] destination. The browser window\n\
               belongs to the running Oxterm and needs sftp or lftp.\n",
    },
    Verb {
        name: "events",
        synopsis: "events [--pane ID] [--event NAME]... [--output] [--json]",
        summary: "Subscribe to a pane's event stream",
        help: "Usage: oxterm events [--pane ID] [--event NAME]... [--output] [--json]\n\n\
               Subscribe to a running Oxterm and print one JSON object per line until\n\
               the stream is closed. Without --event every event is delivered; repeat\n\
               --event to choose. --output also enables output_line, which can contain\n\
               secrets and is off by default. --json is accepted for symmetry.\n",
    },
    Verb {
        name: "action",
        synopsis: "action set-tab-color|set-tab-title|notify|open-url|...",
        summary: "Ask a running Oxterm to perform an action",
        help: "Usage: oxterm action set-tab-color [--pane ID] COLOR\n\
               \x20      oxterm action set-tab-title [--pane ID] TITLE\n\
               \x20      oxterm action notify [--title TITLE] [--body BODY]\n\
               \x20      oxterm action open-url URL\n\
               \x20      oxterm action insert-text [--pane ID] TEXT...|-\n\
               \x20      oxterm action mark-failure|clear-failure|clear-tab-color [--pane ID]\n\n\
               Ask the running Oxterm to change a tab, show a notification, open an\n\
               http(s) URL, type without pressing Enter, or set and clear the failure\n\
               mark. The target pane comes from --pane, then $OXTERM_PANE, then the\n\
               only open pane.\n",
    },
    Verb {
        name: "workspace",
        synopsis: "workspace [--new-window] [--replace] FILE",
        summary: "Open tabs, splits and commands from a JSON file",
        help: "Usage: oxterm workspace [--new-window] [--replace] FILE\n\n\
               Read a workspace file and open its tabs, splits and commands in a\n\
               running Oxterm. --new-window starts a fresh window first (and implies\n\
               --replace); --replace closes the window's existing tabs after the new\n\
               ones are open. Without a running window a new one is started.\n",
    },
    Verb {
        name: "mcp",
        synopsis: "mcp",
        summary: "Serve the Model Context Protocol on stdin/stdout",
        help: "Usage: oxterm mcp\n\n\
               Speak MCP over stdin and stdout, one JSON object per line. The tools\n\
               create tabs, split panes, type, capture output, wait for a command and\n\
               open workspaces in a running Oxterm. Nothing is printed except protocol\n\
               messages; diagnostics go to standard error.\n\n\
               Agent features are opt-in: when agent_enabled is false the server\n\
               exposes no tools, and every call is recorded in the audit log.\n",
    },
    Verb {
        name: "fix",
        synopsis: "fix [--pane ID] [--print] [HINT...]",
        summary: "Ask the agent to diagnose the last failed command",
        help: "Usage: oxterm fix [--pane ID] [--print] [HINT...]\n\n\
               Collect the last command reported by the shell integration (command,\n\
               directory, exit status and recent output) and send it to the agent\n\
               pane as a fix request. HINT steers the agent, for example\n\
               'the include path looks wrong'. --print prints the prompt instead of\n\
               sending it, so it can be reviewed or piped to another tool.\n\n\
               Needs agent_enabled and agent_command in the settings; nothing is\n\
               sent without them.\n",
    },
    Verb {
        name: "agent",
        synopsis: "agent [--pane ID] [--list|--show] [PROMPT...]",
        summary: "Open the agent pane or send it a prompt",
        help: "Usage: oxterm agent [--pane ID] [PROMPT...]\n\
               \x20      oxterm agent --list\n\
               \x20      oxterm agent --show\n\n\
               With PROMPT, show the agent pane for the target pane and send the\n\
               prompt; without it, just toggle the pane. --list reports the ACP\n\
               agent CLIs already installed on PATH and the command to configure;\n\
               Oxterm never installs one. --show prints the effective agent settings\n\
               and never a secret. The target pane comes from --pane, then\n\
               $OXTERM_PANE, then the only open pane.\n",
    },
];

pub fn verb(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|verb| verb.name == name)
}

/// The indented `new`/`split`/... block used by `--help`.
pub fn summary() -> String {
    let width = VERBS
        .iter()
        .map(|verb| verb.synopsis.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for verb in VERBS {
        out.push_str(&format!("  {:<width$}  {}\n", verb.synopsis, verb.summary));
    }
    out
}

pub fn parse_cli_mode(args: &[String]) -> Option<Result<CliMode, String>> {
    let name = args.get(1)?.as_str();
    if !VERBS.iter().any(|entry| entry.name == name) {
        return None;
    }
    Some(parse_verb(&args[1..]))
}

fn parse_connect(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut name = None;
    let mut split = false;
    let mut pane = None;
    let mut yes = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--split" => split = true,
            "--yes" | "-y" => yes = true,
            "--pane" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option: {}", option));
            }
            value if name.is_none() => name = Some(value.to_string()),
            value => return Err(format!("unexpected argument: {}", value)),
        }
        i += 1;
    }
    if pane.is_some() && !split {
        return Err("--pane only applies with --split".to_string());
    }
    Ok(CliMode::Connect {
        name: name.ok_or("connect requires a connection name")?,
        split,
        pane,
        yes,
    })
}

fn parse_connections(args: &[String], help: &'static str) -> Result<CliMode, String> {
    match args.first().map(String::as_str) {
        Some("export") => {
            if let Some(extra) = args.get(1) {
                return Err(format!("unexpected argument: {}", extra));
            }
            return Ok(CliMode::ConnectionsExport);
        }
        Some("import") => {
            let mut file = None;
            let mut allow_local = false;
            for arg in &args[1..] {
                match arg.as_str() {
                    "-h" | "--help" => return Ok(CliMode::Help(help)),
                    "--allow-local-commands" => allow_local = true,
                    option if option.starts_with('-') => {
                        return Err(format!("unknown option: {}", option));
                    }
                    value if file.is_none() => file = Some(value.to_string()),
                    value => return Err(format!("unexpected argument: {}", value)),
                }
            }
            return Ok(CliMode::ConnectionsImport {
                file: file.ok_or("connections import requires a file")?,
                allow_local,
            });
        }
        _ => {}
    }
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--json" => json = true,
            other => return Err(format!("unknown option: {}", other)),
        }
    }
    Ok(CliMode::Connections { json })
}

fn parse_tunnel(args: &[String], help: &'static str) -> Result<CliMode, String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(CliMode::Help(help));
    }
    let action = match args.first().map(String::as_str) {
        Some("start") => TunnelAction::Start,
        Some("stop") => TunnelAction::Stop,
        Some("list") | None => TunnelAction::List,
        Some(other) => return Err(format!("unknown tunnel action: {}", other)),
    };
    let name = args.get(1).cloned();
    if action != TunnelAction::List && name.is_none() {
        return Err("tunnel start and stop require a connection name".to_string());
    }
    let index = match args.get(2) {
        Some(value) => Some(
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid tunnel index: {}", value))?,
        ),
        None => None,
    };
    if args.len() > 3 {
        return Err(format!("unexpected argument: {}", args[3]));
    }
    Ok(CliMode::Tunnel {
        action,
        name,
        index,
    })
}

fn parse_browse(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut name = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {}", value));
            }
            value if name.is_none() => name = Some(value.to_string()),
            value => return Err(format!("unexpected argument: {}", value)),
        }
    }
    Ok(CliMode::Browse {
        name: name.ok_or("browse requires a connection name")?,
    })
}

fn parse_fix(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut pane = None;
    let mut print_only = false;
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--pane" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            "--print" => print_only = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {}", value));
            }
            value => words.push(value.to_string()),
        }
        i += 1;
    }
    let hint = words.join(" ").trim().to_string();
    Ok(CliMode::Fix {
        pane,
        print_only,
        hint: (!hint.is_empty()).then_some(hint),
    })
}

fn parse_agent(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut pane = None;
    let mut list = false;
    let mut show = false;
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--pane" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            "--list" => list = true,
            "--show" => show = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {}", value));
            }
            value => words.push(value.to_string()),
        }
        i += 1;
    }
    if list && show {
        return Err("--list and --show cannot be combined".to_string());
    }
    if (list || show) && !words.is_empty() {
        return Err("--list and --show do not take a prompt".to_string());
    }
    let action = if list {
        AgentAction::List
    } else if show {
        AgentAction::Show
    } else {
        let prompt = words.join(" ").trim().to_string();
        AgentAction::Prompt((!prompt.is_empty()).then_some(prompt))
    };
    Ok(CliMode::Agent { pane, action })
}

fn parse_events(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut pane = None;
    let mut events = Vec::new();
    let mut output = false;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--pane" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            "--event" => {
                i += 1;
                let name = args.get(i).ok_or("--event requires a name")?;
                if !super::events::EVENT_NAMES.contains(&name.as_str()) {
                    return Err(format!(
                        "unknown event: {} (see oxterm events --help)",
                        name
                    ));
                }
                events.push(name.clone());
            }
            "--output" => output = true,
            "--json" => json = true,
            other => return Err(format!("unknown option: {}", other)),
        }
        i += 1;
    }
    Ok(CliMode::Events {
        pane,
        events,
        output,
        json,
    })
}

/// Split `--pane ID` from positional values for the simple action verbs.
fn pane_and_values(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut pane = None;
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pane" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            value if value.starts_with('-') && value != "-" => {
                return Err(format!("unknown option: {}", value));
            }
            value => values.push(value.to_string()),
        }
        i += 1;
    }
    Ok((pane, values))
}

fn parse_workspace(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let mut file: Option<String> = None;
    let mut new_window = false;
    let mut replace = false;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(help)),
            "--new-window" => new_window = true,
            "--replace" => replace = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {}", value));
            }
            value if file.is_none() => file = Some(value.to_string()),
            value => return Err(format!("unexpected argument: {}", value)),
        }
    }
    let file = file.ok_or("workspace requires a file")?;
    Ok(CliMode::Workspace {
        file,
        new_window,
        // A fresh window has nothing worth keeping.
        replace: replace || new_window,
    })
}

fn parse_action(args: &[String], help: &'static str) -> Result<CliMode, String> {
    let Some(name) = args.first().map(String::as_str) else {
        return Err("action requires a subcommand (set-tab-color, clear-tab-color, set-tab-title, notify, open-url, insert-text, mark-failure, clear-failure)".to_string());
    };
    if name == "-h" || name == "--help" {
        return Ok(CliMode::Help(help));
    }
    let rest = &args[1..];
    match name {
        "set-tab-color" => {
            let (pane, values) = pane_and_values(rest)?;
            let [color] = values.as_slice() else {
                return Err("set-tab-color requires exactly one colour".to_string());
            };
            let color = crate::headless::triggers::parse_color(color)?;
            Ok(CliMode::Action {
                pane,
                command: ActionCommand::SetTabColor { color },
            })
        }
        "set-tab-title" => {
            let (pane, values) = pane_and_values(rest)?;
            if values.is_empty() {
                return Err("set-tab-title requires a title".to_string());
            }
            Ok(CliMode::Action {
                pane,
                command: ActionCommand::SetTabTitle {
                    title: values.join(" "),
                },
            })
        }
        "notify" => {
            let mut title = "Oxterm".to_string();
            let mut body = String::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "-h" | "--help" => return Ok(CliMode::Help(help)),
                    "--title" => {
                        i += 1;
                        title = rest.get(i).ok_or("--title requires a value")?.clone();
                    }
                    "--body" => {
                        i += 1;
                        body = rest.get(i).ok_or("--body requires a value")?.clone();
                    }
                    other => return Err(format!("unknown option: {}", other)),
                }
                i += 1;
            }
            Ok(CliMode::Action {
                pane: None,
                command: ActionCommand::Notify { title, body },
            })
        }
        "open-url" => {
            let (pane, values) = pane_and_values(rest)?;
            let [url] = values.as_slice() else {
                return Err("open-url requires exactly one URL".to_string());
            };
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("open-url only opens http and https URLs".to_string());
            }
            Ok(CliMode::Action {
                pane,
                command: ActionCommand::OpenUrl { url: url.clone() },
            })
        }
        "insert-text" => {
            let (pane, values) = pane_and_values(rest)?;
            let text = if values.as_slice() == ["-"] {
                None
            } else if values.is_empty() {
                return Err("insert-text requires text or -".to_string());
            } else {
                Some(values.join(" "))
            };
            Ok(CliMode::Action {
                pane,
                command: ActionCommand::InsertText { text },
            })
        }
        "mark-failure" | "clear-failure" | "clear-tab-color" => {
            let (pane, values) = pane_and_values(rest)?;
            if !values.is_empty() {
                return Err(format!("unexpected argument: {}", values[0]));
            }
            Ok(CliMode::Action {
                pane,
                command: match name {
                    "mark-failure" => ActionCommand::MarkFailure,
                    "clear-failure" => ActionCommand::ClearFailure,
                    _ => ActionCommand::ClearTabColor,
                },
            })
        }
        other => Err(format!("unknown action: {}", other)),
    }
}

pub fn parse_verb(args: &[String]) -> Result<CliMode, String> {
    let name = args.first().map(String::as_str).unwrap_or("");
    let Some(entry) = verb(name) else {
        return Err(format!("unknown control command: {name}"));
    };
    match name {
        "connect" => return parse_connect(&args[1..], entry.help),
        "connections" => return parse_connections(&args[1..], entry.help),
        "tunnel" => return parse_tunnel(&args[1..], entry.help),
        "events" => return parse_events(&args[1..], entry.help),
        "browse" => return parse_browse(&args[1..], entry.help),
        "fix" => return parse_fix(&args[1..], entry.help),
        "agent" => return parse_agent(&args[1..], entry.help),
        "action" => return parse_action(&args[1..], entry.help),
        "workspace" => return parse_workspace(&args[1..], entry.help),
        "mcp" => {
            if let Some(extra) = args.get(1) {
                if extra == "-h" || extra == "--help" {
                    return Ok(CliMode::Help(entry.help));
                }
                return Err(format!("unexpected argument: {}", extra));
            }
            return Ok(CliMode::Mcp);
        }
        _ => {}
    }
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    let mut pane: Option<String> = None;
    let mut vertical = true;
    let mut enter = false;
    let mut scope = CaptureScope::Screen;
    let mut timeout: Option<Duration> = None;
    let mut command: Vec<String> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    let mut read_stdin = false;
    let mut captured = false;
    let mut clear = false;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliMode::Help(entry.help)),
            "--cwd" if name == "new" || name == "split" => {
                i += 1;
                let value = args.get(i).ok_or("--cwd requires a directory")?;
                cwd = Some(expand_dir(value)?);
            }
            "--title" if name == "new" || name == "split" => {
                i += 1;
                title = Some(args.get(i).ok_or("--title requires a value")?.to_string());
            }
            "--pane" if name != "new" => {
                i += 1;
                pane = Some(args.get(i).ok_or("--pane requires a session id")?.clone());
            }
            "--vertical" if name == "split" => vertical = true,
            "--horizontal" if name == "split" => vertical = false,
            "--enter" if name == "send" => enter = true,
            "--all" if name == "capture" => scope = CaptureScope::All,
            "--captured" if name == "capture" => captured = true,
            "--clear" if name == "capture" => clear = true,
            "--scrollback" if name == "capture" => {
                i += 1;
                let value = args.get(i).ok_or("--scrollback requires a line count")?;
                let lines = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --scrollback '{}'", value))?;
                scope = CaptureScope::Lines(lines);
            }
            "--timeout" if name == "wait" => {
                i += 1;
                let value = args.get(i).ok_or("--timeout requires seconds")?;
                let secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout '{}': expected seconds", value))?;
                timeout = Some(Duration::from_secs(secs));
            }
            "--" if name == "new" || name == "split" => {
                command = args[i + 1..].to_vec();
                if command.is_empty() {
                    return Err("-- must be followed by a command".to_string());
                }
                break;
            }
            "--" if name == "send" => {
                text_parts = args[i + 1..].to_vec();
                break;
            }
            _ if name == "send" => {
                if arg == "-" {
                    read_stdin = true;
                } else if arg.starts_with('-') {
                    return Err(format!("unknown option: {}", arg));
                } else {
                    text_parts.push(arg.clone());
                }
            }
            _ if name == "new" && !arg.starts_with('-') => {
                // `oxterm new DIR` is a convenient shorthand for --cwd.
                if cwd.is_some() {
                    return Err(format!("unexpected argument: {}", arg));
                }
                cwd = Some(expand_dir(arg)?);
            }
            _ => return Err(format!("unknown option: {}", arg)),
        }
        i += 1;
    }

    if captured {
        if scope != CaptureScope::Screen {
            return Err("--captured cannot be combined with --scrollback or --all".to_string());
        }
        scope = CaptureScope::Captured { clear };
    } else if clear {
        return Err("--clear only applies with --captured".to_string());
    }

    match name {
        "new" => Ok(CliMode::New {
            cwd,
            title,
            command,
        }),
        "split" => Ok(CliMode::Split {
            pane,
            vertical,
            cwd,
            title,
            command,
        }),
        "send" => {
            let text = if read_stdin {
                None
            } else {
                Some(text_parts.join(" "))
            };
            if text.as_deref() == Some("") && !enter {
                return Err("nothing to send".to_string());
            }
            Ok(CliMode::Send { pane, enter, text })
        }
        "capture" => Ok(CliMode::Capture { pane, scope }),
        "wait" => Ok(CliMode::Wait { pane, timeout }),
        _ => unreachable!(),
    }
}

pub fn expand_dir(raw: &str) -> Result<String, String> {
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_default()
    } else if raw == "~" {
        dirs::home_dir().unwrap_or_default()
    } else {
        std::path::PathBuf::from(raw)
    };
    if !expanded.is_dir() {
        return Err(format!("not a directory: {}", raw));
    }
    std::fs::canonicalize(&expanded)
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|e| format!("cannot resolve {}: {}", raw, e))
}

#[cfg(test)]
mod connection_verb_tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<CliMode, String> {
        let args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        parse_verb(&args)
    }

    #[test]
    fn connect_requires_a_name_and_keeps_pane_with_split() {
        assert!(parse(&["connect"]).is_err());
        assert!(parse(&["connect", "a", "b"]).is_err());
        assert!(parse(&["connect", "a", "--pane", "1"]).is_err());
        assert!(parse(&["connect", "--nope", "a"]).is_err());
        assert_eq!(
            parse(&["connect", "db", "--split", "--pane", "1", "-y"]).unwrap(),
            CliMode::Connect {
                name: "db".to_string(),
                split: true,
                pane: Some("1".to_string()),
                yes: true,
            }
        );
    }

    #[test]
    fn connections_and_tunnel_are_strict() {
        assert_eq!(
            parse(&["connections", "--json"]).unwrap(),
            CliMode::Connections { json: true }
        );
        assert!(parse(&["connections", "x"]).is_err());
        assert_eq!(
            parse(&["connections", "export"]).unwrap(),
            CliMode::ConnectionsExport
        );
        assert!(parse(&["connections", "import"]).is_err());
        assert_eq!(
            parse(&["connections", "import", "f", "--allow-local-commands"]).unwrap(),
            CliMode::ConnectionsImport {
                file: "f".to_string(),
                allow_local: true,
            }
        );
        assert_eq!(
            parse(&["tunnel"]).unwrap(),
            CliMode::Tunnel {
                action: TunnelAction::List,
                name: None,
                index: None,
            }
        );
        assert_eq!(
            parse(&["tunnel", "start", "db", "1"]).unwrap(),
            CliMode::Tunnel {
                action: TunnelAction::Start,
                name: Some("db".to_string()),
                index: Some(1),
            }
        );
        assert!(parse(&["tunnel", "start"]).is_err());
        assert!(parse(&["tunnel", "stop", "db", "x"]).is_err());
        assert!(parse(&["tunnel", "boom", "db"]).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_new_with_cwd_title_and_command() {
        let mode = parse_verb(&args(&[
            "new", "--cwd", "/tmp", "--title", "Build", "--", "make", "test",
        ]))
        .unwrap();
        assert_eq!(
            mode,
            CliMode::New {
                cwd: Some("/tmp".to_string()),
                title: Some("Build".to_string()),
                command: args(&["make", "test"]),
            }
        );
    }

    #[test]
    fn parses_split_defaults_and_orientation() {
        let mode = parse_verb(&args(&["split", "--horizontal", "--pane", "123"])).unwrap();
        assert_eq!(
            mode,
            CliMode::Split {
                pane: Some("123".to_string()),
                vertical: false,
                cwd: None,
                title: None,
                command: Vec::new(),
            }
        );
        assert!(matches!(
            parse_verb(&args(&["split"])).unwrap(),
            CliMode::Split { vertical: true, .. }
        ));
    }

    #[test]
    fn parses_send_text_stdin_and_options() {
        let mode = parse_verb(&args(&["send", "--pane", "1", "--enter", "make", "test"])).unwrap();
        assert_eq!(
            mode,
            CliMode::Send {
                pane: Some("1".to_string()),
                enter: true,
                text: Some("make test".to_string()),
            }
        );
        assert_eq!(
            parse_verb(&args(&["send", "-"])).unwrap(),
            CliMode::Send {
                pane: None,
                enter: false,
                text: None,
            }
        );
        assert!(parse_verb(&args(&["send"])).is_err());
        assert!(parse_verb(&args(&["send", "--enter", "--"])).is_ok());
    }

    #[test]
    fn parses_capture_scopes_and_wait_timeout() {
        assert_eq!(
            parse_verb(&args(&["capture", "--scrollback", "500"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::Lines(500),
            }
        );
        assert_eq!(
            parse_verb(&args(&["capture", "--all"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::All,
            }
        );
        assert_eq!(
            parse_verb(&args(&["wait", "--timeout", "5"])).unwrap(),
            CliMode::Wait {
                pane: None,
                timeout: Some(Duration::from_secs(5)),
            }
        );
    }

    #[test]
    fn rejects_unknown_options_and_missing_values() {
        assert!(parse_verb(&args(&["split", "--bogus"])).is_err());
        assert!(parse_verb(&args(&["send", "--pane"])).is_err());
        assert!(parse_verb(&args(&["capture", "--scrollback", "many"])).is_err());
        assert!(parse_verb(&args(&["new", "--cwd", "/definitely/not/a/dir"])).is_err());
        assert!(parse_cli_mode(&args(&["oxterm", "nope"])).is_none());
        assert!(parse_cli_mode(&args(&["oxterm", "new"])).is_some());
    }

    #[test]
    fn verb_help_is_generated_from_the_table() {
        for entry in VERBS {
            let help = parse_verb(&args(&[entry.name, "--help"])).unwrap();
            assert_eq!(help, CliMode::Help(entry.help));
            assert!(entry.help.contains("Usage: oxterm"));
            assert!(summary().contains(entry.name));
        }
    }

    #[test]
    fn parses_fix_and_agent() {
        assert_eq!(
            parse_verb(&args(&[
                "fix", "--pane", "3", "--print", "check", "the", "path"
            ]))
            .unwrap(),
            CliMode::Fix {
                pane: Some("3".to_string()),
                print_only: true,
                hint: Some("check the path".to_string()),
            }
        );
        assert_eq!(
            parse_verb(&args(&["fix"])).unwrap(),
            CliMode::Fix {
                pane: None,
                print_only: false,
                hint: None,
            }
        );
        assert!(parse_verb(&args(&["fix", "--bogus"])).is_err());

        assert_eq!(
            parse_verb(&args(&["agent", "--list"])).unwrap(),
            CliMode::Agent {
                pane: None,
                action: AgentAction::List,
            }
        );
        assert_eq!(
            parse_verb(&args(&["agent", "--show"])).unwrap(),
            CliMode::Agent {
                pane: None,
                action: AgentAction::Show,
            }
        );
        assert_eq!(
            parse_verb(&args(&["agent", "--pane", "4", "why", "failed?"])).unwrap(),
            CliMode::Agent {
                pane: Some("4".to_string()),
                action: AgentAction::Prompt(Some("why failed?".to_string())),
            }
        );
        assert_eq!(
            parse_verb(&args(&["agent"])).unwrap(),
            CliMode::Agent {
                pane: None,
                action: AgentAction::Prompt(None),
            }
        );
        assert!(parse_verb(&args(&["agent", "--list", "--show"])).is_err());
        assert!(parse_verb(&args(&["agent", "--list", "prompt"])).is_err());
    }

    #[test]
    fn parses_events_and_actions() {
        assert_eq!(
            parse_verb(&args(&[
                "events", "--pane", "1", "--event", "bell", "--output"
            ]))
            .unwrap(),
            CliMode::Events {
                pane: Some("1".to_string()),
                events: vec!["bell".to_string()],
                output: true,
                json: false,
            }
        );
        assert!(parse_verb(&args(&["events", "--event", "nope"])).is_err());

        assert_eq!(
            parse_verb(&args(&["action", "set-tab-color", "--pane", "1", "#ABC"])).unwrap(),
            CliMode::Action {
                pane: Some("1".to_string()),
                command: ActionCommand::SetTabColor {
                    color: "#aabbcc".to_string(),
                },
            }
        );
        assert_eq!(
            parse_verb(&args(&["action", "set-tab-title", "build", "failed"])).unwrap(),
            CliMode::Action {
                pane: None,
                command: ActionCommand::SetTabTitle {
                    title: "build failed".to_string(),
                },
            }
        );
        assert_eq!(
            parse_verb(&args(&["action", "notify", "--title", "T", "--body", "B"])).unwrap(),
            CliMode::Action {
                pane: None,
                command: ActionCommand::Notify {
                    title: "T".to_string(),
                    body: "B".to_string(),
                },
            }
        );
        assert_eq!(
            parse_verb(&args(&["action", "open-url", "https://example.com"])).unwrap(),
            CliMode::Action {
                pane: None,
                command: ActionCommand::OpenUrl {
                    url: "https://example.com".to_string(),
                },
            }
        );
        assert!(parse_verb(&args(&["action", "open-url", "file:///etc/passwd"])).is_err());
        assert_eq!(
            parse_verb(&args(&["action", "insert-text", "-"])).unwrap(),
            CliMode::Action {
                pane: None,
                command: ActionCommand::InsertText { text: None },
            }
        );
        assert!(parse_verb(&args(&["action", "mark-failure", "x"])).is_err());
        assert_eq!(
            parse_verb(&args(&["action", "clear-tab-color", "--pane", "1"])).unwrap(),
            CliMode::Action {
                pane: Some("1".to_string()),
                command: ActionCommand::ClearTabColor,
            }
        );
        assert!(parse_verb(&args(&["action", "nope"])).is_err());
    }

    #[test]
    fn parses_captured_output_and_clear() {
        assert_eq!(
            parse_verb(&args(&["capture", "--captured"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::Captured { clear: false },
            }
        );
        assert_eq!(
            parse_verb(&args(&["capture", "--captured", "--clear"])).unwrap(),
            CliMode::Capture {
                pane: None,
                scope: CaptureScope::Captured { clear: true },
            }
        );
        assert!(parse_verb(&args(&["capture", "--clear"])).is_err());
        assert!(parse_verb(&args(&["capture", "--captured", "--all"])).is_err());
    }

    #[test]
    fn parses_workspace_options() {
        assert_eq!(
            parse_verb(&args(&["workspace", "project.json"])).unwrap(),
            CliMode::Workspace {
                file: "project.json".to_string(),
                new_window: false,
                replace: false,
            }
        );
        assert_eq!(
            parse_verb(&args(&["workspace", "--new-window", "project.json"])).unwrap(),
            CliMode::Workspace {
                file: "project.json".to_string(),
                new_window: true,
                replace: true,
            }
        );
        assert_eq!(
            parse_verb(&args(&["workspace", "--replace", "p.json"])).unwrap(),
            CliMode::Workspace {
                file: "p.json".to_string(),
                new_window: false,
                replace: true,
            }
        );
        assert!(parse_verb(&args(&["workspace"])).is_err());
        assert!(parse_verb(&args(&["workspace", "a", "b"])).is_err());
        assert!(parse_verb(&args(&["workspace", "--nope", "a"])).is_err());
    }

    #[test]
    fn parses_mcp_and_browse() {
        assert_eq!(parse_verb(&args(&["mcp"])).unwrap(), CliMode::Mcp);
        assert!(parse_verb(&args(&["mcp", "x"])).is_err());
        assert_eq!(
            parse_verb(&args(&["browse", "db"])).unwrap(),
            CliMode::Browse {
                name: "db".to_string()
            }
        );
        assert!(parse_verb(&args(&["browse"])).is_err());
        assert!(parse_verb(&args(&["browse", "a", "b"])).is_err());
    }
}
