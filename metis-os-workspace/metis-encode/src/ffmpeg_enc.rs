//! FFmpeg VAAPI / NVENC encoder fed from LINEAR compositor dmabufs.
//!
//! Upload paths (Phase 24 E):
//! 1. **Zero-copy (preferred):** DRM_PRIME import + VAAPI `scale_vaapi` / CUDA
//!    `scale_cuda` RGB→NV12 into the encoder hardware pool.
//! 2. **Fallback:** `mmap` + `DMA_BUF_IOCTL_SYNC` + swscale NV12 +
//!    `av_hwframe_transfer_data`.
//!
//! There is deliberately no bare `av_hwframe_map` of a DRM_PRIME frame whose
//! frames context is derived only from the encoder destination pool (that path
//! segfaults). Zero-copy uses an owned source DRM_PRIME frames context.
//!
//! Used inside the isolated `metis-encode-probe worker` by default, and
//! in-process under Performance (`prefer_native_encode`).

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::Dictionary;
use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;

use crate::HwEncoder;
use crate::convert::{DmabufConverter, check, drain_packets, ensure_ffmpeg, fourcc_to_sw_pixel};
use crate::types::{
    EncodeError, EncodeInput, EncodeResult, EncodedPacket, EncoderBackend, EncoderConfig,
    EncoderInfo, RudpCodec,
};
use crate::zerocopy::{ZerocopyPreference, zerocopy_preference};
use crate::zerocopy_nvenc::NvencCudaUpload;
use crate::zerocopy_vaapi::VaapiVppUpload;

/// FFmpeg hardware encoder name; `None` for [`EncoderBackend::Soft`] (see `soft_enc`).
fn encoder_codec_name(backend: EncoderBackend, codec: RudpCodec) -> Option<&'static str> {
    Some(match (backend, codec) {
        (EncoderBackend::Soft, _) => return None,
        (EncoderBackend::Vaapi | EncoderBackend::Auto, RudpCodec::Hevc) => "hevc_vaapi",
        (EncoderBackend::Vaapi | EncoderBackend::Auto, RudpCodec::H264) => "h264_vaapi",
        (EncoderBackend::Vaapi | EncoderBackend::Auto, RudpCodec::Av1) => "av1_vaapi",
        (EncoderBackend::Nvenc, RudpCodec::Hevc) => "hevc_nvenc",
        (EncoderBackend::Nvenc, RudpCodec::H264) => "h264_nvenc",
        (EncoderBackend::Nvenc, RudpCodec::Av1) => "av1_nvenc",
    })
}

/// `Soft` never reaches device creation (rejected in [`FfmpegHwEncoder::open`]).
fn hw_device_type(backend: EncoderBackend) -> ffi::AVHWDeviceType {
    match backend {
        EncoderBackend::Nvenc => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
        EncoderBackend::Vaapi | EncoderBackend::Auto | EncoderBackend::Soft => {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
        }
    }
}

/// Map target bitrate to a VAAPI CQP quantizer (lower = higher quality).
///
/// Bands align with [`metis_config::RudpStreamQuality`] bitrates so Low /
/// Medium / High / Performance are visibly distinct (older ladder collapsed
/// High 50 Mbps and Performance 80 Mbps onto the same QP 20).
fn bitrate_kbps_to_vaapi_qp(bitrate_kbps: u32) -> i32 {
    match bitrate_kbps {
        0..=9_999 => 34,       // Low (~8 Mbps)
        10_000..=34_999 => 28, // Medium (~25 Mbps)
        35_000..=64_999 => 22, // High (~50 Mbps)
        _ => 16,               // Performance (~80 Mbps)
    }
}

enum UploadMode {
    Mmap,
    VaapiVpp(VaapiVppUpload),
    NvencCuda(NvencCudaUpload),
}

pub struct FfmpegHwEncoder {
    info: EncoderInfo,
    ctx: *mut ffi::AVCodecContext,
    hw_device: *mut ffi::AVBufferRef,
    hw_frames: *mut ffi::AVBufferRef,
    /// Hardware encode surface (VAAPI / CUDA).
    hw_frame: *mut ffi::AVFrame,
    /// Software NV12 staging frame (mmap path).
    sw_frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    pts: i64,
    pending_seq: u64,
    force_keyframe: bool,
    pending_damage_full: bool,
    pending_damage: Vec<crate::DamageRect>,
    converter: DmabufConverter,
    upload: UploadMode,
}

// Safety: the encoder is owned and used by exactly one thread at a time.
unsafe impl Send for FfmpegHwEncoder {}

impl FfmpegHwEncoder {
    pub fn open(cfg: &EncoderConfig, drm_render_node: &str) -> EncodeResult<Self> {
        ensure_ffmpeg();
        let cfg = cfg.clone().sanitize();
        let backend = match cfg.backend {
            EncoderBackend::Auto => EncoderBackend::Vaapi,
            b => b,
        };
        let Some(name) = encoder_codec_name(backend, cfg.codec) else {
            return Err(EncodeError::Unavailable(
                "software backend is not a hardware encoder (use FfmpegSoftEncoder)".into(),
            ));
        };
        let c_name = CString::new(name).map_err(|e| EncodeError::InvalidInput(e.to_string()))?;
        let width = i32::try_from(cfg.width)
            .map_err(|_| EncodeError::InvalidInput("width out of range".into()))?;
        let height = i32::try_from(cfg.height)
            .map_err(|_| EncodeError::InvalidInput("height out of range".into()))?;
        let fps = i32::try_from(cfg.fps_hint.clamp(1, 1000)).unwrap_or(60);

        unsafe {
            let codec = ffi::avcodec_find_encoder_by_name(c_name.as_ptr());
            if codec.is_null() {
                return Err(EncodeError::Unavailable(format!(
                    "FFmpeg encoder `{name}` not found (install VAAPI/NVENC-capable FFmpeg)"
                )));
            }

            let mut hw_device: *mut ffi::AVBufferRef = ptr::null_mut();
            let device_path = CString::new(drm_render_node)
                .map_err(|e| EncodeError::InvalidInput(e.to_string()))?;
            // VAAPI: pass render node. CUDA/NVENC: let FFmpeg pick the default device.
            let open_path = if backend == EncoderBackend::Nvenc {
                ptr::null()
            } else {
                device_path.as_ptr()
            };
            let err = ffi::av_hwdevice_ctx_create(
                &mut hw_device,
                hw_device_type(backend),
                open_path,
                ptr::null_mut(),
                0,
            );
            if err < 0 {
                return Err(EncodeError::Unavailable(format!(
                    "hwdevice_ctx_create({name}) failed ({err}); check drivers / render node {drm_render_node}"
                )));
            }

            let mut ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }

            (*ctx).width = width;
            (*ctx).height = height;
            (*ctx).time_base = ffi::AVRational { num: 1, den: fps };
            (*ctx).framerate = ffi::AVRational { num: fps, den: 1 };
            // VAAPI on many Intel iGPUs only supports CQP — a bit_rate forces
            // CBR/VBR and the open fails. NVENC uses the bitrate.
            match backend {
                EncoderBackend::Vaapi | EncoderBackend::Auto | EncoderBackend::Soft => {
                    (*ctx).bit_rate = 0;
                    (*ctx).rc_max_rate = 0;
                    (*ctx).global_quality = bitrate_kbps_to_vaapi_qp(cfg.bitrate_kbps);
                }
                EncoderBackend::Nvenc => {
                    (*ctx).bit_rate = i64::from(cfg.bitrate_kbps.saturating_mul(1000));
                }
            }
            let gop_secs = if cfg.gop_secs == 0 {
                2
            } else {
                cfg.gop_secs.min(8)
            };
            let gop_secs = i32::try_from(gop_secs).unwrap_or(2);
            (*ctx).gop_size = fps.saturating_mul(gop_secs);
            (*ctx).max_b_frames = 0;
            // Encoders require their native hw pixel formats.
            (*ctx).pix_fmt = match backend {
                EncoderBackend::Nvenc => Pixel::CUDA.into(),
                EncoderBackend::Vaapi | EncoderBackend::Auto | EncoderBackend::Soft => {
                    Pixel::VAAPI.into()
                }
            };
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device);

            // hw_frames_ctx MUST exist before avcodec_open2 for VAAPI/NVENC.
            let mut hw_frames = ffi::av_hwframe_ctx_alloc(hw_device);
            if hw_frames.is_null() {
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("av_hwframe_ctx_alloc failed".into()));
            }
            {
                let frames_ctx = (*hw_frames).data as *mut ffi::AVHWFramesContext;
                (*frames_ctx).format = (*ctx).pix_fmt;
                (*frames_ctx).sw_format = Pixel::NV12.into();
                (*frames_ctx).width = width;
                (*frames_ctx).height = height;
                (*frames_ctx).initial_pool_size = 8;
            }
            if ffi::av_hwframe_ctx_init(hw_frames) < 0 {
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Unavailable(format!(
                    "av_hwframe_ctx_init failed for {name} on {drm_render_node}"
                )));
            }
            (*ctx).hw_frames_ctx = ffi::av_buffer_ref(hw_frames);

            let mut opts = Dictionary::new();
            match backend {
                EncoderBackend::Vaapi | EncoderBackend::Auto | EncoderBackend::Soft => {
                    opts.set("async_depth", "1");
                    opts.set("rc_mode", "CQP");
                    let qp = bitrate_kbps_to_vaapi_qp(cfg.bitrate_kbps);
                    opts.set("qp", &qp.to_string());
                }
                EncoderBackend::Nvenc => {
                    opts.set("delay", "0");
                    opts.set("zerolatency", "1");
                    opts.set("preset", "p1");
                    opts.set("tune", "ull");
                }
            }

            // avcodec_open2 may free / replace the AVDictionary and update the
            // caller's pointer; ffmpeg-next's Dictionary Drop would not see that
            // and double-free. Disown before open and free leftovers here.
            let mut opts_ptr = opts.disown();
            let open_err = ffi::avcodec_open2(ctx, codec, &mut opts_ptr);
            if !opts_ptr.is_null() {
                ffi::av_dict_free(&mut opts_ptr);
            }
            if open_err < 0 {
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Unavailable(format!(
                    "avcodec_open2({name}) failed ({open_err})"
                )));
            }

            let mut hw_frame = ffi::av_frame_alloc();
            let mut sw_frame = ffi::av_frame_alloc();
            let mut packet = ffi::av_packet_alloc();
            if hw_frame.is_null() || sw_frame.is_null() || packet.is_null() {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut packet);
                }
                if !sw_frame.is_null() {
                    ffi::av_frame_free(&mut sw_frame);
                }
                if !hw_frame.is_null() {
                    ffi::av_frame_free(&mut hw_frame);
                }
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("av_frame/packet alloc failed".into()));
            }

            // Default export fourcc is XRGB8888 → BGR0 / BGRZ.
            let default_sw = fourcc_to_sw_pixel(0x3432_5258).unwrap_or(Pixel::BGRZ);
            let upload = match (zerocopy_preference(), backend) {
                (ZerocopyPreference::Off, _) => UploadMode::Mmap,
                (ZerocopyPreference::Try, EncoderBackend::Nvenc) => {
                    match NvencCudaUpload::open(hw_device, hw_frames, width, height, default_sw) {
                        Ok(z) => UploadMode::NvencCuda(z),
                        Err(err) => {
                            tracing::warn!(
                                %err,
                                "metis-encode: NVENC zero-copy open failed — mmap fallback"
                            );
                            UploadMode::Mmap
                        }
                    }
                }
                (ZerocopyPreference::Try, _) => {
                    match VaapiVppUpload::open(hw_device, hw_frames, width, height, default_sw) {
                        Ok(z) => UploadMode::VaapiVpp(z),
                        Err(err) => {
                            tracing::warn!(
                                %err,
                                "metis-encode: VAAPI zero-copy open failed — mmap fallback"
                            );
                            UploadMode::Mmap
                        }
                    }
                }
            };

            Ok(Self {
                info: EncoderInfo {
                    backend,
                    codec: cfg.codec,
                    encoder_name: name.to_string(),
                    width: cfg.width,
                    height: cfg.height,
                },
                ctx,
                hw_device,
                hw_frames,
                hw_frame,
                sw_frame,
                packet,
                pts: 0,
                pending_seq: 0,
                force_keyframe: true,
                pending_damage_full: true,
                pending_damage: Vec::new(),
                converter: DmabufConverter::new(Pixel::NV12),
                upload,
            })
        }
    }

    unsafe fn upload_frame_mmap(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        unsafe {
            self.converter
                .convert(input, self.info.width, self.info.height, self.sw_frame)?;

            ffi::av_frame_unref(self.hw_frame);
            check(ffi::av_hwframe_get_buffer(self.hw_frames, self.hw_frame, 0))?;
            check(ffi::av_hwframe_transfer_data(
                self.hw_frame,
                self.sw_frame,
                0,
            ))?;
            (*self.hw_frame).pts = self.pts;
            (*self.hw_frame).pict_type = if self.force_keyframe {
                ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
        }
        Ok(())
    }

    /// Upload into `hw_frame` (zero-copy or mmap). On zero-copy submit failure,
    /// permanently fall back to mmap for this encoder instance.
    unsafe fn upload_frame(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        let pts = self.pts;
        let key = self.force_keyframe;
        let hw = self.hw_frame;
        match &mut self.upload {
            UploadMode::Mmap => unsafe { self.upload_frame_mmap(input) },
            UploadMode::VaapiVpp(z) => match unsafe { z.upload(input, hw, pts, key) } {
                Ok(()) => Ok(()),
                Err(err) => {
                    tracing::warn!(
                        %err,
                        "metis-encode: VAAPI zero-copy upload failed — switching to mmap"
                    );
                    self.upload = UploadMode::Mmap;
                    unsafe { self.upload_frame_mmap(input) }
                }
            },
            UploadMode::NvencCuda(z) => match unsafe { z.upload(input, hw, pts, key) } {
                Ok(()) => Ok(()),
                Err(err) => {
                    tracing::warn!(
                        %err,
                        "metis-encode: NVENC zero-copy upload failed — switching to mmap"
                    );
                    self.upload = UploadMode::Mmap;
                    unsafe { self.upload_frame_mmap(input) }
                }
            },
        }
    }
}

impl HwEncoder for FfmpegHwEncoder {
    fn info(&self) -> &EncoderInfo {
        &self.info
    }

    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()> {
        unsafe {
            self.upload_frame(frame)?;
            check(ffi::avcodec_send_frame(self.ctx, self.hw_frame))?;
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

impl Drop for FfmpegHwEncoder {
    fn drop(&mut self) {
        self.converter.reset();
        // Drop upload filters before tearing down device/frames they borrow.
        self.upload = UploadMode::Mmap;
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.sw_frame.is_null() {
                ffi::av_frame_free(&mut self.sw_frame);
            }
            if !self.hw_frame.is_null() {
                ffi::av_frame_free(&mut self.hw_frame);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
            if !self.hw_frames.is_null() {
                ffi::av_buffer_unref(&mut self.hw_frames);
            }
            if !self.hw_device.is_null() {
                ffi::av_buffer_unref(&mut self.hw_device);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vaapi_qp_monotonic() {
        assert!(bitrate_kbps_to_vaapi_qp(1_000) > bitrate_kbps_to_vaapi_qp(50_000));
    }

    #[test]
    fn vaapi_qp_distinct_for_stream_quality_bitrates() {
        let low = bitrate_kbps_to_vaapi_qp(8_000);
        let medium = bitrate_kbps_to_vaapi_qp(25_000);
        let high = bitrate_kbps_to_vaapi_qp(50_000);
        let performance = bitrate_kbps_to_vaapi_qp(80_000);
        assert!(low > medium);
        assert!(medium > high);
        assert!(high > performance);
    }
}
