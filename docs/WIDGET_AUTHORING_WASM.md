# Authoring Metis WASM widgets (API 2)

End users install packs; they do not write WASM. This guide is for pack authors.

## Pack layout

```text
~/.local/share/metis/widgets/com.example.mywidget/
  manifest.json    # "api": 2, "wasm": { "module": "module.wasm" }
  widget.json      # layout; give patchable nodes an "id"
  module.wasm      # guest binary
```

Copy the example pack
`metis-os-workspace/assets/widgets/com.metis.example.wasmstatus/` as a starting
point.

## Guest contract

See [`WIDGET_WASM_ABI.md`](WIDGET_WASM_ABI.md). Minimum exports: `memory`,
`metis_init`, `metis_tick`. Optional: `metis_on_event`.

Wire buttons with:

```json
"on_click": { "action": "wasm_event", "event": "open" }
```

## Building `module.wasm`

### WAT (tiny examples)

```bash
# with the `wat` crate / wabt wat2wasm
wat2wasm module.wat -o module.wasm
```

### Rust → wasm32-unknown-unknown

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
# no_std friendly crates only; no WASI
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/mywidget.wasm module.wasm
```

Import only the `metis` host functions from the ABI doc. Do **not** enable WASI.

## Limits

Fuel, memory, and module size caps are enforced by Metis (see
[`decisions/widget-wasm.md`](decisions/widget-wasm.md)). Keep ticks tiny.

## Kill-switch

`METIS_WIDGET_WASM=0` disables instantiate; layout still renders without guest
updates.
