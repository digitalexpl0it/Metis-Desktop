# Metis widget pack schema (API 1–2)

Author guide for declarative desktop widget extensions (Phase 14 §E) and optional
sandboxed WASM (M7 / API 2). Runtime validation is **fail-closed** at discovery
(Phase 18 C): invalid packs are skipped with a warning (and a toast at
widgets-host startup). They never appear in Settings → Desktop widgets → Add,
and they must not crash the host.

See also: [User Guide — Desktop widgets](USER_GUIDE.md) (Extensions subsection),
[`WIDGET_WASM_ABI.md`](WIDGET_WASM_ABI.md),
[`decisions/widget-wasm.md`](decisions/widget-wasm.md).

## Install layout

```text
~/.local/share/metis/widgets/<id>/
  manifest.json
  widget.json
  helper                 # optional basename only
  module.wasm            # optional; when manifest.wasm is set (api: 2)
```

Also searched (lower priority than the user dir):

- `/usr/local/share/metis/widgets/<id>/`
- `/usr/share/metis/widgets/<id>/`

Rules:

- Folder name **must** equal `manifest.id` (reverse-DNS, lowercase, contains `.`).
- Both JSON files are required.
- No `.so` or Electron. Scripts are not loaded; API 2 may ship a **WASM** module
  only (see ABI doc). Optional helper binary remains out-of-process.
- Unknown JSON fields are **rejected** (`deny_unknown_fields`).

Example packs in-tree: `metis-os-workspace/assets/widgets/com.metis.example.quicklinks/`,
`…/com.metis.example.helperstatus/`, `…/com.metis.example.wasmstatus/`.

## `manifest.json`

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | Extension id; must match folder name |
| `name` | string | yes | Display name in Settings |
| `version` | string | no | Default `1.0.0` |
| `api` | number | no | `1` (JSON only) or `2` (JSON + optional WASM) |
| `default_size` | `[w, h]` | no | Default `[320, 200]` |
| `min_size` | `[w, h]` | no | Optional minimum |
| `settings_schema` | array | no | See below |
| `helper` | object | no | See Helper |
| `wasm` | object | no | API 2 only — see WASM |

### Settings schema entries

| Field | Type | Notes |
|-------|------|-------|
| `key` | string | Non-empty; no `/`, `\`, or `..` |
| `type` | `"string"` \| `"bool"` \| `"number"` | Default `string` |
| `label` | string | UI label |
| `default` | JSON value | Typed default |

## `widget.json`

Root is one layout **node**. Node types (`type` tag):

| `type` | Fields |
|--------|--------|
| `column` / `row` / `list` | optional `id`, `spacing` (int, default 0), `children` |
| `label` | optional `id`, `text`, `style` (`body` \| `title` \| `muted`) |
| `icon` | optional `id`, `name` (theme icon, no paths), `pixel_size` (default 24) |
| `button` | optional `id`, `label`, `on_click` (action object) |
| `separator` | optional `id` |

Stable `id` values (ASCII alnum / `_` / `-` / `.`, ≤ 64 chars) are required for
WASM `set_text` / `set_visible` patching.

### Caps

| Cap | Limit |
|-----|-------|
| `widget.json` size | 256 KiB |
| WASM module size | 512 KiB |
| Nesting depth | 12 |
| Total nodes | 256 |
| Label / URI strings | 2048 chars |
| `copy_text` payload | 4096 chars |

### Actions (`on_click`)

Tagged with `action`:

| `action` | Fields | Rules |
|----------|--------|-------|
| `open_uri` | `uri` | **http/https only**; no `file:`, userinfo, or custom schemes |
| `launch` | `id` and/or `exec` | Desktop id **or** single PATH basename; no argv/paths; interpreters denylisted |
| `copy_text` | `text` | Length-capped |
| `wasm_event` | `event` | API 2 + wasm; calls guest `metis_on_event` with this name |

Placeholders:

- Labels / copy text: `{settings.<key>}`, `{host.time}`, `{host.date}`, `{host.weather.*}`, `{host.sys.*}`, `{helper.<key>}`
- **Not** allowed in `open_uri` / `launch` targets (actions are not settings-interpolated)

## Helper (optional)

In `manifest.json`:

```json
"helper": {
  "exec": "helper",
  "poll_seconds": 5
}
```

- `exec` is a **basename** under the pack root (no `/`, no `..`).
- Discovery requires the file to exist and stay under the pack after `canonicalize`.
- Host runs argv-only, cleared env (`PATH`/`HOME`/`LANG` only), timeout 3s, stdout ≤ 8 KiB.
- Stdout must be a JSON object; string/number/bool values become `{helper.<key>}` in labels.

## WASM (optional, `api: 2`)

```json
"api": 2,
"wasm": {
  "module": "module.wasm"
}
```

- `module` is a **basename** ending in `.wasm` under the pack root.
- Discovery requires the file (≤ 512 KiB) under the pack after `canonicalize`.
- Runtime: Wasmtime, deny-by-default host ABI — see [`WIDGET_WASM_ABI.md`](WIDGET_WASM_ABI.md).
- Kill-switch: `METIS_WIDGET_WASM=0` skips instantiate (layout still shows).

## Discovery behavior

1. Parse `manifest.json` (strict).
2. Validate id / folder / `api` (1 or 2) / settings keys.
3. Parse + validate `widget.json` (strict + action hardening).
4. If `helper` is set, resolve the helper binary under the pack root.
5. If `wasm` is set, resolve the module under the pack root (`api` must be 2).
6. On any failure: **skip** the pack (`tracing::warn`), continue scanning.
7. Widgets host startup: one toast if any packs were skipped.

Configured instances that reference a missing/skipped pack show an in-card error UI; the host keeps running.

## Versioning

`api: 1` — JSON + helper only.  
`api: 2` — adds optional WASM + `wasm_event` + node `id`.  
Packs with an unsupported `api` are rejected at discovery.
