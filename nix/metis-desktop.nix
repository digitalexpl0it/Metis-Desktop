{
  lib,
  rustPlatform,
  pkg-config,
  gtk4,
  gtk4-layer-shell,
  libadwaita,
  graphene,
  pango,
  cairo,
  glib,
  libpulseaudio,
  libinput,
  seatd,
  libgbm,
  libdrm,
  libglvnd,
  libdisplay-info,
  pam,
  pipewire,
  lcms2,
  wayland,
  wayland-protocols,
  libei,
  openssl,
  mesa,
  gettext,
  src,
}:

assert lib.assertMsg (lib.versionAtLeast rustPlatform.rust.rustc.version "1.95")
  "metis-desktop needs rustc >= 1.95 (workspace rust-version); use a newer nixpkgs";
assert lib.assertMsg (lib.versionAtLeast gtk4.version "4.18")
  "metis-desktop needs GTK >= 4.18 (gtk4-rs feature v4_18)";
assert lib.assertMsg (lib.versionAtLeast gtk4-layer-shell.version "1.0")
  "metis-desktop needs gtk4-layer-shell >= 1.0";

rustPlatform.buildRustPackage rec {
  pname = "metis-desktop";
  version = "0.1.0.12";

  inherit src;
  sourceRoot = "${src.name or "source"}/metis-os-workspace";

  cargoLock = {
    lockFile = ../metis-os-workspace/Cargo.lock;
    # smithay is pinned via git in Cargo.lock — refresh after bumping the rev:
    #   nix build .#metis-desktop 2>&1 | grep -A2 'got:'
    outputHashes = {
      "smithay-0.7.0" = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    };
  };

  nativeBuildInputs = [
    pkg-config
    gettext
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    gtk4
    gtk4-layer-shell
    libadwaita
    graphene
    pango
    cairo
    glib
    libpulseaudio
    libinput
    seatd
    libgbm
    libdrm
    libglvnd
    libdisplay-info
    pam
    pipewire
    lcms2
    wayland
    wayland-protocols
    libei
    openssl
    mesa
  ];

  cargoBuildFlags = [
    "-p" "metis-compositor"
    "-p" "metis-shell"
    "-p" "metis-settings"
    "-p" "metis-portal"
    "-p" "metis-remote"
    "-p" "metis-viewer"
    "-p" "metis-gaming"
  ];

  doCheck = false;

  postInstall = ''
    assets="$NIX_BUILD_TOP/${sourceRoot}/assets"
    polkit="$NIX_BUILD_TOP/${sourceRoot}/packaging/polkit/org.metis.policy"

    mkdir -p \
      "$out/share/wayland-sessions" \
      "$out/share/xdg-desktop-portal/portals" \
      "$out/share/applications" \
      "$out/share/icons/hicolor/48x48/apps" \
      "$out/share/icons/hicolor/256x256/apps" \
      "$out/share/metis/wallpapers" \
      "$out/share/metis/locale" \
      "$out/share/polkit-1/actions" \
      "$out/etc/pam.d"

    install -Dm755 "$assets/metis-session" "$out/bin/metis-session"
    install -Dm644 "$assets/metis.desktop" "$out/share/wayland-sessions/metis.desktop"
    install -Dm644 "$assets/metis.portal" "$out/share/xdg-desktop-portal/portals/metis.portal"
    install -Dm644 "$assets/metis-portals.conf" "$out/share/xdg-desktop-portal/metis-portals.conf"
    install -Dm644 "$assets/metis-settings.desktop" "$out/share/applications/metis-settings.desktop"
    install -Dm644 "$assets/metis-viewer.desktop" "$out/share/applications/metis-viewer.desktop"
    install -Dm644 "$assets/metis-settings-48.png" "$out/share/icons/hicolor/48x48/apps/metis-settings.png"
    install -Dm644 "$assets/metis-settings.png" "$out/share/icons/hicolor/256x256/apps/metis-settings.png"
    install -Dm644 "$assets/metis-viewer-48.png" "$out/share/icons/hicolor/48x48/apps/metis-viewer.png"
    install -Dm644 "$assets/metis-viewer.png" "$out/share/icons/hicolor/256x256/apps/metis-viewer.png"
    install -Dm644 "$assets/pam-metis" "$out/etc/pam.d/metis"
    if [[ -f "$polkit" ]]; then
      install -Dm644 "$polkit" "$out/share/polkit-1/actions/org.metis.policy"
    fi

    substituteInPlace "$out/share/wayland-sessions/metis.desktop" \
      --replace-fail "Exec=metis-session" "Exec=$out/bin/metis-session" \
      --replace-fail "TryExec=metis-session" "TryExec=$out/bin/metis-session"

    if [[ -d "$assets/locale" ]]; then
      cp -a "$assets/locale/." "$out/share/metis/locale/"
    fi
    shopt -s nullglob
    for wp in "$assets"/wallpapers/*.{png,jpg,jpeg,webp,PNG,JPG,JPEG,WEBP}; do
      [[ -f "$wp" ]] || continue
      install -Dm644 "$wp" "$out/share/metis/wallpapers/$(basename "$wp")"
    done
  '';

  meta = with lib; {
    description = "Metis Wayland desktop environment";
    homepage = "https://github.com/digitalexpl0it/Metis";
    license = licenses.mit;
    platforms = platforms.linux;
    mainProgram = "metis-session";
  };
}
