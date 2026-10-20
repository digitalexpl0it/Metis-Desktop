# Decision: Metis-native remote host vs GRD (+ viewer)

**Status:** accepted — portal-backed MetisNative landed (2026-10-03); multi-monitor
atlas (2026-10-20); **GRD remains default** until an explicit default-flip  
**Date:** 2026-07-27 (updated 2026-10-20)  
**Default product host:** `gnome-remote-desktop` (GRD) via `metis-remote`  
**Opt-in host:** `metis-rdp-host` (portal ScreenCast + FreeRDP) via `metis-remote native`  
**Related:** [UBUNTU_DEV.md](../UBUNTU_DEV.md), maturity **M5** in
`metis-os-workspace/TODO.md`

## Question

Should Metis replace GRD with a first-party remote **host** protocol (capture +
input + transport), or keep investing in GRD + the first-party **viewer**
(`metis-viewer`)?

## Building blocks

| Concern | Path |
|--------|----------------|
| Capture | `metis-portal` ScreenCast + PipeWire dmabuf; compositor image-capture |
| Input inject | Compositor EIS / `remote_input.rs`; Mutter RemoteDesktop D-Bus shim in portal |
| Session share UX | Settings → Remote access; pause-on-lock; LAN firewall; credentials via `grdctl` / native password file |
| Client | `metis-viewer` (FreeRDP → GRD or Metis native RDP) |
| Metis Remote (RUDP) | Compositor export + Quinn + `metis-encode` / `metis-decode` (primary low-latency path) |
| Opt-in RDP host | `metis-rdp-host` — ashpd ScreenCast client + FreeRDP shadow subsystem |

## Decision

**Keep RDP as a wire format** so Metis Viewer stays the client. Prefer a
**maintained FreeRDP server** path driven by Metis portal capture over inventing
a second custom TLS protocol for classic RDP clients.

- **Default:** GRD remains the supported session-sharing host under Remote access.
- **Opt-in MetisNative:** `RemoteBackend::MetisNative` starts `metis-rdp-host`,
  which opens an xdg ScreenCast session against `metis-portal` (multi-monitor
  when requested), composites outputs into a virtual-desktop atlas
  (`rdp-frames.shm` + `rdp-layout.json`), and feeds FreeRDP shadow on TCP 3389.
  Input maps to `InjectRemote*` in global logical coordinates. Pause/resume on
  lock reuses `metis-remote`. Session autostart dispatches MetisNative without
  requiring `grdctl`.
- **Metis Remote (RUDP)** is the primary low-latency first-party stream (separate
  Settings page); it is not gated on this decision.

## GA criteria (default flip — not done yet)

- [x] Credential UX matching GRD (stdin-set password, no argv secrets) — done:
  `metis-remote set-credentials` / `native set-credentials` write
  `$XDG_RUNTIME_DIR/metis/rdp-password` (0600, fail-closed runtime dir) and the
  username to `remote.json`; enable is refused without credentials; status
  `password_set` is derived; `metis-rdp-host` rejects empty-password auth.
- [x] Metis Remote (RUDP) multi-monitor + lock pause — protocol v3 layout /
  subscribe / per-output streams (2026-10-11).
- [x] MetisNative RDP multi-monitor — virtual-desktop atlas + FreeRDP
  `EnumMonitors` from `rdp-layout.json` (2026-10-20); Viewer `/multimon` /
  `/span` against the native host. Cap 4 outputs. Host audio/clipboard still
  absent (use GRD or RUDP when needed).
- [x] Packaging path without a heavier GNOME stack — deb/Arch/Nix already
  **Depends** FreeRDP shadow libs; GRD is Suggests/optdepend. MetisNative
  autostart no longer requires `grdctl`. **Manual QA checklist below** must
  pass on target distros before flipping `RemoteBackend` default.

Until the checklist is signed off and an explicit default-flip lands: **GRD
default**, MetisNative opt-in, RustDesk optional third-party.

## Manual QA checklist (before default flip)

1. Fresh Metis DRM session **without** `gnome-remote-desktop` installed; enable
   Metis native under Settings → Remote access; Viewer connects on 3389.
2. Dual-monitor: Viewer with `/multimon` or `/span` shows both outputs; absolute
   pointer hits the correct display.
3. Credentials survive logout (runtime password file); pause-on-lock / resume;
   LAN-only firewall with native backend only.
4. Confirm FreeRDP shadow runtime packages on Ubuntu 26.04+ / Debian 13
   (`libfreerdp*-shadow` family as packaged).
