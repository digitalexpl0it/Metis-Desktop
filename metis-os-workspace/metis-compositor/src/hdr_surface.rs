//! Per-surface HDR content awareness (Wave 3c + urgent #2 decode).
//!
//! Clients that negotiate `wp_color_management_v1` image descriptions with a
//! PQ / HLG transfer function are treated as HDR content surfaces.
//!
//! On an HDR output:
//! - **HdrOnly** (every mapped window on the output is HDR, or a fullscreen HDR
//!   client covers it) → pass-through: skip SDR→HDR encode so client PQ/HLG is
//!   not double-transformed.
//! - **Mixed** (SDR chrome / windows alongside HDR) → decode each HDR window into
//!   display-referred sRGB before compositing, then run the normal encode pass
//!   so SDR content stays correct on the HDR panel.
//!
//! On an SDR output, HDR windows are always decoded (there is no pass-through).
//!
//! Requires `METIS_COLOR_MGMT=1` (or default-on after upstream wayland-rs fix)
//! for clients to advertise descriptions.

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::seat::WaylandFocus;

use crate::color_management::NamedTransferFunction;
use crate::hdr_encode::HdrTransfer;
use crate::output_hdr::query_hdr_active;
use crate::state::MetisState;

/// How HDR client content relates to the windows on one output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdrContentMode {
    /// No mapped HDR surfaces on this output.
    #[default]
    None,
    /// Every counted window is HDR, or a fullscreen HDR client covers the output.
    HdrOnly,
    /// At least one HDR and one SDR window share the output.
    Mixed,
}

impl HdrContentMode {
    /// Skip the SDR→HDR encode post-pass (client buffers already PQ/HLG).
    pub fn allows_passthrough(self, output_hdr_active: bool) -> bool {
        output_hdr_active && matches!(self, Self::HdrOnly)
    }

    /// Decode HDR window buffers into sRGB before compositing.
    pub fn needs_surface_decode(self, output_hdr_active: bool) -> bool {
        match self {
            Self::None => false,
            // HDR-only on an HDR panel: pass through. On SDR: must decode.
            Self::HdrOnly => !output_hdr_active,
            Self::Mixed => true,
        }
    }
}

/// True when the named TF is a HDR transfer (ST.2084 PQ or HLG).
pub fn is_hdr_transfer(tf: Option<NamedTransferFunction>) -> bool {
    matches!(
        tf,
        Some(NamedTransferFunction::St2084Pq) | Some(NamedTransferFunction::Hlg)
    )
}

/// Map a colour-management TF hint to the encode/decode transfer enum.
pub fn hdr_transfer_from_tf(tf: NamedTransferFunction) -> Option<HdrTransfer> {
    match tf {
        NamedTransferFunction::St2084Pq => Some(HdrTransfer::Pq),
        NamedTransferFunction::Hlg => Some(HdrTransfer::Hlg),
        NamedTransferFunction::Srgb | NamedTransferFunction::Gamma22 => None,
    }
}

impl MetisState {
    /// Classify HDR vs SDR client windows visible on `output_name`.
    pub fn hdr_content_mode_for_output(&self, output_name: Option<&str>) -> HdrContentMode {
        let mut hdr_windows = 0u32;
        let mut sdr_windows = 0u32;
        let mut fullscreen_hdr = false;

        for window in self.space.elements() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.windows.is_minimized(id) {
                continue;
            }
            if let Some(name) = output_name {
                let on_output = self.space.outputs().any(|o| {
                    o.name().as_str() == name && self.space.outputs_for_element(window).contains(o)
                });
                if !on_output {
                    continue;
                }
            }

            let Some(surface) = window.wl_surface() else {
                sdr_windows = sdr_windows.saturating_add(1);
                continue;
            };
            if surface_has_hdr_hint(self, &surface.id()) {
                hdr_windows = hdr_windows.saturating_add(1);
                if self.windows.get(id).is_some_and(|r| r.fullscreen) {
                    fullscreen_hdr = true;
                }
            } else {
                sdr_windows = sdr_windows.saturating_add(1);
            }
        }

        if hdr_windows == 0 {
            return HdrContentMode::None;
        }
        if fullscreen_hdr || sdr_windows == 0 {
            HdrContentMode::HdrOnly
        } else {
            HdrContentMode::Mixed
        }
    }

    /// Whether this output should decode HDR client windows while building the stack.
    pub fn should_decode_hdr_surfaces(&self, output_name: Option<&str>) -> bool {
        let mode = self.hdr_content_mode_for_output(output_name);
        let hdr_active = output_name.is_some_and(|n| query_hdr_active(self, n));
        mode.needs_surface_decode(hdr_active)
    }

    /// Content peak (nits) for the mixed-HDR decode tone-map shoulder.
    ///
    /// When the output is in HDR mode, use the advertised mastering peak; otherwise
    /// fall back to a typical PQ/HLG grade peak so highlights still roll off softly.
    pub fn hdr_decode_content_max_nits(&self, output_name: Option<&str>) -> f32 {
        if output_name.is_some_and(|n| query_hdr_active(self, n)) {
            crate::hdr_encode::OUTPUT_MASTERING_PEAK_NITS
        } else {
            crate::hdr_encode::DEFAULT_CONTENT_MAX_NITS
        }
    }

    /// HDR transfer hint for a window's root surface, when present.
    pub fn window_hdr_transfer(&self, window: &smithay::desktop::Window) -> Option<HdrTransfer> {
        let surface = window.wl_surface()?;
        let (tf, _) = self.color_mgmt.surface_colour_hint(&surface.id())?;
        hdr_transfer_from_tf(tf?)
    }
}

fn surface_has_hdr_hint(state: &MetisState, surface_id: &ObjectId) -> bool {
    let Some((tf, _)) = state.color_mgmt.surface_colour_hint(surface_id) else {
        return false;
    };
    is_hdr_transfer(tf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_only_when_hdr_output_and_hdr_only() {
        assert!(!HdrContentMode::None.allows_passthrough(true));
        assert!(!HdrContentMode::Mixed.allows_passthrough(true));
        assert!(!HdrContentMode::HdrOnly.allows_passthrough(false));
        assert!(HdrContentMode::HdrOnly.allows_passthrough(true));
    }

    #[test]
    fn decode_on_sdr_for_hdr_only_and_always_for_mixed() {
        assert!(!HdrContentMode::None.needs_surface_decode(false));
        assert!(!HdrContentMode::None.needs_surface_decode(true));
        assert!(HdrContentMode::HdrOnly.needs_surface_decode(false));
        assert!(!HdrContentMode::HdrOnly.needs_surface_decode(true));
        assert!(HdrContentMode::Mixed.needs_surface_decode(false));
        assert!(HdrContentMode::Mixed.needs_surface_decode(true));
    }

    #[test]
    fn tf_mapping() {
        assert_eq!(
            hdr_transfer_from_tf(NamedTransferFunction::St2084Pq),
            Some(HdrTransfer::Pq)
        );
        assert_eq!(
            hdr_transfer_from_tf(NamedTransferFunction::Hlg),
            Some(HdrTransfer::Hlg)
        );
        assert_eq!(hdr_transfer_from_tf(NamedTransferFunction::Srgb), None);
    }
}
