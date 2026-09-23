//! Lock-screen appearance persisted to `~/.config/metis/lock.json`.
//!
//! The compositor renders the lock screen itself (Option A), so this only
//! describes how it should look: which background to show (reuse the desktop
//! wallpaper, a dedicated picture, a solid colour, or a gradient), whether to
//! blur/dim it, and whether to show the clock. The settings app writes this
//! file; the compositor reads it on startup and re-reads it live via the
//! `ReloadLock` IPC command.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::wallpaper::GradientDirection;
use crate::{config_dir, ensure_config_dirs};

/// Where the lock screen sources its background from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockBackgroundSource {
    /// Reuse whatever the desktop wallpaper currently is (default).
    #[default]
    Wallpaper,
    /// A dedicated picture chosen just for the lock screen.
    Picture,
    /// A solid colour.
    Solid,
    /// A two-stop linear gradient.
    Gradient,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockConfig {
    /// Which background style the lock screen shows.
    #[serde(default)]
    pub background: LockBackgroundSource,
    /// Absolute path to the dedicated lock picture (used when
    /// `background == Picture`).
    #[serde(default)]
    pub picture_path: Option<String>,
    /// Solid background colour (`#rrggbb`), used when `background == Solid`.
    #[serde(default = "default_lock_solid")]
    pub color: String,
    /// Gradient start colour (`#rrggbb`), used when `background == Gradient`.
    #[serde(default = "default_lock_grad_start")]
    pub gradient_start: String,
    /// Gradient end colour (`#rrggbb`), used when `background == Gradient`.
    #[serde(default = "default_lock_grad_end")]
    pub gradient_end: String,
    /// Direction the gradient sweeps.
    #[serde(default)]
    pub gradient_direction: GradientDirection,
    /// Blur the background (gives the classic frosted lock look).
    #[serde(default = "default_true")]
    pub blur: bool,
    /// Darken the background by this percentage (0-100) so the clock and prompt
    /// stay legible over bright wallpapers.
    #[serde(default = "default_dim")]
    pub dim_percent: u8,
    /// Show the clock on the lock screen.
    #[serde(default = "default_true")]
    pub show_clock: bool,
    /// Use a 24-hour clock (`true`, default) or a 12-hour clock with AM/PM.
    #[serde(default = "default_true")]
    pub clock_24h: bool,
    /// Automatically lock when the screen blanks on idle.
    #[serde(default)]
    pub lock_on_idle_blank: bool,
}

fn default_lock_solid() -> String {
    "#0b0d12".to_string()
}

fn default_lock_grad_start() -> String {
    "#0f172a".to_string()
}

fn default_lock_grad_end() -> String {
    "#1e293b".to_string()
}

fn default_true() -> bool {
    true
}

fn default_dim() -> u8 {
    35
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            background: LockBackgroundSource::default(),
            picture_path: None,
            color: default_lock_solid(),
            gradient_start: default_lock_grad_start(),
            gradient_end: default_lock_grad_end(),
            gradient_direction: GradientDirection::default(),
            blur: true,
            dim_percent: default_dim(),
            show_clock: true,
            clock_24h: true,
            lock_on_idle_blank: false,
        }
    }
}

pub fn lock_config_path() -> PathBuf {
    config_dir().join("lock.json")
}

pub fn load_lock_config() -> LockConfig {
    let path = lock_config_path();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    LockConfig::default()
}

pub fn save_lock_config(cfg: &LockConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(lock_config_path(), json)
}
