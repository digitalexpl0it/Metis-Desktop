//! Privileged helpers intended to run only under `pkexec` (Phase 15 §B).

use std::path::Path;
use std::process::Command;

/// Packages Metis may install via Polkit (onboarding + gaming health fixes).
pub const APT_ALLOWLIST: &[&str] = &[
    "gnome-remote-desktop",
    "flatpak",
    "gamemode",
    "bluez",
    "bluetooth",
    "cups",
    "system-config-printer",
    "gnome-keyring",
    "mesa-vulkan-drivers",
    "mesa-vulkan-drivers:i386",
    "pipewire-audio",
    "steam-installer",
    "steam-devices",
    "nftables",
    "policykit-1-gnome",
    "mate-polkit",
];

fn require_root() -> Result<(), String> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if uid == "0" {
        Ok(())
    } else {
        Err("this command must run as root (via pkexec)".into())
    }
}

fn package_allowed(pkg: &str) -> bool {
    APT_ALLOWLIST.contains(&pkg)
}

/// `apt-get install -y -- <allowlisted packages…>` — root only.
pub fn apt_install(packages: &[String]) -> Result<(), String> {
    require_root()?;
    if packages.is_empty() {
        return Ok(());
    }
    for pkg in packages {
        if pkg.is_empty()
            || pkg.contains(['/', ' ', '\0', ';', '|', '&', '$', '`', '\n', '\r'])
            || !package_allowed(pkg)
        {
            return Err(format!(
                "package '{pkg}' is not on the Metis allowlist (refusing apt-get)"
            ));
        }
    }
    let status = Command::new("apt-get")
        .args(["install", "-y", "--"])
        .args(packages)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .map_err(|e| format!("apt-get failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apt-get exited with {status}"))
    }
}

/// Guided NVIDIA install: fixed `ubuntu-drivers install` argv only (no free-form shell).
/// Best-effort matching `libnvidia-gl-<series>:i386` when a series can be detected.
pub fn ubuntu_drivers_install() -> Result<(), String> {
    require_root()?;
    let ubuntu_drivers = if Path::new("/usr/bin/ubuntu-drivers").is_file() {
        "/usr/bin/ubuntu-drivers"
    } else {
        "ubuntu-drivers"
    };
    let status = Command::new(ubuntu_drivers)
        .arg("install")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .map_err(|e| format!("ubuntu-drivers failed: {e}"))?;
    if !status.success() {
        return Err(format!("ubuntu-drivers install exited with {status}"));
    }
    // Best-effort i386 GL for Proton; series detection may fail until reboot.
    if let Some(series) = detect_nvidia_series_digits() {
        let pkg = format!("libnvidia-gl-{series}:i386");
        let _ = Command::new("apt-get")
            .args(["install", "-y", "--", &pkg])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .status();
    } else {
        tracing::warn!(
            "could not detect NVIDIA driver series for i386 GL — install libnvidia-gl-*:i386 after reboot if Proton needs it"
        );
    }
    Ok(())
}

fn detect_nvidia_series_digits() -> Option<String> {
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
            if !series.is_empty() && series.len() <= 4 {
                return Some(series);
            }
        }
    }
    None
}

/// Validate a Unix username (no path separators / control chars).
pub fn validate_username(user: &str) -> Result<(), String> {
    if user.is_empty() || user.len() > 64 {
        return Err("invalid username".into());
    }
    if !user
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err("username contains invalid characters".into());
    }
    Ok(())
}

/// `usermod -aG input <user>` — root only.
pub fn add_input_group(user: &str) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    let status = Command::new("usermod")
        .args(["-aG", "input", user])
        .status()
        .map_err(|e| format!("usermod failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("usermod exited with {status}"))
    }
}

/// Prefer the packaged binary so pkexec cannot escalate a writable cwd copy.
pub fn privileged_exe() -> std::path::PathBuf {
    const INSTALLED: &str = "/usr/bin/metis-remote";
    if Path::new(INSTALLED).is_file() {
        return Path::new(INSTALLED).to_path_buf();
    }
    std::env::current_exe().unwrap_or_else(|_| Path::new("metis-remote").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_includes_gaming_packages() {
        assert!(APT_ALLOWLIST.contains(&"mesa-vulkan-drivers"));
        assert!(APT_ALLOWLIST.contains(&"mesa-vulkan-drivers:i386"));
        assert!(APT_ALLOWLIST.contains(&"steam-devices"));
    }

    #[test]
    fn rejects_unknown_package() {
        assert!(!package_allowed("evil-package"));
        assert!(!package_allowed("mesa-vulkan-drivers;rm -rf /"));
    }
}
