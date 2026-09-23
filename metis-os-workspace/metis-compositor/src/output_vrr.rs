//! Per-output variable refresh rate (VRR / adaptive sync) via DRM `VRR_ENABLED`.

use metis_config::{OutputsConfig, output_prefs};
use smithay::backend::drm::VrrSupport;

use crate::state::MetisState;

pub fn vrr_available(support: VrrSupport) -> bool {
    !matches!(support, VrrSupport::NotSupported)
}

pub fn query_vrr_support(state: &MetisState, name: &str) -> VrrSupport {
    let Some(udev) = state.udev.as_ref() else {
        return VrrSupport::NotSupported;
    };
    let Some(surface) = udev.surfaces().find(|s| s.output.name() == name) else {
        return VrrSupport::NotSupported;
    };
    surface.drm_output.with_compositor(|compositor| {
        match compositor.vrr_supported(surface.connector) {
            Ok(s) => s,
            Err(err) => {
                tracing::debug!(%name, ?err, "VRR support query failed");
                VrrSupport::NotSupported
            }
        }
    })
}

pub fn query_vrr_active(state: &MetisState, name: &str) -> bool {
    let Some(udev) = state.udev.as_ref() else {
        return false;
    };
    let Some(surface) = udev.surfaces().find(|s| s.output.name() == name) else {
        return false;
    };
    surface
        .drm_output
        .with_compositor(|compositor| compositor.vrr_enabled())
}

pub fn apply_output_vrrs(state: &mut MetisState, cfg: &OutputsConfig) -> bool {
    let mut changed = false;
    for output in state.connected_outputs() {
        if !state.is_output_enabled(&output.name()) {
            continue;
        }
        let prefs = output_prefs(cfg, &output.name());
        if sync_vrr_for_output(state, &output.name(), prefs.vrr_enabled) {
            changed = true;
        }
    }
    changed
}

/// Ensure the next frame on `id` uses the saved VRR preference.
pub fn prepare_vrr_for_render(state: &MetisState, id: crate::udev::UdevOutputId) {
    let Some(udev) = state.udev.as_ref() else {
        return;
    };
    let Some(name) = udev.surface(id).map(|s| s.output.name()) else {
        return;
    };
    let want = output_prefs(state.output_runtime.cached(), &name).vrr_enabled;
    let _ = sync_vrr_for_crtc(state, id, want);
}

fn sync_vrr_for_output(state: &mut MetisState, name: &str, want: bool) -> bool {
    let id = state.udev.as_ref().and_then(|u| u.output_id_by_name(name));
    let Some(id) = id else {
        return false;
    };
    sync_vrr_for_crtc(state, id, want)
}

fn sync_vrr_for_crtc(state: &MetisState, id: crate::udev::UdevOutputId, want: bool) -> bool {
    let Some(udev) = state.udev.as_ref() else {
        return false;
    };
    let Some(surface) = udev.surface(id) else {
        return false;
    };

    surface.drm_output.with_compositor(|compositor| {
        if compositor.vrr_enabled() == want {
            return false;
        }
        if want {
            match compositor.vrr_supported(surface.connector) {
                Ok(s) if vrr_available(s) => {}
                _ => return false,
            }
        }
        match compositor.use_vrr(want) {
            Ok(()) => {
                tracing::info!(output = %surface.output.name(), vrr = want, "applied VRR");
                true
            }
            Err(err) => {
                tracing::warn!(
                    output = %surface.output.name(),
                    want,
                    ?err,
                    "failed to set VRR"
                );
                false
            }
        }
    })
}
