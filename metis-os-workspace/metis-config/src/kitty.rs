//! Default kitty terminal config seeded for Metis sessions.
//!
//! Kitty ships opaque by default. Metis writes a small `kitty.conf` on first run
//! (onboarding finish/skip, or shell start) so the default terminal shows the
//! wallpaper through a translucent background. Existing user configs are never
//! overwritten.

use std::path::PathBuf;

/// Contents written when `~/.config/kitty/kitty.conf` is missing.
pub const KITTY_DEFAULT_CONF: &str = "\
# Written by Metis on first run. Delete or edit freely — Metis will not
# overwrite an existing kitty.conf.
background_opacity 0.75
dynamic_background_opacity yes
";

/// `~/.config/kitty/kitty.conf` (respects `$XDG_CONFIG_HOME` when set).
pub fn kitty_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("kitty").join("kitty.conf");
    }
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config/kitty/kitty.conf"))
        .unwrap_or_else(|_| PathBuf::from(".config/kitty/kitty.conf"))
}

/// Create Metis kitty defaults if the config file does not already exist.
///
/// Returns `Ok(true)` when a new file was written, `Ok(false)` when skipped
/// because a config is already present.
pub fn ensure_kitty_defaults() -> std::io::Result<bool> {
    let path = kitty_config_path();
    if path.is_file() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, KITTY_DEFAULT_CONF)?;
    tracing::info!(path = %path.display(), "seeded Metis kitty defaults");
    Ok(true)
}
