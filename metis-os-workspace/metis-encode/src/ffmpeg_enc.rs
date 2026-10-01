//! FFmpeg VAAPI / NVENC encoder with DRM-PRIME (dmabuf) import — no CPU mmap.

use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr;
use std::sync::Once;

use ffmpeg_next::Dictionary;
use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;

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

pub struct FfmpegHwEncoder {
    info: EncoderInfo,
    ctx: *mut ffi::AVCodecContext,
    hw_device: *mut ffi::AVBufferRef,
    hw_frames: *mut ffi::AVBufferRef,
    frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    pts: i64,
    pending_seq: u64,
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
            // VAAPI: pass render node. CUDA/NVENC: let FFmpeg pick the default device
            // (drm path is still used for DRM_PRIME → CUDA mapping when supported).
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
                    "hwdevice_ctx_create({name:?}) failed ({err}); check drivers / render node {drm_render_node}"
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
            (*ctx).pix_fmt = Pixel::DRM_PRIME.into();
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device);

            // Low-latency hints (best-effort; ignore missing options).
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
            // Dictionary consumed / ownership transferred awkwardly — drop opts after.
            drop(opts);
            if open_err < 0 {
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(EncodeError::Unavailable(format!(
                    "avcodec_open2({name}) failed ({open_err})"
                )));
            }

            // Optional hw_frames_ctx for DRM_PRIME pools (VAAPI). Best-effort.
            let mut hw_frames: *mut ffi::AVBufferRef = ptr::null_mut();
            if backend == EncoderBackend::Vaapi {
                hw_frames = ffi::av_hwframe_ctx_alloc(hw_device);
                if !hw_frames.is_null() {
                    let frames_ctx = (*hw_frames).data as *mut ffi::AVHWFramesContext;
                    (*frames_ctx).format = Pixel::VAAPI.into();
                    (*frames_ctx).sw_format = Pixel::NV12.into();
                    (*frames_ctx).width = cfg.width as i32;
                    (*frames_ctx).height = cfg.height as i32;
                    (*frames_ctx).initial_pool_size = 4;
                    if ffi::av_hwframe_ctx_init(hw_frames) >= 0 {
                        (*ctx).hw_frames_ctx = ffi::av_buffer_ref(hw_frames);
                    } else {
                        ffi::av_buffer_unref(&mut hw_frames);
                        hw_frames = ptr::null_mut();
                    }
                }
            }

            let frame = ffi::av_frame_alloc();
            let packet = ffi::av_packet_alloc();
            if frame.is_null() || packet.is_null() {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut (packet as *mut _));
                }
                if !frame.is_null() {
                    ffi::av_frame_free(&mut (frame as *mut _));
                }
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                if !hw_frames.is_null() {
                    ffi::av_buffer_unref(&mut hw_frames);
                }
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
                ctx,
                hw_device,
                hw_frames,
                frame,
                packet,
                pts: 0,
                pending_seq: 0,
            })
        }
    }

    unsafe fn fill_drm_prime_frame(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        // SAFETY: caller guarantees `self.frame` is a live AVFrame from open().
        // EncodeInput FDs must remain valid until avcodec_send_frame returns.
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

            ffi::av_frame_unref(self.frame);
            (*self.frame).format = pixel_as_i32(Pixel::DRM_PRIME);
            (*self.frame).width = input.width as i32;
            (*self.frame).height = input.height as i32;
            (*self.frame).pts = self.pts;
            self.pts = self.pts.saturating_add(1);
            self.pending_seq = input.seq;

            // Heap-allocate descriptor; FFmpeg frees via buffer callback.
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
            (*self.frame).buf[0] = buf;
            (*self.frame).data[0] = desc_ptr as *mut u8;
            Ok(())
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
            self.fill_drm_prime_frame(frame)?;
            let err = ffi::avcodec_send_frame(self.ctx, self.frame);
            // Frame holds borrowed FDs only for the duration of send_frame when
            // the encoder copies / maps asynchronously; keep EncodeInput alive
            // until submit returns (caller holds ExportedFrame).
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
                    pts_us: (*self.packet).pts.saturating_mul(1_000_000)
                        / i64::from(self.info.width.max(1)), // rough; refined later
                });
            }
        }
        // Fix pts: use encoder time_base.
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
}

impl Drop for FfmpegHwEncoder {
    fn drop(&mut self) {
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

// Silence unused import in some ffmpeg builds.
#[allow(dead_code)]
fn _borrow_fd_ty(_: BorrowedFd<'_>) {}
