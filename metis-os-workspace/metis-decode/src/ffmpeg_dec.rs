//! FFmpeg software H.264 / HEVC decode → packed RGBA8.

use std::ffi::CString;
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

fn decoder_name(codec: RudpCodec) -> &'static str {
    match codec {
        RudpCodec::H264 => "h264",
        RudpCodec::Hevc => "hevc",
        RudpCodec::Av1 => "av1",
    }
}

fn pixel_as_i32(pixel: Pixel) -> i32 {
    ffi::AVPixelFormat::from(pixel) as i32
}

pub struct FfmpegSoftDecoder {
    info: DecoderInfo,
    ctx: *mut ffi::AVCodecContext,
    packet: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
    sws: *mut ffi::SwsContext,
    rgba_frame: *mut ffi::AVFrame,
    width: i32,
    height: i32,
}

// Safety: decoder is used from a single Tokio/worker thread.
unsafe impl Send for FfmpegSoftDecoder {}

impl FfmpegSoftDecoder {
    pub fn open(codec: RudpCodec, width: u32, height: u32) -> DecodeResult<Self> {
        ensure_ffmpeg();
        let width = width.max(1) as i32;
        let height = height.max(1) as i32;
        let name = decoder_name(codec);
        let c_name = CString::new(name).map_err(|e| DecodeError::InvalidInput(e.to_string()))?;

        unsafe {
            let codec_ptr = ffi::avcodec_find_decoder_by_name(c_name.as_ptr());
            if codec_ptr.is_null() {
                return Err(DecodeError::Unavailable(format!(
                    "FFmpeg decoder `{name}` not found"
                )));
            }

            let ctx = ffi::avcodec_alloc_context3(codec_ptr);
            if ctx.is_null() {
                return Err(DecodeError::Ffmpeg("avcodec_alloc_context3 failed".into()));
            }
            (*ctx).width = width;
            (*ctx).height = height;
            (*ctx).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
            (*ctx).thread_count = 2;

            if let Err(err) = check(ffi::avcodec_open2(ctx, codec_ptr, ptr::null_mut())) {
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return Err(err);
            }

            let packet = ffi::av_packet_alloc();
            let frame = ffi::av_frame_alloc();
            let rgba_frame = ffi::av_frame_alloc();
            if packet.is_null() || frame.is_null() || rgba_frame.is_null() {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut (packet as *mut _));
                }
                if !frame.is_null() {
                    ffi::av_frame_free(&mut (frame as *mut _));
                }
                if !rgba_frame.is_null() {
                    ffi::av_frame_free(&mut (rgba_frame as *mut _));
                }
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return Err(DecodeError::Ffmpeg("alloc frame/packet failed".into()));
            }

            Ok(Self {
                info: DecoderInfo {
                    backend: DecoderBackend::Soft,
                    codec,
                    decoder_name: name.to_string(),
                    width: width as u32,
                    height: height as u32,
                },
                ctx,
                packet,
                frame,
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

    fn take_decoded(&mut self) -> DecodeResult<Option<DecodedFrame>> {
        unsafe {
            loop {
                let ret = ffi::avcodec_receive_frame(self.ctx, self.frame);
                if ret == ffi::AVERROR(ffi::EAGAIN) || ret == ffi::AVERROR_EOF {
                    return Ok(None);
                }
                check(ret)?;

                let w = (*self.frame).width;
                let h = (*self.frame).height;
                let fmt = (*self.frame).format;
                if w <= 0 || h <= 0 {
                    ffi::av_frame_unref(self.frame);
                    continue;
                }

                let src_fmt = std::mem::transmute::<i32, ffi::AVPixelFormat>(fmt);
                self.ensure_sws(src_fmt, w, h)?;

                let ret = ffi::sws_scale(
                    self.sws,
                    (*self.frame).data.as_ptr() as *const *const u8,
                    (*self.frame).linesize.as_ptr(),
                    0,
                    h,
                    (*self.rgba_frame).data.as_ptr(),
                    (*self.rgba_frame).linesize.as_ptr(),
                );
                if ret <= 0 {
                    ffi::av_frame_unref(self.frame);
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

                let pts = (*self.frame).pts;
                let pts_us = if pts == ffi::AV_NOPTS_VALUE { 0 } else { pts };
                ffi::av_frame_unref(self.frame);

                return Ok(Some(DecodedFrame {
                    rgba,
                    width: w as u32,
                    height: h as u32,
                    pts_us,
                }));
            }
        }
    }
}

impl VideoDecoder for FfmpegSoftDecoder {
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

impl Drop for FfmpegSoftDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.sws.is_null() {
                ffi::sws_freeContext(self.sws);
                self.sws = ptr::null_mut();
            }
            if !self.rgba_frame.is_null() {
                ffi::av_frame_free(&mut self.rgba_frame);
            }
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
        }
    }
}
