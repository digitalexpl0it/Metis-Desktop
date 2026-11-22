//! Apply accessibility toggles: AT tools, audio, Metis theme overrides.

use std::process::{Command, Stdio};
use std::sync::Mutex;

use metis_config::AccessibilityConfig;

static MONO_MODULE: Mutex<Option<String>> = Mutex::new(None);

/// Persist and apply a full accessibility snapshot.
pub fn save_and_apply(cfg: &AccessibilityConfig) {
    if let Err(err) = metis_config::save_accessibility_config(cfg) {
        tracing::warn!(%err, "failed to save accessibility.json");
    }
    apply(cfg);
}

pub fn apply(cfg: &AccessibilityConfig) {
    apply_screen_reader(cfg.screen_reader);
    apply_mono_audio(cfg.mono_audio);
    // Magnifier + shake-pointer live in the compositor (DRM zoom / sonar).
    notify_compositor();
    // Vision chrome (high contrast / invert / filters) lives in theme tokens —
    // ask the shell to rebuild stylesheets from the updated config.
    crate::ui::theme::reload_stylesheet();
}

fn notify_compositor() {
    // Kill any leftover Magnus from older builds.
    let _ = Command::new("pkill")
        .args(["-x", "magnus"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(err) = metis_protocol::send_compositor_command(
        &metis_protocol::CompositorCommand::ReloadAccessibility,
    ) {
        tracing::warn!(%err, "failed to reload accessibility in compositor");
    }
}

pub fn set_screen_reader(on: bool) {
    let mut cfg = metis_config::load_accessibility_config();
    cfg.screen_reader = on;
    save_and_apply(&cfg);
}

pub fn set_magnifier(on: bool) {
    let mut cfg = metis_config::load_accessibility_config();
    cfg.magnifier = on;
    save_and_apply(&cfg);
}

pub fn set_high_contrast(on: bool) {
    let mut cfg = metis_config::load_accessibility_config();
    cfg.high_contrast = on;
    save_and_apply(&cfg);
}

pub fn set_invert_colors(on: bool) {
    let mut cfg = metis_config::load_accessibility_config();
    cfg.invert_colors = on;
    save_and_apply(&cfg);
}

fn apply_screen_reader(on: bool) {
    if on {
        // Avoid stacking multiple Orca instances.
        let _ = Command::new("pkill")
            .args(["-x", "orca"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match Command::new("orca")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => tracing::info!("screen reader (orca) started"),
            Err(err) => tracing::warn!(%err, "failed to start orca"),
        }
    } else {
        let _ = Command::new("pkill")
            .args(["-x", "orca"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn apply_mono_audio(on: bool) {
    let mut slot = match MONO_MODULE.lock() {
        Ok(g) => g,
        Err(err) => {
            tracing::warn!(%err, "mono audio lock poisoned");
            return;
        }
    };
    if on {
        if slot.is_some() {
            return;
        }
        // Remap the default sink to mono. Module index is parsed so we can unload.
        let output = Command::new("pactl")
            .args([
                "load-module",
                "module-remap-sink",
                "sink_name=metis_mono",
                "master=@DEFAULT_SINK@",
                "channels=1",
                "channel_map=mono",
            ])
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let id = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !id.is_empty() {
                    let _ = Command::new("pactl")
                        .args(["set-default-sink", "metis_mono"])
                        .status();
                    *slot = Some(id);
                    tracing::info!("mono audio remap loaded");
                }
            }
            Ok(o) => {
                tracing::warn!(
                    status = ?o.status,
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "pactl load-module mono failed"
                );
            }
            Err(err) => tracing::warn!(%err, "pactl unavailable for mono audio"),
        }
    } else if let Some(id) = slot.take() {
        let _ = Command::new("pactl")
            .args(["unload-module", &id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
