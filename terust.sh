#!/bin/bash
# TPGK (Rust port) launcher — mirrors tpgk.sh behaviour.
SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
BIN="$SCRIPT_DIR/target/release/terust"
if [ ! -x "$BIN" ]; then
    BIN="$SCRIPT_DIR/target/debug/terust"
fi
if [ ! -x "$BIN" ]; then
    echo "terust: binary not found, run 'cargo build --release' first" >&2
    exit 1
fi
if [ -d "$1" ]; then
    cd "$1" || cd "$HOME"
    shift
fi
exec "$BIN" "$@"
