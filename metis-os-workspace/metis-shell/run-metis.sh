#!/usr/bin/env bash
# Launch Metis — Wayland compositor + shell.
#
# Usage (from this directory):
#   ./run-metis.sh --install-session # normal path: build release, install to
#                                    # /usr/local, log out, pick "Metis" at greeter
#   ./run-metis.sh --session         # dev only: run compositor+shell from target/
#                                    # without installing (nested GNOME or bare TTY)
#                                    # Compositor spawns metis-shell (edge bar) and
#                                    # metis-shell --desktop-widgets (isolated)
#   ./run-metis.sh --session -- -c foot   # session + spawn a client app
#   ./run-metis.sh --session --import-env # also route D-Bus/systemd-activated
#                                         # apps into the nested session (dev)
#   ./run-metis.sh              # shell only (compositor must already run)
#   ./run-metis.sh --build      # force rebuild before run
#   ./run-metis.sh --release    # optimized binaries (default LTO release profile)
#   ./run-metis.sh --release-small # smallest install footprint (see Cargo.toml)
#   ./run-metis.sh --stop       # stop background shell process
#   ./run-metis.sh --verify-grid # compare compositor vs shell grid layouts
#   ./run-metis.sh --session --drm   # same as --session on a bare TTY (explicit)
#                                    # (switch to a free VT first; keep SSH open)
#
# Logs:
#   ~/.local/state/metis/logs/metis-YYYYMMDD-HHMMSS.log
#   ~/.local/state/metis/logs/latest.log  -> most recent run

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$ROOT/.." && pwd)"
cd "$WORKSPACE"

unset CARGO_TARGET_DIR
export CARGO_TARGET_DIR="$WORKSPACE/target"

WALLPAPER_DEFAULT="$WORKSPACE/assets/wallpapers/default.jpg"
WALLPAPER_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/metis/wallpaper.jpg"
# wallpaper.json holds the Settings app's selection; let the compositor resolve it
# (so a user pick survives restarts) and only fall back to a default here.
WALLPAPER_JSON="${XDG_CONFIG_HOME:-$HOME/.config}/metis/wallpaper.json"
if [[ -z "${METIS_WALLPAPER:-}" ]] && [[ -z "${METIS_NO_WALLPAPER:-}" ]]; then
    if [[ -f "$WALLPAPER_JSON" ]]; then
        : # compositor reads wallpaper.json directly — don't override it
    elif [[ -f "$WALLPAPER_CONFIG" ]]; then
        export METIS_WALLPAPER="$WALLPAPER_CONFIG"
    elif [[ -f "$WALLPAPER_DEFAULT" ]]; then
        export METIS_WALLPAPER="$WALLPAPER_DEFAULT"
    fi
fi

LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/metis/logs"
PID_FILE="${XDG_STATE_HOME:-$HOME/.local/state}/metis/metis.pid"
mkdir -p "$LOG_DIR" "$(dirname "$PID_FILE")"

# --- launch audit -----------------------------------------------------------
# Records who invoked this script (PID + full parent chain) on every run. If a
# Metis session ever relaunches itself "automatically", this log names the exact
# process responsible — the script/compositor have no respawn logic of their own,
# so any reopen comes from an external invoker (IDE task, shell trap, systemd,…).
AUDIT_LOG="${XDG_STATE_HOME:-$HOME/.local/state}/metis/launch-audit.log"
{
    printf '[%s] invoked pid=%s ppid=%s args=[%s]\n' "$(date '+%F %T')" "$$" "$PPID" "$*"
    p="$PPID"
    depth=0
    while [[ -n "$p" && "$p" -gt 1 && "$depth" -lt 10 ]]; do
        cmd="$(tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null)"
        printf '    parent[%s] pid=%s : %s\n' "$depth" "$p" "${cmd:-<gone>}"
        p="$(awk '{print $4}' "/proc/$p/stat" 2>/dev/null)"
        depth=$((depth + 1))
    done
} >>"$AUDIT_LOG" 2>/dev/null || true

FORCE_BUILD=0
SESSION=0
DRM=0
IMPORT_ENV=0
FOREGROUND=0
DO_STOP=0
DO_VERIFY=0
DO_VERIFY_GRID=0
DO_INSTALL_SESSION=0
PROFILE="dev"
COMP_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --)
            shift
            COMP_ARGS=("$@")
            break
            ;;
        --build) FORCE_BUILD=1 ;;
        --release) PROFILE="release" ;;
        --release-small) PROFILE="release-small" ;;
        --session) SESSION=1 ;;
        --drm) DRM=1 ;;
        --import-env) IMPORT_ENV=1 ;;
        --foreground) FOREGROUND=1 ;;
        --stop) DO_STOP=1 ;;
        --verify) DO_VERIFY=1 ;;
        --verify-grid) DO_VERIFY_GRID=1 ;;
        --install-session) DO_INSTALL_SESSION=1 ;;
        -h|--help)
            sed -n '2,29p' "$ROOT/run-metis.sh"
            exit 0
            ;;
        *)
            echo "Unknown option: $1 (try --help)" >&2
            exit 2
            ;;
    esac
    shift
done

log() {
    printf '[%s] %s\n' "$(date '+%H:%M:%S')" "$*"
}

# Keep Metis Settings / Viewer visible to gio::AppInfo (launcher + taskbar icons).
install_metis_settings_user_data() {
    local assets="$WORKSPACE/assets"
    local data="${XDG_DATA_HOME:-$HOME/.local/share}"
    mkdir -p "$data/applications" \
        "$data/icons/hicolor/256x256/apps" \
        "$data/icons/hicolor/48x48/apps"
    install -Dm644 "$assets/metis-settings.desktop" "$data/applications/metis-settings.desktop"
    install -Dm644 "$assets/metis-settings.png" "$data/icons/hicolor/256x256/apps/metis-settings.png"
    install -Dm644 "$assets/metis-settings-48.png" "$data/icons/hicolor/48x48/apps/metis-settings.png"
    if [[ -f "$assets/metis-viewer.desktop" ]]; then
        install -Dm644 "$assets/metis-viewer.desktop" "$data/applications/metis-viewer.desktop"
        install -Dm644 "$assets/metis-viewer.png" "$data/icons/hicolor/256x256/apps/metis-viewer.png"
        install -Dm644 "$assets/metis-viewer-48.png" "$data/icons/hicolor/48x48/apps/metis-viewer.png"
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$data/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

metis_data_dirs_base() {
    printf '%s' "${XDG_DATA_HOME:-$HOME/.local/share}:/usr/local/share:/usr/share"
}

# Append distro export trees so gio::AppInfo sees Flatpak and Snap launchers.
metis_extend_data_dirs() {
    XDG_DATA_DIRS="$(metis_data_dirs_base)"
    for _dir in "${XDG_DATA_HOME:-$HOME/.local/share}/flatpak/exports/share" \
                "/var/lib/flatpak/exports/share" \
                "/var/lib/snapd/desktop"; do
        case ":$XDG_DATA_DIRS:" in
            *":$_dir:"*) : ;;
            *) XDG_DATA_DIRS="$XDG_DATA_DIRS:$_dir" ;;
        esac
    done
    unset _dir
    export XDG_DATA_DIRS
}

# Map PROFILE (dev | release | release-small) to cargo flags and target subdir.
cargo_target_subdir() {
    case "$PROFILE" in
        release) printf '%s' release ;;
        release-small) printf '%s' release-small ;;
        *) printf '%s' debug ;;
    esac
}

cargo_build_flag() {
    case "$PROFILE" in
        release) printf '%s' --release ;;
        release-small) printf '%s' --profile ;;
        *) return 1 ;;
    esac
}

cargo_build_profile_name() {
    case "$PROFILE" in
        release-small) printf '%s' release-small ;;
        *) return 1 ;;
    esac
}

# `sudo ./run-metis.sh` sets HOME=/root — load the invoking user's rustup cargo.
resolve_build_user_home() {
    if [[ -n "${SUDO_USER:-}" ]] && [[ "$SUDO_USER" != "root" ]]; then
        getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6
    else
        printf '%s' "$HOME"
    fi
}

ensure_cargo_in_path() {
    local build_home
    build_home="$(resolve_build_user_home)"
    if [[ -f "$build_home/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "$build_home/.cargo/env"
    fi
    if [[ -d "$build_home/.cargo/bin" ]]; then
        export PATH="$build_home/.cargo/bin${PATH:+:$PATH}"
    fi
    # sudo sets HOME=/root; point rustup/cargo at the invoking user's toolchain.
    if [[ -n "${SUDO_USER:-}" ]] && [[ "$SUDO_USER" != "root" ]]; then
        export CARGO_HOME="${CARGO_HOME:-$build_home/.cargo}"
        export RUSTUP_HOME="${RUSTUP_HOME:-$build_home/.rustup}"
    fi
}

run_section() {
    log "--- $* ---"
}

verify_keybind_chain() {
    local ok=0
    local cmd_script="$ROOT/scripts/metis-cmd.sh"
    local runtime="${XDG_RUNTIME_DIR:-}"

    run_section "Metis compositor + shell verification"
    log "Run this while Metis is active (./run-metis.sh --session or shell attached to compositor)."

    if [[ -z "$runtime" ]]; then
        log "FAIL: XDG_RUNTIME_DIR is not set."
        ok=1
    else
        log "OK:   XDG_RUNTIME_DIR=$runtime"
    fi

    if [[ ! -x "$cmd_script" ]]; then
        log "FAIL: $cmd_script missing or not executable (run ./run-metis.sh once to install)."
        ok=1
    else
        log "OK:   metis-cmd.sh is executable"
    fi

    if [[ -S "${XDG_RUNTIME_DIR:-/tmp}/metis/compositor.sock" ]]; then
        log "OK:   Metis compositor IPC socket present"
    else
        log "FAIL: compositor socket missing — start with ./run-metis.sh --session"
        ok=1
    fi

    if [[ -f "$PID_FILE" ]]; then
        local pid
        pid="$(cat "$PID_FILE")"
        if kill -0 "$pid" 2>/dev/null; then
            log "OK:   Metis daemon running (PID $pid)"
        else
            log "FAIL: Metis pid file exists but process $pid is dead — run ./run-metis.sh --stop && ./run-metis.sh"
            ok=1
        fi
    else
        log "FAIL: Metis is not running — Super+Space has nothing to talk to. Run ./run-metis.sh"
        ok=1
    fi

    if [[ -n "$runtime" && -x "$cmd_script" && -f "$PID_FILE" ]]; then
        local pid
        pid="$(cat "$PID_FILE")"
        if kill -0 "$pid" 2>/dev/null; then
            run_section "IPC smoke test"
            rm -f "$runtime/metis/command"
            if bash "$cmd_script" close-popovers; then
                sleep 0.15
                if [[ ! -f "$runtime/metis/command" ]]; then
                    log "OK:   metis-cmd close-popovers was consumed by the shell"
                else
                    log "FAIL: command file still present — shell is not polling $runtime/metis/command"
                    ok=1
                fi
            else
                log "FAIL: metis-cmd.sh close-popovers exited with error"
                ok=1
            fi
        fi
    fi

    echo
    if [[ "$ok" -eq 0 ]]; then
        log "All checks passed. Runtime commands reach the edge bar."
    else
        log "Some checks failed. Fix the FAIL lines above, then: ./run-metis.sh --stop && ./run-metis.sh --build"
    fi
    return "$ok"
}

if [[ "$DO_INSTALL_SESSION" -eq 1 ]]; then
    # Install the GDM/login "Metis" session entry, Hyprland-style:
    #   - release binaries  -> /usr/local/bin/{metis-compositor,metis-shell,metis-settings}
    #   - launcher script    -> /usr/local/bin/metis-session
    #   - wayland-session    -> /usr/local/share/wayland-sessions/metis.desktop
    # Re-login at the greeter and pick "Metis" from the gear/session menu.
    BIN_DST="${METIS_PREFIX_BIN:-/usr/local/bin}"
    SESSIONS_DST="${METIS_SESSIONS_DIR:-/usr/local/share/wayland-sessions}"
    ASSETS_DIR="$WORKSPACE/assets"

    # Install always builds optimized binaries unless --release-small was passed.
    [[ "$PROFILE" == "dev" ]] && PROFILE="release"

    ensure_cargo_in_path
    for pc_dir in \
        /usr/local/lib/x86_64-linux-gnu/pkgconfig \
        /usr/local/lib/pkgconfig \
        /usr/lib/x86_64-linux-gnu/pkgconfig; do
        [[ -d "$pc_dir" ]] && PKG_CONFIG_PATH="${pc_dir}${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    done
    export PKG_CONFIG_PATH

    echo "Building ${PROFILE} binaries …"
    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo not in PATH." >&2
        echo "  Install Rust: https://rustup.rs" >&2
        echo "  Prefer running without sudo — only the install step needs root:" >&2
        echo "    ./run-metis.sh --install-session" >&2
        exit 1
    fi
    BUILD_ARGS=()
    if flag="$(cargo_build_flag)"; then
        BUILD_ARGS+=("$flag")
        if prof="$(cargo_build_profile_name)"; then
            BUILD_ARGS+=("$prof")
        fi
    fi
    if ! cargo build "${BUILD_ARGS[@]}" -p metis-compositor -p metis-shell -p metis-settings -p metis-portal -p metis-remote -p metis-polkit-agent -p metis-viewer -p metis-screenshot -p metis-gaming; then
        echo "ERROR: release build failed." >&2
        exit 1
    fi

    REL="$CARGO_TARGET_DIR/$(cargo_target_subdir)"
    # xdg-desktop-portal only loads backend descriptors from
    # /usr/share/xdg-desktop-portal/portals/ — installing under /usr/local alone
    # makes apps wait ~25s per portal call while xdp times out on GNOME backends.
    PORTALS_DST="${METIS_PORTALS_DIR:-/usr/share/xdg-desktop-portal}"
    # Privileged copies: re-exec the install steps under sudo if not already root.
    SUDO=""
    if [[ "$(id -u)" -ne 0 ]]; then
        if command -v sudo >/dev/null 2>&1; then
            SUDO="sudo"
        else
            echo "ERROR: need root to write $BIN_DST and $SESSIONS_DST (install sudo or run as root)." >&2
            exit 1
        fi
    fi

    echo "Installing binaries to $BIN_DST …"
    $SUDO install -Dm755 "$REL/metis-compositor" "$BIN_DST/metis-compositor"
    $SUDO install -Dm755 "$REL/metis-shell" "$BIN_DST/metis-shell"
    if [[ -x "$REL/metis-settings" ]]; then
        $SUDO install -Dm755 "$REL/metis-settings" "$BIN_DST/metis-settings"
    fi
    if [[ -x "$REL/metis-remote" ]]; then
        $SUDO install -Dm755 "$REL/metis-remote" "$BIN_DST/metis-remote"
        # Polkit actions annotate /usr/bin/metis-remote — keep a copy there too.
        if [[ "$BIN_DST" != /usr/bin ]]; then
            $SUDO install -Dm755 "$REL/metis-remote" /usr/bin/metis-remote
        fi
    fi
    if [[ -x "$REL/metis-polkit-agent" ]]; then
        $SUDO install -Dm755 "$REL/metis-polkit-agent" "$BIN_DST/metis-polkit-agent"
        $SUDO install -Dm755 "$REL/metis-polkit-agent" /usr/libexec/metis-polkit-agent
    fi

    POLICY_SRC="$WORKSPACE/packaging/polkit/org.metis.policy"
    if [[ -f "$POLICY_SRC" ]]; then
        echo "Installing Polkit policy …"
        $SUDO install -Dm644 "$POLICY_SRC" /usr/share/polkit-1/actions/org.metis.policy
    fi
    if [[ -x "$REL/metis-viewer" ]]; then
        $SUDO install -Dm755 "$REL/metis-viewer" "$BIN_DST/metis-viewer"
    fi
    if [[ -x "$REL/metis-screenshot" ]]; then
        $SUDO install -Dm755 "$REL/metis-screenshot" "$BIN_DST/metis-screenshot"
    fi
    if [[ -x "$REL/metis-gamingd" ]]; then
        $SUDO install -Dm755 "$REL/metis-gamingd" "$BIN_DST/metis-gamingd"
    fi
    $SUDO install -Dm755 "$ASSETS_DIR/metis-session" "$BIN_DST/metis-session"
    if [[ -x "$REL/metis-portal" ]]; then
        $SUDO install -Dm755 "$REL/metis-portal" "$BIN_DST/metis-portal"
    fi

    echo "Installing portal registration to $PORTALS_DST …"
    $SUDO install -Dm644 "$ASSETS_DIR/metis.portal" "$PORTALS_DST/portals/metis.portal"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-portals.conf" "$PORTALS_DST/metis-portals.conf"
    # Legacy path used by early installs — keep both in sync.
    if [[ "$PORTALS_DST" != /usr/local/share/xdg-desktop-portal ]]; then
        $SUDO install -Dm644 "$ASSETS_DIR/metis.portal" /usr/local/share/xdg-desktop-portal/portals/metis.portal
        $SUDO install -Dm644 "$ASSETS_DIR/metis-portals.conf" /usr/local/share/xdg-desktop-portal/metis-portals.conf
    fi

    echo "Installing session entry to $SESSIONS_DST/metis.desktop …"
    $SUDO install -Dm644 "$ASSETS_DIR/metis.desktop" "$SESSIONS_DST/metis.desktop"

    echo "Installing Metis Settings / Viewer desktop entries …"
    ICONS_DST="${METIS_ICONS_DIR:-/usr/share/icons/hicolor}"
    APPS_DST="${METIS_APPS_DIR:-/usr/share/applications}"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-settings-48.png" "$ICONS_DST/48x48/apps/metis-settings.png"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-settings.png" "$ICONS_DST/256x256/apps/metis-settings.png"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-viewer-48.png" "$ICONS_DST/48x48/apps/metis-viewer.png"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-viewer.png" "$ICONS_DST/256x256/apps/metis-viewer.png"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-settings.desktop" "$APPS_DST/metis-settings.desktop"
    $SUDO install -Dm644 "$ASSETS_DIR/metis-viewer.desktop" "$APPS_DST/metis-viewer.desktop"
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        $SUDO gtk-update-icon-cache -f -t "$ICONS_DST" >/dev/null 2>&1 || true
    fi

    # Bundled wallpapers for onboarding / Appearance (looked up under
    # /usr/share/metis/wallpapers and /usr/local/share/metis/wallpapers).
    WALLPAPERS_DST="${METIS_WALLPAPERS_DIR:-/usr/share/metis/wallpapers}"
    echo "Installing bundled wallpapers to $WALLPAPERS_DST …"
    $SUDO mkdir -p "$WALLPAPERS_DST"
    shopt -s nullglob
    for wp in "$ASSETS_DIR/wallpapers"/*.{png,jpg,jpeg,webp,PNG,JPG,JPEG,WEBP}; do
        [[ -f "$wp" ]] || continue
        $SUDO install -Dm644 "$wp" "$WALLPAPERS_DST/$(basename "$wp")"
    done
    shopt -u nullglob

    # Example / system JSON widget extension packs (Phase 14 §E).
    WIDGETS_DST="${METIS_WIDGETS_DIR:-/usr/share/metis/widgets}"
    if [[ -d "$ASSETS_DIR/widgets" ]]; then
        echo "Installing widget extension packs to $WIDGETS_DST …"
        $SUDO mkdir -p "$WIDGETS_DST"
        for pack in "$ASSETS_DIR/widgets"/*; do
            [[ -d "$pack" ]] || continue
            name="$(basename "$pack")"
            $SUDO mkdir -p "$WIDGETS_DST/$name"
            if [[ -f "$pack/manifest.json" ]]; then
                $SUDO install -Dm644 "$pack/manifest.json" "$WIDGETS_DST/$name/manifest.json"
            fi
            if [[ -f "$pack/widget.json" ]]; then
                $SUDO install -Dm644 "$pack/widget.json" "$WIDGETS_DST/$name/widget.json"
            fi
            # Optional out-of-process helper binary (basename only; +x).
            if [[ -f "$pack/helper" ]]; then
                $SUDO install -Dm755 "$pack/helper" "$WIDGETS_DST/$name/helper"
            fi
            # Any other executable files declared by the pack (not .json).
            for bin in "$pack"/*; do
                [[ -f "$bin" && -x "$bin" ]] || continue
                base="$(basename "$bin")"
                [[ "$base" == "helper" ]] && continue
                case "$base" in
                    *.json|*.md|*.txt) continue ;;
                esac
                $SUDO install -Dm755 "$bin" "$WIDGETS_DST/$name/$base"
            done
        done
    fi

    # i18n catalogs (gettext .mo + Fluent .ftl).
    LOCALE_SRC="$(cd "$(dirname "$0")/.." && pwd)/assets/locale"
    # Prefer workspace assets next to this script's package root.
    if [[ ! -d "$LOCALE_SRC" ]]; then
        LOCALE_SRC="$ASSETS_DIR/../locale"
    fi
    if [[ ! -d "$LOCALE_SRC" ]]; then
        LOCALE_SRC="$(cd "$(dirname "$0")/../../assets/locale" 2>/dev/null && pwd)" || true
    fi
    LOCALE_DST="${METIS_LOCALE_DIR:-/usr/local/share/metis/locale}"
    if [[ -d "$LOCALE_SRC" ]]; then
        if [[ -x "$(dirname "$0")/../../scripts/i18n-compile.sh" ]]; then
            "$(dirname "$0")/../../scripts/i18n-compile.sh" || true
        elif [[ -x "$(cd "$(dirname "$0")/.." && pwd)/scripts/i18n-compile.sh" ]]; then
            "$(cd "$(dirname "$0")/.." && pwd)/scripts/i18n-compile.sh" || true
        fi
        echo "Installing locale catalogs to $LOCALE_DST …"
        $SUDO mkdir -p "$LOCALE_DST"
        $SUDO cp -a "$LOCALE_SRC/." "$LOCALE_DST/"
        echo "  catalogs: $(find "$LOCALE_DST" -name 'metis.mo' 2>/dev/null | wc -l) .mo, $(find "$LOCALE_DST" -name 'metis.ftl' 2>/dev/null | wc -l) .ftl"
    else
        echo "WARNING: locale catalogs not found (looked for assets/locale) — Language & region will stay English." >&2
        echo "  expected near: $LOCALE_SRC" >&2
    fi

    # PAM service for the compositor lock screen. Without it the compositor
    # falls back to the system "login" stack; installing the dedicated service
    # keeps unlocking working consistently across distros.
    echo "Installing PAM service to /etc/pam.d/metis …"
    $SUDO install -Dm644 "$ASSETS_DIR/pam-metis" /etc/pam.d/metis

    # Runtime dependency: Metis is only a *client* of the Secret Service, so a
    # provider must exist or keyring-backed apps (and Metis's own credential
    # storage) degrade to plaintext. The session launcher auto-starts whichever
    # provider is installed; warn here if none is present yet.
    echo
    if command -v gnome-keyring-daemon >/dev/null 2>&1 \
        || command -v kwalletd6 >/dev/null 2>&1 \
        || command -v kwalletd5 >/dev/null 2>&1 \
        || command -v keepassxc >/dev/null 2>&1 \
        || command -v pass-secret-service >/dev/null 2>&1 \
        || ls /usr/share/dbus-1/services/org.freedesktop.secrets.service >/dev/null 2>&1; then
        echo "Keyring: a Secret Service provider is installed — good."
    else
        echo "WARNING: no keyring / Secret Service provider found."
        echo "  Keyring-backed apps (Cursor, GitHub Desktop, browsers) and Metis's own"
        echo "  credential storage will fall back to plaintext until one is installed."
        echo "  Recommended (desktop-independent): sudo apt install -y gnome-keyring"
    fi

    echo
    echo "Done. Log out, then pick 'Metis' from your login manager's session menu."
    echo "Tip: keep an SSH session open the first time; Ctrl+Alt+Backspace quits Metis,"
    echo "     Ctrl+Alt+F<n> switches VT."
    exit 0
fi

if [[ "$DO_STOP" -eq 1 ]]; then
    stopped=0
    if [[ -f "$PID_FILE" ]]; then
        pid="$(cat "$PID_FILE")"
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" && echo "Stopped Metis shell (PID $pid)."
            stopped=1
        else
            echo "Metis shell not running (stale PID $pid)."
        fi
        rm -f "$PID_FILE"
    fi
    if comp_pid="$(pgrep -x metis-compositor 2>/dev/null)"; then
        kill "$comp_pid" 2>/dev/null && echo "Stopped Metis compositor (PID $comp_pid)."
        stopped=1
    fi
    if [[ "$stopped" -eq 0 && ! -f "$PID_FILE" ]]; then
        echo "Metis is not running (no pid file)."
    fi
    exit 0
fi

verify_grid_layout() {
    local ok=0
    local runtime="${XDG_RUNTIME_DIR:-}"
    local socket="${runtime}/metis/compositor.sock"
    local shell_layout
    shell_layout="$(python3 - <<'PY'
import json, os
from pathlib import Path
home = Path(os.environ.get("HOME", ""))
for path in [
    home / ".config/metis/desk.json",
]:
    if path.exists():
        data = json.loads(path.read_text())
        ids = sorted(t["id"] for t in data.get("tiles", []))
        print(json.dumps(ids))
        break
else:
    print("[]")
PY
)"

    run_section "Grid layout verification"
    if [[ ! -S "$socket" ]]; then
        log "FAIL: compositor socket missing — start with ./run-metis.sh --session"
        return 1
    fi

    local comp_layout
    comp_layout="$(python3 - <<'PY'
import json, socket, os
path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "metis/compositor.sock")
payload = json.dumps({"cmd": "get_layout"}) + "\n"
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
s.sendall(payload.encode())
data = s.recv(65536).decode()
line = data.strip().splitlines()[0]
evt = json.loads(line)
ids = sorted(t["id"] for t in evt.get("layout", {}).get("tiles", []))
print(json.dumps(ids))
PY
)" || true

    if [[ -z "$comp_layout" ]]; then
        log "FAIL: could not read layout from compositor (GetLayout IPC)"
        ok=1
    else
        log "Shell tile ids:      $shell_layout"
        log "Compositor tile ids: $comp_layout"
        if [[ "$shell_layout" == "$comp_layout" ]]; then
            log "OK:   compositor and shell tile lists match"
        else
            log "FAIL: compositor/shell tile lists diverge"
            ok=1
        fi
    fi

    if [[ "$comp_layout" == *"app-"* ]]; then
        log "OK:   compositor layout contains app-* tile(s)"
    else
        log "WARN: no app-* tiles in compositor layout (launch foot to verify)"
    fi

    return "$ok"
}

if [[ "$DO_VERIFY_GRID" -eq 1 ]]; then
    verify_grid_layout
    exit $?
fi

if [[ "$DO_VERIFY" -eq 1 ]]; then
    verify_keybind_chain
    exit $?
fi

STAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="$LOG_DIR/metis-$STAMP.log"
ln -sfn "$LOG_FILE" "$LOG_DIR/latest.log"

# From here on, log() output is also tee'd to LOG_FILE via the block at end of script.
log_tee() {
    printf '[%s] %s\n' "$(date '+%H:%M:%S')" "$*"
}

log() {
    log_tee "$@"
}

run_section() {
    log "--- $* ---"
}

binary_needs_rebuild() {
    local bin="$1"
    [[ ! -x "$bin" ]] && return 0

    local workspace="$WORKSPACE"
    local newest_src
    newest_src="$(find "$workspace/metis-compositor" "$workspace/metis-shell" "$workspace/metis-grid" "$workspace/metis-protocol" \
        "$workspace/metis-config" "$workspace/metis-secrets" "$workspace/metis-settings" "$workspace/metis-viewer" \
        "$workspace/metis-screenshot" "$workspace/metis-capture" \
        -name '*.rs' -newer "$bin" 2>/dev/null | head -1)"
    if [[ -n "$newest_src" ]]; then
        log "Source changed since last build ($newest_src) — rebuild required."
        return 0
    fi

    local interp
    interp="$(readelf -l "$bin" 2>/dev/null | awk '/Requesting program interpreter/{print $NF}' | tr -d '[]')"
    if [[ -n "$interp" && "$interp" == /nix/store/* && ! -f "$interp" ]]; then
        log "Stale binary: linked against Nix glibc ($interp) — rebuild required."
        return 0
    fi

    # Stale-binary check WITHOUT executing the binary. We must NOT run the binary
    # to probe it: `metis-compositor` ignores `--help` (its arg parser only knows
    # `-c`/`--command`) and would boot a full nested compositor window — which
    # looked exactly like the session "closing and auto-reopening". Just verify
    # the ELF interpreter still exists on disk.
    if [[ -n "$interp" && ! -f "$interp" ]]; then
        log "Stale binary: interpreter missing ($interp) — rebuild required."
        return 0
    fi

    return 1
}

check_build_deps() {
    local missing=0
    for pc in gtk4 gtk4-layer-shell-0; do
        if pkg-config --exists "$pc" 2>/dev/null; then
            log "pkg-config $pc: $(pkg-config --modversion "$pc")"
        else
            log "ERROR: pkg-config '$pc' not found."
            missing=1
        fi
    done

    if [[ "$missing" -eq 1 ]]; then
        log ""
        log "Install build dependencies, then re-run:"
        log "  sudo apt install -y libgtk-4-dev libgtk4-layer-shell-dev libgraphene-1.0-dev pkg-config"
        log "  # or run ../../install.sh (Ubuntu 26.04+, Debian 13+, Arch)"
        log "  ./run-metis.sh --build"
        return 1
    fi

    local gtk_ver
    gtk_ver="$(pkg-config --modversion gtk4)"
    if ! pkg-config --atleast-version=4.18 gtk4; then
        log "ERROR: GTK $gtk_ver is too old; Metis needs GTK >= 4.18 (Ubuntu 26.04+, Debian 13+)."
        return 1
    fi
    if ! pkg-config --atleast-version=1.0 gtk4-layer-shell-0; then
        log "ERROR: gtk4-layer-shell $(pkg-config --modversion gtk4-layer-shell-0) is too old; Metis needs >= 1.0."
        return 1
    fi
    return 0
}

# True if the freedesktop Secret Service is already owned on the user bus or is
# D-Bus activatable (a provider auto-starts on first access — nothing to launch).
metis_have_secret_service() {
    if command -v busctl >/dev/null 2>&1 \
        && busctl --user status org.freedesktop.secrets >/dev/null 2>&1; then
        return 0
    fi
    if command -v gdbus >/dev/null 2>&1 \
        && gdbus call --session --dest org.freedesktop.DBus \
            --object-path /org/freedesktop/DBus \
            --method org.freedesktop.DBus.NameHasOwner org.freedesktop.secrets \
            2>/dev/null | grep -q true; then
        return 0
    fi
    local dir
    local IFS=:
    for dir in /usr/local/share /usr/share ${XDG_DATA_DIRS:-}; do
        [ -e "$dir/dbus-1/services/org.freedesktop.secrets.service" ] && return 0
    done
    return 1
}

# Start the best available Secret Service provider so keyring-backed apps (and
# Metis's own oo7 client) don't fall back to plaintext. Preference order favors
# whatever is installed; gnome-keyring is the recommended default.
metis_start_secret_service() {
    if metis_have_secret_service; then
        log "Keyring: org.freedesktop.secrets already available"
        return 0
    fi
    if command -v gnome-keyring-daemon >/dev/null 2>&1; then
        eval "$(gnome-keyring-daemon --start --components=secrets,ssh 2>/dev/null)"
        export SSH_AUTH_SOCK
        log "Keyring: started gnome-keyring-daemon (secrets + ssh-agent)"
    elif command -v kwalletd6 >/dev/null 2>&1; then
        kwalletd6 >/dev/null 2>&1 &
        log "Keyring: started kwalletd6 (KWallet Secret Service)"
    elif command -v kwalletd5 >/dev/null 2>&1; then
        kwalletd5 >/dev/null 2>&1 &
        log "Keyring: started kwalletd5 (KWallet Secret Service)"
    elif command -v keepassxc >/dev/null 2>&1; then
        keepassxc >/dev/null 2>&1 &
        log "Keyring: started KeePassXC (enable Secret Service integration in its settings)"
    elif command -v pass-secret-service >/dev/null 2>&1; then
        pass-secret-service >/dev/null 2>&1 &
        log "Keyring: started pass-secret-service"
    else
        log "Keyring: WARNING no Secret Service provider found — keyring-backed apps will"
        log "         fall back to plaintext. Install one (recommended: sudo apt install gnome-keyring)."
    fi
}

# --- environment -----------------------------------------------------------

ensure_cargo_in_path

# gtk4-layer-shell built from source (METIS_LAYER_SHELL_FROM_SOURCE=1) lands in /usr/local
for pc_dir in \
    /usr/local/lib/x86_64-linux-gnu/pkgconfig \
    /usr/local/lib/pkgconfig \
    /usr/lib/x86_64-linux-gnu/pkgconfig; do
    if [[ -d "$pc_dir" ]]; then
        PKG_CONFIG_PATH="${pc_dir}${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    fi
done
export PKG_CONFIG_PATH

export RUST_BACKTRACE=1
export RUST_LOG="${RUST_LOG:-metis_shell=info,metis_compositor=info,warn}"

# --- preflight -------------------------------------------------------------

{
    run_section "Metis Shell launch"
    log "Project:  $ROOT"
    log "Log file: $LOG_FILE"
    log "Profile:  $PROFILE"
    echo

    run_section "Session"
    # True when a host compositor already owns the display (GNOME terminal, SSH
    # with forwarded WAYLAND_DISPLAY, etc.). A bare TTY has neither set.
    nested_host_session() {
        [[ -n "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]
    }

    if [[ "$SESSION" -eq 1 ]] && { [[ "$DRM" -eq 1 ]] || ! nested_host_session; }; then
        # Standalone DRM/TTY session: own the seat outright. Pin the backend and
        # use the real defaults (Super modifier, wallpaper/briefing ON) rather
        # than the nested dev fallbacks below.
        export METIS_BACKEND=drm
        : "${METIS_MOD:=super}"
        export METIS_MOD
        # Standalone session has no host desktop: set the Metis identity (with a
        # trailing :GNOME so Chromium/Electron apps pick the gnome-libsecret
        # keyring backend) unless the caller already chose one.
        : "${XDG_CURRENT_DESKTOP:=Metis:GNOME}"
        export XDG_CURRENT_DESKTOP
        : "${XDG_SESSION_DESKTOP:=metis}"
        export XDG_SESSION_DESKTOP
        log "Backend: DRM/KMS (TTY / standalone) — METIS_BACKEND=drm"
        log "Escape:  Ctrl+Alt+Backspace quits · Ctrl+Alt+F<n> switches VT"
        log "Tip:     daily use → ./run-metis.sh --install-session, then pick Metis at greeter"
    elif [[ "$SESSION" -eq 1 ]]; then
        # Nested dev session inside GNOME/KDE/etc. Never inherit METIS_BACKEND=drm
        # from a prior login-session environment — DRM cannot open the GPU twice.
        export METIS_BACKEND=winit

        # Nested dev sessions default wallpaper + briefing OFF. Distinguish an
        # explicit empty value (METIS_NO_WALLPAPER=, meaning "enable") from an
        # unset var ("use the session default of disabled"). Using ${VAR-...}
        # (no colon) treats an explicit empty value as set.
        if [[ -z "${METIS_NO_WALLPAPER+set}" ]]; then
            export METIS_NO_WALLPAPER=1   # unset → disable
            unset METIS_WALLPAPER
        elif [[ -z "$METIS_NO_WALLPAPER" ]]; then
            unset METIS_NO_WALLPAPER       # explicit empty → enable
        else
            unset METIS_WALLPAPER          # non-empty → disable
        fi

        if [[ -z "${METIS_NO_BRIEFING+set}" ]]; then
            export METIS_NO_BRIEFING=1     # unset → disable
        elif [[ -z "$METIS_NO_BRIEFING" ]]; then
            unset METIS_NO_BRIEFING        # explicit empty → enable
        fi

        # GNOME (and most host compositors) grab Super globally, so nested dev
        # sessions default to Alt for compositor shortcuts. Override with
        # METIS_MOD=super if you've disabled the conflicting host bindings.
        if [[ -z "${METIS_MOD+set}" ]]; then
            export METIS_MOD=alt
        fi
    fi
    log "XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset}"
    log "WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset}"
    log "XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-unset}"
    log "DESKTOP_SESSION=${DESKTOP_SESSION:-unset}"
    if [[ "$SESSION" -eq 1 ]] && nested_host_session; then
        log "METIS_BACKEND=${METIS_BACKEND:-unset} (nested dev — winit window inside host desktop)"
        if [[ -n "${DESKTOP_SESSION:-}" ]] && [[ "${DESKTOP_SESSION}" == metis* ]]; then
            log "WARN: WAYLAND_DISPLAY is set — you look nested inside another session."
            log "      For a real TTY desktop: ./run-metis.sh --install-session, log out, pick Metis."
        fi
    elif [[ -n "${METIS_BACKEND:-}" ]]; then
        log "METIS_BACKEND=${METIS_BACKEND}"
    fi
    if [[ -n "${METIS_MOD:-}" ]]; then
        log "METIS_MOD=${METIS_MOD} (compositor shortcuts use this modifier)"
    fi
    if [[ -n "${METIS_WALLPAPER:-}" ]]; then
        log "Wallpaper: $METIS_WALLPAPER"
    elif [[ -n "${METIS_NO_WALLPAPER:-}" ]]; then
        log "Wallpaper: disabled (METIS_NO_WALLPAPER)"
    fi

    if [[ "$IMPORT_ENV" -eq 1 ]] && [[ "$SESSION" -eq 0 ]]; then
        log "WARN: --import-env only applies to a full session; ignoring (add --session)."
    fi

    if [[ "$SESSION" -eq 0 ]] && [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
        log "ERROR: WAYLAND_DISPLAY is not set."
        log "Install + greeter login: ./run-metis.sh --install-session"
        log "Or dev session:          ./run-metis.sh --session"
        exit 1
    fi

    if [[ "$SESSION" -eq 1 ]]; then
        # Nested compositor: Cairo renderer avoids blank/hung GTK layer-shell on some drivers.
        # Opt into GPU GSK with METIS_SHELL_GSK_RENDERER=gl (or GSK_RENDERER=gl) when drivers are stable.
        export GDK_BACKEND="${GDK_BACKEND:-wayland}"
        export GSK_RENDERER="${METIS_SHELL_GSK_RENDERER:-${GSK_RENDERER:-cairo}}"
        # Prefer native Wayland for Electron/Chromium apps — their XWayland
        # map/unmap lifecycle is flaky under Metis (Claude Desktop opens then
        # cleanly quits). CLAUDE_USE_WAYLAND flips Claude's launcher off its
        # forced `--ozone-platform=x11`; the Ozone hint covers other Electron apps.
        export ELECTRON_OZONE_PLATFORM_HINT="${ELECTRON_OZONE_PLATFORM_HINT:-auto}"
        export CLAUDE_USE_WAYLAND="${CLAUDE_USE_WAYLAND:-1}"
        # Flatpak + Snap export their .desktop/icons under dedicated trees; put
        # those on XDG_DATA_DIRS so the Metis launcher/dock (via gio::AppInfo)
        # sees them. Mirrors assets/metis-session; harmless when absent.
        metis_extend_data_dirs
        # Nested session: ignore stale IPC sockets from a prior crashed run.
        rm -f "${XDG_RUNTIME_DIR:-/tmp}/metis/compositor.sock" \
              "${XDG_RUNTIME_DIR:-/tmp}/metis/compositor-events.sock" 2>/dev/null || true
    elif [[ -S "${XDG_RUNTIME_DIR:-/tmp}/metis/compositor.sock" ]]; then
        log "Compositor IPC: connected"
    elif [[ "$SESSION" -eq 0 ]]; then
        log "WARN: Metis compositor not running — use ./run-metis.sh --session"
    fi

    if ! command -v cargo >/dev/null 2>&1; then
        log "ERROR: cargo not in PATH. Install Rust: https://rustup.rs"
        exit 1
    fi
    log "Cargo:    $(cargo --version) ($(command -v cargo))"
    log "Rustc:    $(rustc --version 2>/dev/null || echo 'not found')"
    log "PKG_CONFIG_PATH=${PKG_CONFIG_PATH:-unset}"

    run_section "Build dependencies"
    if ! check_build_deps; then
        exit 1
    fi
    echo

    TARGET_DIR="$CARGO_TARGET_DIR"
    TARGET_SUB="$(cargo_target_subdir)"
    if [[ "$PROFILE" != "dev" ]]; then
        SHELL_BIN="$TARGET_DIR/$TARGET_SUB/metis-shell"
        COMP_BIN="$TARGET_DIR/$TARGET_SUB/metis-compositor"
        BUILD_ARGS=()
        if flag="$(cargo_build_flag)"; then
            BUILD_ARGS+=("$flag")
            if prof="$(cargo_build_profile_name)"; then
                BUILD_ARGS+=("$prof")
            fi
        fi
        BUILD_CMD=(cargo build "${BUILD_ARGS[@]}" -p metis-shell -p metis-compositor -p metis-settings -p metis-remote -p metis-polkit-agent -p metis-viewer -p metis-screenshot -p metis-gaming)
    else
        SHELL_BIN="$TARGET_DIR/debug/metis-shell"
        COMP_BIN="$TARGET_DIR/debug/metis-compositor"
        BUILD_CMD=(cargo build -p metis-shell -p metis-compositor -p metis-settings -p metis-remote -p metis-polkit-agent -p metis-viewer -p metis-screenshot -p metis-gaming)
    fi

    if [[ "$FORCE_BUILD" -eq 1 ]] || binary_needs_rebuild "$SHELL_BIN" || binary_needs_rebuild "$COMP_BIN"; then
        run_section "Build"
        log "Running: ${BUILD_CMD[*]} (compiler warnings are OK)"
        build_log="$(mktemp)"
        if ! "${BUILD_CMD[@]}" 2>&1 | tee "$build_log"; then
            log "ERROR: cargo build failed — see log above."
            rm -f "$build_log"
            exit 1
        fi
        warn_count="$(grep -c '^warning:' "$build_log" 2>/dev/null || true)"
        rm -f "$build_log"
        log "Build OK: $SHELL_BIN"
        log "Build OK: $COMP_BIN"
        if [[ "${warn_count:-0}" -gt 0 ]]; then
            log "Note: $warn_count compiler warning(s) — safe to ignore for dev builds."
        fi
        log "Interpreter: $(readelf -l "$SHELL_BIN" 2>/dev/null | awk '/Requesting program interpreter/{print $NF}' | tr -d '[]')"
    else
        run_section "Build"
        log "Using existing binaries (pass --build to rebuild)"
        log "  shell: $SHELL_BIN"
        log "  compositor: $COMP_BIN"
    fi
    echo

    # Dev binaries live in the cargo target dir, which is usually not on PATH.
    # GIO's TryExec check (via AppInfo::should_show) needs them there so menu
    # entries such as metis-settings are not filtered out before launch.
    export PATH="$TARGET_DIR/$TARGET_SUB${PATH:+:$PATH}"

    # --- run -----------------------------------------------------------------

    if [[ -f "$PID_FILE" ]]; then
        old_pid="$(cat "$PID_FILE")"
        if kill -0 "$old_pid" 2>/dev/null; then
            log "Metis already running (PID $old_pid). Stop it first: ./run-metis.sh --stop"
            exit 1
        fi
        rm -f "$PID_FILE"
    fi

    run_section "Run"
    install_metis_settings_user_data
    log "Controls: Metis edge bar · Mod+F maximize · Mod+Q close (see METIS_MOD)"
    log "Stop shell: ./run-metis.sh --stop"
    echo

    if [[ "$SESSION" -eq 1 ]]; then
        export METIS_SHELL_BIN="$SHELL_BIN"

        # Opt-in: redirect the user D-Bus/systemd activation environment at the
        # nested compositor so D-Bus-activated and single-instance apps open
        # inside Metis. The compositor performs (and reverts) the import once it
        # knows its auto-assigned socket name. Heads-up: while active this
        # temporarily points the logged-in user's activation env at Metis.
        if [[ "$IMPORT_ENV" -eq 1 ]]; then
            export METIS_IMPORT_ACTIVATION_ENV=1
            log "Activation-env import ENABLED — D-Bus/systemd apps will target the nested session."
        fi

        SESSION_DIR="${XDG_RUNTIME_DIR:-/tmp}/metis"
        # Avoid `session.lock` — unrelated apps (e.g. Claude Desktop's VM service)
        # have been observed to open that path and block flock even when Metis
        # is not running.
        LOCK_FILE="$SESSION_DIR/compositor-session.flock"
        LAST_EXIT_FILE="$SESSION_DIR/session.last-exit"
        mkdir -p "$SESSION_DIR" 2>/dev/null || true

        # Single-instance lock. A stray relaunch that OVERLAPS a live session
        # can't stack a second nested compositor — it hits this guard and bails.
        exec 9>"$LOCK_FILE"
        if ! flock -n 9; then
            holder="$(cat "$LOCK_FILE" 2>/dev/null)"
            lock_note=""
            if ! pgrep -x metis-compositor >/dev/null 2>&1; then
                lock_holder="$(fuser "$LOCK_FILE" 2>/dev/null | tr -s ' ' || true)"
                if [[ -n "$lock_holder" ]]; then
                    lock_note=" (compositor not running; lock held by PID(s): ${lock_holder})"
                else
                    lock_note=" (compositor not running; stale lock)"
                fi
            fi
            log "ERROR: a Metis session is already running (lock held${holder:+ by PID $holder})${lock_note}."
            log "       Refusing to start a second session. Stop the first: ./run-metis.sh --stop"
            if [[ -n "$lock_note" ]]; then
                log "       If Metis is not actually open, close the app holding the lock or restart it."
            fi
            log "       If you didn't start this, see launch audit: $AUDIT_LOG"
            exit 1
        fi
        printf '%s\n' "$BASHPID" >&9

        # Rapid-relaunch cooldown. Stops an instant auto-reopen after you close
        # the window (a session exiting and immediately respawning). A genuine
        # quick restart can override with METIS_FORCE=1.
        COOLDOWN="${METIS_SESSION_COOLDOWN:-4}"
        if [[ -z "${METIS_FORCE:-}" && -f "$LAST_EXIT_FILE" ]]; then
            now="$(date +%s)"
            last="$(cat "$LAST_EXIT_FILE" 2>/dev/null || echo 0)"
            delta=$(( now - last ))
            if (( delta >= 0 && delta < COOLDOWN )); then
                log "ERROR: a Metis session exited ${delta}s ago — refusing rapid auto-relaunch (cooldown ${COOLDOWN}s)."
                log "       This breaks an automatic close→reopen loop. To restart on purpose:"
                log "       METIS_FORCE=1 ./run-metis.sh --session   (or wait ${COOLDOWN}s)"
                log "       Who launched this run is recorded in: $AUDIT_LOG"
                exit 1
            fi
        fi

        # Standalone DRM/TTY session owns the seat outright, so (like the
        # installed metis-session) it must provide the Secret Service itself.
        # A nested dev session is skipped — the host desktop's keyring already
        # serves org.freedesktop.secrets on the shared user bus.
        if [[ "$DRM" -eq 1 ]] || ! nested_host_session; then
            metis_start_secret_service
        fi

        log "Starting Metis compositor session (spawns shell automatically) …"
        if [[ ${#COMP_ARGS[@]} -gt 0 ]]; then
            log "Compositor args: ${COMP_ARGS[*]}"
        fi
        # Run as a child (not exec) so we can stamp the exit time for the cooldown
        # guard above. The lock on FD 9 is held by this subshell for the session's
        # lifetime and released when it returns.
        "$COMP_BIN" "${COMP_ARGS[@]}"
        session_rc=$?
        date +%s >"$LAST_EXIT_FILE" 2>/dev/null || true
        if [[ "$session_rc" -ne 0 ]] && [[ "${METIS_BACKEND:-}" == "drm" ]]; then
            log "ERROR: compositor exited with status $session_rc (see log above)."
            log "       DRM busy? From another VT/SSH: pkill metis-compositor"
            log "       Daily driver: ./run-metis.sh --install-session → pick Metis at greeter."
        fi
        exit "$session_rc"
    fi

    if [[ "$FOREGROUND" -eq 1 ]]; then
        log "Foreground shell — Ctrl+C stops Metis shell."
        exec "$SHELL_BIN"
    fi

    log "Starting metis-shell in background …"
    nohup "$SHELL_BIN" >>"$LOG_FILE" 2>&1 &
    metis_pid=$!
    echo "$metis_pid" >"$PID_FILE"
    disown "$metis_pid" 2>/dev/null || true
    log "Metis running (PID $metis_pid). This terminal is free — open apps as usual."
    log "Controls: search pill at top · Super+Space command bar · Super+D desk grid"
    sleep 2
    if kill -0 "$metis_pid" 2>/dev/null; then
        if grep -q "panicked at" "$LOG_FILE" 2>/dev/null; then
            log "ERROR: Metis crashed on startup — see $LOG_FILE"
            grep "panicked at" "$LOG_FILE" | tail -1 || true
            rm -f "$PID_FILE"
            exit 1
        fi
        log "Post-start check: Metis still alive (PID $metis_pid)."
    else
        log "ERROR: Metis exited immediately — see $LOG_FILE"
        grep -E "panicked at|ERROR|error" "$LOG_FILE" 2>/dev/null | tail -5 || true
        rm -f "$PID_FILE"
        exit 1
    fi
    log "Verify keybinds anytime: ./run-metis.sh --verify"
    log "Verify grid sync:       ./run-metis.sh --verify-grid"
} 2>&1 | tee -a "$LOG_FILE"
