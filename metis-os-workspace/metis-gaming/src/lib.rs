//! Metis Gaming Platform 2.0 — Flatpak optimizer, health checks, and session hooks.

pub mod detect;
pub mod flatpak;
pub mod health;
pub mod power;
pub mod session;

pub use detect::{
    detect_steam, flatpak_has_app, gamemode_installed, i386_vulkan_likely_missing,
    mesa_vulkan_amd64_missing, nvidia_gpu_present, nvidia_reboot_required, SteamInstall,
};
pub use flatpak::{ensure_steam_launcher, flatpak_steam_launch_command, optimize_flatpak_gaming};
pub use health::install_nvidia_drivers;
pub use session::GamingDaemon;
