//! Gaming health checks with optional auto-fix actions.

use std::path::Path;

use metis_config::load_gaming_config;

use crate::detect::{
    binary_in_path, detect_steam, flatpak_has_app, gamemode_installed, hybrid_gpu_summary,
    i386_vulkan_likely_missing, mark_nvidia_reboot_required, mesa_vulkan_amd64_missing,
    nvidia_driver_loaded, nvidia_gpu_present, nvidia_reboot_required, pipewire_or_pulse_available,
    steam_devices_installed, user_in_input_group, SteamInstall,
};
use crate::flatpak::{flatpak_steam_needs_optimize, optimize_flatpak_gaming};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthSeverity {
    Ok,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthItem {
    pub id: &'static str,
    pub label: String,
    pub severity: HealthSeverity,
    pub detail: String,
    pub fix_hint: Option<String>,
    pub auto_fixable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HealthCheck {
    pub items: Vec<HealthItem>,
}

pub fn run_health_check() -> HealthCheck {
    let cfg = load_gaming_config();
    let mut items = Vec::new();

    match detect_steam() {
        SteamInstall::Native => items.push(ok("steam", "Steam", "Installed (native)")),
        SteamInstall::Flatpak => items.push(ok("steam", "Steam", "Installed (Flatpak)")),
        SteamInstall::None => items.push(HealthItem {
            id: "steam",
            label: "Steam".into(),
            severity: HealthSeverity::Info,
            detail: "Not detected".into(),
            fix_hint: Some(if cfg.steam_prefer_native {
                "sudo apt install -y steam-installer   # or Flatpak: flathub com.valvesoftware.Steam"
                    .into()
            } else {
                "flatpak install -y flathub com.valvesoftware.Steam   # or: sudo apt install steam-installer"
                    .into()
            }),
            auto_fixable: true,
        }),
    }

    if flatpak_has_app("com.valvesoftware.Steam") && flatpak_steam_needs_optimize() {
        items.push(HealthItem {
            id: "flatpak_steam",
            label: "Flatpak Steam overrides".into(),
            severity: HealthSeverity::Warn,
            detail: "Gaming overrides not applied".into(),
            fix_hint: Some(
                "metis-cmd optimize-gaming --yes   # or Settings → Gaming → Optimize now".into(),
            ),
            auto_fixable: true,
        });
    } else if flatpak_has_app("com.valvesoftware.Steam") {
        items.push(ok("flatpak_steam", "Flatpak Steam overrides", "Optimized"));
    }

    if mesa_vulkan_amd64_missing() {
        items.push(HealthItem {
            id: "mesa_vulkan",
            label: "Mesa Vulkan (64-bit)".into(),
            severity: HealthSeverity::Error,
            detail: "mesa-vulkan-drivers not detected".into(),
            fix_hint: Some("sudo apt install -y mesa-vulkan-drivers".into()),
            auto_fixable: true,
        });
    } else {
        items.push(ok("mesa_vulkan", "Mesa Vulkan (64-bit)", "OK"));
    }

    if i386_vulkan_likely_missing() {
        items.push(HealthItem {
            id: "vulkan_i386",
            label: "32-bit Vulkan".into(),
            severity: HealthSeverity::Error,
            detail: "Proton may fail without i386 Vulkan drivers".into(),
            fix_hint: Some("sudo apt install -y mesa-vulkan-drivers:i386".into()),
            auto_fixable: true,
        });
    } else {
        items.push(ok("vulkan_i386", "32-bit Vulkan", "OK"));
    }

    if cfg.auto_gamemode && !gamemode_installed() {
        items.push(HealthItem {
            id: "gamemode",
            label: "GameMode".into(),
            severity: HealthSeverity::Info,
            detail: "auto_gamemode on but gamemoderun missing".into(),
            fix_hint: Some("sudo apt install -y gamemode".into()),
            auto_fixable: true,
        });
    } else if gamemode_installed() {
        items.push(ok("gamemode", "GameMode", "Available"));
    }

    if !steam_devices_installed()
        && (binary_in_path("steam") || flatpak_has_app("com.valvesoftware.Steam"))
    {
        items.push(HealthItem {
            id: "steam_devices",
            label: "Controller udev rules".into(),
            severity: HealthSeverity::Warn,
            detail: "steam-devices package not detected".into(),
            fix_hint: Some("sudo apt install -y steam-devices".into()),
            auto_fixable: true,
        });
    } else if steam_devices_installed() {
        items.push(ok("steam_devices", "Controller udev rules", "OK"));
    }

    if !user_in_input_group() {
        items.push(HealthItem {
            id: "input_group",
            label: "Input group".into(),
            severity: HealthSeverity::Warn,
            detail: "User not in input group".into(),
            fix_hint: Some("sudo usermod -aG input $USER  (then log out and back in)".into()),
            auto_fixable: true,
        });
    } else {
        items.push(ok("input_group", "Input group", "OK"));
    }

    if nvidia_gpu_present() {
        if nvidia_driver_loaded() {
            items.push(ok("nvidia_driver", "NVIDIA driver", "Loaded"));
        } else if nvidia_reboot_required() {
            items.push(HealthItem {
                id: "nvidia_driver",
                label: "NVIDIA driver".into(),
                severity: HealthSeverity::Error,
                detail: "Installed — reboot required before the driver loads".into(),
                fix_hint: Some("reboot".into()),
                auto_fixable: false,
            });
        } else {
            items.push(HealthItem {
                id: "nvidia_driver",
                label: "NVIDIA driver".into(),
                severity: HealthSeverity::Error,
                detail: "NVIDIA GPU without proprietary driver".into(),
                // Consent-only Install path in Settings — never silent / session-start.
                fix_hint: Some("sudo ubuntu-drivers install".into()),
                auto_fixable: false,
            });
        }
    }

    if let Some(label) = hybrid_gpu_summary() {
        items.push(ok(
            "hybrid_gpu",
            "Hybrid GPU",
            &format!("Discrete: {label}"),
        ));
    } else if nvidia_gpu_present() {
        items.push(ok("hybrid_gpu", "GPU layout", "Single NVIDIA GPU"));
    } else {
        items.push(ok("hybrid_gpu", "Hybrid GPU", "Single GPU"));
    }

    if !pipewire_or_pulse_available() {
        items.push(HealthItem {
            id: "audio",
            label: "Audio".into(),
            severity: HealthSeverity::Warn,
            detail: "No PipeWire/Pulse on PATH".into(),
            fix_hint: Some("sudo apt install -y pipewire-audio".into()),
            auto_fixable: true,
        });
    } else {
        items.push(ok("audio", "Audio", "OK"));
    }

    HealthCheck { items }
}

pub fn auto_fix_item(id: &str) -> Result<String, String> {
    match id {
        "flatpak_steam" => {
            let results = optimize_flatpak_gaming()?;
            Ok(results
                .iter()
                .map(|r| format!("{}: {}", r.app_id, r.message))
                .collect::<Vec<_>>()
                .join("; "))
        }
        "input_group" => add_user_to_input_group(),
        "gamemode" => pkexec_apt_install(&["gamemode"], "GameMode"),
        "mesa_vulkan" => pkexec_apt_install(&["mesa-vulkan-drivers"], "Mesa Vulkan drivers"),
        "vulkan_i386" => pkexec_apt_install(&["mesa-vulkan-drivers:i386"], "32-bit Vulkan drivers"),
        "steam_devices" => pkexec_apt_install(&["steam-devices"], "Steam controller udev rules"),
        "audio" => pkexec_apt_install(&["pipewire-audio"], "PipeWire audio"),
        "steam" => install_steam(),
        "nvidia_driver" => {
            Err("NVIDIA drivers require explicit consent — use Install in Settings → Gaming".into())
        }
        other => Err(format!("no auto-fix for {other}")),
    }
}

/// Consent-gated NVIDIA install via Polkit (`ubuntu-drivers install` fixed argv).
pub fn install_nvidia_drivers() -> Result<String, String> {
    if !nvidia_gpu_present() {
        return Ok("No NVIDIA GPU detected".into());
    }
    if nvidia_driver_loaded() {
        return Ok("NVIDIA driver is already loaded".into());
    }
    if !binary_in_path("pkexec") {
        return Err("pkexec not found — run: sudo ubuntu-drivers install  (then reboot)".into());
    }
    if !binary_in_path("ubuntu-drivers") && !Path::new("/usr/bin/ubuntu-drivers").exists() {
        return Err("ubuntu-drivers not found — install ubuntu-drivers-common, then retry".into());
    }
    let bin = metis_remote_bin();
    let output = std::process::Command::new("timeout")
        .args(["--signal=TERM", "--kill-after=5s", "600s"])
        .arg("pkexec")
        .arg(&bin)
        .arg("pk-ubuntu-drivers-install")
        .output()
        .map_err(|e| format!("failed to start timeout/pkexec: {e}"))?;
    if output.status.success() {
        mark_nvidia_reboot_required();
        return Ok("NVIDIA drivers installed — reboot required before the driver loads".into());
    }
    let code = output.status.code();
    if code == Some(124) || code == Some(137) {
        return Err(
            "Timed out waiting for admin approval or driver install. Retry from Settings → Gaming."
                .into(),
        );
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        "Could not install NVIDIA drivers (auth cancelled?). Run: sudo ubuntu-drivers install"
            .into()
    } else {
        format!("NVIDIA install failed: {detail}")
    })
}

fn install_steam() -> Result<String, String> {
    match detect_steam() {
        SteamInstall::Native | SteamInstall::Flatpak => {
            return Ok("Steam is already installed".into());
        }
        SteamInstall::None => {}
    }
    let prefer_native = load_gaming_config().steam_prefer_native;
    if prefer_native {
        match pkexec_apt_install(&["steam-installer"], "Steam") {
            Ok(msg) => return Ok(msg),
            Err(err) => {
                tracing::warn!(%err, "native Steam install failed; trying Flatpak");
            }
        }
    }
    if binary_in_path("flatpak") {
        let status = std::process::Command::new("flatpak")
            .args(["install", "-y", "flathub", "com.valvesoftware.Steam"])
            .status()
            .map_err(|e| format!("failed to start flatpak: {e}"))?;
        if status.success() {
            let _ = optimize_flatpak_gaming();
            return Ok("Installed Flatpak Steam (overrides applied when needed)".into());
        }
        if prefer_native {
            return Err(
                "Could not install Steam via apt or Flatpak. Run: sudo apt install steam-installer"
                    .into(),
            );
        }
    }
    if !prefer_native {
        return pkexec_apt_install(&["steam-installer"], "Steam");
    }
    Err("Could not install Steam — install flatpak or run: sudo apt install steam-installer".into())
}

fn pkexec_apt_install(packages: &[&str], label: &str) -> Result<String, String> {
    if packages.is_empty() {
        return Ok(format!("{label}: nothing to install"));
    }
    if !binary_in_path("pkexec") {
        return Err(format!(
            "pkexec not found — run: sudo apt install -y {}",
            packages.join(" ")
        ));
    }
    let bin = metis_remote_bin();
    let status = std::process::Command::new("pkexec")
        .arg(&bin)
        .arg("pk-apt-install")
        .args(packages)
        .status()
        .map_err(|e| format!("failed to start pkexec: {e}"))?;
    if status.success() {
        Ok(format!("Installed {label}"))
    } else {
        Err(format!(
            "Could not install {label} (auth cancelled?). Run: sudo apt install -y {}",
            packages.join(" ")
        ))
    }
}

fn metis_remote_bin() -> String {
    const INSTALLED: &str = "/usr/bin/metis-remote";
    if Path::new(INSTALLED).is_file() {
        return INSTALLED.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("metis-remote");
            if sibling.is_file() {
                return sibling.to_string_lossy().into_owned();
            }
        }
    }
    "metis-remote".into()
}

fn add_user_to_input_group() -> Result<String, String> {
    let user = std::env::var("USER").map_err(|_| "USER is not set".to_string())?;
    if user.is_empty() || user.contains(['/', ' ', '\0']) {
        return Err("refusing to modify an invalid USER".into());
    }
    if user_in_input_group() {
        return Ok("Already in the input group".into());
    }
    if !binary_in_path("pkexec") {
        return Err("pkexec not found — run: sudo usermod -aG input $USER  (then log out)".into());
    }
    let bin = metis_remote_bin();
    let status = std::process::Command::new("pkexec")
        .arg(&bin)
        .args(["pk-add-input-group", &user])
        .status()
        .map_err(|e| format!("failed to start pkexec: {e}"))?;
    if status.success() {
        Ok(format!(
            "Added {user} to the input group — log out and back in for it to apply"
        ))
    } else {
        Err(
            "Could not add you to the input group (auth cancelled?). Run: sudo usermod -aG input $USER"
                .into(),
        )
    }
}

fn ok(id: &'static str, label: &str, detail: &str) -> HealthItem {
    HealthItem {
        id,
        label: label.into(),
        severity: HealthSeverity::Ok,
        detail: detail.into(),
        fix_hint: None,
        auto_fixable: false,
    }
}
