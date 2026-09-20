# Packaging Metis

Metis ships as **per-suite** packages. The Debian package name is always
**`metis-desktop`** — never bare `metis` (Ubuntu universe already has an unrelated
graph-partitioning package named `metis` at 5.1.x; `apt upgrade` would replace
the desktop).

| Audience | Path |
|----------|------|
| Ubuntu / Debian users | `.deb` from [GitHub Releases](https://github.com/digitalexpl0it/Metis/releases) |
| Arch users | [`packaging/arch/PKGBUILD`](../metis-os-workspace/packaging/arch/PKGBUILD) (`makepkg -si`) |
| NixOS | [flake + module](../nix/README.md) |
| From source (any supported distro) | [`./install.sh`](../install.sh) at the repo root |

## Install from a `.deb`

Pick the artifact that matches your OS:

| File suffix | Target |
|-------------|--------|
| `…amd64.ubuntu24.04.deb` | Ubuntu 24.04 (noble) |
| `…amd64.ubuntu26.04.deb` | Ubuntu 26.04 |
| `…amd64.debian13.deb` | Debian 13 (trixie) |

```bash
# Ubuntu 26.04: enable universe if apt reports libseat1 / kitty / layer-shell missing
sudo apt update
sudo apt install ./metis-desktop_VERSION-1_amd64.ubuntu24.04.deb
dpkg -l metis-desktop
command -v metis-remote metis-settings metis-session
```

Log out and pick **Metis** at the greeter.

**Ubuntu 26.04 note:** packages from `0.1.0.15a` and earlier declared
`Depends: libdisplay-info1`, which resolute no longer ships (it has
`libdisplay-info3`). Use a newer `…ubuntu26.04.deb` rebuilt after that fix.

### Upgrading

Use terminal `apt`, not App Center / GDebi. Log out of Metis first.

Older releases used `Package: metis` (colliding name). Installing `metis-desktop`
`Breaks`/`Replaces` those `0.1.0.x` packages only — not Ubuntu’s `metis` 5.x math
package.

If `apt upgrade` already swapped you onto Ubuntu’s math `metis`:

```bash
sudo apt remove metis
sudo apt install ./metis-desktop_*.ubuntu24.04.deb   # or matching suite
```

Do **not** mix a `/usr` package install with `./install.sh` / `--install-session`
(`/usr/local`) without cleaning one of them first.

### Dependency policy (`.deb`)

| Field | Role |
|-------|------|
| **Depends** | Required to start a Metis session (GTK4, seat, DRM, PipeWire, kitty, …) |
| **Bundled** | `libgtk4-layer-shell` on Ubuntu 24.04 only; 26.04 / Debian 13 use `libgtk4-layer-shell0` |
| **Recommends** | keyring, portals helpers, volumes, **nftables** (apt installs by default) |
| **Suggests** | GRD, FreeRDP, GameMode, Flatpak, BT, printers, biometrics |

## From source: `./install.sh`

```bash
git clone https://github.com/digitalexpl0it/Metis.git
cd Metis
./install.sh                  # confirm packages, then --install-session
./install.sh --yes            # noninteractive
./install.sh --deps-only      # packages + Rust + layer-shell only
./install.sh --with-remote    # also GRD + FreeRDP packages
```

Supported: **Ubuntu 24.04 / 26.04**, **Debian 13**, **Arch Linux**. Dep lists live in
[`metis-os-workspace/scripts/deps/`](../metis-os-workspace/scripts/deps/).
Build deps include **`libclang-dev`** (bindgen for PipeWire/`libspa-sys`). On
Ubuntu 24.04, gtk4-layer-shell is fetched via
[`scripts/fetch-gtk4-layer-shell.sh`](../metis-os-workspace/scripts/fetch-gtk4-layer-shell.sh)
(tarball with retries) when building from source.

This installs to **`/usr/local`** via `run-metis.sh --install-session`. Prefer the
`.deb` for production machines.

## Arch (`makepkg`)

```bash
cd metis-os-workspace/packaging/arch
# From a release tag (default):
makepkg -si
# Or from a local clone of this repo:
METIS_LOCAL_SRC=/path/to/Metis makepkg -si
```

Publishing to the AUR is manual (out of tree). Keep `pkgver` in sync with tags.

## NixOS

See [`nix/README.md`](../nix/README.md). Enable `programs.metis` and set
`programs.metis.package` to the flake package.

## Build a `.deb` locally

```bash
cd metis-os-workspace
VERSION=0.1.0.12 DISTRO_SUITE=ubuntu24.04 ./scripts/package-deb.sh
VERSION=0.1.0.12 DISTRO_SUITE=ubuntu26.04 ./scripts/package-deb.sh
VERSION=0.1.0.12 DISTRO_SUITE=debian13 ./scripts/package-deb.sh
# → dist/metis-desktop_${VERSION}-1_amd64.${DISTRO_SUITE}.deb
```

| Variable | Default | Meaning |
|----------|---------|---------|
| `VERSION` | *(required)* | Package / GitHub version (e.g. `0.1.0.13`) |
| `DISTRO_SUITE` | `ubuntu24.04` | `ubuntu24.04` \| `ubuntu26.04` \| `debian13` |
| `UBUNTU_SUITE` | — | Legacy (`24.04` → `ubuntu24.04`) |
| `BUNDLE_GTK4_LAYER_SHELL` | suite default | `1` bundles the .so into the deb |
| `SKIP_BUILD` | `0` | `1` = stage existing `target/release` only |

### Crate versions vs GitHub tags

Cargo requires SemVer (`MAJOR.MINOR.PATCH`). GitHub tags use a four-part product
scheme (`v0.1.0.13`). All crates inherit one workspace version:

```toml
# metis-os-workspace/Cargo.toml
[workspace.package]
version = "0.1.13"
```

| GitHub / `.deb` `VERSION` | Cargo workspace version |
|---------------------------|-------------------------|
| `0.1.0.13` | `0.1.13` |
| `0.1.0.13a` | `0.1.13-a` |
| `0.1.0` | `0.1.0` |

Before a release build, sync (also done automatically by `package-deb.sh`):

```bash
./scripts/sync-version.sh 0.1.0.14   # or: VERSION=0.1.0.14 ./scripts/sync-version.sh
```

Shared FHS staging: [`scripts/stage-fhs.sh`](../metis-os-workspace/scripts/stage-fhs.sh)
(used by deb, Arch PKGBUILD, and aligned with Nix `postInstall`).

## GitHub Actions

Workflow: [`.github/workflows/release-deb.yml`](../.github/workflows/release-deb.yml)

Tag `v*` builds **ubuntu24.04**, **ubuntu26.04**, and **debian13** artifacts and
attaches them to the GitHub Release.

Nix: [`.github/workflows/nix-flake.yml`](../.github/workflows/nix-flake.yml).

## What the package installs

| Path | Role |
|------|------|
| `/usr/bin/metis-{compositor,shell,settings,portal,remote,viewer,gamingd,polkit-agent}` | Binaries |
| `/usr/bin/metis-session` | Greeter session launcher |
| `/usr/share/wayland-sessions/metis.desktop` | Session entry |
| `/usr/share/xdg-desktop-portal/…` | Portal backend |
| `/usr/share/applications/metis-*.desktop` + icons | Settings / Viewer |
| `/usr/share/metis/{wallpapers,widgets,locale}` | Assets / i18n |
| `/usr/share/polkit-1/actions/org.metis.policy` | Polkit actions for `metis-remote` |
| `/usr/bin/metis-polkit-agent` / `/usr/libexec/metis-polkit-agent` | Metis PolicyKit auth agent (session password dialogs) |
| `/etc/pam.d/metis` | Lock-screen PAM |

## Explicit non-goals (for now)

- Fedora/COPR RPM
- Flatpak/AppImage for the compositor
- One `.deb` for all Debian-family suites
- Official Arch `[extra]` or nixpkgs upstream
