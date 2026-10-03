//! Subprocess encode probe — isolates libva/FFmpeg aborts from the compositor.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use crate::types::{EncoderBackend, EncoderConfig, RudpCodec};

/// Locate `metis-encode-probe` next to the running binary, on PATH, or via
/// `METIS_ENCODE_PROBE`.
pub fn probe_binary_path() -> Option<PathBuf> {
    static CACHED: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Ok(p) = std::env::var("METIS_ENCODE_PROBE") {
                let path = PathBuf::from(p);
                if path.is_file() {
                    return Some(path);
                }
            }
            if let Ok(exe) = std::env::current_exe()
                && let Some(dir) = exe.parent()
            {
                let sibling = dir.join("metis-encode-probe");
                if sibling.is_file() {
                    return Some(sibling);
                }
            }
            which("metis-encode-probe")
        })
        .clone()
}

fn which(name: &str) -> Option<PathBuf> {
    let Ok(path_var) = std::env::var("PATH") else {
        return None;
    };
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn backend_arg(backend: EncoderBackend) -> &'static str {
    match backend {
        EncoderBackend::Vaapi | EncoderBackend::Auto => "vaapi",
        EncoderBackend::Nvenc => "nvenc",
    }
}

/// Return true only when a child process successfully opens the encoder.
///
/// On probe-binary missing, timeout, or crash: returns false (safe — skip this
/// codec rather than risk aborting the DRM session).
pub fn probe_encoder_open(cfg: &EncoderConfig, drm_render_node: &str) -> bool {
    let Some(bin) = probe_binary_path() else {
        tracing::warn!(
            "metis-encode: metis-encode-probe not found — refusing in-process open \
             (install the probe next to metis-compositor)"
        );
        return false;
    };

    // Tiny surfaces are enough to exercise avcodec_open2 / VA entrypoints.
    let width = cfg.width.clamp(64, 256);
    let height = cfg.height.clamp(64, 256);

    let mut child = match Command::new(&bin)
        .arg(backend_arg(cfg.backend))
        .arg(cfg.codec.as_str())
        .arg(width.to_string())
        .arg(height.to_string())
        .arg(drm_render_node)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, path = %bin.display(), "metis-encode: failed to spawn probe");
            return false;
        }
    };

    // NVENC/CUDA can stall when the DRM session already owns the GPU; keep this
    // short so Auto can fall through to VAAPI without blocking login.
    let deadline = Duration::from_secs(3);
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return true;
                }
                let stderr = child
                    .stderr
                    .take()
                    .and_then(|mut s| {
                        use std::io::Read;
                        let mut buf = String::new();
                        s.read_to_string(&mut buf).ok()?;
                        Some(buf)
                    })
                    .unwrap_or_default();
                tracing::warn!(
                    ?status,
                    backend = ?cfg.backend,
                    codec = ?cfg.codec,
                    stderr = %stderr.trim(),
                    "metis-encode: probe rejected encoder"
                );
                return false;
            }
            Ok(None) => {
                if started.elapsed() > deadline {
                    tracing::warn!(
                        backend = ?cfg.backend,
                        codec = ?cfg.codec,
                        "metis-encode: probe timed out — killing child"
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                tracing::warn!(%err, "metis-encode: probe wait failed");
                let _ = child.kill();
                return false;
            }
        }
    }
}

/// Codec try order. VAAPI prefers H.264 first — many Intel iGPUs lack HEVC
/// encode entrypoints and libva/ffmpeg have aborted the process on open.
pub fn codec_ladder(preferred: RudpCodec, backend: EncoderBackend) -> [RudpCodec; 2] {
    match backend {
        EncoderBackend::Vaapi | EncoderBackend::Auto => match preferred {
            RudpCodec::H264 => [RudpCodec::H264, RudpCodec::Hevc],
            // Still honour an explicit HEVC preference *second* only after H.264
            // has been probed — never open HEVC first on VAAPI.
            RudpCodec::Hevc => [RudpCodec::H264, RudpCodec::Hevc],
        },
        EncoderBackend::Nvenc => match preferred {
            RudpCodec::Hevc => [RudpCodec::Hevc, RudpCodec::H264],
            RudpCodec::H264 => [RudpCodec::H264, RudpCodec::Hevc],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vaapi_always_h264_first() {
        assert_eq!(
            codec_ladder(RudpCodec::Hevc, EncoderBackend::Vaapi),
            [RudpCodec::H264, RudpCodec::Hevc]
        );
        assert_eq!(
            codec_ladder(RudpCodec::H264, EncoderBackend::Vaapi),
            [RudpCodec::H264, RudpCodec::Hevc]
        );
    }

    #[test]
    fn nvenc_honours_preference() {
        assert_eq!(
            codec_ladder(RudpCodec::Hevc, EncoderBackend::Nvenc),
            [RudpCodec::Hevc, RudpCodec::H264]
        );
    }
}
