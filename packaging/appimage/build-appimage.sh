#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${ROOT_DIR}/Cargo.toml" | head -n 1)}"
ARCH="${ARCH:-$(uname -m)}"
APPDIR="${ROOT_DIR}/target/appimage/oxterm.AppDir"
OUTPUT="${ROOT_DIR}/target/oxterm-${VERSION}-${ARCH}.AppImage"

command -v appimagetool >/dev/null 2>&1 || {
    printf '%s\n' "appimagetool is required to build an AppImage" >&2
    exit 1
}

cargo build --release --locked --manifest-path "${ROOT_DIR}/Cargo.toml"
rm -rf "${APPDIR}"
install -Dm755 "${ROOT_DIR}/target/release/oxterm" "${APPDIR}/usr/bin/oxterm"
install -Dm644 "${ROOT_DIR}/packaging/appimage/oxterm.desktop" "${APPDIR}/oxterm.desktop"
install -Dm644 "${ROOT_DIR}/packaging/appimage/oxterm.svg" "${APPDIR}/oxterm.svg"
ln -s usr/bin/oxterm "${APPDIR}/AppRun"
appimagetool "${APPDIR}" "${OUTPUT}"
printf '%s\n' "Created ${OUTPUT}"
