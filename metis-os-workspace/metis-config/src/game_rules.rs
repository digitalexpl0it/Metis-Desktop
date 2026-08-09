//! Window rules for gaming, persisted to `~/.config/metis/game-rules.json`.
//!
//! The tiling grid is great for productivity but wrong for games: a fresh
//! game/launcher window that lands in a grid tile gets clamped to the tile size
//! and fights the reflow engine. These rules let Metis recognise games (and
//! game launchers/stores) by their `app_id` / X11 class or title and force them
//! to float (escape the grid), and optionally to go true-fullscreen once mapped.
//!
//! A curated built-in default list covers Steam and common launchers/games; the
//! file is user-extensible so anyone can add their own titles without a rebuild.
//! Matching is case-insensitive substring so a single entry (e.g. `"steam"`)
//! catches every related surface (`Steam`, `steamwebhelper`,
//! `com.valvesoftware.Steam`, …).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{config_dir, ensure_config_dirs};

/// A single window-matching rule. A window matches when ANY of its `app_id` or
/// `title` substrings is contained (case-insensitively) in the window's
/// respective property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowRule {
    /// Human-readable label (documentation only; ignored by matching).
    #[serde(default)]
    pub name: String,
    /// Lowercase substrings matched against the window's `app_id` / X11 class.
    #[serde(default)]
    pub app_id_contains: Vec<String>,
    /// Lowercase substrings matched against the window's title.
    #[serde(default)]
    pub title_contains: Vec<String>,
    /// Float the window (never tile it into the grid).
    #[serde(default = "default_true")]
    pub float: bool,
    /// Request true fullscreen once the window is mapped and ready.
    #[serde(default)]
    pub fullscreen: bool,
}

/// The outcome of consulting the rule set for a given window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowRuleOutcome {
    pub float: bool,
    pub fullscreen: bool,
}

impl WindowRuleOutcome {
    pub fn matched(&self) -> bool {
        self.float || self.fullscreen
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameRulesConfig {
    /// When false, the rules are ignored entirely (grid placement as usual).
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_rules")]
    pub rules: Vec<WindowRule>,
}

fn default_true() -> bool {
    true
}

/// Curated defaults. `float` keeps games out of the grid; `fullscreen` is left
/// off by default so we never surprise a user by covering their screen — they
/// can opt in per-rule. Substrings are lowercase; matching lowercases inputs.
fn default_rules() -> Vec<WindowRule> {
    let float_only = |name: &str, app_ids: &[&str], titles: &[&str]| WindowRule {
        name: name.to_string(),
        app_id_contains: app_ids.iter().map(|s| s.to_string()).collect(),
        title_contains: titles.iter().map(|s| s.to_string()).collect(),
        float: true,
        fullscreen: false,
    };
    vec![
        float_only("Steam", &["steam", "com.valvesoftware.steam"], &[]),
        float_only(
            "Steam Big Picture",
            &["steamwebhelper", "gamepadui"],
            &["big picture", "steam big picture"],
        ),
        float_only("Lutris", &["lutris", "net.lutris.lutris"], &[]),
        float_only(
            "Heroic Games Launcher",
            &["heroic", "com.heroicgameslauncher.hgl"],
            &[],
        ),
        float_only("Bottles", &["bottles", "com.usebottles.bottles"], &[]),
        float_only("gamescope", &["gamescope"], &[]),
        float_only("Minecraft", &["minecraft", "lwjgl"], &["minecraft"]),
        // The Hytale *launcher* (and any other Hytale/Hypixel surface) stays a
        // normal floating window.
        float_only(
            "Hytale (launcher & surfaces)",
            &["hytale", "hypixel"],
            &["hytale"],
        ),
        // The Hytale *game client* reports `HytaleClient` and is configured for
        // fullscreen in-game; force true-fullscreen once it maps so it covers the
        // output on the first frame instead of coming up as a large float that the
        // user has to F11. Scoped to the game client's app_id so the launcher is
        // unaffected. (Both share the broad `hytale` match above, which only adds
        // `float`.)
        WindowRule {
            name: "Hytale game client (fullscreen)".to_string(),
            app_id_contains: vec!["hytaleclient".to_string()],
            title_contains: Vec::new(),
            float: true,
            fullscreen: true,
        },
        // Proton / Wine Steam titles: true-fullscreen on map so borderless-windowed
        // clients cannot land as a float inset under the edge bar (clipped right/bottom).
        WindowRule {
            name: "Proton / Wine games (fullscreen)".to_string(),
            app_id_contains: vec![
                "steam_app_".to_string(),
                "proton".to_string(),
                ".exe".to_string(),
            ],
            title_contains: Vec::new(),
            float: true,
            fullscreen: true,
        },
    ]
}

impl Default for GameRulesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: default_rules(),
        }
    }
}

impl GameRulesConfig {
    /// Consult the rules for a window's `app_id`/X11 class and title. Returns the
    /// combined outcome (float / fullscreen) across all matching rules.
    pub fn evaluate(&self, app_id: Option<&str>, title: Option<&str>) -> WindowRuleOutcome {
        let mut outcome = WindowRuleOutcome::default();
        if !self.enabled {
            return outcome;
        }
        let app_id = app_id.map(|s| s.to_ascii_lowercase());
        let title = title.map(|s| s.to_ascii_lowercase());
        for rule in &self.rules {
            let app_hit = app_id.as_deref().is_some_and(|a| {
                rule.app_id_contains
                    .iter()
                    .any(|needle| !needle.is_empty() && a.contains(&needle.to_ascii_lowercase()))
            });
            let title_hit = title.as_deref().is_some_and(|t| {
                rule.title_contains
                    .iter()
                    .any(|needle| !needle.is_empty() && t.contains(&needle.to_ascii_lowercase()))
            });
            if app_hit || title_hit {
                outcome.float |= rule.float;
                outcome.fullscreen |= rule.fullscreen;
            }
        }
        outcome
    }
}

pub fn game_rules_config_path() -> PathBuf {
    config_dir().join("game-rules.json")
}

pub fn load_game_rules_config() -> GameRulesConfig {
    let path = game_rules_config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str(&text) {
            return cfg;
        }
    }
    GameRulesConfig::default()
}

pub fn save_game_rules_config(cfg: &GameRulesConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(game_rules_config_path(), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_class_floats() {
        let cfg = GameRulesConfig::default();
        let out = cfg.evaluate(Some("Steam"), Some("Steam"));
        assert!(out.float);
        assert!(!out.fullscreen);
    }

    #[test]
    fn unknown_app_does_not_match() {
        let cfg = GameRulesConfig::default();
        let out = cfg.evaluate(Some("org.gnome.TextEditor"), Some("Untitled"));
        assert!(!out.matched());
    }

    #[test]
    fn disabled_matches_nothing() {
        let cfg = GameRulesConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!cfg.evaluate(Some("steam"), None).matched());
    }

    #[test]
    fn proton_exe_floats_and_fullscreens() {
        let cfg = GameRulesConfig::default();
        let out = cfg.evaluate(Some("hl2.exe"), None);
        assert!(out.float);
        assert!(out.fullscreen);
        let steam_app = cfg.evaluate(Some("steam_app_123456"), None);
        assert!(steam_app.float);
        assert!(steam_app.fullscreen);
    }

    #[test]
    fn hytale_game_client_fullscreens_but_launcher_does_not() {
        let cfg = GameRulesConfig::default();
        // The game client reports `HytaleClient` — float AND fullscreen.
        let game = cfg.evaluate(Some("HytaleClient"), Some("Application"));
        assert!(game.float);
        assert!(game.fullscreen);
        // The launcher stays a normal floating window (never force-fullscreened).
        let launcher = cfg.evaluate(Some("com.hypixel.HytaleLauncher"), Some("Hytale Launcher"));
        assert!(launcher.float);
        assert!(!launcher.fullscreen);
    }
}
