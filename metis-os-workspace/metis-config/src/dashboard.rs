use serde::{Deserialize, Serialize};

/// Built-in dashboard widget identifiers (v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardWidgetId {
    Cpu,
    Memory,
    Disk,
    Network,
    Battery,
    Logs,
    Processes,
}

impl DashboardWidgetId {
    pub fn default_order() -> &'static [DashboardWidgetId] {
        &[
            DashboardWidgetId::Cpu,
            DashboardWidgetId::Memory,
            DashboardWidgetId::Disk,
            DashboardWidgetId::Network,
            DashboardWidgetId::Battery,
            DashboardWidgetId::Logs,
            DashboardWidgetId::Processes,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "Processor",
            Self::Memory => "Memory",
            Self::Disk => "Storage",
            Self::Network => "Network",
            Self::Battery => "Battery",
            Self::Logs => "Logs",
            Self::Processes => "Processes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardConfig {
    /// Master switch for the pull-down dashboard gesture and surface.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum panel height as a percentage of the space below the bar (20–100).
    #[serde(default = "default_max_height_percent")]
    pub max_height_percent: u8,
    /// Background poll interval in milliseconds (500–5000).
    #[serde(default = "default_refresh_ms")]
    pub refresh_interval_ms: u32,
    /// Ask before sending SIGTERM to a process from the list.
    #[serde(default = "default_true")]
    pub confirm_before_kill: bool,
    /// Enabled widgets in display order.
    #[serde(default = "default_widgets")]
    pub widgets: Vec<DashboardWidgetId>,
    /// Process monitor app: binary name or absolute path. `None` = auto-detect
    /// from [`KNOWN_PROCESS_MONITORS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_monitor: Option<String>,
}

/// Known system/process monitors in auto-detect preference order: `(binary, label)`.
/// TUI tools (`btop`, `htop`) are launched inside the configured terminal.
pub const KNOWN_PROCESS_MONITORS: &[(&str, &str)] = &[
    ("btop", "btop"),
    ("htop", "htop"),
    ("gnome-system-monitor", "GNOME System Monitor"),
    ("plasma-systemmonitor", "Plasma System Monitor"),
    ("mate-system-monitor", "MATE System Monitor"),
    ("xfce4-taskmanager", "XFCE Task Manager"),
    ("ksysguard", "KSysGuard"),
];

/// True when `bin` is a TUI monitor that needs a terminal (`-e` / equivalent).
pub fn process_monitor_needs_terminal(bin: &str) -> bool {
    let name = std::path::Path::new(bin.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(bin)
        .to_ascii_lowercase();
    matches!(name.as_str(), "btop" | "htop" | "top" | "atop" | "glances")
}

fn default_true() -> bool {
    true
}

fn default_max_height_percent() -> u8 {
    100
}

fn default_refresh_ms() -> u32 {
    1000
}

fn default_widgets() -> Vec<DashboardWidgetId> {
    DashboardWidgetId::default_order().to_vec()
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_height_percent: default_max_height_percent(),
            refresh_interval_ms: default_refresh_ms(),
            confirm_before_kill: default_true(),
            widgets: default_widgets(),
            process_monitor: None,
        }
    }
}

pub fn dashboard_config_path() -> std::path::PathBuf {
    super::config_dir().join("dashboard.json")
}

pub fn load_dashboard_config() -> DashboardConfig {
    let path = dashboard_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return sanitize(cfg);
    }
    DashboardConfig::default()
}

pub fn save_default_dashboard_config() -> std::io::Result<()> {
    let path = dashboard_config_path();
    if path.exists() {
        return Ok(());
    }
    save_dashboard_config(&DashboardConfig::default())
}

pub fn save_dashboard_config(cfg: &DashboardConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json =
        serde_json::to_string_pretty(&sanitize(cfg.clone())).map_err(std::io::Error::other)?;
    std::fs::write(dashboard_config_path(), json)
}

fn sanitize(mut cfg: DashboardConfig) -> DashboardConfig {
    cfg.max_height_percent = cfg.max_height_percent.clamp(20, 100);
    cfg.refresh_interval_ms = cfg.refresh_interval_ms.clamp(500, 5000);
    if cfg.widgets.is_empty() {
        cfg.widgets = default_widgets();
    }
    if let Some(mon) = cfg.process_monitor.as_mut() {
        let trimmed = mon.trim().to_string();
        if trimmed.is_empty() {
            cfg.process_monitor = None;
        } else {
            *mon = trimmed;
        }
    }
    cfg
}
