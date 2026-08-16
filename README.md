# terust

[![CI](https://github.com/buzzqw/terust/actions/workflows/ci.yml/badge.svg)](https://github.com/buzzqw/terust/actions/workflows/ci.yml)
[![Build](https://github.com/buzzqw/terust/actions/workflows/build.yml/badge.svg)](https://github.com/buzzqw/terust/actions/workflows/build.yml)
[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![GTK3](https://img.shields.io/badge/GTK-3-blue?logo=gnome)](https://www.gtk.org/)
[![License](https://img.shields.io/badge/License-EUPL%201.2-blue)](LICENSE)

**terust** is a fast, native Linux terminal emulator written in Rust with GTK3
and VTE. It is the Rust port of [TPGK](https://github.com/buzzqw/TPGK), with
the same configuration and history format so existing TPGK users can migrate
without losing their data.

The project combines a full terminal emulator with tabs, split panes, command
history, notes, shell integration, profiles, sessions, and optional AI chat.

- **[English user manual](manual.md)**
- **[Report an issue](https://github.com/buzzqw/terust/issues)**

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

terust currently targets Linux with GTK3 and VTE 2.91 development libraries.

### Debian or Ubuntu

```bash
sudo apt update
sudo apt install build-essential pkg-config libgtk-3-dev libvte-2.91-dev
```

### Arch Linux

```bash
sudo pacman -S base-devel rust gtk3 vte3
```

Rust can also be installed with [rustup](https://rustup.rs/).

## Build and Run

Clone the repository and build the optimized executable:

```bash
git clone https://github.com/buzzqw/terust.git
cd terust
cargo build --release
./target/release/terust
```

The launcher uses the release binary when available and falls back to the debug
binary:

```bash
./terust.sh
```

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
terust [DIRECTORY] [OPTIONS]

  -w, --working-directory DIR  Start in DIR
      --new-window             Open an independent window
      --no-restore              Do not restore the last session
  -e, --execute CMD...         Run CMD instead of the configured shell
      --version                Print the version
  -h, --help                   Print help
```

Examples:

```bash
terust ~/src/project
terust --working-directory ~/src/project
terust --new-window
terust --no-restore
terust --execute git status
```

Without an explicit directory, terust starts in the current working directory.

## Built-in Commands

Commands beginning with `/` are handled by terust:

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
| `Ctrl+R` | Interactive history search |
| `Ctrl+PageUp` / `Ctrl+PageDown` | Previous / next tab |
| `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` | Move tab |
| `Ctrl+Alt+PageUp` | Switch split pane |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Previous / next OSC 133 prompt |
| `Ctrl+Shift+P` | Command palette |
| `Alt+1` ... `Alt+9` | Replay a history result |
| `F11` | Fullscreen |

## Configuration and Data

terust intentionally uses the TPGK data directory:

```text
~/.config/tpgk/settings.json   Preferences and provider configuration
~/.config/tpgk/history.db      SQLite command history
~/.config/tpgk/sessions/       Saved sessions
~/.config/tpgk/profiles/       Named profiles
```

Open **Edit > Preferences** to configure the terminal, appearance, colors,
compatibility, AI providers, and notes. Most changes are applied immediately.

API keys are stored in the settings file. Protect that file appropriately and
never commit it to a repository.

## AI Providers

The AI page supports:

- OpenAI
- Anthropic Claude
- Google Gemini
- DeepSeek
- Ollama for local models
- Custom OpenAI-compatible endpoints

Cloud providers require their API key. Ollama requires a running local Ollama
server. AI chat is optional and does not affect normal terminal operation.

## AppImage

If `appimagetool` is installed locally:

```bash
packaging/appimage/build-appimage.sh
```

The resulting AppImage is written to `target/`.

## Development

Run the standard checks before submitting changes:

```bash
cargo fmt --check
cargo test --all-targets
cargo build --release
```

The GitHub Actions workflows run the same checks and build an x86_64 release
artifact on every push, pull request, and manual workflow dispatch.

Release automation is available through:

```bash
./versiona.sh --dry-run
./versiona.sh
```

## License

terust is distributed under the [European Union Public Licence 1.2](LICENSE).

Copyright 2026 Andres Zanzani.
