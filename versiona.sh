#!/usr/bin/env bash
# versiona.sh - Release automation for Oxterm.
#
# Flow:
#   1. Calculate or accept the next Cargo semver version.
#   2. Generate release notes from commits after the last tag.
#   3. Update Cargo.toml and the AUR metadata, create a release commit and
#      annotated tag.
#   4. Push the branch and tag, then create the GitHub release.
#   5. Build and install the local binary the user launches (skip with --no-install).
#
# Usage:
#   ./versiona.sh --dry-run
#   ./versiona.sh                 # bump the patch component
#   ./versiona.sh 1.2.0           # release an explicit version
#   ./versiona.sh --no-install    # release without updating the local binary
#
# The local binary is installed in the same place used by the desktop launcher
# (see install.sh): /usr/bin/oxterm when it is writable or sudo is available,
# otherwise ${HOME}/.local/bin/oxterm. The documented root copy
# (oxterm-linux-x86-64, see build.sh) is refreshed as well.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info() { printf '%b\n' "${CYAN}i${RESET} $*"; }
ok() { printf '%b\n' "${GREEN}OK${RESET} $*"; }
warn() { printf '%b\n' "${YELLOW}Warning:${RESET} $*" >&2; }
err() { printf '%b\n' "${RED}Error:${RESET} $*" >&2; exit 1; }
separator() { printf '%b\n' "${BOLD}----------------------------------------------${RESET}"; }

DRY_RUN=false
SKIP_INSTALL=false
REQUESTED_VERSION=""
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        --no-install) SKIP_INSTALL=true ;;
        -h|--help)
            sed -n '1,22p' "$0"
            exit 0
            ;;
        v*) REQUESTED_VERSION="${arg#v}" ;;
        *) REQUESTED_VERSION="$arg" ;;
    esac
done

git rev-parse --git-dir >/dev/null 2>&1 || err "Run this script inside a Git repository."
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

configure_external_proxy() {
    if (exec 3<>/dev/tcp/127.0.0.1/8088) 2>/dev/null; then
        exec 3>&-
        exec 3<&-
        local proxy="http://127.0.0.1:8088"
        local mitm_ca="${MITM_CA_FILE:-${HOME}/.mitmproxy/mitmproxy-ca-cert.pem}"
        export http_proxy="$proxy"
        export https_proxy="$proxy"
        export HTTP_PROXY="$proxy"
        export HTTPS_PROXY="$proxy"
        if [[ -f "$mitm_ca" ]]; then
            export SSL_CERT_FILE="$mitm_ca"
            export CURL_CA_BUNDLE="$mitm_ca"
            export GIT_SSL_CAINFO="$mitm_ca"
            info "Using local MITM CA at ${mitm_ca}."
        fi
        info "Using local MITM proxy at ${proxy}."
    fi
}

configure_external_proxy

if [[ "$DRY_RUN" == false && -n "$(git status --porcelain)" ]]; then
    err "The working tree must be clean before creating a release."
fi

if [[ "$DRY_RUN" == false ]]; then
    command -v gh >/dev/null 2>&1 || err "GitHub CLI (gh) is required."
    gh auth status >/dev/null 2>&1 || \
        err "GitHub CLI is not authenticated or its token is invalid. Run 'gh auth login' or 'gh auth refresh -h github.com'."
fi

detect_main_branch() {
    local ref
    ref="$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's|.*/||' || true)"
    [[ -n "$ref" ]] && { printf '%s\n' "$ref"; return; }
    for branch in main master; do
        git show-ref --verify --quiet "refs/heads/$branch" && {
            printf '%s\n' "$branch"
            return
        }
    done
    git branch --show-current
}

# Keep packaging/aur in sync with the Cargo version; the "AUR package"
# workflow fails when PKGBUILD or .SRCINFO lag behind a release.
sync_aur_version() {
    local pkgbuild="packaging/aur/PKGBUILD"
    local srcinfo="packaging/aur/.SRCINFO"

    [[ -f "$pkgbuild" ]] || return 0

    sed -i -E "s/^pkgver=.*/pkgver=${VERSION}/" "$pkgbuild"

    if [[ -f "$srcinfo" ]]; then
        sed -i -E \
            -e "s/^(\tpkgver = ).*/\1${VERSION}/" \
            -e "s|^(\tsource = .*#tag=v)[0-9.]*$|\1${VERSION}|" \
            "$srcinfo"
    fi
}

MAIN_BRANCH="$(detect_main_branch)"
CURRENT_VERSION="$(sed -n 's/^version = "\([0-9][0-9.]*\)"/\1/p' Cargo.toml | head -n 1)"
[[ "$CURRENT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    err "Could not read a semver package version from Cargo.toml."

if [[ -n "$REQUESTED_VERSION" ]]; then
    VERSION="$REQUESTED_VERSION"
else
    IFS=. read -r major minor patch <<< "$CURRENT_VERSION"
    VERSION="${major}.${minor}.$((patch + 1))"
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    err "Version must use MAJOR.MINOR.PATCH format."

TAG_VERSION="v${VERSION}"
LATEST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"
if [[ -n "$LATEST_TAG" ]]; then
    RAW_COMMITS="$(git log "${LATEST_TAG}..HEAD" --pretty=format:'%s' --no-merges || true)"
else
    RAW_COMMITS="$(git log --pretty=format:'%s' --no-merges || true)"
fi
FILTERED_COMMITS="$(printf '%s\n' "$RAW_COMMITS" | grep -Eiv '^chore: (release|bump|version) ' || true)"

generate_release_notes() {
    local commits="$1"
    local features="" fixes="" improvements="" maintenance=""
    while IFS= read -r subject; do
        [[ -z "$subject" ]] && continue
        local clean="${subject#*: }"
        case "$subject" in
            feat*|add*|new*) features+="- ${clean}"$'\n' ;;
            fix*|bug*) fixes+="- ${clean}"$'\n' ;;
            perf*|improve*|refactor*) improvements+="- ${clean}"$'\n' ;;
            *) maintenance+="- ${clean}"$'\n' ;;
        esac
    done <<< "$commits"

    local output=""
    [[ -n "$features" ]] && output+=$'## New features\n'"$features"$'\n'
    [[ -n "$improvements" ]] && output+=$'## Improvements\n'"$improvements"$'\n'
    [[ -n "$fixes" ]] && output+=$'## Bug fixes\n'"$fixes"$'\n'
    [[ -n "$maintenance" ]] && output+=$'## Maintenance\n'"$maintenance"$'\n'
    printf '%s' "${output%$'\n'}"
}

CHANGELOG="$(generate_release_notes "$FILTERED_COMMITS")"
[[ -n "${CHANGELOG//[$' \t\n']/}" ]] || CHANGELOG="- No user-facing changes listed."

printf '\n%b\n' "${BOLD}=== Oxterm release ${VERSION} ===${RESET}"
$DRY_RUN && printf '%b\n' "${YELLOW}[DRY RUN - no files or Git refs will be changed]${RESET}"
info "Current version: ${CURRENT_VERSION} -> ${VERSION}"
info "Release tag: ${TAG_VERSION}"
info "Branch: ${MAIN_BRANCH}"
info "Commits since: ${LATEST_TAG:-repository start}"
separator
printf '%s\n' "$CHANGELOG"
separator

if $DRY_RUN; then
    ok "Dry run complete."
    exit 0
fi

read -r -p "Create release ${TAG_VERSION} and push it to GitHub? [y/N] " answer
[[ "$answer" =~ ^[Yy]$ ]] || { warn "Release cancelled."; exit 0; }

if [[ "$CURRENT_VERSION" != "$VERSION" ]]; then
    sed -i -E "0,/^version = \"[0-9.]+\"$/s//version = \"${VERSION}\"/" Cargo.toml
    cargo update -p oxterm --offline
fi

sync_aur_version

AUR_FILES=()
for file in packaging/aur/PKGBUILD packaging/aur/.SRCINFO; do
    if [[ -f "$file" ]]; then
        AUR_FILES+=("$file")
    fi
done

git add Cargo.toml Cargo.lock "${AUR_FILES[@]}"
git commit -m "chore: release ${VERSION}"
git tag -a "$TAG_VERSION" -m "Oxterm ${VERSION}" -m "$CHANGELOG"
git push origin "$MAIN_BRANCH"
git push origin "$TAG_VERSION"

NOTES_FILE="$(mktemp)"
trap 'rm -f "$NOTES_FILE"' EXIT
printf '%s\n' "$CHANGELOG" > "$NOTES_FILE"
gh release create "$TAG_VERSION" \
    --title "Oxterm ${VERSION}" \
    --notes-file "$NOTES_FILE"

ok "Oxterm ${VERSION} released as ${TAG_VERSION}."

install_local_binary() {
    info "Building the local release binary..."
    cargo build --release --locked

    local built="${REPO_ROOT}/target/release/oxterm"
    if [[ ! -x "$built" ]]; then
        warn "Build did not produce ${built}; skipping local install."
        return 0
    fi

    # Keep the documented root copy in sync (see build.sh).
    install -m 0755 "$built" "${REPO_ROOT}/oxterm-linux-x86-64"

    local system_bin="/usr/bin/oxterm"
    if [[ -w "$(dirname "$system_bin")" ]]; then
        install -m 0755 "$built" "$system_bin"
        ok "Installed ${system_bin}; the desktop launcher now uses ${VERSION}."
    elif command -v sudo >/dev/null 2>&1; then
        sudo install -m 0755 "$built" "$system_bin"
        ok "Installed ${system_bin}; the desktop launcher now uses ${VERSION}."
    else
        local user_bin="${HOME}/.local/bin/oxterm"
        install -Dm755 "$built" "$user_bin"
        ok "Installed ${user_bin}; make sure it is on your PATH."
    fi
}

if [[ "$SKIP_INSTALL" == true ]]; then
    info "Skipping local binary install (--no-install)."
else
    install_local_binary
fi
