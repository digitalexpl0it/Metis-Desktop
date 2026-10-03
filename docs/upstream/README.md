# Upstream issues

Tracked dependency bugs Metis cannot fix in-tree.

## wayland-rs — server/sys ObjectData UAF

- **Issue:** [Smithay/wayland-rs#949](https://github.com/Smithay/wayland-rs/issues/949)
- Local body copy: [`wayland-rs-server-objectdata-uaf.md`](wayland-rs-server-objectdata-uaf.md)
- Blocks default-on `wp_color_management_v1` (`METIS_COLOR_MGMT=1` remains opt-in)
- **Wave 3b (2026-07-27 / recheck 2026-10-03):** still open — do not flip Metis
  default-on until a wayland-backend release fixes **server/sys**. Metis pins
  wayland-backend **0.3.17**; 0.3.16/0.3.17 changelogs only fix **client/sys**.
  Issue still open as of 2026-09-17. After upstream lands: flip
  `color_protocol_enabled()` to default-on with `METIS_COLOR_MGMT=0` opt-out,
  expand Chromium/Firefox/mpv QA, update USER_GUIDE.
