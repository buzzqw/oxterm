#!/usr/bin/env bash
# build.sh - Build the definitive terust release and always copy it to
# terust-linux-x86-64 in the project root at the end of the compilation.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cargo build --release --manifest-path "${ROOT_DIR}/Cargo.toml"
install -m 0755 "${ROOT_DIR}/target/release/terust" "${ROOT_DIR}/terust-linux-x86-64"

printf '%s\n' "terust: copied definitive build to ${ROOT_DIR}/terust-linux-x86-64"
