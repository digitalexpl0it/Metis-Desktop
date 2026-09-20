# shellcheck shell=bash
# Metis build + session packages for Ubuntu 26.04 (resolute).
# Dev package is libgtk4-layer-shell-dev (not libgtk-4-layer-shell-dev).

METIS_LAYER_SHELL_FROM_SOURCE=0

METIS_APT_PACKAGES=(
  build-essential
  pkg-config
  libssl-dev
  libclang-dev
  curl
  git
  gettext
  meson
  ninja-build
  valac
  libgirepository1.0-dev
  gobject-introspection
  libgtk-4-dev
  libadwaita-1-dev
  libgraphene-1.0-dev
  libgtk4-layer-shell-dev
  libpulse-dev
  libudev-dev
  libinput-dev
  libseat-dev
  libgbm-dev
  libdrm-dev
  libegl1-mesa-dev
  libgles2-mesa-dev
  libdisplay-info-dev
  libpam0g-dev
  libpipewire-0.3-dev
  liblcms2-dev
  libwayland-dev
  wayland-protocols
  libeis-dev
  kitty
  gnome-keyring
  xdg-desktop-portal
  xdg-desktop-portal-gtk
  tesseract-ocr
  tesseract-ocr-eng
  ffmpeg
  nftables
  pkexec
  polkitd
)

METIS_APT_REMOTE=(
  gnome-remote-desktop
  freerdp3-wayland
)
