# Development setup — Metis

Primary target: **Ubuntu 24.04+**. For a one-shot bootstrap on Ubuntu 24.04 / 26.04,
Debian 13, or Arch, prefer the repo-root installer:

```bash
./install.sh --yes
# or deps only:
./install.sh --deps-only
```

## System packages (Ubuntu)

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config libssl-dev libclang-dev \
  libgtk-4-dev libadwaita-1-dev \
  libpulse-dev \
  curl git
# Ubuntu 26.04 / Debian 13:
sudo apt install -y libgtk4-layer-shell-dev
```

On **Ubuntu 24.04**, there is no `libgtk4-layer-shell-dev` — build
[gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell) from source (or let
`./install.sh` do it) and set `PKG_CONFIG_PATH`.
To build/run the **standalone DRM session** (Metis on its own TTY/GPU, not nested),
also install the session, input, and GPU libraries:

```bash
sudo apt install -y \
  libudev-dev libinput-dev libseat-dev \
  libgbm-dev libdrm-dev libegl1-mesa-dev libgles2-mesa-dev \
  libdisplay-info-dev libpam0g-dev \
  libpipewire-0.3-dev \
  liblcms2-dev
```

`liblcms2-dev` is required to build Stage 2 colour (ICC → GLES 3D-LUT).
If `libgtk4-layer-shell-dev` is unavailable on your release (e.g. Ubuntu 24.04), build [gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell) from source and set `PKG_CONFIG_PATH` accordingly.

### Lock screen biometrics (optional)

Password unlock works with stock `/etc/pam.d/metis`. For fingerprint or YubiKey
(FIDO2/U2F touch via `pam_u2f`):

```bash
sudo apt install -y fprintd libpam-fprintd libpam-u2f pamu2fcfg
# enroll fingerprint: fprintd-enroll
# enroll YubiKey: mkdir -p ~/.config/Yubico && pamu2fcfg >> ~/.config/Yubico/u2f_keys
# then uncomment the auth sufficient lines in /etc/pam.d/metis (see asset comments)
```

### Keyring (Secret Service) — runtime dependency

Metis is only a *client* of the freedesktop Secret Service (`org.freedesktop.secrets`, via `oo7`), and so are apps like Cursor, GitHub Desktop, and browsers. A Metis session must therefore have a **provider** running, or those apps fall back to plaintext credential storage ("encryption is low"). The session launcher (`metis-session` / `run-metis.sh --session --drm`) auto-detects and starts whichever of these is installed — install **one** (any desktop works; `gnome-keyring` is not GNOME-specific and is the lightest):

```bash
sudo apt install -y gnome-keyring   # recommended, desktop-independent
# alternatives that also implement the Secret Service API:
#   kwalletd6 / kwalletd5 (KWallet) · keepassxc · pass + pass-secret-service
```

Without PAM auto-unlock (`pam_gnome_keyring`), the login keyring starts locked and the first secret access prompts once per session via gcr's prompter (pulled in by `gnome-keyring`).

**What Metis stores there:** CalDAV passwords and Microsoft 365 refresh tokens
(`metis-secrets`). Account lists in `~/.config/metis/calendars.json` hold no
secrets; removing an account deletes the matching keyring items.

### Phase 4 runtime tools (standalone session)

Several settings pages shell out to system services (same pattern as `nmcli` for
Network). Install what you need on the host:

```bash
sudo apt install -y \
  bluez bluetooth \
  cups system-config-printer \
  power-profiles-daemon
```

PipeWire/PulseAudio (`pipewire-pulse` / `pulseaudio-utils` for `pactl`) is usually
already present on Ubuntu desktop installs.

### Remote desktop (Phase 7)

Desktop sharing uses **gnome-remote-desktop** in **headless** mode (not the GNOME
Shell interactive sharing daemon). Metis ships `metis-remote` to start/stop RDP
and report JSON status for Settings.

```bash
sudo apt install -y gnome-remote-desktop
```

**Systemd user unit:** `gnome-remote-desktop-headless.service`  
**CLI:** `grdctl --headless` (status, `rdp enable`, `rdp set-credentials`, …)  
**Default port:** 3389

Manual spike on a DRM Metis session (RicePudding or `./run-metis.sh --session --drm`):

```bash
systemctl --user enable --now gnome-remote-desktop-headless.service
grdctl --headless rdp set-credentials "$USER" 'your-strong-password'
grdctl --headless rdp enable
grdctl --headless status
# From another machine on LAN:
xfreerdp /v:$(hostname -I | awk '{print $1}'):3389 /u:$USER /p:'your-strong-password' /dynamic-resolution
```

Metis integration: `~/.config/metis/remote.json` + `metis-remote
{status|enable|disable|autostart|set-credentials|firewall|…}`; Settings page
**Remote access**; `metis-session` calls `metis-remote autostart` when enabled.
LAN-only firewall apply uses timed `pkexec` — install `nftables` (preferred) and
a PolicyKit agent in the session (`policykit-1-gnome` or `mate-polkit`) so the
admin password dialog can appear. See USER_GUIDE → Remote desktop.

**XWayland:** default one shared server (`xwayland_mode: shared`). Opt-in
`xwayland_mode: isolated` in `config.json` starts a second gaming bucket
(Steam/Proton `DISPLAY`); see USER_GUIDE isolation notes. Abstract socket stays
off by default (`xwayland_abstract_socket: false`).

#### Compatibility matrix

| Tool | Status | Capture / input | Notes |
|------|--------|-----------------|-------|
| **gnome-remote-desktop (RDP)** | **Supported (v1)** | Portal / PipeWire via `metis-portal`; EIS remote input | Settings toggle; text+image clipboard; LAN-only defaults; multi-monitor `RecordMonitor` selects by connector name |
| **Metis Viewer (`metis-viewer`)** | **Supported (v1 client)** | FreeRDP → GRD host | GTK connect UI; argv spawn of `wlfreerdp3`/`xfreerdp…`; recent hosts in `viewer.json` (no passwords) |
| **RustDesk** | **Settings preset (detect/open)** | Prefer portal/PipeWire on Wayland; own capture may fail | Settings → Remote access card; not in `metis-remote` — see below |
| **wayvnc** | **Spike / unsupported** | Needs compositor screencopy or portal consumer | No Metis integration; Smithay capture path TBD |
| **TigerVNC / x11vnc** | **Not applicable** | X11 | Metis is Wayland-first; do not expect these to attach to the DRM session |
| **xrdp** | **Out of toggle scope** | Separate X11 login session | Different problem (new login), not session sharing |
| **AnyDesk** | **Spot-check only** | Proprietary capture | May work if it uses portal; not tested in CI |
| **Chrome Remote Desktop** | **Spot-check only** | Proprietary / CRD host | Not integrated; often expects GNOME/Chrome host helpers |

Lock behaviour: compositor refuses capture while locked — remote view freezes until unlock (documented in USER_GUIDE).

#### RustDesk on a Metis host

RustDesk is a reasonable third-party option when you need ID/relay access instead of
LAN RDP. Metis ships a **Settings → Remote access** preset (detect install, open
app, copy install instructions). It does **not** orchestrate enable/disable,
credentials, or firewall ports (`metis-remote` backend remains `gnome_rdp` only).

**Install (Ubuntu):**

```bash
# Official .deb from https://github.com/rustdesk/rustdesk/releases (amd64)
sudo apt install -y ./rustdesk-*.deb
# or Flatpak if you prefer:
# flatpak install flathub com.rustdesk.RustDesk
```

**Wayland / capture notes:**

- On Metis DRM, prefer enabling RustDesk’s **PipeWire / portal** capture when the
  UI offers it so frames come from `metis-portal` ScreenCast (same path as RDP).
- If RustDesk falls back to “proprietary” or wlroots-only screencopy, expect a
  black or stalled picture — Metis is Smithay, not wlroots.
- Input: when portal capture is used, remote pointer/keyboard usually reach the
  compositor; verify both native Wayland clients and XWayland (Steam/Proton).

**Firewall / ports (defaults):**

| Port | Proto | Purpose |
|------|-------|---------|
| 21115 | TCP | Hole punching / ID server (direct) |
| 21116 | TCP/UDP | ID / relay coordination |
| 21117 | TCP | Relay |
| 21118–21119 | TCP | Web / optional |

For LAN-only testing, allow those ports from your subnet (example):

```bash
sudo ufw allow from 192.168.0.0/16 to any port 21115:21119 proto tcp
sudo ufw allow from 192.168.0.0/16 to any port 21116 proto udp
```

Do **not** expose RustDesk ID/relay ports to the public internet without
understanding relay trust and strong authentication.

**Verification checklist (DRM Metis session):**

1. Host is on Metis DRM (`./run-metis.sh --session --drm` or installed session), not nested winit.
2. Start RustDesk; note the ID; set a permanent password (or one-time).
3. From another machine, connect with RustDesk client.
4. Confirm: desktop visible, edge bar clickable, Settings opens, XWayland app
   (e.g. `xterm` / Steam) receives clicks and keys.
5. Lock with `Super+L` — picture should freeze/black until unlock (same capture
   block as RDP).
6. If black screen with portal errors, check `metis-portal` / PipeWire and that
   RustDesk requested ScreenCast permission.

#### wayvnc / VNC

`wayvnc` targets wlroots protocols. Metis does not implement those; a VNC path
would need either a Smithay screencopy exporter or a portal-consuming VNC server.
Treat VNC as **unsupported** until a dedicated spike lands (Phase 7 ScreenCast
backend follow-up; portal dmabuf zero-copy already landed for ScreenCast apps).

#### Multi-user / VT behaviour

- Metis runs **one graphical session per seat**. Logging into Metis on VT2 while
  another user owns VT1 is a separate session — remote RDP attaches to the
  **active** Metis session that started `gnome-remote-desktop-headless`, not to
  an arbitrary VT.
- Switching away from the Metis VT (Ctrl+Alt+F3, etc.) typically pauses DRM
  presentation; remote viewers may freeze until you switch back.
- **Multi-seat** (two users, two GPUs/seats) is unsupported.
- Nesting (`run-metis.sh` without `--drm`) is for development only — do not use
  it to validate remote desktop.

### Portal stack (standalone session)

Screenshot and ScreenCast apps talk to **xdg-desktop-portal**, which routes
capture requests to **metis-portal**. Install the portal front-end and GTK
helper on the host:

```bash
sudo apt install -y xdg-desktop-portal xdg-desktop-portal-gtk
```

`./run-metis.sh --install-session` installs `metis-portal` plus
`metis.portal` / `metis-portals.conf` under `/usr/share/xdg-desktop-portal/`.
The compositor starts `metis-portal` before `xdg-desktop-portal` on DRM boot.

To verify screenshot capture without Flameshot:

```bash
metis-portal --capture-test /tmp/test.png
```

### Flatpak (optional)

For sandboxed apps and games from Flathub:

```bash
sudo apt install -y flatpak xdg-desktop-portal xdg-desktop-portal-gtk
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
```

Flatpak apps use the same portal stack as native apps and show up in the Metis
launcher/dock automatically — `run-metis.sh --session` (and the installed
`metis-session`) add the Flatpak `exports/share` dirs to `XDG_DATA_DIRS`, which
is what makes GIO find their `.desktop` entries. Gamepads usually need a Flatpak
device override (`flatpak override --user --device=all <app-id>`) — see the
[User Guide](USER_GUIDE.md#flatpak-apps-and-games) and
[`TODO.md`](../metis-os-workspace/TODO.md) Phase 6.

### Steam / Proton (gaming)

For SteamOS-class desktop gaming on Metis:

```bash
sudo dpkg --add-architecture i386
sudo apt update
sudo apt install -y \
  steam-installer \
  mesa-vulkan-drivers mesa-vulkan-drivers:i386
# NVIDIA: also install 32-bit GL/Vulkan for your driver series, e.g.
# sudo apt install -y libnvidia-gl-XXX libnvidia-gl-XXX:i386
```

Optional: `gamescope` for per-game nested compositor (Steam launch options:
`gamescope -W 1920 -H 1080 -f -- %command%`).

Hybrid GPU laptops: see `METIS_DRM_DEVICE` and `METIS_GAME_GPU` in the
standalone session section below. To debug Proton pointer-lock / click-jump
issues (see CHANGELOG 2026-07-19), filter compositor logs with:

```bash
rg 'game-pointer' ~/.local/state/metis/logs/session-latest.log
```

Look for `is_locked=true` on fire clicks and the absence of `click remapped`
lines after the 2026-07-19 fix.
Full gaming checklist: [User Guide — Steam & Proton](USER_GUIDE.md#steam-proton--steamos-class-gaming).

**Gaming Platform 2.0 (Phase 11):** `metis-gamingd` starts automatically with the
Metis session (`run-metis.sh` / `metis-session`). Settings → Gaming writes
`gaming.json`; reload with `metis-cmd reload-gaming`, optimize Flatpak overrides
with `metis-cmd optimize-gaming --yes` (requires explicit consent; Settings →
Gaming shows a permission dialog). Optional: `gamemode` package for per-game CPU
scheduler tweaks (`gamemoderun %command%` in Steam launch options).

### IPC trust model

Compositor control sockets live under `$XDG_RUNTIME_DIR/metis/` (`0700` dir,
`0600` sockets and command files). Accepts require `SO_PEERCRED` so the peer UID
matches the compositor's euid — **cross-UID** clients are dropped. Same-UID
malware still has full DE control by design of the session control plane.
`XDG_RUNTIME_DIR` must be set; Metis refuses the old world-readable `/tmp/metis`
fallback.

Command files (`command`, `command-widgets`) are allowlisted by verb (512-byte
cap) at write and read time — see `metis_protocol::parse_runtime_command`. That
limits drive-by pokes; it is not a security boundary against same-UID attackers
(use the socket + `METIS_IPC_TOKEN` widgets channel for capability separation).

Phase 18 B adds sliding-window rate limits on command IPC, event subscribe, and
command-file write/dispatch (see User Guide — Session IPC trust model). Widgets
tokens remain spawn-scoped (cleared on widgets exit), not wall-clock TTL.

### Release build profiles

Default **`release`** uses thin LTO, single codegen unit, and strips symbols —
smaller than stock Cargo release with minimal compositor perf impact. For the
smallest install footprint:

```bash
./run-metis.sh --build --release-small
./run-metis.sh --install-session --release-small
```

The compositor stays at `opt-level=3` in `release-small`; GTK/shell binaries use
size optimization. Details: [`PERF_AUDIT.md`](PERF_AUDIT.md).

## Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## Build & run

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --build --session
```

To exercise multi-monitor behaviour in the nested session, split the window into
N side-by-side virtual outputs:

```bash
METIS_VIRTUAL_OUTPUTS=2 ./run-metis.sh --session
```

For day-to-day usage (keybinds, workspaces, scrolling layout, settings), see the
[User Guide](USER_GUIDE.md).

## Standalone session (run on a real TTY/GPU)

Metis autodetects its backend: with `WAYLAND_DISPLAY`/`DISPLAY` set it nests
(winit), otherwise it drives DRM/KMS directly. Force it with
`METIS_BACKEND=winit|drm`.

### Option A — log in from your display manager (recommended, Hyprland-style)

Install the session entry, then pick **Metis** from the GDM/SDDM/greetd session
menu, exactly like selecting Hyprland:

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --install-session    # builds release; prompts for sudo
```

This installs:

- `/usr/local/bin/{metis-compositor,metis-shell,metis-settings,metis-portal}`
- `/usr/local/bin/metis-session` — the session launcher (sets
  `XDG_CURRENT_DESKTOP=Metis`, `METIS_BACKEND=drm`, exports the activation
  environment, then execs the compositor)
- `/usr/local/share/wayland-sessions/metis.desktop` — the greeter entry
- `/usr/local/share/applications/metis-settings.desktop` — Settings app launcher
  (`Icon=metis-settings`, transparent PNG installed to hicolor)
- `/usr/share/xdg-desktop-portal/{metis-portals.conf,portals/metis.portal}` —
  routes Settings, Screenshot, ScreenCast, Background, and PowerProfileMonitor
  to the Metis portal backend

Log out and choose **Metis** at the login screen. The display manager hands the
session its own VT + seat, so libseat takes DRM master cleanly and exiting drops
back to the greeter. **Keep an SSH session open the first few times** in case the
greeter does not return.

### Option B — from a bare TTY (quick test)

Switch to a free VT (`Ctrl+Alt+F3`), log in, then:

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --session --drm
```

### Escape hatches (DRM session only)

- **Ctrl+Alt+Backspace** — quit Metis (returns to the greeter / shell)
- **Ctrl+Alt+F<n>** — switch virtual terminal

`METIS_DRM_DEVICE=/dev/dri/cardN` overrides primary-GPU autodetection. The DRM
session paints its own (XCursor-themed) pointer; set `XCURSOR_THEME` /
`XCURSOR_SIZE` to change it.

**Client GPU steering (hybrid laptops).** On the DRM backend, the compositor
resolves the PCI identity of the render node it actually draws on and forwards it
to every spawned client as `DRI_PRIME` (Mesa GL) and `MESA_VK_DEVICE_SELECT`
(Mesa Vulkan), so Steam/Proton/XWayland default to the *same* GPU the session
uses rather than silently picking the wrong card. It only sets vars that are
unset, so per-game Steam launch options (`DRI_PRIME=1 %command%`, `prime-run`,
NVIDIA offload) still win. Set `METIS_NO_CLIENT_GPU=1` to disable forwarding, or
`METIS_DRM_DEVICE` to change which card the whole session (and thus clients) use.
When a discrete GPU is present but the panel is driven by the integrated GPU,
Metis also auto-offloads **game and Steam launches** onto the dGPU
(`DgpuOffload::detect`). Override with `METIS_GAME_GPU=igpu|dgpu|off`. This is
inert under the nested winit backend, where the host compositor owns device
selection.

On first run, Metis writes defaults to `~/.config/metis/`:

- `bar.json` — edge bar layout and widgets
- `clock.json` — world clocks and alarms
- `calendars.json` — calendar accounts
- `themes/dark.json`, `themes/light.json` — design tokens

Created later, on demand:

- `config.json` — active theme, onboarding state, briefing-on-login (written when you change a preference)
- `menu.json` — app launcher terminal / file-manager defaults and pinned apps
- `wallpaper.json` — background picture / colour / gradient (and per-output overrides).
  Settings → Background also offers system images from `/usr/share/backgrounds`
  (paginated; not copied into the Metis store unless imported).
- `weather.json` — bar weather unit, auto-detect / IP-geolocation, saved locations
- `dismissed.json` — dismissed calendar reminders
- `desk.json` — compositor window-grid layout (written by the compositor, same directory)
- `briefing.json` — weather coordinates and RSS feed URL (optional; create it yourself)

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Compositor shortcuts don't work (nested in GNOME) | GNOME grabs **Super** globally. Nested sessions default to **`METIS_MOD=alt`** — use **Alt+1**…**Alt+9**, **Alt+Shift+←/→**, etc. Click the Metis window first so it has keyboard focus. To force Super: `METIS_MOD=super ./run-metis.sh --session` after disabling conflicting GNOME shortcuts (Settings → Keyboard → Keyboard Shortcuts). |
| Layer surfaces invisible | Confirm Wayland session + `echo $WAYLAND_DISPLAY` |
| Missing layer-shell | Install `libgtk4-layer-shell-dev` (26.04 / Debian 13), or build from source on 24.04 |
| Shell hangs on startup | Rebuild compositor + shell (`./run-metis.sh --build --session`) |
| Theme not applied | Delete `~/.config/metis/themes/*.json` and restart to regenerate |
| Settings scroll freezes on Windows/Display (26.04 / hybrid GPU) | Rebuild/reinstall `metis-settings` (2026-09-19 Cairo GSK default). Test GPU GSK with `METIS_SETTINGS_GSK_RENDERER=gl` |
| Settings closes but process stays (`pgrep metis-settings`) | Rebuild/reinstall `metis-settings` (2026-09-19 quit-on-close). Sheet **X** returns to Home; window close exits the app |
| Gtk theme parser warnings on Settings open | Expected noise from the shared shell stylesheet (unsupported CSS props / `color-mix`). Harmless — ignore unless the UI looks wrong |
| Bottom edge bar freezes with maximized Settings open | Rebuild/reinstall compositor (2026-09-19 maximized reclamp geometry-only fix) |
| DRM session: black screen / no input | Run from a VT you own (or via the display-manager entry) so libseat can take DRM master; check the log and SSH in to `Ctrl+Alt+Backspace` is unavailable — `pkill metis-compositor`. |
| DRM session: "no GPU found for seat" | Ensure you are in the `video`/`render`/`input` groups and `seatd`/logind is running; try `METIS_DRM_DEVICE=/dev/dri/card0`. |
| Screenshot / Flameshot fails | `./run-metis.sh --install-session`, log out and back in, then `metis-portal --capture-test /tmp/test.png`; install `xdg-desktop-portal` + `xdg-desktop-portal-gtk` if missing |
