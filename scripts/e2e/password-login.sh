#!/usr/bin/env bash
#
# password-login.sh - the whole stored-password chain, with nothing faked but the
# server: real ssh, real oxterm-askpass, and a real keyring in a private D-Bus
# session. Run by hand when the helper or the prompt rules change, since it needs
# python-paramiko, gnome-keyring and libsecret's secret-tool.
#
# Usage:
#   scripts/e2e/password-login.sh [path/to/oxterm-askpass]

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${1:-$DIR/../../target/release/oxterm-askpass}"
HELPER="$(realpath "$HELPER")"

if [[ -z "${OXTERM_E2E_IN_DBUS:-}" ]]; then
    OXTERM_E2E_IN_DBUS=1 exec dbus-run-session -- "$0" "$HELPER"
fi

# A private HOME: the keyring and the attempt markers never touch real ones.
TMP="$(mktemp -d "${TMPDIR:-/tmp}/oxp.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
export HOME="$TMP" XDG_DATA_HOME="$TMP/share"
mkdir -p "$XDG_DATA_HOME"
printf 'test\n' | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1
sleep 1
printf 's3cret-pw' | secret-tool store --label='Oxterm e2e' oxterm-connection good
printf 'wrong-pw' | secret-tool store --label='Oxterm e2e' oxterm-connection bad

client() {
    rm -rf "$HOME/.config/oxterm/askpass"
    python3 -I "$DIR/password-client.py" "$HOME" "$HELPER" "$@"
}

client password good login
client keyboard-interactive good login
# A wrong password goes out exactly once, then the person is asked.
client password bad 1
client keyboard-interactive bad 1
# Nothing stored: the person is asked and nothing is sent.
client password absent 0
printf '%s\n' 'password-login: ok'
