#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-${HOME}/.local}"

if git -C "${ROOT_DIR}" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "${ROOT_DIR}" config core.hooksPath .githooks
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "terust: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

printf '%s\n' "Building terust..."
cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"

install -Dm755 "${ROOT_DIR}/target/release/terust" "${PREFIX}/bin/terust"

desktop_dir="${PREFIX}/share/applications"
install -d "${desktop_dir}"
cat > "${desktop_dir}/terust.desktop" <<EOF
[Desktop Entry]
Name=terust Terminal
Comment=GTK3/VTE terminal emulator with AI, history and notes
Exec=${PREFIX}/bin/terust
Icon=utilities-terminal
Terminal=false
Type=Application
Categories=System;TerminalEmulator;
StartupWMClass=terust
MimeType=inode/directory;
EOF

printf '%s\n' "Installed terust to ${PREFIX}/bin/terust"
