use crate::focus::KeyboardFocusTarget;
use crate::grabs::{MoveSurfaceGrab, ResizeSurfaceGrab};
use crate::state::{read_toplevel_decoration_mode, MetisState};
use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Space,
        Window, WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords,
        layer_map_for_output,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData as PointerGrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Resource,
            protocol::{wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Rectangle, Serial},
    wayland::{
        seat::WaylandFocus,
        shell::xdg::{
            decoration::XdgDecorationHandler,
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
    },
};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;

impl XdgShellHandler for MetisState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface.clone());
        self.register_new_window(window, "Application".into(), None);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        track_popup(self, surface);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        // A client-initiated move (e.g. dragging a GTK headerbar) floats the window
        // out of the grid so it follows the pointer with no snap-back.
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            // A maximized (or fullscreen) window is pinned — ignore drag requests
            // so its headerbar can't be used to move it around the screen. The
            // user must unmaximize first.
            if let Some(record) = self.windows.get(id)
                && (record.maximized || record.fullscreen)
            {
                return;
            }
            self.floating.insert(id);
        }

        let Some(seat) = Seat::from_resource(&seat) else {
            tracing::warn!("xdg move_request: seat resource has no Seat");
            return;
        };
        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let Some(pointer) = seat.get_pointer() else {
                tracing::warn!("xdg move_request: seat has no pointer");
                return;
            };
            let Some(window) = self
                .space
                .elements()
                .find(|w| w.wl_surface().is_some_and(|s| s.as_ref() == wl_surface))
                .cloned()
            else {
                tracing::warn!("xdg move_request: no mapped window for surface");
                return;
            };
            // Raise to the top before moving. Dragging a GTK headerbar issues this
            // client-side move request; without an explicit raise the window keeps
            // its old stacking position and slides *behind* whatever it's dragged
            // over (its titlebar disappears under the other app). The SSD-titlebar
            // and X11 move paths already raise — mirror them here.
            self.space.raise_element(&window, true);
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(window.clone().into()), serial);
            }
            if let Some(id) = self.windows.id_for_window(&window) {
                self.event_bus
                    .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
            }
            self.schedule_redraw();
            let Some(initial_window_location) = self.space.element_location(&window) else {
                tracing::warn!("xdg move_request: window has no space location");
                return;
            };
            let grab = MoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
                drag_active: true,
                pending_maximized_demote: false,
            };
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        if let Some(id) = self.window_id_for_toplevel(&surface)
            && self.is_window_grid_managed(id)
        {
            return;
        }

        let Some(seat) = Seat::from_resource(&seat) else {
            tracing::warn!("xdg resize_request: seat resource has no Seat");
            return;
        };
        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let Some(pointer) = seat.get_pointer() else {
                tracing::warn!("xdg resize_request: seat has no pointer");
                return;
            };
            let Some(window) = self
                .space
                .elements()
                .find(|w| w.wl_surface().is_some_and(|s| s.as_ref() == wl_surface))
                .cloned()
            else {
                tracing::warn!("xdg resize_request: no mapped window for surface");
                return;
            };
            let Some(initial_window_location) = self.space.element_location(&window) else {
                tracing::warn!("xdg resize_request: window has no space location");
                return;
            };
            let initial_window_size = window.geometry().size;

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });
            surface.send_pending_configure();

            let grab = ResizeSurfaceGrab::start(
                start_data,
                window,
                edges.into(),
                Rectangle::new(initial_window_location, initial_window_size),
            );
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        // Honor xdg_popup grabs so GTK popovers that need keyboard/pointer focus
        // (text entries, dropdowns) can present and dismiss correctly. The root of
        // the grab is either an app window or one of our layer surfaces (the bar).
        let Some(seat) = Seat::<MetisState>::from_resource(&seat) else {
            tracing::warn!("xdg popup grab: seat resource has no Seat");
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Some(root) = find_popup_root_surface(&kind).ok().and_then(|root| {
            self.space
                .elements()
                .find(|w| {
                    w.toplevel()
                        .map(|t| t.wl_surface() == &root)
                        .unwrap_or(false)
                })
                .cloned()
                .map(KeyboardFocusTarget::from)
                .or_else(|| {
                    self.space.outputs().find_map(|o| {
                        let map = layer_map_for_output(o);
                        map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                            .cloned()
                            .map(KeyboardFocusTarget::LayerSurface)
                    })
                })
        }) else {
            return;
        };

        // Bar popovers are non-autohide and dismissed via toggle + the compositor
        // "close-popovers" signal. `xdg_popup` grabs rooted on a layer-shell
        // surface (KeyboardMode::None) deadlock GTK in a nested session — the menu
        // would freeze the whole Metis session while trying to establish the grab.
        if matches!(root, KeyboardFocusTarget::LayerSurface(_)) {
            return;
        }

        if let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) {
            if let Some(keyboard) = seat.get_keyboard() {
                if keyboard.is_grabbed()
                    && !(keyboard.has_grab(serial)
                        || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                {
                    grab.ungrab(PopupUngrabStrategy::All);
                    return;
                }
                keyboard.set_focus(self, grab.current_grab(), serial);
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            }
            if let Some(pointer) = seat.get_pointer() {
                if pointer.is_grabbed()
                    && !(pointer.has_grab(serial)
                        || pointer
                            .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                {
                    grab.ungrab(PopupUngrabStrategy::All);
                    return;
                }
                pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
            }
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            self.set_maximized(id, true);
        } else if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            self.set_maximized(id, false);
        } else if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            tracing::info!(
                id,
                "client requested fullscreen (xdg_toplevel.set_fullscreen)"
            );
            self.set_fullscreen(id, true, output);
        } else if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            tracing::info!(
                id,
                "client requested unfullscreen (xdg_toplevel.unset_fullscreen)"
            );
            self.set_fullscreen(id, false, None);
        } else if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_id_for_toplevel(&surface) {
            if let Some(tile_id) = self.tile_id_for_window(id) {
                self.set_tile_mode(&tile_id, metis_protocol::TileMode::Minimized);
            } else {
                self.minimize_window(id);
            }
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(id) = self.window_id_for_toplevel(&surface) else {
            return;
        };
        let ready = self.windows.is_ready(id);
        // Release any hold this window had on the edge bar *unconditionally* —
        // even a not-yet-`ready` fullscreen window (a game that fullscreened then
        // exited before its first activation) must restore the bar, otherwise it
        // stays hidden until the shell is killed. `on_window_destroyed` also calls
        // this, but only when `ready`; do it up front so the gate can't strand the
        // bar.
        self.drop_window_fullscreen(id);
        // Remember floating app geometry before the record is dropped.
        self.save_window_geometry(id);
        if let Some(record) = self.windows.unregister(id) {
            self.space.unmap_elem(&record.window);
        }
        if ready {
            self.on_window_destroyed(id);
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.sync_toplevel_metadata(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.sync_toplevel_metadata(&surface);
    }
}

impl XdgDecorationHandler for MetisState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        if let Some(id) = self.window_id_for_toplevel(&toplevel) {
            self.windows.set_decoration_bound(id, true);
            self.refresh_window_decoration_mode(id);
        }
        // Orphan decoration objects (toplevel not tracked yet) get their mode from
        // `refresh_window_decoration_mode` once the window is registered — do not
        // pre-grant ServerSide here or Chromium draws a lone close button.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        self.apply_xdg_decoration_request(toplevel, Some(mode));
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.apply_xdg_decoration_request(toplevel, Some(DecorationMode::ClientSide));
    }
}

fn check_grab(
    seat: &Seat<MetisState>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<MetisState>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

pub fn track_popup(state: &mut MetisState, surface: PopupSurface) {
    state.unconstrain_popup(&surface);
    if let Err(err) = state.popups.track_popup(PopupKind::Xdg(surface)) {
        tracing::warn!(%err, "failed to track popup");
    }
}

pub fn handle_commit(popups: &mut PopupManager, _space: &Space<Window>, surface: &WlSurface) {
    // Toplevel initial configure is sent from `compositor::commit` via
    // `ensure_initial_configure` so the first event includes `decoration_mode`
    // and placement size. An early bare `send_configure` here made Chromium
    // latch server-side mode (close button only) before app_id was known.
    popups.commit(surface);
    if let Some(PopupKind::Xdg(ref xdg)) = popups.find_popup(surface)
        && !xdg.is_initial_configure_sent()
    {
        let _ = xdg.send_configure();
    }
}

impl MetisState {
    fn apply_xdg_decoration_request(
        &mut self,
        toplevel: ToplevelSurface,
        requested: Option<DecorationMode>,
    ) {
        let Some(id) = self.window_id_for_toplevel(&toplevel) else {
            let mode = requested.unwrap_or(DecorationMode::ClientSide);
            toplevel.with_pending_state(|state| {
                state.decoration_mode = Some(mode);
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
            return;
        };

        self.windows.set_decoration_negotiated(id, true);
        self.windows.set_decoration_bound(id, true);
        let app_id = self.windows.get(id).and_then(|r| r.app_id.clone());
        let negotiated_mode = requested.or_else(|| read_toplevel_decoration_mode(&toplevel));
        let user_override = self.decoration_overrides.user_override(app_id.as_deref());
        let mut uses_ssd = crate::decoration_policy::resolve_uses_ssd(
            app_id.as_deref(),
            negotiated_mode,
            true,
            user_override,
        );
        if user_override.is_none()
            && !uses_ssd
            && self.chromium_window_needs_ssd(id, app_id.as_deref())
        {
            uses_ssd = true;
        }
        let granted = crate::decoration_policy::grant_decoration_mode(uses_ssd);
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(granted);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }

        let prev = self.windows.uses_ssd(id);
        let was_draw = self.should_draw_metis_ssd(id);
        self.windows.set_uses_ssd(id, uses_ssd);
        if prev != uses_ssd {
            tracing::info!(
                id,
                uses_ssd,
                ?app_id,
                ?requested,
                "xdg-decoration negotiated"
            );
            if uses_ssd {
                self.clear_auto_hide(id);
            }
        }
        let now_draw = self.should_draw_metis_ssd(id);
        if prev != uses_ssd || was_draw != now_draw {
            self.apply_window_rect(id);
            self.schedule_redraw();
        }
    }

    fn sync_toplevel_metadata(&mut self, surface: &ToplevelSurface) {
        let Some(id) = self.window_id_for_toplevel(surface) else {
            return;
        };
        let (title, app_id) = crate::state::read_toplevel_metadata(surface);
        self.windows.set_metadata(id, title.clone(), app_id.clone());
        self.set_app_tile_display_name(id, &title, app_id.as_deref());
        self.refresh_window_decoration_mode(id);
        if !self.maybe_register_capture_overlay(id) {
            // GTK frequently sets app_id just after its first buffer commit, so the
            // initial activation can miss it. Now that it's known, place the window
            // (center default-floating apps / restore saved geometry) if it hasn't
            // already been positioned.
            self.maybe_autoplace_window(id);
        }
        if self.windows.is_ready(id) {
            use metis_protocol::CompositorEvent;
            self.event_bus
                .emit(&CompositorEvent::WindowMetadata { id, title, app_id });
        }
    }

    pub(crate) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };

        // Keep popovers from sliding flush against (or off) the screen edge by
        // shrinking the allowed placement area on every side.
        const SCREEN_MARGIN: i32 = 10;
        let inset = |mut r: Rectangle<i32, smithay::utils::Logical>| {
            r.loc.x += SCREEN_MARGIN;
            r.loc.y += SCREEN_MARGIN;
            r.size.w = (r.size.w - 2 * SCREEN_MARGIN).max(1);
            r.size.h = (r.size.h - 2 * SCREEN_MARGIN).max(1);
            r
        };

        if let Some(window) = self
            .space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == root))
        {
            let Some(window_geo) = self.space.element_geometry(window) else {
                tracing::warn!("xdg popup: window has no geometry");
                return;
            };
            // Constrain to the output the window actually sits on, not always the
            // primary — otherwise toplevel popups on a secondary output get clamped
            // to the wrong monitor's area. Both `output_geo` and `window_geo` are
            // global, so the global origins cancel below.
            let Some(output) = self
                .space
                .outputs()
                .find(|o| {
                    self.space
                        .output_geometry(o)
                        .is_some_and(|g| g.overlaps(window_geo))
                })
                .or_else(|| self.space.outputs().next())
            else {
                tracing::warn!("xdg popup: no output for window popup constraint");
                return;
            };
            let Some(output_geo) = self.space.output_geometry(output) else {
                tracing::warn!("xdg popup: output has no geometry");
                return;
            };

            let mut target = inset(output_geo);
            target.loc -= get_popup_toplevel_coords(&kind);
            target.loc -= window_geo.loc;

            popup.with_pending_state(|state| {
                state.geometry = state.positioner.get_unconstrained_geometry(target);
            });
            return;
        }

        let Some((output, layer_geo)) = self.layer_geometry_for_surface(&root) else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            tracing::warn!("xdg popup: layer output has no geometry");
            return;
        };
        // `get_unconstrained_geometry` expects `target` in the parent layer
        // surface's local frame. `layer_geo.loc` is output-local while `output_geo`
        // is global, so subtract BOTH the output origin and the layer offset —
        // without the output origin, popovers on a non-primary output are pushed
        // off-screen by that output's global X (and never appear / take input).
        let mut target = inset(output_geo);
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= layer_geo.loc;
        target.loc -= output_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    fn layer_geometry_for_surface(
        &self,
        surface: &WlSurface,
    ) -> Option<(
        smithay::output::Output,
        smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    )> {
        self.space.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)?;
            let geo = map.layer_geometry(layer)?;
            Some((output.clone(), geo))
        })
    }
}
