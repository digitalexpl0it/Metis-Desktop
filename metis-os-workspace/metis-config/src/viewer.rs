//! RDP viewer recent hosts — `~/.config/metis/viewer.json`.
//!
//! Passwords are never stored here.

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerHost {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
}

fn default_port() -> u16 {
    3389
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ViewerConfig {
    #[serde(default)]
    pub recent: Vec<ViewerHost>,
}

pub fn viewer_config_path() -> std::path::PathBuf {
    super::config_dir().join("viewer.json")
}

pub fn load_viewer_config() -> ViewerConfig {
    let path = viewer_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str(&text) {
            return cfg;
        }
        tracing::warn!("viewer.json parse failed — using defaults");
    }
    ViewerConfig::default()
}

pub fn save_viewer_config(cfg: &ViewerConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(viewer_config_path(), json)
}

/// Push `entry` to the front of recent hosts (dedupe by host+port+user).
pub fn remember_host(entry: ViewerHost) -> std::io::Result<()> {
    let mut cfg = load_viewer_config();
    cfg.recent.retain(|h| {
        !(h.host == entry.host && h.port == entry.port && h.username == entry.username)
    });
    cfg.recent.insert(0, entry);
    if cfg.recent.len() > MAX_RECENT {
        cfg.recent.truncate(MAX_RECENT);
    }
    save_viewer_config(&cfg)
}

/// Remove a recent host matching host+port+user.
pub fn remove_recent(entry: &ViewerHost) -> std::io::Result<()> {
    let mut cfg = load_viewer_config();
    let before = cfg.recent.len();
    cfg.recent.retain(|h| {
        !(h.host == entry.host && h.port == entry.port && h.username == entry.username)
    });
    if cfg.recent.len() == before {
        return Ok(());
    }
    save_viewer_config(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate XDG_CONFIG_HOME.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn remember_and_remove_recent_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("metis-viewer-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("metis")).unwrap();
        // SAFETY: serialized test; restored before unlock.
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };

        let entry = ViewerHost {
            host: "192.168.1.10".into(),
            port: 3389,
            username: "alice".into(),
        };
        remember_host(entry.clone()).unwrap();
        let cfg = load_viewer_config();
        assert_eq!(cfg.recent.len(), 1);
        assert_eq!(cfg.recent[0], entry);

        remove_recent(&entry).unwrap();
        let cfg = load_viewer_config();
        assert!(cfg.recent.is_empty());

        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
