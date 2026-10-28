#!/usr/bin/env bash
# refresh-installed.sh - make `oxterm` in PATH run target/release/oxterm.
#
# Called by .githooks/post-commit and .githooks/post-merge so that the next
# launch runs the build from this checkout instead of a binary installed weeks
# ago. It never prompts:
#
#   1. an existing ~/.local/bin/oxterm is refreshed;
#   2. otherwise, when the system copy cannot be written without a password and
#      ~/.local/bin is in PATH, a user-scoped copy is installed there (it
#      shadows /usr/bin/oxterm for this user only);
#   3. /usr/bin/oxterm is refreshed when it is writable or passwordless sudo
#      works; otherwise the caller is told to run ./update.sh.

set -u

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILT="${ROOT_DIR}/target/release/oxterm"
if [[ ! -x "$BUILT" ]]; then
    printf '%s\n' 'Oxterm: target/release/oxterm is missing; nothing installed' >&2
    exit 0
fi

# GNU install replaces the destination inode, so a running old binary does not
# make the copy fail with ETXTBSY.
install_copy() {
    local dest="$1"
    shift
    if "$@" install -m 0755 "$BUILT" "$dest"; then
        printf '%s\n' "Oxterm: updated ${dest}"
        return 0
    fi
    printf '%s\n' "Oxterm: could not update ${dest}" >&2
    return 1
}

USER_DIR="${PREFIX:-${HOME}/.local}/bin"
USER_BIN="${USER_DIR}/oxterm"
SYSTEM_BIN="/usr/bin/oxterm"

system_writable=false
if [[ -f "$SYSTEM_BIN" && -w "$(dirname "$SYSTEM_BIN")" ]]; then
    system_writable=true
fi
system_sudo=false
if [[ -f "$SYSTEM_BIN" && "$system_writable" == false ]] &&
    command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    system_sudo=true
fi

user_installed=false
if [[ -f "$USER_BIN" ]]; then
    install_copy "$USER_BIN" && user_installed=true
elif [[ "$system_writable" == false && "$system_sudo" == false ]] &&
    [[ ":${PATH:-}:" == *":${USER_DIR}:"* ]]; then
    mkdir -p "$USER_DIR"
    if install_copy "$USER_BIN"; then
        user_installed=true
        printf '%s\n' "Oxterm: ${USER_DIR} comes first in PATH, so 'oxterm' now runs this build"
    fi
fi

if [[ -f "$SYSTEM_BIN" ]]; then
    if [[ "$system_writable" == true ]]; then
        install_copy "$SYSTEM_BIN" || true
    elif [[ "$system_sudo" == true ]]; then
        install_copy "$SYSTEM_BIN" sudo -n || true
    elif [[ "$user_installed" != true ]]; then
        printf '%s\n' \
            'Oxterm: /usr/bin/oxterm not refreshed (sudo required); run ./update.sh' >&2
    fi
fi

exit 0
