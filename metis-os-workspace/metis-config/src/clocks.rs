use serde::{Deserialize, Serialize};

/// Persistent clock/alarm state, stored at `~/.config/metis/clock.json`.
/// Distinct from `bar::ClockConfig`, which only holds the bar's display format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClocksConfig {
    #[serde(default)]
    pub world_clocks: Vec<String>,
    #[serde(default)]
    pub alarms: Vec<Alarm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alarm {
    pub id: String,
    pub hour: u8,
    pub minute: u8,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Days the alarm repeats on (0 = Sunday .. 6 = Saturday). Empty = every day.
    #[serde(default)]
    pub days: Vec<u8>,
    /// Identifier of the alarm sound to play (see `AlarmSound`). `None` = default.
    #[serde(default)]
    pub sound: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A selectable alarm sound, mapped to a freedesktop/libcanberra event id.
#[derive(Debug, Clone, Copy)]
pub struct AlarmSound {
    pub id: &'static str,
    pub label: &'static str,
    pub canberra_id: &'static str,
}

/// The alarm sounds offered in the picker. The first entry is the default.
pub const ALARM_SOUNDS: &[AlarmSound] = &[
    AlarmSound {
        id: "default",
        label: "Default",
        canberra_id: "alarm-clock-elapsed",
    },
    AlarmSound {
        id: "bell",
        label: "Bell",
        canberra_id: "bell",
    },
    AlarmSound {
        id: "complete",
        label: "Complete",
        canberra_id: "complete",
    },
    AlarmSound {
        id: "message",
        label: "Message",
        canberra_id: "message-new-instant",
    },
    AlarmSound {
        id: "phone",
        label: "Phone",
        canberra_id: "phone-incoming-call",
    },
];

/// Resolve a stored sound id to its libcanberra event id, falling back to default.
pub fn alarm_sound_canberra_id(id: Option<&str>) -> &'static str {
    let id = id.unwrap_or("default");
    ALARM_SOUNDS
        .iter()
        .find(|s| s.id == id)
        .unwrap_or(&ALARM_SOUNDS[0])
        .canberra_id
}

pub fn clocks_config_path() -> std::path::PathBuf {
    super::config_dir().join("clock.json")
}

/// Load clock.json, creating it on first run seeded with `seed_timezones`.
pub fn load_clocks_config(seed_timezones: &[String]) -> ClocksConfig {
    let path = clocks_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str::<ClocksConfig>(&text) {
            return cfg;
        }
        tracing::warn!("clock.json parse failed — using defaults");
    }
    let seeded = ClocksConfig {
        world_clocks: seed_timezones
            .iter()
            .filter(|tz| tz.as_str() != "UTC" || seed_timezones.len() == 1)
            .cloned()
            .collect(),
        alarms: Vec::new(),
    };
    let _ = save_clocks_config(&seeded);
    seeded
}

pub fn save_clocks_config(config: &ClocksConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(clocks_config_path(), json)
}
