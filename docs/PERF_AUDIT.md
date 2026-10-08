# Metis performance audit

Audit date: **2026-10-07** (refresh of 2026-08-02 / 2026-06-28 baselines).
Scope: compositor hot path, shell/bar overhead, portal capture, binary
footprint, and follow-ups.

**Host for this refresh:** x86_64, Ubuntu 26.04, hybrid Intel UHD + NVIDIA
RTX 2070 Max-Q (`METIS_BACKEND=drm`, `/dev/dri/card1`+`card2`), rustc 1.98.1,
12-thread i7-10750H. Session had Cursor IDE open (continuous client damage) —
not an empty-desktop idle.

---

## Executive summary

| Area | Rating | Notes |
|------|--------|-------|
| Idle CPU (compositor) | **OK** | Damage-gated; ~6% of one core over 30 s with Cursor redrawing; NVIDIA util 0% |
| Interactive latency | **Good–OK** | Pointer throttling, partial damage; `state.rs` still ~10.1k lines |
| DRM session | **OK** | Vblank + damage-gated flips; hybrid MultiRenderer Wave A/B/C (2026-10-03) |
| Shell / edge bar | **OK** | D-Bus dirty wakes (NM / BlueZ / UPower); ~200–800 ms adaptive; Pulse still `pactl` |
| Screen capture | **Good** | DRM: dmabuf → PipeWire; MemFd fallback; one-shot portal capture ~0.13 s |
| Gaming / Steam | **Improving** | Fullscreen fast path + scanout trace; `metis-gamingd`; PRIME smoke |
| Install footprint | **Grew** | Four-bin `release` **~54 MiB** (was ~40 MiB Jun); still LTO + strip + `panic=abort` |

Metis remains **past prototype** on compositor fundamentals (no busy loops,
deliberate throttles, async portal warm-up). ScreenCast dmabuf is landed;
hybrid MultiRenderer presents on secondary CRTCs. Remaining gaps: continuous
ScreenCast profiling under OBS/GRD on hybrid NVIDIA (MemFd fallbacks), and
empty-desktop idle once Electron clients are closed.

---

## Compositor — what is already optimized

### Damage-driven rendering

- Global `damaged` flag; winit/DRM skip GL when nothing changed.
- **16 ms heartbeat** caps nested dev at ~60 fps and avoids unbounded
  `RedrawRequested` loops (`winit.rs`).
- **`OutputDamageTracker`** for partial repaints.
- DRM: `drm_dispatch_damage()` only flips outputs with `pending && !queued`.

### Input & housekeeping throttles

- **Pointer motion** forwarded at most ~48 ms / 3 px unless grab or bar hit
  (`state.rs::should_forward_pointer_motion`) — prevents GTK hover storms.
- **`input.json`** reload throttled to ~1 s.
- **Wallpaper decode** debounced off the render path.
- **Portal stack** started on a detached thread (login no longer blocks 10+ s).
- **Dim-on-battery** samples `/sys/class/power_supply` every **2 s** from the
  16 ms housekeeping tick (`battery_dim::BatteryDimRuntime`) — not every frame.
- **Notification Center** 1 s date/world + 500 ms calendar poll sources are
  armed only while the panel is open (`notification_center` arm/disarm).
- **ScreenCast DmaBuf** success path skips CPU `mmap`→`Vec` copy; MemFd BGRx
  fallback uses row-wise copy + opaque alpha fill.

### Cheap bar blur

- Backdrop blur samples **wallpaper texture under the bar**, not a full
  framebuffer capture (`blur.rs`). Skipped while the bar is auto-hidden.

### Shared logic

- **`metis-grid`** — pure layout/reflow, no I/O in hot path.
- **`metis-protocol`** — JSON IPC for control plane only (windows, workspaces).

### Release profile hardening (current)

Workspace [`Cargo.toml`](../metis-os-workspace/Cargo.toml) `release` profile:

- `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`
- **`panic = "abort"`** and **`overflow-checks = true`** (Phase 15) — applied on
  default `release` and inherited by `release-small`.

---

## Hotspots & risks (priority order)

### P0 — ScreenCast / continuous capture — **landed**

**Status (2026-07-24+):** DRM sessions advertise dmabuf capture constraints.
The compositor renders ScreenCast frames into client GBM buffers (no
`copy_framebuffer` readback). `metis-portal` prefers linux-dmabuf + PipeWire
`SPA_DATA_DmaBuf`, with MemFd BGRx fallback for peers that reject DmaBuf
(e.g. some GRD paths). Nested winit remains SHM-only.

**2026-10-07 sample (this host):** `org.gnome.Mutter.ScreenCast` owned by
`metis-portal`; no OBS/GRD continuous stream active. One-shot
`metis-portal --capture-test $XDG_RUNTIME_DIR/metis/….png` completed in
**~0.13 s** (458 KiB PNG). `perf top`/`perf stat` blocked by
`kernel.perf_event_paranoid=4` — raise to ≤2 for DRM hot-path samples.

**Remaining gap:** multi-plane / non-linear modifiers may still take the MemFd
fallback; profile on hybrid NVIDIA stacks with OBS or gnome-remote-desktop.
DmaBuf-success no longer `to_vec`s the mmap (2026-10-03). Full multi-GPU
(`GpuManager`) validated 2026-07-26; MultiRenderer Wave A/B/C landed 2026-10-03.

**Recommendation:** validate OBS / gnome-remote-desktop under a live DRM
session; watch portal logs for `dmabuf` vs MemFd negotiation.

### P1 — Hybrid cross-GPU transfer / MultiRenderer — **Wave A/B/C 2026-10-03**

**Status:** Hybrid CRTCs present via `hybrid_multi::try_multirenderer_frame`
(`GpuManager::renderer` + `HybridOutputStack`): textured SSD, wallpaper
`ImportMem` cache, bar blur and HDR decode through `HybridTexShaderElement`
(MultiFrame→GlesFrame), and LUT/HDR encode via same-node Multi offscreen +
primary GLES post-pass then single-element present. `cross_gpu::try_transfer_frame`
is **fail-fallback only** (GBM dmabuf preferred; ExportMem/ImportMem still
available). `CrossGpuStats` counts dmabuf / ExportMem / MultiRenderer ok|fail.

### P1 — Fullscreen direct scanout (hybrid PRIME)

**Status:** Fullscreen fast path skips wallpaper, blur, night-light, and
compositor cursor when a client is true fullscreen. Per-surface dmabuf feedback
advertises scanout-capable formats. Trace: `scanout_promoted=true`
(`RUST_LOG=metis_compositor=trace`).

**Validation:** `metis-os-workspace/scripts/gaming-prime-smoke.sh` on hybrid hardware.

### P2 — `state.rs` monolith (~5.5k lines after 2026-10-07 split)

`MetisState` still owns IPC, spawn, and output reflow in `state.rs`. Desk/scroll/
workspaces live in `desk.rs`, snap/FS/max/min geometry in `window_geometry.rs`,
X11 map lifecycle in `xwayland.rs` (plus earlier `ipc_dispatch.rs`). Continue
incremental splits when touching remaining areas.

### P3 — Shell bar polling

**File:** `metis-shell/src/services/poll.rs`

Background thread (~200–800 ms adaptive) with D-Bus dirty wakes for
NetworkManager, BlueZ, and UPower; slow fallback ticks when signals are quiet.
UPower peripheral batteries are read over zbus (no `upower` CLI). Volume/mic
still use `pactl`. Occasional subprocess I/O remains for `nmcli`,
`bluetoothctl` inventory (and `info` / `solaar` only when %/charging is still
missing).

**Impact:** Low average CPU; not on compositor thread. **2026-10-07 DRM sample
(30 s):** edge bar ~0.8% of one core, desktop-widgets ~1.1%, portal ~0.1%.

### P4 — Default Cairo shell renderer

Session default: `METIS_SHELL_GSK_RENDERER=cairo` — **software GTK** for
reliability on fresh DRM sessions.

**Opt-in GPU GSK:** set `METIS_SHELL_GSK_RENDERER=gl` in the session environment
(or Flatpak override for GTK apps) when Mesa/NVIDIA drivers are stable. Games are
unaffected either way.

**Settings:** `metis-settings` defaults to `GSK_RENDERER=cairo` as well (2026-09-19)
to avoid multi-second scroll stalls on GTK 4.22 / hybrid NVIDIA. Override with
`METIS_SETTINGS_GSK_RENDERER=gl` (or `vulkan`) for experiments. The compositor
passes the same default when spawning Settings. Closing the Settings window
hard-exits the process (2026-09-19) so unique-instance + page poll timers cannot
leave a background `metis-settings`.

**Wallpaper apply:** Background changes soft-hold the previous GPU texture and
crossfade (~280 ms). Decoded full RGBA is cached under
`~/.cache/metis/wallpaper-rgba/` (warmed while Settings loads thumbs) so repeat
clicks skip multi‑MB PNG decode. Cover-crop crops to aspect before resize.
Prefer a **release** compositor for first-time applies of large system
wallpapers — debug builds decode far slower even with `profile.dev.package.image`
at `opt-level = 3`.

### P5 — Dependency feature bloat

| Crate | Issue | Action taken |
|-------|--------|--------------|
| `metis-shell` | `tokio` `full` | Trimmed to `rt`, `rt-multi-thread`, `macros`, `time`, `sync` |
| `metis-compositor` | Smithay `renderer_multi` | Needed for multi-GPU; keep |
| `metis-shell` | `rusqlite bundled` | Acceptable for calendar cache |

---

## Binary footprint

Measured **2026-10-07** (`cargo build --release` / `--profile release-small`,
`-p metis-compositor -p metis-shell -p metis-portal -p metis-settings`, x86_64).
Sizes are stripped on-disk (`stat` bytes → MiB = ÷1024²).

| Binary | Jun 2026 `release` | **Oct 2026 `release`** | **Oct 2026 `release-small`** |
|--------|--------------------|------------------------|------------------------------|
| metis-compositor | 11 MiB | **17.3 MiB** | 17.7 MiB |
| metis-shell | 15 MiB | **18.8 MiB** | **13.5 MiB** |
| metis-portal | 5.7 MiB | **6.1 MiB** | **3.8 MiB** |
| metis-settings | 8.6 MiB | **11.4 MiB** | **7.8 MiB** |
| **Four-bin total** | **~40 MiB** | **~53.6 MiB** (+34%) | **~42.9 MiB** |

Growth vs June is expected (RUDP/encode path in the compositor, MultiRenderer /
HDR, secrets, richer shell). Compositor is slightly **larger** under
`release-small` because that profile keeps `metis-compositor` at `opt-level=3`
while using fat LTO — size wins land on shell/portal/settings.

Optional session binaries (also `release`, not in the four-bin total):
`metis-secretsd` 4.2, `metis-polkit-agent` 4.2, `metis-gamingd` 2.2,
`metis-screenshot` 2.8, `metis-viewer` 7.9, `metis-rdp-host` 4.7,
`metis-remote` 2.0 MiB.

### Build profiles (`metis-os-workspace/Cargo.toml`)

| Profile | Use | Settings |
|---------|-----|----------|
| **`release`** (default) | `./run-metis.sh --release`, `--install-session` | `opt-level=3`, `lto=thin`, `codegen-units=1`, `strip=symbols`, **`panic=abort`**, overflow-checks |
| **`release-small`** | `./run-metis.sh --release-small --install-session` | `opt-level=s`, `lto=fat`, strip; **compositor stays `opt-level=3`** |

```bash
cd metis-os-workspace
cargo build --release -p metis-compositor -p metis-shell -p metis-portal -p metis-settings
cargo build --profile release-small -p metis-compositor -p metis-shell -p metis-portal -p metis-settings
ls -lh target/release/metis-{compositor,shell,portal,settings}
ls -lh target/release-small/metis-{compositor,shell,portal,settings}
```

Further size wins (optional):

- Split calendar/SNI into optional features on `metis-shell`
- System SQLite instead of `rusqlite/bundled` where distros allow

---

## DRM idle sample (2026-10-07)

Live Metis DRM session (`METIS_BACKEND=drm`), hybrid Intel+NVIDIA, Cursor IDE
open (client damage every frame from Electron).

| Process | ~% of one core (30 s `/proc` utime+stime) | RSS |
|---------|------------------------------------------|-----|
| metis-compositor | **~6.1%** | ~180 MiB |
| metis-shell (bar) | ~0.8% | ~222 MiB |
| metis-shell --desktop-widgets | ~1.1% | ~80 MiB |
| metis-portal | ~0.1% | — |
| NVIDIA GPU util | **0%** | 7 MiB used |

Empty-desktop / no-Electron idle was **not** re-measured this pass; expect
lower compositor % when nothing damages. Continuous ScreenCast + `perf top`
deferred until `perf_event_paranoid` allows and OBS/GRD is available.

---

## Measurement checklist

Run under a real Metis DRM session when validating changes:

```bash
# Idle CPU (prefer empty desktop; avoid Electron if measuring "near zero")
COMP=$(pgrep -f '/metis-compositor$' | head -1)
# two snapshots of utime+stime over 30s → % of one core = Δjiffies / CLK_TCK / wall * 100
top -p "$COMP"

# Needs kernel.perf_event_paranoid ≤ 2 (often root/sysctl)
perf top -p "$COMP"
perf stat -p "$COMP" -- sleep 10

ls -lh metis-os-workspace/target/{release,release-small}/metis-{compositor,shell,portal,settings}

# One-shot capture path (not continuous ScreenCast)
/usr/bin/time -f '%e sec' metis-portal --capture-test "$XDG_RUNTIME_DIR/metis/t.png"
```

**Hybrid NVIDIA MemFd checklist:** confirm ScreenCast/OBS negotiation logs
`dmabuf` vs MemFd; run `gaming-prime-smoke.sh`; note driver + Flatpak GL version
match.

---

## Recommended roadmap (perf)

1. **ScreenCast** dmabuf + PipeWire — landed; keep validating on DRM / hybrid
   (OBS/GRD continuous stream + `perf` when paranoid allows).
2. **Shell poll** — D-Bus dirty wakes landed 2026-10-07; residual Pulse/`pactl`
   and BlueZ inventory CLI.
3. **Split `state.rs`** when refactoring (maintainability; ~10.1k lines).
4. **Phase 5 colour** — default-on `wp_color_management_v1` blocked on upstream
   wayland-rs ObjectData UAF
   ([wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)).

See also [`TODO.md`](../metis-os-workspace/TODO.md) Engineering review backlog /
Phase 16.
