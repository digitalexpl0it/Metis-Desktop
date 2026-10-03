//! Build the VAAPI → NVDEC → software candidate ladder from DRM render nodes.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::DecoderBackend;

const PCI_NVIDIA: &str = "0x10de";

/// Ordered `(backend, device)` attempts for `requested`.
///
/// Device is a DRM render-node path for VAAPI, or `None` for CUDA default /
/// software. Auto always ends with [`DecoderBackend::Soft`].
pub fn decode_candidates(requested: DecoderBackend) -> Vec<(DecoderBackend, Option<PathBuf>)> {
    match requested {
        DecoderBackend::Soft => vec![(DecoderBackend::Soft, None)],
        DecoderBackend::Vaapi => vaapi_candidates(),
        DecoderBackend::Nvdec => vec![(DecoderBackend::Nvdec, None)],
        DecoderBackend::Auto => {
            let mut out = vaapi_candidates();
            out.push((DecoderBackend::Nvdec, None));
            out.push((DecoderBackend::Soft, None));
            out
        }
    }
}

fn vaapi_candidates() -> Vec<(DecoderBackend, Option<PathBuf>)> {
    let nodes: Vec<PathBuf> = render_nodes()
        .into_iter()
        .filter(|p| !is_nvidia_path(p))
        .collect();
    if nodes.is_empty() {
        // No usable node discovered — still offer a conventional path so an
        // explicit Vaapi request can fail with a driver/FFmpeg error.
        return vec![(
            DecoderBackend::Vaapi,
            Some(PathBuf::from("/dev/dri/renderD128")),
        )];
    }
    nodes
        .into_iter()
        .map(|p| (DecoderBackend::Vaapi, Some(p)))
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
    let sys = Path::new("/sys/class/drm").join(name).join("device/vendor");
    match fs::read_to_string(&sys) {
        Ok(v) => v.trim().eq_ignore_ascii_case(PCI_NVIDIA),
        Err(_) => {
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
    fn auto_ends_with_soft() {
        let c = decode_candidates(DecoderBackend::Auto);
        assert_eq!(c.last().map(|(b, _)| *b), Some(DecoderBackend::Soft));
        assert!(c.iter().any(|(b, _)| *b == DecoderBackend::Nvdec));
    }

    #[test]
    fn soft_only() {
        let c = decode_candidates(DecoderBackend::Soft);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, DecoderBackend::Soft);
    }

    #[test]
    fn nvdec_uses_default_cuda() {
        let c = decode_candidates(DecoderBackend::Nvdec);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, DecoderBackend::Nvdec);
        assert!(c[0].1.is_none());
    }
}
