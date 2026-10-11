# Decision: Metis-native remote host vs GRD (+ viewer)

**Status:** accepted — portal-backed MetisNative landed (2026-10-03); GRD remains default  
**Date:** 2026-07-27 (updated 2026-10-03)  
**Default product host:** `gnome-remote-desktop` (GRD) via `metis-remote`  
**Opt-in host:** `metis-rdp-host` (portal ScreenCast + FreeRDP) via `metis-remote native`  
**Related:** [UBUNTU_DEV.md](../UBUNTU_DEV.md), Phase 7 in `metis-os-workspace/TODO.md`

## Question

Should Metis replace GRD with a first-party remote **host** protocol (capture + input + transport), or keep investing in GRD + the first-party **viewer** (`metis-viewer`)?

## Building blocks

| Concern | Path |
|--------|----------------|
| Capture | `metis-portal` ScreenCast + PipeWire dmabuf; compositor image-capture |
| Input inject | Compositor EIS / `remote_input.rs`; Mutter RemoteDesktop D-Bus shim in portal |
| Session share UX | Settings → Remote access; pause-on-lock; LAN firewall; credentials via `grdctl` |
| Client | `metis-viewer` (FreeRDP → GRD or Metis native RDP) |
| Metis Remote (RUDP) | Compositor export + Quinn + `metis-encode` / `metis-decode` (primary low-latency path) |
| Opt-in RDP host | `metis-rdp-host` — ashpd ScreenCast client + FreeRDP shadow subsystem |

## Decision

**Keep RDP as a wire format** so Metis Viewer stays the client. Prefer a
**maintained FreeRDP server** path driven by Metis portal capture over inventing
a second custom TLS protocol for classic RDP clients.

- **Default:** GRD remains the supported session-sharing host under Remote access.
- **Opt-in MetisNative:** `RemoteBackend::MetisNative` starts `metis-rdp-host`,
  which opens an xdg ScreenCast session against `metis-portal`, consumes the
  PipeWire node, and feeds frames into a FreeRDP shadow server on TCP 3389.
  Input maps to `InjectRemote*`. Pause/resume on lock reuses `metis-remote`.
- **Metis Remote (RUDP)** is the primary low-latency first-party stream (separate
  Settings page); it is not gated on this decision.

## GA criteria (default flip — not done yet)

- [x] Credential UX matching GRD (stdin-set password, no argv secrets) — done:
  `metis-remote set-credentials` / `native set-credentials` write
  `$XDG_RUNTIME_DIR/metis/rdp-password` (0600, fail-closed runtime dir) and the
  username to `remote.json`; enable is refused without credentials; status
  `password_set` is derived; `metis-rdp-host` rejects empty-password auth.
- [x] Metis Remote (RUDP) multi-monitor + lock pause — protocol v3 layout /
  subscribe / per-output streams (2026-10-11). MetisNative RDP host still
  single-surface vs GRD multi-monitor QA.
- Packaging on Ubuntu 26.04+ / Debian 13 without a heavier GNOME stack.

Until those land: **GRD default**, MetisNative opt-in, RustDesk optional third-party.
