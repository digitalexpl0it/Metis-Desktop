//! Steam / package / hardware detection shared by health checks and settings.

use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamInstall {
    Native,
    Flatpak,
    None,
}

pub fn detect_steam() -> SteamInstall {
    if binary_in_path("steam") {
        return SteamInstall::Native;
    }
    if binary_in_path("flatpak") && flatpak_has_app("com.valvesoftware.Steam") {
        return SteamInstall::Flatpak;
    }
    SteamInstall::None
}

pub fn flatpak_has_app(app_id: &str) -> bool {
    Command::new("flatpak")
        .args(["info", app_id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn binary_in_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
            && fs::metadata(&candidate)
                .map(|m| {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                })
                .unwrap_or(false)
    })
}

pub fn gamemode_installed() -> bool {
    binary_in_path("gamemoderun")
}

/// True when native Steam / Proton is present but 32-bit Vulkan looks missing.
pub fn i386_vulkan_likely_missing() -> bool {
    if dpkg_package_installed("mesa-vulkan-drivers:i386") {
        return false;
    }
    if Path::new("/usr/lib/i386-linux-gnu/libvulkan.so.1").exists()
        || Path::new("/usr/lib32/libvulkan.so.1").exists()
    {
        return false;
    }
    binary_in_path("steam") || flatpak_has_app("com.valvesoftware.Steam")
}

/// True when the amd64 Mesa Vulkan package / ICD is missing on a Debian/Ubuntu host.
pub fn mesa_vulkan_amd64_missing() -> bool {
    if dpkg_package_installed("mesa-vulkan-drivers") {
        return false;
    }
    // Heuristic: ICD JSON or libvulkan for amd64.
    if Path::new("/usr/share/vulkan/icd.d/radeon_icd.x86_64.json").exists()
        || Path::new("/usr/share/vulkan/icd.d/intel_icd.x86_64.json").exists()
        || Path::new("/usr/lib/x86_64-linux-gnu/libvulkan_radeon.so").exists()
        || Path::new("/usr/lib/x86_64-linux-gnu/libvulkan_intel.so").exists()
    {
        return false;
    }
    // Only flag when gaming is relevant (Steam present or NVIDIA/AMD discrete).
    binary_in_path("steam")
        || flatpak_has_app("com.valvesoftware.Steam")
        || nvidia_gpu_present()
        || hybrid_gpu_summary().is_some()
}

pub fn steam_devices_installed() -> bool {
    dpkg_package_installed("steam-devices")
        || Path::new("/lib/udev/rules.d/60-steam-input.rules").exists()
        || Path::new("/usr/lib/udev/rules.d/60-steam-input.rules").exists()
}

fn dpkg_package_installed(pkg: &str) -> bool {
    Command::new("dpkg")
        .args(["-l", pkg])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ii"))
        .unwrap_or(false)
}

pub fn user_in_input_group() -> bool {
    let Ok(passwd) = fs::read_to_string("/etc/group") else {
        return true;
    };
    let user = std::env::var("USER").unwrap_or_default();
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 4 && parts[0] == "input" {
            return parts[3].split(',').any(|m| m == user);
        }
    }
    true
}

pub fn nvidia_driver_loaded() -> bool {
    Path::new("/proc/driver/nvidia").exists()
}

/// PCI vendor `10de` on any DRM card (hybrid or single-GPU NVIDIA).
pub fn nvidia_gpu_present() -> bool {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return false;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let vendor = fs::read_to_string(entry.path().join("device/vendor")).unwrap_or_default();
        if vendor.trim().eq_ignore_ascii_case("0x10de") {
            return true;
        }
    }
    false
}

/// Best-effort NVIDIA driver series digits (e.g. `"535"`) from installed packages.
pub fn installed_nvidia_driver_series() -> Option<String> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Package}\n", "nvidia-driver-*"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let pkg = line.trim();
        if let Some(rest) = pkg.strip_prefix("nvidia-driver-") {
            let series: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !series.is_empty() {
                return Some(series);
            }
        }
    }
    None
}

pub fn pipewire_or_pulse_available() -> bool {
    binary_in_path("pipewire") || binary_in_path("pulseaudio") || binary_in_path("pw-cli")
}

pub fn hybrid_gpu_summary() -> Option<String> {
    let display = metis_config::display_gpu_pci();
    metis_config::detect_hybrid_gpu(display.as_deref()).map(|h| h.discrete_label)
}

/// Marker written after a successful guided NVIDIA install until the module loads.
pub fn nvidia_reboot_required_path() -> std::path::PathBuf {
    metis_config::config_dir().join("nvidia-reboot-required")
}

pub fn nvidia_reboot_required() -> bool {
    if nvidia_driver_loaded() {
        let _ = fs::remove_file(nvidia_reboot_required_path());
        return false;
    }
    nvidia_reboot_required_path().is_file()
}

pub fn mark_nvidia_reboot_required() {
    let path = nvidia_reboot_required_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, b"1\n");
}
