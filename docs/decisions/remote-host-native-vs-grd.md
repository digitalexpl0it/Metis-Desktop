# Decision: Metis-native remote host vs GRD (+ viewer)

**Status:** accepted — portal-backed MetisNative is the **default** Remote access
host (default flip 2026-10-20); GRD remains an opt-in alternative  
**Date:** 2026-07-27 (updated 2026-10-20)  
**Default product host:** `metis-rdp-host` via `metis-remote` (`RemoteBackend::MetisNative`)  
**Opt-in host:** `gnome-remote-desktop` (GRD) via `metis-remote` / `grdctl`  
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
| Session share UX | Settings → Remote access; pause-on-lock; LAN firewall; credentials via native password file / `grdctl` |
| Client | `metis-viewer` (FreeRDP → Metis native RDP or GRD) |
| Metis Remote (RUDP) | Compositor export + Quinn + `metis-encode` / `metis-decode` (primary low-latency path) |
| Default RDP host | `metis-rdp-host` — ashpd ScreenCast client + FreeRDP shadow subsystem |

## Decision

**Keep RDP as a wire format** so Metis Viewer stays the client. Prefer a
**maintained FreeRDP server** path driven by Metis portal capture over inventing
a second custom TLS protocol for classic RDP clients.

- **Default:** `RemoteBackend::MetisNative` — `metis-rdp-host` opens an xdg
  ScreenCast session against `metis-portal` (multi-monitor when requested),
  composites outputs into a virtual-desktop atlas (`rdp-frames.shm` +
  `rdp-layout.json`), and feeds FreeRDP shadow on TCP 3389. Input maps to
  `InjectRemote*` in global logical coordinates. Pause/resume on lock reuses
  `metis-remote`. Session autostart does not require `grdctl`.
- **Opt-in GRD:** still available when the user selects GNOME RDP (audio /
  clipboard on the classic path).
- **Metis Remote (RUDP)** is the primary low-latency first-party stream (separate
  Settings page); it is not gated on this decision.

## GA criteria (default flip — done 2026-10-20)

- [x] Credential UX matching GRD (stdin-set password, no argv secrets) —
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
  autostart no longer requires `grdctl`.
- [x] Manual QA checklist (below) signed off for default flip.

**`RemoteBackend` default is `MetisNative`.** Existing `remote.json` files that
explicitly store `"backend": "gnome_rdp"` keep GRD.

## Manual QA checklist (signed off for default flip)

1. Fresh Metis DRM session **without** `gnome-remote-desktop` installed; enable
   sharing under Settings → Remote access; Viewer connects on 3389.
2. Dual-monitor: Viewer with `/multimon` or `/span` shows both outputs; absolute
   pointer hits the correct display.
3. Credentials survive logout (runtime password file); pause-on-lock / resume;
   LAN-only firewall with native backend only.
4. Confirm FreeRDP shadow runtime packages on Ubuntu 26.04+ / Debian 13
   (`libfreerdp*-shadow` family as packaged).
