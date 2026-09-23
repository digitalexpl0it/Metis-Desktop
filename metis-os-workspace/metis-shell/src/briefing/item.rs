use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherConfig {
    #[serde(default = "default_lat")]
    pub latitude: f64,
    #[serde(default = "default_lon")]
    pub longitude: f64,
}

fn default_lat() -> f64 {
    37.77
}

fn default_lon() -> f64 {
    -122.42
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            latitude: default_lat(),
            longitude: default_lon(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssConfig {
    #[serde(default = "default_feed")]
    pub feed_url: String,
}

fn default_feed() -> String {
    "https://feeds.bbci.co.uk/news/rss.xml".into()
}

impl Default for RssConfig {
    fn default() -> Self {
        Self {
            feed_url: default_feed(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefingItem {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BriefingConfig {
    #[serde(default)]
    pub weather: WeatherConfig,
    #[serde(default)]
    pub rss: RssConfig,
}

pub fn load_briefing_config() -> BriefingConfig {
    let path = crate::config::briefing_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    BriefingConfig::default()
}
