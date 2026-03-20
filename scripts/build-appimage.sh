#!/usr/bin/env bash
#
# AppImage Build Script for Durthang
# Usage: ./scripts/build-appimage.sh build
#
# Downloads linuxdeploy (if needed), creates an AppDir, and packages the
# durthang binary into an AppImage stored in target/dist/.
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

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TOML="${PROJECT_ROOT}/Cargo.toml"
BUILD_DIR="${PROJECT_ROOT}/target/appimage-build"
DIST_DIR="${PROJECT_ROOT}/target/dist"
APPDIR="${BUILD_DIR}/AppDir"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
LINUXDEPLOY="${BUILD_DIR}/linuxdeploy-x86_64.AppImage"

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

download_linuxdeploy() {
    if [ -f "$LINUXDEPLOY" ] && [ -x "$LINUXDEPLOY" ]; then
        log_info "linuxdeploy already present"
        return 0
    fi
    log_step "Downloading linuxdeploy..."
    mkdir -p "$BUILD_DIR"
    if command -v wget &>/dev/null; then
        wget -qO "$LINUXDEPLOY" "$LINUXDEPLOY_URL"
    elif command -v curl &>/dev/null; then
        curl -sSfL "$LINUXDEPLOY_URL" -o "$LINUXDEPLOY"
    else
        log_error "Neither wget nor curl found"
        exit 1
    fi
    chmod +x "$LINUXDEPLOY"
    log_success "linuxdeploy downloaded"
}

create_appdir() {
    local pkg="$1"
    local version="$2"

    log_step "Creating AppDir..."
    rm -rf "$APPDIR"
    mkdir -p "$APPDIR/usr/bin"
    mkdir -p "$APPDIR/usr/share/applications"
    mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"

    # Binary
    cp "${PROJECT_ROOT}/target/release/$pkg" "$APPDIR/usr/bin/$pkg"

    # .desktop file
    cat > "$APPDIR/usr/share/applications/${pkg}.desktop" << DESKTOP
[Desktop Entry]
Type=Application
Name=Durthang
Comment=A modern, terminal-based MUD client
Exec=durthang
Icon=durthang
Categories=Game;Network;
Terminal=true
DESKTOP

    # Icon — use docs/images/durthang.png if available, else create a placeholder with ImageMagick
    local icon_src="${PROJECT_ROOT}/docs/images/durthang.png"
    if [ -f "$icon_src" ]; then
        cp "$icon_src" "$APPDIR/usr/share/icons/hicolor/256x256/apps/${pkg}.png"
        log_info "Using icon: docs/images/durthang.png"
    elif command -v convert &>/dev/null; then
        log_warning "Icon not found, generating placeholder with ImageMagick"
        convert -size 256x256 xc:'#0c0a08' \
            -fill '#c84a12' -font DejaVu-Sans-Bold -pointsize 72 \
            -gravity center -annotate 0 'D' \
            "$APPDIR/usr/share/icons/hicolor/256x256/apps/${pkg}.png"
    else
        log_warning "No icon found and ImageMagick not available; AppImage may lack icon"
        # Create minimal 1x1 placeholder so linuxdeploy does not fail
        printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00\x00\x01\x01\x00\x05\x18\xd8N\x00\x00\x00\x00IEND\xaeB`\x82' \
            > "$APPDIR/usr/share/icons/hicolor/256x256/apps/${pkg}.png"
    fi

    # AppRun
    cat > "$APPDIR/AppRun" << 'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "${HERE}/usr/bin/durthang" "$@"
APPRUN
    chmod +x "$APPDIR/AppRun"

    log_success "AppDir created at $APPDIR"
}

build_appimage() {
    local pkg="$1"
    local version="$2"
    local output_name="${pkg}-${version}-x86_64.AppImage"
    local output_path="${DIST_DIR}/${output_name}"

    log_step "Building AppImage with linuxdeploy..."
    mkdir -p "$DIST_DIR"

    # linuxdeploy needs FUSE; try --appimage-extract-and-run as fallback
    export OUTPUT="${output_path}"
    if ! ARCH=x86_64 "$LINUXDEPLOY" \
            --appdir "$APPDIR" \
            --desktop-file "$APPDIR/usr/share/applications/${pkg}.desktop" \
            --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/${pkg}.png" \
            --output appimage 2>&1; then
        log_warning "linuxdeploy failed with FUSE; retrying with --appimage-extract-and-run"
        export APPIMAGE_EXTRACT_AND_RUN=1
        ARCH=x86_64 "$LINUXDEPLOY" \
            --appdir "$APPDIR" \
            --desktop-file "$APPDIR/usr/share/applications/${pkg}.desktop" \
            --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/${pkg}.png" \
            --output appimage
    fi

    # linuxdeploy writes the AppImage into CWD; move it to DIST_DIR
    if [ ! -f "$output_path" ]; then
        local found
        found=$(find "$BUILD_DIR" "$PROJECT_ROOT" -maxdepth 1 -name '*.AppImage' -newer "$LINUXDEPLOY" 2>/dev/null | head -n1)
        if [ -n "$found" ]; then
            mv "$found" "$output_path"
        else
            log_error "Could not locate built AppImage"
            exit 1
        fi
    fi

    chmod +x "$output_path"
    log_success "AppImage: $output_path ($(du -sh "$output_path" | cut -f1))"
}

main() {
    local command="${1:-build}"

    if [ "$command" != "build" ]; then
        echo "Usage: $0 build"
        exit 1
    fi

    cd "$PROJECT_ROOT"

    local pkg version
    pkg=$(get_package_name)
    version=$(get_version)

    log_info "Building AppImage for $pkg v$version"
    echo ""

    # Ensure release binary exists
    if [ ! -f "target/release/$pkg" ]; then
        log_step "Building release binary first..."
        cargo build --release
    fi

    download_linuxdeploy
    create_appdir "$pkg" "$version"
    build_appimage "$pkg" "$version"

    echo ""
    log_success "AppImage build complete"
}

main "$@"
