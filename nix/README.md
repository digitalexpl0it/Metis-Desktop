# Metis on NixOS

Arch and NixOS users are often the earliest adopters of experimental
Rust/Wayland compositors. Metis ships a **flake** and a **NixOS module** so you
can try or install without inventing packaging.

## Quick try (no system rebuild)

```bash
nix build github:digitalexpl0it/Metis#metis-desktop
nix develop github:digitalexpl0it/Metis   # rustc / cargo / pkg-config shell
```

Prefer a nested session first (safe — does not take over your login):

```bash
# after a local clone + nix develop / cargo build, or from the store path:
cd metis-os-workspace/metis-shell
./run-metis.sh --session
```

See the [User Guide](../docs/USER_GUIDE.md#1-launching-metis) for nested vs DRM.

## NixOS module (greeter session)

In your system flake:

```nix
{
  inputs.metis.url = "github:digitalexpl0it/Metis";
  # …
  outputs = { self, nixpkgs, metis, … }: {
    nixosConfigurations.hostname = nixpkgs.lib.nixosSystem {
      modules = [
        metis.nixosModules.metis
        {
          programs.metis.enable = true;
          programs.metis.package = metis.packages.${pkgs.system}.metis-desktop;
        }
      ];
    };
  };
}
```

Then `nixos-rebuild switch`, log out, and pick **Metis** at the greeter.

## Packaging notes

- Package definition: [`metis-desktop.nix`](metis-desktop.nix)
- Module: [`module.nix`](module.nix)
- Flake entry: [`../flake.nix`](../flake.nix)
- CI: [`.github/workflows/nix-flake.yml`](../.github/workflows/nix-flake.yml)

First-time package build needs a valid smithay `cargoLock.outputHashes` entry in
`metis-desktop.nix` (run `nix build .#metis-desktop` and paste the `got:` hash
when the lock drifts).

Broader packaging matrix (deb, Arch PKGBUILD, AUR): [`docs/PACKAGING.md`](../docs/PACKAGING.md).
