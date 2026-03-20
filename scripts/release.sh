#!/usr/bin/env bash
#
# Complete Release Pipeline for Durthang
# Usage: ./scripts/release.sh [OPTIONS]
#
# This script automates the complete release process:
# 1. Runs tests
# 2. Publishes to crates.io
# 3. Creates GitHub release with binary assets
# 4. Deploys to AUR (durthang and durthang-bin)
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Configuration
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_TOML="${PROJECT_ROOT}/Cargo.toml"

# Parse arguments
DRAFT_MODE=false
SKIP_CRATES=false
SKIP_GITHUB=false
SKIP_AUR=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --draft)
            DRAFT_MODE=true
            shift
            ;;
        --skip-crates)
            SKIP_CRATES=true
            shift
            ;;
        --skip-github)
            SKIP_GITHUB=true
            shift
            ;;
        --skip-aur)
            SKIP_AUR=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Complete release pipeline: crates.io → GitHub → AUR"
            echo ""
            echo "Options:"
            echo "  --draft              Create GitHub release as draft"
            echo "  --skip-crates        Skip crates.io publish"
            echo "  --skip-github        Skip GitHub release"
            echo "  --skip-aur           Skip AUR deployment"
            echo "  -h, --help           Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0                   # Full release pipeline"
            echo "  $0 --draft           # Create draft GitHub release"
            echo "  $0 --skip-crates     # Skip crates.io (already published)"
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
separator()   { echo "" >&2; echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}" >&2; echo "" >&2; }

get_version() {
    grep '^version = ' "$CARGO_TOML" | head -n1 | sed 's/version = "\(.*\)"/\1/'
}

confirm() {
    local prompt="$1"
    echo -e -n "${YELLOW}❓${NC} $prompt (y/N): " >&2
    read -r response
    case "$response" in
        [yY][eE][sS]|[yY]) return 0 ;;
        *) return 1 ;;
    esac
}

run_tests() {
    log_step "Running tests..."
    echo ""
    if cargo test; then
        log_success "All tests passed"
        return 0
    else
        log_error "Tests failed"
        return 1
    fi
}

publish_crates() {
    log_step "Publishing to crates.io..."
    echo ""
    if ! confirm "Publish version $VERSION to crates.io?"; then
        log_warning "Skipping crates.io publish"
        return 0
    fi
    if cargo publish; then
        log_success "Published to crates.io"
        log_info "Waiting 30 seconds for crates.io to sync..."
        sleep 30
        return 0
    else
        log_error "Failed to publish to crates.io"
        return 1
    fi
}

create_github_release() {
    log_step "Creating GitHub release..."
    echo ""
    local extra_args=""
    [ "$DRAFT_MODE" = true ] && extra_args="--draft"
    if "${PROJECT_ROOT}/scripts/release-github.sh" $extra_args; then
        log_success "GitHub release created"
        log_info "Waiting 10 seconds for GitHub to process release..."
        sleep 10
        return 0
    else
        log_error "Failed to create GitHub release"
        return 1
    fi
}

deploy_aur() {
    log_step "Deploying to AUR..."
    echo ""
    if ! confirm "Deploy to AUR (durthang and durthang-bin)?"; then
        log_warning "Skipping AUR deployment"
        return 0
    fi
    if "${PROJECT_ROOT}/scripts/deploy-aur.sh" --push; then
        log_success "Deployed to AUR"
        return 0
    else
        log_error "Failed to deploy to AUR"
        return 1
    fi
}

display_summary() {
    separator
    echo -e "${GREEN}${BOLD}🎉 Release Complete!${NC}"
    separator

    echo -e "${BOLD}Version:${NC} v$VERSION"
    echo ""

    if [ "$SKIP_CRATES" = false ]; then
        echo -e "${BOLD}📦 crates.io:${NC}"
        echo "   https://crates.io/crates/durthang"
        echo ""
    fi

    if [ "$SKIP_GITHUB" = false ]; then
        echo -e "${BOLD}🐙 GitHub:${NC}"
        if [ "$DRAFT_MODE" = true ]; then
            echo "   https://github.com/Pommersche92/durthang/releases (DRAFT)"
        else
            echo "   https://github.com/Pommersche92/durthang/releases/tag/v$VERSION"
        fi
        echo "   Assets: Linux x64 tarball, Linux AppImage, Windows x64 zip"
        echo ""
    fi

    if [ "$SKIP_AUR" = false ]; then
        echo -e "${BOLD}🐧 AUR:${NC}"
        echo "   https://aur.archlinux.org/packages/durthang"
        echo "   https://aur.archlinux.org/packages/durthang-bin"
        echo ""
    fi

    echo -e "${BOLD}Installation:${NC}"
    echo "   cargo install durthang"
    echo "   yay -S durthang"
    echo "   yay -S durthang-bin"
    echo ""
}

main() {
    cd "$PROJECT_ROOT"

    echo ""
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}${BOLD}          🏰 Durthang Release Pipeline 🏰${NC}"
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    VERSION=$(get_version)
    log_info "Version: $VERSION"
    echo ""

    log_info "Pipeline steps:"
    echo ""
    [ "$SKIP_CRATES" = false ] && echo "  1. ✓ Publish to crates.io" || echo "  1. ✗ Skip crates.io"
    [ "$SKIP_GITHUB" = false ] && echo "  2. ✓ Create GitHub release (Linux tarball, AppImage, Windows zip)" || echo "  2. ✗ Skip GitHub release"
    [ "$SKIP_AUR"    = false ] && echo "  3. ✓ Deploy to AUR (durthang + durthang-bin)" || echo "  3. ✗ Skip AUR"
    echo ""

    separator

    if ! run_tests; then
        log_error "Tests must pass before release"
        exit 1
    fi

    separator

    if [ "$SKIP_CRATES" = false ]; then
        publish_crates || exit 1
        separator
    fi

    if [ "$SKIP_GITHUB" = false ]; then
        create_github_release || exit 1
        separator
    fi

    if [ "$SKIP_AUR" = false ]; then
        deploy_aur || exit 1
        separator
    fi

    display_summary
}

main
