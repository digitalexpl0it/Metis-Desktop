//! VAAPI DRM_PRIME import + `scale_vaapi` RGB→NV12 (Phase 24 E).
//!
//! Owns a **source** DRM_PRIME `AVHWFramesContext` on the encoder's VAAPI
//! device and a short filter graph. Never maps a DRM_PRIME frame whose frames
//! context is derived only from the encoder NV12 destination pool.

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::ffi;
use ffmpeg_next::format::Pixel;

use crate::convert::{check, pixel_as_i32};
use crate::types::{EncodeError, EncodeInput, EncodeResult};
use crate::zerocopy_drm::{alloc_drm_prime_frames, sw_format_for_input, wrap_drm_prime_frame};

pub(crate) struct VaapiVppUpload {
    drm_frames: *mut ffi::AVBufferRef,
    graph: *mut ffi::AVFilterGraph,
    src: *mut ffi::AVFilterContext,
    sink: *mut ffi::AVFilterContext,
    filtered: *mut ffi::AVFrame,
    width: i32,
    height: i32,
    enc_hw_frames: *mut ffi::AVBufferRef,
}

unsafe impl Send for VaapiVppUpload {}

impl VaapiVppUpload {
    /// # Safety
    /// Live VAAPI device and encoder frames refs; matching width/height.
    pub(crate) unsafe fn open(
        hw_device: *mut ffi::AVBufferRef,
        enc_hw_frames: *mut ffi::AVBufferRef,
        width: i32,
        height: i32,
        sw_format: Pixel,
    ) -> EncodeResult<Self> {
        unsafe {
            let drm_frames = alloc_drm_prime_frames(hw_device, width, height, sw_format)?;
            let mut graph = ffi::avfilter_graph_alloc();
            if graph.is_null() {
                let mut d = drm_frames;
                ffi::av_buffer_unref(&mut d);
                return Err(EncodeError::Ffmpeg("avfilter_graph_alloc failed".into()));
            }

            let buffersrc = ffi::avfilter_get_by_name(c"buffer".as_ptr());
            let buffersink = ffi::avfilter_get_by_name(c"buffersink".as_ptr());
            let hwmap = ffi::avfilter_get_by_name(c"hwmap".as_ptr());
            let scale = ffi::avfilter_get_by_name(c"scale_vaapi".as_ptr());
            if buffersrc.is_null() || buffersink.is_null() || hwmap.is_null() || scale.is_null() {
                ffi::avfilter_graph_free(&mut graph);
                let mut d = drm_frames;
                ffi::av_buffer_unref(&mut d);
                return Err(EncodeError::Unavailable(
                    "FFmpeg filters buffer/hwmap/scale_vaapi/buffersink unavailable".into(),
                ));
            }

            let src_args = CString::new(format!(
                "video_size={width}x{height}:pix_fmt={}:time_base=1/60:pixel_aspect=1/1",
                pixel_as_i32(Pixel::DRM_PRIME)
            ))
            .map_err(|e| EncodeError::InvalidInput(e.to_string()))?;

            let mut src: *mut ffi::AVFilterContext = ptr::null_mut();
            let mut map: *mut ffi::AVFilterContext = ptr::null_mut();
            let mut vpp: *mut ffi::AVFilterContext = ptr::null_mut();
            let mut sink: *mut ffi::AVFilterContext = ptr::null_mut();

            let cleanup = |mut graph: *mut ffi::AVFilterGraph, mut drm: *mut ffi::AVBufferRef| {
                ffi::avfilter_graph_free(&mut graph);
                ffi::av_buffer_unref(&mut drm);
            };

            if ffi::avfilter_graph_create_filter(
                &mut src,
                buffersrc,
                c"in".as_ptr(),
                src_args.as_ptr(),
                ptr::null_mut(),
                graph,
            ) < 0
            {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable(
                    "create buffer source failed".into(),
                ));
            }

            let par = ffi::av_buffersrc_parameters_alloc();
            if par.is_null() {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Ffmpeg(
                    "av_buffersrc_parameters_alloc failed".into(),
                ));
            }
            (*par).format = pixel_as_i32(Pixel::DRM_PRIME);
            (*par).width = width;
            (*par).height = height;
            (*par).time_base = ffi::AVRational { num: 1, den: 60 };
            (*par).hw_frames_ctx = ffi::av_buffer_ref(drm_frames);
            let par_err = ffi::av_buffersrc_parameters_set(src, par);
            ffi::av_free(par as *mut _);
            if par_err < 0 {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable(format!(
                    "av_buffersrc_parameters_set failed ({par_err})"
                )));
            }

            if ffi::avfilter_graph_create_filter(
                &mut map,
                hwmap,
                c"hwmap".as_ptr(),
                c"mode=direct".as_ptr(),
                ptr::null_mut(),
                graph,
            ) < 0
            {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable("create hwmap failed".into()));
            }

            let scale_args = CString::new(format!("w={width}:h={height}:format=nv12"))
                .map_err(|e| EncodeError::InvalidInput(e.to_string()))?;
            if ffi::avfilter_graph_create_filter(
                &mut vpp,
                scale,
                c"vpp".as_ptr(),
                scale_args.as_ptr(),
                ptr::null_mut(),
                graph,
            ) < 0
            {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable("create scale_vaapi failed".into()));
            }

            if ffi::avfilter_graph_create_filter(
                &mut sink,
                buffersink,
                c"out".as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                graph,
            ) < 0
            {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable("create buffersink failed".into()));
            }

            if ffi::avfilter_link(src, 0, map, 0) < 0
                || ffi::avfilter_link(map, 0, vpp, 0) < 0
                || ffi::avfilter_link(vpp, 0, sink, 0) < 0
            {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable("avfilter_link failed".into()));
            }

            if ffi::avfilter_graph_config(graph, ptr::null_mut()) < 0 {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Unavailable(
                    "avfilter_graph_config failed (VAAPI VPP)".into(),
                ));
            }

            let filtered = ffi::av_frame_alloc();
            if filtered.is_null() {
                cleanup(graph, drm_frames);
                return Err(EncodeError::Ffmpeg(
                    "av_frame_alloc(filtered) failed".into(),
                ));
            }

            tracing::info!(
                width,
                height,
                "metis-encode: VAAPI zero-copy upload ready (DRM_PRIME + scale_vaapi)"
            );

            Ok(Self {
                drm_frames,
                graph,
                src,
                sink,
                filtered,
                width,
                height,
                enc_hw_frames,
            })
        }
    }

    /// # Safety
    /// `hw_out` is a live encoder VAAPI frame pointer.
    pub(crate) unsafe fn upload(
        &mut self,
        input: &EncodeInput<'_>,
        hw_out: *mut ffi::AVFrame,
        pts: i64,
        keyframe: bool,
    ) -> EncodeResult<()> {
        unsafe {
            let _ = sw_format_for_input(input)?;
            if input.width as i32 != self.width || input.height as i32 != self.height {
                return Err(EncodeError::InvalidInput("frame size mismatch".into()));
            }

            let mut drm_frame =
                wrap_drm_prime_frame(self.drm_frames, input, self.width, self.height)?;
            (*drm_frame).pts = pts;

            let push = ffi::av_buffersrc_add_frame_flags(
                self.src,
                drm_frame,
                ffi::AV_BUFFERSRC_FLAG_KEEP_REF as i32,
            );
            ffi::av_frame_free(&mut drm_frame);
            check(push)?;

            ffi::av_frame_unref(self.filtered);
            check(ffi::av_buffersink_get_frame(self.sink, self.filtered))?;

            ffi::av_frame_unref(hw_out);
            let same_pool = !(*self.filtered).hw_frames_ctx.is_null()
                && (*self.filtered).hw_frames_ctx == self.enc_hw_frames;
            if same_pool {
                check(ffi::av_frame_ref(hw_out, self.filtered))?;
            } else {
                check(ffi::av_hwframe_get_buffer(self.enc_hw_frames, hw_out, 0))?;
                let map_err =
                    ffi::av_hwframe_map(hw_out, self.filtered, ffi::AV_HWFRAME_MAP_READ as i32);
                if map_err < 0 {
                    check(ffi::av_hwframe_transfer_data(hw_out, self.filtered, 0))?;
                }
            }
            (*hw_out).pts = pts;
            (*hw_out).pict_type = if keyframe {
                ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
            ffi::av_frame_unref(self.filtered);
            Ok(())
        }
    }
}

impl Drop for VaapiVppUpload {
    fn drop(&mut self) {
        unsafe {
            if !self.filtered.is_null() {
                ffi::av_frame_free(&mut self.filtered);
            }
            if !self.graph.is_null() {
                ffi::avfilter_graph_free(&mut self.graph);
                self.src = ptr::null_mut();
                self.sink = ptr::null_mut();
            }
            if !self.drm_frames.is_null() {
                ffi::av_buffer_unref(&mut self.drm_frames);
            }
        }
    }
}
