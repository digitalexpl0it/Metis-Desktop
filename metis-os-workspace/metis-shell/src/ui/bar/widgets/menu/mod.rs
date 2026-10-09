//! The Metis app-menu popover: selectable layouts (Metis default, Whisker, ArcMenu,
//! Mint, plus the Bracket / Ledger / Mosaic / Ramp / Ladder / Plaza / Crest / Chip
//! pack) sharing rails, search, lists, grids, and a Pinned column where relevant.
//!
//! It reuses the bar's non-autohide popover scheme (see `dropdown.rs`): no popup
//! grab (the compositor ignores those), dismissed via toggle and the compositor
//! "close-popovers" signal. The alphabetical app list is filled incrementally on
//! idle; that must not steal keyboard focus from the search entry while typing.

mod chrome;
mod classic;
mod pack;
mod pin;

use std::cell::RefCell;

use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, LayerShell};

thread_local! {
    /// Menu instances for every output. Weak handles avoid keeping bars alive
    /// across a live bar rebuild.
    static MENU_POPOVERS: RefCell<Vec<glib::WeakRef<gtk::Popover>>> =
        const { RefCell::new(Vec::new()) };
}

pub(crate) fn register_menu_popover(popover: &gtk::Popover) {
    MENU_POPOVERS.with(|menus| menus.borrow_mut().push(popover.downgrade()));
}

/// Toggle the first live Metis menu. Used by the compositor's standalone Super
/// shortcut; pointer-opened menus use the same popover and focus behavior.
pub(crate) fn request_toggle() {
    let target = MENU_POPOVERS.with(|menus| {
        let mut menus = menus.borrow_mut();
        menus.retain(|weak| weak.upgrade().is_some());
        let live: Vec<gtk::Popover> = menus.iter().filter_map(glib::WeakRef::upgrade).collect();
        live.iter()
            .find(|popover| popover.is_visible())
            .cloned()
            .or_else(|| live.into_iter().next())
    });
    let Some(popover) = target else {
        tracing::debug!("menu toggle requested before a bar menu was available");
        return;
    };
    if popover.is_visible() {
        crate::ui::bar::dropdown::clear_trigger_highlight(&popover);
        glib::idle_add_local_once(move || popover.popdown());
    } else {
        crate::ui::bar::dropdown::close_all();
        glib::idle_add_local_once(move || popover.popup());
    }
}

/// Demote the bar's Exclusive keyboard while the screenshot Overlay is up so
/// Esc reaches the picker; restore Exclusive if the menu is still open after.
pub(crate) fn set_below_screenshot(below: bool) {
    MENU_POPOVERS.with(|menus| {
        for weak in menus.borrow().iter() {
            let Some(popover) = weak.upgrade() else {
                continue;
            };
            if !popover.is_visible() {
                continue;
            }
            let Some(window) = popover.root().and_downcast::<gtk::Window>() else {
                continue;
            };
            window.set_keyboard_mode(if below {
                KeyboardMode::OnDemand
            } else {
                KeyboardMode::Exclusive
            });
        }
    });
}

/// Build the menu popover and wire it to `button` (the brand launcher button).
pub fn install(button: &gtk::Button) {
    let menu_cfg = metis_config::load_menu_config();
    if !menu_cfg.style.classic_columns() {
        pack::install_pack_layout(button, &menu_cfg);
        return;
    }
    classic::install_classic(button, &menu_cfg);
}
