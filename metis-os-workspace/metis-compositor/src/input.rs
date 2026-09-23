use smithay::{
    backend::input::KeyState,
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchEvent,
    },
    input::{
        keyboard::{FilterResult, keysyms},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
        touch::{DownEvent, MotionEvent as TouchMotionEventWl, UpEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint},
};

use crate::focus::KeyboardFocusTarget;
use crate::keybinds::{capture_active, keysym_to_token, mod_active};
use crate::state::MetisState;
use metis_config::KeybindAction;

/// Map a Ctrl+Alt+F<n> press to a 1-based virtual terminal number. Matches both
/// the dedicated `XF86Switch_VT_n` keysyms (when xkb is configured for them) and
/// the plain `F1..F12` keysyms (the common case under Ctrl+Alt).
fn vt_from_keysym(sym: u32) -> Option<i32> {
    const VT_BASE: u32 = 0x1008_FE01; // XF86Switch_VT_1
    if (VT_BASE..VT_BASE + 12).contains(&sym) {
        return Some((sym - VT_BASE + 1) as i32);
    }
    if (keysyms::KEY_F1..=keysyms::KEY_F12).contains(&sym) {
        return Some((sym - keysyms::KEY_F1 + 1) as i32);
    }
    None
}

fn is_super_keysym(sym: u32) -> bool {
    matches!(
        sym,
        keysyms::KEY_Super_L | keysyms::KEY_Super_R | keysyms::KEY_Meta_L | keysyms::KEY_Meta_R
    )
}

/// Multimedia / hardware keysyms (`XF86*`) emitted by laptop function rows and
/// media keyboards. Matched by raw value so we do not depend on which XF86
/// symbols the installed `xkbcommon` happens to export as named constants.
mod xf86 {
    pub const MON_BRIGHTNESS_UP: u32 = 0x1008_FF02;
    pub const MON_BRIGHTNESS_DOWN: u32 = 0x1008_FF03;
    pub const KBD_LIGHT_ON_OFF: u32 = 0x1008_FF04;
    pub const KBD_BRIGHTNESS_UP: u32 = 0x1008_FF05;
    pub const KBD_BRIGHTNESS_DOWN: u32 = 0x1008_FF06;
    pub const AUDIO_LOWER_VOLUME: u32 = 0x1008_FF11;
    pub const AUDIO_MUTE: u32 = 0x1008_FF12;
    pub const AUDIO_RAISE_VOLUME: u32 = 0x1008_FF13;
    pub const AUDIO_PLAY: u32 = 0x1008_FF14;
    pub const AUDIO_STOP: u32 = 0x1008_FF15;
    pub const AUDIO_PREV: u32 = 0x1008_FF16;
    pub const AUDIO_NEXT: u32 = 0x1008_FF17;
    pub const AUDIO_PAUSE: u32 = 0x1008_FF31;
    pub const AUDIO_REWIND: u32 = 0x1008_FF3E;
    pub const AUDIO_FORWARD: u32 = 0x1008_FF97;
    pub const AUDIO_MICMUTE: u32 = 0x1008_FFB2;
    pub const DISPLAY: u32 = 0x1008_FF59;
}

/// Map a multimedia keysym to the shell runtime command that services it. The
/// shell owns audio (PipeWire/PulseAudio), backlight (logind), and MPRIS media
/// control plus the on-screen level overlay, so the compositor just forwards the
/// intent. `XF86Display` is handled in the compositor itself (see caller) since
/// it toggles output mirror/extend mode.
fn hardware_key_command(sym: u32) -> Option<&'static str> {
    Some(match sym {
        xf86::AUDIO_RAISE_VOLUME => "hw volume-up",
        xf86::AUDIO_LOWER_VOLUME => "hw volume-down",
        xf86::AUDIO_MUTE => "hw volume-mute",
        xf86::AUDIO_MICMUTE => "hw mic-mute",
        xf86::MON_BRIGHTNESS_UP => "hw brightness-up",
        xf86::MON_BRIGHTNESS_DOWN => "hw brightness-down",
        xf86::KBD_BRIGHTNESS_UP => "hw kbd-backlight-up",
        xf86::KBD_BRIGHTNESS_DOWN => "hw kbd-backlight-down",
        xf86::KBD_LIGHT_ON_OFF => "hw kbd-backlight-toggle",
        xf86::AUDIO_PLAY | xf86::AUDIO_PAUSE => "hw media-playpause",
        xf86::AUDIO_STOP => "hw media-stop",
        xf86::AUDIO_NEXT => "hw media-next",
        xf86::AUDIO_PREV => "hw media-prev",
        xf86::AUDIO_FORWARD => "hw media-forward",
        xf86::AUDIO_REWIND => "hw media-rewind",
        _ => return None,
    })
}

/// Append the connector name under the pointer so the shell captures the right
/// output on multi-monitor setups (`screenshot DP-1`, …).
fn screenshot_runtime_cmd(state: &MetisState, verb: &str) -> String {
    match state.output_under_pointer() {
        Some(out) => format!("{verb} {}", out.name()),
        None => verb.to_string(),
    }
}

/// Run a configured desktop shortcut. Returns `true` when the event should be
/// intercepted (even if the action was a no-op for the current layout).
fn dispatch_keybind(state: &mut MetisState, action: KeybindAction) -> bool {
    if state.session_is_locked() {
        return false;
    }

    match action {
        KeybindAction::Screenshot => {
            let _ =
                metis_protocol::write_runtime_command(&screenshot_runtime_cmd(state, "screenshot"));
            true
        }
        KeybindAction::ScreenshotFull => {
            let _ = metis_protocol::write_runtime_command(&screenshot_runtime_cmd(
                state,
                "screenshot instant-full",
            ));
            true
        }
        KeybindAction::ScreenshotWindow => {
            let _ = metis_protocol::write_runtime_command(&screenshot_runtime_cmd(
                state,
                "screenshot window",
            ));
            true
        }
        KeybindAction::WindowSwitcherNext => {
            let _ = metis_protocol::write_runtime_command("task-view-next");
            true
        }
        KeybindAction::WindowSwitcherPrev => {
            let _ = metis_protocol::write_runtime_command("task-view-prev");
            true
        }
        KeybindAction::WorkspaceOverview => {
            // Super+Tab opens Task View, then each further Super+Tab cycles —
            // same verb as next so the chord never toggles the overlay closed.
            let _ = metis_protocol::write_runtime_command("task-view-next");
            true
        }
        KeybindAction::CycleWorkspacePrev => {
            let key = state
                .output_under_pointer()
                .map(|o| o.name())
                .unwrap_or_else(|| state.primary_key());
            state.cycle_workspace_routed(&key, -1);
            true
        }
        KeybindAction::CycleWorkspaceNext => {
            let key = state
                .output_under_pointer()
                .map(|o| o.name())
                .unwrap_or_else(|| state.primary_key());
            state.cycle_workspace_routed(&key, 1);
            true
        }
        KeybindAction::Workspace1
        | KeybindAction::Workspace2
        | KeybindAction::Workspace3
        | KeybindAction::Workspace4
        | KeybindAction::Workspace5
        | KeybindAction::Workspace6
        | KeybindAction::Workspace7
        | KeybindAction::Workspace8
        | KeybindAction::Workspace9 => {
            if let Some(ws) = action.workspace_number() {
                let key = state
                    .output_under_pointer()
                    .map(|o| o.name())
                    .unwrap_or_else(|| state.primary_key());
                state.switch_workspace_routed(&key, ws);
            }
            true
        }
        KeybindAction::MoveToWorkspace1
        | KeybindAction::MoveToWorkspace2
        | KeybindAction::MoveToWorkspace3
        | KeybindAction::MoveToWorkspace4
        | KeybindAction::MoveToWorkspace5
        | KeybindAction::MoveToWorkspace6
        | KeybindAction::MoveToWorkspace7
        | KeybindAction::MoveToWorkspace8
        | KeybindAction::MoveToWorkspace9 => {
            if let (Some(ws), Some(id)) = (action.workspace_number(), state.focused_window_id()) {
                state.move_window_to_workspace(id, ws);
            }
            true
        }
        KeybindAction::ScrollFocusLeft => state.scroll_focus_left(),
        KeybindAction::ScrollFocusRight => state.scroll_focus_right(),
        KeybindAction::ScrollFocusUp => state.scroll_focus_up(),
        KeybindAction::ScrollFocusDown => state.scroll_focus_down(),
        KeybindAction::ScrollMoveLeft => {
            if state.scroll_move_left() {
                true
            } else if !state.scroll_navigation_active() {
                if let Some(id) = state.focused_window_id() {
                    state.move_window_to_adjacent_output(id, -1);
                }
                true
            } else {
                false
            }
        }
        KeybindAction::ScrollMoveRight => {
            if state.scroll_move_right() {
                true
            } else if !state.scroll_navigation_active() {
                if let Some(id) = state.focused_window_id() {
                    state.move_window_to_adjacent_output(id, 1);
                }
                true
            } else {
                false
            }
        }
        KeybindAction::ScrollMoveUp => state.scroll_move_up(),
        KeybindAction::ScrollMoveDown => state.scroll_move_down(),
        KeybindAction::ScrollConsume => state.scroll_consume(),
        KeybindAction::ScrollExpel => state.scroll_expel(),
        KeybindAction::ScrollCycleWidth => state.scroll_cycle_width(),
        KeybindAction::MoveWorkspaceOutputLeft => {
            if state.workspace_mode() == metis_config::WorkspaceMode::Separate {
                let key = state
                    .output_under_pointer()
                    .map(|o| o.name())
                    .unwrap_or_else(|| state.primary_key());
                state.move_active_workspace_to_adjacent_output(&key, -1);
                true
            } else {
                false
            }
        }
        KeybindAction::MoveWorkspaceOutputRight => {
            if state.workspace_mode() == metis_config::WorkspaceMode::Separate {
                let key = state
                    .output_under_pointer()
                    .map(|o| o.name())
                    .unwrap_or_else(|| state.primary_key());
                state.move_active_workspace_to_adjacent_output(&key, 1);
                true
            } else {
                false
            }
        }
        KeybindAction::LayoutGrid => {
            let key = state
                .output_under_pointer()
                .map(|o| o.name())
                .unwrap_or_else(|| state.primary_key());
            state.enable_grid_tiling(&key);
            true
        }
        KeybindAction::LayoutFree => {
            let key = state
                .output_under_pointer()
                .map(|o| o.name())
                .unwrap_or_else(|| state.primary_key());
            state.disable_grid_tiling(&key);
            true
        }
        KeybindAction::CloseWindow => {
            if let Some(id) = state.focused_window_id() {
                state.close_window(id);
            }
            true
        }
        KeybindAction::Fullscreen => {
            if let Some(id) = state.focused_window_id() {
                let fs = state.windows.get(id).map(|w| w.fullscreen).unwrap_or(false);
                state.set_fullscreen(id, !fs, None);
            }
            true
        }
        KeybindAction::Maximize => {
            if let Some(id) = state.focused_window_id() {
                return state.toggle_maximized_debounced(id);
            }
            true
        }
        KeybindAction::Minimize => {
            if let Some(id) = state.focused_window_id() {
                state.minimize_by_id(id);
            }
            true
        }
        KeybindAction::Lock => {
            state.lock_session();
            true
        }
        KeybindAction::OpenTerminal => {
            match metis_config::resolve_terminal() {
                Some(term) => {
                    state.spawn_client_argv(&[term]);
                }
                None => {
                    tracing::warn!(
                        "OpenTerminal keybind: no terminal configured or found in PATH \
                         (Settings → Metis Menu)"
                    );
                }
            }
            true
        }
        KeybindAction::ExitFullscreenStack => {
            if let Some(id) = state.focused_window_id() {
                if state.windows.get(id).is_some_and(|w| w.fullscreen) {
                    state.set_fullscreen(id, false, None);
                } else if state.windows.get(id).is_some_and(|w| w.maximized) {
                    state.set_maximized(id, false);
                } else if let Some(tile_id) = state.tile_id_for_window(id) {
                    state.set_tile_mode(&tile_id, metis_protocol::TileMode::Grid);
                }
            }
            true
        }
    }
}

impl MetisState {
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        // Any hardware input counts as activity: wake a blanked screen and
        // restart the idle countdown before dispatching the event.
        self.idle_notify_activity();

        // While the session is locked, no pointer input reaches clients — motion
        // still moves the (compositor-drawn) cursor and repaints, but buttons and
        // scroll are swallowed. Keyboard events fall through to the filter below,
        // which routes them into the password buffer and never forwards them.
        if self.lock.locked && !matches!(event, InputEvent::Keyboard { .. }) {
            if let Some(pointer) = self.seat.get_pointer() {
                match &event {
                    InputEvent::PointerMotion { event: e, .. } => {
                        let loc = self.clamp_to_desktop(pointer.current_location() + e.delta());
                        pointer.set_location(loc);
                        self.lock_update_hover(loc);
                    }
                    InputEvent::PointerMotionAbsolute { event: e, .. } => {
                        let bounds = self.desktop_bounds();
                        let pos = e.position_transformed(bounds.size) + bounds.loc.to_f64();
                        pointer.set_location(pos);
                        self.lock_update_hover(pos);
                    }
                    InputEvent::PointerButton { event: e, .. } => {
                        // Left-press on a power control (suspend/restart/shutdown) fires it.
                        if e.state() == ButtonState::Pressed && e.button_code() == 0x110 {
                            let loc = pointer.current_location();
                            self.lock_pointer_click(loc);
                        }
                    }
                    InputEvent::TouchDown { event: e, .. } => {
                        if let Some(loc) = self.touch_location_transformed(e) {
                            self.lock_update_hover(loc);
                            self.lock_pointer_click(loc);
                        }
                    }
                    InputEvent::TouchMotion { event: e, .. } => {
                        if let Some(loc) = self.touch_location_transformed(e) {
                            self.lock_update_hover(loc);
                        }
                    }
                    _ => {}
                }
            }
            self.schedule_redraw();
            return;
        }

        let mut needs_redraw = false;
        match event {
            InputEvent::Keyboard { event, .. } => {
                needs_redraw = true;
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event);
                let key_state = event.state();

                // Anvil-style: Exclusive Top/Overlay layers own the keyboard
                // whenever mapped (Metis Menu, Control Center, screenshot). Set
                // focus *before* the filter so Forward delivers keys even when
                // the pointer is not over the surface — required for Super-key
                // menu opens where there was never a click to claim OnDemand focus.
                if !self.session_is_locked() {
                    if let Some(layer) = self.exclusive_keyboard_layer()
                        && let Some(keyboard) = self.seat.get_keyboard()
                    {
                        keyboard.set_focus(self, Some(KeyboardFocusTarget::from(layer)), serial);
                    }
                } else if self.protocol_lock.is_locked()
                    && let Some(focus) = self.protocol_lock_keyboard_focus()
                    && let Some(keyboard) = self.seat.get_keyboard()
                {
                    keyboard.set_focus(self, Some(focus), serial);
                }

                let Some(keyboard) = self.seat.get_keyboard() else {
                    tracing::warn!("keyboard event: seat has no keyboard");
                    return;
                };
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |state, modifiers, keysym| {
                        // Locked: capture every key into the password field and
                        // never forward it to a client. Ctrl+Alt+Backspace still
                        // quits the DRM session (escape hatch). Ctrl+Alt+F<n> VT
                        // switches are blocked while locked (Phase 15 §C).
                        if state.lock.locked {
                            if key_state == KeyState::Pressed {
                                let sym = u32::from(keysym.modified_sym());
                                if state.is_drm_backend()
                                    && modifiers.ctrl
                                    && modifiers.alt
                                    && sym == keysyms::KEY_BackSpace
                                {
                                    state.drm_quit();
                                    return FilterResult::Intercept(());
                                }
                                match sym {
                                    keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                                        state.lock_submit()
                                    }
                                    keysyms::KEY_BackSpace => state.lock_backspace(),
                                    keysyms::KEY_Escape => state.lock_clear_input(),
                                    _ => {
                                        if let Some(c) = keysym.modified_sym().key_char()
                                            && !c.is_control()
                                        {
                                            state.lock_push_char(c);
                                        }
                                    }
                                }
                            }
                            return FilterResult::Intercept(());
                        }
                        // Protocol locker owns the session: block VT switches and
                        // desktop keybinds; forward everything else to the locker.
                        if state.protocol_lock.is_locked() {
                            if key_state == KeyState::Pressed {
                                let sym = u32::from(keysym.modified_sym());
                                if state.is_drm_backend()
                                    && modifiers.ctrl
                                    && modifiers.alt
                                    && sym == keysyms::KEY_BackSpace
                                {
                                    state.drm_quit();
                                    return FilterResult::Intercept(());
                                }
                                if state.is_drm_backend()
                                    && modifiers.ctrl
                                    && modifiers.alt
                                    && (keysyms::KEY_F1..=keysyms::KEY_F12).contains(&sym)
                                {
                                    return FilterResult::Intercept(());
                                }
                            }
                            // Swallow Super / configured chords so they cannot
                            // manipulate the desktop underneath the locker.
                            let sym = u32::from(keysym.modified_sym());
                            if is_super_keysym(sym) {
                                return FilterResult::Intercept(());
                            }
                            return FilterResult::Forward;
                        }
                        let sym = u32::from(keysym.modified_sym());

                        // Reserve a standalone Super tap for the application
                        // menu. Any other pressed key turns it into a normal
                        // shortcut chord instead.
                        //
                        // Task View is a *sticky* overlay (Windows Task View):
                        // releasing Super must neither open the menu nor
                        // activate/dismiss — the user releases Super to click
                        // and drag with the mouse.
                        if is_super_keysym(sym) {
                            match key_state {
                                KeyState::Pressed => {
                                    state.super_tap_armed = !modifiers.ctrl
                                        && !modifiers.alt
                                        && !state.task_view_overlay_active();
                                }
                                KeyState::Released => {
                                    let toggle_menu =
                                        state.super_tap_armed && !state.task_view_overlay_active();
                                    state.super_tap_armed = false;
                                    if toggle_menu {
                                        let _ =
                                            metis_protocol::write_runtime_command("toggle-menu");
                                    }
                                }
                            }
                            return FilterResult::Intercept(());
                        }

                        // Multimedia / hardware keys (volume, brightness, media
                        // transport, display switch). These carry fixed XF86
                        // keysyms and are handled system-wide regardless of focus
                        // or Settings shortcut-capture — matching GNOME/KDE. Act
                        // on press; swallow the release too so clients never see a
                        // dangling release for a key they never received.
                        if sym == xf86::DISPLAY || hardware_key_command(sym).is_some() {
                            if key_state == KeyState::Pressed {
                                state.super_tap_armed = false;
                                if sym == xf86::DISPLAY {
                                    state.toggle_display_mirror();
                                } else if let Some(cmd) = hardware_key_command(sym) {
                                    let _ = metis_protocol::write_runtime_command(cmd);
                                }
                            }
                            return FilterResult::Intercept(());
                        }

                        if key_state == KeyState::Pressed {
                            state.super_tap_armed = false;
                            if state.screenshot_overlay_active()
                                && sym == keysyms::KEY_Escape
                                && !mod_active(&state.keybinds, modifiers)
                            {
                                let _ = metis_protocol::write_runtime_command("dismiss-screenshot");
                                return FilterResult::Intercept(());
                            }
                            // Use the layout's raw Latin sym so Mod+Shift+<n>
                            // (whose modified sym is punctuation) still maps to a digit.
                            let digit_sym = keysym
                                .raw_latin_sym_or_raw_current_sym()
                                .map(u32::from)
                                .unwrap_or(sym);
                            // Standalone-session escape hatches (DRM backend only):
                            // Ctrl+Alt+F<n> switches VT, Ctrl+Alt+Backspace quits
                            // back to the greeter. Always reserved — not user-rebindable.
                            if state.is_drm_backend() && modifiers.ctrl && modifiers.alt {
                                if sym == keysyms::KEY_BackSpace {
                                    state.drm_quit();
                                    return FilterResult::Intercept(());
                                }
                                let vt_sym = keysym
                                    .raw_latin_sym_or_raw_current_sym()
                                    .map(u32::from)
                                    .unwrap_or(sym);
                                if let Some(vt) =
                                    vt_from_keysym(sym).or_else(|| vt_from_keysym(vt_sym))
                                {
                                    state.drm_change_vt(vt);
                                    return FilterResult::Intercept(());
                                }
                            }
                            if state.control_center_mapped() {
                                let bare_escape = sym == keysyms::KEY_Escape
                                    && !modifiers.ctrl
                                    && !modifiers.alt
                                    && !modifiers.shift
                                    && !modifiers.logo;
                                let super_q = modifiers.logo
                                    && !modifiers.ctrl
                                    && !modifiers.alt
                                    && matches!(sym, keysyms::KEY_q | keysyms::KEY_Q);
                                if bare_escape || super_q {
                                    state.request_close_bar_popovers();
                                    return FilterResult::Intercept(());
                                }
                            }
                            // Settings shortcut capture: do not fire global actions.
                            if capture_active() {
                                return FilterResult::Forward;
                            }
                            // Screenshot chords are global (like hardware keys): fire
                            // even while an Exclusive shell layer owns the keyboard
                            // (Metis Menu, Control Center, Notification Center).
                            if let Some(token) = keysym_to_token(sym, digit_sym)
                                && let Some(action) = state.keybinds.lookup(modifiers, &token)
                            {
                                let is_screenshot = matches!(
                                    action,
                                    KeybindAction::Screenshot
                                        | KeybindAction::ScreenshotFull
                                        | KeybindAction::ScreenshotWindow
                                );
                                let is_task_view_cycle = matches!(
                                    action,
                                    KeybindAction::WindowSwitcherNext
                                        | KeybindAction::WindowSwitcherPrev
                                );
                                let is_task_view =
                                    matches!(action, KeybindAction::WorkspaceOverview);
                                let allow = if is_screenshot {
                                    true
                                } else if is_task_view_cycle || is_task_view {
                                    // Keep cycling / toggling once Task View owns
                                    // the keyboard; otherwise only when free.
                                    state.task_view_overlay_active()
                                        || state.exclusive_keyboard_layer().is_none()
                                } else {
                                    state.exclusive_keyboard_layer().is_none()
                                };
                                if allow && dispatch_keybind(state, action) {
                                    return FilterResult::Intercept(());
                                }
                            }
                            // While an Exclusive layer owns the keyboard (menu,
                            // Control Center, …), skip remaining compositor chords
                            // so typing and Escape reach the shell surface instead.
                            if state.exclusive_keyboard_layer().is_some() {
                                return FilterResult::Forward;
                            }
                            // Trace bare Esc forwarded to a game (usually opens pause menu).
                            if sym == keysyms::KEY_Escape
                                && !mod_active(&state.keybinds, modifiers)
                                && !state.session_is_locked()
                                && let Some(id) = state.focused_window_id()
                            {
                                let app_id = state.windows.get(id).and_then(|r| r.app_id.clone());
                                if app_id.as_deref().is_some_and(|a| {
                                    a.starts_with("steam_app_") || a.contains(".exe")
                                }) {
                                    tracing::info!(
                                        id,
                                        ?app_id,
                                        "game-pointer: Esc forwarded to game"
                                    );
                                }
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("pointer motion: seat has no pointer");
                    return;
                };
                let current = pointer.current_location();
                // Surface under the *current* position drives constraint checks:
                // when the pointer is locked it never moves, so the target can't
                // change from raw motion.
                let under = self.pointer_target_at(current);

                if let Some((surface, _)) = under.as_ref() {
                    self.sync_pointer_constraint_phase(surface, &pointer);
                }

                // Pointer constraints (games: mouse-look lock / region confinement).
                let mut pointer_locked = false;
                let mut pointer_confined = false;
                let mut confine_region = None;
                if let Some((surface, surface_loc)) = under.as_ref() {
                    with_pointer_constraint(surface, &pointer, |constraint| {
                        let Some(constraint) = constraint else { return };
                        if !constraint.is_active() {
                            return;
                        }
                        // A region-limited constraint only applies while the
                        // pointer sits inside that region.
                        if !constraint.region().is_none_or(|region| {
                            region.contains((current - *surface_loc).to_i32_round())
                        }) {
                            return;
                        }
                        match &*constraint {
                            PointerConstraint::Locked(_) => pointer_locked = true,
                            PointerConstraint::Confined(confine) => {
                                pointer_confined = true;
                                confine_region = confine.region().cloned();
                            }
                        }
                    });
                }

                // Raw, unclamped delta always goes out as relative motion — this is
                // the signal games use for camera "look".
                pointer.relative_motion(
                    self,
                    under.clone(),
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        time: event.time(),
                    },
                );

                if pointer_locked {
                    // Spec: locked pointer emits relative motion only (Mutter/KWin).
                    // Locked: relative motion only (Mutter/KWin). Hints are kept
                    // for unlock restore, never for click remapping.
                    pointer.frame(self);
                    return;
                }

                // Relative motion (libinput) can run off-screen; clamp to the
                // union of output geometries so the cursor stays reachable.
                let location = self.clamp_to_desktop(current + event.delta());

                // Confined: reject moves that would leave the surface or its region.
                if pointer_confined && let Some((surface, surface_loc)) = under.as_ref() {
                    let new_under = self.pointer_target_at(location);
                    let same_surface = new_under.as_ref().map(|(s, _)| s) == Some(surface);
                    let in_region = confine_region.as_ref().is_none_or(|region| {
                        region.contains((location - *surface_loc).to_i32_round())
                    });
                    if !same_surface || !in_region {
                        pointer.frame(self);
                        return;
                    }
                }

                pointer.set_location(location);
                // Redraw so a client-drawn cursor follows the pointer.
                self.schedule_redraw();
                self.update_hover_cursor(location);
                self.enforce_capture_overlay_stacking();
                self.maintain_focus_stacking(location);
                self.maybe_clear_bar_edge_hover(location);
                self.maybe_reveal_auto_hidden_bar(location);
                let serial = SERIAL_COUNTER.next_serial();
                let new_under = self.pointer_target_at(location);
                let forward = self.should_forward_pointer_motion(location);
                if forward {
                    pointer.motion(
                        self,
                        new_under.clone(),
                        &MotionEvent {
                            location,
                            serial,
                            time: event.time(),
                        },
                    );
                }
                // Always frame so `relative_motion` flushes even when absolute
                // motion is throttled (desktop GTK hover path).
                pointer.frame(self);

                // Arm a not-yet-active constraint once the pointer enters its
                // region (games commonly request the lock before grabbing focus).
                // Never re-arm a lock the client deactivated for a pause menu —
                // see `maybe_arm_pointer_constraint`.
                if let Some((surface, surface_loc)) = new_under {
                    self.maybe_arm_pointer_constraint(&surface, &pointer, location, surface_loc);
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                // The absolute position is normalized to the whole winit window, so
                // map it across the full virtual desktop (all outputs), not just the
                // first one — otherwise multi-output sessions compress the cursor
                // into the primary output's (now smaller) rect.
                let bounds = self.desktop_bounds();
                let pos = event.position_transformed(bounds.size) + bounds.loc.to_f64();
                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("pointer absolute motion: seat has no pointer");
                    return;
                };
                pointer.set_location(pos);
                // Redraw so a client-drawn cursor follows the pointer.
                self.schedule_redraw();
                self.update_hover_cursor(pos);
                self.enforce_capture_overlay_stacking();
                self.maintain_focus_stacking(pos);
                self.maybe_clear_bar_edge_hover(pos);
                self.maybe_reveal_auto_hidden_bar(pos);
                let serial = SERIAL_COUNTER.next_serial();
                let under = self.pointer_target_at(pos);
                if self.should_forward_pointer_motion(pos) {
                    pointer.motion(
                        self,
                        under,
                        &MotionEvent {
                            location: pos,
                            serial,
                            time: event.time(),
                        },
                    );
                }
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                needs_redraw = true;
                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("pointer button: seat has no pointer");
                    return;
                };
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();
                let raw_loc = pointer.current_location();
                let under = self.pointer_target_at(raw_loc);
                // Mutter/KWin: activate a pending lock before click delivery so a
                // brief unlock cannot inject absolute desktop motion on fire.
                if let Some((surface, surface_loc)) = under.as_ref() {
                    self.maybe_arm_pointer_constraint(surface, &pointer, raw_loc, *surface_loc);
                }
                let loc = raw_loc;
                let pointer_locked = under
                    .as_ref()
                    .is_some_and(|(surface, _)| self.pointer_locked_on_surface(surface, &pointer));

                if let Some((surface, _)) = under.as_ref() {
                    self.sync_pointer_constraint_phase(surface, &pointer);
                }

                if ButtonState::Pressed == button_state
                    && let Some((surface, _)) = under.as_ref()
                {
                    self.trace_game_pointer(surface, &pointer, "pointer button press", Some(loc));
                }

                // Mutter/KWin: no absolute wl_pointer.motion while locked.
                // Do not remap locked clicks through cursor_position_hint — Proton
                // streams hints during mouse-look; remapping them warps the camera.
                if !pointer_locked {
                    pointer.motion(
                        self,
                        under.clone(),
                        &MotionEvent {
                            location: loc,
                            serial,
                            time: event.time(),
                        },
                    );
                }

                if ButtonState::Pressed == button_state {
                    const BTN_LEFT: u32 = 0x110;
                    const BTN_MIDDLE: u32 = 0x112;
                    const BTN_RIGHT: u32 = 0x111;
                    if self.capture_overlay_active() {
                        self.enforce_capture_overlay_stacking();
                    }
                    let paste_button = button == BTN_MIDDLE || button == BTN_RIGHT;
                    // A press over the bar or one of its open popovers (e.g. the app
                    // launcher) belongs to the shell. The bar's popovers don't take a
                    // pointer grab, so without this guard a click would fall through to
                    // window resize/move chrome rendered geometrically *beneath* the
                    // popover — letting you drag a window through the open menu.
                    let on_bar_ui = self.metis_bar_ui_hit(loc);
                    let on_nc = self.metis_notification_center_hit(loc);
                    // Any press outside the Notification Center dismisses it (and
                    // bar popovers) — including presses on the edge bar. Presses
                    // on the NC panel itself must not dismiss. While a capture
                    // picker owns the pointer, no press counts as an outside click:
                    // the shell UI being framed must survive the selection drag.
                    // Task View is sticky (Win11-style): pointer presses must not
                    // broadcast close-popovers, or the shell tears the overlay down
                    // on button-down before click/drag can complete.
                    if !pointer.is_grabbed()
                        && !on_nc
                        && !self.screenshot_overlay_active()
                        && !self.task_view_overlay_active()
                        && !self.capture_overlay_active()
                        && (!on_bar_ui || self.notification_center_mapped())
                    {
                        self.request_close_bar_popovers();
                    }
                    // Terminals (kitty, foot, …) use right/middle-click paste and
                    // context menus against the surface under the pointer — align
                    // clipboard + primary-selection focus before any chrome handler
                    // can short-circuit the press path.
                    if paste_button && !on_bar_ui {
                        self.sync_selection_focus_from_target(&under);
                    }
                    let mut chrome_press = false;
                    if !on_bar_ui
                        && button == BTN_LEFT
                        && !self.capture_overlay_active()
                        && !self.screenshot_overlay_active()
                        && !self.task_view_overlay_active()
                    {
                        chrome_press = self.handle_resize_press(loc, serial, button)
                            || self.handle_decoration_press(loc, serial, button);
                        if chrome_press {
                            self.schedule_redraw();
                        }
                    }
                    if !chrome_press && !self.task_view_overlay_active() {
                        self.update_keyboard_focus(loc, serial);
                        if !paste_button {
                            self.sync_selection_focus_from_target(&under);
                        }
                    }
                } else if button_state == ButtonState::Released && button == 0x110 {
                    self.clear_titlebar_press_pending();
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time(),
                    },
                );
                if let Some((surface, _)) = under
                    && button_state == ButtonState::Pressed
                {
                    self.trace_game_pointer(&surface, &pointer, "after pointer button", Some(loc));
                }
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                needs_redraw = true;
                let source = event.source();
                let mult = self.input_runtime.scroll_multiplier();
                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                }) * mult;
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                }) * mult;

                let mut frame = AxisFrame::new(event.time()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                }

                let Some(pointer) = self.seat.get_pointer() else {
                    tracing::warn!("pointer axis: seat has no pointer");
                    return;
                };
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputEvent::TouchDown { event, .. } => {
                needs_redraw = true;
                self.on_touch_down::<B>(event);
            }
            InputEvent::TouchUp { event, .. } => {
                needs_redraw = true;
                self.on_touch_up::<B>(event);
            }
            InputEvent::TouchMotion { event, .. } => {
                needs_redraw = true;
                self.on_touch_motion::<B>(event);
            }
            InputEvent::TouchFrame { event, .. } => {
                self.on_touch_frame::<B>(event);
            }
            InputEvent::TouchCancel { event, .. } => {
                needs_redraw = true;
                self.on_touch_cancel::<B>(event);
            }
            _ => {}
        }
        if needs_redraw {
            self.schedule_redraw();
        }
    }

    /// Lazily add a `wl_touch` device when the first touchscreen appears.
    pub fn ensure_touch_device(&mut self) {
        if self.seat.get_touch().is_none() {
            self.seat.add_touch();
            tracing::info!("touchscreen detected — wl_touch enabled on seat");
        }
    }

    fn touch_location_transformed<B: InputBackend, E: AbsolutePositionEvent<B>>(
        &self,
        evt: &E,
    ) -> Option<Point<f64, Logical>> {
        let bounds = self.desktop_bounds();
        Some(evt.position_transformed(bounds.size) + bounds.loc.to_f64())
    }

    fn on_touch_down<B: InputBackend>(&mut self, evt: B::TouchDownEvent) {
        self.ensure_touch_device();
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let Some(loc) = self.touch_location_transformed(&evt) else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.update_keyboard_focus(loc, serial);
        let under = self.pointer_target_at(loc);
        handle.down(
            self,
            under,
            &DownEvent {
                slot: evt.slot(),
                location: loc,
                serial,
                time: evt.time(),
            },
        );
    }

    fn on_touch_up<B: InputBackend>(&mut self, evt: B::TouchUpEvent) {
        self.ensure_touch_device();
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        handle.up(
            self,
            &UpEvent {
                slot: evt.slot(),
                serial,
                time: evt.time(),
            },
        );
    }

    fn on_touch_motion<B: InputBackend>(&mut self, evt: B::TouchMotionEvent) {
        self.ensure_touch_device();
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let Some(loc) = self.touch_location_transformed(&evt) else {
            return;
        };
        let under = self.pointer_target_at(loc);
        handle.motion(
            self,
            under,
            &TouchMotionEventWl {
                slot: evt.slot(),
                location: loc,
                time: evt.time(),
            },
        );
    }

    fn on_touch_frame<B: InputBackend>(&mut self, _evt: B::TouchFrameEvent) {
        if let Some(handle) = self.seat.get_touch() {
            handle.frame(self);
        }
    }

    fn on_touch_cancel<B: InputBackend>(&mut self, _evt: B::TouchCancelEvent) {
        if let Some(handle) = self.seat.get_touch() {
            handle.cancel(self);
        }
    }

    fn update_keyboard_focus(&mut self, location: Point<f64, Logical>, serial: Serial) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            tracing::warn!("update_keyboard_focus: seat has no keyboard");
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            tracing::warn!("update_keyboard_focus: seat has no pointer");
            return;
        };

        if pointer.is_grabbed() || keyboard.is_grabbed() {
            return;
        }

        if self.screenshot_overlay_active() {
            self.focus_screenshot_overlay(serial);
            return;
        }

        if self.capture_overlay_active() {
            if let Some(window) = self.top_capture_overlay_window() {
                self.space.raise_element(&window, false);
                if let Some(id) = self.windows.id_for_window(&window) {
                    self.note_window_focus(id);
                }
                keyboard.set_focus(self, Some(window.into()), serial);
            }
            return;
        }

        if let Some(target) = self.focus_target_at(location) {
            if let KeyboardFocusTarget::Window(ref window) = target {
                // Keyboard focus for X11 windows now routes through `X11Surface`,
                // whose `enter` sets X input focus (`XSetInputFocus`) and sends
                // `WM_TAKE_FOCUS`. Re-entering the *same* already-focused window on
                // every pointer click resets the client's in-game UI state: menu
                // items stop opening their dialogs (settings panels never appear),
                // and the game repositions its cursor as if focus changed — the
                // "mouse jumps from the menu on the left to the middle-top where
                // the dialog should be" report during Proton gameplay.
                if self.windows.id_for_window(window) == self.focused_window_id() {
                    return;
                }
                self.space.raise_element(window, true);
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_pending_configure();
                }
                // Tell the shell (taskbar) which window now has focus — focus
                // changes are otherwise only reported as a reply to FocusWindow.
                if let Some(id) = self.windows.id_for_window(window) {
                    self.note_window_focus(id);
                    self.sync_scroll_focus_for_window(id);
                    self.event_bus
                        .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
                }
            }
            keyboard.set_focus(self, Some(target), serial);
            return;
        }

        keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
    }
}
