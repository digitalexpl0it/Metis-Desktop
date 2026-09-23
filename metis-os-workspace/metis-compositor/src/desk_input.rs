use metis_grid::{PixelRect, TileKind, app_tile_body_rect, cell_to_pixels};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{PopupKind, PopupManager, WindowSurfaceType, layer_map_for_output},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::seat::WaylandFocus,
    wayland::shell::wlr_layer::Layer as WlrLayer,
};

use crate::focus::KeyboardFocusTarget;
use crate::state::MetisState;

/// True when the surface currently has a committed buffer.
///
/// NOTE: must read the *renderer* surface state, not `SurfaceAttributes.buffer`.
/// `on_commit_buffer_handler` consumes the attribute buffer on every commit, so
/// `SurfaceAttributes.current().buffer` is `None` except on the exact frame a new
/// buffer was attached — which made every bar click miss.
fn surface_has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

fn popup_geometry_contains(
    local: Point<f64, Logical>,
    popup: &PopupKind,
    location: Point<i32, Logical>,
) -> bool {
    let geo = popup.geometry();
    if geo.size.w <= 0 || geo.size.h <= 0 {
        return false;
    }
    let origin = location + geo.loc;
    Rectangle::new(origin, geo.size).to_f64().contains(local)
}

/// Innermost popup under `root` whose geometry contains `local`.
fn metis_bar_deepest_popup(
    root: &WlSurface,
    local: Point<f64, Logical>,
) -> Option<(PopupKind, Point<i32, Logical>)> {
    for (popup, location) in PopupManager::popups_for_surface(root) {
        if !popup_geometry_contains(local, &popup, location) {
            continue;
        }
        if let Some(nested) = metis_bar_deepest_popup(popup.wl_surface(), local) {
            return Some(nested);
        }
        return Some((popup, location));
    }
    None
}
fn metis_bar_popup_tree_contains(root: &WlSurface, local: Point<f64, Logical>) -> bool {
    metis_bar_deepest_popup(root, local).is_some()
}

fn metis_bar_region_contains(
    layer: &smithay::desktop::LayerSurface,
    layers: &smithay::desktop::LayerMap,
    rel: Point<f64, Logical>,
) -> bool {
    let Some(layer_geo) = layers.layer_geometry(layer) else {
        return false;
    };
    // Popovers can extend below the bar strip — always honor their geometry even
    // when the root layer surface has not committed a buffer yet. Include nested
    // popovers (e.g. tray icon context menus parented to a bar popover).
    let local = rel - layer_geo.loc.to_f64();
    let root = layer.wl_surface();
    if metis_bar_popup_tree_contains(root, local) {
        return true;
    }

    if !surface_has_buffer(layer.wl_surface()) {
        return false;
    }

    if layer_geo.to_f64().contains(rel) {
        return true;
    }

    if layer.bbox().to_f64().contains(local) {
        return true;
    }

    false
}

fn layer_accepts_pointer(
    layer: &smithay::desktop::LayerSurface,
    layers: &smithay::desktop::LayerMap,
    rel: Point<f64, Logical>,
) -> bool {
    if layer.namespace().starts_with("metis-bar") {
        return metis_bar_region_contains(layer, layers, rel);
    }
    if !surface_has_buffer(layer.wl_surface()) {
        return false;
    }
    let Some(layer_loc) = layers.layer_geometry(layer).map(|g| g.loc) else {
        return false;
    };
    layer
        .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
        .is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskHit {
    WidgetTile { tile_id: String },
    AppHeader { tile_id: String, window_id: u32 },
    AppBody { window_id: u32 },
    Gutter,
    Empty,
}

pub fn point_in_rect(x: i32, y: i32, rect: PixelRect) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

impl MetisState {
    pub fn classify_hit(&self, x: i32, y: i32) -> DeskHit {
        let (metrics, key) = match self.output_at(Point::from((x, y))) {
            Some(o) => (self.grid_metrics_for(&o), o.name()),
            None => (self.grid_metrics(), self.primary_key()),
        };
        let Some(desk) = self.desk(&key) else {
            return DeskHit::Empty;
        };

        // Scrolling workspaces position app windows from the strip, not the tile
        // grid: test those frames first (topmost), then fall through to the desk's
        // widget tiles (which sit behind the windows).
        if self.active_layout_kind(&key) == metis_grid::LayoutKind::Scroll {
            for (wid, full) in self.scroll_frames_for(&key) {
                if !point_in_rect(x, y, full) {
                    continue;
                }
                let body = if self.window_uses_ssd(wid) {
                    app_tile_body_rect(full)
                } else {
                    full
                };
                if point_in_rect(x, y, body) {
                    return DeskHit::AppBody { window_id: wid };
                }
                if self.window_uses_ssd(wid) {
                    return DeskHit::AppHeader {
                        tile_id: format!("app-{wid}"),
                        window_id: wid,
                    };
                }
                return DeskHit::AppBody { window_id: wid };
            }
            for tile in &desk.layout.tiles {
                if let TileKind::Widget { .. } = &tile.kind {
                    let full = cell_to_pixels(&metrics, &tile.rect);
                    if point_in_rect(x, y, full) {
                        return DeskHit::WidgetTile {
                            tile_id: tile.id.clone(),
                        };
                    }
                }
            }
            return DeskHit::Empty;
        }

        for tile in &desk.layout.tiles {
            let full = cell_to_pixels(&metrics, &tile.rect);
            if !point_in_rect(x, y, full) {
                continue;
            }
            match &tile.kind {
                TileKind::Widget { .. } => {
                    return DeskHit::WidgetTile {
                        tile_id: tile.id.clone(),
                    };
                }
                TileKind::App {
                    window_id: Some(wid),
                    ..
                } => {
                    let body = if self.window_uses_ssd(*wid) {
                        app_tile_body_rect(full)
                    } else {
                        full
                    };
                    if point_in_rect(x, y, body) {
                        return DeskHit::AppBody { window_id: *wid };
                    }
                    if self.window_uses_ssd(*wid) {
                        return DeskHit::AppHeader {
                            tile_id: tile.id.clone(),
                            window_id: *wid,
                        };
                    }
                    return DeskHit::AppBody { window_id: *wid };
                }
                TileKind::App {
                    window_id: None, ..
                } => {
                    return DeskHit::AppHeader {
                        tile_id: tile.id.clone(),
                        window_id: 0,
                    };
                }
            }
        }

        if self.is_gutter_hit(x, y) {
            return DeskHit::Gutter;
        }

        DeskHit::Empty
    }

    fn is_gutter_hit(&self, x: i32, y: i32) -> bool {
        let metrics = match self.output_at(Point::from((x, y))) {
            Some(o) => self.grid_metrics_for(&o),
            None => self.grid_metrics(),
        };
        let gutter = metrics.gutter as i32;
        if gutter <= 0 {
            return false;
        }
        let cols = metrics.columns.max(1) as i32;
        let rows = metrics.rows.max(1) as i32;
        let usable_w = metrics.monitor.width - gutter * (cols + 1);
        let usable_h = metrics.monitor.height - gutter * (rows + 1);
        let cell_w = usable_w / cols;
        let cell_h = usable_h / rows;

        for col in 1..cols {
            let gx = metrics.monitor.x + gutter + col * (cell_w + gutter) - gutter / 2;
            if (x - gx).abs() <= gutter {
                return true;
            }
        }
        for row in 1..rows {
            let gy = metrics.monitor.y + gutter + row * (cell_h + gutter) - gutter / 2;
            if (y - gy).abs() <= gutter {
                return true;
            }
        }
        false
    }

    /// True when the monitor-space point is inside an app tile body (below the header chrome).
    pub fn app_body_hit(&self, x: i32, y: i32, window_id: u32) -> bool {
        matches!(self.classify_hit(x, y), DeskHit::AppBody { window_id: wid } if wid == window_id)
    }

    pub fn app_tile_body_rect(&self, window_id: u32) -> Option<PixelRect> {
        // Scrolling workspace: the body comes from the strip frame.
        if let Some(frame) = self.scroll_frame_for_window(window_id) {
            return Some(self.tile_client_rect(window_id, frame));
        }
        let key = self.desk_key_for_window(window_id);
        let metrics = match self.output_by_name(&key) {
            Some(o) => self.grid_metrics_for(&o),
            None => self.grid_metrics(),
        };
        let desk = self.desk(&key)?;
        for tile in &desk.layout.tiles {
            let TileKind::App {
                window_id: Some(wid),
                ..
            } = &tile.kind
            else {
                continue;
            };
            if *wid != window_id {
                continue;
            }
            let full = cell_to_pixels(&metrics, &tile.rect);
            return Some(self.tile_client_rect(window_id, full));
        }
        None
    }

    pub fn window_id_at(&self, pos: Point<f64, Logical>) -> Option<u32> {
        self.topmost_window_at_pointer(pos)
            .and_then(|(window, _)| self.windows.id_for_window(&window))
            .or_else(|| {
                self.space
                    .element_under(pos)
                    .and_then(|(window, _)| self.windows.id_for_window(window))
            })
    }

    /// Topmost mapped window that should receive pointer events at `pos`. Unlike
    /// [`Space::element_under`](smithay::desktop::Space::element_under), accounts
    /// for compositor-drawn SSD chrome (overlay titlebars and border strips) that
    /// sits outside the client's committed surface tree.
    pub(crate) fn topmost_window_at_pointer(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(smithay::desktop::Window, Point<i32, Logical>)> {
        use crate::decoration::overlay_chrome_rect;

        let (x, y) = (pos.x as i32, pos.y as i32);

        for window in self.space.elements().rev() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.windows.is_minimized(id) {
                continue;
            }
            let Some(record) = self.windows.get(id) else {
                continue;
            };
            let Some(loc) = self.space.element_location(window) else {
                continue;
            };
            let size = window.geometry().size;
            if size.w <= 0 || size.h <= 0 {
                continue;
            }

            if record.fullscreen {
                if let Some(geo) = self.space.element_geometry(window)
                    && geo.contains(pos.to_i32_round())
                {
                    return Some((window.clone(), loc));
                }
                continue;
            }

            if self.should_draw_metis_ssd(id) {
                let client_frame = PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: size.w,
                    height: size.h,
                };

                if self.auto_hide_titlebar.contains(&id) {
                    if self.titlebar_reveal_window == Some(id)
                        && self.titlebar_reveal_progress > 0.0
                    {
                        let compact = self.window_uses_compact_overlay(id);
                        let chrome = overlay_chrome_rect(
                            client_frame,
                            self.titlebar_reveal_progress,
                            compact,
                        );
                        if point_in_rect(x, y, chrome) {
                            return Some((window.clone(), loc));
                        }
                    }
                } else if let Some(frame) = self.ssd_frame_for_mapped_window(id, window)
                    && point_in_rect(x, y, frame)
                    && !point_in_rect(x, y, app_tile_body_rect(frame))
                {
                    return Some((window.clone(), loc));
                }
            }

            if let Some(geo) = self.space.element_geometry(window)
                && x >= geo.loc.x
                && x < geo.loc.x + geo.size.w
                && y >= geo.loc.y
                && y < geo.loc.y + geo.size.h
            {
                // Use the mapped origin, not geo.loc (which includes
                // geometry.loc shadow offsets). surface_under expects
                // coordinates relative to element_location.
                return Some((window.clone(), loc));
            }
        }
        None
    }

    pub fn window_surface_at(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        // Override-redirect X11 popups (Steam/game menus, tooltips, combo
        // dropdowns) are intentionally kept out of the window registry, so the
        // SSD-aware `topmost_window_at_pointer` scan below skips them and would
        // hand pointer focus to the *registered* toplevel stacked beneath the
        // popup. The owning app then sees the pointer as having left its menu and
        // dismisses it after a short hover timeout (and the per-frame focus
        // ping-pong shows up as flicker). Honor true stacking order first: if the
        // topmost element actually under the pointer is a raised OR popup, it
        // wins. `element_under` walks the Space in z-order, so a popup mapped with
        // `activate = true` sits above its parent here.
        if let Some((window, _)) = self.space.element_under(pos) {
            let is_or_popup = window
                .x11_surface()
                .map(|surface| surface.is_override_redirect())
                .unwrap_or(false);
            if is_or_popup {
                let window = window.clone();
                return self.window_surface_for(pos, &window);
            }
        }
        let (window, _location) = self.topmost_window_at_pointer(pos).or_else(|| {
            self.space
                .element_under(pos)
                .map(|(window, location)| (window.clone(), location))
        })?;
        self.window_surface_for(pos, &window)
    }

    /// Pointer target while a capture overlay session is active.
    pub(crate) fn window_surface_for(
        &self,
        pos: Point<f64, Logical>,
        window: &smithay::desktop::Window,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map_loc = self.space.element_location(window)?;
        // Match smithay's render origin: mapped location minus the client's
        // geometry offset (CSD shadow margins Firefox keeps while unmaximized).
        let geo = window.geometry();
        let surface_origin = map_loc.to_f64() - geo.loc.to_f64();
        let rel = pos - surface_origin;
        if let Some((surface, loc)) = window
            .surface_under(rel, WindowSurfaceType::ALL)
            .map(|(surface, loc)| (surface, (loc.to_f64() + surface_origin).to_f64()))
        {
            return Some((surface, loc));
        }
        // Clicks on compositor-drawn titlebar/border land outside the client
        // subsurface tree — still deliver them to the window so paste, primary
        // selection, and context menus (e.g. kitty right-click) work.
        if let Some(toplevel) = window.toplevel() {
            return Some((toplevel.wl_surface().clone(), pos));
        }
        window
            .wl_surface()
            .map(|surface| (surface.into_owned(), pos))
    }

    /// Best-effort pointer target for button events and clipboard focus.
    pub(crate) fn pointer_target_at(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.capture_overlay_active() {
            return self.capture_overlay_surface_at(pos);
        }
        if self.screenshot_overlay_active() {
            return self.screenshot_overlay_surface_at(pos);
        }
        if self.task_view_overlay_active() {
            return self.task_view_overlay_surface_at(pos);
        }
        if self.protocol_lock.is_locked() {
            return self.protocol_lock_surface_at(pos);
        }
        self.surface_under(pos)
            .or_else(|| self.window_surface_at(pos))
    }

    fn layer_surface_at(
        &self,
        layer: &smithay::desktop::LayerSurface,
        layers: &smithay::desktop::LayerMap,
        rel: Point<f64, Logical>,
        output_geo: smithay::utils::Rectangle<i32, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if layer.namespace().starts_with("metis-bar") {
            return metis_bar_layer_surface_at(layer, layers, rel, output_geo);
        }
        let layer_loc = layers.layer_geometry(layer)?.loc;
        layer
            .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
            .map(|(surface, loc)| (surface, (loc + layer_loc + output_geo.loc).to_f64()))
    }

    /// True when a pointer press should dismiss open bar popovers.
    ///
    /// Any press that does not land on the bar chrome or its popover (i.e. bare
    /// desktop OR an app window) dismisses. Bar-local clicks are handled by the
    /// shell itself (GTK), which avoids a race with the poll-based dismiss signal.
    pub fn should_dismiss_bar_popovers(&self, pos: Point<f64, Logical>) -> bool {
        !self.metis_bar_ui_hit(pos)
    }

    /// Bar chrome or an attached dropdown panel below the bar strip.
    pub(crate) fn metis_bar_ui_hit(&self, pos: Point<f64, Logical>) -> bool {
        let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .is_some_and(|geo| geo.contains(pos.to_i32_round()))
        }) else {
            return false;
        };
        if !self.output_has_bar(output) {
            return false;
        }
        let Some(output_geo) = self.space.output_geometry(output) else {
            return false;
        };
        let (x, y) = (pos.x as i32, pos.y as i32);
        let auto_hidden = self.bar_is_auto_hidden(output);

        // Peek-only while auto-hidden so maximized titlebar controls under the
        // (CSS-slid) full layer surface stay clickable. Full strip when shown.
        if let Some(strip) = self.bar_input_block_rect(output, &output_geo)
            && point_in_rect(x, y, strip)
        {
            return true;
        }

        let rel = pos - output_geo.loc.to_f64();
        let layers = layer_map_for_output(output);

        for layer in layers.layers().filter(|layer| {
            let ns = layer.namespace();
            ns.starts_with("metis-bar") || ns == "metis-dashboard" || ns == "metis-control-center"
        }) {
            if layer.namespace().starts_with("metis-bar") && auto_hidden {
                // Invisible full-size layer must not steal titlebar clicks —
                // only real popovers outside the peek strip count.
                if !surface_has_buffer(layer.wl_surface()) {
                    continue;
                }
                let Some(layer_geo) = layers.layer_geometry(layer) else {
                    continue;
                };
                let local = rel - layer_geo.loc.to_f64();
                if metis_bar_popup_tree_contains(layer.wl_surface(), local) {
                    return true;
                }
                continue;
            }
            if layer_accepts_pointer(layer, &layers, rel) {
                return true;
            }
        }
        false
    }

    /// True when the Notification Center layer surface is mapped with a buffer.
    pub(crate) fn notification_center_mapped(&self) -> bool {
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            for layer in map.layers() {
                if layer.namespace() == "metis-notification-center"
                    && surface_has_buffer(layer.wl_surface())
                {
                    return true;
                }
            }
        }
        false
    }

    /// True while the Control Center layer surface is mapped with a buffer.
    /// Used to make global close shortcuts dismiss the panel instead of the
    /// application underneath it.
    pub(crate) fn control_center_mapped(&self) -> bool {
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            for layer in map.layers() {
                let namespace = layer.namespace();
                if (namespace == "metis-dashboard" || namespace == "metis-control-center")
                    && surface_has_buffer(layer.wl_surface())
                {
                    return true;
                }
            }
        }
        false
    }

    /// Topmost mapped Top/Overlay layer requesting `KeyboardInteractivity::Exclusive`.
    ///
    /// Per the layer-shell protocol (and Anvil), these surfaces own the keyboard
    /// whenever they are mapped — even when the pointer is not over them. Used so
    /// Super-opened menus / Control Center / screenshot UI receive typing without
    /// a prior click on the surface.
    pub(crate) fn exclusive_keyboard_layer(&self) -> Option<smithay::desktop::LayerSurface> {
        use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;

        for layer_kind in [WlrLayer::Overlay, WlrLayer::Top] {
            for output in self.space.outputs() {
                let map = layer_map_for_output(output);
                // Walk top-to-bottom within the layer so the most recently mapped
                // Exclusive surface wins (GTK popovers share the bar surface).
                for layer in map.layers_on(layer_kind).rev() {
                    if !surface_has_buffer(layer.wl_surface()) {
                        continue;
                    }
                    let exclusive = layer.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::Exclusive;
                    if exclusive {
                        return Some(layer.clone());
                    }
                }
            }
        }
        None
    }

    /// Pointer is over the Notification Center panel (not the rest of the screen).
    pub(crate) fn metis_notification_center_hit(&self, pos: Point<f64, Logical>) -> bool {
        let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .is_some_and(|geo| geo.contains(pos.to_i32_round()))
        }) else {
            return false;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return false;
        };
        let rel = pos - output_geo.loc.to_f64();
        let layers = layer_map_for_output(output);
        for layer in layers
            .layers()
            .filter(|layer| layer.namespace() == "metis-notification-center")
        {
            if layer_accepts_pointer(layer, &layers, rel) {
                return true;
            }
        }
        false
    }

    /// Configured edge-bar strip (margin + body + shadow pad) in monitor-global
    /// coordinates. Does not consult the layer map — safe while a layer lock is held.
    pub(crate) fn bar_config_strip_rect(
        output_geo: &smithay::utils::Rectangle<i32, Logical>,
    ) -> PixelRect {
        let cfg = metis_config::load_bar_config();
        let margin = cfg.margin_top as i32;
        let visible = cfg.height as i32;
        let shadow = metis_config::bar::bar_layer_shadow_pad(&cfg);
        let thickness = margin + visible + shadow;
        Self::bar_edge_strip_rect(output_geo, cfg.position, thickness)
    }

    /// Peek-only strip while the bar is auto-hidden (matches shell peek height).
    pub(crate) fn bar_peek_strip_rect(
        output_geo: &smithay::utils::Rectangle<i32, Logical>,
    ) -> PixelRect {
        let cfg = metis_config::load_bar_config();
        // Keep a slightly generous hit target (min 4) even when peek is thin —
        // nested sessions and fractional scale make 1–2px strips easy to miss.
        let peek = cfg.auto_hide_peek_px.clamp(2, 8).max(4) as i32;
        Self::bar_edge_strip_rect(output_geo, cfg.position, peek)
    }

    fn bar_edge_strip_rect(
        output_geo: &smithay::utils::Rectangle<i32, Logical>,
        position: metis_config::BarPosition,
        thickness: i32,
    ) -> PixelRect {
        let w = output_geo.size.w;
        let h = output_geo.size.h;
        match position {
            metis_config::BarPosition::Top => PixelRect {
                x: output_geo.loc.x,
                y: output_geo.loc.y,
                width: w,
                height: thickness,
            },
            metis_config::BarPosition::Bottom => PixelRect {
                x: output_geo.loc.x,
                y: output_geo.loc.y + h - thickness,
                width: w,
                height: thickness,
            },
            metis_config::BarPosition::Left => PixelRect {
                x: output_geo.loc.x,
                y: output_geo.loc.y,
                width: thickness,
                height: h,
            },
            metis_config::BarPosition::Right => PixelRect {
                x: output_geo.loc.x + w - thickness,
                y: output_geo.loc.y,
                width: thickness,
                height: h,
            },
        }
    }

    /// Monitor-global rect covering the edge bar's layer surface (margin + body +
    /// shadow pad) on `output`, used for input blocking and cursor selection.
    /// While auto-hidden, only the peek strip is blocked so apps receive clicks
    /// on the rest of the former bar band.
    pub(crate) fn bar_input_block_rect(
        &self,
        output: &smithay::output::Output,
        output_geo: &smithay::utils::Rectangle<i32, Logical>,
    ) -> Option<PixelRect> {
        if !self.output_has_bar(output) {
            return None;
        }
        Some(if self.bar_is_auto_hidden(output) {
            Self::bar_peek_strip_rect(output_geo)
        } else {
            Self::bar_config_strip_rect(output_geo)
        })
    }

    /// Route pointer hits: app bodies pass through, then layer-shell UI, then windows.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.capture_overlay_active() {
            return self.capture_overlay_surface_at(pos);
        }
        // Native screenshot UI must own the pointer everywhere (including over
        // app bodies). Without this, AppBody passthrough below delivers events
        // to windows and compositor resize chrome still fires.
        if self.screenshot_overlay_active() {
            return self.screenshot_overlay_surface_at(pos);
        }
        if self.task_view_overlay_active() {
            return self.task_view_overlay_surface_at(pos);
        }

        let output = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .is_some_and(|geo| geo.contains(pos.to_i32_round()))
        })?;
        let output_geo = self.space.output_geometry(output)?;
        let rel = pos - output_geo.loc.to_f64();
        let (x, y) = (pos.x as i32, pos.y as i32);

        // Classify the desk hit BEFORE locking this output's layer map below.
        // `classify_hit` resolves grid metrics, which lock the same output's layer
        // map (via `usable_zone_for` → `non_exclusive_zone`). Smithay's layer map
        // is a non-reentrant Mutex, so computing this while holding `layers` would
        // self-deadlock the compositor thread on the next pointer motion.
        let desk_hit = self.classify_hit(x, y);

        let has_bar = self.output_has_bar(output);
        let layers = layer_map_for_output(output);

        // Block the configured bar strip even when layer geometry/buffer state is
        // briefly stale. Must not call `metis_bar_ui_hit()` here — it re-locks the
        // layer map we already hold.
        if has_bar {
            let auto_hidden = self.bar_is_auto_hidden(output);
            let strip = if auto_hidden {
                Self::bar_peek_strip_rect(&output_geo)
            } else {
                Self::bar_config_strip_rect(&output_geo)
            };
            if point_in_rect(x, y, strip) {
                for layer in layers
                    .layers()
                    .filter(|layer| layer.namespace().starts_with("metis-bar"))
                {
                    if let Some(hit) = self.layer_surface_at(layer, &layers, rel, output_geo) {
                        return Some(hit);
                    }
                    // Auto-hidden peek: always deliver to the bar root so GTK
                    // gets enter even if layer geometry briefly lags the strip
                    // (margin clear / resize race). Without this the bar stays gone.
                    if auto_hidden
                        && surface_has_buffer(layer.wl_surface())
                        && let Some(layer_geo) = layers.layer_geometry(layer)
                    {
                        let local = rel - layer_geo.loc.to_f64();
                        return Some((
                            layer.wl_surface().clone(),
                            (local + layer_geo.loc.to_f64() + output_geo.loc.to_f64()),
                        ));
                    }
                    if !layer_accepts_pointer(layer, &layers, rel) {
                        continue;
                    }
                    // In the bar strip but no subsurface — fall through to the
                    // priority pass below (root-surface fallback).
                    break;
                }
            }
        }

        // The bar strip and its popovers (e.g. the start menu) take priority over
        // everything below — including the desk-grid app-body passthrough. Without
        // this, an app window stacked behind an open popover would win pointer focus,
        // so scroll/clicks over the menu went to the window instead of the menu.
        // (When the window was moved away the passthrough missed and it "worked".)
        //
        // NOTE: must NOT call `self.metis_bar_ui_hit()` here — it re-locks this
        // output's layer map, which we already hold via `layers`, deadlocking the
        // compositor thread. Use the held guard directly.
        //
        // While auto-hidden the layer keeps full size (CSS-slid); only the peek
        // strip and real popovers may claim the pointer — otherwise the invisible
        // band steals clicks and can pin keyboard focus on the bar.
        let auto_hidden = has_bar && self.bar_is_auto_hidden(output);
        let peek_strip = if auto_hidden {
            Some(Self::bar_peek_strip_rect(&output_geo))
        } else {
            None
        };
        for layer in layers
            .layers()
            .filter(|layer| layer.namespace().starts_with("metis-bar"))
        {
            if !surface_has_buffer(layer.wl_surface()) {
                continue;
            }
            if let Some(peek) = peek_strip {
                let in_peek = point_in_rect(x, y, peek);
                if !in_peek {
                    // Popovers can extend past the peek; still honor those.
                    let Some(layer_geo) = layers.layer_geometry(layer) else {
                        continue;
                    };
                    let local = rel - layer_geo.loc.to_f64();
                    if !metis_bar_popup_tree_contains(layer.wl_surface(), local) {
                        continue;
                    }
                }
            }
            if !metis_bar_region_contains(layer, &layers, rel) {
                continue;
            }
            if let Some(hit) = self.layer_surface_at(layer, &layers, rel, output_geo) {
                return Some(hit);
            }
            // Over a bar region (strip or popover) with no concrete input surface —
            // a transparent gutter. Block fallthrough so nothing below grabs focus.
            return None;
        }

        // Shell panels on Top/Overlay (Control Center, toasts, etc.) must win over
        // desk AppBody passthrough — otherwise clicks/keys go to the window behind
        // the panel. The bar already has its own priority pass above; skip it here.
        for layer_kind in [WlrLayer::Overlay, WlrLayer::Top] {
            if let Some(layer) = layers.layer_under(layer_kind, rel) {
                if !surface_has_buffer(layer.wl_surface()) {
                    continue;
                }
                if layer.namespace().starts_with("metis-bar") {
                    continue;
                }
                if let Some(hit) = self.layer_surface_at(layer, &layers, rel, output_geo) {
                    return Some(hit);
                }
            }
        }

        if let DeskHit::AppBody { window_id } = desk_hit
            && self.window_id_at(pos) == Some(window_id)
            && let Some(hit) = self.window_surface_at(pos)
        {
            return Some(hit);
        }

        if let Some(hit) = self.window_surface_at(pos) {
            return Some(hit);
        }

        for layer_kind in [WlrLayer::Bottom, WlrLayer::Background] {
            if let Some(layer) = layers.layer_under(layer_kind, rel) {
                if !surface_has_buffer(layer.wl_surface()) {
                    continue;
                }
                if let Some(hit) = self.layer_surface_at(layer, &layers, rel, output_geo) {
                    return Some(hit);
                }
            }
        }

        None
    }

    /// Keyboard focus follows the same desk/app passthrough rules as pointer routing.
    pub fn focus_target_at(&self, location: Point<f64, Logical>) -> Option<KeyboardFocusTarget> {
        if self.screenshot_overlay_active()
            && let Some(layer) = self.screenshot_overlay_layer()
            && layer.can_receive_keyboard_focus()
        {
            return Some(layer.into());
        }

        if self.capture_overlay_active() {
            return self
                .top_capture_overlay_window()
                .map(|window| window.into());
        }

        let output = self
            .space
            .outputs()
            .find(|o| {
                self.space
                    .output_geometry(o)
                    .is_some_and(|geo| geo.contains(location.to_i32_round()))
            })
            .cloned()?;

        let output_geo = self.space.output_geometry(&output)?;
        let rel = location - output_geo.loc.to_f64();
        let (x, y) = (location.x as i32, location.y as i32);

        // Resolve the desk hit BEFORE locking the layer map below — `classify_hit`
        // locks this output's (non-reentrant) layer map internally, so doing it
        // while holding `layers` would self-deadlock the compositor thread.
        let desk_hit = self.classify_hit(x, y);

        let layers = layer_map_for_output(&output);

        for layer_kind in [WlrLayer::Overlay] {
            if let Some(layer) = layers.layer_under(layer_kind, rel)
                && layer.can_receive_keyboard_focus()
            {
                let Some(layer_loc) = layers.layer_geometry(layer).map(|g| g.loc) else {
                    continue;
                };
                if layer
                    .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
                {
                    return Some(layer.clone().into());
                }
            }
        }

        // NOTE: must NOT call `self.metis_bar_ui_hit()` here — it re-locks this
        // output's layer map, which we already hold via `layers`, deadlocking the
        // compositor thread on click. Inline the check against the held guard.
        for layer in layers
            .layers()
            .filter(|layer| layer.namespace().starts_with("metis-bar"))
        {
            if layer_accepts_pointer(layer, &layers, rel) {
                return Some(layer.clone().into());
            }
        }

        // Control Center and other Top Exclusive panels must beat AppBody
        // passthrough — otherwise typing in the Processes search goes to the
        // window underneath the panel.
        for layer_kind in [WlrLayer::Top] {
            if let Some(layer) = layers.layer_under(layer_kind, rel) {
                if layer.namespace().starts_with("metis-bar") {
                    continue;
                }
                if layer.can_receive_keyboard_focus() {
                    let Some(layer_loc) = layers.layer_geometry(layer).map(|g| g.loc) else {
                        continue;
                    };
                    if layer
                        .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
                        .is_some()
                    {
                        return Some(layer.clone().into());
                    }
                }
            }
        }

        match desk_hit {
            DeskHit::AppBody { window_id } => {
                if self.window_id_at(location) == Some(window_id)
                    && let Some((window, _)) = self.topmost_window_at_pointer(location)
                {
                    return Some(window.clone().into());
                }
            }
            DeskHit::WidgetTile { .. } | DeskHit::AppHeader { .. } => {
                for layer_kind in [WlrLayer::Top] {
                    if let Some(layer) = layers.layer_under(layer_kind, rel)
                        && layer.can_receive_keyboard_focus()
                    {
                        let Some(layer_loc) = layers.layer_geometry(layer).map(|g| g.loc) else {
                            continue;
                        };
                        if layer
                            .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
                            .is_some()
                        {
                            return Some(layer.clone().into());
                        }
                    }
                }
            }
            DeskHit::Gutter | DeskHit::Empty => {}
        }

        if let Some((window, _)) = self.topmost_window_at_pointer(location) {
            return Some(window.clone().into());
        }

        if let Some((window, _)) = self.space.element_under(location) {
            return Some(window.clone().into());
        }

        for layer_kind in [WlrLayer::Bottom, WlrLayer::Background] {
            if let Some(layer) = layers.layer_under(layer_kind, rel)
                && layer.can_receive_keyboard_focus()
            {
                let Some(layer_loc) = layers.layer_geometry(layer).map(|g| g.loc) else {
                    continue;
                };
                if layer
                    .surface_under(rel - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
                {
                    return Some(layer.clone().into());
                }
            }
        }

        None
    }
}

/// Resolve a pointer target on the bar layer, including transparent popover gutters.
fn metis_bar_layer_surface_at(
    layer: &smithay::desktop::LayerSurface,
    layers: &smithay::desktop::LayerMap,
    rel: Point<f64, Logical>,
    output_geo: smithay::utils::Rectangle<i32, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let layer_loc = layers.layer_geometry(layer)?.loc;
    let local = rel - layer_loc.to_f64();

    if let Some((surface, loc)) = layer.surface_under(local, WindowSurfaceType::ALL) {
        return Some((surface, (loc + layer_loc + output_geo.loc).to_f64()));
    }

    if !metis_bar_region_contains(layer, layers, rel) {
        return None;
    }

    // Transparent popover gutters have no input region — deliver to the popup whose
    // geometry contains the point, searching nested popovers (tray menus, etc.).
    let root = layer.wl_surface();
    if let Some((popup, location)) = metis_bar_deepest_popup(root, local) {
        let popup_origin_global = (location + layer_loc + output_geo.loc).to_f64();
        return Some((popup.wl_surface().clone(), popup_origin_global));
    }

    // GTK icon buttons may not appear in surface_under on every frame; still
    // deliver to the layer root so the shell can hit-test widgets internally.
    if metis_bar_region_contains(layer, layers, rel) {
        return Some((
            root.clone(),
            (local + layer_loc.to_f64() + output_geo.loc.to_f64()),
        ));
    }

    None
}
