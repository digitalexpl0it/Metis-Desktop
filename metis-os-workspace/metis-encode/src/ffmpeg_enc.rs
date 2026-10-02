//! FFmpeg VAAPI / NVENC encoder with DRM-PRIME (dmabuf) import.
//!
//! VAAPI (`hevc_vaapi` / `h264_vaapi`) only accepts `AV_PIX_FMT_VAAPI` surfaces —
//! never raw `DRM_PRIME` on the codec context. We import compositor dmabufs by
//! mapping/transferring into a VAAPI NV12 frame (with a Linear mmap + swscale
//! fallback). Sending `DRM_PRIME` into `*_vaapi` previously aborted the process
//! and killed the Metis DRM session.

use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr;
use std::sync::Once;

use ffmpeg_next::Dictionary;
use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling::{context::Context as SwsContext, flag::Flags as SwsFlags};

use crate::HwEncoder;
use crate::types::{
    EncodeError, EncodeInput, EncodeResult, EncodedPacket, EncoderBackend, EncoderConfig,
    EncoderInfo, RudpCodec,
};

static FFMPEG_INIT: Once = Once::new();

fn ensure_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg_next::init().ok();
    });
}

fn ffmpeg_err(code: i32) -> EncodeError {
    EncodeError::Ffmpeg(format!("libav error {code}"))
}

fn check(code: i32) -> EncodeResult<()> {
    if code < 0 {
        Err(ffmpeg_err(code))
    } else {
        Ok(())
    }
}

fn encoder_codec_name(backend: EncoderBackend, codec: RudpCodec) -> &'static str {
    match (backend, codec) {
        (EncoderBackend::Vaapi, RudpCodec::Hevc) => "hevc_vaapi",
        (EncoderBackend::Vaapi, RudpCodec::H264) => "h264_vaapi",
        (EncoderBackend::Nvenc, RudpCodec::Hevc) => "hevc_nvenc",
        (EncoderBackend::Nvenc, RudpCodec::H264) => "h264_nvenc",
        (EncoderBackend::Auto, _) => "hevc_vaapi",
    }
}

fn hw_device_type(backend: EncoderBackend) -> ffi::AVHWDeviceType {
    match backend {
        EncoderBackend::Nvenc => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
        EncoderBackend::Vaapi | EncoderBackend::Auto => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
    }
}

fn pixel_as_i32(pixel: Pixel) -> i32 {
    ffi::AVPixelFormat::from(pixel) as i32
}

/// DRM fourcc → FFmpeg software pixel format (little-endian hosts).
fn fourcc_to_sw_pixel(fourcc: u32) -> Option<Pixel> {
    match fourcc {
        // DRM_FORMAT_XRGB8888 ('XR24') → B,G,R,X in memory (FFmpeg BGR0 / BGRZ)
        0x3432_5258 => Some(Pixel::BGRZ),
        // DRM_FORMAT_ARGB8888 ('AR24')
        0x3432_5241 => Some(Pixel::BGRA),
        // DRM_FORMAT_XBGR8888 ('XB24') → R,G,B,X (FFmpeg RGB0 / RGBZ)
        0x3432_4258 => Some(Pixel::RGBZ),
        // DRM_FORMAT_ABGR8888 ('AB24')
        0x3432_4241 => Some(Pixel::RGBA),
        _ => None,
    }
}

fn modifier_is_linear(modifier: u64) -> bool {
    // DRM_FORMAT_MOD_LINEAR == 0. `Invalid` (!0) is not mmap-safe.
    modifier == 0
}

pub struct FfmpegHwEncoder {
    info: EncoderInfo,
    backend: EncoderBackend,
    ctx: *mut ffi::AVCodecContext,
    hw_device: *mut ffi::AVBufferRef,
    hw_frames: *mut ffi::AVBufferRef,
    /// Derived DRM_PRIME frames context (VAAPI), used for hwmap imports.
    drm_frames: *mut ffi::AVBufferRef,
    /// Staging DRM_PRIME source frame (descriptor + optional derived ctx).
    drm_frame: *mut ffi::AVFrame,
    /// Hardware encode surface (VAAPI / CUDA).
    hw_frame: *mut ffi::AVFrame,
    /// Software NV12 staging for Linear mmap fallback.
    sw_frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    pts: i64,
    pending_seq: u64,
    force_keyframe: bool,
    pending_damage_full: bool,
    pending_damage: Vec<crate::DamageRect>,
    sws: Option<SwsContext>,
}

// Safety: encoder is only used from the dedicated RUDP frame thread.
unsafe impl Send for FfmpegHwEncoder {}

impl FfmpegHwEncoder {
    pub fn open(cfg: &EncoderConfig, drm_render_node: &str) -> EncodeResult<Self> {
        ensure_ffmpeg();
        let cfg = cfg.clone().sanitize();
        let backend = match cfg.backend {
            EncoderBackend::Auto => EncoderBackend::Vaapi,
            b => b,
        };
        let name = encoder_codec_name(backend, cfg.codec);
        let c_name = CString::new(name).map_err(|e| EncodeError::InvalidInput(e.to_string()))?;

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
            let device_type = hw_device_type(backend);
            // VAAPI: pass render node. CUDA/NVENC: let FFmpeg pick the default device.
            let open_path = if backend == EncoderBackend::Nvenc {
                ptr::null()
            } else {
                device_path.as_ptr()
            };
            let err = ffi::av_hwdevice_ctx_create(
                &mut hw_device,
                device_type,
                open_path,
                ptr::null_mut(),
                0,
            );
            if err < 0 {
                return Err(EncodeError::Unavailable(format!(
                    "hwdevice_ctx_create({name}) failed ({err}); check drivers / render node {drm_render_node}"
                )));
            }

            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }

            (*ctx).width = cfg.width as i32;
            (*ctx).height = cfg.height as i32;
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: cfg.fps_hint.max(1) as i32,
            };
            (*ctx).framerate = ffi::AVRational {
                num: cfg.fps_hint.max(1) as i32,
                den: 1,
            };
            (*ctx).bit_rate = i64::from(cfg.bitrate_kbps.saturating_mul(1000));
            (*ctx).gop_size = (cfg.fps_hint.max(1) * 2) as i32;
            (*ctx).max_b_frames = 0;
            // VAAPI/NVENC encoders require their native hw pixel formats — not DRM_PRIME.
            (*ctx).pix_fmt = match backend {
                EncoderBackend::Nvenc => Pixel::CUDA.into(),
                EncoderBackend::Vaapi | EncoderBackend::Auto => Pixel::VAAPI.into(),
            };
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device);

            // hw_frames_ctx MUST exist before avcodec_open2 for VAAPI/NVENC.
            let mut hw_frames = ffi::av_hwframe_ctx_alloc(hw_device);
            if hw_frames.is_null() {
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("av_hwframe_ctx_alloc failed".into()));
            }
            {
                let frames_ctx = (*hw_frames).data as *mut ffi::AVHWFramesContext;
                (*frames_ctx).format = (*ctx).pix_fmt;
                (*frames_ctx).sw_format = Pixel::NV12.into();
                (*frames_ctx).width = cfg.width as i32;
                (*frames_ctx).height = cfg.height as i32;
                (*frames_ctx).initial_pool_size = 8;
            }
            if ffi::av_hwframe_ctx_init(hw_frames) < 0 {
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Unavailable(format!(
                    "av_hwframe_ctx_init failed for {name} on {drm_render_node}"
                )));
            }
            (*ctx).hw_frames_ctx = ffi::av_buffer_ref(hw_frames);

            // Derived DRM_PRIME frames context enables hwmap from compositor dmabufs.
            let mut drm_frames: *mut ffi::AVBufferRef = ptr::null_mut();
            if backend == EncoderBackend::Vaapi {
                let der = ffi::av_hwframe_ctx_create_derived(
                    &mut drm_frames,
                    Pixel::DRM_PRIME.into(),
                    hw_device,
                    hw_frames,
                    0,
                );
                if der < 0 {
                    tracing::warn!(
                        code = der,
                        "metis-encode: DRM_PRIME derived frames ctx unavailable — CPU upload fallback only"
                    );
                    drm_frames = ptr::null_mut();
                }
            }

            let mut opts = Dictionary::new();
            match backend {
                EncoderBackend::Vaapi => {
                    opts.set("async_depth", "1");
                }
                EncoderBackend::Nvenc => {
                    opts.set("delay", "0");
                    opts.set("zerolatency", "1");
                    opts.set("preset", "p1");
                    opts.set("tune", "ull");
                }
                EncoderBackend::Auto => {}
            }

            let mut opts_ptr = opts.as_mut_ptr();
            let open_err = ffi::avcodec_open2(ctx, codec, &mut opts_ptr);
            drop(opts);
            if open_err < 0 {
                if !drm_frames.is_null() {
                    ffi::av_buffer_unref(&mut drm_frames);
                }
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Unavailable(format!(
                    "avcodec_open2({name}) failed ({open_err})"
                )));
            }

            let drm_frame = ffi::av_frame_alloc();
            let hw_frame = ffi::av_frame_alloc();
            let sw_frame = ffi::av_frame_alloc();
            let packet = ffi::av_packet_alloc();
            if drm_frame.is_null() || hw_frame.is_null() || sw_frame.is_null() || packet.is_null() {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut (packet as *mut _));
                }
                if !sw_frame.is_null() {
                    ffi::av_frame_free(&mut (sw_frame as *mut _));
                }
                if !hw_frame.is_null() {
                    ffi::av_frame_free(&mut (hw_frame as *mut _));
                }
                if !drm_frame.is_null() {
                    ffi::av_frame_free(&mut (drm_frame as *mut _));
                }
                if !drm_frames.is_null() {
                    ffi::av_buffer_unref(&mut drm_frames);
                }
                ffi::av_buffer_unref(&mut hw_frames);
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Ffmpeg("av_frame/packet alloc failed".into()));
            }

            Ok(Self {
                info: EncoderInfo {
                    backend,
                    codec: cfg.codec,
                    encoder_name: name.to_string(),
                    width: cfg.width,
                    height: cfg.height,
                },
                backend,
                ctx,
                hw_device,
                hw_frames,
                drm_frames,
                drm_frame,
                hw_frame,
                sw_frame,
                packet,
                pts: 0,
                pending_seq: 0,
                force_keyframe: true,
                pending_damage_full: true,
                pending_damage: Vec::new(),
                sws: None,
            })
        }
    }

    unsafe fn fill_drm_prime_frame(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        unsafe {
            if input.fds.is_empty() {
                return Err(EncodeError::InvalidInput("no dmabuf planes".into()));
            }
            if input.fds.len() != input.offsets.len() || input.fds.len() != input.strides.len() {
                return Err(EncodeError::InvalidInput(
                    "plane fd/offset/stride length mismatch".into(),
                ));
            }
            if input.fds.len() > ffi::AV_DRM_MAX_PLANES as usize {
                return Err(EncodeError::InvalidInput("too many dmabuf planes".into()));
            }

            ffi::av_frame_unref(self.drm_frame);
            (*self.drm_frame).format = pixel_as_i32(Pixel::DRM_PRIME);
            (*self.drm_frame).width = input.width as i32;
            (*self.drm_frame).height = input.height as i32;
            (*self.drm_frame).pts = self.pts;
            if !self.drm_frames.is_null() {
                (*self.drm_frame).hw_frames_ctx = ffi::av_buffer_ref(self.drm_frames);
            }

            let desc = Box::new(ffi::AVDRMFrameDescriptor {
                nb_objects: input.fds.len() as i32,
                objects: std::array::from_fn(|i| {
                    if i < input.fds.len() {
                        ffi::AVDRMObjectDescriptor {
                            fd: input.fds[i].as_raw_fd(),
                            size: 0,
                            format_modifier: input.modifier,
                        }
                    } else {
                        ffi::AVDRMObjectDescriptor {
                            fd: -1,
                            size: 0,
                            format_modifier: 0,
                        }
                    }
                }),
                nb_layers: 1,
                layers: std::array::from_fn(|layer_i| {
                    if layer_i == 0 {
                        let mut planes = [ffi::AVDRMPlaneDescriptor {
                            object_index: 0,
                            offset: 0,
                            pitch: 0,
                        };
                            ffi::AV_DRM_MAX_PLANES as usize];
                        for (i, _) in input.fds.iter().enumerate() {
                            planes[i] = ffi::AVDRMPlaneDescriptor {
                                object_index: i as i32,
                                offset: input.offsets[i] as isize,
                                pitch: input.strides[i] as isize,
                            };
                        }
                        ffi::AVDRMLayerDescriptor {
                            format: input.fourcc,
                            nb_planes: input.fds.len() as i32,
                            planes,
                        }
                    } else {
                        ffi::AVDRMLayerDescriptor {
                            format: 0,
                            nb_planes: 0,
                            planes: [ffi::AVDRMPlaneDescriptor {
                                object_index: 0,
                                offset: 0,
                                pitch: 0,
                            }; ffi::AV_DRM_MAX_PLANES as usize],
                        }
                    }
                }),
            });

            let desc_ptr = Box::into_raw(desc);
            let buf = ffi::av_buffer_create(
                desc_ptr as *mut u8,
                std::mem::size_of::<ffi::AVDRMFrameDescriptor>(),
                Some(free_drm_desc),
                ptr::null_mut(),
                0,
            );
            if buf.is_null() {
                let _ = Box::from_raw(desc_ptr);
                return Err(EncodeError::Ffmpeg("av_buffer_create failed".into()));
            }
            (*self.drm_frame).buf[0] = buf;
            (*self.drm_frame).data[0] = desc_ptr as *mut u8;
            Ok(())
        }
    }

    /// Map/transfer DRM_PRIME → VAAPI/CUDA NV12 surface for the encoder.
    unsafe fn import_to_hw_frame(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        unsafe {
            self.fill_drm_prime_frame(input)?;

            ffi::av_frame_unref(self.hw_frame);
            check(ffi::av_hwframe_get_buffer(self.hw_frames, self.hw_frame, 0))?;
            (*self.hw_frame).pts = self.pts;
            if self.force_keyframe {
                (*self.hw_frame).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_I;
            } else {
                (*self.hw_frame).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_NONE;
            }

            // Prefer zero-copy hwmap; fall back to transfer_data.
            let map_flags = ffi::AV_HWFRAME_MAP_READ as i32;
            let mapped = ffi::av_hwframe_map(self.hw_frame, self.drm_frame, map_flags);
            if mapped < 0 {
                let xfer = ffi::av_hwframe_transfer_data(self.hw_frame, self.drm_frame, 0);
                if xfer < 0 {
                    // Linear BO → CPU NV12 → upload (stable path when hw import fails).
                    if self.backend == EncoderBackend::Vaapi && modifier_is_linear(input.modifier) {
                        self.cpu_upload_nv12(input)?;
                    } else {
                        return Err(EncodeError::Ffmpeg(format!(
                            "DRM_PRIME→hw import failed (map={mapped}, transfer={xfer})"
                        )));
                    }
                }
            }

            if self.force_keyframe {
                self.force_keyframe = false;
            }
            self.pts = self.pts.saturating_add(1);
            self.pending_seq = input.seq;
            self.pending_damage_full = input.damage_full;
            self.pending_damage = input.damage.to_vec();
            Ok(())
        }
    }

    /// mmap a Linear XRGB/ARGB dmabuf, swscale to NV12, upload into `hw_frame`.
    unsafe fn cpu_upload_nv12(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        unsafe {
            let sw_pix = fourcc_to_sw_pixel(input.fourcc).ok_or_else(|| {
                EncodeError::InvalidInput(format!(
                    "unsupported dmabuf fourcc 0x{:08x} for CPU upload",
                    input.fourcc
                ))
            })?;
            let fd = input.fds[0].as_raw_fd();
            let stride = input.strides[0] as usize;
            let height = input.height as usize;
            let width = input.width as usize;
            let map_len = stride
                .checked_mul(height)
                .ok_or_else(|| EncodeError::InvalidInput("frame size overflow".into()))?;
            if map_len == 0 {
                return Err(EncodeError::InvalidInput("empty frame".into()));
            }

            let mapped = libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if mapped == libc::MAP_FAILED {
                return Err(EncodeError::Ffmpeg(format!(
                    "mmap dmabuf failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let upload = (|| -> EncodeResult<()> {
                if self.sws.is_none() {
                    self.sws = Some(
                        SwsContext::get(
                            sw_pix,
                            width as u32,
                            height as u32,
                            Pixel::NV12,
                            width as u32,
                            height as u32,
                            SwsFlags::BILINEAR,
                        )
                        .map_err(|e| EncodeError::Ffmpeg(format!("sws_getContext: {e:?}")))?,
                    );
                }

                ffi::av_frame_unref(self.sw_frame);
                (*self.sw_frame).format = pixel_as_i32(Pixel::NV12);
                (*self.sw_frame).width = width as i32;
                (*self.sw_frame).height = height as i32;
                check(ffi::av_frame_get_buffer(self.sw_frame, 32))?;

                // Source as wrapped BGR0/BGRA plane (no copy of input).
                let mut src_data = [ptr::null_mut(); 8];
                let mut src_linesize = [0i32; 8];
                src_data[0] = mapped as *mut u8;
                src_linesize[0] = stride as i32;

                let sws = self
                    .sws
                    .as_mut()
                    .ok_or_else(|| EncodeError::Ffmpeg("sws missing".into()))?;
                let err = ffi::sws_scale(
                    sws.as_mut_ptr(),
                    src_data.as_ptr() as *const *const u8,
                    src_linesize.as_ptr(),
                    0,
                    height as i32,
                    (*self.sw_frame).data.as_ptr() as *mut *mut u8,
                    (*self.sw_frame).linesize.as_ptr(),
                );
                if err < 0 {
                    return Err(EncodeError::Ffmpeg(format!("sws_scale failed ({err})")));
                }

                ffi::av_frame_unref(self.hw_frame);
                check(ffi::av_hwframe_get_buffer(self.hw_frames, self.hw_frame, 0))?;
                (*self.hw_frame).pts = self.pts;
                check(ffi::av_hwframe_transfer_data(
                    self.hw_frame,
                    self.sw_frame,
                    0,
                ))?;
                Ok(())
            })();

            libc::munmap(mapped, map_len);
            upload
        }
    }
}

unsafe extern "C" fn free_drm_desc(opaque: *mut libc::c_void, data: *mut u8) {
    let _ = opaque;
    if !data.is_null() {
        unsafe {
            let _ = Box::from_raw(data as *mut ffi::AVDRMFrameDescriptor);
        }
    }
}

impl HwEncoder for FfmpegHwEncoder {
    fn info(&self) -> &EncoderInfo {
        &self.info
    }

    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()> {
        if frame.width != self.info.width || frame.height != self.info.height {
            return Err(EncodeError::InvalidInput(format!(
                "frame {}x{} does not match encoder {}x{}",
                frame.width, frame.height, self.info.width, self.info.height
            )));
        }
        unsafe {
            self.import_to_hw_frame(frame)?;
            let err = ffi::avcodec_send_frame(self.ctx, self.hw_frame);
            check(err)?;
        }
        Ok(())
    }

    fn drain(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        let mut out = Vec::new();
        unsafe {
            loop {
                ffi::av_packet_unref(self.packet);
                let err = ffi::avcodec_receive_packet(self.ctx, self.packet);
                if err == ffi::AVERROR(ffi::EAGAIN) || err == ffi::AVERROR_EOF {
                    break;
                }
                check(err)?;
                let size = (*self.packet).size as usize;
                let data_ptr = (*self.packet).data;
                if data_ptr.is_null() || size == 0 {
                    continue;
                }
                let slice = std::slice::from_raw_parts(data_ptr, size);
                let flags = (*self.packet).flags;
                out.push(EncodedPacket {
                    seq: self.pending_seq,
                    codec: self.info.codec,
                    is_keyframe: (flags & ffi::AV_PKT_FLAG_KEY) != 0,
                    data: slice.to_vec(),
                    pts_us: 0,
                    damage_full: self.pending_damage_full,
                    damage: self.pending_damage.clone(),
                });
            }
        }
        for pkt in &mut out {
            unsafe {
                let tb = (*self.ctx).time_base;
                if tb.den != 0 {
                    pkt.pts_us = self.pts.saturating_sub(1) * 1_000_000 * i64::from(tb.num)
                        / i64::from(tb.den);
                }
            }
        }
        Ok(out)
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
        self.sws = None;
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
            if !self.drm_frame.is_null() {
                ffi::av_frame_free(&mut self.drm_frame);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
            if !self.drm_frames.is_null() {
                ffi::av_buffer_unref(&mut self.drm_frames);
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

#[allow(dead_code)]
fn codec_long_name(codec: *const ffi::AVCodec) -> String {
    unsafe {
        if codec.is_null() || (*codec).long_name.is_null() {
            return String::new();
        }
        CStr::from_ptr((*codec).long_name)
            .to_string_lossy()
            .into_owned()
    }
}

#[allow(dead_code)]
fn _borrow_fd_ty(_: BorrowedFd<'_>) {}
