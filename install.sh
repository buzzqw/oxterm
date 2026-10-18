#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="${ROOT_DIR}/target/release/oxterm"
INSTALL_BIN="/usr/bin/oxterm"
INSTALL_ICON="/usr/share/icons/hicolor/scalable/apps/oxterm.svg"
INSTALL_DESKTOP="/usr/share/applications/oxterm.desktop"

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "Oxterm: cargo is required (install Rust from https://rustup.rs/)" >&2
    exit 1
fi

# shellcheck source=packaging/install-deps.sh
. "${ROOT_DIR}/packaging/install-deps.sh"
oxterm_install_build_deps

printf '%s\n' "Oxterm: building release binary..."
cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"

if [[ ! -x "${SOURCE_BIN}" ]]; then
    printf '%s\n' "Oxterm: release binary was not produced" >&2
    exit 1
fi

install_file() {
    local mode="$1" source_file="$2" target_file="$3"
    if [[ -w "$(dirname "${target_file}")" ]]; then
        install -D "-m${mode}" "${source_file}" "${target_file}"
    elif command -v sudo >/dev/null 2>&1; then
        sudo install -D "-m${mode}" "${source_file}" "${target_file}"
    else
        printf '%s\n' "Oxterm: write access to the system installation directories is required (sudo not found)" >&2
        exit 1
    fi
}

install_file 0755 "${SOURCE_BIN}" "${INSTALL_BIN}"
install_file 0644 "${ROOT_DIR}/packaging/oxterm.svg" "${INSTALL_ICON}"
install_file 0644 "${ROOT_DIR}/packaging/oxterm.desktop" "${INSTALL_DESKTOP}"
install_file 0644 "${ROOT_DIR}/packaging/oxterm.1" "/usr/share/man/man1/oxterm.1"
if [[ -w "/usr/share/man/man1" ]]; then
    gzip -nf "/usr/share/man/man1/oxterm.1"
elif command -v sudo >/dev/null 2>&1; then
    sudo gzip -nf "/usr/share/man/man1/oxterm.1"
fi
install_file 0644 "${ROOT_DIR}/packaging/completions/oxterm.bash" "/usr/share/bash-completion/completions/oxterm"
install_file 0644 "${ROOT_DIR}/packaging/completions/_oxterm" "/usr/share/zsh/site-functions/_oxterm"
install_file 0644 "${ROOT_DIR}/packaging/completions/oxterm.fish" "/usr/share/fish/vendor_completions.d/oxterm.fish"

printf '%s\n' "Oxterm: installed ${INSTALL_BIN}, desktop launcher, manual page and shell completions"
