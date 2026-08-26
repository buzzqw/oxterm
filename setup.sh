#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-${HOME}/.local}"

if git -C "${ROOT_DIR}" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "${ROOT_DIR}" config core.hooksPath .githooks
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "Oxterm: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

printf '%s\n' "Building Oxterm..."
cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"

install -Dm755 "${ROOT_DIR}/target/release/oxterm" "${PREFIX}/bin/oxterm"

desktop_dir="${PREFIX}/share/applications"
install -d "${desktop_dir}"
icon_dir="${PREFIX}/share/icons/hicolor/scalable/apps"
install -Dm644 "${ROOT_DIR}/packaging/oxterm.svg" "${icon_dir}/oxterm.svg"
cat > "${desktop_dir}/oxterm.desktop" <<EOF
[Desktop Entry]
Name=Oxterm Terminal
Comment=GTK3/VTE terminal emulator with AI, history and notes
Exec="${PREFIX}/bin/oxterm" %f
Icon=oxterm
Terminal=false
Type=Application
Categories=System;TerminalEmulator;
StartupWMClass=Oxterm
MimeType=inode/directory;
EOF

rm -f "${PREFIX}/bin/TRust" "${desktop_dir}/TRust.desktop" "${icon_dir}/TRust.svg"

printf '%s\n' "Installed Oxterm to ${PREFIX}/bin/oxterm"
