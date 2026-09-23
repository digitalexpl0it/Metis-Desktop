//! Remote-desktop input injection (gnome-remote-desktop / libei → compositor seat).

use smithay::backend::input::{ButtonState, InputTime, KeyState};
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::state::MetisState;

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
        if !self.should_forward_pointer_motion(loc) {
            return;
        }
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

    pub fn inject_remote_pointer_button(&mut self, button: u32, pressed: bool) {
        if self.session_is_locked() {
            return;
        }
        self.idle_notify_activity();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let loc = pointer.current_location();
        self.sync_selection_focus_at(loc);
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
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
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time: InputTime::now(),
            },
        );
        pointer.frame(self);
        self.schedule_redraw();
    }

    pub fn inject_remote_pointer_scroll(&mut self, dx: f64, dy: f64) {
        if self.session_is_locked() {
            return;
        }
        self.idle_notify_activity();
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        use smithay::backend::input::{Axis, AxisSource};
        use smithay::input::pointer::AxisFrame;
        let mut frame = AxisFrame::new(InputTime::now()).source(AxisSource::Finger);
        if dx != 0.0 {
            frame = frame.value(Axis::Horizontal, dx);
        }
        if dy != 0.0 {
            frame = frame.value(Axis::Vertical, dy);
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
        keyboard.input::<(), _>(
            self,
            Keycode::new(keycode),
            state,
            serial,
            InputTime::now(),
            |_, _, _| smithay::input::keyboard::FilterResult::Forward,
        );
        self.schedule_redraw();
    }
}
