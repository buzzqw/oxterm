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
history, notes, shell integration, profiles, sessions, and optional AI chat —
all with a small memory footprint and the memory-safety guarantees of Rust.

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
- AI chat through OpenAI, Anthropic Claude, Google Gemini, DeepSeek, Ollama, and custom APIs
- AI failure diagnosis with `/ai explain` and safe repair suggestions with `/ai repair`
- Timestamped Markdown notes and configurable editor integration
- OSC 133 shell integration for prompts, command boundaries, and exit status
- Session restore, named sessions, profiles, command palette, quickmarks, and hints
- Parameterized snippets and JSON export for saved sessions
- System statistics and SSH-aware status information

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
- VTE 2.91 development files
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

## Build and Run

Clone the repository and build the optimized executable:

```bash
git clone https://github.com/buzzqw/oxterm.git
cd oxterm
cargo build --release --locked
./target/release/oxterm
```

The launcher uses the release binary when available and falls back to the debug
binary:

```bash
./oxterm.sh
```

The repository includes local build hooks. Configure them once with
`./setup.sh` or `git config core.hooksPath .githooks`; after each commit it
builds `oxterm-linux-x86-64` in the project root. The generated file is ignored
by Git. The displayed Oxterm version uses the Cargo version plus the total Git
commit count as its fourth component, for example `1.1.0.20`.

Install the binary and desktop entry under `~/.local`:

```bash
./setup.sh
```

The installation prefix can be changed with `PREFIX`:

```bash
PREFIX=/usr/local ./setup.sh
```

To install the latest release build system-wide as `/usr/bin/oxterm`, run:

```bash
./install.sh
```

The script always rebuilds the release binary before installing it and asks for
`sudo` only when it needs permission to write `/usr/bin`. Run it again after
building a newer version so the system command points to the new binary. This
installs the executable only; use `setup.sh` when you also want the per-user
desktop entry. Both installers remove the legacy `TRust` executable and desktop
entry when present; they never remove the unrelated lowercase `trust` command.

The command is `oxterm` in lowercase. This avoids the unrelated `/usr/bin/trust`
`p11-kit` certificate utility.

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
  -p, --profile NAME          Start with a saved profile (session only)
      --config FILE           Use an alternative settings file
  -o, --option KEY=VALUE      Override a setting for this session (repeatable)
      --font FAMILY           Override the font family for this session
      --font-size N           Override the font size for this session
      --new-window             Open an independent window
      --no-restore             Do not restore the last session
      --hold                   Keep the terminal open after the command exits
  -e, --execute CMD...         Run CMD instead of the configured shell
      --list                    List active Oxterm terminals
      --info SESSION_ID          Show one active terminal
  -a, --attach [SESSION_ID]    Attach to one or choose an Oxterm terminal
      --detach SESSION_ID       Detach its remote controller
      --broker SOCKET ID        Run the persistent PTY broker (internal)
      --                       Treat every following argument as the directory
  -V, --version                Print the version
  -h, --help                   Print help
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
oxterm -a                                  # attach automatically or choose one
oxterm -a 12345-2                          # attach to one terminal
oxterm --detach 12345-2                   # release a remote attach
```

`--option`, `--font`, `--font-size` and `--profile` overrides apply only to the
launched session and are never written back to your saved settings. `--config`
points Oxterm at an alternative settings file (handy for demos).

Without an explicit directory, Oxterm starts in the current working directory.

`oxterm --list` and `oxterm -a [SESSION_ID]` are headless commands. They do
not open a GTK window: they inspect or attach to the live broker session of a
running Oxterm terminal on the same host. This makes them suitable for use
through SSH. Broker sockets are private (`0700` directory, `0600` socket) and
the broker process owns the child PTY, so an unexpected GUI crash does not
terminate the shell. An intentional GUI close still terminates its tabs through
the normal cleanup path. Multiple `-a` clients may be connected at once; their
input is forwarded to the same shell and output is broadcast to each client.
The listing includes the last command/application received from the shell, with
`running` while OSC 133 reports that command as active and `last` otherwise.

The internal `oxterm --broker SOCKET SESSION_ID` mode is started by the GUI and
is not normally invoked manually. Its framed Unix-socket protocol supports
`LIST`, `INFO`, `ATTACH`, `DETACH`, `RENAME`, `COMMAND`, `LOCAL_ON`, `LOCAL_OFF`,
and `KILL`. Attached clients exchange length-prefixed frames containing terminal
input/output, and can detach without stopping the broker. Reconnecting does not
replay output produced before the new client attached; it receives subsequent
terminal output only. The broker removes its socket after the shell exits.

The listing columns are `ID`, `NAME`, `TITLE`, `DIRECTORY`, `STATUS`,
`APPLICATION`, and `APP_STATUS`. `APP_STATUS` is `running` while the command is
active and `last` after completion.

## Built-in Commands

Commands beginning with `/` are handled by Oxterm:

| Command | Purpose |
| --- | --- |
| `/help` | Show the command reference |
| `/history [terms]` | Search command history using AND filters |
| `/history :sql SELECT ...` | Run a read-only history query |
| `/ai` | Enter AI chat mode |
| `/ai explain` | Explain the latest failed command in the current directory |
| `/ai repair` | Suggest a safe repair for the latest failed command |
| `/ai context N <question>` | Ask AI about the last N terminal lines |
| `/ai off` | Leave AI chat mode |
| `/connect [provider]` | Select an AI provider and model |
| `/wnotes [-file.md] <text>` | Save a timestamped note |
| `/onotes [-file.md]` | Open a notes file in the configured editor |
| `/learn <file>` | Import commands into history without executing them |
| `/optimize history` | Deduplicate and optimize the history database |
| `/session export NAME [FILE]` | Export a saved session as private JSON |
| `/session name NAME` | Assign a stable name shown by remote listing |
| `/session list` | List saved sessions |
| `/snippet add NAME COMMAND` | Save a parameterized command snippet |
| `/snippet NAME [ARGS...]` | Expand a snippet into the prompt without executing it |
| `/snippet list` | List saved snippets |

Press `Ctrl+Shift+P` for the command palette and `Tab` after `/` for command
completion.

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+T` | New tab |
| `Ctrl+Shift+N` | New window |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+Shift+S` | Set tab title |
| `Ctrl+Shift+R` | Reset terminal |
| `Ctrl+Shift+X` | Reset and clear terminal |
| `Ctrl+Shift+F` | Search the scrollback (Enter/Shift+Enter = next/prev) |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Zoom font in / out / reset |
| `click` | Open the URL under the cursor |
| `Ctrl+R` | Interactive history search |
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
| `Alt+1` ... `Alt+9` | Replay a history result |
| `F11` | Fullscreen |

## Configuration and Data

Oxterm intentionally uses the existing shared data directory:

```text
~/.config/tpgk/settings.json   Preferences and provider configuration
~/.config/tpgk/settings.json.bak Previous valid Preferences snapshot
~/.config/tpgk/history.db      SQLite command history
~/.config/tpgk/sessions/       Saved sessions
~/.config/tpgk/profiles/       Named profiles
~/.config/tpgk/remote/         Private broker sockets
```

Open **Edit > Preferences** to configure the terminal, appearance, colors,
compatibility, AI providers, and notes. Most changes are applied immediately.

API keys are stored in the settings file. Protect that file appropriately and
never commit it to a repository. The file and its backup are written with
restrictive permissions, but they are not encrypted.

## AI Providers

The AI page supports:

- OpenAI
- Anthropic Claude
- Google Gemini
- DeepSeek
- Ollama for local models
- Custom OpenAI-compatible endpoints

Cloud providers require their API key. Ollama requires a running local Ollama
server. Custom endpoints must use HTTPS; plain HTTP is accepted only for local
services such as `localhost`, `127.0.0.1`, and `::1`. AI chat is optional and
does not affect normal terminal operation.

`/ai context` sends the selected recent terminal lines to the configured
provider after basic secret redaction. Do not use it with sensitive output that
must not leave the machine.

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

Release automation is available through:

```bash
./versiona.sh --dry-run
./versiona.sh
```

## Support

If Oxterm is useful to you, you can support development through
[PayPal](https://www.paypal.com/cgi-bin/webscr?cmd=_donations&business=azanzani@gmail.com&item_name=Support+Oxterm+Project).

## License

Oxterm is distributed under the [European Union Public Licence 1.2](LICENSE).

Copyright 2026 Andres Zanzani.
