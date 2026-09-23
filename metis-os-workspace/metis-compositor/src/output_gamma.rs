//! Per-output hardware colour calibration.
//!
//! Uploads the `vcgt` calibration ramp from an output's ICC profile
//! (`outputs.json` `color_profile`) into that CRTC's GPU gamma ramp via DRM
//! `set_gamma`. This is the Stage 1 path (what colord/GNOME do for `vcgt`-only
//! profiles). Stage 2 GLES 3D-LUT ([`crate::color_lut`]) takes over when a LUT
//! bake succeeds — then this path uploads an identity ramp so TRC is not
//! double-applied.

use smithay::reexports::drm::control::{Device as DrmControlDevice, crtc};

use crate::color_management::vcgt::{self, GammaRamps};
use crate::state::MetisState;

/// Re-upload gamma ramps for every enabled output. Safe to call on any
/// `ReloadOutputs`, after a mode-set, and on VT resume. No-op under the nested
/// winit backend (there is no DRM device).
///
/// When Stage 2 GLES LUT owns an output (full ICC transform), CRTC `vcgt` is
/// skipped so TRC is not applied twice.
pub fn apply_output_gamma(state: &MetisState) {
    let Some(udev) = state.udev.as_ref() else {
        return;
    };
    for backend in udev.backends.values() {
        for (crtc, surface) in &backend.surfaces {
            let name = surface.output.name();
            if !state.is_output_enabled(&name) {
                continue;
            }
            // Hardware gamma on a PQ signal skews colour (warm/muddy cast). Keep
            // the CRTC ramp identity while HDR encode is active. Same when the
            // GLES 3D LUT already includes the profile TRC.
            let icc = if surface.hdr_active || state.color_lut.lut_owns_output(&name) {
                None
            } else {
                state.color_mgmt.icc_bytes_for_output(&name)
            };
            sync_gamma_for_crtc(backend, *crtc, &name, icc.as_deref());
        }
    }
}

fn sync_gamma_for_crtc(
    backend: &crate::udev::BackendData,
    crtc: crtc::Handle,
    name: &str,
    icc: Option<&[u8]>,
) {
    let device = backend.drm_output_manager.device();
    let gamma_length = match device.get_crtc(crtc) {
        Ok(info) => info.gamma_length() as usize,
        Err(err) => {
            tracing::debug!(output = %name, ?err, "gamma: get_crtc failed; skipping");
            return;
        }
    };
    if gamma_length == 0 {
        // Driver reports no programmable gamma ramp for this CRTC.
        return;
    }

    let ramps = build_ramps(icc, name, gamma_length);
    match device.set_gamma(crtc, &ramps.r, &ramps.g, &ramps.b) {
        Ok(()) => {
            tracing::info!(
                output = %name,
                entries = gamma_length,
                calibrated = icc.is_some(),
                "gamma: applied CRTC ramp"
            );
        }
        Err(err) => {
            // Some drivers reject legacy gamma under atomic; don't let it cascade.
            tracing::warn!(output = %name, ?err, "gamma: set_gamma failed");
        }
    }
}

/// Resolve the ramps to upload: the profile's `vcgt` when present and parseable,
/// otherwise an identity ramp of `len` entries.
fn build_ramps(icc: Option<&[u8]>, name: &str, len: usize) -> GammaRamps {
    let parsed = match icc {
        Some(bytes) => match vcgt::parse_vcgt(bytes) {
            Ok(Some(ramps)) => Some(ramps),
            Ok(None) => {
                tracing::debug!(output = %name, "gamma: profile has no vcgt tag; using identity");
                None
            }
            Err(err) => {
                tracing::warn!(output = %name, %err, "gamma: failed to parse vcgt; using identity");
                None
            }
        },
        None => None,
    };

    match parsed {
        Some(ramps) => GammaRamps {
            r: resample(&ramps.r, len),
            g: resample(&ramps.g, len),
            b: resample(&ramps.b, len),
        },
        None => {
            let id = identity(len);
            GammaRamps {
                r: id.clone(),
                g: id.clone(),
                b: id,
            }
        }
    }
}

/// Linear-interpolate a source ramp to exactly `target` entries (the CRTC's
/// gamma length).
fn resample(src: &[u16], target: usize) -> Vec<u16> {
    if target == 0 {
        return Vec::new();
    }
    if src.is_empty() {
        return identity(target);
    }
    if src.len() == target {
        return src.to_vec();
    }
    if target == 1 {
        return vec![*src.last().unwrap_or(&0)];
    }
    let last = (src.len() - 1) as f64;
    (0..target)
        .map(|i| {
            let pos = i as f64 * last / (target as f64 - 1.0);
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(src.len() - 1);
            let frac = pos - lo as f64;
            let a = src[lo] as f64;
            let b = src[hi] as f64;
            (a + (b - a) * frac).round().clamp(0.0, 65535.0) as u16
        })
        .collect()
}

/// A neutral ramp: `0..=65535` spread evenly across `len` entries.
fn identity(len: usize) -> Vec<u16> {
    if len == 0 {
        return Vec::new();
    }
    if len == 1 {
        return vec![65535];
    }
    (0..len)
        .map(|i| ((i as u64 * 65535) / (len as u64 - 1)) as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_spans_full_range() {
        let id = identity(256);
        assert_eq!(id.first(), Some(&0));
        assert_eq!(id.last(), Some(&65535));
        assert_eq!(id.len(), 256);
    }

    #[test]
    fn resample_upscales_endpoints() {
        let out = resample(&[0, 65535], 256);
        assert_eq!(out.len(), 256);
        assert_eq!(out.first(), Some(&0));
        assert_eq!(out.last(), Some(&65535));
    }

    #[test]
    fn resample_identity_when_same_len() {
        let src = vec![1, 2, 3, 4];
        assert_eq!(resample(&src, 4), src);
    }

    #[test]
    fn resample_handles_degenerate_targets() {
        assert!(resample(&[5, 6, 7], 0).is_empty());
        assert_eq!(resample(&[5, 6, 7], 1), vec![7]);
    }
}
