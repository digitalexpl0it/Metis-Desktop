use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Local,
    Caldav,
    Thunderbird,
    Ms365,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarAccount {
    pub id: String,
    pub kind: AccountKind,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub read_only: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarsConfig {
    #[serde(default)]
    pub accounts: Vec<CalendarAccount>,
    #[serde(default)]
    pub local_dir: Option<String>,
}

impl Default for CalendarsConfig {
    fn default() -> Self {
        Self {
            accounts: vec![CalendarAccount {
                id: "local".into(),
                kind: AccountKind::Local,
                name: "Local".into(),
                url: None,
                username: None,
                tenant: None,
                client_id: None,
                color: Some("rgba(34, 211, 238, 0.9)".into()),
                enabled: true,
                read_only: false,
            }],
            local_dir: None,
        }
    }
}

pub fn calendars_config_path() -> std::path::PathBuf {
    super::config_dir().join("calendars.json")
}

/// Default local calendar directory: `~/.local/share/metis/calendars`.
pub fn default_local_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "metis", "metis")
        .map(|d| d.data_dir().join("calendars"))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".local/share/metis/calendars"))
                .unwrap_or_else(|_| std::path::PathBuf::from(".local/share/metis/calendars"))
        })
}

pub fn load_calendars_config() -> CalendarsConfig {
    let path = calendars_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str::<CalendarsConfig>(&text) {
            return cfg;
        }
        tracing::warn!("calendars.json parse failed — using defaults");
    }
    let cfg = CalendarsConfig::default();
    let _ = save_calendars_config(&cfg);
    cfg
}

pub fn save_calendars_config(config: &CalendarsConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(calendars_config_path(), json)
}
