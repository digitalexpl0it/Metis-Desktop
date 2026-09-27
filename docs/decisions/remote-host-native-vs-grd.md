# Decision: Metis-native remote host vs GRD (+ viewer)

**Status:** accepted direction (experimental host) — 2026-09-27  
**Date:** 2026-07-27 (updated 2026-09-27)  
**Default product host:** `gnome-remote-desktop` (GRD) via `metis-remote`  
**Experimental host:** FreeRDP shadow (`freerdp-shadow-cli`) via `metis-remote native`  
**Related:** [UBUNTU_DEV.md](../UBUNTU_DEV.md), Phase 7 in `metis-os-workspace/TODO.md`

## Question

Should Metis replace GRD with a first-party remote **host** protocol (capture + input + transport), or keep investing in GRD + the first-party **viewer** (`metis-viewer`)?

## Building blocks already in-tree

| Concern | Existing path |
|--------|----------------|
| Capture | `metis-portal` ScreenCast + PipeWire dmabuf; compositor image-capture |
| Input inject | Compositor EIS / `remote_input.rs`; Mutter RemoteDesktop D-Bus shim in portal |
| Session share UX | Settings → Remote access; pause-on-lock; LAN firewall; credentials via `grdctl` |
| Client | `metis-viewer` (FreeRDP → GRD or Metis native RDP) |
| Stretch peer | Optional RustDesk backend (`metis-remote rustdesk …`) — not a Metis protocol |
| Experimental host | `metis-remote native` → FreeRDP shadow server (RDP wire; X11-oriented package) |

## Decision (2026-09-27)

**Keep RDP as the wire format** so Metis Viewer stays the client. Prefer a
**maintained FreeRDP server/shadow** path over inventing a custom TLS protocol.

- **Default:** GRD remains the supported session-sharing host.
- **Experimental:** `RemoteBackend::MetisNative` starts `freerdp-shadow-cli`
  (package `freerdp-shadow-x11`) on TCP 3389, reuses LAN firewall + pause/resume
  on lock. Documented limitations: packaged shadow is X11-oriented and may not
  capture a pure Wayland Metis session until FreeRDP gains a portal/Wayland
  shadow backend Metis can drive.
- **Viewer polish** (saved-host grid, FreeRDP sessions on a dedicated workspace)
  ships independently of host maturity.

## Prior recommendation (2026-07)

Invest in GRD + viewer; defer native host. That still holds for **product default**.
Urgent Wave 4c now lands an **experimental** FreeRDP-based native host rather than
a greenfield protocol.

## Criteria to promote MetisNative to default

- Reliable capture of the Metis Wayland session (portal or compositor-direct).
- Credential UX matching GRD (stdin-set password, no argv secrets).
- Multi-monitor + lock pause parity with GRD QA.
- Packaging on Ubuntu 26.04+ / Debian 13 without pulling a full GNOME stack beyond
  what Metis already Suggests.

Until then: **GRD default**, MetisNative opt-in experimental, RustDesk optional third-party.
