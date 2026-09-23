//! Per-application window geometry persistence.
//!
//! Floating app windows (e.g. the settings app, or any window the user has
//! dragged off the grid) remember their last on-screen position and size, keyed
//! by Wayland `app_id`. The store is written to `~/.config/metis/windows.json`
//! and reloaded on the next compositor start so apps reopen where they were left.
//!
//! Geometry is re-validated on restore: saved position and size are kept when
//! still visible on a connected output; otherwise the window is pulled back
//! on-screen. Saved geometry is only applied in free or scroll layout mode;
//! grid workspaces tile.

use std::collections::HashMap;
use std::path::PathBuf;

use metis_grid::PixelRect;
use serde::{Deserialize, Serialize};

/// A single app's saved floating geometry, in global logical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl SavedGeometry {
    pub fn from_rect(rect: PixelRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    pub fn to_rect(self) -> PixelRect {
        PixelRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// Map of `app_id` -> last known floating geometry.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WindowStateStore {
    #[serde(default)]
    geometries: HashMap<String, SavedGeometry>,
}

impl WindowStateStore {
    /// Load the store from disk, returning an empty store on any error.
    pub fn load() -> Self {
        let path = window_state_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(store) => store,
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "failed to parse windows.json");
                Self::default()
            }
        }
    }

    pub fn get(&self, app_id: &str) -> Option<SavedGeometry> {
        self.geometries.get(app_id).copied()
    }

    /// Record an app's geometry and persist the whole store to disk.
    pub fn set(&mut self, app_id: &str, geometry: SavedGeometry) {
        self.geometries.insert(app_id.to_string(), geometry);
        self.save();
    }

    /// Drop an app's saved geometry (e.g. a stale/degenerate entry) and persist.
    pub fn remove(&mut self, app_id: &str) {
        if self.geometries.remove(app_id).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let path = window_state_path();
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(%err, "failed to create config dir for windows.json");
            return;
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    tracing::warn!(%err, path = %path.display(), "failed to write windows.json");
                }
            }
            Err(err) => tracing::warn!(%err, "failed to serialize window state"),
        }
    }
}

pub fn window_state_path() -> PathBuf {
    directories::ProjectDirs::from("com", "metis", "metis")
        .map(|dirs| dirs.config_dir().join("windows.json"))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config/metis/windows.json"))
                .unwrap_or_else(|_| PathBuf::from(".config/metis/windows.json"))
        })
}
