//! User locale override (`locale.json`).

use serde::{Deserialize, Serialize};

/// Language & region preferences for the Metis session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocaleConfig {
    /// BCP-47 / `ll_CC` override. `None` means follow `LANG` / `LC_*`.
    #[serde(default)]
    pub locale: Option<String>,
    /// When true, date/number formatting follows the active locale.
    #[serde(default = "default_true")]
    pub formats_from_locale: bool,
}

fn default_true() -> bool {
    true
}

impl Default for LocaleConfig {
    fn default() -> Self {
        Self {
            locale: None,
            formats_from_locale: true,
        }
    }
}

pub fn locale_config_path() -> std::path::PathBuf {
    super::config_dir().join("locale.json")
}

pub fn load_locale_config() -> LocaleConfig {
    let path = locale_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    LocaleConfig::default()
}

pub fn save_locale_config(cfg: &LocaleConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(locale_config_path(), text)
}
