//! FFmpeg software encoders (CPU) fed from LINEAR compositor dmabufs.
//!
//! Frame path: `mmap` the LINEAR dmabuf (bracketed by `DMA_BUF_IOCTL_SYNC`) →
//! swscale to YUV420P → encode on the CPU. Supports **H.264 (`libx264`)** for
//! interactive Soft fallback and **AV1** (`libsvtav1` / `libaom-av1`) as a
//! heavier last resort. No GPU or render node is needed.
//!
//! Like the hardware encoders this only ever runs inside the isolated
//! `metis-encode-probe worker` process — never inside the compositor (except
//! Wave 2 Performance native, which does not use Soft).

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
/// Interactive Soft path — prefer zerolatency H.264 over Soft AV1.
const SOFT_H264_CANDIDATES: [&str; 2] = ["libx264", "h264"];

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

/// Best software H.264 encoder name when present (`libx264`).
pub fn soft_h264_encoder_name() -> Option<&'static str> {
    ensure_ffmpeg();
    SOFT_H264_CANDIDATES
        .iter()
        .copied()
        .find(|name| encoder_exists(name))
}

fn available_soft_h264_encoders() -> Vec<&'static str> {
    ensure_ffmpeg();
    SOFT_H264_CANDIDATES
        .iter()
        .copied()
        .filter(|name| encoder_exists(name))
        .collect()
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
        "libx264" | "h264" => {
            opts.set("preset", "ultrafast");
            opts.set("tune", "zerolatency");
        }
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
    /// Open a software encoder for `cfg` (H.264 via `libx264`, or AV1).
    ///
    /// `cfg.backend` is ignored. H.264 is preferred for interactive Soft
    /// fallback; AV1 remains available when requested.
    pub fn open(cfg: &EncoderConfig) -> EncodeResult<Self> {
        ensure_ffmpeg();
        let cfg = cfg.clone().sanitize();
        match cfg.codec {
            RudpCodec::H264 => {
                Self::open_codec(&cfg, RudpCodec::H264, &available_soft_h264_encoders())
            }
            RudpCodec::Av1 => {
                Self::open_codec(&cfg, RudpCodec::Av1, &available_soft_av1_encoders())
            }
            RudpCodec::Hevc => Err(EncodeError::Unavailable(
                "software encode does not support HEVC (use H.264 or AV1)".into(),
            )),
        }
    }

    fn open_codec(
        cfg: &EncoderConfig,
        codec: RudpCodec,
        candidates: &[&'static str],
    ) -> EncodeResult<Self> {
        if candidates.is_empty() {
            let hint = match codec {
                RudpCodec::H264 => "install libx264 / ffmpeg with libx264",
                RudpCodec::Av1 => "install libsvtav1 or libaom-av1",
                RudpCodec::Hevc => "unsupported",
            };
            return Err(EncodeError::Unavailable(format!(
                "no software {} encoder in FFmpeg ({hint})",
                codec.as_str()
            )));
        }
        let mut errors: Vec<String> = Vec::new();
        for name in candidates {
            match Self::open_named(cfg, codec, name) {
                Ok(enc) => return Ok(enc),
                Err(err) => {
                    tracing::warn!(
                        encoder = name,
                        codec = %codec.as_str(),
                        %err,
                        "metis-encode: soft open failed"
                    );
                    errors.push(format!("{name}: {err}"));
                }
            }
        }
        Err(EncodeError::Unavailable(format!(
            "no software {} encoder opened ({})",
            codec.as_str(),
            errors.join("; ")
        )))
    }

    fn open_named(
        cfg: &EncoderConfig,
        rudp_codec: RudpCodec,
        name: &'static str,
    ) -> EncodeResult<Self> {
        let c_name = CString::new(name).map_err(|e| EncodeError::InvalidInput(e.to_string()))?;
        let width = i32::try_from(cfg.width)
            .map_err(|_| EncodeError::InvalidInput("width out of range".into()))?;
        let height = i32::try_from(cfg.height)
            .map_err(|_| EncodeError::InvalidInput("height out of range".into()))?;
        let fps = i32::try_from(cfg.fps_hint.clamp(1, 1000)).unwrap_or(60);
        let bit_rate = kbps_to_bits(cfg.bitrate_kbps);

        unsafe {
            let av_codec = ffi::avcodec_find_encoder_by_name(c_name.as_ptr());
            if av_codec.is_null() {
                return Err(EncodeError::Unavailable(format!(
                    "FFmpeg encoder `{name}` not found"
                )));
            }
            let mut ctx = ffi::avcodec_alloc_context3(av_codec);
            if ctx.is_null() {
                return Err(EncodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }

            (*ctx).width = width;
            (*ctx).height = height;
            (*ctx).time_base = ffi::AVRational { num: 1, den: fps };
            (*ctx).framerate = ffi::AVRational { num: fps, den: 1 };
            (*ctx).pix_fmt = Pixel::YUV420P.into();
            (*ctx).bit_rate = bit_rate;
            let gop_secs = if cfg.gop_secs == 0 {
                2
            } else {
                cfg.gop_secs.min(8)
            };
            let gop_secs = i32::try_from(gop_secs).unwrap_or(2);
            (*ctx).gop_size = fps.saturating_mul(gop_secs);
            (*ctx).max_b_frames = 0;
            (*ctx).thread_count = soft_thread_count();
            (*ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;

            // avcodec_open2 may free / replace the AVDictionary and update the
            // caller's pointer; ffmpeg-next's Dictionary Drop would not see that
            // and double-free. Disown before open and free leftovers here.
            let mut opts_ptr = encoder_options(name).disown();
            let open_err = ffi::avcodec_open2(ctx, av_codec, &mut opts_ptr);
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
                    codec: rudp_codec,
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
    fn hevc_soft_is_rejected() {
        let cfg = EncoderConfig {
            backend: EncoderBackend::Soft,
            codec: RudpCodec::Hevc,
            width: 64,
            height: 64,
            fps_hint: 30,
            bitrate_kbps: 2_000,
            gop_secs: 0,
        };
        assert!(matches!(
            FfmpegSoftEncoder::open(&cfg),
            Err(EncodeError::Unavailable(_))
        ));
    }

    #[test]
    fn soft_h264_opens_when_libx264_present() {
        let Some(_) = soft_h264_encoder_name() else {
            eprintln!("skipped: no software H.264 encoder in FFmpeg");
            return;
        };
        let cfg = EncoderConfig {
            backend: EncoderBackend::Soft,
            codec: RudpCodec::H264,
            width: 128,
            height: 128,
            fps_hint: 30,
            bitrate_kbps: 2_000,
            gop_secs: 1,
        };
        let enc = FfmpegSoftEncoder::open(&cfg).expect("soft H.264 open");
        assert_eq!(enc.info().codec, RudpCodec::H264);
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
            gop_secs: 0,
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
