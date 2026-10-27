# Oxterm User Manual

## Contents

1. [Starting Oxterm](#starting-oxterm)
2. [The interface](#the-interface)
3. [Tabs, windows, and splits](#tabs-windows-and-splits)
4. [Built-in commands](#built-in-commands)
5. [History](#history)
6. [Notes](#notes)
7. [Preferences](#preferences)
8. [Shell integration](#shell-integration)
9. [Keyboard shortcuts](#keyboard-shortcuts)
10. [Configuration files](#configuration-files)
11. [Troubleshooting](#troubleshooting)

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
instead, with the same permissions. The internal
`oxterm --broker SOCKET SESSION_ID` mode
owns the child PTY and accepts length-prefixed Unix-socket frames for
`LIST`, `INFO`, `ATTACH`, `MOVE`, `DETACH`, `RENAME`, `COMMAND`, `LOCAL_ON`,
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
oxterm capture [--pane ID] [--scrollback N | --all]
oxterm wait [--pane ID] [--timeout SECONDS]
```

`new` prints the id of the new pane and `new DIR` is a shorthand for
`new --cwd DIR`. `split` prints the id of the new pane and refuses to split a
window that already has two panes. `send` types into the target pane and, with
`--enter`, submits the line. `capture` prints the visible screen by default, the
last `N` lines of history with `--scrollback`, or the whole buffer with `--all`.
`wait` waits for the next command in the pane to finish and exits with that
command's status (clamped to 0–255), which makes it possible to chain a build and
read its result:

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
the network. `send` respects read-only mode.

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

### Command blocks and history chrome

With shell integration enabled (**Preferences > Compatibility > OSC 133**),
Oxterm records each command as a block: its command line, exit status, start
time and duration. While you scroll back into a block, a pinned header at the
top shows which command produced the visible output and how it ended (running,
successful, or failed with its exit code). Click the header to jump back to the
command. The scrollbar marks running commands in amber, failures in red, and
bookmarks in blue.

Press `Ctrl+Shift+G` (or use **Copy As > Open Last Output in Pager**) to open
the last command's output in a new tab with `less -R`. Set `pager_command` in
`settings.json` to use another pager. The temporary file is removed when the
pager exits.

### VI copy mode

Enable it in **Preferences > General** and press `Ctrl+Shift+Y`. The terminal
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
  a link.
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

### SSH connections

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

The status bar shows the remote host's CPU, memory, and disk usage for `ssh`,
`autossh`, and `mosh` sessions. Oxterm reads them through a shared connection to
that host only (for mosh, one opened by another tab); it never opens extra
logins, so hosts without a shared connection simply show no remote statistics.
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
| `Ctrl+Shift+H` | Hint mode for URLs, paths and Git SHAs |
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
| `F11` | Fullscreen |
| `Click` | Open a detected URL |
<!-- END GENERATED KEYBINDS -->

The tmux-style prefix defaults to `Ctrl+B`. Change it under Preferences →
General → "tmux-style prefix keys" and the "Prefix key (Ctrl+)" field, for
example to `Ctrl+A` when you use tmux or screen inside Oxterm. Any other key
after the prefix is passed to the running program together with the prefix.

## Configuration Files

```text
~/.config/oxterm/settings.json
~/.config/oxterm/settings.json.bak
~/.config/oxterm/history.db
~/.config/oxterm/sessions/
~/.config/oxterm/profiles/
~/.config/oxterm/remote/
~/.config/oxterm/themes/
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
