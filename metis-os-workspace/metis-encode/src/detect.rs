//! Pick VAAPI vs NVENC from the DRM render node path / PCI vendor.

use std::fs;
use std::path::Path;

use crate::types::EncoderBackend;

const PCI_NVIDIA: &str = "0x10de";

/// Resolve `Auto` using the render node; explicit backends pass through.
pub fn resolve_backend(requested: EncoderBackend, drm_render_node: &str) -> EncoderBackend {
    match requested {
        EncoderBackend::Auto => detect_preferred_backend(drm_render_node),
        other => other,
    }
}

/// NVIDIA PCI vendor → NVENC; everything else → VAAPI.
pub fn detect_preferred_backend(drm_render_node: &str) -> EncoderBackend {
    if render_node_is_nvidia(drm_render_node) {
        EncoderBackend::Nvenc
    } else {
        EncoderBackend::Vaapi
    }
}

fn render_node_is_nvidia(drm_render_node: &str) -> bool {
    let path = Path::new(drm_render_node);
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // /sys/class/drm/renderD128/device/vendor
    let sys = Path::new("/sys/class/drm").join(name).join("device/vendor");
    match fs::read_to_string(&sys) {
        Ok(v) => v.trim().eq_ignore_ascii_case(PCI_NVIDIA),
        Err(_) => {
            // Fallback: card name heuristics (nouveau/nvidia in uevent).
            let uevent = Path::new("/sys/class/drm").join(name).join("device/uevent");
            fs::read_to_string(uevent)
                .map(|t| t.to_ascii_lowercase().contains("nvidia"))
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_passthrough() {
        assert_eq!(
            resolve_backend(EncoderBackend::Vaapi, "/dev/dri/renderD128"),
            EncoderBackend::Vaapi
        );
        assert_eq!(
            resolve_backend(EncoderBackend::Nvenc, "/dev/dri/renderD128"),
            EncoderBackend::Nvenc
        );
    }
}
