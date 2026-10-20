//! `wp_fractional_scale_v1` + `wl_surface.preferred_buffer_scale` (compositor v6).
//!
//! Output scale already drives render/layout; this module advertises the matching
//! client-facing preferred scales (Anvil-style).

use smithay::{
    desktop::utils::{surface_primary_scanout_output, with_surfaces_surface_tree},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        compositor::{get_parent, send_surface_state, with_states},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        seat::WaylandFocus,
    },
};

use crate::state::MetisState;

/// Push fractional + integer preferred scales for one surface from `output`.
pub(crate) fn apply_scales_from_output(surface: &WlSurface, output: &Output) {
    let scale_f = output.current_scale().fractional_scale();
    let scale_i = output.current_scale().integer_scale();
    let transform = output.current_transform();
    with_states(surface, |states| {
        with_fractional_scale(states, |fractional_scale| {
            fractional_scale.set_preferred_scale(scale_f);
        });
        send_surface_state(surface, states, scale_i, transform);
    });
}

impl MetisState {
    /// Refresh preferred scales from the surface's primary scan-out output, if any.
    pub(crate) fn refresh_surface_scales(&self, surface: &WlSurface) {
        with_states(surface, |states| {
            if let Some(output) = surface_primary_scanout_output(surface, states) {
                let scale_f = output.current_scale().fractional_scale();
                let scale_i = output.current_scale().integer_scale();
                let transform = output.current_transform();
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(scale_f);
                });
                send_surface_state(surface, states, scale_i, transform);
            }
        });
    }

    /// After a frame (or output scale change): update preferred scales for every
    /// surface shown on `output` (windows, layers, client cursor).
    pub(crate) fn refresh_scales_on_output(&self, output: &Output) {
        for window in self.space.elements() {
            window.with_surfaces(|surface, states| {
                let primary = surface_primary_scanout_output(surface, states);
                let use_output = primary
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or_else(|| self.space.outputs_for_element(window).contains(output));
                if use_output {
                    let src = primary.as_ref().unwrap_or(output);
                    let scale_f = src.current_scale().fractional_scale();
                    let scale_i = src.current_scale().integer_scale();
                    let transform = src.current_transform();
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(scale_f);
                    });
                    send_surface_state(surface, states, scale_i, transform);
                }
            });
        }

        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                let primary = surface_primary_scanout_output(surface, states);
                let src = primary.as_ref().unwrap_or(output);
                let scale_f = src.current_scale().fractional_scale();
                let scale_i = src.current_scale().integer_scale();
                let transform = src.current_transform();
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(scale_f);
                });
                send_surface_state(surface, states, scale_i, transform);
            });
        }
        drop(map);

        if let smithay::input::pointer::CursorImageStatus::Surface(ref surface) = self.cursor_status
        {
            with_surfaces_surface_tree(surface, |surface, _states| {
                self.refresh_surface_scales(surface);
            });
        }
    }

    /// Walk every connected output after a prefs-driven scale/layout change.
    pub(crate) fn refresh_all_surface_scales(&self) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in &outputs {
            self.refresh_scales_on_output(output);
        }
    }

    fn output_for_fractional_scale_surface(&self, surface: &WlSurface) -> Option<Output> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        with_states(surface, |states| {
            surface_primary_scanout_output(surface, states).or_else(|| {
                if root != *surface {
                    with_states(&root, |states| {
                        surface_primary_scanout_output(&root, states).or_else(|| {
                            self.window_for_wl_surface(&root).and_then(|window| {
                                self.space.outputs_for_element(&window).first().cloned()
                            })
                        })
                    })
                } else {
                    self.window_for_wl_surface(&root)
                        .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
                }
            })
        })
        .or_else(|| self.space.outputs().next().cloned())
    }

    fn window_for_wl_surface(&self, surface: &WlSurface) -> Option<smithay::desktop::Window> {
        self.space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == *surface))
            .cloned()
    }
}

impl FractionalScaleHandler for MetisState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        if let Some(output) = self.output_for_fractional_scale_surface(&surface) {
            apply_scales_from_output(&surface, &output);
        }
    }
}

/// Stable `ext_foreign_toplevel_handle` identifier for a Metis window id.
pub(crate) fn foreign_toplevel_identifier(id: u32) -> String {
    format!("metis-{id}")
}

#[cfg(test)]
mod tests {
    use super::foreign_toplevel_identifier;

    #[test]
    fn foreign_identifier_fits_protocol_limits() {
        let id = foreign_toplevel_identifier(u32::MAX);
        assert!(!id.is_empty());
        assert!(id.len() <= 32);
        assert!(id.is_ascii());
        assert_eq!(id, "metis-4294967295");
    }
}
