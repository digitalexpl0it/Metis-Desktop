# Changelog

All notable changes to Metis are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2026-09-20]

### Added

- **Metis Menu layout pack** — eight additional layouts with Metis names (not
  Arc/Whisker clones): **Bracket** (categories | apps), **Ledger** (tall list),
  **Mosaic** (icon grid), **Ramp** (rail + wide center + pinned), **Ladder**
  (category → apps), **Plaza** (search + grid + profile/power footer), **Crest**
  (centered avatar + grid), **Chip** (dense grid). Settings → Metis Menu shows
  scrollable thumbnails; Freedesktop categories feed Bracket/Ladder.
- **Settings → System → About** — product version (`0.1.0.N` from crate SemVer),
  installed Metis components, author DigitlExpl0it, GitHub link, and host OS
  info (distro, kernel, CPU, memory).
- **Settings → System → Reset** — factory-reset `~/.config/metis` with optional
  backup (`~/metis-config-backup-…`), keep custom themes, and re-run first-run
  setup. Destructive confirm; session restart recommended afterward.
- **Onboarding: Network + Desktop widgets** — first-run wizard steps for Wi-Fi /
  wired connect and optional desktop widgets (seeds Clock + Folders when enabled).
- **`metis-polkit-agent`** — first-party GTK4 PolicyKit authentication agent
  started with the Metis session (compositor + watchdog). `pkexec` prompts
  (Users, firewall, Date & Time, …) no longer require GNOME/KDE agents. PAM
  goes through the system helper: prefer systemd socket activation at
  `/run/polkit/agent-helper.socket` (polkit ≥ 127), with legacy setuid
  `polkit-agent-helper-1` as fallback.
- **Users list polish** — Settings → System → Users shows each account as a
  separated row with circular avatar, display name, username, and
  Administrator / Standard user role (plus admin toggle and actions).
- **Cross-DE avatar lookup** — resolve profile pictures from `~/.face`,
  `~/.face.icon`, then AccountsService
  (`/var/lib/AccountsService/icons/<username>`) so GNOME/KDE-set avatars
  appear in Settings and the Metis Menu header.

### Changed

- Packaging and session install ship `metis-polkit-agent`; deb Recommends drop
  `policykit-1-gnome | mate-polkit` in favor of depending on `pkexec`/`polkitd`.
- Polkit auth dialog: single compositor titlebar (no GTK CSD double chrome),
  Metis-themed Authenticate button contrast, and lighter password entry
  (`GTK_A11Y=none` / Cairo) to avoid typing lag.

## [2026-09-19]

### Added

- **Users settings** — Settings → System → Users: change display name and
  `~/.face` avatar (synced with Metis Menu), change password, list other local
  users, add/remove accounts, and toggle Administrator (`sudo` group). Privileged
  ops use PolicyKit via `metis-remote` (`org.metis.accounts.*`).
- **Date & Time settings** — Settings → System → Date & Time: automatic NTP,
  automatic timezone (IP geolocation), manual date/time and timezone sheets,
  12/24-hour bar clock format, and first day of week (`datetime.json` + calendar).
  System changes go through `timedatectl` / `org.metis.datetime.*` Polkit actions.
- **Selectable Metis Menu layouts** — Settings → Metis Menu offers Metis
  (today’s classic), Whisker, ArcMenu, and Mint layout thumbnails, plus toggles
  for user avatar/name, the places/power rail, and pinned apps. Presets and
  flags live in `menu.json`; changing them sends `reload-bar` so the shell
  rebuilds the menu live. Missing style fields keep the Metis default.
- **Settings UI 2.0** — Home overview with category tiles, a compact icon
  sidebar, and category panels via a GtkStack slide (not an Overlay — GTK 4.22
  overlays were eating pointer events). Search on Home filters tiles and lists
  matching pages. Confirmations and pickers (colour / font, Keep display
  settings, Wi‑Fi password, Known Wi‑Fi, OpenVPN / WireGuard add, Calendars add
  account, Remote password) drop in from the **top** as in-window sheets.
  Network adds a dedicated **DNS** tab; Wi‑Fi and Bluetooth lists use zebra
  rows. `--page` still deep-links into the right category.
- **System wallpapers in Settings → Background** — the picture picker lists
  distro/DE backgrounds from `/usr/share/backgrounds` (and related data dirs)
  after Your pictures and Metis bundles, without copying them into the Metis
  store. Paginated grid (9 per page) with All / Metis / System filters; thumbs
  decode off-thread at preview size with caching and next/prev preload so page
  flips stay responsive.

### Fixed

- **Wallpaper flash on change** — switching picture / solid / gradient in
  Settings no longer clears the desktop to black while the next image decodes.
  The compositor keeps the previous GPU texture on screen, then crossfades into
  the new background (~280 ms ease-out). Apply is faster too: cover-crop no
  longer resizes the full source before cropping, decoded RGBA is cached under
  `~/.cache/metis/wallpaper-rgba/` (warmed while browsing thumbs), and Settings
  sends `ApplyBackground` off the GTK thread.
- **Settings stays running after close** — closing the Settings window now tears
  down chrome state, hides leftover dialogs, quits the unique-instance
  `GtkApplication`, and hard-exits so perpetual page timers (network / Bluetooth
  / power polls) cannot leave a zombie `metis-settings` process.
- **Bottom edge bar freezes Settings / bar** — maximized windows (including
  Settings) no longer enter a configure storm when the bar is on the bottom.
  `reclamp_maximized_geometry` was treating CSD shadow `bbox` overflow past the
  reserved strip as a permanent mismatch and re-sending configures on every
  commit. It now compares client geometry only.
- **Settings scroll freezes (Windows / Display)** — multi-second “lock then
  catch-up” on tall pages under GTK 4.22 / hybrid NVIDIA. Settings defaults to
  `GSK_RENDERER=cairo` (override with `METIS_SETTINGS_GSK_RENDERER`); strips
  touchpad kinetic flags on scrollers; Display hotplug/`list_outputs` no longer
  blocks the GTK thread; compositor IPC connect is timed out; `reload-bar` is
  debounced. Arrangement preview no longer uses a nested `ScrolledWindow` that
  ate wheel events.

## [2026-08-08]

### Fixed

- **X11 borderless games off-screen** — game windows that grow large/undecorated
  re-center or go fullscreen instead of clipping under the edge bar. Steam/Lutris
  splash windows are excluded from that path (they animate size while loading).
  Proton/`steam_app_*` defaults request fullscreen on map.

### Added

- **Phase 19 — Gaming Setup UX** — guided Steam / Vulkan / controllers /
  GameMode / GPU mode wizard; health rows for Mesa Vulkan (amd64), NVIDIA
  (PCI + reboot banner), and `steam-devices`; consent-only
  `pkexec metis-remote pk-ubuntu-drivers-install`; Settings UI for
  `extra_steam_paths`; Metis-owned `mangohud_for_games` /
  `gamescope_big_picture` toggles (spawn-time only, no Steam VDF writes).
- **Phase 18 D — Soft X11 isolation** — Settings → Gaming toggle for
  `xwayland_mode` (shared / isolated); gaming-class launches use a lazy-spawned
  second XWayland independent of GPU/battery; `xwayland_policy` patterns in
  `config.json`.

### Security

- **Phase 18 D** — policy-driven two-bucket XWayland soft isolation (not a
  sandbox). SECURITY residual #4 updated accordingly.

## [2026-08-07]

### Added

- **Phase 19 roadmap** — Gaming Setup UX in `TODO.md`: guided NVIDIA
  (`ubuntu-drivers` + i386) / Mesa Vulkan install with consent + reboot,
  first-run wizard polish, Metis-owned Gamescope tweaks; explicitly rejects
  Steam Launch Options autofill and silent driver installs.

### Security

- **Phase 18 C — Widget pack schema gate** — discovery rejects unknown JSON
  fields, invalid `widget.json`, and missing declared helpers; skipped packs
  warn + toast at widgets startup (host never crashes). Author schema:
  `docs/WIDGET_PACK_SCHEMA.md`. SECURITY residual #3 done.
- **Phase 18 B — IPC rate limits** — sliding 1s windows on compositor command
  IPC, event subscribe (max 16 subscribers), and command-file write/dispatch;
  excess socket requests get `Error` / `rate limited`. Widgets `METIS_IPC_TOKEN`
  documented as spawn-scoped (no wall-clock TTL). SECURITY residual #2 done.
- **Phase 18 A — Gaming path / env sanitization** — `extra_steam_paths` in
  `gaming.json` are fail-closed (canonicalize; directories only; allowed under
  `$HOME`, `/mnt`, `/media`, `/run/media`). Flatpak `--env` and `launch-steam`
  exports allowlist GPU offload keys and reject unsafe values; shell exports are
  POSIX single-quoted. SECURITY residual #1 marked done.

### Fixed

- **Screen recording stop bar** — compositor capture excludes the
  `metis-screenshot-record` layer (same as the screenshot picker), so the
  bottom Stop control no longer appears in whole-screen recordings.
- **Flatpak Steam library mounts** — `optimize_flatpak_gaming` now reads
  sanitized `extra_steam_paths` from config instead of an unused empty slice.

## [2026-08-06]

### Added

- **Task View (Super+Tab)** — unified Exclusive overlay: large live thumbnails
  of apps on the current workspace (Tab / Shift+Tab cycle, Enter or click to
  activate) plus a bottom shelf of live workspace mini-desktops (wallpaper +
  non-minimized windows). Drag an app onto a shelf tile to move it. Esc or a
  second `Super+Tab` dismisses. `Alt+Tab` is unbound by default (remap
  Task View next/prev in Settings if desired).
- **Phase 18 roadmap** — security polish backlog in `TODO.md` / `SECURITY.md`:
  gaming path canonicalize, IPC rate limits, widget pack schema gate, per-sandbox
  XWayland stretch; explicitly defers wayland-rs ObjectData wrappers and unprofiled
  grid memoization.
- **App launch single-flight** — launcher and pinned-dock starts suppress
  duplicate clicks for the same desktop id until a matching window maps (or ~8s
  timeout). The dock icon pulses while starting; a short “Starting …” toast
  appears after ~1s if still pending. Taskbar **New window** bypasses the gate.

### Fixed

- **Task View click / drag** — compositor `close-popovers` on every pointer
  press was tearing down the overlay on button-down (Enter still worked). Task
  View is excluded from that path, owns full-output pointer hits like the
  screenshot overlay, and no longer dismisses via `close-popovers`. Click an
  app to focus; drag to a shelf tile without the overlay closing; empty
  backdrop click dismisses.
- **Task View close control** — each app card has a top-right × that closes
  that window via `CloseWindow`; the overlay stays open and rebuilds.
- **Task View drag preview + shelf refresh** — dragging an app shows a mini
  card that follows the cursor; after drop (or close), workspace shelf thumbs
  are recaptured so mini-desktops update.
- **Task View shelf stacking** — workspace thumbs render windows in live
  `space` raise order so focusing an already-open app updates which window
  sits on top in the mini-desktop. Drag preview uses an in-overlay ghost
  (native Wayland drag icons are unreliable on Exclusive layer-shell).
- **Task View drag hang** — native `GtkDragSource`/`set_icon` froze the
  Exclusive overlay after the ghost appeared. App→workspace moves now use
  `GestureDrag` + a Fixed-layer ghost; UI rebuilds are deferred until drag ends.
  Short click activates; press-and-drag past `gtk-dnd-drag-threshold` moves.
- **Task View docs** — README screenshots restored; USER_GUIDE / README / TODO
  updated for Super+Tab Task View (Alt+Tab unbound by default).
- **Task View live previews** — app cards use per-window compositor PNGs
  (`CaptureWindowThumbs`); shelf tiles use `CaptureWorkspaceThumbs` (wallpaper +
  windows, including inactive workspaces).
- **GitHub Desktop / Electron multi-minute relaunch hang** — the compositor kept
  exited client processes as zombies (`child_processes` was only `try_wait`’d for
  desktop-widgets). Chromium ProcessSingleton treats a zombie PID as still alive
  and can block the next launch for minutes notifying that “primary”. Housekeeping
  now reaps all exited clients every tick.
- **PR CI quality gate** — `.github/workflows/ci.yml` runs `cargo deny`,
  `fmt --check`, `clippy -D warnings`, and `cargo test --workspace` on every
  pull request and push to `main`/`master`.
- **`cargo-deny` policy** — `metis-os-workspace/deny.toml` (advisories, licenses,
  git source allowlist); wired into CI and all `release-deb` suite jobs.
- **Trust-boundary unit tests** — `metis-protocol` runtime dir / private file /
  argv split / peercred / command-file allowlist; compositor `ipc_dispatch`
  caps matrix; portal clipboard allowlist, screencast refuse-while-locked, and
  screensaver peer reclaim.
- **Runtime command allowlist** — validated verbs and max length for
  `$XDG_RUNTIME_DIR/metis/` command files (same-UID trust model documented in
  USER_GUIDE / UBUNTU_DEV).

### Changed

- **Compositor IPC dispatch extracted** — `ipc_dispatch.rs` owns capability
  resolution and widgets/lock gating for testability.
- **Shell status poll** — NetworkManager D-Bus signals mark network dirty;
  battery/Bluetooth use slower ticks with adaptive sleep. Opt-in GPU GSK via
  `METIS_SHELL_GSK_RENDERER=gl` (Cairo remains default).
- **PERF_AUDIT refreshed** — documents ScreenCast dmabuf, `panic = "abort"`,
  and current shell/GSK notes (2026-08-02).
- **Packaging CI** — locale `.mo`/`.ftl` smoke checks on Ubuntu 24.04, 26.04,
  and Debian 13 release jobs; `cargo deny` replaces `cargo audit` there.
- **`clippy::unwrap_used` denied** on `metis-protocol` and `metis-config`
  (non-test builds).
- **Workspace clippy clean-up** — idiomatic fixes across config/grid/capture/
  gaming/remote/compositor/portal/settings/shell so PR CI (`-D warnings`) stays
  green (derive `Default`, collapsible match/if, `clamp`/`contains`, etc.).
- **`SECURITY.md`** — root trust-model map (IPC / XWayland / gaming / colour
  management / residual targets) with README pointers for auditors.

### Fixed

- **Compositor hot-path panics** — event-loop unwraps in xdg shell, desk input,
  state, input, and udev paths warn and recover instead of aborting the session
  under `panic = "abort"`. Colour management stays default-off.

## [2026-07-31]

### Added

- **`metis-screenshot` editor** — post-capture annotate, OCR (`tesseract`), pin
  sticky window, and MP4 region recording (FFmpeg H.264 with MPEG-4 fallback).
  Interactive PrtSc defaults
  to **Edit**. The toolbar uses a built-in Cairo icon set (no icon-theme
  dependency) with a shared colour/size popover, a fit-to-window canvas, real
  pixelation and crop, live on-canvas text boxes with a blinking caret that can
  be moved and resized, undo/redo history, and
  Ctrl+Z / Ctrl+Shift+Z / Ctrl+C / Ctrl+S / Esc shortcuts.
- **Selectable OCR results** — Extract Text scans the complete edited image and
  opens recognised text in a selectable view with Copy Selection and Copy All.
- **OCR runtime included by installers** — Debian/Ubuntu packages and dependency
  profiles require `tesseract-ocr` plus English data; Arch requires `tesseract`
  and `tesseract-data-eng`.
- **Recording runtime included by installers** — FFmpeg is now a required
  package because screen recording is a standard screenshot feature.
- **Settings → System → Screenshot** — defaults for mode, pointer, delay,
  after-capture, save folder, and pointer-output targeting.
- **Per-output PrtSc targeting** — compositor appends the connector under the
  pointer; picker and capture use that output.

### Fixed

- **Ubuntu 26.04 `.deb` install failed on `libdisplay-info1`** — resolute ships
  `libdisplay-info3`; the package `Depends` now accepts
  `libdisplay-info3 | libdisplay-info2 | libdisplay-info1` (same OR chain on
  Debian 13). Rebuild/re-download the `…ubuntu26.04.deb` after this change.
- **Auto-hidden edge bar left a thick strip on screen (Ubuntu 26.04)** — the bar's
  layer surface is sized from the content's natural size, so a taller GTK/theme
  minimum grew it past `bar_body_thickness()` while the slide still used the
  configured thickness, parking the overflow on screen. The slide now uses the
  measured/allocated extent, and the compositor skips the bar's backdrop blur
  while the bar is auto-hidden.
- **Settings sidebar icon badges** — coloured nav/page icon badges are the default
  (with a gray fallback when no hue is set). Icons are centred with equal CSS
  padding instead of fixed-size wraps that pinned glyphs to the top-left.
- **Screenshot editor annotations landed in the wrong place** — `GestureDrag`
  reports offsets from the press point, but the canvas treated them as widget
  coordinates, so arrows, rectangles, and ellipses collapsed toward the top-left
  corner. Pointer positions now map through the fit transform to image pixels.
- **Screenshot editor showed two titlebars** — the editor drops its GTK titlebar
  under a Metis session, and `com.metis.Screenshot` / `com.metis.Viewer` are
  pinned to server-side decorations.
- **Saved annotations no longer differ from the preview** — export renders the
  same Cairo pass as the canvas instead of re-tracing strokes as raw lines, so
  shapes, highlighter, text, and colours survive Copy / Save / Save As / Pin.
- **Stopped recordings are finalized before the controller exits** — Stop now
  remains open in a “Saving MP4…” state until FFmpeg writes the trailer, avoiding
  truncated, unplayable videos.
- **PrtSc over Metis Menu / Control Center / Notification Center** — screenshot
  chords fire globally even while Exclusive shell layers own the keyboard.
- **Screenshot no longer dismisses shell chrome** — edge-bar popovers, Control
  Center, and Notification Center stay mapped under the picker so hide-before-capture
  can include them.
- **Selection drag no longer tears down the UI being captured** — while the
  screenshot picker owns the pointer, presses are not treated as outside clicks,
  so `close-popovers` is not broadcast. Edge-bar auto-hide reveal/hide is also
  frozen, so the shot matches what was on screen when PrtSc was pressed.
- **Screenshot options persist** — mode, include-pointer, delay, and after-capture
  write to `screenshot.json` as soon as they change in the overlay.
- **`screenshot instant-full` / `window` runtime commands** — shell parsing used
  only the first token, so Shift/Ctrl+PrtSc never reached those launch modes.
- **Removed scroll capture mode** — the stitched vertical screenshot path and its
  picker/settings entry are gone; leftover `default_mode: "scroll"` in
  `screenshot.json` maps to Selection.
- **Interactive after-capture defaults to Edit** — Settings no longer stores the
  Instant (Shift+PrtSc) choice in the interactive field, and the picker options
  segment leads with Edit so PrtSc opens `metis-screenshot` by default.

## [2026-07-30]

### Fixed

- **Edge bar auto-hide overlays windows** — peek/reveal no longer resizes
  maximized or snapped windows. With `auto_hide` on, the bar draws over the
  desktop; layout only reflows when auto-hide itself is toggled.
- **Top auto-hide vs titlebar controls** — while peeked, only the thin edge
  strip opens the bar; the titlebar band just below stays usable. The auto-hidden
  full-size layer surface no longer steals clicks from traffic-light buttons.
- **Edge bar squashed after moving position** — remount resets the Control Center
  host size/visibility and re-applies strip geometry so the pill keeps its height.
- **Ubuntu 26.04 / Debian 13 layer-shell package name** — use
  `libgtk4-layer-shell-dev` (not `libgtk-4-layer-shell-dev`).
- **PipeWire 1.x builds** — `metis-portal` uses `pipewire` 0.9 (fixes `libspa`
  `spa_pod_builder` bindgen breakage on newer headers).
- **Debian CI libclang** — install `libclang-dev` so `libspa-sys` bindgen can run
  in bare `debian:trixie` images.
- **gtk4-layer-shell fetch in CI** — `scripts/fetch-gtk4-layer-shell.sh` downloads
  the release tarball with retries (git clone fallback) to survive DNS blips.

### Changed

- **Workspace crate versions** — all crates inherit `[workspace.package].version`;
  `scripts/sync-version.sh` maps GitHub `v0.1.0.N` → Cargo `0.1.N` (also run by
  `package-deb.sh`).

## [2026-07-29]

### Fixed

- **Settings single-instance** — edge-bar Network / VPN / Power / Bluetooth
  “Settings…” no longer spawns a second `metis-settings`. A unique GApplication
  (`com.metis.Settings`) forwards `--page` (including `network/vpn`) to the
  running window, navigates there, and asks the compositor to unminimize / raise
  it when minimized.
- **Metis Viewer clipboard abort** — FreeRDP is spawned with `-clipboard` so
  gnome-remote-desktop format-list negotiation cannot kill the session with
  `cliprdr_packet_format_list_new failed!`. Test RDP from another machine (not
  from Viewer inside the same shared session).
- **Deb name collision with Ubuntu `metis`** — the desktop `.deb` is now
  `metis-desktop` (not `metis`). Ubuntu universe already ships a graph-partitioning
  package named `metis` at 5.1.x; `apt upgrade` treated that as a newer version of
  our 0.1.0.x package and replaced the entire desktop. `Breaks`/`Replaces` migrate
  old `metis (<< 1)` installs only.

### Added

- **`./install.sh`** — repo-root bootstrap for Ubuntu 24.04 / 26.04, Debian 13,
  and Arch (deps + optional remote packages + `run-metis.sh --install-session`).
- **Multi-suite debs** — CI builds `ubuntu24.04`, `ubuntu26.04`, and `debian13`
  artifacts via `DISTRO_SUITE` / shared `stage-fhs.sh`.
- **Arch PKGBUILD** — `metis-os-workspace/packaging/arch/` (`makepkg -si`).
- **Nix flake + NixOS module** — `flake.nix`, `nix/metis-desktop.nix`,
  `programs.metis` (`nix/README.md`).

### Changed

- **Packaging upgrade guidance** — Prefer `sudo apt install ./metis-desktop_….deb`
  after logging out of Metis; avoid App Center/GDebi local upgrades that can remove
  the package and leave nothing installed (`docs/PACKAGING.md`, `USER_GUIDE`).
- **Deb Recommends for LAN firewall** — `nftables` and `policykit-1-gnome |
  mate-polkit` moved from Suggests to Recommends so a normal `apt install` of the
  Metis `.deb` pulls a PolicyKit agent + nftables (fixes Remote access password
  dialog timeouts on minimal VMs). GRD / FreeRDP stay Suggests.

## [2026-07-28]

### Fixed

- **Edge bar auto-hide session crash** — hide is visual-only (CSS slide). No
  exclusive-zone toggle or window reflow on hide/show (that path crashed the
  session). Compositor reveals via `reveal-edge-bar` when the pointer enters the
  bar strip while a shell flag file is set.
- **Edge bar auto-hide timing** — hide starts as soon as the pointer leaves (no
  idle delay). Slide uses a 200ms ease-out transform (pixel offsets — GTK rejects
  `calc()` in `transform`, which previously made auto-hide appear to do nothing).
  Enabling Auto-hide in Settings hides the bar immediately.

### Added

- **Edge bar fill, length, and auto-hide** — Settings → Edge bar: theme/solid/
  gradient background (with direction), centered length 40–100%, and classic
  auto-hide with a configurable peek strip. Live-reloads via `bar.json`
  (`bar_fill`, `length_percent`, `auto_hide*`).

## [2026-07-27]

### Added

- **Startup applications** — empty-by-default `startup.json` + Settings → System →
  Startup (master toggle, per-app enable, app picker). Compositor resolves
  desktop ids to argv and launches ~2s after session start (skips onboarding /
  lock). No custom command lines; Flatpak portal autostart still deferred.
- **IPC accept helper** — `metis_protocol::accept_same_euid` centralizes
  `SO_PEERCRED` same-euid gating for compositor command + event sockets.
- **Super+T opens the default terminal** — new `open_terminal` keybind (default
  Mod+T) launches the terminal from Settings → Metis Menu (`resolve_terminal`).
  Editable under Keyboard → Shortcuts.
- **Dim on battery** — `power.json` → `dim_on_battery` drives a light compositor
  overlay when on battery (skipped on HDR-active outputs); live-reloads.
- **Widget out-of-process helpers (§E.2)** — packs may declare `helper.exec` +
  `poll_seconds`; argv-only spawn with timeout/stdout caps; `{helper.*}` tokens.
  Example: `com.metis.example.helperstatus`. Scripts/WASM still deferred.
- **Cross-GPU primary→secondary transfer** — hybrid outputs composite on the
  primary GPU (blur on) and present on the secondary; falls back to local
  renderer with blur off if transfer fails.
- **Per-surface HDR pass-through** — when colour-management clients advertise
  PQ/HLG, skip SDR→HDR re-encode on HDR outputs (requires `METIS_COLOR_MGMT=1`).
- **`metis-remote rustdesk`** — optional RustDesk backend (`status|enable|disable`)
  + LAN firewall helpers; Settings card drives it. GRD remains default host.
- **Native remote host decision** — `docs/decisions/remote-host-native-vs-grd.md`
  (research only; keep GRD).
- **Control Center — Battery + Logs** — optional overview cards: battery charge
  history sparkline (hidden on desktops without a battery) and a compact
  `journalctl` log tail (unavailable when journalctl is missing or denied).
  Toggle in Settings → Control Center (`dashboard.json`).
- **Widget extension live host binds** — JSON packs can use `{host.time}`,
  `{host.date}`, `{host.weather.temp|unit|summary}`, `{host.sys.cpu|mem|disk}`;
  labels refresh live. Not interpolated into `open_uri` / `launch` action
  fields. Example pack shows `{host.time}`.
- **`ext-session-lock-v1`** — optional third-party lockers (swaylock, gtklock,
  …). Metis compositor PAM lock (`Super+L`) remains the default; only one lock
  owner per cycle. Protocol unlock runs the same side effects as PAM unlock
  (RDP resume, capture/inject denylist).

### Deferred

- **Default-on `wp_color_management_v1`** — still blocked on upstream wayland-rs
  server/sys ObjectData UAF ([docs/upstream/](docs/upstream/README.md)); keep
  `METIS_COLOR_MGMT=1` opt-in.

### Fixed

- **Maximize key-repeat / wobble lockup** — holding the maximize shortcut no
  longer restarts the wobble forever or floods configure/redraw. Debounced
  toggle (~400ms), in-flight FX is not timer-reset, and move/resize grabs
  clear the wobble so the window can be dragged after unmaximize.
- **Maximize wobble restored** — wobble is enabled again for all apps
  (including Chromium/Electron). A short-lived titlebar button grab no longer
  cancels the animation on the first tick.

### Improved

- **Browser / windowed web-game performance** — pointer motion over Firefox /
  Chromium-family windows is no longer throttled (~20 Hz) so canvas/WebGL input
  stays smooth without fullscreen. Auto GPU mode steers Chrome/Firefox/Edge/
  Brave/… onto the dGPU when on AC (same battery-iGPU preference as games;
  browsers do not trigger GameMode). `MOZ_ENABLE_WAYLAND=1` is set for spawned
  clients when unset.
- **Onboarding resumable** — mid-wizard progress is stored as `onboarding_step`
  in `config.json` and restored after a session restart; Finish/Skip clears it.
  Settings "Run setup again" resets to step 0.

## [2026-07-26]

### Added

- **Phase 14 §E Widget Extension API v1** — declarative JSON desktop widgets:
  `manifest.json` + `widget.json` under `~/.local/share/metis/widgets/<id>/`
  (also `/usr/share/metis/widgets/`). Host renders layout DSL; actions
  `open_uri` / `launch` / `copy_text`. Settings Add lists discovered packs;
  schema-driven configure. Example: `com.metis.example.quicklinks`. No scripts
  or in-process plugins.

### Security

- **Widget extension host hardening** — `open_uri` allowlists `http`/`https`
  only (no `file:` / bare paths / UriLauncher→FileLauncher fallback); `launch`
  accepts desktop ids or a single PATH basename (no absolute paths, argv, or
  shell metacharacters; shells/interpreters denylisted); action fields are not
  settings-interpolated; `widget.json` size/depth/node caps; actions validated
  on load and again at click.
- **Phase 15 §F Metis Viewer** — first-party RDP client (`metis-viewer`): GTK
  connect UI spawns FreeRDP under `/usr/bin` (argv-only; no shell). Recent hosts
  in `viewer.json` without passwords (one-click connect + remove). Early FreeRDP
  failure feedback; dedicated `metis-viewer` icon. Settings → Remote access →
  **Connect with Metis Viewer…** (soft-warns if FreeRDP/sharing/password missing);
  `metis-cmd viewer`. Host sharing remains GRD + `metis-remote`.
- **Phase 15 §F RustDesk Settings preset** — Settings → Remote access card:
  detect system/Flatpak install, **Open RustDesk**, copy install instructions,
  portal/ports notes. No `metis-remote` backend / firewall orchestration.
- **Phase 15 §F lock fingerprint + YubiKey** — lock screen soft-detects readers /
  Yubico USB keys, shows touch cues, allows empty-password PAM + one auto-attempt
  for host `pam_fprintd` / `pam_u2f`. Password path unchanged; enrollment stays
  on the host.
- **Phase 15 §F image RDP clipboard** — portal Mutter clipboard shim syncs
  `image/png|jpeg|bmp` both ways (≤10 MiB, path-allowlisted under runtime
  `clipboard/`). Text path unchanged.
- **Phase 3 multi-GPU hardware validation** — closed on hybrid iGPU+dGPU: HDMI
  projector output, gaming PRIME offload (fast), pointer/input stable. Explicit
  Smithay `MultiRenderer` primary→secondary transfer remains deferred.
- **Phase 5 HDR stretch** — SDR→HDR encode applies Rec.709→BT.2020 before PQ;
  mastering metadata uses BT.2020 primaries and prefers DRM BT.2020 Colorspace.
  EDID HLG detection enables HDR on HLG-only panels with HLG encode (EOTF=3);
  ST.2084 still preferred when both are present. VT resume dirties ICC profiles
  and invalidates LUT / HDR GL programs so they rebake after DRM pause.

### Fixed

- **Settings scroll hitch / wallpaper bleed** — page and nav scrollers are opaque
  again; removed section hover box-shadow and page-icon scale transitions that
  forced wallpaper re-composite under an ARGB Settings window while scrolling.
  Modal password/widget sheets still use transparent windows for rounded cards.
- **Settings Display ICC clear** — label reads “Default (sRGB)” (removed stale
  “pipeline apply pending” copy).

## [2026-07-25]

### Security

- **cargo audit CI** — bump `crossbeam-epoch` 0.9.20, `quinn-proto` 0.11.15,
  `quick-xml` 0.41 (direct + via `wayland-scanner` 0.31.11) to clear
  RUSTSEC-2026-0204 / 0194 / 0195 / 0185.
- **Phase 15 — Security hardening** — `cargo audit` in release + PR CI;
  `overflow-checks` + `panic = "abort"` on release profiles; argv-only Launch
  (no `sh -lc`); Metis Polkit policies + `metis-remote pk-apt-install` /
  `pk-add-input-group`; NM SSID/id sanitizers; lock blocks Ctrl+Alt+Fn VT switch
  (Backspace quit kept); desktop-widgets IPC capability token; opt-in
  `xwayland_mode: isolated` gaming XWayland bucket. Stretch §F (viewer, RustDesk
  preset, image clipboard, lock fingerprint/YubiKey) shipped 2026-07-26.
- **Phase 7 remote security closeout** — LAN-only RDP firewall enforcement
  (`metis-remote firewall` via nftables/ufw + Settings toggle); RDP password via
  stdin (not Metis argv); lock pauses RDP listen and unlock resumes if still
  enabled; RDP clipboard text-only (no local image mimes); XWayland abstract
  socket default-off (`config.json` `xwayland_abstract_socket`).
- **IPC hardening** — `$XDG_RUNTIME_DIR/metis` is `0700`; compositor sockets and
  command/clipboard files are `0600`; accepts require same-UID `SO_PEERCRED`;
  insecure `/tmp/metis` fallback removed (fail closed if `XDG_RUNTIME_DIR` is
  unset). Lock screen IPC denylist expanded (close window, end session,
  workspace moves, screenshot overlay, inject, …).
- **Flatpak gaming optimize consent** — Settings shows a permission dialog before
  overrides; `metis-cmd optimize-gaming` requires `--yes` (or
  `METIS_GAMING_OPTIMIZE_YES=1`); shell/gamingd ignore bare optimize commands.
- **Calendar keyring hygiene** — removing a calendar account deletes its CalDAV
  password / M365 refresh token from the Secret Service.
- **Docs** — USER_GUIDE / UBUNTU_DEV / PACKAGING cover IPC trust model, Polkit,
  C ABI residual risk, and X11 isolation modes.

### Fixed

- **Remote access LAN firewall UX** — Enabling sharing with **LAN only** on
  applies firewall rules automatically; status and **Retry** live under
  Security (no separate “Apply LAN firewall…” under sharing). `pkexec` is
  wrapped with a timeout so Settings cannot stick on “Applying…” when no
  PolicyKit agent is present; failures surface under Security. USER_GUIDE
  updated (PolicyKit agent / nftables troubleshooting).
- **GitHub Desktop / Electron dark-mode portal parse** — Settings portal
  `org.freedesktop.appearance` `color-scheme` was serialized as signed `i`
  (ashpd default) instead of the spec type `u`. Chromium's
  `DarkModeManagerLinux` failed `PopVariantOfUint32` (`dark_mode_manager_linux.cc`).
  `metis-portal` now returns and emits uint32. Verify with
  `busctl … Settings Read … color-scheme` → `v v u …`.
- **Folders delete/rename hollow dialog** — toplevel GTK windows inherit the
  shell's transparent `window` fill, so confirms showed wallpaper through the
  frame (and Metis SSD ghost chrome). Delete / Rename now use opaque Popovers
  on the folders card with a Cairo-painted panel.
- **Wi-Fi radio killed on HDMI plug/unplug** — flaky GTK switch notifies and
  Settings poll could run `nmcli radio wifi off`. Radio-off now requires a
  real click arm in the bar and Settings; unarmed offs are refused at the
  nmcli spawn site.

### Added

- **Phase 5 colour protocol closeout** — product Display pipeline remains
  closed (ICC `vcgt` / Stage 2 3D-LUT, HDR H1–H3, VRR, night light). Default-on
  `wp_color_management_v1` stays deferred: reconfirmed that `wayland-backend`
  0.3.16 only fixes client/sys races (`server_impl` unchanged), so Metis keeps
  `METIS_COLOR_MGMT=1` opt-in. Filed upstream as
  [wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949).
  USER_GUIDE notes the gate.
- **Phase 5 complete (HDR H3 / Stage 2 colour)** — ICC profiles bake a 33³
  sRGB→display GLES 3D-LUT (`lcms2`); DRM scanout uses a unified post-pass
  (high-bit offscreen when available → optional LUT → optional SDR→PQ). CRTC
  `vcgt` is skipped when the LUT owns the output. Opt-in
  `wp_color_management_v1` (`METIS_COLOR_MGMT=1`) advertises PQ/HLG/BT.2020 and
  records parametric TF/primaries; remains default-off (upstream wayland-rs
  **server/sys** ObjectData UAF; 0.3.16 does not fix it).
  Requires `liblcms2-dev` to build.
- **Phase 5 HDR encode (H2)** — with HDR on, the DRM compositor composites the
  SDR desktop offscreen and blits through an sRGB→linear→ST.2084 PQ shader
  (203‑nit reference white) so the panel shows correctly mapped HDR10 rather
  than raw SDR levels. Falls back to the prior scanout path if encode fails.
- **Phase 5 HDR foundation (H1)** — DRM sessions require monitor EDID ST.2084/
  HDR10 plus `HDR_OUTPUT_METADATA`, expose Settings → Display **HDR** (live) and
  `outputs.json` `hdr_enabled`, and apply/clear Default/RGB Colorspace + HDR10
  static metadata (+ `max_bpc=10`). Logs the negotiated primary-plane Fourcc
  (10-bit preferred). Night light is skipped on HDR-active outputs.
- **Phase 3 Stage G multi-GPU DRM backend** — compositor now opens every GPU on
  the seat through Smithay `GpuManager`, keeps DRM scanners/output managers/CRTC
  surfaces per device, renders each output with its device renderer, early-imports
  client buffers on the primary GPU, and handles secondary GPU hotplug/removal.
  Cross-GPU outputs suppress the GLES-only blur effect; context-bound wallpaper
  and decoration caches are rebuilt when switching render nodes. Explicit
  `MultiRenderer` transfer remains a documented fallback caveat pending a generic
  render-element stack and hardware validation.
- **ScreenCast dmabuf zero-copy (Phase 3)** — DRM compositor advertises capture
  dmabuf formats and renders into client GBM buffers; `metis-portal` prefers
  linux-dmabuf + PipeWire `SPA_DATA_DmaBuf` with MemFd fallback. Removes the
  per-frame GLES `copy_framebuffer` readback on the live cast path.
- **Settings → Shortcuts** — searchable, grouped read-only guide of current
  desktop chords (plus reserved System binds); jump to Keyboard to edit.
- **Desktop-widget process isolation** — compositor spawns
  `metis-shell --desktop-widgets` beside the edge bar; widgets own Gio watches,
  weather, and `$XDG_RUNTIME_DIR/metis/command-widgets` so a widget hang cannot
  freeze the bar. Rate-limited respawn if the widgets process exits.
- **Phase 8 complete** — remaining shell/settings/onboarding strings routed
  through `tr()`; live Apply rebuilds desktop widgets; catalogs extracted for
  `.deb` CI. English msgid fallback for any still-untranslated entries.
- **Phase 7 remote docs** — RustDesk host install, ports, Wayland/portal
  verification checklist; expanded remote compatibility matrix; multi-user / VT
  behaviour (`docs/UBUNTU_DEV.md`, USER_GUIDE pointer).
- **ScreenCast multi-monitor** — Mutter `RecordMonitor(connector)` and the live
  PipeWire pump select the matching `wl_output` by name (`metis-capture`,
  `metis-portal`). GRD already used the portal path; this makes per-display
  capture correct on multi-head DRM sessions.

### Fixed

- **App titlebars overrides reset on reopen** — opening Settings → App titlebars
  pruned intentional overrides for Text Editor, Metis Settings, and GitHub
  Desktop (one-time cleanup list left in place). Prune now only drops junk keys.
- **HDR toggle on SDR laptop panels (washed-out desktop)** — `hdr_available`
  now requires the monitor EDID to advertise ST.2084/HDR10, not merely a DRM
  `HDR_OUTPUT_METADATA` property (common on Intel/AMD eDP). Settings clears a
  stale `hdr_enabled` when the panel is not HDR-capable.
- **HDR looked warm / tan on laptop panels** — SDR→PQ path was forcing BT.2020
  Colorspace and BT.2020 mastering metadata without a gamut convert, and CRTC
  gamma still ran on the PQ signal. Keep Default/RGB Colorspace, Rec.709
  mastering + ~203‑nit CLL, and identity gamma while HDR is active.
- **Metis Menu hover flickered search + titlebars** — with the grab-less menu
  open, pointer motion over the app list ran `maintain_focus_stacking` into
  windows under the translucent popover, ping-ponging Exclusive keyboard focus
  with the bar layer; search `:focus-within` and SSD focused/unfocused chrome
  flashed every move. Skip stacking/focus restore (and auto-hide titlebar
  reveal) while the pointer is over bar UI or an Exclusive layer owns the seat.
- **Desktop widgets stayed English after Language Apply** — `reload()` short-
  circuited when `desktop-widgets.json` identity was unchanged; locale Apply now
  force-rebuilds hosts (`metis-shell`).
- **Weather overlay English conditions** — translate WMO labels / hour chips at
  paint time from codes, not baked English snapshot strings.

## [2026-07-22]

### Fixed

- **Settings closed on Language Apply** — live rebuild borrowed `REBUILD_UI`
  while still inside `rebuild_ui_for_locale`, causing a `RefCell already borrowed`
  abort; rebuild is now idle-deferred and holds GApplication across window
  recreate (`metis-settings`).
- **Language Apply did nothing** — gettext fell back to `C.UTF-8` when the host
  lacked e.g. `es_ES.UTF-8`, which makes GNU gettext ignore `LANGUAGE`; Metis
  now binds a non-`C` UTF-8 locale and selects catalogs via `LANGUAGE`. Settings
  also rebuilds its UI after Apply, and session install copies
  `/usr/local/share/metis/locale` catalogs (`metis-i18n`, `metis-settings`,
  `run-metis.sh`).
- **Skeleton catalogs for all Settings picker languages** — `fr`, `de`, `pt`,
  `it`, `nl`, `pl`, `ru`, `ja`, `zh`, `ko`, `ar`, `he` (plus refreshed `es`) with
  gettext `.po`/`.mo` and compositor Fluent `.ftl` under `assets/locale/`.
  `package-deb.sh` and release CI ship them under `/usr/share/metis/locale`.
- **Metis Menu / tray chrome** — rail actions and menu section labels use
  `tr()` so `reload-locale` can switch them (Settings nav was already covered).
- **Shell + Settings UI externalized** — Control Center, screenshots,
  notification center, bar sys/VPN/volumes, clock widgets, and Settings page
  bodies wrap user-facing strings with `tr()`; **all shipped language catalogs**
  (`es`/`fr`/`de`/`pt`/`it`/`nl`/`pl`/`ru`/`ja`/`zh`/`ko`/`ar`/`he`) filled for
  the expanded msgid set (`metis-shell`, `metis-settings`, `assets/locale`).

### Added

- **Phase 8 — Hybrid i18n** — `metis-i18n` crate (gettext for shell/settings,
  Fluent for compositor); `locale.json`; Settings → Language & region; onboarding
  language step; Spanish skeleton catalogs; `docs/I18N.md`; live `reload-locale` /
  `ReloadLocale` (`metis-i18n`, `metis-config`, `metis-settings`, `metis-shell`,
  `metis-compositor`, `metis-protocol`).

## [2026-07-21]

### Added

- **Edge-bar VPN widget** — dedicated `network-vpn-symbolic` icon with its own
  connect/disconnect popover (no longer nested under Network). Existing
  `bar.json` layouts gain a `vpn` entry after Network on next shell start
  (`metis-config` `bar.rs`; `metis-shell` `sys.rs`).
- **VPN connect feedback** — Settings and the edge-bar VPN popover show
  Connecting… / Disconnecting… with spinners and surface `nmcli` errors;
  OpenVPN-missing plugin banner on the VPN tab; **Edit** for WireGuard
  profiles; bar **VPN Settings…** opens `--page network/vpn`
  (`metis-settings` `net.rs`, `pages/network.rs`, `main.rs`; `metis-shell`
  `poll.rs`, `sys.rs`).
- **NetworkManager VPN (OpenVPN + WireGuard)** — Settings → Network → **VPN**
  tab lists NM profiles with Connect / Disconnect / Delete, **Import…** for
  `.ovpn` / WireGuard `.conf`, and **Add WireGuard…** for a simple peer form.
  Profiles created in Cinnamon or `nmcli` appear automatically. OpenVPN needs
  `network-manager-openvpn` (`metis-settings` `net.rs`, `pages/network.rs`;
  `metis-shell` `poll.rs`, `ui/bar/widgets/sys.rs`).

### Fixed

- **Settings window min width** — Network (and other) hint labels wrap with a
  capped width, page scrollers no longer propagate child natural width, and VPN
  rows tuck Auto-connect under the profile name so Settings can shrink below
  the default 960px (`metis-settings` `ui.rs`, `pages/network.rs`,
  `pages/input_common.rs`).
- **Proxy mode dropdown closes immediately** — Network’s 2s refresh was
  rebuilding the Proxy editor (and other DropDown/entry panels) while the
  popover was open; skip auto-refresh on the Proxy tab and only rebuild editors
  when their data actually changes (`metis-settings` `pages/network.rs`).
- **Modal dialog grey corner ears** — Settings sheets (Add OpenVPN / WireGuard,
  VPN password, Remote, Gaming, widget configure) keep a transparent window
  buffer and paint the rounded card on an inner sheet, so corners outside the
  radius are true alpha instead of solid grey (`metis-settings` `theme.rs`,
  `ui.rs`).
- **Import dialog light theme** — Settings Import/Open file choosers follow Metis
  dark/light again (`GTK_USE_PORTAL=0` in Settings; dark mode restores
  `gtk-theme=Adwaita-dark` + portal-gtk `GTK_THEME`; Settings re-syncs gsettings
  on theme apply) (`metis-settings` `main.rs`, `theme.rs`; `metis-config`;
  `metis-portal`; `metis-compositor`).
- **Add OpenVPN…** — Network → VPN gains a password-auth OpenVPN create dialog
  (gateway / user / optional password + CA); full provider configs still use
  Import… (`metis-settings` `net.rs`, `pages/network.rs`).
- **VPN password prompt** — when NetworkManager needs a secret (common for
  OpenVPN profiles imported from another desktop), Settings and the edge-bar
  VPN popover ask for a password, connect via `passwd-file`, and can remember
  it on the NM profile (`metis-settings` `net.rs`, `pages/network.rs`;
  `metis-shell` `poll.rs`, `sys.rs`).
- **WireGuard Edit peer fields** — imported profiles now load peer public key,
  endpoint, and allowed IPs via NetworkManager D-Bus `GetSettings` (`nmcli
  connection export` rejects native WireGuard as “not VPN”)
  (`metis-settings` `net.rs`).
- **VPN Settings polish** — WireGuard **Edit** works on NetworkManager 1.46+
  (peers are read via connection export; `wireguard.peers` is not a readable
  nmcli field). VPN rows align actions, show an exclusive **Auto-connect**
  switch (clears other VPN profiles; shell brings the tunnel up after login
  once the underlay is connected — NM alone often skips WireGuard), refresh
  every 2s so edge-bar disconnects update the list, and treat “already
  disconnected” as success. Edge-bar VPN icon is wider, shows a spinner while
  connecting/disconnecting, and raises a toast + notification on result
  (`metis-settings` `net.rs`, `pages/network.rs`; `metis-shell` `poll.rs`,
  `sys.rs`; `metis-config` `css.rs`).
- **Removable-drive bar polish** — opening a volume's context menu no longer
  grows the icon / expands the edge bar (stable transient popover + fixed
  button geometry). Drive labels prefer the filesystem label or mount-folder
  name instead of GIO's UUID / FAT serial when no friendly name is set
  (`metis-shell` `volumes.rs`, `ui/bar/widgets/volumes.rs`; `metis-config`
  `css.rs`).
- **VPN Settings UI polish** — Add WireGuard dialog is undecorated (no Metis
  SSD over the close control); VPN card actions/list use standard section
  padding (`metis-settings` `network.rs`, `theme.rs`).
- **WireGuard import naming** — provider configs whose filename is not a valid
  Linux interface name (e.g. Proton `Proton-US-FREE-79.conf`, >15 chars) are
  copied to a sanitized temp name for `nmcli import`, then titled with the
  original stem (`metis-settings` `net.rs`).
- **VPN tab deep-link** — `--page network/vpn` selects the VPN stack page after
  tab children exist (pill was active while Wireless content still showed)
  (`metis-settings` `pages/network.rs`).

## [2026-07-20]

### Added

- **Super-key application menu** — tapping Super toggles the Metis Menu.
  Programmatic opens give keyboard focus to the existing search entry, so
  typing immediately filters applications. Super chords still dispatch their
  configured actions (`metis-compositor` `input.rs`, `state.rs`; `metis-shell`
  `menu.rs`, `bar/mod.rs`).
- **Control Center keyboard dismissal** — both `Super+Q` and `Esc` close the
  Control Center. The compositor intercepts `Super+Q` before the normal
  close-window action, and the panel's Escape controller runs in capture phase
  so focused search/dropdown children cannot consume it (`input.rs`,
  `desk_input.rs`, `dashboard/mod.rs`).
- **Multimedia & hardware keys** — the `XF86*` function-row keys are now serviced
  system-wide with a bottom-center on-screen display (OSD):
  - **Volume up / down / mute** and **microphone mute** via PipeWire/PulseAudio
    (`pactl`), with a live level readout.
  - **Display brightness up / down** and **keyboard backlight up / down / toggle**
    through logind `SetBrightness` (no root, udev rule, or setuid helper needed).
  - **Play/Pause, Stop, Next, Previous, Fast-forward, Rewind** via MPRIS D-Bus,
    preferring the currently-playing player.
  - **`XF86Display`** toggles multi-monitor **mirror ⇄ extend** mode (persisted to
    `outputs.json`) with a confirmation overlay.
  The compositor intercepts the keysyms and forwards an intent over the runtime
  command channel; the shell runs the action on a dedicated worker thread and
  flashes the OSD. These keys are fixed (firmware/xkb) and listed under
  Settings → Keyboard → Shortcuts → System for discoverability
  (`metis-compositor` `input.rs`, `state.rs`; `metis-shell` `services/hardware.rs`,
  `ui/osd.rs`, `bar/mod.rs`; `metis-config` `keybinds.rs`, `css.rs`).

### Changed

- **Settings launcher reuses its window** — selecting Settings from the Metis
  Menu now activates an existing `com.metis.Settings` window, including
  restoring it from minimized state and switching to its workspace, instead of
  spawning a duplicate (`metis-shell` `menu.rs`).

## [2026-07-19]

### Fixed

- **Proton mouse-look: camera jumps on left-click (attack)** — two compositor
  bugs stacked on titles like The Mound (`steam_app_2569760`):
  1. **Pause-menu lock latch** — when a lock briefly dropped, Metis treated a
     visible theme cursor (normal in windowed mode) as a pause menu
     (`ClientDeactivated`), then refused to re-arm and killed “spurious”
     re-locks on click. Looking around then moved the desktop cursor; fire
     injected that absolute position. Removed the latch / suppress path;
     inactive locks re-arm while the pointer stays over the surface
     (Mutter/KWin-style).
  2. **Cursor-position-hint click remapping** — Proton streams
     `set_cursor_position_hint` during mouse-look. Remapping locked clicks
     through those hints warped the camera on attack. Hints are now used only
     to restore the desktop cursor when the lock is destroyed — never to
     rewrite locked button delivery. Locked path stays relative motion +
     button + frame only.
  Diagnose with `rg 'game-pointer' ~/.local/state/metis/logs/session-latest.log`
  (`is_locked=true` on fire; no `click remapped` lines).
  (`metis-compositor` `input.rs`, `state.rs`, `handlers/mod.rs`; `USER_GUIDE.md`)

### Changed

- **Gamescope install note** — Ubuntu 24.04 has no `gamescope` apt package;
  document building from source when needed (`USER_GUIDE.md`).

## [2026-07-18]

### Added

- **Desktop widgets (Phase 14 complete)** — Folders (`~/Desktop` or custom path),
  Apps (follows start-menu pins from `menu.json` by default; optional dedicated
  import), Clock, System (CPU / RAM / disk), Weather, and **Equalizer** over the
  wallpaper. Config: `~/.config/metis/desktop-widgets.json` (master switch
  default off). Compositor lock screen covers the session (widgets stay off the
  lock UI).
- **Equalizer desktop widget** — visualizes default-sink audio (PipeWire/Pulse
  monitor). Styles: Spectrum lines, Bars, Neon wave, Radial. Bar shapes:
  Segmented, Solid, Dots, Dense dots. Colour modes: Solid, Gradient
  (start/end), Theme. Bars: height gradient, peak caps + colour, reflection.
  Neon wave: mirror on/off. Settings options switch with the selected style.
- **Clock / Weather / System text style** — per-widget font (family, weight,
  size), text colour, and accent (System progress fills) in the configure
  dialog.
- **Settings → Desktop widgets list UX** — compact zebra-striped rows with kind
  icons; **Add widget** pinned at the top; gear opens a modal for per-widget
  options (keeps the page scannable).

### Changed

- **Folders widget is a grid** — app icons for `.desktop` files (extension kept
  in the label), clickable launch; folders open in the file manager; other files
  use `xdg-open`. Right-click: Open, Open with…, Rename, Delete, New Folder.
- **Folders / Apps view modes** — per-widget Grid or List (Settings). Both sort
  A–Z; Folders lists directories first, then files.
- **Desktop widgets** — free placement (edge snap removed); optional title bar;
  Settings → Desktop widgets includes **Default look** (background
  opacity/colour, border width/colour) plus optional **per-widget overrides**.
  Opacity 0 clears the fill; border width 0 removes the edge.
- **Equalizer colour mode** — “Multi colour” relabelled **Gradient** with
  configurable start/end colours.
- **Desktop widget chrome edits** — opacity/border sliders update card CSS in
  place (no layer-shell teardown), so dragging overrides no longer blanks the
  desktop for seconds.
- **Desktop weather widget** — reads the shared weather snapshot correctly (it
  was stuck on “Waiting…” because the cache lived on the wrong thread).
- **Desktop widget drag** — geometry saves no longer race a file-monitor rebuild
  that yanked cards back.
- **Settings widget page scroll** — changing a setting no longer rebuilds the
  whole list and jumps to the top; most toggles update in place.

### Fixed

- **Minimized windows stay on the task dock** — `ListWindows` now includes
  minimized/unmapped windows from the registry, so reconcile no longer drops
  them (which forced a second launch to “get back” to the app).
- **Equalizer latency** — low Pulse fragsize + overlapping FFT hops so the
  visualizer tracks audio instead of lagging ~1s.
- **Equalizer Solid colour** — bars no longer force a pink peak / neon height
  wash; height gradient and peak colour are configurable.

## [2026-07-17]

### Added

- **Desktop widgets (Phase 14 foundation)** — optional free-floating wallpaper
  widgets (off by default). Settings → Desktop widgets toggles the layer and
  edit mode; add a Placeholder to try move/resize. Config:
  `~/.config/metis/desktop-widgets.json`. Folders / Apps / Clock / System /
  Weather content comes next.

- **Num Lock on login** — Settings → Keyboard. Default **Auto** turns Num Lock
  on when libinput reports a numeric keypad (`KEY_KP0`), so the numpad types
  digits without pressing NumLk first. Always on / Off overrides available.

### Changed

- **Graphics profile lives under Display** — moved from Settings → Windows to
  Settings → Display (top of the page). It controls session GTK rendering, not
  window chrome.

### Fixed

- **Pinned GNOME Terminal won’t launch** — Terminal’s Wayland `app_id` is
  `gnome-terminal-server`, which was stored as the pin id and could not resolve
  a `.desktop` Exec. Pins/launch now map that alias to
  `org.gnome.Terminal.desktop`.
- **Desktop widget drag / resize** — move is title-bar only; resize uses a plain
  grip (not a Button). Gestures track **surface** pointer coords (widget-local
  `GestureDrag` offsets rubberband when the card moves under the cursor). Move
  uses a live CSS translate, then commits `Fixed` position on release; config
  reloads stay suppressed during edit.
- **Desktop widgets reset on leaving edit mode** — Settings re-reads
  `desktop-widgets.json` before every save so toggling edit/enable/lock no
  longer overwrites shell-saved geometry with a stale in-memory copy.

- **CSD apps show close-only** — session start now writes
  `org.gnome.desktop.wm.preferences button-layout` to
  `appmenu:minimize,maximize,close` (and gtk-decoration-layout when the schema
  has it). Fresh installs and leftover `appmenu:close` dconf no longer hide
  minimize/maximize on Firefox, Chromium, and GTK headerbars on bare metal or
  VMs. The Settings portal also emits the layout so portal clients refresh.
- **Edge-bar “Settings…” buttons** — Network / Power / Bluetooth overlays launch
  `metis-settings --page …`, but GApplication treated `--page` as an unknown
  option and exited immediately. Settings now registers the option, runs with a
  stripped argv, and uses a valid `com.metis.Settings` application id.
- **Network → Known Wi-Fi networks** — no longer lists ethernet / loopback NM
  profiles under Wireless (Ethernet-only machines looked broken). Wired and
  Proxy Apply buttons use the shared actions inset for padding.
- **Taskbar pin/unpin** — unpinning waited on the bar.json file monitor / window
  reconcile (felt like ~5–10s). The dock now refreshes immediately after the
  pin toggle. Pinned launches also resolve `.desktop` entries directly so
  `OnlyShowIn=GNOME` apps (e.g. GNOME Terminal) get a working Exec line.

## [2026-07-16]

### Added

- **Graphics profile** — Settings → Display dropdown (Auto / Compatibility /
  Normal). Auto detects VMs (VirtualBox, VMware, KVM, …) and forces Cairo GSK
  plus disables window animations so GTK apps stay usable on broken guest GL.
  Normal overrides autodetection when hardware rendering works.
- **Removable volumes on the edge bar** — USB / SD / optical / ISO icons appear
  left of the system tray. Left-click opens in the configured file manager
  (mounts/unlocks LUKS first when needed); right-click offers Open, Mount/Unlock,
  Unmount, and Eject. Driven by Gio VolumeMonitor; recommends `udisks2` / `gvfs`.

### Changed

- **Fresh-install theme default** — new sessions default to Dark instead of Light
  so the splash logo and Metis chrome match branding out of the box.
- **Default terminal is kitty** — auto-detect prefers kitty first (Settings → Menu
  picker unchanged). The `.deb` now `Depends: kitty` so it installs with Metis.
- **Kitty transparency defaults** — onboarding finish/skip (and later sessions if
  missing) seeds `~/.config/kitty/kitty.conf` with `background_opacity 0.75` and
  `dynamic_background_opacity yes`. Existing kitty configs are never overwritten.

### Fixed

- **Settings shortcut capture** — Change never focused a key target, so presses
  were ignored and Save stayed hidden/disabled. Capture now uses a focusable
  field, grabs focus after Change, and Esc cancels.
- **Splash / boot white flash** — compositor draws a solid dark underlay when the
  wallpaper texture is not ready yet, so the splash never sits on an empty frame.
- **GTK apps ignore Metis dark mode** — `metis-portal` now emits Settings
  `SettingChanged` for color-scheme / gtk-theme, and Metis syncs
  `org.gnome.desktop.interface` gsettings on theme change and portal start so
  non-Metis GTK apps follow light/dark. Dark mode keeps `gtk-theme=Adwaita`
  (not obsolete `Adwaita-dark`) and sets `GTK_THEME=Adwaita:dark` for spawned
  clients so libadwaita apps like Nautilus pick up PreferDark reliably.

## [2026-07-14]

### Fixed

- **System tray overlay width** — empty / sparse tray panels stretched to a
  full 5-column natural size (FlowBox planned for max columns always). The
  popover now grows with icon count up to 5×3 like Windows 11, then scrolls;
  empty state is a compact hint outside the flow.

## [2026-07-13]

### Added

- **Settings → App titlebars** — searchable list of installed apps with
  Auto / Metis / App DropDown overrides. Persists to
  `~/.config/metis/decorations.json` and reloads live via `ReloadDecorations`
  (plus mtime poll). Built-in heuristics remain Auto defaults. Search hides the
  prebuilt browse list as one widget and paints only matching rows (per-row
  show/hide of 100+ heavy rows stalled 10–20s per keystroke). Debounced ~120ms.

### Changed

- **Decoration Auto defaults from titlebar audit** — bare WM classes that needed
  Force App/Metis titlebar (EOG, Evince, Blueman, update-manager, firmware-updater,
  Extension Manager, Qt5/6ct, gnome-terminal, …) plus Claude/mpv SSD are now
  built-in heuristics. `decorations.json` stays for personal overrides only.
- **Settings page fade** — sidebar navigation crossfades between pages (~200ms;
  skipped when GTK animations are disabled).

### Fixed

- **Remote access button padding** — Connection actions sat flush on the card
  edge; they now use the shared section actions inset.
- **Background wallpapers listed twice** — Settings scanned both
  `/usr/share/metis/wallpapers` and the build-tree `assets/wallpapers`, so each
  bundled image appeared twice. Prefer packaged dirs and skip duplicate basenames.
- **App titlebars double scrollbar** — page + list both scrolled; the page is
  fixed and only the applications `ListView` scrolls.
- **App titlebars search lag** — replaced the `ListBox` filter (still remeasured
  every DropDown row) with a virtualized `ListView` + `FilterListModel` so typing
  only changes the filter; widgets exist only for on-screen rows.
- **Screenshot Selection icon / mode pills** — `select-rect-symbolic` is missing
  on Yaru/Adwaita (blank icon); switched to `edit-select-symbolic`. Mode toggles
  are fixed square hit-targets so the checked highlight is circular, not stretched.
- **Screenshot options gear theme** — the Capture toolbar settings control is a
  `MenuButton`; CSS only targeted `button`, so Adwaita’s grey chip stuck in both
  light and dark. It now follows Metis toolbar tokens.
- **App titlebars list theme** — the applications `ListBox` was using raw
  Adwaita chrome (wrong dark grey; stayed dark in light mode). It now follows
  Metis surface tokens with zebra-striped rows and readable labels.
- **Tray overlay early scrollbar** — the Background apps popover grew a
  scrollbar after ~2 icons because it was capped near one row. It now sizes to
  content: 1 icon is compact, expands to a **5×3** grid (15 icons), then scrolls.
- **Claude / bare Electron chromeless** — Claude Desktop reports Wayland
  `app_id` `electron` and disables its custom titlebar; Auto now gives Metis SSD
  instead of treating bare `electron` as CSD.
- **Wine / Cheat Engine no titlebar** — Auto no longer classifies every `*.exe`
  as CSD. Wine classes default to Metis SSD; Settings overrides also match
  `cheat engine.exe` ↔ `cheatengine-x86_64-*.exe` (alphanumeric stem).
- **LibreOffice opens tiny** — splash-sized saved geometry (e.g. `soffice`
  ~580×180) is ignored on restore; usable save floor is 480×320.
- **Splash screens tiled** — X11 Splash / `*splash*` / `frmCE*` windows keep
  their natural size instead of stretching into the app's saved main geometry
  (LibreOffice / Cheat Engine / DBeaver boot images no longer tile).
- **Double titlebar on App Center / Help / LibreOffice / Mousepad** — known
  client-decorated apps (and `org.gnome.*` / `org.xfce.*` / LibreOffice /
  snap-store ids) now keep native chrome even when they request
  `xdg-decoration` ServerSide. Frameless Electron shells still get Metis SSD via
  the chromium executable check.
- **Maximized titlebar reveal thrash** — sticky hit zone for the auto-hide
  strip includes the original trigger band and the edge-bar overhang, so the
  strip no longer oscillates while sliding down.
- **Auto-hide titlebar corners** — maximized/snap overlay chrome uses the same
  rounded top corners as windowed Metis titlebars.

## [2026-07-11]

### Added

- **Ubuntu `.deb` packaging** — `metis-os-workspace/scripts/package-deb.sh` stages
  the session under `/usr` and builds `metis_*_amd64.ubuntu24.04.deb` (bundles
  `libgtk4-layer-shell`, which Ubuntu 24.04 does not package). GitHub Actions
  (`.github/workflows/release-deb.yml`) publishes on `v*` tags and manual
  `workflow_dispatch` prereleases. See `docs/PACKAGING.md`.
- **Onboarding optional software** — after Gaming, a step detects Remote desktop,
  Flatpak, GameMode, Bluetooth, printers (and keyring if missing). Installed
  rows are greyed out; toggles + **Install selected** run one
  `pkexec apt-get install` for the chosen packages.
- **Configurable desktop shortcuts** — Settings → Keyboard → Shortcuts lists
  Metis actions with Change / capture / Save / Reset. Bindings live in
  `keybinds.json`; compositor reloads live (`ReloadKeybinds`). Ctrl+Alt+F1–F12
  and Ctrl+Alt+Backspace stay visible as locked system binds. Capture mode
  suppresses global shortcuts via `SetKeybindCapture`.
- **Control Center process tree** — Processes tab nests children under their
  parent PID with expand/collapse. Search keeps ancestor paths; right-click adds
  End / Force quit process tree (descendants then root).
- **Control Center process monitor picker** — Settings → Control Center chooses
  Auto-detect, an installed monitor (`btop`/`htop`/GUI), or a custom path.
  TUI tools launch inside the Menu terminal; persisted as `process_monitor` in
  `dashboard.json`.
- **Maximized window padding** — Settings → Windows slider (`window_gap_px`,
  0–10) controls the inset around maximized and edge-snapped windows. `0` is
  flush; default remains 8. Live-reloads within ~1s via `bar.json`.

### Fixed

- **Settings → Gaming Optimize now** — was a silent Flatpak-only no-op when the
  reported issue was “not in input group”. Optimize now also offers to add you
  via `pkexec usermod`, shows a status summary afterward, and health rows offer
  both **Fix** (one-click: GameMode, i386 Vulkan, audio, Steam, input group,
  Flatpak overrides) and **Copy command** for a manual terminal install.
  NVIDIA drivers stay copy-only (`ubuntu-drivers` is interactive).
- **Settings button hover contrast** — hover no longer snaps to a near-black
  surface that hides labels; accent (**Optimize now**) keeps readable on-accent
  text on hover.
- **Onboarding blank wallpapers on `.deb` installs** — bundled images now install
  to `/usr/share/metis/wallpapers` (deb + `--install-session`) and the shell looks
  there at runtime (compile-time workspace paths alone were empty after install).
- **Onboarding Optional software card growth** — feature list scrolls inside a
  fixed-height region so the content-sized overlay no longer expands on that step.
- **Onboarding light theme** — splash/onboarding cards no longer use a hardcoded
  dark charcoal background; they follow theme surface tokens so Light (the fresh
  install default) keeps readable title/subtitle contrast. Theme picker preview
  tiles also get their CSS in the shell stylesheet (previously Settings-only).
- **CI `.deb` build** — install `libdisplay-info-dev` and `libpam0g-dev` on the
  Ubuntu 24.04 runner so `libdisplay-info-sys` / compositor PAM link successfully.
- **Control Center search types into app behind** — Top-layer shell panels
  (`metis-dashboard`) now win pointer and keyboard focus over desk AppBody
  passthrough, so Processes search no longer sends keys to the window underneath.
- **Control Center process list scrollbar light theme** — scrollbars use Metis
  text-token tints instead of Adwaita prefer-dark charcoal.
- **Control Center process search** — panel keyboard mode is Exclusive while open
  so the Processes filter SearchEntry receives typing (OnDemand never focused).
- **Control Center process context menu** — right-click menu dismisses on primary
  click elsewhere in the panel (Escape too); it no longer sticks open because
  shell UI clicks never sent compositor `close-popovers`.
- **Control Center DropDown light theme** — filter/list popovers use Metis surface
  tokens instead of Adwaita prefer-dark charcoal menus.
- **Control Center “Open monitor”** — no longer `exec btop` without a TTY; launches
  via terminal or a GUI monitor from auto-detect / Settings.
- **Screenshot paste TTY lockup** — compositor-owned clipboard offers (region
  PNGs after PrtSc) no longer `write_all` / `fs::read` on the compositor thread.
  Paste always serves on a worker thread so XWayland/Electron clients cannot
  deadlock the session. `SetClipboard` image history also reuses the capture
  path instead of sync-reading the full PNG at offer time.
- **Clipboard history image stall** — history PNG materialization (and durable
  cache copy) runs off the compositor thread; event-bus writes are non-blocking;
  the shell skips history list rebuilds while the clipboard popover is closed and
  loads thumbnails scaled on a worker thread (no full-res `Picture::for_filename`
  on every capture).
- **Control Center Processes click closes panel** — the CC layer namespace is
  `metis-dashboard` again so compositor hit-tests count presses as shell UI
  (not bare desktop `close-popovers`). Header dismiss-drag ignores tab/close
  buttons.
- **Control Center Overview/Processes light theme** — tab chip CSS now targets
  `stackswitcher.metis-dash-tabs > button` (the class is on the switcher itself),
  so Metis tokens win over Adwaita prefer-dark charcoal chips.
- **Control Center no longer moves the edge bar** — the panel is a separate
  layer-shell surface attached inside the pill; opening it no longer resizes the
  bar window.
- **Edge bar distance 0 keeps stadium ends** — flush-to-edge no longer squares the
  pill; rounded ends stay at every distance. Side inset remains for the drop
  shadow; full-width bars no longer also apply `margin_h` (that double-counted
  side gaps). Drop shadow faces the desktop so distance maps 1:1 to the slider.
- **Bottom edge bar flush at distance 0** — layer surface height equals the pill
  (no spare shadow-pad strip) so the bar sits on the screen edge with no gap.
- **Rounded edge bar ends** — stadium ends stay at every distance (including 0);
  distance only controls the gap from the anchored screen edge.
- **Bottom-bar maximize underlap (Cursor/Chromium)** — `reclamp_maximized_geometry`
  now corrects wrong size and bbox overflow past the expected bottom (not only
  wrong `y`), and logs the reclamp at info for verification.
- **Control Center Processes light theme** — process rows, search, filter, kill
  buttons, and the right-click menu now force Metis text/surface tokens so
  Adwaita prefer-dark no longer leaves unreadable light labels on the frosted
  panel.
- **Control Center “Copy PID” / process menu abort** — process context menu
  closed/dismiss no longer nests `RefCell` borrows (was aborting the shell).
  Bar popover `close_all` also releases its registry borrow before `popdown`.
  Clipboard history refresh also invokes callbacks outside the listener `RefCell`.
- **Edge bar distance 0** — config migration no longer treats `margin_top == 0` as
  a legacy layout and rewrites it to 4. Flush-to-edge distance now sticks.

## [2026-07-10]

### Added

- **Notification Center (Phase 13)** — Win11-style right-edge layer-shell panel
  opened from the clock: notifications card, calendar events card, and calendar /
  tools card with an icon rail (Calendar, World, Stopwatch, Timer, Alarms).
- **Closable toasts** — top-right banners gain an explicit dismiss (X); stack
  shifts left while the Notification Center is open.
- **Theme-aware Notification Center CSS** — `metis-nc-*` classes use design tokens
  for light and dark (panel, cards, tool rail, entries, switches).

### Changed

- **Notification Center follows edge bar position** — slides from the left when
  the bar is on the right; full height beside left/right bars; sits above a
  bottom bar (and under a top bar as before).
- **Clock merges the notifications bell** — unread badge on the clock; default
  `bar.json` no longer includes a separate notifications widget (existing configs
  that list both are migrated).
- **Clock popover retired** — calendar / world / timer / alarm / stopwatch UI
  lives in the Notification Center panel instead of a tabbed `GtkPopover`.

### Fixed

- **Bottom edge bar maximize + Notification Center** — placement no longer
  trusts layer-shell exclusive maps (they were not shrinking for bottom bars).
  Maximize subtracts `margin + height` from the output; NC uses an explicit
  bottom margin. Vertical maximize gaps stay tight so the top of the screen is
  not left empty.
- **Edge bar distance** — full-width pills pin to the anchored edge (no longer
  sit in the shadow pad); exclusive zone is body-only so margin is not
  double-counted. Distance 0/1 now matches Settings for every edge.
- **Control Center on side bars** — panel content sizes against full bar height ×
  pull width (was treating pull extent as height, which crushed charts).
- **Screenshot settings popover light theme** — options panel (`contents`,
  switch, spinbutton, after-capture buttons) follows Metis tokens instead of
  Adwaita dark chrome.
- **Bottom edge bar maximize** — bottom bar reserves an exclusive zone again so
  maximized windows sit above it (no top gap / under-bar overlap).
- **Notification Center above bottom bar** — panel bottom inset tracks the bar
  strip so it no longer paints over a bottom edge bar.
- **Notification Center flush under edge bar** — top inset sits 1px below the
  bar pill so the panel no longer paints into the strip.
- **Notification Center light-theme buttons** — calendar / tools chrome forces
  Metis tokens so Adwaita dark fills cannot linger on light panels.
- **Screenshot over Notification Center** — picker UI stacks above the panel
  while the panel stays visible and capturable.
- **Notification Center layout** — panel fills from the edge bar to the bottom;
  calendar/tools card pinned to the bottom; notifications header always visible
  with collapsible list; events card stays visible and collapses when empty.
- **Notification Center open/close** — removed slow full-height slide revealer
  (instant map/unmap) to stop stutter.
- **Screenshot + Notification Center** — opening PrtSc no longer dismisses the
  Notification Center, so it can be included in captures.

## [2026-07-09]

### Added

- **Native Screenshot Tool (Phase 12)** — **PrtSc** opens a Metis-themed interactive
  overlay (Selection / Screen / Window) with accent Capture button, pointer toggle,
  and configurable after-capture (clipboard, save, viewer).
- **`metis-capture` crate** — shared Wayland `ext-image-copy-capture` client for
  shell and portal; crop + PNG encoding.
- **`screenshot.json`** — default mode, draw cursor, delay, after-capture action,
  save directory under `~/.config/metis/`.
- **Theme-aware screenshot CSS** — `metis-screenshot-*` classes in the global
  stylesheet; live reload with dark/light/custom token edits.
- **Compositor keybinds** — PrtSc, Shift+PrtSc (instant full screen), Ctrl+PrtSc
  (window mode); `BeginScreenshotOverlay` / `EndScreenshotOverlay` IPC; capture pass
  excludes `metis-screenshot` layer namespace.
- **`metis-cmd screenshot`** — scriptable overlay trigger.

### Fixed

- **Screenshot Capture button** — accent blue in the default state (no longer grey
  until hover); `background-image: none` on the base CSS rule.
- **Screenshot clipboard history** — `SetClipboard` after capture emits
  `ClipboardChanged` so PNG copies appear in the bar clipboard history.
- **Screenshot Options popover** — after-capture control uses inline segmented
  toggles (Copy / Save / Both / Open) instead of `GtkDropDown`, which rendered
  behind layer-shell popovers on Metis.
- **Screenshot window mode** — click locks the highlighted window; hover no longer
  overwrites the pick when moving to the Capture button; `ListWindows` returns
  top-to-bottom stacking order so the frontmost window wins hit-testing.
- **Screenshot Escape** — compositor intercepts bare **Esc** while the overlay is
  active (`dismiss-screenshot` runtime command) and routes keyboard focus to the
  `metis-screenshot` layer; dismiss works in all modes without clicking the toolbar
  first.

### Changed

- **Screenshot toolbar** — icon toggle buttons for Selection / Full screen / Window
  plus an Options gear popover (include pointer, delay, after-capture); replaces
  the mode `StackSwitcher` and dropdown after-capture control.

## [2026-07-07]

### Added

- **Gaming Platform 2.0 (Phase 11)** — `gaming.json` config, `metis-gaming` crate
  with Flatpak optimizer, health checks, and event-driven `metis-gamingd` daemon.
- **Settings → Gaming v2** — editable graphics mode, battery/performance/GameMode
  toggles, health checklist with Fix buttons, Optimize and Run gaming setup.
- **Compositor gaming integration** — `ReloadGaming` IPC, `GameSession` events,
  battery-aware dGPU offload from `gaming.json`, scanout promotion trace.
- **Onboarding gaming step** — optional skippable step before Finish; applies
  Flatpak overrides and GPU defaults.
- **Flatpak launcher wrapper** — `~/.local/share/metis/bin/launch-steam` with GPU
  env for Flatpak Steam; Big Picture uses it automatically.
- **`metis-cmd reload-gaming` / `optimize-gaming`** — runtime reload and Flatpak
  optimize hooks.
- **`scripts/gaming-prime-smoke.sh`** — hybrid PRIME validation helper.

**Gaming automation scope:** Metis auto-detects hybrid GPU, routes game launches to
the dGPU, applies Flatpak overrides after **Optimize now** / **Run gaming setup**, and
switches power profile / GameMode while gaming. It does **not** install Steam,
drivers, `mesa-vulkan-drivers:i386`, or `gamemode` — the health checklist shows
install commands for those gaps.

### Fixed

- **Settings → Gaming** — background health checks use an mpsc channel back to the
  GTK main thread (fixes release-build `Send` errors on `MainContext::invoke`).
- **Gaming setup wizard** — undecorated dialog with in-panel header (no duplicate
  title bar under Metis SSD); singleton reuse avoids duplicate windows; clearer
  message when no Flatpak Steam/Lutris/Heroic is installed (normal for native Steam).
- **Control Center end task** — confirm uses a popover instead of a modal dialog
  (no longer freezes the layer-shell session).
- **Processes tab** — right-click context menu (End task, Force quit, Copy PID)
  anchors at the cursor, uses non-autohide layer-shell popovers, and pauses list
  rebuild while open (Task Manager–style stability).

### Changed

- **Settings → Control Center** — toggle the pull-down system monitor, panel
  height %, refresh interval, confirm-before-kill, and which overview widgets are
  active; writes `dashboard.json` for live shell reload.
- **`dashboard.json` live reload** — file monitor reapplies enabled state, max
  height, Processes tab visibility, and workspace Control Center button without
  restarting the shell.
- **GPU load %** — discrete GPU gauge cards show utilization when sysfs
  (`gpu_busy_percent`) or `nvidia-smi` exposes it (e.g. `78°C · 45%`).
- **Processes → Open monitor** — launches `btop` or `htop` directly, or in the
  configured terminal as a fallback.
- **Widget registry** — built-in Control Center widgets registered by id for
  future script/plugin extensions.
- **Bar pull gesture** — always wired; gated at drag time so re-enabling Control
  Center in Settings works without a bar rebuild.

## [2026-07-06]

### Added

- **Control Center bar button** — `view-grid` icon to the right of workspace dots opens
  the system monitor with the same slide-down animation as a bar pull gesture.
- **Overview charts** — gradient fills and strokes on CPU, memory, network, and disk
  series; cubic-smoothed curves; Y-axis grid labels (percent and throughput).
- **Network & disk I/O cards** — per-interface ethernet/wifi rates, dual-line
  RX/TX and read/write charts with colour legends.
- **Temperature gauges** — semicircular CPU and discrete-GPU cards (0–150 °C) with
  theme-gradient arc bands; hybrid-laptop NVIDIA temps via `nvidia-smi` when sysfs
  has no `hwmon` entry.
- **Metis Settings icon** — `metis_Settings.png` (transparent RGBA) for the Settings
  app and app-menu entry only; edge-bar **Metis menu launcher keeps its original
  `metis_icon.png`**.
- **Dashboard icon fallbacks** — card icons resolve through a candidate list so
  missing theme names (e.g. on Yaru) do not show broken glyphs.

### Changed

- **Control Center embed** — dashboard lives in the bar's layer window directly
  below the pill (no separate surface gap); `resize_bar_for_dashboard()` grows the
  bar host for the overlay without reflowing tiled windows.
- **Overview layout** — rows: CPU | Memory, Network | Disk I/O, Session | Storage,
  then CPU/GPU gauges + compact System card; health badges removed from overview.
- **Processor chart** — per-core gradient strokes with an expanded palette (C0
  white, theme accents, distinct hues); aggregate Σ fill drawn **behind** core
  lines so the gradient does not paint over them.
- **GPU gauges** — only discrete GPUs (Intel iGPU skipped); one card per discrete
  GPU with a readable temp — no placeholder N/A card when no dGPU sensor exists.
- **Processes tab** — opaque card wrapper, aligned sortable headers, user column
  falls back to `/proc/<pid>/status` when `sysinfo` omits the username.
- **Theme reload** — `reload_stylesheet()` calls `load_active_theme()` so
  dashboard tabs and cards follow light/dark switches immediately.

### Fixed

- **CPU temp gauge** — Cairo arc direction (was ~full ring at ~78 °C on 0–150 scale);
  replaced stroked-cap “arrow” artefact with a filled annular band; symmetric layout.
- **`run-metis.sh --help`** — help text used `$0` after `cd` to the workspace root.
- **GPU services compile error** — duplicate/corrupt `drm_gpu_label` definition.
- **Dashboard icons** — CPU temp and Processor cards no longer reference icons
  absent from Adwaita/Yaru.

## [2026-07-05]

### Added

- **Control Center (dashboard v2)** — Metis-style system monitor with four tabs:
  **Overview** (health badges, CPU/memory sparklines, storage meters, load/uptime),
  **Processes** (zebra-striped table, search, All/User/System filter, end-task),
  **Network** (RX/TX rate charts + firewall status via ufw/firewalld), and
  **System** (hostname, CPU model, cores, kernel, memory, uptime). Theme-aware
  Cairo charts and semantic health colors follow light/dark tokens. Lazy-loaded on
  first bar pull; pollers collect hardware info, load average, and metric history.

- **Phase 10 — System dashboard (v1)** — pull-down system monitor from the top
  edge bar: drag the handle below the bar (or click it) to reveal CPU sparkline,
  memory/swap, storage mounts, network throughput, and a searchable process list
  with end-task (SIGTERM). Background polling via `spawn_dashboard_pollers()`
  (`sysinfo` + `/proc/net/dev`); config in `~/.config/metis/dashboard.json`.
  Dismiss with Esc, drag up on the header, click outside, or the close button.
- **Phase 7 — Desktop sharing (RDP v1)** — GNOME-style **Settings → Remote access**
  master toggle for headless RDP via `gnome-remote-desktop`. New `metis-remote`
  CLI (`status`, `enable`, `disable`, `autostart`, `set-credentials`) wraps
  `grdctl --headless` and `gnome-remote-desktop-headless.service`. Config in
  `~/.config/metis/remote.json` (`metis-config`); session autostart from
  `metis-session` when enabled. Launch page with `metis-cmd settings remote`.
  RustDesk, VNC, and classic xrdp deferred to follow-up milestones.
- **Phase 6 wrap-up — portal stubs** — `metis-portal` now serves
  `org.freedesktop.impl.portal.Background` (allows sandboxed background runs;
  autostart returns false) and `org.freedesktop.impl.portal.PowerProfileMonitor`
  (mirrors `powerprofilesctl get` with property-change signals). Registered in
  `metis.portal` and `metis-portals.conf`.
- **Phase 6 wrap-up — touch input** — compositor forwards libinput touch events
  through `wl_touch` (lazy `seat.add_touch()` when a touchscreen appears). Lock
  screen handles touch taps on power controls. Multi-output coordinate mapping
  uses the same desktop-bounds transform as absolute pointer motion.
- **Settings → Gaming page** — read-only diagnostics under Input: connected
  gamepads and touchscreens (from `/proc/bus/input/devices`), Steam install
  detection, GPU offload env hints, and links to the user guide. Launch with
  `metis-cmd settings gaming`.

- **RDP clipboard bridge** — `metis-portal` now implements Mutter's
  `EnableClipboard` / `SetSelection` / `SelectionRead` / `SelectionWrite` D-Bus
  methods (correct `a{sv}` option dicts — the old wrong signatures caused GRD
  `UnknownMethod` errors). Local clipboard changes from the compositor are
  forwarded over the event socket; remote text pastes are written back via IPC.
- **RDP remote input focus** — pointer button injection now syncs selection
  focus at the click location before delivery so RDP clicks target the correct
  window for focus and clipboard routing.

### Changed

- **Portal PipeWire logging** — demoted noisy per-state transition logs to
  debug; process warnings only when diagnostics are needed.
- **libinput device logging** — `on_device_added` now logs capability flags
  (keyboard, pointer, touch, etc.) and documents that gamepad nodes are not
  configured or grabbed by the compositor.
- **USER_GUIDE** — portal permission management (`flatpak permission-show/reset`),
  Flatpak override cookbook, expanded controller/touch notes, and handheld
  compatibility guidance.

- **Control Center layout** — flush attach to the edge bar (no desktop gap), taller
  default panel (58%), denser overview with icons, per-core CPU chart + legend,
  network snapshot on Overview, scrollable content, and sortable process table
  (Name, PID, User, Type, CPU, Memory).

- **Control Center polish** — fixes process-sort crash (RefCell panic), live bar
  attach inset, near-full-screen height (92%), memory line chart with swap overlay,
  network sparklines on Overview, 2-column storage tiles, larger session stats.

### Fixed

- **Control Center window push** — dashboard no longer sets a layer-shell
  exclusive zone, so opening it overlays windows instead of reflowing the grid.

- **App menu search focus** — typing in "Search applications" no longer unfocuses
  after each key: search rebuild is debounced, focus is restored on idle, pinned
  grid is not rebuilt while filtering, and focus-enter no longer cancels the list
  on every `grab_focus` during rebuild.

### Changed

- **System dashboard tabs** — **Control Center** with Overview, Processes,
  Network, and System tabs (health badges, sparkline charts, zebra process table,
  firewall card, hardware panel). Theme-aware styling via `metis-dash-*` CSS.

- **System dashboard peek + bar line** — removed the full-width drag handle below
  the bar (it drew a visible accent line across the monitor). Open gesture is now
  press-and-drag on the bar pill toward the desktop; panel tracks the drag live.
  Dashboard layer starts fully hidden (opacity 0, extent 0, clipped) instead of
  peeking under the bar. Works for all four bar positions (top/bottom/left/right).
  **Lazy load:** dashboard surface + sysinfo pollers start on first pull and tear
  down when dismissed — no idle resource use while closed.

- **RDP remote desktop black screen** — gnome-remote-desktop connected and EIS
  input worked, but the RDP client showed a blank display. Capture and PipeWire
  negotiation succeeded (`Streaming`, frames pushed to the portal), yet memfd
  buffers were never filled for GRD: the PipeWire hub used `ThreadLoop` incorrectly
  (340k+ `pthread_equal` failures in `portal.log`), and the output-stream process
  callback was not scheduled. Rewrote the hub on a single-threaded `MainLoop`,
  call `set_active(true)` when the stream enters `Streaming`, and
  `trigger_process()` after each captured frame so pixels reach GRD. Portal stderr
  now flushes line-by-line to `$XDG_RUNTIME_DIR/metis/portal.log` for live
  `tail -f` diagnostics (`metis-portal` pipewire + logging).

## [2026-07-04]

### Added

- **First-run onboarding wizard (Phase 9)** — a 7-step GTK4 layer-shell overlay
  in `metis-shell` (`ui/onboarding.rs`) runs after the startup splash when
  `config.json` → `onboarding_complete` is false. Steps: Welcome, Light/Dark theme
  (Light default), bundled wallpaper pick, 12h/24h clock, edge-bar position/
  displays/opacity/blur, weather auto-detect + optional city search, and Finish
  with keybind tips + pointer to Settings → Display. Skip or Finish calls
  `mark_onboarding_complete()`. Re-run from Settings → Appearance → "Run setup
  again" or `metis-cmd.sh show-onboarding`. Dev opt-out: `METIS_NO_ONBOARDING=1`.
  Default theme for fresh installs is now **light** (`metis-config`).

- **Game mouse-look: pointer lock & relative motion** — the compositor now
  implements `zwp_pointer_constraints_v1` (locked/confined pointer) and
  `zwp_relative_pointer_v1` (raw, unaccelerated deltas). Together these let games
  (Steam/Proton titles, Hytale, Minecraft-likes, any FPS/3D app) capture the
  mouse for camera "look": previously such apps received clicks and buttons but
  no look motion, because absolute cursor position never changes while a game
  holds a pointer lock. The relative-motion path emits accelerated + unaccelerated
  deltas on every hardware move; while a lock is active the system cursor stays
  put and only relative motion is delivered, and a confinement refuses moves that
  would leave the surface/region. Constraints arm when the owning surface has
  pointer focus (or when the pointer enters the constraint region) and, on
  release, the cursor is restored to the client's last-drawn position hint. Both
  globals are inert until a client requests them, so non-gaming apps are
  unaffected (`state.rs`, `handlers/mod.rs`, `input.rs`).

- **Game pointer-lock diagnostics (`game-pointer`)** — INFO-level tracing for
  Proton/Steam game input debugging: constraint create/remove/activate/deactivate,
  pause-menu phase transitions, cursor-position-hint restore, pointer button
  press (before and after spurious-lock suppression), click remapping via cursor
  position hint, and bare Esc forwarded to a game. Filter session logs with
  `rg 'game-pointer' ~/.local/state/metis/logs/session-*.log`. Covers
  `steam_app_*`, `*.exe`, Proton, and `HytaleClient` windows
  (`state.rs`, `input.rs`, `handlers/mod.rs`).

### Changed

- **Gaming performance (fullscreen fast path)** — several compositor-side
  bottlenecks that kept fullscreen games composited every frame even when only
  the game buffer changed:
  1. **Skip wallpaper + bar blur under fullscreen** — when an output has a true
     fullscreen client, the wallpaper texture and backdrop blur are no longer
     drawn beneath it, giving Smithay a cleaner shot at primary-plane direct
     scanout (`render.rs`).
  2. **Skip night light under fullscreen** — the warm overlay blocked scanout
     promotion; it is suppressed while a fullscreen window covers the output
     (`night_light.rs`).
  3. **Hide compositor cursor during pointer lock** — while
     `zwp_locked_pointer_v1` is active the theme cursor is not composited
     (games draw their own or hide it); keeps the cursor off the primary plane
     (`state.rs`, `udev.rs`).
  4. **Stop repainting on locked-pointer motion** — mouse-look no longer calls
     `schedule_redraw` on every hardware move; only the game's own commits drive
     frames (`input.rs`).
  5. **Per-output damage on client commit** — surface commits arm only the
     output the window sits on instead of every connected display
     (`state.rs::schedule_redraw_for_window`, `handlers/compositor.rs`).
  Verified on hardware: **Hytale** (native Wayland) runs **High** smoothly; was
  Low-only before these changes.

### Fixed

- **Onboarding Finish crashed the edge bar** — the Finish button's click handler
  held a mutable `RefCell` borrow on the wizard state and then called
  `dismiss()`, which tried to borrow again — a runtime panic that took down the
  whole shell (looked like the edge bar dying). Skip worked because it called
  `dismiss()` without an active borrow. Finish now checks the step with an
  immutable borrow before calling `dismiss()` (`onboarding.rs`).
- **GitHub Desktop missing titlebar** — Flatpak (`io.github.shiftey.Desktop`) and
  native (`GitHub Desktop`) builds are frameless Electron apps with no Wayland
  client-side titlebar, but Metis classified every `io.github.*` app as CSD and
  drew no server-side chrome. GitHub Desktop is now on the SSD allowlist so Metis
  paints titlebar + resize borders (`decoration_policy.rs`).
- **In-game menu clicks always open Settings (Proton / pointer lock)** — while a
  `zwp_locked_pointer_v1` lock is active the compositor cursor stays at the lock
  anchor, but Proton games track menu hover via `set_cursor_position_hint` and
  expect clicks at the hinted coordinates. Every press was delivered at the
  frozen anchor regardless of where the user moved the mouse, so only the menu
  item at that spot (Settings) ever activated. Clicks now remap through the
  latest cursor position hint before motion/button delivery
  (`state.rs::effective_pointer_delivery_loc`, `input.rs`).
- **Compositor freeze at game title screen (MOUSE / Proton)** — the new
  `game-pointer` tracing called `pointer_constraint_snapshot()` from inside
  Smithay's `with_pointer_constraint` callback, which re-enters the same mutex
  and deadlocks the compositor thread the moment a game creates its first
  pointer lock (e.g. "press any key to continue"). Tracing now uses
  `trace_game_pointer_at()` with constraint state read directly inside the
  callback and logged only after it returns (`state.rs`, `handlers/mod.rs`).
- **In-game menu clicks do nothing / cursor jumps (Proton)** — two compounding
  bugs after the X11 keyboard-focus fix:
  1. **Pointer lock re-engaged during the pause menu** — when a Proton game opens
     its pause menu it *deactivates* (but often keeps) its `zwp_locked_pointer_v1`
     mouse-look lock. Metis was re-arming it on pointer motion *or* letting the
     game re-activate it on menu clicks (cursor vanishes until the mouse moves
     again). Constraint lifecycle is now tracked per surface and synced on every
     pointer motion/click; a client-deactivated lock is never re-armed on motion,
     is marked `ClientDeactivated` before click delivery, and any spurious
     re-activation during the pause menu is immediately dropped again
     (`state.rs`, `input.rs`, `handlers/mod.rs`).
  2. **Keyboard refocus on every click** — re-entering the same already-focused
     X11 window on each pointer click ran `XSetInputFocus`/`WM_TAKE_FOCUS` again,
     resetting in-game UI state so dialogs never opened. Pointer clicks now skip
     refocus when the clicked window already holds keyboard focus (`input.rs`).
- **Keyboard input ignored by XWayland games (Esc / keys dead, mouse works)** —
  the real root cause of the long-running "can't Esc in a Steam/Proton game"
  report. Metis's keyboard focus wrapper delivered key events for an X11 window
  straight to its raw `wl_surface`, bypassing `X11Surface`'s own `KeyboardTarget`
  impl — which is what calls `XSetInputFocus` (and sends `WM_TAKE_FOCUS`) so the
  X client actually accepts keyboard input. Without it XWayland received the
  events but the client never held X input focus, so every keystroke was dropped
  while pointer *clicks* (which don't need focus) still worked. Keyboard focus for
  X11 windows now targets the `X11Surface`, which both sets X input focus and
  buffers the enter until the surface associates (`pending_enter`), natively
  covering the map-before-surface race too (`focus.rs`).
- **Tray "Exit"/"Quit" fails (Steam) — ids resolved by label now** — Steam
  *renumbers every dbusmenu item id* whenever its menu tree is rebuilt, so the id
  captured when the menu was shown was routinely dead by click time ("The ID
  supplied N does not refer to a menu item we have"), and the previously-shown
  menu was even served from a cache fetched at registration. The click dispatch
  now re-fetches the live layout and re-resolves the target by its (stable) label
  before delivering the event, with a one-shot retry if the client renumbers
  again mid-click. The clicked row's label is threaded through `MenuClicked`
  (`tray.rs`, `tray_watcher.rs`).
- **Games & Steam Big Picture forced onto the weak iGPU (slow) on hybrid
  laptops** — on an Optimus system (Intel UHD + NVIDIA RTX 2070) the compositor
  correctly renders on the iGPU that drives the panel, but its client GPU
  steering then pinned *every* spawned process — including Steam, Big Picture and
  the games Steam launches — to that same integrated GPU
  (`DRI_PRIME=pci-…_00_02_0`, `MESA_VK_DEVICE_SELECT=8086:…`), leaving the
  discrete GPU idle. That is why Big Picture loaded slowly and games were only
  playable on Low, while the same titles run fine under GNOME (which does not
  pin). Metis now detects a discrete GPU distinct from the display GPU
  (`DgpuOffload::detect`) and PRIME-offloads game/launcher launches onto it —
  NVIDIA via `__NV_PRIME_RENDER_OFFLOAD`/`__GLX_VENDOR_LIBRARY_NAME=nvidia`/
  `__VK_LAYER_NV_optimus=NVIDIA_only` (proprietary driver only; nouveau falls
  through to the Mesa path), or a Mesa dGPU via `DRI_PRIME`/
  `MESA_VK_DEVICE_SELECT` for its render node. Lightweight desktop apps stay on
  the power-efficient iGPU. The heuristic matches Steam/`-gamepadui`/gamescope/
  Lutris/Heroic/Bottles/Proton/Wine/`.exe`; `METIS_GAME_GPU=igpu|dgpu|off`
  overrides it and per-game Steam launch options still win
  (`state.rs`, `udev.rs`).
- **Esc / keys don't reach a Proton game (mouse works but keyboard doesn't)** —
  root cause of the "still can't Esc in game" report. An XWayland toplevel (a
  Proton/Wine game launched from Steam) issues its `MapRequest` — at which point
  `map_x11_toplevel` gives it keyboard focus — *before* XWayland has associated
  its `wl_surface`. `keyboard.set_focus` therefore delivered `wl_keyboard.enter`
  to a window with no live surface, so no keystrokes ever reached the game, while
  the pointer (resolved per-motion) still worked — the exact "mouse works, keys
  don't" symptom. The game also read as "focused" (`focused_window_id` matches the
  `Window`, not its surface), which masked the bug. Now, when an XWayland surface
  associates on its first commit, Metis indexes it and — if that window is still
  the intended keyboard-focus target — re-delivers focus (drop-then-set, since
  re-setting the same target is a no-op in Smithay) so XWayland gets a fresh
  `enter` for the now-live surface. Keyboard-focus changes are also logged
  (`state.rs::note_surface_committed_for_focus`, `focus_window_id`,
  `handlers/compositor.rs`).
- **Tray "Exit"/"Quit" still ignored on Steam (the fix's own regression)** — the
  earlier settle-delay let `fetch_menu` parse live ids, but the click dispatch
  *then called `AboutToShow(0)` again right before sending the click*. Steam
  rebuilds its dbusmenu and **renumbers every item id** on each `AboutToShow`, so
  that second call invalidated the very id (`33`) we were about to click — the
  event failed with "The ID supplied 33 does not refer to a menu item we have"
  every time. Removed the redundant pre-click `AboutToShow` calls; the menu is
  already made live (and its ids parsed) when the context menu is built, so the
  click now lands on a valid id the first time (`tray_watcher.rs`).
- **Steam steals focus from a running game (foreground pop-in + Esc/keys stop
  working)** — while playing a Proton game (`steam_app_*`), Steam fired
  `_NET_ACTIVE_WINDOW` at its own main window (tray updates, friends/downloads)
  and Metis honored it unconditionally, popping Steam over the game *and* moving
  keyboard focus off the game. The game kept only *pointer* focus (mouse still
  worked) but lost keyboard focus, so Esc and movement keys no longer reached it
  and the in-game menu was unreachable. Added focus-stealing prevention: while a
  *running game* (`steam_app_*`/`*.exe`/Proton/`HytaleClient`, or any fullscreen
  window) holds focus, a **different** window's self-activation via
  `_NET_ACTIVE_WINDOW` is ignored (logged). A game activating itself, a launcher
  handing off to a freshly-mapped game, and user-initiated dock/taskbar raises are
  all still honored (`xwayland.rs`, `state.rs::window_is_running_game`).

### Docs

- **Proton input & hybrid-GPU gaming** — `docs/USER_GUIDE.md` now documents
  automatic dGPU offload for game/Steam launches (`METIS_GAME_GPU=igpu|dgpu|off`),
  pointer lock / relative motion for mouse-look, and troubleshooting rows for
  Proton keyboard, menu-click, and tray-quit issues. `docs/UBUNTU_DEV.md` adds
  `METIS_GAME_GPU` and the `game-pointer` log filter. `README.md` and `TODO.md`
  Phase 6 updated to reflect on-hardware Proton verification (MOUSE: P.I. For Hire).
  Hytale **High** verified on hardware after the fullscreen fast-path perf fixes.

## [2026-07-03]

### Added

- **Hytale launches fullscreen** — the game client (`HytaleClient`) is configured
  for fullscreen in-game but was coming up as a large float that the user had to
  F11. A dedicated game rule now force-fullscreens the game client on its first
  map (via the existing `pending_game_fullscreen` path), while the Hytale
  *launcher* stays a normal floating window. Client-initiated
  `xdg_toplevel.set_fullscreen`/`unset_fullscreen` requests are now logged so we
  can see when a game drives its own fullscreen (`metis-config/game_rules.rs`,
  `handlers/xdg_shell.rs`).
- **Gaming: high-refresh, vblank-driven rendering** — the DRM backend no longer
  paces frames off a fixed 16 ms (≈60 Hz) heartbeat. Damage now arms the
  scan-out surface immediately (`schedule_redraw`), and each surface repaints on
  its *next vblank* (`on_drm_vblank`), so a continuously-committing client (a
  game) runs the render loop at the panel's full refresh — 120/144/240 Hz — and
  VRR genuinely engages on demand. The heartbeat is retained only for
  housekeeping and to kick the first frame out of idle; it no longer caps FPS.
- **Gaming: `wp_presentation` (presentation-time feedback)** — the compositor
  now advertises `wp_presentation` and reports each frame's real scan-out
  timestamp/sequence to clients on vblank (hardware timestamp when the driver
  supplies one, monotonic fallback otherwise), reporting `Variable` refresh when
  VRR is active. Lets games pace frames accurately. Frame user-data on the DRM
  output carries an `OutputPresentationFeedback` token per queued frame.
- **Gaming: explicit sync (`linux-drm-syncobj-v1`)** — advertised when the
  primary GPU supports syncobj eventfd. NVIDIA + DXVK/VKD3D and modern XWayland
  negotiate explicit acquire/release fences instead of implicit sync, removing
  the tell-tale Proton stutter/glitching. A pre-commit hook holds a surface
  commit (via a calloop blocker) until its acquire fence signals, with an
  implicit-sync dmabuf-readability fallback for clients that don't use it.
- **Gaming: hardware cursor plane + scanout dmabuf feedback** — enabled the
  `renderer_pixman` cursor-plane fallback so the pointer stays on the DRM cursor
  plane even for scaled cursors on fractional-scale outputs (instead of dropping
  to the primary plane, which would block a fullscreen game from direct
  scanout). Each display also advertises a per-surface `Scanout` dmabuf-feedback
  tranche of its directly-scannable plane formats, so fullscreen games allocate
  buffers that can be promoted to the primary plane (zero-copy).
- **Game window rules** — a new `game-rules.json` (curated built-in defaults for
  Steam, Big Picture, Lutris, Heroic, Bottles, gamescope, Minecraft, Hytale and
  Proton/Wine `.exe` windows, and fully user-extensible) auto-floats games and
  launchers so they escape the tiling grid, with an optional per-rule
  auto-fullscreen. Native-Wayland games are no longer forced into a grid tile
  (`metis_config::game_rules`, `place_new_window`).
- **Force-fullscreen keybind** — `Super+Shift+F` toggles true fullscreen on the
  focused window regardless of whether the client requested it — a reliable
  rescue for games that only offer windowed mode (e.g. Hytale).
- **Flatpak apps in the launcher & dock** — Flatpak applications now show up in
  the Metis app launcher and the running-apps dock alongside native apps, with
  their proper names and icons. Flatpak installs export their `.desktop` entries
  and icons under dedicated `exports/share` trees rather than the system
  applications dir, and a login-manager session frequently does not source
  `/etc/profile.d/flatpak.sh`, so those trees were invisible to GIO (which the
  launcher and dock enumerate through). `metis-session` — and `run-metis.sh`
  for the dev session — now augment `XDG_DATA_DIRS` with the user
  (`${XDG_DATA_HOME:-~/.local/share}/flatpak/exports/share`) and system
  (`/var/lib/flatpak/exports/share`) export dirs before exporting the session
  activation environment (deduped; harmless when Flatpak is not installed).
  Because discovery already runs through `gio::AppInfo`, no launcher code change
  was needed for apps to appear. Window→app matching also learned the
  `X-Flatpak` desktop key: `AppEntry` captures it, and both the launcher
  (`resolve_entry_for_app_id`) and dock (`matches_app_id`) match a running
  window's `app_id` against it in addition to the desktop-id basename and
  `StartupWMClass`, so Flatpak windows that report their reverse-DNS Flatpak id
  (e.g. `com.valvesoftware.Steam`) group under and reuse the right launcher icon.
- **Client GPU steering (hybrid-laptop correctness)** — on the DRM/udev backend
  the compositor now resolves the PCI identity of the render node it actually
  draws on (`/sys/dev/char/<maj>:<min>/device` → `DRI_PRIME` tag `pci-DDDD_BB_DD_F`
  and `MESA_VK_DEVICE_SELECT` `vendor:device`) and forwards it to every spawned
  client. Games, Proton, XWayland, and Vulkan apps now default to the **same** GPU
  the session renders on instead of silently picking the wrong card on hybrid
  systems. Each variable is only set when unset, so per-game Steam launch options
  (`DRI_PRIME=1 %command%`, `prime-run`, NVIDIA offload vars) still win; set
  `METIS_NO_CLIENT_GPU=1` to disable forwarding entirely. Inert under the nested
  winit backend, where the host compositor owns device selection.
- **Steam Big Picture launcher** — when Steam is installed (native `steam` on
  `PATH` or the `com.valvesoftware.Steam` Flatpak), the app-menu rail shows a
  controller-friendly **Big Picture** button that runs `steam -gamepadui` (or the
  Flatpak equivalent). The entry is hidden entirely on machines without Steam, so
  non-gaming setups are unaffected (`applications::steam_big_picture_command` +
  `menu.rs`).
- **Weather fallback provider (MET Norway)** — the bar weather service now falls
  back to `api.met.no` (MET Norway Locationforecast 2.0) whenever the primary
  Open-Meteo endpoint fails. Some networks/ISPs blackhole `api.open-meteo.com`
  (`188.40.99.226`) at the TLS layer — TCP connects but the HTTPS session
  silently stalls — which stranded the widget on an error even though the rest
  of the internet was reachable. On an Open-Meteo failure the service now
  transparently queries MET Norway (a different host/operator), converting its
  Celsius hourly timeseries to the configured unit and translating MET's
  `symbol_code` strings into the same WMO codes the icon table already uses. A
  proper identifying `User-Agent` is now sent on all weather requests (MET
  requires one). Purely additive: Open-Meteo stays primary, and the primary
  error is still what's surfaced if both providers are unreachable
  (`services/weather.rs`).

### Fixed

- **Tray "Exit"/"Quit" ignored on the first right-click (Steam)** — a
  `StatusNotifierItem` context menu built its dbusmenu tree *asynchronously* after
  `AboutToShow`; Metis fetched the layout immediately and parsed a stale/empty
  tree, so the first click targeted a dead item id and only the second open (after
  the menu had settled) worked. When `AboutToShow` reports the layout needs an
  update, the shell now waits briefly for the client to publish it before reading
  the layout, so the very first open has live item ids
  (`metis-shell/services/tray_watcher.rs`).
- **Steam window drag (diagnostics)** — added logging to the XWayland
  `move_request` (`_NET_WM_MOVERESIZE`) and `configure_request` (client self-move
  via ConfigureRequest) paths so we can tell exactly how Steam attempts to move its
  self-decorated window (managed X11 toplevels are Metis-positioned, so a
  configure-request self-move is otherwise dropped silently) (`xwayland.rs`).
- **Edge bar never returns after a fullscreen game exits** — the bar's
  hide-while-fullscreen logic used a per-output refcount, and any missed
  decrement (a launcher that hides the bar then hands off to a game and closes
  while its record's fullscreen flag had already been cleared, an output-name
  mismatch between enter and exit, etc.) stranded the counter above zero, so the
  bar stayed hidden until `metis-shell` was killed and the session re-logged.
  Fullscreen state is now tracked as a per-output **set of window ids**, and a
  window's hold on the bar is released **unconditionally on teardown**
  (`drop_window_fullscreen`, called from every destroy/withdraw/un-fullscreen
  path) rather than depending on a still-set flag. Bar visibility is now a pure
  function of live state and can no longer drift (`state.rs`, `xwayland.rs`).
- **Fullscreen surface offset (wallpaper visible along the top/left edge)** —
  a CSD game (Hytale, GLFW/libdecor) kept drawing its own titlebar+shadow frame
  on the *first* fullscreen and reported a window geometry whose origin sits
  *above* its buffer (`geometry().loc.y = -37`). Compositing subtracts that inset
  (`render_loc = elem_loc − geometry.loc`), shifting the surface down/right and
  exposing the wallpaper along the top/left edge — the offset the client only
  cleared after toggling fullscreen off/on. Root cause: the client negotiated
  client-side decorations, and `refresh_window_decoration_mode` (which reads the
  *committed* decoration mode) would re-grant ClientSide even for a fullscreen
  window. A fullscreen surface has no chrome, so:
  - entering fullscreen now grants the client **server-side** decorations, which
    makes libdecor drop its own frame on the very first fullscreen configure
    (`set_fullscreen`, `push_preferred_decoration_mode`);
  - `should_draw_metis_ssd` returns false for fullscreen windows, so the forced
    server-side grant can't loop back into Metis painting its own titlebar over
    the game; and
  - exiting fullscreen restores the client's windowed decoration mode.
  Also gave `reclamp_auto_hide` a `!fullscreen` guard (mirroring
  `reclamp_maximized_geometry`) so a window that entered fullscreen while
  maximized is never re-anchored to the gap-inset maximized footprint. A
  once-per-session diagnostic logs the placement math whenever a fullscreen
  window would still not sit flush at its output origin (`state.rs`, `render.rs`).
- **Edge bar not restored after a fullscreen game exits (follow-up)** — in
  addition to the drift-proof per-output fullscreen tracking, the Wayland
  `toplevel_destroyed` path now releases a window's hold on the bar
  *unconditionally* rather than only when the window was flagged `ready`, so a
  game that fullscreened and then exited before its first activation can no
  longer strand the bar hidden (`handlers/xdg_shell.rs`).
- **Bare `Esc` now reaches games and apps** — the compositor previously
  swallowed every bare `Escape`, breaking in-game menus (Hytale) and cancel/close
  in every app. The un-fullscreen/un-maximize/un-tile escape hatch moved to
  `Super+Esc`; bare `Esc` always forwards to the focused client (`input.rs`).
- **Steam/CEF menus appearing in the top-left corner** — `configure_request` for
  XWayland windows configured at `geometry().loc` (always ~(0,0)), teleporting the
  X-server root position to the corner on every self-resize and desyncing it from
  the Metis Space location. Since override-redirect popups are positioned by the
  client in root coordinates, they landed top-left. Configures now anchor at the
  window's actual Space location so menus appear under their anchor (`xwayland.rs`).
- **Steam maximize/minimize/activate** — implemented the missing
  `maximize_request`/`unmaximize_request`/`minimize_request`/`unminimize_request`
  and `active_window_request` `XwmHandler` hooks, routed through the shared
  `set_maximized`/`minimize_by_id`/`restore_by_id`/`activate_window_by_id` paths
  (`xwayland.rs`).
- **Tray "Exit" (Steam) reliability** — the dbusmenu click now sends an
  `AboutToShow` on the menu root before the item's subtree and delivers the
  `clicked` event with an `i32` payload (matching Qt/libdbusmenu expectations
  rather than the previous `u32`), with the parsed item ids/labels and the
  dispatch result logged for diagnosis (`tray_watcher.rs`).
- **Steam menus closing on the first mouse move** — with a self-decorated X11
  app's dropdown/tooltip open, moving the mouse instantly dismissed it. The
  per-motion focus-stacking pass (`maintain_focus_stacking`) auto-raises the
  registered window under the cursor, which restacked the toplevel *above its own
  override-redirect popup* — the owning app then treated the covered menu as
  dismissed. Stacking is now left untouched while any override-redirect popup is
  mapped (`has_mapped_override_redirect_popup`), so menus stay put as the pointer
  moves into them (`state.rs`).
- **XWayland popup menus dismissing themselves / flicker (Steam)** — hovering a
  self-decorated X11 app's menu (Steam's dropdowns, tooltips, combo popups) would
  make it flicker and auto-close after a moment. Override-redirect popups are
  intentionally kept out of the window registry, but the pointer-routing scan
  (`topmost_window_at_pointer`) only considers registered windows, so it handed
  hover/click focus to the toplevel stacked *beneath* the popup. The owning app
  saw the pointer as having left its menu and dismissed it (and the per-frame
  focus ping-pong read as flicker). `window_surface_at` now honors true stacking
  order first: when the topmost element under the pointer is a raised
  override-redirect popup, it wins pointer focus over the registered window below
  it (`desk_input.rs`).

### Docs

- **Flatpak host prerequisites** — `docs/USER_GUIDE.md` and `docs/UBUNTU_DEV.md`
  now document the required packages (`flatpak`, `xdg-desktop-portal`,
  `xdg-desktop-portal-gtk`), the Flathub remote, `input`/`video`/`render` group
  membership, and that Flatpak apps appear in the launcher automatically (with a
  re-login note for existing installs and a troubleshooting row for when they do
  not). Corrected a stale "idle inhibit portal not implemented" troubleshooting
  row (it shipped 2026-07-02) and refreshed the Phase 6 status in `README.md`.
- **Steam / Proton / gaming guide** — expanded the `docs/USER_GUIDE.md`
  "Steam, Proton & SteamOS-class gaming" section: native vs Flatpak install
  (`steam-devices`, PipeWire, `~/.var/app` pressure-vessel layout, `--device=all`
  / `--filesystem` overrides), Proton (Experimental / GE-Proton, common
  failures), the new hybrid-GPU client default and per-game dGPU offload launch
  options, the Big Picture menu entry, Steam Input / controllers (Metis does not
  grab evdev), the Steam overlay (click-to-focus), Remote Play (ScreenCast pump),
  power while gaming (idle-inhibit), polish prefixes (`gamemoderun`, MangoHud,
  vkBasalt), and SteamOS/handheld notes. Added Big Picture + overlay
  troubleshooting rows and refreshed the wrong-GPU row. `docs/UBUNTU_DEV.md`
  documents the client-GPU env behavior alongside `METIS_DRM_DEVICE`; `README.md`
  and `TODO.md` Phase 6D/E/F updated. GameMode is documented (install `gamemode`,
  use `gamemoderun`) rather than built — it is its own D-Bus service. Hardware-
  dependent items (actual Proton launch, dGPU offload on a real hybrid laptop,
  overlay in specific titles, Remote Play streaming) are documented and remain to
  be verified on hardware.

## [2026-07-02]

### Added

- **Compositor-rendered session lock** — Metis now has a first-party lock
  screen. `Super+L`, the shell menu's **Lock** button, and the `LockSession` IPC
  command all put the compositor into a locked mode where it renders the lock UI
  itself (background + optional blur/dim + clock + password field) and no client
  is composited or sent input; keyboard focus is cleared, and focus/reveal/launch
  and screen-capture IPC are refused while locked. Authentication runs real
  **PAM** on a worker thread (service `/etc/pam.d/metis`, installed by
  `run-metis.sh`, with a graceful fallback to the system `login` stack), and the
  result is marshaled back to the event loop via a `calloop` channel; the typed
  password is never logged, is zeroized after every attempt, and failures are
  throttled. The lock background is configurable on **Settings → Appearance →
  Background → Lock screen**: reuse the desktop wallpaper (default), a dedicated
  picture, a solid colour, or a gradient — with blur and a ~35% dim on by
  default, a show-clock toggle, and a **Lock when the screen blanks** option that
  closes the idle → lock loop (`lock.json`, live-reloaded via `ReloadLock`). The
  lock UI reuses the existing `fontdue` text and wallpaper/blur GL pipelines, so
  it renders identically on the nested winit dev session and the DRM backend.
  The lock screen now also: offers a **12-/24-hour clock** toggle (Settings →
  Background); draws a rounded, translucent **password field** (brighter border
  once you start typing, with an "Enter Password" placeholder) so it's obvious
  where input goes; greets you by your account **display name** (GECOS), falling
  back to the login name; and shows **suspend / restart / shut down** controls in
  the bottom-right that highlight on hover and are clickable straight from the
  lock screen (each drawn as a small anti-aliased glyph).
- **Idle screen blanking with inhibitors** — the compositor now powers the
  display down after the Settings → Power **screen blank** timeout
  (`power.json` `blank_after_minutes`, `0` = never) and wakes instantly on any
  input. A new `idle` module tracks the last input activity, arms a self-
  correcting calloop timer, and on timeout drives every connected DRM connector's
  `DPMS` property off (`udev` backend; a no-op under the nested winit dev
  session). Blanking keeps the `wl_output` globals and CRTC mode intact, so
  clients never see a monitor "disconnect" and nothing reflows across a
  blank/wake cycle; scan-out is suspended while blanked so a page-flip can't wedge
  a powered-down connector. The blank timeout live-reloads — saving the Power
  page sends the new `ReloadPower` IPC command and the compositor re-arms without
  a restart.
- **Idle inhibitors (keep the screen awake)** — three inhibitor sources feed a
  single count that suspends the blanker (and wakes a blanked screen):
  - **Wayland `zwp_idle_inhibit_manager_v1`** — native apps (video players,
    presentations) that mark a surface as "keep awake".
  - **`ext_idle_notify_v1`** — idle-notification clients (swayidle-style) are kept
    in sync via Smithay's `IdleNotifierState` (activity + inhibit state).
  - **D-Bus `org.freedesktop.ScreenSaver`** (and
    `org.freedesktop.PowerManagement.Inhibit`) — the interfaces Chromium/Electron,
    Firefox, SDL games, and VLC/mpv actually use. `metis-portal` now owns both
    names, allocates per-caller cookies, forwards `Inhibit`/`UnInhibit` to the
    compositor over the new `InhibitIdle`/`UninhibitIdle` IPC commands, and
    reclaims a crashed client's inhibitors when its D-Bus peer drops off the bus
    (`NameOwnerChanged` watch), so a dead browser can never pin the screen awake.
  - While any inhibitor is held the compositor also takes a **logind `idle`
    inhibitor lock** (`systemd-inhibit --what=idle --mode=block`), so a running
    game/video blocks automatic idle-suspend as well as blank — without blocking
    a manual suspend. Released as soon as the last inhibitor clears.
- **Settings inputs commit cleanly** — the Power page persists on a debounced
  background thread (no more UI stall from `powerprofilesctl`/`busctl` on every
  keystroke), stops periodic refreshes from stomping a value mid-edit, and every
  settings page now drops focus from an entry when you click empty space.
- **Per-output ICC colour calibration (hardware gamma)** — a display's ICC
  profile assigned on the Settings → Display page now actually affects the
  screen. A new dependency-free parser (`color_management/vcgt.rs`) reads the
  profile's `vcgt` calibration curves (both table and formula encodings, fully
  bounds-checked with unit tests), and `output_gamma.rs` resamples them to each
  CRTC's gamma length and uploads them via DRM `set_gamma`. Calibration is
  applied on every outputs reload (after `apply_color_profiles`), on connector
  bring-up, after mode-sets, and re-applied on VT-switch/session resume (which
  reset the ramp). Outputs with no profile — or a profile without a `vcgt` tag —
  get an identity ramp, so toggling a profile off restores neutral output.
  Full 3D gamut mapping / HDR (a GLES LUT post-pass) remains a documented
  Stage 2 follow-up.
- **`wp_color_management_v1` handler hardened** (still opt-in, `METIS_COLOR_MGMT=1`)
  — every request now initialises the objects it creates, validates client
  input, and reclaims its description records on destroy (previously the
  `descriptions`/`description_objects` maps grew without bound as Chromium
  churned image-description objects). The global is **not** advertised by
  default: advertising it still destabilises the session — Chromium (Cursor)
  engaged the colour pipeline and the compositor died with heap corruption
  (`malloc_consolidate(): unaligned fastbin chunk`), blanking the display. The
  per-output ICC hardware-gamma calibration above is independent of this global
  and stays on. See below for the root-cause investigation.

### Fixed

- **`wp_color_management_v1` description records no longer leak** — destroying a
  `wp_image_description_v1` now runs a `destroyed` handler that drops its record
  from the `descriptions`/`description_objects` maps (and inert/error-path
  descriptions register for the same cleanup). Chromium/Ozone create and destroy
  image descriptions constantly — reusing the freed protocol ids for other
  interfaces — so without this the maps grew without bound and pinned each
  destroyed object's `alive` `Arc` for the life of the session.

### Investigated (not yet fixed)

- **`wp_color_management_v1` heap corruption root-caused to the wayland-rs object
  lifecycle** — the crash that blanks the display when the global is advertised
  was reproduced deterministically (~4 s) in a nested `--session` under gdb with
  `METIS_COLOR_MGMT=1` and a Chromium client, matching the hardware signature
  exactly (`malloc_consolidate(): unaligned fastbin chunk`). The abort backtrace
  is a use-after-free while dropping a wayland `ObjectData` `Arc` inside
  `wayland-backend`'s `resource_dispatcher` — **not** in Metis code. It fires
  when Chromium destroys a `wp_image_description_v1` and immediately reuses the
  freed protocol id for a `wp_image_description_info_v1` in the same dispatch
  batch. None of the compositor's `unsafe` (the ICC memfd path) executes in the
  crashing trace — only safe `get_information` → parametric-info event sends — and
  adding the description-cleanup handler above did not change it, confirming the
  fault is in the wayland-rs/Smithay-fork lifecycle rather than Metis. A
  dependency bump was tried and ruled out: `wayland-backend 0.3.15` /
  `wayland-server 0.31.13` are already the newest published, and bumping
  `wayland-protocols` to 0.32.13 (which only adds the `windows_bt2100` v3
  feature) reproduced the identical crash. Since the generic destroy+id-reuse
  pattern works for every other client, this is a colour-management-path bug in
  the sys backend, not a version lag. **Decision:** the global stays **opt-in**
  and we wait for an upstream fix; revisiting means an ASAN build to pin the
  exact allocation, or bisecting which generated info event triggers it.

- **`wp_color_management_v1` can no longer panic and abort the compositor** —
  several request handlers returned early on error (unresolved output, a surface
  that already had a colour object, `create` before the required ICC/parametric
  data was set, `get_information` on a stale/opaque description, and the
  unadvertised `create_windows_scrgb`) while leaving the request's newly created
  protocol object uninitialised. wayland-rs treats an uninitialised `New<>` as a
  fatal error and panics, which under the DRM backend takes down the whole
  session. Every such path now initialises the object first and then raises the
  spec-mandated protocol error (`unsupported_feature`, `incomplete_set`,
  `surface_exists`, `no_information`), faulting only the offending client. This
  is what made the protocol safe to advertise by default.

- **First-class XWayland window management** — X11 windows are now full Metis
  citizens instead of bare centered surfaces. They enter the shared window
  registry, receive a Metis server-side titlebar (drag-move, close/maximize/
  minimize controls, edge-resize), are placed as bar-aware floating windows
  (restoring saved per-app geometry), participate in focus/stacking, and appear
  in the dock with open/close/focus IPC events. This is what actually fixes
  **Claude Desktop**: its launcher defaults to `--ozone-platform=x11`
  (`use_x11_on_wayland=true` unless `CLAUDE_USE_WAYLAND=1`), so Claude runs as an
  XWayland client and never used the native-Wayland `xdg-decoration` path the
  previous fix targeted — which is why it kept loading under the edge bar with no
  titlebar and no way to move or resize it.
  - `WindowRecord` now holds a `WindowSurface` enum (`Wayland(ToplevelSurface)` /
    `X11(X11Surface)`); `WindowRegistry::register_x11` keys X11 windows by their
    stable X11 window id (their `wl_surface` may associate later).
  - A single `send_window_configure` seam pushes geometry to either client type
    (Wayland pending-size + configure, X11 absolute `configure`), so
    `apply_window_rect`, maximize, snap-restore, and resize all work for both.
  - X11 windows are floating-only (kept out of the tiling grid); maximize and
    fullscreen route through the X11 `set_maximized` / dedicated fullscreen path.
  - Move/resize grabs and titlebar hit-testing were switched from surface-keyed
    to `id_for_window` lookups so they no longer panic on X11 windows.
  - Override-redirect surfaces (menus/tooltips) stay undecorated and untracked.

### Fixed

- **Electron apps launched from the Metis menu prefer native Wayland (fixes Claude
  Desktop "opens then closes")** — Claude Desktop launched from the menu opened for a
  moment and immediately quit. Its own launcher log showed `Electron exited with
  code: 0` (a clean `window-all-closed` quit, not a crash): Claude defaults to the
  **XWayland** backend (`--ozone-platform=x11`), and Electron's XWayland map/unmap
  window juggling on launch is unstable under Metis, so it tore its own windows down
  and quit. Launching it as a native Wayland client (`CLAUDE_USE_WAYLAND=1`) is
  stable. Metis now injects native-Wayland preference into every app it spawns —
  `ELECTRON_OZONE_PLATFORM_HINT=auto` (standard Electron opt-in) and
  `CLAUDE_USE_WAYLAND=1` (Claude's launcher ignores the hint and force-passes
  `--ozone-platform=x11` otherwise) — from the compositor's client-spawn env, the
  installed `metis-session`, and the dev `run-metis.sh --session`. Both defer to any
  value the surrounding session already set, so XWayland can still be forced per app.
- **Restoring an XWayland app from its system tray no longer flashes open and
  closes** — a client-driven remap (e.g. left-clicking Claude Desktop's tray icon)
  went straight through `map_x11_toplevel` without adopting the active workspace,
  unlike the dock-activate path. `apply_window_rect`'s visibility guard then unmapped
  the window whose stale workspace no longer matched the active one, so it opened for
  a frame and vanished. The remap path now re-homes the window to the current
  workspace before applying geometry.
- **XWayland snap zones now apply correctly** — dragging an X11 window into an edge
  snap zone tiled it to the wrong size: the move-grab release ran an unconditional
  X11 position-sync `configure` *after* `apply_snap`, clobbering the snapped size
  with the client's pre-snap size. The sync is now scoped to plain drags only
  (snap/tile paths already push geometry to the client themselves).
- **Electron "close to tray" no longer leaves a hollow dock window, and tray restore
  no longer flashes open-and-closed** — when an X11 app (Claude Desktop) withdraws
  its window to the system tray, Metis drops it from the dock/tasklist (matching
  GNOME/KDE) instead of keeping a stale entry that restored to an empty frame. The
  withdraw is now **debounced** (600 ms): Electron unmaps/remaps its X11 window
  constantly during normal operation and while restoring from the tray, so reacting
  to every unmap thrashed the dock and made the window flash open then vanish. Metis
  now only treats a *sustained* unmap as a real withdraw; a re-map within the grace
  period cancels it. The registry record is retained keyed by X11 window id, so a
  genuine tray restore re-announces (WindowOpened) and re-shows the window with focus
  on the current workspace.
- **XWayland windows no longer snap to the top-left corner under the edge bar** —
  `configure_notify` was mapping the compositor-side element to whatever position
  the X11 client reported. Chromium/Electron apps (Claude Desktop) map at `(0,0)`
  and re-assert their own position, so right after Metis placed the window below
  the bar the client's next configure dragged it back into the corner — both on
  first map and immediately after an interactive move. Metis now owns placement
  for managed X11 toplevels and ignores client-driven position changes (only
  override-redirect surfaces track their own geometry); the move grab syncs the X
  server to the final on-screen position on release so client popups/menus still
  land correctly.
- **Terminals get Metis server-side titlebars again** — `SSD_APP_IDS` now lists the
  bare runtime `app_id`s terminals actually report (`kitty`, `Alacritty`, `foot`,
  `ghostty`, `wezterm`, `xterm`, …) instead of only reverse-DNS forms. Terminals
  bind `xdg-decoration` early, so an unlisted id fell through to the CSD fallback
  and lost Metis chrome. Also removed the garbled `org.wezfwez.foot` entry.
- **Frameless Electron apps (e.g. Claude Desktop) get a Metis titlebar** — these
  apps report the generic `chromium` Wayland `app_id`, so Metis treated them like
  the Chromium browser (client-side decorations) — but they launch Ozone with
  `WaylandWindowDecorations` + no custom titlebar, i.e. they *ask the compositor*
  to decorate and draw no controls themselves. That left them with no titlebar,
  no resize borders, unmovable, and mapped under the edge bar. `resolve_uses_ssd`
  now **honors an explicit `xdg-decoration` server-side request over the
  `chromium ⇒ CSD` heuristic**, so such apps get full Metis server-side chrome
  (titlebar, drag-move, resize, correct placement below the bar). Real browsers
  request client-side, so they're unaffected. As a secondary signal, a
  chromium-class window whose client process (`/proc/<pid>/comm`) isn't a known
  browser is also treated as an Electron shell.
- **Capture-overlay detection no longer grabs large app windows** — the non-portal
  geometry heuristic now requires a floating window to span the *entire* desktop
  (including the edge-bar strip at y≈0), rather than merely covering ≥85% of it. A
  normal or maximized app always sits below the bar, so it can no longer be mistaken
  for a screenshot overlay (Flameshot) and yanked under the bar or have its stacking
  hijacked.
- **System tray icons clear when an app exits** — the `StatusNotifierWatcher` now
  watches `org.freedesktop.DBus.NameOwnerChanged` and drops any tray item whose
  owning connection vanishes. Apps usually exit without calling
  `UnregisterStatusNotifierItem`, so their tray icon previously lingered until a
  later interaction failed with `ServiceUnknown`. Matches both items registered
  under a unique connection name (Claude: `:1.55`) and those advertising a
  well-known name (Flameshot / most Qt/KDE apps: `org.kde.StatusNotifierItem-…`),
  and reports the item's destination bus name so the bar UI actually removes it.
- **`outputs.json` reload feedback loop** — the night-light schedule time picker
  wrote its entry programmatically (`refresh` / reformat), which re-fired GTK's
  `changed` signal, rescheduled the 450 ms debounce, and called `save_and_apply`
  → `ReloadOutputs` again — an endless ~2×/sec loop that started with no user
  interaction while the Settings **Display** page was open. The picker now
  suppresses `changed` during its own writes and skips no-op applies. As
  defense-in-depth the compositor now treats a `ReloadOutputs` whose on-disk
  config is unchanged as a no-op (no wallpaper re-decode / reflow / log spam).
- **Settings: broken (missing) icons on several rows** — six symbolic icon names
  used across the settings pages don't exist in the Adwaita icon theme and were
  rendering as the "missing image" placeholder: `view-columns` (Edge bar → New
  workspace layout), `preferences-desktop-effects` (Windows → Window animations),
  `gesture-single-tap` / `gesture-drag` (Touchpad → Tap to click / Tap and drag),
  and `preferences-system-mouse` / `speedometer` (Mouse + Touchpad → Pointer speed
  / Acceleration). All now use on-theme names that actually ship (`view-dual`,
  `preferences-desktop-screensaver`, `input-touchpad`, `object-select`,
  `input-mouse`, `power-profile-performance`).
- **Settings: mouse wheel over a slider no longer drags it (and spams IPC)** — the
  Edge bar and Windows sliders (opacity, blur radius, distance, and the border
  thickness controls) now forward wheel events to the page scroller instead of
  changing their own value, matching the Display page. Scrolling past a slider
  scrolls the page rather than nudging the setting (which also fired a `reload-bar`
  on every notch).
- **Settings: toggle switches no longer lag when flipped** — the Edge bar "Backdrop
  blur" and Windows "Window animations" switches ran their `bar.json` write + IPC
  synchronously inside the toggle handler, so the switch visibly stalled before
  moving. They now paint the new state first and defer the save to the next idle
  turn (via `defer_switch_active_notify`).

### Changed

- **Settings: split the "Appearance" page into four focused pages** — the single
  page had grown to cover theme, colours, font, wallpaper, the entire edge bar,
  and every window-decoration control. It is now four sidebar entries under
  **Desktop**: **Appearance** (theme mode, accent/semantic colours, font),
  **Background** (wallpaper picture/solid/gradient plus per-display overrides),
  **Edge bar** (position, displays, workspaces, layout, tray mode, distance,
  opacity, blur, and the bar border), and **Windows** (animations, titlebar
  opacity, and the title-pill/window-frame borders). Shared colour/wallpaper
  plumbing moved to a new `appearance_common` module, and Edge bar/Windows now
  persist only the `bar.json` fields they each own (via `update_bar`) so the two
  pages can't clobber each other's settings.

## [2026-07-01]

### Added

- **Generic capture-overlay session** — Screenshot/screencast portal requests notify the
  compositor via `BeginCaptureOverlay` / `EndCaptureOverlay` IPC. Floating windows that
  span the desktop are elevated automatically (no per-app whitelist). While active,
  pointer routing and focus stacking ignore apps beneath the overlay so hover cannot
  steal stacking from tools like Flameshot; region-drag clicks stay on the overlay
  instead of raising apps beneath transparent areas.
- **Night light compositor** — Settings → Display night light toggle and colour
  temperature now apply live via a warm fullscreen overlay (`outputs.json`
  `night_light_enabled` / `night_light_temperature`); works in nested winit and
  DRM sessions.
- **Display — duplicate (mirror) mode** — Settings → Display **Duplicate displays**
  toggle with a **Show on** source picker; persisted in `outputs.json`
  (`display_mode`, `mirror_source`). On a DRM session with two or more monitors,
  the compositor maps all outputs to the origin, renders the source once per
  frame, and scale-to-fits onto each CRTC with letterboxing when resolutions
  differ. Uses the existing **Save display settings** keep/revert flow.
- **VRR / adaptive sync** — Settings → Display **Adaptive sync** toggle per
  output (Applies live); persisted as `vrr_enabled` in `outputs.json` and
  applied via DRM `VRR_ENABLED` on real hardware when supported.
- **Night light schedule** — Settings → Display **Use schedule** with local
  **From** / **To** times (overnight ranges supported); compositor applies the
  warm overlay only inside the window while night light is enabled.
- **Colour profile paths** — per-output ICC file picker in Settings → Display;
  paths saved to `outputs.json` and loaded by the compositor on apply (GPU
  colour transforms still pending).
- **`wp_color_management_v1` compositor handler** — Metis can advertise the colour
  management protocol when `METIS_COLOR_MGMT=1` (off by default). Per-output ICC
  profiles from `outputs.json` are loaded on apply/reload. Disabled by default
  because Chromium/Ozone currently destabilises the DRM session when the global
  is present.

### Fixed

- **Chromium / Cursor session crash** — `wp_color_management_v1` is now **off by
  default** (`METIS_COLOR_MGMT=1` to enable). Chromium/Ozone was crashing the
  DRM session once colour info was delivered; disabling the global restores the
  pre-protocol launch path while ICC profile loading in `outputs.json` remains
  for follow-up GPU transforms.
- **CSD browser chrome + resize** — grant `ClientSide` xdg-decoration as soon as
  app_id classifies Chromium/Cursor; defer decoration pushes until app_id is
  known; include `decoration_mode` on the first configure (Chromium latched
  server-side / close-only when an early bare configure raced ahead); restore
  compositor edge resize bands on native CSD footprints.
- **Chromium / video fullscreen** — client `xdg_toplevel` fullscreen requests
  (e.g. YouTube in Chromium) now enter true fullscreen on the correct output
  instead of being ignored; exiting video fullscreen from a maximized window
  restores maximize below the edge bar instead of leaving the window under the
  bar or losing maximized state (re-seat maximized CSD clients on commit and
  when the edge bar layer reclaims its exclusive zone).
- **Capture overlay false positive** — maximized Chromium was mistaken for a
  screenshot overlay (session lockup, window under the edge bar). Overlay
  detection now requires a floating near-fullscreen window and excludes browsers;
  input blocking only applies while an overlay window is actually registered.
- **X11 / XWayland fullscreen** — `_NET_WM_STATE_FULLSCREEN` from X11 clients
  (Steam, legacy players, etc.) now fills the monitor and restores the prior
  window geometry on exit.
- **Settings → Detect displays crash** — clicking **Detect displays** no longer
  panics with `RefCell already borrowed`; the display arrangement canvas now
  releases the selection borrow before updating the selected monitor.
- **Settings sidebar width on maximize** — left nav stays fixed at 248 px; only
  the page content expands when the window is maximized.

## [2026-06-30]

### Added

- **Settings — macOS-style control center** — grouped sidebar (Displays, Desktop,
  Connectivity, Input, System), coloured icon badges per page, headers with
  subtitles, inset grouped cards, sidebar search, and **Display** as the default
  landing page.
- **Display — resolution & refresh** — per-output DRM mode dropdown in
  Settings → Display; modes saved to `outputs.json` (`mode_width` /
  `mode_height` / `mode_refresh_millihz`) and applied via `DrmOutput::use_mode`
  on `ReloadOutputs` (real DRM session).
- **Display — arrangement canvas** — multi-monitor drag-to-arrange preview with
  **Save display settings** and a 15-second keep/revert confirmation;
  single-monitor preview only (no drag).
- **`ListOutputModes` IPC** — compositor exposes the DRM mode list per output for
  the settings UI (`OutputModeInfo`, `OutputModes` event).

### Changed

- **Settings navigation** — Display first; Weather and Calendars grouped under
  Desktop; Input and System sections aligned with GNOME / macOS conventions.
- **Display save model** — arrangement and resolution changes batch behind
  **Save display settings**; per-output scale and **Active** still apply live.

### Fixed

- **Settings scroll lag** — capture-phase wheel handler on page scrollers,
  non-overlay scrollbars, kinetic scrolling disabled; page content scrolls
  reliably over rows and controls.
- **Settings search lag and lockups** — plain search field (no GtkSearchEntry
  delay), 16 ms debounced filter while typing, immediate flush on Backspace/Delete
  release, selection guard during filter applies, and backspace key-repeat
  swallowed when the field is already empty.
- **Display arrangement on one monitor** — drag preview no longer commits layout
  live (compositor keeps a single output at the origin).
- **Settings row padding** — printers status, power battery line, and display
  per-monitor cards use consistent inset rows.
- **Arrangement save on single output** — no longer displaced the desktop away
  from the origin (edge bar, title bar, and regions stayed usable).

## [2026-06-29]

### Added

- **ScreenCast portal (Phase 3)** — `metis-portal` now runs a persistent
  `ext-image-copy-capture-v1` session and a ~30 Hz frame pump that pushes BGRx
  frames into a real PipeWire output stream node (dedicated PipeWire thread).
  `start_cast` spawns the pump; session close tears down the stream.
- **Clipboard history edge-bar widget** — new `clipboard` bar widget with a
  popover listing recent clipboard entries (text previews and image thumbnails).
  Click a row to recall via `SetClipboard` compositor IPC. History is persisted
  to `~/.local/state/metis/clipboard.json` (50-entry ring buffer; 10 MB image cap).
- **Clipboard IPC** — `CompositorEvent::ClipboardChanged` and
  `CompositorCommand::SetClipboard` in `metis-protocol`; compositor reads client
  clipboard offers on selection change and can set the Wayland clipboard from the
  shell.

### Changed

- **Live `bar.json` reload** — cosmetic edits (opacity, tray mode, margins, blur,
  borders, taskbar pins) now update the bar in place without destroying widgets or
  closing open popovers. A full rebuild runs only when the widget list, bar position,
  display set, clock format, or workspace count changes.
- **Bar symbolic icons** — bluetooth, clipboard, and notification bar icons use GTK
  symbolic rendering with theme text color so they match the rest of the bar in both
  light and dark modes. Removed the unused bundled Papirus icon set from
  `metis-shell`.

### Fixed

- **Terminal right-click and middle-click paste** — right/middle button press now
  syncs data-device and primary-selection focus from the resolved pointer target
  *before* titlebar/resize chrome handlers, so context menus and primary-selection
  paste work reliably in Wayland terminals (kitty, foot) across tiled, floating,
  maximized, and auto-hide-titlebar layouts.
- **Settings changes closed bar popovers** — changing tray mode, opacity, or other
  edge-bar settings used to run a full bar rebuild that called `close_all()` on
  every popover (bluetooth, clock, etc.) and recreated widgets. Cosmetic settings
  now take the live-apply path so open popovers stay up.
- **Weather flashed empty on bar reload** — the weather label briefly cleared
  during structural bar rebuilds. The shell now caches the last weather snapshot
  and re-applies it immediately after rebuild; the status label stays hidden until
  data arrives.
- **Tray pinned mode did nothing** — switching from collapsed to pinned in Settings
  left the tray empty until the next D-Bus update. The tray widget now rebuilds its
  icon row immediately when the mode changes.
- **Tray tooltips showed internal ids** — SNI items like Cursor could display
  `chrome_status_icon_1` instead of the app name. Tooltips now prefer the SNI
  `ToolTip` title and fall back to a readable name from the D-Bus bus name.
- **Tray icons washed out on a light bar** — pixmap tray icons and light symbolic
  assets are inverted or filtered in light mode so they remain visible against the
  pale bar surface.
- **Light-mode popover controls stayed dark** — entries, buttons, switches, and icon
  actions inside bar dropdown panels (clock calendar add/refresh/dismiss, clipboard,
  network, notifications, etc.) now derive colors from Metis theme tokens instead of
  GTK Adwaita defaults.
- **Bundled wallpapers missing in Settings** — the background picker now discovers
  `default.png` … `default9.png` from the workspace assets tree (and install
  prefixes), not a hard-coded `default.jpg` path relative to the binary.

## [2026-06-28]

### Added

- **Release build profiles** — workspace `release` uses thin LTO, `codegen-units=1`,
  and `strip=symbols` (~34% smaller installed binaries). Optional `release-small`
  profile (`opt-level=s`, fat LTO, compositor stays at `opt-level=3`) for ~56%
  smaller installs; `./run-metis.sh --release-small --install-session`.
- **Performance audit** — [`docs/PERF_AUDIT.md`](docs/PERF_AUDIT.md) documents
  compositor hot paths, hotspots (capture, scanout, `state.rs` size), and sizing
  measurements.
- **Bluetooth device battery in the edge bar** — the Bluetooth popover now lists
  every connected device with a battery icon and percentage when the device reports
  one. Low levels (≤20%) use an amber warning style; charging devices show a
  charging icon and `(charging)` label. A **Bluetooth settings** shortcut remains
  at the bottom of the popover.
- **Multi-source peripheral battery reads** — connected-device battery level and
  charging state are assembled from several sources, in priority order:
  1. Kernel HID battery (`/sys/class/power_supply/hid-<mac>-battery`) — capacity
     plus a `status` field mapped to charging/discharging.
  2. **UPower** — peripheral devices enumerated once per poll; percentage and
     `state` (charging / discharging / fully-charged) when the driver reports them.
  3. **Solaar** (optional) — when installed, `solaar show` is parsed for Logitech
     HID++ charging state (and percentage when BlueZ/UPower lack one). Results are
     cached (~20s) on a background thread in the shell so the ~2s CLI call never
     blocks the bar poller; if Solaar is absent the path is a silent no-op.
  4. BlueZ `Battery1` via `bluetoothctl info` — percentage only (the standard BT
     Battery Service has no charging characteristic).
- **Bluetooth low-battery alerts** — when a connected device's battery drops to
  ≤20%, Metis fires a one-shot in-bar notification (with sound, unless Do Not
  Disturb is on). Alerts use per-device hysteresis and are suppressed while the
  device is charging; the latch clears at ≥25% or on disconnect.
- **Power settings — connected devices** — Settings → Power now has a
  **Connected devices** section listing paired Bluetooth peripherals with battery
  percentage, charging icon, and low-battery styling (same source stack as the
  bar).
- **Bluetooth scan toggle** — Settings → Bluetooth **Scan for devices** toggles
  to **Stop scanning** while discovery is active and auto-stops after 30 seconds
  if left running.
- **Window animation effects** — optional maximize **wobble** (whole-window position
  ripple) and **genie** minimize (shrink/fade toward the edge bar before unmap).
  Toggle in **Settings → Appearance → Windows → Window animations**
  (`bar.json` → `window_animations`, default on).
- **Auto-hide titlebar slide-down** — maximized and edge-snapped SSD windows hide
  the Metis titlebar so the client fills the tile; hovering the top edge slides the
  chrome down as an animated overlay (~200 ms) with a sticky zone while shown.
- **Compact titlebar overlay for tabbed browsers** — Chromium-family apps keep SSD
  but reveal only a top-right control strip (~96 px) so the tab bar stays clickable;
  other SSD apps (kitty, Settings, …) use the full-width overlay.
- **Double-click titlebar to maximize** — a double-click on the titlebar (not the
  traffic-light buttons) toggles maximize/unmaximize; single-click no longer
  demotes a maximized window until you drag (5 px movement threshold).
- **Screenshot portal (`org.freedesktop.impl.portal.Screenshot`)** — `metis-portal`
  captures the desktop via a native Wayland client using
  `ext-image-copy-capture-v1` / `ext-image-capture-source-v1` (no `grim` or
  `wlr-screencopy`). PNGs are written under `$XDG_RUNTIME_DIR/metis-screenshot-*.png`
  and returned to apps through `xdg-desktop-portal`. Verified with Flameshot.
- **Compositor image capture** — the Smithay compositor registers
  `ext_output_image_capture_source_manager_v1` and
  `ext_image_copy_capture_manager_v1`, queues capture frames from the protocol
  handler, and renders them on the next GL pass into client SHM buffers.

### Changed

- **metis-shell tokio features** — trimmed from `full` to `rt`, `rt-multi-thread`,
  `macros`, `time`, `sync` (smaller shell binary, same runtime surface).
- **Client-side vs server-side decoration policy** — Chromium and similar tabbed
  browsers are forced to Metis server-side chrome; Firefox and GNOME apps keep
  native client-side decorations where appropriate.
- **Bluetooth polling** — `bluetoothctl` reads in the bar poller run every ~6s
  (was ~1.6s) and all external commands (`bluetoothctl`, `nmcli`, `upower`,
  `solaar`) go through bounded timeouts so a stuck daemon cannot stall the UI.
- **Wi-Fi bar icon stability** — during an `nmcli` rescan the active network can
  briefly disappear or report zero signal; the poller now holds the last known
  connection through a short grace window so the icon does not flash to "no bars"
  and back.
- **Portal stack startup** — `xdg-desktop-portal` and `xdg-desktop-portal-gtk`
  are started on a detached background thread so portal cold-start no longer
  blocks the compositor event loop (which caused a 10+ second black screen on
  login).
- **Client rendering defaults** — `GSK_RENDERER=cairo` is forced only for
  `metis-shell` (via `METIS_SHELL_GSK_RENDERER`); other spawned apps no longer
  inherit a global software-rendering override, restoring hardware-accelerated GTK
  for Chromium and other clients.
- **Portal backend selection** — `metis-portals.conf` sets `default=gtk` and
  routes Settings to Metis; `xdg-desktop-portal` is spawned with
  `XDG_CURRENT_DESKTOP=Metis` (GNOME stripped) so unimplemented GNOME portal
  backends are not probed (each miss used to cost ~25s). Portal files install to
  `/usr/share/xdg-desktop-portal/`; `XDG_DATA_DIRS` / `XDG_CONFIG_DIRS` propagate
  through the session activation environment.

### Fixed

- **Slow app launch (Chromium and others)** — root causes were forced Cairo
  rendering for all clients, synchronous portal startup blocking login, and
  `xdg-desktop-portal` timing out on missing GNOME backends; all three are
  addressed above.
- **Bluetooth battery parsing in Settings** — `Battery Percentage: 0x40 (64)` from
  BlueZ is now decoded via the decimal in parentheses (or hex fallback), matching
  the shell poller.
- **Settings build warnings** — removed unused imports/variables in the Display and
  Sound pages; wired up the previously unused `stop_scan()` backend.
- **Maximized auto-hide titlebar would not reveal** — the edge bar's input block
  (bar + margin + shadow pad) overlaps the top of maximized clients; moving the
  pointer there was treated as "over the bar," which cleared the reveal and never
  ran the slide-down logic, leaving apps like kitty stuck maximized with no way to
  reach the unmaximize control. Reveal now continues while over the bar strip,
  treats horizontal pointer-over-window in that strip as a trigger, and uses the
  full titlebar height on the client top edge.
- **Chromium tab bar blocked by full-width overlay** — the auto-hide titlebar
  spanned the whole window width; the compact top-right control strip fixes this.
- **Maximize wobble reset every frame** — post-snap `reclamp_auto_hide` ran on
  every commit during the wobble FX and snapped the window back; wobble is now
  skipped until the effect finishes.
- **Auto-hide titlebar clicks missed during slide** — decoration presses were
  hit-tested against the client frame instead of the sliding chrome rect.
- **Single-click unmaximize on maximized windows** — titlebar press now arms a
  pending drag instead of immediately demoting, so double-click and button clicks
  work reliably.
- **Screenshot capture sessions stopped immediately** — the compositor's
  `new_session` handler did not retain the owned `Session`, so the capture source
  was torn down before the client could attach a buffer. Sessions are now stored
  in `ImageCaptureRuntime` until the client destroys them.
- **Portal screenshot on Metis failed with Ubuntu `grim`** — Debian/Ubuntu grim
  only supports `wlr-screencopy-unstable-v1`, which Metis does not implement;
  replaced with the native `ext-image-copy-capture` client in `metis-portal`.
- **Resize edge bleed through a front auto-hide window** — when the top window's
  Metis titlebar was hidden, `resize_edge_at` skipped it (`ssd_frame_for_mapped_window`
  returned `None`) and the compositor hit-tested the window below, showing resize
  cursors and starting resizes through the foreground app. Resize bands now use
  client geometry plus a 12 px occlusion halo (`resize_frame_for_mapped_window` /
  `resize_occlusion_rect`).
- **Resize cursor bleed through maximized windows** — maximized and fullscreen
  windows were omitted from `resize_edge_at`, so a lower window's edge bands were
  still active under a maximized neighbor (resize cursor and click-to-resize on the
  background window). Maximized/fullscreen windows now occlude lower windows without
  exposing their own resize edges.
- **Pointer hover and click through the auto-hide titlebar overlay** — Smithay's
  `element_under` only considers mapped client geometry; the sliding Metis titlebar
  sits above that rect, so motion and button events could reach windows stacked
  below (background buttons highlighting and receiving clicks through the
  foreground titlebar). `topmost_window_at_pointer` now owns revealed overlay
  chrome and permanent SSD border strips before falling through to client geometry.
- **Log out left a blank screen on the DRM session** — `EndSession` only stopped
  the compositor event loop without tearing down clients or handing the VT back to
  the display manager. `end_compositor_session()` now kills spawned clients, switches
  to VT 1 on the DRM backend, then stops the loop; the shell also falls back to
  `loginctl terminate-user` when IPC does not reply.

## [2026-06-27]

### Added

- **Standalone DRM/KMS session — Metis runs as a real desktop on its own GPU.**
  A new DRM/udev + libseat + libinput backend lets the compositor own a TTY/GPU
  directly, alongside the existing nested winit dev backend:
  - **Backend selection** (`main.rs`) — autodetects nested (winit) vs. standalone
    (DRM) from `WAYLAND_DISPLAY`/`DISPLAY`; override with `METIS_BACKEND=winit|drm`.
    Nested-only side effects (host activation-env import) are confined to winit.
  - **Shared render path** — `render.rs::build_render_elements` and
    `state.tick_housekeeping()` are now shared by both backends; the DRM backend
    renders one framebuffer per output in output-local coordinates.
  - **DRM render** (`udev.rs`) — libseat session, primary-GPU selection (render
    node detection, `METIS_DRM_DEVICE` override), GBM allocator + `GlesRenderer`,
    a `DrmOutput` per connector, damage-gated page-flips driven by a 16 ms
    heartbeat + vblank (zero GPU work when idle), and a dmabuf global so EGL/GPU
    clients (GTK) submit hardware buffers.
  - **Input + session control** — real devices via libinput feed the shared
    `process_input_event`; relative pointer motion is clamped to the desktop.
    **Ctrl+Alt+F<n>** switches VT, **Ctrl+Alt+Backspace** safe-quits, and
    session pause/resume (VT switch / suspend) re-arms input and DRM.
  - **Pointer** — the DRM session paints its own XCursor-themed cursor (with a
    generated fallback) on the cursor plane and honors client `set_cursor`
    surfaces; `XCURSOR_THEME`/`XCURSOR_SIZE` apply.
  - **Hotplug / robustness** — live connector connect/disconnect with output
    re-packing (no gaps/overlap), and a clean shutdown if the primary GPU is
    removed.
  - **Login-manager entry (GDM/SDDM/greetd)** — `assets/metis-session` launcher +
    `assets/metis.desktop` wayland-session, installed via
    `run-metis.sh --install-session`; pick **Metis** from the greeter like
    Hyprland. `run-metis.sh --session --drm` runs it directly from a TTY.
- **Settings portal (`metis-portal`)** — a new `metis-portal` binary serves
  `org.freedesktop.impl.portal.Settings` for the standalone Metis session:
  color-scheme and gtk-theme from `metis-config`, plus empty decoration/button
  layouts so GTK clients defer to Metis server-side chrome. Registered via
  `metis.portal` + `metis-portals.conf`; the compositor starts it before
  `xdg-desktop-portal` on DRM boot (replacing the old `GDK_DEBUG=no-portals`
  workaround).

### Changed

- **Scrolling layout reworked (niri / paneru style)** — the strip is now an
  infinite row of full-height columns with continuous, mouse-resizable widths:
  - Drag a window's **right** border to set its width; columns to the right slide
    over to make room. Dragging the **left** border resizes the previous window.
    Columns are full-height, so there is no vertical resize. New
    `ScrollResizeGrab` drives the resize and reflows live.
  - Column width is stored as a fraction of the viewport (continuous), replacing
    the fixed ⅓/½/⅔/full presets. `Super`+`-` / `Super`+`=` now snaps the focused
    column to full width, then back to half.
  - Opening a new window never resizes the windows already on the strip — it just
    appends a column (new windows open at half-width).
- **Scroll viewport easing** — focus changes in a scroll workspace now animate
  the strip toward the focused column. Client surfaces stay mapped at the target
  offset; the compositor applies a render-time X nudge
  (`scroll_x_target - scroll_x`) so resize-averse apps are not reconfigured every
  frame.

### Fixed

- **New windows join the scroll strip** — in a scrolling workspace, opening a
  window used to drop it as a centered floating window on top of the strip
  (`place_new_window` floated scroll-mode windows). They are now strip-managed
  like grid tiles, so they form their own column.
- **Scroll strip reflows on open/close** — adding or removing a window now slides
  the existing columns into their new positions instead of leaving the newcomer
  painted on top of its neighbour (the scroll path had no equivalent of the grid
  auto-reflow).
- **Scroll columns clipped to their display** — a column scrolled past the screen
  edge is now clipped to its own output (via `CropRenderElement`) instead of
  bleeding onto the adjacent monitor; fully off-screen columns stay unmapped.
- **Copy/paste across XWayland and Wayland clients** — the compositor now
  bridges clipboard and primary selection between native Wayland apps and X11
  clients (e.g. Chrome → terminal). Previously only the data-device globals were
  advertised; selections were never forwarded in either direction.

## [2026-06-26]

### Added

- **Cross-output window moves** — dragging a window onto another monitor (or
  snapping it there) re-homes its desk tile and scroll membership to that output
  automatically. On grid workspaces, `Super`+`Shift`+`←`/`→` moves the focused
  window to the adjacent output; scroll workspaces keep those keys for column
  navigation. New `MoveWindowToOutput` compositor IPC command.
- **Move workspace to another output** — `Super`+`Ctrl`+`Shift`+`←`/`→` moves
  every window on the active workspace (under the pointer) to the same workspace
  number on the adjacent monitor, including scroll state and layout mode.
  Independent per-output workspace mode only. `MoveWorkspaceToOutput` IPC.

### Fixed

- **Session lockup on pointer input** — fixed a self-deadlock where moving the
  pointer over a window could freeze the whole session. Input hit-testing
  (`surface_under` / `focus_target_at`) acquired the per-output layer map and then
  re-entered it via grid-zone resolution; Smithay's layer map is a non-reentrant
  mutex, so the compositor thread deadlocked. The desk hit is now classified before
  the layer map is locked.
- **Free-mode window geometry restore** — in the default desktop layout, moving or
  resizing a window and then closing and reopening it now restores the saved
  position and size instead of always reopening centered at the default size.
  Placement is no longer locked in before the app's `app_id` is known (GTK assigns
  it just after the first commit), so the saved geometry lookup is no longer
  skipped.
- **Nested session keybinds** — GNOME grabs Super globally; nested dev sessions now
  default to `METIS_MOD=alt` so shortcuts work as Alt+… (override with
  `METIS_MOD=super` when desired).
- **Edge snap top gap** — left/right half snaps on the outermost monitor no longer
  pick up a spurious titlebar inset after cross-output adoption.
- **Mod+F maximize** — the shortcut now toggles maximize (usable area below the
  edge bar), matching the titlebar maximize button and top-edge snap, instead of
  true XDG fullscreen that drew under the bar.
- **Automatic grid tiling** — on grid workspaces, opening or closing an app window
  (or switching back to a workspace) re-splits the area below desk widgets among
  visible tiled windows; the focused window takes the primary slot when three or
  more share the workspace.
- **Scrolling layout polish** — column focus pans with a smooth viewport animation;
  off-screen scroll columns are unmapped so they no longer bleed onto adjacent
  outputs; vertical stacks distribute height evenly; scroll offset is clamped to
  the strip width. `ListOutputs` now marks the primary monitor and sorts outputs
  left-to-right.
- **Scroll layout lockup fix** — changing the default layout to Scrolling now
  actually seeds scroll strips (the early-return path used to skip that when
  `bar.json` already matched), `Mod+\` is debounced against key-repeat, and
  scroll animation only reconfigures scroll-layout windows instead of the whole
  session each frame.

## [2026-06-25]

### Added

- **Scrolling workspace layout (niri / PaperWM style)** — a second per-workspace
  layout mode alongside the grid. App windows form a horizontal strip of columns,
  each holding a vertical stack of windows; the viewport scrolls so the focused
  column stays visible.
  - **`metis-grid` scroll engine** — new `scroll.rs` (`ScrollState` / `ScrollColumn`
    / `ColumnWidth`) with pure insert/remove/focus/move/consume/expel/width-cycle
    ops and pixel-frame layout + scroll-into-view math. A `LayoutKind { Grid, Scroll }`
    enum selects the mode.
  - **Per-workspace mode** — each (output, workspace) tracks its own `layout_kind`
    and scroll strip. App tiles remain the membership/stash source of truth, so
    open/close, workspace switch/move, and dock filtering are unchanged; scroll mode
    only overrides pixel placement and hit-testing.
  - **Keybinds** (active scroll workspace only) — `Super`+arrows move focus across
    columns / within a column; `Super`+`Shift`+arrows move the column / window;
    `Super`+`,` consumes a window into the previous column, `Super`+`.` expels it to
    a new column; `Super`+`-`/`=` cycles the focused column width. `Super`+`\`
    toggles the active workspace between grid and scroll on any workspace.
  - **Settings + IPC** — `bar.json#default_layout` (Settings → Appearance → Edge bar
    → New workspace layout) acts as a live global on/off: changing it applies the
    mode to every workspace on every output at once (`SetDefaultLayout` IPC). A
    `SetWorkspaceLayout` command sets the mode for a single output/workspace.

- **Per-output workspaces (Phase 3)** — each output now owns an independent set
  of virtual workspaces, Hyprland-style. Every monitor has its own active
  workspace and its own grid of app windows; switching one output never disturbs
  the others.
  - **Per-output desk state** — the compositor's single global grid /
    `active_workspace` / stash was replaced by an `OutputDesk` per output
    (`desks: HashMap<output, OutputDesk>`), seeded lazily as outputs map. The
    primary output keeps the `desk.json` widget tiles; secondary outputs get an
    app-only grid. Grid metrics, tiling, hit-testing, and window placement are
    now computed against the window's (or cursor's) own output.
  - **Window → output binding** — windows are tagged with the output they open
    on (the one under the cursor) and tile within it; a window is mapped only
    while its output's active workspace matches.
  - **Keybinds** — `Super`+`1`…`9` switch the workspace on the output *under the
    pointer*; `Super`+`Shift`+`1`…`9` move the focused window between its own
    output's workspaces.
  - **Per-output bar dots** — each monitor's edge bar drives and reflects its own
    output's workspaces. The shell tracks active workspace per output and matches
    each bar to its compositor output via the GDK monitor connector name.
  - **Protocol** — `SwitchWorkspace` and `WorkspaceChanged` now carry an `output`
    name (back-compatible default); `WorkspaceChanged` is emitted per output.
  - **Workspace mode toggle** — Settings → Appearance → Edge bar → Workspaces lets
    you pick `separate` (independent per output, default) or `linked` (every monitor
    switches to the same workspace at once). Stored as `bar.json#workspace_mode`; the
    compositor routes workspace switches (`Super`+`n` and `SwitchWorkspace`) through
    `switch_workspace_routed`, fanning out to all outputs in linked mode.
- **Taskbar follows the output + workspace** — each monitor's dock now shows only
  the windows on that output's currently-visible workspace (pinned launchers still
  appear everywhere). `WindowInfo.output` is populated with the window's monitor
  name; the dock filters by `(output, active workspace)`, repaints on workspace
  switch, and dedups per bar (so multiple monitors no longer thrash one shared
  signature). The shell reconciles its window cache on workspace change / window
  open so a window lands on the right dock immediately.

## [2026-06-24]

### Added

- **Multi-output groundwork (Phase 3)** — first slices of the output-agnostic
  refactor:
  - **Output-geometry foundation** — a centralized helper layer
    (`primary_output` / `output_rect` / `desktop_bounds`) that
    `grid_metrics`, `usable_zone`, `placement_zone`, `set_fullscreen`, and
    `arrange_layers` now route through instead of scattered
    `space.outputs().next()` / cached-`monitor` reads. Behavior is identical with
    one output; per-output work now only changes "which output" at these
    chokepoints.
  - **Virtual-output dev rig** — `METIS_VIRTUAL_OUTPUTS=2` tiles the nested winit
    window into two side-by-side logical monitors so multi-output behavior
    (per-output bars, placement, layer-shell) can be exercised before a real
    DRM/udev backend. A dedicated full-window render output drives the damage
    tracker / scale / wallpaper / frame timing, and the render loop now gathers
    layer-shell surfaces + bar blur from every output (offset by each output's
    global origin). Default (unset / `1`) is byte-for-byte the previous
    single-output path.
- **Virtual workspaces (single output)** — the workspace dots in the bar are now
  live. Each workspace is its own independently-tiled set of app windows; the desk
  widgets (clock/weather/rss) are shared across all of them. Switching stashes the
  current workspace's app tiles, hides its windows, then restores and remaps the
  target's, and focuses the topmost window there.
  - **Keybinds** — `Super`+`1`…`9` switch workspace; `Super`+`Shift`+`1`…`9` move
    the focused window to a workspace (digit detection is shift-independent).
  - **Bar dots** — clicking a dot switches via the compositor; the compositor's
    new `WorkspaceChanged` event keeps the active dot in sync (single source of
    truth), with an optimistic local update for instant feedback.
  - **Protocol** — new `SwitchWorkspace` / `MoveWindowToWorkspace` commands and a
    `WorkspaceChanged` event; `WindowInfo.workspace` is now populated (1-based).
    Workspace count comes from `bar.json` `workspace_count` (1–12, default 4).
  - **Multi-output input/drag fixes** — absolute-pointer motion now maps across
    the whole virtual desktop (union of all outputs) instead of the first output,
    so the cursor is no longer compressed into the primary monitor; and titlebar
    drags clamp to the full desktop bounds so a window can be moved between
    outputs (previously it was pinned to the primary output's zone).
  - **Per-output edge bar** — the shell now spawns one edge bar per connected
    output (bound via `gtk4-layer-shell` `set_monitor`), rebuilding on monitor
    hotplug. A new **Settings · Appearance · Edge bar → "Show bar on"** control
    (`bar.json` `displays`: `all` | `primary`, default `all`) switches between a
    bar on every display and a single bar on the primary output.
  - **Per-bar live updates on every output** — the notification and taskbar
    refresh registries held a single callback, so only the last-built bar updated
    instantly while others waited (5–10s) for an unrelated poll change. They now
    fan out to one (weak) hook per bar, so notifications and dock changes appear on
    every display at once. Volume/mic slider and mute actions now broadcast the
    new level to every bar instantly (optimistic, with poll suppression) so the
    other displays update immediately instead of waiting for the pactl read-back;
    audio actions also force an immediate poll read-back as a backstop.
  - **Popover positioning on secondary outputs** — `unconstrain_popup` now
    expresses the allowed area in the parent surface's local frame (subtracting the
    output's global origin as well as the layer offset), and toplevel popups
    constrain to the window's actual output. Without this, every bar popover
    (Metis Menu, calendar, Wi-Fi, notifications, weather) on a non-primary output
    was pushed off-screen and could neither be seen nor clicked.
  - **Per-output wallpaper** — the desktop background is now composed per output
    instead of one image stretched across the whole framebuffer. Each display is
    cover-cropped to its own resolution (so the same picture fills a 16:9 and a
    16:10 monitor correctly), and a display can carry its own picture via
    `wallpaper.json`'s new `per_output` map (output name → path). The compositor
    still uploads a single framebuffer-sized texture (each output's crop blitted at
    its global origin), so the bar backdrop-blur path is unchanged. A new
    **Settings · Appearance · Per-display background** card (shown only with 2+
    displays) lists each output and lets you set or clear a per-display picture;
    it discovers outputs through the new `ListOutputs` IPC command
    (`CompositorEvent::OutputList` / `OutputInfo`). Sources are cached by path, so
    two displays sharing an image only read it from disk once.
  - **Per-output window placement, snapping & maximize** — window management now
    follows the monitor the cursor (or window) is on instead of always the primary
    output. New floating windows open centered on the output **under the cursor**;
    dragging a window to a screen edge snaps/tiles it on the **hovered** output;
    the maximize button/zone fills the output the window currently sits on; and a
    floating window is clamped within **its own** monitor so it's no longer yanked
    back to the primary. Built on new output-resolution helpers (`output_at` /
    `output_under_pointer` / `output_for_window`) plus output-parameterized zone
    helpers (`usable_zone_for` / `placement_zone_for` / `window_placement_zone_for`
    / `centered_rect_in`); the overlay-bar strip is now reserved only on outputs
    that actually show a bar. Grid/tiling stays on the primary output until
    per-output workspaces land. Single-output behavior is unchanged.
- **Metis Menu settings page** — a new **Settings · Metis Menu** page gathers all
  launcher settings in one place, separate from the Edge bar card:
  - **Quick launchers** — choose which **terminal** and **file manager** the rail
    opens. Each picker auto-detects installed options
    (kgx/gnome-terminal/konsole/foot/… and nautilus/dolphin/nemo/thunar/…) and
    offers a **Custom** entry with a file chooser to point at any executable path.
    Choices persist to `menu.json` (`terminal` / `file_manager`); the shell reads
    them at launch time and falls back to `$TERMINAL`/`$FILE_MANAGER`, then the
    known candidates, then `xdg-open` — so an unset or missing program degrades
    gracefully without a restart.
  - **Appearance** — the menu **panel opacity** moved here from the Edge bar card
    (still stored in `bar.json` `menu_opacity`, applied live via `reload-bar`).

### Fixed

- **Snapping/maximizing on a secondary output pulled the window toward the
  primary** — the snap math was correct, but the follow-up auto-hide re-anchor
  (`reclamp_auto_hide`, run after every snap/maximize) still clamped against the
  *primary* output's zone, so a left-half snap on the second display was dragged
  back across the monitor boundary (half on each screen). It now re-anchors within
  the window's own output.
- **Shell crash when changing the bar's "Show bar on" display set (root cause)** —
  removing a per-output bar tore down its `zwlr_layer_surface_v1`, and the
  compositor killed the shell with `invalid_size` ("width 0 requested without
  setting left and right anchors"). This was a smithay + gtk4-layer-shell teardown
  interaction: destroying a layer surface resets its cached state to defaults
  (size 0×0, no anchors), and the trailing `attach(null); commit` that the toolkit
  sends then fails smithay's pre-commit size/anchor validation. The compositor now
  installs a pre-commit hook (ahead of smithay's, registered on the bare surface in
  `new_surface`) that repairs that degenerate teardown state so the unmap commits
  cleanly. Normal layer surfaces are untouched. This is the actual fix; the
  shell-side rebuild changes below are defensive hardening that remain in place.
- **Shell crash when changing the bar's "Show bar on" display set** — several
  issues around rebuilding the per-output bars at runtime:
  - Rebuilding destroyed every bar window *before* building the replacement, so the
    `GtkApplication` briefly owned zero windows and auto-quit, tearing down the
    Wayland connection ("Error flushing display: Broken pipe"). The rebuild now
    builds the new bars first and destroys the old ones afterward (window count
    never hits zero, no one-frame flash), and the shell holds the application alive
    across transient zero-window states as a backstop.
  - A fresh bar's layer-shell role/anchors/output binding are now established before
    the window is realized or any child widgets are built — runtime window creation
    realizes immediately (unlike the forgiving startup path), so out-of-order setup
    could commit an invalid surface and get the client dropped.
  - Rebuild triggers are coalesced: a single settings change writes `bar.json`
    *and* sends `reload-bar`, so multiple rebuilds were storming in within a few
    hundred ms and racing each other; they now collapse into one deferred pass.
  - The compositor now logs client disconnects, including the exact object/code/
    message of a Wayland protocol error, so client-kill bugs are diagnosable
    instead of silent.
- **Bottom edge-bar left too much gap below maximized/snapped windows** — overlay
  bars reserved an extra `SHADOW_PAD` (16px) of transparent pad above the pill, so
  windows stopped ~18px short of the visible bar regardless of the **Distance from
  edge** value (even 0–1). All overlay bars (bottom/left/right) now reserve only the
  visible strip (`margin + body`), matching the top bar, so windows tuck right up to
  the pill with the same small `BAR_GAP_PX` breathing gap.
- **Start-menu scroll over a window** — with the launcher open above an app window,
  the window behind it stole wheel/click focus, so the app list only scrolled when
  the window was moved away. The compositor now gives the bar strip and its popovers
  top pointer priority in `surface_under`, hit-testing them before the desk-grid
  app-body passthrough; transparent popover gutters route to the popup surface
  instead of falling through to whatever is underneath.
- **Start-menu dismissal** — clicking the desktop or another window's titlebar now
  reliably closes an open popover. Hit-testing uses the bar strip's `bbox()` plus each
  popup's client `geometry()` (instead of `bbox_with_popups()`, which could balloon to
  the whole output and swallow outside clicks), and the `close-popovers` command is
  issued before titlebar/resize handlers can consume the press and return early.
- **Compositor deadlock on pointer move** — `surface_under` no longer calls
  `metis_bar_ui_hit()` while already holding the output's layer map (a re-lock that
  froze the compositor thread); the region check now uses the held guard directly.
- **Dark-mode start-menu scrollbar artifacts** — the shell now syncs GTK's built-in
  Adwaita dark/light variant to the saved theme preference on live `reload-theme`, and
  the menu CSS flattens GtkScrolledWindow undershoot/overshoot/trough chrome so the
  scroll gutter stays flat and scrollable in dark themes. Invalid GTK4 CSS (GTK3-only
  scrollbar stepper props, `min-width: 100%`) was removed.
- **Settings app re-theming the shell on open** — building the Appearance page no
  longer fires spurious `reload-theme` commands; programmatic init of the theme/font
  controls is suppressed so opening Settings doesn't re-trigger the shell's theming.
- **Type-to-search restored** — the launcher again filters as you type without first
  clicking the search box, via `SearchEntry::set_key_capture_widget` (which only takes
  focus once typing begins) instead of grabbing focus on open, so wheel scrolling over
  the app list keeps working.

### Changed

- **Input-routing & menu cleanup** — collapsed redundant compositor hit-test helpers,
  pruned the stacked `wire_vertical_scroll` controllers down to one Capture-phase
  controller per `ScrolledWindow`, and removed dead shell/compositor code (unused
  theme/bar/dropdown helpers and imports).

## [2026-06-23]

### Added

- **Configurable edge-bar position** — the bar can now anchor to the **top, bottom,
  left, or right** of the screen via **Settings · Appearance · Edge bar** (`bar.json`
  → `position`, now incl. `bottom`). The reserved exclusive zone follows the chosen
  edge, the pill sits flush against it (drop-shadow pad on the inner side), the
  layout flips between horizontal and vertical correctly on a live switch, and bar
  popovers / the app launcher / task pickers open away from the bar (down for top,
  up for bottom, right for left, left for right).
- **Edge-bar distance slider** — a new **Distance from edge** control sets the gap
  between the bar and its anchored screen edge (`bar.json` → `margin_top`, applied to
  whichever edge the bar is on).
- **Configurable edge-bar border** — the bar pill's border is now styleable via
  `bar.json` → `bar_border` and **Settings · Appearance · Edge bar**: `mode`
  (`accent` follows the theme accent gradient, `solid` a single color, or a custom
  `gradient`), per-stop colors, and `width_px` (**0 disables the border**). The
  gradient follows the bar's long axis and hugs the pill's rounded corners (rendered
  via a layered `background-clip` stroke). Applied live (~1s) — no restart.

## [2026-06-22]

### Added

- **Configurable title-pill border** — the window title now sits on a flat, solid
  plate (dark on dark themes, light on light) ringed by a thin accent stroke on the
  focused window; unfocused windows use a muted slate. The stroke is configurable
  via `bar.json` → `titlebar_pill_border` and **Settings · Appearance · Windows**:
  `mode` (`accent` follows the theme accent gradient, `solid` a single color, or a
  custom `gradient`), per-stop colors, and `width_px`. The accent gradient flows
  left→right across the pill. Picked up live (~1s) — no restart.
- **Configurable window-frame border (independent of the pill)** — the whole window
  frame (titlebar ring + left/right/bottom edges) draws the same style options as a
  **top→bottom** gradient, configured separately via `bar.json` → `window_border`
  and its own controls in **Settings · Appearance · Windows**. The frame
  **thickness** is now adjustable (`width_px`, 0–16px): it both strokes the border
  and insets the client body to match, applied live via a runtime grid border width
  (`metis_grid::set_app_tile_border_px`) plus a relayout so existing windows resize.

### Fixed

- **Titlebar click-through raising the wrong window** — clicking a foreground
  window over a spot where a window *behind* it had its titlebar/border could raise
  the background window. Server-side decoration presses are now hit-tested in
  stacking order (topmost first), so the window in front owns the click and a
  covered window's chrome can no longer intercept it. Genuinely exposed background
  titlebars still raise as expected.

## [2026-06-21]

### Added

- **App menu launcher (ArcMenu-style)** — the bar's Metis brand icon now opens a
  popover app menu anchored to the icon (not a fullscreen overlay): a left rail of
  quick launchers (Files, Terminal, Settings) and power actions (Suspend, Log Out,
  Restart, Shut Down via `systemctl`), a center column with a **Frequent Apps** +
  alphabetical list and an apps-only **search**, and a **Pinned** grid you can add
  to / remove from. Launching an app (or opening Settings) dismisses the menu
  synchronously so it never lingers behind the new window, and icon tooltips render
  as an in-surface overlay label so they always paint on top of the panel.
- **Start menu & window-titlebar transparency** — both the launcher panel and the
  server-side window titlebar can be made translucent, with independent **Start
  menu opacity** and **Titlebar opacity** sliders in Settings · Appearance (and
  `menu_opacity` / `titlebar_opacity` in `bar.json`). Only the backgrounds go
  transparent — text, icons, and the control buttons stay fully opaque. The
  titlebar is a cached, anti-aliased texture with rounded top corners and a border
  that wraps continuously around the window and under the titlebar.
- **Theme-aware titlebars + auto-hide for maximized / edge-snapped windows** — the
  server-side titlebar follows the active light/dark theme palette (live, via the
  compositor's ~1s theme poll). Maximized windows and left/right/top-corner snaps
  (whose top meets the bar) hide their titlebar so the client uses the whole
  footprint; moving the pointer into the top strip reveals it as a translucent,
  borderless overlay with working minimize / maximize / close, hidden again when
  the pointer leaves.
- **XWayland support** — the compositor spawns and manages an XWayland server
  (`X11Wm` / `XwmHandler`), so X11-only apps run inside a nested Metis session and
  are placed in the window grid alongside Wayland clients.
- **`run-metis.sh --import-env`** — an opt-in flag for `--session` that pushes the
  nested `WAYLAND_DISPLAY` (and `DISPLAY`) into the user D-Bus / systemd activation
  environment so D-Bus-activated apps open inside the nested session, restoring the
  previous values on exit.
- **Decoration polish — rounded buttons + focus-aware dimming** — the server-side
  window controls are no longer flat squares: each is a cached, anti-aliased
  rounded button rendered at 3× supersampling. Focused windows show the
  traffic-light colors (red/green/yellow) with a dark glyph (× close, + maximize,
  − minimize); unfocused windows desaturate all three buttons to gray with no
  glyph, complementing the existing focus-aware titlebar/border/title dimming.
  Button textures are cached per window and only re-rasterized when focus flips.
- **Settings · Appearance Font section** — a new Font card lets you choose the
  DE-wide UI font family + size (via a native font picker) and the body text
  color. Font family/size are stored as theme tokens (`font_family`,
  `font_size_pt`) and applied through the shared stylesheet's base `window` rule;
  when unset (the default) rendering is unchanged.
- **Settings · Network tab overhaul** — the Network page is now split into three
  segmented pill tabs (**Wireless / Wired / Proxy**):
  - **Wireless** keeps the Wi-Fi scan/connect/radio controls and known-networks
    list, and adds a **DNS override** for the active connection (a manual DNS list
    applied with `ipv4.ignore-auto-dns` so it overrides the DHCP-provided servers;
    empty restores DHCP DNS).
  - **Wired** offers per-NIC IPv4 configuration: **Automatic (DHCP)** or **Manual
    (static)** with address/gateway, plus a DNS override that applies in either
    mode.
  - **Proxy** edits the system proxy (**None / Manual / Automatic (PAC)**) with
    per-protocol HTTP/HTTPS/SOCKS host:port, an ignore-hosts list, and a PAC URL.
    Values are read from and written to GNOME's `org.gnome.system.proxy`
    gsettings (honoured by GLib/GTK apps via the default proxy resolver); the tab
    degrades to a hint when the schema is unavailable.
- **Window snap zones (edge/corner drag-to-tile)** — dragging a window by its
  titlebar shows a live translucent preview when the pointer nears a screen edge
  and drops the window into that region: top edge → maximize, left/right →
  halves, the four corners → quarters, bottom → bottom half. The maximize zone
  routes through the same path as the titlebar maximize button, and half/quarter
  snaps mark the window *tiled* (so GTK squares its corners and drops its CSD
  shadow) with edge gaps matching the maximize look — uniform spacing between and
  around snapped windows. Pulling a snapped window off by drag or edge-resize
  restores its normal floating chrome. The top/maximize trigger band is tight
  (16 px) so a normal drag upward doesn't prematurely maximize; the other edges
  use a roomy 64 px band. Core hit-testing lives in
  `metis-grid::pixel_snap_target` (unit-tested); the compositor applies the gaps,
  wires it into `MoveSurfaceGrab`, and renders the overlay in the winit pass.
- **Session relaunch guards + launch audit (`run-metis.sh`)** — `--session` is now
  hardened against an automatic close→reopen loop:
  - **Single-instance lock** (`flock`): a relaunch that overlaps a live session
    can't stack a second nested compositor.
  - **Rapid-relaunch cooldown**: a session that respawns within `4s`
    (`METIS_SESSION_COOLDOWN`) of a clean exit is refused, breaking an instant
    auto-reopen. Override an intentional quick restart with `METIS_FORCE=1`.
  - **Launch audit**: every invocation appends its PID + full parent-process
    chain to `~/.local/state/metis/launch-audit.log`, so an unexpected reopen can
    be traced to the exact invoker (the script/compositor have no respawn logic).
- **Window edge/corner resizing** — any window can now be resized by grabbing its
  edges or corners (an 8px band straddling the border). Because the compositor
  forces server-side decorations, it hit-tests the border itself and starts the
  existing `ResizeSurfaceGrab` directly. Grabbing a tiled window's edge pops it
  out of the grid into a freely-resizable floating window; the new size is
  persisted so it survives later re-layouts. The host cursor shows the matching
  directional resize arrow on hover (↔, ↕, and the two diagonals).

### Changed

- **Window dragging may now run off the left/right/bottom screen edges** —
  floating windows can be dragged partially off those edges (Hyprland-style),
  keeping a grabbable `MIN_VISIBLE_PX` (64px) slice on-screen so they can always
  be pulled back. The top edge is still hard-blocked just under the edge bar.
  Windows that end up off *every* active output (e.g. an external monitor that
  disconnected) are still rescued onto the primary screen.

### Fixed

- **Windows could be dragged "through" the open app launcher** — the bar's
  popovers don't take a Wayland pointer grab, so a press over the open menu fell
  through to whatever window resize band / titlebar sat geometrically beneath it,
  letting you move or resize that window through the menu. Presses over the bar UI
  (strip + any open popover, popup region included) now skip the window-chrome
  hit-tests entirely.
- **Snapped / maximized windows lost their screen-edge gap** — an app that can't
  shrink to its snapped footprint (minimum size larger than the zone, common on a
  small nested screen) overflowed past the reserved edge gap. Oversized auto-hide
  windows are now re-anchored to the snapped edge once they commit, so the
  screen-edge gap survives (the overflow spills toward screen center instead).
- **App launcher stayed open after launching an app** — launching from the menu
  deferred the popdown to an idle callback, which the newly focused window
  swallowed. App launches now close the menu synchronously before spawning.
- **Duplicate / mis-stacked launcher tooltips** — the rail showed both a native
  GTK tooltip and the custom one, and they rendered behind the translucent panel.
  The native tooltip is gone and the custom tooltip is an in-surface overlay label
  (accessible name preserved), so a single tooltip always paints on top.
- **Light-mode launcher search box stayed dark** — the search entry inherited
  GTK's default dark styling; it now uses theme-aware background/border/text/caret
  colors so it matches both light and dark themes.
- **Settings · unreadable text on dark accents** — picking a dark accent (e.g.
  black) left the on-accent text dark and illegible. Changing the primary accent
  now auto-derives a readable on-accent text color (near-black or white) from the
  accent's perceived luminance.
- **Settings · "Colours" → "Colors"** — the Appearance section and related color
  labels now use the American spelling.
- **Settings · colour picker was transparent** — the shared bar stylesheet makes
  every `window` transparent for the layer-shell overlays, which leaked into the
  settings app's spawned dialogs (the colour chooser rendered see-through). The
  settings app now forces a solid themed background on its own windows/dialogs.
- **Settings · Network card padding + stretched Wi-Fi toggle** — list/card content
  (SSID rows, NIC editor text) was flush against the card edges; the
  `.metis-settings-list` container now has internal padding, and the Wi-Fi radio
  switch centres vertically instead of stretching to the row height.
- **Resize band swallowed edge-hugging scrollbars** — the window resize grab band
  reached 8 px *inside* each edge, so hovering a right-edge scrollbar triggered the
  resize cursor instead of the scrollbar. The band now reaches mostly *outside* the
  window (8 px into the gap) and only 3 px inside, so scrollbars stay clickable.
- **Metis brand icon was hard to see in light mode** — the gradient launcher icon
  washed out against the pale light-mode bar. It now gets a soft `-gtk-icon-shadow`
  in light themes (omitted in dark, where a dark shadow would be invisible).
- **Maximized windows could still be dragged** — a maximized (or fullscreen)
  window's GTK headerbar still issued `xdg_toplevel` move requests, letting it be
  dragged around the screen. The compositor now ignores client move requests for
  maximized/fullscreen windows; unmaximize first to move.
- **Session "closed and auto-reopened" (the real root cause)** — `run-metis.sh`'s
  `binary_needs_rebuild()` probed the binaries with `"$bin" --help`. But
  `metis-compositor` ignores `--help` (its parser only knows `-c`/`--command`),
  so the probe **booted a full nested compositor window**. The user saw that
  probe window, closed it (assuming it was the session), and the script then
  proceeded to launch the *actual* session — looking exactly like a
  close→auto-reopen. It was intermittent because the probe only runs when the
  binaries are already up to date (otherwise `find -newer` returns first). The
  binary is no longer executed to test it; the ELF interpreter is checked on disk
  instead.
- **Backdrop blur bled into the bar's drop shadow** — the blur was applied to the
  bar's entire layer surface, which includes the transparent shadow-padding margin
  around the pill, producing an ugly blurred rectangle below/around the bar. The
  compositor now confines the blur to the visible pill (excluding the shadow pad),
  using bar-geometry constants shared via `metis-config`.

### Added

- **Background picker (Appearance)** — the Settings app's Appearance page now has
  a GNOME-style **Style** chooser (two large Light/Dark preview tiles that show
  the current background with a mock window) plus a **Background** section with a
  **Type** selector offering three modes:
  - **Picture** — a thumbnail grid of bundled and user-imported wallpapers plus
    an **Add Picture…** button that copies a chosen image into
    `~/.config/metis/wallpapers/`.
  - **Solid colour** — a single colour picker.
  - **Gradient** — start/end colour pickers and a direction selector
    (top↓bottom, bottom↑top, left→right, right→left, and both diagonals).

  Changes apply **live** via a new `ApplyBackground` compositor IPC command and
  persist to `~/.config/metis/wallpaper.json` so they survive restarts.
- **Live background switching (compositor)** — `CompositorCommand::ApplyBackground`
  makes the compositor re-read `wallpaper.json` and rebuild the background without
  a restart. Solid/gradient backgrounds are generated procedurally at the output
  resolution (and feed the bar's backdrop blur just like a wallpaper image does).
  `resolve_path` honors `wallpaper.json`, and `run-metis.sh` defers to it instead
  of forcing `METIS_WALLPAPER` to a default when a selection exists.
- **Settings app (`metis-settings`)** — a standalone GTK4 application with a
  sidebar/stack layout and four pages: **Appearance** (theme mode, accent/
  secondary/semantic color pickers, bar opacity + blur + blur radius),
  **Weather** (units, auto-detect/IP-geolocation toggles, Open-Meteo location
  search, saved-location management), **Network** (Wi-Fi scan/connect/forget +
  radio toggle, and per-NIC Ethernet IPv4 DHCP/static editors via `nmcli`), and
  **Calendars** (CalDAV + Microsoft 365 account management). Pages write the
  shared `~/.config/metis/*.json` files; the running shell picks changes up via
  its file watchers and new `reload-theme`/`reload-weather`/`reload-calendars`
  runtime commands. Launch it from the bar's launcher icon, the wired-only
  network popover's "Network Settings…" button, or `metis-cmd settings [page]`
  (`--page {appearance|weather|network|calendars}` preselects a page).
- **Shared `metis-config` crate** — all pure (serde + filesystem, no GTK)
  configuration types and the theme token model / stylesheet builder moved into a
  new workspace crate so both the shell and the settings app consume one source
  of truth. Added `save_bar_config`/`save_weather_config`/`save_theme_tokens`
  helpers and exported the per-file path getters.
- **Shared `metis-secrets` crate** — a thin `oo7` (freedesktop Secret Service)
  wrapper so both apps read/write the same keyring items for CalDAV passwords and
  Microsoft 365 refresh tokens.
- **Server-side window decorations** — the compositor now advertises
  `zxdg_decoration_manager_v1` and forces server-side mode on every toplevel, so
  GTK omits its client-side headerbar. Each tiled app window gets a compositor-
  drawn titlebar (with the window title rendered via `fontdue`), a thin border,
  and macOS-style close / minimize / maximize buttons. Clicking the buttons maps
  to the existing close/minimize/maximize actions and dragging the titlebar moves
  the window; the old undrawn 44 px grid "tile header" inset is replaced by this
  real chrome.

## [2026-06-20]

### Added

- **Live theme reload** — editing `~/.config/metis/themes/dark.json` or
  `light.json` now re-applies the active theme within ~250 ms (a GFileMonitor on
  the themes directory re-runs `init_theme()`), mirroring the existing live
  `bar.json` reload. No restart needed to tweak colors.
- **Freedesktop notifications** — Metis now runs an `org.freedesktop.Notifications`
  D-Bus daemon (zbus, on a background thread) so any desktop app's notifications
  surface in the in-bar notification popup. The bus name is acquired with the
  replace flag, so Metis takes over from a previously running daemon (dunst/mako).
  Urgency hints map to notification kinds (low→info, normal→alert, critical→error).
- **Themeable palette** — the stylesheet is now token-driven instead of hardcoded.
  Themes gain a secondary accent (`accent[1]`), a `semantic` status palette
  (`error`/`warning`/`success`/`info`/`payment`), and `text_on_accent`. ~27 fixed
  cyan literals, the notification kind colors, on-accent text, and the floating
  card shadows now derive from theme tokens, so accent/secondary/semantic/shadow
  changes flow through the whole bar UI. New token fields use serde defaults, so
  existing theme files keep working.
- **Bar transparency + backdrop blur** — `bar.json` `opacity` makes the bar
  see-through, and a new compositor Gaussian **backdrop blur** frosts the
  wallpaper behind the bar. Controlled by `blur` (on/off) and a new `blur_radius`
  (pixels, 1–64). The compositor samples the wallpaper under the bar through a
  custom GLES shader and re-reads the blur fields from `bar.json` live (~1s), so
  a future Settings app only needs to write the file. (Blur targets the bar's
  exclusive-zone rectangle; rounded-corner masking is a future refinement.)

## [2026-06-19]

### Added

- **Weather widget** — a condition icon + temperature in the bar opens a popover
  with current conditions (temp, label, daily high/low), a short hourly forecast
  strip, any additional saved locations, and an Open-Meteo attribution footer.
  Data is fetched (keyless) from Open-Meteo on a background thread and refreshed
  every 15 minutes (and on popover open). Location auto-detects via IP
  geolocation (city-level, keyless ipwho.is) with an offline system-timezone
  fallback; temperature unit auto-resolves (US-style regions → °F, otherwise °C).
  A `weather.json` config (unit, auto-detect, pinned locations) is read when
  present (incl. an `ip_geolocation` toggle) — the upcoming Settings app will
  manage it. Auto-detection prefers IP geolocation and falls back to the offline
  `zoneinfo` tables, caches the result, applies a 12s timeout to all HTTP so a
  stalled host can't hang the widget, logs failures, and retries every 30s after
  a failed fetch instead of waiting the full refresh window.
- **Wi-Fi popover** — the bar network icon is now interactive: clicking it opens a
  popover with a read-only Ethernet status row, a scrollable list of nearby Wi-Fi
  networks (signal strength, lock for secured, check for the active one), a Wi-Fi
  radio toggle, and a refresh/rescan button. Secured networks reveal an inline
  password entry; a spinner shows on the row while connecting. Backed by `nmcli`
  through a background command queue mirroring the audio widget.
- **Startup splash** — a centered overlay shows the Metis logo on a translucent card
  with a loading progress bar at session start. The bar crawls while the shell comes
  up, ramps to 100% once it's ready (with a minimum on-screen time and a hard timeout
  fallback), then fades out. The logo is embedded in the binary. Like the timer HUD,
  the layer surface is kept mapped and parked off-screen rather than destroyed, so
  closing it never disconnects the shell.
- **Startup chime** — an embedded `startup.mp3` plays once alongside the splash via
  GTK's media backend (best-effort; degrades silently if no media backend is present).
- **Launcher icon** — the Metis brand icon is pinned to the far-leading edge of the
  bar as a button (the seed of the upcoming app-menu launcher). The icon asset is
  embedded in the binary, so it renders regardless of the working directory.
- **Clock popover suite** — the bar clock now opens a tabbed popover with pill-style
  tabs: Calendar, World Clocks, Stopwatch, Timer, and Alarms.
  - **World Clocks** — inline searchable timezone picker (entry + list, no nested
    dropdown), up to three additional zones listed under the calendar.
  - **Stopwatch** — full-size view with lap times in a scrollable list.
  - **Timer** — a movable, always-on-top layer-shell HUD with pause/close controls
    that can be dragged anywhere on screen but never over the edge bar.
  - **Alarms** — a segmented sound selector (replaces the dropdown) for choosing the
    alarm tone.
- **Notification popup** improvements:
  - "Clear all" button (bottom-right) that clears every notification.
  - Identical notifications are grouped into one card with a count badge.
  - Per-kind icons (error, alarm/notification, success, information, payment),
    tinted to match the notification's accent color.
  - Clearing animates each card sliding out to the left.
  - Vertical scrollbar when the list overflows.
  - Demo feed for testing, enabled with `METIS_DEMO_NOTIFICATIONS=1`.
- **In-bar notification routing** — timer/alarm/reminder alerts are pushed into the
  bar's notification popup via a runtime notification store instead of spawning an
  external `notify-send` process.

### Changed

- Existing `bar.json` layouts are migrated to include the new `weather` widget
  ahead of the system/clock cluster.
- Dropped two redundant `nmcli` calls per network poll (the legacy
  `network_label`/`network_connected` snapshot fields the Wi-Fi popover replaced).
- Timer-finished alerts no longer tear down the HUD's layer surface; the HUD is parked
  off-screen instead, keeping the surface mapped for the session.
- Notification cards are wider with proper internal padding so text no longer sits at
  the card edge.
- **Wallpaper decoding** is now fully off the compositor's main thread and debounced:
  - `invalidate()` detaches the in-flight decode instead of joining it on the main
    loop, fixing a multi-second freeze on every window resize.
  - Resizes (maximize/restore) are debounced into a single decode and driven from the
    compositor heartbeat, so a re-decoded wallpaper appears promptly instead of after
    the next unrelated damage event (previously a 10–20s delay on maximize).
  - The full-resolution source image is cached in memory, so resizing only re-scales
    instead of re-reading and re-decoding the JPEG from disk.
- **Dev builds compile dependencies optimized** (`[profile.dev.package."*"] opt-level = 3`).
  The `image` crate was running unoptimized, making wallpaper decode/resize take
  several seconds; this brings it down to tens of milliseconds while keeping our own
  crates fast to compile.

### Fixed

- Silenced the benign `surface missing from known popups` ERROR from Smithay
  (GTK tears down short-lived entry sub-popups before their grab resolves) with a
  targeted log filter that keeps all other xdg-shell diagnostics.
- **Blank screen on startup from the chime** — the GStreamer media backend aborts
  (`gtk_gst_media_file_open: code should not be reached`) when a `MediaFile` is built
  from an input stream, which killed the shell before the bar appeared. The embedded
  chime is now materialized to a temp file and opened via `MediaFile::for_filename`.
- **Square edge-bar shadow** — the rounded pill's drop shadow was clipped square at
  the layer surface's rectangular edge. The surface now reserves a small padding
  around the pill (`BAR_SHADOW_PAD`) with the pill inset inside it, and the shadow was
  tightened so it renders fully and follows the bar's rounded corners.
- **Edge bar crash on timer completion** — removing the HUD's tooltips and keeping its
  layer surface mapped eliminates the `Broken pipe` Wayland protocol error that
  disconnected the shell when a timer ended.
- **Clock popover freeze / `surface missing from known popups`** — keyboard/text input
  in the popover now works via proper `xdg_popup` grabs; dropdowns no longer render
  behind the popover.
- **Notification popup layout overflow** — bounded the wrapping title/message labels
  (`max_width_chars`) and pinned the per-kind icon to a fixed size, fixing the GTK
  `gtk_widget_measure`/`size_allocate` overflow (huge/`INT_MIN` widths) introduced with
  the notification icons.
- **Wallpaper/briefing could not be re-enabled in `--session`** — `METIS_NO_WALLPAPER=`
  / `METIS_NO_BRIEFING=` (explicit empty) now correctly enable wallpaper and briefing.
  `run-metis.sh` no longer collapses an empty value back to `1` (`${VAR:-1}`), and the
  compositor/shell now treat the flags as disabled only when set to a *non-empty* value
  (previously any set value, including empty, counted as disabled).
