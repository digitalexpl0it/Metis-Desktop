use std::cell::RefCell;

use gtk::prelude::*;

thread_local! {
    static POPOVERS: RefCell<Vec<gtk::Popover>> = const { RefCell::new(Vec::new()) };
}

/// Styled panel container for popover content.
pub fn build_panel() -> gtk::Box {
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    panel.add_css_class("metis-bar-dropdown-panel");
    panel
}

/// Single-click toggle using a GTK popover (no layer-shell resize).
pub fn wire_toggle(button: &gtk::Button, panel: &gtk::Box, _key: &str) {
    wire_toggle_prepare(button, panel, || {});
}

/// Run `prepare` before opening. Returns the popover so callers can track map/unmap
/// (e.g. skip heavy clipboard history rebuilds while the panel is closed).
pub fn wire_toggle_prepare(
    button: &gtk::Button,
    panel: &gtk::Box,
    prepare: impl Fn() + 'static,
) -> gtk::Popover {
    // NOTE: `autohide` popovers request an xdg_popup grab. Our compositor
    // intentionally ignores popup grabs (they hang GTK clients), and the bar is
    // a `KeyboardMode::None` layer surface, so an autohide popover can never
    // establish its grab and silently fails to present. A non-autohide popover
    // needs no grab; we dismiss it via toggle + the compositor "close-popovers"
    // signal when the pointer hits bare desktop.
    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::popover_position())
        .child(panel)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(button);

    // Highlight the triggering icon only while its popover is open; clear it the
    // moment the popover closes, regardless of pointer position.
    {
        let btn = button.clone();
        popover.connect_map(move |_| {
            btn.add_css_class("metis-bar-dropdown-active");
        });
    }
    wire_clear_on_dismiss(&popover);

    POPOVERS.with(|list| list.borrow_mut().push(popover.clone()));

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            clear_trigger_highlight(&popover);
            glib::idle_add_local_once(move || {
                popover.popdown();
            });
            return;
        }
        // Single-open behavior: close any other popover before opening this one,
        // so clicking a different bar icon switches instead of stacking.
        close_all();
        crate::ui::notification_center::dismiss();
        prepare();
        // Defer popup so we are not inside the compositor's pointer-dispatch stack.
        glib::idle_add_local_once(move || {
            popover.popup();
            crate::ui::bar::notify_bar_interaction();
        });
    });
    popover
}

/// Register an externally-managed popover (e.g. Metis Menu) so the compositor
/// "close-popovers" signal and single-open logic can still pop it down.
pub fn register(popover: &gtk::Popover) {
    wire_clear_on_dismiss(popover);
    POPOVERS.with(|list| list.borrow_mut().push(popover.clone()));
}

pub fn close_all() {
    // Clone the list first — `popdown` can re-enter via closed/unmap handlers
    // (process context menu, bar toggles) that also touch POPOVERS.
    let popovers = POPOVERS.with(|list| {
        let mut list = list.borrow_mut();
        list.retain(|p| p.parent().is_some());
        list.clone()
    });
    for popover in &popovers {
        // Clear the cyan "open" chrome *before* popdown. Launching an app from
        // inside the Metis Menu steals focus mid-dismiss; GTK can skip / delay
        // unmap, which left the launcher button stuck highlighted until the
        // next bar click.
        clear_trigger_highlight(popover);
        popover.popdown();
    }
}

pub fn is_open() -> bool {
    POPOVERS.with(|list| list.borrow().iter().any(|p| p.is_visible()))
}

pub fn request_close_all() {
    if !is_open() {
        return;
    }
    glib::idle_add_local_once(close_all);
}

/// Drop `metis-bar-dropdown-active` (+ pressed/focus flags) on the popover's
/// parent bar button. Safe to call repeatedly.
pub fn clear_trigger_highlight(popover: &gtk::Popover) {
    let Some(parent) = popover.parent() else {
        return;
    };
    parent.remove_css_class("metis-bar-dropdown-active");
    parent.unset_state_flags(
        gtk::StateFlags::ACTIVE
            | gtk::StateFlags::CHECKED
            | gtk::StateFlags::FOCUSED
            | gtk::StateFlags::FOCUS_WITHIN
            | gtk::StateFlags::PRELIGHT,
    );
}

fn wire_clear_on_dismiss(popover: &gtk::Popover) {
    // `closed` fires even when unmap is delayed (common when an app launch
    // steals focus during popdown from a child click).
    popover.connect_closed(|popover| {
        clear_trigger_highlight(popover);
    });
    popover.connect_unmap(|popover| {
        clear_trigger_highlight(popover);
    });
}
