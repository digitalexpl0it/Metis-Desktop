//! Edge-bar geometry reflow when bar position or auto-hide settings change.
//! Sibling `impl MetisState` extracted from `state.rs` (mechanical split; no
//! behavior change).

use crate::state::MetisState;

impl MetisState {
    /// Re-apply window geometry after the edge bar moves between reserved
    /// (top) and overlay (bottom/left/right) modes.
    pub fn reflow_for_bar_geometry_change(&mut self) {
        let ids: Vec<u32> = self.windows.ids();
        for id in ids {
            if self.windows.is_minimized(id) {
                continue;
            }
            if self.windows.get(id).is_some_and(|r| r.maximized) {
                self.reapply_maximized_geometry(id);
            } else if self.windows.is_snapped(id) {
                self.reflow_snapped_window(id);
            } else {
                self.apply_window_rect(id);
            }
        }
        self.sync_all_app_windows();
        self.refresh_all_scroll_offsets();
        self.arrange_layers();
        self.restore_focus_stacking();
        self.schedule_redraw();
    }

    /// Force every mapped toplevel to receive a configure (same size) so clients
    /// redraw after a DRM modeset or output layout change. Without this, GTK
    /// windows often keep a fully transparent buffer while SSD borders still draw.
    pub(crate) fn nudge_clients_after_output_change(&mut self) {
        let ids: Vec<u32> = self.windows.ids();
        for id in ids {
            if self.windows.is_minimized(id) {
                continue;
            }
            let Some(record) = self.windows.get(id).cloned() else {
                continue;
            };
            if !self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                continue;
            }
            let Some(loc) = self.space.element_location(&record.window) else {
                continue;
            };
            let size = record.window.geometry().size;
            self.send_window_configure(&record, loc, size);
        }
        self.schedule_redraw();
    }

    /// Re-clamp a snapped (non-maximized) window into the new placement zone after
    /// the edge bar moves.
    fn reflow_snapped_window(&mut self, id: u32) {
        let Some(mut rect) = self.windows.target_rect(id) else {
            return;
        };
        rect = self.clamp_rect_on_screen(rect);
        self.windows.set_target_rect(id, rect);
        self.apply_window_rect(id);
        self.reclamp_auto_hide(id);
    }

    /// Called when the bar layer commits; reflows windows if position changed.
    /// Also keeps the auto-hide cache warm (CSS hide does not always re-commit).
    pub(crate) fn on_bar_layer_committed(&mut self, output: &smithay::output::Output) {
        let cfg = metis_config::load_bar_config();
        let pos = cfg.position;
        if pos != self.last_bar_position {
            tracing::info!(?pos, "edge bar position changed — reflowing windows");
            self.last_bar_position = pos;
            self.blur.position = pos;
            self.reflow_for_bar_geometry_change();
        }
        self.sync_bar_auto_hidden_for(output);
    }

    /// Poll the shell auto-hide flag for input/hit-testing. Maximize/snap layout
    /// does not reflow on peek/reveal — the bar overlays. Reflow only when the
    /// `auto_hide` setting itself is toggled.
    pub(crate) fn maybe_sync_bar_auto_hidden(&mut self) {
        let cfg = metis_config::load_bar_config();
        if cfg.auto_hide != self.last_bar_auto_hide_enabled {
            self.last_bar_auto_hide_enabled = cfg.auto_hide;
            if !cfg.auto_hide {
                self.bar_auto_hidden.clear();
            }
            tracing::info!(
                auto_hide = cfg.auto_hide,
                "edge bar auto_hide setting changed — reflowing windows"
            );
            self.reflow_for_bar_geometry_change();
        }
        if !cfg.auto_hide {
            return;
        }
        let hidden = metis_protocol::bar_auto_hidden_flag();
        for output in self.space.outputs() {
            if self.output_has_bar(output) {
                self.bar_auto_hidden.insert(output.name(), hidden);
            }
        }
    }

    fn sync_bar_auto_hidden_for(&mut self, output: &smithay::output::Output) {
        let hidden = metis_protocol::bar_auto_hidden_flag();
        self.bar_auto_hidden.insert(output.name(), hidden);
    }
}
