#!/usr/bin/env bash
# =============================================================================
# Durthang — update-aur.sh
# Copyright (c) 2026 Raimo Geisel
# SPDX-License-Identifier: GPL-3.0-only
#
# Clones the durthang-bin AUR repository, bumps the version, recomputes the
# SHA-256 checksum, regenerates .SRCINFO, and pushes the update.
#
# Usage (called automatically by release.sh):
#   bash scripts/aur/update-aur.sh <VERSION> <PATH_TO_LINUX_TGZ>
#
# First-time setup
# ────────────────
# 1. Create the AUR package once at https://aur.archlinux.org/packages/
#    (submit an initial PKGBUILD via the web UI or SSH push).
# 2. Add your SSH key to your AUR account at https://aur.archlinux.org/account/
# 3. Ensure ~/.ssh/config contains an entry for aur.archlinux.org:
#       Host aur.archlinux.org
#         IdentityFile ~/.ssh/id_ed25519_aur   # or your key file
#         User aur
# =============================================================================

set -euo pipefail

die()  { echo "❌  $*" >&2; exit 1; }
info() { echo "▶  $*"; }
ok()   { echo "✅  $*"; }

# ── arguments ─────────────────────────────────────────────────────────────────
[[ $# -ge 2 ]] || die "Usage: $0 <VERSION> <PATH_TO_LINUX_TGZ>"
VERSION="$1"
LINUX_TGZ="$2"

[[ -f "$LINUX_TGZ" ]] || die "Tarball not found: $LINUX_TGZ"

AUR_REPO="ssh://aur@aur.archlinux.org/durthang-bin.git"
AUR_DIR="$(mktemp -d)/durthang-bin"

# ── compute SHA-256 of the release tarball ────────────────────────────────────
SHA256=$(sha256sum "$LINUX_TGZ" | awk '{print $1}')
info "SHA-256 of $(basename "$LINUX_TGZ"): ${SHA256}"

# ── clone AUR repo ────────────────────────────────────────────────────────────
info "Cloning AUR repo…"
if ! git clone "$AUR_REPO" "$AUR_DIR" 2>/dev/null; then
  # Package not yet on AUR — initialise an empty repo
  info "AUR package not found — initialising new repo"
  mkdir -p "$AUR_DIR"
  cd "$AUR_DIR"
  git init
  git remote add origin "$AUR_REPO"
else
  cd "$AUR_DIR"
fi

# ── write PKGBUILD ────────────────────────────────────────────────────────────
info "Writing PKGBUILD for v${VERSION}…"
cat > "${AUR_DIR}/PKGBUILD" <<PKGBUILD
# Maintainer: Raimo Geisel <raimog92@protonmail.com>
pkgname=durthang-bin
pkgver=${VERSION}
pkgrel=1
pkgdesc="A modern, terminal-based MUD client with TLS, GMCP, automap, aliases, triggers, and a sidebar panel system"
arch=('x86_64')
url="https://github.com/Pommersche92/durthang"
license=('GPL-3.0-only')
provides=('durthang')
conflicts=('durthang')

source_x86_64=(
  "durthang-${VERSION}-x86_64-linux.tar.gz::https://github.com/Pommersche92/durthang/releases/download/v${VERSION}/durthang-${VERSION}-x86_64-linux.tar.gz"
)
sha256sums_x86_64=('${SHA256}')

package() {
  install -Dm755 "\${srcdir}/durthang" "\${pkgdir}/usr/bin/durthang"
}
PKGBUILD

# ── generate .SRCINFO ─────────────────────────────────────────────────────────
info "Generating .SRCINFO…"
# makepkg must run inside the directory containing PKGBUILD
(cd "$AUR_DIR" && makepkg --printsrcinfo > .SRCINFO)

echo "──── .SRCINFO ────"
cat "${AUR_DIR}/.SRCINFO"
echo "──────────────────"

# ── commit and push ───────────────────────────────────────────────────────────
info "Committing and pushing to AUR…"
cd "$AUR_DIR"
git add PKGBUILD .SRCINFO
git commit -m "Update to v${VERSION}"
git push origin master

ok "AUR package durthang-bin updated to v${VERSION}"
ok "https://aur.archlinux.org/packages/durthang-bin"
