# Extensions and automation

Developer notes for Oxterm's scriptable and declarative surface. The user-facing
reference is in [the manual](../manual.md#scriptable-control); `oxterm
--list-actions` prints the control commands.

Oxterm has triggers, events, actions, workspaces, status widgets, palette
entries and an MCP server. The external plugin package system of Phase 3 was
**removed**; there is no supported plugin format today.

## Removed: plugin packages

The external-process plugin host, the graphical plugin manager, `examples/plugins`
and the **Plugins** menu were removed on request: the model could not draw UI and
the dedicated menu confused more than it helped. The code and examples remain in
Git history (`bceb0a2` and later); `src/headless/plugins.rs`, `src/plugins.rs` and
`src/plugins_ui.rs` no longer exist.

## What is implemented

| Surface | Entry point |
| --- | --- |
| Control API | `oxterm new`, `split`, `send`, `capture`, `wait` |
| Events | `oxterm events [--output] [--json]` |
| Actions | `oxterm action set-tab-color`, `set-tab-title`, `notify`, `open-url`, `insert-text`, `mark-failure`, `clear-failure`, `clear-tab-color` |
| Triggers | `~/.config/oxterm/triggers.json` |
| Workspaces | `oxterm workspace [--new-window] [--replace] FILE` |
| Status widgets and palette entries | `~/.config/oxterm/extensions.json` |
| MCP | `oxterm mcp` |

**Triggers.** A JSON file of regular expressions with actions `highlight_line`,
`highlight_text`, `notify`, `capture`, `set_tab_color`, `set_tab_title`,
`hyperlink` and `mark_failure`. A trigger can be limited to `profiles` and to
`command_output` (only between an OSC 133 command start and its end). `capture`
adds a line to a per-pane list, read with `oxterm capture --captured` and
cleared with `--clear`. Triggers never execute code. VTE 0.70 cannot restyle
text that was already printed, so `highlight_line`, `highlight_text` and
`hyperlink` are parsed but not applied yet.

**Events.** `oxterm events` streams `command_started`, `command_finished`,
`prompt`, `bell`, `title_changed`, `cwd_changed`, `tab_created`, `tab_closed`,
`pane_focused`, `selection_changed` and `agent_status`.
`output_line` is opt-in with `--output`
and can contain secrets. `selection_changed` carries only the length, never the
selected text.

**Actions.** Small changes to a running pane: tab colour and title, a desktop
notification, `http`/`https` URLs only, text insertion that respects read-only
mode, and failure marks. No action accepts arbitrary shell commands;
`new`/`split -- COMMAND` already covers that.

**Workspaces.** A JSON file with `single`, `vertical` or `horizontal` layout, one
or two panes, and tabs with `title`, `cwd`, `command` and `color`. A workspace
lives in one window; `--new-window` starts a clean one.

**Status widgets and palette entries.** `extensions.json` adds a status bar of
shell commands and extra command-palette rows. Widgets run in the active
terminal's directory with a two-second timeout and a 1–3600 second interval;
the first non-empty output line is shown. Palette commands run in a new tab.
The file is read once per process.

**MCP.** `oxterm mcp` speaks JSON-RPC on stdin/stdout and exposes
`terminal_list`, `terminal_new`, `terminal_split`, `terminal_send`,
`terminal_capture`, `terminal_wait` and `terminal_workspace` through the control
socket.

## Boundaries

- Display-free parsing, validation and matching live in `src/headless/`;
  `tests/headless.rs` fails if they import GTK, VTE or GUI modules.
- The control socket is private to the user (`0700` directory, `0600` socket,
  `SO_PEERCRED` check) and never listens on the network.
- Event subscriptions are bounded: a slow subscriber is disconnected instead of
  blocking the GUI.
- A trigger cannot read or modify the input line, so it cannot intercept
  passwords.
- Still deferred: `theme_changed` is a reserved event name that is not emitted,
  context-menu contributions from `extensions.json`, hot reload of triggers and
  extensions, and a WASM panel runtime.
- The file browser is core, not a plugin; an external process cannot draw a
  window inside Oxterm.

## Files and tests

| Piece | File |
| --- | --- |
| Trigger parsing, validation, matching | `src/headless/triggers.rs` |
| Event schema, filters, `EventHub` | `src/headless/events.rs` |
| Control-command parsing | `src/headless/control_cli.rs` |
| Control socket client and server | `src/control.rs` |
| Trigger application | `src/terminal.rs` |
| Tab colour/title, notifications, captured lists | `src/window.rs` |
| Workspace parsing and application | `src/headless/workspace.rs`, `src/window.rs` |
| Widget and palette parsing | `src/headless/extensions.rs` |
| Status bar and palette entries | `src/status_bar.rs`, `src/extensions.rs`, `src/terminal.rs` |
| MCP server and tools | `src/headless/mcp.rs`, `src/control.rs` |

Tests live in `src/headless/` and in `tests/control_api.rs`, `tests/mcp.rs` and
the GUI lifecycle suite. Triggers and `extensions.json` are read once at startup;
restart Oxterm after editing them.
