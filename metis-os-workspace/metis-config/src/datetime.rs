//! Metis date/time preferences (`~/.config/metis/datetime.json`).
//! System NTP/timezone remain owned by `timedatectl`; this file stores Metis UI
//! prefs (auto-timezone intent, first day of week).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstDayOfWeek {
    /// Follow the session locale (glib / LC_TIME).
    #[default]
    LocaleDefault,
    Sunday,
    Monday,
}

impl FirstDayOfWeek {
    pub fn as_str(self) -> &'static str {
        match self {
            FirstDayOfWeek::LocaleDefault => "locale_default",
            FirstDayOfWeek::Sunday => "sunday",
            FirstDayOfWeek::Monday => "monday",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            FirstDayOfWeek::LocaleDefault => "Locale Default",
            FirstDayOfWeek::Sunday => "Sunday",
            FirstDayOfWeek::Monday => "Monday",
        }
    }

    /// Chrono weekday index with Sunday = 0 (matches existing calendar math).
    /// `None` means “ask the locale”.
    pub fn sunday_based_index(self) -> Option<u32> {
        match self {
            FirstDayOfWeek::LocaleDefault => None,
            FirstDayOfWeek::Sunday => Some(0),
            FirstDayOfWeek::Monday => Some(1),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateTimeConfig {
    /// When true, Settings may push an IP-geolocated zone via timedatectl.
    #[serde(default)]
    pub auto_timezone: bool,
    #[serde(default)]
    pub first_day_of_week: FirstDayOfWeek,
}

impl Default for DateTimeConfig {
    fn default() -> Self {
        Self {
            auto_timezone: false,
            first_day_of_week: FirstDayOfWeek::LocaleDefault,
        }
    }
}

pub fn datetime_config_path() -> std::path::PathBuf {
    super::config_dir().join("datetime.json")
}

pub fn load_datetime_config() -> DateTimeConfig {
    let path = datetime_config_path();
    if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<DateTimeConfig>(&text) {
                return cfg;
            }
            tracing::warn!("datetime.json parse failed — using defaults");
        }
    }
    DateTimeConfig::default()
}

pub fn save_datetime_config(config: &DateTimeConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(datetime_config_path(), json)
}
