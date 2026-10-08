//! Per-output desks, scroll-strip layout, and workspace routing.
//! Sibling `impl MetisState` extracted from `state.rs` (mechanical split).

use metis_grid::{GridLayout, GridMetrics, MonitorRect, PixelRect, TileKind, cell_to_pixels};
use smithay::{
    desktop::Window,
    utils::{Logical, Point},
};

use crate::focus::KeyboardFocusTarget;
use crate::state::{MetisState, default_app_tile_rect, desk_config_path};

/// Per-output desktop state. Each output (monitor) owns an independent set of
/// virtual workspaces: its visible grid (`layout`), which workspace is showing
/// (`active_workspace`), and the hidden workspaces' app tiles (`stashed_app_tiles`).
/// Desk widget tiles (clock/weather/…) only exist on the primary output's desk.
pub struct OutputDesk {
    pub layout: GridLayout,
    /// Currently visible virtual workspace on this output (1-based).
    pub active_workspace: u32,
    /// App tiles for this output's hidden workspaces, keyed by workspace id.
    pub stashed_app_tiles: std::collections::HashMap<u32, Vec<metis_grid::GridTile>>,
    /// Per-workspace layout mode (grid vs. scroll). Absent entries fall back to the
    /// configured default; the grid tiles above remain the membership source of
    /// truth in either mode.
    pub layout_kind: std::collections::HashMap<u32, metis_grid::LayoutKind>,
    /// Per-workspace scrolling-strip arrangement, used when that workspace's
    /// `layout_kind` is `Scroll`.
    pub scroll: std::collections::HashMap<u32, metis_grid::ScrollState>,
}

impl MetisState {
    /// Desk for an output key, if it exists.
    pub fn desk(&self, key: &str) -> Option<&OutputDesk> {
        self.desks.get(key)
    }

    /// Desk for an output key, creating it on demand. The first desk created is
    /// the primary (seeded with widgets from `desk.json`); later ones are app-only.
    pub fn desk_mut_or_default(&mut self, key: &str) -> &mut OutputDesk {
        if !self.desks.contains_key(key) {
            let is_primary = self.desks.is_empty();
            let mut layout = self.default_layout.clone();
            if !is_primary {
                layout
                    .tiles
                    .retain(|t| matches!(t.kind, TileKind::App { .. }));
            }
            self.desks.insert(
                key.to_string(),
                OutputDesk {
                    layout,
                    active_workspace: 1,
                    stashed_app_tiles: std::collections::HashMap::new(),
                    layout_kind: std::collections::HashMap::new(),
                    scroll: std::collections::HashMap::new(),
                },
            );
        }
        self.desks
            .get_mut(key)
            .expect("desk entry exists after insert-or-present check")
    }

    /// Ensure a desk exists for `output` (called when an output is mapped).
    pub fn ensure_desk_for_output(&mut self, output: &smithay::output::Output) {
        let key = output.name();
        let _ = self.desk_mut_or_default(&key);
    }

    /// Desk key (output name) a window belongs to. Prefers its assigned `output`,
    /// then the output under its geometry, then the primary.
    pub fn desk_key_for_window(&self, id: u32) -> String {
        if let Some(name) = self.windows.output_name(id)
            && !name.is_empty()
            && self.desks.contains_key(&name)
        {
            return name;
        }
        self.output_for_window(id)
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key())
    }

    /// Active workspace on the given output key (defaults to 1).
    pub fn active_workspace_for(&self, key: &str) -> u32 {
        self.desk(key).map(|d| d.active_workspace).unwrap_or(1)
    }

    // --- Scrolling layout -----------------------------------------------------

    /// Layout mode new workspaces start in (from `bar.json`).
    pub fn default_layout_kind(&self) -> metis_grid::LayoutKind {
        match metis_config::load_bar_config().default_layout {
            metis_config::DefaultLayout::Free => metis_grid::LayoutKind::Free,
            metis_config::DefaultLayout::Grid => metis_grid::LayoutKind::Grid,
            metis_config::DefaultLayout::Scroll => metis_grid::LayoutKind::Scroll,
        }
    }

    /// Layout mode of a specific workspace on an output (falls back to the default).
    pub fn layout_kind_for(&self, key: &str, ws: u32) -> metis_grid::LayoutKind {
        self.desk(key)
            .and_then(|d| d.layout_kind.get(&ws).copied())
            .unwrap_or_else(|| self.default_layout_kind())
    }

    /// Layout mode of the output's currently-active workspace.
    pub fn active_layout_kind(&self, key: &str) -> metis_grid::LayoutKind {
        self.layout_kind_for(key, self.active_workspace_for(key))
    }

    /// The bar-excluded usable zone for an output key, used as the scroll viewport.
    pub(crate) fn scroll_zone_for(&self, key: &str) -> PixelRect {
        match self.output_by_name(key) {
            Some(o) => self.window_placement_zone_for(&o),
            None => self.window_placement_zone(),
        }
    }

    /// Full (titlebar-inclusive) frames for the active scroll workspace on `key`,
    /// using the animated viewport offset (visual position during easing).
    pub(crate) fn scroll_frames_for(&self, key: &str) -> Vec<(u32, PixelRect)> {
        self.scroll_frames_at(key, false)
    }

    /// Full frames at the scroll target offset — where client surfaces are mapped.
    fn scroll_frames_placed_for(&self, key: &str) -> Vec<(u32, PixelRect)> {
        self.scroll_frames_at(key, true)
    }

    fn scroll_frames_at(&self, key: &str, placed: bool) -> Vec<(u32, PixelRect)> {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) != metis_grid::LayoutKind::Scroll {
            return Vec::new();
        }
        let Some(scroll) = self.desk(key).and_then(|d| d.scroll.get(&ws)) else {
            return Vec::new();
        };
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        if placed {
            scroll.layout_placed(zone, gutter)
        } else {
            scroll.layout(zone, gutter)
        }
    }

    /// Full frame for a single window when its workspace is the active scroll
    /// workspace on its output; `None` otherwise (for decorations / hit-testing).
    pub(crate) fn scroll_frame_for_window(&self, id: u32) -> Option<PixelRect> {
        let key = self.desk_key_for_window(id);
        self.scroll_frames_for(&key)
            .into_iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, rect)| rect)
    }

    /// Mapped (target-offset) frame for scroll-managed window placement.
    fn scroll_frame_placed_for_window(&self, id: u32) -> Option<PixelRect> {
        let key = self.desk_key_for_window(id);
        self.scroll_frames_placed_for(&key)
            .into_iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, rect)| rect)
    }

    /// Render-time X offset for a scroll-managed window while the viewport eases.
    pub(crate) fn scroll_render_nudge(&self, id: u32) -> i32 {
        if !self.is_active_scroll_window(id) {
            return 0;
        }
        let key = self.desk_key_for_window(id);
        let ws = self.active_workspace_for(&key);
        let Some(scroll) = self.desk(&key).and_then(|d| d.scroll.get(&ws)) else {
            return 0;
        };
        scroll.scroll_x_target - scroll.scroll_x
    }

    /// Mutable scroll state for an output's workspace, creating it on demand.
    fn scroll_state_mut(&mut self, key: &str, ws: u32) -> &mut metis_grid::ScrollState {
        self.desk_mut_or_default(key).scroll.entry(ws).or_default()
    }

    /// Recompute the scroll offset for an output's active workspace so the focused
    /// column is visible. When `animate` is true the viewport eases toward the
    /// target via render-time translation (no per-frame client reconfigure).
    fn refresh_scroll_offset(&mut self, key: &str, animate: bool) {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) != metis_grid::LayoutKind::Scroll {
            return;
        }
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        if let Some(scroll) = self.desk_mut_or_default(key).scroll.get_mut(&ws) {
            let target = scroll.desired_scroll_x(zone, gutter);
            scroll.set_scroll_target(target, zone, gutter);
            if !animate {
                scroll.snap_scroll();
            }
        }
    }

    /// Update the scroll viewport and remap windows to the target strip layout.
    fn apply_scroll_viewport(&mut self, key: &str, animate: bool) {
        self.refresh_scroll_offset(key, animate);
        self.reposition_scroll_windows();
        if animate {
            self.damaged = true;
            self.request_redraw();
        }
    }

    /// Re-snap every active scroll workspace after the usable zone changes.
    pub(crate) fn refresh_all_scroll_offsets(&mut self) {
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll {
                self.refresh_scroll_offset(&key, false);
            }
        }
    }

    /// Advance scroll-strip animations on every output; returns true while any strip
    /// is still easing toward its target.
    pub fn tick_scroll_animations(&mut self) -> bool {
        let now = std::time::Instant::now();
        let dt = self
            .last_scroll_tick
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016);
        self.last_scroll_tick = Some(now);

        let keys: Vec<String> = self.desks.keys().cloned().collect();
        let mut moved = false;
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
                continue;
            }
            if let Some(scroll) = self.desk_mut_or_default(&key).scroll.get_mut(&ws)
                && scroll.scroll_x != scroll.scroll_x_target
            {
                moved |= scroll.advance_scroll_animation(dt);
            }
        }
        if moved {
            self.request_redraw();
        }
        moved
    }

    /// Advance auto-hide titlebar slide animations. Returns true while a reveal or
    /// hide is still in progress.
    pub fn tick_titlebar_reveal_animation(&mut self) -> bool {
        const DURATION_SECS: f32 = 0.2;

        let now = std::time::Instant::now();
        let dt = self
            .last_titlebar_reveal_tick
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016);
        self.last_titlebar_reveal_tick = Some(now);

        let target = if self.revealed_titlebar.is_some() {
            1.0
        } else {
            0.0
        };

        if let Some(id) = self.revealed_titlebar {
            self.titlebar_reveal_window = Some(id);
        }

        if self.titlebar_reveal_window.is_none() {
            return false;
        }

        let before = self.titlebar_reveal_progress;
        if (before - target).abs() < 0.001 {
            self.titlebar_reveal_progress = target;
            if target <= 0.0 {
                self.titlebar_reveal_window = None;
            }
            return false;
        }

        let step = dt / DURATION_SECS;
        self.titlebar_reveal_progress = if !crate::window_fx::animations_enabled() {
            target
        } else if target > before {
            (before + step).min(target)
        } else {
            (before - step).max(target)
        };

        if self.titlebar_reveal_progress <= 0.0 {
            self.titlebar_reveal_window = None;
        }

        if (self.titlebar_reveal_progress - before).abs() > f32::EPSILON {
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Window ids slotted on a specific (output, workspace), in tile order.
    fn app_ids_for_workspace(&self, key: &str, ws: u32) -> Vec<u32> {
        let active = self.active_workspace_for(key);
        let collect_ids = |tiles: &[metis_grid::GridTile]| -> Vec<u32> {
            tiles
                .iter()
                .filter_map(|t| match &t.kind {
                    TileKind::App {
                        window_id: Some(wid),
                        ..
                    } => Some(*wid),
                    _ => None,
                })
                .collect()
        };
        self.desk(key)
            .map(|d| {
                if ws == active {
                    collect_ids(&d.layout.tiles)
                } else {
                    d.stashed_app_tiles
                        .get(&ws)
                        .map(|t| collect_ids(t))
                        .unwrap_or_default()
                }
            })
            .unwrap_or_default()
    }

    /// App windows positioned by the scroll strip (excludes free-floating clients).
    fn scroll_managed_app_ids(&self, key: &str, ws: u32) -> Vec<u32> {
        self.app_ids_for_workspace(key, ws)
            .into_iter()
            .filter(|id| !self.floating.contains(id))
            .collect()
    }

    /// True when the scroll strip lists exactly the workspace's app windows.
    fn scroll_strip_matches(app_ids: &[u32], scroll: &metis_grid::ScrollState) -> bool {
        use std::collections::HashSet;
        let strip: HashSet<u32> = scroll
            .columns
            .iter()
            .flat_map(|c| c.windows.iter().copied())
            .collect();
        let tiles: HashSet<u32> = app_ids.iter().copied().collect();
        strip == tiles
    }

    /// Window ids on a specific (output, workspace).
    pub(crate) fn window_ids_on_workspace(&self, key: &str, ws: u32) -> Vec<u32> {
        self.windows
            .ids()
            .into_iter()
            .filter(|id| {
                self.desk_key_for_window(*id) == key
                    && self.windows.workspace(*id).unwrap_or(1) == ws
            })
            .collect()
    }

    /// Bottom-to-top window order for workspace mini-desktop thumbs.
    ///
    /// Mapped windows follow `space.elements()` so raising/focusing an already-
    /// open app updates which window appears on top in the shelf preview.
    /// Unmapped windows (inactive workspaces) are appended afterward.
    pub(crate) fn workspace_thumb_stack_order(&self, key: &str, ws: u32) -> Vec<u32> {
        let mut ordered = Vec::new();
        for window in self.space.elements() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.desk_key_for_window(id) != key {
                continue;
            }
            if self.windows.workspace(id).unwrap_or(1) != ws {
                continue;
            }
            ordered.push(id);
        }
        for id in self.window_ids_on_workspace(key, ws) {
            if !ordered.contains(&id) {
                ordered.push(id);
            }
        }
        ordered
    }

    /// True when a window belongs to the active workspace on its output and may
    /// be mapped (minimized windows are handled separately).
    pub(crate) fn window_visible_on_desktop(&self, id: u32) -> bool {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        ws == self.active_workspace_for(&key)
    }

    /// Best-effort client body rect for a mapped or placed window.
    pub(crate) fn current_window_body_rect(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        if let Some(loc) = self.space.element_location(&record.window) {
            let size = record.window.geometry().size;
            if size.w > 0 && size.h > 0 {
                return Some(PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: size.w,
                    height: size.h,
                });
            }
        }
        self.windows.target_rect(id).or_else(|| {
            self.rect_for_window_tile(id)
                .map(|full| self.tile_client_rect(id, full))
        })
    }

    /// Drop grid/scroll management for a workspace — windows keep their on-screen
    /// geometry and float freely.
    fn release_workspace_to_free(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            if let Some(body) = self.current_window_body_rect(id) {
                self.windows.set_target_rect(id, body);
            }
            self.floating.insert(id);
            self.sync_auto_hide_titlebar(id);
            self.save_window_geometry(id);
        }
        let active = self.active_workspace_for(key);
        let desk = self.desk_mut_or_default(key);
        if ws == active {
            desk.layout
                .tiles
                .retain(|t| !matches!(t.kind, TileKind::App { .. }));
        } else {
            desk.stashed_app_tiles.remove(&ws);
        }
        desk.scroll.remove(&ws);
    }

    /// Pull every window on a workspace into the grid and reserve tiles.
    fn adopt_workspace_to_grid(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            self.floating.remove(&id);
            self.ensure_app_tile_for_window(id);
        }
    }

    /// Pull every window on a workspace into the scroll strip.
    fn adopt_workspace_to_scroll(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            self.floating.remove(&id);
            self.ensure_app_tile_for_window(id);
        }
        self.seed_scroll_state(key, ws);
    }

    /// Build or refresh the scroll strip for a workspace from its app tiles.
    fn seed_scroll_state(&mut self, key: &str, ws: u32) {
        let app_ids = self.scroll_managed_app_ids(key, ws);
        let focused = self.focused_window_id();
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        let desk = self.desk_mut_or_default(key);
        let needs_rebuild = desk.scroll.get(&ws).is_none_or(|s| {
            (s.columns.is_empty() && !app_ids.is_empty())
                || !Self::scroll_strip_matches(&app_ids, s)
        });
        if needs_rebuild {
            let mut scroll = metis_grid::ScrollState::new();
            for wid in &app_ids {
                scroll.insert_window_after_focus(*wid);
            }
            if let Some(f) = focused {
                scroll.focus_window(f);
            }
            desk.scroll.insert(ws, scroll);
        }
        if let Some(scroll) = desk.scroll.get_mut(&ws) {
            let target = scroll.desired_scroll_x(zone, gutter);
            scroll.set_scroll_target(target, zone, gutter);
            scroll.snap_scroll();
        }
    }

    /// Re-position only the windows on active scroll workspaces (used during
    /// viewport animation so we don't reconfigure every client every frame).
    pub(crate) fn reposition_scroll_windows(&mut self) {
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
                continue;
            }
            for id in self.scroll_managed_app_ids(&key, ws) {
                self.apply_window_rect(id);
            }
        }
    }

    /// True when `id` belongs to the active scroll workspace on its output.
    pub(crate) fn is_active_scroll_window(&self, id: u32) -> bool {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        ws == self.active_workspace_for(&key)
            && self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll
    }

    /// Physical-space clip rect for a scroll-managed window so its column can
    /// scroll off its own display's edge (carousel) without bleeding onto an
    /// adjacent output. Returns `None` for windows that aren't scroll-managed —
    /// those must not be clipped (e.g. a floating window dragged across outputs).
    pub(crate) fn scroll_window_clip(
        &self,
        id: u32,
        scale: impl Into<smithay::utils::Scale<f64>>,
    ) -> Option<smithay::utils::Rectangle<i32, smithay::utils::Physical>> {
        if !self.is_active_scroll_window(id) {
            return None;
        }
        let key = self.desk_key_for_window(id);
        let output = self.output_by_name(&key)?;
        let geo = self.space.output_geometry(&output)?;
        Some(geo.to_physical_precise_round(scale))
    }

    /// Resolve a border drag on scroll window `id` to the column it should resize.
    /// The right edge grows this window's column; the left edge grows the previous
    /// column (the shared border). Returns a representative window of the target
    /// column plus that column's current pixel width, or `None` when the drag isn't
    /// a horizontal resize of a scroll column (e.g. left edge of the first column).
    pub(crate) fn scroll_resize_target(
        &self,
        id: u32,
        edges: crate::grabs::ResizeEdge,
    ) -> Option<(u32, i32)> {
        use crate::grabs::ResizeEdge;
        if !self.is_active_scroll_window(id) {
            return None;
        }
        let key = self.desk_key_for_window(id);
        let ws = self.active_workspace_for(&key);
        let scroll = self.desk(&key)?.scroll.get(&ws)?;
        let ci = scroll.column_index_of(id)?;
        let target_ci = if edges.contains(ResizeEdge::RIGHT) {
            ci
        } else if edges.contains(ResizeEdge::LEFT) {
            ci.checked_sub(1)?
        } else {
            return None;
        };
        let zone = self.scroll_zone_for(&key);
        let target_window = *scroll.columns.get(target_ci)?.windows.first()?;
        Some((target_window, scroll.column_width_px(target_ci, zone)))
    }

    /// Set the pixel width of the scroll column holding `target_window` and reflow
    /// the strip so the columns to its right slide over to make room. Driven live
    /// from [`crate::grabs::ScrollResizeGrab`] during a mouse resize.
    pub(crate) fn scroll_set_column_width_px(&mut self, target_window: u32, width_px: i32) {
        let key = self.desk_key_for_window(target_window);
        let ws = self.active_workspace_for(&key);
        let zone = self.scroll_zone_for(&key);
        if let Some(scroll) = self.desk_mut_or_default(&key).scroll.get_mut(&ws)
            && !scroll.set_column_width_px_for(target_window, width_px, zone)
        {
            return;
        }
        self.refresh_scroll_offset(&key, false);
        self.reposition_scroll_windows();
        self.damaged = true;
        self.request_redraw();
    }

    /// Drop a window from every output's scroll state (used on destroy / move).
    fn remove_from_scroll_everywhere(&mut self, id: u32) {
        for desk in self.desks.values_mut() {
            for scroll in desk.scroll.values_mut() {
                scroll.remove_window(id);
            }
        }
    }

    /// Set the layout mode of a specific (output, workspace) without repositioning.
    /// Entering scroll seeds the strip from that workspace's app tiles (visible or
    /// stashed); leaving scroll drops the strip and de-overlaps the visible grid.
    pub(crate) fn set_layout_kind_on(&mut self, key: &str, ws: u32, kind: metis_grid::LayoutKind) {
        let active = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) == kind {
            // Pin an explicit entry so a later default change can't silently flip it.
            self.desk_mut_or_default(key).layout_kind.insert(ws, kind);
            // Still sync backing state. When bar.json already matches the target
            // (e.g. SetDefaultLayout after saving the dropdown), the early return
            // used to skip seeding scroll strips entirely.
            match kind {
                metis_grid::LayoutKind::Scroll => self.seed_scroll_state(key, ws),
                metis_grid::LayoutKind::Grid => {
                    let desk = self.desk_mut_or_default(key);
                    desk.scroll.remove(&ws);
                    if ws == active {
                        metis_grid::sanitize_layout(&mut desk.layout);
                    }
                }
                metis_grid::LayoutKind::Free => {
                    self.desk_mut_or_default(key).scroll.remove(&ws);
                }
            }
            return;
        }

        match kind {
            metis_grid::LayoutKind::Scroll => {
                self.seed_scroll_state(key, ws);
                self.desk_mut_or_default(key).layout_kind.insert(ws, kind);
            }
            metis_grid::LayoutKind::Grid => {
                let desk = self.desk_mut_or_default(key);
                desk.layout_kind.insert(ws, kind);
                desk.scroll.remove(&ws);
                if ws == active {
                    metis_grid::sanitize_layout(&mut desk.layout);
                }
            }
            metis_grid::LayoutKind::Free => {
                let desk = self.desk_mut_or_default(key);
                desk.layout_kind.insert(ws, kind);
                desk.scroll.remove(&ws);
            }
        }
    }

    /// Set the layout mode of an output's active workspace and apply it live.
    pub fn set_layout_kind(&mut self, key: &str, kind: metis_grid::LayoutKind) {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) == kind {
            return;
        }
        let focused = self.focused_window_id();
        self.set_layout_kind_on(key, ws, kind);
        match kind {
            metis_grid::LayoutKind::Grid => {
                self.adopt_workspace_to_grid(key, ws);
                self.auto_reflow_grid_apps(key, focused, false);
            }
            metis_grid::LayoutKind::Scroll => {
                self.adopt_workspace_to_scroll(key, ws);
                self.refresh_scroll_offset(key, false);
                self.reposition_scroll_windows();
            }
            metis_grid::LayoutKind::Free => {
                self.release_workspace_to_free(key, ws);
                self.reposition_all_windows();
                self.persist_layout();
            }
        }
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
    }

    /// Apply a layout mode to every workspace on every output at once, so the
    /// settings "New workspace layout" default behaves as a live global on/off.
    pub fn set_layout_kind_all(&mut self, kind: metis_grid::LayoutKind) {
        let count = self.workspace_count();
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        let focused = self.focused_window_id();
        for key in &keys {
            for ws in 1..=count {
                self.set_layout_kind_on(key, ws, kind);
            }
            match kind {
                metis_grid::LayoutKind::Free => {
                    for ws in 1..=count {
                        self.release_workspace_to_free(key, ws);
                    }
                }
                metis_grid::LayoutKind::Grid => {
                    for ws in 1..=count {
                        self.adopt_workspace_to_grid(key, ws);
                    }
                    self.auto_reflow_grid_apps(key, focused, false);
                }
                metis_grid::LayoutKind::Scroll => {
                    for ws in 1..=count {
                        self.adopt_workspace_to_scroll(key, ws);
                    }
                    self.refresh_scroll_offset(key, false);
                }
            }
        }
        self.reposition_all_windows();
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
    }

    /// Turn on grid tiling for the active workspace under `key`.
    pub fn enable_grid_tiling(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);
        if self.active_layout_kind(key) == metis_grid::LayoutKind::Grid {
            return;
        }
        tracing::info!(output = key, "enable_grid_tiling");
        self.set_layout_kind(key, metis_grid::LayoutKind::Grid);
    }

    /// Return the active workspace to a normal floating desktop (grid/scroll off).
    pub fn disable_grid_tiling(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);
        if self.active_layout_kind(key) == metis_grid::LayoutKind::Free {
            return;
        }
        tracing::info!(output = key, "disable_grid_tiling");
        self.set_layout_kind(key, metis_grid::LayoutKind::Free);
    }

    /// Cycle the active workspace: free desktop → grid tiling → scrolling.
    pub fn toggle_layout_kind(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);

        let next = match self.active_layout_kind(key) {
            metis_grid::LayoutKind::Free => metis_grid::LayoutKind::Grid,
            metis_grid::LayoutKind::Grid => metis_grid::LayoutKind::Scroll,
            metis_grid::LayoutKind::Scroll => metis_grid::LayoutKind::Free,
        };
        tracing::info!(output = key, ?next, "toggle_layout_kind");
        self.set_layout_kind(key, next);
    }

    /// Give a window keyboard focus and raise it (mirrors `activate_window`'s tail).
    pub fn focus_window_id(&mut self, id: u32) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.note_window_focus(id);
        self.space.raise_element(&record.window, true);
        if self.focused_window_id() == Some(id) {
            return;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
            // Keyboard-focus diagnostics: the reported "mouse works but Esc/keys
            // don't reach the game" is a keyboard-focus problem, so log exactly
            // which window (and whether its XWayland surface is associated yet —
            // an X11 game that recreates its window on a video-mode change can be
            // focused before its `wl_surface` is linked, which drops key delivery).
            use smithay::wayland::seat::WaylandFocus;
            tracing::info!(
                id,
                app_id = ?record.app_id,
                is_x11 = record.is_x11,
                has_wl_surface = record.window.wl_surface().is_some(),
                "focus: keyboard focus set"
            );
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
        }
    }

    /// Apply a scroll action to the output under the pointer's active scroll
    /// workspace, then reposition and refocus. No-op unless that workspace is in
    /// scroll mode.
    fn with_active_scroll<F: FnOnce(&mut metis_grid::ScrollState)>(&mut self, f: F) -> bool {
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        let ws = self.active_workspace_for(&key);
        if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
            return false;
        }
        {
            let scroll = self.scroll_state_mut(&key, ws);
            f(scroll);
        }
        self.apply_scroll_viewport(&key, true);
        let focused = self
            .desk(&key)
            .and_then(|d| d.scroll.get(&ws))
            .and_then(|s| s.focused_window());
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        true
    }

    /// Keep the scroll strip's focused column aligned with keyboard focus.
    pub fn sync_scroll_focus_for_window(&mut self, id: u32) {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if ws != self.active_workspace_for(&key) {
            return;
        }
        if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
            return;
        }
        let changed = self
            .desk_mut_or_default(&key)
            .scroll
            .get_mut(&ws)
            .map(|scroll| {
                let before = scroll.focused_window();
                scroll.focus_window(id);
                before != scroll.focused_window()
            })
            .unwrap_or(false);
        if changed {
            self.apply_scroll_viewport(&key, true);
        }
    }

    pub fn scroll_focus_left(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_left())
    }
    pub fn scroll_focus_right(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_right())
    }
    pub fn scroll_focus_up(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_up())
    }
    pub fn scroll_focus_down(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_down())
    }
    pub fn scroll_move_left(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_column_left())
    }
    pub fn scroll_move_right(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_column_right())
    }
    pub fn scroll_move_up(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_window_up())
    }
    pub fn scroll_move_down(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_window_down())
    }
    pub fn scroll_consume(&mut self) -> bool {
        self.with_active_scroll(|s| s.consume_into_prev())
    }
    pub fn scroll_expel(&mut self) -> bool {
        self.with_active_scroll(|s| s.expel_to_new_column())
    }
    pub fn scroll_cycle_width(&mut self) -> bool {
        self.with_active_scroll(|s| s.cycle_focus_width())
    }

    /// Grid metrics (columns/rows/gutter + monitor rect) for a specific output.
    pub fn grid_metrics_for(&self, output: &smithay::output::Output) -> GridMetrics {
        let key = output.name();
        let (columns, rows) = self
            .desk(&key)
            .map(|d| (d.layout.columns, d.layout.rows))
            .unwrap_or((self.default_layout.columns, self.default_layout.rows));
        let zone = self.grid_placement_zone_for(output);
        GridMetrics {
            columns,
            rows,
            gutter: self.gutter_px,
            monitor: MonitorRect {
                x: zone.x,
                y: zone.y,
                width: zone.width,
                height: zone.height,
            },
        }
    }

    /// Hide persistent titlebars for grid-tiled windows; reveal on hover.
    fn sync_grid_titlebar_chrome(&mut self, output_key: &str) {
        let ws = self.active_workspace_for(output_key);
        if self.layout_kind_for(output_key, ws) != metis_grid::LayoutKind::Grid {
            return;
        }
        for id in self.window_ids_on_workspace(output_key, ws) {
            if self.tile_id_for_window(id).is_some()
                && !self.floating.contains(&id)
                && self.should_auto_hide_titlebar(id)
            {
                self.auto_hide_titlebar.insert(id);
            } else {
                self.sync_auto_hide_titlebar(id);
            }
        }
    }

    /// Grid metrics for the primary output (back-compat for output-agnostic call sites).
    pub fn grid_metrics(&self) -> GridMetrics {
        match self.primary_output() {
            Some(o) => self.grid_metrics_for(&o),
            None => GridMetrics {
                columns: self.default_layout.columns,
                rows: self.default_layout.rows,
                gutter: self.gutter_px,
                monitor: self.monitor,
            },
        }
    }

    /// Find the app tile for `window_id` across all outputs' visible layouts,
    /// returning its output key and a clone of the tile.
    pub fn find_app_tile(&self, window_id: u32) -> Option<(String, metis_grid::GridTile)> {
        for (key, desk) in &self.desks {
            for tile in &desk.layout.tiles {
                if let TileKind::App {
                    window_id: Some(wid),
                    ..
                } = &tile.kind
                    && *wid == window_id
                {
                    return Some((key.clone(), tile.clone()));
                }
            }
        }
        None
    }

    /// Output key whose visible layout currently contains `tile_id`.
    pub fn desk_key_for_tile(&self, tile_id: &str) -> Option<String> {
        self.desks.iter().find_map(|(key, desk)| {
            desk.layout
                .tiles
                .iter()
                .any(|t| t.id == tile_id)
                .then(|| key.clone())
        })
    }

    /// Drop app tiles whose window no longer exists and dedupe multiple tiles for
    /// the same live window (stale `desk.json` entries otherwise block reflow).
    fn prune_stale_app_tiles(&mut self, output_key: &str) {
        use std::collections::{HashMap, HashSet};

        let live: HashSet<u32> = self.windows.ids().into_iter().collect();
        let desk = self.desk_mut_or_default(output_key);

        let prune_list = |tiles: &mut Vec<metis_grid::GridTile>| {
            tiles.retain(|t| match &t.kind {
                TileKind::App {
                    window_id: Some(wid),
                    ..
                } => live.contains(wid),
                TileKind::App {
                    window_id: None, ..
                } => false,
                _ => true,
            });
            let mut keep: HashMap<u32, String> = HashMap::new();
            for t in tiles.iter() {
                let TileKind::App {
                    window_id: Some(wid),
                    ..
                } = &t.kind
                else {
                    continue;
                };
                let canonical = format!("app-{wid}");
                keep.entry(*wid).or_insert_with(|| t.id.clone());
                if t.id == canonical {
                    keep.insert(*wid, canonical);
                }
            }
            tiles.retain(|t| match &t.kind {
                TileKind::App {
                    window_id: Some(wid),
                    ..
                } => keep.get(wid) == Some(&t.id),
                _ => true,
            });
        };

        prune_list(&mut desk.layout.tiles);
        for tiles in desk.stashed_app_tiles.values_mut() {
            prune_list(tiles);
        }
    }

    /// Drop a window's app tile from every desk (visible and stashed).
    pub(crate) fn remove_app_tile_everywhere(&mut self, window_id: u32) {
        let matches_window = |t: &metis_grid::GridTile| matches!(&t.kind, TileKind::App { window_id: Some(wid), .. } if *wid == window_id);
        for desk in self.desks.values_mut() {
            desk.layout.tiles.retain(|t| !matches_window(t));
            for tiles in desk.stashed_app_tiles.values_mut() {
                tiles.retain(|t| !matches_window(t));
            }
            for scroll in desk.scroll.values_mut() {
                scroll.remove_window(window_id);
            }
        }
    }

    pub(crate) fn rect_for_window_tile(&self, id: u32) -> Option<PixelRect> {
        if self.floating.contains(&id) {
            return None;
        }
        // Scrolling workspaces position from the strip, not the tile grid.
        if let Some(frame) = self.scroll_frame_placed_for_window(id) {
            return Some(frame);
        }
        let key = self.desk_key_for_window(id);
        let desk = self.desk(&key)?;
        let tile = desk.layout.tiles.iter().find(
            |t| matches!(&t.kind, TileKind::App { window_id: Some(wid), .. } if *wid == id),
        )?;
        let metrics = match self.output_by_name(&key) {
            Some(o) => self.grid_metrics_for(&o),
            None => self.grid_metrics(),
        };
        Some(cell_to_pixels(&metrics, &tile.rect))
    }

    pub fn apply_grid_layout(&mut self, shell_layout: GridLayout, gutter_px: u32) {
        use std::collections::HashMap;

        // The shell desk editor is dormant; this path applies to the primary desk.
        let key = self.primary_key();
        let compositor_apps: HashMap<String, metis_grid::GridTile> = self
            .desk(&key)
            .map(|d| {
                d.layout
                    .tiles
                    .iter()
                    .filter(|t| matches!(t.kind, TileKind::App { .. }))
                    .map(|t| (t.id.clone(), t.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut merged = shell_layout;
        for tile in &mut merged.tiles {
            let TileKind::App { window_id, class } = &mut tile.kind else {
                continue;
            };
            let Some(existing) = compositor_apps.get(&tile.id) else {
                continue;
            };
            let TileKind::App {
                window_id: existing_wid,
                class: existing_class,
            } = &existing.kind
            else {
                continue;
            };
            if window_id.is_none() {
                *window_id = *existing_wid;
            }
            if class.as_deref().unwrap_or("").is_empty() {
                *class = existing_class.clone();
            }
        }

        for app in compositor_apps.values() {
            if !merged.tiles.iter().any(|t| t.id == app.id) {
                merged.tiles.push(app.clone());
            }
        }

        self.desk_mut_or_default(&key).layout = merged;
        self.gutter_px = gutter_px;
        self.ensure_app_tiles_for_open_windows();
        self.sync_grid_titlebar_chrome(&key);
        self.reposition_all_windows();
    }

    fn ensure_app_tiles_for_open_windows(&mut self) {
        for id in self.windows.ids() {
            self.ensure_app_tile_for_window(id);
        }
    }

    pub(crate) fn reposition_all_windows(&mut self) {
        for id in self.windows.ids() {
            self.remap_window_for_desktop(id);
        }
        self.restore_focus_stacking();
    }

    /// After bulk layout sync, put the user-focused window back on top without
    /// toggling xdg activation state on neighbors.
    pub(crate) fn restore_focus_stacking(&mut self) {
        let Some(id) = self.preferred_stacking_window() else {
            return;
        };
        self.raise_stacking_window(id, false);
    }

    /// Keep the window the user picked above neighbors while the pointer is over
    /// its chrome/body. Uses the window's frame geometry, not `element_under`, so
    /// a neighbor stacked too high during minimize/maximize restore cannot block
    /// the raise when the cursor reaches the chosen app.
    pub(crate) fn maintain_focus_stacking(&mut self, loc: Point<f64, Logical>) {
        use crate::desk_input::point_in_rect;

        if self.capture_overlay_active() {
            return;
        }
        if self.screenshot_overlay_active() {
            return;
        }

        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return;
        }

        // Grab-less Metis menu / NC / CC: Exclusive keyboard focus lives on the
        // bar layer while the pointer still geometrically hits windows under the
        // translucent popover. Raising/focusing those windows every motion would
        // ping-pong with `handle_layer_commit` reclaiming Exclusive — flickering
        // the menu search `:focus-within` border and every SSD titlebar.
        if self.metis_bar_ui_hit(loc) {
            return;
        }
        if self.exclusive_keyboard_layer().is_some() {
            return;
        }

        // A transient X11 popup (menu / tooltip / combo dropdown) is mapped above
        // its parent toplevel. Auto-raising the registered window under the
        // pointer would restack it *above* its own override-redirect popup,
        // occluding the menu — the owning app (e.g. Steam) then treats it as
        // dismissed and closes it on the very next mouse move. Leave stacking
        // untouched while any OR popup is up.
        if self.has_mapped_override_redirect_popup() {
            return;
        }

        let Some(preferred) = self.preferred_stacking_window() else {
            return;
        };
        let Some(record) = self.windows.get(preferred).cloned() else {
            return;
        };
        if record.maximized || record.fullscreen || self.windows.is_minimized(preferred) {
            return;
        }
        let frame = self
            .ssd_frame_for_mapped_window(preferred, &record.window)
            .or_else(|| {
                let loc = self.space.element_location(&record.window)?;
                let size = record.window.geometry().size;
                Some(PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: size.w.max(1),
                    height: size.h.max(1),
                })
            });
        let Some(frame) = frame else {
            return;
        };
        let (x, y) = (loc.x as i32, loc.y as i32);
        if !point_in_rect(x, y, frame) {
            return;
        }
        if self.higher_window_client_occludes(x, y, preferred) {
            return;
        }
        self.raise_stacking_window(preferred, false);
        if self.focused_window_id() != Some(preferred) {
            let pointer_ok = self.seat.get_pointer().is_none_or(|p| !p.is_grabbed());
            let keyboard_ok = self.seat.get_keyboard().is_none_or(|k| !k.is_grabbed());
            if pointer_ok
                && keyboard_ok
                && let Some(keyboard) = self.seat.get_keyboard()
            {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.set_focus(self, Some(record.window.into()), serial);
            }
        }
    }

    /// Reserve a grid slot as soon as an app registers (before its first buffer commit).
    pub(crate) fn ensure_app_tile_for_window(&mut self, id: u32) {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Free {
            return;
        }

        let tile_id = format!("app-{id}");
        // Already present, visible or stashed on this output's desk?
        if let Some(desk) = self.desk(&key)
            && (desk.layout.tiles.iter().any(|t| t.id == tile_id)
                || desk
                    .stashed_app_tiles
                    .values()
                    .any(|tiles| tiles.iter().any(|t| t.id == tile_id)))
        {
            return;
        }
        let class = self.windows.get(id).and_then(|r| r.app_id.clone());
        let active = self.active_workspace_for(&key);
        let desk = self.desk_mut_or_default(&key);
        let tile = metis_grid::GridTile {
            id: tile_id,
            rect: default_app_tile_rect(&desk.layout),
            kind: TileKind::App {
                window_id: Some(id),
                class,
            },
            glow: "cool".into(),
            pinned: false,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        };
        if ws == active {
            desk.layout.tiles.push(tile);
        } else {
            desk.stashed_app_tiles.entry(ws).or_default().push(tile);
        }
        // Mirror membership into the scroll strip when this workspace scrolls.
        if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(&key, ws);
            self.refresh_scroll_offset(&key, false);
            // Slide the windows already in the strip into their new frames — a fresh
            // column shifts every column to its right, so the prior window must move
            // instead of the newcomer just painting on top of it.
            self.reposition_scroll_windows();
        } else {
            self.auto_reflow_grid_apps(&key, Some(id), true);
        }
    }

    /// Split the active grid workspace among grid-managed app windows.
    pub(crate) fn auto_reflow_grid_apps(
        &mut self,
        output_key: &str,
        focus_window_id: Option<u32>,
        emit: bool,
    ) {
        let ws = self.active_workspace_for(output_key);
        if self.layout_kind_for(output_key, ws) != metis_grid::LayoutKind::Grid {
            return;
        }

        self.prune_stale_app_tiles(output_key);

        let include: Vec<String> = self
            .desk(output_key)
            .map(|desk| {
                desk.layout
                    .tiles
                    .iter()
                    .filter_map(|t| {
                        if let TileKind::App {
                            window_id: Some(wid),
                            ..
                        } = &t.kind
                        {
                            self.is_window_grid_managed(*wid).then(|| t.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if include.is_empty() {
            return;
        }

        let focus_tile_id = focus_window_id.and_then(|id| self.tile_id_for_window(id));

        let desk = self.desk_mut_or_default(output_key);
        metis_grid::sanitize_layout(&mut desk.layout);
        let focus = focus_tile_id.as_deref();
        if let Err(err) = metis_grid::auto_tile_apps(&mut desk.layout, focus, &include) {
            tracing::warn!(%err, output = output_key, "auto_tile_apps failed after sanitize; retrying");
            metis_grid::sanitize_layout(&mut desk.layout);
            if let Err(err) = metis_grid::auto_tile_apps(&mut desk.layout, focus, &include) {
                tracing::warn!(%err, output = output_key, "auto_tile_apps failed after retry");
            }
        }
        self.sync_grid_titlebar_chrome(output_key);
        self.reposition_all_windows();
        self.persist_layout();
        if emit {
            self.emit_layout_changed();
        }
    }

    pub fn sync_all_app_windows(&mut self) {
        for id in self.windows.ids() {
            self.ensure_app_tile_for_window(id);
        }
        self.try_activate_all_pending();
        self.reposition_all_windows();
    }

    pub fn try_activate_all_pending(&mut self) {
        let pending: Vec<_> = self
            .windows
            .ids()
            .into_iter()
            .filter(|id| !self.windows.is_ready(*id))
            .filter_map(|id| {
                self.windows
                    .get(id)
                    .and_then(|record| record.wl_toplevel())
                    .map(|toplevel| toplevel.wl_surface().clone())
            })
            .collect();
        for surface in pending {
            self.try_activate_committed_window(&surface);
        }
    }

    pub(crate) fn persist_layout(&mut self) {
        // Persist the primary desk's layout (its widget positions) to `desk.json`.
        let key = self.primary_key();
        if let Some(desk) = self.desk(&key)
            && let Err(err) = desk.layout.save_to_path(&desk_config_path())
        {
            tracing::warn!(%err, "failed to persist grid layout");
        }
    }

    pub fn emit_layout_changed(&self) {
        use metis_protocol::CompositorEvent;
        let key = self.primary_key();
        let layout = self
            .desk(&key)
            .map(|d| d.layout.clone())
            .unwrap_or_else(|| self.default_layout.clone());
        self.event_bus.emit(&CompositorEvent::LayoutChanged {
            layout,
            gutter_px: self.gutter_px,
            metrics: self.grid_metrics(),
        });
    }

    pub fn emit_monitor_changed(&self) {
        use metis_protocol::CompositorEvent;
        self.event_bus
            .emit(&CompositorEvent::MonitorChanged { rect: self.monitor });
    }

    pub fn emit_workspace_changed(&self, output_key: &str) {
        use metis_protocol::CompositorEvent;
        self.event_bus.emit(&CompositorEvent::WorkspaceChanged {
            output: output_key.to_string(),
            active: self.active_workspace_for(output_key),
            count: self.workspace_count(),
            ephemeral_remote: self.ephemeral_remote_workspace(output_key),
        });
    }

    /// Configured number of virtual workspaces (clamped to a sane 1..=12).
    pub fn workspace_count(&self) -> u32 {
        metis_config::load_bar_config().workspace_count.clamp(1, 12)
    }

    /// Ephemeral FreeRDP dedicated-desk id (`workspace_count() + 1`) while one is
    /// reserved for `output_key`; otherwise `None`.
    pub(crate) fn ephemeral_remote_workspace(&self, output_key: &str) -> Option<u32> {
        self.remote_viewer_workspace.get(output_key).copied()
    }

    /// Highest reachable workspace id on this output (permanent count, or the
    /// ephemeral remote desk when one is active).
    fn workspace_span_for(&self, output_key: &str) -> u32 {
        let count = self.workspace_count().max(1);
        self.ephemeral_remote_workspace(output_key)
            .unwrap_or(count)
            .max(count)
    }

    /// Clamp a workspace id to permanent desks, or allow the registered ephemeral
    /// remote id for this output.
    fn clamp_workspace_for(&self, output_key: &str, target: u32) -> u32 {
        let count = self.workspace_count().max(1);
        if (1..=count).contains(&target) {
            return target;
        }
        if self.ephemeral_remote_workspace(output_key) == Some(target) {
            return target;
        }
        target.clamp(1, count)
    }

    /// Configured multi-monitor workspace behavior (independent vs. linked).
    pub fn workspace_mode(&self) -> metis_config::WorkspaceMode {
        metis_config::load_bar_config().workspace_mode
    }

    /// Switch workspace honoring the configured multi-monitor mode. In `Separate`
    /// only `requested_output` changes; in `Linked` every output switches to the
    /// same workspace at once (each emits its own `WorkspaceChanged`).
    /// Ephemeral remote desks never fan out — they exist only on the output that
    /// hosts the FreeRDP session.
    pub fn switch_workspace_routed(&mut self, requested_output: &str, target: u32) {
        if self.ephemeral_remote_workspace(requested_output) == Some(target) {
            self.switch_workspace(requested_output, target);
            return;
        }
        if self.workspace_mode() == metis_config::WorkspaceMode::Linked {
            let keys: Vec<String> = self.space.outputs().map(|o| o.name()).collect();
            if keys.is_empty() {
                self.switch_workspace(requested_output, target);
            } else {
                for key in keys {
                    self.switch_workspace(&key, target);
                }
            }
        } else {
            self.switch_workspace(requested_output, target);
        }
    }

    /// Step to the previous/next workspace (wrapping across permanent desks and
    /// the ephemeral remote desk when present), honoring linked vs. separate mode.
    pub fn cycle_workspace_routed(&mut self, requested_output: &str, delta: i32) {
        let max = self.workspace_span_for(requested_output);
        let current = self.active_workspace_for(requested_output);
        let target = if delta >= 0 {
            if current >= max { 1 } else { current + 1 }
        } else if current <= 1 {
            max
        } else {
            current - 1
        };
        self.switch_workspace_routed(requested_output, target);
    }

    /// Show a different virtual workspace on a single output. Stashes that
    /// output's visible app tiles (and unmaps their windows), then restores the
    /// target workspace's tiles and remaps its windows. Other outputs and the
    /// desk widget tiles are untouched.
    pub fn switch_workspace(&mut self, output_key: &str, target: u32) {
        let target = self.clamp_workspace_for(output_key, target);
        let current = self.active_workspace_for(output_key);
        if target == current {
            return;
        }

        if self.layout_kind_for(output_key, current) == metis_grid::LayoutKind::Free {
            for id in self.window_ids_on_workspace(output_key, current) {
                if let Some(record) = self.windows.get(id).cloned() {
                    self.space.unmap_elem(&record.window);
                }
            }
            self.desk_mut_or_default(output_key).active_workspace = target;
            for id in self.window_ids_on_workspace(output_key, target) {
                self.ensure_app_tile_for_window(id);
                self.remap_window_for_desktop(id);
            }
            self.focus_topmost_on_active_workspace();
            self.emit_workspace_changed(output_key);
            return;
        }

        // Pull this output's app tiles out of its live grid and remember them.
        let mut stashed: Vec<metis_grid::GridTile> = Vec::new();
        {
            let desk = self.desk_mut_or_default(output_key);
            desk.layout.tiles.retain(|t| {
                if matches!(t.kind, TileKind::App { .. }) {
                    stashed.push(t.clone());
                    false
                } else {
                    true
                }
            });
        }
        // Hide the windows that just left the visible workspace.
        for tile in &stashed {
            if let TileKind::App {
                window_id: Some(wid),
                ..
            } = &tile.kind
                && let Some(record) = self.windows.get(*wid).cloned()
            {
                self.space.unmap_elem(&record.window);
            }
        }
        {
            let desk = self.desk_mut_or_default(output_key);
            desk.stashed_app_tiles.insert(current, stashed);
            desk.active_workspace = target;
            // Restore the target workspace's app tiles.
            if let Some(tiles) = desk.stashed_app_tiles.remove(&target) {
                desk.layout.tiles.extend(tiles);
            }
        }
        self.refresh_scroll_offset(output_key, false);
        if self.layout_kind_for(output_key, target) == metis_grid::LayoutKind::Grid {
            self.auto_reflow_grid_apps(
                output_key,
                self.last_focused_window.or(self.focused_window_id()),
                false,
            );
        }
        for id in self.window_ids_on_workspace(output_key, target) {
            self.ensure_app_tile_for_window(id);
        }
        self.reposition_all_windows();
        self.focus_topmost_on_active_workspace();

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(output_key);
    }

    /// Output names sorted left-to-right (then top-to-bottom) for adjacent-monitor
    /// navigation.
    fn output_keys_left_to_right(&self) -> Vec<String> {
        let mut outputs: Vec<_> = self.space.outputs().collect();
        outputs.sort_by_key(|o| {
            self.space
                .output_geometry(o)
                .map(|g| (g.loc.x, g.loc.y))
                .unwrap_or((0, 0))
        });
        outputs.into_iter().map(|o| o.name()).collect()
    }

    fn adjacent_output_key(&self, from: &str, direction: i32) -> Option<String> {
        let keys = self.output_keys_left_to_right();
        let idx = keys.iter().position(|k| k == from)?;
        let next = idx as i32 + direction;
        if next < 0 || next >= keys.len() as i32 {
            return None;
        }
        Some(keys[next as usize].clone())
    }

    /// Remove a window's app tile from one output desk (visible layout or stash).
    fn take_app_tile_from_desk(
        &mut self,
        desk_key: &str,
        window_id: u32,
        workspace: u32,
    ) -> Option<metis_grid::GridTile> {
        let tile_id = format!("app-{window_id}");
        let desk = self.desk_mut_or_default(desk_key);
        if let Some(pos) = desk.layout.tiles.iter().position(|t| t.id == tile_id) {
            return Some(desk.layout.tiles.remove(pos));
        }
        if let Some(tiles) = desk.stashed_app_tiles.get_mut(&workspace)
            && let Some(pos) = tiles.iter().position(|t| t.id == tile_id)
        {
            return Some(tiles.remove(pos));
        }
        for tiles in desk.stashed_app_tiles.values_mut() {
            if let Some(pos) = tiles.iter().position(|t| t.id == tile_id) {
                return Some(tiles.remove(pos));
            }
        }
        None
    }

    fn remove_window_from_desk_scroll(&mut self, desk_key: &str, window_id: u32, workspace: u32) {
        if let Some(desk) = self.desks.get_mut(desk_key)
            && let Some(scroll) = desk.scroll.get_mut(&workspace)
        {
            scroll.remove_window(window_id);
        }
    }

    /// Move a window to another output, keeping its workspace number. Desk tiles
    /// and scroll membership follow the window; visibility follows the destination
    /// output's active workspace.
    pub fn move_window_to_output(&mut self, window_id: u32, target_key: &str) {
        self.move_window_to_output_inner(window_id, target_key, true);
    }

    /// Like [`move_window_to_output`](Self::move_window_to_output) but optionally
    /// skips geometry clamp/reposition (used when a snap immediately follows).
    pub(crate) fn move_window_to_output_inner(
        &mut self,
        window_id: u32,
        target_key: &str,
        reposition: bool,
    ) {
        if target_key.is_empty() {
            return;
        }
        if self.output_by_name(target_key).is_none() && !self.desks.contains_key(target_key) {
            return;
        }
        self.desk_mut_or_default(target_key);

        let source_key = self.desk_key_for_window(window_id);
        if source_key == target_key {
            return;
        }

        let workspace = self.windows.workspace(window_id).unwrap_or(1);
        let source_active = self.active_workspace_for(&source_key);
        let target_active = self.active_workspace_for(target_key);
        let was_visible = workspace == source_active;
        let will_be_visible = workspace == target_active;

        let mut tile = self.take_app_tile_from_desk(&source_key, window_id, workspace);
        self.remove_window_from_desk_scroll(&source_key, window_id, workspace);

        if tile.is_none() {
            let class = self.windows.get(window_id).and_then(|r| r.app_id.clone());
            tile = Some(metis_grid::GridTile {
                id: format!("app-{window_id}"),
                rect: default_app_tile_rect(
                    self.desk(target_key)
                        .map(|d| &d.layout)
                        .unwrap_or(&self.default_layout),
                ),
                kind: TileKind::App {
                    window_id: Some(window_id),
                    class,
                },
                glow: "cool".into(),
                pinned: false,
                min_w: None,
                max_w: None,
                min_h: None,
                max_h: None,
            });
        }

        self.windows.set_output(window_id, target_key.to_string());

        if let Some(tile) = tile {
            let desk = self.desk_mut_or_default(target_key);
            if will_be_visible {
                desk.layout.tiles.push(tile);
            } else {
                desk.stashed_app_tiles
                    .entry(workspace)
                    .or_default()
                    .push(tile);
            }
        }

        if self.layout_kind_for(target_key, workspace) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(target_key, workspace);
            self.refresh_scroll_offset(target_key, false);
        }

        if was_visible && !will_be_visible {
            if let Some(record) = self.windows.get(window_id).cloned() {
                self.space.unmap_elem(&record.window);
            }
            self.focus_topmost_on_active_workspace();
        } else if will_be_visible && reposition {
            if self.floating.contains(&window_id) {
                // Auto-hide / snapped windows keep their footprint — never apply
                // the ordinary floating titlebar inset (`APP_TILE_HEADER_PX`).
                if !self.auto_hide_titlebar.contains(&window_id)
                    && !self.windows.is_snapped(window_id)
                    && let Some(rect) = self.windows.target_rect(window_id)
                {
                    let clamped = self.clamp_floating_rect_for(window_id, rect);
                    if clamped != rect {
                        self.windows.set_target_rect(window_id, clamped);
                    }
                }
                self.apply_window_rect(window_id);
            } else {
                self.apply_window_rect(window_id);
            }
            self.focus_window_id(window_id);
        }

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(&source_key);
        self.emit_workspace_changed(target_key);
    }

    /// If a window's center sits on a different output than its assigned desk,
    /// re-home it there. Called after a drag-drop or snap on another monitor.
    pub fn maybe_adopt_window_output(&mut self, window_id: u32) {
        let Some(target) = self.output_for_window(window_id).map(|o| o.name()) else {
            return;
        };
        if target == self.desk_key_for_window(window_id) {
            return;
        }
        self.move_window_to_output(window_id, &target);
    }

    /// Move the focused window one output to the left (`direction` = -1) or right (+1).
    pub fn move_window_to_adjacent_output(&mut self, window_id: u32, direction: i32) {
        let from = self.desk_key_for_window(window_id);
        let Some(target) = self.adjacent_output_key(&from, direction) else {
            return;
        };
        self.move_window_to_output(window_id, &target);
    }

    /// Move every window on `workspace` from `source_key` to `target_key` (keeping
    /// the same workspace number). Layout mode and scroll state for that workspace
    /// move with the windows. Only valid in independent per-output workspace mode.
    pub fn move_workspace_to_output(&mut self, source_key: &str, workspace: u32, target_key: &str) {
        if source_key.is_empty() || target_key.is_empty() || source_key == target_key {
            return;
        }
        if self.workspace_mode() != metis_config::WorkspaceMode::Separate {
            return;
        }
        if self.output_by_name(target_key).is_none() && !self.desks.contains_key(target_key) {
            return;
        }
        self.desk_mut_or_default(target_key);

        let ws = workspace.clamp(1, self.workspace_count());
        let source_active = self.active_workspace_for(source_key);
        let target_active = self.active_workspace_for(target_key);
        let was_visible_on_source = ws == source_active;
        let will_be_visible_on_target = ws == target_active;

        let window_ids: Vec<u32> = self
            .windows
            .ids()
            .into_iter()
            .filter(|&id| {
                self.desk_key_for_window(id) == source_key && self.windows.workspace(id) == Some(ws)
            })
            .collect();

        let (kind, scroll, mut tiles) = {
            let desk = self.desk_mut_or_default(source_key);
            let kind = desk.layout_kind.remove(&ws);
            let scroll = desk.scroll.remove(&ws);
            let mut tiles = desk.stashed_app_tiles.remove(&ws).unwrap_or_default();
            if was_visible_on_source {
                let on_ws: std::collections::HashSet<u32> = window_ids.iter().copied().collect();
                desk.layout.tiles.retain(|t| {
                    if let TileKind::App {
                        window_id: Some(wid),
                        ..
                    } = &t.kind
                        && on_ws.contains(wid)
                    {
                        tiles.push(t.clone());
                        return false;
                    }
                    true
                });
            }
            (kind, scroll, tiles)
        };

        let default_layout = self
            .desk(target_key)
            .map(|d| &d.layout)
            .unwrap_or(&self.default_layout);
        for &id in &window_ids {
            let tile_id = format!("app-{id}");
            if tiles.iter().any(|t| t.id == tile_id) {
                continue;
            }
            let class = self.windows.get(id).and_then(|r| r.app_id.clone());
            tiles.push(metis_grid::GridTile {
                id: tile_id,
                rect: default_app_tile_rect(default_layout),
                kind: TileKind::App {
                    window_id: Some(id),
                    class,
                },
                glow: "cool".into(),
                pinned: false,
                min_w: None,
                max_w: None,
                min_h: None,
                max_h: None,
            });
        }

        if was_visible_on_source {
            for &id in &window_ids {
                if let Some(record) = self.windows.get(id).cloned() {
                    self.space.unmap_elem(&record.window);
                }
            }
        }

        for &id in &window_ids {
            self.windows.set_output(id, target_key.to_string());
        }

        {
            let desk = self.desk_mut_or_default(target_key);
            if let Some(k) = kind {
                desk.layout_kind.insert(ws, k);
            }
            if let Some(s) = scroll {
                desk.scroll.insert(ws, s);
            }
            if will_be_visible_on_target {
                desk.layout.tiles.extend(tiles);
            } else {
                desk.stashed_app_tiles.entry(ws).or_default().extend(tiles);
            }
        }

        if will_be_visible_on_target {
            self.refresh_scroll_offset(target_key, false);
            for &id in &window_ids {
                if !self.windows.is_minimized(id) {
                    self.apply_window_rect(id);
                }
            }
            self.focus_topmost_on_active_workspace();
        } else if was_visible_on_source {
            self.focus_topmost_on_active_workspace();
        }

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(source_key);
        self.emit_workspace_changed(target_key);
    }

    /// Move the active workspace on `source_key` one output left/right.
    pub fn move_active_workspace_to_adjacent_output(&mut self, source_key: &str, direction: i32) {
        let ws = self.active_workspace_for(source_key);
        let Some(target) = self.adjacent_output_key(source_key, direction) else {
            return;
        };
        self.move_workspace_to_output(source_key, ws, &target);
    }

    /// True when the workspace under the pointer uses the scrolling layout (so
    /// Super+Shift+arrow is reserved for scroll navigation).
    pub fn scroll_navigation_active(&self) -> bool {
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        self.active_layout_kind(&key) == metis_grid::LayoutKind::Scroll
    }

    /// Move a window to another workspace on its own output. When it leaves or
    /// joins that output's visible workspace its tile is stashed/restored and the
    /// window is hidden/shown.
    pub fn move_window_to_workspace(&mut self, window_id: u32, target: u32) {
        let key = self.desk_key_for_window(window_id);
        let target = self.clamp_workspace_for(&key, target);
        let Some(current) = self.windows.workspace(window_id) else {
            return;
        };
        if target == current {
            return;
        }
        self.windows.set_workspace(window_id, target);
        let tile_id = format!("app-{window_id}");
        let active = self.active_workspace_for(&key);

        if current == active {
            // Leaving the visible workspace: stash its tile and hide it.
            let mut moved: Vec<metis_grid::GridTile> = Vec::new();
            {
                let desk = self.desk_mut_or_default(&key);
                desk.layout.tiles.retain(|t| {
                    if t.id == tile_id {
                        moved.push(t.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            if let Some(record) = self.windows.get(window_id).cloned() {
                self.space.unmap_elem(&record.window);
            }
            self.desk_mut_or_default(&key)
                .stashed_app_tiles
                .entry(target)
                .or_default()
                .extend(moved);
            self.reposition_all_windows();
            self.focus_topmost_on_active_workspace();
        } else if target == active {
            // Joining the visible workspace: pull its tile back into the grid.
            let desk = self.desk_mut_or_default(&key);
            if let Some(tiles) = desk.stashed_app_tiles.get_mut(&current)
                && let Some(pos) = tiles.iter().position(|t| t.id == tile_id)
            {
                let tile = tiles.remove(pos);
                desk.layout.tiles.push(tile);
            }
            self.reposition_all_windows();
        } else {
            // Hidden-to-hidden: just relocate the stashed tile.
            let desk = self.desk_mut_or_default(&key);
            let tile = desk.stashed_app_tiles.get_mut(&current).and_then(|tiles| {
                tiles
                    .iter()
                    .position(|t| t.id == tile_id)
                    .map(|pos| tiles.remove(pos))
            });
            if let Some(tile) = tile {
                desk.stashed_app_tiles.entry(target).or_default().push(tile);
            }
        }

        // Keep the scroll strips in sync: drop from the source workspace, add to
        // the target if it scrolls.
        self.remove_from_scroll_everywhere(window_id);
        if self.layout_kind_for(&key, target) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(&key, target);
        }
        self.refresh_scroll_offset(&key, false);

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        // Nudge the shell to reconcile its window cache so per-output/per-workspace
        // dock filtering reflects the move promptly (the active workspace itself is
        // unchanged; this just carries the refresh).
        self.emit_workspace_changed(&key);
    }

    /// Give keyboard focus to the topmost mapped window on its output's active
    /// workspace, or clear focus if no eligible window is visible.
    fn focus_topmost_on_active_workspace(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // `space.elements()` is bottom-to-top; the last match is the topmost.
        let ordered: Vec<Window> = self.space.elements().cloned().collect();
        let candidate = ordered.into_iter().rev().find_map(|w| {
            let id = self.windows.id_for_window(&w)?;
            let key = self.desk_key_for_window(id);
            let on_active = self.windows.workspace(id) == Some(self.active_workspace_for(&key));
            if on_active && !self.windows.is_minimized(id) {
                Some((id, w))
            } else {
                None
            }
        });
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        match candidate {
            Some((id, window)) => {
                self.space.raise_element(&window, true);
                keyboard.set_focus(self, Some(window.into()), serial);
                self.event_bus
                    .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
            }
            None => {
                keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
            }
        }
    }

    /// Dedicated FreeRDP sessions always own an ephemeral desk at
    /// `workspace_count() + 1` so permanent desks 1..=N stay untouched.
    pub(crate) fn pick_remote_viewer_workspace(&self, key: &str, window_id: u32) -> u32 {
        let ephemeral = self.workspace_count().saturating_add(1);
        if let Some(&cached) = self.remote_viewer_workspace.get(key)
            && cached == ephemeral
            && (self.workspace_has_remote_viewer(key, cached, Some(window_id))
                || self.workspace_occupant_count(key, cached, Some(window_id)) == 0)
        {
            return cached;
        }
        ephemeral
    }

    fn workspace_occupant_count(&self, key: &str, ws: u32, exclude: Option<u32>) -> usize {
        self.windows
            .ids()
            .into_iter()
            .filter(|&id| {
                if exclude == Some(id) {
                    return false;
                }
                let Some(rec) = self.windows.get(id) else {
                    return false;
                };
                if self.windows.is_minimized(id) {
                    return false;
                }
                rec.output == key && rec.workspace == ws
            })
            .count()
    }

    pub(crate) fn workspace_has_remote_viewer(
        &self,
        key: &str,
        ws: u32,
        exclude: Option<u32>,
    ) -> bool {
        self.windows.ids().into_iter().any(|id| {
            if exclude == Some(id) {
                return false;
            }
            let Some(rec) = self.windows.get(id) else {
                return false;
            };
            rec.output == key
                && rec.workspace == ws
                && rec
                    .app_id
                    .as_deref()
                    .is_some_and(crate::decoration_policy::id_looks_freerdp_client)
        })
    }
}
