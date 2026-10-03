//! Video decode for Metis Remote (RUDP).
//!
//! Decodes H.264 / HEVC access units into packed RGBA8 frames via system
//! FFmpeg. [`open_decoder`] tries VAAPI → NVDEC/cuvid → software, always
//! downloading hardware surfaces to CPU RGBA for GTK.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod detect;
mod ffmpeg_dec;
mod ffmpeg_hw;
mod types;

pub use ffmpeg_dec::FfmpegSoftDecoder;
pub use ffmpeg_hw::FfmpegHwDecoder;
pub use types::{DecodeError, DecodeResult, DecodedFrame, DecoderBackend, DecoderInfo, RudpCodec};

/// Video decoder: push compressed AUs, pull RGBA frames.
pub trait VideoDecoder: Send {
    fn info(&self) -> &DecoderInfo;
    /// Feed one access unit. Returns the latest fully decoded frame, if any.
    fn push_au(&mut self, data: &[u8]) -> DecodeResult<Option<DecodedFrame>>;
    /// Flush delayed frames (end of stream).
    fn flush(&mut self) -> DecodeResult<Vec<DecodedFrame>> {
        Ok(Vec::new())
    }
}

/// Open a decoder with the Auto ladder (VAAPI → NVDEC → software).
pub fn open_decoder(
    codec: RudpCodec,
    width: u32,
    height: u32,
) -> DecodeResult<Box<dyn VideoDecoder>> {
    open_decoder_with(DecoderBackend::Auto, codec, width, height)
}

/// Open a decoder for `backend` (Auto expands to the candidate ladder).
pub fn open_decoder_with(
    backend: DecoderBackend,
    codec: RudpCodec,
    width: u32,
    height: u32,
) -> DecodeResult<Box<dyn VideoDecoder>> {
    let candidates = detect::decode_candidates(backend);
    let mut last_err = DecodeError::Unavailable("no decoder candidates".into());

    for (cand_backend, device) in candidates {
        match cand_backend {
            DecoderBackend::Soft => match FfmpegSoftDecoder::open(codec, width, height) {
                Ok(dec) => {
                    tracing::info!(
                        ?codec,
                        width,
                        height,
                        name = %dec.info().decoder_name,
                        "metis-decode: software decoder ready"
                    );
                    return Ok(Box::new(dec));
                }
                Err(err) => {
                    tracing::warn!(%err, "metis-decode: software decoder open failed");
                    last_err = err;
                }
            },
            DecoderBackend::Vaapi | DecoderBackend::Nvdec => {
                let device_ref = device.as_deref();
                match FfmpegHwDecoder::try_open(cand_backend, device_ref, codec, width, height) {
                    Ok(dec) => {
                        tracing::info!(
                            ?codec,
                            width,
                            height,
                            backend = ?cand_backend,
                            name = %dec.info().decoder_name,
                            device = ?device,
                            "metis-decode: hardware decoder ready"
                        );
                        return Ok(Box::new(dec));
                    }
                    Err(err) => {
                        tracing::info!(
                            backend = ?cand_backend,
                            device = ?device,
                            %err,
                            "metis-decode: hardware decoder unavailable; trying next"
                        );
                        last_err = err;
                    }
                }
            }
            DecoderBackend::Auto => {
                // Candidates are always concrete backends.
                last_err = DecodeError::InvalidInput("unexpected Auto candidate".into());
            }
        }
    }

    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::sync::Once;

    use ffmpeg_next::ffi;

    static INIT: Once = Once::new();

    fn ensure_ffmpeg() {
        INIT.call_once(|| {
            ffmpeg_next::init().ok();
        });
    }

    /// Soft open must work without DRM / HW drivers.
    #[test]
    fn software_open_without_drm() {
        ensure_ffmpeg();
        let dec = open_decoder_with(DecoderBackend::Soft, RudpCodec::H264, 16, 16)
            .expect("soft h264 open");
        assert_eq!(dec.info().backend, DecoderBackend::Soft);
        assert_eq!(dec.info().decoder_name, "h264");
    }

    /// Encode one solid-color frame with libx264 (Annex-B), then decode via
    /// soft path. Skips when the host FFmpeg lacks `libx264`.
    #[test]
    fn software_h264_roundtrip_rgba() {
        ensure_ffmpeg();
        let Some(au) = encode_tiny_h264() else {
            eprintln!("skip: libx264 not available");
            return;
        };
        assert!(!au.is_empty());

        let mut dec = open_decoder_with(DecoderBackend::Soft, RudpCodec::H264, 16, 16)
            .expect("open h264 decoder");
        let frame = dec
            .push_au(&au)
            .expect("push")
            .or_else(|| {
                // Some builds need a flush / second push to emit.
                dec.flush().ok().and_then(|mut v| v.pop())
            })
            .expect("decoded frame");
        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.rgba.len(), 16 * 16 * 4);
        // Non-zero pixels (green-ish solid).
        assert!(frame.rgba.iter().any(|&b| b > 0));
    }

    /// Auto ladder must still yield a usable decoder (soft last).
    #[test]
    fn auto_open_falls_back_to_usable_decoder() {
        ensure_ffmpeg();
        let dec = open_decoder(RudpCodec::H264, 64, 64).expect("auto open");
        assert!(!dec.info().decoder_name.is_empty());
    }

    fn encode_tiny_h264() -> Option<Vec<u8>> {
        unsafe {
            let name = std::ffi::CString::new("libx264").ok()?;
            let codec = ffi::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return None;
            }
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return None;
            }
            (*ctx).width = 16;
            (*ctx).height = 16;
            (*ctx).time_base = ffi::AVRational { num: 1, den: 30 };
            (*ctx).framerate = ffi::AVRational { num: 30, den: 1 };
            (*ctx).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
            (*ctx).gop_size = 1;
            (*ctx).max_b_frames = 0;
            (*ctx).flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;

            let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
            let k = std::ffi::CString::new("preset").ok()?;
            let v = std::ffi::CString::new("ultrafast").ok()?;
            ffi::av_dict_set(&mut opts, k.as_ptr(), v.as_ptr(), 0);
            let k2 = std::ffi::CString::new("tune").ok()?;
            let v2 = std::ffi::CString::new("zerolatency").ok()?;
            ffi::av_dict_set(&mut opts, k2.as_ptr(), v2.as_ptr(), 0);

            if ffi::avcodec_open2(ctx, codec, &mut opts) < 0 {
                ffi::av_dict_free(&mut opts);
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return None;
            }
            ffi::av_dict_free(&mut opts);

            let frame = ffi::av_frame_alloc();
            let packet = ffi::av_packet_alloc();
            if frame.is_null() || packet.is_null() {
                if !frame.is_null() {
                    ffi::av_frame_free(&mut (frame as *mut _));
                }
                if !packet.is_null() {
                    ffi::av_packet_free(&mut (packet as *mut _));
                }
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return None;
            }
            (*frame).format = (*ctx).pix_fmt as i32;
            (*frame).width = 16;
            (*frame).height = 16;
            if ffi::av_frame_get_buffer(frame, 32) < 0 {
                ffi::av_frame_free(&mut (frame as *mut _));
                ffi::av_packet_free(&mut (packet as *mut _));
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return None;
            }
            if ffi::av_frame_make_writable(frame) < 0 {
                ffi::av_frame_free(&mut (frame as *mut _));
                ffi::av_packet_free(&mut (packet as *mut _));
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return None;
            }

            // Y=plane0 solid mid, U/V=128 → roughly green-grey.
            let y_size = ((*frame).linesize[0] * 16) as usize;
            ptr::write_bytes((*frame).data[0], 180, y_size);
            let uv_h = 8;
            let u_size = ((*frame).linesize[1] * uv_h) as usize;
            let v_size = ((*frame).linesize[2] * uv_h) as usize;
            ptr::write_bytes((*frame).data[1], 64, u_size);
            ptr::write_bytes((*frame).data[2], 64, v_size);
            (*frame).pts = 0;

            if ffi::avcodec_send_frame(ctx, frame) < 0 {
                ffi::av_frame_free(&mut (frame as *mut _));
                ffi::av_packet_free(&mut (packet as *mut _));
                ffi::avcodec_free_context(&mut (ctx as *mut _));
                return None;
            }
            // Flush encoder.
            let _ = ffi::avcodec_send_frame(ctx, ptr::null());

            let mut out = Vec::new();
            // Prepend extradata (SPS/PPS) when GLOBAL_HEADER is set.
            if (*ctx).extradata_size > 0 && !(*ctx).extradata.is_null() {
                let n = (*ctx).extradata_size as usize;
                let slice = std::slice::from_raw_parts((*ctx).extradata, n);
                out.extend_from_slice(slice);
            }
            loop {
                let ret = ffi::avcodec_receive_packet(ctx, packet);
                if ret == ffi::AVERROR(ffi::EAGAIN) || ret == ffi::AVERROR_EOF {
                    break;
                }
                if ret < 0 {
                    break;
                }
                let size = (*packet).size as usize;
                let data = std::slice::from_raw_parts((*packet).data, size);
                out.extend_from_slice(data);
                ffi::av_packet_unref(packet);
            }

            ffi::av_frame_free(&mut (frame as *mut _));
            ffi::av_packet_free(&mut (packet as *mut _));
            ffi::avcodec_free_context(&mut (ctx as *mut _));
            if out.is_empty() { None } else { Some(out) }
        }
    }
}
