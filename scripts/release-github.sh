#!/usr/bin/env bash
#
# GitHub Release Script for Durthang
# Usage: ./scripts/release-github.sh [OPTIONS]
#
# Builds Linux x64 tarball, AppImage, and Windows x64 zip, then creates
# (or updates) a GitHub release with those assets attached.
#

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Configuration
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TOML="${PROJECT_ROOT}/Cargo.toml"
DIST_DIR="${PROJECT_ROOT}/target/dist"

# Parse arguments
DRAFT_MODE=false
SKIP_BUILD=false
SKIP_WINDOWS=false
SKIP_APPIMAGE=false
RELEASE_NOTES=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --draft)
            DRAFT_MODE=true
            shift
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --skip-windows)
            SKIP_WINDOWS=true
            shift
            ;;
        --skip-appimage)
            SKIP_APPIMAGE=true
            shift
            ;;
        --notes)
            RELEASE_NOTES="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Build assets and create/update a GitHub release."
            echo ""
            echo "Options:"
            echo "  --draft              Create the release as a draft"
            echo "  --skip-build         Skip the cargo build step"
            echo "  --skip-windows       Skip Windows cross-compile"
            echo "  --skip-appimage      Skip AppImage build"
            echo "  --notes TEXT         Release notes text"
            echo "  -h, --help           Show this help"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Helper functions
log_info()    { echo -e "${BLUE}ℹ${NC} $1" >&2; }
log_success() { echo -e "${GREEN}✓${NC} $1" >&2; }
log_warning() { echo -e "${YELLOW}⚠${NC} $1" >&2; }
log_error()   { echo -e "${RED}✗${NC} $1" >&2; }
log_step()    { echo -e "${CYAN}${BOLD}▶ $1${NC}" >&2; }

get_version() {
    grep '^version = ' "$CARGO_TOML" | head -n1 | sed 's/version = "\(.*\)"/\1/'
}

get_package_name() {
    grep '^name = ' "$CARGO_TOML" | head -n1 | sed 's/name = "\(.*\)"/\1/'
}

check_gh_cli() {
    if ! command -v gh &>/dev/null; then
        log_error "GitHub CLI (gh) is not installed. Install it from https://cli.github.com/"
        exit 1
    fi
}

check_gh_auth() {
    if ! gh auth status &>/dev/null; then
        log_error "Not authenticated to GitHub. Run: gh auth login"
        exit 1
    fi
}

check_release_exists() {
    local tag="$1"
    gh release view "$tag" &>/dev/null
}

build_release() {
    log_step "Building Linux release binary..."
    cd "$PROJECT_ROOT"
    export RUSTUP_TOOLCHAIN=stable
    cargo build --release
    log_success "Linux binary built: target/release/$PACKAGE_NAME"
}

build_windows() {
    log_step "Building Windows release binary..."
    if ! command -v x86_64-w64-mingw32-gcc &>/dev/null; then
        log_warning "mingw cross-compiler not found (x86_64-w64-mingw32-gcc)"
        log_warning "Install with: sudo apt install gcc-mingw-w64-x86-64"
        log_warning "Skipping Windows build"
        return 0
    fi
    if ! rustup target list --installed | grep -q 'x86_64-pc-windows-gnu'; then
        log_info "Adding Windows target for Rust..."
        rustup target add x86_64-pc-windows-gnu
    fi
    cd "$PROJECT_ROOT"
    export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
    cargo build --release --target x86_64-pc-windows-gnu
    log_success "Windows binary built: target/x86_64-pc-windows-gnu/release/$PACKAGE_NAME.exe"
}

create_tarball() {
    local tarball="${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-x86_64.tar.gz"
    log_step "Creating Linux tarball: $(basename "$tarball")"

    local staging
    staging=$(mktemp -d)
    local staging_dir="${staging}/${PACKAGE_NAME}-${VERSION}"
    mkdir -p "$staging_dir"

    cp "target/release/$PACKAGE_NAME" "$staging_dir/"
    [ -f LICENSE ] && cp LICENSE "$staging_dir/"
    [ -f README.md ] && cp README.md "$staging_dir/"

    tar -czf "$tarball" -C "$staging" "${PACKAGE_NAME}-${VERSION}"
    rm -rf "$staging"

    log_success "Created: $(basename "$tarball") ($(du -sh "$tarball" | cut -f1))"
    echo "$tarball"
}

create_windows_zip() {
    local win_exe="target/x86_64-pc-windows-gnu/release/${PACKAGE_NAME}.exe"
    if [ ! -f "$win_exe" ]; then
        log_warning "Windows binary not found, skipping zip"
        return 0
    fi

    if ! command -v zip &>/dev/null; then
        log_warning "zip command not found, skipping Windows zip"
        return 0
    fi

    local zipfile="${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-x86_64-windows.zip"
    log_step "Creating Windows zip: $(basename "$zipfile")"

    local staging
    staging=$(mktemp -d)
    local staging_dir="${staging}/${PACKAGE_NAME}-${VERSION}"
    mkdir -p "$staging_dir"

    cp "$win_exe" "$staging_dir/"
    [ -f LICENSE ] && cp LICENSE "$staging_dir/"
    [ -f README.md ] && cp README.md "$staging_dir/"

    (cd "$staging" && zip -r "$zipfile" "${PACKAGE_NAME}-${VERSION}")
    rm -rf "$staging"

    log_success "Created: $(basename "$zipfile") ($(du -sh "$zipfile" | cut -f1))"
    echo "$zipfile"
}

build_appimage() {
    log_step "Building AppImage..."
    if "${PROJECT_ROOT}/scripts/build-appimage.sh" build; then
        log_success "AppImage built"
    else
        log_warning "AppImage build failed — asset will be skipped"
    fi
}

create_github_release() {
    local tag="v$VERSION"
    local title="🏰 Durthang v${VERSION}"

    log_step "Creating GitHub release: $tag"

    local -a assets
    assets=()

    local tarball="${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-x86_64.tar.gz"
    local zipfile="${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-x86_64-windows.zip"
    local appimage="${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-x86_64.AppImage"

    [ -f "$tarball" ]   && assets+=("$tarball")   && log_info "  + $(basename "$tarball")"
    [ -f "$appimage" ]  && assets+=("$appimage")  && log_info "  + $(basename "$appimage")"
    [ -f "$zipfile" ]   && assets+=("$zipfile")   && log_info "  + $(basename "$zipfile")"

    if [ "${#assets[@]}" -eq 0 ]; then
        log_warning "No release assets found in $DIST_DIR"
    fi

    local -a gh_args
    gh_args=(release create "$tag" --title "$title")
    [ "$DRAFT_MODE" = true ] && gh_args+=(--draft)
    [ -n "$RELEASE_NOTES" ] && gh_args+=(--notes "$RELEASE_NOTES") || gh_args+=(--generate-notes)
    gh_args+=(--repo "Pommersche92/durthang")

    if check_release_exists "$tag"; then
        log_warning "Release $tag already exists — deleting and recreating"
        gh release delete "$tag" --yes --repo "Pommersche92/durthang" || true
    fi

    gh "${gh_args[@]}" "${assets[@]}"

    log_success "GitHub release created: https://github.com/Pommersche92/durthang/releases/tag/$tag"
}

main() {
    cd "$PROJECT_ROOT"

    check_gh_cli
    check_gh_auth

    VERSION=$(get_version)
    PACKAGE_NAME=$(get_package_name)

    log_info "Package: $PACKAGE_NAME v$VERSION"
    echo ""

    mkdir -p "$DIST_DIR"

    if [ "$SKIP_BUILD" = false ]; then
        build_release
        echo ""
    fi

    if [ "$SKIP_WINDOWS" = false ]; then
        build_windows
        echo ""
    fi

    if [ "$SKIP_APPIMAGE" = false ]; then
        build_appimage
        echo ""
    fi

    create_tarball
    WINDOWS_ZIP=$(create_windows_zip || true)
    echo ""

    create_github_release

    echo ""
    echo -e "${GREEN}${BOLD}✓ Release assets in: target/dist/${NC}"
    ls -lh "$DIST_DIR" 2>/dev/null || true
}

main
