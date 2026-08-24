#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-${HOME}/.local}"

if git -C "${ROOT_DIR}" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "${ROOT_DIR}" config core.hooksPath .githooks
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "TRust: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

printf '%s\n' "Building TRust..."
cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"

install -Dm755 "${ROOT_DIR}/target/release/TRust" "${PREFIX}/bin/TRust"

desktop_dir="${PREFIX}/share/applications"
install -d "${desktop_dir}"
cat > "${desktop_dir}/TRust.desktop" <<EOF
[Desktop Entry]
Name=TRust Terminal
Comment=GTK3/VTE terminal emulator with AI, history and notes
Exec="${PREFIX}/bin/TRust" %f
Icon=utilities-terminal
Terminal=false
Type=Application
Categories=System;TerminalEmulator;
StartupWMClass=TRust
MimeType=inode/directory;
EOF

printf '%s\n' "Installed TRust to ${PREFIX}/bin/TRust"
