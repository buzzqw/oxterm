#!/usr/bin/env bash
#
# smoke-connections.sh - drive the connections panel and form as a person would.
#
# The panel and the form have no control API on purpose, so this types into
# them with xdotool. `ssh` and `secret-tool` are fakes that record what they
# receive: the point is what Oxterm asks them to do, and no network is involved.
#
# Usage:
#   xvfb-run -a ./scripts/smoke-connections.sh
#
# Environment:
#   OXTERM_BIN   binary under test (default: target/release/oxterm)
#   OXTERM_SMOKE_SHOTS   directory for a screenshot of each failure

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${OXTERM_BIN:-${ROOT_DIR}/target/release/oxterm}"

die() {
    printf 'smoke-connections: %s\n' "$*" >&2
    if [[ -n "${OXTERM_SMOKE_SHOTS:-}" ]] && command -v import >/dev/null; then
        import -window root "${OXTERM_SMOKE_SHOTS}/failure.png" || true
    fi
    exit 1
}

[[ -x "$BIN" ]] || die "$BIN is missing; run 'cargo build --release --bins' first"
[[ -n "${DISPLAY:-}" ]] || die "no DISPLAY; run under xvfb-run"
command -v xdotool >/dev/null || die "xdotool is not installed"

unset OXTERM_CONTROL OXTERM_PANE OXTERM_SHELL_INTEGRATION OXTERM_SSH_SHARING
unset OXTERM_OSC133_FIFO OXTERM_OSC133_SCRIPT WAYLAND_DISPLAY

# Short HOME: the control socket lives under it. English dialog buttons.
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oxc.XXXXXX")"
export HOME="${TMP_DIR}/h" LC_ALL=C LANGUAGE=C
export XDG_CONFIG_HOME="${HOME}/.config" XDG_DATA_HOME="${HOME}/.local/share"
export XDG_STATE_HOME="${HOME}/.local/state" XDG_CACHE_HOME="${HOME}/.cache"
export XDG_RUNTIME_DIR="${HOME}/run"
mkdir -p "$HOME/bin" "$XDG_RUNTIME_DIR" "$XDG_CACHE_HOME" "$HOME/.ssh"
chmod 700 "$XDG_RUNTIME_DIR"
export PATH="$HOME/bin:$PATH"

SSH_LOG="$HOME/ssh.log"
KEYRING_LOG="$HOME/keyring.log"
BOOK="$HOME/.config/oxterm/connections.json"

cat >"$HOME/bin/ssh" <<EOF
#!/bin/sh
if [ "\$1" = "-G" ]; then
    printf 'user tester\nhostname h.example\ncontrolpath none\nserveraliveinterval 0\n'
    exit 0
fi
printf '%s\n' "\$*" >> '$SSH_LOG'
exec sleep 300
EOF
cat >"$HOME/bin/secret-tool" <<EOF
#!/bin/sh
printf '%s\n' "\$1" >> '$KEYRING_LOG'
[ "\$1" = store ] && cat >> '$HOME/keyring.secret'
exit 0
EOF
chmod +x "$HOME/bin/ssh" "$HOME/bin/secret-tool"

APP_PID=""
cleanup() {
    if [[ -n "$APP_PID" ]]; then
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

wait_for() {
    local what="$1"
    shift
    local _
    for _ in $(seq 1 100); do
        if "$@" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    die "timed out waiting for $what"
}

keys() {
    xdotool key --clearmodifiers --delay 80 "$@"
    sleep 0.4
}

type_text() {
    xdotool type --clearmodifiers --delay 15 -- "$1"
    sleep 0.2
}

ssh_lines() {
    if [[ -f "$SSH_LOG" ]]; then wc -l <"$SSH_LOG"; else echo 0; fi
}

ssh_lines_are() {
    [[ "$(ssh_lines)" == "$1" ]]
}

# With no window manager nothing gives the focus back when a dialog closes.
refocus() {
    xdotool windowfocus "$WINDOW" 2>/dev/null || true
    sleep 0.3
}

form_is_open() {
    xdotool search --onlyvisible --name "$1" >/dev/null 2>&1
}

"$BIN" >"$TMP_DIR/app.log" 2>&1 &
APP_PID=$!
wait_for "the control socket" "$BIN" new --title first
sleep 1
WINDOW="$(xdotool search --onlyvisible --pid "$APP_PID" | tail -1)"
[[ -n "$WINDOW" ]] || die "the window did not appear"
xdotool windowfocus "$WINDOW" 2>/dev/null || true
xdotool mousemove --window "$WINDOW" 300 300
sleep 0.5

# ── Create an entry through the form ──
keys ctrl+shift+j
keys ctrl+n
wait_for "the form" form_is_open "New Connection"
type_text "smoke-box"
# Mnemonics rather than Tab: with no window manager, as under xvfb-run, Tab does not
# leave a combo box such as Protocol.
keys alt+h
type_text "server.example"
keys alt+u
type_text "admin"
keys alt+r
type_text "2200"
keys Return
wait_for "the saved entry" grep -q '"smoke-box"' "$BOOK"
"$BIN" connections --json | grep -q '"command":"ssh -l admin -p 2200 server.example"' ||
    die "the form did not save the expected command"
printf '%s\n' "smoke-connections: form saved the entry"

# ── A bad value keeps the form open, and nothing is saved ──
refocus
keys ctrl+n
wait_for "the form" form_is_open "New Connection"
type_text "bad-box"
keys alt+h
type_text "-oProxyCommand=evil"
keys Return
sleep 0.5
grep -q 'bad-box' "$BOOK" && die "the form saved an invalid host"
form_is_open "New Connection" || die "the form closed on an invalid host"
keys Escape
refocus
printf '%s\n' "smoke-connections: form refused an option as a host"

# ── Search and connect from the panel ──
type_text "smoke"
sleep 0.5
keys Return
wait_for "the panel connection" ssh_lines_are 1
grep -q -- '-l admin -p 2200 server.example$' "$SSH_LOG" ||
    die "unexpected command: $(cat "$SSH_LOG")"
grep -q -- '-o ControlMaster=auto' "$SSH_LOG" || die "connection sharing is missing"
printf '%s\n' "smoke-connections: panel opened the connection"

# ── Store a password through the form: keyring only, never the book ──
# Connecting moved the focus to the new terminal, as it should. Back to the panel.
keys ctrl+shift+j
# Choose the method in the file: closing a combo box popup leaves no X input
# focus when no window manager runs, as under xvfb-run. Saving must keep it.
sed -i 's/"host": "server.example"/"auth": "secret_service",\n      &/' "$BOOK"
keys ctrl+shift+j
keys Down
keys F2
wait_for "the form" form_is_open "Edit Connection"
keys ctrl+Next
keys alt+p
type_text "hunter2-smoke"
keys Return
wait_for "the keyring write" grep -q '^store$' "$KEYRING_LOG"
refocus
grep -q 'secret_service' "$BOOK" || die "saving lost the method"
grep -q 'hunter2-smoke' "$BOOK" && die "the password reached the book"
grep -q 'hunter2-smoke' "$HOME/keyring.secret" || die "the keyring did not get the password"
printf '%s\n' "smoke-connections: password went to the keyring only"

# ── The panel closes by shortcut with the focus inside it ──
keys ctrl+shift+j
sleep 0.3

# ── Palette and /ssh, from the terminal ──
keys ctrl+shift+p
type_text "connect smoke"
sleep 0.4
keys Return
wait_for "the palette connection" ssh_lines_are 2
printf '%s\n' "smoke-connections: palette opened the connection"

# Oxterm reads /commands at a local prompt, so this needs a shell tab rather than
# the fake ssh session that has the focus now.
SHELL_PANE="$("$BIN" new --title shell)"
sleep 1
type_text "/help"
keys Return
sleep 0.5
"$BIN" capture --pane "$SHELL_PANE" --all | grep -q "Oxterm Commands" ||
    die "a /command typed at the prompt reached the shell"
sleep 0.5
type_text "/ssh smoke-box"
keys Return
wait_for "the /ssh connection" ssh_lines_are 3
printf '%s\n' "smoke-connections: /ssh opened the connection"

# ── Delete: the entry and its stored password both go ──
keys ctrl+shift+j
xdotool key --clearmodifiers ctrl+a
type_text "smoke"
sleep 0.4
keys Down
keys Delete
keys alt+y
wait_for "the deletion" bash -c "! grep -q 'smoke-box' '$BOOK'"
wait_for "the keyring cleanup" grep -q '^clear$' "$KEYRING_LOG"
printf '%s\n' "smoke-connections: deleting removed the entry and its password"

# ── A detached tab can come back, with its session untouched ──
keys Escape
LINES_BEFORE="$(ssh_lines)"
"$BIN" new --title back-and-forth -- sh -c 'echo still-the-same-session; exec sleep 300' >/dev/null
sleep 1
BACK_PANE="$("$BIN" --list | awk -F'\t' '/back-and-forth/ {print $1}' | tail -1)"
[[ -n "$BACK_PANE" ]] || die "the test tab did not appear"
keys ctrl+shift+p
type_text "detach tab"
keys Return
sleep 1
xdotool search --onlyvisible --name 'back-and-forth' windowfocus 2>/dev/null || true
sleep 0.3
keys ctrl+shift+p
type_text "return to main"
keys Return
sleep 1
"$BIN" capture --pane "$BACK_PANE" | grep -q still-the-same-session ||
    die "the session did not survive coming back"
[[ "$("$BIN" --list | grep -c "$BACK_PANE")" == 1 ]] || die "the pane was lost"
[[ "$(ssh_lines)" == "$LINES_BEFORE" ]] || die "coming back opened a connection"
printf '%s\n' "smoke-connections: a detached tab came back"

# ── A restored session asks, then reopens connections where they were ──
kill "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
: >"$SSH_LOG"
cat >"$BOOK" <<EOF
{"version":1,"connections":[
  {"id":"left1","name":"left-box","host":"left.example"},
  {"id":"right1","name":"right-box","host":"right.example","color_scheme":"Gruvbox Dark"}
]}
EOF
mkdir -p "$HOME/.config/oxterm/sessions"
cat >"$HOME/.config/oxterm/sessions/last.json" <<EOF
{"timestamp":"2026-01-01T00:00:00Z","split_mode":"vertical",
 "tabs_left":[{"base_title":"left-box","title":"left-box","cwd":"$HOME","custom_title":true,"connection_id":"left1"}],
 "tabs_right":[{"base_title":"right-box","title":"right-box","cwd":"$HOME","custom_title":true,"connection_id":"right1"}]}
EOF
"$BIN" >"$TMP_DIR/app2.log" 2>&1 &
APP_PID=$!
sleep 3
[[ ! -s "$SSH_LOG" ]] || die "a restored session connected without asking"
WINDOW="$(xdotool search --onlyvisible --name 'Oxterm|last session' | tail -1)"
xdotool windowfocus "$WINDOW" 2>/dev/null || true
keys alt+y
wait_for "the reconnected tabs" ssh_lines_are 2
sed -n 1p "$SSH_LOG" | grep -q 'left.example$' || die "wrong order: $(cat "$SSH_LOG")"
sed -n 2p "$SSH_LOG" | grep -q 'right.example$' || die "wrong order: $(cat "$SSH_LOG")"
if [[ -n "${OXTERM_SMOKE_SHOTS:-}" ]] && command -v import >/dev/null; then
    sleep 0.5
    import -window root "${OXTERM_SMOKE_SHOTS}/restored.png" || true
fi
printf '%s\n' "smoke-connections: restore asked first and kept the order"

printf '%s\n' "smoke-connections: ok"
