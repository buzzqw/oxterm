# Oxterm User Manual

## Contents

1. [Starting Oxterm](#starting-oxterm)
2. [The interface](#the-interface)
3. [Tabs, windows, and splits](#tabs-windows-and-splits)
4. [Built-in commands](#built-in-commands)
5. [History](#history)
6. [AI chat](#ai-chat)
7. [Notes](#notes)
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
the system desktop launcher, and the Oxterm bull icon. It uses `sudo` only when
needed. Run it again after compiling a newer version to update the installed
command and launcher. The installer removes the legacy `/usr/bin/TRust`, its
desktop entry, and its icon when present, but never touches lowercase `trust`,
which is the unrelated `p11-kit` certificate utility. For a per-user
installation, use `./setup.sh` instead.

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
oxterm -- ~/dir-with-dashes        # everything after -- is treated as a directory
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
usable, and more than one remote client can attach to the same session. Client
disconnects do not stop the shell or broker, so a later `-a SESSION_ID` can
reconnect. An unexpected GUI crash also leaves the broker and shell running;
an intentional GUI close follows the normal tab cleanup path and terminates
its shell.

With one active terminal, `oxterm -a` attaches immediately. With multiple active
terminals it displays a numbered chooser. Use `oxterm -a SESSION_ID` to bypass
the chooser.

Use `/session name NAME` in a live terminal to assign a readable name. Use
`oxterm --detach SESSION_ID` from another SSH shell to detach all remote clients
without stopping the broker. `Ctrl+B`, then `d` turns local forwarding off;
`Ctrl+B`, then `s` or `w` and selecting the terminal turns it on again.

The broker socket uses a `0700` directory and `0600` socket under
`~/.config/tpgk/remote/`. The internal `oxterm --broker SOCKET SESSION_ID` mode
owns the child PTY and accepts length-prefixed Unix-socket frames for
`LIST`, `INFO`, `ATTACH`, `DETACH`, `RENAME`, `COMMAND`, `LOCAL_ON`, `LOCAL_OFF`,
and `KILL`. The GUI PTY is a separate endpoint, allowing its local forwarding
to be disabled without releasing the shell session. A reconnect receives output
from that point onward; Oxterm does not persist or replay terminal scrollback.
When the shell exits, the broker removes the session socket.

The listing columns are `ID`, `NAME`, `TITLE`, `DIRECTORY`, `STATUS`,
`APPLICATION`, and `APP_STATUS`. The application column contains the latest
shell command; `APP_STATUS` is `running` while it is active and `last` after it
has completed.

The Oxterm binary running the GUI must already include this feature. An Oxterm
process started with an older binary cannot be discovered retroactively because
it never created the private session socket.

## The Interface

### Menu bar

- **File** manages tabs, windows, the file manager, and quitting.
- **Edit** provides clipboard actions and Preferences.
- **View** controls tabs, panes, toolbar, menu bar, scrollbar, statistics, zoom, and fullscreen.
- **Terminal** controls encoding, signals, reset, read-only mode, tab navigation, and detaching.
- **Tabs** lists open tabs and lets you switch between them.
- **Help** opens the About dialog.

### Toolbar

The toolbar provides quick access to new tabs, new windows, splitting, copy, and
paste. It can be shown or hidden from **View** or **Preferences**.

### Terminal interaction

Oxterm uses VTE, the same terminal widget family used by GNOME Terminal. Normal
shell input, scrolling, selection, copy/paste, 256-color output, true color,
and standard terminal escape sequences are supported.

URLs are detected in terminal output. Click an `http://` or `https://` URL to
open it with the desktop browser. Explicit OSC 8 hyperlinks emitted by
programs (for example `ls --hyperlink=auto`) are also honored when they target
one of those two schemes; other schemes are blocked.

Press `Ctrl+Shift+F` to search the scrollback: type a query and use `Enter` /
`Shift+Enter` to jump to the next / previous match, with optional case-sensitive
and regular-expression modes. This searches the on-screen text and scrollback,
which is different from `/history` (a database of the commands you ran).

Zoom the font at any time with `Ctrl++`, `Ctrl+-`, and `Ctrl+0` to reset.

## Tabs, Windows, and Splits

### Tabs

Use **File > New Tab** or `Ctrl+Shift+T`. Tabs can be reordered, renamed, moved
between panes, detached into a window, or closed independently.

`View > Always Show Tabs` controls whether the tab bar remains visible when only
one tab is open.

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
shell. Press `Ctrl+Shift+P` to search them in the command palette.

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

### `/ai`

Starts AI chat with the configured provider:

```text
/ai
/ai context 30 why did the command fail?
/ai off
```

`/ai context N` includes the last N visible terminal lines in the request after
basic secret redaction. The resulting context is sent to the configured
provider, so do not use it with sensitive output that must remain local. Use
`Ctrl+C` to cancel an in-progress response.

### `/ai explain` and `/ai repair`

These commands use the latest failed command in the current directory, its exit
code, duration, Git branch, and recent redacted terminal output:

```text
/ai explain
/ai repair
/ai repair the failure only happens in CI
```

`/ai explain` focuses on the likely cause and next checks. `/ai repair` asks for
the smallest safe fix and displays commands for review; Oxterm never executes the
suggested repair automatically.

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

### `/connect`

Selects and tests an AI provider:

```text
/connect
/connect ollama
/connect openai
```

With no provider, Oxterm displays configured providers and their availability.
For Ollama and custom endpoints, available models can be detected automatically.

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

Performs occasional SQLite maintenance: removes duplicate command/directory
pairs, checkpoints the WAL, updates query statistics, and vacuums the database.

## History

Oxterm records the command, working directory, timestamp, and exit status in
`~/.config/tpgk/history.db`.

Press `Ctrl+R` for reverse interactive search:

- Type to filter.
- Use the arrow keys to select.
- Press Enter to use the selected command.
- Press Escape to cancel.

The `Tab` key can open the history picker when shell completion does not change
the current input. Selecting a result fills the command line without executing
it immediately.

## AI Chat

Configure AI providers under **Edit > Preferences > AI**. Supported providers:

| Provider | API key | Typical use |
| --- | --- | --- |
| OpenAI | Required | Cloud chat and coding assistance |
| Anthropic Claude | Required | Cloud chat and analysis |
| Google Gemini | Required | Cloud chat and analysis |
| DeepSeek | Required | Cloud chat and coding assistance |
| Ollama | Not required | Local models |
| Custom | Optional | OpenAI-compatible servers |

Each provider can have its own model, endpoint, and system prompt. Cloud API
keys are stored in `~/.config/tpgk/settings.json`; protect the file and do not
share it. Custom endpoints must use HTTPS, except for local HTTP services on
`localhost`, `127.0.0.1`, or `::1`.

For Ollama, start the local server before connecting. Custom endpoints should
provide an OpenAI-compatible chat API; local servers such as llama.cpp, vLLM,
and LM Studio are common examples.

## Notes

Notes are Markdown files with timestamped entries. Configure their location in
**Edit > Preferences > Notes**, then use `/wnotes` to append and `/onotes` to
open them.

The editor preference is used as a fallback when the desktop `xdg-open` helper
is unavailable.

## Latest Development Package

Every successful push to the `master` branch publishes a rolling prerelease:

<https://github.com/buzzqw/oxterm/releases/tag/latest>

It contains the current Linux executable, an AppImage, and a `SHA256SUMS` file.
This package is for testing and is replaced by the next successful build. Use a
versioned release for a stable installation.

## Preferences

Open **Edit > Preferences**. Changes are applied live where possible.

### General

Configure the default and dynamic tab title, login shell, custom shell command,
terminal dimensions, scrollbar, scrollback, scroll behavior, close confirmation,
selection copying, unsafe-paste warnings, file manager, session restore, bell
notifications, hints, and VI copy mode.

### Appearance

Choose the font and size, bold text, color scheme, foreground/background/cursor
colors, selection colors, tab colors, cursor shape, cursor blinking, opacity,
transparency, padding, and undercurl style.

### Colors

Edit all 16 ANSI palette entries individually. Built-in presets include Dark,
Light, Solarized, Monokai, Gruvbox, Nord, and Matrix variants. Use **Save As
Custom** to persist the current palette.

### Compatibility

Configure Backspace and Delete bindings, terminal encoding, and OSC 133 shell
integration. Supported encodings include UTF-8, ISO-8859 variants, UTF-16
variants, CP1252, CP850, ASCII, KOI8-R, Shift_JIS, EUC-JP, and GBK.

### AI

Enter provider keys, models, endpoints, and system prompts. Ollama and custom
providers can be used without a cloud API key.

### Notes

Configure the notes directory, default Markdown filename, and fallback editor.

## Shell Integration

OSC 133 lets Oxterm identify prompt start, command start, command output, and
exit status. It enables prompt navigation and command-output-aware features.

Enable **Preferences > Compatibility > OSC 133**. Oxterm creates a setup script
under `~/.config/tpgk/`; run it for the shell you use and restart the shell:

```bash
bash ~/.config/tpgk/osc-setup.sh
source ~/.bashrc
```

The integration supports Bash and Zsh. Use `Ctrl+Shift+Up` and
`Ctrl+Shift+Down` to move between detected prompts.

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Shift+N` | New window |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+Q` | Close window |
| `Ctrl+Shift+C` | Copy |
| `Ctrl+Shift+V` | Paste |
| `Ctrl+Shift+A` | Select all |
| `Ctrl+Shift+S` | Set title |
| `Ctrl+Shift+R` | Reset terminal |
| `Ctrl+Shift+X` | Reset and clear |
| `Ctrl+R` | Interactive history search |
| `Ctrl+U` | Kill line |
| `Ctrl+W` | Kill word |
| `Ctrl+L` | Clear screen |
| `Ctrl+C` | Interrupt or cancel AI/history mode |
| `Ctrl+D` | EOF; closes an empty shell tab on exit |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next tab |
| `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` | Move tab |
| `Ctrl+Alt+PageUp` | Switch split pane |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Previous / next prompt |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+B`, then `c` | New tab (tmux-compatible) |
| `Ctrl+B`, then `n` / `p` | Next / previous tab |
| `Ctrl+B`, then `%` / `"` | Vertical / horizontal split |
| `Ctrl+B`, then `o` | Switch split pane |
| `Ctrl+B`, then `s` / `w` | Show the interactive terminal list |
| `Ctrl+B`, then `d` | Detach the local terminal PTY; passed through when `ssh` is foreground |
| `Ctrl+B`, then `x` | Close tab |
| `Alt+1` ... `Alt+9` | Replay history entry |
| `F11` | Fullscreen |
| `Click` | Open a detected URL |

## Configuration Files

```text
~/.config/tpgk/settings.json
~/.config/tpgk/settings.json.bak
~/.config/tpgk/history.db
~/.config/tpgk/sessions/
~/.config/tpgk/profiles/
~/.config/tpgk/remote/
```

The directory is shared with the original application by design. Oxterm keeps
the previous valid settings file as `settings.json.bak` when replacing
Preferences. Back up the whole directory before manually editing settings or
migrating between versions.

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

### AI does not respond

Check the provider key, model, endpoint, and network connection. For Ollama,
verify that its service is running and that at least one model is installed.

### Shell integration is not visible

Confirm that OSC 133 is enabled, run the generated setup script, reload the
shell configuration, and start a new terminal tab.

### Reset all settings

Close Oxterm and back up or remove the configuration directory:

```bash
mv ~/.config/tpgk ~/.config/tpgk.backup
```

The next launch creates a fresh configuration. The same directory contains the
history database, so keep the backup if you want to restore history later.

## License

Oxterm is licensed under the European Union Public Licence 1.2. See [LICENSE](LICENSE).
