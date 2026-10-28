#!/usr/bin/env bash
#
# install.sh - Build and install Oxterm system-wide (/usr/bin).
#
# Usage:
#   ./install.sh              # build the release binary, then install it
#   ./install.sh --no-build   # install the binary already in target/release
#
# The installer writes the executable, desktop launcher, manual page, shell
# completions, documentation and icon. It uses sudo only when the target
# directories are not writable.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="${ROOT_DIR}/target/release/oxterm"
INSTALL_BIN="/usr/bin/oxterm"
INSTALL_ICON="/usr/share/icons/hicolor/scalable/apps/oxterm.svg"
INSTALL_DESKTOP="/usr/share/applications/oxterm.desktop"

BUILD=true
for arg in "$@"; do
    case "$arg" in
        --no-build) BUILD=false ;;
        -h | --help)
            sed -n '3,11p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            printf 'Oxterm: unknown option: %s\n' "$arg" >&2
            exit 1
            ;;
    esac
done

if git -C "${ROOT_DIR}" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "${ROOT_DIR}" config core.hooksPath .githooks 2>/dev/null || true
fi

if [[ "$BUILD" == true ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        printf '%s\n' "Oxterm: cargo is required (install Rust from https://rustup.rs/)" >&2
        exit 1
    fi

    # shellcheck source=packaging/install-deps.sh
    . "${ROOT_DIR}/packaging/install-deps.sh"
    oxterm_install_build_deps

    printf '%s\n' "Oxterm: building release binary..."
    cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

if [[ ! -x "${SOURCE_BIN}" ]]; then
    if [[ "$BUILD" == true ]]; then
        printf '%s\n' "Oxterm: release binary was not produced" >&2
    else
        printf '%s\n' "Oxterm: ${SOURCE_BIN} is missing; run ./install.sh without --no-build" >&2
    fi
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
install_file 0644 "${ROOT_DIR}/manual.md" "/usr/share/doc/oxterm/manual.md"
install_file 0644 "${ROOT_DIR}/docs/settings.md" "/usr/share/doc/oxterm/settings.md"
install_file 0644 "${ROOT_DIR}/docs/compatibility.md" "/usr/share/doc/oxterm/compatibility.md"
install_file 0644 "${ROOT_DIR}/themes/licenses.md" "/usr/share/doc/oxterm/themes-licenses.md"
install_file 0644 "${ROOT_DIR}/packaging/oxterm.1" "/usr/share/man/man1/oxterm.1"
if [[ -w "/usr/share/man/man1" ]]; then
    gzip -nf "/usr/share/man/man1/oxterm.1"
elif command -v sudo >/dev/null 2>&1; then
    sudo gzip -nf "/usr/share/man/man1/oxterm.1"
fi
install_file 0644 "${ROOT_DIR}/packaging/completions/oxterm.bash" "/usr/share/bash-completion/completions/oxterm"
install_file 0644 "${ROOT_DIR}/packaging/completions/_oxterm" "/usr/share/zsh/site-functions/_oxterm"
install_file 0644 "${ROOT_DIR}/packaging/completions/oxterm.fish" "/usr/share/fish/vendor_completions.d/oxterm.fish"

printf '%s\n' "Oxterm: installed ${INSTALL_BIN}, desktop launcher, manual, man page and shell completions"
