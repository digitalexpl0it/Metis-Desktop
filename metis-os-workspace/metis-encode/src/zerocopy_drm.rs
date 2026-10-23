//! Wrap compositor LINEAR dmabufs as FFmpeg `AV_PIX_FMT_DRM_PRIME` frames.
//!
//! The descriptor does **not** own the plane FDs (caller keeps [`EncodeInput`]
//! alive for the duration of the filter push). Never map these frames with an
//! `hw_frames_ctx` derived only from the encoder destination pool — that is the
//! known `av_hwframe_map` segfault path.

use std::os::fd::AsRawFd;
use std::ptr;

use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;

use crate::convert::{dmabuf_size, fourcc_to_sw_pixel, pixel_as_i32};
use crate::types::{EncodeError, EncodeInput, EncodeResult};

/// Build a DRM_PRIME `AVHWFramesContext` on `hw_device` (VAAPI or CUDA) for
/// importing user dmabufs. Frames are user-allocated (no pool get_buffer).
///
/// # Safety
/// `hw_device` must be a live `AVBufferRef` for a HW device.
pub(crate) unsafe fn alloc_drm_prime_frames(
    hw_device: *mut ffi::AVBufferRef,
    width: i32,
    height: i32,
    sw_format: Pixel,
) -> EncodeResult<*mut ffi::AVBufferRef> {
    unsafe {
        let mut drm_frames = ffi::av_hwframe_ctx_alloc(hw_device);
        if drm_frames.is_null() {
            return Err(EncodeError::Ffmpeg(
                "av_hwframe_ctx_alloc(DRM_PRIME) failed".into(),
            ));
        }
        {
            let frames_ctx = (*drm_frames).data as *mut ffi::AVHWFramesContext;
            (*frames_ctx).format = Pixel::DRM_PRIME.into();
            (*frames_ctx).sw_format = sw_format.into();
            (*frames_ctx).width = width;
            (*frames_ctx).height = height;
            // User-supplied frames only; pool unused.
            (*frames_ctx).initial_pool_size = 0;
        }
        let err = ffi::av_hwframe_ctx_init(drm_frames);
        if err < 0 {
            ffi::av_buffer_unref(&mut drm_frames);
            return Err(EncodeError::Unavailable(format!(
                "av_hwframe_ctx_init(DRM_PRIME) failed ({err})"
            )));
        }
        Ok(drm_frames)
    }
}

/// Software pixel format for the export fourcc (needed as DRM frames `sw_format`).
pub(crate) fn sw_format_for_input(input: &EncodeInput<'_>) -> EncodeResult<Pixel> {
    fourcc_to_sw_pixel(input.fourcc).ok_or_else(|| {
        EncodeError::InvalidInput(format!("unsupported dmabuf fourcc 0x{:08x}", input.fourcc))
    })
}

/// Allocate an `AVFrame` wrapping `input`'s plane-0 dmabuf as DRM_PRIME.
///
/// # Safety
/// `drm_frames` must remain valid for the frame lifetime. Caller must
/// `av_frame_free` the returned pointer. `input` FDs must outlive the push into
/// the filter (descriptor does not dup/close them).
pub(crate) unsafe fn wrap_drm_prime_frame(
    drm_frames: *mut ffi::AVBufferRef,
    input: &EncodeInput<'_>,
    width: i32,
    height: i32,
) -> EncodeResult<*mut ffi::AVFrame> {
    unsafe {
        let fd = *input
            .fds
            .first()
            .ok_or_else(|| EncodeError::InvalidInput("no dmabuf planes".into()))?;
        let offset = input.offsets.first().copied().unwrap_or(0) as isize;
        let pitch = input.strides.first().copied().unwrap_or(input.stride) as isize;
        let size = dmabuf_size(fd).unwrap_or_else(|| {
            (pitch as usize)
                .saturating_mul(height as usize)
                .saturating_add(offset as usize)
        });

        let desc = ffi::av_mallocz(std::mem::size_of::<ffi::AVDRMFrameDescriptor>())
            as *mut ffi::AVDRMFrameDescriptor;
        if desc.is_null() {
            return Err(EncodeError::Ffmpeg(
                "av_mallocz(AVDRMFrameDescriptor) failed".into(),
            ));
        }
        (*desc).nb_objects = 1;
        (*desc).objects[0].fd = fd.as_raw_fd();
        (*desc).objects[0].size = size;
        (*desc).objects[0].format_modifier = input.modifier;
        (*desc).nb_layers = 1;
        (*desc).layers[0].format = input.fourcc;
        (*desc).layers[0].nb_planes = 1;
        (*desc).layers[0].planes[0].object_index = 0;
        (*desc).layers[0].planes[0].offset = offset;
        (*desc).layers[0].planes[0].pitch = pitch;

        let mut frame = ffi::av_frame_alloc();
        if frame.is_null() {
            ffi::av_free(desc as *mut _);
            return Err(EncodeError::Ffmpeg(
                "av_frame_alloc(DRM_PRIME) failed".into(),
            ));
        }
        let buf = ffi::av_buffer_create(
            desc as *mut u8,
            std::mem::size_of::<ffi::AVDRMFrameDescriptor>(),
            Some(ffi::av_buffer_default_free),
            ptr::null_mut(),
            0,
        );
        if buf.is_null() {
            ffi::av_free(desc as *mut _);
            ffi::av_frame_free(&mut frame);
            return Err(EncodeError::Ffmpeg(
                "av_buffer_create(DRM_PRIME) failed".into(),
            ));
        }
        (*frame).buf[0] = buf;
        (*frame).data[0] = desc as *mut u8;
        (*frame).format = pixel_as_i32(Pixel::DRM_PRIME);
        (*frame).width = width;
        (*frame).height = height;
        (*frame).hw_frames_ctx = ffi::av_buffer_ref(drm_frames);
        if (*frame).hw_frames_ctx.is_null() {
            ffi::av_frame_free(&mut frame);
            return Err(EncodeError::Ffmpeg(
                "av_buffer_ref(drm_frames) failed".into(),
            ));
        }
        Ok(frame)
    }
}
