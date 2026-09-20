# Metis performance audit

Audit date: **2026-08-02** (refresh of 2026-06-28 baseline). Scope: compositor
hot path, shell/bar overhead, portal capture, binary footprint, and follow-ups.

---

## Executive summary

| Area | Rating | Notes |
|------|--------|-------|
| Idle CPU (compositor) | **Good** | Damage-gated render; ~60 fps cap; near-zero work when idle |
| Interactive latency | **Good–OK** | Pointer throttling, partial damage; `state.rs` still large (~9k lines) |
| DRM session | **OK** | Vblank + damage-gated flips; hybrid PRIME validated |
| Shell / edge bar | **OK** | Background poll + D-Bus where available; ~400 ms–6 s fallbacks |
| Screen capture | **Good** | DRM: dmabuf → PipeWire; MemFd fallback; nested winit SHM-only |
| Gaming / Steam | **Improving** | Fullscreen fast path + scanout trace; `metis-gamingd`; PRIME smoke |
| Install footprint | **Improved** | Release: LTO + strip + **`panic = "abort"`** + overflow-checks |

Metis is **past prototype** on compositor fundamentals (no busy loops, deliberate
throttles, async portal warm-up). ScreenCast dmabuf is landed; remaining gaps are
hybrid NVIDIA MemFd fallbacks and shell poll wakeups.

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

**Remaining gap:** multi-plane / non-linear modifiers may still take the MemFd
fallback; profile on hybrid NVIDIA stacks. Full multi-GPU (`GpuManager`) is
**validated 2026-07-26** on hybrid iGPU+dGPU; explicit `MultiRenderer` transfer
remains deferred.

**Recommendation:** validate OBS / gnome-remote-desktop under a live DRM
session; watch portal logs for `dmabuf` vs MemFd negotiation.

### P1 — Fullscreen direct scanout (hybrid PRIME)

**Status:** Fullscreen fast path skips wallpaper, blur, night-light, and
compositor cursor when a client is true fullscreen. Per-surface dmabuf feedback
advertises scanout-capable formats. Trace: `scanout_promoted=true`
(`RUST_LOG=metis_compositor=trace`).

**Validation:** `metis-os-workspace/scripts/gaming-prime-smoke.sh` on hybrid hardware.

### P2 — `state.rs` monolith (~9k lines)

Single `MetisState` holds windowing, workspaces, scroll layout, IPC, wallpaper,
decorations, grabs, etc. Phase 16 extracted `ipc_dispatch.rs` for capability
gating; continue incremental splits when touching areas.

### P3 — Shell bar polling

**File:** `metis-shell/src/services/poll.rs`

Background thread (~400 ms) with D-Bus-driven updates where available
(NetworkManager / UPower / Pulse) and a slow fallback tick for sources without
signals. Occasional subprocess I/O remains for Bluetooth / Solaar.

**Impact:** Low average CPU; not on compositor thread.

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

Measured on 2026-06-28 (x86_64, after profile + tokio trim):

| Binary | Stock release (before) | **`release`** (LTO + strip) | **`release-small`** |
|--------|------------------------|----------------------------|---------------------|
| metis-compositor | 16 MB | **11 MB** (−31%) | 9.2 MB |
| metis-shell | 21 MB | **15 MB** (−29%) | **9.5 MB** (−55%) |
| metis-portal | 9.7 MB | **5.7 MB** (−41%) | **3.2 MB** (−67%) |
| metis-settings | 14 MB | **8.6 MB** (−39%) | **5.0 MB** (−64%) |
| **Total** | **~61 MB** | **~40 MB** (−34%) | **~27 MB** (−56%) |

### Build profiles (`metis-os-workspace/Cargo.toml`)

| Profile | Use | Settings |
|---------|-----|----------|
| **`release`** (default) | `./run-metis.sh --release`, `--install-session` | `opt-level=3`, `lto=thin`, `codegen-units=1`, `strip=symbols`, **`panic=abort`**, overflow-checks |
| **`release-small`** | `./run-metis.sh --release-small --install-session` | `opt-level=s`, `lto=fat`, strip; **compositor stays `opt-level=3`** |

```bash
cd metis-os-workspace/metis-shell
./run-metis.sh --build --release
./run-metis.sh --build --release-small
ls -lh ../target/release/metis-compositor ../target/release-small/metis-compositor
```

Further size wins (optional):

- Split calendar/SNI into optional features on `metis-shell`
- System SQLite instead of `rusqlite/bundled` where distros allow

---

## Measurement checklist

Run under a real Metis DRM session when validating changes:

```bash
top -p $(pgrep metis-compositor)
perf top -p $(pgrep metis-compositor)
ls -lh metis-os-workspace/target/{release,release-small}/metis-*
/usr/bin/time -f '%e sec' metis-portal --capture-test /tmp/t.png
```

**Hybrid NVIDIA MemFd checklist:** confirm ScreenCast/OBS negotiation logs
`dmabuf` vs MemFd; run `gaming-prime-smoke.sh`; note driver + Flatpak GL version
match.

---

## Recommended roadmap (perf)

1. **ScreenCast** dmabuf + PipeWire — landed; keep validating on DRM / hybrid.
2. **Shell poll** — prefer D-Bus signals; keep slow fallback (Phase 16).
3. **Split `state.rs`** when refactoring (maintainability).
4. **Phase 5 colour** — default-on `wp_color_management_v1` blocked on upstream
   wayland-rs ObjectData UAF
   ([wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)).

See also [`TODO.md`](../metis-os-workspace/TODO.md) Phase 16.
