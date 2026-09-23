use crate::state::MetisState;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
};

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<MetisState>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
    /// Wait for pointer movement before unmaximizing / starting a drag.
    pub drag_active: bool,
    /// Set when the grab began on a maximized window — demote only after drag starts.
    pub pending_maximized_demote: bool,
}

impl PointerGrab<MetisState> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;

        if !self.drag_active {
            if delta.x * delta.x + delta.y * delta.y < 25.0 {
                return;
            }
            self.drag_active = true;
            if self.pending_maximized_demote {
                if let Some(id) = data.windows.id_for_window(&self.window) {
                    data.unmaximize_for_titlebar_drag(id);
                    if let Some(loc) = data.space.element_location(&self.window) {
                        self.initial_window_location = loc;
                    }
                }
                self.pending_maximized_demote = false;
            }
        }

        let mut new_location = self.initial_window_location.to_f64() + delta;

        // Clamp to the whole virtual desktop (all outputs) so a window can be
        // dragged across monitors while staying at least `keep` px on-screen. Only
        // the top/bottom edge bar (exclusive zone) hard-limits the vertical range;
        // left/right bars overlay the desktop and windows slide under them.
        {
            let bounds = data.desktop_bounds();
            let zone = metis_grid::PixelRect {
                x: bounds.loc.x,
                y: bounds.loc.y,
                width: bounds.size.w,
                height: bounds.size.h,
            };
            let size = self.window.geometry().size;
            let gaps = data.zone_edge_gaps();
            let keep = crate::state::MIN_VISIBLE_PX as f64;
            let header = if data
                .windows
                .id_for_window(&self.window)
                .is_some_and(|id| data.window_uses_ssd(id))
            {
                metis_grid::APP_TILE_HEADER_PX as f64
            } else {
                0.0
            };
            let pos = metis_config::load_bar_config().position;

            let (min_y, max_y) = match pos {
                metis_config::BarPosition::Top => (
                    zone.y as f64 + gaps.top as f64 + header,
                    (zone.y + zone.height) as f64 - keep,
                ),
                _ => (
                    zone.y as f64 + keep - size.h as f64,
                    (zone.y + zone.height) as f64 - keep,
                ),
            };

            let (min_x, max_x) = (
                zone.x as f64 + keep - size.w as f64,
                (zone.x + zone.width) as f64 - keep,
            );

            new_location.x = new_location.x.clamp(min_x, max_x.max(min_x));
            new_location.y = new_location.y.clamp(min_y, max_y.max(min_y));
        }

        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);

        if let Some(id) = data.windows.id_for_window(&self.window) {
            let loc = new_location.to_i32_round();
            let geo = self.window.geometry();
            data.windows.set_target_rect(
                id,
                metis_grid::PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: geo.size.w,
                    height: geo.size.h,
                },
            );
        }

        // Live snap-zone preview follows the pointer (not the window), so the
        // overlay highlights the destination as the cursor approaches an edge.
        let p = event.location;
        data.snap_preview = data.snap_target_at(p.x.round() as i32, p.y.round() as i32);
        data.damaged = true;
    }

    fn relative_motion(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        const BTN_LEFT: u32 = 0x110;
        if !handle.current_pressed().contains(&BTN_LEFT) {
            // Capture the snap target BEFORE unset_grab: unset_grab() invokes our
            // `unset()`, which clears `snap_preview`, so reading it afterwards
            // would always be `None` (the window would never actually snap).
            let snap = data.snap_preview.take();
            handle.unset_grab(self, data, event.serial, event.time, true);
            if let Some(id) = data.windows.id_for_window(&self.window) {
                if !self.drag_active {
                    // Click without drag — leave maximized windows maximized.
                } else if let Some((rect, label)) = snap {
                    // `apply_snap` already pushes the snapped geometry to the client
                    // (X11 included) via `send_window_configure`; don't re-sync below
                    // or we'd clobber the snapped size with the pre-snap one.
                    data.apply_snap(id, rect, label);
                } else if self.drag_active {
                    // Drop on a different monitor: re-home the window's desk tile
                    // before snapping a grid window back to its tile.
                    data.maybe_adopt_window_output(id);
                    if data.is_window_grid_managed(id) {
                        data.enforce_grid_window_geometry(id);
                    } else {
                        data.save_window_geometry(id);
                    }
                    // XWayland only: sync the X server to the final on-screen
                    // position so the client reports correct root coordinates
                    // (popup/menu placement). configure_notify for managed windows
                    // is ignored, so this can't loop.
                    if let Some(x11) = self.window.x11_surface()
                        && let Some(loc) = data.space.element_location(&self.window)
                    {
                        let _ = x11.configure(Rectangle::new(loc, self.window.geometry().size));
                    }
                }
            }
            data.damaged = true;
        }
    }

    fn axis(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut MetisState, handle: &mut PointerInnerHandle<'_, MetisState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut MetisState,
        handle: &mut PointerInnerHandle<'_, MetisState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<MetisState> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut MetisState) {
        // Clear any lingering preview if the grab ends without a normal release
        // (e.g. the window closed mid-drag), so the overlay never sticks.
        if data.snap_preview.take().is_some() {
            data.damaged = true;
        }
    }
}
