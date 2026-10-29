#!/usr/bin/env bash
#
# smoke-gui.sh - start the GUI, drive it through the control API, shut it down.
#
# Usage:
#   ./scripts/smoke-gui.sh
#
# This is the run-time counterpart of `cargo build`: it catches what a compile
# cannot see. A window that never becomes ready, a control action that stopped
# working, a symbol resolved at run time that is missing on the host VTE, or a
# process that hangs on exit all pass a build and fail here.
#
# The CI job that builds against the oldest supported VTE (0.70, Debian 12)
# runs it under xvfb-run, so a regression that only shows on old VTE is caught
# before a release instead of by a user.
#
# Environment:
#   OXTERM_BIN   binary to smoke test (default: target/release/oxterm)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${OXTERM_BIN:-${ROOT_DIR}/target/release/oxterm}"

if [[ ! -x "${BIN}" ]]; then
    printf '%s\n' "smoke-gui: ${BIN} is missing; run 'cargo build --release' first" >&2
    exit 1
fi

if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    printf '%s\n' "smoke-gui: no DISPLAY or WAYLAND_DISPLAY; run under xvfb-run" >&2
    exit 1
fi

# The script may run inside an Oxterm pane, where the shell integration exports
# OXTERM_CONTROL pointing at *that* instance: an action would then land in the
# developer's terminal instead of the one under test.
unset OXTERM_CONTROL OXTERM_PANE OXTERM_SHELL_INTEGRATION OXTERM_SSH_SHARING
unset OXTERM_OSC133_FIFO OXTERM_OSC133_SCRIPT

# Keep the temporary HOME short: the control socket lives under it and Unix
# socket paths are limited to 107 bytes. Point the XDG directories there too so
# nothing (a cache file, a theme, a session) touches the real profile.
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oxs.XXXXXX")"
export HOME="${TMP_DIR}/h"
export XDG_CONFIG_HOME="${HOME}/.config"
export XDG_DATA_HOME="${HOME}/.local/share"
export XDG_STATE_HOME="${HOME}/.local/state"
export XDG_CACHE_HOME="${HOME}/.cache"
export XDG_RUNTIME_DIR="${HOME}/run"
mkdir -p "${HOME}" "${XDG_RUNTIME_DIR}" "${XDG_CACHE_HOME}"
chmod 700 "${XDG_RUNTIME_DIR}"

LOG="${TMP_DIR}/app.log"
APP_PID=""

cleanup() {
    if [[ -n "${APP_PID}" ]]; then
        kill "${APP_PID}" 2>/dev/null || true
        wait "${APP_PID}" 2>/dev/null || true
    fi
    rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

fail() {
    printf 'smoke-gui: %s\n' "$*" >&2
    printf -- '--- application log ---\n' >&2
    cat "${LOG}" >&2 || true
    exit 1
}

wait_for() {
    local deadline=$((SECONDS + 30))
    while (( SECONDS < deadline )); do
        if "$@"; then
            return 0
        fi
        kill -0 "${APP_PID}" 2>/dev/null || return 1
        sleep 0.05
    done
    return 1
}

# `--list` prints a header and one line per window; the first column is the
# session id, which starts with the GUI process id. Matching that id is what
# proves the session belongs to the instance this script started, not to a
# window that happened to be running already.
has_our_session() {
    "${BIN}" --list 2>/dev/null |
        awk -v prefix="${APP_PID}-" 'NR > 1 && index($1, prefix) == 1 { found = 1 } END { exit !found }'
}

# Ask for non-default font rendering: on VTE older than 0.74 that path has to
# skip the call resolved with dlsym instead of crashing.
"${BIN}" --no-restore \
    -o font_rendering=grayscale -o font_hinting=full >"${LOG}" 2>&1 &
APP_PID=$!

wait_for has_our_session || fail "the window did not become ready"
printf '%s\n' "smoke-gui: window ready"

pane="$("${BIN}" new --title smoke 2>>"${LOG}")" || fail "the new action failed"
pane="${pane%%$'\n'*}"
[[ -n "${pane}" ]] || fail "the new action printed no pane id"
[[ "${pane}" == "${APP_PID}-"* ]] ||
    fail "the new tab belongs to another instance: ${pane}"

# Type into the new tab and read the result back: this covers the PTY, the
# terminal widget and the buffer that capture reads.
marker="smoke-$(date +%s)"
"${BIN}" send --pane "${pane}" --enter "printf '%s\\n' ${marker}" 2>>"${LOG}" ||
    fail "the send action failed"

shows_marker() {
    "${BIN}" capture --pane "${pane}" 2>/dev/null | grep -qF "${marker}"
}
wait_for shows_marker || fail "the command typed into the tab never showed its output"
printf '%s\n' "smoke-gui: control API round trip ok"

# Closing must end the process: a hung window keeps its shells and brokers
# alive long after the user is gone.
kill "${APP_PID}" 2>/dev/null || true
for _ in $(seq 200); do
    kill -0 "${APP_PID}" 2>/dev/null || break
    sleep 0.05
done
if kill -0 "${APP_PID}" 2>/dev/null; then
    fail "the window did not exit after SIGTERM"
fi
wait "${APP_PID}" 2>/dev/null || true
APP_PID=""

if grep -q "font_options_unavailable" "${LOG}"; then
    printf '%s\n' "smoke-gui: VTE is older than 0.74, font rendering options were skipped"
fi
printf '%s\n' "smoke-gui: ok"
