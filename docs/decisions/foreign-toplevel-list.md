# Decision: Foreign toplevel list (M4)

**Status:** accepted — Phase A landed (2026-10-20)  
**Related:** [`metis-os-workspace/TODO.md`](../../metis-os-workspace/TODO.md)
(Desktop maturity **M4**), [`metis-protocol`](../../metis-os-workspace/metis-protocol/)

## Question

Should third-party docks and task tools see Metis windows over a standard
Wayland protocol, and should that replace Metis’s private shell IPC?

## Decision

**Export `ext-foreign-toplevel-list-v1` as a read-only projection of the
compositor window registry. Keep Metis shell/taskbar on `metis-protocol` IPC.**

### Phase A (shipped)

- Advertise `ext_foreign_toplevel_list_v1` from `metis-compositor`.
- On window ready / open: create a handle with stable identifier `metis-{id}`,
  title, and app_id; update on metadata changes; `closed` on destroy.
- Minimized windows stay listed (same as IPC).
- Bind filter: only same-euid peers (credentials via Wayland client peercred).
- Metis shell continues to use `ListWindows` / `WindowOpened` / …
  `ActivateWindow` / `CloseWindow` — it does **not** bind the foreign list.

### Phase B (deferred)

- Activate / minimize / close / state for third-party docks typically needs
  `zwlr_foreign_toplevel_management_v1` (or a future ext-* management /
  state protocol). Smithay at the pinned rev does not ship a management
  helper; implementing it is a separate maturity follow-up.
- Toplevel image-capture source / portal window picker via foreign handles
  is also out of Phase A.

## Consequences

- List-only clients can enumerate session windows without talking Metis IPC.
- Docks that require activate-on-click still need Phase B or stay on Metis IPC.
- No second window registry: foreign handles live on `WindowRecord` and mirror
  the same lifecycle as shell events.
