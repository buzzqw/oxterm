#!/usr/bin/env bash
#
# setup.sh - Build and install Oxterm under a user-writable prefix.
#
# Usage:
#   ./setup.sh              # build the release binary, then install it
#   ./setup.sh --no-build   # install the binary already in target/release
#   PREFIX=/usr/local ./setup.sh
#
# The default prefix is ~/.local. The installer writes the executable, desktop
# launcher, manual page, shell completions, documentation and icon.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-${HOME}/.local}"

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
    git -C "${ROOT_DIR}" config core.hooksPath .githooks
fi

if [[ "$BUILD" == true ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        printf '%s\n' "Oxterm: cargo is required (install Rust from https://rustup.rs/)" >&2
        exit 1
    fi

    # shellcheck source=packaging/install-deps.sh
    . "${ROOT_DIR}/packaging/install-deps.sh"
    oxterm_install_build_deps

    printf '%s\n' "Building Oxterm..."
    cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

SOURCE_BIN="${ROOT_DIR}/target/release/oxterm"
if [[ ! -x "${SOURCE_BIN}" ]]; then
    if [[ "$BUILD" == true ]]; then
        printf '%s\n' "Oxterm: release binary was not produced" >&2
    else
        printf '%s\n' "Oxterm: ${SOURCE_BIN} is missing; run ./setup.sh without --no-build" >&2
    fi
    exit 1
fi

install -Dm755 "${SOURCE_BIN}" "${PREFIX}/bin/oxterm"

install -Dm644 "${ROOT_DIR}/packaging/oxterm.svg" \
    "${PREFIX}/share/icons/hicolor/scalable/apps/oxterm.svg"

# Keep the launcher in sync with the packaged one, but point it at PREFIX.
desktop_dir="${PREFIX}/share/applications"
install -d "${desktop_dir}"
sed "s|^Exec=oxterm|Exec=${PREFIX}/bin/oxterm|" \
    "${ROOT_DIR}/packaging/oxterm.desktop" > "${desktop_dir}/oxterm.desktop"
chmod 0644 "${desktop_dir}/oxterm.desktop"

install -Dm644 "${ROOT_DIR}/manual.md" "${PREFIX}/share/doc/oxterm/manual.md"

install -Dm644 "${ROOT_DIR}/packaging/oxterm.1" "${PREFIX}/share/man/man1/oxterm.1"
gzip -nf "${PREFIX}/share/man/man1/oxterm.1"

install -Dm644 "${ROOT_DIR}/packaging/completions/oxterm.bash" \
    "${PREFIX}/share/bash-completion/completions/oxterm"
install -Dm644 "${ROOT_DIR}/packaging/completions/_oxterm" \
    "${PREFIX}/share/zsh/site-functions/_oxterm"
install -Dm644 "${ROOT_DIR}/packaging/completions/oxterm.fish" \
    "${PREFIX}/share/fish/vendor_completions.d/oxterm.fish"

printf '%s\n' "Installed Oxterm to ${PREFIX}/bin/oxterm"
case ":${PATH}:" in
    *":${PREFIX}/bin:"*) ;;
    *) printf '%s\n' "Add ${PREFIX}/bin to PATH to run 'oxterm' from any shell." ;;
esac
