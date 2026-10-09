//! Classic column layouts (Metis / Whisker / ArcMenu / Mint).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, LayerShell};

use crate::gtk_cb::OptFn0Cell;
use crate::services::applications;

use super::chrome::{
    build_rail, build_user_header, populate_center, populate_pinned, refresh_user_header,
    restore_search_focus, wire_vertical_scroll,
};
use super::pin::PinContext;
use super::register_menu_popover;

pub(crate) fn install_classic(button: &gtk::Button, menu_cfg: &metis_config::MenuConfig) {
    let style = menu_cfg.style;
    let show_rail = menu_cfg.show_rail;
    let show_pinned = menu_cfg.show_pinned && !style.hides_pinned();
    let show_user = menu_cfg.show_user_header || style.prefers_user_header();
    let search_on_top = style.search_on_top();

    let panel = if show_user || search_on_top {
        // Header / top-search styles stack above the rail|list|pinned body.
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        panel.add_css_class("metis-bar-dropdown-panel");
        panel.add_css_class("metis-menu-panel");
        panel.add_css_class(style.css_class());
        panel
    } else {
        // Metis default (and ArcMenu without header): same horizontal root as
        // today's menu — rail | Frequent+search | Pinned.
        let panel = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        panel.add_css_class("metis-bar-dropdown-panel");
        panel.add_css_class("metis-menu-panel");
        panel.add_css_class(style.css_class());
        panel
    };
    let stacked = show_user || search_on_top;

    // The rail's icon tooltips render as a label inside this overlay (part of the
    // menu's own surface) rather than a child popup, so they always paint on top of
    // the panel — a separate popup gets stacked *behind* the translucent menu.
    let overlay = gtk::Overlay::new();
    let tip = gtk::Label::new(None);
    tip.add_css_class("metis-menu-tooltip-label");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);

    let user_header_widgets: Option<(gtk::Image, gtk::Label)> = if show_user {
        let (header, user_avatar, user_name) = build_user_header(menu_cfg);
        panel.append(&header);
        Some((user_avatar, user_name))
    } else {
        None
    };

    let search = gtk::SearchEntry::builder()
        .placeholder_text(metis_i18n::tr("Search applications…"))
        .build();
    search.add_css_class("metis-menu-search");

    if search_on_top {
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_row.add_css_class("metis-menu-search-row");
        search.set_hexpand(true);
        search_row.append(&search);
        panel.append(&search_row);
    }

    // Columns live on `columns` — the horizontal panel itself for Metis default,
    // or a nested body row when a header / top search sits above.
    let columns = if stacked {
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.add_css_class("metis-menu-body");
        body.set_hexpand(true);
        body.set_vexpand(true);
        body
    } else {
        panel.clone()
    };

    if show_rail {
        let rail = build_rail(&overlay, &tip);
        columns.append(&rail);
    }

    // ---- Center column: header + scrollable app list (+ search when bottom) ----
    let center = gtk::Box::new(gtk::Orientation::Vertical, 8);
    center.add_css_class("metis-menu-center");
    center.set_hexpand(true);
    center.set_vexpand(true);

    let header = gtk::Label::builder()
        .label(metis_i18n::tr("Frequent Apps"))
        .halign(gtk::Align::Start)
        .build();
    header.add_css_class("metis-bar-section-title");
    center.append(&header);

    let apps_container = gtk::Box::new(gtk::Orientation::Vertical, 2);
    apps_container.add_css_class("metis-menu-list");
    apps_container.set_hexpand(true);
    apps_container.set_halign(gtk::Align::Fill);
    let apps_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&apps_container)
        .build();
    apps_scroll.add_css_class("metis-menu-scroll");
    // A single Capture-phase controller on the scrolled window intercepts wheel
    // events for its whole subtree (rows + transparent gutters) before the row
    // buttons can swallow them — no per-widget wiring needed.
    wire_vertical_scroll(&apps_scroll, &apps_scroll);
    center.append(&apps_scroll);

    if !search_on_top {
        center.append(&search);
    }

    columns.append(&center);

    // ---- Pinned column (optional) ----
    let pinned_flow = gtk::FlowBox::builder()
        .orientation(gtk::Orientation::Horizontal)
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(3)
        .max_children_per_line(3)
        .homogeneous(true)
        .row_spacing(6)
        .column_spacing(6)
        .build();
    pinned_flow.add_css_class("metis-menu-pinned-flow");
    pinned_flow.set_valign(gtk::Align::Start);

    let pinned_hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Right-click an app and choose Pin to Start.",
    )));
    pinned_hint.add_css_class("metis-menu-empty");
    pinned_hint.set_wrap(true);
    pinned_hint.set_halign(gtk::Align::Start);
    pinned_hint.set_valign(gtk::Align::Start);
    pinned_hint.set_xalign(0.0);
    pinned_hint.set_visible(false);

    if show_pinned {
        let divider = gtk::Separator::new(gtk::Orientation::Vertical);
        divider.add_css_class("metis-menu-divider");
        columns.append(&divider);

        let pinned_col = gtk::Box::new(gtk::Orientation::Vertical, 8);
        pinned_col.add_css_class("metis-menu-pinned");
        let pinned_header = gtk::Label::builder()
            .label(metis_i18n::tr("Pinned"))
            .halign(gtk::Align::Start)
            .build();
        pinned_header.add_css_class("metis-bar-section-title");
        pinned_col.append(&pinned_header);
        pinned_col.append(&pinned_hint);

        let pinned_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pinned_wrap.set_valign(gtk::Align::Start);
        pinned_wrap.append(&pinned_flow);
        let pinned_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&pinned_wrap)
            .build();
        pinned_scroll.add_css_class("metis-menu-scroll");
        wire_vertical_scroll(&pinned_scroll, &pinned_scroll);
        pinned_col.append(&pinned_scroll);
        columns.append(&pinned_col);
    }

    if stacked {
        panel.append(&columns);
    }

    overlay.set_child(Some(&panel));
    overlay.add_overlay(&tip);
    let pin_ctx = PinContext::new(&overlay);
    // Primary click outside the pin sheet dismisses it without stealing the
    // click when the target is the Pin/Unpin button itself.
    {
        let pin_ctx = pin_ctx.clone();
        let overlay_for_pick = overlay.clone();
        let dismiss = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        dismiss.connect_pressed(move |_, _, x, y| {
            if !pin_ctx.panel.is_visible() {
                return;
            }
            if let Some(target) = overlay_for_pick.pick(x, y, gtk::PickFlags::DEFAULT) {
                let mut node = Some(target);
                while let Some(w) = node {
                    if w == pin_ctx.panel {
                        return;
                    }
                    node = w.parent();
                }
            }
            pin_ctx.dismiss();
        });
        overlay.add_controller(dismiss);
    }

    // ---- Rebuild plumbing ----
    // A shared `refresh` handle lets row/tile context actions (pin/unpin) trigger
    // a full repopulate. It dispatches through a slot so it can reference the
    // rebuild closure that is defined just below it.
    let rebuild_slot: OptFn0Cell = Rc::new(RefCell::new(None));
    let refresh: Rc<dyn Fn()> = {
        let slot = rebuild_slot.clone();
        Rc::new(move || {
            let f = slot.borrow().clone();
            if let Some(f) = f {
                f();
            }
        })
    };
    pin_ctx.set_refresh(refresh.clone());

    let list_generation = Rc::new(Cell::new(0_u64));
    let search_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let rebuild: Rc<dyn Fn()> = {
        let apps_container = apps_container.clone();
        let pinned_flow = pinned_flow.clone();
        let pinned_hint = pinned_hint.clone();
        let header = header.clone();
        let search = search.clone();
        let list_generation = list_generation.clone();
        let pin_ctx = pin_ctx.clone();
        Rc::new(move || {
            let keep_search_focus = search.has_focus();
            let query = search.text().to_string();
            let apps = applications::list_apps();
            pin_ctx.dismiss();
            pin_ctx.clear_targets();
            populate_center(
                &apps_container,
                &header,
                &query,
                &apps,
                &search,
                &list_generation,
                &pin_ctx,
            );
            // Always refresh pins — including during search — so a right-click
            // Pin action is visible immediately in the pinned column.
            if show_pinned {
                populate_pinned(&pinned_flow, &pinned_hint, &apps, &pin_ctx);
            }
            restore_search_focus(&search, keep_search_focus);
        })
    };
    *rebuild_slot.borrow_mut() = Some(rebuild.clone());
    applications::register_refresh(rebuild.clone());

    let search_changed = {
        let rebuild = rebuild.clone();
        let search_debounce = search_debounce.clone();
        search.connect_search_changed(move |_| {
            if let Some(id) = search_debounce.borrow_mut().take() {
                id.remove();
            }
            let rebuild = rebuild.clone();
            let debounce_slot = search_debounce.clone();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
                *debounce_slot.borrow_mut() = None;
                rebuild();
                glib::ControlFlow::Break
            });
            *search_debounce.borrow_mut() = Some(id);
        })
    };

    // ---- Popover (non-autohide; mirrors dropdown::wire_toggle) ----
    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&overlay)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.add_css_class("metis-menu-popover");
    popover.set_parent(button);
    register_menu_popover(&popover);

    // Type-to-search without forcing focus on open: key capture routes typing
    // anywhere in the popover to the search entry (which only grabs focus once you
    // start typing). Scroll is positional, not focus-based, so this never steals
    // wheel events from the app list.
    search.set_key_capture_widget(Some(&popover));

    {
        let btn = button.clone();
        let rebuild = rebuild.clone();
        let search = search.clone();
        let user_header_widgets = user_header_widgets.clone();
        popover.connect_map(move |popover| {
            btn.add_css_class("metis-bar-dropdown-active");
            if let Some(window) = popover.root().and_downcast::<gtk::Window>() {
                window.set_keyboard_mode(KeyboardMode::Exclusive);
            }
            // Refresh profile header from disk each open (Settings may have
            // changed ~/.face / menu.json without remounting the bar).
            if let Some((avatar, name)) = user_header_widgets.as_ref() {
                refresh_user_header(avatar, name);
            }
            // Clearing the search entry fires `search_changed`, which would
            // synchronously rebuild the entire app list during `map` and freeze
            // the nested session — block it and defer one rebuild on idle instead.
            search.block_signal(&search_changed);
            search.set_text("");
            search.unblock_signal(&search_changed);
            applications::invalidate_app_cache();
            let rebuild = rebuild.clone();
            glib::idle_add_local_once(move || rebuild());
            // Super-key opens never get a pointer click to claim OnDemand focus;
            // Exclusive is set above and the compositor routes keys to this layer.
            // Grab (and re-grab once after a tick) so SearchEntry is the GTK focus
            // target as soon as the layer owns the seat.
            search.grab_focus();
            let search_again = search.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                if !search_again.has_focus() {
                    search_again.grab_focus();
                }
            });
        });
    }
    {
        let btn = button.clone();
        let pin_ctx = pin_ctx.clone();
        popover.connect_unmap(move |popover| {
            pin_ctx.dismiss();
            btn.remove_css_class("metis-bar-dropdown-active");
            if let Some(window) = popover.root().and_downcast::<gtk::Window>() {
                window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
        });
    }

    crate::ui::bar::dropdown::register(&popover);

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            crate::ui::bar::dropdown::clear_trigger_highlight(&popover);
            glib::idle_add_local_once(move || popover.popdown());
            return;
        }
        crate::ui::bar::dropdown::close_all();
        glib::idle_add_local_once(move || popover.popup());
    });
}
