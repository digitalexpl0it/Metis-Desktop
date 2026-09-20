# Metis

> **Beta** — Metis is under active development. Expect rough edges: session setup,
> window management, and configuration formats may change between releases. Bug
> reports and feedback are welcome.

> **Metis** is a next-generation Wayland desktop environment built in Rust. The
> **Metis compositor** owns the Wayland session, the window grid, and the
> wallpaper; it spawns the **Metis shell** (GTK4 layer-shell edge bar plus
> on-demand popovers, Notification Center, and Control Center) and, when enabled,
> a separate **desktop-widgets** process so a hung widget cannot freeze the bar.

New to Metis? Start with the **[User Guide](docs/USER_GUIDE.md)**.

**Security / trust model** (IPC peer credentials, XWayland isolation, gaming
hooks, colour-management opt-in): **[SECURITY.md](SECURITY.md)**.

## Screenshots

**Desktop** — edge bar, workspaces, weather, and server-side window decorations on a
theme-aware wallpaper.

![Metis desktop with edge bar, workspaces, and tiled windows](Screenshots/metis_desktop.png)

**Control Center** — pull-down system monitor with live charts and a searchable process
list (Settings → Control Center to configure).

![Metis Control Center with CPU, memory, and process list](Screenshots/metis_control_center.png)

**Settings** — Home overview + mini sidebar (UI 2.0) for display, appearance,
connectivity, input, gaming, and system configuration. Closing the window fully
quits the app.

![Metis Settings control center](Screenshots/metis_settings.png)

## Philosophy

- **Performance first** — idiomatic, low-overhead Rust with `tokio` async and damage-driven rendering.
- **Compositor-first** — a Smithay compositor owns the session; the shell is spawned by it.
- **On-demand shell** — `wlr-layer-shell` overlays (edge bar, launcher, popovers,
  Notification Center, Control Center, optional desktop widgets) summoned when
  needed and torn down cleanly.

## Workspace layout

```
.
├── install.sh                   # One-shot deps + release install → /usr/local + greeter
├── flake.nix                    # Nix flake entry (see nix/)
├── CHANGELOG.md / SECURITY.md / LICENSE
├── Screenshots/                 # README showcase images
├── docs/                        # User guide, packaging, i18n, perf, widget schema
│   ├── USER_GUIDE.md
│   ├── UBUNTU_DEV.md
│   ├── PACKAGING.md
│   ├── I18N.md
│   ├── PERF_AUDIT.md
│   ├── WIDGET_PACK_SCHEMA.md
│   ├── decisions/               # Architecture decision records
│   └── upstream/                # Upstream blockers (e.g. wayland-rs UAF)
├── nix/                         # NixOS module + packaging notes
├── .github/                     # CI workflows + issue templates
└── metis-os-workspace/          # Cargo workspace (all crates live here)
    ├── Cargo.toml               # Workspace root
    ├── TODO.md                  # Detailed roadmap (phases + checklists)
    ├── assets/                  # Wallpapers, portal registration, session launcher
    ├── packaging/               # .deb / Arch / polkit policy helpers
    ├── scripts/                 # package-deb.sh + packaging / smoke helpers
    ├── metis-capture/           # Shared Wayland ext-image-copy-capture client
    ├── metis-compositor/        # Smithay Wayland compositor (winit + DRM backends)
    ├── metis-config/            # Shared config + theme tokens (serde, no GTK)
    ├── metis-gaming/            # Flatpak optimizer, health checks, metis-gamingd
    ├── metis-grid/              # Window grid / tiling + scrolling layout (pure logic)
    ├── metis-i18n/              # gettext (shell/settings) + Fluent (compositor)
    ├── metis-portal/            # xdg-desktop-portal backend
    ├── metis-protocol/          # Shared JSON IPC contracts + rate limits
    ├── metis-remote/            # Desktop sharing + Polkit privileged helpers
    ├── metis-screenshot/        # Native screenshot / recording helpers
    ├── metis-secrets/           # Freedesktop Secret Service (oo7) wrapper
    ├── metis-settings/          # GTK4 settings app
    ├── metis-shell/             # GTK4 layer-shell bar, panels, Task View, widgets host
    └── metis-viewer/            # Remote desktop viewer client
```

## Technology stack

- **Language:** Rust (stable), `tokio` async, `serde`/`serde_json` for JSON contracts.
- **Compositor:** [Smithay](https://github.com/Smithay/smithay) with a `winit` nested backend for development; DRM/KMS session backend; `calloop` event loop; `image` for wallpaper decode; XWayland for X11 apps.
- **Shell / UI:** GTK4 with [`gtk4-layer-shell`](https://github.com/wmww/gtk4-layer-shell); `zbus` for the freedesktop notification daemon.
- **IPC:** JSON over Unix sockets (`metis-protocol`) plus a runtime command file under `$XDG_RUNTIME_DIR/metis/`.
- **Configuration:** JSON under `~/.config/metis/`.

## Quick start

### Install from a `.deb` (Ubuntu / Debian)

Download the matching `metis-desktop_*_amd64.<suite>.deb` from
[GitHub Releases](https://github.com/digitalexpl0it/Metis/releases):

| Suite in filename | OS                 |
| ----------------- | ------------------ |
| `ubuntu24.04`     | Ubuntu 24.04       |
| `ubuntu26.04`     | Ubuntu 26.04       |
| `debian13`        | Debian 13 (trixie) |

```bash
sudo apt install ./metis-desktop_VERSION-1_amd64.ubuntu24.04.deb
```

See [`docs/PACKAGING.md`](docs/PACKAGING.md). Log out and pick **Metis** at the greeter.

**Note:** The package is named `metis-desktop`, not `metis` (avoids colliding with
Ubuntu’s unrelated math package).

### From source (`./install.sh`)

Ubuntu 24.04 / 26.04, Debian 13, or Arch:

```bash
git clone https://github.com/digitalexpl0it/Metis.git
cd Metis
./install.sh --yes          # deps + release build → /usr/local + greeter session
```

### Arch / NixOS

- Arch: `cd metis-os-workspace/packaging/arch && makepkg -si`
- NixOS: flake + `programs.metis` — see [`nix/README.md`](nix/README.md)

### Build from source (dev)

See [`docs/UBUNTU_DEV.md`](docs/UBUNTU_DEV.md), or `./install.sh --deps-only`.

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --build --session
```

The nested session runs inside your existing Wayland session via the winit
backend. Session mode disables the wallpaper and login briefing by default;
re-enable them with:

```bash
METIS_NO_WALLPAPER= METIS_NO_BRIEFING= ./run-metis.sh --session
```

To simulate multiple monitors in the dev session, split the window into N
side-by-side virtual outputs:

```bash
METIS_VIRTUAL_OUTPUTS=2 ./run-metis.sh --session
```

### Standalone session (real TTY/GPU)

Metis also runs as a real desktop session on its own GPU via a DRM/KMS + libseat
+ libinput backend (autodetected when no parent Wayland/X11 session is present).
Install the login entry and pick **Metis** from your display manager, just like
Hyprland:

```bash
./run-metis.sh --install-session   # build release + install the session entry
```

Or test it directly from a free VT with `./run-metis.sh --session --drm`. See
[`docs/UBUNTU_DEV.md`](docs/UBUNTU_DEV.md) for details and escape hatches
(Ctrl+Alt+Backspace to quit, Ctrl+Alt+F to switch VT).

## Using Metis

Full walkthrough in the **[User Guide](docs/USER_GUIDE.md)**. The essentials:

- **Edge bar** — app launcher, taskbar dock, workspaces, weather, battery,
  Bluetooth (when an adapter is present), network, volume, system tray, removable
  volumes (USB / SD / optical / ISO — open, mount/unlock, eject), and clock
  (opens Notification Center). Right-click dock icons to pin/close.
- **Desktop widgets** *(optional)* — free-floating wallpaper panels (Folders,
  Apps, Clock, System, Weather, Equalizer, plus JSON extension packs) in a
  dedicated `metis-shell --desktop-widgets` process. Off by default; enable in
  Settings → Desktop widgets. Edit mode to move/resize; configure via the gear
  on each instance. Writes `desktop-widgets.json` (live reload). Install packs
  under `~/.local/share/metis/widgets/<id>/`.
- **Control Center** — pull the edge bar toward the desktop (or click the grid
  icon beside the workspace dots) for a system monitor: CPU/memory/network/disk
  charts, temperature gauges, and a searchable process list with right-click
  actions. Configure in Settings → Control Center.
- **Windows** — every app gets a server-side titlebar with close / minimize /
  maximize. Drag the titlebar to move; drag to a screen edge to snap
  (half / quarter / maximize); drag a border to resize. On the default desktop
  layout, windows reopen at the position and size you last left them. Settings →
  Display includes a **Graphics profile** (Auto / Compatibility / Normal) for
  VM-safe GTK rendering.
- **Workspaces** — `Super`+`1`..`9` switch, `Super`+`Shift`+`1`..`9` move the
  focused window, `Super`+`Alt`+`←`/`→` cycle workspaces (wraps). Each monitor
  has its own workspaces (configurable).
- **Task View** — `Super`+`Tab` opens a sticky Windows-style overlay: live app
  cards for the current workspace plus a bottom shelf of desktop thumbnails.
  Click an app to focus; press-and-drag a card onto a desktop to move it; click
  × on a card to close; Esc or empty backdrop dismisses. `Super`+`Shift`+`Tab`
  cycles backward. `Alt`+`Tab` is unbound by default (remap Task View in
  Settings → Keyboard if you want it).
- **Cross-output moves** — drag a window onto another monitor (or snap it there)
  and it follows that display's desk; on grid workspaces `Super`+`Shift`+`←`/`→`
  sends the focused window to the adjacent monitor.
- **Scrolling layout** — toggle any workspace into a niri/PaperWM-style scrolling
  strip with `Super`+`\`; navigate with `Super`+arrows.
- **Settings** — launch from the app launcher, or `metis-cmd settings`. **UI 2.0:**
  Home overview + mini sidebar; category pages slide in; top-drop sheets for
  pickers and confirmations. Closing the window fully quits the process. Pages
  include Display, Appearance, Background, Edge bar, Windows, **Desktop widgets**,
  Metis Menu, Weather, Network (incl. **DNS** / VPN), Calendars, Input,
  **Shortcuts** (read-only guide; edit under Keyboard), Bluetooth, Printers,
  Power, Sound, **Gaming**, **Control Center**, and **Remote access**.
- **Gaming** — Settings → Gaming: graphics mode, health → Fix, guided **Run gaming
  setup** wizard (Steam / Vulkan / controllers / GameMode / NVIDIA consent),
  Flatpak optimize, Steam library path picker, Metis-owned MangoHud / Gamescope
  toggles (spawn-time only — no Steam VDF writes). Soft isolated X11 for gaming
  launches. See the [User Guide — Steam & Proton](docs/USER_GUIDE.md#steam-proton--steamos-class-gaming).
- **Screenshots** — **PrtSc** opens a native Metis overlay (Selection / Full screen /
  Window); **Shift+PrtSc** captures the full screen instantly; **Ctrl+PrtSc** starts in
  Window mode. **Esc** dismisses without capturing. Third-party apps (Flameshot, etc.)
  still use the xdg-desktop-portal Screenshot interface via `metis-portal`.
- **Notification Center** — click the clock for a right-side panel (notifications,
  calendar events, world clocks / timer / alarms). Toasts appear top-right with a
  close button.

| Shortcut                         | Action                                                       |
| -------------------------------- | ------------------------------------------------------------ |
| `PrtSc`                          | Interactive screenshot overlay                               |
| `Shift`+`PrtSc`                  | Instant full-screen capture (no overlay)                     |
| `Ctrl`+`PrtSc`                   | Screenshot overlay starting in Window mode                   |
| `Esc`                            | (screenshot overlay / Task View) Dismiss                     |
| `Super`+`Tab`                    | Task View (open / cycle next); sticky until Esc / activate   |
| `Super`+`Shift`+`Tab`            | Task View previous                                           |
| `Super`+`1`..`9`                 | Switch workspace (on the monitor under the pointer)          |
| `Super`+`Shift`+`1`..`9`         | Move focused window to a workspace                           |
| `Super`+`Alt`+`←` / `→`          | Cycle to previous / next workspace (wraps at 1..=count)      |
| `Super`+`Shift`+`←` / `→`        | (grid) Move focused window to adjacent monitor               |
| `Super`+`Ctrl`+`Shift`+`←` / `→` | Move active workspace to adjacent monitor (independent mode) |
| `Super`+`F`                      | Toggle maximize for the focused window (below the edge bar)  |
| `Super`+`Q`                      | Close the focused window                                     |
| `Esc`                            | Exit fullscreen / immersive (focused window)                 |
| `Super`+`\`                      | Toggle the active workspace between grid and scrolling       |
| `Super`+arrows                   | (scrolling) Move focus across columns / within a stack       |
| `Super`+`Shift`+arrows           | (scrolling) Move the column / window                         |
| `Super`+`,` / `Super`+`.`        | (scrolling) Consume into / expel from a column               |
| `Super`+`-` / `Super`+`=`        | (scrolling) Cycle the focused column width                   |

## Configuration

Configuration lives in `~/.config/metis/`. On first run the shell writes these
defaults:

| File                                    | Purpose                                                                                                |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `bar.json`                              | Edge bar position/size/opacity/blur, widget order, workspaces, window/titlebar borders, default layout |
| `clock.json`                            | World clocks and alarms                                                                                |
| `calendars.json`                        | Calendar accounts (local / CalDAV / Thunderbird / Microsoft 365)                                       |
| `themes/dark.json`, `themes/light.json` | Design tokens — accents, semantic status colors, `text_on_accent`, shadows/glows                       |

Other files are created on demand:

| File                   | Created when                       | Purpose                                                                                           |
| ---------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------- |
| `config.json`          | You change a preference            | Active theme (defaults to dark), graphics profile, onboarding state, briefing-on-login, XWayland mode |
| `menu.json`            | You set launcher defaults / pins   | App launcher: terminal + file-manager choices (kitty preferred on auto-detect), pinned apps       |
| `wallpaper.json`       | You pick a background              | Wallpaper picture / colour / gradient (+ per-output overrides). Settings → Background lists imports, Metis bundles, and system images under `/usr/share/backgrounds`. Changes crossfade live; decode cache in `~/.cache/metis/wallpaper-rgba/` |
| `weather.json`         | You configure weather              | Bar weather: unit, auto-detect / IP-geolocation, saved locations                                  |
| `desk.json`            | The compositor persists its layout | Compositor window-grid layout (app tiles)                                                         |
| `desktop-widgets.json` | You enable Desktop widgets         | Wallpaper widgets: enable, edit mode, chrome, builtins + JSON extension instances                 |
| `dismissed.json`       | You dismiss a calendar reminder    | Dismissed reminder IDs                                                                            |
| `briefing.json`        | You create it (optional)           | Login-briefing weather coordinates + RSS feed                                                     |
| `input.json`           | You configure input devices        | Mouse, touchpad, keyboard (compositor live-reload)                                                |
| `keybinds.json`        | You edit Shortcuts                 | Desktop chords → actions (Settings → Keyboard)                                                    |
| `power.json`           | You configure power settings       | Power profile (`powerprofilesctl`), idle blank/suspend, lid-close                                 |
| `remote.json`          | You configure Remote access        | Live-session RDP sharing via gnome-remote-desktop                                                 |
| `dashboard.json`       | You configure Control Center       | Enable, widget order, max height %, refresh interval, confirm-before-kill                         |
| `gaming.json`          | You configure gaming               | Graphics mode, auto performance/GameMode, Flatpak GPU env, library paths, Metis launch tweaks     |
| `gaming-flatpak.json`  | Gaming setup runs                  | Record of applied Flatpak gaming overrides                                                        |
| `game-rules.json`      | Optional override of defaults      | Float / fullscreen rules for Steam/Proton games (built-ins if absent)                             |
| `screenshot.json`      | You configure screenshots          | Default mode, pointer toggle, delay, after-capture, save dir                                      |
| `outputs.json`         | You configure displays             | Per-output scale, resolution/refresh, layout, `display_mode` / `mirror_source`, night-light prefs |

Edit `bar.json`, `themes/*.json`, or `desktop-widgets.json` while the shell runs —
changes apply live (widgets rebuild or update chrome in place). Set bar `opacity`
< 1 for a see-through bar and `blur: true` (with an optional `blur_radius`,
default 18) for a compositor Gaussian backdrop blur. See the
[User Guide](docs/USER_GUIDE.md#10-configuration-reference) for the full field
reference.

## Status

- **Phase 1 — Edge bar:** complete. App launcher, dock, workspaces, weather,
  tray, removable volumes, token-driven theming with live reload, transparency,
  and backdrop blur. Clock opens Notification Center (Phase 13).
- **Phase 2 — Settings app + window decorations:** complete. Standalone
  `metis-settings`, compositor SSD titlebars, edge snapping, XWayland support,
  Appearance light/dark sync for session GTK apps. (Taskbar / running-apps dock
  is Phase 2.5 in the roadmap — also complete.)
- **Phase 3 — Multi-monitor, workspaces & tiling:** complete. Per-output bars and
  desks; independent or linked workspaces; optional scrolling layout; ScreenCast
  dmabuf zero-copy; Stage G multi-GPU DRM (**hardware validation 2026-07-26** on
  hybrid iGPU+dGPU: HDMI projector, gaming PRIME, input); full `MultiRenderer`
  transfer remains deferred.
- **Phase 4 — System settings expansion:** complete (Input, Bluetooth, Printers,
  Power, Sound, Display).
- **Phase 5 — Display pipeline (mode-setting, HDR / VRR / colour):** **complete**
  (2026-07-25; HDR stretch 2026-07-26) — resolution / refresh, arrangement,
  duplicate mode, VRR, night light, Stage 1 ICC→CRTC gamma, Stage 2 GLES 3D-LUT,
  HDR H1–H3 with Rec.709→BT.2020 + PQ/HLG encode. Default-on
  `wp_color_management_v1` deferred (upstream wayland-rs **server/sys** ObjectData
  UAF; opt-in via `METIS_COLOR_MGMT=1`); true per-surface HDR content remains stretch.
- **Phase 6 — Flatpak, Steam & gaming (v1):** **complete** (2026-07-05).
- **Phase 7 — Remote access:** complete for GRD session sharing — Settings →
  Remote access, portal capture + EIS input, Metis Viewer client, RustDesk
  Settings preset, security closeout. Still deferred: Metis-native host protocol.
- **Phase 8 — Internationalization:** **complete** (2026-07-24). Hybrid gettext
  (shell/settings) + Fluent (compositor); Settings Language & region; onboarding
  language step; live Apply rebuilds. See [`docs/I18N.md`](docs/I18N.md).
- **Phase 9 — Onboarding:** **complete** (2026-07-04); language step with Phase 8.
- **Phase 10 — Control Center:** **complete** (2026-07-07; process tree + monitor
  picker 2026-07-11).
- **Phase 11 — Gaming Platform 2.0:** **complete** (2026-07-07).
- **Phase 12 — Native Screenshot Tool:** **complete** (2026-07-09).
- **Phase 13 — Notification Center:** **complete** (2026-07-10).
- **Phase 14 — Desktop Widgets:** **complete** (2026-07-18) — optional wallpaper
  panels (Folders, Apps, Clock, System, Weather, Equalizer) in a dedicated
  process; Settings list + configure dialogs; chrome and text style.
  **Extension API v1** (2026-07-26): JSON declarative packs under
  `…/metis/widgets/<id>/` (no Electron / scripts / `.so`).
- **Phase 15 — Session lock / remote / viewer closeout:** **complete** (2026-07-26/27).
- **Phase 16 — CI / packaging / security baseline:** **complete** (2026-08-02) —
  cargo-deny, PR quality gate, trust-boundary tests, command-file allowlist.
- **Phase 17 — Task View (Super+Tab):** **complete** (2026-08-06) — sticky
  Windows-style overlay with live app cards, workspace shelf, click-to-focus,
  press-and-drag to move, and per-card close.
- **Phase 18 — Security polish (IPC / isolation):** **A–D complete** (2026-08-08) —
  gaming path/env sanitization, IPC rate limits, widget pack schema gate, soft
  two-bucket XWayland isolation. **§E track-only** — default-on colour management
  (upstream UAF) + GLES MultiRenderer remain deferred.
- **Phase 19 — Gaming Setup UX:** **complete** (2026-08-08) — guided drivers /
  Steam / Vulkan / controllers wizard; NVIDIA consent + reboot banner; Settings
  library paths + Metis MangoHud/Gamescope toggles; X11 borderless game placement
  (Steam splash excluded from resize loops).

Optional follow-up (remaining): default-on colour-management protocol (upstream
wayland-rs ObjectData UAF — still opt-in `METIS_COLOR_MGMT=1`); fuller per-surface
HDR tone-map; dmabuf MultiRenderer without CPU readback; per-Steam-appid
Gamescope profile UI.

See [`metis-os-workspace/TODO.md`](metis-os-workspace/TODO.md) for the detailed
roadmap, [`CHANGELOG.md`](CHANGELOG.md) for recent changes,
[`SECURITY.md`](SECURITY.md) for the session trust model, and
[`docs/PERF_AUDIT.md`](docs/PERF_AUDIT.md) for performance and binary-size notes.

## License

Licensed under the [MIT License](LICENSE).
