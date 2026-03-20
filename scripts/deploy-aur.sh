#!/usr/bin/env bash
#
# AUR Deployment Script for Durthang
# Usage: ./scripts/deploy-aur.sh [OPTIONS]
#
# Updates the PKGBUILDs for durthang (source) and durthang-bin (binary),
# regenerates .SRCINFO, and optionally pushes to the AUR.
#
# Prerequisites:
#   - AUR repos cloned into aur/durthang/aur-repo/ and aur/durthang-bin/aur-repo/
#   - makepkg available (Arch Linux or docker image)
#   - curl available
#
# AUR setup (one-time):
#   mkdir -p aur/durthang && cd aur/durthang
#   git clone ssh://aur@aur.archlinux.org/durthang.git aur-repo
#   cd ../..
#   mkdir -p aur/durthang-bin && cd aur/durthang-bin
#   git clone ssh://aur@aur.archlinux.org/durthang-bin.git aur-repo
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
AUR_DIR="${PROJECT_ROOT}/aur"
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

# Parse arguments
PUSH=false
PACKAGE=""   # empty = process all

while [[ $# -gt 0 ]]; do
    case $1 in
        --push)
            PUSH=true
            shift
            ;;
        --package)
            PACKAGE="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Deploy durthang PKGBUILDs to the AUR."
            echo ""
            echo "Options:"
            echo "  --push               Commit and push to AUR (default: dry-run)"
            echo "  --package NAME       Process only this package (durthang or durthang-bin)"
            echo "  -h, --help           Show this help"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

log_info()    { echo -e "${BLUE}ℹ${NC} $1" >&2; }
log_success() { echo -e "${GREEN}✓${NC} $1" >&2; }
log_warning() { echo -e "${YELLOW}⚠${NC} $1" >&2; }
log_error()   { echo -e "${RED}✗${NC} $1" >&2; }
log_step()    { echo -e "${CYAN}${BOLD}▶ $1${NC}" >&2; }

get_version() {
    grep '^version = ' "$CARGO_TOML" | head -n1 | sed 's/version = "\(.*\)"/\1/'
}

# Replace pkgver= line in a PKGBUILD
update_pkgbuild_version() {
    local pkgbuild="$1"
    local new_version="$2"
    sed -i "s/^pkgver=.*/pkgver=${new_version}/" "$pkgbuild"
    sed -i "s/^pkgrel=.*/pkgrel=1/" "$pkgbuild"
    log_info "Updated pkgver to $new_version"
}

# Compute sha256 for the crates.io source tarball
update_source_checksum() {
    local pkgbuild="$1"
    local version="$2"
    local url="https://static.crates.io/crates/durthang/durthang-${version}.crate"

    log_info "Downloading crate to compute sha256: $url"
    local crate_file="${TMP_DIR}/durthang-${version}.crate"
    if command -v wget &>/dev/null; then
        wget -qO "$crate_file" "$url"
    else
        curl -sSfL "$url" -o "$crate_file"
    fi

    local sha256
    sha256=$(sha256sum "$crate_file" | awk '{print $1}')
    log_info "sha256: $sha256"

    # Replace the sha256sums line (handles both SKIP and previous value)
    sed -i "s/^sha256sums=.*/sha256sums=('${sha256}')/" "$pkgbuild"
    log_success "Updated source checksum"
}

# Compute sha256 for the GitHub release binary tarball
update_binary_checksum() {
    local pkgbuild="$1"
    local version="$2"
    local url="https://github.com/Pommersche92/durthang/releases/download/v${version}/durthang-${version}-x86_64.tar.gz"

    log_info "Downloading release tarball to compute sha256: $url"
    local tarball="${TMP_DIR}/durthang-${version}-x86_64.tar.gz"
    if command -v wget &>/dev/null; then
        wget -qO "$tarball" "$url"
    else
        curl -sSfL "$url" -o "$tarball"
    fi

    local sha256
    sha256=$(sha256sum "$tarball" | awk '{print $1}')
    log_info "sha256: $sha256"

    sed -i "s/^sha256sums=.*/sha256sums=('${sha256}')/" "$pkgbuild"
    log_success "Updated binary checksum"
}

generate_srcinfo() {
    local dir="$1"
    log_step "Generating .SRCINFO..."
    (cd "$dir" && makepkg --printsrcinfo > .SRCINFO)
    log_success ".SRCINFO generated"
}

push_to_aur() {
    local aur_repo="$1"
    local pkgbuild_src="$2"
    local version="$3"
    local pkgname
    pkgname=$(basename "$(dirname "$pkgbuild_src")")

    if [ ! -d "$aur_repo" ]; then
        log_error "AUR repo not found: $aur_repo"
        log_info "Set it up with:"
        log_info "  mkdir -p $(dirname "$aur_repo")"
        log_info "  cd $(dirname "$aur_repo")"
        log_info "  git clone ssh://aur@aur.archlinux.org/${pkgname}.git aur-repo"
        return 1
    fi

    # Copy updated files to AUR repo
    cp "$pkgbuild_src" "$aur_repo/PKGBUILD"
    [ -f "$(dirname "$pkgbuild_src")/.SRCINFO" ] && cp "$(dirname "$pkgbuild_src")/.SRCINFO" "$aur_repo/.SRCINFO"

    cd "$aur_repo"

    if git diff --quiet; then
        log_info "No changes to push for $pkgname"
        return 0
    fi

    log_step "Pushing $pkgname to AUR..."
    git add PKGBUILD .SRCINFO
    git commit -m "Update to version $version"

    if [ "$PUSH" = true ]; then
        git push
        log_success "Pushed $pkgname v$version to AUR"
    else
        log_warning "Dry-run: commit made but not pushed (pass --push to push)"
        git log --oneline -1
    fi
}

process_package() {
    local pkgname="$1"
    local version="$2"
    local pkg_dir="${AUR_DIR}/${pkgname}"
    local pkgbuild="${pkg_dir}/PKGBUILD"
    local aur_repo="${pkg_dir}/aur-repo"

    echo ""
    log_step "Processing $pkgname..."

    if [ ! -f "$pkgbuild" ]; then
        log_error "PKGBUILD not found: $pkgbuild"
        return 1
    fi

    update_pkgbuild_version "$pkgbuild" "$version"

    if [ "$pkgname" = "durthang" ]; then
        update_source_checksum "$pkgbuild" "$version"
    else
        update_binary_checksum "$pkgbuild" "$version"
    fi

    if command -v makepkg &>/dev/null; then
        generate_srcinfo "$pkg_dir"
    else
        log_warning "makepkg not available — skipping .SRCINFO generation"
        log_warning "Install makepkg (Arch Linux) to generate .SRCINFO"
    fi

    push_to_aur "$aur_repo" "$pkgbuild" "$version"
}

main() {
    cd "$PROJECT_ROOT"

    local version
    version=$(get_version)

    log_info "Deploying durthang v$version to AUR"
    [ "$PUSH" = true ] && log_info "Mode: PUSH" || log_warning "Mode: dry-run (use --push to actually push)"
    echo ""

    if [ -n "$PACKAGE" ]; then
        process_package "$PACKAGE" "$version"
    else
        process_package "durthang" "$version"
        process_package "durthang-bin" "$version"
    fi

    echo ""
    log_success "AUR deployment complete"
    echo ""
    log_info "AUR package pages:"
    echo "   https://aur.archlinux.org/packages/durthang"
    echo "   https://aur.archlinux.org/packages/durthang-bin"
}

main
