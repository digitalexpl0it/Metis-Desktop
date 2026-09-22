# Metis User Guide

Welcome to **Metis** — a Wayland desktop environment built on a custom Smithay
compositor with a GTK4 layer-shell edge bar. This guide covers everyday use:
launching a session, the edge bar, managing windows, workspaces, **Task View**,
the scrolling layout, keyboard shortcuts, and the Settings app.

For installation and build prerequisites, see [`UBUNTU_DEV.md`](UBUNTU_DEV.md).

---

## 1. Launching Metis

You can run Metis as a **nested** session inside your current Wayland desktop
(via the winit backend — ideal for development), or as a **standalone** DRM
session from the greeter / a free VT (see also [`UBUNTU_DEV.md`](UBUNTU_DEV.md)
and `./run-metis.sh --install-session`).

### Nested (dev)

```bash
cd metis-os-workspace/metis-shell

./run-metis.sh --session            # start the compositor + shell
./run-metis.sh --build --session    # rebuild first, then start
./run-metis.sh --stop               # stop a running session
```

The compositor opens a window that *is* your Metis desktop. The shell (edge bar)
is spawned automatically.

**Wallpaper & briefing.** A nested dev session turns the wallpaper and login
briefing off by default. Turn them on for the run:

```bash
METIS_NO_WALLPAPER= METIS_NO_BRIEFING= ./run-metis.sh --session
```

**Multiple monitors (simulated).** Split the session window into N side-by-side
virtual outputs to test multi-monitor behaviour:

```bash
METIS_VIRTUAL_OUTPUTS=2 ./run-metis.sh --session
```

Each virtual output gets its own edge bar, wallpaper, and workspaces (subject to
your settings).

**First-run setup.** On a fresh install (when `onboarding_complete` is false in
`config.json`), the shell shows an onboarding wizard after the startup splash:
theme, wallpaper, clock format, edge bar, **network** (Wi-Fi / wired), weather,
**desktop widgets**, gaming, **optional software** (Remote desktop, Flatpak,
GameMode, Bluetooth, printers, and keyring if missing), and a finish screen with
keybind tips. Already-installed optionals are greyed out; use toggles and
**Install selected** to install the rest in one `pkexec apt-get` pass (or skip
and install later). Progress is saved as `onboarding_step` so a session restart
mid-wizard resumes where you left off. Skip or Finish marks setup complete so it
does not appear again. Reopen it anytime from **Settings → Appearance → Run setup
again**, or with `metis-cmd.sh show-onboarding`. Disable for dev with
`METIS_NO_ONBOARDING=1`.

**Install Metis.** Prefer the Ubuntu `.deb` from GitHub Releases when available
([`docs/PACKAGING.md`](PACKAGING.md)); developers can still use
`./run-metis.sh --install-session` for a `/usr/local` greeter entry.

---

## 2. The desktop at a glance

- **Edge bar** — a thin bar anchored to one screen edge (top by default). It
  holds the app launcher, a taskbar dock of running apps, workspace dots, and
  status widgets (weather, battery, Bluetooth, network, volume, clipboard, clock).
- **Desktop widgets** *(optional)* — free-floating panels over the wallpaper
  (Folders, Apps, Clock, System, Weather, Equalizer) in their own shell process.
  Off by default; turn on in **Settings → Desktop widgets**, then use **Edit
  mode** to move/resize.
- **Windows** — every app gets a compositor-drawn **titlebar** with close,
  minimize, and maximize buttons, plus a border. Windows tile into a grid by
  default — opening or closing an app re-splits the area below desk widgets among
  visible tiled windows. You can float, snap, maximize, or switch a workspace into a
  scrolling layout.
- **Popovers & panels** — clicking a bar widget opens an on-demand popover (Wi-Fi,
  volume, weather, app launcher). The **clock** opens the right-side **Notification
  Center** (notifications, calendar, world clocks, timer, alarms). Clicking
  elsewhere or pressing **Esc** dismisses it.
- **Task View** — `Super`+`Tab` opens a sticky overlay of current-workspace apps
  and desktop thumbnails (see §6). Click to focus, press-and-drag to another
  desktop, or Esc to dismiss.

---

## 3. The edge bar

Widgets appear in the order set by `bar.json#widgets`. The defaults:

| Widget | What it does |
|--------|--------------|
| **App launcher** | The brand icon at the start of the bar. Opens the launcher panel (see §4). |
| **Tasks (dock)** | Icons for running (and pinned) apps on this output's current workspace. Click to focus/minimize; right-click to pin/close. |
| **Workspaces** | One dot per workspace; the active one is highlighted. Click a dot to switch (see §6). |
| **Weather** | Condition icon + temperature. Click for a forecast popover with hourly strip and saved locations. |
| **Battery** | Charge level and state (hidden on desktops without a battery). Click to open Power settings. |
| **Bluetooth** | Shown when a Bluetooth adapter is present. Click for connected devices (with battery level and charging icon when reported), plus a shortcut to Bluetooth settings. |
| **Network** | Wired/Wi-Fi status. Click for a network popover (Wi-Fi scan/connect, Ethernet status). The signal icon stays stable during background rescans. |
| **VPN** | NetworkManager VPN / WireGuard. Click for connect/disconnect per profile (bar spinner while connecting; toast + notification on result). If a password is required, the popover (or Settings) prompts and can remember it on the profile. Tooltip shows active tunnel names. **VPN Settings…** opens Settings → Network → VPN. Profiles with **Auto-connect** are brought up after login once Wi‑Fi/Ethernet is ready (one profile at a time). |
| **Volume** | Current output volume. Click for a slider + mute. |
| **Notifications** | *(optional)* Legacy bell — opens the same Notification Center as the clock. Removed from the default bar layout in Phase 13. |
| **Clock** | Date/time with unread badge. Click opens the **Notification Center** (right panel): notifications, calendar events, and calendar/tools (world clocks, stopwatch, timer, alarms). **Esc** closes. |

**Per-output bars.** With multiple outputs you can show the bar on **all
displays** (each is independent and live) or **the primary display only** —
configured in Settings → Appearance → Edge bar → *Show bar on*.

**Live editing.** Edit `~/.config/metis/bar.json` while Metis runs; bar changes
apply within about a second. Theme edits (`themes/*.json`) re-apply live too.

### Notification Center

Click the **clock** to slide open a frosted panel from the right edge:

1. **Notifications** — grouped cards, Do Not Disturb, Clear all (hides when empty).
2. **Events** — selected-day calendar events (hides when empty).
3. **Calendar / tools** — month grid plus an icon rail for World clocks, Stopwatch,
   Timer, and Alarms.

Transient **toasts** still appear top-right (with a close button) and shift left
while the panel is open. Press **Esc** or click the clock again to dismiss.

---

## 4. Launching and managing apps

### App launcher

Click the brand icon (or the launcher widget) to open the launcher panel. It has:

- **Quick launchers + power actions** — a rail with your terminal, file manager,
  Settings, and power actions. The terminal and file manager are configurable in
  Settings → Metis Menu.
- **App list** — a Frequent/alphabetical list. Just start typing to search
  (no need to click the search box first).
- **Pinnable apps grid** — pin favourites for quick access.

Selecting an app launches it and dismisses the panel.

While an app is still starting (slow Electron/Flatpak cold starts), Metis
**suppresses duplicate launches** of the same desktop id so impatient clicks do
not open multiple windows. Pinned dock icons pulse while starting; after about a
second a short “Starting …” toast appears if the window has not mapped yet. Use
the dock’s **New window** action when you intentionally want another instance.

**Browsers feeling slow on first open after login** is often the portal stack
warming up (file dialogs / settings), not Wi‑Fi — Metis pre-starts
`xdg-desktop-portal` in the background; later navigations are unrelated to the
shell network widget.

### Taskbar dock

The dock shows apps running on the **current output and workspace**, grouped by
app identity. A dot marks running apps; the focused app is highlighted; minimized
apps are dimmed. A pinned icon pulses while that app’s launch is still pending.

- **Left-click** — focus the window (or minimize it if already focused). If an
  app has several windows, a picker popover appears. Pinned-but-not-running apps
  launch (duplicate clicks ignored until the first window appears).
- **Right-click** — pin/unpin the app, open **New window**, or close its window(s).
- The dock scrolls horizontally if it outgrows the bar.

### Screenshots

Metis ships a **native screenshot tool** (Phase 12) integrated into the shell.
Press **PrtSc** to open the interactive overlay (default **Selection** mode):

| Key | Action |
|-----|--------|
| **PrtSc** | Interactive overlay (Selection / Screen / Window) |
| **Shift+PrtSc** | Instant full-screen on the monitor under the pointer (no editor) |
| **Ctrl+PrtSc** | Interactive overlay starting in **Window** mode |
| **Esc** | Close the overlay without capturing (all modes) |

**PrtSc works over shell UI** — Metis Menu, Control Center, Notification Center,
and edge-bar widget popovers stay open when you open the picker (they are included
in the capture after the overlay hides). Screenshot shortcuts are not blocked by
those Exclusive keyboard layers.

**Multi-monitor** — picker and capture target the output under the pointer.
Toggle in Settings → Screenshot → *Capture monitor under pointer*.

**After capture** — interactive PrtSc defaults to **Edit**, opening
`metis-screenshot`. Shift+PrtSc never auto-opens the editor. Toolbar **Record**
starts a region recording as a player-compatible MP4 under `~/Videos/Metis`.
Metis uses H.264 when the installed FFmpeg supports it and falls back to MPEG-4.

### The screenshot editor

The editor's tool row holds **Pen**, **Highlighter**, **Arrow**, **Rectangle**,
**Ellipse**, and **Text**, then **Pixelate**, **Crop**, and **Extract text**
after the divider. Drag on the image to use the selected tool; the colour chip
beside the tools opens a palette and a stroke-size slider that both apply to the
next thing you draw. The image is scaled to fit the window and never upscaled,
so the pointer always lands on the pixel under it.

Pixelate and Crop rewrite the picture itself; every other tool stays an editable
overlay until you export. For text, drag out a dotted box and type directly on
the image. Drag the box to move it and drag its lower-right handle to resize it;
the text wraps live inside the new bounds. A blinking caret marks the current
typing position.

**Extract text** scans the full image immediately and opens a selectable results
view. Drag across any passage to highlight and copy it, use **Copy Selection**,
or choose **Copy All**. Official Metis packages and dependency installers include
Tesseract plus English language data automatically.

| Shortcut | Action |
| --- | --- |
| **Ctrl+Z** / **Ctrl+Shift+Z** | Undo / redo (covers crop and pixelate too) |
| **Ctrl+C** | Copy the annotated image to the clipboard |
| **Ctrl+S** | Overwrite the capture file |
| **Esc** | Close the editor |

**Copy**, **Save**, **Save As**, and **Pin** all export the same flattened image
the canvas shows.

The overlay uses your active Metis theme (dark/light/custom tokens): frosted
toolbar, accent **Capture** button, and dashed selection border all update live when
you change theme in Settings or edit `themes/*.json`.

**Modes** — **Selection**, **Full screen**, and **Window**. Press **Esc** at any
time to close without capturing.

**Options** (gear) and **Settings → System → Screenshot** share
`~/.config/metis/screenshot.json`. PNGs save under `~/Pictures/Metis` by default.

From a script: `metis-cmd screenshot` (same as PrtSc).

**Third-party apps** (Flameshot, browser pickers, etc.) still use the freedesktop
**Screenshot** portal (`org.freedesktop.impl.portal.Screenshot`):

- The **first** capture from an app may show a permission dialog; grant it once
  and later captures proceed silently.
- Portal screenshots are saved as PNGs under `$XDG_RUNTIME_DIR/metis-screenshot-*.png`
  and returned to the requesting app as a `file://` URI.

If screenshots fail after an upgrade, log out and back into Metis so the updated
compositor and portal binaries are running (`./run-metis.sh --install-session`
installs both). To verify portal capture directly:

```bash
metis-portal --capture-test /tmp/test.png
ls -la /tmp/test.png
```

### Flatpak apps and games

Flatpak apps run as ordinary Wayland clients in the same session and use the same
**xdg-desktop-portal** stack as native apps. Metis does not ship a Flatpak-specific
runner — installed Flatpaks launch like any other app.

**Host prerequisites** (Debian/Ubuntu shown; use your distro's packages otherwise):

```bash
# Flatpak + the portal stack Metis relies on for file dialogs, notifications,
# screenshots, and screencast.
sudo apt install flatpak xdg-desktop-portal xdg-desktop-portal-gtk
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo

# DRM / evdev access for the standalone session and for games that read input
# devices directly. Log out and back in after changing groups.
sudo usermod -aG input,video,render "$USER"
```

**Apps appear in the launcher automatically.** Flatpak installs export their
`.desktop` entries and icons under `exports/share` trees
(`~/.local/share/flatpak/exports/share` for `--user` installs,
`/var/lib/flatpak/exports/share` for system installs) rather than the normal
applications dir. Metis adds both to `XDG_DATA_DIRS` at session start
(`metis-session`, and `run-metis.sh --session` for dev), so Flatpak apps show up
in the app launcher and running-apps dock — with their proper names and icons —
right alongside native apps, and new installs appear live without a restart.

> If you installed a Metis login session before 2026-07-03, re-run
> `./run-metis.sh --install-session` and log out/in so the updated
> `metis-session` (with the Flatpak export dirs) takes effect.

**Permissions** come from three places:

1. **Flatpak manifest / overrides** — e.g. `socket=wayland`, `device=dri`, and
   often `--device=all` for gamepads (`flatpak override --user --device=all …`).
2. **Portal prompts** — screenshot/screencast/file access; stored by system
   `xdg-permission-store` (the first-time Flameshot dialog).
3. **Metis portal backends** — Settings, Screenshot, ScreenCast, Background, and
   PowerProfileMonitor via `metis-portal`; idle-inhibit via legacy ScreenSaver
   D-Bus names; file dialogs and notifications via the GTK portal backend.

#### Portal permission management

First-time portal prompts (screenshot, screencast, etc.) are stored by the system
**permission store**, not by Metis:

```bash
# List persisted portal permissions for an app
flatpak permission-show com.example.App

# Reset all portal permissions for an app
flatpak permission-reset com.example.App

# Show Flatpak sandbox overrides (devices, filesystems, sockets)
flatpak override --show com.example.App
flatpak info --show-permissions com.example.App
```

On-disk state lives under `~/.local/share/xdg-desktop-portal/` (portal runtime
data) and the system `xdg-permission-store` service. If a sandboxed app keeps
failing capture or file access after you denied a prompt once, reset its
permissions and try again.

#### Flatpak override cookbook

| Goal | Command |
|------|---------|
| Gamepads / all input devices | `flatpak override --user --device=all com.example.Game` |
| GPU / render node | `flatpak override --user --device=dri com.example.Game` |
| Extra game library on another disk | `flatpak override --user --filesystem=/mnt/games com.example.Game` |
| Wayland socket (usually in manifest) | `flatpak override --user --socket=wayland com.example.Game` |
| Steam (Flatpak) — typical gaming setup | `flatpak override --user --device=all com.valvesoftware.Steam` |

**Controllers:** games read `/dev/input/event*` directly (SDL, Proton), not through
the compositor. Metis opens libinput devices in **shared** mode and does **not**
EVIOCGRAB gamepad nodes — native and Proton titles keep full evdev access while
Metis runs. Touchscreens are forwarded to Wayland clients via `wl_touch`. Check
*Settings → Gaming* for a live list of detected gamepads and touchscreens. If a
Flatpak game has no gamepad, try:

```bash
flatpak override --user --device=all com.example.Game
```

Your user should also be in the `input`, `video`, and `render` groups for DRM
and evdev access.

### Steam, Proton & SteamOS-class gaming

Metis is intended to work as a **gaming desktop** with the same stack SteamOS
Desktop Mode uses (Steam + Proton), without requiring KDE or GNOME.

**Install Steam (pick one):**

```bash
# Native .deb (Valve repo — Ubuntu/Debian)
sudo dpkg --add-architecture i386
sudo apt update
sudo apt install -y steam-installer mesa-vulkan-drivers mesa-vulkan-drivers:i386

# Or Flatpak
flatpak install flathub com.valvesoftware.Steam
```

For **native** Steam, also install the controller udev rules so gamepads and the
Steam Controller are accessible without root, and make sure a PipeWire/Pulse
sound server is running (Steam and most games expect one):

```bash
sudo apt install -y steam-devices        # /usr/lib/udev/rules.d for controllers
# PipeWire is standard on modern Ubuntu; Pulse works too. No compositor config needed.
```

For **Flatpak** Steam, the runtime is sandboxed (pressure-vessel). Games and the
Proton prefix live under `~/.var/app/com.valvesoftware.Steam/` (not `~/.steam`).
If controllers, extra drives, or specific devices are missing, widen its device
access and confirm portal permissions:

```bash
flatpak override --user --device=all com.valvesoftware.Steam
# Extra library on another disk:
flatpak override --user --filesystem=/mnt/games com.valvesoftware.Steam
```

**Launch:** open Steam from the app launcher or run `steam`. When Steam is
detected (native on `PATH` or the Flatpak package), Metis also shows a
controller-friendly **Big Picture** button in the app-menu rail, which runs
`steam -gamepadui` (or `flatpak run com.valvesoftware.Steam -gamepadui`). The
button is hidden entirely on machines without Steam.

**Proton** runs Windows games as child processes of Steam over Wayland/XWayland.
Enable it in *Steam → Settings → Compatibility → Run other titles with…* and pick
**Proton Experimental** or a **GE-Proton** build (install GE-Proton via
[ProtonUp-Qt](https://github.com/DavidoTek/ProtonUp-Qt) or by dropping it in
`compatibilitytools.d`). Common failure modes:

- **Black screen / no Vulkan** — missing 32-bit Vulkan. Install `i386` +
  `mesa-vulkan-drivers:i386` (native) or update the Flatpak runtime.
- **Wrong GPU picked** — see hybrid-GPU below.
- **Anti-cheat** — enable *Steam Play* for the title and check
  [ProtonDB](https://www.protondb.com) for per-game tweaks.

**Hybrid GPU (laptops).** Metis exports the compositor's own render GPU to
every spawned client, so Steam, Proton, XWayland, and Vulkan apps default to the
**same** card the session renders on instead of silently picking the wrong one.
The card is chosen by the compositor (override with `METIS_DRM_DEVICE`, see dev
docs) and forwarded as `DRI_PRIME` (Mesa GL) and `MESA_VK_DEVICE_SELECT` (Mesa
Vulkan). On hybrid laptops where the panel is driven by the integrated GPU but a
discrete GPU is present, Metis also **auto-offloads game and Steam launches** onto
the dGPU (NVIDIA PRIME offload or Mesa `DRI_PRIME` for the dGPU render node), and
**web browsers** when on AC power (so windowed WebGL / canvas games are not stuck
on a weak iGPU). Lightweight editors and other desktop apps stay on the
power-efficient iGPU. Override session-wide with `METIS_GAME_GPU=igpu|dgpu|off`.
To run a *specific* title on the discrete GPU
instead, set a per-game launch option in Steam (*Properties → Launch Options*) —
these still win because Metis only sets the vars when they are unset:

```text
DRI_PRIME=1 %command%
prime-run %command%
__NV_PRIME_RENDER_OFFLOAD=1 __GLX_VENDOR_LIBRARY_NAME=nvidia %command%   # NVIDIA
```

Set `METIS_NO_CLIENT_GPU=1` in the session environment to disable the automatic
forwarding entirely.

**Gaming Platform 2.0 (Settings → Gaming).** Metis productizes GPU routing and
Flatpak setup in `~/.config/metis/gaming.json` instead of env-var recipes:

- **Graphics mode** — `auto`, desktop iGPU / games dGPU, always dGPU/iGPU, or off.
- **On battery** — prefer iGPU unless you override per session.
- **Auto performance profile** — `metis-gamingd` switches to Performance while a
  game session is active and restores on exit.
- **Auto GameMode** — registers detected game PIDs with `gamemoded` when installed.
- **Flatpak GPU env** — applies NVIDIA/Mesa offload vars to Steam, Lutris, and Heroic
  via idempotent `flatpak override` (state in `gaming-flatpak.json`).
- **Extra Steam library mounts** — Settings → Gaming → **Steam library paths**
  (or `extra_steam_paths` in `gaming.json`). Each path must exist as a directory
  and resolve under `$HOME`, `/mnt`, `/media`, or `/run/media` (`~` expands via
  `$HOME` only). Invalid entries are dropped on load; survivors are passed to
  Flatpak Steam as `--filesystem`. After adding paths, run **Optimize Flatpak
  Steam** (or **Optimize now**).
- **MangoHud / Gamescope (Metis launches only)** — optional toggles in
  `gaming.json` / Settings → Gaming. When Metis starts Steam or Big Picture and
  the binary is on `PATH`, Metis sets `MANGOHUD=1` or prefixes `gamescope --`.
  These never write Steam Properties or Launch Options.
- **Optimize for gaming** — Flatpak overrides + input-group Fix.
- **Run gaming setup** — stepped wizard: Steam → Vulkan → controllers →
  GameMode → GPU mode Auto → NVIDIA consent (when needed) → Flatpak finish.
- **First-run wizard** — optional Gaming step in onboarding; same messaging;
  use Settings → Gaming Fix / setup for missing pieces (no silent driver install).

Flatpak Steam launches through `~/.local/share/metis/bin/launch-steam` when the
Flatpak is installed (Big Picture and menu entries use it automatically). Runtime
reload: `metis-cmd reload-gaming`; optimize: `metis-cmd optimize-gaming`.

**What Metis installs (with your consent):** allowlisted apt packages via health
**Fix** (Steam, Mesa Vulkan amd64/`i386`, `steam-devices`, GameMode, PipeWire,
input group) and an explicit NVIDIA path (`ubuntu-drivers install` after a
confirm dialog + admin password). A reboot banner appears until the NVIDIA
module loads. Drivers are never installed at session start.

**What Steam / Proton own:** game installs, Proton versions, Steam Input, and
per-title Launch Options. Leave GPU Launch Options empty on hybrid laptops —
Metis session offload already sets PRIME / `DRI_PRIME` when unset.

**Game windows (X11 / borderless).** Metis keeps Steam’s splash and main window
as ordinary floats (they animate size while loading). Proton / `steam_app_*`
titles and other large undecorated game windows are placed flush or promoted to
true fullscreen so borderless-windowed clients are not inset under the edge bar
and clipped. Defaults live in `~/.config/metis/game-rules.json` (built-in list
if the file is absent); edit that file to float-only or fullscreen per app id.

**Controllers & Steam Input.** Games read `/dev/input/event*` directly (SDL,
Proton, Steam Input) — Metis does **not** grab evdev devices, so gamepads,
the Steam Controller, DualSense, and Switch Pro controllers work as they do
under any desktop. Configure mappings in *Steam → Settings → Controller*; there
is no compositor-side gamepad driver. Flatpak Steam may still need
`--device=all` (above). Ensure your user is in the `input` group.

**Steam overlay (Shift+Tab).** Works on Metis: focus follows clicks (no
focus-follows-mouse), so the overlay keeps input while it is up. It is most
reliable on XWayland/Proton titles; some native-Wayland games render their own
overlay differently. If it seems unresponsive, click the game window first so it
holds focus.

**Remote Play / Steam Link.** In-home streaming captures the game via the
PipeWire **ScreenCast** portal that Metis ships (`metis-portal`), so host
streaming works without extra setup. Encoding performance is hardware-dependent.

**Power while gaming.** Steam and games hold an idle inhibitor through the
Wayland idle-inhibit protocol and the `org.freedesktop.ScreenSaver` /
`PowerManagement` D-Bus interfaces, both wired end-to-end in Metis, so the
screen will not blank and the machine will not auto-suspend mid-game. For
sustained performance, pick a performance profile in *Settings → Power*.

**Gaming polish (optional).** Prefer Metis Settings toggles for MangoHud /
Gamescope on Metis-launched Steam / Big Picture. For a single title only, you
can still add Steam launch-option prefixes:

```text
gamemoderun %command%                         # sudo apt install gamemode
mangohud %command%                            # FPS/frametime overlay
MANGOHUD=1 %command%                           # Flatpak Steam
```

[GameMode](https://github.com/FeralInteractive/gamemode) is a standalone D-Bus
service (`com.feralinteractive.GameMode`); games talk to it directly via
`gamemoderun`, so nothing needs configuring in Metis beyond installing the
package. MangoHud/vkBasalt likewise attach per game.

**Mouse-look & in-game menus (pointer lock).** Metis implements the standard
Wayland pointer-constraints and relative-pointer protocols (same model as
Mutter/KWin). While locked, only relative motion is delivered and the system
cursor stays put; locked clicks are not remapped through
`set_cursor_position_hint` (Proton streams hints during mouse-look — remapping
them warps the camera). Hints are used only to restore the desktop cursor when
the lock ends. If a lock briefly drops, Metis re-arms it while the pointer
stays over the game surface. During pointer lock the compositor does not
repaint on mouse motion — only the game's commits drive frames. Fullscreen
games skip wallpaper, night light, and the compositor cursor so the display
path can promote the game buffer to direct scanout when formats match. Enable
**Adaptive sync** in Settings → Display for VRR on supported panels. Pause
menus typically destroy the lock so absolute clicks work; keyboard navigation
is always a fallback.

**Gamescope (optional):** SteamOS Gaming Mode uses [Gamescope](https://github.com/ValveSoftware/gamescope)
as its compositor. On Metis, Gamescope is optional — add to a game's Steam launch
options to wrap only that title:

```text
gamescope -W 1920 -H 1080 -f -- %command%
```

Metis stays the session compositor; Gamescope nests inside it for that game
(frame limit, scaling, FSR). On Ubuntu 24.04, `gamescope` is not in apt — build
from [source](https://github.com/ValveSoftware/gamescope) if you need it.

**SteamOS / handheld (experimental).** Valve's SteamOS 3.x uses Gamescope for
handheld Gaming Mode and KDE for Desktop Mode. Running Metis *on* SteamOS
(replacing Desktop Mode) is experimental and unsupported: SteamOS mounts its
root filesystem read-only (use `steamos-readonly disable` at your own risk to
install packages), and Gamescope Gaming Mode and Metis are alternative session
compositors — you run one *or* the other, not as the outer session. The
supported target is **Steam + Proton working on a Metis session** on Ubuntu and
similar distros.

On Deck-class hardware running Metis on a normal distro, SD-card readers, volume
buttons, and gyro (where exposed as evdev) should pass through to Steam Input
like on any other desktop — this has **not** been verified on Metis hardware yet.
Use *Settings → Gaming* to confirm controllers and touchscreens are visible to the
session.

---

## 5. Window management

Metis draws **server-side decorations**, so every window (Wayland or XWayland)
gets a consistent titlebar and border that follow your theme. Electron/Chromium
apps (Cursor, Claude Desktop, …) are steered onto native Wayland when launched
from Metis, which is more stable than their default XWayland path and still gets
the Metis titlebar.

**X11 vs Wayland isolation.** Native Wayland clients cannot be keylogged or
surface-sniffed by X11/XWayland apps — rootless XWayland does not see Wayland
input or buffers. By default all X11 clients share **one** XWayland server
(`config.json` → `"xwayland_mode": "shared"`), so a malicious X11 app can still
attack other X11 apps (classic X11↔X11). Metis does **not** claim XSECURITY
sandboxes.

**Isolated X11 (soft bucketing).** Settings → Gaming → **Isolated X11 (gaming
bucket)** (or `"xwayland_mode": "isolated"`) steers Metis-spawned gaming-class
launches (Steam, Proton, Lutris, Heroic, Wine, Flatpak Steam ids, …) onto a
**second** XWayland `DISPLAY`. That server is **lazy-started** on the first such
launch (not at login). Gaming X11 class is independent of GPU/battery offload, so
Steam still gets the gaming bucket on battery. Optional
`config.json` → `xwayland_policy.extra_gaming_patterns` appends match substrings.
This only applies to compositor `Launch` / Metis-spawned clients; it is **not** a
sandbox — same-UID processes can open either X socket, and residual X11↔X11 risk
remains **inside** each bucket. Restart the Metis session after changing the
toggle. By default Metis also starts XWayland **without** the abstract Unix
socket (`"xwayland_abstract_socket": false`); set it to `true` if a legacy local
client needs `@/tmp/.X11-unix/...`.

- **Move** — drag the titlebar.
- **Close / minimize / maximize** — the three titlebar buttons (× / − / +).
- **Resize** — drag a window border or corner; tiled windows float out of the
  grid when you resize them, and the new geometry is remembered.
- **Snap zones** — drag a window to a screen edge for a live translucent preview,
  then drop to snap into half / quarter / maximize regions. Snapping respects the
  bar, so the top zone clears it.
- **Maximize** — `Super`+`F` toggles maximize for the focused window (fills the
  area below the edge bar, same as the titlebar + button or top-edge snap);
  `Escape` exits maximize, fullscreen, or grid tile mode.
- **Close** — `Super`+`Q`.
- **Geometry memory** — on the default desktop layout, a window you've moved or
  resized reopens at the same position and size next time you launch it (saved per
  app in `~/.config/metis/windows.json`). Off-screen saved positions are pulled
  back on-screen; grid/scrolling workspaces tile instead.

**Auto-hiding titlebars.** Maximized and edge-snapped windows hide their titlebar
so the client fills the space; hover the top strip to reveal it as a translucent
overlay.

Titlebar translucency, the title "pill" border, and the window frame border are
all configurable in Settings → Appearance → Windows.

---

## 6. Workspaces

Each workspace is a separate set of app windows. Switch between them with the bar
dots or the keyboard:

- `Super`+`1` .. `Super`+`9` — switch to that workspace.
- `Super`+`Shift`+`1` .. `9` — move the focused window to that workspace.
- `Super`+`Alt`+`←` / `→` — cycle to the previous / next workspace (wraps at
  1..=count). Always uses **Super**+**Alt** — not remapped by `METIS_MOD` (see
  nested sessions below).
- Click a workspace dot in the bar to switch.
- `Super`+`Tab` — **Task View** (Windows-style sticky overlay) for the focused
  output: live thumbnails of apps on the current workspace, plus a bottom shelf
  of desktop mini-previews. Click an app card (or press Enter) to focus it and
  dismiss; press-and-drag a card onto a desktop tile to move it; click × on a
  card to close that window without leaving Task View; click a desktop tile to
  switch workspaces; Esc or empty backdrop dismisses. Further `Super`+`Tab`
  cycles apps; `Super`+`Shift`+`Tab` cycles backward. Releasing Super does **not**
  close Task View. `Alt`+`Tab` is unbound by default — remap **Task View (next) /
  (previous)** in Settings → Keyboard if you want Alt+Tab again.

### Task View

Task View replaces the older separate Alt+Tab strip and workspace-overview grid.
It is sticky (like Windows Task View): open with `Super`+`Tab`, release Super to
use the mouse, then click or press Enter to activate.

Keybinds and clicks act on the monitor under the pointer.

### Per-output behaviour

With multiple displays you choose how workspaces relate, in
Settings → Appearance → Edge bar → *Workspaces*:

- **Independent per display** (default) — each monitor keeps its own set of
  workspaces and its own active one. The dots on each bar reflect that monitor.
- **Linked across displays** — switching a workspace moves every monitor at once,
  so all displays stay on the same virtual desktop number.

The taskbar dock always follows its own bar's output and active workspace.

### Moving windows between monitors

With multiple displays (or `METIS_VIRTUAL_OUTPUTS=2` in a dev session):

- **Drag** — titlebar-drag a window onto another monitor and release; Metis
  re-homes its desk tile to that output automatically (snapping on a secondary
  monitor does the same).
- **Keyboard** — on a **grid** workspace, `Super`+`Shift`+`←` / `→` moves the
  focused window to the adjacent monitor (left-to-right order). On a **scrolling**
  workspace those keys still move columns instead.

The window keeps its workspace number on the destination output (e.g. workspace 2
on monitor A becomes workspace 2 on monitor B). If that workspace is not active
on the destination, the window is stashed until you switch to it there.

**Move the whole workspace** — with independent per-output workspaces,
`Super`+`Ctrl`+`Shift`+`←` / `→` moves every window on the active workspace
(under the pointer) to the same workspace number on the adjacent monitor, including
scroll layout state.

---

## 7. Scrolling layout (niri / PaperWM style)

Any workspace can be a **grid** (the default tiling) or a **scrolling** layout —
an infinite horizontal strip of full-height columns (niri / PaperWM / paneru
style). Each column is one window (or a vertical stack), and the strip extends to
the right as you open more. The viewport scrolls to keep the focused column in
view; off-screen columns are clipped to the current display, so a column scrolled
past the edge never bleeds onto an adjacent monitor.

Opening a new window **never resizes the windows already on the strip** — it just
appends a column. New windows open at half-width.

### Resizing columns

- **Mouse** — drag a window's **right** border to set its width; everything to the
  right slides over to make room. Dragging the **left** border resizes the
  previous window. Columns are full-height, so there's no vertical resize.
- **Keyboard** — `Super`+`-` / `Super`+`=` snaps the focused column to full width,
  then back to half.

### Turning it on

- **Per workspace** — `Super`+`\` toggles the active workspace between grid and
  scrolling.
- **Everywhere** — Settings → Appearance → Edge bar → *New workspace layout*.
  Choosing Grid tiling or Scrolling applies to **every** workspace on **every**
  output immediately (it acts as a global on/off switch).

### Navigating a scrolling workspace

| Shortcut | Action |
|----------|--------|
| `Super`+`←` / `Super`+`→` | Move focus to the previous / next column |
| `Super`+`↑` / `Super`+`↓` | Move focus up / down within the focused column's stack |
| `Super`+`Shift`+`←` / `→` | Move the focused column left / right |
| `Super`+`Shift`+`↑` / `↓` | Move the focused window up / down in its stack |
| `Super`+`,` | Consume: pull the next window into the focused column |
| `Super`+`.` | Expel: push the focused window out into its own column |
| `Super`+`-` / `Super`+`=` | Snap the focused column to full width / back to half (or drag a border to resize) |
| `Super`+`\` | Toggle this workspace back to grid |

These scrolling keybinds are only active while the focused workspace is in
scrolling mode; in grid mode they're inert.

---

## 8. Keyboard shortcuts reference

Defaults are listed below. Browse them in **Settings → Shortcuts** (searchable
read-only guide), and change them in **Settings → Keyboard → Shortcuts** (saved
to `~/.config/metis/keybinds.json`, live-reloaded). Ctrl+Alt+F1–F12,
Ctrl+Alt+Backspace, and the multimedia / hardware keys (see below) are system-only
and cannot be rebound.

| Shortcut | Action |
|----------|--------|
| `Super`+`1`..`9` | Switch to workspace 1–9 (monitor under the pointer) |
| `Super`+`Shift`+`1`..`9` | Move the focused window to workspace 1–9 |
| `Super`+`Alt`+`←` / `→` | Cycle to previous / next workspace (wraps at 1..=count) |
| `Super`+`Shift`+`←` / `→` | (grid) Move the focused window to the adjacent monitor; (scroll) move column/window |
| `Super`+`Ctrl`+`Shift`+`←` / `→` | Move the active workspace to the adjacent monitor (independent mode) |
| `Super`+`F` | Toggle maximize for the focused window (fills the usable area, with Settings → Windows padding; with edge-bar auto-hide the bar overlays rather than shrinking the window) |
| `Super`+`Shift`+`F` | Toggle true fullscreen |
| `Super`+`Q` | Close the focused window |
| `Super`+`M` | Minimize the focused window |
| `Super`+`L` | Lock the session |
| `Super`+`T` | Open the default terminal (Settings → Metis Menu) |
| `Super`+`Esc` | Exit fullscreen / maximize / tile |
| `Super`+`/` | Enable grid tiling |
| `Super`+`\` | Disable tiling (free desktop) |
| `Print` / `Shift`+`Print` / `Ctrl`+`Print` | Screenshot interactive / full / window |
| `Super`+`Tab` | Task View — open / cycle next (sticky; Esc or click to dismiss) |
| `Super`+`Shift`+`Tab` | Task View — cycle previous |
| `Super`+`←` `→` `↑` `↓` | (scrolling) Move focus between/within columns |
| `Super`+`,` / `Super`+`.` | (scrolling) Consume into / expel from a column |
| `Super`+`-` / `Super`+`=` | (scrolling) Snap the focused column to full / half width |

### Session lock (password, fingerprint, YubiKey)

`Super+L` (or the shell menu **Lock**) shows the compositor lock screen. Unlock
is PAM against `/etc/pam.d/metis` (falls back to `login` if that file is missing).
Password unlock always works.

**Third-party lockers (`ext-session-lock-v1`).** Metis advertises the standard
session-lock protocol so tools like **swaylock** or **gtklock** can lock the
session when you launch them. While a protocol locker is active, outputs are
blanked to that locker’s surfaces only (Metis PAM chrome is not drawn for that
cycle). Unlocking the locker restores the desktop and runs the same side effects
as Metis unlock (RDP resume, etc.). Super+L / idle lock keep using Metis PAM when
no protocol locker is active; only one lock owner is allowed at a time.

**Fingerprint / YubiKey (optional).** Metis does not enroll devices itself. When
a fingerprint reader or Yubico USB key is detected, the lock screen shows a
touch hint and may start one empty PAM attempt so `pam_fprintd` / `pam_u2f` can
succeed without typing. Configure the host:

1. Install (Ubuntu): `sudo apt install fprintd libpam-fprintd` and/or
   `sudo apt install libpam-u2f pamu2fcfg`
2. Enroll: `fprintd-enroll` ; for YubiKey FIDO2/U2F:
   `mkdir -p ~/.config/Yubico && pamu2fcfg >> ~/.config/Yubico/u2f_keys`
3. Uncomment the optional `auth sufficient` lines in `/etc/pam.d/metis`
   (see comments in that file / the packaged `pam-metis` asset) **above**
   `@include common-auth`, then lock and try again.

YubiKey support is touch-based FIDO2/U2F via `pam_u2f` (not Yubico OTP).

### Multimedia & hardware keys

Laptop function-row and media-keyboard keys work system-wide and flash a
bottom-center overlay showing the new level. They are wired in firmware/xkb, so
they are fixed and cannot be rebound (they appear under **Settings → Keyboard →
Shortcuts → System** for reference).

| Key | Action |
|-----|--------|
| Volume Up / Down / Mute | Adjust or mute the default output (PipeWire/PulseAudio) |
| Mic Mute | Mute / unmute the default microphone |
| Brightness Up / Down | Display backlight (via logind) |
| Keyboard Backlight Up / Down / Toggle | Keyboard backlight (via logind) |
| Play/Pause · Stop · Next · Prev | Media transport for the active player (MPRIS) |
| Rewind / Fast-forward | Seek the active player ∓10 s |
| Display (monitor switch) | Toggle **mirror ⇄ extend** across displays (DRM sessions) |

Brightness and keyboard backlight use logind's `SetBrightness`, so no root
privileges, udev rule, or setuid helper is required. Media control targets the
first player reporting `Playing`; if none is playing, the first available player.

**Nested in GNOME?** `./run-metis.sh --session` may set `METIS_MOD=alt` for first-run
defaults — read **Super** as **Alt** in the table above **except** workspace cycle
(`Super`+`Alt`+`←`/`→`), which always uses the logo key plus **Alt**. Prefer changing
the Metis modifier and individual chords in Settings → Keyboard. Click the Metis
window first so it has keyboard focus. On a real Metis session, **Super** is the
logo / Windows key by default.

---

## 9. The Settings app

Settings **2.0** opens on a **Home** overview with category tiles (Displays,
Desktop, Connectivity, Input, System). A slim icon sidebar jumps to the same
categories. Choosing a category (or a search hit) slides that category’s pages
in from the **right** (GtkStack). Confirmations and pickers (colour / font,
display keep/revert, Wi‑Fi password, Known Wi‑Fi, VPN add forms, Calendars add
account, Remote password) drop in from the **top** as in-window sheets.

Closing the Settings **window** fully quits the process (unique-instance
`com.metis.Settings` does not linger for page timers). The sheet **X** on a
category returns to Home; use the window close control to exit.

Launch Settings from the app launcher's quick-launch rail, or from a terminal:

```bash
metis-cmd settings            # open Settings (Home)
metis-cmd settings appearance # open a specific page in its category sheet
```

Search on Home filters category tiles and lists matching pages. Deep-link with
`metis-cmd settings <page>` (e.g. `display`, `network`, `power`).

- **Display** — per-output scale, enable/disable, resolution & refresh (DRM mode
  list on real hardware), **Duplicate displays** (mirror clone with scale-to-fit
  letterboxing on DRM hardware), and multi-monitor arrangement (drag preview when
  two or more outputs are connected; hidden while duplicating; **Save display
  settings** with a keep/revert confirmation). Scale, **Active**, **Adaptive
  sync** (VRR), and **HDR** (when the monitor EDID advertises HDR10 / ST.2084
  or HLG — many laptop panels do not) apply live;
  duplicate mode, arrangement, and resolution changes are batched behind save.
  Night-light preferences apply live in the compositor (warm overlay; skipped
  while HDR is active on that output). With **HDR** on, the compositor
  tone-maps the desktop through Rec.709→BT.2020 then **PQ** (preferred when
  EDID has ST.2084) or **HLG** (HLG-only panels); reference white ≈ 203 nits.
  Per-output **ICC colour profiles** apply a GLES 3D LUT when possible
  (otherwise the profile `vcgt` drives CRTC gamma). The Wayland
  `wp_color_management_v1` protocol is
  **experimental and off by default** — set `METIS_COLOR_MGMT=1` only for
  testing; advertising it to Chromium/Ozone can crash the session (upstream
  wayland-rs server ObjectData bug). Hardware ICC / LUT / HDR do **not** need
  that env var. When a client advertises PQ/HLG via colour management (opt-in
  protocol), the compositor **pass-through** skips SDR→HDR re-encode for that
  output so HDR content is not double-transformed (mixed SDR+HDR is approximate).
  Rotation is still upcoming.
- **Appearance** — Light/Dark style; accent, secondary, and semantic status
  colors; font. (Wallpaper, edge bar, and window chrome live on their own pages
  below.)
- **Background** — picture / solid colour / gradient, applied live and remembered
  in `wallpaper.json`, with optional per-output picture overrides. Changes
  **crossfade** (~280 ms) from the previous background — the desktop never goes
  black while the next image decodes. The picture picker groups **Your
  pictures** (imports under `~/.config/metis/wallpapers`), **Metis** (bundled
  defaults), and **System** (e.g. Ubuntu/GNOME images under
  `/usr/share/backgrounds`). Large libraries are paginated (9 thumbs per page)
  with All / Metis / System filters; thumbnails load asynchronously and warm a
  decode cache (`~/.cache/metis/wallpaper-rgba/`) so clicks apply quickly.
- **Edge bar** — position (top/bottom/left/right),
  distance from the edge, **bar length** (40–100%, centered), **bar background**
  (theme / solid / gradient + direction), **auto-hide** (slides to a peek and
  **overlays** maximized windows — peek/reveal does not resize them; move to the
  thin edge strip to open again; with a top bar, use the titlebar band just below
  the peek for window controls),
  the bar border, *Show bar on* (all displays / primary
  only), *Workspaces* (independent vs linked), and *New workspace layout* (grid
  vs scrolling).
- **Windows** — titlebar opacity, the title pill
  border, the window frame border, and **window padding** around maximized /
  snapped windows.
- **Desktop widgets** — optional wallpaper panels (Folders, Apps, Clock, System,
  Weather, Equalizer, plus **JSON extensions**; off by default) hosted by a
  separate `metis-shell --desktop-widgets` process so a hung widget cannot freeze
  the edge bar. Enable the layer, turn on **Edit mode** to move/resize, set
  **Default look** (fill / border), and manage instances in a compact zebra list
  (**Add** stays at the top; gear opens a configure dialog). Clock / Weather /
  System: font, text colour, accent. Equalizer: Spectrum / Bars / Neon wave /
  Radial, bar shapes, colour modes, peaks, reflection / mirror. Per-widget look
  overrides live in each dialog. Writes `desktop-widgets.json`; the widgets
  process live-reloads.

  **Extensions (Phase 14 §E).** Install a pack under
  `~/.local/share/metis/widgets/<id>/` (or `/usr/share/metis/widgets/<id>/`) with
  `manifest.json` + `widget.json`. Settings → Desktop widgets lists discovered
  packs in the Add dropdown. **Author schema:** [`WIDGET_PACK_SCHEMA.md`](WIDGET_PACK_SCHEMA.md)
  (API 1). Discovery is fail-closed (Phase 18 C): unknown JSON fields, invalid
  layouts, or missing declared helpers skip the pack (warn + startup toast) —
  they never crash the widgets host. Declarative layout only (labels, icons,
  buttons, lists); button actions: `open_uri` (**http/https only**), `launch`
  (desktop id or a single PATH basename — no shell/argv/paths; interpreters
  denylisted), `copy_text`. Settings placeholders apply to labels/copy text only
  — **not** to URI/launch targets. Live host tokens in labels: `{host.time}`,
  `{host.date}`, `{host.weather.temp}`, `{host.weather.unit}`,
  `{host.weather.summary}`, `{host.sys.cpu}`, `{host.sys.mem}`,
  `{host.sys.disk}` (refreshed by the widgets host; not allowed in action
  fields). Optional **out-of-process helpers**: declare `helper.exec` (basename
  under the pack) + `poll_seconds` in `manifest.json`; the helper is spawned
  argv-only (cleared env, timeout, stdout size cap) and must print a JSON
  object — labels may use `{helper.<key>}`. No network from helpers by default;
  not available to `open_uri` / `launch`. Rhai/Lua/WASM remain deferred. Example
  packs: `com.metis.example.quicklinks`, `com.metis.example.helperstatus`.
- **Metis Menu** — pick a **layout** from paginated thumbnails (Metis, Whisker,
  ArcMenu, Mint, plus the layout pack: **Bracket**, **Ledger**, **Mosaic**,
  **Ramp**, **Ladder**, **Plaza**, **Crest**, **Chip**), toggle the user avatar/name header,
  places/power rail, and pinned column (some layouts hide pinned by design);
  choose your default **terminal** and **file manager** (auto-detected installs
  or a custom binary path); and set launcher panel opacity. Tap **Super** to
  toggle the menu; start typing while it is open to filter applications.
  **Bracket** / **Ladder** browse by Freedesktop categories; grid layouts
  (**Mosaic**, **Chip**, **Plaza**, **Crest**) show icon tiles; **Plaza** adds
  a profile/power footer; **Crest** centers a large avatar header. Selecting
  **Settings** restores and focuses the existing Settings window (including when
  minimized) instead of opening a duplicate. Saved to `menu.json` (layout
  changes reload the edge bar live).
- **Users** — profile picture and display name (synced with Metis Menu **and**
  AccountsService so GDM / other greeters update), password change, and local
  account management (add/remove, Administrator / `sudo` toggle). The **Other
  users** list shows each account with a circular avatar, display name,
  username, and role. Avatars resolve from `~/.face`, `~/.face.icon`, or the
  GNOME/KDE AccountsService icon (`/var/lib/AccountsService/icons/<username>`),
  then a default icon. Changing your picture also writes `~/.face.icon` and
  calls AccountsService `SetIconFile` (privileged copy fallback). Privileged
  actions show Metis’s PolicyKit password dialog (`metis-polkit-agent` +
  `metis-remote`).
- **Date & Time** — automatic date/time (NTP), automatic timezone, manual
  clock/timezone when auto is off, 12/24-hour bar format, and calendar first day
  of the week (`datetime.json`).
- **Weather** — manual location override + search, multiple saved locations
  (reorder/remove), °F/°C unit, and an IP-geolocation toggle.
- **Network** — Wireless / Wired / **VPN** / **DNS** / Proxy. Wi-Fi
  scan/connect/forget with zebra rows; **Known Wi‑Fi** and Wi‑Fi password as
  top-slide sheets; wired DHCP vs static; VPN import (OpenVPN `.ovpn`,
  WireGuard `.conf`) plus **Add OpenVPN…** / **Add WireGuard…** top-slide forms,
  autoconnect toggle, and connect/disconnect/delete for NetworkManager profiles.
  On Debian/Ubuntu/Mint install `network-manager-openvpn` for OpenVPN; WireGuard
  is built into modern NetworkManager. Edge-bar **VPN** icon toggles
  connect/disconnect.
- **Calendars** — calendar accounts (local / CalDAV / Thunderbird / Microsoft
  365) used by the Notification Center calendar. **Add account** uses a
  top-slide sheet. Account metadata lives in `calendars.json`; **passwords and
  M365 refresh tokens** are stored in the freedesktop Secret Service
  (`metis-secrets` / oo7), not in the JSON file. Removing an account also
  deletes its keyring entries.
- **Input** — mouse, touchpad, and keyboard layout/repeat settings (`input.json`),
  plus **Keyboard → Shortcuts** to edit desktop keybinds (`keybinds.json`, live
  reload). **Shortcuts** (sidebar) is a searchable read-only chord guide with a
  jump to the editor; System VT/quit chords are listed but not editable.
- **Bluetooth** — adapter on/off, scan for devices (toggle stop, auto-stops after
  30s), pair / connect / trust / remove. Device list uses zebra rows. Battery
  percentage and charging state appear when the device or driver reports them.
- **Printers** — list CUPS queues; open the system printer config when needed.
- **Gaming** — graphics mode (auto / iGPU / dGPU), battery and performance
  toggles, health checklist with Fix buttons, **Optimize now** (permission dialog
  before Flatpak `--device=all` / network / Wayland overrides), and **Run gaming
  setup** wizard; writes `gaming.json`. CLI: `metis-cmd optimize-gaming --yes`.
- **Control Center** — enable/disable the pull-down system monitor, max panel
  height %, refresh interval, confirm-before-kill, overview widgets (CPU, Memory,
  Network, Disk, Session, Storage, System, Processes, **Battery**, **Logs**), and
  which process monitor **Open monitor** launches (auto-detect / installed /
  custom); writes `dashboard.json` for live shell reload. Battery history is
  hidden when no battery is present; Logs show a short journalctl tail or
  “unavailable”.
- **Power** — power profile (power-saver / balanced / performance via
  `power-profiles-daemon`), laptop battery details, idle blank/suspend timeouts,
  lid-close action, and a **Connected devices** list for Bluetooth peripherals
  with battery status. See [Power profiles](#power-profiles-settings--power) below.
- **Startup** — applications that launch once after sign-in (`startup.json`).
  Empty by default; use **Add application…** to pick desktop apps (no custom
  command lines). Master switch plus per-app enable toggles. The compositor
  starts them ~2 seconds after the session begins (skipped during onboarding
  or while the session is locked). Changes apply on the next login.
- **Remote access** — GNOME-style desktop sharing toggle: enable RDP to your
  **live** Metis session via `gnome-remote-desktop` (headless). Set credentials
  via a top-slide password sheet, copy the connection address, or open **Metis
  Viewer** / connect with Remmina / FreeRDP. Requires a real (DRM) session — not
  nested dev. Open with `metis-cmd settings remote` or `metis-cmd viewer`.
- **Sound** — default output and input device selection (bar volume widget
  unchanged).
- **Reset** — factory-reset Metis preferences under `~/.config/metis` with an
  optional backup to `~/metis-config-backup-…`, keep custom themes (not stock
  dark/light), and optionally run first-run setup again. Confirm before wipe;
  log out / restart the session afterward for a full reload.
- **About** — Metis product version (from the workspace crate / GitHub tag
  scheme), installed Metis components, author **DigitalExpl0it**, GitHub link,
  and host OS details (distro, kernel, CPU, memory).

**Bluetooth battery notes.** Many devices only expose a coarse percentage over
plain Bluetooth (often updating on reconnect). Charging state requires a driver
that reports it — kernel HID batteries, UPower, or **Solaar** for Logitech
peripherals (optional; Metis ignores Solaar silently when it is not installed).
For the most accurate Logitech battery and charging info, use a Unifying/Bolt USB
receiver or install Solaar.

Most appearance and bar changes apply live; some device-backed settings only take
full effect under a real (DRM) session.

### Power profiles (Settings → Power)

| Control | Applies via | Notes |
|---------|-------------|-------|
| **Power saver / Balanced / Performance** | `powerprofilesctl` → `power-profiles-daemon` | Requires the daemon package (`power-profiles-daemon` on Ubuntu). Changes CPU/platform power behaviour on supported laptops. On desktops without platform profiles the effect may be minimal. Verify with `powerprofilesctl get`. |
| **Blank screen after** | Metis compositor (DPMS) | Live-reloaded via `ReloadPower` IPC; independent of logind. |
| **Suspend after idle** | systemd-logind (`busctl`) | Best-effort; needs logind and appropriate permissions. |
| **When lid is closed** | systemd-logind | Laptop only; suspend / ignore / hibernate / power off. |
| **Dim on battery** | Compositor overlay | When on battery and enabled in `power.json`, applies a light full-output dim (skipped on HDR-active outputs). Live-reloads with Power settings. |

While gaming, **Settings → Gaming → Auto performance profile** (via `metis-gamingd`)
can temporarily switch to **Performance** and restore your previous profile when the
game session ends.

### Remote desktop (RDP)

**Settings → System → Remote access** turns on headless RDP sharing for the
session you are logged into — the same idea as GNOME Settings → Sharing → Remote
Desktop. Metis orchestrates `gnome-remote-desktop` via the `metis-remote` helper;
credentials live in the headless store (`grdctl --headless`), not in Metis config
files.

1. Install the backend on the host (Ubuntu):
   `sudo apt install gnome-remote-desktop`
2. Open **Settings → Remote access** (or `metis-cmd settings remote`).
3. Click **Set password…** and choose the RDP username and password clients will
   use. Settings pipes the password into `metis-remote` on stdin — never put the
   password on a shell command line.
4. Leave **LAN only (firewall)** on under **Security** (default). That preference
   alone does not open a password dialog — rules are applied when sharing is on.
5. Turn on **Allow desktop session sharing**. Metis then applies nftables (preferred)
   or active ufw rules so TCP **3389** accepts only private, loopback, and
   link-local sources. A PolicyKit password dialog may appear; status and any
   **Retry firewall apply** live under **Security**.
6. Copy the connection address (hostname or LAN IP plus port **3389**), or click
   **Connect with Metis Viewer…** to open the first-party client with host/port
   prefilled.

**Test from another machine** (or the VM host), not from Metis Viewer *inside*
the same shared session. Connecting a guest to its own RDP address nests FreeRDP
inside the desktop GRD is capturing and often fails during clipboard setup.

**CLI credentials (if needed):**

```bash
printf '%s\n' 'your-password' | metis-remote set-credentials YOUR_USER
```

#### Metis Viewer (RDP client)

**Metis Viewer** (`metis-viewer`) is the first-party RDP *client* — a GTK connect
dialog that spawns FreeRDP (`wlfreerdp3` → `wlfreerdp` → `xfreerdp3` →
`xfreerdp`, searched under `/usr/bin` only; argv spawn, no shell). Host sharing
stays **Settings → Remote access** + `gnome-remote-desktop` / `metis-remote`.

1. Install a FreeRDP client on the machine that will connect (Ubuntu):
   `sudo apt install freerdp3-wayland`  
   (or `freerdp2-x11` if Wayland FreeRDP is unavailable).
2. Open **Metis Viewer** from the app launcher, `metis-cmd viewer`, or
   **Settings → Remote access → Connect with Metis Viewer…**.
3. Enter host, port (default **3389**), username, and optionally password.
   Recent hosts are stored in `~/.config/metis/viewer.json` (**no passwords**).
   Click a recent row to fill fields and connect; use the trash control to remove
   an entry. If the password field is left empty, FreeRDP prompts (GUI dialog);
   if filled, Metis passes `/p:` only on the FreeRDP child argv (never logged,
   never written to config — briefly visible in `/proc` while FreeRDP starts).
   Quick FreeRDP failures (auth/connect) surface in the status line within a few
   seconds. Metis Viewer passes `/cert:ignore` so GNOME Remote Desktop’s
   self-signed LAN certificates work without an interactive prompt.

**Other clients.** Windows: *Remote Desktop Connection* (`mstsc`). macOS:
*Microsoft Remote Desktop* from the App Store. Linux CLI:

```bash
xfreerdp /v:HOST:3389 /u:USERNAME /dynamic-resolution
```

(`grdctl` still receives the password on its argv when Metis sets credentials —
that is a GNOME Remote Desktop limitation; minimize how long that process runs.
Prefer `printf '%s\\n' '…' | metis-remote set-credentials USER` so the password
never appears on a Metis argv.)

**Security.** `remote.json` defaults to `"lan_only": true`. Metis applies named
firewall rules (`metis-rdp-lan-only` / nft table `inet metis_rdp`) when sharing
is enabled with LAN only on, and clears them on disable or when you turn LAN
only off (Settings warns first). Firewall status appears under the Security
card — not as a separate step on the sharing card. Do not expose RDP to the
internet without a VPN or strong perimeter controls. Manual check:
`metis-remote firewall status`.

**Session lock.** While the session is locked (`Super+L`), Metis pauses RDP listen
(`metis-remote pause`) without clearing `remote.json.enabled`, and blocks capture
and input injection. Unlock resumes sharing if it was still enabled. Remote
clients cannot view or control the desktop while locked. On a DRM session,
**Ctrl+Alt+F\<n\>** (VT switch) is blocked while locked; **Ctrl+Alt+Backspace**
still quits the session as an escape hatch.

**Auto-start.** When `remote.json` has `"enabled": true` and `"auto_start": true`
(the defaults after you turn sharing on), `metis-session` runs `metis-remote
autostart` at login so you do not need to reopen Settings each time.

**Clipboard.** Text copy/paste between the Metis session and an RDP client is
synced via the portal's Mutter clipboard bridge. **Text and images**
(`text/plain`, `image/png` / `jpeg` / `bmp`, ≤10 MB) — local durable clipboard
paths are advertised to RDP; remote images are written under
`$XDG_RUNTIME_DIR/metis/clipboard/` then set on the compositor.

**Troubleshooting.** If the page shows an install hint, install
`gnome-remote-desktop` and re-login. If enable fails with “Set RDP credentials”,
set a password first. If Metis Viewer says FreeRDP was not found, install
`freerdp3-wayland` or `freerdp2-x11`. If **Security** says firewall rules are not
applied (or Retry fails / times out), install `nftables` (recommended) or enable
`ufw` (`sudo ufw enable`). Metis starts `metis-polkit-agent` with the session so
a password dialog can appear for `pkexec`; without it, authorization waits until
it times out. On polkit 127+, the agent talks to
`/run/polkit/agent-helper.socket` (enable `polkit-agent-helper.socket` if auth
fails with a setuid-helper error). You can also run
`pkexec metis-remote firewall apply` from a terminal. PipeWire and the
Metis ScreenCast portal must be running in the DRM session — re-run
`./run-metis.sh --install-session` if portal capture is broken. Check status:
`metis-remote status` (JSON).

VNC (`wayvnc`) and classic `xrdp` login sessions are **not** driven by
Settings → Remote access. For VNC / compatibility notes, see
[`docs/UBUNTU_DEV.md`](UBUNTU_DEV.md) (Remote desktop).

#### RustDesk (optional Metis backend)

Settings → Remote access includes a **RustDesk** card that detects a system or
Flatpak install, opens the app, copies install instructions, and can run
`metis-remote rustdesk enable|disable` (optional Metis backend + LAN firewall for
ports **21115–21119** TCP / **21116** UDP). **GNOME Remote Desktop remains the
default session-sharing host.** Prefer PipeWire / portal capture on Metis.
Full host-install notes: [`docs/UBUNTU_DEV.md`](UBUNTU_DEV.md).

### System dashboard (Control Center)

Open the monitor by **pressing on the edge bar** and **dragging toward the desktop**,
or click the **grid icon** to the right of the workspace dots. The panel is
embedded directly under the bar with no gap.

| Bar position | Drag direction |
|--------------|----------------|
| Top | Down |
| Bottom | Up |
| Left | Right |
| Right | Left |

The panel tracks your drag (rubber-band) and snaps open if you pull far enough.
The dashboard **loads on demand** — no background polling until you open it; it
tears down again when dismissed.

**Overview** tab:

| Row | Cards |
|-----|--------|
| 1 | **Processor** (per-core lines + Σ total, gradient charts) · **Memory** (RAM + swap) |
| 2 | **Network** (ethernet/wifi rates + chart) · **Disk I/O** (read/write chart) |
| 3 | **Session** (load, uptime) · **Storage** (mount tiles) |
| 4 | **CPU temp** gauge · **GPU temp** gauge(s) for discrete GPUs only · **System** (hostname, CPU, kernel) |

CPU and discrete-GPU temperature gauges use a 0–150 °C semicircle. On hybrid
laptops (Intel + NVIDIA), the Intel iGPU is not shown; NVIDIA temps are read from
sysfs when available, otherwise from `nvidia-smi`.

**Processes** tab — searchable, sortable PPID tree (Name, PID, User, Type, CPU,
Memory). Expand a parent to see child processes. Search keeps ancestor paths so
matches stay visible under their tree. Right-click a killable row for **End task**,
**Force quit** (SIGKILL), **End process tree** / **Force quit tree** (when it has
children), or **Copy PID**. End-task confirmation is optional
(`dashboard.json` → `confirm_before_kill`). Use **Open monitor** to launch your
configured process monitor (Settings → Control Center: auto-detect prefers
btop/htop in a terminal, then GUI monitors). The process list pauses refresh while
a context menu is open so actions stay usable.

Dismiss with **Esc**, **Super+Q**, the close button, **drag back toward the bar**
on the header, or by clicking the desktop. While the panel is open, `Super+Q`
closes it rather than the application underneath. Configure in Settings →
**Control Center** or edit `~/.config/metis/dashboard.json` directly.

---

## 10. Configuration reference

All configuration lives in `~/.config/metis/` as JSON. You can edit files by
hand — `bar.json` and `themes/*.json` reload while Metis runs.

### Session IPC trust model

Short map for auditors: [`SECURITY.md`](../SECURITY.md).

Metis shell ↔ compositor control uses Unix sockets and command files under
`$XDG_RUNTIME_DIR/metis/` (directory mode `0700`; sockets and command files
`0600`). The compositor checks `SO_PEERCRED` and rejects connections whose peer
UID is not the session euid — that stops **cross-user** abuse. Any process
running as **your** UID can still drive the DE (launch apps, inject input,
end session); that is the intended same-session control plane. If
`XDG_RUNTIME_DIR` is unset, Metis fails closed rather than falling back to
`/tmp/metis`.

The isolated **desktop-widgets** process is spawned with a spawn-scoped
`METIS_IPC_TOKEN` and may only use a **widgets** capability (Launch, list
windows/outputs, light reloads) — it cannot EndSession, inject input, or start
capture overlays. The token is cleared when the widgets process exits or is
respawned (no wall-clock / idle TTL). The edge bar and Settings keep the
full-privilege channel (no token).

**Command files** (`command`, `command-widgets`) are a same-UID poke channel
(weaker than the socket + token path). Writers and readers enforce a strict
verb allowlist, a 512-byte length cap, and reject unknown verbs. That reduces
accidental or drive-by shell pokes; it does **not** protect against a malicious
process already running as your UID (it can still use the full IPC socket).

**Rate limits (Phase 18 B).** Same-UID clients can still connect, but Metis
bounds spam with sliding one-second windows: compositor command IPC (~120
requests/s, max 32 accepts per drain tick; excess gets a JSON
`Error` / `rate limited` reply), event-bus subscribe accepts (cap 16
subscribers), and command-file writes/dispatches. Direct `printf` to the
command file (e.g. `metis-cmd.sh`) bypasses the write helper; the shell poller
still rate-limits dispatch before running expensive verbs.

While the session is locked, IPC also rejects focus/launch/clipboard/capture/
workspace/session-control and remote-input inject commands. Desktop sharing also
pauses RDP listen until unlock (see Remote desktop above).

**Native libraries.** GTK, lcms2, OpenSSL/system TLS, libinput, and similar C
dependencies are outside Rust’s safety model — rely on distro security updates
for those packages (see [`PACKAGING.md`](PACKAGING.md)).

### Nested dev sessions (GNOME / host compositor)

When Metis runs inside another desktop (the default `./run-metis.sh --session`
winit window), the **host grabs Super** for its own shortcuts. Metis shortcuts
won't fire with Super unless you reconfigure the host.

**Default:** nested sessions may set `METIS_MOD=alt` so first-run defaults use Alt
instead of Super — e.g. **Alt+1** switches workspace. Prefer **Settings → Keyboard
→ Shortcuts** to set the Metis modifier and individual chords (`keybinds.json`).
**`Super`+`Alt`+←/→`** (workspace cycle) defaults to the logo/Windows key plus
**Alt**. **Click the Metis session window first** so it has keyboard focus.

Override with `METIS_MOD=super` or `METIS_MOD=ctrl` only when no `keybinds.json`
mod preference is set yet. On a real Metis session, the default modifier is Super.

| File | Purpose |
|------|---------|
| `bar.json` | Edge bar position/size/opacity/blur, widget order, workspaces, borders, default layout |
| `clock.json` | World clocks and alarms |
| `calendars.json` | Calendar accounts (no passwords — secrets in Keyring / Secret Service) |
| `themes/dark.json`, `themes/light.json` | Design tokens — accents, semantic colors, `text_on_accent`, shadows/glows |
| `config.json` | Active theme, onboarding state, briefing-on-login |
| `menu.json` | App launcher layout style, feature toggles, terminal / file-manager defaults, and pinned apps |
| `datetime.json` | Auto-timezone preference and calendar first day of week |
| `wallpaper.json` | Background picture / colour / gradient, plus per-output overrides; live crossfade. Decode cache: `~/.cache/metis/wallpaper-rgba/` |
| `weather.json` | Bar weather: unit, auto-detect, IP-geolocation, saved locations |
| `desk.json` | Compositor window-grid layout (widget tiles) |
| `desktop-widgets.json` | Wallpaper desktop widgets: enable, edit mode, chrome, instances |
| `dismissed.json` | Dismissed calendar reminder IDs |
| `briefing.json` | Login-briefing weather coordinates + RSS feed (optional) |
| `input.json` | Mouse, touchpad, and keyboard layout/repeat (compositor live-reload) |
| `keybinds.json` | Desktop shortcuts (chords → actions); browse in Settings → Shortcuts, edit under Keyboard → Shortcuts |
| `power.json` | Power profile, idle blank/suspend timeouts, lid-close action, dim-on-battery (compositor overlay) |
| `startup.json` | Session startup apps: master enable + desktop ids (empty by default; Settings → Startup) |
| `remote.json` | Desktop sharing: enabled, backend (`gnome_rdp` default / `rustdesk`), auto-start, LAN-only + firewall state |
| `dashboard.json` | Control Center: enabled, widgets, height %, refresh, confirm-before-kill, process monitor |
| `gaming.json` | Graphics mode, on-battery iGPU preference, auto performance/GameMode, Flatpak GPU env, `extra_steam_paths`, Metis MangoHud/Gamescope toggles |
| `gaming-flatpak.json` | Record of applied Flatpak gaming overrides (managed by `metis-gaming`) |
| `game-rules.json` | Float / fullscreen rules for games and launchers (built-in defaults if absent) |
| `outputs.json` | Per-output scale, resolution/refresh, arrangement (`layout_x`/`layout_y`), `display_mode` / `mirror_source`, VRR / HDR toggles, night-light prefs |

### Key `bar.json` fields

| Field | Meaning |
|-------|---------|
| `position` | `top` / `bottom` / `left` / `right` |
| `height` / `width` | Bar thickness on the long / short axes |
| `margin_top` / `margin_h` | Gap from the anchored edge / along the edge |
| `full_width` | Legacy full-edge stretch (`length_percent` preferred) |
| `length_percent` | Along-edge length 40–100% (centered; 100 = full edge) |
| `opacity` | Bar background opacity (`< 1` = see-through) |
| `menu_opacity` | App launcher panel opacity |
| `blur` / `blur_radius` | Compositor backdrop blur behind the bar |
| `bar_fill` | Pill fill: theme / solid / gradient + `gradient_direction` |
| `auto_hide` / `auto_hide_peek_px` | Auto-hide to a peek strip; bar **overlays** windows (no maximize reflow on show/hide). Move to the peek edge to open; top-bar title controls use the band just below the peek |
| `window_gap_px` | Padding around maximized / edge-snapped windows (0–10) |
| `displays` | Show the bar on all displays or the primary only |
| `workspace_mode` | Workspaces independent per display, or linked across displays |
| `default_layout` | Default workspace layout: grid or scrolling (live global switch) |
| `titlebar_opacity` | Window titlebar background opacity |
| `titlebar_pill_border` / `window_border` / `bar_border` | Border style (accent gradient / solid / custom gradient + width) |
| `widgets` | Ordered list of bar widgets |
| `taskbar_pinned` | Apps pinned to the dock |

Prefer the Settings app for most of these — it writes the same files and applies
changes live.

---

## 11. Troubleshooting

| Symptom | Try |
|---------|-----|
| Settings window shows desktop wallpaper through the body, or scrolling hitchs | Rebuild/restart Settings — page chrome is opaque; see [`CHANGELOG.md`](../CHANGELOG.md) 2026-07-26. Modal password/widget sheets intentionally stay transparent outside the rounded card. |
| Settings scroll locks for seconds on Windows / Display (GTK 4.22, hybrid NVIDIA) | Reopen Settings after a rebuild (2026-09-19): Settings defaults to Cairo GSK; Display IPC is async. Override renderer with `METIS_SETTINGS_GSK_RENDERER=gl` only for testing |
| Settings window closes but `metis-settings` still runs | Rebuild/reinstall Settings (2026-09-19 quit-on-close). Closing the window should exit the process; confirm with `pgrep metis-settings`. Category sheet **X** only returns to Home |
| Terminal floods with Gtk theme parser warnings when Settings opens | Harmless: Settings loads the shared shell stylesheet, which uses some CSS GTK’s engine skips (`max-width`, `color-mix`, etc.). UI still works |
| Background picker feels sluggish with many system wallpapers | Use the page controls / filters (2026-09-19); thumbs load async and cache. Prefer **Metis** or **Your pictures** if you do not need distro images |
| Wallpaper flashes black / takes seconds before the fade when changing Background | Rebuild/reinstall compositor + Settings (2026-09-19): soft hold + crossfade; RGBA cache under `~/.cache/metis/wallpaper-rgba/`. Prefer a release build. First browse of a page warms the cache |
| Bottom edge bar freezes while Settings (or another maximized window) is open | Rebuild/reinstall compositor (2026-09-19): maximized reclamp no longer storms configures from CSD shadow bbox overflow |
| Missing layer-shell | Install `libgtk4-layer-shell-dev` (26.04 / Debian 13), or build from source on 24.04 |
| Maximized title controls unusable (top auto-hide bar) | Move the pointer just **below** the thin peek strip to reveal the titlebar; the absolute screen edge opens the edge bar instead |
| Bar or popovers don't appear | Confirm a Wayland session (`echo $WAYLAND_DISPLAY`) and that `libgtk4-layer-shell` is installed |
| Electron app (e.g. Claude Desktop) opens then immediately closes | Metis launches Electron/Chromium apps on native Wayland by default (`ELECTRON_OZONE_PLATFORM_HINT=auto`, and `CLAUDE_USE_WAYLAND=1` for Claude), which is stable; their XWayland path can quit on launch. Re-login after `./run-metis.sh --install-session` so the session env applies. To force XWayland for one app, launch it with `ELECTRON_OZONE_PLATFORM_HINT=x11` (or `CLAUDE_USE_WAYLAND=0`) |
| GitHub Desktop / Electron takes minutes to open (every launch) | Exited app processes were left as zombies under the compositor; Chromium ProcessSingleton then blocks notifying that PID. Fixed by reaping all clients each tick — rebuild/reinstall the compositor session. Until then, log out of Metis (or restart the session) to clear zombies (`ps -ef \| grep defunct`) |
| GitHub Desktop / Electron logs `Failed to read color-scheme` (`dark_mode_manager_linux.cc`) | Rebuild/restart `metis-portal` (session reinstall). Confirm the portal returns uint32: `busctl --user call org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop org.freedesktop.portal.Settings Read ss org.freedesktop.appearance color-scheme` should print `v v u …` (not `i`). See [`CHANGELOG.md`](../CHANGELOG.md) 2026-07-25 |
| Apps slow to open / black screen on login | Ensure portal files are installed (`./run-metis.sh --install-session` or rebuild with `--session`); see [`CHANGELOG.md`](../CHANGELOG.md) 2026-06-28 |
| Screenshot / Flameshot fails | Re-login after `./run-metis.sh --install-session`; run `metis-portal --capture-test /tmp/test.png` to isolate portal vs app issues; grant the first-time portal permission |
| Flatpak app won't start / no Wayland | Install `flatpak` + portal packages; ensure app has `socket=wayland` (`flatpak info --show-permissions …`) |
| Flatpak app missing from the launcher | Metis adds the Flatpak `exports/share` dirs to `XDG_DATA_DIRS` at session start — re-run `./run-metis.sh --install-session` and log out/in if you installed the session before 2026-07-03. Verify with `echo $XDG_DATA_DIRS \| tr ':' '\n' \| grep flatpak` inside the session |
| Flatpak game: no controller | `flatpak override --user --device=all <app-id>`; confirm user is in `input` group |
| Steam / Proton game black screen or wrong GPU | Install 32-bit Vulkan (`i386` + `mesa-vulkan-drivers:i386`). Metis auto-forwards its render GPU to clients and auto-offloads game/Steam launches to a discrete GPU when present (`METIS_GAME_GPU` = igpu, dgpu, or off). Per-game, override with `DRI_PRIME=1 %command%` / `prime-run %command%` (or NVIDIA offload vars). Session-wide, set `METIS_DRM_DEVICE=/dev/dri/cardN`; disable fullscreen optimizations per-game |
| Game window borderless and clipped off-screen | Rebuild/reinstall compositor (2026-08-08 placement fix). Prefer exclusive fullscreen in-game, or `Super`+`Shift`+`F`. Adjust `game-rules.json` if a title should not auto-fullscreen |
| Steam splash expands/shrinks and never loads | Fixed 2026-08-08: launcher windows are excluded from game borderless re-placement. Rebuild/reinstall and restart the Metis session; `pkill steam` if a stuck client remains |
| Proton game: keys dead but mouse works | Re-login after `./run-metis.sh --install-session` (2026-07-04 XWayland keyboard-focus fix). Click the game window so it holds focus; confirm Steam is not popping over the game (focus-stealing prevention is in place) |
| Proton game: menu clicks open wrong item / only Settings | Prefer unlocking the pointer for menus (Esc / game UI). Locked clicks stay at the lock anchor (Mutter/KWin); hint remapping was removed 2026-07-19 because it broke mouse-look. Filter logs with `rg 'game-pointer' ~/.local/state/metis/logs/session-latest.log` |
| Proton game: cursor jumps on left/right click while aiming | Fixed 2026-07-19: re-arm inactive locks; do not remap locked clicks through `cursor_position_hint`. Rebuild/reinstall compositor and re-login. Verify `is_locked=true` on fire and no `click remapped` lines in session logs |
| Proton game: mouse-look stops at window/screen edges | Use exclusive fullscreen so XWayland can keep a lock; windowed absolute cursor clamps at edges when unlocked |
| Steam tray Quit / Exit does nothing | Fixed 2026-07-04 (dbusmenu label re-resolve). Rebuild shell and reinstall session |
| Steam overlay (Shift+Tab) missing | Click the game window so it holds focus (Metis is click-to-focus, no focus-follows-mouse). Most reliable on XWayland/Proton titles; some native-Wayland games draw the overlay differently |
| Big Picture button missing from menu | The rail shows it only when Steam is installed — native `steam` on `PATH` or the `com.valvesoftware.Steam` Flatpak. Install Steam and reopen the menu |
| Session sleeps during game | The idle-inhibit portal is implemented — video players, games, and browsers that request `org.freedesktop.ScreenSaver`/`PowerManagement.Inhibit` (or the Wayland idle-inhibit protocol) suspend blanking automatically. If something still sleeps, that app isn't requesting an inhibit; extend the timeout in Settings → Power, or confirm the inhibit reached the compositor |
| gdbus request path "does not exist" | Portal request objects are ephemeral — trigger a fresh `Screenshot` call; use `gdbus monitor --session --dest org.freedesktop.portal.Desktop` *before* the call to see the `Response` signal |
| Bluetooth shows stale battery | Many devices only refresh over BT on reconnect; install **Solaar** for Logitech charging state, or use a Unifying/Bolt receiver |
| Session won't start / behaves oddly | `./run-metis.sh --stop` then `./run-metis.sh --build --session` |
| Theme looks wrong | Delete `~/.config/metis/themes/*.json` and restart to regenerate, or use **Settings → System → Reset** |
| Want factory defaults | **Settings → System → Reset** (backup first); then log out and sign back in |
| Verify the shell is reachable | `./run-metis.sh --verify` |
| Compare compositor vs shell grid | `./run-metis.sh --verify-grid` |
| Remote desktop toggle greyed out | Install `gnome-remote-desktop`; set a password on **Settings → Remote access** before enabling |
| LAN firewall not applied / Retry times out | Install `nftables` (or active `ufw`); ensure `metis-polkit-agent` is running (`ps` / session logs); use **Retry firewall apply** under Security, or `pkexec metis-remote firewall apply`. On polkit 127+, confirm `systemctl is-active polkit-agent-helper.socket` |
| PolicyKit password dialog missing / auth fails | Confirm `metis-polkit-agent` is running; kill competing GNOME/KDE agents if register fails. Helper path: socket `/run/polkit/agent-helper.socket` or setuid `polkit-agent-helper-1` |
| User list shows default icon instead of DE picture | Metis reads `~/.face`, `~/.face.icon`, then `/var/lib/AccountsService/icons/<username>`. Set a picture in Settings → Users or ensure the AccountsService icon exists and is world-readable |
| RDP connects but screen is black | Confirm you are on a DRM session (not nested dev); unlock if the session is locked; check `metis-remote status` and PipeWire/portal stack |
| `metis-remote` not found | Package may be missing — `dpkg -l metis-desktop` and reinstall with `sudo apt install ./metis-desktop_*.deb`. Dev trees: `./run-metis.sh --install-session` |
| Metis Viewer: `cliprdr_… failed` / instant disconnect | Update Viewer (clipboard channel disabled in spawn). **Do not RDP into the same session from itself** — connect from another machine (e.g. the KVM host → guest IP) |
| `.deb` upgrade removed Metis / left nothing installed | Use `sudo apt install ./metis-desktop_….deb` from a terminal after logging out of Metis — not Ubuntu Software / App Center. Then `sudo apt-get install -f` if needed. See [`PACKAGING.md`](PACKAGING.md) |
| `apt upgrade` replaced Metis with a tiny “graph partitioning” package | Name collision: Ubuntu’s `metis` ≠ Metis desktop. Remove it (`sudo apt remove metis`) and install `metis-desktop_*.deb`. Fixed by renaming the package |
| From-source install on Ubuntu/Debian/Arch | Use repo-root `./install.sh` (see [`PACKAGING.md`](PACKAGING.md)) |

Logs are written to `~/.local/state/metis/logs/` (`latest.log` points at the most
recent run).

---

Questions, roadmap, and recent changes: see
[`../metis-os-workspace/TODO.md`](../metis-os-workspace/TODO.md) and
[`../CHANGELOG.md`](../CHANGELOG.md).
