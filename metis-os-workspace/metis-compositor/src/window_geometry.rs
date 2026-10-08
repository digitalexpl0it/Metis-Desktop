//! Window geometry: snap regions, placement zones, fullscreen / maximize /
//! minimize, and bar-aware clamping. Sibling `impl MetisState` extracted from
//! `state.rs` (mechanical split; no behavior change).

use metis_grid::{PixelRect, app_tile_body_rect};
use smithay::{
    desktop::layer_map_for_output,
    utils::{Logical, Point, Size},
};

use crate::focus::KeyboardFocusTarget;
use crate::state::{MIN_VISIBLE_PX, MetisState, WINDOW_GAP_PX, ZoneGaps};

impl MetisState {
    /// Snap a grid-managed window back to its tile body if the client moved or resized it.
    pub fn enforce_grid_window_geometry(&mut self, id: u32) {
        if !self.is_window_grid_managed(id) {
            return;
        }
        let Some(expected) = self
            .rect_for_window_tile(id)
            .map(|full| self.tile_client_rect(id, full))
        else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let drifted = self
            .space
            .element_location(&record.window)
            .is_none_or(|loc| loc.x != expected.x || loc.y != expected.y)
            || record.window.geometry().size
                != smithay::utils::Size::from((expected.width, expected.height))
            || record.target_rect != expected;
        if drifted {
            self.apply_window_rect(id);
        }
    }

    /// Compute the snap-zone target for a pointer at global-logical (`x`, `y`),
    /// in pixel space against the usable area (so the top edge maximizes below
    /// the bar). Returns the final *client* rect (gaps already applied to match
    /// the maximize look) + label, or `None` when the pointer isn't near an edge.
    pub fn snap_target_at(&self, x: i32, y: i32) -> Option<(PixelRect, &'static str)> {
        // Snap against the output the pointer is over, so dragging a window to a
        // secondary monitor's edge tiles it on *that* monitor.
        let place = match self.output_at(Point::from((x, y))) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let pos = metis_config::load_bar_config().position;

        // Snap geometry is computed in `place` (beside the bar). When the pointer
        // hugs the physical monitor edge over an overlay bar, project it onto the
        // nearest `place` edge so left/right/bottom snaps still fire.
        let mut sx = x;
        let mut sy = y;
        match pos {
            metis_config::BarPosition::Left if x < place.x => sx = place.x,
            metis_config::BarPosition::Right if x > place.x + place.width => {
                sx = place.x + place.width;
            }
            metis_config::BarPosition::Bottom if y > place.y + place.height => {
                sy = place.y + place.height;
            }
            _ => {}
        }

        let (raw, label) = metis_grid::pixel_snap_target(sx, sy, place)?;
        let gaps = self.zone_edge_gaps();
        Some((snap_client_rect(raw, place, gaps), label))
    }

    /// Drop a window into a snap zone. The "Maximize" zone routes through the real
    /// `set_maximized` so it's pixel-identical to the titlebar maximize button.
    /// Half / quarter zones float the window and mark it *tiled* (all four edges)
    /// so GTK squares its corners and drops its drop-shadow, filling the snapped
    /// rect exactly — otherwise the leftover CSD shadow makes the padding look
    /// uneven from edge to edge.
    pub fn apply_snap(&mut self, id: u32, rect: PixelRect, label: &str) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        if label == "Maximize" {
            self.set_maximized(id, true);
            return;
        }

        // Re-home desk membership to the monitor this snap targets *before*
        // applying geometry. Doing this after the snap (via `maybe_adopt`) used
        // to run `clamp_floating_rect`, adding a spurious titlebar inset on
        // auto-hide edge snaps dragged across outputs.
        let snap_center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        if let Some(output) = self.output_at(snap_center) {
            let key = output.name();
            if key != self.desk_key_for_window(id) {
                self.move_window_to_output_inner(id, &key, false);
            }
        }

        self.capture_pre_snap_geometry(id);

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.space.raise_element(&record.window, true);
        self.floating.insert(id);
        self.windows.set_maximized(id, false);

        // Snaps whose top edge meets the bar auto-hide the titlebar as a hover
        // overlay on the client's top strip.
        let top_touching = matches!(label, "Left half" | "Right half" | "Top-left" | "Top-right");
        let uses_ssd = self.window_uses_ssd(id);
        let body = if uses_ssd {
            if top_touching && self.should_auto_hide_titlebar(id) {
                metis_grid::app_tile_auto_hide_body_rect(rect)
            } else {
                app_tile_body_rect(rect)
            }
        } else {
            rect
        };
        let size = Size::from((body.width.max(1), body.height.max(1)));
        let loc = Point::from((body.x, body.y));
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::TiledLeft);
                state.states.set(xdg_toplevel::State::TiledRight);
                state.states.set(xdg_toplevel::State::TiledTop);
                state.states.set(xdg_toplevel::State::TiledBottom);
                state.size = Some(size);
            });
        }
        self.space.map_element(record.window.clone(), loc, true);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, body);
        if uses_ssd && top_touching && self.should_auto_hide_titlebar(id) {
            self.auto_hide_titlebar.insert(id);
        } else {
            self.clear_auto_hide(id);
        }
        self.reclamp_auto_hide(id);
        self.windows.set_snapped(id, true);
        self.save_window_geometry(id);
        tracing::info!(id, ?rect, label, "snap: window snapped to zone");
    }

    /// Clear the tiled states a snap applied, so a window pulled off a snapped
    /// position regains its normal floating chrome (GTK rounded corners + drop
    /// shadow). `send_pending_configure` is a no-op when nothing actually changed.
    pub(crate) fn clear_tiled_states(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(State::TiledLeft);
                state.states.unset(State::TiledRight);
                state.states.unset(State::TiledTop);
                state.states.unset(State::TiledBottom);
            });
            toplevel.send_pending_configure();
        }
    }

    /// Drop a window's auto-hide-titlebar state (e.g. on unmaximize, unsnap,
    /// minimize, fullscreen, or close), clearing the reveal if it was showing.
    pub fn clear_auto_hide(&mut self, id: u32) {
        self.auto_hide_titlebar.remove(&id);
        if self.revealed_titlebar == Some(id) {
            self.revealed_titlebar = None;
        }
        if self.titlebar_reveal_window == Some(id) {
            self.titlebar_reveal_window = None;
            self.titlebar_reveal_progress = 0.0;
        }
    }

    /// Tabbed browsers and other CSD clients keep native chrome; SSD windows use
    /// the hover overlay when grid-tiled, snapped, or maximized.
    pub(crate) fn sync_auto_hide_titlebar(&mut self, id: u32) {
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let ssd = self.should_auto_hide_titlebar(id) && self.should_draw_metis_ssd(id);
        if !ssd {
            return;
        }
        let grid_tiled = self.tile_id_for_window(id).is_some() && !self.floating.contains(&id);
        let maximized_or_snapped = record.maximized || record.snapped;
        let tabbed_floating = ssd
            && self.window_uses_compact_overlay(id)
            && self.floating.contains(&id)
            && !maximized_or_snapped;
        let snap_auto_hide =
            ssd && record.snapped && !record.maximized && self.should_auto_hide_titlebar(id);
        let maximized_auto_hide =
            ssd && record.maximized && self.maximized_uses_auto_hide_titlebar(id);
        let should_overlay = grid_tiled || maximized_auto_hide || snap_auto_hide || tabbed_floating;
        if should_overlay {
            let was_auto_hide = self.auto_hide_titlebar.contains(&id);
            self.auto_hide_titlebar.insert(id);
            if !was_auto_hide && record.maximized && ssd {
                self.reapply_maximized_geometry(id);
            }
        } else if self.auto_hide_titlebar.contains(&id) {
            self.auto_hide_titlebar.remove(&id);
        }
    }

    /// Live client-surface (body) geometry for a mapped window.
    pub(crate) fn window_body_rect(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        let loc = self.space.element_location(&record.window)?;
        let size = record.window.geometry().size;
        Some(PixelRect {
            x: loc.x,
            y: loc.y,
            width: size.w.max(1),
            height: size.h.max(1),
        })
    }

    /// Remember a floating window's size/position before the first snap in a chain.
    fn capture_pre_snap_geometry(&mut self, id: u32) {
        if self.windows.is_snapped(id) {
            return;
        }
        let Some(body) = self.window_body_rect(id) else {
            return;
        };
        self.windows.set_restore_rect(id, body);
    }

    /// Pull a snapped/maximized window back to its pre-snap floating size when the
    /// user starts dragging it by the titlebar. Keeps the grab point under the
    /// pointer so the window doesn't jump.
    pub(crate) fn restore_floating_from_snap(
        &mut self,
        id: u32,
        pointer: Point<f64, Logical>,
    ) -> Point<i32, Logical> {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return Point::default();
        };

        let current = self.window_body_rect(id).unwrap_or(record.target_rect);
        let restore = self.windows.take_restore_rect(id).unwrap_or(current);

        self.clear_auto_hide(id);
        self.windows.set_maximized(id, false);
        self.windows.set_snapped(id, false);

        let rel_x = if current.width > 0 {
            (pointer.x - current.x as f64) / current.width as f64
        } else {
            0.5
        };
        let rel_y = if current.height > 0 {
            (pointer.y - current.y as f64) / current.height as f64
        } else {
            0.0
        };
        let rel_x = rel_x.clamp(0.0, 1.0);
        let rel_y = rel_y.clamp(0.0, 1.0);

        let mut body = PixelRect {
            x: (pointer.x - rel_x * restore.width as f64).round() as i32,
            y: (pointer.y - rel_y * restore.height as f64).round() as i32,
            width: restore.width.max(1),
            height: restore.height.max(1),
        };
        body = self.clamp_floating_rect_for(id, body);

        let loc = Point::from((body.x, body.y));
        let size = Size::from((body.width, body.height));
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.unset(xdg_toplevel::State::TiledLeft);
                state.states.unset(xdg_toplevel::State::TiledRight);
                state.states.unset(xdg_toplevel::State::TiledTop);
                state.states.unset(xdg_toplevel::State::TiledBottom);
                state.size = Some(size);
                state.fullscreen_output = None;
            });
        }
        self.space.map_element(record.window.clone(), loc, true);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, body);
        self.schedule_redraw();
        loc
    }

    /// Per-edge outward ripple (top, right, bottom, left) in logical px during the
    /// post-maximize wobble. Zero when the effect has finished.
    pub(crate) fn maximize_wobble_offset(&self, id: u32) -> (i32, i32) {
        const DURATION_SECS: f32 = 0.55;
        const AMP_PX: f32 = 14.0;
        const FREQ_HZ: f32 = 5.5;

        let Some(start) = self.maximize_fx_started.get(&id) else {
            return (0, 0);
        };
        let elapsed = start.elapsed().as_secs_f32();
        if elapsed >= DURATION_SECS {
            return (0, 0);
        }
        let decay = (1.0_f32 - elapsed / DURATION_SECS).powi(2);
        let amp = AMP_PX * decay;
        let phase = elapsed * FREQ_HZ * std::f32::consts::TAU;
        (
            (amp * phase.sin()).round() as i32,
            (amp * (phase * 1.37).sin()).round() as i32,
        )
    }

    fn window_base_location(&self, id: u32) -> Option<Point<i32, Logical>> {
        let record = self.windows.get(id)?;
        if record.maximized {
            return self
                .maximized_client_geometry(id)
                .map(|(client, _)| Point::from((client.x, client.y)));
        }
        Some(Point::from((record.target_rect.x, record.target_rect.y)))
    }

    fn apply_maximize_wobble(&mut self, id: u32) {
        let (dx, dy) = self.maximize_wobble_offset(id);
        if dx == 0 && dy == 0 && !self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(base) = self.window_base_location(id) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id))
        {
            return;
        }
        self.space
            .relocate_element(&record.window, Point::from((base.x + dx, base.y + dy)));
        self.schedule_redraw();
    }

    fn snap_maximize_wobble(&mut self, id: u32) {
        let Some(base) = self.window_base_location(id) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id))
        {
            self.space.relocate_element(&record.window, base);
            self.schedule_redraw();
        }
    }

    /// End post-maximize wobble for `id` and snap the map origin back to base.
    pub(crate) fn clear_maximize_fx(&mut self, id: u32) {
        if self.maximize_fx_started.remove(&id).is_some() {
            self.snap_maximize_wobble(id);
        }
    }

    fn start_maximize_fx(&mut self, id: u32) {
        if !crate::window_fx::animations_enabled() {
            return;
        }
        if self.window_uses_compact_overlay(id) {
            return;
        }
        // Do not restart an in-flight wobble — key-repeat / rapid toggles used to
        // reset the timer forever so relocate fought drags and stalled the loop.
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        self.maximize_fx_started
            .insert(id, std::time::Instant::now());
        self.apply_maximize_wobble(id);
    }

    /// Advance post-maximize wobble animations. Returns true while any are active.
    pub fn tick_maximize_fx(&mut self) -> bool {
        const DURATION_SECS: f32 = 0.55;
        // Do **not** cancel on any pointer grab — a titlebar maximize click can
        // leave a short-lived button grab that would kill the wobble on the
        // first tick. Move/resize paths call [`Self::clear_maximize_fx`] when
        // they take ownership of the map origin.
        let active_ids: Vec<u32> = self.maximize_fx_started.keys().copied().collect();
        for id in active_ids {
            self.apply_maximize_wobble(id);
        }
        let ended: Vec<u32> = self
            .maximize_fx_started
            .iter()
            .filter(|(_, t)| t.elapsed().as_secs_f32() >= DURATION_SECS)
            .map(|(id, _)| *id)
            .collect();
        for id in ended {
            self.maximize_fx_started.remove(&id);
            self.snap_maximize_wobble(id);
        }
        !self.maximize_fx_started.is_empty()
    }

    pub(crate) fn window_uses_compact_overlay(&self, id: u32) -> bool {
        self.windows
            .get(id)
            .and_then(|r| r.app_id.as_deref())
            .is_some_and(crate::decoration_policy::id_uses_compact_overlay)
    }

    fn minimize_visual_bounds(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        let loc = self.space.element_location(&record.window)?;
        if let Some(frame) = self.ssd_frame_for_mapped_window(id, &record.window) {
            return Some(frame);
        }
        let geo = record.window.geometry();
        Some(PixelRect {
            x: loc.x,
            y: loc.y,
            width: geo.size.w.max(1),
            height: geo.size.h.max(1),
        })
    }

    fn genie_minimize_target(&self, anchor: &PixelRect, id: u32) -> Option<Point<i32, Logical>> {
        let output = self
            .output_for_window(id)
            .or_else(|| self.primary_output())?;
        if !self.output_has_bar(&output) {
            return None;
        }
        let cfg = metis_config::load_bar_config();
        let margin = cfg.margin_top as i32;
        let half = cfg.height as i32 / 2;
        let zone = self.placement_zone_for(&output);
        let usable = self.usable_zone_for(&output).unwrap_or(zone);
        let cx = (anchor.x + anchor.width / 2).clamp(zone.x + 40, zone.x + zone.width - 40);
        let cy = anchor.y + anchor.height / 2;
        Some(match cfg.position {
            metis_config::BarPosition::Top => Point::from((cx, usable.y - margin - half)),
            metis_config::BarPosition::Bottom => {
                Point::from((cx, zone.y + zone.height - margin - half))
            }
            metis_config::BarPosition::Left => Point::from((zone.x + margin + half, cy)),
            metis_config::BarPosition::Right => {
                Point::from((zone.x + zone.width - margin - half, cy))
            }
        })
    }

    fn begin_minimize_genie(&mut self, id: u32) -> bool {
        if self.windows.is_minimized(id) {
            return false;
        }
        let anchor = if self.windows.get(id).is_some_and(|r| r.maximized) {
            self.maximized_client_geometry(id).map(|(c, _)| c)
        } else {
            self.minimize_visual_bounds(id)
        };
        let Some(anchor) = anchor else {
            return false;
        };
        let Some(target) = self.genie_minimize_target(&anchor, id) else {
            return false;
        };
        self.minimize_genie_fx.insert(
            id,
            crate::window_fx::MinimizeGenieFx {
                started: std::time::Instant::now(),
                anchor,
                target,
            },
        );
        self.schedule_redraw();
        true
    }

    pub(crate) fn tick_minimize_genie_fx(&mut self) -> bool {
        let finished: Vec<u32> = self
            .minimize_genie_fx
            .iter()
            .filter(|(_, fx)| fx.finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            self.minimize_genie_fx.remove(&id);
            self.minimize_window_now(id);
        }
        !self.minimize_genie_fx.is_empty()
    }

    /// Render clip + alpha for an in-flight minimize genie, if any.
    pub(crate) fn minimize_genie_render(&self, id: u32) -> Option<(PixelRect, f32)> {
        self.minimize_genie_fx.get(&id).map(|fx| fx.frame())
    }

    pub(crate) fn is_minimize_genie_active(&self, id: u32) -> bool {
        self.minimize_genie_fx.contains_key(&id)
    }

    /// Usable desktop band for grid tiling on `output` (below/ beside the edge bar).
    pub(crate) fn grid_placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        let mut zone = self.window_placement_zone_for(output);
        if !self.output_has_bar(output) {
            return zone;
        }
        let Some(output_geo) = self.output_rect(output) else {
            return zone;
        };
        let reserve = self.bar_reserved_px_for(output);
        let gaps = self.zone_edge_gaps();
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                let min_y = output_geo.y + reserve + gaps.top;
                if zone.y < min_y {
                    let delta = min_y - zone.y;
                    zone.y = min_y;
                    zone.height = (zone.height - delta).max(1);
                }
            }
            metis_config::BarPosition::Bottom => {
                zone.height = (zone.height - gaps.bottom).max(1);
            }
            metis_config::BarPosition::Left => {
                let min_x = output_geo.x + reserve + gaps.left;
                if zone.x < min_x {
                    let delta = min_x - zone.x;
                    zone.x = min_x;
                    zone.width = (zone.width - delta).max(1);
                }
            }
            metis_config::BarPosition::Right => {
                zone.width = (zone.width - gaps.right).max(1);
            }
        }
        zone
    }

    pub fn set_fullscreen(
        &mut self,
        id: u32,
        enabled: bool,
        requested_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        use smithay::reexports::wayland_server::Resource;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if self.windows.is_minimized(id) {
            self.unminimize_window(id);
        }

        // XWayland fullscreen goes through the dedicated X11 path (which sets the
        // X11 fullscreen property + reconfigures the surface); the Wayland
        // `xdg_toplevel`-state body below does not apply to it.
        if let Some(x11) = record.x11().cloned() {
            let output_for_event = requested_output
                .as_ref()
                .and_then(|wl| self.output_for_wl_output(wl))
                .or_else(|| self.output_for_window(id))
                .or_else(|| self.primary_output());
            if enabled {
                self.apply_x11_fullscreen(x11);
            } else {
                self.apply_x11_unfullscreen(x11);
            }
            self.windows.set_fullscreen(id, enabled);
            if let Some(output) = output_for_event {
                self.event_bus
                    .emit(&metis_protocol::CompositorEvent::WindowFullscreen {
                        id,
                        fullscreen: enabled,
                        output: output.name(),
                    });
            }
            self.schedule_redraw();
            return;
        }

        let output_for_event = if enabled {
            requested_output
                .as_ref()
                .and_then(|wl| self.output_for_wl_output(wl))
                .or_else(|| self.output_for_window(id))
                .or_else(|| self.primary_output())
        } else if record.fullscreen {
            self.output_for_window(id).or_else(|| self.primary_output())
        } else {
            None
        };

        let output_name_for_event = output_for_event.as_ref().map(smithay::output::Output::name);

        if enabled {
            let output = output_for_event.clone().or_else(|| self.primary_output());
            let Some(output) = output else {
                return;
            };
            let Some(geo) = self.space.output_geometry(&output) else {
                tracing::warn!(
                    output = %output.name(),
                    "fullscreen: output has no geometry"
                );
                return;
            };
            let wl_surface = record.wl_toplevel().map(|t| t.wl_surface().clone());
            let wl_output = wl_surface.as_ref().and_then(|wl_surface| {
                self.display_handle
                    .get_client(wl_surface.id())
                    .ok()
                    .and_then(|client| output.client_outputs(&client).next())
            });

            let current = self.window_body_rect(id).unwrap_or(record.target_rect);
            self.windows
                .set_pre_fullscreen_maximized(id, record.maximized);
            // Keep the pre-maximize floating geometry when entering fullscreen
            // from a maximized window — we re-maximize on exit instead.
            if !record.maximized {
                self.windows.set_restore_rect(id, current);
            }
            self.floating.insert(id);

            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.states.set(xdg_toplevel::State::Fullscreen);
                    state.size = Some(geo.size);
                    state.fullscreen_output = wl_output;
                    // Fullscreen has no chrome — grant server-side so a CSD
                    // toolkit (libdecor / GLFW games) drops its own frame on the
                    // very first fullscreen configure instead of leaving a stale
                    // titlebar+shadow that reports a negative window-geometry
                    // inset and shifts the surface off the output origin.
                    state.decoration_mode =
                        Some(crate::decoration_policy::grant_decoration_mode(true));
                });
            }
            self.space.map_element(record.window.clone(), geo.loc, true);
            self.windows.set_fullscreen(id, true);
            self.windows.set_maximized(id, false);
            self.clear_auto_hide(id);
            self.note_output_fullscreen(&output, id, true);
            self.focus_window_id(id);
        } else {
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.fullscreen_output = None;
                    // Restore the windowed decoration mode negotiated for this
                    // client so a CSD app gets its own frame back on exit.
                    state.decoration_mode = Some(crate::decoration_policy::grant_decoration_mode(
                        record.uses_ssd,
                    ));
                });
            }
            self.windows.set_fullscreen(id, false);
            let was_maximized = self.windows.take_pre_fullscreen_maximized(id);
            // Clear from every output's set: the window may have been fullscreen
            // on a different output than `output_for_event` resolves to now.
            self.drop_window_fullscreen(id);
            if was_maximized {
                let _ = self.windows.take_restore_rect(id);
                self.set_maximized(id, true);
                self.reclamp_maximized_geometry(id);
            } else if self.tile_id_for_window(id).is_some() {
                self.floating.remove(&id);
                let _ = self.windows.take_restore_rect(id);
                self.apply_window_rect(id);
            } else {
                self.restore_floating_from_transient(id);
                self.apply_window_rect(id);
            }
        }

        if let Some(output_name) = output_name_for_event {
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowFullscreen {
                    id,
                    fullscreen: enabled,
                    output: output_name,
                });
        }

        if let Some(record) = self.windows.get(id)
            && let Some(toplevel) = record.wl_toplevel()
        {
            toplevel.send_pending_configure();
        }
        self.schedule_redraw();
    }

    /// Record (or clear) a window's true-fullscreen state on an output and drive
    /// the edge bar's visibility. The bar hides while the output's fullscreen set
    /// is non-empty and reappears the instant it empties. Keyed by window id so
    /// entering twice is idempotent and a stray leave for an unknown id is a
    /// no-op — the counter model this replaced could drift out of sync and leave
    /// the bar hidden forever after a game exited while fullscreen.
    pub(crate) fn note_output_fullscreen(
        &mut self,
        output: &smithay::output::Output,
        id: u32,
        entering: bool,
    ) {
        use metis_protocol::CompositorEvent;

        let name = output.name();
        if entering {
            let set = self
                .output_fullscreen_windows
                .entry(name.clone())
                .or_default();
            let was_empty = set.is_empty();
            set.insert(id);
            if was_empty {
                self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                    output: name,
                    visible: false,
                });
            }
        } else {
            let Some(set) = self.output_fullscreen_windows.get_mut(&name) else {
                return;
            };
            if set.remove(&id) && set.is_empty() {
                self.output_fullscreen_windows.remove(&name);
                self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                    output: name,
                    visible: true,
                });
            }
        }
    }

    /// Unconditionally drop a window from every output's fullscreen set (used on
    /// teardown). Unlike `note_output_fullscreen(.., false)`, this does not need
    /// the caller to know which output the window was fullscreen on, nor does it
    /// depend on the window's registry `fullscreen` flag still being set — so a
    /// window that is destroyed or withdrawn while fullscreen always releases its
    /// hold on the bar. Re-shows the edge bar for any output whose set empties.
    pub(crate) fn drop_window_fullscreen(&mut self, id: u32) {
        use metis_protocol::CompositorEvent;

        self.fs_offset_warned.remove(&id);
        let mut emptied = Vec::new();
        for (name, set) in self.output_fullscreen_windows.iter_mut() {
            if set.remove(&id) && set.is_empty() {
                emptied.push(name.clone());
            }
        }
        for name in emptied {
            self.output_fullscreen_windows.remove(&name);
            self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                output: name,
                visible: true,
            });
        }
    }

    pub fn set_maximized(&mut self, id: u32, enabled: bool) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if self.windows.is_minimized(id) {
            self.unminimize_window(id);
        }

        if enabled {
            if record.maximized && !record.fullscreen {
                // Already maximized: leave an in-flight wobble alone. Checking
                // map location would see the wobble offset as "wrong" and clear
                // the FX on every redundant set_maximized(true).
                if self.maximize_fx_started.contains_key(&id) {
                    return;
                }
                if let Some((client, client_size)) = self.maximized_client_geometry(id) {
                    let loc = Point::from((client.x, client.y));
                    let at_loc = self.space.element_location(&record.window) == Some(loc);
                    let size_ok = record.window.geometry().size == client_size;
                    let rect_ok = self.windows.target_rect(id) == Some(client);
                    if at_loc && size_ok && rect_ok {
                        return;
                    }
                }
            }

            // Finish any in-flight wobble before remapping so restore/max geometry
            // is not applied on top of a stale offset (and so a fresh FX can start).
            self.clear_maximize_fx(id);

            // Mark maximized before any nested layout/configure work so bulk
            // `apply_window_rect` passes cannot reposition this window back into
            // its grid tile mid-transition.
            self.windows.set_maximized(id, true);

            let current = self.window_body_rect(id).unwrap_or(record.target_rect);
            self.windows.set_restore_rect(id, current);

            if self.maximized_uses_auto_hide_titlebar(id) {
                self.auto_hide_titlebar.insert(id);
            } else {
                self.clear_auto_hide(id);
            }

            let Some((client, client_size)) = self.maximized_client_geometry(id) else {
                return;
            };

            let loc = Point::from((client.x, client.y));
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some(client_size);
                    state.fullscreen_output = None;
                });
            } else if let Some(x11) = record.x11() {
                let _ = x11.set_maximized(true);
            }
            self.space.map_element(record.window.clone(), loc, true);
            self.send_window_configure(&record, loc, client_size);
            self.windows.set_rect(id, client);
            self.reclamp_auto_hide(id);
            self.windows.set_snapped(id, true);
            self.sync_auto_hide_titlebar(id);
            self.start_maximize_fx(id);
        } else {
            self.demote_maximized(id);
            // Match the maximize path: remapping with `activate: true` forces the
            // freshly restored window above any neighbor that kept a stale
            // full-screen stack slot after `relocate_element`-only demotion.
            if let Some(record) = self.windows.get(id).cloned()
                && let Some(loc) = self.space.element_location(&record.window)
            {
                self.space.map_element(record.window.clone(), loc, true);
            }
        }

        // Wayland maximize/demote set pending states above; flush them (X11 was
        // already reconfigured via `send_window_configure` / `demote_maximized`).
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.send_pending_configure();
        }
        self.focus_window_id(id);
        self.restore_focus_stacking();
        self.schedule_redraw();
    }

    /// Client body rect + configure for a window already marked maximized. Does
    /// not change focus or re-raise unless the caller does so afterward.
    pub(crate) fn reapply_maximized_geometry(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.maximized {
            return;
        }
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        if self.maximized_uses_auto_hide_titlebar(id) {
            self.auto_hide_titlebar.insert(id);
        } else {
            self.clear_auto_hide(id);
        }
        let Some((client, client_size)) = self.maximized_client_geometry(id) else {
            return;
        };
        let loc = Point::from((client.x, client.y));
        let current_loc = self.space.element_location(&record.window);
        let current_size = record.window.geometry().size;
        if current_loc == Some(loc)
            && current_size == client_size
            && self.windows.target_rect(id) == Some(client)
        {
            return;
        }
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(client_size);
                state.fullscreen_output = None;
            });
        }
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        if mapped {
            self.space.relocate_element(&record.window, loc);
        } else {
            self.space.map_element(record.window.clone(), loc, false);
        }
        self.windows.set_rect(id, client);
        self.reclamp_auto_hide(id);
        self.send_window_configure(&record, loc, client_size);
    }

    /// Usable-zone footprint for a maximized window on its current output.
    fn maximized_client_geometry(&self, id: u32) -> Option<(PixelRect, Size<i32, Logical>)> {
        let zone = match self.output_for_window(id) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let gaps = self.zone_edge_gaps();
        let full = PixelRect {
            x: zone.x + gaps.left,
            y: zone.y + gaps.top,
            width: (zone.width - gaps.left - gaps.right).max(1),
            height: (zone.height - gaps.top - gaps.bottom).max(1),
        };
        let client = if self.window_uses_ssd(id) {
            self.ssd_client_rect(id, full)
        } else {
            full
        };
        let client_size = Size::from((client.width.max(1), client.height.max(1)));
        Some((client, client_size))
    }

    /// Drop a window out of maximized mode without stealing focus (internal).
    fn demote_maximized(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.maximized {
            return;
        }
        // Snap out of any in-flight wobble before restoring geometry.
        self.clear_maximize_fx(id);
        // Clear before `apply_window_rect` — while `maximized` is still true that
        // path returns immediately and the window stays at its maximized map origin
        // (often tucked under the edge bar once chrome is restored).
        self.windows.set_maximized(id, false);
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            });
        } else if let Some(x11) = record.x11() {
            let _ = x11.set_maximized(false);
        }
        self.clear_tiled_states(id);
        self.clear_auto_hide(id);
        self.windows.set_snapped(id, false);
        if self.tile_id_for_window(id).is_some() {
            // Grid apps return to their tile instead of staying ad-hoc floating.
            self.floating.remove(&id);
        } else {
            self.floating.insert(id);
            self.restore_floating_from_transient(id);
        }
        self.sync_auto_hide_titlebar(id);
        self.apply_window_rect(id);
        self.start_maximize_fx(id);
    }

    /// Toggle maximize for the focused window, debounced against key-repeat.
    /// Returns `true` when the keybind should be consumed.
    pub fn toggle_maximized_debounced(&mut self, id: u32) -> bool {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
        let now = std::time::Instant::now();
        if self
            .last_maximize_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return true;
        }
        self.last_maximize_toggle = Some(now);
        let maxed = self.windows.get(id).map(|w| w.maximized).unwrap_or(false);
        self.set_maximized(id, !maxed);
        true
    }

    pub fn minimize_window(&mut self, id: u32) {
        if self.minimize_genie_fx.contains_key(&id) {
            return;
        }
        if crate::window_fx::animations_enabled() && self.begin_minimize_genie(id) {
            return;
        }
        self.minimize_window_now(id);
    }

    fn minimize_window_now(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.size = None;
                state.fullscreen_output = None;
            });
            toplevel.send_pending_configure();
        }

        self.space.unmap_elem(&record.window);
        self.windows.set_minimized(id, true);
        self.windows.set_maximized(id, false);
        self.windows.set_fullscreen(id, false);
        self.clear_auto_hide(id);

        if self.focused_window_id() == Some(id) {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            match self.seat.get_keyboard() {
                Some(keyboard) => {
                    keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
                }
                _ => {
                    tracing::warn!("minimize_window: seat has no keyboard");
                }
            }
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowMinimized {
                id,
                minimized: true,
            });
    }

    pub(crate) fn unminimize_window(&mut self, id: u32) {
        self.windows.set_minimized(id, false);
        self.apply_window_rect(id);
        if self.preferred_stacking_window() == Some(id) {
            self.raise_stacking_window(id, true);
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowMinimized {
                id,
                minimized: false,
            });
    }

    /// Minimize a window by id, routing grid tiles through `set_tile_mode` (so the
    /// tile's mode stays consistent) and floating windows directly. Mirrors the
    /// decoration minimize button.
    pub fn minimize_by_id(&mut self, id: u32) {
        if let Some(tile_id) = self.tile_id_for_window(id) {
            self.set_tile_mode(&tile_id, metis_protocol::TileMode::Minimized);
        } else {
            self.minimize_window(id);
        }
    }

    /// Restore a window by id (grid tiles back to Grid mode, floating windows via
    /// `unminimize_window`).
    pub fn restore_by_id(&mut self, id: u32) {
        if !self.windows.is_minimized(id) {
            return;
        }
        if let Some(tile_id) = self.tile_id_for_window(id) {
            // Grid restore clears minimized inside `set_tile_mode`; still force
            // unminimize if the tile path left the flag set (e.g. mode already Grid).
            self.set_tile_mode(&tile_id, metis_protocol::TileMode::Grid);
            if self.windows.is_minimized(id) {
                self.unminimize_window(id);
            }
        } else {
            self.unminimize_window(id);
        }
    }

    /// Bring a window to the foreground: restore if minimized, raise, and focus.
    pub fn activate_window_by_id(&mut self, id: u32) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        // Ignore Exclusive shell layers (Alt+Tab overlay) so activation is not
        // treated as a no-op while the switcher still owns the seat briefly.
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if ws != self.active_workspace_for(&key) {
            self.switch_workspace(&key, ws);
        }
        self.restore_by_id(id);
        self.note_window_focus(id);
        self.ensure_app_tile_for_window(id);
        self.remap_window_for_desktop(id);
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.raise_stacking_window(id, true);
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        match self.seat.get_keyboard() {
            Some(keyboard) => {
                keyboard.set_focus(self, Some(record.window.clone().into()), serial);
            }
            _ => {
                tracing::warn!("activate_window_by_id: seat has no keyboard");
            }
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
        // Do not call `restore_focus_stacking` here — it can re-raise a different
        // preferred window (e.g. after Exclusive layer teardown) and undo the
        // user's Alt+Tab selection.
        self.schedule_redraw();
    }

    /// Map, unmap, or refresh a window for the active workspace on its output.
    /// Grid tiles, floating geometry, maximize, and fullscreen each have their
    /// own placement path; hidden workspaces always unmap.
    pub(crate) fn remap_window_for_desktop(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if self.windows.is_minimized(id) {
            return;
        }
        if !self.window_visible_on_desktop(id) {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        }
        if record.maximized {
            self.reapply_maximized_geometry(id);
            return;
        }
        if record.fullscreen {
            self.reapply_fullscreen_geometry(id);
            return;
        }
        self.apply_window_rect(id);
    }

    fn reapply_fullscreen_geometry(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        use smithay::reexports::wayland_server::Resource;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.fullscreen {
            return;
        }
        let Some(output) = self.output_for_window(id).or_else(|| self.primary_output()) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        // XWayland fullscreen geometry is owned by the X11 configure path.
        let Some(toplevel) = record.wl_toplevel() else {
            return;
        };
        let wl_surface = toplevel.wl_surface().clone();
        let wl_output = self
            .display_handle
            .get_client(wl_surface.id())
            .ok()
            .and_then(|client| output.client_outputs(&client).next());
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geo.size);
            state.fullscreen_output = wl_output;
        });
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        if mapped {
            self.space.relocate_element(&record.window, geo.loc);
        } else {
            self.space
                .map_element(record.window.clone(), geo.loc, false);
        }
        toplevel.send_pending_configure();
        self.schedule_redraw();
    }

    pub fn apply_window_rect(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if let Some(toplevel) = record.window.toplevel()
            && crate::grabs::resize_grab::surface_is_interactively_resizing(toplevel.wl_surface())
        {
            return;
        }
        // Never (re)map a minimized window. Restoring goes through
        // `unminimize_window`, which clears the flag *before* calling this. Without
        // this guard a bulk `reposition_all_windows` (triggered when restoring a
        // single grid tile) would re-map and un-minimize *every* minimized window.
        if self.windows.is_minimized(id) {
            return;
        }
        if !self.window_visible_on_desktop(id) {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        }
        // Maximized / fullscreen geometry is owned by `set_maximized` /
        // `set_fullscreen` / `reapply_*`. Bulk tile passes must not snap these
        // back to their grid slot.
        if record.maximized || record.fullscreen {
            return;
        }
        // Floating windows keep their free geometry (only recovered if they'd
        // land off every active output); grid windows snap to their tile.
        let rect = if self.floating.contains(&id) {
            // Auto-hide (snapped/maximized) windows map flush under the bar; only
            // ordinary floating windows reserve the titlebar strip above the body.
            // Borderless / near-fullscreen game floats stay at output origin — the
            // usable-zone clamp would inset them under the bar and clip the edges.
            let auto_hide = self.auto_hide_titlebar.contains(&id);
            self.windows.target_rect(id).map(|r| {
                let r = self.recover_offscreen_rect(r);
                if auto_hide {
                    r
                } else if self.rect_is_output_covering(r) {
                    if let Some(snap) = self.borderless_output_rect_for(id, r.width, r.height, true)
                    {
                        snap
                    } else {
                        r
                    }
                } else if self.x11_keep_on_full_output(id, r) {
                    // Large undecorated X11 games: do not inset under the edge bar.
                    r
                } else if self.should_draw_metis_ssd(id) {
                    self.clamp_body_below_bar(r)
                } else {
                    self.clamp_floating_rect_for(id, r)
                }
            })
        } else {
            self.rect_for_window_tile(id).and_then(|full| {
                let body = self.tile_client_rect(id, full);
                if self.is_active_scroll_window(id) {
                    let key = self.desk_key_for_window(id);
                    let zone = self.scroll_zone_for(&key);
                    if !body.intersects(&zone) {
                        return None;
                    }
                }
                Some(body)
            })
        };
        let Some(rect) = rect else {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        };
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        let loc = Point::from((rect.x, rect.y));
        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let size = Size::from((width, height));
        if mapped {
            let prev_loc = self.space.element_location(&record.window);
            let unchanged = prev_loc == Some(loc)
                && record.target_rect == rect
                && (record.window.geometry().size == size
                    || self.floating.contains(&id)
                    || self.is_active_scroll_window(id));
            if unchanged {
                return;
            }
            // `map_element` always inserts at the top of the stack, even with
            // `activate: false`, so routine layout sync must relocate in place
            // instead of unmap/remap — otherwise a grid reflow raises every
            // repositioned window above a maximized or focused one.
            if prev_loc != Some(loc) {
                self.space.relocate_element(&record.window, loc);
            }
            self.send_window_configure(&record, loc, size);
            self.windows.set_target_rect(id, rect);
            self.sync_auto_hide_titlebar(id);
            self.reclamp_auto_hide(id);
            self.schedule_redraw();
            return;
        }
        // First map — insert without stealing keyboard activation.
        self.space.map_element(record.window.clone(), loc, false);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, rect);
        // An auto-hide (maximized / edge-snapped) window may refuse to shrink to
        // its footprint; re-anchor it so the screen-edge gap survives.
        self.sync_auto_hide_titlebar(id);
        self.reclamp_auto_hide(id);
    }

    /// Keep an auto-hide (maximized / edge-snapped) window pinned to its snapped
    /// edge so the screen-edge gap survives even when the client refuses to
    /// shrink to its footprint (e.g. an app whose minimum width is wider than the
    /// snap zone on a small display). The footprint (`target_rect`) encodes the
    /// desired gaps; if the committed size is larger we re-anchor the window to
    /// the edge the footprint hugs so the overflow spills toward screen center
    /// instead of off the screen edge.
    pub fn reclamp_auto_hide(&mut self, id: u32) {
        if !self.auto_hide_titlebar.contains(&id) {
            return;
        }
        // A fullscreen window is mapped flush at its output origin by the
        // fullscreen path and its geometry is owned there — never by the
        // auto-hide (maximized / edge-snapped) footprint. Without this guard a
        // window that entered fullscreen while maximized (e.g. a game maximized
        // at launch, then F11'd) would be re-anchored to the gap-inset maximized
        // footprint on its next commit, shifting the fullscreen surface off the
        // origin and exposing the wallpaper along the top/left edge. Mirrors the
        // `!fullscreen` guard in `reclamp_maximized_geometry`.
        if self.windows.get(id).is_some_and(|r| r.fullscreen) {
            return;
        }
        // CSD overlay windows only use auto_hide for hover chrome — not geometry.
        if !self.should_draw_metis_ssd(id) {
            return;
        }
        // Post-maximize wobble temporarily offsets the map origin; reclamp would
        // snap it back every client commit and kill the animation.
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let Some(foot) = self.windows.target_rect(id) else {
            return;
        };
        let Some(loc) = self.space.element_location(&record.window) else {
            return;
        };
        let size = record.window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return;
        }
        // Anchor against the output the window sits on, not always the primary —
        // otherwise an auto-hide (maximized / edge-snapped) window on a secondary
        // monitor gets dragged back toward the primary output's zone.
        let zone = match self.output_for_window(id) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let gaps = self.zone_edge_gaps();
        let pos = metis_config::load_bar_config().position;
        let y_anchor = match pos {
            metis_config::BarPosition::Bottom => Some(BarEdgeAnchor::Max),
            metis_config::BarPosition::Top => Some(BarEdgeAnchor::Min),
            _ => None,
        };
        let x_anchor = match pos {
            metis_config::BarPosition::Left => Some(BarEdgeAnchor::Min),
            metis_config::BarPosition::Right => Some(BarEdgeAnchor::Max),
            _ => None,
        };
        let new_x = anchor_axis(
            foot.x, foot.width, zone.x, zone.width, size.w, gaps.left, gaps.right, x_anchor,
        );
        let new_y = anchor_axis(
            foot.y,
            foot.height,
            zone.y,
            zone.height,
            size.h,
            gaps.top,
            gaps.bottom,
            y_anchor,
        );
        if new_x != loc.x || new_y != loc.y {
            self.space
                .relocate_element(&record.window, Point::from((new_x, new_y)));
            self.schedule_redraw();
        }
    }

    /// Re-anchor a maximized window when a client (especially CSD Chromium/Cursor)
    /// commits the wrong origin *or* size. `apply_window_rect` skips maximized
    /// windows, so this runs on commit instead.
    ///
    /// Critical for bottom edge bars: clients often keep a full-output height
    /// while accepting the correct `y`, which paints under the bar. Detect that
    /// via `geometry().size` (not `bbox()` — shadows/subsurfaces routinely extend
    /// past the reserved bottom and would otherwise reconfigure forever, freezing
    /// the session while Settings or other maximized windows are open).
    pub fn reclamp_maximized_geometry(&mut self, id: u32) {
        if !self
            .windows
            .get(id)
            .is_some_and(|r| r.maximized && !r.fullscreen && !self.windows.is_minimized(id))
        {
            return;
        }
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let Some((expected, expected_size)) = self.maximized_client_geometry(id) else {
            return;
        };
        let Some(loc) = self.space.element_location(&record.window) else {
            return;
        };
        let expected_loc = Point::from((expected.x, expected.y));
        let current_size = record.window.geometry().size;
        // Geometry bottom (not bbox): bbox includes CSD shadows that intentionally
        // spill a few pixels past the client edge.
        let mapped_bottom = loc.y + current_size.h;
        let expected_bottom = expected.y + expected.height;
        let already_ok = loc == expected_loc
            && current_size == expected_size
            && self.windows.target_rect(id) == Some(expected);
        if already_ok {
            return;
        }
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(expected_size);
                state.fullscreen_output = None;
            });
        } else if let Some(x11) = record.x11() {
            let _ = x11.set_maximized(true);
        }
        self.space.relocate_element(&record.window, expected_loc);
        self.windows.set_rect(id, expected);
        self.send_window_configure(&record, expected_loc, expected_size);
        self.schedule_redraw();
        tracing::debug!(
            id,
            ?loc,
            ?expected_loc,
            current_w = current_size.w,
            current_h = current_size.h,
            expected_w = expected_size.w,
            expected_h = expected_size.h,
            mapped_bottom,
            expected_bottom,
            "reclamped maximized geometry after client commit"
        );
    }

    /// Gaps for maximize/snap/clamp. All edges use the configured `window_gap_px`
    /// (0 = flush). Bar reserve is applied separately via `bar_reserved_px_for`.
    pub(crate) fn zone_edge_gaps(&self) -> ZoneGaps {
        let g = self.configured_window_gap();
        ZoneGaps {
            top: g,
            bottom: g,
            left: g,
            right: g,
        }
    }

    /// Live maximize/snap padding from `bar.json` (`window_gap_px`, 0..=10).
    pub(crate) fn configured_window_gap(&self) -> i32 {
        metis_config::bar::window_gap_px(&metis_config::load_bar_config())
    }

    /// Pick up Settings changes to `window_gap_px` (~1s) and reflow maximized/
    /// snapped windows so the new padding applies without a restart.
    pub(crate) fn maybe_refresh_window_gap(&mut self) {
        if self.last_window_gap_check.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.last_window_gap_check = std::time::Instant::now();
        let gap = self.configured_window_gap();
        if gap == self.last_window_gap_px {
            return;
        }
        tracing::info!(gap, "window gap changed — reflowing windows");
        self.last_window_gap_px = gap;
        self.reflow_for_bar_geometry_change();
    }

    /// Full edge-bar strip from `bar.json` (margin + visible body). Used when
    /// auto-hide is off. Prefer [`bar_reserved_px_for`](Self::bar_reserved_px_for).
    fn bar_reserved_px_full() -> i32 {
        let cfg = metis_config::load_bar_config();
        cfg.margin_top as i32 + cfg.height as i32
    }

    /// Pixels reserved for the edge bar on `output`. With auto-hide enabled the
    /// bar overlays windows (always `0`) so peek/reveal never resizes maximize /
    /// snap geometry. With auto-hide off, reserves the full strip.
    fn bar_reserved_px_for(&self, _output: &smithay::output::Output) -> i32 {
        if metis_config::load_bar_config().auto_hide {
            0
        } else {
            Self::bar_reserved_px_full()
        }
    }

    /// Region for maximize, snap, and clamp. Subtracts the configured bar strip
    /// when auto-hide is off. With auto-hide the bar overlays, so the zone is the
    /// full output (windows still get `window_gap_px` via [`zone_edge_gaps`]).
    pub(crate) fn window_placement_zone(&self) -> PixelRect {
        match self.primary_output() {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.placement_zone(),
        }
    }

    /// Like [`window_placement_zone`](Self::window_placement_zone) but for a
    /// specific `output`, so snap/maximize/placement target the monitor a window
    /// (or the cursor) is on.
    pub(crate) fn window_placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        let mut zone = match self.output_rect(output) {
            Some(m) => PixelRect {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            },
            None => self.placement_zone_for(output),
        };
        if !self.output_has_bar(output) {
            return self.usable_zone_for(output).unwrap_or(zone);
        }
        let reserve = self.bar_reserved_px_for(output);
        if reserve <= 0 {
            return zone;
        }
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                zone.y += reserve;
                zone.height = (zone.height - reserve).max(1);
            }
            metis_config::BarPosition::Bottom => {
                zone.height = (zone.height - reserve).max(1);
            }
            metis_config::BarPosition::Left => {
                zone.x += reserve;
                zone.width = (zone.width - reserve).max(1);
            }
            metis_config::BarPosition::Right => {
                zone.width = (zone.width - reserve).max(1);
            }
        }
        zone
    }

    /// The output area not covered by exclusive layer-shell zones, in global logical coordinates.
    ///
    /// `BAR_GAP_PX` is the thin padding kept between the edge bar and any window so
    /// the bar's drop shadow has breathing room and nothing visually touches it.
    pub fn usable_zone(&self) -> Option<PixelRect> {
        let output = self.primary_output()?;
        self.usable_zone_for(&output)
    }

    /// The usable area of a specific `output` (its geometry minus that output's
    /// exclusive layer-shell zones), in global logical coordinates.
    pub fn usable_zone_for(&self, output: &smithay::output::Output) -> Option<PixelRect> {
        let zone = layer_map_for_output(output).non_exclusive_zone();
        let origin = self.space.output_geometry(output)?.loc;
        Some(PixelRect {
            x: zone.loc.x + origin.x,
            y: zone.loc.y + origin.y,
            width: zone.size.w,
            height: zone.size.h,
        })
    }

    /// The usable area, falling back to the full output if the bar zone isn't
    /// known yet, and finally to the configured monitor size. Always returns the
    /// main (first) output, so off-screen windows are recovered onto it.
    fn placement_zone(&self) -> PixelRect {
        match self.primary_output() {
            Some(output) => self.placement_zone_for(&output),
            None => {
                let monitor = self.monitor;
                PixelRect {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                }
            }
        }
    }

    /// Usable area of `output`, falling back to its full geometry if the bar
    /// zone isn't known yet, and finally to the configured monitor size.
    pub(crate) fn placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        // Maximize/snap prefer `window_placement_zone_for`. With auto-hide the
        // shell may still advertise a top/bottom exclusive zone — ignore it so
        // floating clamps match overlay maximize.
        let auto_hide = metis_config::load_bar_config().auto_hide;
        let mut zone = if let Some(zone) = self.usable_zone_for(output) {
            if self.output_has_bar(output) && auto_hide {
                match self.output_rect(output) {
                    Some(m) => PixelRect {
                        x: m.x,
                        y: m.y,
                        width: m.width,
                        height: m.height,
                    },
                    None => zone,
                }
            } else {
                zone
            }
        } else {
            let mut zone = {
                let monitor = self.output_rect(output).unwrap_or(self.monitor);
                PixelRect {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                }
            };
            // Layer-shell exclusive zone may not be committed yet at startup.
            if self.output_has_bar(output) {
                let reserve = self.bar_reserved_px_for(output);
                match metis_config::load_bar_config().position {
                    metis_config::BarPosition::Top => {
                        zone.y += reserve;
                        zone.height = (zone.height - reserve).max(1);
                    }
                    metis_config::BarPosition::Bottom => {
                        zone.height = (zone.height - reserve).max(1);
                    }
                    metis_config::BarPosition::Left => {
                        zone.x += reserve;
                        zone.width = (zone.width - reserve).max(1);
                    }
                    metis_config::BarPosition::Right => {
                        zone.width = (zone.width - reserve).max(1);
                    }
                }
            }
            zone
        };
        self.enforce_bar_reserve_on_zone(output, &mut zone);
        zone
    }

    /// Keep the bar strip reserved when auto-hide is off. With auto-hide the
    /// reserve is 0 (bar overlays; windows keep `window_gap_px` only).
    fn enforce_bar_reserve_on_zone(&self, output: &smithay::output::Output, zone: &mut PixelRect) {
        if !self.output_has_bar(output) {
            return;
        }
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let reserve = self.bar_reserved_px_for(output);
        if reserve <= 0 {
            return;
        }
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                let min_y = output_geo.loc.y + reserve;
                if zone.y < min_y {
                    let delta = min_y - zone.y;
                    zone.y = min_y;
                    zone.height = (zone.height - delta).max(1);
                }
            }
            metis_config::BarPosition::Bottom => {
                let max_bottom = output_geo.loc.y + output_geo.size.h - reserve;
                let zone_bottom = zone.y + zone.height;
                if zone_bottom > max_bottom {
                    zone.height = (max_bottom - zone.y).max(1);
                }
            }
            metis_config::BarPosition::Left => {
                let min_x = output_geo.loc.x + reserve;
                if zone.x < min_x {
                    let delta = min_x - zone.x;
                    zone.x = min_x;
                    zone.width = (zone.width - delta).max(1);
                }
            }
            metis_config::BarPosition::Right => {
                let max_right = output_geo.loc.x + output_geo.size.w - reserve;
                let zone_right = zone.x + zone.width;
                if zone_right > max_right {
                    zone.width = (max_right - zone.x).max(1);
                }
            }
        }
    }

    /// Output a window was opened on (assigned at registration from the pointer).
    pub(crate) fn launch_output_for(&self, id: u32) -> Option<smithay::output::Output> {
        self.windows
            .output_name(id)
            .and_then(|name| self.output_by_name(&name))
            .or_else(|| self.output_under_pointer())
            .or_else(|| self.primary_output())
    }

    /// Center a client rect for a window, accounting for SSD chrome insets.
    pub(crate) fn centered_body_for_window(&self, id: u32, body_w: i32, body_h: i32) -> PixelRect {
        if !self.should_draw_metis_ssd(id) {
            let rect = match self.launch_output_for(id) {
                Some(output) => self.centered_rect_in(&output, body_w, body_h),
                None => self.centered_rect(body_w, body_h),
            };
            return self.clamp_floating_rect_for(id, rect);
        }
        if self.window_uses_compact_overlay(id) {
            let rect = match self.launch_output_for(id) {
                Some(output) => self.centered_rect_in(&output, body_w, body_h),
                None => self.centered_rect(body_w, body_h),
            };
            return self.clamp_floating_rect(rect);
        }
        let border = metis_grid::app_tile_border_px() as i32;
        let header = metis_grid::APP_TILE_HEADER_PX;
        let footprint_w = body_w + border * 2;
        let footprint_h = body_h + header + border;
        let footprint = match self.launch_output_for(id) {
            Some(output) => self.centered_rect_in(&output, footprint_w, footprint_h),
            None => self.centered_rect(footprint_w, footprint_h),
        };
        self.clamp_body_below_bar(app_tile_body_rect(footprint))
    }

    /// Restore a saved client rect, keeping position and size when possible.
    pub(crate) fn restore_body_for_window(&self, id: u32, saved: PixelRect) -> PixelRect {
        let rect = self.recover_offscreen_rect(saved);
        if self.should_draw_metis_ssd(id) {
            self.clamp_floating_rect(rect)
        } else {
            self.clamp_floating_rect_for(id, rect)
        }
    }

    /// A rect of `width`x`height` centered in the primary output's usable area.
    fn centered_rect(&self, width: i32, height: i32) -> PixelRect {
        self.centered_rect_in_zone(self.placement_zone(), width, height)
    }

    /// A rect of `width`x`height` centered in `output`'s usable area.
    fn centered_rect_in(
        &self,
        output: &smithay::output::Output,
        width: i32,
        height: i32,
    ) -> PixelRect {
        self.centered_rect_in_zone(self.placement_zone_for(output), width, height)
    }

    /// A rect of `width`x`height` centered in `zone` (clamped to fit).
    fn centered_rect_in_zone(&self, zone: PixelRect, width: i32, height: i32) -> PixelRect {
        let w = width.min((zone.width - WINDOW_GAP_PX * 2).max(1)).max(1);
        let h = height.min((zone.height - WINDOW_GAP_PX * 2).max(1)).max(1);
        PixelRect {
            x: zone.x + (zone.width - w) / 2,
            y: zone.y + (zone.height - h).max(0) / 2,
            width: w,
            height: h,
        }
    }

    /// True when `rect` is visible on at least one active output — i.e. it
    /// overlaps some monitor by a grabbable amount. A window on a secondary
    /// monitor counts as on-screen; only a window that lies off *every* output is
    /// considered lost. The minimum overlap ensures the titlebar stays reachable.
    fn rect_visible_on_any_output(&self, rect: PixelRect) -> bool {
        // Require a chunk at least this big (incl. the titlebar) on some output.
        const MIN_VISIBLE: i32 = MIN_VISIBLE_PX;
        for output in self.space.outputs() {
            let Some(g) = self.space.output_geometry(output) else {
                continue;
            };
            let left = rect.x.max(g.loc.x);
            let right = (rect.x + rect.width).min(g.loc.x + g.size.w);
            let top = rect.y.max(g.loc.y);
            let bottom = (rect.y + rect.height).min(g.loc.y + g.size.h);
            let overlap_w = (right - left).min(rect.width);
            let overlap_h = (bottom - top).min(rect.height);
            if overlap_w >= MIN_VISIBLE.min(rect.width) && overlap_h >= MIN_VISIBLE.min(rect.height)
            {
                return true;
            }
        }
        false
    }

    /// Keep a window reachable: if `rect` lies off every active output (e.g. it
    /// was saved on a monitor that's no longer connected), pull it back onto the
    /// primary output. Windows already visible on *some* monitor — including a
    /// secondary one in a multi-monitor setup — are left exactly where they are.
    pub fn recover_offscreen_rect(&self, rect: PixelRect) -> PixelRect {
        if self.rect_visible_on_any_output(rect) {
            return rect;
        }
        self.clamp_rect_on_screen(rect)
    }

    /// Force `rect` to be fully visible on the main output: cap its size to the
    /// usable area and shift its origin so the whole window is on-screen (under
    /// the bar). Used to recover a window that's off every active output.
    pub fn clamp_rect_on_screen(&self, rect: PixelRect) -> PixelRect {
        let zone = self.window_placement_zone();
        let gaps = self.zone_edge_gaps();
        let width = rect.width.clamp(1, zone.width.max(1));
        let height = rect.height.clamp(1, zone.height.max(1));
        let min_x = zone.x + gaps.left;
        let min_y = zone.y + gaps.top;
        let max_x = (zone.x + zone.width - width - gaps.right).max(min_x);
        let max_y = (zone.y + zone.height - height - gaps.bottom).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Keep a floating window's body clear of the top edge bar (exclusive zone).
    /// Bottom/left/right bars overlay the desktop — floating windows may pass under them.
    pub fn clamp_body_below_bar(&self, mut rect: PixelRect) -> PixelRect {
        let pos = metis_config::load_bar_config().position;
        if !matches!(pos, metis_config::BarPosition::Top) {
            return rect;
        }
        let gaps = self.zone_edge_gaps();
        let header = metis_grid::APP_TILE_HEADER_PX;
        let zone = self.placement_zone();
        let min_y = zone.y + gaps.top + header;
        if rect.y < min_y {
            rect.y = min_y;
        }
        rect
    }

    /// Keep a restored floating CSD window below the visible top bar.
    ///
    /// Layer-shell exclusive zones can lag by a commit when the bar reappears
    /// after fullscreen, so use configured bar geometry directly.
    fn clamp_below_top_bar_edge_for(&self, id: u32, mut rect: PixelRect) -> PixelRect {
        if !matches!(
            metis_config::load_bar_config().position,
            metis_config::BarPosition::Top
        ) {
            return rect;
        }
        let Some(output) = self.output_for_window(id).or_else(|| self.primary_output()) else {
            return rect;
        };
        if !self.output_has_bar(&output) {
            return rect;
        }
        let origin_y = self
            .space
            .output_geometry(&output)
            .map(|g| g.loc.y)
            .unwrap_or(0);
        let min_y = origin_y + self.bar_reserved_px_for(&output) + self.configured_window_gap();
        if rect.y < min_y {
            rect.y = min_y;
        }
        rect
    }

    fn clamp_restored_floating_rect(&self, id: u32, rect: PixelRect) -> PixelRect {
        let rect = self.recover_offscreen_rect(rect);
        if self.should_draw_metis_ssd(id) {
            if self.window_uses_compact_overlay(id) {
                self.clamp_floating_rect(rect)
            } else {
                self.clamp_body_below_bar(rect)
            }
        } else {
            let rect = self.clamp_floating_rect_for(id, rect);
            self.clamp_below_top_bar_edge_for(id, rect)
        }
    }

    /// Restore geometry saved before maximize/fullscreen and clamp below the bar.
    fn restore_floating_from_transient(&mut self, id: u32) {
        let Some(restore) = self.windows.take_restore_rect(id) else {
            return;
        };
        let restore = self.clamp_restored_floating_rect(id, restore);
        self.windows.set_target_rect(id, restore);
    }

    /// Keep a floating window on-screen. Overlay edge bars do not inset the bounds
    /// (windows may slide underneath); only the top bar reserves space for SSD windows.
    pub(crate) fn clamp_floating_rect_for(&self, id: u32, rect: PixelRect) -> PixelRect {
        if self.should_draw_metis_ssd(id) {
            self.clamp_floating_rect(rect)
        } else {
            self.clamp_floating_rect_no_header(rect)
        }
    }

    /// Like [`Self::clamp_floating_rect`] but without reserving space for Metis SSD chrome.
    fn clamp_floating_rect_no_header(&self, rect: PixelRect) -> PixelRect {
        let center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        let zone = match self.output_at(center) {
            Some(output) => self.placement_zone_for(&output),
            None => self.placement_zone(),
        };
        let g = WINDOW_GAP_PX;
        let width = rect.width.clamp(1, (zone.width - g * 2).max(1));
        let height = rect.height.clamp(1, (zone.height - g * 2).max(1));
        let min_x = zone.x + g;
        let min_y = zone.y + g;
        let max_x = (zone.x + zone.width - width - g).max(min_x);
        let max_y = (zone.y + zone.height - height - g).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Keep a floating window on-screen. Overlay edge bars do not inset the bounds
    /// (windows may slide underneath); only the top bar reserves space.
    fn clamp_floating_rect(&self, rect: PixelRect) -> PixelRect {
        // Clamp within the output the window mostly sits on (by its center), so a
        // floating window on a secondary monitor isn't yanked back to primary.
        let center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        let zone = match self.output_at(center) {
            Some(output) => self.placement_zone_for(&output),
            None => self.placement_zone(),
        };
        let g = WINDOW_GAP_PX;
        let pos = metis_config::load_bar_config().position;
        let gaps = self.zone_edge_gaps();
        let width = rect.width.clamp(1, (zone.width - g * 2).max(1));
        let height = rect.height.clamp(1, (zone.height - g * 2).max(1));
        let min_x = zone.x + g;
        let min_y = match pos {
            metis_config::BarPosition::Top => zone.y + gaps.top + metis_grid::APP_TILE_HEADER_PX,
            _ => zone.y + g,
        };
        let max_x = (zone.x + zone.width - width - g).max(min_x);
        let max_y = (zone.y + zone.height - height - g).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Unmaximize only after the user actually drags the titlebar (not on click).
    pub fn unmaximize_for_titlebar_drag(&mut self, id: u32) {
        if self.windows.get(id).is_some_and(|r| r.maximized) {
            self.set_maximized(id, false);
        }
    }
}

/// When re-anchoring an oversized client, keep this placement-zone edge flush
/// with the footprint (overflow spills toward the interior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarEdgeAnchor {
    Min,
    Max,
}

/// Pick a window origin on one axis that preserves the footprint's screen-edge
/// gap when the client is larger than the footprint. Honors the footprint origin
/// when the client fits; otherwise anchors to whichever screen edge the footprint
/// hugs (so the overflow grows toward the opposite, interior side).
#[allow(clippy::too_many_arguments)]
fn anchor_axis(
    foot_min: i32,
    foot_size: i32,
    zone_min: i32,
    zone_size: i32,
    actual: i32,
    gap_min: i32,
    gap_max: i32,
    bar_edge: Option<BarEdgeAnchor>,
) -> i32 {
    if actual <= foot_size {
        return foot_min;
    }
    let foot_max = foot_min + foot_size;
    let zone_max = zone_min + zone_size;
    let touches_min = foot_min - zone_min <= gap_min;
    let touches_max = zone_max - foot_max <= gap_max;

    // Maximize hugs both zone edges — keep the bar-adjacent side fixed so any
    // overflow spills away from the edge bar instead of underneath it.
    if touches_min && touches_max {
        return match bar_edge {
            Some(BarEdgeAnchor::Max) => foot_max - actual,
            Some(BarEdgeAnchor::Min) | None => foot_min,
        };
    }
    if touches_max && !touches_min {
        foot_max - actual
    } else {
        foot_min
    }
}

/// Apply maximize-consistent edge gaps to a raw snap region. Boundary sides use
/// `gaps`; interior split lines get half the configured window gap.
fn snap_client_rect(raw: PixelRect, zone: PixelRect, gaps: ZoneGaps) -> PixelRect {
    let half = (gaps.left.max(gaps.right).max(gaps.top).max(gaps.bottom) / 2).max(0);
    let touches_left = raw.x <= zone.x;
    let touches_right = raw.x + raw.width >= zone.x + zone.width;
    let touches_top = raw.y <= zone.y;
    let touches_bottom = raw.y + raw.height >= zone.y + zone.height;

    let l = if touches_left { gaps.left } else { half };
    let r = if touches_right { gaps.right } else { half };
    let t = if touches_top { gaps.top } else { half };
    let b = if touches_bottom { gaps.bottom } else { half };

    PixelRect {
        x: raw.x + l,
        y: raw.y + t,
        width: (raw.width - l - r).max(1),
        height: (raw.height - t - b).max(1),
    }
}
