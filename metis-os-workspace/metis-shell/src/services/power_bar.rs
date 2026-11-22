//! Power profiles for the edge-bar battery / power overlay.
//!
//! Mirrors Settings (`powerprofilesctl` get/list/set) so the bar and Power page
//! stay aligned without linking the settings crate into the shell.

use metis_config::{PowerConfig, PowerProfile};

/// Current profile plus which profiles power-profiles-daemon exposes.
pub fn read_power_profile() -> (PowerProfile, Vec<PowerProfile>) {
    let output = std::process::Command::new("powerprofilesctl")
        .arg("get")
        .output()
        .ok();
    let text = output
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_lowercase())
        .unwrap_or_default();
    let profile = match text.as_str() {
        "power-saver" => PowerProfile::PowerSaver,
        "performance" => PowerProfile::Performance,
        _ => PowerProfile::Balanced,
    };
    let list_out = std::process::Command::new("powerprofilesctl")
        .args(["list"])
        .output()
        .ok();
    let mut available = Vec::new();
    if let Some(o) = list_out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let key = line.split(':').next().unwrap_or("").trim().to_lowercase();
            match key.as_str() {
                "power-saver" => available.push(PowerProfile::PowerSaver),
                "balanced" => available.push(PowerProfile::Balanced),
                "performance" => available.push(PowerProfile::Performance),
                _ => {}
            }
        }
    }
    if available.is_empty() {
        // Daemon missing — still offer the three canonical choices; set may no-op.
        available = vec![
            PowerProfile::PowerSaver,
            PowerProfile::Balanced,
            PowerProfile::Performance,
        ];
    }
    (profile, available)
}

/// Whether `powerprofilesctl` responds (hide profile list when totally absent).
pub fn power_profiles_available() -> bool {
    std::process::Command::new("powerprofilesctl")
        .arg("list")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn apply_power_profile(profile: PowerProfile) {
    let name = match profile {
        PowerProfile::Balanced => "balanced",
        PowerProfile::PowerSaver => "power-saver",
        PowerProfile::Performance => "performance",
    };
    let _ = std::process::Command::new("powerprofilesctl")
        .args(["set", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // Keep power.json in sync with Settings.
    let mut cfg = metis_config::load_power_config();
    if cfg.profile != profile {
        cfg.profile = profile;
        if let Err(err) = metis_config::save_power_config(&cfg) {
            tracing::warn!(%err, "failed to persist power profile");
        }
    }
}

pub fn profile_title(profile: PowerProfile) -> String {
    match profile {
        PowerProfile::PowerSaver => metis_i18n::tr("Power Saver"),
        PowerProfile::Balanced => metis_i18n::tr("Balanced"),
        PowerProfile::Performance => metis_i18n::tr("High Performance"),
    }
}

pub fn profile_subtitle(profile: PowerProfile) -> String {
    match profile {
        PowerProfile::PowerSaver => metis_i18n::tr("Reduced power usage and performance."),
        PowerProfile::Balanced => metis_i18n::tr("Standard performance and battery usage."),
        PowerProfile::Performance => metis_i18n::tr("High performance and power usage."),
    }
}

/// Preferred order in the overlay (Power Saver → Balanced → High Performance).
pub fn profile_display_order(available: &[PowerProfile]) -> Vec<PowerProfile> {
    const ORDER: [PowerProfile; 3] = [
        PowerProfile::PowerSaver,
        PowerProfile::Balanced,
        PowerProfile::Performance,
    ];
    ORDER
        .into_iter()
        .filter(|p| available.contains(p))
        .collect()
}

#[allow(dead_code)] // handy for tests / future OSD
pub fn load_power_config() -> PowerConfig {
    metis_config::load_power_config()
}
