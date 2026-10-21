# Decision: Foreign toplevel list + management (M4)

**Status:** accepted — Phase A landed (2026-10-20); Phase B landed (2026-10-21)  
**Related:** [`metis-os-workspace/TODO.md`](../../metis-os-workspace/TODO.md)
(Desktop maturity **M4**), [`metis-protocol`](../../metis-os-workspace/metis-protocol/),
Phase 22 §B

## Question

Should third-party docks and task tools see Metis windows over a standard
Wayland protocol, and should that replace Metis’s private shell IPC?

## Decision

**Export `ext-foreign-toplevel-list-v1` as a read-only projection of the
compositor window registry, plus `zwlr_foreign_toplevel_management_v1` for
activate / minimize / maximize / fullscreen / close. Keep Metis shell/taskbar
on `metis-protocol` IPC.**

A future `ext-*` state/request protocol may supersede wlr management; ship wlr
now for dock compatibility (waybar / sfwbar-class clients). Do not wait on
unfinished ext-*.

### Phase A (shipped)

- Advertise `ext_foreign_toplevel_list_v1` from `metis-compositor`.
- On window ready / open: create a handle with stable identifier `metis-{id}`,
  title, and app_id; update on metadata changes; `closed` on destroy.
- Minimized windows stay listed (same as IPC).
- Bind filter: only same-euid peers (credentials via Wayland client peercred).
- Metis shell continues to use `ListWindows` / `WindowOpened` / …
  `ActivateWindow` / `CloseWindow` — it does **not** bind the foreign list.

### Phase B (shipped — Phase 22)

- Advertise `zwlr_foreign_toplevel_management_v1` with the same same-euid filter.
- Per-window management handle alongside the list handle on `WindowRecord`.
- Push state: activated, minimized, maximized, fullscreen (and output
  enter/leave when available).
- Map requests to existing compositor ops (`activate_window_by_id`,
  `minimize_by_id`, `close_window`, `set_maximized`, `set_fullscreen`).
- `set_rectangle` (icon geometry) is a documented no-op.
- Hand-rolled `wayland-server` Dispatch from `wayland-protocols-wlr` XML
  (Smithay pin has no management helper).

### Still deferred

- Toplevel image-capture / portal window picker via foreign handles.
- Replacing Metis IPC taskbar with foreign management.
- Cross-UID globals.

## Consequences

- List + management clients can enumerate and control session windows without
  talking Metis IPC.
- Metis dock remains on private IPC (single code path, richer events).
- No second window registry: foreign handles live on `WindowRecord` and mirror
  the same lifecycle as shell events.
