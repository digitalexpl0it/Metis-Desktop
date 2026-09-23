//! Duplicate (mirror) display mode for the DRM backend.
//!
//! When active, every enabled output is mapped at the origin and the compositor
//! renders the mirror source once per frame, then scale-to-fits it onto each
//! CRTC with letterboxing.

use metis_config::{DisplayLayoutMode, OutputsConfig, output_prefs};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{
    Kind,
    texture::{TextureBuffer, TextureRenderElement},
};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::night_light::RenderTargetInfo;
use crate::render::{CLEAR_COLOR, OutputStack};
use crate::state::MetisState;
use crate::udev::UdevOutputId;

/// Black clear used behind letterboxed mirror blits.
const MIRROR_LETTERBOX_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Mirror-source frame for the current damage-dispatch batch.
pub struct MirrorBatchCache {
    pub buffer: TextureBuffer<GlesTexture>,
    pub src_size: Size<i32, Physical>,
}

impl MetisState {
    pub(crate) fn display_layout_mode(&self) -> DisplayLayoutMode {
        self.output_runtime.cached().display_mode
    }

    /// True when duplicate mode should be active (DRM session, ≥2 enabled outputs).
    pub(crate) fn mirror_mode_active(&self) -> bool {
        self.is_drm_backend()
            && self.display_layout_mode() == DisplayLayoutMode::Mirror
            && self.enabled_output_count() >= 2
    }

    /// Enabled outputs sorted for mirror-source fallback (layout x, then name).
    pub(crate) fn enabled_outputs_sorted(&self) -> Vec<Output> {
        let mut outputs: Vec<Output> = self
            .connected_outputs()
            .into_iter()
            .filter(|o| self.is_output_enabled(&o.name()))
            .collect();
        outputs.sort_by(|a, b| {
            let ax = self.space.output_geometry(a).map(|g| g.loc.x).unwrap_or(0);
            let bx = self.space.output_geometry(b).map(|g| g.loc.x).unwrap_or(0);
            ax.cmp(&bx).then_with(|| a.name().cmp(&b.name()))
        });
        outputs
    }

    /// Resolved mirror source output name (config override or first enabled).
    pub(crate) fn resolve_mirror_source_name(&self) -> Option<String> {
        if !self.mirror_mode_active() {
            return None;
        }
        let cfg = self.output_runtime.cached();
        let sorted = self.enabled_outputs_sorted();
        if let Some(name) = cfg.mirror_source.as_ref()
            && sorted.iter().any(|o| o.name() == *name)
        {
            return Some(name.clone());
        }
        sorted.first().map(|o| o.name())
    }

    pub(crate) fn resolve_mirror_source(&self) -> Option<Output> {
        let name = self.resolve_mirror_source_name()?;
        self.connected_outputs()
            .into_iter()
            .find(|o| o.name() == name && self.is_output_enabled(&o.name()))
    }

    pub(crate) fn clear_mirror_batch_cache(&mut self) {
        if let Some(udev) = self.udev.as_mut() {
            udev.mirror_batch = None;
        }
    }
}

/// Map every enabled output to the origin for duplicate mode.
pub fn apply_mirror_layout(state: &mut MetisState, _cfg: &OutputsConfig) -> bool {
    if !state.mirror_mode_active() {
        return false;
    }
    let origin = Point::from((0i32, 0i32));
    let mut changed = false;
    for output in state.enabled_outputs_sorted() {
        let current = state.space.output_geometry(&output).map(|g| g.loc);
        if current != Some(origin) {
            output.change_current_state(None, None, None, Some(origin));
            state.space.map_output(&output, origin);
            tracing::info!(name = %output.name(), "applied mirror layout at origin");
            changed = true;
        }
    }
    if changed {
        state.clear_mirror_batch_cache();
    }
    changed
}

/// In mirror mode every enabled output inherits the source scale.
pub fn apply_mirror_scales(state: &mut MetisState, cfg: &OutputsConfig) -> bool {
    if !state.mirror_mode_active() {
        return false;
    }
    let Some(source_name) = state.resolve_mirror_source_name() else {
        return false;
    };
    let source_scale = output_prefs(cfg, &source_name).scale.clamp(1.0, 4.0);
    let mut changed = false;
    for output in state.enabled_outputs_sorted() {
        let current = output.current_scale().fractional_scale();
        if (current - source_scale).abs() <= 0.001 {
            continue;
        }
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(source_scale)),
            None,
        );
        tracing::info!(name = %output.name(), scale = source_scale, "applied mirror source scale");
        changed = true;
    }
    if changed {
        state.clear_mirror_batch_cache();
    }
    changed
}

fn ensure_mirror_batch_cache(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    allow_blur: bool,
) -> bool {
    if state
        .udev
        .as_ref()
        .is_some_and(|u| u.mirror_batch.is_some())
    {
        return true;
    }

    let source = match state.resolve_mirror_source() {
        Some(o) => o,
        None => return false,
    };
    let mode = match source.current_mode() {
        Some(m) => m,
        None => return false,
    };
    let src_size: Size<i32, Physical> = mode.size;
    if src_size.w <= 0 || src_size.h <= 0 {
        return false;
    }

    let output_scale = Scale::from(source.current_scale().fractional_scale());
    let render_origin: Point<i32, Physical> = state
        .space
        .output_geometry(&source)
        .map(|g| g.loc.to_physical_precise_round(output_scale))
        .unwrap_or_default();

    let mut elements = state.build_render_elements(
        renderer,
        render_origin,
        output_scale,
        RenderTargetInfo {
            size: src_size,
            output_name: Some(source.name().as_str()),
            skip_night_light: false,
        },
        &[],
        allow_blur,
    );
    let mut cursor = state.build_cursor_elements(renderer, &source, output_scale);
    if !cursor.is_empty() {
        cursor.append(&mut elements);
        elements = cursor;
    }

    let size_buf: Size<i32, Buffer> = Size::from((src_size.w, src_size.h));
    let mut offscreen =
        match Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size_buf) {
            Ok(buf) => buf,
            Err(err) => {
                tracing::warn!(?err, "mirror offscreen buffer creation failed");
                return false;
            }
        };
    {
        let mut framebuffer = match renderer.bind(&mut offscreen) {
            Ok(fb) => fb,
            Err(err) => {
                tracing::warn!(?err, "mirror offscreen bind failed");
                return false;
            }
        };
        let mut damage_tracker =
            OutputDamageTracker::new(src_size, output_scale, Transform::Normal);
        if let Err(err) =
            damage_tracker.render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR)
        {
            tracing::warn!(?err, "mirror source render failed");
            return false;
        }
    }

    let buffer = TextureBuffer::from_texture(renderer, offscreen, 1, Transform::Normal, None);
    let cache = MirrorBatchCache { buffer, src_size };
    if let Some(udev) = state.udev.as_mut() {
        udev.mirror_batch = Some(cache);
    }
    true
}

/// Blit the cached mirror source onto a DRM surface (scale-to-fit + letterbox).
pub fn render_mirror_surface(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    id: UdevOutputId,
    allow_blur: bool,
) -> Result<bool, String> {
    if !ensure_mirror_batch_cache(state, renderer, allow_blur) {
        return Ok(false);
    }

    let (src_size, buffer) = {
        let udev = state.udev.as_ref().ok_or("no udev")?;
        let cache = udev.mirror_batch.as_ref().ok_or("no mirror cache")?;
        (cache.src_size, cache.buffer.clone())
    };

    let dst_size = {
        let udev = state.udev.as_ref().ok_or("no udev")?;
        let surface = udev.surface(id).ok_or("no surface")?;
        let dst_mode = surface
            .output
            .current_mode()
            .ok_or_else(|| "mirror target missing mode".to_string())?;
        let dst_size: Size<i32, Physical> = dst_mode.size;
        if dst_size.w <= 0 || dst_size.h <= 0 {
            return Err("mirror target has zero size".into());
        }
        dst_size
    };

    let src_w = src_size.w;
    let src_h = src_size.h;
    let fit = (dst_size.w as f64 / src_w as f64).min(dst_size.h as f64 / src_h as f64);
    let scaled_w = (src_w as f64 * fit).round() as i32;
    let scaled_h = (src_h as f64 * fit).round() as i32;
    let ox = (dst_size.w - scaled_w) / 2;
    let oy = (dst_size.h - scaled_h) / 2;

    let src_rect = Rectangle::<f64, Logical>::new(
        Point::from((0.0, 0.0)),
        Size::from((src_w as f64, src_h as f64)),
    );
    let loc = Point::<f64, Physical>::from((ox as f64, oy as f64));
    let tex = TextureRenderElement::from_texture_buffer(
        loc,
        &buffer,
        None,
        Some(src_rect),
        Some(Size::from((scaled_w, scaled_h))),
        Kind::Unspecified,
    );
    let elements: Vec<OutputStack> = vec![OutputStack::Wallpaper(tex)];

    crate::output_vrr::prepare_vrr_for_render(state, id);
    let udev = state.udev.as_mut().ok_or("no udev")?;
    let surface = udev.surface_mut(id).ok_or("no surface")?;
    let outcome = surface
        .drm_output
        .render_frame(
            renderer,
            &elements,
            MIRROR_LETTERBOX_COLOR,
            FrameFlags::DEFAULT,
        )
        .map_err(|e| format!("{e:?}"))?;

    Ok(!outcome.is_empty)
}
