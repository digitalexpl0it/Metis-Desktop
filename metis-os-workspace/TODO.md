# Metis Shell — Edge Bar (v2)

**Current phase:** Phases **1–17** are complete for their shipped product bars
(Phase 17 Task View / Super+Tab shipped).
**Phase 18** (security / IPC / isolation polish) **A–D complete** 2026-08-08;
§E remains track-only (colour management UAF + MultiRenderer) — see below.
**Phase 19** (Gaming Setup UX — guided drivers + first-run polish) shipped
2026-08-08 — see Phase 19 below.
**Phase 16** (Engineering hardening) closed 2026-08-02 — PR CI quality gate,
trust-boundary tests, compositor panic triage, portal coverage, `cargo-deny`,
command-file allowlist, PERF_AUDIT refresh, shell poll D-Bus path, packaging CI
alignment. **Phase 15** (Security hardening) closed 2026-07-25 — supply-chain /
compiler, spawn + Polkit, lock/VT, capability IPC, opt-in XWayland isolation.
**Phase 15 §F stretch complete 2026-07-26** (first-party viewer, RustDesk preset,
image RDP clipboard, lock fingerprint / YubiKey niceties). **`ext-session-lock-v1`
shipped 2026-07-27** (optional third-party lockers; Metis PAM lock stays default).
**Phase 3** multi-GPU hardware validation closed 2026-07-26 on hybrid
iGPU+dGPU (HDMI projector, gaming PRIME, input). **Phase 5** (Display pipeline)
closed 2026-07-25 with HDR H1–H3 and Stage 2 GLES 3D-LUT colour; 2026-07-26
stretch added BT.2020 encode + HLG path and VT LUT invalidate. Default-on
`wp_color_management_v1` remains deferred (upstream wayland-rs **server/sys**
ObjectData UAF — see Phase 5 §B). **Phase 8** (i18n) complete 2026-07-24.
**Phase 7** (remote access security closeout) complete 2026-07-25. See
individual phase sections for deferred follow-ups.

---

## Optional stretch (Waves 1–4)

Sequenced leftover stretch after Phases 1–15. See plan *Optional stretch backlog*.

### Wave 1 — Dim on battery
- [x] Compositor overlay from `power.json` → `dim_on_battery` + AC/battery poll
- [x] Skip when HDR-active; live-reload; USER_GUIDE / CHANGELOG

### Wave 2 — Widget §E.2 helpers
- [x] Pack `helper` manifest + argv-only spawn + `{helper.*}` + example pack
- [ ] In-process scripts / WASM (explicitly deferred)

### Wave 3 — Graphics
- [x] **3a** Primary→secondary transfer for hybrid outputs (CPU readback path;
      local+no-blur fallback). Full `MultiRenderer` element typing later.
- [ ] **3b** Default-on `wp_color_management_v1` — **blocked** on upstream
      wayland-rs server/sys ObjectData UAF; keep `METIS_COLOR_MGMT=1` opt-in
      ([docs/upstream/](../docs/upstream/README.md))
- [x] **3c** Per-surface HDR pass-through into encode path (PQ/HLG hints;
      mixed SDR+HDR approximate). Fuller tone-map matrix still welcome.

### Wave 4 — Remote
- [x] **4a** `metis-remote rustdesk status|enable|disable` + firewall/Polkit +
      Settings; GRD remains default
- [x] **4b** Decision doc only:
      [`docs/decisions/remote-host-native-vs-grd.md`](../docs/decisions/remote-host-native-vs-grd.md)

### Wave 5 — Session startup
- [x] `startup.json` — global enable + ordered desktop ids + per-entry enable/delay
- [x] Settings → System → **Startup** — master toggle, list, app picker (no custom CLI)
- [x] Compositor delayed one-shot spawn after shell (~2s); skip if onboarding incomplete
- [x] USER_GUIDE / CHANGELOG / config table
- [ ] Portal `Background.enable_autostart` — deferred (stub still returns false)

### Wave 6 — Edge bar polish
- [x] `bar_fill` — theme / solid / gradient + direction
- [x] `length_percent` — centered strip 40–100%
- [x] Auto-hide with peek + delay; force-show on popover / Control Center / NC
- [x] While auto-hidden: snap/maximize fill the edge; reseat on show
- [x] Settings Edge bar controls + USER_GUIDE / CHANGELOG

---

## Phase 1 — Edge bar

- [x] `bar.json` config — position (top/bottom/left/right), height/width, opacity, widget order
- [x] Live reload when `bar.json` changes (cosmetic edits apply in place; full
      rebuild only when widgets, position, displays, clock format, or workspace
      count change)
- [x] Bar position — top/bottom/left/right dropdown (Settings → Appearance → Edge bar);
      exclusive zone, pill flush-to-edge, and popover/menu open direction adapt per side
- [x] Distance from edge — slider for the gap between the bar and its anchored screen edge
- [x] Edge-bar border — `bar_border` (accent gradient / solid / custom gradient + width,
      0 disables); rounded gradient via layered `background-clip`, flows along the long axis
- [x] Workspace indicator — live virtual workspaces (click a dot or `Super`+`n` to switch)
- [x] Clock popover — tabbed: calendar, world clocks, stopwatch, timer, alarms
      *(superseded by Phase 13 Notification Center)*
- [x] World Clocks — inline searchable timezone picker (up to 3 zones)
- [x] Stopwatch with scrollable lap list
- [x] Timer — movable, always-on-top layer-shell HUD with pause/close
- [x] Alarms — segmented sound selector
- [x] Battery, network, volume indicators
- [x] Bluetooth bar widget — appears when an adapter is present; popover lists
      connected devices with battery level and charging icon (UPower / HID sysfs /
      optional Solaar for Logitech HID++; BlueZ fallback); low-battery alerts
- [x] Wi-Fi icon stability — holds the last known active network during rescans so
      the signal icon does not flicker offline
- [x] Notifications popup — badge, grouped duplicates + count badge, per-kind icons,
      clear-all with slide-out animation, scrollbar, in-bar alert routing
      *(list UI moved into Phase 13 Notification Center; optional bell widget remains)*
- [x] **Clipboard history widget** — edge-bar popover listing recent clipboard
      entries (text previews + image thumbnails); click a row to recall via
      compositor IPC; history persisted to `~/.local/state/metis/clipboard.json`
      (max 50 entries; 10 MB image cap; clear-history button)
- [x] WiFi / audio popover controls
- [x] Weather widget — icon + temperature with a forecast popover (see Phase 2);
      cached snapshot re-applied after bar rebuilds (no empty flash)
- [x] System tray — collapsed popover vs pinned-on-bar modes; readable tooltips
      (SNI title / bus-name fallback); light-mode pixmap/symbolic icon rendering
- [x] Removable volumes — USB / SD / optical / ISO icons left of tray; open in
      file manager; Mount / Unlock (LUKS) / Unmount / Eject via Gio VolumeMonitor
- [x] Light-mode bar popover styling — theme-token entries, buttons, and icon
      actions in dropdown panels (clock calendar, clipboard, network, notifications)
- [x] Bundled default wallpapers (`default.png` … `default9.png`) listed in
      Settings → Appearance background picker
- [x] Bar symbolic icons — bluetooth, clipboard, and notifications match other bar
      icons (GTK symbolic + theme text color)
- [x] Theme file watcher (live `themes/*.json` reload)
- [x] Freedesktop notification D-Bus daemon (`org.freedesktop.Notifications`)
- [x] Themeable token palette — accent + secondary accent, semantic status colors,
      `text_on_accent`, configurable shadows/glows drive the stylesheet
- [x] Bar transparency (`opacity`) + compositor backdrop blur (`blur`/`blur_radius`)
- [x] App launcher — ArcMenu-style popover off the brand icon: quick launchers +
      power actions rail, Frequent/alphabetical app list with apps-only search,
      and a pinnable apps grid; translucent panel (`menu_opacity`) with in-surface
      tooltips, dismissed synchronously on launch
  - [x] Dedicated **Settings · Metis Menu** page — Quick launchers (configurable
        Terminal / File-manager from auto-detected installs or a custom binary path,
        persisted to `menu.json`; shell falls back to env hint → known candidates →
        `xdg-open`) plus the menu panel opacity (moved out of the Edge bar card)
  - [x] Input robustness — compositor gives the bar popover top pointer priority so
        the app list scrolls even over a window behind it; click-outside (desktop or
        another window's titlebar) reliably dismisses; type-to-search works without
        clicking the search box first (`SearchEntry` key capture, no focus grab); and
        the scroll gutter/scrollbar stay flat in dark mode (GTK variant synced to the
        active theme)

---

## Phase 2 — Settings app + Window decorations

A standalone `metis-settings` GTK4 toplevel (the first real window-managed
client) with a sidebar and per-domain pages, plus compositor-drawn server-side
decorations so it (and every app) gets a real titlebar.

### Shared crates
- [x] `metis-config` — pure (serde + fs, no GTK) config + theme-token types and
      stylesheet builder, shared by the shell and settings app
- [x] `metis-secrets` — shared freedesktop Secret Service (`oo7`) wrapper

### `metis-settings` shell
- [x] New `metis-settings` binary — sidebar nav + content stack, `--page` preselect
- [x] Launch from the bar launcher icon and via `metis-cmd settings [page]`
- [x] **macOS-style sidebar** (2026-06-30) — grouped sections (Displays, Desktop,
      Connectivity, Input, System), coloured icon badges, page headers with
      subtitles, inset cards, sidebar search filter, Display as default page
- [x] Appearance page — Light/Dark style chooser, accent/secondary/semantic
      colors, opacity, blur + blur radius (writes `themes/*.json` + `bar.json`,
      live reload)
- [x] Appearance page — background picker with three types: Picture (bundled +
      imported grid, "Add Picture…"), Solid colour, and Gradient (start/end +
      direction); live switch via `ApplyBackground` IPC, persisted to
      `wallpaper.json`
- [x] Network page — wired/NIC config (DHCP vs static), Wi-Fi scan/connect/forget;
      bar "wired-only" network click opens this page
- [x] Network page — **VPN** tab (OpenVPN `.ovpn` + WireGuard `.conf` import,
      simple WireGuard create, connect/disconnect/delete via `nmcli`); dedicated
      edge-bar VPN icon + popover. Deferred: PPTP/L2TP/IPsec editors, full
      cert/MFA wizards, auto-connect-on-Wi-Fi
- [x] Calendars page — moved CalDAV/MS365 account config out of the clock popover
- [x] **Split Appearance page** — extracted Background, Edge bar, and Windows into
      separate sidebar pages under Desktop; Appearance now holds only theme mode,
      colours, and font. Per-display wallpaper lives on the new Background page
      (kept with the rest of the wallpaper controls rather than moved to Display).
      Shared plumbing sits in `pages/appearance_common.rs`; Edge bar/Windows each
      persist only their own `bar.json` fields via `update_bar`.

### Window decorations (server-side)
- [x] `zxdg_decoration_manager_v1` + `XdgDecorationHandler`; force SSD so GTK
      omits its client-side headerbar
- [x] Frame vs client geometry split — titlebar + border replace the old undrawn
      44 px grid tile-header inset
- [x] Compositor-drawn titlebar (title via `fontdue`), border, and close /
      minimize / maximize buttons
- [x] Decoration input — button actions + titlebar drag (`MoveSurfaceGrab`)
- [x] Border resize — compositor-side edge/corner hit-test (`RESIZE_MARGIN_PX`)
      starts `ResizeSurfaceGrab`, floats tiled windows out of the grid, persists
      the new geometry, and shows directional resize cursors on hover
- [x] Resize edge occlusion — auto-hide, maximized, and fullscreen windows block
      lower-window resize bands (client geometry + 12 px halo) without stealing
      titlebar/decoration input
- [x] Auto-hide titlebar pointer routing — revealed overlay chrome and permanent
      SSD border strips own pointer hits above the mapped client rect so hover and
      clicks cannot fall through to windows below
- [x] **Terminal right-click / primary-selection paste** — right/middle press
      syncs data-device + primary-selection focus before chrome handlers so
      context menus and middle-click paste work in kitty/foot on tiled, floating,
      maximized, and auto-hide-titlebar layouts
- [x] Per-app geometry memory — free-layout windows save their position/size per
      `app_id` to `~/.config/metis/windows.json` and restore it on reopen (placement
      defers until `app_id` is known so the restore isn't lost to a centered default)
- [x] Polish — rounded, anti-aliased control buttons with glyphs (× / + / −) and
      focus-aware dimming (traffic-light colors when focused, gray when not)
- [x] Snap zones / hot-spots — drag a window to a screen edge for a live
      translucent preview that drops it into half / quarter / maximize regions
      (`metis-grid::pixel_snap_target`, wired through `MoveSurfaceGrab` + winit
      overlay; computed against the usable area so the top zone clears the bar)
- [x] Theme-aware + translucent titlebar — palette follows the active light/dark
      theme (live ~1s poll), background opacity via `titlebar_opacity`, rounded
      top corners + a border that wraps under the titlebar; text/buttons stay opaque
- [x] Title pill border — flat solid pill plate ringed by a thin stroke on the
      focused window; configurable via `titlebar_pill_border` (accent gradient /
      solid / custom gradient + width) from Settings → Appearance → Windows
- [x] Window frame border — independent of the pill (`window_border`): accent
      gradient / solid / custom gradient, vertical top→bottom ramp, with a
      configurable thickness that also insets the client body (live-applied via a
      runtime `metis_grid::set_app_tile_border_px` + relayout)
- [x] Auto-hide titlebar — maximized and left/right/top-corner snaps hide the
      titlebar (client fills the footprint) and reveal it as a borderless
      translucent overlay on top-strip hover; oversized clients are re-anchored so
      the screen-edge gap survives
- [x] XWayland — spawn/manage an X11 server (`X11Wm`/`XwmHandler`) so X11-only
      apps run in the nested session alongside Wayland clients
- [x] **First-class XWayland windows** — X11 toplevels join the shared window
      registry via a `WindowSurface` enum, get a Metis server-side titlebar,
      bar-aware floating placement, move/resize/snap, focus/stacking, and
      dock/IPC events; override-redirect surfaces stay undecorated (2026-07-02).
- [x] **Electron apps prefer native Wayland** — the compositor's client-spawn env
      (and `metis-session` / `run-metis.sh --session`) set
      `ELECTRON_OZONE_PLATFORM_HINT=auto` + `CLAUDE_USE_WAYLAND=1`, since Electron's
      XWayland launch juggling is unstable under Metis (Claude Desktop "opened then
      closed"). Overridable per app (2026-07-02).
- [x] **Client xdg fullscreen** — Chromium / Firefox video fullscreen and other
      `xdg_toplevel.set_fullscreen` requests map the window to the output under
      the cursor (`fullscreen_request` / `unfullscreen_request` wired 2026-07-01).
- [x] **X11 / XWayland fullscreen** — `_NET_WM_STATE_FULLSCREEN` requests from
      X11 clients (e.g. Steam, legacy video players) map to the output the window
      is on and restore prior geometry on exit.

### Weather
Backend: **Open-Meteo** (keyless) — reuse/extend `briefing/connectors/weather.rs`
for `current` + `hourly` + `daily`, plus the Open-Meteo geocoding API for city
search. Attribution shown in the popover footer.

- [x] Bar widget — condition icon + temperature (matches reference screenshot)
- [x] Popover — current conditions (temp, label, H/L), hourly strip, saved
      locations list, attribution footer
- [x] Auto-detect location — IP geolocation (city-level, keyless ipwho.is) with
      an offline system-timezone (`zoneinfo`) fallback; 12s HTTP timeouts, cached,
      retries every 30s on failure
- [x] `weather.json` config (unit, `auto_detect`, `ip_geolocation`, locations)
- [x] Settings → Weather page — manual location override + search, multiple saved
      locations (reorder/remove), °F/°C unit toggle, IP-geolocation toggle

---

## Phase 2.5 — Taskbar / running-apps dock

A dock-style `tasks` widget on the edge bar, driven by live compositor window
state. Built for a single output now, but designed forward-compatible with the
Phase 3 per-output bars + per-output workspaces.

- [x] Protocol — `WindowInfo` gains `minimized`/`focused` (+ `output`/`workspace`
      placeholders for Phase 3); new `SetMinimized`/`ActivateWindow` commands and a
      `WindowMinimized` event; `WindowFocused` now also pushed on the event bus
- [x] Compositor — minimize/restore/activate by id (grid tiles routed through
      `set_tile_mode`, floating windows directly), focus emitted on pointer focus
- [x] Shell window-state cache (`services/windows.rs`) — folds the compositor event
      stream into a snapshot, seeded by `list_windows()` with a slow reconcile
- [x] Per-app grouping — windows grouped by resolved app identity (desktop id /
      `StartupWMClass`, case-insensitive `app_id` match, fallback icon)
- [x] Running indicator dot + focus highlight; minimized apps dimmed
- [x] Click — toggle focus/minimize (single window), window picker popover
      (multi-window), launch (pinned-but-not-running)
- [x] Right-click — pin/unpin (separate `taskbar_pinned` list in `bar.json`) and
      close window(s)
- [x] Overflow — horizontal scroll when the dock outgrows the bar

---

## Phase 3 — Multi-monitor, workspaces & tiling

Broaden window management beyond the single-output, single-desktop stub. Staged
so each milestone is shippable on its own:

- [x] **Virtual workspaces (single output)** — fixed-N workspaces on the one
      output, each its own set of tiled app windows with shared desk widgets.
      `SwitchWorkspace`/`MoveWindowToWorkspace` commands + `WorkspaceChanged`
      event; `Super`+`1..9` / `Super`+`Shift`+`1..9` keybinds; `Super`+`Alt`+`←`/`→`
      cycles workspaces in order (wraps at 1..=count); bar dots wired
      through IPC; `WindowInfo.workspace` populated. Count from `bar.json`
      `workspace_count`. (Per-output split comes with the refactor below.)
- [x] **Output-agnostic refactor** — remove the `space.outputs().next()` /
      single-monitor assumptions; thread an output id through placement, grid,
      snapping, decorations, and IPC. Per-output placement/snap routing, absolute-
      pointer mapping across outputs, `ListOutputs` IPC (name, geometry, primary
      flag, sorted left-to-right).
- [x] **Virtual outputs under winit** — `METIS_VIRTUAL_OUTPUTS=2` tiles the
      nested window into two side-by-side logical outputs (dedicated full-window
      render output + multi-output layer/blur/frame loop). The test rig for the
      rest of the refactor; cursor + cross-output drag now work on it.
- [x] **Per-output edge bar** — one bar per output (`gtk4-layer-shell`
      `set_monitor`), rebuilt on monitor hotplug; `bar.json` `displays`
      (`all`/`primary`) + Settings control to limit it to the primary output.
- [x] **Per-output state** — each output owns its own usable area, grid, and
      wallpaper. Per-output wallpaper — each display is cover-cropped to its
      own resolution and can carry its own picture via `wallpaper.json` `per_output`
      (Settings · Appearance · Per-display background; outputs discovered via the
      `ListOutputs` IPC); per-output usable area drives floating placement,
      snapping, and maximize; per-output grid/tiling landed with per-output
      workspaces below; per-output dock landed with "Taskbar follows" below.
- [x] **Per-output workspaces** — Hyprland-style: each output owns an independent
      set of workspaces, its own active workspace, and its own grid of app
      windows. Compositor state is now per-output (`OutputDesk` per output: grid +
      active workspace + stashed tiles); windows are tagged with their output and
      map only while their output's active workspace matches. `Super`+`n` /
      `Super`+`Shift`+`n` act on the output under the pointer; `SwitchWorkspace` /
      `WorkspaceChanged` carry an output id; each per-output bar drives and
      reflects its own output's workspaces (matched via the GDK monitor connector).
- [x] **Workspace mode toggle** — Settings → Appearance → Edge bar → Workspaces
      chooses `separate` (independent per output, default) or `linked` (every output
      switches to the same workspace at once). `bar.json#workspace_mode`; the
      compositor routes `Super`+`n` / `SwitchWorkspace` through `switch_workspace_routed`,
      fanning out to all outputs in linked mode (each emits its own `WorkspaceChanged`).
- [x] **Cross-output moves** — drag a window onto another monitor (or snap it
      there) and its desk tile + scroll membership follow automatically
      (`maybe_adopt_window_output` on drag-drop / snap). `Super`+`Shift`+`←`/`→`
      moves the focused window to the adjacent output on grid workspaces
      (scroll mode keeps those keys for column moves). `MoveWindowToOutput` IPC.
      Move a whole workspace with `Super`+`Ctrl`+`Shift`+`←`/`→` (independent
      per-output mode) or `MoveWorkspaceToOutput` IPC.
- [x] **Automatic dynamic tiling** — grid workspaces auto-split among open app
      windows on open/close and workspace switch (below desk widgets; master stack
      for three; focused window gets the primary slot)
- [x] **Scrolling layout option** — niri/PaperWM/paneru-style horizontally
      scrolling workspace, selectable per-workspace as a second mode in `metis-grid`
      (`scroll.rs`: `ScrollState` of columns, each a vertical window stack). App
      tiles stay the membership/stash source of truth; scroll mode only overrides
      pixel placement + hit-testing. Toggle live with `Super`+`\`; settings default
      for new workspaces via `bar.json#default_layout`. Keybinds (scroll workspace
      only): `Super`+arrows focus, `Super`+`Shift`+arrows move, `Super`+`,`/`.`
      consume/expel a window into/out of a column stack, `Super`+`-`/`=` snap the
      column to full/half width. `SetWorkspaceLayout` IPC.
  - Full-height columns with **continuous, mouse-resizable widths** (drag the
        right border to grow a window and push the rest over; left border resizes
        the previous column — `ScrollResizeGrab`). Width is a fraction of the
        viewport, not fixed presets.
  - Opening a window appends a column and **never resizes existing windows**;
        the strip reflows on open/close so columns slide into place.
  - Off-screen columns are clipped to their own output (`CropRenderElement`) so a
        column scrolled past the edge never bleeds onto an adjacent monitor; fully
        off-screen columns stay unmapped; scroll offset is clamped to strip width.
  - Viewport pan **eases** toward the focused column (`advance_scroll_animation`);
        client surfaces stay mapped at the target offset and render with an X nudge
        (`scroll_x_target - scroll_x`) so resize-averse clients are not reconfigured
        every frame.
- [x] **Taskbar follows** — each output's dock shows only the windows on that
      output's currently-visible workspace (pinned launchers persist everywhere).
      `WindowInfo.output` carries the monitor name; the dock filters by
      `(output, active workspace)`, repaints on workspace switch, and dedups per
      bar. The "Per-output state" per-output dock item is now also covered.
- [x] **DRM/udev backend (real standalone session)** — DRM/KMS + libseat +
      libinput backend selected alongside the nested winit dev path
      (`METIS_BACKEND` / autodetect). Damage-gated per-output page-flips on the
      primary GPU's `GlesRenderer`, dmabuf global for EGL clients, libinput input
      with VT-switch / safe-quit / suspend-resume, software/HW cursor (XCursor +
      client surfaces), and live connector hotplug with output re-packing.
      Login-manager entry installable via `run-metis.sh --install-session`
      (`metis-session` + `metis.desktop`); `--session --drm` runs from a TTY.
  - [x] **Full multi-GPU device/output ownership** (2026-07-24) — Smithay
        `GpuManager<GbmGlesBackend<…>>`, all seat GPUs opened at startup/hotplug,
        and per-device scanners, output managers, render nodes, notifier tokens,
        and CRTC surface maps. Outputs render with their device's
        `single_renderer`; primary-GPU client buffers are early-imported.
        **Caveat (updated Wave 3a, 2026-07-27):** hybrid outputs try a
        primary→secondary framebuffer transfer (composite on primary with blur,
        present on secondary); on failure fall back to the local renderer with
        blur off. Full GLES `OutputStack` typing for Smithay `MultiRenderer`
        (dmabuf path without CPU readback) remains a later stretch.
        Wallpaper/decoration GL caches are invalidated on renderer-context
        switches. **Hardware validation (2026-07-26):** hybrid iGPU+dGPU laptop
        — HDMI projector output, gaming PRIME offload (fast), pointer/input
        stable.
- [x] **Settings portal (`org.freedesktop.portal.Settings`)** — `metis-portal`
      serves color-scheme (`u` uint32 per xdg-desktop-portal spec), gtk-theme,
      and empty decoration/button layouts from `metis-config` so GTK /
      Electron clients pick up Metis light/dark prefs; registered via
      `metis.portal` + `metis-portals.conf`, started by the compositor before
      `xdg-desktop-portal` in the DRM session. (2026-07-25: fixed ashpd signing
      color-scheme as `i`, which broke Chromium `DarkModeManagerLinux`.)
- [x] **Screenshot portal** — compositor exposes
      `ext-image-copy-capture-v1` / `ext-image-capture-source-v1`; `metis-portal`
      serves `org.freedesktop.impl.portal.Screenshot` via a native Wayland capture
      client (SHM buffer + PNG encode). Compositor retains capture `Session` objects
      for the client lifetime. Verified with Flameshot via `xdg-desktop-portal`.
      Registered in `metis.portal` + `metis-portals.conf`.
- [x] **ScreenCast portal (live streaming)** — `metis-portal` registers
      `org.freedesktop.impl.portal.ScreenCast`; persistent
      `ext-image-copy-capture-v1` session + ~30 Hz frame pump pushes frames into
      a PipeWire output stream. DRM sessions advertise dmabuf capture and the
      portal prefers GBM/`SPA_DATA_DmaBuf` with MemFd fallback (2026-07-24).
- [x] **ScreenCast dmabuf zero-copy** — compositor advertises
      `DmabufConstraints` and renders into client dmabufs (no GLES readback);
      portal allocates via linux-dmabuf + GBM and pushes `SPA_DATA_DmaBuf` when
      the peer accepts it, otherwise MemFd from a linear mmap (2026-07-24).

---

## Phase 4 — System settings expansion

Grow `metis-settings` from appearance / menu / weather / network / calendars into a
proper control center. Most pages need real device or service backends (libinput,
D-Bus services, PipeWire) that only work under the DRM/udev session — under the
nested winit dev session they degrade to read-only or no-op. Group the sidebar into
**Input**, **Devices**, and **System** sections as it grows.

### Input devices (libinput / xkb)
Compositor applies device config from a new `input.json`; settings writes it and the
compositor live-reloads (mirrors the `bar.json` watcher pattern).

- [x] **Mouse** — pointer speed / acceleration, acceleration profile (flat vs
      adaptive), natural scroll, primary button (left/right-handed), scroll
      wheel speed multiplier; compositor applies via libinput + axis scaling from
      `input.json` with live reload
- [x] **Touchpad** — tap-to-click, tap-and-drag, natural scroll,
      disable-while-typing, pointer speed + acceleration profile, scroll speed;
      stored in `input.json`, applied when a touchpad is present
- [x] **Keyboard** — layout(s) + variant, repeat delay + rate, compose key,
      Caps/Esc/Control remap via xkb options; applied via Smithay xkb config +
      `wl_keyboard` `repeat_info`

### Devices (D-Bus services)
- [x] **Bluetooth** — adapter on/off, scan (toggle + auto-stop), pair / connect /
      trust / remove, battery level and charging state where reported; via BlueZ
      (`bluetoothctl`) with UPower and optional Solaar (Logitech HID++) overlays.
      Bar indicator appears only when an adapter is present
      (`BarWidgetId::Bluetooth`); popover lists connected devices with battery
      icons; low-battery in-bar alerts (≤20%, hysteresis, suppressed while
      charging)
- [x] **Printers** — list queues via CUPS (`lpstat`); launcher for
      `system-config-printer` / CUPS web UI

### System
- [x] **Power / Battery** — power profiles (power-saver / balanced / performance) via
      `powerprofilesctl`, battery details via sysfs, idle blank via compositor +
      suspend/lid via logind (best-effort); persisted to `power.json`; battery widget
      links to Power settings; **Connected devices** section lists Bluetooth peripherals
      with battery + charging status. **Follow-up:** wire **Dim on battery** to compositor.
- [x] **Sound** — output / input device selection via `pactl` default sink/source;
      volume readout on the settings page (bar volume widget unchanged)
- [x] **Display (settings UI)** — Settings → Display: monitor picker chips, per-output
      scale/enabled, night-light prefs in `outputs.json`; scale and enable/disable
      apply live via `ReloadOutputs` IPC + compositor file poll. Resolution /
      refresh and multi-monitor arrangement are Phase 5 below; night-light
      compositor pipeline remain open.

---

## Phase 5 — Display pipeline (mode-setting, HDR / VRR / colour)

Advanced output features, all gated on the real DRM/udev backend (deferred in
Phase 3) — none of these are possible under the nested winit dev session.

### A. Display settings & mode-setting (Settings → Display)

- [x] **Wire `outputs.json` scale to the compositor** — `ReloadOutputs` IPC +
      ~1s file poll; fractional scale applied live; window reflow + wallpaper relayout
- [x] **Per-output enable/disable** — honour the Display page “Enabled” switch
      (unmap output, reclaim desktop space, evacuate windows to another display)
- [x] **Display picker UX** — basic monitor chips + per-display panel (make/model
      from EDID when available); refresh list on hotplug / manual refresh
- [x] **Resolution & refresh rate** — `ListOutputModes` IPC, DRM mode list in
      Settings, saved to `outputs.json` (`mode_width`/`mode_height`/
      `mode_refresh_millihz`), applied via `DrmOutput::use_mode` on
      `ReloadOutputs`; Windows-style keep/revert confirmation on save
- [x] **Multi-monitor arrangement** — draggable canvas preview (multi-monitor
      only; single output is preview-only), `layout_x`/`layout_y` in
      `outputs.json`, **Save display settings** + 15 s keep/revert dialog
- [x] **Mirror / clone displays** — Settings **Duplicate displays** toggle +
      **Show on** source picker; `display_mode` / `mirror_source` in
      `outputs.json`; DRM compositor renders source once per frame and
      scale-to-fits (letterbox) onto every enabled output; arrangement canvas
      hidden while duplicating; nested winit ignores mirror prefs

### B. Colour pipeline

- [x] **VRR / adaptive sync** — per-output opt-in toggle in Settings → Display
      (**Adaptive sync** under Applies live); saved as `vrr_enabled` in
      `outputs.json`; compositor sets DRM `VRR_ENABLED` via Smithay on real DRM
      sessions when the connector advertises `vrr_capable`
- [x] **Colour management** — ICC paths saved per output in Settings → Display;
      compositor loads profiles from `outputs.json`. Stage 1 is live: the ICC
      `vcgt` calibration curves are parsed (`color_management/vcgt.rs`) and
      uploaded to each CRTC's hardware gamma ramp (`output_gamma.rs`), re-applied
      after outputs reload, connector bring-up, mode-set, and VT resume.
      Stage 2 GLES 3D-LUT shipped (see H3 below). `wp_color_management_v1` is
      hardened (no request can leave a `New<>` uninitialised / panic the session;
      description records are reclaimed on destroy) but remains **opt-in**
      (`METIS_COLOR_MGMT=1`): advertising the global still crashes the session
      with heap corruption (`malloc_consolidate(): unaligned fastbin chunk`),
      blanking the display.
      **Root-caused (2026-07-02):** reproduced deterministically (~4 s) in a
      nested `--session` under gdb with a Chromium client, matching the hardware
      signature. The abort is a use-after-free dropping a wayland `ObjectData`
      `Arc` inside `wayland-backend`'s **server/sys** `resource_dispatcher` —
      **not** Metis code — when Chromium destroys a `wp_image_description_v1` and
      reuses its freed id for a `wp_image_description_info_v1` in the same
      dispatch batch. No Metis `unsafe` runs in the crash trace.
      **Reconfirmed (2026-07-25 closeout):** crates.io `wayland-backend` **0.3.16**
      / `wayland-server` **0.31.14** only fix **client/sys** races (`server_impl`
      unchanged); Metis still locks **0.3.15** / **0.31.13** via smithay. Bumping
      alone will not clear the Chromium path. **Rejected ship workarounds:**
      default-on now; Chromium/Electron blacklist; disabling `get_information`;
      dropping `use_system_lib` only for colour management (whole-compositor
      backend change, unproven). **Decision:** keep the global **opt-in** until a
      **server-side** wayland-rs fix is in the lockfile and nested Chromium retest
      (~4 s) stays clean — then flip `color_protocol_enabled()` to default-on.
      Upstream:
      [Smithay/wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)
      (local copy:
      [`docs/upstream/wayland-rs-server-objectdata-uaf.md`](../docs/upstream/wayland-rs-server-objectdata-uaf.md)).
      **Follow-up when revisited:** ASAN build to pin the faulting allocation;
      after upstream fix, default-on gate + Chromium/Cursor retest.
      Mode-set / VT resume re-apply for CRTC gamma + HDR blobs is **wired**;
      VT also invalidates LUT/HDR GL atlases (2026-07-26). Hardware QA across a
      real mode-set/VT switch remains welcome.
- [x] **Night light schedule** — local-time From/To window in Settings → Display;
      compositor toggles the warm overlay inside the schedule while the master
      night-light switch is on
- [x] **HDR foundation (H1, 2026-07-24)** — detect HDR-capable DRM connectors
      (`HDR_OUTPUT_METADATA` **and** EDID ST.2084/HDR10); per-output `hdr_enabled` in
      `outputs.json` + Settings → Display toggle; apply/clear Colorspace +
      HDR10 static metadata blob + `max_bpc=10`; log negotiated 10-bit vs 8-bit
      primary-plane scanout format; skip night-light overlay while HDR is active.
      Nested winit / SDR eDP (DRM prop without EDID HDR) report `hdr_available=false`.
- [x] **HDR encode (H2, 2026-07-24)** — when `hdr_active`, DRM path composites
      SDR offscreen then blits through an sRGB→linear→ST.2084 PQ texture shader
      (BT.2408 203‑nit reference white) so enabling HDR visibly maps the desktop
      into the HDR10 signal (`hdr_encode.rs`). Falls back to SDR scanout if the
      shader/offscreen path fails. Identity CRTC gamma while HDR is active.
- [x] **HDR H3 / Stage 2 colour (2026-07-25)** — GLES 3D-LUT (`lcms2` bake,
      33³ atlas) for ICC sRGB→display gamut mapping; high-bit offscreen when
      available; unified LUT→PQ post-pass; skip CRTC `vcgt` when LUT owns the
      output. Opt-in `wp_color_management_v1` (`METIS_COLOR_MGMT=1`) advertises
      PQ/HLG/BT.2020 and stores parametric TF/primaries; **default-off** until
      upstream wayland-rs **server/sys** ObjectData UAF is fixed (0.3.16 does not
      fix it — see §B). **Phase 5 product scope closed** with this residual gate.
- [x] **HDR stretch (2026-07-26)** — Rec.709→BT.2020 in encode + BT.2020 mastering
      metadata / prefer DRM BT.2020 Colorspace; HLG EDID detection + HLG encode
      (EOTF=3) when HLG-only (PQ preferred when both); VT resume invalidates LUT/
      HDR GL and dirties ICC rebake. Mode-set/VT gamma+HDR re-apply is **wired**
      (hardware QA still welcome). **Still deferred:** default-on colour protocol
      ([wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)), true
      per-surface HDR decode.
---

## Phase 6 — Flatpak, Steam & gaming

Sandboxed apps (Flatpak), **Steam / Proton**, and native games share the same
Wayland session and **xdg-desktop-portal** stack. Metis does not ship gaming-specific
code today — apps work incidentally when the host has the right packages, portal
daemons, and device permissions. This phase makes Flatpak and **SteamOS-class gaming**
(first-class Steam client, Proton, controllers, optional Gamescope) explicit and
fills portal gaps launchers expect.

**Target:** a Metis session where you can install Steam (`.deb` or Flatpak), launch
Proton titles, use Steam Input / common controllers, and optionally wrap games in
**Gamescope** — the same building blocks Valve uses on SteamOS (minus replacing
SteamOS's own Gamescope gaming-mode session).

**Permissions model (three layers):**

1. **Flatpak manifest / overrides** — `socket=wayland`, `device=dri`, `device=all`
   (gamepads), `share=network`, etc. (`flatpak override --user …`).
2. **Portal runtime prompts** — screenshot, screencast, file access; persisted by
   system **`xdg-permission-store`** (not `metis-portal`).
3. **Metis portal backends** — only interfaces registered in `metis.portal` /
   `metis-portals.conf` (Settings, Screenshot, ScreenCast today; gtk default for
   file dialogs and notifications).

**Gamepads:** Wayland has no standard gamepad protocol on `wl_seat`. Games read
`/dev/input/event*` via SDL/Proton/evdev. The compositor seat is keyboard +
pointer only; libinput gamepad events are not forwarded. Flatpak games typically
need `--device=all` (or equivalent overrides), not a compositor gamepad driver.

### A. Flatpak session integration

- [x] **Host prerequisites** — documented in `docs/USER_GUIDE.md` (Flatpak apps
      and games) and `docs/UBUNTU_DEV.md`: `flatpak` + `xdg-desktop-portal` +
      `xdg-desktop-portal-gtk`, Flathub remote, `input`/`video`/`render` group
      membership, and how Metis surfaces Flatpak apps in the launcher via
      `XDG_DATA_DIRS` (2026-07-03).
- [x] **Session env** — `metis-session` (and `run-metis.sh --session`) now
      augment `XDG_DATA_DIRS` with the Flatpak export trees
      (`${XDG_DATA_HOME:-~/.local/share}/flatpak/exports/share`,
      `/var/lib/flatpak/exports/share`) before the activation-environment export,
      deduped and harmless when Flatpak is absent. A login-manager session often
      does not source `/etc/profile.d/flatpak.sh`, so this is what makes Flatpak
      apps visible at all (2026-07-03).
- [x] **App launcher** — Flatpak `.desktop` entries now appear alongside native
      apps in the launcher and dock. Discovery is via `gio::AppInfo` (already
      path-agnostic), so the session-env change above is the only requirement; no
      launcher code change was needed (2026-07-03).
- [x] **Window identity** — `AppEntry` now captures the `X-Flatpak` desktop key,
      and both the launcher (`resolve_entry_for_app_id`) and dock
      (`matches_app_id`) match a running window's `app_id` against it in addition
      to the desktop-id basename and `StartupWMClass` — so Flatpak windows
      reporting their reverse-DNS Flatpak id group and icon correctly (2026-07-03).

### B. Portal completeness for sandboxed apps

- [x] **Inhibit portal** — block idle blank while a game or media app holds an
      inhibit request (2026-07-02). Compositor now blanks the display (DRM `DPMS`
      off) after the `power.json` blank timeout and wakes on input; three
      inhibitor sources feed one count that suspends it — Wayland
      `zwp_idle_inhibit_manager_v1`, `ext_idle_notify_v1`, and the legacy D-Bus
      `org.freedesktop.ScreenSaver` / `org.freedesktop.PowerManagement.Inhibit`
      names owned by `metis-portal` (per-caller cookies forwarded over new
      `InhibitIdle`/`UninhibitIdle` IPC; crashed peers auto-released via
      `NameOwnerChanged`). Blank timeout live-reloads via `ReloadPower`. While
      inhibited the compositor also holds a **logind `idle` inhibitor**
      (`systemd-inhibit --what=idle --mode=block`) so auto-suspend is blocked too.
      Follow-up: **idle→session-lock** — ~~blocked on there being a lock
      screen~~ **done** (2026-07-02). Metis now ships a compositor-rendered lock
      screen (Option A) with PAM auth; the **Lock when the screen blanks** toggle
      in Settings → Appearance → Background → Lock screen wires idle-blank to
      `lock_session()`. See Phase 7 "Session lock".
- [x] **ScreenCast live pump** — OBS / Discord / browser share via PipeWire SHM
      pump (~1080p30). **Deferred:** dmabuf zero-copy perf pass (Phase 3 / Phase 7)
- [x] **Background / PowerProfile** — stub backends in `metis-portal` (2026-07-05):
      Background allows sandboxed background runs; PowerProfileMonitor mirrors
      `powerprofilesctl` for GIO clients. GameMode remains a standalone D-Bus service
- [x] **Permission UX docs** — `flatpak permission-show/reset`, override cookbook,
      portal file locations in `docs/USER_GUIDE.md` (2026-07-05)

### C. Controllers & input (host + Flatpak)

- [x] **Flatpak override guide** — override cookbook + controller notes in
      `docs/USER_GUIDE.md` (2026-07-05)
- [x] **Settings → Gaming** — read-only gamepad/touchscreen list, Steam detection,
      GPU hint; `metis-cmd settings gaming` (2026-07-05)
- [x] **libinput audit** — confirmed compositor does not EVIOCGRAB gamepad nodes;
      capability logging on device add; documented in `device_input.rs` + USER_GUIDE
      (2026-07-05)
- [x] **Touch** — `wl_touch` on seat for touchscreen Flatpak apps; lazy
      `seat.add_touch()` on libinput touch device (2026-07-05)

### D. Steam & Proton (SteamOS-class desktop gaming)

Steam on Metis runs as a **normal Wayland client** (native `.deb` or Flatpak
`com.valvesoftware.Steam`). Games launch as child processes with Proton/Wine;
most do **not** go through compositor gamepad protocols — they use evdev, Steam
Input, and SDL. SteamOS Desktop Mode uses KDE today; Metis aims to be a viable
alternative desktop with the same Steam/Proton stack.

- [x] **Native Steam (.deb)** — documented Valve repo install on Ubuntu/Debian
      (`USER_GUIDE.md` §Steam): 32-bit (`i386`), Vulkan, PipeWire/Pulse note,
      `steam-devices` udev rules
- [x] **Flatpak Steam** — documented `com.valvesoftware.Steam` from Flathub;
      pressure-vessel / `~/.var/app` layout; `--device=all` and `--filesystem`
      overrides; portal permissions (`USER_GUIDE.md`)
- [x] **Proton** — documented Proton Experimental / GE-Proton (ProtonUp-Qt) and
      common failures (missing i386 Vulkan, wrong GPU → new client-GPU default +
      `DRI_PRIME`, anti-cheat / ProtonDB). On-hardware Proton launch: verify on hw
- [x] **Gamescope (optional)** — per-game launch option
      (`gamescope -W … -H … -- %command%`) documented as nested compositor
      (existing snippet retained)
- [x] **Big Picture / `-gamepadui`** — Steam-gated app-menu rail button runs
      `steam -gamepadui` (or Flatpak equivalent); hidden when Steam is absent
      (`menu.rs` + `applications::steam_big_picture_command`)
- [x] **Steam Input & hardware** — documented Steam Controller / Deck / Switch Pro
      via Steam user-space mapping; confirmed Metis/libinput does **not** grab
      exclusive evdev on gamepad nodes (games read `/dev/input/event*` directly)
- [x] **Steam overlay** — audited: click-to-focus (no focus-follows-mouse) keeps
      Shift+Tab working; documented XWayland vs native-Wayland caveats
- [x] **Steam Remote Play / Link** — relies on the shipped ScreenCast portal +
      PipeWire pump (§B); host-side encode is hardware-dependent
- [x] **Power while gaming** — idle-inhibit wired end-to-end (Wayland +
      `ScreenSaver`/`PowerManagement` D-Bus + logind); documented Settings → Power
      performance-profile tie-in
- [x] **Client GPU steering** — compositor forwards its render node's PCI identity
      to spawned clients as `DRI_PRIME` + `MESA_VK_DEVICE_SELECT` (if-unset;
      `METIS_NO_CLIENT_GPU=1` opt-out), so hybrid laptops default to the right card
- [x] **Automatic dGPU offload for games** — `DgpuOffload::detect` PRIME-offloads
      Steam/Proton/game launches onto the discrete GPU when distinct from the display
      GPU; `METIS_GAME_GPU=igpu|dgpu|off` overrides (2026-07-04)
- [x] **Pointer lock & relative motion** — `zwp_pointer_constraints_v1` +
      `zwp_relative_pointer_v1` for mouse-look (2026-07-04). **Follow-up
      (2026-07-19):** Mutter/KWin-aligned path — re-arm inactive locks while the
      pointer stays over the surface (removed `ClientDeactivated` latch);
      `set_cursor_position_hint` restores the cursor on unlock only (no locked
      click remapping — Proton hint streams during look caused camera jumps on
      fire). Verified on The Mound (`steam_app_2569760`).
- [x] **Proton keyboard focus (XWayland)** — keyboard events routed through
      `X11Surface::KeyboardTarget` (`XSetInputFocus`); map-before-surface race
      fixed on first surface commit (2026-07-04)
- [x] **Steam tray Quit / focus stealing** — dbusmenu clicks re-resolved by label;
      focus-stealing prevention while a game is running (2026-07-04)
- [x] **On-hardware Proton smoke test** — MOUSE: P.I. For Hire (`steam_app_2416450`):
      title screen, in-game menu clicks, Esc/keyboard verified (2026-07-04)

### E. SteamOS & handheld compatibility (optional / stretch)

Running **Metis on SteamOS** (replacing Desktop Mode) or on Deck-class hardware
is a stretch goal — SteamOS is immutable/read-only and ships Gamescope for Gaming
Mode. Track compatibility either way:

- [x] **Steam Deck / handheld inputs** — documented in `USER_GUIDE.md` (SD reader,
      volume buttons, gyro via evdev → Steam Input); on-hardware verification not
      done (no Deck in test rig)
- [x] **SteamOS host notes** — documented (`USER_GUIDE.md`): read-only root,
      `steamos-readonly disable` caveat, one-session-compositor-at-a-time; marked
      experimental / unsupported
- [x] **Gamescope vs Metis** — documented roles: Metis = session compositor,
      Gamescope = optional per-game nested compositor (run one outer session, not
      both)

### F. Gaming polish (optional)

- [x] **gamemoded** — documented as a standalone D-Bus service
      (`com.feralinteractive.GameMode`); install `gamemode` and use
      `gamemoderun %command%`. No Metis portal/stub needed (a compositor
      performance-profile tie-in could be a later follow-up)
- [x] **Fullscreen / pointer confinement** — compositor implements
      `zwp_pointer_constraints_v1` + `zwp_relative_pointer_v1` (mouse-look lock,
      region confinement; cursor hints restore on unlock only as of 2026-07-19)
      (2026-07-04 / 2026-07-19)
- [x] **XWayland game notes** — documented XWayland vs native-Wayland caveats
      (overlay, Proton) in `USER_GUIDE.md`
- [x] **MangoHud / vkBasalt** — documented `%command%` prefix patterns in Steam
      launch options (no Metis code required)
- [x] **INVESTIGATE: in-game GPU performance regression vs GNOME/Mutter** — Hytale
      (native-Wayland) was only playable on **Low** under Metis while **High** ran
      smoothly on gnome-shell/Mutter on the same hardware. **Root cause:** the
      compositor kept drawing wallpaper, night light, and the theme cursor under
      fullscreen, repainted on every locked-pointer mouse move, and armed every
      output on each client commit — so fullscreen games never got a clean direct
      scanout path. **Fixed (2026-07-04):** skip wallpaper/blur/night-light under
      fullscreen, hide compositor cursor during pointer lock, stop
      `schedule_redraw` on locked-pointer motion, per-output commit damage
      (`render.rs`, `night_light.rs`, `input.rs`, `state.rs`,
      `handlers/compositor.rs`). **Verified on hardware:** Hytale **High** runs
      perfectly under Metis after the fix. Remaining if needed: profile Proton/
      cross-GPU PRIME titles separately (dGPU render → iGPU scanout).

---

## Phase 7 — Remote access (full desktop sharing)

**Status: complete for the GNOME RDP path (2026-07-25 security closeout).** Metis
hardens session sharing via `metis-remote` + `gnome-remote-desktop` + portal
clipboard/input. **Phase 15 §F:** first-party **viewer** + RustDesk Settings
preset shipped; host remains GRD. Still deferred: Metis-native host protocol;
optional `metis-remote` RustDesk backend. Deep per-app X11 isolation shipped as
Phase 15 §E opt-in.

Let you **remote into a Metis machine from another device** (laptop, tablet,
phone) with full interactive control — not just “share screen” in a call.
ScreenCast (Phase 3) covers **local video capture** for portal apps; remote
desktop also needs **remote input injection**, **network transport**, and
**session security**. Third-party servers (RustDesk, RDP) may work incidentally
today when installed on the host; this phase makes remote access a documented,
tested, and optionally Metis-integrated capability.

**Target:** from another machine on LAN or over the internet, connect to a Metis
session and see + control the desktop (windows, bar, games) with acceptable
latency and clear setup docs.

### A. Third-party remote desktop (document + verify)

- [x] **RustDesk** — documented host install, firewall ports, Wayland/portal
      capture notes, and Metis DRM verification checklist in `docs/UBUNTU_DEV.md`
      + USER_GUIDE appendix (2026-07-24). Optional `metis-remote` backend still TBD.
- [x] **RDP (gnome-remote-desktop headless)** — `metis-remote` orchestrates
      `gnome-remote-desktop-headless.service` + `grdctl --headless`; Settings →
      **Remote access** master toggle; `remote.json` + session autostart; USER_GUIDE
      + UBUNTU_DEV spike docs. **Video + input + text clipboard v1** (2026-07-05).
      **Deferred:** classic `xrdp` X11 login sessions (out of toggle scope).
- [x] **Other tools** — compatibility matrix in `docs/UBUNTU_DEV.md` for
      AnyDesk, Chrome Remote Desktop, TigerVNC / `wayvnc` (known-good /
      known-broken / not integrated) (2026-07-24).
- [x] **Settings → System → Remote access** — master switch, status card, password
      gate, install hint, copy connection address (`metis-cmd settings remote`)

### B. Metis-native / portal integration (longer term)

- [x] **Remote input path (v1)** — compositor injects EIS pointer/keyboard via
      `remote_input.rs`; pointer clicks sync selection focus at the click location
      so RDP focus and clipboard targeting match local behaviour (2026-07-05).
      **Follow-up:** multi-monitor edge cases, scroll/wheel polish, game pointer-lock.
- [x] **RDP clipboard bridge (v1)** — `metis-portal` Mutter session clipboard
      D-Bus (`EnableClipboard`, `SetSelection`, `SelectionRead`/`Write`); compositor
      `ClipboardChanged` events forwarded to active GRD sessions (2026-07-05).
      **2026-07-26:** image clipboard (`image/png|jpeg|bmp`) via portal shim;
      text path unchanged. **Follow-up:** format conversion when client asks for
      BMP but local store is PNG-only.
- [x] **ScreenCast as capture backend** — GRD RDP already consumes Mutter
      ScreenCast → `metis-portal` PipeWire (memfd BGRx). **2026-07-24:** multi-
      monitor `RecordMonitor(connector)` selects the matching `wl_output` by
      name for the live pump (portal + Mutter shim). **2026-07-24:** dmabuf
      zero-copy path (GBM capture + PipeWire `SPA_DATA_DmaBuf`, MemFd fallback).
- [x] **First-party remote viewer** (Phase 15 §F, 2026-07-26) — `metis-viewer`
      GTK connect UI spawns FreeRDP (argv-only); host remains GRD + `metis-remote`.
      Settings **Connect with Metis Viewer…**; recent hosts in `viewer.json`
      (no passwords). Viewer polish: early FreeRDP failure feedback, recent
      one-click connect/remove, dedicated icon.
- [x] **RustDesk Settings preset** (2026-07-26) — Settings → Remote access card:
      detect system/Flatpak, Open RustDesk, copy install instructions + ports/
      portal notes. Optional `metis-remote` RustDesk backend still TBD.

### C. Security & session policy

- [x] **Firewall / LAN-only defaults** — `remote.json` `lan_only: true` (default);
      Settings **Security** toggle; apply on enable / LAN-on while sharing
      (Retry under Security if needed); `metis-remote firewall apply|clear`
      (nftables preferred, ufw fallback, timed `pkexec`). Status reports
      `firewall_applied` / `firewall_backend` / last error (2026-07-25).
- [x] **Credential hygiene** — `metis-remote set-credentials <user>` reads the
      password from stdin; Settings pipes it (never on Metis argv). `grdctl` still
      requires argv (GRD limitation). Documented in USER_GUIDE (2026-07-25).
- [x] **Session lock** — local lock (2026-07-02) plus **remote pause** (2026-07-25):
      lock runs `metis-remote pause` (RDP disable, keep `remote.json.enabled`);
      unlock runs `resume` if still enabled. Capture/inject remain denied while
      locked. `ext-session-lock-v1` for third-party lockers shipped 2026-07-27.
- [x] **Multi-user / VT** — documented in `docs/UBUNTU_DEV.md` (2026-07-24):
      Metis is a single graphical session per seat; switching TTYs pauses the
      DRM session; remote RDP attaches to the active logged-in session only;
      multi-seat is unsupported.
- [x] **IPC hardening** (2026-07-24) — `$XDG_RUNTIME_DIR/metis` `0700`, sockets /
      command files `0600`, `SO_PEERCRED` same-UID check, refuse `/tmp/metis`,
      expanded lock-screen IPC denylist. Documented in USER_GUIDE / UBUNTU_DEV.
- [x] **Flatpak gaming optimize confirmation** — Settings modal + CLI
      `--yes` / `METIS_GAMING_OPTIMIZE_YES` gate.
- [x] **Calendar keyring cleanup** — delete CalDAV / M365 secrets on account remove.
- [x] **XWayland abstract socket** — `config.json` `xwayland_abstract_socket`
      default **false** (2026-07-25). Residual: one shared X11 server remains.
- [x] **Deeper X11 isolation** (Phase 15 §E, 2026-07-25) — opt-in
      `xwayland_mode: isolated` two-bucket prototype (default stays shared).
      XSECURITY research note in USER_GUIDE.

---

## Phase 8 — Internationalization (i18n / l10n)

**Status: complete (2026-07-24).** Metis UI strings route through gettext (shell /
settings) and Fluent (compositor); catalogs ship in `assets/locale/` and the
`.deb`. Live language Apply rebuilds Settings, the edge bar, Notification Center,
and desktop widgets. Remaining catalog gaps fall back to English msgids; native
translation review is welcome but not a phase gate.

Metis previously shipped **English (US) only** — all user-facing strings in the
shell (edge bar, launcher, popovers, notifications), the settings app, the lock
screen, and the compositor's on-screen text (titlebars, lock clock/labels) were
hard-coded English literals. This phase made Metis translatable and locale-aware.

**Target:** a Metis session that renders its own UI in the user's system locale
(with a manual override in Settings), falls back cleanly to English for missing
strings, and lays out correctly for RTL scripts — without per-string rebuilds.

### A. Foundations (decide the stack first)

- [x] **Pick the i18n toolchain** — **Hybrid:** GNU gettext for GTK
      (`metis-shell` / `metis-settings`); **Fluent** for `metis-compositor`.
      Documented in `docs/I18N.md`.
- [x] **`metis-i18n` shared crate** — locale resolve, `tr`/`trn`/`tr_ftl`,
      catalog roots, formatting helpers.
- [x] **Locale detection + override** — `locale.json` + Settings → System →
      **Language & region**.

### B. Extract & translate strings

- [x] **Audit and externalize hard-coded strings** — shell, settings, onboarding,
      bar overlays, NC, desktop widgets, splash/toast; remaining gaps use English
      msgid fallback via gettext.
- [x] **Catalog extraction + build wiring** — `scripts/i18n-extract.sh`,
      `scripts/i18n-compile.sh`; install under `/usr/local/share/metis/locale`
      and `/usr/share/metis/locale` (`.deb` / release CI).
- [x] **Translation workflow docs** — `docs/I18N.md`.

### C. Locale-aware formatting & layout

- [x] **Numbers / dates / times** — chrono localized formatting behind
      `metis_i18n::format_*` / `format_pattern`.
- [x] **RTL support** — GTK default direction; compositor lock text via
      `unicode-bidi` + `rustybuzz` probe.
- [x] **Fonts / CJK & complex scripts** — fontconfig + Noto CJK/Arabic fallbacks
      in decoration/lock font loader; documented in `docs/I18N.md`.

### D. Polish

- [x] **Per-string fallback** — gettext/Fluent fall back to English source /
      English Fluent bundle.
- [x] **Live language switch** — `reload-locale` force-rebuilds bar widgets,
      Notification Center, desktop widgets; Settings rebuilds in-place; compositor
      `ReloadLocale` for Fluent lock/SSD.

---

## Phase 9 — First-run setup wizard (onboarding)

A welcoming **first-login wizard** that runs once on a fresh install, greets the
user, and walks them through a few simple choices before dropping them at the
desktop. Implemented as a GTK4 layer-shell overlay in `metis-shell` (`ui/onboarding.rs`).

**Target:** on first session start (when `onboarding_complete == false`), the
shell presents a clean, modern, multi-step wizard; finishing (or skipping) sets
`onboarding_complete = true` so it never shows again. Re-runnable on demand from
Settings → Appearance ("Run setup again") or `metis-cmd.sh show-onboarding`.

### A. Shell & trigger

- [x] **Wizard surface** — centered layer-shell overlay (`Layer::Overlay`,
      namespace `metis-onboarding`); blocks interaction until completed or skipped;
      desktop visible behind for live preview.
- [x] **First-run gate** — read `onboarding_complete` after splash fade; launch the
      wizard when false, and call `mark_onboarding_complete()` on finish/skip.
      `METIS_NO_ONBOARDING=1` disables for dev. "Run setup again" in Settings
      Appearance re-triggers via `show-onboarding` runtime command.

### B. Steps (keep it short and friendly)

- [x] **Welcome** — branded greeting + intro to Metis.
- [x] **Theme** — Light / Dark toggle (Light default; live preview).
- [x] **Wallpaper** — bundled thumbnails only; live apply via compositor IPC.
- [x] **Clock** — 12h / 24h format (`bar.json`).
- [x] **Edge bar** — position, show on all/primary, opacity, blur.
- [x] **Weather** — auto-detect on; optional city search (Open-Meteo geocoding).
- [x] **Gaming** — hybrid/Steam summary + auto GPU / Flatpak optimize prefs.
- [x] **Optional software** — detect Remote / Flatpak / GameMode / Bluetooth /
      printers / keyring; grey out installed; toggles + `pkexec apt-get install`
      for selected packages (deb `Suggests:` alignment).
- [x] **(Later) Language & region** — once Phase 8 lands, offer locale selection
      here as the very first step. **Done (Phase 8):** first wizard step writes
      `locale.json` and rebinds gettext for later steps.
- [x] **Finish** — keybind cheatsheet + pointer to Settings → Display for monitors.

Display arrangement / resolution / Hz deliberately deferred to Settings → Display.

### C. Polish

- [x] **Skippable** — clear "Skip" that still marks onboarding done.
- [x] **Resumable** — remember progress if the session restarts mid-wizard
      (`config.json` → `onboarding_step`; cleared on Finish/Skip).
- [x] **Accessible & translatable** — keyboard-navigable, and route all copy
      through the Phase 8 i18n catalog so the wizard itself is localizable
      (chrome + language step; remaining step bodies continue via `tr()`).

---

## Phase 10 — Edge-bar system dashboard (pull-down monitor)

A **system dashboard** that slides down from the edge bar — the Metis answer to
GNOME's usage popover / KDE's system monitor tray, but richer and gesture-driven.
Click-and-drag **down** on the bar (or a dedicated strip/handle) reveals a
full-width panel under the bar with live resource graphs, storage/network
throughput, and a process list with end-task actions. Designed from the start for
**pluggable widgets** so users can add/reorder panels later (weather summary,
GPU stats, custom scripts, etc.) without rewriting the shell.

**Target UX:** feels like pulling down a shade from the bar — smooth slide
(~200–300 ms), bar stays visible at the top, dashboard fills the area below
(does not steal the whole screen unless expanded to a "full monitor" mode later).
Dismiss: drag up, click outside (compositor `close-popovers`-style signal), or
`Esc`. Same layer-shell lifetime rules as splash/onboarding — park off-screen,
never destroy mid-session.

### A. Shell surface & gesture

- [x] **Pull-down gesture** — press the bar pill and drag toward the desktop;
      panel tracks the drag (rubber-band) and snaps open past a threshold.
      Direction follows bar edge: top→down, bottom→up, left→right, right→left
      (2026-07-05). Skips bar icon widgets so popovers still work.
- [x] **Dashboard layer surface** — `gtk4-layer-shell` `Layer::Overlay`,
      namespace `metis-dashboard`, anchored below the bar; height animates to
      `max_height_percent` from config (2026-07-05).
- [x] **Overlay behavior** — dashboard uses `exclusive_zone(0)` so it draws on top
      of tiled windows without pushing or reflowing them (2026-07-05).
- [x] **Dismiss & focus** — pointer-outside via compositor `close-popovers`
      (dashboard included in bar UI hit test), Esc, drag-up on header, close
      button; `KeyboardMode::Exclusive` while open so Processes search receives
      keys (2026-07-05; Exclusive 2026-07-11).

### B. Core widgets (v1)

- [x] **CPU** — aggregate % + sparkline history (sysinfo, ~1 Hz refresh).
- [x] **Memory** — RAM + swap used/total with level bar + history chart.
- [x] **Disk** — per-mount used/free (sysinfo disks).
- [x] **Network** — aggregate RX/TX rates from `/proc/net/dev` + throughput charts.
- [x] **Processes** — PPID tree with expand/collapse; search keeps ancestor paths;
      All/User/System filter; end task (SIGTERM) / force quit; End process tree
      actions; Metis PIDs highlighted; zebra rows (tree nesting 2026-07-11).
- [x] **System health** — CPU/memory/storage health badges (semantic colors).
- [x] **Firewall** — ufw / firewalld status card on Network tab (2026-07-05).
- [x] **Hardware** — hostname, CPU model, cores, kernel on System tab (2026-07-05).

### C. Data & services (`metis-shell`)

- [x] **`spawn_dashboard_pollers()`** — dedicated thread + channel snapshots
      (2026-07-05).
- [x] **Shared snapshot type** — `DashboardSnapshot` in `metis-shell` services
      (2026-07-05).
- [x] **Process actions** — `kill_process()` / `kill_process_tree()` via `nix`
      (own uid only; tree walks descendants then root) (2026-07-11).

### D. Extensibility (v2+)

- [x] **`dashboard.json`** — enabled flag, widget order, max height %, refresh
      interval, confirm-before-kill, optional `process_monitor` launcher
      (2026-07-05; process monitor picker 2026-07-11). **Live reload** via file
      monitor (2026-07-07).
- [x] **Overview v2 layout** — CPU | Memory, Network | Disk I/O, Session | Storage,
      System row with temp gauges; embedded in bar window (no gap) (2026-07-06).
- [x] **Control Center button** — workspace-dot grid icon toggles panel with
      slide animation (2026-07-06).
- [x] **Control Center keyboard dismissal** — Esc (capture phase) and Super+Q
      close the panel without closing the underlying app (2026-07-20).
- [x] **Chart polish** — gradient fills/strokes, smooth curves, Y-axis ticks,
      per-core CPU palette, aggregate fill behind core lines (2026-07-06).
- [x] **Widget registry** — built-in widgets register by id in
      `metis-shell/src/ui/dashboard/widgets.rs`; later: JSON-defined script widgets
      or Rust plugin slots (2026-07-07).
- [x] **GPU temperature (partial)** — discrete GPUs only; sysfs `hwmon` + DRM
      device paths; `nvidia-smi` fallback on hybrid laptops without NVIDIA
      `hwmon`; Intel iGPU skipped (2026-07-06).
- [x] **Optional additions** — GPU load % on discrete GPU gauges (`gpu_busy_percent`
      / `nvidia-smi`); Open monitor auto-detects TUI/GUI monitors (terminal for
      btop/htop) from Settings → Control Center (2026-07-11).
- [x] **Process context menu** — right-click End task / Force quit / End process
      tree / Force quit tree / Copy PID; cursor-anchored popover, list refresh
      paused while open (2026-07-07; tree actions 2026-07-11).
- [x] **Optional additions** — battery history graph, log tail snippet
      (journalctl; card hidden/unavailable when missing) (2026-07-27).

### E. Settings & docs

- [x] **Settings → Control Center** — enable/disable dashboard, max height,
      refresh interval, confirm-before-kill toggle, overview widget checkboxes,
      process monitor picker (`dashboard.json`) (2026-07-07; monitor 2026-07-11).
- [x] **Settings → Keyboard → Shortcuts** — capture/edit desktop shortcuts;
      `keybinds.json` + compositor `ReloadKeybinds` / `SetKeybindCapture`;
      reserved DRM VT/quit binds locked (2026-07-11).
- [x] **Settings → Shortcuts (guide)** — searchable grouped read-only chord list
      under Input; links to Keyboard for editing (2026-07-24).
- [x] **Multimedia & hardware keys** — `XF86*` volume/mute/mic, display + keyboard
      backlight (logind `SetBrightness`), MPRIS media transport, and `XF86Display`
      mirror⇄extend toggle, with a bottom-center OSD. Compositor forwards intents;
      shell services them on a worker thread (`services/hardware.rs`, `ui/osd.rs`);
      listed under Shortcuts → System (2026-07-20).
- [x] **Shell launch polish** — standalone Super toggles the Metis Menu and
      focuses type-to-search; the menu's Settings action restores an existing
      minimized Settings window instead of spawning a duplicate (2026-07-20).
- [x] **USER_GUIDE** — gesture, overview layout, temp gauges, discrete GPU
      behaviour, kill semantics (2026-07-06; process tree + keybinds 2026-07-11).
- [x] **Metis Settings icon** — transparent `metis-settings.png` for Settings app
  only; edge-bar menu launcher unchanged (`metis_icon.png`) (2026-07-06).

**Dependencies:** Phase 1 edge bar (done); benefits from Phase 4 System page
patterns; i18n (Phase 8) before shipping strings broadly.

---

## Phase 11 — Gaming Platform 2.0

Beat Pop!_OS out of the box: hybrid GPU switching without launch-option hacks,
Flatpak Steam/Lutris/Heroic auto-setup, first-run gaming wizard, compositor hybrid
PRIME + fullscreen scanout polish, and a lean event-driven `metis-gamingd` service.

**Principles:** productize compositor dGPU offload (`gaming.json` + Settings UI);
automate first-run setup; finish hybrid PRIME / scanout perf; stay lean (no polling
daemon).

### A. GPU switching (productize compositor wins)

- [x] **`gaming.json`** — `graphics_mode`, `on_battery_prefer_igpu`,
      `auto_performance_profile`, `auto_gamemode`, `flatpak_gpu_env`,
      `steam_prefer_native`; per-game Gamescope profiles (optional).
- [x] **Compositor wiring** — read `gaming.json` on spawn + `ReloadGaming` IPC;
      map modes to `prefer_dgpu` / battery-aware offload.
- [x] **Settings → Gaming v2** — editable controls, hybrid GPU summary, optimize
      button (replaces read-only diagnostics page).

### B. Flatpak zero-config

- [x] **`metis-gaming` Flatpak optimizer** — idempotent overrides for Steam,
      Lutris, Heroic (`--device=all`, sockets, GPU env); state in
      `gaming-flatpak.json`.
- [x] **Flatpak GPU env injection** — NVIDIA/Mesa offload vars via `flatpak override`.
- [x] **Menu launcher wrappers** — `~/.local/share/metis/bin/launch-steam` for
      Flatpak + GPU env when configured.

### C. First-run gaming onboarding

- [x] **Onboarding gaming step** — optional skippable step before Finish; detect
      Steam / hybrid GPU / i386 Vulkan / gamemode.
- [x] **Gaming setup wizard** — re-runnable from Settings → Gaming.
- [x] **`gaming_setup_complete`** flag in `config.json`.

### D. Compositor performance (hybrid PRIME + scanout)

- [x] **`scripts/gaming-prime-smoke.sh`** — hybrid PRIME validation helper.
- [x] **Fullscreen scanout promotion** — trace when primary-plane scanout succeeds;
      audit `surface_primary_scanout_output` path.

### E. Auto-detect and self-heal

- [x] **Health check engine** — Steam, Flatpak overrides, i386 Vulkan, gamemode,
      input group, NVIDIA driver, PipeWire; per-row Fix in Settings.
- [x] **`metis-gamingd`** — event-driven: compositor `GameSession` +
      `WindowFullscreen` / idle inhibit; auto performance profile + GameMode hooks.
- [x] **Protocol** — `ReloadGaming` command, `GameSession` event.

### F. Session integration

- [x] **Spawn `metis-gamingd`** — from `metis-session` / `run-metis.sh`.
- [x] **`metis-cmd reload-gaming`** — runtime reload hook.
- [x] **Docs** — `USER_GUIDE.md`, `PERF_AUDIT.md`, `CHANGELOG.md`.

**Deferred (later phase):** apt/polkit auto-install of Steam, drivers, i386 Vulkan,
and GameMode; ProtonDB per-title tuning; session-wide auto-optimize on first login
without user clicking **Optimize now**; **Settings → Power → Dim on battery**
compositor hook (preference saved in `power.json` today).

**Dependencies:** Phase 6 (done); compositor `DgpuOffload` (done); Phase 9
onboarding shell (done).

---

## Phase 12 — Native Screenshot Tool

Metis-native interactive screenshot: **PrtSc** opens a Deepin-inspired overlay
(default **Selection**), frosted theme-aware toolbar, accent **Capture** button,
clipboard/save, and compositor exclusion so overlay chrome never appears in the PNG.

### Core

- [x] **`metis-capture` crate** — shared Wayland `ext-image-copy-capture` client +
      crop/PNG helpers (used by shell + portal).
- [x] **`screenshot.json`** — default mode, pointer toggle, delay, after-capture
      action, save directory (`~/Pictures/Metis`).
- [x] **Compositor** — PrtSc / Shift+PrtSc / Ctrl+PrtSc → runtime commands;
      `BeginScreenshotOverlay` / `EndScreenshotOverlay` IPC; exclude
      `metis-screenshot` namespace from capture pass.
- [x] **Shell `ui/screenshot/`** — Selection / Screen / Window modes, dashed rect +
      size label, bottom toolbar, hide-before-capture (unmap + frame delay).
- [x] **Overlay UX polish** — icon mode toggles + Options popover; click-to-lock
      window pick; **Esc** dismiss via compositor (`dismiss-screenshot`); segmented
      after-capture buttons (no `GtkDropDown` under layer-shell popovers).
- [x] **Theme integration** — `metis-screenshot-*` CSS in `css.rs`; live reload via
      `screenshot::on_theme_changed()` (dark/light/custom accent tokens).
- [x] **Clipboard + save** — `SetClipboard` image path; optional save copy and
      `xdg-open` viewer via `after_capture` config.
- [x] **`metis-cmd screenshot`** — open overlay from script/launcher.
- [x] **Docs** — `USER_GUIDE.md`, `CHANGELOG.md`.

**v1.1 (2026-07-31):**
- [x] **Per-output targeting** — PrtSc appends pointer connector; picker
      `set_monitor` + `CaptureOptions.connector`.
- [x] **`metis-screenshot` binary** — post-capture editor (annotate / OCR /
      pin / record); spawned via compositor `Launch`.
- [x] **`AfterCaptureAction::Edit`** — interactive default; instant path never
      auto-opens the editor.
- [x] **Settings → System → Screenshot** — binds `screenshot.json`.
- [ ] ~~**Scroll mode** — selection + synthetic scroll stitch (`stitch_vertical_append`).~~
      Removed; not useful enough to keep.
- [x] **Pin + Record** — layer-shell pin window; ffmpeg WebM record handoff.

**Dependencies:** Phase 1 theme/CSS pipeline (done); compositor image capture (done).

---

## Phase 13 — Notification Center (Win11-style)

Right-edge layer-shell panel opened from the **clock** (bell merged in). Theme-aware
frosted panel with collapsible cards and closable top-right toasts.

### Core

- [x] **Toast polish** — close (X) on each banner; shift right margin while the
      panel is open; token-driven light/dark card styles.
- [x] **Notification Center shell** — `metis-notification-center` layer-shell
      Overlay; slide from right; Esc / `close-popovers` / clock toggle dismiss;
      park window when closed.
- [x] **Notifications card** — DND, Clear all, grouped list; auto-collapse when empty.
- [x] **Events + calendar/tools cards** — events auto-collapse when empty; calendar
      card icon rail switches Calendar / World / Stopwatch / Timer / Alarms.
- [x] **Clock merge** — unread badge on clock; default `bar.json` drops
      `notifications`; migrate existing configs that still list both.
- [x] **Theme** — `metis-nc-*` CSS from design tokens; `on_theme_changed` hook;
      themed entries/switches/buttons (no `GtkDropDown` in the panel).
- [x] **Docs** — `USER_GUIDE.md`, `CHANGELOG.md`, `README.md`, this file.

**Deferred (v1.1):** per-app notification settings; follow-bar panel side; rich
media in toasts.

**Dependencies:** Phase 1 notifications + clock pages (done); theme/CSS pipeline (done).

---

## Phase 14 — Desktop Widgets

Optional **free-floating wallpaper widgets** instead of classic desktop icons.
Users who want a glanceable desktop get Folders / Apps / Clock / System /
Weather / Equalizer panels they can place, resize, and lock; everyone else
leaves the feature off. Wallpaper stays the hero; widgets sit above it and
below normal windows.

**Non-goals (v1):** classic desktop icons; reviving compositor `desk.json`
`TileKind::Widget` grid tiles (already stripped on load — keep `desk.json` for
app-grid persistence only); one Wayland surface per widget instance.

**Host model:** GTK4 + `gtk4-layer-shell` in a dedicated `metis-shell
--desktop-widgets` process (spawned by the compositor beside the edge bar),
Background (or Bottom) layer, `exclusive_zone(0)`, **one surface per output**
hosting many widget instances. Config: `~/.config/metis/desktop-widgets.json`
(master switch default **off**). Edit mode = move/resize; locked = content
clicks only. Empty chrome aims for click-through where layer-shell allows
(imperfect pass-through OK in v1). Settings reloads use
`$XDG_RUNTIME_DIR/metis/command-widgets` so they do not race the bar.

### A. Platform

- [x] **Layer host** — per-output Bottom layer-shell surface; theme tokens; tear
      down when master switch is off. Compositor lock screen covers the session
      (widgets are not shown on the lock UI).
- [x] **Process isolation** — `metis-shell --desktop-widgets` owns watches,
      weather, and the widgets command file; bar no longer hosts widgets
      (2026-07-24).
- [x] **Widget registry** — kind id → factory (build GTK content + settings);
      iterate `instances` from config (not hardcoded layout)
- [x] **Edit / lock** — global edit mode + per-instance `locked`; move/resize
      only when unlocked; persist geometry (`x`, `y`, `w`, `h`, `output`)
- [x] **Multi-monitor** — instances bound to an output name; recreate on hotplug
- [x] **Hit-testing** — v1: content and chrome receive clicks as expected;
      empty desktop around widgets stays usable (no special pass-through work)
- [x] **Live reload** — Gio monitor on `desktop-widgets.json` (same pattern as
      `bar.json` / `dashboard.json`)

### B. Folders + Apps (v1 differentiators)

- [x] **Folders widget** — path default `~/Desktop` or custom; directories first,
      then files, A–Z; Gio `FileMonitor`; open via configured file manager /
      launch; faint transparent panel; cap/virtualize huge folders; context
      menu Open / Rename / Delete (opaque Popover confirms — not hollow
      toplevel windows)
- [x] **Apps widget** — dedicated widget pin list (not a silent dump of start-menu
      pins); “Import start-menu pins” action; launch via `applications::launch_id`
      (handles `OnlyShowIn=GNOME` desktop files)

### C. Clock / System / Weather / Equalizer

- [x] **Clock** — large time + date; Metis light/dark tokens; optional font /
      text colour / accent
- [x] **System** — CPU / RAM / disk glance; optional font / text colour /
      progress-bar accent
- [x] **Weather** — reuse shell weather service (same data as the edge-bar
      widget); optional font / text colour
- [x] **Equalizer** — default-sink monitor (Pulse/PipeWire) FFT visualizer;
      styles: spectrum lines / bars / neon wave / radial; bar shapes:
      segmented / solid / dots / dense dots; colour: solid / gradient /
      theme; bars: height gradient, peak caps, reflection; neon: mirror

### D. Settings + config

- [x] **`desktop-widgets.json`** in `metis-config` — `enabled`, `edit_mode`,
      global `chrome` (bg opacity/colour, border width/colour) + per-instance
      `chrome` overrides, `instances[]` (`id`, `kind`, `output`, geometry,
      `locked`, kind-specific fields: `path` / `pins` / `view` / `font` /
      `text_color` / `accent_color` / equalizer viz options); sanitize +
      defaults; write on demand
- [x] **Settings → Desktop → Desktop widgets** — master enable, edit mode,
      default look, compact zebra list + configure dialog, add/remove,
      folder path, import app pins, per-instance lock, text style, equalizer
      options (style-dependent)
- [x] **Docs** — User Guide + CHANGELOG + README (2026-07-18)
- [x] **Widget chrome** — global defaults + per-widget overrides; opacity 0 /
      border width 0 fully clear the fill / edge (text stays opaque)

Config sketch:

```json
{
  "enabled": false,
  "edit_mode": false,
  "chrome": {
    "background_opacity": 0.4,
    "background_color": "",
    "border_width": 1.0,
    "border_color": ""
  },
  "instances": [
    {
      "id": "uuid",
      "kind": "folders",
      "output": "DP-1",
      "x": 80,
      "y": 80,
      "w": 360,
      "h": 280,
      "locked": false,
      "path": "~/Desktop",
      "chrome": {
        "background_opacity": 0.0,
        "border_width": 0.0
      }
    }
  ]
}
```

### E. Extension API (JSON widgets v1 — 2026-07-26)

- [x] **Manifest** — widget id, name, version, size hints, settings schema
      (`~/.local/share/metis/widgets/<id>/manifest.json`)
- [x] **Host API** — theme CSS classes; hardened `open_uri` (http/https),
      `launch` (desktop id / PATH basename), `copy_text`; layout size caps;
- [x] **Live `{host.*}` binds** (clock / weather / sysinfo) (2026-07-27)
- [x] **JSON widgets** — declarative `widget.json` layout DSL rendered in
      `metis-shell --desktop-widgets`; sandboxed; **no** Electron / scripts /
      in-process `.so` in v1
- [x] **§E.2 out-of-process helpers** (2026-07-27) — pack `helper.exec` basename,
      argv-only spawn, `{helper.*}` tokens; example `com.metis.example.helperstatus`
- [ ] **§E.2 scripts / WASM** — Rhai/Lua/WASM runtimes still deferred

**Phase 14 complete (2026-07-18); §E JSON extensions shipped 2026-07-26; helpers 2026-07-27.**

**Dependencies:** Phase 1 theme/CSS + layer-shell patterns (done); Phase 10
dashboard sampling (done); Phase 2 weather service (done); app launch /
`launch_id` (done); Phase 15 widgets IPC capability token (done).

---

## Phase 15 — Security hardening

Raise Metis’s security floor without rebuilding the DE. Phase 7 already hardened
the GNOME RDP path (LAN firewall, stdin credentials, lock pauses listen,
text-only clipboard, abstract X socket off). This phase closes the residual
audit gaps: supply chain, spawn hygiene, lock/VT policy, capability-scoped IPC,
and deeper X11 isolation.

**Target:** a Metis session where privileged host actions are Polkit-declared,
user-controlled strings never hit a shell, lock mode cannot be trivially bypassed
via VT switch without a documented policy, widgets cannot issue compositor
commands outside their capability, CI refuses known-CVE crates, and untrusted
X11 apps are isolated where practical.

**Non-goals:** rewriting Smithay; a Metis-native remote protocol as the primary
deliverable (optional stretch in §F); claiming full XSECURITY on a single
shared X server.

### A. Supply chain & compiler mitigations (quick wins)

- [x] **`cargo-audit` in release CI** — run `cargo audit` (or `cargo deny`) in
      `.github/workflows/release-deb.yml` before the `.deb` build; fail the job
      on unignored advisories. Document ignore policy for accepted risks.
- [x] **Dev / PR audit optional** — lightweight `cargo audit` job on PRs that
      touch `Cargo.lock` (or nightly), so CVEs surface before tag day.
- [x] **Release profile hardening** — enable `overflow-checks = true` on
      `[profile.release]` (and inherit in `release-small`); set
      `panic = "abort"` on default `release` (already on `release-small`) to
      avoid unwind metadata leaks. Re-benchmark compositor hot path; document
      any intentional opt-outs per crate if needed.
- [x] **Document residual C ABI risk** — USER_GUIDE / PACKAGING note that GTK,
      lcms2, OpenSSL, libinput, etc. remain outside Rust’s safety model; point
      at distro security updates.

### B. Process spawn & Polkit

- [x] **Ban shell interpolation for user input** — audit `Command::new("sh")` /
      `-c` across compositor, shell, settings, remote, gaming; replace with
      argv vectors (`.arg()` / `.args()`). Keep `sh -c` only for fixed literals
      with no interpolated untrusted data (or eliminate entirely).
- [x] **SSID / VPN / process-name sanitization** — validate and pass NetworkManager
      / systemctl / related arguments as discrete argv; reject control chars and
      unexpected separators where tools accept free-form names.
- [x] **Metis Polkit policies** — ship `.policy` (and optional `.rules`) for
      privileged helpers (`metis-remote firewall`, optional software install,
      group membership changes, etc.) bound to Metis desktop IDs; prefer
      `pkexec` of a fixed helper binary + subcommand over free-form command
      strings. Document required PolicyKit prompts in USER_GUIDE.
- [x] **Minimize credential lifetime on argv** — where third-party tools still
      require argv secrets (`grdctl`), document and keep the window as short as
      possible (already noted for RDP); prefer stdin/socket APIs when upstream
      allows.

### C. Session lock & VT switching

- [x] **Decide lock/VT product policy** — pick one and document:
      (1) **Block** Ctrl+Alt+F\<n\> while locked (keep Ctrl+Alt+Backspace quit as
      escape hatch), or (2) **Allow** VT switch but blank/lock DRM outputs and
      require re-auth on return, or (3) leave current escape-hatch behavior with
      an explicit USER_GUIDE security note. Default recommendation: (1) or (2).
- [x] **Implement chosen policy** — gate `drm_change_vt` on `lock.locked` (and
      optionally idle-blank); on VT resume while session still “locked”, re-show
      lock UI before client input/capture.
- [x] **`ext-session-lock-v1` (optional follow-up)** — protocol support for
      third-party lockers (swaylock / gtklock); Metis PAM lock remains default;
      one lock owner per cycle; unlock side effects match PAM unlock
      (2026-07-27).

### D. Capability-scoped IPC (widgets & helpers)

- [x] **Command allowlists per channel** — `command-widgets` (and any future
      helper sockets) accept only a documented subset (e.g. reload widgets,
      update weather state) — refuse `Launch`, geometry/window manip, session
      end, inject, etc. Keep bar/main channel for privileged DE control.
- [x] **Capability tokens (v1)** — compositor issues short-lived, scoped tokens
      when spawning `--desktop-widgets` (and similar); widget process must
      present token + command; reject mismatched scope. Same-UID SO_PEERCRED
      remains necessary but not sufficient.
- [x] **Docs** — USER_GUIDE IPC trust model: same-UID baseline, per-channel
      allowlists, token scopes, what a compromised widget can / cannot do.

### E. Deeper X11 / XWayland isolation

Carried from Phase 7 residual. Wayland clients stay isolated; X11↔X11 on one
server does not.

- [x] **Per-app / per-sandbox XWayland** — research + prototype separate rootless
      XWayland instances for untrusted classes (e.g. Flatpak X11, Steam/Proton
      bucket vs random native X11). Measure RAM/CPU vs single server.
- [x] **Launch policy** — Settings or `config.json` knob: shared XWayland
      (compat) vs isolated buckets (hardened). Default stays shared until
      isolation is stable.
- [x] **Abstract socket remains default-off** — keep `xwayland_abstract_socket:
      false`; document when enabling it is necessary.
- [x] **XSECURITY research note** — document usefulness (often limited under
      rootless XWayland); do not claim sandboxing we cannot enforce.

### F. Stretch / related (not phase gates)

- [x] **First-party remote viewer** (2026-07-26) — `metis-viewer` GTK + FreeRDP
      client; GRD host unchanged. Viewer polish + dedicated icon (same day).
- [x] **RustDesk Settings preset** (2026-07-26) — detect/open/copy-install card;
      no `metis-remote` backend.
- [x] **Image RDP clipboard** (2026-07-26) — `metis-portal` Mutter clipboard shim
      advertises/serves local `image/png|jpeg|bmp` from compositor durable paths
      (≤10 MiB) and accepts remote image writes into `$XDG_RUNTIME_DIR/metis/clipboard/`
      then `SetClipboard`. Text path unchanged; lock still pauses RDP.
- [x] **Fingerprint / YubiKey lock niceties** (2026-07-26) — soft device detect,
      lock UI cues, empty-password PAM + one auto-attempt for host `pam_fprintd` /
      `pam_u2f`. Password always works; enroll on the host.

**Suggested order:** Phase 15 product bar + `ext-session-lock-v1` complete.

**Dependencies:** Phase 7 remote + IPC baseline (done); Phase 14 widget process
split (done); DRM/libseat VT path (done).

---

## Phase 16 — Engineering hardening

Close the engineering gap after Phase 15 product security: CI that gates PRs,
trust-boundary unit tests, panic hygiene under `panic = "abort"`, supply-chain
deny rules, command-file hardening, and focused stability/perf cleanup. No new
desktop product features in this phase.

**Non-goals:** full Wayland e2e harness in CI; default-on colour management;
big-bang `state.rs` rewrite; Sober/Flatpak app bugs unrelated to Metis portals.

### A. PR quality gate

- [x] **`.github/workflows/ci.yml`** — on every PR + push to `main`/`master`:
      `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace`, `cargo deny check` (Ubuntu 24.04 + gtk4-layer-shell)
- [x] **Fold advisory gate into CI** — `cargo deny` on every PR; remove path-filtered
      `audit.yml` so advisories are not skipped when Rust paths are untouched

### B. Trust-boundary tests

- [x] **`metis-protocol` unit tests** — `runtime_dir` / `ensure_runtime_dir` 0700,
      `write_private_file` 0600, `split_command_line` / `launch_argv`, peercred
      same-euid accept, runtime command allowlist parser
- [x] **Compositor IPC helpers** — extract `ipc_dispatch.rs` (`IpcCaps`,
      `resolve_ipc_caps`, widgets allowlist, lock denylist, `gate_ipc_command`)
      with table-driven unit tests (no live Wayland loop)
- [x] **Portal unit tests** — clipboard path allowlist + size cap; screencast
      refuse-while-locked; screensaver idle-inhibit peer reclaim (no full
      PipeWire/EIS integration in CI)

### C. Panic & structure

- [x] **Compositor hot-path unwrap triage** — replace event-loop unwraps in
      `xdg_shell`, `desk_input`, `state`, `input`, `udev` with warn+skip/recover;
      keep init-only expects where invariants are setup-only
- [x] **Colour-management default-off assertion** — unit/doc coverage that the
      default env/config path does not enable `wp_color_management_v1`
- [x] **Opportunistic `state.rs` extract** — `ipc_dispatch.rs` for IPC caps /
      gating; further extracts as areas are touched (no big-bang rewrite)
- [x] **Remaining intentional panics** — init-only `expect` / impossible
      calloop registration failures may still abort under `panic = "abort"`;
      prefer `tracing::warn!` on hot paths. Defer crate-wide `unwrap_used` deny
      on compositor/shell until a later pass.

### D. Supply chain & packaging CI

- [x] **`deny.toml`** — advisories, license allowlist, banned unknown git sources
      (Smithay allowlisted)
- [x] **`cargo deny` in `release-deb.yml`** — all suite jobs (24.04, 26.04, debian13)
- [x] **Locale smoke on all suites** — `.mo` / `.ftl` presence checks on every
      packaged `.deb` (not only Ubuntu 24.04); workspace tests gated by `ci.yml`

### E. Command file & docs

- [x] **Runtime command allowlist** — verb lists + max length +
      `parse_runtime_command` / `write_runtime_command*` in `metis-protocol`;
      shell bar, desktop-widgets, gamingd consume the parser
- [x] **Document same-UID trust model** — USER_GUIDE / UBUNTU_DEV (socket+token
      vs weaker command file)
- [x] **`docs/PERF_AUDIT.md` refresh** — ScreenCast dmabuf, `panic = "abort"`,
      shell poll / GSK (`METIS_SHELL_GSK_RENDERER`), hybrid NVIDIA MemFd checklist

### F. Shell poll

- [x] **D-Bus-driven network dirty flag** — NetworkManager `StateChanged` →
      immediate refresh; slower battery/BT ticks; adaptive sleep
- [x] **GSK env documented** — `METIS_SHELL_GSK_RENDERER=gl` opt-in; Cairo default

### G. Lint policy (follow-on)

- [x] **`clippy::unwrap_used` deny** on `metis-protocol` and `metis-config`
      (non-test builds); defer compositor/shell until further triage

**Dependencies:** Phase 15 IPC / portal / release CI baseline (done).

---

## Phase 17 — Task View (Super+Tab)

Unified Task View replaces the separate Alt+Tab strip and workspace grid.

### Shipped

- [x] **`Super+Tab` Task View** — current-workspace app cards (live thumbs) +
      bottom shelf of live workspace mini-desktops; Tab/Enter/click; DnD move
- [x] **`CaptureWindowThumbs` / `CaptureWorkspaceThumbs`** compositor PNG path
- [x] **Defaults** — only `Super+Tab`; Alt+Tab unbound (remap Task View next/prev)
- [x] USER_GUIDE / README / CHANGELOG / this checklist (Task View docs refreshed 2026-08-06)

**Out of scope:** “New desktop” button; all-monitors unified overview; overview
on vertical bars / tablet gestures.

**Dependencies:** Phase 3 workspaces + window IPC; Phase 12 overlay patterns.

---

## Phase 18 — Security polish, IPC DoS, isolation stretch

Post–Phase 15/16 residual hardening from external review, aligned with
[`SECURITY.md`](../SECURITY.md) “Residual hardening”. Same-UID trust model stays
explicit: peercred stops cross-user access; this phase hardens *abuse* and
config edges inside that model.

**Non-goals:** forking wayland-rs; claiming XSECURITY / true sandboxing we cannot
enforce; speculative `metis-grid` memoization without a profiled hotspot;
re-doing ScreenCast dmabuf (already shipped).

### A. Gaming / config path edges (P0)

- [x] **Canonicalize `extra_steam_paths`** — `std::fs::canonicalize` (or
      equivalent fail-closed resolve) before any filesystem / env use; reject
      paths that escape allowed roots (`$HOME`, `/mnt`, `/media`, `/run/media`)
- [x] **Sanitize launcher env exports** — validate generated gaming / Flatpak
      launch environment lines (offload-key allowlist + shell-safe quoting)
- [x] **Tests** — table-driven reject/accept for malicious relative and
      symlink-escape paths (`metis-config` `gaming_paths`)
- [x] **Docs** — SECURITY residual #1 → done; USER_GUIDE note on `extra_steam_paths`

### B. IPC rate limits & token lifecycle (P0)

- [x] **Sliding-window rate limit** on compositor Unix-socket accept/dispatch
      (global 1s window + max accepts per drain). Valid same-UID clients can
      still spam; bound parser / event-bus work without changing the trust model
- [x] **Command-file poke channel** — mirror budgets on `write_runtime_command*`
      and shell poller dispatch so `$XDG_RUNTIME_DIR/metis/command*` is not a
      cheap DoS path
- [x] **`METIS_IPC_TOKEN` TTL / idle invalidate** (optional) — documented as
      spawn-scoped only (cleared on widgets exit); no wall-clock TTL
- [x] **Unit tests** — rate-limit helper table tests (`metis-protocol` `rate_limit`)
- [x] **Docs** — USER_GUIDE IPC trust model + SECURITY residual #2

### C. Widget pack schema gate (P1)

- [x] **Serde-strict validation** for community / local widget packs under
      `~/.local/share/metis/widgets/` (and system share dirs) at discovery /
      desktop-widgets startup (`deny_unknown_fields` + validated `widget.json`
      + helper resolve)
- [x] **Fail closed** — drop invalid packs with warn + startup toast; do not
      crash the shell
- [x] **Ship a schema document** — [`docs/WIDGET_PACK_SCHEMA.md`](../docs/WIDGET_PACK_SCHEMA.md)

### D. Per-sandbox rootless XWayland (P1 / stretch)

- [x] Evolve Phase 15 §E two-bucket toward **policy-driven soft classes**
      (gaming vs desktop): lazy gaming XWayland, DISPLAY routing decoupled from
      GPU offload; Flatpak Steam/Lutris/Heroic ids map to gaming bucket (not a
      Flatpak sandbox)
- [x] Launch policy in Settings → Gaming + `config.json` (`xwayland_mode`,
      `xwayland_policy`); document residual X11↔X11 / same-UID risk
- [x] Do **not** claim full isolation — SECURITY residual #4 marked done as soft
      policy only

### E. Display / upstream (track only)

- [ ] **Default-on `wp_color_management_v1`** — only after upstream wayland-rs
      **server/sys** ObjectData UAF fix (keep `METIS_COLOR_MGMT=1` opt-in). No
      local ObjectData lifecycle wrapper in Metis
- [ ] **GLES `MultiRenderer` / zero-copy compositor path** — Phase 3 stretch for
      hybrid multi-GPU *compositor* binding (ScreenCast dmabuf already done).
      Profile before prioritizing

### F. Explicitly deferred / rejected from review

- Local safe-wrapper around wayland-rs ObjectData destruction (prefer upstream)
- Blind `metis-grid` static layout cache (measure contention first)

**Suggested order:** A → B → C → D; E stays gated on upstream / profiling.

**Dependencies:** Phase 15 IPC / XWayland / gaming; Phase 16 trust-boundary
tests; Phase 14 widgets process split.

---

## Phase 19 — Gaming Setup UX (drivers + first-run polish)

Productize “install Metis → Gaming setup → play” so hybrid / Steam / Proton
setup feels simpler than Pop!_OS or CachyOS *without* editing Steam’s per-game
Launch Options for GPU (compositor offload already owns that).

**Goals:** guided, consent-based driver and dependency install; clear health →
fix; onboarding copy that Metis handles GPU routing; Metis-owned optional
tweaks (not Steam VDF writes).

**Non-goals:** auto-writing Steam Launch Options / `localconfig.vdf`; silent
NVIDIA install on first login; shipping a Metis driver PPA or `.run` installers;
out-kernelling CachyOS.

### A. Guided GPU drivers (P0)

- [x] **Detect** — NVIDIA (PCI + `/proc/driver/nvidia`), AMD/Intel Mesa, hybrid
      vs single GPU; surface in Settings → Gaming health + setup wizard
- [x] **NVIDIA (opt-in)** — guided `ubuntu-drivers` install via pkexec (recommended
      package); matching **i386** GL/Vulkan libs for the installed series; hard
      **reboot required** banner; re-run health after reboot. No blind auto-fix
      today stays until this ships with explicit consent UI
- [x] **AMD / Intel** — ensure `mesa-vulkan-drivers` (+ `:i386`); no separate
      “proprietary AMD gaming driver” path
- [x] **Refuse silent install** — never run driver install from session start;
      require Settings / wizard confirmation
- [x] **Docs** — USER_GUIDE Gaming: what Metis installs vs what Steam/Proton own

### B. Gaming Setup wizard polish (P0)

- [x] **One-shot flow** — Steam (prefer native) → Vulkan/i386 → controllers /
      `steam-devices` / input group → GameMode → GPU mode (Auto) → driver step
      when needed → “you’re ready”
- [x] **Health → Fix** — expand one-click fixes for safe packages; driver step
      uses the guided path from §A
- [x] **Copy** — state clearly that Launch Options stay empty for GPU; Metis
      session offload handles hybrid

### C. Metis-owned optional tweaks (P1)

- [x] **Deferred per-app `gamescope_profiles`** — config retained; full per-Steam
      appid UI left for a follow-up (Big Picture uses `gamescope_big_picture`)
- [x] **Optional toggles** — `mangohud_for_games` / `gamescope_big_picture` in
      Settings, applied via env/wrapper when Metis starts Steam / Big Picture
- [x] **Settings UI** for `extra_steam_paths` (Flatpak only) — path picker using
      Phase 18 A validation

### D. Explicitly deferred / rejected

- Auto-fill Steam Launch Options text box for `DRI_PRIME` / NVIDIA offload
- Background proprietary driver install without consent
- Competing with Cachy on custom kernels / schedulers

**Suggested order:** A → B → C.

**Dependencies:** Phase 15/Gaming Platform health + `metis-gaming` pkexec
install helpers; Phase 18 A path/env sanitization (done); Ubuntu `ubuntu-drivers`
/ apt (packaging target).

---

## Config

Config lives under `~/.config/metis/`. Written on first run: `bar.json`,
`clock.json`, `calendars.json`, `themes/dark.json`, `themes/light.json`. Created
on demand: `config.json` (on preference change), `menu.json` (launcher defaults /
pins), `wallpaper.json` (background pick), `weather.json` (weather setup),
`dismissed.json`, `desk.json` (compositor app-grid), `briefing.json` (optional,
user-created), `desktop-widgets.json` *(Phase 14)*, `viewer.json` (Metis Viewer
recent hosts; no passwords).

| File | Purpose |
|------|---------|
| `bar.json` | Edge bar layout, widgets, workspaces, borders, default layout |
| `clock.json` | World clocks and alarms |
| `calendars.json` | Calendar accounts |
| `config.json` | Active theme, onboarding state (`onboarding_complete` + `onboarding_step`), briefing-on-login |
| `menu.json` | App launcher: terminal + file-manager defaults, pinned apps |
| `wallpaper.json` | Wallpaper picture / colour / gradient (+ per-output overrides) |
| `desk.json` | Compositor window-grid layout (app tiles; not desktop widgets) |
| `desktop-widgets.json` | *(Phase 14)* Wallpaper widgets: enable, edit mode, chrome, instances (Folders / Apps / Clock / System / Weather / Equalizer) |
| `themes/dark.json`, `themes/light.json` | Design tokens (accent + secondary accent, semantic colors, `text_on_accent`, shadows/glows); live-reloaded |
| `briefing.json` | Weather coordinates + RSS feed URL |
| `weather.json` | Bar weather: unit, auto-detect, IP-geolocation, saved locations |
| `dashboard.json` | *(Phase 10)* Control Center: widget order, height, refresh, confirm-before-kill, process monitor |
| `keybinds.json` | Desktop shortcuts (chord → action); Mod key for defaults; live `ReloadKeybinds` |
| `locale.json` | *(Phase 8)* Session language override + formats-follow-language |
| `gaming.json` | *(Phase 11/19)* Graphics mode, auto performance/GameMode, Flatpak GPU env, library paths, Metis launch tweaks |
| `gaming-flatpak.json` | *(Phase 11)* Record of applied Flatpak gaming overrides |
| `game-rules.json` | Float / fullscreen rules for Steam/Proton games (built-in defaults if absent) |
| `screenshot.json` | *(Phase 12)* Native screenshot defaults: mode, pointer, delay, after-capture, save dir |
| `input.json` | Mouse, touchpad, and keyboard settings (compositor live-reload) |
| `power.json` | Power profile, idle blank/suspend timeouts, lid-close action |
| `outputs.json` | Per-output scale, enabled, layout, saved video mode, night-light prefs |

### `bar.json` (defaults)

```json
{
  "position": "top",
  "height": 36,
  "width": 48,
  "margin_top": 8,
  "margin_h": 10,
  "full_width": true,
  "opacity": 0.92,
  "menu_opacity": 0.92,
  "titlebar_opacity": 1.0,
  "titlebar_pill_border": {
    "mode": "accent",
    "color": "#00F2FE",
    "gradient": ["#00F2FE", "#4FACFE", "#A24BFF"],
    "width_px": 1.0
  },
  "window_border": {
    "mode": "accent",
    "color": "#00F2FE",
    "gradient": ["#00F2FE", "#4FACFE", "#A24BFF"],
    "width_px": 1.0
  },
  "bar_border": {
    "mode": "accent",
    "color": "#00F2FE",
    "gradient": ["#00F2FE", "#4FACFE", "#A24BFF"],
    "width_px": 1.0
  },
  "blur": true,
  "blur_radius": 18.0,
  "widgets": [
    "workspaces",
    "tasks",
    "spacer",
    "tray",
    "weather",
    "battery",
    "network",
    "bluetooth",
    "volume",
    "clipboard",
    "clock"
  ],
  "clock": {
    "time_format": "%I:%M %p",
    "date_format": "%a %b %d",
    "timezones": ["UTC"]
  },
  "workspace_count": 4,
  "workspace_mode": "separate",
  "default_layout": "grid",
  "taskbar_pinned": []
}
```

| Field | Meaning |
|-------|---------|
| `position` | `top`, `bottom`, `left`, or `right` edge (Settings → Appearance → Edge bar) |
| `height` | Bar thickness when `position: top`/`bottom` |
| `width` | Bar thickness when `position: left`/`right` |
| `margin_top` | Distance from the anchored screen edge (all positions) |
| `margin_h` | Margin along the bar's long axis |
| `full_width` | Span the entire edge vs. hug content |
| `opacity` | Pill background opacity (0–1); enables a see-through bar |
| `menu_opacity` | App launcher panel background opacity (0–1); text/icons stay opaque |
| `titlebar_opacity` | Server-side titlebar background opacity (0–1); title/buttons stay opaque |
| `titlebar_pill_border.mode` | Focused title-pill border: `accent` (theme accent gradient), `solid`, or `gradient` |
| `titlebar_pill_border.color` | Stroke color (`#rrggbb`) when `mode: solid` |
| `titlebar_pill_border.gradient` | Stops (`#rrggbb`), left→right, when `mode: gradient` |
| `titlebar_pill_border.width_px` | Stroke thickness in pixels (0–8) |
| `window_border.mode` | Focused window frame border: `accent`, `solid`, or `gradient` (independent of the pill) |
| `window_border.color` | Frame stroke color (`#rrggbb`) when `mode: solid` |
| `window_border.gradient` | Stops (`#rrggbb`), top→bottom, when `mode: gradient` |
| `window_border.width_px` | Frame thickness in pixels (0–16); also insets the client body |
| `bar_border.mode` | Edge-bar pill border: `accent` (theme accent gradient), `solid`, or `gradient` |
| `bar_border.color` | Stroke color (`#rrggbb`) when `mode: solid` |
| `bar_border.gradient` | Stops (`#rrggbb`), along the bar's long axis, when `mode: gradient` |
| `bar_border.width_px` | Stroke thickness in pixels (0 disables the border) |
| `blur` | Enable the compositor Gaussian backdrop blur behind the bar |
| `blur_radius` | Blur strength in pixels (1–64) when `blur` is on |
| `widgets` | Ordered list; `spacer` pushes following widgets apart. Includes `tasks` (the running-apps dock) |
| `clock.time_format` / `date_format` | `chrono` format strings |
| `clock.timezones` | Extra zones listed in the calendar popover |
| `workspace_count` | Number of workspace indicator dots (1–12) |
| `workspace_mode` | Multi-monitor workspace behavior: `separate` (each output independent) or `linked` (all outputs switch together). Settings → Appearance → Edge bar → Workspaces |
| `default_layout` | Layout mode: `grid` (tiling) or `scroll` (niri-style strip). Changing it in Settings → Appearance → Edge bar → New workspace layout applies live to every workspace; `Super`+`\` toggles a single workspace |
| `taskbar_pinned` | App ids pinned to the `tasks` dock, in order (independent of `menu.json` launcher pins) |

Edit `bar.json` while the shell runs — changes apply within ~1s. Cosmetic fields
(opacity, tray mode, margins, blur, borders, taskbar pins) update in place without
closing popovers; structural changes (widgets, position, displays, clock format,
workspace count) trigger a full widget rebuild. The compositor also re-reads
`blur`/`blur_radius` live. Legacy layouts are migrated to the current defaults
automatically. Editing `themes/dark.json` / `themes/light.json` re-applies the
active theme live as well.

---

## Run

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --build --session
```

| Flag | Effect |
|------|--------|
| `--session` | Start Metis compositor + shell (nested dev session) |
| `--import-env` | (With `--session`) route D-Bus/systemd-activated apps into the nested session, restored on exit |
| `--build` | Force a rebuild before running |
| `--release` | Use optimized binaries |
| `--foreground` | Run in the foreground instead of backgrounding the shell |
| `--stop` | Stop the background shell process |
| `--verify` | Check keybind / socket health while running |
| `--verify-grid` | Compare compositor vs shell grid layouts |
| `-- -c <app>` | Spawn a client app inside the session |

Session mode disables wallpaper and briefing by default (`METIS_NO_WALLPAPER=1`, `METIS_NO_BRIEFING=1`). Re-enable with:

```bash
METIS_NO_WALLPAPER= METIS_NO_BRIEFING= ./run-metis.sh --session
```

Send runtime commands to a running shell with `scripts/metis-cmd.sh {close-popovers|reload-bar}`.

---

## Widget map

| Widget | Source |
|--------|--------|
| Workspaces | Metis compositor — live virtual workspaces (single output) |
| Tasks | Running-apps dock — live compositor window state (`services/windows.rs`), per-app grouping, pin/minimize |
| Clock | Opens Notification Center (calendar, world clocks, stopwatch, timer, alarms) + unread badge |
| Battery | `/sys/class/power_supply/BAT*` |
| Bluetooth | BlueZ (`bluetoothctl`) + UPower + optional Solaar (Logitech HID++); bar popover lists connected devices |
| Network | `nmcli` (Wi-Fi + Ethernet) |
| VPN | `nmcli` (OpenVPN / WireGuard connect/disconnect) |
| Volume | `pactl` (scroll on widget to adjust) |
| Notifications | *(optional)* Same Notification Center as clock; freedesktop D-Bus → runtime store |
| Weather | Open-Meteo (keyless); IP-geolocation auto-detect (timezone fallback), override in Settings |

> Tip: set `METIS_DEMO_NOTIFICATIONS=1` before launching to seed demo notifications
> for testing the Notification Center list.
