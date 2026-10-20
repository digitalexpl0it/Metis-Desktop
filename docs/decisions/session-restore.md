# Decision: Compositor session restore / crash recovery (M1)

**Status:** accepted  
**Date:** 2026-10-20  
**Related:** [`metis-os-workspace/TODO.md`](../../metis-os-workspace/TODO.md) (Desktop maturity **M1**),
[`assets/metis-session`](../../metis-os-workspace/assets/metis-session),
[`metis-config` session module](../../metis-os-workspace/metis-config/src/session.rs)

## Question

When `metis-compositor` dies (panic=`abort`, SIGKILL, GPU hang teardown), should
the user return to the greeter with a cold desk, or stay logged in and get a
best-effort desktop back?

## Decision

**Supervise the compositor from `metis-session` and relaunch apps from a
snapshot.** Wayland clients cannot reattach to a new display server; restore
means serialize → restart compositor/shell → spawn by desktop id → place.

### Supervisor

- [`metis-session`](../../metis-os-workspace/assets/metis-session) no longer
  `exec`s the compositor. It runs a restart loop for the DRM greeter path.
- **Clean logout** (`EndSession`, Ctrl+Alt+Backspace, power quit): compositor
  writes `$XDG_STATE_HOME/metis/session-clean-exit` and exits 0 → supervisor
  exits → greeter.
- **Unclean exit** (no clean marker): supervisor restarts the compositor with
  `METIS_SESSION_RESTORE=1`.
- **Crash-loop brake:** ≥3 unclean exits within 60 seconds → stop looping and
  return to the greeter (same philosophy as the RUDP host crash guard).

### Snapshot

- Path: `$XDG_STATE_HOME/metis/session-snapshot.json` (state, not config).
- Preference: `~/.config/metis/session.json` → `restore_on_crash` (default **on**).
- Debounced writes from live `WindowRecord` / `OutputDesk` (workspace, output,
  float vs tile, geometry, fullscreen / maximized / minimized).
- Metis chrome (`metis-shell`, desktop-widgets) is never snapshotted.

### Restore

On boot with `METIS_SESSION_RESTORE=1` and `restore_on_crash` and a valid
eligible snapshot: restore per-output active workspace / layout kind, stagger-
launch unique desktop ids (skip duplicate `startup.json` entries), match windows
as they map, place best-effort.

### Explicit non-goals

- Migrating live `wl_surface` clients
- Faithful XWayland / override-redirect / tray / splash restore
- Exclusive fullscreen / Gamescope / isolated gaming XWayland fidelity
- In-app document state
- Nested winit `--session` auto-restart
- systemd user units / XSMP

## Manual checklist

1. Kill `-9` the compositor under a greeter Metis session → stay logged in;
   apps relaunch when restore is enabled.
2. Clean logout → greeter; next login does **not** restore the previous desk.
3. Force three rapid compositor crashes → greeter (brake).

## Alternatives considered

| Option | Why not |
|--------|---------|
| systemd `Restart=` on compositor | Would fight libseat/DRM ownership; greeter session model is `metis-session` today |
| XSMP / ICE | Dead on Wayland; no client support worth depending on |
| Snapshot only, no supervisor | Still drops to greeter; fails the “without full logout” goal |
| Supervisor only, no relaunch | Empty desk after crash; weak trust win |
