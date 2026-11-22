//! Persist and apply accessibility preferences from Settings.

use metis_config::{AccessibilityConfig, ColorFilterType};

pub fn load() -> AccessibilityConfig {
    metis_config::load_accessibility_config()
}

pub fn save(cfg: &AccessibilityConfig) {
    if let Err(err) = metis_config::save_accessibility_config(cfg) {
        tracing::warn!(%err, "failed to save accessibility.json");
    }
    apply(cfg);
}

pub fn apply(cfg: &AccessibilityConfig) {
    apply_screen_reader(cfg.screen_reader);
    apply_mono_audio(cfg.mono_audio);
    // Magnifier + shake-pointer are compositor-owned.
    crate::runtime::reload_accessibility();
    // Vision chrome: reload Metis theme tokens + ask the shell to refresh.
    crate::theme::reapply();
    crate::runtime::send("reload-theme");
}

pub fn launch_on_screen_keyboard() {
    for bin in ["squeekboard", "onboard", "wvkbd-mobintl", "wvkbd"] {
        match std::process::Command::new(bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => return,
            Err(err) => tracing::debug!(bin, %err, "on-screen keyboard unavailable"),
        }
    }
    tracing::warn!("no on-screen keyboard found");
}

fn apply_screen_reader(on: bool) {
    if on {
        let _ = std::process::Command::new("pkill")
            .args(["-x", "orca"])
            .status();
        let _ = std::process::Command::new("orca")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    } else {
        let _ = std::process::Command::new("pkill")
            .args(["-x", "orca"])
            .status();
    }
}

fn apply_mono_audio(on: bool) {
    // Best-effort: load/unload a mono remap. Settings does not track module id
    // across restarts; turning off unloads by sink name when possible.
    if on {
        let _ = std::process::Command::new("pactl")
            .args([
                "load-module",
                "module-remap-sink",
                "sink_name=metis_mono",
                "master=@DEFAULT_SINK@",
                "channels=1",
                "channel_map=mono",
            ])
            .status();
        let _ = std::process::Command::new("pactl")
            .args(["set-default-sink", "metis_mono"])
            .status();
    } else {
        let _ = std::process::Command::new("pactl")
            .args(["unload-module", "module-remap-sink"])
            .status();
    }
}

pub fn color_filter_index(kind: ColorFilterType) -> u32 {
    ColorFilterType::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(0) as u32
}

pub fn color_filter_from_index(idx: u32) -> ColorFilterType {
    ColorFilterType::ALL
        .get(idx as usize)
        .copied()
        .unwrap_or_default()
}
