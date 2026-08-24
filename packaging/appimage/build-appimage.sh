#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT_DIR}/Cargo.toml" | head -n 1)}"
ARCH="${ARCH:-$(uname -m)}"
APPDIR="${ROOT_DIR}/target/appimage/TRust.AppDir"
OUTPUT="${ROOT_DIR}/target/TRust-${VERSION}-${ARCH}.AppImage"

command -v appimagetool >/dev/null 2>&1 || {
    printf '%s\n' "appimagetool is required to build an AppImage" >&2
    exit 1
}

cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"
rm -rf "${APPDIR}"
install -Dm755 "${ROOT_DIR}/target/release/TRust" "${APPDIR}/usr/bin/TRust"
install -Dm644 "${ROOT_DIR}/packaging/appimage/TRust.desktop" "${APPDIR}/TRust.desktop"
install -Dm644 "${ROOT_DIR}/packaging/appimage/TRust.svg" "${APPDIR}/TRust.svg"
ln -s usr/bin/TRust "${APPDIR}/AppRun"
appimagetool "${APPDIR}" "${OUTPUT}"
printf '%s\n' "Created ${OUTPUT}"
