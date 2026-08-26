#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="${ROOT_DIR}/target/release/TRust"
INSTALL_BIN="/usr/bin/TRust"

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "TRust: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

printf '%s\n' "TRust: building release binary..."
cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"

if [[ ! -x "${SOURCE_BIN}" ]]; then
    printf '%s\n' "TRust: release binary was not produced" >&2
    exit 1
fi

install_command=(install -m 0755 "${SOURCE_BIN}" "${INSTALL_BIN}")
if [[ -w "$(dirname "${INSTALL_BIN}")" ]]; then
    "${install_command[@]}"
elif command -v sudo >/dev/null 2>&1; then
    sudo "${install_command[@]}"
else
    printf '%s\n' "TRust: write access to $(dirname "${INSTALL_BIN}") is required (sudo not found)" >&2
    exit 1
fi

printf '%s\n' "TRust: installed ${INSTALL_BIN}"
