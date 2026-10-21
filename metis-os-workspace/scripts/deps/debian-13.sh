# shellcheck shell=bash
# Metis build + session packages for Debian 13 (trixie).
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
  libasound2-dev
  libopus-dev
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
  libavcodec-dev
  freerdp3-dev
  libwinpr3-dev
  libavformat-dev
  libavutil-dev
  libswscale-dev
  libswresample-dev
  libva-dev
  nftables
  pkexec
  polkitd
  # Runtime hosts Metis shells out to / talks to over D-Bus (not build-only).
  network-manager
  network-manager-openvpn
  upower
  bluez
  bluetooth
  packagekit-tools
  pipewire
  pipewire-pulse
  wireplumber
)

METIS_APT_REMOTE=(
  gnome-remote-desktop
  freerdp3-wayland
)
