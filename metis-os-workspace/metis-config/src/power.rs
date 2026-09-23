use serde::{Deserialize, Serialize};

/// Power profile exposed by power-profiles-daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PowerProfile {
    #[default]
    Balanced,
    PowerSaver,
    Performance,
}

/// What to do when the laptop lid is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LidCloseAction {
    #[default]
    Suspend,
    Ignore,
    Hibernate,
    PowerOff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerConfig {
    /// Preferred power profile (applied via `powerprofilesctl` when available).
    #[serde(default)]
    pub profile: PowerProfile,
    /// Screen blank timeout in minutes (`0` = never). Persisted preference; applied
    /// via logind when supported.
    #[serde(default = "default_blank_minutes")]
    pub blank_after_minutes: u32,
    /// Suspend after idle in minutes (`0` = never).
    #[serde(default = "default_suspend_minutes")]
    pub suspend_after_minutes: u32,
    #[serde(default)]
    pub lid_close: LidCloseAction,
    /// Dim the screen when on battery (compositor overlay wash).
    #[serde(default = "default_true")]
    pub dim_on_battery: bool,
}

fn default_blank_minutes() -> u32 {
    10
}

fn default_suspend_minutes() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            profile: PowerProfile::default(),
            blank_after_minutes: default_blank_minutes(),
            suspend_after_minutes: default_suspend_minutes(),
            lid_close: LidCloseAction::default(),
            dim_on_battery: true,
        }
    }
}

pub fn power_config_path() -> std::path::PathBuf {
    super::config_dir().join("power.json")
}

pub fn load_power_config() -> PowerConfig {
    let path = power_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    PowerConfig::default()
}

pub fn save_power_config(cfg: &PowerConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(power_config_path(), json)
}
