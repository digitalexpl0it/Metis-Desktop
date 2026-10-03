//! FFmpeg VAAPI / NVENC encoder fed from LINEAR compositor dmabufs.
//!
//! Frame path: `mmap` the LINEAR XRGB dmabuf (bracketed by `DMA_BUF_IOCTL_SYNC`)
//! → swscale to NV12 → `av_hwframe_transfer_data` into a VAAPI / CUDA surface →
//! encode. There is deliberately no `av_hwframe_map` DRM_PRIME import: mapping a
//! DRM_PRIME frame whose frames context is derived from the destination context
//! is treated by libavutil as an *unmap* and dereferences our descriptor as an
//! internal `HWMapDescriptor` (segfault). A correct zero-copy path would also
//! need a VPP RGB→NV12 conversion stage.
//!
//! This module only ever runs inside the isolated `metis-encode-probe worker`
//! process (see [`crate::process`]) — never inside the compositor.

use std::ffi::CString;
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
        (EncoderBackend::Vaapi | EncoderBackend::Auto, RudpCodec::Hevc) => "hevc_vaapi",
        (EncoderBackend::Vaapi | EncoderBackend::Auto, RudpCodec::H264) => "h264_vaapi",
        (EncoderBackend::Nvenc, RudpCodec::Hevc) => "hevc_nvenc",
        (EncoderBackend::Nvenc, RudpCodec::H264) => "h264_nvenc",
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
    // DRM_FORMAT_MOD_LINEAR == 0. `Invalid` (!0) and tiled layouts are not mmap-safe.
    modifier == 0
}

/// Map target bitrate to a VAAPI CQP quantizer (lower = higher quality).
fn bitrate_kbps_to_vaapi_qp(bitrate_kbps: u32) -> i32 {
    match bitrate_kbps {
        0..=3_999 => 32,
        4_000..=9_999 => 28,
        10_000..=19_999 => 25,
        20_000..=39_999 => 22,
        _ => 20,
    }
}

// linux/dma-buf.h
const DMA_BUF_SYNC_READ: u64 = 1 << 0;
const DMA_BUF_SYNC_START: u64 = 0;
const DMA_BUF_SYNC_END: u64 = 1 << 2;
/// `_IOW('b', 0, struct dma_buf_sync)`.
const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;

#[repr(C)]
struct DmaBufSync {
    flags: u64,
}

fn dma_buf_sync(fd: BorrowedFd<'_>, flags: u64) {
    let arg = DmaBufSync { flags };
    loop {
        // SAFETY: valid fd + correctly sized argument for DMA_BUF_IOCTL_SYNC.
        let rc = unsafe { libc::ioctl(fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC as _, &arg) };
        if rc == 0 {
            return;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            // Not fatal: older kernels / non-dmabuf fds simply skip cache sync.
            tracing::trace!(%err, "metis-encode: DMA_BUF_IOCTL_SYNC failed");
            return;
        }
    }
}

/// Size of the dmabuf behind `fd` (dmabufs support `SEEK_END`), if known.
fn dmabuf_size(fd: BorrowedFd<'_>) -> Option<usize> {
    // SAFETY: lseek on a valid fd; dmabuf offsets are not shared state we rely on.
    let end = unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END) };
    if end <= 0 {
        return None;
    }
    // Restore offset (mmap ignores it, but keep the fd tidy).
    unsafe {
        libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_SET);
    }
    usize::try_from(end).ok()
}

pub struct FfmpegHwEncoder {
    info: EncoderInfo,
    ctx: *mut ffi::AVCodecContext,
    hw_device: *mut ffi::AVBufferRef,
    hw_frames: *mut ffi::AVBufferRef,
    /// Hardware encode surface (VAAPI / CUDA).
    hw_frame: *mut ffi::AVFrame,
    /// Software NV12 staging frame.
    sw_frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    pts: i64,
    pending_seq: u64,
    force_keyframe: bool,
    pending_damage_full: bool,
    pending_damage: Vec<crate::DamageRect>,
    sws: Option<SwsContext>,
    /// (source pixel format, width, height) the cached `sws` was built for.
    sws_key: Option<(Pixel, u32, u32)>,
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
        let name = encoder_codec_name(backend, cfg.codec);
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
                EncoderBackend::Vaapi | EncoderBackend::Auto => {
                    (*ctx).bit_rate = 0;
                    (*ctx).rc_max_rate = 0;
                    (*ctx).global_quality = bitrate_kbps_to_vaapi_qp(cfg.bitrate_kbps);
                }
                EncoderBackend::Nvenc => {
                    (*ctx).bit_rate = i64::from(cfg.bitrate_kbps.saturating_mul(1000));
                }
            }
            (*ctx).gop_size = fps.saturating_mul(2);
            (*ctx).max_b_frames = 0;
            // Encoders require their native hw pixel formats.
            (*ctx).pix_fmt = match backend {
                EncoderBackend::Nvenc => Pixel::CUDA.into(),
                EncoderBackend::Vaapi | EncoderBackend::Auto => Pixel::VAAPI.into(),
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
                EncoderBackend::Vaapi | EncoderBackend::Auto => {
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
                sws: None,
                sws_key: None,
            })
        }
    }

    /// Validate `input` and return (sw pixel format, plane-0 fd, offset, stride, map length).
    fn validate_input<'a>(
        &self,
        input: &EncodeInput<'a>,
    ) -> EncodeResult<(Pixel, BorrowedFd<'a>, usize, usize, usize)> {
        if input.width != self.info.width || input.height != self.info.height {
            return Err(EncodeError::InvalidInput(format!(
                "frame {}x{} does not match encoder {}x{}",
                input.width, input.height, self.info.width, self.info.height
            )));
        }
        if !modifier_is_linear(input.modifier) {
            return Err(EncodeError::InvalidInput(format!(
                "dmabuf modifier 0x{:x} is not LINEAR — refusing CPU read",
                input.modifier
            )));
        }
        let sw_pix = fourcc_to_sw_pixel(input.fourcc).ok_or_else(|| {
            EncodeError::InvalidInput(format!("unsupported dmabuf fourcc 0x{:08x}", input.fourcc))
        })?;
        let fd = *input
            .fds
            .first()
            .ok_or_else(|| EncodeError::InvalidInput("no dmabuf planes".into()))?;
        let offset = input.offsets.first().copied().unwrap_or(0) as usize;
        let stride = input.strides.first().copied().unwrap_or(input.stride) as usize;
        let width = input.width as usize;
        let height = input.height as usize;
        if stride < width.saturating_mul(4) {
            return Err(EncodeError::InvalidInput(format!(
                "stride {stride} too small for {width}px XRGB"
            )));
        }
        let map_len = stride
            .checked_mul(height)
            .and_then(|n| n.checked_add(offset))
            .ok_or_else(|| EncodeError::InvalidInput("frame size overflow".into()))?;
        // Reading past the end of a dmabuf mapping raises SIGBUS — check first.
        if let Some(size) = dmabuf_size(fd)
            && map_len > size
        {
            return Err(EncodeError::InvalidInput(format!(
                "dmabuf too small: need {map_len} bytes, buffer is {size}"
            )));
        }
        Ok((sw_pix, fd, offset, stride, map_len))
    }

    /// mmap the LINEAR dmabuf, swscale to NV12, upload into `hw_frame`.
    unsafe fn upload_frame(&mut self, input: &EncodeInput<'_>) -> EncodeResult<()> {
        let (sw_pix, fd, offset, stride, map_len) = self.validate_input(input)?;
        let width = input.width;
        let height = input.height;
        let line = i32::try_from(stride)
            .map_err(|_| EncodeError::InvalidInput("stride out of range".into()))?;

        if self.sws_key != Some((sw_pix, width, height)) {
            self.sws = None;
            let ctx = SwsContext::get(
                sw_pix,
                width,
                height,
                Pixel::NV12,
                width,
                height,
                SwsFlags::BILINEAR,
            )
            .map_err(|e| EncodeError::Ffmpeg(format!("sws_getContext: {e:?}")))?;
            self.sws = Some(ctx);
            self.sws_key = Some((sw_pix, width, height));
        }

        unsafe {
            let mapped = libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if mapped == libc::MAP_FAILED {
                return Err(EncodeError::Ffmpeg(format!(
                    "mmap dmabuf failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            dma_buf_sync(fd, DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ);

            let result = (|| -> EncodeResult<()> {
                ffi::av_frame_unref(self.sw_frame);
                (*self.sw_frame).format = pixel_as_i32(Pixel::NV12);
                (*self.sw_frame).width = width as i32;
                (*self.sw_frame).height = height as i32;
                check(ffi::av_frame_get_buffer(self.sw_frame, 32))?;

                let mut src_data: [*const u8; 4] = [ptr::null(); 4];
                let mut src_linesize = [0i32; 4];
                src_data[0] = (mapped as *const u8).add(offset);
                src_linesize[0] = line;

                let sws = self
                    .sws
                    .as_mut()
                    .ok_or_else(|| EncodeError::Ffmpeg("sws missing".into()))?;
                let scaled = ffi::sws_scale(
                    sws.as_mut_ptr(),
                    src_data.as_ptr(),
                    src_linesize.as_ptr(),
                    0,
                    height as i32,
                    (*self.sw_frame).data.as_ptr(),
                    (*self.sw_frame).linesize.as_ptr(),
                );
                if scaled <= 0 {
                    return Err(EncodeError::Ffmpeg(format!("sws_scale failed ({scaled})")));
                }
                Ok(())
            })();

            dma_buf_sync(fd, DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ);
            libc::munmap(mapped, map_len);
            result?;

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
        let mut out = Vec::new();
        let tb = unsafe { (*self.ctx).time_base };
        unsafe {
            loop {
                ffi::av_packet_unref(self.packet);
                let err = ffi::avcodec_receive_packet(self.ctx, self.packet);
                if err == ffi::AVERROR(ffi::EAGAIN) || err == ffi::AVERROR_EOF {
                    break;
                }
                check(err)?;
                let size = usize::try_from((*self.packet).size).unwrap_or(0);
                let data_ptr = (*self.packet).data;
                if data_ptr.is_null() || size == 0 {
                    continue;
                }
                let data = std::slice::from_raw_parts(data_ptr, size).to_vec();
                let pts = (*self.packet).pts;
                let pts_us = if tb.den != 0 && pts != ffi::AV_NOPTS_VALUE {
                    pts.saturating_mul(1_000_000)
                        .saturating_mul(i64::from(tb.num))
                        / i64::from(tb.den)
                } else {
                    0
                };
                out.push(EncodedPacket {
                    seq: self.pending_seq,
                    codec: self.info.codec,
                    is_keyframe: ((*self.packet).flags & ffi::AV_PKT_FLAG_KEY) != 0,
                    data,
                    pts_us,
                    damage_full: self.pending_damage_full,
                    damage: self.pending_damage.clone(),
                });
            }
            ffi::av_packet_unref(self.packet);
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
    fn only_linear_is_mmap_safe() {
        assert!(modifier_is_linear(0));
        assert!(!modifier_is_linear(u64::MAX >> 8));
    }

    #[test]
    fn xrgb_maps_to_bgr0() {
        assert_eq!(fourcc_to_sw_pixel(0x3432_5258), Some(Pixel::BGRZ));
        assert_eq!(fourcc_to_sw_pixel(0), None);
    }
}
