# shellcheck shell=bash
# Metis build + session packages for Arch Linux.

METIS_LAYER_SHELL_FROM_SOURCE=0

METIS_PACMAN_PACKAGES=(
  base-devel
  rust
  pkgconf
  openssl
  clang
  curl
  git
  gettext
  meson
  ninja
  vala
  gobject-introspection
  gtk4
  libadwaita
  gtk4-layer-shell
  libpulse
  libinput
  seatd
  mesa
  libdrm
  libdisplay-info
  pam
  pipewire
  lcms2
  wayland
  wayland-protocols
  libei
  kitty
  gnome-keyring
  xdg-desktop-portal
  xdg-desktop-portal-gtk
  tesseract
  tesseract-data-eng
  ffmpeg
  nftables
  polkit
)

METIS_PACMAN_REMOTE=(
  gnome-remote-desktop
  freerdp
)
