//! Dashboard public lifecycle hooks and bar-pull wiring.

use std::rc::Rc;
use std::sync::mpsc::Receiver;

use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, LayerShell};
use metis_config::{BarPosition, load_bar_config, load_dashboard_config};

use crate::services::DashboardSnapshot;
use crate::ui::bar::{BarShell, ensure_bar_strip_geometry};

use super::panel::{build_dashboard, compute_max_extent};
use super::{DASHBOARD, Dashboard, OPEN_THRESHOLD, POLL_ATTACHED, PULL_START_SLOP};

pub fn init() {
    if let Err(err) = metis_config::save_default_dashboard_config() {
        tracing::warn!(%err, "failed to write default dashboard.json");
    }
}

/// Press on the bar pill and drag toward the desktop to pull the dashboard open.
pub fn wire_bar_pull(pill: &gtk::Box, shell: &BarShell) {
    let pill_weak = pill.downgrade();
    let _shell_pull = shell.clone();

    let drag = gtk::GestureDrag::new();
    drag.set_button(0);
    drag.set_touch_only(false);

    drag.connect_drag_begin(move |gesture, start_x, start_y| {
        if !load_dashboard_config().enabled {
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        let Some(pill) = pill_weak.upgrade() else {
            return;
        };
        if press_on_bar_widget(&pill, start_x, start_y) {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });

    let shell_update = shell.clone();
    drag.connect_drag_update(move |gesture, offset_x, offset_y| {
        if !load_dashboard_config().enabled {
            return;
        }
        let position = load_bar_config().position;
        let delta = pull_delta(position, offset_x, offset_y);
        if delta < PULL_START_SLOP {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let dash = ensure_dashboard(&shell_update);
        if dash.open.get() && !dash.pulling.get() {
            return;
        }
        if !dash.pulling.get() {
            super::dropdown::request_close_all();
            dash.pulling.set(true);
            dash.max_extent.set(compute_max_extent(
                position,
                dash.shell.window.monitor().as_ref(),
            ));
        }
        let max = dash.max_extent.get().max(1);
        let extent = (delta as i32).clamp(0, max);
        dash.set_pull_preview(extent);
    });

    let shell_end = shell.clone();
    drag.connect_drag_end(move |_, offset_x, offset_y| {
        let position = load_bar_config().position;
        let delta = pull_delta(position, offset_x, offset_y);
        let Some(dash) = DASHBOARD.with(|d| d.borrow().clone()) else {
            return;
        };
        dash.pulling.set(false);
        if dash.open.get() {
            return;
        }
        let monitor = shell_end.window.monitor();
        let max = compute_max_extent(position, monitor.as_ref());
        dash.max_extent.set(max);
        if delta >= OPEN_THRESHOLD {
            dash.snap_open();
        } else {
            dash.snap_closed();
        }
    });

    pill.add_controller(drag);
}

pub fn on_theme_changed() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.header.queue_draw();
            dash.tab_switcher.queue_draw();
            dash.redraw_for_theme();
        }
    });
}

pub fn on_bar_config_changed() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.relayout_for_bar();
            if !dash.open.get() && !dash.pulling.get() {
                dash.set_closed_state();
            } else if dash.open.get() {
                dash.max_extent.set(compute_max_extent(
                    load_bar_config().position,
                    dash.shell.window.monitor().as_ref(),
                ));
                dash.apply_extent(dash.max_extent.get());
            }
        }
    });
}

pub fn on_dashboard_config_changed() {
    crate::ui::bar::sync_control_center_button();
    crate::ui::bar::refresh_workspaces();

    if !load_dashboard_config().enabled {
        request_close();
    }

    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.apply_widget_config();
            let position = load_bar_config().position;
            let max = compute_max_extent(position, dash.shell.window.monitor().as_ref());
            dash.max_extent.set(max);
            if dash.open.get() {
                dash.apply_extent(max);
            } else if !dash.pulling.get() {
                dash.set_closed_state();
            }
        }
    });
}

/// Drop a built Control Center so the next open uses freshly translated chrome.
pub fn reload_for_locale() {
    let was_open = DASHBOARD.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|dash| dash.open.get() || dash.current_extent.get() > 0)
    });
    teardown_dashboard();
    if was_open {
        // Reopen requires a bar shell; next toggle / pull rebuilds lazily.
        tracing::debug!("control center torn down for locale; reopen from the edge bar");
    }
}

pub fn attach_poll_channel(rx: Receiver<DashboardSnapshot>) {
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        while let Ok(snapshot) = rx.try_recv() {
            apply_snapshot(&snapshot);
        }
        glib::ControlFlow::Continue
    });
}

pub fn request_close() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.snap_closed();
        }
    });
}

/// True while the control center panel is open (or mid-open animation).
pub fn is_open() -> bool {
    DASHBOARD.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|dash| dash.open.get() || dash.window.is_visible())
    })
}

/// Demote Exclusive keyboard while the screenshot Overlay owns input so Control
/// Center stays visible for capture without covering Esc / type-ahead.
pub fn set_below_screenshot(below: bool) {
    DASHBOARD.with(|d| {
        let borrow = d.borrow();
        let Some(dash) = borrow.as_ref() else {
            return;
        };
        if !dash.window.is_visible() && !dash.open.get() {
            return;
        }
        dash.window.set_keyboard_mode(if below {
            KeyboardMode::OnDemand
        } else {
            KeyboardMode::Exclusive
        });
    });
}

/// Open or close the control center with the same slide animation as a bar pull.
pub fn request_toggle(shell: &BarShell) {
    if !load_dashboard_config().enabled {
        return;
    }
    crate::ui::bar::notify_bar_interaction();
    let dash = ensure_dashboard(shell);
    if dash.open.get() || dash.current_extent.get() > OPEN_THRESHOLD as i32 / 2 {
        dash.snap_closed();
    } else {
        dash.snap_open();
    }
}

pub(crate) fn ensure_dashboard(shell: &BarShell) -> Rc<Dashboard> {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            if dash.shell.window != shell.window {
                teardown_dashboard();
            } else {
                return dash.clone();
            }
        }
        tracing::debug!("lazy-starting system dashboard");
        let dash = Rc::new(build_dashboard(shell));
        dash.set_closed_state();
        *d.borrow_mut() = Some(dash.clone());
        start_polling();
        dash
    })
}

pub(crate) fn start_polling() {
    if POLL_ATTACHED.get() {
        crate::services::set_polling_active(true);
        return;
    }
    POLL_ATTACHED.set(true);
    attach_poll_channel(crate::services::spawn_dashboard_pollers());
}

pub(crate) fn teardown_dashboard() {
    crate::services::set_polling_active(false);
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow_mut().take() {
            dash.set_closed_state();
            dash.window.set_visible(false);
            dash.window.destroy();
            ensure_bar_strip_geometry(&dash.shell);
        }
    });
    tracing::debug!("system dashboard torn down (idle)");
}

pub(crate) fn apply_snapshot(snapshot: &DashboardSnapshot) {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.update(snapshot);
        }
    });
}

pub(crate) fn press_on_bar_widget(pill: &gtk::Box, x: f64, y: f64) -> bool {
    let Some(target) = pill.pick(x, y, gtk::PickFlags::DEFAULT) else {
        return false;
    };
    let mut node = Some(target);
    while let Some(w) = node {
        if w.has_css_class("metis-bar-widget") {
            return true;
        }
        node = w.parent();
    }
    false
}

pub(crate) fn pull_delta(position: BarPosition, offset_x: f64, offset_y: f64) -> f64 {
    match position {
        BarPosition::Top => offset_y,
        BarPosition::Bottom => -offset_y,
        BarPosition::Left => offset_x,
        BarPosition::Right => -offset_x,
    }
}

pub(crate) fn close_delta(position: BarPosition, offset_x: f64, offset_y: f64) -> f64 {
    match position {
        BarPosition::Top => -offset_y,
        BarPosition::Bottom => offset_y,
        BarPosition::Left => -offset_x,
        BarPosition::Right => offset_x,
    }
}
