//! Shared FFmpeg plumbing for the hardware and software encoders.
//!
//! Frame path: `mmap` the LINEAR dmabuf (bracketed by `DMA_BUF_IOCTL_SYNC`) →
//! swscale into a caller-provided CPU frame (NV12 for VAAPI / NVENC upload,
//! YUV420P for software AV1) and packet draining.

use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr;
use std::sync::Once;

use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling::{context::Context as SwsContext, flag::Flags as SwsFlags};

use crate::DamageRect;
use crate::types::{EncodeError, EncodeInput, EncodeResult, EncodedPacket, RudpCodec};

static FFMPEG_INIT: Once = Once::new();

pub(crate) fn ensure_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg_next::init().ok();
    });
}

pub(crate) fn ffmpeg_err(code: i32) -> EncodeError {
    EncodeError::Ffmpeg(format!("libav error {code}"))
}

pub(crate) fn check(code: i32) -> EncodeResult<()> {
    if code < 0 {
        Err(ffmpeg_err(code))
    } else {
        Ok(())
    }
}

pub(crate) fn pixel_as_i32(pixel: Pixel) -> i32 {
    ffi::AVPixelFormat::from(pixel) as i32
}

/// DRM fourcc → FFmpeg software pixel format (little-endian hosts).
pub(crate) fn fourcc_to_sw_pixel(fourcc: u32) -> Option<Pixel> {
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

pub(crate) fn modifier_is_linear(modifier: u64) -> bool {
    // DRM_FORMAT_MOD_LINEAR == 0. `Invalid` (!0) and tiled layouts are not mmap-safe.
    modifier == 0
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

/// Validate `input` and return (sw pixel format, plane-0 fd, offset, stride, map length).
fn validate_input<'a>(
    input: &EncodeInput<'a>,
    enc_width: u32,
    enc_height: u32,
) -> EncodeResult<(Pixel, BorrowedFd<'a>, usize, usize, usize)> {
    if input.width != enc_width || input.height != enc_height {
        return Err(EncodeError::InvalidInput(format!(
            "frame {}x{} does not match encoder {enc_width}x{enc_height}",
            input.width, input.height,
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

/// Reads LINEAR dmabufs through the CPU and converts them to `dst` (NV12 or
/// YUV420P) with swscale. The scaler is cached per (source format, size).
pub(crate) struct DmabufConverter {
    dst: Pixel,
    sws: Option<SwsContext>,
    /// (source pixel format, width, height) the cached `sws` was built for.
    sws_key: Option<(Pixel, u32, u32)>,
}

impl DmabufConverter {
    pub(crate) fn new(dst: Pixel) -> Self {
        Self {
            dst,
            sws: None,
            sws_key: None,
        }
    }

    /// Release the cached scaler (call before tearing down libav contexts).
    pub(crate) fn reset(&mut self) {
        self.sws = None;
        self.sws_key = None;
    }

    /// mmap the LINEAR dmabuf and swscale it into `dst_frame` (re-allocated here).
    ///
    /// # Safety
    /// `dst_frame` must be a valid, exclusively owned `AVFrame`.
    pub(crate) unsafe fn convert(
        &mut self,
        input: &EncodeInput<'_>,
        enc_width: u32,
        enc_height: u32,
        dst_frame: *mut ffi::AVFrame,
    ) -> EncodeResult<()> {
        let (sw_pix, fd, offset, stride, map_len) = validate_input(input, enc_width, enc_height)?;
        let width = input.width;
        let height = input.height;
        let line = i32::try_from(stride)
            .map_err(|_| EncodeError::InvalidInput("stride out of range".into()))?;
        let w_i32 = i32::try_from(width)
            .map_err(|_| EncodeError::InvalidInput("width out of range".into()))?;
        let h_i32 = i32::try_from(height)
            .map_err(|_| EncodeError::InvalidInput("height out of range".into()))?;

        if self.sws_key != Some((sw_pix, width, height)) {
            self.sws = None;
            let ctx = SwsContext::get(
                sw_pix,
                width,
                height,
                self.dst,
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
                ffi::av_frame_unref(dst_frame);
                (*dst_frame).format = pixel_as_i32(self.dst);
                (*dst_frame).width = w_i32;
                (*dst_frame).height = h_i32;
                check(ffi::av_frame_get_buffer(dst_frame, 32))?;

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
                    h_i32,
                    (*dst_frame).data.as_ptr(),
                    (*dst_frame).linesize.as_ptr(),
                );
                if scaled <= 0 {
                    return Err(EncodeError::Ffmpeg(format!("sws_scale failed ({scaled})")));
                }
                Ok(())
            })();

            dma_buf_sync(fd, DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ);
            libc::munmap(mapped, map_len);
            result
        }
    }
}

/// Receive every ready packet from `ctx` (stops on EAGAIN / EOF).
///
/// # Safety
/// `ctx` must be an opened `AVCodecContext` and `packet` a valid `AVPacket`.
pub(crate) unsafe fn drain_packets(
    ctx: *mut ffi::AVCodecContext,
    packet: *mut ffi::AVPacket,
    codec: RudpCodec,
    seq: u64,
    damage_full: bool,
    damage: &[DamageRect],
) -> EncodeResult<Vec<EncodedPacket>> {
    let mut out = Vec::new();
    unsafe {
        let tb = (*ctx).time_base;
        loop {
            ffi::av_packet_unref(packet);
            let err = ffi::avcodec_receive_packet(ctx, packet);
            if err == ffi::AVERROR(ffi::EAGAIN) || err == ffi::AVERROR_EOF {
                break;
            }
            check(err)?;
            let size = usize::try_from((*packet).size).unwrap_or(0);
            let data_ptr = (*packet).data;
            if data_ptr.is_null() || size == 0 {
                continue;
            }
            let data = std::slice::from_raw_parts(data_ptr, size).to_vec();
            let pts = (*packet).pts;
            let pts_us = if tb.den != 0 && pts != ffi::AV_NOPTS_VALUE {
                pts.saturating_mul(1_000_000)
                    .saturating_mul(i64::from(tb.num))
                    / i64::from(tb.den)
            } else {
                0
            };
            out.push(EncodedPacket {
                seq,
                codec,
                is_keyframe: ((*packet).flags & ffi::AV_PKT_FLAG_KEY) != 0,
                data,
                pts_us,
                damage_full,
                damage: damage.to_vec(),
            });
        }
        ffi::av_packet_unref(packet);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
