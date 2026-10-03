//! FFmpeg VAAPI / NVDEC (cuvid) decode → download → packed RGBA8.
//!
//! Hardware surfaces are transferred to system memory with
//! `av_hwframe_transfer_data`, then swscaled to RGBA for GTK
//! `MemoryTexture`. No dmabuf / GL zero-copy in v1.

use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::Once;

use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling::Flags as SwsFlags;
use metis_encode::RudpCodec;

use crate::VideoDecoder;
use crate::types::{DecodeError, DecodeResult, DecodedFrame, DecoderBackend, DecoderInfo};

static FFMPEG_INIT: Once = Once::new();

fn ensure_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg_next::init().ok();
    });
}

fn ffmpeg_err(code: i32) -> DecodeError {
    DecodeError::Ffmpeg(format!("libav error {code}"))
}

fn check(code: i32) -> DecodeResult<()> {
    if code < 0 {
        Err(ffmpeg_err(code))
    } else {
        Ok(())
    }
}

fn hw_decoder_names(backend: DecoderBackend, codec: RudpCodec) -> &'static [&'static str] {
    match (backend, codec) {
        (DecoderBackend::Vaapi, RudpCodec::H264) => &["h264_vaapi"],
        (DecoderBackend::Vaapi, RudpCodec::Hevc) => &["hevc_vaapi"],
        (DecoderBackend::Nvdec, RudpCodec::H264) => &["h264_nvdec", "h264_cuvid"],
        (DecoderBackend::Nvdec, RudpCodec::Hevc) => &["hevc_nvdec", "hevc_cuvid"],
        _ => &[],
    }
}

fn hw_device_type(backend: DecoderBackend) -> DecodeResult<ffi::AVHWDeviceType> {
    match backend {
        DecoderBackend::Vaapi => Ok(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI),
        DecoderBackend::Nvdec => Ok(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA),
        DecoderBackend::Auto | DecoderBackend::Soft => Err(DecodeError::InvalidInput(
            "hw decoder requires Vaapi or Nvdec backend".into(),
        )),
    }
}

fn hw_pix_fmt(backend: DecoderBackend) -> DecodeResult<ffi::AVPixelFormat> {
    match backend {
        DecoderBackend::Vaapi => Ok(Pixel::VAAPI.into()),
        DecoderBackend::Nvdec => Ok(Pixel::CUDA.into()),
        DecoderBackend::Auto | DecoderBackend::Soft => Err(DecodeError::InvalidInput(
            "hw decoder requires Vaapi or Nvdec backend".into(),
        )),
    }
}

fn pixel_as_i32(pixel: Pixel) -> i32 {
    ffi::AVPixelFormat::from(pixel) as i32
}

/// Prefer the hardware pixel format stored in `AVCodecContext.opaque`.
unsafe extern "C" fn get_hw_format(
    ctx: *mut ffi::AVCodecContext,
    pix_fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: FFmpeg invokes this with a live codec context and a
    // `AV_PIX_FMT_NONE`-terminated format list; `opaque` is our boxed format.
    unsafe {
        if ctx.is_null() || pix_fmts.is_null() || (*ctx).opaque.is_null() {
            return ffi::AVPixelFormat::AV_PIX_FMT_NONE;
        }
        let want = *((*ctx).opaque as *const ffi::AVPixelFormat);
        let mut p = pix_fmts;
        while *p != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == want {
                return want;
            }
            p = p.add(1);
        }
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    }
}

pub struct FfmpegHwDecoder {
    info: DecoderInfo,
    ctx: *mut ffi::AVCodecContext,
    hw_device: *mut ffi::AVBufferRef,
    /// Boxed `AVPixelFormat` pointed at by `ctx.opaque` for `get_format`.
    opaque_fmt: *mut ffi::AVPixelFormat,
    packet: *mut ffi::AVPacket,
    hw_frame: *mut ffi::AVFrame,
    sw_frame: *mut ffi::AVFrame,
    sws: *mut ffi::SwsContext,
    rgba_frame: *mut ffi::AVFrame,
    width: i32,
    height: i32,
}

// Safety: decoder is used from a single Tokio/worker thread.
unsafe impl Send for FfmpegHwDecoder {}

impl FfmpegHwDecoder {
    /// Try to open a hardware decoder for `backend` on `device` (VAAPI render
    /// node path, or `None` for CUDA default).
    pub fn try_open(
        backend: DecoderBackend,
        device: Option<&Path>,
        codec: RudpCodec,
        width: u32,
        height: u32,
    ) -> DecodeResult<Self> {
        ensure_ffmpeg();
        let width_i = width.max(1) as i32;
        let height_i = height.max(1) as i32;
        let names = hw_decoder_names(backend, codec);
        if names.is_empty() {
            return Err(DecodeError::InvalidInput(format!(
                "no HW decoder names for {backend:?}/{codec:?}"
            )));
        }

        let mut last_err = DecodeError::Unavailable("no HW decoder candidates".into());
        for name in names {
            match Self::try_open_named(backend, device, codec, name, width_i, height_i) {
                Ok(dec) => return Ok(dec),
                Err(err) => {
                    tracing::debug!(%name, %err, "metis-decode: HW decoder candidate failed");
                    last_err = err;
                }
            }
        }
        Err(last_err)
    }

    fn try_open_named(
        backend: DecoderBackend,
        device: Option<&Path>,
        codec: RudpCodec,
        name: &str,
        width: i32,
        height: i32,
    ) -> DecodeResult<Self> {
        let c_name = CString::new(name).map_err(|e| DecodeError::InvalidInput(e.to_string()))?;
        let device_type = hw_device_type(backend)?;
        let pix_fmt = hw_pix_fmt(backend)?;

        unsafe {
            let codec_ptr = ffi::avcodec_find_decoder_by_name(c_name.as_ptr());
            if codec_ptr.is_null() {
                return Err(DecodeError::Unavailable(format!(
                    "FFmpeg decoder `{name}` not found"
                )));
            }

            let mut hw_device: *mut ffi::AVBufferRef = ptr::null_mut();
            let device_cstr = match (backend, device) {
                (DecoderBackend::Vaapi, Some(path)) => {
                    let s = path
                        .to_str()
                        .ok_or_else(|| DecodeError::InvalidInput("device path not UTF-8".into()))?;
                    Some(CString::new(s).map_err(|e| DecodeError::InvalidInput(e.to_string()))?)
                }
                _ => None,
            };
            let open_path = device_cstr
                .as_ref()
                .map(|c| c.as_ptr())
                .unwrap_or(ptr::null());
            let err = ffi::av_hwdevice_ctx_create(
                &mut hw_device,
                device_type,
                open_path,
                ptr::null_mut(),
                0,
            );
            if err < 0 {
                let hint = device
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "default".into());
                return Err(DecodeError::Unavailable(format!(
                    "hwdevice_ctx_create({name}) failed ({err}); check drivers / device {hint}"
                )));
            }

            let ctx = ffi::avcodec_alloc_context3(codec_ptr);
            if ctx.is_null() {
                ffi::av_buffer_unref(&mut hw_device);
                return Err(DecodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }

            (*ctx).width = width;
            (*ctx).height = height;
            // HW decoders generally dislike frame threading.
            (*ctx).thread_count = 1;
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device);

            let opaque_fmt = Box::into_raw(Box::new(pix_fmt));
            (*ctx).opaque = opaque_fmt as *mut _;
            (*ctx).get_format = Some(get_hw_format);

            if let Err(err) = check(ffi::avcodec_open2(ctx, codec_ptr, ptr::null_mut())) {
                (*ctx).opaque = ptr::null_mut();
                drop(Box::from_raw(opaque_fmt));
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(err);
            }

            let packet = ffi::av_packet_alloc();
            let hw_frame = ffi::av_frame_alloc();
            let sw_frame = ffi::av_frame_alloc();
            let rgba_frame = ffi::av_frame_alloc();
            if packet.is_null() || hw_frame.is_null() || sw_frame.is_null() || rgba_frame.is_null()
            {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut (packet as *mut _));
                }
                if !hw_frame.is_null() {
                    ffi::av_frame_free(&mut (hw_frame as *mut _));
                }
                if !sw_frame.is_null() {
                    ffi::av_frame_free(&mut (sw_frame as *mut _));
                }
                if !rgba_frame.is_null() {
                    ffi::av_frame_free(&mut (rgba_frame as *mut _));
                }
                (*ctx).opaque = ptr::null_mut();
                drop(Box::from_raw(opaque_fmt));
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                ffi::av_buffer_unref(&mut hw_device);
                return Err(DecodeError::Ffmpeg("alloc frame/packet failed".into()));
            }

            Ok(Self {
                info: DecoderInfo {
                    backend,
                    codec,
                    decoder_name: name.to_string(),
                    width: width as u32,
                    height: height as u32,
                },
                ctx,
                hw_device,
                opaque_fmt,
                packet,
                hw_frame,
                sw_frame,
                sws: ptr::null_mut(),
                rgba_frame,
                width,
                height,
            })
        }
    }

    fn ensure_sws(&mut self, src_fmt: ffi::AVPixelFormat, w: i32, h: i32) -> DecodeResult<()> {
        unsafe {
            if !self.sws.is_null() && self.width == w && self.height == h {
                return Ok(());
            }
            if !self.sws.is_null() {
                ffi::sws_freeContext(self.sws);
                self.sws = ptr::null_mut();
            }
            self.width = w;
            self.height = h;
            self.info.width = w as u32;
            self.info.height = h as u32;

            self.sws = ffi::sws_getContext(
                w,
                h,
                src_fmt,
                w,
                h,
                ffi::AVPixelFormat::AV_PIX_FMT_RGBA,
                SwsFlags::BILINEAR.bits(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if self.sws.is_null() {
                return Err(DecodeError::Ffmpeg("sws_getContext failed".into()));
            }

            ffi::av_frame_unref(self.rgba_frame);
            (*self.rgba_frame).format = pixel_as_i32(Pixel::RGBA);
            (*self.rgba_frame).width = w;
            (*self.rgba_frame).height = h;
            check(ffi::av_frame_get_buffer(self.rgba_frame, 32))?;
            Ok(())
        }
    }

    fn pack_rgba_from_sw(&mut self) -> DecodeResult<DecodedFrame> {
        unsafe {
            let w = (*self.sw_frame).width;
            let h = (*self.sw_frame).height;
            if w <= 0 || h <= 0 {
                return Err(DecodeError::Ffmpeg("invalid transferred frame size".into()));
            }
            let fmt = (*self.sw_frame).format;
            let src_fmt = std::mem::transmute::<i32, ffi::AVPixelFormat>(fmt);
            self.ensure_sws(src_fmt, w, h)?;

            let ret = ffi::sws_scale(
                self.sws,
                (*self.sw_frame).data.as_ptr() as *const *const u8,
                (*self.sw_frame).linesize.as_ptr(),
                0,
                h,
                (*self.rgba_frame).data.as_ptr(),
                (*self.rgba_frame).linesize.as_ptr(),
            );
            if ret <= 0 {
                return Err(DecodeError::Ffmpeg(format!("sws_scale failed ({ret})")));
            }

            let stride = (*self.rgba_frame).linesize[0] as usize;
            let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
            let src = (*self.rgba_frame).data[0];
            for row in 0..h as usize {
                let src_off = row * stride;
                let dst_off = row * (w as usize) * 4;
                ptr::copy_nonoverlapping(
                    src.add(src_off),
                    rgba.as_mut_ptr().add(dst_off),
                    (w as usize) * 4,
                );
            }

            let pts = (*self.hw_frame).pts;
            let pts_us = if pts == ffi::AV_NOPTS_VALUE { 0 } else { pts };

            Ok(DecodedFrame {
                rgba,
                width: w as u32,
                height: h as u32,
                pts_us,
            })
        }
    }

    fn take_decoded(&mut self) -> DecodeResult<Option<DecodedFrame>> {
        unsafe {
            let ret = ffi::avcodec_receive_frame(self.ctx, self.hw_frame);
            if ret == ffi::AVERROR(ffi::EAGAIN) || ret == ffi::AVERROR_EOF {
                return Ok(None);
            }
            check(ret)?;

            ffi::av_frame_unref(self.sw_frame);
            let xfer = ffi::av_hwframe_transfer_data(self.sw_frame, self.hw_frame, 0);
            if xfer < 0 {
                ffi::av_frame_unref(self.hw_frame);
                return Err(DecodeError::Ffmpeg(format!(
                    "av_hwframe_transfer_data failed ({xfer})"
                )));
            }

            let frame = self.pack_rgba_from_sw();
            ffi::av_frame_unref(self.sw_frame);
            ffi::av_frame_unref(self.hw_frame);
            Ok(Some(frame?))
        }
    }
}

impl VideoDecoder for FfmpegHwDecoder {
    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn push_au(&mut self, data: &[u8]) -> DecodeResult<Option<DecodedFrame>> {
        if data.is_empty() {
            return Ok(None);
        }
        unsafe {
            ffi::av_packet_unref(self.packet);
            check(ffi::av_new_packet(self.packet, data.len() as i32))?;
            ptr::copy_nonoverlapping(data.as_ptr(), (*self.packet).data, data.len());
            (*self.packet).pts = ffi::AV_NOPTS_VALUE;

            let send = ffi::avcodec_send_packet(self.ctx, self.packet);
            if send == ffi::AVERROR(ffi::EAGAIN) {
                let _ = self.take_decoded()?;
                check(ffi::avcodec_send_packet(self.ctx, self.packet))?;
            } else {
                check(send)?;
            }
            self.take_decoded()
        }
    }

    fn flush(&mut self) -> DecodeResult<Vec<DecodedFrame>> {
        unsafe {
            check(ffi::avcodec_send_packet(self.ctx, ptr::null()))?;
        }
        let mut out = Vec::new();
        while let Some(frame) = self.take_decoded()? {
            out.push(frame);
        }
        Ok(out)
    }
}

impl Drop for FfmpegHwDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.sws.is_null() {
                ffi::sws_freeContext(self.sws);
                self.sws = ptr::null_mut();
            }
            if !self.rgba_frame.is_null() {
                ffi::av_frame_free(&mut self.rgba_frame);
            }
            if !self.sw_frame.is_null() {
                ffi::av_frame_free(&mut self.sw_frame);
            }
            if !self.hw_frame.is_null() {
                ffi::av_frame_free(&mut self.hw_frame);
            }
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.ctx.is_null() {
                (*self.ctx).opaque = ptr::null_mut();
                (*self.ctx).get_format = None;
                ffi::avcodec_free_context(&mut self.ctx);
            }
            if !self.opaque_fmt.is_null() {
                drop(Box::from_raw(self.opaque_fmt));
                self.opaque_fmt = ptr::null_mut();
            }
            if !self.hw_device.is_null() {
                ffi::av_buffer_unref(&mut self.hw_device);
            }
        }
    }
}
