#!/usr/bin/env bash
#
# Regenerate the screenshots used by the README.
#
# The script runs a throwaway Oxterm instance on a private X display with its
# own HOME, so the user's settings, history and notes are never touched. It
# drives the window through the control API, captures it with ImageMagick and
# writes the PNG files under docs/screenshots/.
#
# Requirements: a built binary (cargo build --release), Xvnc (tigervnc),
# xdotool, ImageMagick (import) and xfwm4. On Debian/Ubuntu:
#
#   sudo apt install tigervnc-standalone-server xdotool imagemagick xfwm4
#
# Usage:
#
#   scripts/capture-screenshots.sh
#   OXTERM_SCREENSHOT_DISPLAY=:98 scripts/capture-screenshots.sh
#   OXTERM_BIN=./oxterm-linux-x86-64 scripts/capture-screenshots.sh

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT_DIR/docs/screenshots"
DISPLAY_NUM="${OXTERM_SCREENSHOT_DISPLAY:-:97}"
FONT="${OXTERM_SCREENSHOT_FONT:-MesloLGL Nerd Font Mono}"
SCHEME="${OXTERM_SCREENSHOT_SCHEME:-Gruvbox Dark}"

die() {
    printf 'capture-screenshots: %s\n' "$*" >&2
    exit 1
}

for tool in Xvnc xdotool import; do
    command -v "$tool" >/dev/null 2>&1 || die "missing '$tool' (see the header of this script)"
done

BIN="${OXTERM_BIN:-$ROOT_DIR/target/release/oxterm}"
[ -x "$BIN" ] || die "'$BIN' not found; run 'cargo build --release' first"

mkdir -p "$OUT_DIR"

TMP_DIR="$(mktemp -d /tmp/oxterm-shots.XXXXXX)"
export HOME="$TMP_DIR/home"
mkdir -p "$HOME"

GUI_PID=""
XVNC_PID=""
WM_PID=""

cleanup() {
    for pid in "$GUI_PID" "$WM_PID" "$XVNC_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# Isolated desktop.
Xvnc "$DISPLAY_NUM" -geometry 1280x900 -depth 24 -SecurityTypes None \
    -AlwaysShared -dpi 96 -nolisten tcp >"$TMP_DIR/xvnc.log" 2>&1 &
XVNC_PID=$!
export DISPLAY="$DISPLAY_NUM"
for _ in $(seq 1 100); do
    xdotool getdisplaygeometry >/dev/null 2>&1 && break
    sleep 0.1
done
xdotool getdisplaygeometry >/dev/null 2>&1 || die "cannot start Xvnc on $DISPLAY_NUM (try another display)"

if command -v xfwm4 >/dev/null 2>&1; then
    xfwm4 --replace >"$TMP_DIR/xfwm.log" 2>&1 &
    WM_PID=$!
    sleep 0.5
fi

"$BIN" --config "$TMP_DIR/settings.json" --no-restore \
    -g 132x26 \
    -o login_shell=false \
    -o shell_command=/bin/bash \
    -o font_name="$FONT" \
    -o font_size=13 \
    -o color_scheme="$SCHEME" \
    -o show_stats=true \
    -o dynamic_title=hide \
    -o tab_title="shell" \
    >"$TMP_DIR/oxterm.log" 2>&1 &
GUI_PID=$!

# Wait for the control socket published by this GUI process.
CONTROL=""
for _ in $(seq 1 100); do
    for dir in "$HOME/.config/oxterm/remote" "/tmp/oxterm-$(id -u)/remote"; do
        candidate="$dir/control-$GUI_PID.sock"
        if [ -S "$candidate" ]; then
            CONTROL="$candidate"
            break 2
        fi
    done
    sleep 0.1
done
[ -n "$CONTROL" ] || die "the GUI did not publish a control socket"
export OXTERM_CONTROL="$CONTROL"

# The first pane of a GUI is its session id ending in "-0".
first_pane() {
    "$BIN" --list 2>/dev/null |
        awk -v prefix="$GUI_PID-" 'index($1, prefix) == 1 && $1 ~ /-0$/ { print $1; exit }'
}

PANE0=""
for _ in $(seq 1 100); do
    PANE0="$(first_pane)"
    [ -n "$PANE0" ] && break
    sleep 0.1
done
[ -n "$PANE0" ] || die "could not find the first terminal pane"

window_id() {
    for _ in $(seq 1 100); do
        id="$(xdotool search --onlyvisible --pid "$GUI_PID" 2>/dev/null | head -1)"
        [ -n "$id" ] && { printf '%s\n' "$id"; return 0; }
        sleep 0.1
    done
    return 1
}

WINDOW="$(window_id)" || die "the Oxterm window did not appear"

# Give the shell a neutral, readable prompt so the capture does not expose the
# real user@host, then build the layout shown in the screenshots.
"$BIN" send --pane "$PANE0" --enter 'export PROMPT_COMMAND=""' >/dev/null
"$BIN" send --pane "$PANE0" --enter 'export PS1="\[\e[1;32m\]oxterm\[\e[0m\] $ "' >/dev/null
"$BIN" send --pane "$PANE0" --enter "cd $ROOT_DIR" >/dev/null

# Two more tabs so the tab bar shows in the capture.
PANE1="$("$BIN" new --title "tests" 2>/dev/null)"
"$BIN" send --pane "$PANE1" --enter "ls tests" >/dev/null
"$BIN" send --pane "$PANE1" --enter "wc -l src/*.rs" >/dev/null
PANE2="$("$BIN" new --title "git" 2>/dev/null)"
"$BIN" send --pane "$PANE2" --enter "git --no-pager log --oneline -5" >/dev/null

# `clear` hides the prompt setup above; the remaining output is what we capture.
"$BIN" send --pane "$PANE0" --enter "clear" >/dev/null
"$BIN" send --pane "$PANE0" --enter "ls" >/dev/null
"$BIN" send --pane "$PANE0" --enter "git --no-pager log --oneline --decorate -8" >/dev/null

# Back to the first tab (the control API always activates new tabs).
xdotool windowactivate --sync "$WINDOW" 2>/dev/null || true
xdotool key --clearmodifiers ctrl+Page_Up
xdotool key --clearmodifiers ctrl+Page_Up
sleep 2

# The startup tab is numbered ("shell 0"); rename it so the tab bar looks tidy.
# Doing it here also pins the window title, because the shell prompt no longer
# overrides a tab the user has renamed.
xdotool windowactivate --sync "$WINDOW" 2>/dev/null || true
sleep 0.3
xdotool key --clearmodifiers ctrl+shift+s
sleep 0.5
xdotool key --clearmodifiers ctrl+a
xdotool type --delay 20 "shell"
xdotool key Return
sleep 0.5

xdotool windowactivate --sync "$WINDOW" 2>/dev/null || true
sleep 0.3

import -window "$WINDOW" "$OUT_DIR/main.png"

# Command bar over the same layout.
xdotool key --clearmodifiers ctrl+shift+p
sleep 0.7
import -window "$WINDOW" "$OUT_DIR/command-bar.png"
xdotool key --clearmodifiers Escape
sleep 0.3

# A wider window and a vertical split, both panes showing real command output.
xdotool windowsize "$WINDOW" 1260 680
xdotool windowmove "$WINDOW" 0 0
sleep 0.5
PANE3="$("$BIN" split --pane "$PANE0" --vertical --cwd "$ROOT_DIR" --title "src" 2>/dev/null)"
if [ -n "$PANE3" ]; then
    "$BIN" send --pane "$PANE3" --enter 'export PROMPT_COMMAND=""' >/dev/null
    "$BIN" send --pane "$PANE3" --enter 'export PS1="\[\e[1;32m\]oxterm\[\e[0m\] $ "' >/dev/null
    "$BIN" send --pane "$PANE3" --enter "clear" >/dev/null
    "$BIN" send --pane "$PANE3" --enter "ls src" >/dev/null
fi
"$BIN" send --pane "$PANE0" --enter "clear" >/dev/null
"$BIN" send --pane "$PANE0" --enter "git --no-pager log --oneline --decorate -8" >/dev/null
sleep 2
import -window "$WINDOW" "$OUT_DIR/splits.png"

printf 'wrote %s\n' "$OUT_DIR/main.png" "$OUT_DIR/command-bar.png" "$OUT_DIR/splits.png"
