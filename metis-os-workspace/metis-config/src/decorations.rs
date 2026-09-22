//! Per-app window decoration overrides (`~/.config/metis/decorations.json`).
//!
//! Lets the user force Metis server-side chrome (`server`) or native client
//! chrome (`client`) for a given Wayland/X11 `app_id`. Missing keys mean Auto —
//! the compositor's built-in heuristics and `xdg-decoration` negotiation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{config_dir, ensure_config_dirs};

/// Force Metis titlebar (`Server`) or leave chrome to the app (`Client`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecorationsOverride {
    /// Draw Metis SSD (titlebar + traffic lights).
    Server,
    /// Do not draw Metis SSD; client / CSD owns chrome.
    Client,
}

impl DecorationsOverride {
    /// `true` when Metis should own window chrome.
    pub fn uses_ssd(self) -> bool {
        matches!(self, Self::Server)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecorationsConfig {
    /// Exact `app_id` keys (stored lowercase). Absent means Auto.
    #[serde(default = "default_decoration_overrides")]
    pub overrides: BTreeMap<String, DecorationsOverride>,
}

/// Apps that ship their own chrome — default to App titlebar so Auto does not
/// paint a second Metis titlebar on first run.
fn default_decoration_overrides() -> BTreeMap<String, DecorationsOverride> {
    let mut overrides = BTreeMap::new();
    for id in [
        "gnome-terminal",
        "gnome-text-editor",
        "org.gnome.terminal",
        "org.gnome.texteditor",
        "org.vinegarhq.sober",
        "virt-manager",
    ] {
        overrides.insert(id.to_string(), DecorationsOverride::Client);
    }
    overrides
}

impl Default for DecorationsConfig {
    fn default() -> Self {
        Self {
            overrides: default_decoration_overrides(),
        }
    }
}

impl DecorationsConfig {
    /// Case-insensitive lookup for a live window `app_id`.
    ///
    /// Matches exactly, or loosely enough that a desktop reverse-DNS override
    /// like `org.gnome.shotwell` still hits a bare WM_CLASS `shotwell` (and the
    /// reverse). Avoids requiring the user to re-toggle every app after we learn
    /// better candidate keys.
    pub fn lookup(&self, app_id: &str) -> Option<DecorationsOverride> {
        let key = norm_key(app_id);
        if key.is_empty() {
            return None;
        }
        if let Some(mode) = self.overrides.get(&key).copied() {
            return Some(mode);
        }
        for (stored, mode) in &self.overrides {
            if keys_related(&key, stored) {
                return Some(*mode);
            }
        }
        None
    }

    /// Set or clear an override for one key.
    pub fn set_override(&mut self, app_id: &str, mode: Option<DecorationsOverride>) {
        let key = norm_key(app_id);
        if key.is_empty() {
            return;
        }
        match mode {
            Some(mode) => {
                self.overrides.insert(key, mode);
            }
            None => {
                self.overrides.remove(&key);
            }
        }
    }

    /// Apply the same mode (or Auto) to every candidate id for one launcher entry.
    pub fn set_for_candidates(&mut self, candidates: &[String], mode: Option<DecorationsOverride>) {
        for id in candidates {
            self.set_override(id, mode);
        }
        // Auto also clears any previously related reverse-DNS / bare keys that
        // aren't in the current candidate list (e.g. old `thunar-settings`-only
        // write) so leftover mismatches don't stick around.
        if mode.is_none() {
            let related: Vec<String> = self
                .overrides
                .keys()
                .filter(|stored| {
                    candidates
                        .iter()
                        .any(|c| keys_related(&norm_key(c), stored))
                })
                .cloned()
                .collect();
            for k in related {
                self.overrides.remove(&k);
            }
        }
    }

    /// Effective UI mode for a launcher entry: first candidate that has an override.
    pub fn mode_for_candidates(&self, candidates: &[String]) -> Option<DecorationsOverride> {
        candidates.iter().find_map(|id| self.lookup(id))
    }
}

/// True when a live `app_id` and a stored override key refer to the same app.
fn keys_related(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // `org.gnome.shotwell` ↔ `shotwell`, `com.github.matoking.protontricks` ↔ `protontricks`
    if a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}")) {
        return true;
    }
    // Reject very short tokens (`app`, `org`, …) to avoid false positives.
    const MIN: usize = 5;
    if a.len() >= MIN && b.contains(a) {
        return true;
    }
    if b.len() >= MIN && a.contains(b) {
        return true;
    }
    // Wine WM_CLASS vs desktop StartupWMClass: `cheatengine-x86_64-sse4-avx2.exe`
    // ↔ `cheat engine.exe` (spaces / arch / SSE tags differ; alphanumeric stem matches).
    wine_stems_related(a, b)
}

/// Strip `.exe` / punctuation so Wine class and desktop WM_CLASS can match.
fn wine_alnum_stem(id: &str) -> String {
    let lower = id.trim().to_ascii_lowercase();
    let bare = lower.strip_suffix(".exe").unwrap_or(&lower);
    bare.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn wine_stems_related(a: &str, b: &str) -> bool {
    let aa = wine_alnum_stem(a);
    let bb = wine_alnum_stem(b);
    // Require a real product stem so short noise keys don't collide.
    const MIN: usize = 6;
    if aa.len() < MIN || bb.len() < MIN {
        return false;
    }
    aa.starts_with(&bb) || bb.starts_with(&aa) || aa.contains(&bb) || bb.contains(&aa)
}

fn norm_key(app_id: &str) -> String {
    app_id.trim().to_ascii_lowercase()
}

pub fn decorations_config_path() -> PathBuf {
    config_dir().join("decorations.json")
}

pub fn load_decorations_config() -> DecorationsConfig {
    let path = decorations_config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(mut cfg) = serde_json::from_str::<DecorationsConfig>(&text) {
            // Normalize keys so hand-edited mixed-case files still match.
            let raw = std::mem::take(&mut cfg.overrides);
            for (k, v) in raw {
                cfg.overrides.insert(norm_key(&k), v);
            }
            return cfg;
        }
    }
    DecorationsConfig::default()
}

/// Write `decorations.json` on first run so Settings shows the factory
/// titlebar overrides. Existing files are left untouched.
pub fn save_default_decorations_config() -> std::io::Result<()> {
    ensure_config_dirs()?;
    let path = decorations_config_path();
    if path.exists() {
        return Ok(());
    }
    save_decorations_config(&DecorationsConfig::default())
}

pub fn save_decorations_config(cfg: &DecorationsConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    let path = decorations_config_path();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_cfg() -> DecorationsConfig {
        DecorationsConfig {
            overrides: BTreeMap::new(),
        }
    }

    #[test]
    fn factory_defaults_force_client_chrome_on_csd_apps() {
        let cfg = DecorationsConfig::default();
        for id in [
            "gnome-terminal",
            "gnome-text-editor",
            "org.gnome.terminal",
            "org.gnome.texteditor",
            "org.vinegarhq.sober",
            "virt-manager",
        ] {
            assert_eq!(cfg.lookup(id), Some(DecorationsOverride::Client));
        }
        assert!(cfg.lookup("kitty").is_none());
    }

    #[test]
    fn lookup_related_reverse_dns_and_bare_class() {
        let mut cfg = empty_cfg();
        cfg.set_override("org.gnome.shotwell", Some(DecorationsOverride::Client));
        assert_eq!(cfg.lookup("shotwell"), Some(DecorationsOverride::Client));
        assert_eq!(
            cfg.lookup("org.gnome.Shotwell"),
            Some(DecorationsOverride::Client)
        );

        let mut cfg2 = empty_cfg();
        cfg2.set_override("seahorse", Some(DecorationsOverride::Client));
        assert_eq!(
            cfg2.lookup("org.gnome.seahorse.Application"),
            Some(DecorationsOverride::Client)
        );
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let mut cfg = empty_cfg();
        cfg.set_override("MousePad", Some(DecorationsOverride::Client));
        assert_eq!(cfg.lookup("mousepad"), Some(DecorationsOverride::Client));
        assert_eq!(cfg.lookup("MOUSEPAD"), Some(DecorationsOverride::Client));
        assert!(cfg.lookup("kitty").is_none());
    }

    #[test]
    fn candidates_set_and_clear() {
        let mut cfg = empty_cfg();
        let cands = vec!["mousepad".into(), "org.xfce.mousepad".into()];
        cfg.set_for_candidates(&cands, Some(DecorationsOverride::Client));
        assert_eq!(
            cfg.mode_for_candidates(&cands),
            Some(DecorationsOverride::Client)
        );
        cfg.set_for_candidates(&cands, None);
        assert!(cfg.mode_for_candidates(&cands).is_none());
        assert!(cfg.overrides.is_empty());
    }

    #[test]
    fn roundtrip_json() {
        let mut cfg = empty_cfg();
        cfg.set_override("kitty", Some(DecorationsOverride::Server));
        cfg.set_override("firefox", Some(DecorationsOverride::Client));
        let json = serde_json::to_string(&cfg).unwrap();
        let back: DecorationsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lookup("kitty"), Some(DecorationsOverride::Server));
        assert_eq!(back.lookup("firefox"), Some(DecorationsOverride::Client));
        assert!(json.contains("\"server\""));
        assert!(json.contains("\"client\""));
    }

    #[test]
    fn wine_exe_matches_desktop_wm_class_with_spaces() {
        let mut cfg = empty_cfg();
        cfg.set_override("cheat engine.exe", Some(DecorationsOverride::Server));
        assert_eq!(
            cfg.lookup("cheatengine-x86_64-sse4-avx2.exe"),
            Some(DecorationsOverride::Server)
        );
        assert_eq!(
            cfg.lookup("cheatengine-x86_64.exe"),
            Some(DecorationsOverride::Server)
        );
        // Unrelated .exe must not collide.
        assert!(cfg.lookup("notepad.exe").is_none());
    }
}
