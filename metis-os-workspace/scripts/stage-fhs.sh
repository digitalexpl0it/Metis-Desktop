#!/usr/bin/env bash
# Stage Metis FHS tree (binaries, session, portals, icons, locale, …).
#
# Required env:
#   STAGE          — destination root (e.g. …/metis-desktop-stage or pkgdir)
#   WORKSPACE      — metis-os-workspace path
#   CARGO_TARGET_DIR — cargo target dir containing release/
#
# Optional:
#   BUNDLE_GTK4_LAYER_SHELL=1  — copy libgtk4-layer-shell into STAGE (Ubuntu 24.04)
#   PREFIX_LIBDIR=usr/lib/x86_64-linux-gnu  — library dir under STAGE (Debian/Ubuntu)
#
# Usage:
#   STAGE=… WORKSPACE=… CARGO_TARGET_DIR=… BUNDLE_GTK4_LAYER_SHELL=1 \
#     ./scripts/stage-fhs.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${STAGE:?STAGE is required}"
: "${WORKSPACE:?WORKSPACE is required}"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"

ASSETS_DIR="${ASSETS_DIR:-$WORKSPACE/assets}"
BUNDLE_GTK4_LAYER_SHELL="${BUNDLE_GTK4_LAYER_SHELL:-0}"
PREFIX_LIBDIR="${PREFIX_LIBDIR:-usr/lib/x86_64-linux-gnu}"

log() { printf '==> %s\n' "$*"; }

require_bin() {
  local path="$1"
  if [[ ! -x "$path" ]]; then
    echo "ERROR: missing binary: $path (build first or unset SKIP_BUILD)" >&2
    exit 1
  fi
}

stage_gtk4_layer_shell() {
  local libdir="$STAGE/$PREFIX_LIBDIR"
  mkdir -p "$libdir"
  local found=""
  local candidate
  for candidate in \
    /usr/local/lib/x86_64-linux-gnu/libgtk4-layer-shell.so.0 \
    /usr/lib/x86_64-linux-gnu/libgtk4-layer-shell.so.0 \
    /usr/local/lib/libgtk4-layer-shell.so.0 \
    /usr/lib/libgtk4-layer-shell.so.0; do
    if [[ -e "$candidate" ]]; then
      found="$candidate"
      break
    fi
  done
  if [[ -z "$found" ]]; then
    local shell_bin="$CARGO_TARGET_DIR/release/metis-shell"
    if [[ -x "$shell_bin" ]]; then
      found="$(ldd "$shell_bin" 2>/dev/null | awk '/libgtk4-layer-shell\.so/ {print $3; exit}')"
    fi
  fi
  if [[ -z "$found" || ! -e "$found" ]]; then
    echo "ERROR: libgtk4-layer-shell.so.0 not found." >&2
    echo "  Build gtk4-layer-shell from https://github.com/wmww/gtk4-layer-shell" >&2
    exit 1
  fi
  local real base
  real="$(readlink -f "$found")"
  base="$(basename "$real")"
  log "Bundling gtk4-layer-shell: $real"
  install -Dm755 "$real" "$libdir/$base"
  ln -sfn "$base" "$libdir/libgtk4-layer-shell.so.0"
  if [[ "$base" != "libgtk4-layer-shell.so" ]]; then
    ln -sfn "$base" "$libdir/libgtk4-layer-shell.so"
  fi
}

stage_locale_catalogs() {
  local locale_src="$ASSETS_DIR/locale"
  local locale_dst="$STAGE/usr/share/metis/locale"
  if [[ ! -d "$locale_src" ]]; then
    echo "ERROR: missing locale catalogs at $locale_src" >&2
    exit 1
  fi
  if [[ -x "$SCRIPT_DIR/i18n-compile.sh" ]]; then
    log "Compiling gettext catalogs (.po → .mo)…"
    "$SCRIPT_DIR/i18n-compile.sh"
  elif command -v msgfmt >/dev/null 2>&1; then
    log "Compiling gettext catalogs with msgfmt…"
    local po mo
    while IFS= read -r -d '' po; do
      mo="${po%.po}.mo"
      msgfmt -o "$mo" "$po"
    done < <(find "$locale_src" -path '*/LC_MESSAGES/*.po' -print0)
  else
    echo "ERROR: msgfmt not found — install gettext to compile locale catalogs" >&2
    exit 1
  fi
  log "Staging locale catalogs to /usr/share/metis/locale…"
  mkdir -p "$locale_dst"
  cp -a "$locale_src"/. "$locale_dst/"
  local mo_count ftl_count
  mo_count="$(find "$locale_dst" -name 'metis.mo' | wc -l)"
  ftl_count="$(find "$locale_dst" -name 'metis.ftl' | wc -l)"
  if [[ "$mo_count" -lt 1 || "$ftl_count" -lt 1 ]]; then
    echo "ERROR: staged locale tree looks empty (mo=$mo_count ftl=$ftl_count)" >&2
    exit 1
  fi
  log "  catalogs: $mo_count .mo, $ftl_count .ftl"
}

log "Staging FHS tree under $STAGE…"
mkdir -p \
  "$STAGE/usr/bin" \
  "$STAGE/$PREFIX_LIBDIR" \
  "$STAGE/usr/share/wayland-sessions" \
  "$STAGE/usr/share/xdg-desktop-portal/portals" \
  "$STAGE/usr/share/applications" \
  "$STAGE/usr/share/icons/hicolor/48x48/apps" \
  "$STAGE/usr/share/icons/hicolor/256x256/apps" \
  "$STAGE/usr/share/metis/wallpapers" \
  "$STAGE/usr/share/metis/locale" \
  "$STAGE/usr/share/metis/widgets" \
  "$STAGE/usr/share/polkit-1/actions" \
  "$STAGE/etc/pam.d"

rel="$CARGO_TARGET_DIR/release"
for bin in metis-compositor metis-shell metis-settings metis-portal metis-remote metis-polkit-agent metis-viewer metis-gamingd metis-screenshot; do
  require_bin "$rel/$bin"
  install -Dm755 "$rel/$bin" "$STAGE/usr/bin/$bin"
done
install -Dm755 "$rel/metis-polkit-agent" "$STAGE/usr/libexec/metis-polkit-agent"

if [[ "$BUNDLE_GTK4_LAYER_SHELL" == "1" ]]; then
  stage_gtk4_layer_shell
fi

install -Dm755 "$ASSETS_DIR/metis-session" "$STAGE/usr/bin/metis-session"
install -Dm644 "$ASSETS_DIR/metis.desktop" "$STAGE/usr/share/wayland-sessions/metis.desktop"
install -Dm644 "$ASSETS_DIR/metis.portal" "$STAGE/usr/share/xdg-desktop-portal/portals/metis.portal"
install -Dm644 "$ASSETS_DIR/metis-portals.conf" "$STAGE/usr/share/xdg-desktop-portal/metis-portals.conf"
install -Dm644 "$ASSETS_DIR/metis-settings.desktop" "$STAGE/usr/share/applications/metis-settings.desktop"
install -Dm644 "$ASSETS_DIR/metis-viewer.desktop" "$STAGE/usr/share/applications/metis-viewer.desktop"
install -Dm644 "$ASSETS_DIR/metis-settings-48.png" "$STAGE/usr/share/icons/hicolor/48x48/apps/metis-settings.png"
install -Dm644 "$ASSETS_DIR/metis-settings.png" "$STAGE/usr/share/icons/hicolor/256x256/apps/metis-settings.png"
install -Dm644 "$ASSETS_DIR/metis-viewer-48.png" "$STAGE/usr/share/icons/hicolor/48x48/apps/metis-viewer.png"
install -Dm644 "$ASSETS_DIR/metis-viewer.png" "$STAGE/usr/share/icons/hicolor/256x256/apps/metis-viewer.png"
install -Dm644 "$ASSETS_DIR/pam-metis" "$STAGE/etc/pam.d/metis"
install -Dm644 "$WORKSPACE/packaging/polkit/org.metis.policy" \
  "$STAGE/usr/share/polkit-1/actions/org.metis.policy"

log "Staging bundled wallpapers…"
shopt -s nullglob
for wp in "$ASSETS_DIR/wallpapers"/*.{png,jpg,jpeg,webp,PNG,JPG,JPEG,WEBP}; do
  [[ -f "$wp" ]] || continue
  install -Dm644 "$wp" "$STAGE/usr/share/metis/wallpapers/$(basename "$wp")"
done
shopt -u nullglob

log "Staging widget extension packs…"
if [[ -d "$ASSETS_DIR/widgets" ]]; then
  local_pack=""
  for local_pack in "$ASSETS_DIR/widgets"/*; do
    [[ -d "$local_pack" ]] || continue
    name="$(basename "$local_pack")"
    mkdir -p "$STAGE/usr/share/metis/widgets/$name"
    [[ -f "$local_pack/manifest.json" ]] && install -Dm644 "$local_pack/manifest.json" \
      "$STAGE/usr/share/metis/widgets/$name/manifest.json"
    [[ -f "$local_pack/widget.json" ]] && install -Dm644 "$local_pack/widget.json" \
      "$STAGE/usr/share/metis/widgets/$name/widget.json"
    [[ -f "$local_pack/helper" ]] && install -Dm755 "$local_pack/helper" \
      "$STAGE/usr/share/metis/widgets/$name/helper"
    for bin in "$local_pack"/*; do
      [[ -f "$bin" && -x "$bin" ]] || continue
      base="$(basename "$bin")"
      [[ "$base" == "helper" ]] && continue
      case "$base" in
        *.json|*.md|*.txt) continue ;;
      esac
      install -Dm755 "$bin" "$STAGE/usr/share/metis/widgets/$name/$base"
    done
  done
fi

stage_locale_catalogs
log "FHS stage complete."
