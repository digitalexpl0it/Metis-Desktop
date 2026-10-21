use serde::{Deserialize, Serialize};

/// Preferences for the compositor scratchpad stash (`~/.config/metis/scratchpad.json`).
///
/// Distinct from `desk.json` (live grid layout) and from minimize / per-workspace
/// `stashed_app_tiles`. Windows whose `app_id` matches [`prefer_app_ids`] are
/// auto-sent to the scratchpad on first map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScratchpadConfig {
    /// Wayland / X11 app ids that prefer the scratchpad on map (case-sensitive).
    #[serde(default)]
    pub prefer_app_ids: Vec<String>,
}

pub fn scratchpad_config_path() -> std::path::PathBuf {
    super::config_dir().join("scratchpad.json")
}

pub fn load_scratchpad_config() -> ScratchpadConfig {
    let path = scratchpad_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<ScratchpadConfig>(&text) {
            Ok(cfg) => return cfg.sanitize(),
            Err(err) => tracing::warn!(%err, "scratchpad.json parse failed — using defaults"),
        }
    }
    ScratchpadConfig::default()
}

pub fn save_scratchpad_config(config: &ScratchpadConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    crate::persist::write_json_atomic(&scratchpad_config_path(), &config.clone().sanitize())
}

impl ScratchpadConfig {
    pub fn sanitize(mut self) -> Self {
        self.prefer_app_ids
            .retain(|id| !id.trim().is_empty() && !id.contains('\0'));
        self.prefer_app_ids.sort();
        self.prefer_app_ids.dedup();
        self
    }

    pub fn prefers(&self, app_id: &str) -> bool {
        !app_id.is_empty() && self.prefer_app_ids.iter().any(|id| id == app_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let cfg = ScratchpadConfig::default();
        assert!(cfg.prefer_app_ids.is_empty());
        assert!(!cfg.prefers("kitty"));
    }

    #[test]
    fn round_trip_json() {
        let cfg = ScratchpadConfig {
            prefer_app_ids: vec!["org.gnome.Calculator".into(), "kitty".into()],
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: ScratchpadConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.sanitize().prefer_app_ids,
            vec!["kitty", "org.gnome.Calculator"]
        );
    }

    #[test]
    fn sanitize_is_idempotent() {
        let once = ScratchpadConfig {
            prefer_app_ids: vec!["".into(), "a".into(), "a".into(), "b".into()],
        }
        .sanitize();
        let twice = once.clone().sanitize();
        assert_eq!(once, twice);
        assert_eq!(once.prefer_app_ids, vec!["a", "b"]);
    }
}
