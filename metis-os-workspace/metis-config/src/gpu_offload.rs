//! Hybrid GPU detection and PRIME offload environment variables (sysfs only —
//! no Smithay dependency). Shared by compositor spawn paths and `metis-gaming`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuOffloadKind {
    Nvidia,
    Mesa {
        dri_prime: String,
        vk_select: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HybridGpuInfo {
    pub kind: GpuOffloadKind,
    pub discrete_label: String,
}

/// Scan `/sys/class/drm` for a discrete GPU distinct from the display GPU.
pub fn detect_hybrid_gpu(display_pci: Option<&str>) -> Option<HybridGpuInfo> {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return None;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = entry.path().join("device");
        if !dev.join("boot_vga").exists() {
            continue;
        }
        let pci = fs::canonicalize(&dev)
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
        if pci.as_deref() == display_pci {
            continue;
        }
        let vendor = read_hex(&dev.join("vendor"));
        let device = read_hex(&dev.join("device"));
        let label = gpu_label(&dev, vendor.as_deref());
        match vendor.as_deref() {
            // Report NVIDIA discrete even before the proprietary module loads so
            // health / setup can surface a driver gap (offload env is only applied
            // when the compositor actually detected a usable dGPU render path).
            Some("10de") => {
                return Some(HybridGpuInfo {
                    kind: GpuOffloadKind::Nvidia,
                    discrete_label: label,
                });
            }
            Some(_) => {
                let dri_prime = pci
                    .as_ref()
                    .map(|p| format!("pci-{}", p.replace([':', '.'], "_")))?;
                let vk_select = match (vendor, device) {
                    (Some(v), Some(d)) => Some(format!("{v}:{d}")),
                    _ => None,
                };
                return Some(HybridGpuInfo {
                    kind: GpuOffloadKind::Mesa {
                        dri_prime,
                        vk_select,
                    },
                    discrete_label: label,
                });
            }
            None => continue,
        }
    }
    None
}

/// Environment variables for PRIME render offload (if-unset semantics for callers).
pub fn offload_env_vars(info: &HybridGpuInfo) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    match &info.kind {
        GpuOffloadKind::Nvidia => {
            env.insert("__NV_PRIME_RENDER_OFFLOAD".into(), "1".into());
            env.insert("__GLX_VENDOR_LIBRARY_NAME".into(), "nvidia".into());
            env.insert("__VK_LAYER_NV_optimus".into(), "NVIDIA_only".into());
        }
        GpuOffloadKind::Mesa {
            dri_prime,
            vk_select,
        } => {
            env.insert("DRI_PRIME".into(), dri_prime.clone());
            if let Some(sel) = vk_select {
                env.insert("MESA_VK_DEVICE_SELECT".into(), sel.clone());
            }
        }
    }
    env
}

/// PCI address of the GPU that owns a connected panel (boot_vga=1), if any.
pub fn display_gpu_pci() -> Option<String> {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return None;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = entry.path().join("device");
        if fs::read_to_string(dev.join("boot_vga"))
            .map(|s| s.trim() == "1")
            .unwrap_or(false)
        {
            return fs::canonicalize(&dev)
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
        }
    }
    None
}

fn read_hex(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let s = text.trim().trim_start_matches("0x").to_lowercase();
    (!s.is_empty()).then_some(s)
}

fn gpu_label(device: &Path, vendor: Option<&str>) -> String {
    if vendor == Some("10de") {
        return "NVIDIA GPU".to_string();
    }
    if vendor == Some("1002") {
        return "AMD GPU".to_string();
    }
    if let Ok(model) = fs::read_to_string(device.join("device")) {
        let id = model.trim();
        if !id.is_empty() {
            return format!("GPU {id}");
        }
    }
    "Discrete GPU".to_string()
}
