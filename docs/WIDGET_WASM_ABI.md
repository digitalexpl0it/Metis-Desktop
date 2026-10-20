# Metis widget WASM ABI (Extension API v2)

Guest modules are WebAssembly MVP (wasm32). No WASI. Imports live under the
`metis` module; the guest must export linear `memory` and `metis_init` /
`metis_tick`. See [`decisions/widget-wasm.md`](decisions/widget-wasm.md).

## Guest exports

| Export | Signature | Notes |
|--------|-----------|-------|
| `memory` | memory | Required |
| `metis_init` | `() -> i32` | `0` = ok; called once after instantiate |
| `metis_tick` | `(reason: i32) -> i32` | `0` timer, `1` helper refresh, `2` button/event follow-up |
| `metis_on_event` | `(ptr: i32, len: i32) -> i32` | Optional; UTF-8 event name from `wasm_event` actions |

Return `0` on success; non-zero is logged and ignored (host stays up).

## Host imports (`metis`)

String arguments are UTF-8 in guest memory (`ptr`, `len`). JSON readers take a
guest buffer (`buf`, `cap`) and return bytes written, or `-1` on error / overflow.

| Import | Signature | Behaviour |
|--------|-----------|-----------|
| `get_settings_json` | `(buf, cap) -> i32` | Instance settings object |
| `get_host_json` | `(buf, cap) -> i32` | `{time,date,weather_*,sys_*}` |
| `get_helper_json` | `(buf, cap) -> i32` | Last helper snapshot object (empty `{}` if none) |
| `set_text` | `(id_ptr, id_len, text_ptr, text_len) -> i32` | Patch label/button by node `id` |
| `set_visible` | `(id_ptr, id_len, visible: i32) -> i32` | Show/hide widget with `id` |
| `request_helper_refresh` | `() -> i32` | Schedule helper re-run |
| `open_uri` | `(ptr, len) -> i32` | Same rules as v1 `open_uri` |
| `launch` | `(id_ptr, id_len, exec_ptr, exec_len) -> i32` | Same rules as v1 `launch` |
| `copy_text` | `(ptr, len) -> i32` | Same rules as v1 `copy_text` |

## Pack layout

```text
~/.local/share/metis/widgets/<id>/
  manifest.json    # "api": 2, optional "wasm": { "module": "module.wasm" }
  widget.json      # layout; patchable nodes need "id"
  module.wasm      # when wasm.module is set
  helper           # optional, unchanged from v1
```

## Kill-switch

`METIS_WIDGET_WASM=0` — do not instantiate modules.
