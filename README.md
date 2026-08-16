# TPGK — Rust port (terust)

TPGK ported from Python to Rust, keeping a single native executable and the
same configuration/history files as the original (`~/.config/tpgk/`).

It is a GTK3 + VTE terminal emulator with AI chat, command history, notes,
tmux-like splits and shell integration (OSC 133).

## Build

Requirements (Arch):

```bash
sudo pacman -S rust gtk3 vte3
```

Build:

```bash
cargo build --release
./target/release/terust
# or
./terust.sh
```

## Features

- Full terminal emulation via VTE (xterm-256color, true color)
- Tabs: detach, move, reorder, rename; split panels (single/vertical/horizontal)
- System stats bar (CPU/RAM/Disk) with SSH detection and remote stats
- 8 color schemes + 16-color palette editor; live-reload preferences (7 tabs)
- AI chat (`/ai`) with OpenAI, Claude, Gemini, DeepSeek, Ollama, Custom;
  streaming with `Ctrl+C` cancellation
- SQLite-backed command history (`/history`, `Ctrl+R`, `Tab Tab` picker,
  `Alt+1..9` replay, `/history :sql SELECT ...`)
- Notes (`/wnotes`, `/onotes`), OSC 133 shell integration (bash/zsh),
  hint mode (`Ctrl+Shift+H`), VI copy mode (`Ctrl+Shift+Y`),
  quickmarks, scrollback search, broadcast input
- Session save/restore and profiles
- Same config (`settings.json`) and history DB as the original Python TPGK

## Command line

```bash
terust /path/to/project          # open in a directory
terust --new-window              # independent window
terust --no-restore              # don't restore last session
terust --execute git status      # run a command
terust --version
```

## License

EUPL 1.2. Uses `zoha-vte` (GPL-3.0 bindings for VTE), compatible under EUPL §5.
