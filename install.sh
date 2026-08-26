#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="${ROOT_DIR}/target/release/oxterm"
INSTALL_BIN="/usr/bin/oxterm"
INSTALL_ICON="/usr/share/icons/hicolor/scalable/apps/oxterm.svg"
INSTALL_DESKTOP="/usr/share/applications/oxterm.desktop"
LEGACY_BIN="/usr/bin/TRust"
LEGACY_DESKTOP="/usr/share/applications/TRust.desktop"
LEGACY_ICON="/usr/share/icons/hicolor/scalable/apps/TRust.svg"

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
install_icon_command=(install -Dm644 "${ROOT_DIR}/packaging/oxterm.svg" "${INSTALL_ICON}")
install_desktop_command=(install -Dm644 "${ROOT_DIR}/packaging/oxterm.desktop" "${INSTALL_DESKTOP}")
remove_legacy_command=(rm -f "${LEGACY_BIN}")
remove_legacy_desktop_command=(rm -f "${LEGACY_DESKTOP}")
remove_legacy_icon_command=(rm -f "${LEGACY_ICON}")
if [[ -w "$(dirname "${INSTALL_BIN}")" &&
    -w "$(dirname "${INSTALL_ICON}")" &&
    -w "$(dirname "${INSTALL_DESKTOP}")" ]]; then
    "${install_command[@]}"
    "${install_icon_command[@]}"
    "${install_desktop_command[@]}"
    if [[ -e "${LEGACY_BIN}" || -L "${LEGACY_BIN}" ]]; then
        "${remove_legacy_command[@]}"
    fi
    if [[ -e "${LEGACY_DESKTOP}" || -L "${LEGACY_DESKTOP}" ]]; then
        "${remove_legacy_desktop_command[@]}"
    fi
    if [[ -e "${LEGACY_ICON}" || -L "${LEGACY_ICON}" ]]; then
        "${remove_legacy_icon_command[@]}"
    fi
elif command -v sudo >/dev/null 2>&1; then
    sudo "${install_command[@]}"
    sudo "${install_icon_command[@]}"
    sudo "${install_desktop_command[@]}"
    if [[ -e "${LEGACY_BIN}" || -L "${LEGACY_BIN}" ]]; then
        sudo "${remove_legacy_command[@]}"
    fi
    if [[ -e "${LEGACY_DESKTOP}" || -L "${LEGACY_DESKTOP}" ]]; then
        sudo "${remove_legacy_desktop_command[@]}"
    fi
    if [[ -e "${LEGACY_ICON}" || -L "${LEGACY_ICON}" ]]; then
        sudo "${remove_legacy_icon_command[@]}"
    fi
else
    printf '%s\n' "Oxterm: write access to the system installation directories is required (sudo not found)" >&2
    exit 1
fi

printf '%s\n' "Oxterm: installed ${INSTALL_BIN} and desktop launcher"
