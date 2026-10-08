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
    if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<CalendarsConfig>(&text) {
                Ok(cfg) => return cfg,
                Err(err) => {
                    // Match bar.json: never rewrite a corrupt file (watcher loops).
                    tracing::warn!(%err, "calendars.json parse failed — using defaults (not rewriting disk)");
                    return CalendarsConfig::default();
                }
            }
        }
        tracing::warn!("calendars.json unreadable — using defaults (not rewriting disk)");
        return CalendarsConfig::default();
    }
    let cfg = CalendarsConfig::default();
    let _ = save_calendars_config(&cfg);
    cfg
}

pub fn save_calendars_config(config: &CalendarsConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    crate::persist::write_json_atomic(&calendars_config_path(), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // Serialise env mutations — other modules also touch XDG_CONFIG_HOME.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_fail_does_not_rewrite_disk() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = std::env::temp_dir().join(format!(
            "metis-calendars-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };

        let path = calendars_config_path();
        fs::create_dir_all(path.parent().expect("parent")).expect("metis dir");
        let corrupt = "{not valid json";
        fs::write(&path, corrupt).expect("write corrupt");
        let before = fs::metadata(&path).expect("meta").modified().ok();

        let cfg = load_calendars_config();
        assert!(cfg.accounts.iter().any(|a| a.id == "local"));
        let after_text = fs::read_to_string(&path).expect("read after");
        assert_eq!(after_text, corrupt, "corrupt file must not be rewritten");
        let after = fs::metadata(&path).expect("meta2").modified().ok();
        assert_eq!(before, after);

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
