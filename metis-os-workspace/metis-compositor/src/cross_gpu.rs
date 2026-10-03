//! Primary→secondary GPU framebuffer transfer for hybrid outputs (Wave 3a).
//!
//! Full-frame primary→secondary transfer fallback when hybrid
//! [`crate::hybrid_multi`] MultiRenderer fails. Wave C runs blur, HDR decode/
//! encode, and Stage-2 LUT on the Multi path; this module remains the hard-fail
//! safety net (and the historical Wave 3a path).
//!
//! For outputs whose render node ≠ the primary GPU when Multi fails:
//!
//! 1. Composite the full stack (blur allowed) on the **primary** GPU into a
//!    GBM `Dmabuf` (preferred) or an offscreen texture.
//! 2. Prefer **dmabuf import** on the secondary GPU (no CPU `to_vec` readback).
//! 3. Fall back to [`ExportMem`] + [`ImportMem`] when dmabuf allocate/import fails.
//! 4. Scan out a single texture element on the secondary CRTC.
//!
//! When transfer fails entirely, callers fall back to the local `single_renderer`
//! path with blur disabled — same behaviour as before Wave 3a.

use std::sync::atomic::{AtomicU64, Ordering};

use smithay::{
    backend::{
        allocator::{
            Allocator, Fourcc, Modifier,
            dmabuf::{Dmabuf, DmabufAllocator},
            gbm::{GbmAllocator, GbmBufferFlags},
        },
        drm::DrmNode,
        renderer::{
            Bind, ExportMem, ImportDma, ImportMem, Offscreen,
            damage::OutputDamageTracker,
            element::{
                Kind, RenderElementStates,
                texture::{TextureBuffer, TextureRenderElement},
            },
            gles::{GlesRenderer, GlesTexture},
            multigpu::{GpuManager, gbm::GbmGlesBackend},
        },
    },
    output::Output,
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::render::{CLEAR_COLOR, OutputStack};
use crate::state::MetisState;
use crate::udev::UdevOutputId;

static DMABUF_OK: AtomicU64 = AtomicU64::new(0);
static EXPORT_MEM_FALLBACK: AtomicU64 = AtomicU64::new(0);
static TRANSFER_FAIL: AtomicU64 = AtomicU64::new(0);
static MULTI_OK: AtomicU64 = AtomicU64::new(0);
static MULTI_FAIL: AtomicU64 = AtomicU64::new(0);

/// Snapshot of hybrid present path counters (Wave A metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CrossGpuStats {
    pub dmabuf_ok: u64,
    pub export_mem_fallback: u64,
    pub transfer_fail: u64,
    pub multi_ok: u64,
    pub multi_fail: u64,
}

impl CrossGpuStats {
    pub fn snapshot() -> Self {
        Self {
            dmabuf_ok: DMABUF_OK.load(Ordering::Relaxed),
            export_mem_fallback: EXPORT_MEM_FALLBACK.load(Ordering::Relaxed),
            transfer_fail: TRANSFER_FAIL.load(Ordering::Relaxed),
            multi_ok: MULTI_OK.load(Ordering::Relaxed),
            multi_fail: MULTI_FAIL.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn record_dmabuf_ok() {
    DMABUF_OK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_export_mem_fallback() {
    EXPORT_MEM_FALLBACK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_transfer_fail() {
    TRANSFER_FAIL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_multi_ok() {
    MULTI_OK.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_multi_fail() {
    MULTI_FAIL.fetch_add(1, Ordering::Relaxed);
}

/// Result of a successful primary→secondary transfer scan-out.
pub struct TransferFrameResult {
    pub empty: bool,
    pub states: RenderElementStates,
}

enum TransferPixels {
    Dmabuf(Dmabuf),
    Cpu(Vec<u8>),
}

/// Try to composite on `primary_gpu` and present on `target_node`.
///
/// Returns `Ok(None)` when transfer is skipped (caller should use local path).
/// Returns `Err` when transfer was attempted but failed (caller should fall back
/// and may log).
pub fn try_transfer_frame(
    state: &mut MetisState,
    gpus: &mut GpuManager<GbmGlesBackend<GlesRenderer, smithay::backend::drm::DrmDeviceFd>>,
    primary_gpu: DrmNode,
    target_node: DrmNode,
    id: UdevOutputId,
    output: &Output,
) -> Result<Option<TransferFrameResult>, String> {
    if primary_gpu == target_node {
        return Ok(None);
    }

    let scale = Scale::from(output.current_scale().fractional_scale());
    let size: Size<i32, Physical> = output
        .current_mode()
        .map(|m| m.size)
        .ok_or_else(|| "cross-GPU transfer: output has no mode".to_string())?;
    if size.w <= 0 || size.h <= 0 {
        return Err("cross-GPU transfer: zero-sized mode".into());
    }
    let size_buf: Size<i32, Buffer> = Size::from((size.w, size.h));
    let origin: Point<i32, Physical> = state
        .space
        .output_geometry(output)
        .map(|g| g.loc.to_physical_precise_round(scale))
        .unwrap_or_default();

    let (hdr_active, hdr_transfer) = state
        .udev
        .as_ref()
        .and_then(|u| u.surface(id))
        .map(|s| (s.hdr_active, s.hdr_transfer))
        .unwrap_or((false, crate::hdr_encode::HdrTransfer::Pq));

    let transfer_gbm = state
        .udev
        .as_ref()
        .and_then(|u| u.gbm_for_render_node(primary_gpu))
        .cloned();

    // --- Primary composite -------------------------------------------------
    let (pixels, used_dmabuf) = {
        let mut primary_guard = gpus
            .single_renderer(&primary_gpu)
            .map_err(|e| format!("cross-GPU primary renderer: {e:?}"))?;
        let renderer = primary_guard.as_mut();

        if state.color_mgmt.profiles_dirty {
            let profiles = state.color_mgmt.profile_map().clone();
            state.color_lut.sync_profiles(renderer, &profiles);
            state.color_mgmt.profiles_dirty = false;
            crate::output_gamma::apply_output_gamma(state);
        }

        let mut elements = state.build_render_elements(
            renderer,
            origin,
            scale,
            crate::night_light::RenderTargetInfo {
                size,
                output_name: Some(output.name().as_str()),
                skip_night_light: false,
            },
            &[],
            true, // blur on primary
        );
        let cursor = state.build_cursor_elements(renderer, output, scale);
        if !cursor.is_empty() {
            let mut stacked = cursor;
            stacked.append(&mut elements);
            elements = stacked;
        }

        let output_name = output.name();
        let (frame_elements, clear): (Vec<OutputStack>, [f32; 4]) = {
            let mode = state.hdr_content_mode_for_output(Some(output_name.as_str()));
            let passthrough = mode.allows_passthrough(hdr_active);
            match crate::output_colour::apply_colour_post_pass(
                &mut state.color_lut,
                &mut state.hdr_encode,
                renderer,
                &elements,
                &output_name,
                size,
                scale,
                hdr_active,
                hdr_transfer,
                passthrough,
            ) {
                Some(pass) => (pass.elements, pass.clear),
                _ => (elements, CLEAR_COLOR),
            }
        };

        // Prefer GBM dmabuf bind (no CPU readback). Fall back to texture + ExportMem.
        if let Some(gbm) = transfer_gbm.as_ref() {
            match render_to_dmabuf(renderer, gbm, size_buf, size, scale, &frame_elements, clear) {
                Ok(dmabuf) => (TransferPixels::Dmabuf(dmabuf), true),
                Err(err) => {
                    tracing::debug!(%err, "cross-GPU dmabuf path failed; trying CPU readback");
                    let cpu =
                        render_to_cpu(renderer, size_buf, size, scale, &frame_elements, clear)
                            .inspect_err(|_| record_transfer_fail())?;
                    record_export_mem_fallback();
                    (TransferPixels::Cpu(cpu), false)
                }
            }
        } else {
            let cpu = render_to_cpu(renderer, size_buf, size, scale, &frame_elements, clear)
                .inspect_err(|_| record_transfer_fail())?;
            record_export_mem_fallback();
            (TransferPixels::Cpu(cpu), false)
        }
    };

    // --- Secondary present -------------------------------------------------
    let mut target_guard = gpus
        .single_renderer(&target_node)
        .map_err(|e| format!("cross-GPU target renderer: {e:?}"))?;
    let renderer = target_guard.as_mut();

    let texture = match pixels {
        TransferPixels::Dmabuf(dmabuf) => renderer.import_dmabuf(&dmabuf, None).map_err(|e| {
            record_transfer_fail();
            format!("cross-GPU import_dmabuf: {e:?}")
        })?,
        TransferPixels::Cpu(bytes) => renderer
            .import_memory(&bytes, Fourcc::Abgr8888, size_buf, false)
            .map_err(|e| {
                record_transfer_fail();
                format!("cross-GPU import_memory: {e:?}")
            })?,
    };
    let buffer = TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);
    let element = TextureRenderElement::from_texture_buffer(
        Point::from((0.0, 0.0)),
        &buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    );
    let frame_elements = vec![OutputStack::Wallpaper(element)];

    crate::output_vrr::prepare_vrr_for_render(state, id);
    crate::output_hdr::maybe_log_scanout_format(state, id);

    let Some(surface) = state.udev.as_mut().and_then(|udev| udev.surface_mut(id)) else {
        return Err("cross-GPU transfer: surface gone".into());
    };
    match surface.drm_output.render_frame(
        renderer,
        &frame_elements,
        CLEAR_COLOR,
        smithay::backend::drm::compositor::FrameFlags::DEFAULT,
    ) {
        Ok(res) => {
            if used_dmabuf {
                record_dmabuf_ok();
            }
            tracing::debug!(
                output = %output.name(),
                ?primary_gpu,
                ?target_node,
                used_dmabuf,
                stats = ?CrossGpuStats::snapshot(),
                "cross-GPU primary→secondary transfer presented"
            );
            Ok(Some(TransferFrameResult {
                empty: res.is_empty,
                states: res.states,
            }))
        }
        Err(err) => {
            record_transfer_fail();
            Err(format!("cross-GPU render_frame: {err:?}"))
        }
    }
}

fn render_to_dmabuf(
    renderer: &mut GlesRenderer,
    gbm: &smithay::backend::allocator::gbm::GbmDevice<smithay::backend::drm::DrmDeviceFd>,
    size_buf: Size<i32, Buffer>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    frame_elements: &[OutputStack],
    clear: [f32; 4],
) -> Result<Dmabuf, String> {
    let mut allocator = DmabufAllocator(GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING));
    let mut dmabuf = allocator
        .create_buffer(
            size_buf.w as u32,
            size_buf.h as u32,
            Fourcc::Abgr8888,
            &[Modifier::Invalid, Modifier::Linear],
        )
        .map_err(|e| format!("cross-GPU dmabuf allocate: {e}"))?;

    {
        let mut framebuffer = renderer
            .bind(&mut dmabuf)
            .map_err(|e| format!("cross-GPU dmabuf bind: {e:?}"))?;
        let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        let result = damage_tracker
            .render_output(renderer, &mut framebuffer, 0, frame_elements, clear)
            .map_err(|e| format!("cross-GPU dmabuf render_output: {e:?}"))?;
        if let Err(err) = result.sync.wait() {
            tracing::debug!(?err, "cross-GPU dmabuf sync wait interrupted");
        }
    }
    Ok(dmabuf)
}

fn render_to_cpu(
    renderer: &mut GlesRenderer,
    size_buf: Size<i32, Buffer>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    frame_elements: &[OutputStack],
    clear: [f32; 4],
) -> Result<Vec<u8>, String> {
    let mut offscreen =
        Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size_buf)
            .map_err(|e| format!("cross-GPU offscreen: {e:?}"))?;
    let mut framebuffer = renderer
        .bind(&mut offscreen)
        .map_err(|e| format!("cross-GPU bind: {e:?}"))?;
    let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
    damage_tracker
        .render_output(renderer, &mut framebuffer, 0, frame_elements, clear)
        .map_err(|e| format!("cross-GPU render_output: {e:?}"))?;

    let region = Rectangle::from_size(size_buf);
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .map_err(|e| format!("cross-GPU copy_framebuffer: {e:?}"))?;
    Ok(renderer
        .map_texture(&mapping)
        .map_err(|e| format!("cross-GPU map_texture: {e:?}"))?
        .to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_gpu_stats_snapshot_reads_atomics() {
        let before = CrossGpuStats::snapshot();
        record_dmabuf_ok();
        record_export_mem_fallback();
        record_multi_ok();
        let after = CrossGpuStats::snapshot();
        assert!(after.dmabuf_ok > before.dmabuf_ok);
        assert!(after.export_mem_fallback > before.export_mem_fallback);
        assert!(after.multi_ok > before.multi_ok);
    }
}
