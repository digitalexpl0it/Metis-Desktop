//! Runtime load / mtime-poll for `~/.config/metis/decorations.json`.

use std::time::SystemTime;

use metis_config::{DecorationsConfig, load_decorations_config};

#[derive(Debug)]
pub struct DecorationsRuntime {
    pub config: DecorationsConfig,
    path_mtime: Option<SystemTime>,
}

impl Default for DecorationsRuntime {
    fn default() -> Self {
        Self::load()
    }
}

impl DecorationsRuntime {
    pub fn load() -> Self {
        let config = load_decorations_config();
        let path_mtime = std::fs::metadata(metis_config::decorations_config_path())
            .and_then(|m| m.modified())
            .ok();
        Self { config, path_mtime }
    }

    pub fn reload(&mut self) {
        *self = Self::load();
        tracing::info!(
            overrides = self.config.overrides.len(),
            "decorations overrides reloaded"
        );
    }

    /// Re-read when `decorations.json` mtime changes. Returns `true` if config changed.
    pub fn maybe_refresh(&mut self) -> bool {
        let path = metis_config::decorations_config_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime != self.path_mtime {
            self.reload();
            return true;
        }
        false
    }

    /// `Some(true)` force SSD, `Some(false)` force CSD, `None` Auto.
    pub fn user_override(&self, app_id: Option<&str>) -> Option<bool> {
        let id = app_id.filter(|s| !s.is_empty())?;
        self.config.lookup(id).map(|o| o.uses_ssd())
    }
}
