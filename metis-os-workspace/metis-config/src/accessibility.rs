//! Accessibility preferences (`~/.config/metis/accessibility.json`).

use serde::{Deserialize, Serialize};

/// Color-vision filter applied to Metis GTK chrome (best-effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorFilterType {
    #[default]
    Greyscale,
    Protanopia,
    Deuteranopia,
    Tritanopia,
}

impl ColorFilterType {
    pub const ALL: [Self; 4] = [
        Self::Greyscale,
        Self::Protanopia,
        Self::Deuteranopia,
        Self::Tritanopia,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Greyscale => "Greyscale",
            Self::Protanopia => "Protanopia",
            Self::Deuteranopia => "Deuteranopia",
            Self::Tritanopia => "Tritanopia",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityConfig {
    #[serde(default)]
    pub screen_reader: bool,
    #[serde(default)]
    pub magnifier: bool,
    #[serde(default)]
    pub high_contrast: bool,
    #[serde(default)]
    pub invert_colors: bool,
    #[serde(default)]
    pub color_filters: bool,
    #[serde(default)]
    pub color_filter_type: ColorFilterType,
    /// Enlarge the pointer briefly when the user shakes it (compositor/shell).
    #[serde(default = "default_true")]
    pub shake_pointer: bool,
    #[serde(default)]
    pub mono_audio: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AccessibilityConfig {
    fn default() -> Self {
        Self {
            screen_reader: false,
            magnifier: false,
            high_contrast: false,
            invert_colors: false,
            color_filters: false,
            color_filter_type: ColorFilterType::default(),
            shake_pointer: true,
            mono_audio: false,
        }
    }
}

pub fn accessibility_config_path() -> std::path::PathBuf {
    super::config_dir().join("accessibility.json")
}

pub fn load_accessibility_config() -> AccessibilityConfig {
    let path = accessibility_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    AccessibilityConfig::default()
}

pub fn save_accessibility_config(cfg: &AccessibilityConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    crate::persist::write_json_atomic(&accessibility_config_path(), cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips() {
        let raw = serde_json::to_string(&AccessibilityConfig::default()).unwrap();
        let again: AccessibilityConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(again, AccessibilityConfig::default());
    }
}
