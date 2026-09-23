//! Per-surface HDR content awareness (Wave 3c).
//!
//! Clients that negotiate `wp_color_management_v1` image descriptions with a
//! PQ / HLG transfer function are treated as HDR content surfaces. When such a
//! surface is mapped and the output is in HDR mode, the colour post-pass skips
//! the SDR→HDR encode so already-encoded buffers are not double-transformed
//! (pass-through into the existing scanout path).
//!
//! Mixed SDR + HDR clients on one output remain approximate: the compositor
//! prefers pass-through when any HDR surface is visible. Full per-surface
//! tone-map into a shared intermediate is a follow-up.
//!
//! Requires `METIS_COLOR_MGMT=1` (or default-on after upstream wayland-rs fix)
//! for clients to advertise descriptions.

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::seat::WaylandFocus;

use crate::color_management::NamedTransferFunction;
use crate::state::MetisState;

/// True when the named TF is a HDR transfer (ST.2084 PQ or HLG).
pub fn is_hdr_transfer(tf: Option<NamedTransferFunction>) -> bool {
    matches!(
        tf,
        Some(NamedTransferFunction::St2084Pq) | Some(NamedTransferFunction::Hlg)
    )
}

impl MetisState {
    /// Recompute whether any mapped window surface currently carries an HDR
    /// image description. Called from the colour post-pass / render path.
    pub fn refresh_hdr_content_flag(&mut self) {
        let mut any = false;
        for window in self.space.elements() {
            let Some(surface) = window.wl_surface() else {
                continue;
            };
            if surface_has_hdr_hint(self, &surface.id()) {
                any = true;
                break;
            }
        }
        self.hdr_client_content_visible = any;
    }
}

fn surface_has_hdr_hint(state: &MetisState, surface_id: &ObjectId) -> bool {
    let Some((tf, _)) = state.color_mgmt.surface_colour_hint(surface_id) else {
        return false;
    };
    is_hdr_transfer(tf)
}
