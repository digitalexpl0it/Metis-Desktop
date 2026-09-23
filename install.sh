#!/usr/bin/env bash
# Bootstrap Metis from a git clone: install distro deps, then
# `run-metis.sh --install-session` (release build → /usr/local + greeter entry).
#
# Supported: Ubuntu 26.04+, Debian 13 (trixie)+, Arch Linux. NixOS uses the flake.
# Floor: GTK >= 4.18, gtk4-layer-shell >= 1.0, Rust >= 1.95 (see Cargo.toml rust-version).
#
# Usage (from repo root):
#   ./install.sh
#   ./install.sh --yes
#   ./install.sh --deps-only
#   ./install.sh --with-remote
#
# End users who only want a binary install should prefer the metis-desktop .deb
# (or Arch PKGBUILD / NixOS module) — see docs/PACKAGING.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$REPO_ROOT/metis-os-workspace"
DEPS_DIR="$WORKSPACE/scripts/deps"
RUN_METIS="$WORKSPACE/metis-shell/run-metis.sh"
GTK4_LAYER_SHELL_TAG="${GTK4_LAYER_SHELL_TAG:-v1.3.0}"

YES=0
DEPS_ONLY=0
WITH_REMOTE=0

usage() {
  cat <<'EOF'
Usage: ./install.sh [options]

  --yes           Noninteractive package install (-y)
  --deps-only     Install deps (+ layer-shell / rust) only; skip --install-session
  --with-remote   Also install gnome-remote-desktop + FreeRDP client packages
  -h, --help      Show this help

Supported distros: Ubuntu 26.04+, Debian 13 (trixie)+, Arch Linux.
NixOS: use the flake module instead (see nix/README.md).
EOF
}

log() { printf '==> %s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# Minimum rustc; keep in sync with `rust-version` in metis-os-workspace/Cargo.toml.
METIS_MIN_RUST="1.95.0"

# True when dotted version $1 >= $2.
version_ge() {
  [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" == "$2" ]]
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --deps-only) DEPS_ONLY=1 ;;
    --with-remote) WITH_REMOTE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
  shift
done

[[ -f /etc/os-release ]] || die "missing /etc/os-release"
# shellcheck disable=SC1091
. /etc/os-release

resolve_deps_file() {
  local id="${ID:-}"
  local ver="${VERSION_ID:-}"
  local codename="${VERSION_CODENAME:-}"

  case "$id" in
    ubuntu)
      if [[ -n "$ver" ]] && version_ge "$ver" "26.04"; then
        echo "$DEPS_DIR/ubuntu-26.04.sh"
      else
        die "unsupported Ubuntu VERSION_ID=${ver:-unknown}; Metis needs Ubuntu 26.04 or newer
(GTK >= 4.18 and gtk4-layer-shell >= 1.0; Ubuntu 24.04 ships GTK 4.14)."
      fi
      ;;
    debian)
      # Testing/sid have no VERSION_ID; they are newer than trixie.
      if [[ -n "$ver" ]] && version_ge "$ver" "13"; then
        echo "$DEPS_DIR/debian-13.sh"
      elif [[ -z "$ver" && -n "$codename" && ! "$codename" =~ ^(buster|bullseye|bookworm)$ ]]; then
        echo "$DEPS_DIR/debian-13.sh"
      else
        die "unsupported Debian (${ver:-?} / ${codename:-?}); Metis needs Debian 13 (trixie) or newer."
      fi
      ;;
    arch)
      echo "$DEPS_DIR/arch.sh"
      ;;
    nixos)
      die "NixOS: install Metis through the flake module (programs.metis), see nix/README.md."
      ;;
    *)
      die "unsupported distro ID=$id. Supported: Ubuntu 26.04+, Debian 13+, Arch, NixOS (flake).
See docs/UBUNTU_DEV.md or docs/PACKAGING.md."
      ;;
  esac
}

confirm_install() {
  local pkgs=("$@")
  log "Will install ${#pkgs[@]} packages:"
  printf '    %s\n' "${pkgs[@]}"
  if [[ "$YES" -eq 1 ]]; then
    return 0
  fi
  read -r -p "Continue? [y/N] " ans
  [[ "$ans" == "y" || "$ans" == "Y" ]] || die "aborted"
}

install_apt_packages() {
  local pkgs=("$@")
  confirm_install "${pkgs[@]}"
  sudo apt-get update
  if [[ "$YES" -eq 1 ]]; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${pkgs[@]}"
  else
    sudo apt-get install --no-install-recommends "${pkgs[@]}"
  fi
}

try_install_apt() {
  local pkgs=("$@")
  local filtered=()
  local p
  if [[ "${METIS_LAYER_SHELL_FROM_SOURCE:-0}" == "1" ]]; then
    for p in "${pkgs[@]}"; do
      [[ "$p" == *layer-shell* ]] && continue
      filtered+=("$p")
    done
    pkgs=("${filtered[@]}")
  fi
  set +e
  install_apt_packages "${pkgs[@]}"
  local st=$?
  set -e
  if [[ "$st" -eq 0 ]]; then
    return 0
  fi
  if [[ "${METIS_LAYER_SHELL_FROM_SOURCE:-0}" != "1" ]]; then
    log "apt install failed; retrying without layer-shell package (build from source)…"
    METIS_LAYER_SHELL_FROM_SOURCE=1
    filtered=()
    for p in "${pkgs[@]}"; do
      [[ "$p" == *layer-shell* ]] && continue
      filtered+=("$p")
    done
    install_apt_packages "${filtered[@]}"
    return
  fi
  die "apt install failed"
}

install_pacman_packages() {
  local pkgs=("$@")
  confirm_install "${pkgs[@]}"
  local flags=(--needed)
  [[ "$YES" -eq 1 ]] && flags+=(--noconfirm)
  sudo pacman -S "${flags[@]}" "${pkgs[@]}"
}

rustc_version() {
  rustc --version 2>/dev/null | awk '{print $2}' | sed 's/-.*//'
}

ensure_rust() {
  if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
    local have
    have="$(rustc_version)"
    if [[ -n "$have" ]] && version_ge "$have" "$METIS_MIN_RUST"; then
      log "Rust: $(rustc --version) / $(cargo --version)"
      return 0
    fi
    if command -v rustup >/dev/null 2>&1; then
      log "Rust ${have:-?} is older than $METIS_MIN_RUST — running rustup update stable…"
      rustup update stable
      rustup default stable >/dev/null
      have="$(rustc_version)"
      version_ge "${have:-0}" "$METIS_MIN_RUST" \
        || die "rustc ${have:-?} still older than $METIS_MIN_RUST after rustup update"
      log "Rust: $(rustc --version)"
      return 0
    fi
    die "rustc ${have:-?} is older than $METIS_MIN_RUST (distro rustc packages lag behind).
Remove the distro rust/cargo packages and install rustup: https://rustup.rs"
  fi
  log "Rust toolchain not found — installing via rustup…"
  if [[ "$YES" -ne 1 ]]; then
    read -r -p "Install rustup (stable)? [y/N] " ans
    [[ "$ans" == "y" || "$ans" == "Y" ]] || die "Rust is required; install rustup and re-run"
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
  command -v cargo >/dev/null 2>&1 || die "rustup finished but cargo not on PATH"
  log "Rust: $(rustc --version)"
}

ensure_gtk4_layer_shell() {
  if pkg-config --exists gtk4-layer-shell-0 2>/dev/null; then
    log "gtk4-layer-shell: $(pkg-config --modversion gtk4-layer-shell-0)"
    return 0
  fi
  if [[ "${METIS_LAYER_SHELL_FROM_SOURCE:-1}" != "1" ]]; then
    die "gtk4-layer-shell-0 not found via pkg-config after package install.
Install libgtk4-layer-shell-dev (Ubuntu / Debian) or gtk4-layer-shell (Arch),
or set METIS_LAYER_SHELL_FROM_SOURCE=1 to build it from source."
  fi
  log "Building gtk4-layer-shell $GTK4_LAYER_SHELL_TAG from source…"
  local src="/tmp/gtk4-layer-shell-metis"
  GTK4_LAYER_SHELL_TAG="$GTK4_LAYER_SHELL_TAG" \
    "$WORKSPACE/scripts/fetch-gtk4-layer-shell.sh" "$src"
  (
    cd "$src"
    meson setup build --prefix=/usr/local -Dexamples=false -Ddocs=false -Dtests=false
    ninja -C build
    sudo ninja -C build install
  )
  if [[ -d /usr/local/lib/x86_64-linux-gnu ]]; then
    echo /usr/local/lib/x86_64-linux-gnu | sudo tee /etc/ld.so.conf.d/gtk4-layer-shell.conf >/dev/null
  fi
  sudo ldconfig
  export PKG_CONFIG_PATH="/usr/local/lib/x86_64-linux-gnu/pkgconfig:/usr/local/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
  pkg-config --exists gtk4-layer-shell-0 || die "gtk4-layer-shell still missing after source build"
  log "gtk4-layer-shell: $(pkg-config --modversion gtk4-layer-shell-0)"
}

ensure_gtk_floor() {
  local gtk
  gtk="$(pkg-config --modversion gtk4 2>/dev/null || true)"
  [[ -n "$gtk" ]] || die "gtk4 not found via pkg-config (install the GTK 4 development package)"
  version_ge "$gtk" "4.18" || die "GTK $gtk is too old; Metis needs GTK >= 4.18."
  log "GTK: $gtk"
  local layer
  layer="$(pkg-config --modversion gtk4-layer-shell-0 2>/dev/null || true)"
  version_ge "${layer:-0}" "1.0" || die "gtk4-layer-shell ${layer:-?} is too old; Metis needs >= 1.0."
}

warn_mixed_install() {
  if [[ -x /usr/bin/metis-compositor && -x /usr/local/bin/metis-compositor ]]; then
    log "WARNING: both /usr and /usr/local Metis binaries exist."
    log "  Prefer one install: metis-desktop .deb (/usr) OR ./install.sh (/usr/local)."
  fi
  if dpkg -l metis 2>/dev/null | grep -q '^ii'; then
    if ! dpkg -l metis-desktop 2>/dev/null | grep -q '^ii'; then
      log "WARNING: package 'metis' is installed — on Ubuntu that may be the unrelated"
      log "  graph-partitioning tool, or an old Metis desktop package name. Prefer metis-desktop."
    fi
  fi
}

# --- main --------------------------------------------------------------------

[[ -x "$RUN_METIS" || -f "$RUN_METIS" ]] || die "missing $RUN_METIS (run from Metis repo root)"
[[ -d "$DEPS_DIR" ]] || die "missing $DEPS_DIR"

# Workspace crate SemVer (GitHub tags like v0.1.0.18 → Cargo 0.1.18 via
# scripts/sync-version.sh / sync-versions.sh). From-source installs use whatever
# is currently in metis-os-workspace/Cargo.toml — sync before tagging a release.
METIS_CARGO_VER="$(python3 - "$WORKSPACE/Cargo.toml" <<'PY'
import re, sys
text = open(sys.argv[1]).read().splitlines()
in_wp = False
for line in text:
    if line.startswith("["):
        in_wp = line.startswith("[workspace.package]")
    if in_wp and re.match(r"^version\s*=", line):
        m = re.search(r'"([^"]+)"', line)
        print(m.group(1) if m else "")
        break
PY
)"
if [[ -n "${METIS_CARGO_VER:-}" ]]; then
  log "Metis workspace version: ${METIS_CARGO_VER} (product tag style 0.1.0.N → sync with scripts/sync-version.sh)"
fi

DEPS_FILE="$(resolve_deps_file)"
log "Distro: ${PRETTY_NAME:-$ID $VERSION_ID}"
log "Deps profile: $(basename "$DEPS_FILE")"
# shellcheck disable=SC1090
. "$DEPS_FILE"

case "${ID:-}" in
  ubuntu|debian)
    pkgs=("${METIS_APT_PACKAGES[@]}")
    if [[ "$WITH_REMOTE" -eq 1 ]]; then
      pkgs+=("${METIS_APT_REMOTE[@]}")
    fi
    try_install_apt "${pkgs[@]}"
    ;;
  arch)
    pkgs=("${METIS_PACMAN_PACKAGES[@]}")
    if [[ "$WITH_REMOTE" -eq 1 ]]; then
      pkgs+=("${METIS_PACMAN_REMOTE[@]}")
    fi
    install_pacman_packages "${pkgs[@]}"
    ;;
  *)
    die "internal: unhandled ID=${ID:-}"
    ;;
esac

ensure_rust
ensure_gtk4_layer_shell
ensure_gtk_floor
warn_mixed_install

if [[ "$DEPS_ONLY" -eq 1 ]]; then
  log "Deps-only complete. Build/run with:"
  log "  cd metis-os-workspace/metis-shell && ./run-metis.sh --install-session"
  exit 0
fi

log "Building release and installing session to /usr/local…"
export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-}"
cd "$WORKSPACE/metis-shell"
./run-metis.sh --install-session

log ""
log "Done. Log out and pick **Metis** at the greeter (GDM/SDDM/…)."
log "Nested dev session (optional): ./run-metis.sh --session"
