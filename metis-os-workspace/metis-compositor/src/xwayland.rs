//! XWayland integration: spawns the `Xwayland` server, runs an X11 window
//! manager, and maps X11 toplevels/override-redirect surfaces into the same
//! `Space<Window>` used for native Wayland clients.
//!
//! Mapped X11 toplevels are registered in the shared window registry (floating,
//! Metis SSD when decorated), announced over IPC for the dock, and placed with
//! bar-aware geometry. They do **not** take tiling-grid tiles. Override-redirect
//! surfaces (menus/tooltips) stay untracked. Heuristics for borderless /
//! near-fullscreen games live in this module alongside map/unmap/destroy.

use std::os::unix::io::OwnedFd;

use metis_grid::PixelRect;
use smithay::{
    desktop::Window,
    input::pointer::{Focus, GrabStartData as PointerGrabStartData},
    reexports::calloop::LoopHandle,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::{
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
    },
};

use crate::clipboard::serve_compositor_selection;
use crate::focus::KeyboardFocusTarget;
use crate::state::MetisState;

/// Near-monitor size that should become true fullscreen (flush to output origin).
pub(crate) fn x11_borderless_fullscreen_intent(
    output: Rectangle<i32, Logical>,
    w: i32,
    h: i32,
    undecorated: bool,
) -> bool {
    if w <= 0 || h <= 0 || output.size.w <= 0 || output.size.h <= 0 {
        return false;
    }
    if w >= output.size.w && h >= output.size.h {
        return true;
    }
    let ow = output.size.w as f32;
    let oh = output.size.h as f32;
    let fw = w as f32 / ow;
    let fh = h as f32 / oh;
    if undecorated {
        // Borderless games: ~85%+ of the panel, or one axis fills with the other
        // still substantial (letterboxed / ultrawide).
        (fw >= 0.85 && fh >= 0.85) || (fw >= 0.95 && fh >= 0.50) || (fh >= 0.95 && fw >= 0.50)
    } else {
        fw >= 0.97 && fh >= 0.97
    }
}

/// Large enough undecorated surface that a stale top-left would clip off-screen
/// when the client grows — re-center on the output (keep client size).
pub(crate) fn x11_large_undecorated_float(output: Rectangle<i32, Logical>, w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 || output.size.w <= 0 || output.size.h <= 0 {
        return false;
    }
    let fw = w as f32 / output.size.w as f32;
    let fh = h as f32 / output.size.h as f32;
    fw >= 0.45 && fh >= 0.45
}

/// Steam / Lutris / Heroic splash & main windows — must NOT get game borderless
/// auto-fullscreen or resize-loop re-centering (they animate size while loading).
pub(crate) fn x11_is_game_launcher(app_id: Option<&str>) -> bool {
    let Some(raw) = app_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    let id = raw.to_ascii_lowercase();
    // Actual games — never treat as the store/launcher.
    if id.starts_with("steam_app_")
        || id.contains(".exe")
        || id.contains("proton")
        || id == "hytaleclient"
    {
        return false;
    }
    id == "steam"
        || id.starts_with("steam.")
        || id.contains("steamwebhelper")
        || id.contains("gamepadui")
        || id.contains("lutris")
        || id.contains("heroic")
        || id.contains("bottles")
        || id.contains("com.valvesoftware.steam")
}

impl MetisState {
    /// Bring an XWayland toplevel under full Metis management: register it in the
    /// shared window registry, give it a Metis server-side titlebar, place it as a
    /// bar-aware floating window, and announce it to the shell (dock/IPC). X11
    /// windows do not participate in the tiling grid — they are always floating.
    pub(crate) fn map_x11_toplevel(&mut self, window: X11Surface) {
        use metis_protocol::CompositorEvent;

        let is_remap = self.windows.id_for_x11_window(window.window_id()).is_some();
        tracing::info!(
            x11_window = window.window_id(),
            override_redirect = window.is_override_redirect(),
            class = %window.class(),
            title = %window.title(),
            remap = is_remap,
            "x11: map request"
        );

        if window.is_override_redirect() {
            // Menus / tooltips / drag surfaces: map at their requested location and
            // leave them undecorated and untracked. `geometry()`/`bbox()` are
            // size-only for X11 (loc is always the origin), so use the configured
            // rectangle's root-relative location instead.
            let loc = window.last_configure().loc;
            let elem = Window::new_x11_window(window);
            self.space.map_element(elem, loc, true);
            self.schedule_redraw();
            return;
        }

        if let Err(err) = window.set_mapped(true) {
            tracing::warn!(%err, "failed to map X11 window");
            return;
        }

        // A remap of an already-tracked window (e.g. an Electron app restoring from
        // its tray) re-applies geometry and re-announces to the shell — the withdraw
        // in `unmap_x11_toplevel` dropped the dock entry, so we re-emit WindowOpened.
        if let Some(existing) = self.windows.id_for_x11_window(window.window_id()) {
            self.windows.set_minimized(existing, false);
            // Cancel any pending withdraw: this remap proves the earlier unmap was
            // transient (Electron churn / a tray restore), not a real close.
            self.x11_pending_withdraw.remove(&existing);
            let was_ready = self.windows.is_ready(existing);
            self.windows.set_ready(existing, true);
            // Restoring from the tray (a client-driven remap) must surface the
            // window on the workspace the user is actually looking at. The dock
            // path does this via `activate_window_by_id`; without it here,
            // `apply_window_rect`'s visibility guard would unmap a window whose
            // stale workspace no longer matches the active one — the window would
            // flash open and immediately vanish ("opens then closes").
            let key = self.desk_key_for_window(existing);
            self.windows
                .set_workspace(existing, self.active_workspace_for(&key));
            self.apply_window_rect(existing);
            if !was_ready && let Some(record) = self.windows.get(existing).cloned() {
                let (title, app_id) = self.read_window_metadata(&record);
                let suggested_rect = self.windows.target_rect(existing).unwrap_or(PixelRect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 600,
                });
                self.event_bus.emit(&CompositorEvent::WindowOpened {
                    id: existing,
                    title,
                    app_id,
                    suggested_rect,
                });
            }
            let _ = window.set_activated(true);
            self.note_window_focus(existing);
            self.focus_window_id(existing);
            self.event_bus
                .emit(&CompositorEvent::WindowFocused { id: existing });
            self.schedule_redraw();
            return;
        }

        let elem = Window::new_x11_window(window.clone());
        // Map once so the space can resolve the element before we position it.
        self.space.map_element(elem.clone(), (0, 0), false);

        let title = {
            let t = window.title();
            if t.trim().is_empty() {
                "Application".to_string()
            } else {
                t
            }
        };
        let app_id = {
            let class = window.class();
            if class.trim().is_empty() {
                None
            } else {
                Some(class)
            }
        };

        let id = self
            .windows
            .register_x11(elem, window.clone(), title.clone(), app_id.clone());
        if let Some(surface) = window.wl_surface() {
            use smithay::reexports::wayland_server::Resource;
            self.windows
                .index_x11_surface(window.window_id(), surface.id());
        }

        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        self.windows.set_output(id, key.clone());
        self.windows
            .set_workspace(id, self.active_workspace_for(&key));
        // X11 windows are floating; never reserve a grid tile for them.
        self.floating.insert(id);
        self.refresh_window_decoration_mode(id);
        let is_splash = {
            use smithay::xwayland::xwm::WmWindowType;
            matches!(window.window_type(), Some(WmWindowType::Splash))
                || crate::state::title_looks_like_splash(&title)
        };
        // Splash screens share the main app's WM_CLASS — force natural size so we
        // don't stretch/tile a small bitmap into the saved main-window geometry.
        if is_splash {
            // Prefer no Metis chrome on boot splash; keep Motif/heuristic unless the
            // user forced SSD (already applied above). Splash bitmaps often look wrong
            // under a titlebar inset.
            tracing::info!(id, %title, "x11: splash window — natural size placement");
        }
        // Match Wayland: game-rules can request true-fullscreen once mapped.
        let rule = self
            .game_rules
            .evaluate(app_id.as_deref(), Some(title.as_str()));
        if rule.fullscreen && !is_splash {
            self.pending_game_fullscreen.insert(id);
        }
        self.place_x11_window(id, window.geometry().size, app_id.as_deref(), is_splash);
        self.apply_window_rect(id);
        self.windows.set_ready(id, true);
        let _ = window.set_activated(true);

        self.persist_layout();
        self.emit_layout_changed();
        let suggested_rect = self.windows.target_rect(id).unwrap_or(PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        });
        self.event_bus.emit(&CompositorEvent::WindowOpened {
            id,
            title,
            app_id,
            suggested_rect,
        });
        self.maybe_place_remote_viewer_window(id);
        self.note_window_focus(id);
        self.focus_window_id(id);
        self.event_bus.emit(&CompositorEvent::WindowFocused { id });
        if self.pending_game_fullscreen.remove(&id) {
            self.set_fullscreen(id, true, None);
        }
        self.schedule_redraw();
    }

    /// Floating placement for a freshly mapped X11 window: restore saved geometry
    /// when the app has been seen before, otherwise center the client's natural
    /// size under the bar. Splash windows always keep their natural size.
    /// Near-fullscreen / borderless game sizes map flush to the output origin so
    /// they are not inset under the edge bar and clipped.
    fn place_x11_window(
        &mut self,
        id: u32,
        natural: Size<i32, Logical>,
        app_id: Option<&str>,
        is_splash: bool,
    ) {
        if is_splash {
            let w = if natural.w > 0 {
                natural.w
            } else {
                crate::state::DEFAULT_FLOAT_W / 2
            };
            let h = if natural.h > 0 {
                natural.h
            } else {
                crate::state::DEFAULT_FLOAT_H / 3
            };
            let rect = self.centered_body_for_window(id, w, h);
            self.windows.set_target_rect(id, rect);
            self.windows.set_placement_chosen(id, true);
            return;
        }
        // Honor the client's requested size when available — enlarging a splash /
        // dialog to DEFAULT_FLOAT makes toolbar bitmaps tile across a huge window.
        let w = if natural.w > 0 {
            natural.w
        } else {
            crate::state::DEFAULT_FLOAT_W
        };
        let h = if natural.h > 0 {
            natural.h
        } else {
            crate::state::DEFAULT_FLOAT_H
        };
        let undecorated = self
            .windows
            .get(id)
            .and_then(|r| r.x11())
            .map(|x11| !x11.is_decorated())
            .unwrap_or(false);
        let is_launcher = x11_is_game_launcher(app_id);
        // Launchers (Steam splash, etc.) animate size while loading — never force
        // fullscreen or special output centering or they fight Metis in a loop.
        if !is_launcher {
            if let Some(rect) = self.borderless_output_rect_for(id, w, h, undecorated) {
                tracing::info!(
                    id,
                    ?rect,
                    undecorated,
                    "x11: borderless/near-fullscreen placement at output origin"
                );
                self.windows.set_target_rect(id, rect);
                self.windows.set_placement_chosen(id, true);
                self.pending_game_fullscreen.insert(id);
                return;
            }
            if undecorated
                && let Some(rect) = self.centered_on_output_for(id, w, h).filter(|_| {
                    self.launch_output_for(id)
                        .and_then(|o| self.space.output_geometry(&o))
                        .is_some_and(|g| x11_large_undecorated_float(g, w, h))
                })
            {
                tracing::info!(id, ?rect, "x11: large undecorated float centered on output");
                self.windows.set_target_rect(id, rect);
                self.windows.set_placement_chosen(id, true);
                return;
            }
        }
        if let Some(app_id) = app_id
            && let Some(saved) = self.window_state.get(app_id)
        {
            let saved_rect = saved.to_rect();
            if crate::state::saved_size_is_usable(saved_rect.width, saved_rect.height) {
                // Never restore a stale near-fullscreen save into the usable
                // zone — that recreates the clipped borderless-window bug.
                if let Some(rect) = self.borderless_output_rect_for(
                    id,
                    saved_rect.width,
                    saved_rect.height,
                    undecorated,
                ) {
                    self.windows.set_target_rect(id, rect);
                    self.windows.set_placement_chosen(id, true);
                    return;
                }
                let rect = self.restore_body_for_window(id, saved_rect);
                self.windows.set_target_rect(id, rect);
                self.windows.set_placement_chosen(id, true);
                return;
            }
            self.window_state.remove(app_id);
        }
        let rect = self.centered_body_for_window(id, w, h);
        self.windows.set_target_rect(id, rect);
        self.windows.set_placement_chosen(id, true);
    }

    /// When `w`×`h` looks like borderless / fake-fullscreen for `id`'s output,
    /// return that output's full geometry (flush origin). Otherwise `None`.
    pub(crate) fn borderless_output_rect_for(
        &self,
        id: u32,
        w: i32,
        h: i32,
        undecorated: bool,
    ) -> Option<PixelRect> {
        let output = self.launch_output_for(id)?;
        let geo = self.space.output_geometry(&output)?;
        if !x11_borderless_fullscreen_intent(geo, w, h, undecorated) {
            return None;
        }
        Some(PixelRect {
            x: geo.loc.x,
            y: geo.loc.y,
            width: geo.size.w,
            height: geo.size.h,
        })
    }

    /// Center `w`×`h` on the full output (not the bar usable zone). Used for
    /// large undecorated game floats so they are not inset under the edge bar.
    pub(crate) fn centered_on_output_for(&self, id: u32, w: i32, h: i32) -> Option<PixelRect> {
        let output = self.launch_output_for(id)?;
        let geo = self.space.output_geometry(&output)?;
        let width = w.clamp(1, geo.size.w);
        let height = h.clamp(1, geo.size.h);
        Some(PixelRect {
            x: geo.loc.x + (geo.size.w - width) / 2,
            y: geo.loc.y + (geo.size.h - height) / 2,
            width,
            height,
        })
    }

    /// Undecorated X11 float large enough that bar-inset clamping would recreate
    /// the off-screen borderless-game bug.
    pub(crate) fn x11_keep_on_full_output(&self, id: u32, rect: PixelRect) -> bool {
        let Some(record) = self.windows.get(id) else {
            return false;
        };
        let Some(x11) = record.x11() else {
            return false;
        };
        if x11.is_decorated() {
            return false;
        }
        let Some(output) = self.launch_output_for(id) else {
            return false;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return false;
        };
        x11_large_undecorated_float(geo, rect.width, rect.height)
    }

    /// True when `rect` already covers (nearly) an entire output — skip bar inset.
    pub(crate) fn rect_is_output_covering(&self, rect: PixelRect) -> bool {
        for output in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            if x11_borderless_fullscreen_intent(geo, rect.width, rect.height, true)
                && (rect.x - geo.loc.x).abs() <= crate::state::WINDOW_GAP_PX * 2
                && (rect.y - geo.loc.y).abs() <= crate::state::WINDOW_GAP_PX * 2
            {
                return true;
            }
        }
        false
    }

    /// Handle a client-initiated unmap of an X11 window. This is *deferred*: we hide
    /// the (now bufferless) element immediately, but only arm a pending-withdraw
    /// timer rather than tearing the window down. Electron apps (Claude Desktop)
    /// unmap/remap their X11 window constantly during normal operation and, notably,
    /// as part of restoring from the tray — reacting to each unmap would thrash the
    /// dock and make the window flash open and vanish. `tick_x11_withdraws` promotes
    /// a still-unmapped window to a real "close to tray" after a grace period;
    /// `map_x11_toplevel` cancels the pending withdraw if the window comes back.
    pub(crate) fn unmap_x11_toplevel(&mut self, window: &X11Surface) {
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.space.unmap_elem(&record.window);
        self.x11_pending_withdraw
            .entry(id)
            .or_insert_with(std::time::Instant::now);
        tracing::info!(
            id,
            x11_window = window.window_id(),
            "x11: unmap (withdraw armed)"
        );
        self.schedule_redraw();
    }

    /// Grace period before a client-unmapped X11 window is treated as withdrawn to
    /// the tray. Long enough to swallow Electron's transient unmap/remap churn, short
    /// enough that a genuine close-to-tray drops from the dock promptly.
    const X11_WITHDRAW_GRACE: std::time::Duration = std::time::Duration::from_millis(600);

    /// Promote X11 windows that have stayed unmapped past the grace period to a real
    /// withdraw: drop them from the dock/tasklist (like GNOME/KDE do for tray apps)
    /// so a stale entry can't restore to an empty frame. The registry record is kept,
    /// keyed by X11 window id, so a later remap re-announces and re-shows the window.
    /// Returns true if anything changed (so the caller can flag damage).
    pub(crate) fn tick_x11_withdraws(&mut self) -> bool {
        if self.x11_pending_withdraw.is_empty() {
            return false;
        }
        let now = std::time::Instant::now();
        let due: Vec<u32> = self
            .x11_pending_withdraw
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= Self::X11_WITHDRAW_GRACE)
            .map(|(id, _)| *id)
            .collect();
        if due.is_empty() {
            return false;
        }
        for id in due {
            self.x11_pending_withdraw.remove(&id);
            // A window that no longer exists, or is already mapped again, needs no
            // teardown (the remap path clears the pending entry, but guard anyway).
            if self.windows.get(id).is_none() {
                continue;
            }
            tracing::info!(id, "x11: withdraw confirmed — dropping dock entry");
            self.drop_window_fullscreen(id);
            self.windows.set_ready(id, false);
            self.windows.set_fullscreen(id, false);
            self.windows.set_maximized(id, false);
            self.clear_auto_hide(id);
            if self.last_focused_window == Some(id) {
                self.last_focused_window = None;
            }
            if self.focused_window_id() == Some(id) {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kbd) = self.seat.get_keyboard() {
                    kbd.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
                }
            }
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowClosed { id });
            self.emit_layout_changed();
        }
        true
    }

    /// Tear down a destroyed X11 window: drop it from the registry, close out any
    /// fullscreen bookkeeping, and notify the shell (dock/IPC) like a Wayland close.
    pub(crate) fn destroy_x11_toplevel(&mut self, window: &X11Surface) {
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            return;
        };
        tracing::info!(id, x11_window = window.window_id(), "x11: window destroyed");
        self.x11_pending_withdraw.remove(&id);
        let ready = self.windows.is_ready(id);
        self.drop_window_fullscreen(id);
        self.save_window_geometry(id);
        if let Some(record) = self.windows.unregister(id) {
            self.space.unmap_elem(&record.window);
        }
        if ready {
            self.on_window_destroyed(id);
        }
        self.schedule_redraw();
    }
}

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
            std::iter::empty::<String>(),
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

    /// Re-place a managed X11 float after the client changes size.
    ///
    /// Games often map small, then grow to a borderless/monitor-sized buffer while
    /// Metis still holds the old centered top-left — that spills the surface off
    /// the right/bottom. Launchers (Steam splash) are excluded: they animate size
    /// while loading and fight aggressive re-configure in a visible loop.
    pub(crate) fn reposition_x11_after_size_change(
        &mut self,
        id: u32,
        window: &X11Surface,
        elem: &Window,
        new_size: Size<i32, Logical>,
    ) {
        let app_id = self.windows.get(id).and_then(|r| r.app_id.clone());
        if x11_is_game_launcher(app_id.as_deref()) {
            self.apply_x11_launcher_configure(id, window, elem, new_size);
            return;
        }

        let undecorated = !window.is_decorated();
        let prev_loc = self
            .space
            .element_location(elem)
            .unwrap_or_else(|| Point::from((0, 0)));
        let old = self
            .windows
            .target_rect(id)
            .map(|r| Size::from((r.width, r.height)))
            .unwrap_or(new_size);
        let size_grew = new_size.w > old.w + 32 || new_size.h > old.h + 32;

        // Near-monitor borderless games: true fullscreen (only after a real grow).
        if size_grew
            && let Some(rect) =
                self.borderless_output_rect_for(id, new_size.w, new_size.h, undecorated)
        {
            tracing::info!(
                id,
                x11_window = window.window_id(),
                ?rect,
                size = ?new_size,
                "x11: size change → borderless flush + fullscreen"
            );
            self.windows.set_target_rect(id, rect);
            self.set_fullscreen(id, true, None);
            return;
        }

        // Only re-center when the client grew — shrinking/oscillating sizes must
        // not yank placement or we loop with splash/loaders.
        if !size_grew {
            self.apply_x11_launcher_configure(id, window, elem, new_size);
            return;
        }

        let output_geo = self
            .launch_output_for(id)
            .and_then(|o| self.space.output_geometry(&o));
        let large_undeco = undecorated
            && output_geo.is_some_and(|g| x11_large_undecorated_float(g, new_size.w, new_size.h));
        let rect = if large_undeco {
            self.centered_on_output_for(id, new_size.w, new_size.h)
                .unwrap_or_else(|| self.centered_body_for_window(id, new_size.w, new_size.h))
        } else {
            self.centered_body_for_window(id, new_size.w, new_size.h)
        };
        // Keep the client's exact size; only move. Changing size here causes
        // clients to ConfigureRequest again → expand/shrink loops.
        let geo = Rectangle::new(
            Point::from((rect.x, rect.y)),
            Size::from((new_size.w.max(1), new_size.h.max(1))),
        );
        let placed = metis_grid::PixelRect {
            x: geo.loc.x,
            y: geo.loc.y,
            width: geo.size.w,
            height: geo.size.h,
        };
        if geo.loc != prev_loc {
            self.space.relocate_element(elem, geo.loc);
        }
        self.windows.set_target_rect(id, placed);
        let _ = window.configure(geo);
        tracing::info!(
            id,
            x11_window = window.window_id(),
            ?placed,
            "x11: size grew → re-centered float (size preserved)"
        );
        self.schedule_redraw();
    }

    /// Honor the client's exact size and keep the current location.
    ///
    /// Critical for Steam splash: never shrink/expand the configure size or the
    /// client fights us in an expand/shrink loop while "Waiting for network…".
    fn apply_x11_launcher_configure(
        &mut self,
        id: u32,
        window: &X11Surface,
        elem: &Window,
        new_size: Size<i32, Logical>,
    ) {
        let loc = self
            .space
            .element_location(elem)
            .unwrap_or_else(|| Point::from((0, 0)));
        let size = Size::from((new_size.w.max(1), new_size.h.max(1)));
        let placed = metis_grid::PixelRect {
            x: loc.x,
            y: loc.y,
            width: size.w,
            height: size.h,
        };
        self.windows.set_target_rect(id, placed);
        let _ = window.configure(Rectangle::new(loc, size));
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
        if let Some(focused) = self.focused_window_id()
            && focused != id
            && self.window_is_running_game(focused)
        {
            tracing::info!(
                requester = id,
                focused,
                x11_window = window.window_id(),
                "x11: blocked _NET_ACTIVE_WINDOW focus-steal while a game is focused"
            );
            return;
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
            if let Some(elem) = self.x11_element(&window)
                && let Some(output) = self.output_for_x11_element(&elem)
                && let Some(geo) = self.space.output_geometry(&output)
            {
                let _ = window.configure(geo);
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

        let Some(elem) = self.x11_element(&window) else {
            let _ = window.configure(geo);
            return;
        };
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            // Unmanaged: keep prior behavior (anchor at current Space loc if any).
            if let Some(loc) = self.space.element_location(&elem) {
                geo.loc = loc;
            }
            let _ = window.configure(geo);
            return;
        };
        if self
            .windows
            .get(id)
            .is_some_and(|r| r.maximized || r.fullscreen)
        {
            if let Some(output) = self.output_for_x11_element(&elem)
                && let Some(out_geo) = self.space.output_geometry(&output)
            {
                let _ = window.configure(out_geo);
            }
            return;
        }

        self.reposition_x11_after_size_change(id, &window, &elem, geo.size);
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
        // map and right after an interactive move.
        //
        // Size changes are different: some games grow their buffer without a
        // ConfigureRequest we still see as a delta (geometry already matches).
        // Re-place when the committed size diverges from our target so borderless
        // titles cannot keep a stale centered top-left and spill off-screen.
        if let Some(id) = self.windows.id_for_x11_window(window.window_id()) {
            if self
                .windows
                .get(id)
                .is_some_and(|r| r.maximized || r.fullscreen)
            {
                self.schedule_redraw();
                return;
            }
            let app_id = self.windows.get(id).and_then(|r| r.app_id.clone());
            // Steam splash animates size — never re-place from ConfigureNotify.
            if x11_is_game_launcher(app_id.as_deref()) {
                if let Some(t) = self.windows.target_rect(id) {
                    // Keep target size in sync so later grow detection is accurate.
                    self.windows.set_target_rect(
                        id,
                        metis_grid::PixelRect {
                            x: t.x,
                            y: t.y,
                            width: geometry.size.w.max(1),
                            height: geometry.size.h.max(1),
                        },
                    );
                }
                self.schedule_redraw();
                return;
            }
            let target = self.windows.target_rect(id);
            let size_grew = target
                .is_some_and(|t| geometry.size.w > t.width + 32 || geometry.size.h > t.height + 32);
            if size_grew {
                self.reposition_x11_after_size_change(id, &window, &elem, geometry.size);
                return;
            }
            if let Some(t) = target {
                self.windows.set_target_rect(
                    id,
                    metis_grid::PixelRect {
                        x: t.x,
                        y: t.y,
                        width: geometry.size.w.max(1),
                        height: geometry.size.h.max(1),
                    },
                );
            }
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
            if let Some(record) = self.windows.get(id)
                && (record.maximized || record.fullscreen)
            {
                return;
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
        if let Some(keyboard) = self.seat.get_keyboard()
            && let Some(KeyboardFocusTarget::Window(w)) = keyboard.current_focus()
            && let Some(surface) = w.x11_surface()
            && surface.xwm_id() == Some(xwm)
        {
            return true;
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

#[cfg(test)]
mod borderless_intent_tests {
    use super::*;

    fn output(w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((0, 0)), Size::from((w, h)))
    }

    #[test]
    fn exact_monitor_size_matches() {
        assert!(x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1920,
            1080,
            false
        ));
    }

    #[test]
    fn undecorated_near_full_matches() {
        assert!(x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1728,
            972,
            true
        ));
    }

    #[test]
    fn undecorated_half_is_large_float() {
        assert!(!x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1280,
            720,
            true
        ));
        assert!(x11_large_undecorated_float(output(1920, 1080), 1280, 720));
    }

    #[test]
    fn small_dialog_does_not_match() {
        assert!(!x11_borderless_fullscreen_intent(
            output(1920, 1080),
            640,
            480,
            true
        ));
        assert!(!x11_large_undecorated_float(output(1920, 1080), 640, 480));
    }

    #[test]
    fn steam_is_launcher_but_steam_app_is_not() {
        assert!(x11_is_game_launcher(Some("steam")));
        assert!(x11_is_game_launcher(Some("Steam")));
        assert!(!x11_is_game_launcher(Some("steam_app_12345")));
        assert!(!x11_is_game_launcher(Some("hl2.exe")));
    }
}
