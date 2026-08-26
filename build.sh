#!/usr/bin/env bash
# build.sh - Build the definitive Oxterm release and always copy it to
# oxterm-linux-x86-64 in the project root at the end of the compilation.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"
install -m 0755 "${ROOT_DIR}/target/release/oxterm" "${ROOT_DIR}/oxterm-linux-x86-64"

printf '%s\n' "Oxterm: copied definitive build to ${ROOT_DIR}/oxterm-linux-x86-64"
