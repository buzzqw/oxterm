# Agent pane and opt-in agent integration

Status: implemented. Everything in this document is off by default.

## Goals

- Let a user who wants one talk to an ACP-compatible coding agent without
  leaving the terminal.
- Give that agent the context of the terminal (command, directory, exit status,
  recent output) instead of a copy-paste.
- Never start, install, or call anything on the user's behalf before they have
  enabled agent features and named the agent they use.
- Keep every write path behind a policy and an audit trail.

Non-goals: embedding a model, shipping credentials, choosing an agent for the
user, persisting conversations.

## Enabling

Two settings must both be true before anything can start:

```jsonc
{
    "agent_enabled": true,
    "agent_command": "opencode acp"   // the ACP agent CLI the user installed
}
```

`oxterm agent --list` reports the known ACP CLIs already on `PATH` and the
command to configure for each; `oxterm agent --show` prints the effective
configuration and the policies. The same settings are on the **Agents** page of
the preferences window, including a "Detect installed agents" button. Oxterm
never installs an agent.

## Architecture

```
MainWindow
  |-- AgentPanel (GTK, per window)
  |     |-- one ACP session per tab, keyed by pane id
  |     `-- transcript is kept while the pane is hidden (stash/restore)
  |
  |-- TerminalBox::last_command_context()  (OSC 133 + scrollback)
  |
  `-- AgentPanel -> child process: ACP over stdio (JSON-RPC, one object/line)
                    `-- mcpServers: ["<oxterm>", "mcp"]  (optional)
```

- `src/headless/acp.rs` owns the protocol shapes and update parsing. It has no
  GTK dependency, like every other module under `src/headless/`.
- `src/agent_panel.rs` owns the transport (one writer thread and one reader
  thread around the child's stdio), the widgets, and the per-tab session map.
- `src/headless/agent.rs` owns the policy, the audit format, agent command
  parsing and the fix prompt.

The agent is not linked into Oxterm. It is a normal child process; killing the
session kills only that process.

## Session lifecycle

1. The first prompt for a tab builds the command from `agent_command` (local) or
   `ssh -T <running ssh args> $agent_remote_command` (when the tab is an SSH
   client and `agent_remote_command` is set).
2. `initialize` advertises the client as `oxterm`, protocol version 1, with no
   filesystem or terminal client capabilities.
3. `session/new` passes the terminal's working directory and, when
   `agent_mcp` is true and the agent is local, Oxterm's own MCP server:
   `{"name": "oxterm", "command": "<current exe>", "args": ["mcp"]}`.
4. `session/prompt` sends one text block. `session/update` notifications are
   rendered as they arrive: message chunks, thoughts, tool calls, plans, usage
   and notices.
5. `session/request_permission` opens a modal dialog with the tool title and the
   options the agent offered. "No" is the default and maps to the first
   `reject_*` option; "Yes" maps to the first `allow_*` option. A cancelled turn
   answers `cancelled`.
6. `session/cancel` is sent by `/stop`. The process stays alive for the next
   prompt.
7. Closing a tab kills only its session; hiding the pane or switching tabs keeps
   the session and its transcript.

Hiding the pane is a stash, not a destroy. `Ctrl+Shift+.` toggles it,
`Ctrl+Shift+I` focuses it, `Ctrl+Alt+.` sends the last failure. The process is
owned by the window: closing the window (or the whole application) ends every
session, and tabs of different windows do not share one.

## Policy

The same policy governs both directions:

- the MCP server (`oxterm mcp`), where the agent drives the terminal;
- the agent pane, where the agent may ask for permission on its own tools.

| Class  | Tools | Default |
| --- | --- | --- |
| read | `terminal_list`, `terminal_capture`, `terminal_wait` | `auto` |
| create | `terminal_new`/`terminal_split` without a command | `ask` |
| input | `terminal_send`, a tab/split with a command, `terminal_workspace` | `ask` |

`terminal_workspace` is input-class because a workspace file can start commands.
The classes are configurable with `agent_policy_read`, `agent_policy_create` and
`agent_policy_input` (`auto`, `ask`, `deny`). Unknown values fall back to the
strict default for the class.

An `ask` decision opens a modal in the running window. There is no window, no
answer, a timeout, or a closed dialog all mean **deny** (fail closed).

## Audit

When `agent_audit` is true (default), every decision appends one JSON line to
`~/.local/state/oxterm/agent-audit.log` (or `$XDG_STATE_HOME/oxterm/`):

```json
{"time":"2026-01-02T03:04:05Z","source":"mcp","tool":"terminal_send",
 "class":"input","decision":"ask","allowed":true,"pane":"3",
 "details":"12 characters"}
```

Typed text is not logged: only its length. Commands passed to `terminal_new`
are reduced to "with a command". The file and its directory are created
`0600`/`0700`.

## Fix context

`oxterm fix`, `/fix` and `Ctrl+Alt+.` build a prompt from
`TerminalBox::last_command_context()`:

- the last finished command block from OSC 133 (or the last one, if none is
  finished),
- the reported directory (OSC 7) when it is absolute, otherwise the process
  directory,
- the exit status and duration,
- up to 500 rows of output, truncated to the last 8,000 characters,
- `$SHELL`, when set.

`oxterm fix --print` prints the prompt instead of sending it. The prompt asks
the agent to explain the failure and show the smallest fix, and explicitly tells
it not to run commands: the user approves commands through the normal policy.

## Command line and palette

```
oxterm agent [--pane ID] [--list|--show] [PROMPT...]
oxterm fix   [--pane ID] [--print] [HINT...]
```

In the command bar, `?prompt` asks the agent with this pane as context,
`/fix [hint]` sends the last failure, and `/agent` focuses the pane.

## Remote hosts

When the tab runs an `ssh`, `autossh` or `mosh` session and
`agent_remote_command` is not empty, the agent starts on the remote host over
the same connection:

```
ssh -T <the running client's arguments> <agent_remote_command>
```

The session's `cwd` is the remote directory reported by OSC 7, or `/`. Oxterm's
MCP server is not passed to a remote agent: the remote host may not have the
binary. `-t` is removed from the inherited arguments because a pseudo-terminal
would corrupt the JSON-RPC stream.

If `agent_remote_command` is empty, an SSH tab uses the local agent with the
local process directory (the SSH client's), which is usually not what the user
wants; configure the remote command for remote work.

## Limitations

- The pane position (`agent_pane_position`) is read when the window is built; a
  change applies to new windows.
- Conversations are not persisted; a new window starts new sessions. ACP
  `session/load` is not used yet.
- Only text prompts are sent. Images and resources are not advertised.
- `agent_remote_command` uses the running `ssh` arguments; if the tab is not an
  SSH client it is ignored.
- There is no shared agent broker yet: sessions survive tab switches and hiding
  the pane, not the window. A broker that outlives the window and multiplexes
  sessions across windows is the next step if the feature is used enough to
  justify it.

## Status event

The panel publishes `agent_status` on the event bus (`ready`, `working`,
`error`, `closed`) with the pane id and no conversation content, so scripts and
triggers can follow a session without subscribing to the transcript.

## Tests

- `src/headless/agent.rs`: defaults are off, policy fallback, tool classes,
  prompt truncation, audit redaction, command parsing, PATH detection.
- `src/headless/acp.rs`: request shapes, message classification, streamed text,
  tools, plans, usage, permission defaults.
- `src/headless/control_cli.rs`: `fix`/`agent` parsing and help.
- `tests/mcp.rs`: with the switch off the server exposes no tools and explains
  why; with it on the normal tool list is back.
