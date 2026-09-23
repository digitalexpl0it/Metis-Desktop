//! Night light — warm colour overlay driven by `outputs.json`.
//!
//! Settings toggles `night_light_enabled` and `night_light_temperature` (kelvin).
//! When active, a fullscreen warm-tinted layer is drawn above the desktop but
//! below the pointer (cursor is composited after the scene in the DRM path).

use metis_config::OutputsConfig;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::utils::{Physical, Rectangle, Size};

use crate::state::MetisState;

/// Physical size of the framebuffer currently being rendered to.
pub struct RenderTargetInfo<'a> {
    pub size: Size<i32, Physical>,
    /// Compositor output name when rendering a single output (DRM); `None` for the
    /// nested winit desktop or when unknown.
    pub output_name: Option<&'a str>,
    /// Omit the night-light warmth overlay (remote-desktop capture shows sRGB).
    pub skip_night_light: bool,
}

impl RenderTargetInfo<'_> {
    pub fn disabled() -> Self {
        Self {
            size: Size::from((0, 0)),
            output_name: None,
            skip_night_light: false,
        }
    }
}

/// Whether night light should tint the current render target.
pub fn night_light_active(cfg: &OutputsConfig, output_name: Option<&str>) -> bool {
    if !metis_config::night_light_effective(cfg) {
        return false;
    }
    // HDR signaling fights the warm SDR overlay — skip on HDR-active outputs.
    if let Some(name) = output_name {
        let prefs = metis_config::output_prefs(cfg, name);
        if prefs.hdr_enabled {
            return false;
        }
    }
    true
}

/// True when the warm overlay should be composited for `target`.
pub fn should_render_night_light(state: &MetisState, target: &RenderTargetInfo<'_>) -> bool {
    if target.skip_night_light {
        return false;
    }
    // Remote-desktop / portal capture must never pick up the warmth layer.
    if state.image_capture.screencast_active() || state.image_capture.has_pending() {
        return false;
    }
    if let Some(name) = target.output_name
        && crate::output_hdr::hdr_active_for_output(state, name)
    {
        return false;
    }
    let cfg = state.output_runtime.cached();
    night_light_active(cfg, target.output_name)
}

/// Map colour temperature (2700–6500 K) to a warm overlay (straight RGBA, not yet
/// premultiplied). Lower kelvin = warmer/stronger shift; 6500 K is barely visible.
pub fn overlay_color_for_temperature(kelvin: u32) -> Color32F {
    let kelvin = kelvin.clamp(2700, 6500);
    let strength = (6500 - kelvin) as f32 / (6500 - 2700) as f32;
    // Blend toward a soft incandescent white — not saturated orange.
    let r = 1.0f32;
    let g = 0.86 - strength * 0.26;
    let b = 0.78 - strength * 0.58;
    // Opacity after premultiply: ~2% at 6500 K, ~18% at 2700 K (GNOME-like).
    let alpha = 0.02 + strength * 0.16;
    Color32F::from([r, g, b, alpha])
}

/// Smithay's solid shader uses `ONE, ONE_MINUS_SRC_ALPHA` — colour must be
/// premultiplied or low-alpha overlays blow out to full-strength RGB.
pub fn premultiply(color: Color32F) -> Color32F {
    let a = color.a();
    Color32F::new(color.r() * a, color.g() * a, color.b() * a, a)
}

pub fn night_light_element(
    state: &MetisState,
    target: &RenderTargetInfo<'_>,
) -> Option<SolidColorRenderElement> {
    let cfg = state.output_runtime.cached();
    if !night_light_active(cfg, target.output_name) {
        return None;
    }
    // A fullscreen overlay tints the whole framebuffer and prevents direct scanout.
    if state.output_has_fullscreen(target.output_name) {
        return None;
    }
    if target.size.w <= 0 || target.size.h <= 0 {
        return None;
    }
    let color = premultiply(overlay_color_for_temperature(cfg.night_light_temperature));
    Some(SolidColorRenderElement::new(
        state.night_light_id.clone(),
        Rectangle::from_size(target.size),
        state.night_light_commit,
        color,
        Kind::Unspecified,
    ))
}

pub fn maybe_tick_schedule(state: &mut MetisState) {
    let cfg = state.output_runtime.cached();
    if !cfg.night_light_schedule.enabled {
        if state.night_light_schedule_effective.take().is_some() {
            state.night_light_commit.increment();
            state.schedule_redraw();
        }
        return;
    }
    let effective = metis_config::night_light_effective(cfg);
    if state.night_light_schedule_effective.replace(effective) != Some(effective) {
        state.night_light_commit.increment();
        state.schedule_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_kelvin_is_minimal_wash() {
        let c = premultiply(overlay_color_for_temperature(6500));
        assert!(c.a() < 0.03, "6500 K should be a very light tint");
    }

    #[test]
    fn warm_kelvin_is_premultiplied_and_subtle() {
        let raw = overlay_color_for_temperature(2700);
        let c = premultiply(raw);
        assert!(c.r() <= c.a() + 0.001);
        assert!(c.a() <= 0.20, "max warmth stays readable");
        assert!(
            raw.b() < raw.g() && raw.g() <= raw.r(),
            "warm hue before premult"
        );
    }

    #[test]
    fn warmer_is_stronger_than_cooler() {
        let warm = premultiply(overlay_color_for_temperature(2700)).a();
        let cool = premultiply(overlay_color_for_temperature(5500)).a();
        assert!(warm > cool);
    }
}
