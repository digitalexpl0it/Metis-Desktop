//! Primary→secondary GPU framebuffer transfer for hybrid outputs (Wave 3a).
//!
//! Metis custom stack elements (`BlurElement`, decorations, HDR encode) are
//! GLES-typed and cannot feed Smithay [`MultiRenderer`] directly. For outputs
//! whose render node ≠ the primary GPU we therefore:
//!
//! 1. Composite the full stack (blur allowed) on the **primary** GPU into an
//!    offscreen buffer.
//! 2. Read back via [`ExportMem`] and upload on the **secondary** GPU.
//! 3. Scan out a single texture element on the secondary CRTC.
//!
//! When transfer fails (export/import/size), callers fall back to the local
//! `single_renderer` path with blur disabled — same behaviour as before Wave 3a.
//!
//! A future path can replace the CPU readback with dmabuf/`GpuManager::renderer`
//! once custom elements implement `RenderElement<MultiRenderer>`.

use smithay::{
    backend::{
        allocator::Fourcc,
        drm::DrmNode,
        renderer::{
            Bind, ExportMem, ImportMem, Offscreen,
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

/// Result of a successful primary→secondary transfer scan-out.
pub struct TransferFrameResult {
    pub empty: bool,
    pub states: RenderElementStates,
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

    // --- Primary composite -------------------------------------------------
    let pixels = {
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
            state.refresh_hdr_content_flag();
            let passthrough = hdr_active && state.hdr_client_content_visible;
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

        let mut offscreen =
            Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size_buf)
                .map_err(|e| format!("cross-GPU offscreen: {e:?}"))?;
        let mut framebuffer = renderer
            .bind(&mut offscreen)
            .map_err(|e| format!("cross-GPU bind: {e:?}"))?;
        let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        damage_tracker
            .render_output(renderer, &mut framebuffer, 0, &frame_elements, clear)
            .map_err(|e| format!("cross-GPU render_output: {e:?}"))?;

        let region = Rectangle::from_size(size_buf);
        let mapping = renderer
            .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
            .map_err(|e| format!("cross-GPU copy_framebuffer: {e:?}"))?;
        renderer
            .map_texture(&mapping)
            .map_err(|e| format!("cross-GPU map_texture: {e:?}"))?
            .to_vec()
    };

    // --- Secondary present -------------------------------------------------
    let mut target_guard = gpus
        .single_renderer(&target_node)
        .map_err(|e| format!("cross-GPU target renderer: {e:?}"))?;
    let renderer = target_guard.as_mut();

    let texture = renderer
        .import_memory(&pixels, Fourcc::Abgr8888, size_buf, false)
        .map_err(|e| format!("cross-GPU import_memory: {e:?}"))?;
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
            tracing::trace!(
                output = %output.name(),
                ?primary_gpu,
                ?target_node,
                "cross-GPU primary→secondary transfer presented"
            );
            Ok(Some(TransferFrameResult {
                empty: res.is_empty,
                states: res.states,
            }))
        }
        Err(err) => Err(format!("cross-GPU render_frame: {err:?}")),
    }
}
