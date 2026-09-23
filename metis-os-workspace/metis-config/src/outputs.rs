use chrono::Timelike;
use serde::{Deserialize, Serialize};

/// How multiple displays are arranged: extended desktop or duplicated (mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DisplayLayoutMode {
    #[default]
    Extend,
    Mirror,
}

/// Per-output display preferences. Keys match compositor output names (`metis-0`, …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputPrefs {
    /// UI scale factor (1.0 = 100%). Applied by the compositor when supported.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Whether this output is enabled (future compositor hook).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Logical desktop X origin (pixels). When set, the compositor places this
    /// output at this position; unset outputs are auto-packed left-to-right.
    #[serde(default)]
    pub layout_x: Option<i32>,
    /// Logical desktop Y origin (pixels).
    #[serde(default)]
    pub layout_y: Option<i32>,
    /// Saved video mode width in pixels (`None` = compositor default / preferred).
    #[serde(default)]
    pub mode_width: Option<i32>,
    /// Saved video mode height in pixels.
    #[serde(default)]
    pub mode_height: Option<i32>,
    /// Saved refresh rate in millihertz (60_000 = 60 Hz).
    #[serde(default)]
    pub mode_refresh_millihz: Option<i32>,
    /// Variable refresh rate (FreeSync / G-Sync compatible) when the DRM driver
    /// supports it.
    #[serde(default)]
    pub vrr_enabled: bool,
    /// HDR output signaling (DRM Colorspace + HDR_OUTPUT_METADATA) when the
    /// connector advertises HDR capability. Compositor PQ encode is follow-up.
    #[serde(default)]
    pub hdr_enabled: bool,
    /// Optional ICC colour profile path (`.icc` / `.icm`). Stage 1 applies the
    /// profile's `vcgt` calibration to the CRTC gamma ramp.
    #[serde(default)]
    pub color_profile: Option<String>,
    /// Night-light warm shift enabled on this output (Phase 5 precursor).
    #[serde(default)]
    pub night_light: bool,
}

fn default_scale() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

impl Default for OutputPrefs {
    fn default() -> Self {
        Self {
            scale: default_scale(),
            enabled: true,
            layout_x: None,
            layout_y: None,
            mode_width: None,
            mode_height: None,
            mode_refresh_millihz: None,
            vrr_enabled: false,
            hdr_enabled: false,
            color_profile: None,
            night_light: false,
        }
    }
}

/// Local-time window for automatic night light (24-hour `HH:MM`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NightLightSchedule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_night_start")]
    pub start: String,
    #[serde(default = "default_night_end")]
    pub end: String,
}

impl Default for NightLightSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            start: default_night_start(),
            end: default_night_end(),
        }
    }
}

fn default_night_start() -> String {
    "20:00".into()
}

fn default_night_end() -> String {
    "07:00".into()
}

/// Half-hour preset slots from midnight through 23:30 (minutes since midnight).
pub fn schedule_half_hour_presets() -> impl Iterator<Item = u32> {
    (0..48).map(|i| i * 30)
}

/// Format minutes since midnight as stored `HH:MM` (24-hour).
pub fn minutes_to_hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Format a stored `HH:MM` value for display (12- or 24-hour).
pub fn format_schedule_hhmm(hhmm: &str, use_12h: bool) -> Option<String> {
    let minutes = parse_hhmm(hhmm)?;
    Some(format_schedule_minutes(minutes, use_12h))
}

/// Format minutes since midnight for the schedule UI.
pub fn format_schedule_minutes(minutes: u32, use_12h: bool) -> String {
    if use_12h {
        let h24 = minutes / 60;
        let m = minutes % 60;
        let (h12, am) = match h24 {
            0 => (12, true),
            1..=11 => (h24, true),
            12 => (12, false),
            _ => (h24 - 12, false),
        };
        format!("{}:{:02} {}", h12, m, if am { "AM" } else { "PM" })
    } else {
        minutes_to_hhmm(minutes)
    }
}

/// Parse user schedule input into stored 24-hour `HH:MM`.
pub fn parse_schedule_input(input: &str, use_12h: bool) -> Option<String> {
    let trimmed = input.trim();
    if use_12h {
        parse_12h_input(trimmed)
    } else {
        parse_hhmm(trimmed).map(minutes_to_hhmm)
    }
}

fn parse_12h_input(input: &str) -> Option<String> {
    let upper = input.to_uppercase();
    let (time_part, pm) = if upper.ends_with(" AM") {
        (&input[..input.len().saturating_sub(3)], false)
    } else if upper.ends_with(" PM") {
        (&input[..input.len().saturating_sub(3)], true)
    } else {
        return None;
    };
    let (h, m) = time_part.trim().split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if !(1..=12).contains(&h) || m >= 60 {
        return None;
    }
    let hour24 = match (h, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, false) => h,
        (h, true) => h + 12,
    };
    Some(minutes_to_hhmm(hour24 * 60 + m))
}

/// Parse `HH:MM` into minutes since local midnight.
pub fn parse_hhmm(value: &str) -> Option<u32> {
    let (h, m) = value.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    Some(h * 60 + m)
}

fn local_minutes_since_midnight() -> Option<u32> {
    let now = chrono::Local::now();
    Some(now.hour() * 60 + now.minute())
}

/// Whether `now` falls inside `[start, end)` with overnight wrap (e.g. 20:00–07:00).
pub fn schedule_window_active(now: u32, start: u32, end: u32) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

impl NightLightSchedule {
    pub fn active_now(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(now) = local_minutes_since_midnight() else {
            return false;
        };
        let Some(start) = parse_hhmm(&self.start) else {
            return false;
        };
        let Some(end) = parse_hhmm(&self.end) else {
            return false;
        };
        schedule_window_active(now, start, end)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OutputsConfig {
    /// Per-output overrides keyed by compositor output name.
    #[serde(default)]
    pub outputs: std::collections::HashMap<String, OutputPrefs>,
    /// Global night-light schedule (future): enabled + warm temperature.
    #[serde(default)]
    pub night_light_enabled: bool,
    /// Colour temperature in kelvin when night light is on (e.g. 4000).
    #[serde(default = "default_night_temp")]
    pub night_light_temperature: u32,
    /// Optional local-time schedule; when enabled, night light only tints inside
    /// `night_light_schedule.start`–`end` while the master toggle is on.
    #[serde(default)]
    pub night_light_schedule: NightLightSchedule,
    /// When true, Settings shows schedule times in 12-hour form; stored values stay
    /// 24-hour `HH:MM`.
    #[serde(default)]
    pub night_light_schedule_12h: bool,
    /// Extended desktop vs duplicate (mirror) mode.
    #[serde(default)]
    pub display_mode: DisplayLayoutMode,
    /// Output name to mirror from when `display_mode` is `Mirror`. `None` uses the
    /// first enabled output (leftmost saved layout, else name order).
    #[serde(default)]
    pub mirror_source: Option<String>,
    /// Which output is the primary display (desk widgets, default focus, bar).
    /// `None` keeps the compositor default (first mapped output).
    #[serde(default)]
    pub primary_output: Option<String>,
}

fn default_night_temp() -> u32 {
    4000
}

pub fn outputs_config_path() -> std::path::PathBuf {
    super::config_dir().join("outputs.json")
}

pub fn load_outputs_config() -> OutputsConfig {
    load_outputs_config_with_fallback(&OutputsConfig::default())
}

/// Like [`load_outputs_config`], but keeps `fallback` when the file is missing or
/// cannot be parsed (e.g. a concurrent write left a partial JSON blob).
pub fn load_outputs_config_with_fallback(fallback: &OutputsConfig) -> OutputsConfig {
    let path = outputs_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    fallback.clone()
}

pub fn save_outputs_config(cfg: &OutputsConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let path = outputs_config_path();
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

/// Merge saved prefs for `name`, creating defaults when missing.
pub fn output_prefs(cfg: &OutputsConfig, name: &str) -> OutputPrefs {
    cfg.outputs.get(name).cloned().unwrap_or_default()
}

/// Whether the warm night-light overlay should render right now.
pub fn night_light_effective(cfg: &OutputsConfig) -> bool {
    if !cfg.night_light_enabled {
        return false;
    }
    cfg.night_light_schedule.active_now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_12h_round_trip() {
        assert_eq!(
            parse_schedule_input("8:30 PM", true).as_deref(),
            Some("20:30")
        );
        assert_eq!(
            format_schedule_hhmm("20:30", true).as_deref(),
            Some("8:30 PM")
        );
        assert_eq!(
            format_schedule_hhmm("00:00", true).as_deref(),
            Some("12:00 AM")
        );
    }

    #[test]
    fn overnight_schedule_window() {
        assert!(schedule_window_active(21 * 60, 20 * 60, 7 * 60));
        assert!(!schedule_window_active(12 * 60, 20 * 60, 7 * 60));
        assert!(schedule_window_active(6 * 60, 20 * 60, 7 * 60));
    }

    #[test]
    fn same_day_schedule_window() {
        assert!(schedule_window_active(10 * 60, 9 * 60, 17 * 60));
        assert!(!schedule_window_active(18 * 60, 9 * 60, 17 * 60));
    }
}
