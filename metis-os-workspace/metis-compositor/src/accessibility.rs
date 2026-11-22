//! Accessibility runtime: magnifier zoom + shake-pointer-to-find (sonar).
//!
//! Owned by the compositor so the DRM software cursor and scene stack can
//! animate without relying on external tools (Magnus) or shell OSD.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use xcursor::parser::Image;

use crate::render::OutputStack;
use crate::state::MetisState;

/// How long the shake "find pointer" effect lasts.
const SHAKE_FX_MS: u64 = 1400;
/// Sliding window for detecting rapid direction changes.
const SHAKE_WINDOW_MS: u64 = 450;
/// Minimum direction reversals (X or Y) inside the window to fire.
const SHAKE_REVERSALS: u32 = 4;
/// Ignore tiny jitter.
const SHAKE_MIN_DELTA: f64 = 2.5;
/// Peak cursor scale during shake FX (relative to configured size).
const SHAKE_CURSOR_PEAK: f32 = 2.75;
/// Fullscreen digital zoom when magnifier is enabled.
const MAGNIFIER_ZOOM: f64 = 2.0;

#[derive(Debug)]
struct MotionSample {
    at: Instant,
    dx: f64,
    dy: f64,
}

#[derive(Debug)]
pub struct ShakeFx {
    pub started: Instant,
}

#[derive(Debug)]
pub struct AccessibilityRuntime {
    pub magnifier: bool,
    pub shake_enabled: bool,
    samples: VecDeque<MotionSample>,
    pub shake_fx: Option<ShakeFx>,
}

impl AccessibilityRuntime {
    pub fn new() -> Self {
        let cfg = metis_config::load_accessibility_config();
        Self {
            magnifier: cfg.magnifier,
            shake_enabled: cfg.shake_pointer,
            samples: VecDeque::with_capacity(64),
            shake_fx: None,
        }
    }

    pub fn reload_from_config(&mut self) {
        let cfg = metis_config::load_accessibility_config();
        self.magnifier = cfg.magnifier;
        self.shake_enabled = cfg.shake_pointer;
        if !self.shake_enabled {
            self.shake_fx = None;
            self.samples.clear();
        }
        tracing::info!(
            magnifier = self.magnifier,
            shake = self.shake_enabled,
            "accessibility runtime reloaded"
        );
    }

    /// Feed relative pointer motion. Returns `true` when a new shake FX started.
    pub fn on_pointer_delta(&mut self, dx: f64, dy: f64) -> bool {
        if !self.shake_enabled {
            return false;
        }
        // Cooldown while an FX is already playing.
        if let Some(fx) = &self.shake_fx
            && fx.started.elapsed() < Duration::from_millis(SHAKE_FX_MS)
        {
            return false;
        }
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            return false;
        }
        let now = Instant::now();
        self.samples.push_back(MotionSample { at: now, dx, dy });
        let cutoff = now - Duration::from_millis(SHAKE_WINDOW_MS);
        while self.samples.front().is_some_and(|s| s.at < cutoff) {
            self.samples.pop_front();
        }
        if detect_shake(&self.samples) {
            self.samples.clear();
            self.shake_fx = Some(ShakeFx { started: now });
            tracing::debug!("shake-pointer: find-cursor FX armed");
            return true;
        }
        false
    }

    /// Progress 0..1 through the active shake FX, or `None` if idle.
    pub fn shake_progress(&self) -> Option<f32> {
        let fx = self.shake_fx.as_ref()?;
        let elapsed = fx.started.elapsed();
        let total = Duration::from_millis(SHAKE_FX_MS);
        if elapsed >= total {
            return None;
        }
        Some((elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0))
    }

    /// Cursor scale multiplier while shake FX is active (eases back to 1.0).
    pub fn shake_cursor_scale(&self) -> f32 {
        let Some(t) = self.shake_progress() else {
            return 1.0;
        };
        // Hold large for the first ~45%, then ease down.
        if t < 0.45 {
            SHAKE_CURSOR_PEAK
        } else {
            let u = ((t - 0.45) / 0.55).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - u) * (1.0 - u);
            SHAKE_CURSOR_PEAK + (1.0 - SHAKE_CURSOR_PEAK) * ease
        }
    }

    pub fn tick_clear_expired(&mut self) -> bool {
        if let Some(fx) = &self.shake_fx
            && fx.started.elapsed() >= Duration::from_millis(SHAKE_FX_MS)
        {
            self.shake_fx = None;
            return true;
        }
        false
    }

    pub fn needs_redraw(&self) -> bool {
        // Magnifier follows pointer motion (already schedules redraw). Shake FX
        // needs a continuous clock while rings animate.
        self.shake_fx.is_some()
    }
}

fn detect_shake(samples: &VecDeque<MotionSample>) -> bool {
    if samples.len() < 6 {
        return false;
    }
    let mut reversals = 0u32;
    let mut last_sx = 0i8;
    let mut last_sy = 0i8;
    let mut travel = 0.0f64;
    for s in samples {
        travel += (s.dx * s.dx + s.dy * s.dy).sqrt();
        let sx = if s.dx.abs() >= SHAKE_MIN_DELTA {
            if s.dx > 0.0 { 1 } else { -1 }
        } else {
            0
        };
        let sy = if s.dy.abs() >= SHAKE_MIN_DELTA {
            if s.dy > 0.0 { 1 } else { -1 }
        } else {
            0
        };
        if sx != 0 && last_sx != 0 && sx != last_sx {
            reversals += 1;
        }
        if sy != 0 && last_sy != 0 && sy != last_sy {
            reversals += 1;
        }
        if sx != 0 {
            last_sx = sx;
        }
        if sy != 0 {
            last_sy = sy;
        }
    }
    reversals >= SHAKE_REVERSALS && travel >= 80.0
}

/// Nearest-neighbor scale of an XCursor RGBA frame (shake enlarge).
pub fn scale_cursor_image(image: &Image, factor: f32) -> Image {
    let factor = factor.clamp(1.0, 4.0);
    if (factor - 1.0).abs() < 0.05 {
        return image.clone();
    }
    let sw = image.width.max(1);
    let sh = image.height.max(1);
    let dw = ((sw as f32) * factor).round().max(1.0) as u32;
    let dh = ((sh as f32) * factor).round().max(1.0) as u32;
    let mut pixels = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let sy = ((y as f32) / factor).floor() as u32;
        let sy = sy.min(sh - 1);
        for x in 0..dw {
            let sx = ((x as f32) / factor).floor() as u32;
            let sx = sx.min(sw - 1);
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 4) as usize;
            if si + 3 < image.pixels_rgba.len() && di + 3 < pixels.len() {
                pixels[di..di + 4].copy_from_slice(&image.pixels_rgba[si..si + 4]);
            }
        }
    }
    Image {
        size: dw.max(dh),
        width: dw,
        height: dh,
        xhot: ((image.xhot as f32) * factor).round() as u32,
        yhot: ((image.yhot as f32) * factor).round() as u32,
        delay: image.delay,
        pixels_rgba: pixels,
        pixels_argb: Vec::new(),
    }
}

/// Build expanding sonar rings centred on the pointer (output-local logical).
pub fn build_sonar_elements(
    runtime: &AccessibilityRuntime,
    renderer: &mut GlesRenderer,
    local_logical: Point<f64, Logical>,
    scale: Scale<f64>,
) -> Vec<OutputStack> {
    let Some(t) = runtime.shake_progress() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // Three staggered rings.
    for phase0 in [0.0f32, 0.22, 0.44] {
        let phase = (t - phase0).clamp(0.0, 1.0);
        if phase <= 0.0 || phase0 > t {
            continue;
        }
        // Radius grows; alpha fades.
        let radius_logical = 18.0 + f64::from(phase) * 72.0;
        let alpha = ((1.0 - phase) * 0.85).clamp(0.0, 1.0);
        let sx = scale.x;
        let diameter = ((radius_logical * 2.0 + 8.0) * sx).ceil().max(8.0) as u32;
        let thickness = (2.5 * sx).clamp(2.0_f64, 5.0) as f32;
        let pixels = raster_ring(diameter, thickness, alpha);
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (diameter as i32, diameter as i32),
            1,
            smithay::utils::Transform::Normal,
            None,
        );
        let half = f64::from(diameter) / 2.0;
        let pos = local_logical.to_physical(scale) - Point::<f64, Physical>::from((half, half));
        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            pos,
            &buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            out.push(OutputStack::CursorMemory(elem));
        }
    }
    out
}

fn raster_ring(diameter: u32, thickness: f32, alpha: f32) -> Vec<u8> {
    let d = diameter.max(4);
    let cx = (d as f32 - 1.0) * 0.5;
    let cy = cx;
    let outer = d as f32 * 0.5 - 1.0;
    let inner = (outer - thickness).max(0.0);
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    // Accent-ish cyan ring (ARGB for Fourcc::Argb8888 — smithay cursor path uses
    // pixels_rgba with Argb8888 fourcc the same way as XCursor frames).
    let mut px = vec![0u8; (d * d * 4) as usize];
    for y in 0..d {
        for x in 0..d {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            if r <= outer && r >= inner {
                // Soft edge.
                let edge = (1.0 - ((r - inner).min(outer - r) / thickness.max(0.01)).min(1.0))
                    .clamp(0.0, 1.0);
                let aa = ((a as f32) * (0.35 + 0.65 * (1.0 - edge))).round() as u8;
                let i = ((y * d + x) * 4) as usize;
                // RGBA bytes (matches cursor MemoryRenderBuffer usage).
                px[i] = 0x00; // R
                px[i + 1] = 0xf2; // G
                px[i + 2] = 0xfe; // B
                px[i + 3] = aa; // A
            }
        }
    }
    px
}

impl MetisState {
    pub(crate) fn reload_accessibility_from_config(&mut self) {
        self.a11y.reload_from_config();
        self.damaged = true;
        self.request_redraw();
    }

    /// Append shake sonar (+ optional cursor scale is applied in build_cursor_elements).
    pub(crate) fn build_shake_sonar_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &smithay::output::Output,
        scale: Scale<f64>,
    ) -> Vec<OutputStack> {
        let Some(geo) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return Vec::new();
        };
        let loc = pointer.current_location();
        if !geo.to_f64().contains(loc) {
            return Vec::new();
        }
        let local = loc - geo.loc.to_f64();
        build_sonar_elements(&self.a11y, renderer, local, scale)
    }
}

/// Apply shake scale to a cursor image, returning a cache key + scaled image.
pub fn maybe_scale_for_shake(image: &Image, shake_scale: f32) -> (Image, Image) {
    if shake_scale <= 1.05 {
        return (image.clone(), image.clone());
    }
    let scaled = scale_cursor_image(image, shake_scale);
    // Distinct cache key so we don't collide with the unscaled frame.
    let mut key = scaled.clone();
    key.delay = key.delay.wrapping_add((shake_scale * 1000.0) as u32);
    (key, scaled)
}

/// Magnifier zoom: src rect in physical pixels of the offscreen scene.
pub fn magnifier_src_rect(
    output_size: Size<i32, Physical>,
    pointer_local_physical: Point<f64, Physical>,
    zoom: f64,
) -> Rectangle<f64, Logical> {
    let zoom = zoom.max(1.01);
    let w = (output_size.w as f64) / zoom;
    let h = (output_size.h as f64) / zoom;
    let mut x = pointer_local_physical.x - w * 0.5;
    let mut y = pointer_local_physical.y - h * 0.5;
    x = x.clamp(0.0, (output_size.w as f64 - w).max(0.0));
    y = y.clamp(0.0, (output_size.h as f64 - h).max(0.0));
    Rectangle::new(Point::from((x, y)), Size::from((w, h)))
}

/// Render `scene` to an offscreen, then present a zoomed crop around the pointer
/// as a fullscreen texture element. Returns `None` on failure (caller keeps the
/// unzoomed scene).
pub fn try_build_magnifier_element(
    state: &MetisState,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    scale: Scale<f64>,
    scene: &[OutputStack],
) -> Option<OutputStack> {
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
    use smithay::backend::renderer::gles::GlesTexture;
    use smithay::backend::renderer::{Bind, Offscreen};
    use smithay::utils::Transform;

    let mode = output.current_mode()?;
    let size = mode.size;
    if size.w < 8 || size.h < 8 {
        return None;
    }
    let geo = state.space.output_geometry(output)?;
    let pointer = state.seat.get_pointer()?;
    let loc = pointer.current_location();
    if !geo.to_f64().contains(loc) {
        return None;
    }
    let local = loc - geo.loc.to_f64();
    let local_phys = local.to_physical(scale);
    let src = magnifier_src_rect(size, local_phys, MAGNIFIER_ZOOM);

    let size_buf: Size<i32, smithay::utils::Buffer> = Size::from((size.w, size.h));
    let mut offscreen =
        Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size_buf).ok()?;
    let rendered = {
        let mut framebuffer = renderer.bind(&mut offscreen).ok()?;
        let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
        damage
            .render_output(
                renderer,
                &mut framebuffer,
                0,
                scene,
                crate::render::CLEAR_COLOR,
            )
            .is_ok()
    };
    if !rendered {
        return None;
    }

    let buffer = TextureBuffer::from_texture(renderer, offscreen, 1, Transform::Normal, None);
    let dst = Size::from((size.w, size.h));
    let tex = TextureRenderElement::from_texture_buffer(
        Point::<f64, Physical>::from((0.0, 0.0)),
        &buffer,
        None,
        Some(src),
        Some(dst),
        Kind::Unspecified,
    );
    Some(OutputStack::Wallpaper(tex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_cursor_doubles_hotspot() {
        let img = Image {
            size: 24,
            width: 24,
            height: 24,
            xhot: 4,
            yhot: 4,
            delay: 0,
            pixels_rgba: vec![255u8; 24 * 24 * 4],
            pixels_argb: Vec::new(),
        };
        let scaled = scale_cursor_image(&img, 2.0);
        assert_eq!(scaled.width, 48);
        assert_eq!(scaled.height, 48);
        assert_eq!(scaled.xhot, 8);
        assert_eq!(scaled.yhot, 8);
    }
}
