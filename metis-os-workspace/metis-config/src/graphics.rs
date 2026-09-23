//! Session graphics profile: Auto / Compatibility / Normal for VM-safe GTK rendering.
//!
//! This is independent of Gaming's PRIME/`graphics_mode` (dGPU offload). Compatibility
//! forces Cairo GSK and disables window animations so VirtualBox / broken guest GL
//! paths do not blank GTK windows or leave cursor trails.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// How Metis should handle GTK software vs hardware UI rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsProfile {
    /// Soft path only when a VM is detected (recommended).
    #[default]
    Auto,
    /// Always force Cairo GSK and suppress window animations.
    Compatibility,
    /// Never force the soft path (user override when guest GL works).
    Normal,
}

impl GraphicsProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Compatibility => "compatibility",
            Self::Normal => "normal",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "compatibility" => Self::Compatibility,
            "normal" => Self::Normal,
            _ => Self::Auto,
        }
    }
}

/// True when this machine looks like a VM (VirtualBox, VMware, KVM, Hyper-V, …).
///
/// Result is cached for the process lifetime.
pub fn is_virtual_machine() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect_virtual_machine)
}

fn detect_virtual_machine() -> bool {
    if let Ok(out) = std::process::Command::new("systemd-detect-virt")
        .arg("--quiet")
        .status()
        && out.success()
    {
        return true;
    }
    if let Ok(out) = std::process::Command::new("systemd-detect-virt").output() {
        let v = String::from_utf8_lossy(&out.stdout)
            .trim()
            .to_ascii_lowercase();
        if matches!(
            v.as_str(),
            "oracle" | "kvm" | "qemu" | "vmware" | "microsoft" | "xen" | "bhyve" | "bochs"
        ) {
            return true;
        }
    }
    for path in [
        "/sys/class/dmi/id/sys_vendor",
        "/sys/class/dmi/id/product_name",
        "/sys/class/dmi/id/board_vendor",
    ] {
        if let Ok(text) = std::fs::read_to_string(path) {
            let lower = text.to_ascii_lowercase();
            if lower.contains("innotek")
                || lower.contains("virtualbox")
                || lower.contains("vmware")
                || lower.contains("qemu")
                || lower.contains("kvm")
                || lower.contains("microsoft corporation")
                || lower.contains("hyper-v")
            {
                return true;
            }
        }
    }
    // VMSVGA / VMware SVGA guest driver — common under VirtualBox with VMSVGA.
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let driver = entry.path().join("device/driver");
            if let Ok(target) = std::fs::read_link(&driver)
                && target
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s == "vmwgfx")
            {
                return true;
            }
        }
    }
    false
}

/// Whether the session should use the soft UI path (Cairo GSK, no window animations).
pub fn effective_graphics_compatibility(profile: GraphicsProfile) -> bool {
    match profile {
        GraphicsProfile::Compatibility => true,
        GraphicsProfile::Normal => false,
        GraphicsProfile::Auto => is_virtual_machine(),
    }
}

/// Load profile from `config.json` and resolve effective compatibility.
pub fn session_graphics_compatibility() -> bool {
    effective_graphics_compatibility(crate::load_app_config().graphics_profile)
}

/// Env value for `METIS_GRAPHICS_PROFILE` (resolved effective mode).
pub fn effective_graphics_profile_label(profile: GraphicsProfile) -> &'static str {
    if effective_graphics_compatibility(profile) {
        "compatibility"
    } else {
        "normal"
    }
}
