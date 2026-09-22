//! Software update preferences (`~/.config/metis/updates.json`).
//!
//! Controls background checks, snooze, and which sources (PackageKit / Flatpak /
//! fwupd) Metis polls. Apply/refresh still go through `metis-remote` + polkit.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSources {
    #[serde(default = "default_true")]
    pub packagekit: bool,
    #[serde(default = "default_true")]
    pub flatpak: bool,
    #[serde(default = "default_true")]
    pub fwupd: bool,
}

impl Default for UpdateSources {
    fn default() -> Self {
        Self {
            packagekit: true,
            flatpak: true,
            fwupd: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdatesConfig {
    /// When false, the shell skips background checks (manual Check still works).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hours between automatic checks (1–168).
    #[serde(default = "default_check_interval_hours")]
    pub check_interval_hours: u32,
    /// Suppress edge-bar icon + notifications until this local time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_until: Option<DateTime<Local>>,
    #[serde(default = "default_true")]
    pub notify_on_available: bool,
    /// Opt-in: install PackageKit security updates without prompting.
    #[serde(default)]
    pub auto_install_security: bool,
    #[serde(default)]
    pub sources: UpdateSources,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_check_interval_hours() -> u32 {
    6
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_hours: default_check_interval_hours(),
            snooze_until: None,
            notify_on_available: true,
            auto_install_security: false,
            sources: UpdateSources::default(),
            last_check: None,
            last_error: None,
        }
    }
}

impl UpdatesConfig {
    pub fn is_snoozed(&self) -> bool {
        self.snooze_until.is_some_and(|until| until > Local::now())
    }

    pub fn clear_expired_snooze(&mut self) {
        if self.snooze_until.is_some_and(|until| until <= Local::now()) {
            self.snooze_until = None;
        }
    }

    /// Snooze for `hours` from now.
    pub fn snooze_hours(&mut self, hours: i64) {
        self.snooze_until = Some(Local::now() + chrono::Duration::hours(hours.max(1)));
    }

    /// Snooze until 21:00 local today, or tomorrow 21:00 if already past.
    pub fn snooze_until_tonight(&mut self) {
        let now = Local::now();
        let today_evening = now
            .date_naive()
            .and_hms_opt(21, 0, 0)
            .and_then(|naive| naive.and_local_timezone(Local).single());
        let until = match today_evening {
            Some(t) if t > now => t,
            _ => now + chrono::Duration::hours(12),
        };
        self.snooze_until = Some(until);
    }

    pub fn snooze_one_day(&mut self) {
        self.snooze_hours(24);
    }
}

pub fn sanitize_updates_config(cfg: &mut UpdatesConfig) {
    cfg.check_interval_hours = cfg.check_interval_hours.clamp(1, 168);
    if let Some(err) = cfg.last_error.as_mut() {
        if err.len() > 512 {
            err.truncate(512);
        }
    }
    cfg.clear_expired_snooze();
}

pub fn updates_config_path() -> std::path::PathBuf {
    super::config_dir().join("updates.json")
}

pub fn load_updates_config() -> UpdatesConfig {
    let path = updates_config_path();
    let mut cfg = if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<UpdatesConfig>(&text) {
                Ok(parsed) => parsed,
                Err(_) => {
                    tracing::warn!("updates.json parse failed — using defaults");
                    UpdatesConfig::default()
                }
            }
        } else {
            UpdatesConfig::default()
        }
    } else {
        UpdatesConfig::default()
    };
    sanitize_updates_config(&mut cfg);
    cfg
}

pub fn save_updates_config(config: &UpdatesConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let mut clean = config.clone();
    sanitize_updates_config(&mut clean);
    let json = serde_json::to_string_pretty(&clean).map_err(std::io::Error::other)?;
    let path = updates_config_path();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_interval() {
        let mut cfg = UpdatesConfig {
            check_interval_hours: 999,
            ..Default::default()
        };
        sanitize_updates_config(&mut cfg);
        assert_eq!(cfg.check_interval_hours, 168);
    }

    #[test]
    fn snooze_hours_is_future() {
        let mut cfg = UpdatesConfig::default();
        cfg.snooze_hours(2);
        assert!(cfg.is_snoozed());
    }
}
