//! Pick VAAPI vs NVENC (and the device for each) from DRM render nodes.
//!
//! Frames reach the encoder as CPU-readable LINEAR buffers, so the encode GPU
//! does not have to be the compositor's render GPU. `Auto` therefore tries the
//! render GPU's native encoder first and falls back to the other vendor.
//! [`encode_ladder`] flattens that into the ordered `(backend, codec)` attempts
//! the worker probes, including the software AV1 rung.

use std::fs;
use std::path::{Path, PathBuf};

use crate::probe::codec_ladder;
use crate::types::{EncoderBackend, RudpCodec};

const PCI_NVIDIA: &str = "0x10de";

/// Resolve `Auto` from the render node vendor; explicit backends (including
/// `Soft`) pass through.
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
        // CPU encode has no device; keep the caller's node for logging only.
        EncoderBackend::Soft => return PathBuf::from(drm_render_node),
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
        // `Soft` yields a single CPU attempt (device is unused).
        other => vec![other],
    };
    order
        .into_iter()
        .map(|b| (b, preferred_render_node(drm_render_node, b)))
        .collect()
}

/// One rung of the encode ladder: try `codec` on `backend` using `device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeAttempt {
    pub backend: EncoderBackend,
    pub codec: RudpCodec,
    /// DRM render node (VAAPI / NVENC); unused by `Soft`.
    pub device: PathBuf,
}

/// Ordered encode attempts for `requested` / `preferred`.
///
/// - `Soft`: software AV1 only (never falls back to hardware).
/// - `Vaapi` / `Nvenc`: that backend's [`codec_ladder`].
/// - `Auto` + preferred AV1: hardware AV1 on every candidate backend, then
///   software AV1, then the remaining hardware H.264/HEVC ladder.
/// - `Auto` otherwise: hardware ladders only (software AV1 is CPU-heavy and
///   never chosen unless AV1 was asked for or `Soft` was selected explicitly).
pub fn encode_ladder(
    requested: EncoderBackend,
    preferred: RudpCodec,
    drm_render_node: &str,
) -> Vec<EncodeAttempt> {
    build_ladder(
        requested,
        preferred,
        &encode_candidates(requested, drm_render_node),
        drm_render_node,
    )
}

/// Pure ladder construction over pre-resolved `candidates` (unit-testable).
fn build_ladder(
    requested: EncoderBackend,
    preferred: RudpCodec,
    candidates: &[(EncoderBackend, PathBuf)],
    drm_render_node: &str,
) -> Vec<EncodeAttempt> {
    let soft = EncodeAttempt {
        backend: EncoderBackend::Soft,
        codec: RudpCodec::Av1,
        device: PathBuf::from(drm_render_node),
    };
    if requested == EncoderBackend::Soft {
        return vec![soft];
    }

    let hw: Vec<&(EncoderBackend, PathBuf)> = candidates
        .iter()
        .filter(|(b, _)| *b != EncoderBackend::Soft)
        .collect();
    let av1_first = requested == EncoderBackend::Auto && preferred == RudpCodec::Av1;

    let mut out = Vec::new();
    if av1_first {
        for (backend, device) in &hw {
            out.push(EncodeAttempt {
                backend: *backend,
                codec: RudpCodec::Av1,
                device: device.clone(),
            });
        }
        out.push(soft);
    }
    for (backend, device) in &hw {
        for codec in codec_ladder(preferred, *backend) {
            if av1_first && codec == RudpCodec::Av1 {
                continue; // already tried above
            }
            out.push(EncodeAttempt {
                backend: *backend,
                codec,
                device: device.clone(),
            });
        }
    }
    out
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

    fn node(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    fn pairs(l: &[EncodeAttempt]) -> Vec<(EncoderBackend, RudpCodec)> {
        l.iter().map(|a| (a.backend, a.codec)).collect()
    }

    #[test]
    fn explicit_soft_is_soft_av1_only() {
        let cands = [(EncoderBackend::Vaapi, node("/dev/dri/renderD128"))];
        for preferred in [RudpCodec::H264, RudpCodec::Hevc, RudpCodec::Av1] {
            let l = build_ladder(
                EncoderBackend::Soft,
                preferred,
                &cands,
                "/dev/dri/renderD128",
            );
            assert_eq!(pairs(&l), vec![(EncoderBackend::Soft, RudpCodec::Av1)]);
        }
        let l = encode_ladder(
            EncoderBackend::Soft,
            RudpCodec::H264,
            "/dev/dri/renderD-missing",
        );
        assert_eq!(pairs(&l), vec![(EncoderBackend::Soft, RudpCodec::Av1)]);
    }

    #[test]
    fn auto_av1_tries_hw_av1_then_soft_then_hw_h264() {
        let cands = [(EncoderBackend::Vaapi, node("/dev/dri/renderD128"))];
        let l = build_ladder(
            EncoderBackend::Auto,
            RudpCodec::Av1,
            &cands,
            "/dev/dri/renderD128",
        );
        assert_eq!(
            pairs(&l),
            vec![
                (EncoderBackend::Vaapi, RudpCodec::Av1),
                (EncoderBackend::Soft, RudpCodec::Av1),
                (EncoderBackend::Vaapi, RudpCodec::H264),
                (EncoderBackend::Vaapi, RudpCodec::Hevc),
            ]
        );
    }

    #[test]
    fn auto_av1_dual_gpu_tries_all_hw_av1_before_soft() {
        let cands = [
            (EncoderBackend::Nvenc, node("/dev/dri/renderD129")),
            (EncoderBackend::Vaapi, node("/dev/dri/renderD128")),
        ];
        let l = build_ladder(
            EncoderBackend::Auto,
            RudpCodec::Av1,
            &cands,
            "/dev/dri/renderD129",
        );
        let p = pairs(&l);
        assert_eq!(p[0], (EncoderBackend::Nvenc, RudpCodec::Av1));
        assert_eq!(p[1], (EncoderBackend::Vaapi, RudpCodec::Av1));
        assert_eq!(p[2], (EncoderBackend::Soft, RudpCodec::Av1));
        assert!(
            p[3..]
                .iter()
                .all(|(b, c)| *b != EncoderBackend::Soft && *c != RudpCodec::Av1)
        );
        assert_eq!(p.len(), 3 + 4);
    }

    #[test]
    fn auto_non_av1_never_uses_soft() {
        let cands = [(EncoderBackend::Vaapi, node("/dev/dri/renderD128"))];
        for preferred in [RudpCodec::H264, RudpCodec::Hevc] {
            let l = build_ladder(
                EncoderBackend::Auto,
                preferred,
                &cands,
                "/dev/dri/renderD128",
            );
            assert!(l.iter().all(|a| a.backend != EncoderBackend::Soft));
            assert_eq!(l[0].codec, RudpCodec::H264);
        }
    }

    #[test]
    fn explicit_hw_never_uses_soft_even_for_av1() {
        let cands = [(EncoderBackend::Nvenc, node("/dev/dri/renderD129"))];
        let l = build_ladder(
            EncoderBackend::Nvenc,
            RudpCodec::Av1,
            &cands,
            "/dev/dri/renderD129",
        );
        assert_eq!(
            pairs(&l),
            vec![
                (EncoderBackend::Nvenc, RudpCodec::Av1),
                (EncoderBackend::Nvenc, RudpCodec::H264),
                (EncoderBackend::Nvenc, RudpCodec::Hevc),
            ]
        );
    }

    #[test]
    fn soft_resolves_and_keeps_node() {
        assert_eq!(
            resolve_backend(EncoderBackend::Soft, "/dev/dri/renderD128"),
            EncoderBackend::Soft
        );
        assert_eq!(
            preferred_render_node("/dev/dri/renderD128", EncoderBackend::Soft),
            PathBuf::from("/dev/dri/renderD128")
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
