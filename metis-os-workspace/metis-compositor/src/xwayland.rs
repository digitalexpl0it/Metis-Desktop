//! XWayland integration: spawns the `Xwayland` server, runs an X11 window
//! manager, and maps X11 toplevels/override-redirect surfaces into the same
//! `Space<Window>` used for native Wayland clients.
//!
//! Scope note: X11 windows are treated as floating, centered surfaces. They are
//! intentionally kept out of Metis's tiling grid and window registry (which are
//! built around `xdg_toplevel`), so they are not snapped, decorated with Metis
//! server-side titlebars, or tracked for IPC. This is enough to run X11-only and
//! D-Bus-activated apps inside a nested session; richer integration can come
//! later.

use std::os::unix::io::OwnedFd;

use smithay::{
    desktop::Window,
    input::pointer::{Focus, GrabStartData as PointerGrabStartData},
    reexports::calloop::LoopHandle,
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::{
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
    },
};

use crate::clipboard::serve_compositor_selection;
use crate::focus::KeyboardFocusTarget;
use crate::state::MetisState;

impl MetisState {
    /// Spawn the XWayland server and, once it is ready, start the X11 window
    /// manager. Best-effort: if `Xwayland` is missing or fails to start we log a
    /// warning and continue with a Wayland-only session.
    ///
    /// Starts the primary XWayland server. With `"xwayland_mode": "isolated"`,
    /// the gaming bucket is **lazy-spawned** on first gaming-class launch
    /// (Phase 18 D) — not at session start.
    pub fn start_xwayland(&mut self, loop_handle: LoopHandle<'static, MetisState>) {
        let cfg = metis_config::load_app_config();
        let open_abstract = cfg.xwayland_abstract_socket;
        tracing::info!(
            open_abstract_socket = open_abstract,
            ?cfg.xwayland_mode,
            "starting XWayland"
        );
        if cfg.xwayland_mode == metis_config::XwaylandMode::Isolated {
            tracing::info!(
                "xwayland_mode=isolated — gaming XWayland will start on first gaming launch"
            );
        }

        self.spawn_one_xwayland(loop_handle, open_abstract, false);
    }

    /// Ensure the gaming XWayland bucket exists (isolated mode only). Idempotent.
    pub(crate) fn ensure_gaming_xwayland(&mut self) {
        use metis_config::XwaylandMode;

        if self.xdisplay_gaming.is_some() || self.xwayland_gaming_spawn_pending {
            return;
        }
        let cfg = metis_config::load_app_config();
        if cfg.xwayland_mode != XwaylandMode::Isolated {
            return;
        }
        self.xwayland_gaming_spawn_pending = true;
        tracing::info!("lazy-starting gaming XWayland bucket");
        self.spawn_one_xwayland(self.loop_handle.clone(), cfg.xwayland_abstract_socket, true);
    }

    pub(crate) fn spawn_one_xwayland(
        &mut self,
        loop_handle: LoopHandle<'static, MetisState>,
        open_abstract: bool,
        gaming_bucket: bool,
    ) {
        use std::process::Stdio;

        let (xwayland, client) = match XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            open_abstract,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        ) {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    %err,
                    gaming_bucket,
                    "could not spawn XWayland — X11 apps will be unavailable"
                );
                return;
            }
        };

        let dh = self.display_handle.clone();
        let wm_handle = loop_handle.clone();
        let inserted = loop_handle.insert_source(xwayland, move |event, _, state| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => {
                match X11Wm::start_wm(wm_handle.clone(), &dh, x11_socket, client.clone()) {
                    Ok(wm) => {
                        let id = wm.id();
                        if gaming_bucket {
                            state.xwm_gaming_id = Some(id);
                            state.xwm_gaming = Some(wm);
                            state.xdisplay_gaming = Some(display_number);
                            state.xwayland_gaming_spawn_pending = false;
                            tracing::info!(
                                display = display_number,
                                "gaming XWayland ready (isolated bucket)"
                            );
                            state.flush_pending_gaming_launches();
                        } else {
                            state.xwm = Some(wm);
                            state.xdisplay = Some(display_number);
                            // Default DISPLAY for non-gaming spawns.
                            unsafe {
                                std::env::set_var("DISPLAY", format!(":{display_number}"));
                            }
                            tracing::info!(
                                display = display_number,
                                "XWayland ready — X11 apps supported"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::error!(%err, gaming_bucket, "failed to start the X11 window manager");
                        if gaming_bucket {
                            state.xwayland_gaming_spawn_pending = false;
                            state.drop_pending_gaming_launches("X11 WM failed");
                        }
                    }
                }
            }
            XWaylandEvent::Error => {
                tracing::warn!(gaming_bucket, "XWayland crashed during startup");
                if gaming_bucket {
                    state.xwayland_gaming_spawn_pending = false;
                    state.drop_pending_gaming_launches("XWayland startup error");
                }
            }
        });

        if let Err(err) = inserted {
            tracing::error!(%err, gaming_bucket, "failed to insert XWayland source into the event loop");
        }
    }

    /// Find the `Space` element backing a given X11 surface, if it is mapped.
    fn x11_element(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.x11_surface() == Some(surface))
            .cloned()
    }

    /// Center a window of `size` within the primary output's logical area,
    /// falling back to the configured monitor rect when no output exists yet.
    fn centered_loc(&self, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let area = self
            .space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
            .unwrap_or_else(|| {
                Rectangle::new(
                    (self.monitor.x, self.monitor.y).into(),
                    (self.monitor.width, self.monitor.height).into(),
                )
            });
        let x = area.loc.x + (area.size.w - size.w).max(0) / 2;
        let y = area.loc.y + (area.size.h - size.h).max(0) / 2;
        (x, y).into()
    }

    fn focus_x11(&mut self, window: &Window) {
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Some(KeyboardFocusTarget::Window(window.clone())),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    fn x11_window_id(window: &X11Surface) -> u32 {
        window.window_id()
    }

    fn output_for_x11_element(&self, elem: &Window) -> Option<smithay::output::Output> {
        self.space
            .outputs_for_element(elem)
            .first()
            .cloned()
            .or_else(|| self.primary_output())
            .or_else(|| self.space.outputs().next().cloned())
    }

    pub(crate) fn apply_x11_fullscreen(&mut self, window: X11Surface) {
        let Some(elem) = self.x11_element(&window) else {
            tracing::debug!("X11 fullscreen request before map");
            return;
        };
        let wid = Self::x11_window_id(&window);
        let restore = self
            .space
            .element_bbox(&elem)
            .or_else(|| Some(window.geometry()));
        if let Some(restore) = restore {
            self.x11_fullscreen_restore.insert(wid, restore);
        }

        let Some(output) = self.output_for_x11_element(&elem) else {
            tracing::warn!("X11 fullscreen: no output available");
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };

        if let Err(err) = window.set_fullscreen(true) {
            tracing::warn!(%err, "X11 set_fullscreen failed");
        }
        if let Err(err) = window.configure(geo) {
            tracing::warn!(%err, "X11 fullscreen configure failed");
            return;
        }
        // Diagnostics for the "fullscreen offset a few pixels" report: the window
        // is mapped so its *visible geometry* origin lands at `geo.loc`; the
        // buffer (bbox) may extend into negative coords by the client-side frame
        // extents (CSD shadow). If the visible content still appears shifted, the
        // deltas below reveal whether the client mis-reports its frame extents.
        tracing::debug!(
            x11_window = window.window_id(),
            output = %output.name(),
            ?geo,
            win_geometry = ?window.geometry(),
            win_bbox = ?window.bbox(),
            "x11: apply fullscreen (map at geo.loc; render offsets by -geometry.loc)"
        );
        self.space.map_element(elem.clone(), geo.loc, true);
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.note_output_fullscreen(&output, id, true);
        }
        self.focus_x11(&elem);
        self.schedule_redraw();
    }

    pub(crate) fn apply_x11_unfullscreen(&mut self, window: X11Surface) {
        let Some(elem) = self.x11_element(&window) else {
            return;
        };
        let wid = Self::x11_window_id(&window);
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.drop_window_fullscreen(id);
        }
        if let Err(err) = window.set_fullscreen(false) {
            tracing::warn!(%err, "X11 unset fullscreen failed");
        }
        let restore = self.x11_fullscreen_restore.remove(&wid).unwrap_or_else(|| {
            let size = window.geometry().size;
            Rectangle::new(self.centered_loc(size), size)
        });
        if let Err(err) = window.configure(restore) {
            tracing::warn!(%err, "X11 unfullscreen configure failed");
            return;
        }
        self.space.map_element(elem.clone(), restore.loc, true);
        self.focus_x11(&elem);
        self.schedule_redraw();
    }
}

impl XWaylandShellHandler for MetisState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for MetisState {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
        if self.xwm_gaming_id == Some(xwm) {
            return self
                .xwm_gaming
                .as_mut()
                .expect("gaming xwm id set without X11Wm");
        }
        self.xwm
            .as_mut()
            .expect("xwm called without a running X11Wm")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // Full Metis management (registry, SSD titlebar, bar-aware floating
        // placement, dock/IPC) lives in `map_x11_toplevel`.
        self.map_x11_toplevel(window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Menus / tooltips / combo dropdowns. These position themselves in X11
        // root coordinates, which Metis keeps in sync with its logical Space (see
        // `send_window_configure`). Use the surface's *configured* rectangle for
        // the location: `X11Surface::geometry()`/`bbox()` are size-only (their loc
        // is always the origin), so reading `geometry().loc` here would slam every
        // popup to the top-left corner. `last_configure()` carries the real
        // root-relative position the X server assigned. Raise so the popup sits
        // above its parent toplevel.
        let loc = window.last_configure().loc;
        tracing::debug!(
            x11_window = window.window_id(),
            ?loc,
            size = ?window.geometry().size,
            "x11: map override-redirect popup"
        );
        let elem = Window::new_x11_window(window);
        self.space.map_element(elem, loc, true);
        self.schedule_redraw();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.unmap_x11_toplevel(&window);
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
        self.schedule_redraw();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_fullscreen_restore
            .remove(&MetisState::x11_window_id(&window));
        self.destroy_x11_toplevel(&window);
        self.schedule_redraw();
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // Route through the shared path so the registry fullscreen flag + bar
        // visibility stay in sync; it delegates to `apply_x11_fullscreen`.
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.set_fullscreen(id, true, None);
        } else {
            self.apply_x11_fullscreen(window);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.set_fullscreen(id, false, None);
        } else {
            self.apply_x11_unfullscreen(window);
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // Steam's window controls (and many X11 apps) maximize via
        // `_NET_WM_STATE_MAXIMIZED_*`; route it through the shared path so the
        // registry flag, geometry, and bar visibility stay in sync.
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.set_maximized(id, true);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.set_maximized(id, false);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.minimize_by_id(id);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            self.restore_by_id(id);
        }
    }

    fn active_window_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _timestamp: u32,
        _currently_active_window: Option<X11Surface>,
    ) {
        // `_NET_ACTIVE_WINDOW` — a client (Steam raising its main window from the
        // tray, a launcher handing off to a game) asks to be brought forward.
        // Restore-if-minimized + raise + focus through the shared activation path.
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            return;
        };
        // Focus-stealing prevention: while a *running game* holds focus, ignore a
        // *different* window's self-activation. Steam fires `_NET_ACTIVE_WINDOW`
        // at its own main window (tray updates, friends/notifications, download
        // finished) mid-game, which would otherwise pop Steam over the game and
        // strip the game's keyboard focus + pointer lock — the reported
        // "Steam comes to the foreground" + "Esc/keys stop working" during play.
        // A game activating itself (focused == id) or a launcher handing off to a
        // freshly-mapped game (the game is the requester, not the focused window)
        // is still honored, so this only blocks background launchers stealing from
        // an active game. User-initiated raises (dock/taskbar) go through
        // `activate_window_by_id` directly and are unaffected.
        if let Some(focused) = self.focused_window_id() {
            if focused != id && self.window_is_running_game(focused) {
                tracing::info!(
                    requester = id,
                    focused,
                    x11_window = window.window_id(),
                    "x11: blocked _NET_ACTIVE_WINDOW focus-steal while a game is focused"
                );
                return;
            }
        }
        self.activate_window_by_id(id);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Diagnostic: reveals whether a client (Steam) tries to *self-move* via
        // ConfigureRequest (client-supplied x/y) vs. `_NET_WM_MOVERESIZE`. Managed
        // toplevels are Metis-positioned, so a self-move here is otherwise silently
        // dropped — logging the requested x/y makes the distinction unambiguous.
        if x.is_some() || y.is_some() {
            tracing::debug!(
                x11_window = window.window_id(),
                req_x = ?x,
                req_y = ?y,
                req_w = ?w,
                req_h = ?h,
                override_redirect = window.is_override_redirect(),
                "x11: configure_request with position (client self-move attempt)"
            );
        }
        if window.is_fullscreen() {
            if let Some(elem) = self.x11_element(&window) {
                if let Some(output) = self.output_for_x11_element(&elem) {
                    if let Some(geo) = self.space.output_geometry(&output) {
                        let _ = window.configure(geo);
                    }
                }
            }
            return;
        }
        // Honor client size requests, but keep placement under our control.
        let mut geo = window.geometry();
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        // `X11Surface::geometry().loc` is always ~(0,0) (it is size-only), so
        // configuring with it teleports the window's X-server *root* position to
        // the top-left on every client-driven resize. That desyncs the X root
        // frame from the Metis Space location, and since override-redirect popups
        // (Steam/CEF dropdowns, tooltips, combo menus) are positioned by the
        // client in root coordinates, they then map into the top-left corner
        // instead of under their anchor. Anchor the configure at the element's
        // actual Space position so root coords stay in lockstep with what we
        // render — this is what keeps menus under the thing that opened them.
        if let Some(elem) = self.x11_element(&window) {
            if let Some(loc) = self.space.element_location(&elem) {
                geo.loc = loc;
            }
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        let Some(elem) = self.x11_element(&window) else {
            return;
        };
        // Metis owns placement for managed (registered) toplevels: rendering
        // follows the compositor-side element position, not the X server's. Many
        // X11 clients (Chromium/Electron) map at (0,0) and re-assert their own
        // position, so blindly following `geometry.loc` here would repeatedly drag
        // the window into the top-left corner under the edge bar — both on first
        // map and right after an interactive move. Ignore client-driven position;
        // only unmanaged / override-redirect surfaces track their own geometry.
        if self.windows.id_for_x11_window(window.window_id()).is_some() {
            self.schedule_redraw();
            return;
        }
        // Unmanaged / override-redirect surface (menu, tooltip, dropdown) moved
        // itself. Track the new position and keep it raised — a popup must never
        // drop behind its parent toplevel when it repositions after mapping.
        let raise = window.is_override_redirect();
        if raise {
            tracing::debug!(
                x11_window = window.window_id(),
                loc = ?geometry.loc,
                "x11: override-redirect popup reposition"
            );
        }
        self.space.map_element(elem, geometry.loc, raise);
        self.schedule_redraw();
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _edges: X11ResizeEdge,
    ) {
        // Interactive resize for X11 windows is not wired into Metis's grab
        // machinery yet; the window keeps its current size.
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        // Self-decorated X11 clients (Steam, game launchers) have no Metis
        // titlebar to grab, so dragging their own titlebar issues
        // `_NET_WM_MOVERESIZE`. Start the same interactive move grab used for
        // Wayland toplevels — it already syncs the X server position on release so
        // popup/menu placement stays correct.
        tracing::info!(
            x11_window = window.window_id(),
            "x11: move_request (_NET_WM_MOVERESIZE) — starting interactive move grab"
        );
        let Some(elem) = self.x11_element(&window) else {
            tracing::warn!(
                x11_window = window.window_id(),
                "x11: move_request for unmapped window — ignored"
            );
            return;
        };
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            if let Some(record) = self.windows.get(id) {
                if record.maximized || record.fullscreen {
                    return;
                }
            }
            // Float it so the drag has no snap-back to a grid tile.
            self.floating.insert(id);
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&elem) else {
            return;
        };
        self.space.raise_element(&elem, true);
        self.focus_x11(&elem);
        let start_data = PointerGrabStartData {
            focus: None,
            button: 0x110,
            location: pointer.current_location(),
        };
        let grab = crate::grabs::MoveSurfaceGrab {
            start_data,
            window: elem,
            initial_window_location,
            drag_active: true,
            pending_maximized_demote: false,
        };
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
        self.schedule_redraw();
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        if let Some(keyboard) = self.seat.get_keyboard() {
            if let Some(KeyboardFocusTarget::Window(w)) = keyboard.current_focus() {
                if let Some(surface) = w.x11_surface() {
                    if surface.xwm_id() == Some(xwm) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                let mut fd = fd;
                if let Some(user_data) = current_data_device_selection_userdata(&self.seat) {
                    match serve_compositor_selection(&user_data, &mime_type, fd) {
                        Ok(()) => return,
                        Err(returned_fd) => {
                            if user_data.has_payload() {
                                tracing::warn!(
                                    %mime_type,
                                    "recalled clipboard: unsupported mime for XWayland paste"
                                );
                                return;
                            }
                            fd = returned_fd;
                        }
                    }
                }
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(?err, "failed to read Wayland clipboard for XWayland");
                }
            }
            SelectionTarget::Primary => {
                let mut fd = fd;
                if let Some(user_data) = current_primary_selection_userdata(&self.seat) {
                    match serve_compositor_selection(&user_data, &mime_type, fd) {
                        Ok(()) => return,
                        Err(returned_fd) => {
                            if user_data.has_payload() {
                                tracing::warn!(
                                    %mime_type,
                                    "recalled primary: unsupported mime for XWayland paste"
                                );
                                return;
                            }
                            fd = returned_fd;
                        }
                    }
                }
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(
                        ?err,
                        "failed to read Wayland primary selection for XWayland"
                    );
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types,
                    crate::clipboard::MetisSelectionUserData::default(),
                );
            }
            SelectionTarget::Primary => {
                set_primary_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types,
                    crate::clipboard::MetisSelectionUserData::default(),
                );
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat);
                }
            }
        }
    }
}
