//! The Metis app-menu popover: selectable layouts (Metis default, Whisker, ArcMenu,
//! Mint, plus the Bracket / Ledger / Mosaic / Ramp / Ladder / Plaza / Crest / Chip
//! pack) sharing rails, search, lists, grids, and a Pinned column where relevant.
//!
//! It reuses the bar's non-autohide popover scheme (see `dropdown.rs`): no popup
//! grab (the compositor ignores those), dismissed via toggle and the compositor
//! "close-popovers" signal. The alphabetical app list is filled incrementally on
//! idle; that must not steal keyboard focus from the search entry while typing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, LayerShell};

use crate::gtk_cb::OptFn0Cell;
use crate::services::{applications, AppEntry};

const APP_ICON_SIZE: i32 = 24;
const PIN_ICON_SIZE: i32 = 34;
const GRID_ICON_SIZE: i32 = 40;
const CHIP_ICON_SIZE: i32 = 28;
const FREQUENT_LIMIT: usize = 8;
/// Alphabetical rows appended per idle slice so opening the menu never blocks the
/// GTK main loop (and the nested compositor Wayland socket) for one giant rebuild.
const MENU_ALPHA_CHUNK: usize = 32;
const PIN_SHEET_WIDTH: i32 = 180;

thread_local! {
    /// Menu instances for every output. Weak handles avoid keeping bars alive
    /// across a live bar rebuild.
    static MENU_POPOVERS: RefCell<Vec<glib::WeakRef<gtk::Popover>>> =
        const { RefCell::new(Vec::new()) };
}

/// Pin/Unpin menu drawn inside the Start menu's `GtkOverlay` (same surface).
/// A nested `GtkPopover` would be a separate Wayland surface the compositor
/// stacks *behind* the translucent menu — same pitfall as rail tooltips.
///
/// Right-clicks are handled on the overlay itself so (x, y) are already in
/// overlay space. Per-row `translate_coordinates` failed for FlowBox tiles and
/// for the second open (stale margins made the clamp snap to top-left).
struct PinContext {
    overlay: gtk::Overlay,
    panel: gtk::Box,
    /// Live row/tile widgets → desktop id, for overlay `pick` → pin target.
    targets: RefCell<HashMap<usize, String>>,
    refresh: RefCell<Option<Rc<dyn Fn()>>>,
}

impl PinContext {
    fn new(overlay: &gtk::Overlay) -> Rc<Self> {
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
        panel.add_css_class("metis-bar-dropdown-panel");
        panel.add_css_class("metis-bar-tasks-menu");
        panel.add_css_class("metis-menu-pin-context");
        panel.set_halign(gtk::Align::Start);
        panel.set_valign(gtk::Align::Start);
        panel.set_width_request(PIN_SHEET_WIDTH);
        panel.set_visible(false);
        overlay.add_overlay(&panel);

        let ctx = Rc::new(Self {
            overlay: overlay.clone(),
            panel,
            targets: RefCell::new(HashMap::new()),
            refresh: RefCell::new(None),
        });

        // Secondary click anywhere in the menu: resolve the app under the
        // pointer (picking through the pin sheet) and open at cursor coords.
        {
            let ctx = ctx.clone();
            let secondary = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .propagation_phase(gtk::PropagationPhase::Capture)
                .build();
            secondary.connect_pressed(move |gesture, n_press, x, y| {
                if n_press != 1 {
                    return;
                }
                let Some(id) = ctx.pick_app_id(x, y) else {
                    return;
                };
                gesture.set_state(gtk::EventSequenceState::Claimed);
                ctx.show(&id, x, y);
            });
            overlay.add_controller(secondary);
        }

        ctx
    }

    fn set_refresh(&self, refresh: Rc<dyn Fn()>) {
        *self.refresh.borrow_mut() = Some(refresh);
    }

    fn clear_targets(&self) {
        self.targets.borrow_mut().clear();
    }

    fn register_target(&self, widget: &impl IsA<gtk::Widget>, id: &str) {
        let widget = widget.as_ref();
        self.targets
            .borrow_mut()
            .insert(widget.as_ptr() as usize, id.to_string());
    }

    fn pick_app_id(&self, x: f64, y: f64) -> Option<String> {
        // Don't let the pin sheet steal the pick when re-opening over another app.
        let was_targetable = self.panel.can_target();
        self.panel.set_can_target(false);
        let target = self.overlay.pick(x, y, gtk::PickFlags::DEFAULT);
        self.panel.set_can_target(was_targetable);

        let mut node = target;
        while let Some(w) = node {
            if let Some(id) = self.targets.borrow().get(&(w.as_ptr() as usize)).cloned() {
                return Some(id);
            }
            node = w.parent();
        }
        None
    }

    fn dismiss(&self) {
        // Clear margins before hide — leftover offsets inflate measure and make
        // the next clamp snap to (0, 0).
        self.panel.set_margin_start(0);
        self.panel.set_margin_top(0);
        while let Some(child) = self.panel.first_child() {
            self.panel.remove(&child);
        }
        self.panel.set_visible(false);
    }

    fn show(&self, id: &str, ox: f64, oy: f64) {
        self.dismiss();

        let already_pinned = metis_config::load_menu_config()
            .pinned
            .iter()
            .any(|p| p == id);
        let label = if already_pinned {
            metis_i18n::tr("Unpin from Start")
        } else {
            metis_i18n::tr("Pin to Start")
        };

        let item = gtk::Button::builder()
            .label(&label)
            .has_frame(false)
            .build();
        item.add_css_class("metis-bar-task-menu-item");
        item.set_halign(gtk::Align::Fill);
        if let Some(child) = item.child() {
            if let Ok(lbl) = child.downcast::<gtk::Label>() {
                lbl.set_halign(gtk::Align::Start);
                lbl.set_xalign(0.0);
            }
        }
        self.panel.append(&item);

        let id = id.to_string();
        let refresh = self.refresh.borrow().clone();
        let panel = self.panel.clone();
        item.connect_clicked(move |_| {
            applications::toggle_pin(&id);
            while let Some(child) = panel.first_child() {
                panel.remove(&child);
            }
            panel.set_margin_start(0);
            panel.set_margin_top(0);
            panel.set_visible(false);
            if let Some(refresh) = &refresh {
                refresh();
            }
        });

        self.place_at(ox, oy);
    }

    fn place_at(&self, ox: f64, oy: f64) {
        self.panel.set_margin_start(0);
        self.panel.set_margin_top(0);
        self.panel.set_visible(true);

        let panel_w = PIN_SHEET_WIDTH;
        let (_, nat_h, _, _) = self.panel.measure(gtk::Orientation::Vertical, panel_w);
        let panel_h = nat_h.max(36);
        let ov_w = self.overlay.width();
        let ov_h = self.overlay.height();
        if ov_w <= 0 || ov_h <= 0 {
            let panel = self.panel.clone();
            let overlay = self.overlay.clone();
            glib::idle_add_local_once(move || {
                Self::apply_place(&panel, &overlay, ox, oy, panel_w, panel_h);
            });
            return;
        }
        Self::apply_place(&self.panel, &self.overlay, ox, oy, panel_w, panel_h);
    }

    fn apply_place(
        panel: &gtk::Box,
        overlay: &gtk::Overlay,
        ox: f64,
        oy: f64,
        panel_w: i32,
        panel_h: i32,
    ) {
        let ov_w = overlay.width().max(1);
        let ov_h = overlay.height().max(1);
        let gap = 4;
        let mut mx = ox as i32 + gap;
        let mut my = oy as i32 + gap;
        if mx + panel_w > ov_w {
            mx = ox as i32 - gap - panel_w;
        }
        if my + panel_h > ov_h {
            my = oy as i32 - gap - panel_h;
        }
        mx = mx.clamp(0, (ov_w - panel_w).max(0));
        my = my.clamp(0, (ov_h - panel_h).max(0));
        panel.set_margin_start(mx);
        panel.set_margin_top(my);
    }
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
        super::super::dropdown::clear_trigger_highlight(&popover);
        glib::idle_add_local_once(move || popover.popdown());
    } else {
        super::super::dropdown::close_all();
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
        install_pack_layout(button, &menu_cfg);
        return;
    }
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
        let (header, user_avatar, user_name) = build_user_header(&menu_cfg);
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
        .position(super::super::popover_position())
        .child(&overlay)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.add_css_class("metis-menu-popover");
    popover.set_parent(button);
    MENU_POPOVERS.with(|menus| menus.borrow_mut().push(popover.downgrade()));

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

    super::super::dropdown::register(&popover);

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            super::super::dropdown::clear_trigger_highlight(&popover);
            glib::idle_add_local_once(move || popover.popdown());
            return;
        }
        super::super::dropdown::close_all();
        glib::idle_add_local_once(move || popover.popup());
    });
}

fn build_user_header(cfg: &metis_config::MenuConfig) -> (gtk::Box, gtk::Image, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-menu-user");
    row.set_hexpand(true);

    let avatar = gtk::Image::new();
    avatar.add_css_class("metis-menu-user-avatar");
    avatar.set_pixel_size(40);
    let name = gtk::Label::builder()
        .label(cfg.resolved_display_name())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    row.append(&avatar);
    row.append(&name);

    (row, avatar, name)
}

fn refresh_user_header(avatar: &gtk::Image, name: &gtk::Label) {
    let cfg = metis_config::load_menu_config();
    name.set_label(&cfg.resolved_display_name());
    // Clear then reload so GTK does not keep a stale paintable when ~/.face
    // is overwritten in place.
    avatar.clear();
    if let Some(path) = cfg.resolved_avatar_path() {
        avatar.set_from_file(Some(path));
    } else {
        avatar.set_from_icon_name(Some("avatar-default-symbolic"));
    }
}

fn build_rail(overlay: &gtk::Overlay, tip: &gtk::Label) -> gtk::Box {
    use metis_i18n::tr;

    let rail = gtk::Box::new(gtk::Orientation::Vertical, 6);
    rail.add_css_class("metis-menu-rail");

    rail.append(&rail_button(
        overlay,
        tip,
        "system-file-manager-symbolic",
        &tr("Files"),
        || launch_quick_action_argv(launch_file_manager_argv()),
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "utilities-terminal-symbolic",
        &tr("Terminal"),
        || launch_quick_action_argv(launch_terminal_argv().unwrap_or_default()),
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "preferences-system-symbolic",
        &tr("Settings"),
        || {
            super::super::dropdown::close_all();
            activate_or_launch_settings();
        },
    ));

    // Controller-friendly Steam Big Picture, shown only when Steam is installed
    // (native on PATH or Flatpak). Absent entirely on non-gaming setups.
    if let Some(cmd) = applications::steam_big_picture_command() {
        rail.append(&rail_button(
            overlay,
            tip,
            "input-gaming-symbolic",
            &tr("Big Picture"),
            move || {
                launch_quick_action_argv(metis_protocol::split_command_line(&cmd));
            },
        ));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    rail.append(&spacer);

    rail.append(&rail_button(
        overlay,
        tip,
        "system-lock-screen-symbolic",
        &tr("Lock"),
        || {
            if let Err(err) = crate::compositor::lock_session() {
                tracing::warn!(%err, "compositor lock_session failed");
            }
            super::super::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "weather-clear-night-symbolic",
        &tr("Suspend"),
        || {
            run_detached("systemctl", &["suspend"]);
            super::super::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-log-out-symbolic",
        &tr("Log Out"),
        || {
            if let Err(err) = crate::compositor::end_session() {
                tracing::warn!(%err, "compositor end_session failed — falling back to loginctl");
                if std::env::var_os("XDG_SESSION_ID").is_some() {
                    run_detached("loginctl", &["terminate-session", "self"]);
                } else if let Ok(user) = std::env::var("USER") {
                    run_detached("loginctl", &["terminate-user", &user]);
                }
            }
            super::super::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-reboot-symbolic",
        &tr("Restart"),
        || {
            run_detached("systemctl", &["reboot"]);
            super::super::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-shutdown-symbolic",
        &tr("Shut Down"),
        || {
            run_detached("systemctl", &["poweroff"]);
            super::super::dropdown::request_close_all();
        },
    ));

    rail
}

fn activate_or_launch_settings() {
    const SETTINGS_APP_ID: &str = "com.metis.Settings";
    let existing = match crate::compositor::list_windows() {
        Ok(windows) => windows
            .into_iter()
            .filter(|window| {
                window
                    .app_id
                    .as_deref()
                    .is_some_and(|app_id| app_id.eq_ignore_ascii_case(SETTINGS_APP_ID))
            })
            .min_by_key(|window| (!window.focused, window.id)),
        Err(err) => {
            tracing::debug!(%err, "could not query open windows before launching Settings");
            None
        }
    };

    if let Some(window) = existing {
        match crate::compositor::activate_window(window.id) {
            Ok(()) => return,
            Err(err) => {
                tracing::warn!(%err, id = window.id, "failed to restore existing Metis Settings");
            }
        }
    }
    if let Err(err) = crate::compositor::launch_program("metis-settings") {
        tracing::warn!(%err, "failed to launch metis-settings");
    }
}

fn rail_button(
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
    icon: &str,
    label: &str,
    on_click: impl Fn() + 'static,
) -> gtk::Button {
    let btn = gtk::Button::builder().has_frame(false).build();
    btn.add_css_class("metis-menu-rail-btn");
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(18);
    btn.set_child(Some(&image));
    btn.connect_clicked(move |_| on_click());
    attach_tooltip(&btn, label, overlay, tip);
    btn
}

/// Tooltip for an icon-only rail control.
///
/// GTK's built-in tooltips don't behave on this non-autohide, grab-less
/// layer-shell popover (and in the nested session they now present as a separate
/// window the compositor stacks *behind* the translucent menu), so we drive our
/// own using a single shared `Label` living in the menu's `GtkOverlay`. Because it
/// is part of the menu's own surface it always paints on top of the panel; we just
/// move it next to the hovered button after a short delay. The accessible label is
/// set directly so screen readers still get the name.
fn attach_tooltip(
    widget: &impl IsA<gtk::Widget>,
    text: &str,
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
) {
    widget
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(text)]);

    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let motion = gtk::EventControllerMotion::new();
    {
        let widget_weak = widget.clone().upcast::<gtk::Widget>().downgrade();
        let overlay_weak = overlay.downgrade();
        let tip = tip.clone();
        let text = text.to_string();
        let timer = timer.clone();
        motion.connect_enter(move |_, _, _| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            let widget_weak = widget_weak.clone();
            let overlay_weak = overlay_weak.clone();
            let tip = tip.clone();
            let text = text.clone();
            let timer_inner = timer.clone();
            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
                    *timer_inner.borrow_mut() = None;
                    let (Some(w), Some(ov)) = (widget_weak.upgrade(), overlay_weak.upgrade())
                    else {
                        return;
                    };
                    tip.set_label(&text);
                    // Position the tooltip just to the right of the button, vertically
                    // centered, in the overlay's coordinate space.
                    if let Some((x, y)) =
                        w.translate_coordinates(&ov, w.width() as f64, w.height() as f64 / 2.0)
                    {
                        tip.set_margin_start((x as i32 + 8).max(0));
                        tip.set_margin_top((y as i32 - 14).max(0));
                    }
                    tip.set_visible(true);
                });
            *timer.borrow_mut() = Some(id);
        });
    }
    {
        let tip = tip.clone();
        let timer = timer.clone();
        motion.connect_leave(move |_| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            tip.set_visible(false);
        });
    }
    widget.add_controller(motion);
}

fn restore_search_focus(search: &gtk::SearchEntry, had_focus: bool) {
    if !had_focus {
        return;
    }
    let search = search.clone();
    glib::idle_add_local_once(move || {
        if !search.has_focus() {
            search.grab_focus();
        }
    });
}

fn populate_center(
    container: &gtk::Box,
    header: &gtk::Label,
    query: &str,
    apps: &[AppEntry],
    search: &gtk::SearchEntry,
    list_generation: &Rc<Cell<u64>>,
    pin_ctx: &Rc<PinContext>,
) {
    let generation = list_generation.get().wrapping_add(1);
    list_generation.set(generation);
    let keep_search_focus = search.has_focus();
    clear_box(container);
    let q = query.trim();
    if q.is_empty() {
        header.set_text(&metis_i18n::tr("Frequent Apps"));
        for entry in applications::frequent_from(apps, FREQUENT_LIMIT) {
            container.append(&app_row(&entry, pin_ctx));
        }
        append_alpha_chunk(
            container,
            apps,
            list_generation,
            generation,
            0,
            '\0',
            pin_ctx,
        );
    } else {
        header.set_text(&metis_i18n::tr("Search Results"));
        let results = applications::search_in(apps, q);
        if results.is_empty() {
            let empty = gtk::Label::new(Some(&metis_i18n::tr("No matching applications")));
            empty.add_css_class("metis-menu-empty");
            empty.set_halign(gtk::Align::Start);
            container.append(&empty);
        }
        for entry in results {
            container.append(&app_row(&entry, pin_ctx));
        }
    }
    restore_search_focus(search, keep_search_focus);
}

/// Append a slice of the alphabetical app list, scheduling the rest on idle.
fn append_alpha_chunk(
    container: &gtk::Box,
    apps: &[AppEntry],
    list_generation: &Rc<Cell<u64>>,
    generation: u64,
    start: usize,
    mut last_letter: char,
    pin_ctx: &Rc<PinContext>,
) {
    if list_generation.get() != generation {
        return;
    }
    if start >= apps.len() {
        return;
    }
    let end = (start + MENU_ALPHA_CHUNK).min(apps.len());
    for entry in &apps[start..end] {
        let letter = entry
            .name
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('#');
        if letter != last_letter {
            last_letter = letter;
            container.append(&letter_header(letter));
        }
        container.append(&app_row(entry, pin_ctx));
    }
    if end < apps.len() {
        let container = container.clone();
        let apps = apps.to_vec();
        let list_generation = list_generation.clone();
        let pin_ctx = pin_ctx.clone();
        glib::idle_add_local_once(move || {
            append_alpha_chunk(
                &container,
                &apps,
                &list_generation,
                generation,
                end,
                last_letter,
                &pin_ctx,
            );
        });
    }
}

fn populate_pinned(
    flow: &gtk::FlowBox,
    hint: &gtk::Label,
    apps: &[AppEntry],
    pin_ctx: &Rc<PinContext>,
) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    let pinned = applications::pinned_entries_from(apps);
    if pinned.is_empty() {
        hint.set_visible(true);
        flow.set_visible(false);
        return;
    }
    hint.set_visible(false);
    flow.set_visible(true);
    for entry in pinned {
        flow.append(&pinned_tile(&entry, pin_ctx));
    }
}

/// Drive a scrolled window's vertical adjustment from wheel events. Attached in
/// Capture phase on the `ScrolledWindow` so it intercepts the whole subtree
/// before child row buttons (which otherwise swallow scroll) and covers the blank
/// gutter beside row labels.
fn wire_vertical_scroll(widget: &impl IsA<gtk::Widget>, scroll: &gtk::ScrolledWindow) {
    let ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let vadj = scroll.vadjustment();
    ctrl.connect_scroll(move |_, _, dy| {
        let page = vadj.page_size();
        let upper = vadj.upper();
        let lower = vadj.lower();
        if upper - lower <= page {
            return glib::Propagation::Proceed;
        }
        let max = (upper - page).max(lower);
        let new_val = (vadj.value() + dy).clamp(lower, max);
        if (new_val - vadj.value()).abs() > f64::EPSILON {
            vadj.set_value(new_val);
        }
        glib::Propagation::Stop
    });
    widget.add_controller(ctrl);
}

fn app_row(entry: &AppEntry, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let row = gtk::Button::builder().has_frame(false).build();
    row.add_css_class("metis-menu-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    hbox.set_hexpand(true);
    hbox.append(&app_image(entry, APP_ICON_SIZE));

    let label = gtk::Label::new(Some(&entry.name));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    hbox.append(&label);
    row.set_child(Some(&hbox));

    {
        let entry = entry.clone();
        row.connect_clicked(move |_| {
            // Close synchronously *before* launching: the new window grabs focus,
            // which otherwise swallows the deferred (idle) popdown and leaves the
            // menu hanging open over the app.
            super::super::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&row, &entry.id, pin_ctx);
    row
}

fn pinned_tile(entry: &AppEntry, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let tile = gtk::Button::builder().has_frame(false).build();
    tile.add_css_class("metis-menu-tile");

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_halign(gtk::Align::Center);
    vbox.append(&app_image(entry, PIN_ICON_SIZE));

    let label = gtk::Label::new(Some(&entry.name));
    label.add_css_class("metis-menu-tile-label");
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(10);
    vbox.append(&label);
    tile.set_child(Some(&vbox));

    {
        let entry = entry.clone();
        tile.connect_clicked(move |_| {
            // See `app_row`: pop down before the launched window steals focus.
            super::super::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&tile, &entry.id, pin_ctx);
    tile
}

/// Register a row/tile so the overlay-level secondary-click handler can resolve
/// which app was under the cursor (coordinates stay in overlay space).
fn attach_pin_target(widget: &impl IsA<gtk::Widget>, id: &str, pin_ctx: &Rc<PinContext>) {
    pin_ctx.register_target(widget, id);
}

fn app_image(entry: &AppEntry, size: i32) -> gtk::Image {
    let image = gtk::Image::new();
    applications::set_app_icon(&image, entry, size);
    image
}

fn letter_header(letter: char) -> gtk::Label {
    let label = gtk::Label::new(Some(&letter.to_string()));
    label.add_css_class("metis-menu-letter");
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Escape a value for safe interpolation inside a double-quoted shell word.
fn launch_terminal_argv() -> Option<Vec<String>> {
    metis_config::resolve_terminal().map(|t| vec![t])
}

fn launch_file_manager_argv() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    if let Some(fm) = metis_config::resolve_file_manager() {
        vec![fm, home]
    } else {
        vec!["xdg-open".into(), home]
    }
}

fn launch_quick_action_argv(argv: Vec<String>) {
    // Close before the launched window grabs focus (see `app_row`).
    super::super::dropdown::close_all();
    if argv.is_empty() {
        tracing::warn!("quick action: no executable resolved");
        return;
    }
    if let Err(err) = crate::compositor::launch_argv(argv) {
        tracing::warn!(%err, "failed to launch quick action");
    }
}

fn run_detached(cmd: &str, args: &[&str]) {
    if let Err(err) = std::process::Command::new(cmd).args(args).spawn() {
        tracing::warn!(%err, cmd, "failed to spawn power action");
    }
}

/// Bracket / Ledger / Mosaic / Ladder / Plaza / Crest / Chip (non-classic).
fn install_pack_layout(button: &gtk::Button, menu_cfg: &metis_config::MenuConfig) {
    let style = menu_cfg.style;
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    panel.add_css_class("metis-bar-dropdown-panel");
    panel.add_css_class("metis-menu-panel");
    panel.add_css_class(style.css_class());

    let overlay = gtk::Overlay::new();
    let tip = gtk::Label::new(None);
    tip.add_css_class("metis-menu-tooltip-label");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);

    let user_header_widgets: Option<(gtk::Image, gtk::Label)> =
        if style == metis_config::MenuStyle::Crest || menu_cfg.show_user_header {
            let (header, avatar, name) = if style == metis_config::MenuStyle::Crest {
                build_crest_header(menu_cfg)
            } else {
                build_user_header(menu_cfg)
            };
            panel.append(&header);
            Some((avatar, name))
        } else {
            None
        };

    let search = gtk::SearchEntry::builder()
        .placeholder_text(metis_i18n::tr("Search applications…"))
        .hexpand(true)
        .build();
    search.add_css_class("metis-menu-search");
    if style.search_on_top() {
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_row.add_css_class("metis-menu-search-row");
        search_row.append(&search);
        panel.append(&search_row);
    }

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.add_css_class("metis-menu-body");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let apps_host = gtk::Box::new(gtk::Orientation::Vertical, 4);
    apps_host.set_hexpand(true);
    apps_host.set_vexpand(true);

    let category_list = gtk::ListBox::new();
    category_list.set_selection_mode(gtk::SelectionMode::Browse);
    category_list.add_css_class("metis-menu-categories");
    let selected_category = Rc::new(RefCell::new("frequent".to_string()));

    if style.uses_categories() {
        let cat_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .width_request(if style == metis_config::MenuStyle::Ladder {
                140
            } else {
                160
            })
            .child(&category_list)
            .build();
        cat_scroll.add_css_class("metis-menu-scroll");
        wire_vertical_scroll(&cat_scroll, &cat_scroll);
        for (id, label, _) in applications::MENU_CATEGORY_DEFS {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(id);
            let lbl = gtk::Label::new(Some(&metis_i18n::tr(label)));
            lbl.set_xalign(0.0);
            lbl.set_margin_start(10);
            lbl.set_margin_end(10);
            lbl.set_margin_top(8);
            lbl.set_margin_bottom(8);
            row.set_child(Some(&lbl));
            category_list.append(&row);
        }
        if let Some(row) = category_list.row_at_index(0) {
            category_list.select_row(Some(&row));
        }
        body.append(&cat_scroll);
        if style == metis_config::MenuStyle::Ladder {
            let div = gtk::Separator::new(gtk::Orientation::Vertical);
            div.add_css_class("metis-menu-divider");
            body.append(&div);
        }
    }

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list_box.add_css_class("metis-menu-list");
    list_box.set_hexpand(true);

    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(if style.dense_grid() { 5 } else { 4 })
        .max_children_per_line(if style.dense_grid() { 6 } else { 5 })
        .row_spacing(6)
        .column_spacing(6)
        .valign(gtk::Align::Start)
        .build();
    grid.add_css_class("metis-menu-app-grid");
    if style.dense_grid() {
        grid.add_css_class("metis-menu-app-grid-dense");
    }

    let apps_scroll = if style.uses_app_grid() {
        let wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrap.set_valign(gtk::Align::Start);
        wrap.append(&grid);
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&wrap)
            .build()
    } else {
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&list_box)
            .build()
    };
    apps_scroll.add_css_class("metis-menu-scroll");
    wire_vertical_scroll(&apps_scroll, &apps_scroll);
    apps_host.append(&apps_scroll);
    body.append(&apps_host);
    panel.append(&body);

    let mut user_header_widgets = user_header_widgets;
    if style == metis_config::MenuStyle::Plaza {
        let (footer, avatar, name) = build_plaza_footer(&overlay, &tip, menu_cfg);
        panel.append(&footer);
        if user_header_widgets.is_none() {
            user_header_widgets = Some((avatar, name));
        }
    }

    overlay.set_child(Some(&panel));
    overlay.add_overlay(&tip);
    let pin_ctx = PinContext::new(&overlay);
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

    let rebuild_slot: OptFn0Cell = Rc::new(RefCell::new(None));
    let refresh: Rc<dyn Fn()> = {
        let slot = rebuild_slot.clone();
        Rc::new(move || {
            if let Some(f) = slot.borrow().clone() {
                f();
            }
        })
    };
    pin_ctx.set_refresh(refresh);

    let rebuild: Rc<dyn Fn()> = {
        let list_box = list_box.clone();
        let grid = grid.clone();
        let search = search.clone();
        let pin_ctx = pin_ctx.clone();
        let selected_category = selected_category.clone();
        let style = style;
        Rc::new(move || {
            let keep = search.has_focus();
            let query = search.text().to_string();
            let apps = applications::list_apps();
            pin_ctx.dismiss();
            pin_ctx.clear_targets();
            clear_box(&list_box);
            while let Some(child) = grid.first_child() {
                grid.remove(&child);
            }
            let filtered = if style.uses_categories() {
                applications::apps_in_category(&apps, &selected_category.borrow(), &query)
            } else if query.trim().is_empty() {
                apps.clone()
            } else {
                applications::search_in(&apps, &query)
            };
            if style.uses_app_grid() {
                let icon = if style.dense_grid() {
                    CHIP_ICON_SIZE
                } else {
                    GRID_ICON_SIZE
                };
                for entry in &filtered {
                    grid.append(&grid_tile(entry, icon, &pin_ctx));
                }
            } else {
                for entry in &filtered {
                    list_box.append(&app_row(entry, &pin_ctx));
                }
                if filtered.is_empty() {
                    let empty = gtk::Label::new(Some(&metis_i18n::tr("No apps found.")));
                    empty.add_css_class("metis-menu-empty");
                    empty.set_halign(gtk::Align::Start);
                    list_box.append(&empty);
                }
            }
            restore_search_focus(&search, keep);
        })
    };
    *rebuild_slot.borrow_mut() = Some(rebuild.clone());
    applications::register_refresh(rebuild.clone());

    if style.uses_categories() {
        let selected_category = selected_category.clone();
        let rebuild = rebuild.clone();
        category_list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            selected_category.replace(row.widget_name().to_string());
            rebuild();
        });
    }

    let search_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
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

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(&overlay)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.add_css_class("metis-menu-popover");
    popover.set_parent(button);
    MENU_POPOVERS.with(|menus| menus.borrow_mut().push(popover.downgrade()));
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
            if let Some((avatar, name)) = user_header_widgets.as_ref() {
                refresh_user_header(avatar, name);
            }
            search.block_signal(&search_changed);
            search.set_text("");
            search.unblock_signal(&search_changed);
            applications::invalidate_app_cache();
            let rebuild = rebuild.clone();
            glib::idle_add_local_once(move || rebuild());
            search.grab_focus();
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
    super::super::dropdown::register(&popover);

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            super::super::dropdown::clear_trigger_highlight(&popover);
            glib::idle_add_local_once(move || popover.popdown());
            return;
        }
        super::super::dropdown::close_all();
        glib::idle_add_local_once(move || popover.popup());
    });
}

fn build_crest_header(cfg: &metis_config::MenuConfig) -> (gtk::Box, gtk::Image, gtk::Label) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    col.add_css_class("metis-menu-crest-header");
    col.set_halign(gtk::Align::Center);
    let avatar = gtk::Image::new();
    avatar.add_css_class("metis-menu-crest-avatar");
    avatar.set_pixel_size(72);
    let name = gtk::Label::builder()
        .label(cfg.resolved_display_name())
        .halign(gtk::Align::Center)
        .build();
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    col.append(&avatar);
    col.append(&name);
    (col, avatar, name)
}

fn build_plaza_footer(
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
    cfg: &metis_config::MenuConfig,
) -> (gtk::Box, gtk::Image, gtk::Label) {
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("metis-menu-plaza-footer");
    footer.set_hexpand(true);
    let avatar = gtk::Image::new();
    avatar.set_pixel_size(28);
    avatar.add_css_class("metis-menu-user-avatar");
    let name = gtk::Label::new(Some(&cfg.resolved_display_name()));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    footer.append(&avatar);
    footer.append(&name);
    footer.append(&rail_button(
        overlay,
        tip,
        "preferences-system-symbolic",
        &metis_i18n::tr("Settings"),
        || {
            super::super::dropdown::close_all();
            activate_or_launch_settings();
        },
    ));
    footer.append(&rail_button(
        overlay,
        tip,
        "system-shutdown-symbolic",
        &metis_i18n::tr("Shut Down"),
        || {
            run_detached("systemctl", &["poweroff"]);
            super::super::dropdown::request_close_all();
        },
    ));
    (footer, avatar, name)
}

fn grid_tile(entry: &AppEntry, icon_size: i32, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let tile = gtk::Button::builder().has_frame(false).build();
    tile.add_css_class("metis-menu-tile");
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_halign(gtk::Align::Center);
    vbox.append(&app_image(entry, icon_size));
    let label = gtk::Label::new(Some(&entry.name));
    label.add_css_class("metis-menu-tile-label");
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(10);
    vbox.append(&label);
    tile.set_child(Some(&vbox));
    {
        let entry = entry.clone();
        tile.connect_clicked(move |_| {
            super::super::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&tile, &entry.id, pin_ctx);
    tile
}
