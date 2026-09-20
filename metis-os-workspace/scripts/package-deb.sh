#!/usr/bin/env bash
# Build a Metis .deb (Ubuntu 24.04 / 26.04 or Debian 13).
#
# Usage:
#   VERSION=0.1.0 ./scripts/package-deb.sh
#   VERSION=0.1.0 DISTRO_SUITE=ubuntu24.04 SKIP_BUILD=1 ./scripts/package-deb.sh
#   VERSION=0.1.0 DISTRO_SUITE=ubuntu26.04 ./scripts/package-deb.sh
#   VERSION=0.1.0 DISTRO_SUITE=debian13 ./scripts/package-deb.sh
#
# Legacy: UBUNTU_SUITE=24.04 still works (maps to ubuntu24.04).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
ASSETS_DIR="$WORKSPACE/assets"

if [[ -n "${METIS_CARGO_TARGET_DIR:-}" ]]; then
  CARGO_TARGET_DIR="$METIS_CARGO_TARGET_DIR"
elif [[ -x "$WORKSPACE/target/release/metis-compositor" ]]; then
  CARGO_TARGET_DIR="$WORKSPACE/target"
else
  CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE/target}"
fi

VERSION="${VERSION:-}"
SKIP_BUILD="${SKIP_BUILD:-0}"
ARCH="${ARCH:-amd64}"
PKG_NAME="metis-desktop"
REVISION="${DEB_REVISION:-1}"

# Resolve suite label for filename + control.
if [[ -n "${DISTRO_SUITE:-}" ]]; then
  :
elif [[ -n "${UBUNTU_SUITE:-}" ]]; then
  DISTRO_SUITE="ubuntu${UBUNTU_SUITE}"
else
  DISTRO_SUITE="ubuntu24.04"
fi

case "$DISTRO_SUITE" in
  ubuntu24.04)
    BUNDLE_GTK4_LAYER_SHELL=1
    CONTROL_PROFILE=ubuntu24.04
    ;;
  ubuntu26.04)
    BUNDLE_GTK4_LAYER_SHELL="${BUNDLE_GTK4_LAYER_SHELL:-0}"
    CONTROL_PROFILE=ubuntu26.04
    ;;
  debian13)
    BUNDLE_GTK4_LAYER_SHELL="${BUNDLE_GTK4_LAYER_SHELL:-0}"
    CONTROL_PROFILE=debian13
    ;;
  *)
    echo "ERROR: unknown DISTRO_SUITE=$DISTRO_SUITE (ubuntu24.04|ubuntu26.04|debian13)" >&2
    exit 1
    ;;
esac

if [[ -z "$VERSION" ]]; then
  echo "ERROR: set VERSION (e.g. VERSION=0.1.0)" >&2
  exit 1
fi
VERSION="${VERSION#v}"

DIST_ROOT="${DIST_ROOT:-$WORKSPACE/dist}"
STAGE="$DIST_ROOT/${PKG_NAME}-stage"
DEB_OUT="$DIST_ROOT/${PKG_NAME}_${VERSION}-${REVISION}_${ARCH}.${DISTRO_SUITE}.deb"

log() { printf '==> %s\n' "$*"; }

ensure_cargo() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not in PATH" >&2
    exit 1
  fi
}

build_binaries() {
  if [[ "$SKIP_BUILD" == "1" ]]; then
    log "Skipping cargo build (SKIP_BUILD=1)"
    return
  fi
  ensure_cargo
  # Align Compiling … vX.Y.Z / CARGO_PKG_VERSION with the GitHub/deb VERSION.
  "$SCRIPT_DIR/sync-version.sh" "$VERSION"
  log "Building release binaries…"
  (
    cd "$WORKSPACE"
    cargo build --release \
      -p metis-compositor \
      -p metis-shell \
      -p metis-settings \
      -p metis-portal \
      -p metis-remote \
      -p metis-polkit-agent \
      -p metis-viewer \
      -p metis-screenshot \
      -p metis-gaming
  )
}

write_control() {
  local installed_size depends recommends suggests layer_note
  installed_size="$(du -sk "$STAGE" | awk '{print $1}')"

  case "$CONTROL_PROFILE" in
    ubuntu24.04)
      depends="libgtk-4-1, libadwaita-1-0, libglib2.0-0t64 | libglib2.0-0, libpango-1.0-0, libcairo2, libgraphene-1.0-0, libseat1, libinput10, libudev1, libgbm1, libdrm2, libegl1, libgles2, libwayland-client0, libwayland-server0, libxkbcommon0, libpipewire-0.3-0, libpulse0, libssl3t64 | libssl3, libpam0g, libdisplay-info1, libeis1, liblcms2-2, xdg-desktop-portal, kitty, pkexec, polkitd"
      recommends="gnome-keyring, xdg-desktop-portal-gtk, udisks2, gvfs, gvfs-fuse, nftables"
      suggests="gnome-remote-desktop, freerdp3-wayland | freerdp2-x11, gamemode, flatpak, bluez, bluetooth, cups, system-config-printer, fprintd, libpam-fprintd, libpam-u2f"
      layer_note="Ships bundled libgtk4-layer-shell (not packaged on Ubuntu 24.04)."
      ;;
    ubuntu26.04)
      # Ubuntu 26.04 (resolute) ships libdisplay-info3 (0.3); libdisplay-info1 is gone.
      depends="libgtk-4-1, libadwaita-1-0, libglib2.0-0t64 | libglib2.0-0, libpango-1.0-0, libcairo2, libgraphene-1.0-0, libseat1, libinput10, libudev1, libgbm1, libdrm2, libegl1, libgles2, libwayland-client0, libwayland-server0, libxkbcommon0, libpipewire-0.3-0, libpulse0, libssl3t64 | libssl3, libpam0g, libdisplay-info3 | libdisplay-info2 | libdisplay-info1, libeis1, liblcms2-2, xdg-desktop-portal, kitty, pkexec, polkitd"
      if [[ "$BUNDLE_GTK4_LAYER_SHELL" != "1" ]]; then
        depends="${depends}, libgtk4-layer-shell0"
      fi
      recommends="gnome-keyring, xdg-desktop-portal-gtk, udisks2, gvfs, gvfs-fuse, nftables"
      suggests="gnome-remote-desktop, freerdp3-wayland | freerdp2-x11, gamemode, flatpak, bluez, bluetooth, cups, system-config-printer, fprintd, libpam-fprintd, libpam-u2f"
      if [[ "$BUNDLE_GTK4_LAYER_SHELL" == "1" ]]; then
        layer_note="Ships bundled libgtk4-layer-shell."
      else
        layer_note="Depends on libgtk4-layer-shell0 (from libgtk4-layer-shell-dev)."
      fi
      ;;
    debian13)
      depends="libgtk-4-1, libadwaita-1-0, libglib2.0-0, libpango-1.0-0, libcairo2, libgraphene-1.0-0, libseat1, libinput10, libudev1, libgbm1, libdrm2, libegl1, libgles2, libwayland-client0, libwayland-server0, libxkbcommon0, libpipewire-0.3-0, libpulse0, libssl3, libpam0g, libdisplay-info3 | libdisplay-info2 | libdisplay-info1, libeis1, liblcms2-2, xdg-desktop-portal, kitty, pkexec, polkitd"
      if [[ "$BUNDLE_GTK4_LAYER_SHELL" != "1" ]]; then
        depends="${depends}, libgtk4-layer-shell0"
      fi
      recommends="gnome-keyring, xdg-desktop-portal-gtk, udisks2, gvfs, gvfs-fuse, nftables"
      suggests="gnome-remote-desktop, freerdp3-wayland | freerdp2-x11, gamemode, flatpak, bluez, bluetooth, cups, system-config-printer, fprintd, libpam-fprintd, libpam-u2f"
      if [[ "$BUNDLE_GTK4_LAYER_SHELL" == "1" ]]; then
        layer_note="Ships bundled libgtk4-layer-shell."
      else
        layer_note="Depends on libgtk4-layer-shell0 (from libgtk4-layer-shell-dev)."
      fi
      ;;
  esac
  # OCR is a first-class editor feature, not an optional integration.
  depends="${depends}, tesseract-ocr, tesseract-ocr-eng, ffmpeg"

  cat >"$STAGE/DEBIAN/control" <<EOF
Package: ${PKG_NAME}
Version: ${VERSION}-${REVISION}
Section: x11
Priority: optional
Architecture: ${ARCH}
Installed-Size: ${installed_size}
Maintainer: Metis Developers <metis@localhost>
Homepage: https://github.com/digitalexpl0it/Metis
Depends: ${depends}
Recommends: ${recommends}
Suggests: ${suggests}
Breaks: metis (<< 1)
Replaces: metis (<< 1)
Description: Metis Wayland desktop environment
 Metis is a Wayland desktop environment built in Rust: a Smithay compositor,
 GTK4 edge bar (shell), Settings app, and xdg-desktop-portal backend.
 .
 After installing, log out and pick "Metis" from your display manager's
 session menu (GDM, SDDM, and other Wayland-capable greeters).
 .
 ${layer_note} Remote-access LAN firewall helpers (nftables + a PolicyKit
 agent) are Recommends. Heavier optionals stay Suggests.
 .
 Named metis-desktop (not metis) to avoid colliding with Ubuntu universe's
 unrelated metis graph-partitioning package.
 See https://github.com/digitalexpl0it/Metis.
EOF
}

write_postinst() {
  cat >"$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
# Bundled gtk4-layer-shell under /usr/lib — refresh linker cache.
if command -v ldconfig >/dev/null 2>&1; then
    ldconfig || true
fi
exit 0
EOF
  chmod 0755 "$STAGE/DEBIAN/postinst"
}

build_deb() {
  log "Building $DEB_OUT…"
  mkdir -p "$DIST_ROOT"
  rm -f "$DEB_OUT"
  if command -v fakeroot >/dev/null 2>&1; then
    fakeroot dpkg-deb --build "$STAGE" "$DEB_OUT"
  else
    dpkg-deb --build "$STAGE" "$DEB_OUT"
  fi
  log "Done: $DEB_OUT"
  dpkg-deb --info "$DEB_OUT" || true
  ls -lh "$DEB_OUT"
}

build_binaries
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN"
export STAGE WORKSPACE CARGO_TARGET_DIR ASSETS_DIR BUNDLE_GTK4_LAYER_SHELL
"$SCRIPT_DIR/stage-fhs.sh"
write_control
write_postinst
build_deb
