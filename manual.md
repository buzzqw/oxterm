# Oxterm User Manual

## Contents

1. [Starting Oxterm](#starting-oxterm)
2. [The interface](#the-interface)
3. [Tabs, windows, and splits](#tabs-windows-and-splits)
4. [Built-in commands](#built-in-commands)
5. [History](#history)
6. [Notes](#notes)
7. [Latest development package](#latest-development-package)
8. [Preferences](#preferences)
9. [Shell integration](#shell-integration)
10. [Keyboard shortcuts](#keyboard-shortcuts)
11. [Configuration files](#configuration-files)
12. [Troubleshooting](#troubleshooting)

## Starting Oxterm

From a source checkout:

```bash
./oxterm.sh
```

If no compiled binary exists, build it first:

```bash
cargo build --release --locked
./target/release/oxterm
```

To build and install the current release as a system command, use the repository
installer:

```bash
./install.sh
oxterm
```

`install.sh` always rebuilds the release binary and installs `/usr/bin/oxterm`,
the system desktop launcher, the `oxterm(1)` man page, bash/zsh/fish
completions, the manual, and the Oxterm ox icon. It uses `sudo` only when
needed. Run it again after compiling a newer version to update the installed
command and launcher. For a per-user installation, use `./setup.sh` instead;
it installs the same files under `~/.local` (or `$PREFIX`).

You can start in a directory, open a new window, skip session restoration, or
execute a command directly:

```bash
oxterm ~/projects/demo
oxterm --new-window
oxterm --no-restore
oxterm --execute git status
```

Oxterm also accepts options inspired by modern terminals:

```bash
oxterm --title "Build server"      # fixed window title
oxterm --geometry 120x40           # initial size in COLSxROWS cells
oxterm --fullscreen                # or --maximize
oxterm --hold --execute make       # keep the tab open after the command exits
oxterm -- ~/dir-with-dashes        # the next argument is treated as the directory
```

`--title` sets a fixed title that programs cannot override through escape
sequences. `--geometry` expects `COLSxROWS` (columns by rows, both positive).
`--fullscreen` and `--maximize` are mutually exclusive. `--hold` keeps the
terminal visible after its command finishes so the final output stays on screen.
`--version` is also available as `-V`.

Further options, inspired by kitty and alacritty, tune identity and settings:

```bash
oxterm --class MyTerm --name floating    # WM_CLASS class / instance name
oxterm --profile work                    # start with a saved profile
oxterm -o opacity=0.9 -o login_shell=false  # override settings for this run
oxterm --font "Fira Code" --font-size 13 # font shortcuts (same as -o)
oxterm --config ~/demo-settings.json     # use an alternative settings file
```

`--class` and `--name` set the two parts of WM_CLASS, letting tiling window
managers (i3, sway, hyprland) match the window for rules and icons. `--profile`
loads a saved profile, and `-o/--option` (repeatable) overrides any setting for
the current session only. `--font` and `--font-size` are convenient shortcuts
for `-o font_name=...` / `-o font_size=...`. All of these overrides are applied
in memory and are **never** written back to your configuration. `--config`
points Oxterm at a different settings file, so you can run throwaway or demo
setups without touching your real preferences.

`--execute` accepts every argument after it as part of the command. A directory
passed both positionally and with `--working-directory` is rejected.

### Remote attach

Oxterm starts a persistent PTY broker for each terminal and publishes it through
a private Unix socket. From another computer, connect with SSH and run the same
Oxterm binary on the host running the GUI:

```text
ssh andres@192.168.1.122
oxterm --list
oxterm --info 12345-2
oxterm -a
```

`--list` displays one row per live Oxterm terminal with its ID, title, current
directory, and the last command/application. `-a` attaches a framed client to
the broker, so input and output
continue from the remote computer without tmux. The local Oxterm window remains
usable, and more than one remote client can attach to the same session. On
attach the broker replays the session's most recent output, so the current
screen is repainted instead of appearing empty. Client
disconnects do not stop the shell or broker, so a later `-a SESSION_ID` can
reconnect. An unexpected GUI crash also leaves the broker and shell running;
an intentional GUI close follows the normal tab cleanup path and terminates
its shell, except for tabs whose local view is off (`Ctrl+B`, then `d`, or a
session taken over with `-M`): those keep running under their broker and can be
reached again with `-a`/`-M`.

Without a `SESSION_ID`, `oxterm -a` and `oxterm -M` print the list of active
terminals and ask which one to use. When only one terminal is running, pressing
Enter selects it. Pass `SESSION_ID` to skip the chooser.

`oxterm -M [SESSION_ID]` (or `--move`) works like `-a` but takes control instead
of sharing: it detaches every remote controller and turns off the local GUI view,
so the chosen session is displayed only in the terminal that ran the command.
Use it to hand a session over from one SSH shell to another. In the original
Oxterm window, `Ctrl+B`, then `s` or `w` and selecting the terminal brings it
back.

Use `/session name NAME` in a live terminal to assign a readable name. Use
`oxterm --detach SESSION_ID` from another SSH shell to detach all remote clients
without stopping the broker. `Ctrl+B`, then `d` turns local forwarding off;
`Ctrl+B`, then `s` or `w` and selecting the terminal turns it on again.

The broker socket uses a `0700` directory and `0600` socket under
`~/.config/oxterm/remote/`. If that path would exceed the Unix socket limit
(for example with a long `$HOME`), Oxterm uses `/tmp/oxterm-<uid>/remote/`
instead, with the same permissions. The GUI normally starts the display-free
`oxterm-broker` executable (falling back to the internal
`oxterm --broker SOCKET SESSION_ID` mode when the sibling file is not
installed), which owns the child PTY and accepts length-prefixed Unix-socket
frames for `LIST`, `INFO`, `ATTACH`, `MOVE`, `DETACH`, `RENAME`, `COMMAND`,
`LOCAL_ON`,
`LOCAL_OFF`, and `KILL`. The GUI PTY is a separate endpoint, allowing its local
forwarding to be disabled without releasing the shell session. On attach the
broker replays the most recent child output (bounded to a fixed size) to repaint
the screen and then nudges the foreground program to redraw; output older than
that window is not kept, so Oxterm does not persist full scrollback. When the
shell exits, the broker removes the session socket. Brokers are event driven: a
session left without a GUI keeps its shell alive at no CPU cost, and a broker
that stops making I/O progress for a few seconds terminates itself instead of
spinning on a stuck descriptor.

The listing columns are `ID`, `NAME`, `TITLE`, `DIRECTORY`, `STATUS`,
`APPLICATION`, and `APP_STATUS`. The application column contains the latest
shell command; `APP_STATUS` is `running` while it is active and `last` after it
has completed.

The Oxterm binary running the GUI must already include this feature. An Oxterm
process started with an older binary cannot be discovered retroactively because
it never created the private session socket.

### Scriptable control

A running Oxterm window also publishes a private control socket. It lets
scripts, editors and coding agents drive tabs and panes without a browser or a
GUI toolkit:

```text
oxterm new [DIR] [--cwd DIR] [--title TITLE] [-- COMMAND...]
oxterm split [--pane ID] [--vertical|--horizontal] [--cwd DIR] [--title TITLE]
             [-- COMMAND...]
oxterm send [--pane ID] [--enter] TEXT...        # or: ... - (read stdin)
oxterm capture [--pane ID] [--scrollback N | --all | --captured [--clear]]
oxterm wait [--pane ID] [--timeout SECONDS]
oxterm connect NAME [--split [--pane ID]] [--yes]
oxterm connections [--json | export | import FILE]
oxterm tunnel list|start|stop [NAME [INDEX]]
oxterm events [--pane ID] [--event NAME]... [--output] [--json]
oxterm action set-tab-color|set-tab-title|notify|open-url|insert-text|...
oxterm workspace [--new-window] [--replace] FILE
oxterm mcp
oxterm browse NAME
```

`new` prints the id of the new pane and `new DIR` is a shorthand for
`new --cwd DIR`. `split` prints the id of the new pane and refuses to split a
window that already has two panes. `send` types into the target pane and, with
`--enter`, submits the line. `capture` prints the visible screen by default, the
last `N` lines of history with `--scrollback`, or the whole buffer with `--all`.
`wait` waits for the next command in the pane to finish and exits with that
command's status (clamped to 0–255), which makes it possible to chain a build and
read its result. If the shell integration finishes the command without reporting
a status, `wait` fails with a message and exit status 1 rather than reporting a
success it cannot confirm:

```sh
pane=$(oxterm split --horizontal --cwd ~/src/app)
oxterm send --pane "$pane" --enter 'make test'
oxterm wait --pane "$pane" --timeout 600
status=$?
oxterm capture --pane "$pane" --scrollback 200
exit "$status"
```

The target pane can be given with `--pane`; otherwise Oxterm uses
`$OXTERM_PANE`, and falls back to the only open pane when there is just one.
Every shell started by Oxterm exports `OXTERM_PANE` and `OXTERM_CONTROL`, so a
tool running in a pane can address itself:

```sh
oxterm send --pane "$OXTERM_PANE" --enter 'git status'
```

Exit statuses are `0` for success, `1` for a failed operation, `2` for a usage
error, `3` when the pane is not found and `4` when no running Oxterm window can
be reached. The socket lives next to the broker sockets (`0700` directory,
`0600` socket), accepts only peers with the same user id, and never listens on
the network. `send` respects read-only mode. `oxterm browse NAME` opens the
two-pane file browser for a saved connection, a host from `~/.ssh/config`, or a
raw `[user@]host[:port]` destination, without opening a terminal tab.

#### Events

`oxterm events` keeps the connection open and prints one JSON object per line
until the window closes or the command is interrupted:

```sh
oxterm events --event command_finished | jq .
```

```json
{"event":"command_finished","pane":"12345-2","command":"make test",
 "exit":1,"duration_ms":4213,"cwd":"/src/app","time":"2026-11-02T09:12:33Z"}
```

The event names are `command_started`, `command_finished`, `prompt`, `bell`,
`title_changed`, `cwd_changed`, `tab_created`, `tab_closed`, `pane_focused`,
`selection_changed`, `agent_status` and `output_line` (only with `--output`).
`agent_status` reports `ready`, `working`, `error` and
`closed` for the agent session of a pane and never carries conversation text.
`selection_changed` reports only the length, never the selected text. The
heartbeat frames the server sends to notice a closed client are filtered out.
`--output` also enables `output_line`, which contains terminal output and can
therefore contain secrets; it is off by default. `--json` is accepted for
symmetry because the stream is always JSON.

#### Actions

`oxterm action` asks the running window to do one small thing:

```sh
oxterm action set-tab-color --pane "$OXTERM_PANE" '#cc3333'
oxterm action set-tab-title --pane "$OXTERM_PANE" 'build failed'
oxterm action notify --title 'Deploy' --body 'Finished'
oxterm action open-url https://example.com
oxterm action insert-text 'git status'
oxterm action mark-failure --pane "$OXTERM_PANE"
oxterm action clear-failure --pane "$OXTERM_PANE"
oxterm action clear-tab-color --pane "$OXTERM_PANE"
```

`open-url` only accepts `http` and `https`. `insert-text` types without pressing
Enter and respects read-only mode. The target pane comes from `--pane`, then
`$OXTERM_PANE`, then the only open pane.

#### Triggers

`~/.config/oxterm/triggers.json` holds regular expressions that act on output:

```json
{
  "version": 1,
  "triggers": [
    {
      "id": "compiler-errors",
      "match": "^([^:/]+):(\\d+):(\\d+): (error|warning): (.*)$",
      "actions": [
        { "type": "highlight_line", "fg": "#ff5555" },
        { "type": "capture", "list": "errors", "label": "{1}:{2}:{3}  {5}" },
        { "type": "mark_failure" }
      ]
    },
    {
      "profiles": ["work"],
      "scope": "command_output",
      "match": "(?i)\\bdeploy(ed)?\\b",
      "actions": [{ "type": "notify", "title": "Deploy", "body": "{0}" }]
    }
  ]
}
```

Actions are `highlight_line`, `highlight_text`, `notify`, `capture`,
`set_tab_color`, `set_tab_title`, `hyperlink` and `mark_failure`. `capture`
adds the expanded label to a per-pane list, printed by
`oxterm capture --captured` and emptied with `--clear`. `{0}` is the whole match
and `{1}`…`{9}` are capture groups; `{{` and `}}` are literal braces. A trigger
fires at most once per output line; `"once": true` disables it after its first
match for the rest of the session. `scope` is `line` (default) or
`command_output` (only while an OSC 133 command is running). A malformed file
is reported in the log and ignored; it never stops Oxterm from starting.

VTE 0.70 does not expose a way to restyle text that has already been printed,
so this release parses `highlight_line`, `highlight_text` and `hyperlink` but
does not apply them yet; they are reserved for a later release and logged once.

#### Workspaces

A workspace is a JSON file that opens a set of tabs, splits and commands:

```json
{
  "version": 1,
  "name": "backend",
  "layout": "vertical",
  "panes": [
    { "tabs": [
        { "title": "api", "cwd": "~/src/api", "command": "cargo watch -x run" },
        { "title": "tests", "cwd": "~/src/api", "command": "cargo test --watch" }
    ] },
    { "tabs": [
        { "title": "logs", "command": "tail -f /var/log/app.log" }
    ] }
  ]
}
```

`layout` is `single` (one pane, the default), `vertical` or `horizontal`.
`vertical` and `horizontal` need exactly two `panes`; a pane has one or more
`tabs`. Each tab accepts `title`, `cwd`, `command` (a shell line, run through
`/bin/sh -c`) and `color` (`#rrggbb`).

```sh
oxterm workspace backend.json                 # add to the running window
oxterm workspace --replace backend.json       # close the old tabs afterwards
oxterm workspace --new-window backend.json    # start a clean window first
```

Without a running window, `--new-window` is implied. Commands keep the tab open
by dropping into `$SHELL` when they finish.

#### Status widgets and palette entries

`~/.config/oxterm/extensions.json` adds a thin status bar and extra command
palette entries:

```json
{
  "version": 1,
  "widgets": [
    { "id": "git", "label": "git", "command": "git branch --show-current",
      "interval": 5, "color": "#8be9fd" },
    { "id": "load", "label": "load", "command": "cut -d' ' -f1 /proc/loadavg",
      "interval": 10 }
  ],
  "palette": [
    { "label": "Open in VS Code", "detail": "Open this directory",
      "command": "code ." }
  ]
}
```

Widgets run their command in the active terminal's directory at most once every
`interval` seconds (1–3600), with a two-second timeout; the first non-empty line
of standard output is shown. A failing or timed-out command shows `—`. Widgets
run locally: on an SSH tab the remote directory does not exist locally, so the
command falls back to Oxterm's own working directory. Palette entries appear in
`Ctrl+Shift+P` and run their command in a new tab, which drops into `$SHELL`
afterwards so the output stays visible.

The file is read once at startup; restart Oxterm after changing it. Commands run
as your user, so only add entries you trust.

#### Model Context Protocol

`oxterm mcp` speaks MCP over stdin/stdout, so an agent can drive a running
Oxterm without a display:

```sh
oxterm mcp
```

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call",
 "params":{"name":"terminal_send","arguments":{"text":"cargo test","enter":true}}}
```

The tools are `terminal_list`, `terminal_new`, `terminal_split`,
`terminal_send`, `terminal_capture`, `terminal_wait` and `terminal_workspace`.
They use the same control socket as the `oxterm` commands, so a window must be
running. Standard output carries only protocol messages; diagnostics go to
standard error.

Agent features are opt-in. With `agent_enabled` false the server still answers
the handshake but exposes no tools and explains why in `instructions`.

#### Agent pane and fix context

The agent pane speaks the Agent Client Protocol (ACP) with any agent CLI the
user has installed. It is disabled by default, and the normal way to set it up
is the interface:

1. Open **Preferences → Agents** (or Tools → **Agent Settings...**, or the ⚙
   button in the agent pane).
2. Choose one of the installed agents Oxterm found, or enter a custom command.
3. Press **Test connection**: Oxterm starts the agent, performs the ACP
   handshake and reports the agent name, protocol and model, or the error.
4. Tick **Enable agent features** and press OK.

Pressing `Ctrl+Shift+.` on a fresh configuration opens the pane with the same
choice inline: pick an agent, press Enable, and a pending prompt is sent
immediately. Nothing is installed or started before the user confirms. The pane
is docked against the terminal and its handle can be dragged to give it more or
less room; the chosen share is stored as `agent_pane_share` and the side it
opens on as `agent_pane_position`. The settings file is only the stored
representation; editing it is never required:

```jsonc
{
    "agent_enabled": true,
    "agent_command": "opencode acp"
}
```

`oxterm agent --list` reports the known ACP CLIs already installed and the
absolute command to configure for each (`PATH` plus the usual per-user bin
directories); `oxterm agent --show` prints the effective configuration and the
permission policies. Oxterm never installs an agent.

Most agents report the models they offer, sometimes in two lists; Oxterm shows
the one with more choices, so every variant the agent can switch to (including
Gemini's High/Medium/Low tiers) is selectable. The agent pane header then shows
a model picker, and the choice is written to `agent_model` so every new session
starts on it. In Preferences > Agents the Model field is the same picker: choose
an installed agent (or press Test connection) and Oxterm asks that agent for its
list, then offers the values in a combo box; the entry stays editable for a
value the agent did not list. That includes the hidden
session used when a command is not found, which is why the picker is also in
Preferences. Authentication is not Oxterm's: if the agent answers that a
provider needs credentials, log in with that agent's own CLI (`opencode auth
login`, for example) or choose a model whose provider is already authenticated.

The CLIs Oxterm recognises, and the command to use once one is installed and
logged in:

| Agent | Install | `agent_command` |
| --- | --- | --- |
| OpenCode | already packaged by OpenCode | `opencode acp` |
| Claude Code | `npm install -g @zed-industries/claude-code-acp` (plus Claude Code, logged in) | `claude-code-acp` |
| Antigravity | already packaged as `agy` | `agy acp` |
| Gemini CLI | `npm install -g @google/gemini-cli`, then run `gemini` once | `gemini --experimental-acp` |
| GitHub Copilot CLI | `npm install -g @github/copilot`, then run `copilot` once | `copilot --acp --stdio` |
| Codex | `npm install -g @agentclientprotocol/codex-acp` (plus Codex, logged in) | `codex-acp` |

Install only the one you want; each has its own account and terms. A
desktop-launched Oxterm searches `~/.opencode/bin`, `~/.local/bin` and the other
usual per-user bin directories in addition to `PATH`, and `agent --list` prints
the absolute path so the setting does not depend on the launcher's environment.

| Action | Shortcut |
| --- | --- |
| Show or hide the agent pane | `Ctrl+Shift+.` |
| Focus the agent pane | `Ctrl+Shift+I` |
| Send the last failed command | `Ctrl+Alt+.` |
| Ask with the active pane as context | `?prompt` at the shell prompt or in the command bar |
| Diagnose the last failure | `/fix [hint]` or `oxterm fix [HINT]` |

Each tab owns one session, keyed by pane id. Hiding the pane or switching tabs
keeps the session and its transcript; closing the tab ends it. A prompt sent
from `oxterm fix --pane ID` or `?prompt` starts the session on demand.

`oxterm fix` builds the prompt from the last command reported by the shell
integration: command line, reported directory, exit status, duration and up to
500 rows of output (the last 8,000 characters are kept). `--print` prints the
prompt instead of sending it. Without OSC 133 there is no context to send.

An SSH tab can run the agent on the remote host instead of locally:

```jsonc
{
    "agent_remote_command": "opencode acp"
}
```

The agent is started over the running `ssh` connection (`ssh -T ...`), with the
remote directory reported by OSC 7 as its working directory. Oxterm's MCP server
is passed only to a local agent.

#### Permissions and audit

What an agent may do is decided per class of action, for both the MCP server and
the agent's own tool permission requests:

| Setting | Class | Default |
| --- | --- | --- |
| `agent_policy_read` | read the terminal (`terminal_list`, `terminal_capture`, `terminal_wait`) | `auto` |
| `agent_policy_create` | open a tab or split, load a workspace | `ask` |
| `agent_policy_input` | type into a shell, start a command | `ask` |

The values are `auto`, `ask` and `deny`. `ask` opens a modal dialog in the
running window; with no window, no answer or a timeout the request is denied.
A tab or split that carries a command, and any workspace (which may contain
commands), counts as input, not create.

When `agent_audit` is true (default), every decision is appended as one JSON
line to `~/.local/state/oxterm/agent-audit.log`. Typed text is not written to the
log, only its length. Oxterm only transports prompts and context to the agent the
user selected: it calls no model or cloud API itself.

## The Interface

### Menu bar

- **File** manages tabs, windows, the file manager, and quitting.
- **Edit** provides clipboard actions and Preferences.
- **View** controls tabs, panes, toolbar, menu bar, scrollbar, statistics, zoom, and fullscreen.
- **Terminal** controls encoding, signals, reset, read-only mode, tab navigation, and detaching.
- **Tabs** lists open tabs and lets you switch between them.
- **Notes** opens the notes panel and the note actions.
- **Connections** opens the connections panel and the tunnel manager, and starts
  saved connections.
- **Help** opens the About dialog.

### Toolbar

The toolbar provides quick access to new tabs, new windows, splitting, copy, and
paste. It can be shown or hidden from **View** or **Preferences**.

### Terminal interaction

Oxterm uses VTE, the same terminal widget family used by GNOME Terminal. Normal
shell input, scrolling, selection, copy/paste, 256-color output, true color,
and standard terminal escape sequences are supported. What Oxterm itself
promises — and what it leaves to VTE — is recorded in
[docs/compatibility.md](docs/compatibility.md).

URLs are detected in terminal output. Click an `http://` or `https://` URL to
open it with the desktop browser. Explicit OSC 8 hyperlinks emitted by
programs (for example `ls --hyperlink=auto`) are also honored when they target
one of those two schemes; other schemes are blocked.

Source locations such as `src/window.rs:1820:34` are recognized too. Press
`Ctrl+Shift+H` and choose the label, or right-click the location and choose
**Open in Editor**. The path is resolved against the pane's working directory
and opened in a new tab with `editor_jump_command` (default
`${EDITOR:-vi} +{line} {file}`). The template supports `{file}`, `{line}`, and
`{col}`; for example, `code --goto {file}:{line}:{col}`.

Hovering over a 10- or 13-digit Unix epoch between 2000 and 2100 shows the
local time in a small bubble. On a line that mentions an error, status or
errno, hovering a known 3-digit code also explains it, for example
`HTTP 504 Gateway Timeout` or `ECONNREFUSED (errno 111): Connection refused`.
Ordinary numbers stay untouched.

Press `Ctrl+Shift+F` to search the scrollback: type a query and use `Enter` /
`Shift+Enter` to jump to the next / previous match, with optional case-sensitive
and regular-expression modes. This searches the on-screen text and scrollback,
which is different from `/history` (a database of the commands you ran).

Zoom the font at any time with `Ctrl++`, `Ctrl+-`, and `Ctrl+0` to reset.

### Command blocks and history chrome

With shell integration enabled (**Preferences > Compatibility > OSC 133**),
Oxterm records each command as a block: its command line, exit status, start
time and duration. While you scroll back into a block, a pinned header at the
top shows which command produced the visible output and how it ended (running,
successful, or failed with its exit code). When the shell integration closes a
command without reporting a status, the header says `status unknown` instead of
claiming success, and the tab failure marker is left as it was. Click the header
to jump back to the command. The scrollbar marks running commands in amber,
failures in red, and bookmarks in blue.

Press `Ctrl+Shift+G` (or use **Copy As > Open Last Output in Pager**) to open
the last command's output in a new tab with `less -R`. Set `pager_command` in
`settings.json` to use another pager. The temporary file is removed when the
pager exits.

### VI copy mode

Enable it in **Preferences > Input** and press `Ctrl+Shift+Y`. The terminal
cursor becomes a block you move with `h`, `j`, `k`, `l`, `w`, `b`, `e`, `0`,
`^`, `$`, `gg`, `G`, `{` and `}` (previous/next prompt), with optional counts
(`10j`, `3w`). `Ctrl+U`/`Ctrl+D` scroll half a page and `Ctrl+F`/`Ctrl+B` a
whole page. The view keeps a few lines of context around the cursor.

- `v` selects characters, `V` lines and `Ctrl+V` a rectangular block. Extend
  the selection with any motion; `y` yanks it to the clipboard.
- `yy`/`Y` yank the current line, and `y` followed by a motion (`yw`, `y$`,
  `yj`, `yG`, …) yanks that range.
- Text objects work in visual mode and after `y`: `iw`/`aw`, `iW`/`aW`,
  `i"`/`a"`, `i'`/`a'`, `` i` ``/`` a` ``, `i(`/`a(`, `i[`/`a[`, `i{`/`a{`,
  `i<`/`a<`. `im`/`am` use the lines between two prompt/bookmark marks.
- `mm` sets or clears a bookmark on the cursor line; `]m` and `[m` jump to the
  next and previous prompt or bookmark. Bookmarks appear as blue diamonds in
  the gutter and as blue ticks on the scrollbar.
- `/` or `?` open the scrollback search, and `Esc` leaves visual mode or the
  copy mode.

Selections are not limited to a fixed number of lines: the whole scrollback
can be yanked.

### Terminal context menu

Right-clicking a terminal opens a context menu with the actions that depend on
the current selection and cursor position. Everyday actions stay at the top
level and the rest is grouped into submenus, so the menu stays short. Items
such as **Copy Link**, **Copy As > Last Command**, and **Tools > Insert
Snippet** only appear when there is something to act on.

- Top level: **Copy**, **Paste**, **Paste Selection**, and **Select All**,
  followed by **Copy Link** / **Open Link in Browser** when the cursor is over
  a link, or **Open in File Manager** / **Copy Path** when the cursor is over
  an existing path (or the current selection is one), or **Open in Editor** /
  **Copy Location** when it is over a `file:line[:col]` location.
- **Copy As**: **HTML**, **Whole Scrollback**, and, when available, **Last
  Command** and **Last Command Output**.
- **Notes**: **Add Selection to Note...**, **Note Command Output...**,
  **New Note...**, **Toggle Notes Panel**, and **Open Notes Folder**.
- **Tab**: **New Tab**, **Rename Tab...**, **Detach Tab**, **Close Tab**, and a
  **Split** submenu (vertical, horizontal, unsplit).
- **Tools**: **Search Scrollback...**, **Command Bar...**, **History
  Search...**, a **Quickmarks** submenu, **Insert Snippet** (saved `/snippet`
  entries), **Open File Manager Here**, **Reset Terminal**, and **Clear
  Scrollback**.
- **View**: **Zoom In/Out/Reset** and **Full Screen**.
- Bottom toggles: **Broadcast Input** and **Read-Only**.

## Tabs, Windows, and Splits

### Tabs

Use **File > New Tab** or `Ctrl+Shift+T`. Tabs can be reordered, renamed, moved
between panes, detached into a window, or closed independently.

Rename a tab with `Ctrl+Shift+S`, by double-clicking its label, or with
**Rename Tab...** in the tab's right-click menu. A custom name is preserved
when the shell changes the terminal title; use **Reset Tab Title** in the same
menu to return to automatic titles. Long titles are ellipsized so a tab never
forces the window (or a split pane) wider than the terminal; hover the label to
see the full title.

`View > Always Show Tabs` controls whether the tab bar remains visible when only
one tab is open.

**Tabs > Move Tab from Another Oxterm…** lists the tabs of the other Oxterm
windows running for the same user and adopts the one you pick: the tab opens
here, showing the running session, and the tab (or the window, when it was the
last one) closes in the other Oxterm. The shell, its process and the recent
output move with it; nothing is restarted. The adopted tab is a relay to the
session, so it is exactly as wide, as fast and as stateful as the terminal it
came from, and it keeps the OSC 133 shell integration when the source window had
it: command blocks, history and notifications continue here. Closing the adopted
tab ends that session, as closing any other tab does; it does not go back to the
window it came from.

**Session switcher** (`Ctrl+Shift+O`, or `Ctrl+B` then `s`/`w`) searches across
the panes of every local Oxterm window, panes owned by other Oxterm processes,
and broker sessions with no local view. Press Enter to focus a local or foreign
pane, or to attach a detached session in a new tab. Filtering matches the title,
pane id, directory, and last command; foreign panes are focused through the
control socket and are not moved.

### Windows

Use **File > New Window** or `Ctrl+Shift+N`. Each launch is an independent
application window; `--new-window` makes this explicit.

### Split panes

**View > Split** provides:

- **Single**: one terminal pane.
- **Vertical**: two panes arranged left and right.
- **Horizontal**: two panes arranged top and bottom.

Use `Ctrl+Alt+PageUp` or the Terminal menu to switch the active pane.

## Built-in Commands

Built-in commands start with `/` and are processed by Oxterm instead of the
shell. Press `Ctrl+Shift+P` for the global command palette: it fuzzy-searches
these `/commands` alongside direct actions (new tab, splits, zoom, hints, VI
copy, pager, tunnels, broadcast, session switcher, SSH picker and more). Use
`Tab` to complete the highlighted row and `Enter` to run it; typing `/`
narrows the list to built-in commands.

### `/help`

Prints a concise command and shortcut reference.

### `/history`

Searches the SQLite history database:

```text
/history
/history ssh
/history git push
/history ssh -private
/history :sql SELECT * FROM commands WHERE exit_code != 0
```

Terms are combined with AND logic. Prefix a term with `-` to exclude it. The
`:sql` form accepts read-only `SELECT` and `EXPLAIN` queries.

When OSC 133 shell integration is enabled, each tracked command also records its
duration and the current Git branch when the working directory is a repository.

Results are numbered. Press `Alt+1` through `Alt+9` to replay a result, or use
the interactive picker to fill or execute it.

### `/session`

Named sessions are saved from the **File > Sessions** menu. Export one for
backup or sharing with:

```text
/session list
/session export work
/session export work ~/backups/oxterm-work.json
```

Name a live remote terminal with:

```text
/session name build-server
```

Exports contain session layout and tab working directories, not terminal output.
The destination is written atomically with private file permissions.

### `/snippet`

Snippets are persistent parameterized command templates. Positional placeholders
use `{1}` through `{9}`; `{*}` expands to all arguments:

```text
/snippet add deploy docker compose -f {1} up -d {2}
/snippet deploy compose.yml api
/snippet list
/snippet delete deploy
```

Expansion fills the shell prompt but does not press Enter, so the command can be
reviewed or edited before execution.

### `/wnotes` and `/onotes`

Save or open Markdown notes:

```text
/wnotes Deploy completed on staging
/wnotes -project.md TODO: review the migration
/onotes
/onotes -project.md
```

The notes directory, default file, and editor are configurable. Selected
terminal text can also be added to a note from the context menu.

### `/learn`

Imports commands from a text file into history without executing them:

```text
/learn commands.txt
/learn ~/snippets/deploy.sh
```

Blank lines and comments are ignored. The importer limits the number and length
of lines to avoid treating pasted output as commands.

### `/optimize history`

Runs SQLite maintenance in the background. It keeps the most recent entry for
each command/directory pair, checkpoints the WAL, updates query statistics, and
vacuums the database. The terminal remains usable while it runs.

### `/ssh`

Opens a saved connection or a host from `~/.ssh/config`. Without a name it opens
the host picker. See [Saved connections](#saved-connections).

### `/clear` and `/cls`

Clear the visible screen; the scrollback is left intact.

## History

Oxterm records the command, working directory, timestamp, and exit status in
`~/.config/oxterm/history.db`.

Press `Ctrl+R` for reverse interactive search:

- Type to filter.
- Use the arrow keys to select.
- Press `Ctrl+R` again to select the next match.
- Use `Ctrl+W` to delete the last filter term or `Ctrl+U` to clear the filter.
- Press Enter to use the selected command.
- Press Escape or `Ctrl+C` to cancel and preserve the current prompt.

Filtering runs outside the terminal UI thread, so typing stays responsive even
when the history database is large.

The `Tab` key can open the history picker when shell completion does not change
the current input. Selecting a result fills the command line without executing
it immediately.

## Notes

Notes are Markdown files with timestamped entries (in local time). They are
stored, by default, in `~/.local/share/oxterm/notes/`; change the directory and
default filename in **Edit > Preferences > Notes**.

The **Notes** menu lets you:

- **Notes Panel** — show or hide the integrated panel (search, tag filter,
  preview and a quick append box).
- **New Note...** — compose a note and choose which file to save it to.
- **Add Selection to Note...** — append the selected terminal text to a note.
- **Copy All to Note...** — append the whole terminal buffer after a confirmation.
- **Note Command Output...** — append the last command output (requires OSC 133
  shell integration), recording the command and its exit status.
- **Open Default Note** / **Open Note...** — open a note in the editor.
- **Open Notes Folder** — open the notes directory in the file manager.

**Notes panel.** The panel indexes every note with SQLite FTS5, so search is
full-text across all entries. Use the search box to filter, the tag selector to
restrict to a `#tag`, the list to browse, the preview to read an entry, and the
bottom box to append a new entry to the selected (or default) note. Entries with
`#tags` are indexed automatically.

Each entry can record a small context comment (source, working directory, host,
and for command output the command and exit status), which can be turned off in
**Preferences > Notes**. A **template** (with `{date}`, `{time}`, `{cwd}`,
`{host}`, `{command}` placeholders) can pre-fill new notes. The same actions are
also available from the terminal context menu.

From the prompt, `/wnotes` appends and `/onotes` opens a note:

```text
/wnotes Deploy completed on staging
/wnotes -project.md TODO: review the migration
/onotes
/onotes -project.md
```

The editor preference is used as a fallback when the desktop `xdg-open` helper
is unavailable.

## Latest Development Package

Every successful push to the `master` branch publishes a rolling prerelease:

<https://github.com/buzzqw/oxterm/releases/tag/latest>

It contains the current Linux executables (`oxterm`, `oxterm-broker` and
`oxterm-askpass`), a tarball with all three, and a `SHA256SUMS` file.
This package is for testing and is replaced by the next successful build. Use a
versioned release for a stable installation.

## Preferences

Open **Edit > Preferences**. Settings are written when you press **OK**.

Type in the filter at the top (or press `Ctrl+F`) to hide every setting that
does not match: the dialog jumps to the first page with a hit and shows how
many settings were found. Each setting carries its explanation under the name,
so the common options do not require hovering.

Pages keep their changes pending until you press **OK**; **Cancel** discards
them. **Restore Defaults** puts every page back to the documented defaults
without writing anything, so it can still be cancelled.

### General

Configure the default and dynamic tab title, login shell, custom shell command,
terminal dimensions, session restore, SSH connection sharing, close
confirmation, selection copying, unsafe-paste warnings, and the file manager.

### Appearance

Choose the font and size, bold text, color scheme, automatic light/dark theme,
foreground, background, cursor and selection colors, tab colors, cursor shape
and blinking, transparency and opacity, padding, and underline style.

### Colors

Edit all 16 ANSI palette entries individually; the current hex value is shown
under each swatch. **Load Preset...** fills the palette from any installed or
imported theme, and **Reset to Theme** restores the colors of the selected
scheme. Nothing is written until you press OK.

### Scrolling

Configure the scrollbar position, the scrollback size (or unlimited), scrolling
on output and on keystroke, and smooth scrolling.

### Input

Configure hint mode, VI copy mode, the tmux-style prefix key, the Backspace and
Delete sequences, and the characters treated as part of a word by double-click
selection.

### Notifications

Configure desktop notifications when a command finishes, whether to notify only
while the terminal is in the background, the minimum command duration, and
failure notifications. These options rely on OSC 133 shell integration.

### Compatibility

Configure the terminal encoding, bold-as-bright text, Sixel images, and OSC 133
shell integration. Supported encodings include UTF-8, ISO-8859 variants, UTF-16
variants, CP1252, CP850, ASCII, KOI8-R, Shift_JIS, EUC-JP, and GBK. A button at
the bottom restores only the compatibility options.

### Notes

Configure the notes directory (empty means `~/.local/share/oxterm/notes`), the
default Markdown filename, whether entries record terminal context, a template
pre-filled in new notes, and the fallback editor.

## Shell Integration

OSC 133 lets Oxterm identify prompt start, command start, command output, and
exit status. It enables prompt navigation and command-output-aware features.

Enable **Preferences > Compatibility > OSC 133**. Oxterm loads the integration
automatically in new Bash, Zsh, and Fish shells started by the application. It
creates temporary startup files and removes them when the session ends; `.bashrc`,
`.bash_profile`, `.zshrc`, and `.zprofile` are not modified.

The integration supports Bash, Zsh, and Fish. Use `Ctrl+Shift+Up` and
`Ctrl+Shift+Down` to move between detected prompts.

The scripts write one event per line (`A` prompt start, `C<command>` command
start, `D<status>` command end) to a private FIFO; Oxterm never reads the
markers back from the screen. The wire format, the size limits, the known
divergences and how they are tested are documented in
[docs/compatibility.md](docs/compatibility.md).

The directory the shell reports through `OSC 7` appears in the tab tooltip and,
when it differs from the directory of the shell process, at the left of the
status bar. It is display-only: history entries, notes and the directory for new
tabs keep reading `/proc`, because a program can print any `OSC 7` it likes
while `/proc` cannot lie about the process.

### Saved connections

Everything starts from the **Connections** menu, to the right of **Notes**:

1. Choose **New Connection...**, fill in a name and a host, and press **Save**.
   Hosts already in `~/.ssh/config` need no setup: they are listed by themselves.
2. Choose the connection from the **Connections** menu, or open **Connections Panel**
   (`Ctrl+Shift+J`) and press `Enter` on it.

The panel lists every saved connection and every host in `~/.ssh/config`, grouped
into Favorites, Recent, your own groups, Ungrouped and `~/.ssh/config`. Type to
search names, hosts, users, groups, tags and notes. Right-click a group for
**Open All**. The **Connections** menu also lists favorites and recent
connections, and the current connection's macros.

- `Enter` or a double click opens a tab, `Ctrl+Enter` a split. The context menu
  also offers a new window, `sftp`, tunnels, copying the `ssh` command, favorites,
  editing, and deleting. `Ctrl+N` creates an entry, `F2` edits, `Delete` deletes,
  and `Esc` or `Ctrl+Shift+J` closes the panel.
- The command palette (`Ctrl+Shift+P`) finds connections as you type, and
  `/ssh NAME` at a local prompt opens one. `/ssh` alone opens the host picker.
- **Color scheme** (Advanced page) gives an entry's tabs their own colors, so a
  server is recognized at a glance. Only those tabs change.
- **New** creates an entry with its own host, user, port, key and `ProxyJump`. For
  a `~/.ssh/config` host, **Edit** only adds a group, tags, notes, tunnels and the
  like. Oxterm never writes `~/.ssh/config`, so OpenSSH stays in charge of it.
- The form always shows the exact `ssh` command the entry produces.
- Every row starts with a dot: green `●` and a bold name when the connection is
  open in a tab of any of this Oxterm's windows (with the count when there are
  several), a faint `○` otherwise. A blue `⇄ N` badge counts the tunnels that
  listen or run, an amber `⇄ N…` those still starting, a red `⚠ N` those that
  failed. Groups add up their connections, so a folded group still shows what
  runs inside, and the tooltip lists every tunnel with its state. **Active** next
  to the search box keeps only open connections and those with tunnels, and the
  line at the bottom sums it all up ("2 connected · 3 tunnels up · 1 failed").
- A connection's tab keeps its name even when the remote shell sets a title, and
  the production highlight also looks at that name, since an address rarely says
  `prod`.
- A tab moved to a separate window, or to another Oxterm, stays that connection.
  To bring a detached tab back, choose **Return to Main Window** in its **File**
  menu or right-click menu, or **Bring Back** in the main window's **Tabs** menu.
  The session carries on untouched.
- **Ask before connecting** adds a confirmation. After a restart, tabs that held
  a connection are never reopened silently: Oxterm asks first, then reopens them in
  their old order and on their old side of a split.

Everything is an ordinary Oxterm terminal running your `ssh`, with the same
connection sharing and keepalives as the shell wrapper.

The book is `~/.config/oxterm/connections.json` (mode `0600`). It holds no
secrets, so it can be copied or versioned; usage counts live in
`connections-state.json`. A damaged file is reported and never overwritten, and a
file from a newer Oxterm opens read-only.

**Other connections.** The **Protocol** field of a new entry offers more than SSH:

| Protocol | Runs | Where |
| --- | --- | --- |
| Mosh, Telnet | `mosh`, `telnet` | Tab |
| Serial port | `picocom` | Tab |
| Docker, Kubernetes, AWS Session Manager | `docker exec`, `kubectl exec`, `aws ssm start-session` | Tab |
| Any command | The command you enter, without a shell | Tab |
| SFTP, FTP | `sftp`, `lftp` | File browser |
| RDP | FreeRDP or `rdesktop` | A tab, and its own window |
| VNC | TigerVNC's `vncviewer`, or `remote-viewer` | A tab, and its own window |
| SPICE | `remote-viewer` | A tab, and its own window |

Tab connections keep macros, automation, logs and colors. A missing program is
reported by name. A desktop entry with **ProxyJump** goes through that host in an
ssh tunnel.

**VNC with a server on the far side.** The **VNC server** field can start one for
you, so you only need ssh access:

| Choice | Server | What you get |
| --- | --- | --- |
| Share the screen: detect | wayvnc if a Wayland session exists, then x11vnc, then x0vncserver | The remote screen |
| Share the screen: x11vnc, x0vncserver, wayvnc | That server | The remote screen |
| Virtual desktop: TigerVNC, TurboVNC, TightVNC | `vncserver`, and so on | A separate desktop that keeps running |
| Already running | None started | A tunnel to the port you give |
| My own command | Your command, with `{port}` | Whatever it starts |
| None | None started | A direct connection to `host:port`, without ssh |

The server listens on the remote loopback only and is reached through an ssh tunnel,
so nothing is exposed on the network. Both the ssh and the viewer run in tabs: you
can answer a password, host-key or certificate question there, and read what the
server or viewer has to say. Closing the viewer stops a shared screen and closes
the tab, and closing the tab closes the viewer.

- **Sharing a screen** uses a password made up for that session. It is typed into the
  tab with echo off and given to the viewer in its private environment, so it never
  appears on a command line. With a viewer other than TigerVNC's, the tab shows it for you to
  type.
- **RDP passwords** from the keyring are typed at the viewer's own `Password:` prompt,
  with echo off; a changed certificate is shown in the tab and waits for your answer,
  as it would from a shell.
- **wayvnc** has no password in this mode: it relies on the loopback and the tunnel,
  so other accounts on that server could reach it while it runs.
- **A virtual desktop** outlives the connection and uses the password you set with
  `vncpasswd` on the server. The tab says how to stop it.

**File browser.** An SFTP or FTP entry opens a two-pane window, with this computer on
the left and the server on the right. **Browse Files...** in the panel's context menu
or the **Connections** menu does the same for any SSH entry, reusing an open
connection so no new login is needed. Right-click an SSH tab and choose
**Browse Remote Files...** to do it from the tab itself; a raw `ssh user@host`
session works too, without a saved entry.

- Double-click a folder to enter it, or a file to copy it to the other side.
- Use **Upload** and **Download**, or drag between the panes. You can also drop files
  from a file manager onto the server's pane.
- **Sync →** and **← Sync** copy the whole current folder tree, recursively,
  overwriting existing files and keeping extra files in the destination. The
  transfer goes through `sftp`/`lftp`, so it reuses the connection's own
  authentication.
- **Bookmark** saves the current folder; **Bookmarks** jumps back to it. Local
  bookmarks are shared by every browser, remote ones belong to the connection.
  They live in `~/.config/oxterm/file_bookmarks.json`.
- **Edit** on a remote file downloads it to a private cache copy, opens it with
  the desktop editor (`xdg-open`), and uploads it back every time it is saved.
  The temporary copy stays in `~/.cache/oxterm/remote-edit/`.
- **Password...** stores a password for this host in the system keyring and
  reconnects. The browser itself cannot ask questions, so a host that only
  accepts a password needs this button (or a key, an agent, or an already open
  shared connection).
- Copies run one at a time in the background, folders included. Oxterm asks before
  replacing an existing name; a sync asks once for the whole folder. Browsing is
  much faster with SSH connection sharing: if the entry has **Do not share this
  connection** checked, every folder and transfer starts a new SSH session.
- **New Folder**, **Rename** (`F2`), **Delete** (`Delete`), **Refresh** (`F5`) and
  **Up** (`Backspace`) work on both sides.
- `oxterm browse NAME` opens the same browser from a terminal, which is handy
  from a script or an agent.
- Browsing cannot ask questions: SFTP needs a key, an agent, a password in the keyring
  or an open tab to that host. Plain FTP sends the password unencrypted, so prefer
  **Use TLS** or SFTP.

**What an entry can do.** The form has these pages:

| Page | Options |
| --- | --- |
| Connection | Name, protocol, host, user, port, `ProxyJump`, group, tags, favorite, notes |
| Service | What only the selected protocol needs; hidden when there is nothing |
| Authentication | Key (those in `~/.ssh` are listed), stored password, one-time codes |
| SSH | X11 (`-X`), compression (`-C`), agent forwarding (`-A`), keepalive, connect timeout, unknown-host policy, `TERM`, no connection sharing, extra arguments, old algorithms for aged servers |
| Session | Remote command (and staying in a shell after it), a local command to run first such as bringing up a VPN, Wake-on-LAN, automatic reconnection, recording the output to a file, asking before connecting |
| Tunnels | Saved local, remote and SOCKS tunnels |
| Macros | Named commands, run from the terminal's right-click menu or the **Connections** menu |
| Automation | Type a command when a pattern appears in the terminal |
| Appearance | Color scheme, font, scrollback and encoding for this entry's tabs |

- **Reconnection** happens only when `ssh` exits with 255, meaning the connection
  failed or dropped: `exit` or a failed last command closes the tab as usual. It
  gives up after 20 attempts in a row, and closing the tab stops it.
- **Session logs** go to a new private file per connection, with the terminal's
  raw output: read them with `less -R`. The PTY broker writes them, so they keep
  going if the window closes.
- **The local command** runs without a shell: quotes work, `;` and `$()` do not. A
  failure is reported, and the connection is still attempted.
- **Automation rules** react to what the server prints, so a server can trigger
  them: never put a secret in one.
- **Keys.** **Generate SSH Key...** in the **Connections** menu runs `ssh-keygen` in a tab.
  **Copy Public Key to Server** in the panel's context menu runs `ssh-copy-id`.

**Passwords.** Keys and an agent are the better choice. Where a server only takes
passwords, set the entry's method to **Password in the system keyring**: it is
stored with `secret-tool`, never in Oxterm's files. `oxterm-askpass`, installed
next to `oxterm`, answers only the exact `user@host's password:` prompt of that
entry, or OpenSSH's `(user@host) Password:` for keyboard-interactive servers, and
only once per `ssh`, so a wrong password cannot lock the account. A
jump host, a generic `Password:`, passphrases and host-key questions are always
asked in the terminal. **Answer one-time codes** does the same for 6-digit,
30-second codes: paste the secret or `otpauth://` link, which goes to the keyring,
and Oxterm answers a code prompt only when it names this user and host. Without `secret-tool` or the helper,
`ssh` simply prompts as usual.

**Tunnels.** An entry can save local (`-L`), remote (`-R`) and SOCKS (`-D`)
tunnels, started from the context menu, automatically with the connection, or
with `oxterm tunnel`. Each runs its own background `ssh`, whose real state is
shown: *listening* only once the local port answers, *running* for a remote
forward, and *failed* with `ssh`'s own error. It cannot prompt, so it needs a key,
an agent, or a stored password, and a host already in `known_hosts`. Tunnels stop
with the application, even if it crashes.

**Tunnel manager.** **Tunnels** in the connections panel, **Tunnel Manager...** in
the **Connections** menu or in a connection's **Tunnels** submenu, or *Tunnel
manager* in the command palette opens a window with the tunnels of every
connection: state, connection, type, listen address, target, autostart and how
long each has been up. **Add**, **Edit**, **Duplicate** (on the next free local
port) and **Delete** change the tunnels saved in the connection, **Start**,
**Stop**, a double click or `Enter` control one, **Start All** and **Stop All**
everything the filter shows. The log below shows each `ssh` command, what it
printed and how it ended. A remote desktop's own tunnel is listed too while it
runs. Closing the window leaves the tunnels running.

**From the command line** (no `DISPLAY` needed to list, export, or import):

```bash
oxterm connect db-prod-1            # new tab; prints the pane id
oxterm connect db-prod-1 --split
oxterm connections                  # or --json
oxterm connections export > book.json
oxterm connections import book.json # only ever adds entries
oxterm tunnel list
oxterm tunnel start db-prod-1 0
oxterm tunnel stop db-prod-1
```

An import gives every entry a new id and never replaces an existing one. An entry
that would act by itself is skipped unless you pass `--allow-local-commands`:
extra arguments such as `ProxyCommand`, a pre-connect command, automation rules, a
log folder, a command entry, a custom VNC server command, or a shared folder.

### SSH connections

**Host picker.** `Ctrl+Shift+K` lists your saved connections and the concrete
`Host` entries from `~/.ssh/config`, including files pulled in by `Include`;
wildcard and negated patterns are skipped. Pick one (or type `user@host`) and
**Connect** opens a new tab running `ssh`. **Copy Integration Command** copies an explicit
`ssh host '…'` command that installs the OSC 133 script on the remote bash/zsh
account; run it yourself when you want it. Oxterm never injects shell
integration into a session automatically. Oxterm reads command events only from a
private local FIFO, which a remote shell cannot reach, so this script gives you
the remote prompt's `OSC 7` directory, not remote entries in the history, the
command blocks, or failure notifications.

Hosts matching `ssh_alert_patterns` (by default names containing `prod`,
`production`, `prd` or `live`) are shown in `ssh_alert_color` with a bold tab
title and a *Production host* tooltip; set the pattern to an empty string to
disable the badge. The tab tooltip also lists the session's `-L`, `-R` and `-D`
forwards, and **SSH Tunnels...** in the terminal context menu opens the same
list in a dialog. It shows what was configured, not the live health of each
port.

With shell integration active, `ssh` typed at the prompt goes through a small
wrapper that:

- shares one authenticated connection per server, port, and user across all
  tabs (`ControlMaster=auto`, socket in `~/.config/oxterm/cm-<hash>`, kept for
  10 minutes after the last session closes). A second tab to the same server
  opens instantly without logging in again;
- enables keepalives (`ServerAliveInterval=30`) when your configuration sets
  none, so a dropped connection ends the session instead of freezing the tab.

Your own `~/.ssh/config` always wins: if it sets `ControlPath` or
`ServerAliveInterval`, Oxterm does not override it. Turn sharing off under
**Preferences > General > SSH connection sharing** (applies to new tabs).

`autossh` gets the same treatment (its own `-M`, `-f`, and `-V` options are kept
as they are). `mosh` is left untouched: in its default mode it disables ssh
connection sharing itself, so Oxterm only recognises mosh sessions.

Prefer `ProxyJump` over agent forwarding: the bastion never receives your SSH
agent socket. The host picker warns when the effective configuration enables
`ForwardAgent yes` for the selected host. For hardware-backed keys, generate a
FIDO2 resident key with
`ssh-keygen -t ed25519-sk -O resident -O verify-required`; Oxterm simply runs
`ssh`, so any key the agent offers works.

The status bar shows the remote host's CPU, memory, and disk usage for `ssh`,
`autossh`, and `mosh` sessions, plus the directory a shell announces with
`OSC 7` when it differs from the local process directory. Oxterm reads the
statistics through a shared connection to that host only (for mosh, one opened
by another tab); it never opens extra logins, so hosts without a shared
connection simply show no remote statistics.
`Ctrl+B`, then `d` is passed through to the remote side for all three, so a
remote tmux detaches instead of the local terminal.

## Keyboard Shortcuts

<!-- BEGIN GENERATED KEYBINDS -->
| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Shift+N` | New window |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+Q` | Close window |
| `Ctrl+Shift+C` | Copy |
| `Ctrl+Shift+V` | Paste |
| `Ctrl+Shift+A` | Select all |
| `Ctrl+Shift+S` | Rename tab |
| `Ctrl+Shift+R` | Reset terminal |
| `Ctrl+Shift+X` | Reset and clear |
| `Ctrl+Shift+F` | Search the scrollback (Enter/Shift+Enter = next/prev) |
| `Ctrl++ / Ctrl+- / Ctrl+0` | Zoom font in / out / reset |
| `Ctrl+Shift+E / Ctrl+Shift+D` | Vertical / horizontal split |
| `Ctrl+Shift+M / Ctrl+M` | Set / jump to next quickmark |
| `Ctrl+Shift+H` | Hint mode for URLs, paths, file:line locations and Git SHAs |
| `Ctrl+Shift+Y` | VI-style copy mode |
| `Ctrl+Shift+G` | Open the last command's output in a pager |
| `Ctrl+Shift+B` | Toggle broadcast input |
| `Ctrl+R` | Interactive history search; repeat to cycle matches |
| `Ctrl+U` | Kill line; clear history filter while its picker is open |
| `Ctrl+W` | Kill word; remove last history filter term while its picker is open |
| `Ctrl+L` | Clear screen |
| `Ctrl+C` | Interrupt or cancel history mode |
| `Ctrl+D` | EOF; closes an empty shell tab on exit |
| `Ctrl+PageUp / Ctrl+PageDown` | Previous / next tab |
| `Ctrl+Shift+PageUp / Ctrl+Shift+PageDown` | Move tab |
| `Ctrl+Alt+PageUp` | Switch split pane |
| `Ctrl+Shift+Up / Ctrl+Shift+Down` | Previous / next prompt |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+Shift+O` | Searchable session switcher |
| `Ctrl+Shift+K` | Connect to a saved connection or a host from ~/.ssh/config |
| `Ctrl+Shift+J` | Show or hide the connections panel |
| `Ctrl+B, then c` | New tab (tmux-compatible) |
| `Ctrl+B, then n / p` | Next / previous tab |
| `Ctrl+B, then % / "` | Vertical / horizontal split |
| `Ctrl+B, then o` | Switch split pane |
| `Ctrl+B, then z` | Zoom the focused pane to fill the window, repeat to restore |
| `Ctrl+B, then =` | Equalize the split panes |
| `Ctrl+B, then s / w` | Show the interactive terminal list |
| `Ctrl+B, then d` | Detach the local terminal PTY; passed through when ssh is foreground |
| `Ctrl+B, then x` | Close tab |
| `Alt+1 ... Alt+9` | Replay history entry |
| `Ctrl+Shift+. / Ctrl+Shift+I` | Show or hide the agent pane / focus it (when agent_enabled is on) |
| `Ctrl+Alt+.` | Ask the agent to fix the last failed command (when agent_enabled is on) |
| `F11` | Fullscreen |
| `Click` | Open a detected URL |
<!-- END GENERATED KEYBINDS -->

The tmux-style prefix defaults to `Ctrl+B`. Change it under Preferences →
Input → "tmux-style prefix keys" and the "Prefix key (Ctrl+)" field, for
example to `Ctrl+A`. Any other key after the prefix is passed to the running
program together with the prefix. When tmux or screen runs in the foreground of
a pane, the prefix is passed to the multiplexer so its own bindings keep working
(`Ctrl+B`, then `w` lists its windows); a multiplexer reached over ssh is not
detected, so use a different prefix for those sessions.

## Configuration Files

```text
~/.config/oxterm/settings.json
~/.config/oxterm/settings.json.bak
~/.config/oxterm/history.db
~/.config/oxterm/notes.db
~/.config/oxterm/sessions/
~/.config/oxterm/profiles/
~/.config/oxterm/connections.json
~/.config/oxterm/connections-state.json
~/.config/oxterm/file_bookmarks.json
~/.config/oxterm/triggers.json
~/.config/oxterm/extensions.json
~/.config/oxterm/remote/
~/.config/oxterm/themes/
~/.local/share/oxterm/notes/
```

Broker sockets move to `/tmp/oxterm-<uid>/remote/` when the configured path is
too long for a Unix socket.

The directory is shared with the original application by design. Oxterm keeps
the previous valid settings file as `settings.json.bak` when replacing
Preferences. Back up the whole directory before manually editing settings or
migrating between versions.

Every setting, its type, range and default is listed in
[docs/settings.md](docs/settings.md). The same information is available from a
terminal without opening a window:

```text
oxterm --list-settings
oxterm --show-config                 # effective values as JSON
oxterm --show-config --default       # built-in defaults
oxterm --validate-config FILE        # check a file before using it
oxterm --list-themes
oxterm --list-keybinds
oxterm --list-actions
oxterm --diagnostics
```

`--validate-config` reports unknown keys as warnings and invalid values as
errors, with the expected type or range for each key. `--show-config` applies
`-o KEY=VALUE` overrides but never writes them back. `--diagnostics` prints
versions, paths, file permissions and the enforced limits, never settings
values, so it is safe to include in a bug report.

### Notifications and failure markers

Enable **Preferences → Notifications → Notify when a command finishes** (requires
OSC 133 shell integration) to receive a desktop notification when a command
finishes. Notifications are intentionally selective:

- nothing while the terminal has keyboard focus;
- successful commands shorter than the minimum duration (10 seconds by default)
  are ignored;
- failures always notify, even when they finish immediately;
- a command whose status the shell did not report is not treated as a failure,
  and its notification says that no status was reported.

The notification names the command, exit status, duration and working
directory. Tune it with `bell_notification`, `notify_only_unfocused`,
`notify_min_seconds` and `notify_on_failure`.

A red dot appears on the tab when its last command failed and you were not
looking at that tab. Selecting the tab, or running a command that succeeds,
clears it.

### Themes

Oxterm ships 247 themes embedded in the binary, so there is nothing to
download or install. `oxterm --list-themes` shows the full list and
**Preferences → Appearance** selects one.

Custom themes are optional: copy a Ghostty-style theme file or an iTerm2
`.itermcolors` file into `~/.config/oxterm/themes/`; the file name becomes the
theme name. Per-theme licences for the bundled collection are recorded in
`themes/licenses.md`.

To follow the desktop's light/dark preference automatically, enable
**Preferences → Appearance → Automatic theme** (or set `auto_theme` to `true`)
and choose the light and dark themes underneath. Open terminals are recolored as
soon as the desktop switches.

### Compatibility

The Compatibility page also controls rendering details that differ between
terminals:

- **Bold is bright** — bold text picks the bright palette color, as in xterm.
- **Sixel images** — allow inline images from applications such as `lsix`,
  `chafa` or `img2sixel` (off by default because images use memory).
- **Smooth scrolling** — scroll by pixels instead of whole lines.
- **Word characters** — extra characters treated as part of a word when
  double-clicking, so paths and URLs are selected whole.

### Utilities

These commands open no window and are safe over SSH:

```text
oxterm --edit-config        Edit settings.json in $EDITOR
oxterm --list-fonts         List font families for font_name
oxterm --list-themes        List installed themes
oxterm --list-actions       List scriptable control actions
oxterm --diagnostics        Print versions, paths and limits
```

## Troubleshooting

### The application does not build

Verify that Rust, `pkg-config`, GTK3, and VTE development packages are installed:

```bash
pkg-config --modversion gtk+-3.0
pkg-config --modversion vte-2.91
cargo --version
```

For development, run the complete verification suite from the repository root:

```bash
cargo fmt --check
cargo test --all-targets --locked
cargo build --release --locked
```

The integration test `tests/remote_broker.rs` starts a real broker process with
two PTYs and verifies socket permissions, metadata commands, multiple attached
clients, input/output forwarding, local forwarding toggles, detach, and broker
cleanup. Run it directly with:

```bash
cargo test --test remote_broker -- --nocapture
```

### The application starts with the wrong colors or encoding

Open Preferences and reapply the desired scheme or encoding. Encoding can also
be changed for the active tab from **Terminal > Set Encoding**.

### Shell integration is not visible

Confirm that OSC 133 is enabled and start a new terminal tab. Existing tabs do
not receive the integration retroactively.

### Reset all settings

Close Oxterm and back up or remove the configuration directory:

```bash
mv ~/.config/oxterm ~/.config/oxterm.backup
```

The next launch creates a fresh configuration. The same directory contains the
history database, so keep the backup if you want to restore history later.

## License

Oxterm is licensed under the European Union Public Licence 1.2. See [LICENSE](LICENSE).
