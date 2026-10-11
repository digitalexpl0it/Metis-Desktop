//! FFmpeg software AV1 encoder (CPU) fed from LINEAR compositor dmabufs.
//!
//! Frame path: `mmap` the LINEAR dmabuf (bracketed by `DMA_BUF_IOCTL_SYNC`) →
//! swscale to YUV420P (the pixel format `libsvtav1` / `libaom-av1` accept; they
//! do not take NV12) → encode on the CPU. No GPU or render node is needed, so
//! this is the last-resort AV1 path when `av1_vaapi` / `av1_nvenc` are missing.
//!
//! Like the hardware encoders this only ever runs inside the isolated
//! `metis-encode-probe worker` process — never inside the compositor.

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::Dictionary;
use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;

use crate::HwEncoder;
use crate::convert::{DmabufConverter, check, drain_packets, ensure_ffmpeg};
use crate::types::{
    EncodeError, EncodeInput, EncodeResult, EncodedPacket, EncoderBackend, EncoderConfig,
    EncoderInfo, RudpCodec,
};

/// Software AV1 encoders in preference order. `libsvtav1` is by far the fastest
/// real-time encoder; `libaom-av1` is the portable fallback; `librav1e` is
/// accepted as a last resort; bare `av1` covers distro builds that alias it.
const SOFT_AV1_CANDIDATES: [&str; 4] = ["libsvtav1", "libaom-av1", "librav1e", "av1"];

/// Cap on encoder worker threads (diminishing returns, keeps the desktop responsive).
const MAX_SOFT_THREADS: usize = 8;

/// First name in [`SOFT_AV1_CANDIDATES`] for which `has_encoder` is true.
fn pick_soft_av1_encoder(has_encoder: impl Fn(&str) -> bool) -> Option<&'static str> {
    SOFT_AV1_CANDIDATES
        .iter()
        .copied()
        .find(|name| has_encoder(name))
}

/// Every candidate present in this FFmpeg build, best first.
fn available_soft_av1_encoders() -> Vec<&'static str> {
    ensure_ffmpeg();
    SOFT_AV1_CANDIDATES
        .iter()
        .copied()
        .filter(|name| encoder_exists(name))
        .collect()
}

fn encoder_exists(name: &str) -> bool {
    let Ok(c_name) = CString::new(name) else {
        return false;
    };
    // SAFETY: `c_name` is a valid NUL-terminated string for the call.
    unsafe { !ffi::avcodec_find_encoder_by_name(c_name.as_ptr()).is_null() }
}

/// Best software AV1 encoder compiled into the system FFmpeg, if any.
///
/// Cheap (name lookup only — no encoder is opened); used to skip tests and to
/// give a clear "install libsvtav1" error.
pub fn soft_av1_encoder_name() -> Option<&'static str> {
    ensure_ffmpeg();
    pick_soft_av1_encoder(encoder_exists)
}

/// Target kbps → `AVCodecContext::bit_rate` bits/s without overflow.
fn kbps_to_bits(kbps: u32) -> i64 {
    i64::from(kbps).saturating_mul(1000)
}

fn soft_thread_count() -> i32 {
    let n = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2);
    i32::try_from(n.clamp(1, MAX_SOFT_THREADS)).unwrap_or(2)
}

/// Real-time, low-latency private options for each encoder (unknown keys are
/// ignored by FFmpeg, so a future encoder rename degrades gracefully).
fn encoder_options(name: &str) -> Dictionary<'static> {
    let mut opts = Dictionary::new();
    match name {
        "libsvtav1" => {
            // Preset 11 = real-time (valid on SVT-AV1 1.x–2.x; 12+ gets remapped);
            // low-delay prediction (no look-ahead latency) and true IDRs on
            // keyframe requests so late joiners decode. Low-delay needs CBR (`rc=2`);
            // no `rc_max_rate` either: SVT
            // rejects a max bitrate outside CRF mode.
            opts.set("preset", "11");
            opts.set("forced-idr", "1");
            opts.set("svtav1-params", "pred-struct=1:rc=2");
        }
        "libaom-av1" => {
            opts.set("usage", "realtime");
            opts.set("cpu-used", "8");
            opts.set("lag-in-frames", "0");
            opts.set("row-mt", "1");
        }
        "librav1e" => {
            opts.set("speed", "10");
            opts.set("rav1e-params", "low_latency=true");
        }
        _ => {}
    }
    opts
}

pub struct FfmpegSoftEncoder {
    info: EncoderInfo,
    ctx: *mut ffi::AVCodecContext,
    /// Software YUV420P frame handed to the encoder.
    frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    converter: DmabufConverter,
    pts: i64,
    pending_seq: u64,
    force_keyframe: bool,
    pending_damage_full: bool,
    pending_damage: Vec<crate::DamageRect>,
}

// Safety: the encoder is owned and used by exactly one thread at a time.
unsafe impl Send for FfmpegSoftEncoder {}

impl FfmpegSoftEncoder {
    /// Open a software AV1 encoder for `cfg` (bitrate / fps from the config).
    ///
    /// Only AV1 is supported; `cfg.backend` is ignored. Tries each installed
    /// candidate (`libsvtav1`, `libaom-av1`, …) until one opens.
    pub fn open(cfg: &EncoderConfig) -> EncodeResult<Self> {
        ensure_ffmpeg();
        let cfg = cfg.clone().sanitize();
        if cfg.codec != RudpCodec::Av1 {
            return Err(EncodeError::Unavailable(format!(
                "software encode supports AV1 only (requested {})",
                cfg.codec.as_str()
            )));
        }
        let candidates = available_soft_av1_encoders();
        if candidates.is_empty() {
            return Err(EncodeError::Unavailable(
                "no software AV1 encoder in FFmpeg (install libsvtav1 or libaom-av1)".into(),
            ));
        }
        let mut errors: Vec<String> = Vec::new();
        for name in candidates {
            match Self::open_named(&cfg, name) {
                Ok(enc) => return Ok(enc),
                Err(err) => {
                    tracing::warn!(encoder = name, %err, "metis-encode: soft AV1 open failed");
                    errors.push(format!("{name}: {err}"));
                }
            }
        }
        Err(EncodeError::Unavailable(format!(
            "no software AV1 encoder opened ({})",
            errors.join("; ")
        )))
    }

    fn open_named(cfg: &EncoderConfig, name: &'static str) -> EncodeResult<Self> {
        let c_name = CString::new(name).map_err(|e| EncodeError::InvalidInput(e.to_string()))?;
        let width = i32::try_from(cfg.width)
            .map_err(|_| EncodeError::InvalidInput("width out of range".into()))?;
        let height = i32::try_from(cfg.height)
            .map_err(|_| EncodeError::InvalidInput("height out of range".into()))?;
        let fps = i32::try_from(cfg.fps_hint.clamp(1, 1000)).unwrap_or(60);
        let bit_rate = kbps_to_bits(cfg.bitrate_kbps);

        unsafe {
            let codec = ffi::avcodec_find_encoder_by_name(c_name.as_ptr());
            if codec.is_null() {
                return Err(EncodeError::Unavailable(format!(
                    "FFmpeg encoder `{name}` not found"
                )));
            }
            let mut ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err(EncodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }

            (*ctx).width = width;
            (*ctx).height = height;
            (*ctx).time_base = ffi::AVRational { num: 1, den: fps };
            (*ctx).framerate = ffi::AVRational { num: fps, den: 1 };
            (*ctx).pix_fmt = Pixel::YUV420P.into();
            (*ctx).bit_rate = bit_rate;
            (*ctx).gop_size = fps.saturating_mul(2);
            (*ctx).max_b_frames = 0;
            (*ctx).thread_count = soft_thread_count();
            (*ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;

            // avcodec_open2 may free / replace the AVDictionary and update the
            // caller's pointer; ffmpeg-next's Dictionary Drop would not see that
            // and double-free. Disown before open and free leftovers here.
            let mut opts_ptr = encoder_options(name).disown();
            let open_err = ffi::avcodec_open2(ctx, codec, &mut opts_ptr);
            if !opts_ptr.is_null() {
                ffi::av_dict_free(&mut opts_ptr);
            }
            if open_err < 0 {
                ffi::avcodec_free_context(&mut ctx);
                return Err(EncodeError::Unavailable(format!(
                    "avcodec_open2({name}) failed ({open_err})"
                )));
            }

            let mut frame = ffi::av_frame_alloc();
            let mut packet = ffi::av_packet_alloc();
            if frame.is_null() || packet.is_null() {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut packet);
                }
                if !frame.is_null() {
                    ffi::av_frame_free(&mut frame);
                }
                ffi::avcodec_free_context(&mut ctx);
                return Err(EncodeError::Ffmpeg("av_frame/packet alloc failed".into()));
            }

            Ok(Self {
                info: EncoderInfo {
                    backend: EncoderBackend::Soft,
                    codec: RudpCodec::Av1,
                    encoder_name: name.to_string(),
                    width: cfg.width,
                    height: cfg.height,
                },
                ctx,
                frame,
                packet,
                converter: DmabufConverter::new(Pixel::YUV420P),
                pts: 0,
                pending_seq: 0,
                force_keyframe: true,
                pending_damage_full: true,
                pending_damage: Vec::new(),
            })
        }
    }
}

impl HwEncoder for FfmpegSoftEncoder {
    fn info(&self) -> &EncoderInfo {
        &self.info
    }

    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()> {
        // SAFETY: `ctx`, `frame` and `packet` live as long as `self`.
        unsafe {
            self.converter
                .convert(frame, self.info.width, self.info.height, self.frame)?;
            (*self.frame).pts = self.pts;
            (*self.frame).pict_type = if self.force_keyframe {
                ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
            check(ffi::avcodec_send_frame(self.ctx, self.frame))?;
        }
        self.force_keyframe = false;
        self.pts = self.pts.saturating_add(1);
        self.pending_seq = frame.seq;
        self.pending_damage_full = frame.damage_full;
        self.pending_damage = frame.damage.to_vec();
        Ok(())
    }

    fn drain(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        // SAFETY: `ctx` is opened and `packet` allocated for the encoder's lifetime.
        unsafe {
            drain_packets(
                self.ctx,
                self.packet,
                self.info.codec,
                self.pending_seq,
                self.pending_damage_full,
                &self.pending_damage,
            )
        }
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        unsafe {
            check(ffi::avcodec_send_frame(self.ctx, ptr::null()))?;
        }
        self.drain()
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }
}

impl Drop for FfmpegSoftEncoder {
    fn drop(&mut self) {
        self.converter.reset();
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_svtav1_then_aom() {
        assert_eq!(
            pick_soft_av1_encoder(|n| n == "libsvtav1" || n == "libaom-av1"),
            Some("libsvtav1")
        );
        assert_eq!(
            pick_soft_av1_encoder(|n| n == "libaom-av1" || n == "librav1e"),
            Some("libaom-av1")
        );
        assert_eq!(pick_soft_av1_encoder(|n| n == "av1"), Some("av1"));
        assert_eq!(pick_soft_av1_encoder(|_| false), None);
    }

    #[test]
    fn bitrate_conversion_saturates() {
        assert_eq!(kbps_to_bits(25_000), 25_000_000);
        assert!(kbps_to_bits(u32::MAX) > 0);
    }

    #[test]
    fn thread_count_is_bounded() {
        let n = soft_thread_count();
        assert!((1..=MAX_SOFT_THREADS as i32).contains(&n));
    }

    #[test]
    fn non_av1_codec_is_rejected() {
        let cfg = EncoderConfig {
            backend: EncoderBackend::Soft,
            codec: RudpCodec::H264,
            width: 64,
            height: 64,
            fps_hint: 30,
            bitrate_kbps: 2_000,
        };
        assert!(matches!(
            FfmpegSoftEncoder::open(&cfg),
            Err(EncodeError::Unavailable(_))
        ));
    }

    #[test]
    fn open_when_encoder_present() {
        let Some(name) = soft_av1_encoder_name() else {
            eprintln!("skipped: no software AV1 encoder in FFmpeg");
            return;
        };
        let cfg = EncoderConfig {
            backend: EncoderBackend::Soft,
            codec: RudpCodec::Av1,
            width: 128,
            height: 128,
            fps_hint: 30,
            bitrate_kbps: 2_000,
        };
        match FfmpegSoftEncoder::open(&cfg) {
            Ok(enc) => {
                assert_eq!(enc.info().backend, EncoderBackend::Soft);
                assert_eq!(enc.info().codec, RudpCodec::Av1);
            }
            Err(err) => eprintln!("skipped: {name} present but did not open: {err}"),
        }
    }
}
