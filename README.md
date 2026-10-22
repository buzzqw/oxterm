# Oxterm

[![CI](https://github.com/buzzqw/oxterm/actions/workflows/ci.yml/badge.svg)](https://github.com/buzzqw/oxterm/actions/workflows/ci.yml)
[![Build](https://github.com/buzzqw/oxterm/actions/workflows/build.yml/badge.svg)](https://github.com/buzzqw/oxterm/actions/workflows/build.yml)
[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![GTK3](https://img.shields.io/badge/GTK-3-blue?logo=gnome)](https://www.gtk.org/)
[![License](https://img.shields.io/badge/License-EUPL%201.2-blue)](LICENSE)

**Oxterm** is a **lightweight, fast, and secure** native Linux terminal
emulator written in Rust with GTK3 and VTE. It is a **complete and fully
functional** terminal: light on resources and quick to start, while providing
everything you expect from a modern terminal.

The project combines a full terminal emulator with tabs, split panes, command
history, notes, shell integration, profiles, and sessions — all with a small
memory footprint and the memory-safety guarantees of Rust.

> **Quick start:** prebuilt Linux executables are already available on the
> [GitHub Releases page](https://github.com/buzzqw/oxterm/releases/latest). This
> is the easiest way to use Oxterm; you do not need Rust or development packages
> if you download a release binary.

- **[English user manual](manual.md)**
- **[Report an issue](https://github.com/buzzqw/oxterm/issues)**

## Highlights

- Native GTK3/VTE terminal with 256 colors and true-color support
- Tabs with reorder, rename, move, detach, and independent windows
- tmux-like single, vertical, and horizontal split layouts
- Configurable font, colors, 16-color palettes, cursor, opacity, padding, and encoding
- Live Preferences reload without restarting the application
- SQLite command history with reverse search, filters, read-only SQL, replay, command duration, and Git branch metadata
- Timestamped Markdown notes with an in-app Notes menu, per-entry context, and configurable editor integration
- OSC 133 shell integration for prompts, command boundaries, and exit status
- Session restore, named sessions, profiles, command palette, quickmarks, and hints
- Compact right-click menu with submenus for clipboard (text/HTML/all/last
  command), links, tabs, splits, tools, zoom, read-only, quickmarks, snippets,
  and notes actions
- Persistent PTY broker with SSH-friendly remote attach/detach, session listing, and multi-client terminal sharing
- Parameterized snippets and JSON export for saved sessions
- System statistics and SSH-aware status information
- Optional close confirmation only when a terminal has an active foreground process

## Requirements

### Using a release binary

You only need a Linux system with GTK3 and VTE 2.91 runtime libraries. Download
the latest executable from the [GitHub Releases
page](https://github.com/buzzqw/oxterm/releases/latest), make it executable, and
run it.

### Building from source

Source builds need:

- Rust stable and Cargo
- GTK3 development files
- VTE 2.91 development files, version 0.70 or newer (Debian 12 ships 0.70;
  Arch, Fedora and Ubuntu 24.04 ship newer releases)
- `pkg-config`

On Debian or Ubuntu:

```bash
sudo apt update
sudo apt install build-essential pkg-config libgtk-3-dev libvte-2.91-dev
```

On Arch Linux:

```bash
sudo pacman -S base-devel rust gtk3 vte3
```

Install Rust with [rustup](https://rustup.rs/) if it is not already available.

### Arch Linux (AUR)

The `oxterm` package builds the latest tagged release from source with the
system GTK3/VTE libraries. Once the package is published in the AUR:

```bash
yay -S oxterm        # or: paru -S oxterm
```

The `PKGBUILD` lives in [`packaging/aur`](packaging/aur) and is validated by CI
on every change.

## Build and Run

Clone the repository and build the optimized executable:

```bash
git clone https://github.com/buzzqw/oxterm.git
cd oxterm
cargo build --release --locked
./target/release/oxterm
```

To create the definitive release binary in the project root, run:

```bash
./build.sh
./oxterm-linux-x86-64
```

`build.sh` runs the locked release build and copies the result to
`oxterm-linux-x86-64`. The generated root binary is ignored by Git.

The launcher uses the release binary when available and falls back to the debug
binary:

```bash
./oxterm.sh
```

The repository includes local build hooks. Configure them with
`git config core.hooksPath .githooks`; `./setup.sh` also configures them as part
of its per-user installation. After each commit, the hook builds
`oxterm-linux-x86-64` in the project root. The generated file is ignored by
Git. The displayed Oxterm version uses the Cargo version plus the total Git
commit count as SemVer build metadata, for example
`MAJOR.MINOR.PATCH+<commit-count>`. Builds without Git metadata show the
plain release version.

Install the executable, desktop entry, manual page, shell completions, and
documentation under `~/.local`:

```bash
./setup.sh
```

The installation prefix can be changed with `PREFIX`; it must be a directory
writable by your user (use `./install.sh` for `/usr/bin`):

```bash
PREFIX="${HOME}/.local/opt" ./setup.sh
```

To build and install the current source version system-wide as `/usr/bin/oxterm`, run:

```bash
./install.sh
```

The script always rebuilds the release binary before installing it and asks for
`sudo` only when it needs permission to write the system installation
directories. It installs the executable, desktop launcher, manual page, shell
completions, documentation, and Oxterm ox icon system-wide. Run it again after
building a newer version so the system command and launcher point to the new
binary.

### Updating an existing clone

Compiled binaries are not tracked by Git, so a plain `git pull` only updates
the sources. Use the update helper instead:

```bash
./update.sh
```

`update.sh` pulls the latest changes, builds the release binary, refreshes
`oxterm-linux-x86-64`, and runs the installer (`/usr/bin` when writable or via
`sudo`, otherwise `~/.local`). It accepts `--user` to force the per-user
install, `--no-pull` to build the current checkout, and `--no-install` to stop
after the build.

`./setup.sh`, `./install.sh`, and `./update.sh` enable the repository hooks, so
later `git pull` commands rebuild automatically through the `post-merge` (or
`post-rewrite` for rebased pulls) hook. The hook refreshes a per-user
installation directly and a system-wide installation only when `sudo` needs no
password; otherwise run `./update.sh` after the pull. When Oxterm is installed
from the AUR package, `yay -Syu` updates it instead.

## Command Line

```text
oxterm [DIRECTORY] [OPTIONS] [-e CMD...]

  -w, --working-directory DIR  Start in DIR
  -T, --title TITLE            Fixed window title (apps cannot override it)
  -g, --geometry COLSxROWS     Initial size in character cells (e.g. 120x40)
  -F, --fullscreen             Start in fullscreen mode
  -m, --maximize               Start maximized
      --class CLASS            Set the WM_CLASS class part (window manager rules)
      --name NAME             Set the WM_CLASS instance name
  -p, --profile NAME           Start with a saved profile (session only)
      --config FILE            Use an alternative settings file
  -o, --option KEY=VALUE       Override a setting for this session (repeatable)
      --font FAMILY           Override the font family for this session
      --font-size N           Override the font size for this session
      --new-window             Open an independent window
      --no-restore             Do not restore the last session
      --hold                   Keep the terminal open after the command exits
  -e, --execute CMD...         Run CMD instead of the configured shell
      --list                   List active Oxterm terminals
      --info SESSION_ID        Show one active terminal
      --broker SOCKET ID       Run the persistent PTY broker (internal)
      --                       Treat the next argument as the directory
  -V, --version                Show the Oxterm version
  -h, --help                   Show this help

Remote sessions (SESSION_ID is the first column printed by --list):
  -a, --attach [SESSION_ID]    Share a live session in this terminal
  -M, --move [SESSION_ID]      Take a live session over in this terminal
      --detach SESSION_ID      Disconnect every remote client of a session

  These headless commands act on a running Oxterm terminal on the same host and
  open no window, so they are safe to use over SSH. Without a SESSION_ID, -a and
  -M use the only session or show a list to choose from.

  attach  Adds this terminal as another view of the session, repainting its
          current screen. The shell keeps running and its original window stays
          live; every view shares the same input and output.

  move    Like attach but takes control: it disconnects every other remote client
          and switches off the original window's view, so the session is shown
          only here. In that window, Ctrl+B then s or w brings the view back.

  detach  Releases every remote client (attach or move) without stopping the
          shell, leaving the original window as the only view again.
```

Examples:

```bash
oxterm ~/src/project
oxterm --working-directory ~/src/project
oxterm --title "Build" --geometry 120x40
oxterm --fullscreen
oxterm --new-window
oxterm --no-restore
oxterm --hold --execute make
oxterm --class MyTerm --name floating   # window-manager matching
oxterm --profile work                   # start with a saved profile
oxterm -o opacity=0.9 -o font_size=14    # ad-hoc setting overrides
oxterm --font "Fira Code" --font-size 13
oxterm --config ~/demo-settings.json     # throwaway configuration
oxterm --execute git status
oxterm --list                             # list terminals on this host
oxterm --info 12345-2                      # inspect one terminal
oxterm -a                                  # choose from the active terminals
oxterm -a 12345-2                          # attach to one terminal
oxterm -M                                  # move one here (detaches its other views)
oxterm -M 12345-2                          # move one terminal here
oxterm --detach 12345-2                   # release a remote attach
```

`--option`, `--font`, `--font-size` and `--profile` overrides apply only to the
launched session and are never written back to your saved settings. `--config`
points Oxterm at an alternative settings file (handy for demos).

`--execute` consumes every following argument as part of the command, so it must
be the last option. `--` disables option parsing for the following directory
argument, which is useful when its name begins with `-`.

Without an explicit directory, Oxterm starts in the current working directory.

`oxterm --list`, `oxterm -a [SESSION_ID]` and `oxterm -M [SESSION_ID]` are
headless commands. They do not open a GTK window: they inspect, attach to, or
move the live broker session of a running Oxterm terminal on the same host.
Attaching repaints the session by replaying its most recent output, so the
current screen appears instead of an empty one. This makes them suitable for use
through SSH. Broker sockets are private
(`0700` directory, `0600` socket) and the broker process owns the child PTY, so
an unexpected GUI crash does not terminate the shell. When the configured
directory would make the Unix socket path too long, Oxterm falls back to
`/tmp/oxterm-<uid>/remote/` (also `0700`) so sessions keep working with a long
`$HOME`. An intentional GUI close
still terminates its attached tabs through the normal cleanup path, but a tab
whose local view was switched off (`Ctrl+B`, `d`) or taken over with `oxterm -M`
keeps running under its broker, so the session survives the close and can be
reached again with `-a`/`-M` or reclaimed from the original window. Multiple `-a`
clients may be connected at once; their input is forwarded to the same shell
and output is broadcast to each client. `-M` instead takes control: it detaches
every remote controller and turns off the local GUI view, so the chosen session
is shown only in the terminal that ran the command. In the original Oxterm
window, `Ctrl+B` then `s`/`w` brings it back. The listing includes the last
command/application received from the shell, with `running` while OSC 133
reports that command as active and `last` otherwise.

The internal `oxterm --broker SOCKET SESSION_ID` mode is started by the GUI and
is not normally invoked manually. Its framed Unix-socket protocol supports
`LIST`, `INFO`, `ATTACH`, `MOVE`, `DETACH`, `RENAME`, `COMMAND`, `LOCAL_ON`,
`LOCAL_OFF`, `KILL`, `IS_SSH`, `ECHO`, and `IS_ACTIVE`. Attached clients exchange
length-prefixed frames containing terminal input/output, and can detach without
stopping the broker. On attach the broker replays the most recent child output
(bounded to a fixed size) so the program's screen is repainted instead of
appearing blank, then nudges the foreground program to redraw. A reconnect still
cannot recover output older than that replay window. The broker removes its
socket after the shell exits.

The listing columns are `ID`, `NAME`, `TITLE`, `DIRECTORY`, `STATUS`,
`APPLICATION`, and `APP_STATUS`. `APP_STATUS` is `running` while the command is
active and `last` after completion.

When **Confirm before closing active terminals** is enabled in Preferences,
closing a window asks for confirmation only if one of its terminal tabs has an
active foreground process. A shell waiting at its prompt closes immediately.

## Built-in Commands

Commands beginning with `/` are handled by Oxterm:

| Command | Purpose |
| --- | --- |
| `/help` | Show the command reference |
| `/history [terms]` | Search command history using AND filters |
| `/history :sql SELECT ...` | Run a read-only history query |
| `/wnotes [-file.md] <text>` | Save a timestamped note |
| `/onotes [-file.md]` | Open a notes file in the configured editor |
| `/learn <file>` | Import commands into history without executing them |
| `/optimize history` | Keep latest command/directory entries and optimize history in background |
| `/session export NAME [FILE]` | Export a saved session as private JSON |
| `/session name NAME` | Assign a stable name shown by remote listing |
| `/session list` | List saved sessions |
| `/snippet add NAME COMMAND` | Save a parameterized command snippet |
| `/snippet NAME [ARGS...]` | Expand a snippet into the prompt without executing it |
| `/snippet list` | List saved snippets |
| `/snippet delete NAME` | Remove a saved snippet |
| `/clear` / `/cls` | Clear the terminal screen |

The **Notes** menu also provides the integrated Notes panel (full-text search,
tags, preview, append) plus New Note, Add Selection to Note, Copy All to Note,
Note Command Output, Open Note, and Open Notes Folder actions (see the manual).

Press `Ctrl+Shift+P` for the command palette and `Tab` after `/` for command
completion.

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Shift+N` | New window |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+Q` | Close window |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+Shift+A` | Select all |
| `Ctrl+Shift+S` | Rename tab |
| `Ctrl+Shift+R` | Reset terminal |
| `Ctrl+Shift+X` | Reset and clear terminal |
| `Ctrl+Shift+F` | Search the scrollback (Enter/Shift+Enter = next/prev) |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Zoom font in / out / reset |
| `Ctrl+Shift+E` / `Ctrl+Shift+D` | Vertical / horizontal split |
| `Ctrl+Shift+M` / `Ctrl+M` | Set / jump to quickmark |
| `Ctrl+Shift+H` | Hint mode for URLs, paths, and Git SHAs |
| `Ctrl+Shift+Y` | VI-style copy mode |
| `Ctrl+Shift+B` | Toggle broadcast input |
| `click` | Open the URL under the cursor |
| `Ctrl+R` | Interactive history search; repeat to cycle matches |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next tab |
| `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` | Move tab |
| `Ctrl+Alt+PageUp` | Switch split pane |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Previous / next OSC 133 prompt |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+B`, then `c` | New tab (tmux-compatible) |
| `Ctrl+B`, then `n` / `p` | Next / previous tab |
| `Ctrl+B`, then `%` / `"` | Vertical / horizontal split |
| `Ctrl+B`, then `o` | Switch split pane |
| `Ctrl+B`, then `s` / `w` | Show the interactive terminal list |
| `Ctrl+B`, then `d` | Detach the local terminal PTY |
| `Ctrl+B`, then `x` | Close tab |
| `Alt+1` ... `Alt+9` | Replay one of the 9 most recent history entries (at the shell prompt) |
| `F11` | Fullscreen |

Any other key after `Ctrl+B` is passed to the running program together with
`Ctrl+B`. The prefix can be turned off under Preferences → General →
"Ctrl+B prefix keys" for programs that need `Ctrl+B` themselves.

## Configuration and Data

Oxterm intentionally uses the existing shared data directory:

```text
~/.config/oxterm/settings.json   Preferences
~/.config/oxterm/settings.json.bak Previous valid Preferences snapshot
~/.config/oxterm/history.db      SQLite command history
~/.config/oxterm/sessions/       Saved sessions
~/.config/oxterm/profiles/       Named profiles
~/.config/oxterm/remote/         Private broker sockets
/tmp/oxterm-<uid>/remote/        Fallback broker sockets when that path is too long
```

Open **Edit > Preferences** to configure the terminal, appearance, colors,
compatibility, and notes. Most changes are applied immediately.

URLs opened from terminal output are restricted to `http://` and `https://`.
Links using other schemes are not passed to desktop URL handlers.

The `master` branch also publishes a rolling prerelease after every successful
push: [Oxterm latest](https://github.com/buzzqw/oxterm/releases/tag/latest).
It contains the Linux executable and `SHA256SUMS`. The release is intended for
testing and is replaced by the next successful master build.

## Development

Run the standard checks before submitting changes:

```bash
cargo fmt --check
cargo test --all-targets --locked
cargo build --release --locked
```

The suite includes unit tests for CLI parsing, frame bounds, metadata
sanitization, and client buffering. `tests/remote_broker.rs` also starts the
release binary as a real broker with two PTYs and verifies control commands,
multi-client input/output, local forwarding toggles, detach, socket
permissions, and cleanup. Run that integration test alone with:

```bash
cargo test --test remote_broker -- --nocapture
```

The GitHub Actions workflows run the same checks and build an x86_64 release
artifact on every commit push, pull request, and manual workflow dispatch on any
branch. Version tags also trigger versioned release artifacts and checksum
generation.

For maintainers, release automation is available through:

```bash
./versiona.sh --dry-run
./versiona.sh
```

The normal release flow updates both `Cargo.toml` and `Cargo.lock`, creates an
annotated version tag, pushes the branch and tag, and publishes the GitHub
release. Use `--dry-run` to inspect the next version and release notes without
changing files or Git refs.

## Support

If Oxterm is useful to you, you can support development through
[PayPal](https://www.paypal.com/cgi-bin/webscr?cmd=_donations&business=azanzani@gmail.com&item_name=Support+Oxterm+Project).

## License

Oxterm is distributed under the [European Union Public Licence 1.2](LICENSE).

Copyright 2026 Andres Zanzani.
