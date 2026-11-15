### Summary

Compositors that advertise `wp_color_management_v1` and use
`wayland-backend` with the **server system** backend (`use_system_lib`) can abort
with heap corruption shortly after a **Chromium/Ozone** client binds the global.
The fault is a use-after-free dropping an `ObjectData` `Arc` inside
`resource_dispatcher` — not in compositor request handlers.

### Symptom

```
malloc_consolidate(): unaligned fastbin chunk
```

Reproduces deterministically in ~4 seconds after Chromium sees the global
(nested Wayland session under gdb; also on DRM hardware).

### Trigger

In one dispatch batch:

1. Client destroys `wp_image_description_v1`
2. Client immediately reuses the freed protocol object id for
   `wp_image_description_info_v1` (typical Chromium `get_information` flow)

### Environment

| Item | Value |
|---|---|
| `wayland-backend` | 0.3.15 (locked via smithay); **0.3.16 does not fix this** |
| `wayland-server` | 0.31.13 (0.31.14 = core protocol XML only) |
| Backend | **server/sys** (`use_system_lib`) |
| Protocol | `wp_color_management_v1` v1 |
| Client | Chromium / Electron / Cursor (Ozone Wayland) |

### What we already ruled out

- Compositor panics / uninitialised `New<>` (handlers hardened; still crashes)
- Description-record leaks / compositor `unsafe` ICC memfd (not on the crashing
  trace — only safe parametric `get_information` → info events)
- Bumping `wayland-protocols` to 0.32.13 (identical crash)
- Expecting **0.3.16** to help: changelog/diff only touches **client/sys**
  (`ObjectData::destroyed` deadlock / udata race). **`server_impl` is unchanged.**

### Ask

Is there a known fix or workaround for **server-side** ObjectData lifetime when
an id is destroyed and reused in the same dispatch batch? Deferred drop / safer
id-reuse on the server path would unblock default-on colour management for
Smithay-based compositors.

Happy to supply a minimal compositor repro or ASAN traces if useful.

### Reporter context

Seen in the [Metis](https://github.com/digitalexpl0it/Metis) Smithay compositor
(Phase 5 colour pipeline). Product workaround: keep the global **opt-in** until a
server-side fix ships.

### Proposed fix (2026-11-14)

PR: [Smithay/wayland-rs#996](https://github.com/Smithay/wayland-rs/pull/996)

`dispatch_all_clients` now sets `PENDING_DESTRUCTORS` for the duration of
`wl_event_loop_dispatch` so `ObjectData::destroyed` / `ResourceUserData` free
runs only after the batch (the drain after dispatch already existed). Metis will
**not** depend on a personal fork; colour management stays `METIS_COLOR_MGMT=1`
opt-in until that fix is on crates.io and pulled in via smithay.
