# TRust

[![CI](https://github.com/buzzqw/TRust/actions/workflows/ci.yml/badge.svg)](https://github.com/buzzqw/TRust/actions/workflows/ci.yml)
[![Build](https://github.com/buzzqw/TRust/actions/workflows/build.yml/badge.svg)](https://github.com/buzzqw/TRust/actions/workflows/build.yml)
[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![GTK3](https://img.shields.io/badge/GTK-3-blue?logo=gnome)](https://www.gtk.org/)
[![License](https://img.shields.io/badge/License-EUPL%201.2-blue)](LICENSE)

**TRust** is a **lightweight, fast, and secure** native Linux terminal
emulator written in Rust with GTK3 and VTE. It is a **complete and fully
functional** terminal: light on resources and quick to start, while providing
everything you expect from a modern terminal.

The project combines a full terminal emulator with tabs, split panes, command
history, notes, shell integration, profiles, sessions, and optional AI chat —
all with a small memory footprint and the memory-safety guarantees of Rust.

> **Quick start:** prebuilt Linux executables are already available on the
> [GitHub Releases page](https://github.com/buzzqw/TRust/releases/latest). This
> is the easiest way to use TRust; you do not need Rust or development packages
> if you download a release binary.

- **[English user manual](manual.md)**
- **[Report an issue](https://github.com/buzzqw/TRust/issues)**

## Highlights

- Native GTK3/VTE terminal with 256 colors and true-color support
- Tabs with reorder, rename, move, detach, and independent windows
- tmux-like single, vertical, and horizontal split layouts
- Configurable font, colors, 16-color palettes, cursor, opacity, padding, and encoding
- Live Preferences reload without restarting the application
- SQLite command history with reverse search, filters, read-only SQL, and replay
- AI chat through OpenAI, Anthropic Claude, Google Gemini, DeepSeek, Ollama, and custom APIs
- Timestamped Markdown notes and configurable editor integration
- OSC 133 shell integration for prompts, command boundaries, and exit status
- Session restore, named sessions, profiles, command palette, quickmarks, and hints
- System statistics and SSH-aware status information

## Requirements

### Using a release binary

You only need a Linux system with GTK3 and VTE 2.91 runtime libraries. Download
the latest executable from the [GitHub Releases
page](https://github.com/buzzqw/TRust/releases/latest), make it executable, and
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
git clone https://github.com/buzzqw/TRust.git
cd TRust
cargo build --release --locked
./target/release/TRust
```

The launcher uses the release binary when available and falls back to the debug
binary:

```bash
./TRust.sh
```

The repository includes local build hooks. Configure them once with
`./setup.sh` or `git config core.hooksPath .githooks`; after each commit it
builds `TRust-linux-x86-64` in the project root. The generated file is ignored
by Git. The displayed TRust version uses the Cargo version plus the total Git
commit count as its fourth component, for example `1.1.0.20`.

Install the binary and desktop entry under `~/.local`:

```bash
./setup.sh
```

The installation prefix can be changed with `PREFIX`:

```bash
PREFIX=/usr/local ./setup.sh
```

## Command Line

```text
TRust [DIRECTORY] [OPTIONS] [-e CMD...]

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
      --                       Treat every following argument as the directory
  -V, --version                Print the version
  -h, --help                   Print help
```

Examples:

```bash
TRust ~/src/project
TRust --working-directory ~/src/project
TRust --title "Build" --geometry 120x40
TRust --fullscreen
TRust --new-window
TRust --no-restore
TRust --hold --execute make
TRust --class MyTerm --name floating   # window-manager matching
TRust --profile work                   # start with a saved profile
TRust -o opacity=0.9 -o font_size=14    # ad-hoc setting overrides
TRust --font "Fira Code" --font-size 13
TRust --config ~/demo-settings.json     # throwaway configuration
TRust --execute git status
```

`--option`, `--font`, `--font-size` and `--profile` overrides apply only to the
launched session and are never written back to your saved settings. `--config`
points TRust at an alternative settings file (handy for demos).

Without an explicit directory, TRust starts in the current working directory.

## Built-in Commands

Commands beginning with `/` are handled by TRust:

| Command | Purpose |
| --- | --- |
| `/help` | Show the command reference |
| `/history [terms]` | Search command history using AND filters |
| `/history :sql SELECT ...` | Run a read-only history query |
| `/ai` | Enter AI chat mode |
| `/ai context N <question>` | Ask AI about the last N terminal lines |
| `/ai off` | Leave AI chat mode |
| `/connect [provider]` | Select an AI provider and model |
| `/wnotes [-file.md] <text>` | Save a timestamped note |
| `/onotes [-file.md]` | Open a notes file in the configured editor |
| `/learn <file>` | Import commands into history without executing them |
| `/optimize history` | Deduplicate and optimize the history database |

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
| `Alt+1` ... `Alt+9` | Replay a history result |
| `F11` | Fullscreen |

## Configuration and Data

TRust intentionally uses the existing shared data directory:

```text
~/.config/tpgk/settings.json   Preferences and provider configuration
~/.config/tpgk/settings.json.bak Previous valid Preferences snapshot
~/.config/tpgk/history.db      SQLite command history
~/.config/tpgk/sessions/       Saved sessions
~/.config/tpgk/profiles/       Named profiles
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
push: [TRust latest](https://github.com/buzzqw/TRust/releases/tag/latest).
It contains the Linux executable and `SHA256SUMS`. The release is intended for
testing and is replaced by the next successful master build.

## Development

Run the standard checks before submitting changes:

```bash
cargo fmt --check
cargo test --all-targets --locked
cargo build --release --locked
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

If TRust is useful to you, you can support development through
[PayPal](https://www.paypal.com/cgi-bin/webscr?cmd=_donations&business=azanzani@gmail.com&item_name=Support+TRust+Project).

## License

TRust is distributed under the [European Union Public Licence 1.2](LICENSE).

Copyright 2026 Andres Zanzani.
