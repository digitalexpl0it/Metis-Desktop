use smithay::desktop::{LayerSurface, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{
    Layer as WlrLayer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::handlers::xdg_shell;
use crate::state::MetisState;

impl WlrLayerShellHandler for MetisState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        _layer: WlrLayer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.space.outputs().next().unwrap().clone());

        tracing::info!(namespace, "mapping layer surface");
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            tracing::warn!(%err, "failed to map layer surface");
        }
        self.schedule_redraw();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let target = self.space.outputs().find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned()?;
            Some((o.clone(), layer))
        });
        if let Some((output, layer)) = target {
            let mut map = layer_map_for_output(&output);
            map.unmap_layer(&layer);
        }
    }

    /// GTK layer-shell menus use `zwlr_layer_surface_v1.get_popup`, not a bare xdg parent.
    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        xdg_shell::track_popup(self, popup);
    }
}

pub fn handle_layer_commit(state: &mut MetisState, surface: &WlSurface) {
    let Some(output) = state.space.outputs().find(|o| {
        let map = layer_map_for_output(o);
        map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some()
    }) else {
        return;
    };
    let output = output.clone();

    let initial_configure_sent = with_states(surface, |states| {
        states
            .data_map
            .get::<LayerSurfaceData>()
            .unwrap()
            .lock()
            .unwrap()
            .initial_configure_sent
    });

    let mut map = layer_map_for_output(&output);
    let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL) else {
        return;
    };
    let namespace = layer.namespace().to_string();
    let layer_surface = layer.layer_surface().clone();

    // Always arrange the Metis bar on commit so anchor/margin changes (e.g. switching
    // between top/bottom/left/right in settings) update layer geometry. Skipping
    // arrange left the surface stuck at its initial top-strip size/position.
    if namespace.starts_with("metis-bar") {
        let exclusive = layer.cached_state().keyboard_interactivity
            == smithay::wayland::shell::wlr_layer::KeyboardInteractivity::Exclusive;
        map.arrange();
        if !initial_configure_sent {
            tracing::debug!(namespace, "layer surface initial configure");
            layer_surface.send_configure();
        }
        drop(map);
        state.on_bar_layer_committed(&output);
        // Super-opened menus set Exclusive without a pointer click — claim seat
        // focus as soon as the layer commits the new interactivity.
        if exclusive {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(layer) = state.exclusive_keyboard_layer()
                && let Some(keyboard) = state.seat.get_keyboard()
            {
                keyboard.set_focus(
                    state,
                    Some(crate::focus::KeyboardFocusTarget::from(layer)),
                    serial,
                );
            }
        }
        state.schedule_redraw();
        return;
    }

    if namespace == "metis-screenshot" {
        map.arrange();
        if !initial_configure_sent {
            tracing::debug!(namespace, "layer surface initial configure");
            layer_surface.send_configure();
        }
        drop(map);
        if state.screenshot_overlay_active() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            state.focus_screenshot_overlay(serial);
        }
        state.schedule_redraw();
        return;
    }

    map.arrange();

    if !initial_configure_sent {
        tracing::debug!(namespace, "layer surface initial configure");
        layer_surface.send_configure();
    }
}
