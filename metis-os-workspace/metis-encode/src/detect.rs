//! Pick VAAPI vs NVENC (and the device for each) from DRM render nodes.
//!
//! Frames reach the encoder as CPU-readable LINEAR buffers, so the encode GPU
//! does not have to be the compositor's render GPU. `Auto` therefore tries the
//! render GPU's native encoder first and falls back to the other vendor.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::EncoderBackend;

const PCI_NVIDIA: &str = "0x10de";

/// Resolve `Auto` from the render node vendor; explicit backends pass through.
pub fn resolve_backend(requested: EncoderBackend, drm_render_node: &str) -> EncoderBackend {
    match requested {
        EncoderBackend::Auto => detect_preferred_backend(drm_render_node),
        other => other,
    }
}

/// NVIDIA render node → NVENC; anything else → VAAPI.
pub fn detect_preferred_backend(drm_render_node: &str) -> EncoderBackend {
    if render_node_is_nvidia(drm_render_node) {
        EncoderBackend::Nvenc
    } else {
        EncoderBackend::Vaapi
    }
}

/// Device for `backend`: NVENC wants an NVIDIA node, VAAPI a non-NVIDIA one
/// (NVIDIA VAAPI shims rarely encode). Falls back to `drm_render_node`.
pub fn preferred_render_node(drm_render_node: &str, backend: EncoderBackend) -> PathBuf {
    let nodes = render_nodes();
    let pick = match backend {
        EncoderBackend::Nvenc => {
            if render_node_is_nvidia(drm_render_node) {
                return PathBuf::from(drm_render_node);
            }
            nodes.iter().find(|p| is_nvidia_path(p))
        }
        EncoderBackend::Vaapi | EncoderBackend::Auto => {
            if !render_node_is_nvidia(drm_render_node) {
                return PathBuf::from(drm_render_node);
            }
            nodes.iter().find(|p| !is_nvidia_path(p))
        }
    };
    pick.cloned()
        .unwrap_or_else(|| PathBuf::from(drm_render_node))
}

/// Ordered `(backend, device)` attempts for `requested`.
pub fn encode_candidates(
    requested: EncoderBackend,
    drm_render_node: &str,
) -> Vec<(EncoderBackend, PathBuf)> {
    let nodes = render_nodes();
    let has_nvidia =
        nodes.iter().any(|p| is_nvidia_path(p)) || render_node_is_nvidia(drm_render_node);
    let has_other =
        nodes.iter().any(|p| !is_nvidia_path(p)) || !render_node_is_nvidia(drm_render_node);
    let order: Vec<EncoderBackend> = match requested {
        EncoderBackend::Auto => {
            let first = detect_preferred_backend(drm_render_node);
            let mut v = vec![first];
            match first {
                EncoderBackend::Nvenc if has_other => v.push(EncoderBackend::Vaapi),
                EncoderBackend::Vaapi if has_nvidia => v.push(EncoderBackend::Nvenc),
                _ => {}
            }
            v
        }
        other => vec![other],
    };
    order
        .into_iter()
        .map(|b| (b, preferred_render_node(drm_render_node, b)))
        .collect()
}

fn render_nodes() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(Path::new("/dev/dri")) else {
        return Vec::new();
    };
    let mut nodes: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes
}

fn is_nvidia_path(path: &Path) -> bool {
    render_node_is_nvidia(&path.to_string_lossy())
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

    #[test]
    fn unknown_node_is_vaapi_first() {
        let c = encode_candidates(EncoderBackend::Auto, "/dev/dri/renderD-missing");
        assert_eq!(c.first().map(|(b, _)| *b), Some(EncoderBackend::Vaapi));
        let explicit = encode_candidates(EncoderBackend::Nvenc, "/dev/dri/renderD-missing");
        assert_eq!(explicit.len(), 1);
        assert_eq!(explicit[0].0, EncoderBackend::Nvenc);
    }
}
