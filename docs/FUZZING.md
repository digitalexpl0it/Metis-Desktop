# Fuzzing Metis parsers (Phase 23 §E)

Hostile-input targets for network-facing and config parsers. **Do not** gate PR
CI on long fuzz runs — run locally or on a nightly job.

## Setup

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
cd metis-os-workspace/fuzz
```

## Targets

| Binary | Input | Code under test |
|--------|-------|-----------------|
| `rudp_ctrl` | framed control bytes | `metis_protocol::try_decode_rudp_frame` |
| `widget_pack` | UTF-8 JSON | `WidgetExtNode` + `validate_widget_layout` |
| `icon_bytes` | raw bytes | `metis_remote::validate_icon_bytes` (avatar upload) |

```bash
cargo +nightly fuzz run rudp_ctrl
cargo +nightly fuzz run widget_pack
cargo +nightly fuzz run icon_bytes
```

Corpus directories are created under `fuzz/corpus/<target>/`. The fuzz package is
**not** a member of the main Cargo workspace so normal `cargo test` /
`cargo clippy --workspace` stay free of `libfuzzer-sys` / nightly requirements.
