#!/bin/bash
# TPGK (Rust port) launcher — always runs a binary built from the current source.
# If the sources are newer than the available binary, it rebuilds before launching,
# so every new instance picks up the latest source and configuration changes.
set -u

SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
BIN="$SCRIPT_DIR/target/release/terust"
if [ ! -x "$BIN" ]; then
    BIN="$SCRIPT_DIR/target/debug/terust"
fi

needs_rebuild() {
    if [ ! -x "$BIN" ]; then
        return 0
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        return 1
    fi
    find "$SCRIPT_DIR/src" "$SCRIPT_DIR/Cargo.toml" "$SCRIPT_DIR/Cargo.lock" \
        -newer "$BIN" -print -quit 2>/dev/null | grep -q .
}

if needs_rebuild; then
    echo "terust: rebuilding from source..." >&2
    if ! cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml" 2>&1; then
        echo "terust: build failed, running existing binary" >&2
    fi
    BIN="$SCRIPT_DIR/target/release/terust"
    if [ ! -x "$BIN" ]; then
        BIN="$SCRIPT_DIR/target/debug/terust"
    fi
fi

if [ ! -x "$BIN" ]; then
    echo "terust: binary not found, run 'cargo build --release' first" >&2
    exit 1
fi

if [ "$#" -gt 0 ] && [ -d "$1" ]; then
    cd "$1" || cd "$HOME"
    shift
fi
exec "$BIN" "$@"
