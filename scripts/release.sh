#!/usr/bin/env bash
# =============================================================================
# Durthang — release.sh
# Copyright (c) 2026 Raimo Geisel
# SPDX-License-Identifier: GPL-3.0-only
#
# Creates a versioned GitHub release with:
#   • Linux AppImage   (durthang-<ver>-x86_64-linux.AppImage)
#   • Linux tarball    (durthang-<ver>-x86_64-linux.tar.gz)   ← also used by AUR
#   • Windows zip      (durthang-<ver>-x86_64-windows.zip)
# Then publishes/updates the durthang-bin AUR package.
#
# Prerequisites
# ─────────────
#   rustup target add x86_64-unknown-linux-musl x86_64-pc-windows-gnu
#   cargo install cross          # or install mingw-w64 + musl-tools natively
#   gh                           # GitHub CLI, logged in  (gh auth login)
#   appimagetool                 # from https://github.com/AppImage/AppImageKit
#   zip
#   makepkg                      # Arch Linux or a Docker container with it
#
# Usage
# ─────
#   bash scripts/release.sh [--skip-aur] [--skip-windows] [--skip-appimage]
# =============================================================================

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$REPO_ROOT"

# ── option flags ─────────────────────────────────────────────────────────────
SKIP_AUR=false
SKIP_WINDOWS=false
SKIP_APPIMAGE=false
for arg in "$@"; do
  case "$arg" in
    --skip-aur)       SKIP_AUR=true ;;
    --skip-windows)   SKIP_WINDOWS=true ;;
    --skip-appimage)  SKIP_APPIMAGE=true ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done

# ── helpers ───────────────────────────────────────────────────────────────────
die()     { echo "❌  $*" >&2; exit 1; }
info()    { echo; echo "▶  $*"; }
ok()      { echo "✅  $*"; }
require() {
  local cmd="$1" hint="${2:-}"
  command -v "$cmd" &>/dev/null || die "Required command not found: $cmd${hint:+ — $hint}"
}

# ── version from Cargo.toml ───────────────────────────────────────────────────
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/version *= *"\(.*\)"/\1/')
TAG="v${VERSION}"
DIST="${REPO_ROOT}/dist"
mkdir -p "$DIST"

echo "═══════════════════════════════════════════════"
echo " 🏰  Durthang ${TAG} — release script"
echo "═══════════════════════════════════════════════"

# ── prerequisite checks ───────────────────────────────────────────────────────
require gh  "install from https://cli.github.com/ and run: gh auth login"
require zip "sudo pacman -S zip  /  sudo apt install zip"

LINUX_TARGET="x86_64-unknown-linux-musl"
WIN_TARGET="x86_64-pc-windows-gnu"

# Prefer `cross` (Docker-based cross-compiler) but fall back to native cargo
# if the required Rust target and system linker are already installed.
if command -v cross &>/dev/null; then
  CARGO_CMD="cross"
  info "Using 'cross' for cross-compilation (Docker required)"
else
  CARGO_CMD="cargo"
  info "Using native 'cargo' — ensure the toolchain targets are installed:"
  echo "    rustup target add ${LINUX_TARGET} ${WIN_TARGET}"
fi

if [[ "$SKIP_APPIMAGE" == false ]]; then
  require appimagetool "download from https://github.com/AppImage/AppImageKit/releases"
fi

# ── build Linux (musl, fully static) ─────────────────────────────────────────
info "Building Linux binary (${LINUX_TARGET})…"
"$CARGO_CMD" build --release --target "$LINUX_TARGET"
LINUX_BIN="target/${LINUX_TARGET}/release/durthang"
[[ -f "$LINUX_BIN" ]] || die "Linux binary not found at $LINUX_BIN"
ok "Linux binary: $LINUX_BIN  ($(du -h "$LINUX_BIN" | cut -f1))"

# ── build Windows ─────────────────────────────────────────────────────────────
if [[ "$SKIP_WINDOWS" == false ]]; then
  info "Building Windows binary (${WIN_TARGET})…"
  "$CARGO_CMD" build --release --target "$WIN_TARGET"
  WIN_BIN="target/${WIN_TARGET}/release/durthang.exe"
  [[ -f "$WIN_BIN" ]] || die "Windows binary not found at $WIN_BIN"
  ok "Windows binary: $WIN_BIN  ($(du -h "$WIN_BIN" | cut -f1))"
fi

# ── Linux tarball (used by GitHub release and AUR) ────────────────────────────
LINUX_TGZ="${DIST}/durthang-${VERSION}-x86_64-linux.tar.gz"
info "Creating Linux tarball…"
tar -czf "$LINUX_TGZ" -C "$(dirname "$LINUX_BIN")" "$(basename "$LINUX_BIN")"
ok "$(basename "$LINUX_TGZ")  ($(du -h "$LINUX_TGZ" | cut -f1))"

# ── Windows zip ──────────────────────────────────────────────────────────────
if [[ "$SKIP_WINDOWS" == false ]]; then
  WIN_ZIP="${DIST}/durthang-${VERSION}-x86_64-windows.zip"
  info "Creating Windows zip…"
  zip -j "$WIN_ZIP" "$WIN_BIN"
  ok "$(basename "$WIN_ZIP")  ($(du -h "$WIN_ZIP" | cut -f1))"
fi

# ── AppImage ──────────────────────────────────────────────────────────────────
if [[ "$SKIP_APPIMAGE" == false ]]; then
  info "Creating AppImage…"

  APPDIR="$(mktemp -d)/durthang.AppDir"
  mkdir -p "${APPDIR}/usr/bin"
  cp "$LINUX_BIN" "${APPDIR}/usr/bin/durthang"
  chmod +x "${APPDIR}/usr/bin/durthang"

  # AppRun launcher
  cat > "${APPDIR}/AppRun" <<'APPRUN'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/durthang" "$@"
APPRUN
  chmod +x "${APPDIR}/AppRun"

  # .desktop file  (Terminal=true is required for a TUI application)
  cat > "${APPDIR}/durthang.desktop" <<DESKTOP
[Desktop Entry]
Name=Durthang
Comment=A modern, terminal-based MUD client
Exec=durthang
Icon=durthang
Type=Application
Categories=Game;Network;
Terminal=true
DESKTOP

  # Icon — look for one in common locations; create a minimal fallback
  ICON_SRC=""
  for candidate in \
    "${REPO_ROOT}/docs/durthang.png" \
    "${REPO_ROOT}/docs/icon.png" \
    "${REPO_ROOT}/assets/durthang.png"; do
    [[ -f "$candidate" ]] && { ICON_SRC="$candidate"; break; }
  done

  if [[ -n "$ICON_SRC" ]]; then
    cp "$ICON_SRC" "${APPDIR}/durthang.png"
  elif command -v convert &>/dev/null; then
    # ImageMagick fallback — dark ember-coloured square
    convert -size 256x256 xc:'#0c0a08' \
            -fill '#c84a12' -font DejaVu-Sans -pointsize 96 \
            -gravity center -annotate 0 'D' \
            "${APPDIR}/durthang.png" 2>/dev/null
    echo "⚠️   No icon found — generated a placeholder. Place docs/durthang.png (512×512) for a real icon."
  else
    die "No icon found and ImageMagick is not available.\nPlace docs/durthang.png (512×512 PNG) and re-run, or install ImageMagick."
  fi

  APPIMAGE="${DIST}/durthang-${VERSION}-x86_64-linux.AppImage"
  ARCH=x86_64 appimagetool "$APPDIR" "$APPIMAGE"
  chmod +x "$APPIMAGE"
  ok "$(basename "$APPIMAGE")  ($(du -h "$APPIMAGE" | cut -f1))"
fi

# ── checksums ─────────────────────────────────────────────────────────────────
info "Computing SHA-256 checksums…"
CHECKSUM_FILE="${DIST}/durthang-${VERSION}-sha256sums.txt"
sha256sum "${DIST}/durthang-${VERSION}-"* > "$CHECKSUM_FILE"
cat "$CHECKSUM_FILE"

# ── GitHub release ────────────────────────────────────────────────────────────
info "Creating GitHub release ${TAG}…"

RELEASE_ASSETS=("$LINUX_TGZ" "$CHECKSUM_FILE")
[[ "$SKIP_WINDOWS"  == false ]] && RELEASE_ASSETS+=("$WIN_ZIP")
[[ "$SKIP_APPIMAGE" == false ]] && RELEASE_ASSETS+=("$APPIMAGE")

APPIMAGE_ROW=""
[[ "$SKIP_APPIMAGE" == false ]] && APPIMAGE_ROW="| 🐧 Linux AppImage | \`durthang-${VERSION}-x86_64-linux.AppImage\` |"
WINDOWS_ROW=""
[[ "$SKIP_WINDOWS" == false ]] && WINDOWS_ROW="| 🪟 Windows (zip)  | \`durthang-${VERSION}-x86_64-windows.zip\` |"

gh release create "$TAG" \
  --title "🏰 Durthang ${TAG}" \
  --notes "## What's new

<!-- Add release notes here -->

## Downloads

| Platform | File |
|---|---|
| 🐧 Linux (tarball) | \`durthang-${VERSION}-x86_64-linux.tar.gz\` |
${APPIMAGE_ROW}
${WINDOWS_ROW}
| 🦀 crates.io | \`cargo install durthang\` |

## SHA-256 checksums

\`\`\`
$(cat "$CHECKSUM_FILE")
\`\`\`" \
  "${RELEASE_ASSETS[@]}"

ok "GitHub release: https://github.com/Pommersche92/durthang/releases/tag/${TAG}"

# ── AUR ───────────────────────────────────────────────────────────────────────
if [[ "$SKIP_AUR" == false ]]; then
  info "Publishing AUR package (durthang-bin)…"
  bash "${SCRIPT_DIR}/aur/update-aur.sh" "$VERSION" "$LINUX_TGZ"
fi

echo
echo "═══════════════════════════════════════════════"
echo " 🎉  Durthang ${TAG} released successfully!"
echo "═══════════════════════════════════════════════"
