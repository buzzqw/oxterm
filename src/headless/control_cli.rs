//! Parsing and help text for the scriptable control commands
//! (`oxterm new|split|send|capture|wait`).
//!
//! The execution side lives in `control.rs` and needs GTK; this half does not,
//! so every verb, option and error message is unit tested without a window.

use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum CaptureScope {
    Screen,
    Lines(usize),
    All,
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
        synopsis: "new [--cwd DIR] [--title TITLE] [-- CMD...]",
        summary: "Open a tab, print its pane id",
        help: "Usage: oxterm new [DIR] [--cwd DIR] [--title TITLE] [-- COMMAND...]\n\n\
               Open a new tab in the running Oxterm and print its pane id. DIR is a\n\
               shorthand for --cwd.\n",
    },
    Verb {
        name: "split",
        synopsis: "split [--pane ID] [--vertical|--horizontal] [--cwd DIR] [-- CMD...]",
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
        synopsis: "capture [--pane ID] [--scrollback N|--all]",
        summary: "Print a pane's text",
        help: "Usage: oxterm capture [--pane ID] [--scrollback N | --all]\n\n\
               Print the target pane's text: the visible screen by default, N lines\n\
               of history with --scrollback, or the whole buffer with --all.\n",
    },
    Verb {
        name: "wait",
        synopsis: "wait [--pane ID] [--timeout SECONDS]",
        summary: "Wait for the next command to finish",
        help: "Usage: oxterm wait [--pane ID] [--timeout SECONDS]\n\n\
               Wait for the next command in the pane to finish and exit with its\n\
               status. Without a timeout it waits indefinitely.\n",
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

pub fn parse_verb(args: &[String]) -> Result<CliMode, String> {
    let name = args.first().map(String::as_str).unwrap_or("");
    let Some(entry) = verb(name) else {
        return Err(format!("unknown control command: {name}"));
    };
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
}
