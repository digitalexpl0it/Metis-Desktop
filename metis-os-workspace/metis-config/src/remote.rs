//! Desktop sharing preferences persisted to `~/.config/metis/remote.json`.
//!
//! Credentials are never stored here — gnome-remote-desktop keeps RDP username
//! and password via `grdctl --headless`.

use serde::{Deserialize, Serialize};

/// Remote desktop backend. Default remains GNOME RDP (GRD); RustDesk is optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteBackend {
    #[default]
    GnomeRdp,
    RustDesk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// User wants desktop sharing active (metis-remote enable on toggle / autostart).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: RemoteBackend,
    /// Start sharing when the Metis session opens (if [`Self::enabled`]).
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
    /// When true, `metis-remote enable` applies host firewall rules so TCP 3389
    /// accepts only private / loopback / link-local sources.
    #[serde(default = "default_true")]
    pub lan_only: bool,
    /// Last successful Metis LAN-only firewall apply. Needed because unprivileged
    /// `nft list` / `ufw status` often cannot see rules even when they exist.
    #[serde(default)]
    pub firewall_applied: bool,
    /// `nft`, `ufw`, or empty — backend used for the last successful apply.
    #[serde(default)]
    pub firewall_backend: String,
    /// Last firewall apply/clear failure (cleared on success). Shown in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firewall_last_error: Option<String>,
    /// Last successful Metis LAN-only firewall apply for RustDesk ports.
    #[serde(default)]
    pub rustdesk_firewall_applied: bool,
    #[serde(default)]
    pub rustdesk_firewall_backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdesk_firewall_last_error: Option<String>,
}

fn default_auto_start() -> bool {
    true
}

fn default_true() -> bool {
    true
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: RemoteBackend::default(),
            auto_start: default_auto_start(),
            lan_only: default_true(),
            firewall_applied: false,
            firewall_backend: String::new(),
            firewall_last_error: None,
            rustdesk_firewall_applied: false,
            rustdesk_firewall_backend: String::new(),
            rustdesk_firewall_last_error: None,
        }
    }
}

pub fn remote_config_path() -> std::path::PathBuf {
    super::config_dir().join("remote.json")
}

pub fn load_remote_config() -> RemoteConfig {
    let path = remote_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str(&text) {
            return cfg;
        }
        tracing::warn!("remote.json parse failed — using defaults");
    }
    RemoteConfig::default()
}

pub fn save_remote_config(cfg: &RemoteConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(remote_config_path(), json)
}
