#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="${ROOT_DIR}/target/release/oxterm"
INSTALL_BIN="/usr/bin/oxterm"
LEGACY_BIN="/usr/bin/TRust"
LEGACY_DESKTOP="/usr/share/applications/TRust.desktop"

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "Oxterm: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

printf '%s\n' "Oxterm: building release binary..."
cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"

if [[ ! -x "${SOURCE_BIN}" ]]; then
    printf '%s\n' "Oxterm: release binary was not produced" >&2
    exit 1
fi

install_command=(install -m 0755 "${SOURCE_BIN}" "${INSTALL_BIN}")
remove_legacy_command=(rm -f "${LEGACY_BIN}")
remove_legacy_desktop_command=(rm -f "${LEGACY_DESKTOP}")
if [[ -w "$(dirname "${INSTALL_BIN}")" ]]; then
    "${install_command[@]}"
    if [[ -e "${LEGACY_BIN}" || -L "${LEGACY_BIN}" ]]; then
        "${remove_legacy_command[@]}"
    fi
    if [[ -e "${LEGACY_DESKTOP}" || -L "${LEGACY_DESKTOP}" ]]; then
        "${remove_legacy_desktop_command[@]}"
    fi
elif command -v sudo >/dev/null 2>&1; then
    sudo "${install_command[@]}"
    if [[ -e "${LEGACY_BIN}" || -L "${LEGACY_BIN}" ]]; then
        sudo "${remove_legacy_command[@]}"
    fi
    if [[ -e "${LEGACY_DESKTOP}" || -L "${LEGACY_DESKTOP}" ]]; then
        sudo "${remove_legacy_desktop_command[@]}"
    fi
else
    printf '%s\n' "Oxterm: write access to $(dirname "${INSTALL_BIN}") is required (sudo not found)" >&2
    exit 1
fi

printf '%s\n' "Oxterm: installed ${INSTALL_BIN}"
