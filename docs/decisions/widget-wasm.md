# Decision: Sandboxed widget WASM (Extension API v2 / M7)

**Status:** accepted  
**Date:** 2026-10-20  
**Related:** [`metis-os-workspace/TODO.md`](../../metis-os-workspace/TODO.md) (Desktop maturity **M7**),
[`docs/WIDGET_PACK_SCHEMA.md`](../WIDGET_PACK_SCHEMA.md),
[`metis-widget-wasm`](../../metis-os-workspace/metis-widget-wasm/),
plan [`.cursor/plans/m7_widget_wasm_a1b2c3d4.plan.md`](../../.cursor/plans/m7_widget_wasm_a1b2c3d4.plan.md)

## Question

How should third-party desktop widgets run real logic without loading arbitrary
`.so`s or Electron, while end users keep the same install/Settings UX?

## Decision

**In-process WASM (Wasmtime) behind a deny-by-default host ABI (`api: 2`).**
Rhai/Lua are declined. Declarative JSON layout (v1) and out-of-process helpers
remain; WASM patches UI state and invokes the same hardened host actions.

### Audiences

- **End users** — install a pack folder/archive under
  `~/.local/share/metis/widgets/<id>/`, place and configure in Settings. They do
  not write WASM.
- **Authors** — ship `module.wasm` (+ `manifest.json` / `widget.json`); use the
  documented ABI / starter template.
- **Metis** — sandbox, fuel/memory/timeouts, fail-closed discovery, kill-switch.

### Engine and caps

| Cap | v2.0 default |
|-----|----------------|
| Engine | Wasmtime (fuel + epoch interruption) |
| Module size | 512 KiB |
| Memory | 16 MiB (256 wasm pages) |
| Fuel per tick | 5_000_000 |
| Wall-clock per call | 50 ms (epoch + timeout) |
| Max live instances | 32 |

Kill-switch: `METIS_WIDGET_WASM=0` disables loading/running modules (packs with
`wasm` are skipped at discovery or run JSON-only if layout is valid without
requiring wasm — discovery requires the module file when declared).

### Host ABI (deny-by-default)

Guest may import only the `metis` module functions documented in
[`docs/WIDGET_WASM_ABI.md`](../WIDGET_WASM_ABI.md): read settings / host /
helper JSON into a guest buffer; `set_text` / `set_visible` by node `id`;
`request_helper_refresh`; `open_uri` / `launch` / `copy_text` (same validators
as Extension API v1).

Guest exports: `metis_init`, `metis_tick`, `metis_on_event` (optional),
`memory`.

### Explicit non-goals (v2.0)

- WASI filesystem, sockets, or clocks that escape the host ABI
- Guest HTTP / notifications / writing settings
- Free GTK widget creation from WASM
- Multi-threaded guests
- Replacing out-of-process helpers for privileged local I/O
- End-user “write WASM” UI / widget studio

## Consequences

- New workspace crate `metis-widget-wasm`; shell desktop-widgets host owns
  instances tied to card lifetime.
- `WIDGET_EXT_API` accepts `1` and `2`; schema gains optional `wasm` + node `id`
  + `wasm_event` action.
- Supply-chain: Wasmtime must pass `cargo deny` (licenses / duplicates).
