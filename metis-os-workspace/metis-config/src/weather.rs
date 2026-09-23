use serde::{Deserialize, Serialize};

/// Temperature display unit. `Auto` resolves at fetch time from the location's
/// country (US-style regions get Fahrenheit) or the system locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TempUnit {
    #[default]
    Auto,
    Celsius,
    Fahrenheit,
}

/// A pinned weather location. When present, these override timezone auto-detect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherLocation {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherConfig {
    #[serde(default)]
    pub unit: TempUnit,
    /// Detect a location automatically when no `locations` are pinned.
    #[serde(default = "default_true")]
    pub auto_detect: bool,
    /// Use IP-based geolocation (city-level) for auto-detect. When false, falls
    /// back to the coarser system-timezone lookup (offline, zone anchor city).
    #[serde(default = "default_true")]
    pub ip_geolocation: bool,
    /// Ordered, user-pinned locations (first is the bar's primary reading).
    #[serde(default)]
    pub locations: Vec<WeatherLocation>,
}

fn default_true() -> bool {
    true
}

// NOTE: implemented by hand (not derived) so `WeatherConfig::default()` — used
// when no `weather.json` exists — enables auto-detect. A derived `Default` would
// set `auto_detect` to `false` (bool's default), disabling detection entirely.
impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            unit: TempUnit::default(),
            auto_detect: true,
            ip_geolocation: true,
            locations: Vec::new(),
        }
    }
}

pub fn weather_config_path() -> std::path::PathBuf {
    super::config_dir().join("weather.json")
}

pub fn load_weather_config() -> WeatherConfig {
    let path = weather_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str(&text) {
            return cfg;
        }
        tracing::warn!("weather.json parse failed — using defaults");
    }
    WeatherConfig::default()
}

/// Persist the weather configuration (used by the settings app's Weather page).
/// The shell refetches on the next poll (or immediately via `reload-weather`).
pub fn save_weather_config(config: &WeatherConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(weather_config_path(), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_autodetect() {
        assert!(
            WeatherConfig::default().auto_detect,
            "auto-detect must default to on when no weather.json exists"
        );
    }
}
