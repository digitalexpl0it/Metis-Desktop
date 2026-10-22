//! Remote-desktop input injection (gnome-remote-desktop / libei / RUDP → seat).
//!
//! Button presses share the local libinput path so SSD titlebar drag, traffic-
//! light buttons, resize edges, and keyboard focus activation work over Metis
//! Remote — not only `wl_pointer.button` to the surface under the cursor.

use smithay::backend::input::{ButtonState, InputTime, KeyState};
use smithay::input::pointer::{ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::utils::SERIAL_COUNTER;
use std::sync::atomic::Ordering;

use crate::state::MetisState;

/// GTK discrete wheel steps are small integers (±1…±4). Continuous trackpad
/// deltas are fractional or larger.
fn is_discrete_scroll_notch(v: f64) -> bool {
    if v == 0.0 {
        return true;
    }
    let a = v.abs();
    a <= 4.0 && (a - a.round()).abs() < 1e-6
}

fn scroll_axis_amounts(delta: f64, discrete: bool) -> (f64, i32) {
    if discrete {
        let steps = delta.round() as i32;
        let v120 = steps.saturating_mul(120);
        (f64::from(steps) * 15.0, v120)
    } else {
        // Treat Viewer continuous deltas as roughly pixel-ish; map to v120.
        let v120 = (delta * 8.0).round() as i32;
        (delta, v120)
    }
}

impl MetisState {
    pub fn inject_remote_pointer_relative(&mut self, dx: f64, dy: f64) {
        if self.session_is_locked() {
            return;
        }
        self.idle_notify_activity();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        use smithay::utils::Point;

        // When a Wayland pointer lock is active (game/CAD), emit relative
        // motion only — same as local libinput under `PointerConstraint::Locked`.
        if self.rudp_pointer_locked.load(Ordering::Relaxed) {
            let current = pointer.current_location();
            let under = self.pointer_target_at(current);
            let delta = Point::from((dx, dy));
            pointer.relative_motion(
                self,
                under,
                &RelativeMotionEvent {
                    delta,
                    delta_unaccel: delta,
                    time: InputTime::now(),
                },
            );
            pointer.frame(self);
            return;
        }

        let current = pointer.current_location();
        let loc = self.clamp_to_desktop(current + Point::from((dx, dy)));
        self.inject_remote_pointer_at(loc);
    }

    pub fn inject_remote_pointer_absolute(&mut self, x: f64, y: f64) {
        if self.session_is_locked() {
            return;
        }
        use smithay::utils::Point;
        let loc = self.clamp_to_desktop(Point::from((x, y)));
        self.inject_remote_pointer_at(loc);
    }

    fn inject_remote_pointer_at(
        &mut self,
        loc: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        self.idle_notify_activity();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.set_location(loc);
        self.schedule_redraw();
        self.update_hover_cursor(loc);
        // Always forward remote absolute/relative inject — the local GTK hover
        // throttle (48ms) must not double-lag RUDP/RDP. Network already limits
        // rate; grabs need every sample for titlebar-drag thresholds.
        let serial = SERIAL_COUNTER.next_serial();
        let under = self.pointer_target_at(loc);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: loc,
                serial,
                time: InputTime::now(),
            },
        );
        pointer.frame(self);
    }

    /// Shared pointer button path for local libinput and remote inject.
    ///
    /// Runs SSD chrome (titlebar move / buttons / resize), shell popover
    /// dismiss, and keyboard/selection focus before emitting `wl_pointer.button`.
    pub(crate) fn dispatch_pointer_button(
        &mut self,
        button: u32,
        button_state: ButtonState,
        time: InputTime,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            tracing::warn!("pointer button: seat has no pointer");
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
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
        if !pointer_locked {
            pointer.motion(
                self,
                under.clone(),
                &MotionEvent {
                    location: loc,
                    serial,
                    time,
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
            let on_bar_ui = self.metis_bar_ui_hit(loc);
            let on_nc = self.metis_notification_center_hit(loc);
            if crate::desk_input::should_close_shell_popovers_on_press(
                pointer.is_grabbed(),
                on_nc,
                self.screenshot_overlay_active(),
                self.task_view_overlay_active(),
                self.capture_overlay_active(),
                on_bar_ui,
                self.notification_center_mapped(),
            ) {
                self.request_close_shell_popovers(Some(loc));
            }
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
                time,
            },
        );
        if let Some((surface, _)) = under
            && button_state == ButtonState::Pressed
        {
            self.trace_game_pointer(&surface, &pointer, "after pointer button", Some(loc));
        }
        pointer.frame(self);
    }

    pub fn inject_remote_pointer_button(&mut self, button: u32, pressed: bool) {
        if self.session_is_locked() {
            return;
        }
        self.idle_notify_activity();
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        self.dispatch_pointer_button(button, state, InputTime::now());
        self.schedule_redraw();
    }

    pub fn inject_remote_pointer_scroll(&mut self, dx: f64, dy: f64) {
        if self.session_is_locked() {
            return;
        }
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.idle_notify_activity();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        use smithay::backend::input::{Axis, AxisSource};
        use smithay::input::pointer::AxisFrame;

        // Viewer used to send AxisSource::Finger without `.stop()`, which many
        // clients ignore. Prefer Wheel + v120 (mouse notches) or Finger+stop
        // (trackpad continuous). Match local libinput scale: value ≈ v120*15/120.
        let discrete = is_discrete_scroll_notch(dx) && is_discrete_scroll_notch(dy);
        let mut frame = if discrete {
            AxisFrame::new(InputTime::now()).source(AxisSource::Wheel)
        } else {
            AxisFrame::new(InputTime::now()).source(AxisSource::Finger)
        };
        if dx != 0.0 {
            let (value, v120) = scroll_axis_amounts(dx, discrete);
            frame = frame
                .value(Axis::Horizontal, value)
                .v120(Axis::Horizontal, v120);
            if !discrete {
                frame = frame.stop(Axis::Horizontal);
            }
        }
        if dy != 0.0 {
            let (value, v120) = scroll_axis_amounts(dy, discrete);
            frame = frame
                .value(Axis::Vertical, value)
                .v120(Axis::Vertical, v120);
            if !discrete {
                frame = frame.stop(Axis::Vertical);
            }
        }
        let loc = pointer.current_location();
        let under = self.pointer_target_at(loc);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: loc,
                serial: SERIAL_COUNTER.next_serial(),
                time: InputTime::now(),
            },
        );
        pointer.axis(self, frame);
        pointer.frame(self);
        self.schedule_redraw();
    }

    pub fn inject_remote_key(&mut self, keycode: u32, pressed: bool) {
        if self.session_is_locked() {
            return;
        }
        self.idle_notify_activity();
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        let serial = SERIAL_COUNTER.next_serial();
        use smithay::backend::input::Keycode;
        // Wire format is Linux evdev (`KEY_*`). Smithay/xkb expect XKB keycodes
        // (evdev + 8), same as the libinput backend: `(self.key() + 8).into()`.
        let xkb = Keycode::new(keycode.saturating_add(8));
        keyboard.input::<(), _>(self, xkb, state, serial, InputTime::now(), |_, _, _| {
            smithay::input::keyboard::FilterResult::Forward
        });
        self.schedule_redraw();
    }
}
