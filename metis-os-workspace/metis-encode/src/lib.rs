//! Video encode for Metis Remote (RUDP): hardware (VAAPI / NVENC) plus a CPU
//! software AV1 fallback (`libsvtav1` / `libaom-av1`).
//!
//! Compositor frames arrive as LINEAR dmabufs, are read with `DMA_BUF_IOCTL_SYNC`
//! bracketing, converted to NV12 and uploaded to VAAPI (Intel/AMD) or NVENC
//! (NVIDIA) hardware frames, or to YUV420P for the software AV1 encoder.
//!
//! Driver code (libva / CUDA / FFmpeg) can segfault or abort. The compositor
//! therefore only uses [`open_isolated_encoder`], which runs everything in a
//! `metis-encode-probe worker` child process and talks to it over a socketpair.
//! [`open_encoder`] is the in-process path used *inside* that worker.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod convert;
mod detect;
mod ffmpeg_enc;
pub mod ipc;
mod null;
mod probe;
mod process;
mod soft_enc;
mod types;
mod worker;

pub use detect::{
    EncodeAttempt, detect_preferred_backend, encode_candidates, encode_ladder,
    preferred_render_node, resolve_backend,
};
pub use ffmpeg_enc::FfmpegHwEncoder;
pub use null::NullEncoder;
pub use probe::{codec_ladder, probe_binary_path, probe_encoder_open};
pub use process::{ProcessEncoder, open_isolated_encoder};
pub use soft_enc::{FfmpegSoftEncoder, soft_av1_encoder_name, soft_h264_encoder_name};
pub use types::{
    DEFAULT_BITRATE_KBPS, DEFAULT_FPS_HINT, DamageRect, EncodeError, EncodeInput, EncodedPacket,
    EncoderBackend, EncoderConfig, EncoderInfo, RudpCodec,
};
pub use worker::run_worker;

use types::EncodeResult;

/// Hardware encoder: submit dmabuf frames, drain compressed packets.
pub trait HwEncoder: Send {
    fn info(&self) -> &EncoderInfo;
    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()>;
    fn drain(&mut self) -> EncodeResult<Vec<EncodedPacket>>;
    /// Flush delayed packets (end of stream / reconfigure).
    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        self.drain()
    }
    /// Request that the next submitted frame be encoded as a keyframe / IDR.
    fn request_keyframe(&mut self) {}
    /// False when the encoder can no longer produce output (e.g. its worker
    /// process died) and must be replaced.
    fn is_healthy(&self) -> bool {
        true
    }
}

/// Open an encoder **in the calling process** for `cfg` using
/// `drm_render_node` (e.g. `/dev/dri/renderD128`).
///
/// Prefer [`open_isolated_encoder`] from the compositor except Wave 2
/// Performance (`prefer_native_encode`). Each rung of [`encode_ladder`] is
/// probed in a child first so an abort on open doesn't lose the fallback
/// ladder. `Soft` prefers H.264 (`libx264`) then AV1.
pub fn open_encoder(
    cfg: &EncoderConfig,
    drm_render_node: &str,
) -> EncodeResult<Box<dyn HwEncoder>> {
    let mut attempts: Vec<String> = Vec::new();
    for rung in encode_ladder(cfg.backend, cfg.codec, drm_render_node) {
        let device = rung.device.to_string_lossy();
        let mut try_cfg = cfg.clone();
        try_cfg.backend = rung.backend;
        try_cfg.codec = rung.codec;
        let label = format!(
            "{}/{} on {device}",
            backend_name(rung.backend),
            rung.codec.as_str()
        );

        if !probe_encoder_open(&try_cfg, device.as_ref()) {
            tracing::warn!(attempt = %label, "metis-encode: probe failed — skipping");
            attempts.push(format!("{label}: probe failed"));
            continue;
        }

        let opened: EncodeResult<Box<dyn HwEncoder>> = match rung.backend {
            EncoderBackend::Soft => {
                FfmpegSoftEncoder::open(&try_cfg).map(|e| Box::new(e) as Box<dyn HwEncoder>)
            }
            _ => FfmpegHwEncoder::open(&try_cfg, device.as_ref())
                .map(|e| Box::new(e) as Box<dyn HwEncoder>),
        };
        match opened {
            Ok(enc) => {
                if rung.backend == EncoderBackend::Soft {
                    tracing::warn!(
                        attempt = %label,
                        encoder = %enc.info().encoder_name,
                        width = cfg.width,
                        height = cfg.height,
                        "metis-encode: using CPU software AV1 (high CPU load)"
                    );
                } else {
                    tracing::info!(
                        attempt = %label,
                        width = cfg.width,
                        height = cfg.height,
                        "metis-encode: hardware encoder ready"
                    );
                }
                return Ok(enc);
            }
            Err(err) => {
                tracing::warn!(attempt = %label, %err, "metis-encode: open failed after probe");
                attempts.push(format!("{label}: {err}"));
            }
        }
    }

    Err(EncodeError::Unavailable(if attempts.is_empty() {
        "no encoder candidates".into()
    } else {
        format!("no encoder opened ({})", attempts.join("; "))
    }))
}

fn backend_name(backend: EncoderBackend) -> &'static str {
    match backend {
        EncoderBackend::Auto => "auto",
        EncoderBackend::Vaapi => "vaapi",
        EncoderBackend::Nvenc => "nvenc",
        EncoderBackend::Soft => "soft",
    }
}
