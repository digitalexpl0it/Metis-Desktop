//! Dim-on-battery overlay driven by `power.json` → `dim_on_battery`.
//!
//! When the laptop is on battery and the preference is enabled, a light black
//! wash sits above the desktop (same stacking slot as night light). HDR-active
//! outputs skip the overlay so PQ/HLG metadata is not washed out.

use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::utils::Rectangle;

use crate::night_light::{RenderTargetInfo, premultiply};
use crate::state::MetisState;

/// Soft black wash (~12% after premultiply) — readable, not a blank.
const DIM_ALPHA: f32 = 0.12;

/// Cached power preference + last battery sample for the dim overlay.
pub struct BatteryDimRuntime {
    pub dim_on_battery: bool,
    /// Last `on_battery()` sample; polled on a slow timer.
    pub on_battery: bool,
    pub id: Id,
    pub commit: smithay::backend::renderer::utils::CommitCounter,
}

impl BatteryDimRuntime {
    pub fn new(dim_on_battery: bool) -> Self {
        Self {
            dim_on_battery,
            on_battery: metis_config::on_battery(),
            id: Id::new(),
            commit: smithay::backend::renderer::utils::CommitCounter::default(),
        }
    }

    pub fn apply_config(&mut self, dim_on_battery: bool) -> bool {
        if self.dim_on_battery == dim_on_battery {
            return false;
        }
        self.dim_on_battery = dim_on_battery;
        self.commit.increment();
        true
    }

    /// Refresh AC/battery sample. Returns true when the effective dim state changed.
    pub fn poll_battery(&mut self) -> bool {
        let now = metis_config::on_battery();
        if now == self.on_battery {
            return false;
        }
        self.on_battery = now;
        self.commit.increment();
        true
    }

    pub fn active(&self) -> bool {
        self.dim_on_battery && self.on_battery
    }
}

pub fn should_render_battery_dim(state: &MetisState, target: &RenderTargetInfo<'_>) -> bool {
    if !state.battery_dim.active() {
        return false;
    }
    if target.skip_night_light {
        // Same capture carve-out as night light — remote viewers get undimmed sRGB.
        return false;
    }
    if state.image_capture.screencast_active() || state.image_capture.has_pending() {
        return false;
    }
    if let Some(name) = target.output_name
        && crate::output_hdr::hdr_active_for_output(state, name)
    {
        return false;
    }
    if state.output_has_fullscreen(target.output_name) {
        return false;
    }
    target.size.w > 0 && target.size.h > 0
}

pub fn battery_dim_element(
    state: &MetisState,
    target: &RenderTargetInfo<'_>,
) -> Option<SolidColorRenderElement> {
    if !should_render_battery_dim(state, target) {
        return None;
    }
    let color = premultiply(Color32F::from([0.0, 0.0, 0.0, DIM_ALPHA]));
    Some(SolidColorRenderElement::new(
        state.battery_dim.id.clone(),
        Rectangle::from_size(target.size),
        state.battery_dim.commit,
        color,
        Kind::Unspecified,
    ))
}
