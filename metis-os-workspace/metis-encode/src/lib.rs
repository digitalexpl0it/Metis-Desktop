//! Hardware video encode for Metis Remote (RUDP).
//!
//! Compositor frames arrive as LINEAR dmabufs, are read with `DMA_BUF_IOCTL_SYNC`
//! bracketing, converted to NV12 and uploaded to VAAPI (Intel/AMD) or NVENC
//! (NVIDIA) hardware frames.
//!
//! Driver code (libva / CUDA / FFmpeg) can segfault or abort. The compositor
//! therefore only uses [`open_isolated_encoder`], which runs everything in a
//! `metis-encode-probe worker` child process and talks to it over a socketpair.
//! [`open_encoder`] is the in-process path used *inside* that worker.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod detect;
mod ffmpeg_enc;
pub mod ipc;
mod null;
mod probe;
mod process;
mod types;
mod worker;

pub use detect::{
    detect_preferred_backend, encode_candidates, preferred_render_node, resolve_backend,
};
pub use ffmpeg_enc::FfmpegHwEncoder;
pub use null::NullEncoder;
pub use probe::{codec_ladder, probe_binary_path, probe_encoder_open};
pub use process::{ProcessEncoder, open_isolated_encoder};
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

/// Open a hardware encoder **in the calling process** for `cfg` using
/// `drm_render_node` (e.g. `/dev/dri/renderD128`).
///
/// Only call this from the isolated worker; the compositor must use
/// [`open_isolated_encoder`]. Each codec is still probed in a child first so an
/// abort on open doesn't lose the fallback ladder. VAAPI tries H.264 first.
pub fn open_encoder(
    cfg: &EncoderConfig,
    drm_render_node: &str,
) -> EncodeResult<Box<dyn HwEncoder>> {
    let mut attempts: Vec<String> = Vec::new();
    for (backend, device) in encode_candidates(cfg.backend, drm_render_node) {
        let device = device.to_string_lossy();
        for codec in codec_ladder(cfg.codec, backend) {
            let mut try_cfg = cfg.clone();
            try_cfg.backend = backend;
            try_cfg.codec = codec;
            let label = format!("{}/{} on {device}", backend_name(backend), codec.as_str());

            if !probe_encoder_open(&try_cfg, device.as_ref()) {
                tracing::warn!(attempt = %label, "metis-encode: probe failed — skipping");
                attempts.push(format!("{label}: probe failed"));
                continue;
            }

            match FfmpegHwEncoder::open(&try_cfg, device.as_ref()) {
                Ok(enc) => {
                    tracing::info!(
                        attempt = %label,
                        width = cfg.width,
                        height = cfg.height,
                        "metis-encode: hardware encoder ready"
                    );
                    return Ok(Box::new(enc));
                }
                Err(err) => {
                    tracing::warn!(attempt = %label, %err, "metis-encode: open failed after probe");
                    attempts.push(format!("{label}: {err}"));
                }
            }
        }
    }

    Err(EncodeError::Unavailable(if attempts.is_empty() {
        "no hardware encoder candidates".into()
    } else {
        format!("no hardware encoder opened ({})", attempts.join("; "))
    }))
}

fn backend_name(backend: EncoderBackend) -> &'static str {
    match backend {
        EncoderBackend::Auto => "auto",
        EncoderBackend::Vaapi => "vaapi",
        EncoderBackend::Nvenc => "nvenc",
    }
}
