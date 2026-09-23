# Distro dependency profiles for `./install.sh`

| File | Target |
|------|--------|
| `ubuntu-26.04.sh` | Ubuntu 26.04+ (resolute and newer) — `libgtk4-layer-shell-dev` + `libclang-dev` |
| `debian-13.sh` | Debian 13 (trixie)+ — `libgtk4-layer-shell-dev` + `libclang-dev` |
| `arch.sh` | Arch Linux (`pacman`) |

NixOS is served by the flake module (`nix/`), not by these profiles.

Support floor (Debian 13 is the oldest target): GTK >= 4.18, GLib >= 2.84,
gtk4-layer-shell >= 1.0, PipeWire >= 1.4, Rust >= 1.95 (via rustup; distro
`rustc` packages are too old). Ubuntu 24.04 (GTK 4.14) is no longer supported.

Each file sets package arrays and `METIS_LAYER_SHELL_FROM_SOURCE`.
All profiles install Tesseract and English language data because Extract Text is
a standard Metis Screenshot feature rather than an optional integration.
