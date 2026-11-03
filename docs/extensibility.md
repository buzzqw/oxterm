# Extensions: design

Design document for Oxterm's extensibility. It describes the chosen model and
the status of each phase; most of it is implemented, and the tables below mark
what remains.

Last updated: 2026-11-03.

## Principles

1. **Display-free logic lives in `src/headless/`.** Parsing, validation, trigger
   matching and event serialisation use no GTK/VTE and are covered by ordinary
   tests. Only the wiring to widgets stays in `src/`.
2. **No new dependency for Phase 1.** Triggers, events and actions use
   `serde_json`, the control socket and the modules that already exist.
3. **Three layers, introduced in order.** Declarative (JSON), external processes
   on the control socket, optional WASM. Each layer must be useful on its own.
4. **The binary stays small.** No always-linked runtime: WASM will live behind a
   Cargo feature or in a separate helper, consistent with LTO, `strip`,
   `codegen-units = 1` and `panic = "abort"`.
5. **Security is part of the format.** A trigger that writes input or runs
   commands is disabled by default and requires explicit consent. Events never
   contain screen contents unless requested.
6. **The broker is an advantage, not a detail.** An external extension must be
   able to run on the remote host and talk to Oxterm through the broker, as the
   control commands already do.

## Phase 1 — Triggers, events, actions

All three pieces are necessary: triggers alone can only perform fixed actions,
events alone cannot react to output, actions alone have no caller.

### 1.1 Trigger file

A dedicated file, not a key in `settings.json`: triggers are a list of objects,
not a scalar value, and `src/headless/spec.rs` describes only scalar values. The
file lives next to the configuration:

```text
~/.config/oxterm/triggers.json          (or $XDG_CONFIG_HOME/oxterm/)
```

```json
{
  "version": 1,
  "triggers": [
    {
      "id": "compiler-errors",
      "enabled": true,
      "match": "^([^:/]+):(\\d+):(\\d+): (error|warning): (.*)$",
      "actions": [
        { "type": "highlight_line", "fg": "#ff5555" },
        {
          "type": "capture",
          "list": "errors",
          "label": "{1}:{2}:{3}  {5}",
          "open": "editor_jump"
        }
      ]
    },
    {
      "id": "test-failures",
      "match": "^test .* \\.\\.\\. FAILED$",
      "actions": [
        { "type": "notify", "title": "Test failed", "body": "{0}" },
        { "type": "set_tab_color", "color": "#cc3333" }
      ]
    }
  ]
}
```

Fields:

| Field | Type | Notes |
| --- | --- | --- |
| `version` | int | Required, currently `1`. A file with a higher version is opened read-only. |
| `id` | string | Unique, used in logs and to disable a trigger. |
| `enabled` | bool | Defaults to `true`. |
| `match` | string | Rust regex (`regex` crate). Required. |
| `actions` | array | At least one action. |
| `profiles` | array | Optional: applies the trigger only to the listed profiles. |
| `scope` | string | `line` (default) or `command_output` (only between the OSC 133 prompt and the end of the command). |

v1 actions, all without code execution:

| Type | Effect | Parameters |
| --- | --- | --- |
| `highlight_line` | Colours the whole line | `fg`, `bg` |
| `highlight_text` | Colours only the match | `fg`, `bg` |
| `notify` | Desktop notification | `title`, `body` |
| `capture` | Appends to a "Captured output" list | `list`, `label`, `open` |
| `set_tab_color` | Colours the tab | `color` |
| `set_tab_title` | Changes the tab title | `title` |
| `hyperlink` | Makes the match clickable | `url` |
| `mark_failure` | Marks the tab as failed | none |

In parameters, `{0}` is the whole match, `{1}`..`{9}` the groups; `{{` and `}}`
are literal braces.

Actions deferred to a later version, because they execute code or write input:
`inject` (sends bytes to the PTY) and `run` (runs a command). If they arrive they
will require `"allow_exec": true` in the file and a visible warning.

#### Reading the output with VTE 0.70

VTE exposes no per-line callback. The hook is `contents_changed`, with these
rules:

1. A `TriggerScanner` in `src/headless/triggers.rs` keeps a monotonic
   `(row, column)` cursor in the text area.
2. On the signal, the GTK side calls `vte_terminal_get_text_range` from the
   cursor to the current position, splits it into complete lines and passes only
   those to the headless module.
3. Events are coalesced with a short timer (50 ms) so the scan does not run on
   every byte.
4. If the cursor moves backwards (scrollback trimmed, reset, clear), the scanner
   realigns to the first visible line and starts over.
5. Matching runs on plain text, without attributes. The GTK side applies colour
   with the available selection/attribute APIs; for v1 it is acceptable to
   colour the line through an overlay or the margin if VTE does not allow
   changing attributes that were already printed.

Point 5 is the main technical risk: VTE does not allow rewriting the attributes
of text already in the buffer. Before implementing `highlight_*` we must check
whether an overlay drawn on the widget is enough; otherwise v1 exposes only
`capture`, `notify`, `set_tab_color`, `set_tab_title` and `mark_failure`, and
defers highlighting.

### 1.2 Events

The control socket already uses JSON frames with a length prefix
(`src/headless/framing.rs`). A long-lived connection is added:

```text
→ {"op":"subscribe","events":["command_finished","bell"],"include_output":false}
← {"ok":true,"subscription":1}
← {"event":"command_finished","pane":"12345-2","tab":"12345-1","exit":1,
   "duration_ms":4213,"cwd":"/src/app","time":"2026-11-02T09:12:33Z"}
← {"event":"bell","pane":"12345-2","time":"..."}
```

v1 events:

| Event | Payload |
| --- | --- |
| `command_started` | `pane`, `command`, `cwd`, `time` |
| `command_finished` | `pane`, `exit`, `duration_ms`, `cwd`, `time` |
| `prompt` | `pane`, `cwd`, `time` |
| `bell` | `pane`, `time` |
| `title_changed` | `pane`, `title`, `time` |
| `cwd_changed` | `pane`, `cwd`, `time` |
| `tab_created` / `tab_closed` | `tab`, `pane`, `time` |
| `pane_focused` | `pane`, `tab`, `time` |
| `selection_changed` | `pane`, `length`, `time` (never the text) |
| `theme_changed` | `light`, `time` |

`output_line` is implemented but **opt-in** per subscription
(`"include_output":true`) and rate-limited per second, because it can contain
secrets. Default: off.

GTK-free implementation for the testable part:

```rust
pub struct EventHub {
    subscribers: Vec<EventSubscriber>, // mpsc::Sender<Value> + filter
}

impl EventHub {
    pub fn publish(&self, event: &Event);
    pub fn subscribe(&mut self, filter: EventFilter) -> SubscriptionId;
}
```

The GUI side owns a single `EventHub`; the points that already know about
OSC 133, the bell, the title and the directory publish here. Queues are
bounded: a slow subscriber is disconnected, it does not block the GUI.

CLI:

```text
oxterm events [--pane ID] [--event NAME]... [--output] [--json]
```

It prints one JSON object per line and exits when the socket closes. Without
`--json` it prints a readable form, useful by hand.

### 1.3 Actions

A new operation on the socket:

```text
→ {"op":"action","action":"set-tab-color","pane":"12345-2","color":"#cc3333"}
← {"ok":true}
```

v1 actions: `set-tab-color`, `clear-tab-color`, `set-tab-title`, `notify`,
`open-url`, `insert-text`, `mark-failure`, `clear-failure`.

Matching CLI:

```text
oxterm action set-tab-color --pane ID COLOR
oxterm action set-tab-title --pane ID TITLE
oxterm action notify --title T --body B
oxterm action open-url URL
oxterm action insert-text --pane ID TEXT
```

`insert-text` reuses the `oxterm send` implementation. The draft also proposed
`open-path`, `send`, `new-tab`, `split` and `set-status` as socket actions; they
were dropped because the `send`, `new` and `split` commands cover them and
`set-status` has no consumer. No action accepts arbitrary shell commands:
`-- COMMAND` in `new`/`split` already covers that.

### 1.4 Files and responsibilities

| File | Role | GTK |
| --- | --- | --- |
| `src/headless/triggers.rs` | Parsing, validation, matching, effects, tests | No |
| `src/headless/events.rs` | Event schema, filters, `EventHub` | No |
| `src/headless/control_cli.rs` | Parsing of `events` and `action` | No |
| `src/control.rs` | Client and server for the new operations | Yes |
| `src/terminal.rs` | `contents_changed` → scanner → effects | Yes |
| `src/window.rs` | Tab colour/title, notifications, captured lists | Yes |
| `manual.md` | User documentation | — |

`docs/settings.md` does not change: triggers are not scalar settings. The manual
explains the file and the commands.

### 1.5 Security

- The socket remains private to the user, with the `SO_PEERCRED` check as today.
- At most 16 concurrent subscriptions, bounded queues, slow clients
  disconnected.
- `output_line` is off by default and rate-limited.
- No trigger can read or modify the current input line, so it cannot intercept
  passwords.
- `inject` and `run` do not exist in v1. If they arrive, only with explicit
  consent and a visible indicator in the tab.
- Notifications never include screen text unless the trigger explicitly asks for
  it.

### 1.6 Test plan

Headless tests (`src/headless/triggers.rs`, `events.rs`):

- parsing of a valid file, future versions, unknown fields, invalid regex;
- `{0}`/`{n}` with missing groups, `{{`/`}}`;
- action order and stop at the first match with `"once": true` (planned field);
- event serialisation and filters by pane/event;
- `EventHub`: slow subscriber, limit, disconnection.

Control API tests (`tests/control_api.rs`):

- `subscribe` receives `command_started`/`command_finished` after a real command;
- `action set-tab-color` changes the state and `--info` reports it;
- action on a nonexistent pane → exit code 3;
- no reply to an unauthorised peer (if the test allows it).

Manual tests:

```bash
printf 'error: boom\n'          # highlight/capture trigger
oxterm events | jq .            # live events
oxterm action set-tab-color --pane "$OXTERM_PANE" '#cc3333'
```

## Phase 2 — Widgets, contributions, workspaces

Implemented in `~/.config/oxterm/extensions.json` and in the
`oxterm workspace` command:

- **Status widget.** Each widget runs a shell command in the active terminal's
  directory, with a 2 s timeout and a configurable interval (1–3600 s). It shows
  the first non-empty line of stdout; an error shows `—`. The renderer is plain
  text with an optional colour: no charts or click actions in this version.
- **Contributions.** The same file declares entries for the command palette
  (`label`, `detail`, `command`). The command runs in a new tab, which then
  enters `$SHELL`. Context-menu entries are deferred.
- **Workspace.** `oxterm workspace [--new-window] [--replace] FILE` with JSON,
  not YAML. Structure: layout `single|vertical|horizontal`, one or two panes,
  tabs with `title`, `cwd`, `command`, `color`. No per-tab `profile` and no
  creation of multiple windows from a single file: a workspace lives in one
  window. `--new-window` starts a clean window and implies `--replace`.

## Phase 3 — Plugin packages: removed

Phase 3 was implemented and then **removed on request**: the external-process
model with events on stdin did not offer the screen and configuration one
expects from a plugin, and the dedicated menu confused more than it helped. The
code and examples remain in Git history (`bceb0a2` and later) in case they serve
as a basis for a future WASM runtime.

## Phase 4 — WASM panels and MCP

- **MCP implemented** as `oxterm mcp` (in the same binary, not a separate
  helper): JSON-RPC on stdin/stdout, tools `terminal_list`, `terminal_new`,
  `terminal_split`, `terminal_send`, `terminal_capture`, `terminal_wait`,
  `terminal_workspace`. It uses the existing control socket.
- **WASM deferred.** `wasmtime` adds weight to the binary and requires a real
  capability model and a panel renderer; without a concrete use case it is not
  worth introducing now. It remains the candidate for extensions that must draw
  UI or orchestrate the workspace in a real sandbox.

## File browser: core, not a plugin

The two-pane SFTP/FTP browser stays in the core (`src/file_browser.rs`) and does
not become an extension: an external process cannot draw a window inside Oxterm.
The "WinSCP-style" features — recursive sync, bookmarks, remote editing with
upload on save and password prompts — were added there, and the browser opens
from the right-click menu on an SSH tab or from the connection list.

## Implementation status (2026-11-02)

Phases 1 and 2 are implemented; of Phase 4 there is MCP, while WASM is deferred.
Phase 3 (plugins) was removed:

| Piece | File | Status |
| --- | --- | --- |
| Trigger parsing, validation and matching | `src/headless/triggers.rs` | done, with tests |
| Event bus and filters | `src/headless/events.rs` | done, with tests |
| `events` / `action` / `capture --captured` CLI | `src/headless/control_cli.rs` | done, with tests |
| Control socket client and server | `src/control.rs` | done |
| Triggers on `contents_changed` | `src/terminal.rs` | done |
| Events `command_started`, `command_finished`, `prompt`, `bell`, `title_changed`, `cwd_changed`, `tab_created`, `tab_closed`, `pane_focused`, `selection_changed` | `src/terminal.rs`, `src/window.rs` | done |
| `output_line` (only with `--output`) | `src/terminal.rs` | done |
| Actions `set-tab-color`, `clear-tab-color`, `set-tab-title`, `notify`, `open-url`, `insert-text`, `mark-failure`, `clear-failure` | `src/control.rs`, `src/window.rs` | done |
| `theme_changed` | — | reserved name, not emitted yet |
| `highlight_line`, `highlight_text`, `hyperlink` | `src/headless/triggers.rs` | accepted and interpreted, not applied: VTE 0.70 does not allow restyling text already printed |
| Workspace parsing and validation | `src/headless/workspace.rs` | done, with tests |
| `oxterm workspace` and applying it in the window | `src/control.rs`, `src/window.rs` | done, with GUI tests |
| Widget and palette command parsing and timeouts | `src/headless/extensions.rs` | done, with tests |
| Widget status bar | `src/status_bar.rs`, `src/window.rs` | done |
| Palette entries from `extensions.json` | `src/extensions.rs`, `src/terminal.rs` | done |
| Context-menu entries | — | deferred |
| MCP server and tools | `src/headless/mcp.rs`, `src/control.rs` | done, with tests and a real run |
| WASM panels | — | deferred |
| Recursive sync, bookmarks and remote editing in the browser | `src/file_browser.rs`, `src/headless/bookmarks.rs`, `src/headless/remote_files.rs` | done, tested against a local SSH server |
| `oxterm browse NAME` | `src/control.rs`, `src/headless/control_cli.rs` | done |

Deviations from the first draft:

- The tab colour is the **text** colour of the label, not the background: GTK
  markup is the only per-tab mechanism that needs no CSS.
- `capture` has no dedicated panel: lines end up in a per-pane list, read with
  `oxterm capture --captured` and cleared with `--clear`. A panel may arrive in a
  later phase.
- Triggers and `extensions.json` are read once per process. Hot reload is still
  to be done.

## Open decisions

1. ~~Triggers in `settings.json` or a dedicated file?~~ Dedicated file,
   implemented.
2. ~~Highlighting with VTE 0.70.~~ Deferred: text already printed cannot be
   restyled. An overlay on the widget remains to be evaluated.
3. ~~Workspace format.~~ JSON, implemented: layout, one or two panes, tabs with
   `title`, `cwd`, `command`, `color`.
4. **WASM runtime.** `wasmtime` or `extism`; **deferred**: no use case justifies
   the weight and complexity while the external host is enough. To be revisited
   if panels that draw inside Oxterm are needed.
5. **Events as JSON Lines or frames?** On the socket they stay frames as today;
   the CLI prints JSON Lines for convenience with `jq`.

## Log

- 2026-11-02: first draft. No code written; Phase 1 ready for implementation.
- 2026-11-02: Phase 1 implemented (triggers, events, actions, `capture
  --captured`) with headless tests. Highlighting and hyperlinks deferred because
  of VTE 0.70 limits.
- 2026-11-02: Phase 2 implemented: `oxterm workspace`, widget status bar and
  palette contributions from `extensions.json`, with headless and GUI tests.
- 2026-11-02: Phase 3 implemented and then removed; `oxterm mcp` from Phase 4
  stays. WASM deferred.
- 2026-11-02: file browser: recursive sync, per-connection bookmarks, remote
  editing with upload on save and `oxterm browse`.
- 2026-11-02: plugins disabled and the **Plugins** menu removed; the file
  browser remains the WinSCP path.
