# Upstream issues

Tracked dependency bugs Metis cannot fix in-tree.

## wayland-rs — server/sys ObjectData UAF

- **Issue:** [Smithay/wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)
- **Fix PR:** [Smithay/wayland-rs#996](https://github.com/Smithay/wayland-rs/pull/996)
  (defer `ObjectData::destroyed` until after `wl_event_loop_dispatch` on
  **server/sys**; Metis does **not** `[patch]` a fork — wait for crates.io /
  smithay bump)
- Local body copy: [`wayland-rs-server-objectdata-uaf.md`](wayland-rs-server-objectdata-uaf.md)
- Blocks default-on `wp_color_management_v1` (`METIS_COLOR_MGMT=1` remains opt-in)
- **Wave 3b (2026-07-27 / recheck 2026-10-03 / PR 2026-11-14):** issue open with
  proposed fix in #996 — do not flip Metis default-on until a wayland-backend
  release that includes that fix is in the smithay lockfile. Metis pins
  wayland-backend via smithay; 0.3.16/0.3.17 changelogs only fix **client/sys**.
  After upstream merges + ships: flip `color_protocol_enabled()` to default-on
  with `METIS_COLOR_MGMT=0` opt-out, expand Chromium/Firefox/mpv QA, update
  USER_GUIDE.
