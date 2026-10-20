#!/usr/bin/env bash
# update.sh - Pull the latest source and install the refreshed Oxterm binary.
#
# Usage:
#   ./update.sh                 # pull, build, install (/usr/bin when possible)
#   ./update.sh --user          # pull, build, install under ~/.local
#   ./update.sh --no-pull       # build and install the current checkout
#   ./update.sh --no-install    # pull and build only
#
# The script enables the repository hooks, so the next `git pull` also rebuilds
# the binary automatically (see .githooks/post-merge).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

PULL=true
INSTALL=true
USER_INSTALL=false
USER_PREFIX="${PREFIX:-${HOME}/.local}"

usage() {
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --user) USER_INSTALL=true ;;
        --no-pull) PULL=false ;;
        --no-install) INSTALL=false ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unknown option: %s\n' "$arg" >&2; exit 1 ;;
    esac
done

if ! git rev-parse --git-dir >/dev/null 2>&1; then
    printf '%s\n' 'Oxterm: update.sh must run inside the Git repository.' >&2
    exit 1
fi

if [[ "$PULL" == true ]]; then
    printf '%s\n' 'Oxterm: pulling the latest changes...'
    # The post-merge hook would rebuild as well; skip it and do the work once.
    OXTERM_SKIP_HOOKS=1 git pull --ff-only
fi

printf '%s\n' 'Oxterm: building the release binary...'
"${ROOT_DIR}/build.sh"

BUILT="${ROOT_DIR}/target/release/oxterm"
if [[ ! -x "$BUILT" ]]; then
    printf '%s\n' 'Oxterm: the release build did not produce a binary.' >&2
    exit 1
fi

if [[ "$INSTALL" == true ]]; then
    # Reuse the installers so the man page, completions and documentation stay
    # in sync with the binary; they skip the build because build.sh just ran.
    if [[ "$USER_INSTALL" == true ]] || \
        { [[ ! -w "$(dirname /usr/bin/oxterm)" ]] && ! command -v sudo >/dev/null 2>&1; }; then
        PREFIX="$USER_PREFIX" "$ROOT_DIR/setup.sh" --no-build
    else
        "$ROOT_DIR/install.sh" --no-build
    fi
fi

# Keep the hooks enabled for this clone so a plain `git pull` stays up to date.
git config core.hooksPath .githooks 2>/dev/null || true

printf '%s\n' "Oxterm: ready ($("$BUILT" --version 2>/dev/null || printf 'unknown version'))"
