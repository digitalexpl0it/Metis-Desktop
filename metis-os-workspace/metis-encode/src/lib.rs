//! Hardware video encode for Metis Remote (RUDP).
//!
//! Imports compositor dmabuf plane FDs (no CPU `mmap`) into FFmpeg and encodes
//! via VAAPI (Intel/AMD) or NVENC (NVIDIA).

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod detect;
mod ffmpeg_enc;
mod null;
mod types;

pub use detect::{detect_preferred_backend, resolve_backend};
pub use ffmpeg_enc::FfmpegHwEncoder;
pub use null::NullEncoder;
pub use types::{
    DEFAULT_BITRATE_KBPS, DEFAULT_FPS_HINT, EncodeError, EncodeInput, EncodedPacket,
    EncoderBackend, EncoderConfig, EncoderInfo, RudpCodec,
};

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
}

/// Open a hardware encoder for `cfg` using `drm_render_node` (e.g. `/dev/dri/renderD128`).
///
/// Codec ladder: try preferred [`RudpCodec`] first, then the other of HEVC/H.264.
pub fn open_encoder(
    cfg: &EncoderConfig,
    drm_render_node: &str,
) -> EncodeResult<Box<dyn HwEncoder>> {
    let backend = resolve_backend(cfg.backend, drm_render_node);
    let codecs = match cfg.codec {
        RudpCodec::Hevc => [RudpCodec::Hevc, RudpCodec::H264],
        RudpCodec::H264 => [RudpCodec::H264, RudpCodec::Hevc],
    };

    let mut last_err = None;
    for codec in codecs {
        let mut try_cfg = cfg.clone();
        try_cfg.backend = backend;
        try_cfg.codec = codec;
        match FfmpegHwEncoder::open(&try_cfg, drm_render_node) {
            Ok(enc) => {
                tracing::info!(
                    ?backend,
                    ?codec,
                    width = cfg.width,
                    height = cfg.height,
                    "metis-encode: hardware encoder ready"
                );
                return Ok(Box::new(enc));
            }
            Err(err) => {
                tracing::warn!(?backend, ?codec, %err, "metis-encode: open failed, trying next");
                last_err = Some(err);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| EncodeError::Unavailable("no hardware encoder opened".into())))
}
