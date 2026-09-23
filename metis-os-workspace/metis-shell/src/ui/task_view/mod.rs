//! Super+Tab Task View (Windows-style sticky overlay).
//!
//! - Super+Tab opens / cycles apps; Super release does **not** close or activate.
//! - Esc, Enter/click app, or clicking a desktop dismisses.
//! - App grid scrolls above a **pinned** workspace shelf so desktops stay on-screen.
//! - Drag an app onto a shelf tile to move it without closing early.
//! - Close (×) on each card closes that window; Task View stays open.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_protocol::WindowInfo;

use crate::services::applications;
use crate::services::windows;
use crate::ui::window_thumbs::{self, ThumbSet, WorkspaceThumbSet};

thread_local! {
    static OVERLAY: RefCell<Option<Rc<TaskView>>> = const { RefCell::new(None) };
    static ACTIVATING: Cell<bool> = const { Cell::new(false) };
    /// True while an app-card drag is in progress. Shelf clicks must ignore the
    /// button-release that ends a drop, or they switch workspace and dismiss.
    static APP_DRAG_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Rebuild/thumb refresh requested while a drag was active — flush on end.
    static REFRESH_PENDING: Cell<bool> = const { Cell::new(false) };
}

struct TaskView {
    window: gtk::Window,
    /// Hit-through layer for the floating drag ghost (Fixed avoids margin reflow).
    drag_layer: gtk::Fixed,
    apps: gtk::FlowBox,
    shelf: gtk::Box,
    cards: RefCell<Vec<gtk::Widget>>,
    windows: RefCell<Vec<WindowInfo>>,
    selected: Cell<usize>,
    output: RefCell<Option<String>>,
    thumbs: RefCell<Option<ThumbSet>>,
    workspace_thumbs: RefCell<Option<WorkspaceThumbSet>>,
    drag_ghost: RefCell<Option<gtk::Widget>>,
    pointer: Cell<(f64, f64)>,
}

pub fn is_open() -> bool {
    OVERLAY.with(|o| o.borrow().is_some())
}

#[allow(dead_code)]
pub fn toggle() {
    cycle_next();
}

pub fn cycle_next() {
    cycle(true);
}

pub fn cycle_prev() {
    cycle(false);
}

pub fn activate_selected() {
    if ACTIVATING.with(|c| c.get()) {
        return;
    }
    let id = OVERLAY.with(|o| {
        let borrow = o.borrow();
        let tv = borrow.as_ref()?;
        let windows = tv.windows.borrow();
        if windows.is_empty() {
            return None;
        }
        let idx = tv.selected.get().min(windows.len().saturating_sub(1));
        windows.get(idx).map(|w| w.id)
    });
    let Some(id) = id else {
        dismiss();
        return;
    };
    ACTIVATING.with(|c| c.set(true));
    dismiss();
    glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        ACTIVATING.with(|c| c.set(false));
        if let Err(err) = crate::compositor::activate_window(id) {
            tracing::warn!(id, %err, "task view activate failed");
        } else {
            tracing::info!(id, "task view activated window");
        }
    });
}

pub fn dismiss() {
    APP_DRAG_ACTIVE.with(|c| c.set(false));
    REFRESH_PENDING.with(|c| c.set(false));
    OVERLAY.with(|o| {
        if let Some(tv) = o.borrow_mut().take() {
            clear_drag_ghost(&tv);
            tv.window.set_visible(false);
            tv.window.destroy();
        }
    });
    restore_sibling_layers();
}

pub fn on_windows_changed() {
    if APP_DRAG_ACTIVE.with(|c| c.get()) {
        // Never rebuild mid-drag — destroying the gesture widget freezes GTK.
        REFRESH_PENDING.with(|c| c.set(true));
        return;
    }
    schedule_refresh_after_change();
}

/// Immediate card/shelf layout from the current window list, then a deferred
/// live thumb recapture so workspace mini-desktops update after moves/closes.
fn schedule_refresh_after_change() {
    if APP_DRAG_ACTIVE.with(|c| c.get()) {
        REFRESH_PENDING.with(|c| c.set(true));
        return;
    }
    rebuild_after_move();
    OVERLAY.with(|o| {
        let Some(tv) = o.borrow().as_ref().cloned() else {
            return;
        };
        // Give the compositor a beat to apply MoveWindow / close before GL grab.
        glib::timeout_add_local_once(std::time::Duration::from_millis(140), move || {
            if !is_open() || APP_DRAG_ACTIVE.with(|c| c.get()) {
                if is_open() {
                    REFRESH_PENDING.with(|c| c.set(true));
                }
                return;
            }
            OVERLAY.with(|o| {
                let borrow = o.borrow();
                let Some(live) = borrow.as_ref() else {
                    return;
                };
                if !Rc::ptr_eq(live, &tv) {
                    return;
                }
                refresh_thumbs_and_rebuild(live);
            });
        });
    });
}

fn finish_app_drag() {
    APP_DRAG_ACTIVE.with(|c| c.set(false));
    OVERLAY.with(|o| {
        if let Some(tv) = o.borrow().as_ref() {
            clear_drag_ghost(tv);
            clear_shelf_drop_hover(tv);
        }
    });
    if REFRESH_PENDING.with(|c| c.replace(false)) {
        windows::reconcile_now();
        schedule_refresh_after_change();
    }
}

/// Rebuild cards/shelf from the current window snapshot, keeping cached thumbs.
fn rebuild_after_move() {
    OVERLAY.with(|o| {
        let borrow = o.borrow();
        let Some(tv) = borrow.as_ref() else {
            return;
        };
        let output = tv.output.borrow().clone();
        let workspace = crate::services::active_workspace_for(output.as_deref());
        let list = windows::windows_for_output_workspace(output.as_deref(), workspace);
        *tv.windows.borrow_mut() = list;
        if tv.selected.get() >= tv.windows.borrow().len() && !tv.windows.borrow().is_empty() {
            tv.selected.set(0);
        }
        rebuild(tv);
    });
}

fn refresh_thumbs_and_rebuild(tv: &Rc<TaskView>) {
    let output = tv.output.borrow().clone();
    let workspace = crate::services::active_workspace_for(output.as_deref());
    let list = windows::windows_for_output_workspace(output.as_deref(), workspace);
    let (win_thumbs, ws_thumbs) = capture_thumbs(output.as_deref(), &list);
    *tv.thumbs.borrow_mut() = win_thumbs;
    *tv.workspace_thumbs.borrow_mut() = ws_thumbs;
    *tv.windows.borrow_mut() = list;
    if tv.selected.get() >= tv.windows.borrow().len() && !tv.windows.borrow().is_empty() {
        tv.selected.set(0);
    }
    rebuild(tv);
}

fn demote_sibling_layers() {
    crate::ui::notification_center::set_below_screenshot(true);
    crate::ui::dashboard::set_below_screenshot(true);
    crate::ui::bar::widgets::set_menu_below_screenshot(true);
}

fn restore_sibling_layers() {
    crate::ui::notification_center::set_below_screenshot(false);
    crate::ui::dashboard::set_below_screenshot(false);
    crate::ui::bar::widgets::set_menu_below_screenshot(false);
}

fn cycle(forward: bool) {
    if !is_open() {
        show(forward);
        return;
    }
    OVERLAY.with(|o| {
        let borrow = o.borrow();
        let Some(tv) = borrow.as_ref() else {
            return;
        };
        let n = tv.windows.borrow().len();
        if n == 0 {
            return;
        }
        let cur = tv.selected.get();
        let next = if forward {
            (cur + 1) % n
        } else {
            cur.checked_sub(1).unwrap_or(n - 1)
        };
        tv.selected.set(next);
        refresh_selection(tv);
    });
}

fn show(forward: bool) {
    if is_open() {
        return;
    }

    windows::reconcile_now();
    let output = windows::focused_output_name();
    let workspace = crate::services::active_workspace_for(output.as_deref());
    let list = windows::windows_for_output_workspace(output.as_deref(), workspace);
    let (win_thumbs, ws_thumbs) = capture_thumbs(output.as_deref(), &list);

    demote_sibling_layers();

    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Task View"))
        .decorated(false)
        .build();
    window.add_css_class("metis-task-view");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-task-view"));
    if let Some(monitor) = gdk_monitor_for_output(output.as_deref()) {
        window.set_monitor(Some(&monitor));
    }
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_exclusive_zone(-1);
        window.set_margin(edge, 0);
    }

    // Vertical layout: scrolling app plane on top, pinned shelf at bottom —
    // shelf never gets pushed off-screen by many app cards.
    // Overlay host lets a DnD ghost follow the cursor above cards/shelf.
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("metis-task-view-overlay");
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    window.set_child(Some(&overlay));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("metis-task-view-backdrop");
    root.set_halign(gtk::Align::Fill);
    root.set_valign(gtk::Align::Fill);
    // Full-surface hit target so transparent regions still receive clicks.
    root.set_can_target(true);
    overlay.set_child(Some(&root));

    // Hit-through Fixed for the drag ghost — `move_` is cheap vs Overlay margins.
    let drag_layer = gtk::Fixed::new();
    drag_layer.add_css_class("metis-task-view-drag-layer");
    drag_layer.set_hexpand(true);
    drag_layer.set_vexpand(true);
    drag_layer.set_can_target(false);
    overlay.add_overlay(&drag_layer);

    // Click empty backdrop (not a card/shelf) to dismiss — Win11 Task View.
    let backdrop_click = gtk::GestureClick::new();
    backdrop_click.set_button(1);
    backdrop_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
    let root_for_pick = root.clone();
    backdrop_click.connect_released(move |_, n_press, x, y| {
        if n_press != 1 || APP_DRAG_ACTIVE.with(|c| c.get()) {
            return;
        }
        let Some(target) = root_for_pick.pick(x, y, gtk::PickFlags::DEFAULT) else {
            dismiss();
            return;
        };
        // Only dismiss when the pick lands on the backdrop itself (or scroll
        // chrome), not on an app card or shelf tile.
        let mut w: Option<gtk::Widget> = Some(target);
        while let Some(widget) = w {
            if widget.has_css_class("metis-task-view-card")
                || widget.has_css_class("metis-task-view-shelf-tile")
                || widget.has_css_class("metis-task-view-shelf")
                || widget.has_css_class("metis-task-view-shelf-bar")
                || widget.has_css_class("metis-task-view-drag-preview")
            {
                return;
            }
            w = widget.parent();
        }
        dismiss();
    });
    root.add_controller(backdrop_click);

    let scroll = gtk::ScrolledWindow::new();
    scroll.add_css_class("metis-task-view-scroll");
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_hexpand(true);
    scroll.set_vexpand(true);
    scroll.set_margin_top(40);
    scroll.set_margin_start(32);
    scroll.set_margin_end(32);
    scroll.set_margin_bottom(12);
    root.append(&scroll);

    let apps_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    apps_host.set_halign(gtk::Align::Fill);
    apps_host.set_valign(gtk::Align::Center);
    apps_host.set_hexpand(true);
    apps_host.set_vexpand(true);
    scroll.set_child(Some(&apps_host));

    let apps = gtk::FlowBox::new();
    apps.add_css_class("metis-task-view-apps");
    apps.set_selection_mode(gtk::SelectionMode::None);
    // Prefer a horizontal row/grid; never force a single-column stack.
    let n = list.len().max(1) as u32;
    let per_line = n.clamp(1, 4);
    apps.set_min_children_per_line(per_line);
    apps.set_max_children_per_line(4);
    apps.set_column_spacing(16);
    apps.set_row_spacing(16);
    apps.set_homogeneous(true);
    apps.set_halign(gtk::Align::Center);
    apps.set_valign(gtk::Align::Center);
    apps.set_hexpand(true);
    apps.set_vexpand(false);
    apps.set_can_focus(false);
    apps_host.append(&apps);

    let shelf_bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shelf_bar.add_css_class("metis-task-view-shelf-bar");
    shelf_bar.set_halign(gtk::Align::Fill);
    shelf_bar.set_valign(gtk::Align::End);
    shelf_bar.set_hexpand(true);
    shelf_bar.set_vexpand(false);
    shelf_bar.set_can_target(true);
    root.append(&shelf_bar);

    let shelf_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shelf_wrap.add_css_class("metis-task-view-shelf");
    shelf_wrap.set_halign(gtk::Align::Center);
    shelf_wrap.set_margin_bottom(18);
    shelf_wrap.set_can_target(true);
    shelf_bar.append(&shelf_wrap);

    let shelf = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    shelf.set_halign(gtk::Align::Center);
    shelf.set_valign(gtk::Align::Center);
    shelf.set_can_target(true);
    shelf_wrap.append(&shelf);

    let selected = if list.len() > 1 {
        if forward { 1 } else { list.len() - 1 }
    } else {
        0
    };

    let tv = Rc::new(TaskView {
        window: window.clone(),
        drag_layer: drag_layer.clone(),
        apps: apps.clone(),
        shelf: shelf.clone(),
        cards: RefCell::new(Vec::new()),
        windows: RefCell::new(list),
        selected: Cell::new(selected),
        output: RefCell::new(output),
        thumbs: RefCell::new(win_thumbs),
        workspace_thumbs: RefCell::new(ws_thumbs),
        drag_ghost: RefCell::new(None),
        pointer: Cell::new((0.0, 0.0)),
    });

    // Keep a last-known pointer for ghost spawn; live tracking uses GestureDrag
    // surface coords (motion controllers go quiet once a native DnD starts).
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(move |_, x, y| {
        OVERLAY.with(|o| {
            if let Some(tv) = o.borrow().as_ref() {
                tv.pointer.set((x, y));
            }
        });
    });
    window.add_controller(motion);

    rebuild(&tv);
    wire_keyboard(&tv);

    OVERLAY.with(|o| *o.borrow_mut() = Some(tv.clone()));
    window.present();
    window.grab_focus();
}

fn capture_thumbs(
    output: Option<&str>,
    list: &[WindowInfo],
) -> (Option<ThumbSet>, Option<WorkspaceThumbSet>) {
    let count = crate::services::workspace_count().max(1);
    let ws_ids: Vec<u32> = (1..=count).collect();
    let win_thumbs = window_thumbs::load_window_thumbs(list);
    let ws_thumbs = output.and_then(|o| window_thumbs::load_workspace_thumbs(o, &ws_ids));
    (win_thumbs, ws_thumbs)
}

fn rebuild(tv: &Rc<TaskView>) {
    while let Some(child) = tv.apps.first_child() {
        tv.apps.remove(&child);
    }
    while let Some(child) = tv.shelf.first_child() {
        tv.shelf.remove(&child);
    }
    tv.cards.borrow_mut().clear();

    let list = tv.windows.borrow().clone();
    let n = list.len().max(1) as u32;
    let per_line = n.clamp(1, 4);
    tv.apps.set_min_children_per_line(per_line);
    tv.apps.set_max_children_per_line(4);

    let thumbs = tv.thumbs.borrow();
    for (i, w) in list.iter().enumerate() {
        let card = build_app_card(tv, w, i, thumbs.as_ref());
        tv.apps.insert(&card, -1);
        tv.cards.borrow_mut().push(card.upcast());
    }
    drop(thumbs);
    refresh_selection(tv);

    let count = crate::services::workspace_count().max(1);
    let output = tv.output.borrow().clone();
    let active = crate::services::active_workspace_for(output.as_deref());
    let ws_thumbs = tv.workspace_thumbs.borrow();
    for ws in 1..=count {
        let tile = build_shelf_tile(tv, ws, active, ws_thumbs.as_ref());
        tv.shelf.append(&tile);
    }
}

/// Compact cards so a row of apps fits above the pinned shelf.
const CARD_W: i32 = 220;
const CARD_H: i32 = 150;
const SHELF_W: i32 = 140;
const SHELF_H: i32 = 72;

fn build_app_card(
    tv: &Rc<TaskView>,
    w: &WindowInfo,
    index: usize,
    thumbs: Option<&ThumbSet>,
) -> gtk::Box {
    // Box + GestureClick + DragSource: GtkButton's `clicked` does not fire when
    // a DragSource is attached to the same widget.
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("metis-task-view-card");
    card.set_size_request(CARD_W, CARD_H);
    card.set_overflow(gtk::Overflow::Hidden);
    card.set_can_target(true);
    card.set_can_focus(false);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("metis-task-view-card-header");
    header.set_halign(gtk::Align::Fill);
    // Header must accept hits so the close button is clickable; icon/title
    // stay non-targeting so most of the chrome still activates the card.
    header.set_can_target(true);

    let icon = gtk::Image::from_gicon(&applications::resolve_icon_for_app_id(w.app_id.as_deref()));
    icon.set_pixel_size(16);
    icon.set_can_target(false);
    header.append(&icon);

    let title = gtk::Label::new(Some(w.title.as_str()));
    title.add_css_class("metis-task-view-card-title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Start);
    title.set_can_target(false);
    header.append(&title);

    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("metis-task-view-card-close");
    close.set_tooltip_text(Some(&metis_i18n::tr("Close")));
    close.set_valign(gtk::Align::Center);
    close.set_halign(gtk::Align::End);
    close.set_focus_on_click(false);
    close.set_can_focus(false);
    close.set_has_frame(false);
    // Claim the click sequence so the card's activate / drag gestures ignore it.
    let claim = gtk::GestureClick::new();
    claim.set_button(1);
    claim.set_propagation_phase(gtk::PropagationPhase::Capture);
    claim.connect_pressed(|gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    close.add_controller(claim);
    let close_id = w.id;
    close.connect_clicked(move |_| {
        if let Err(err) = crate::compositor::close_window(close_id) {
            tracing::warn!(id = close_id, %err, "task view close failed");
        }
        // Keep Task View open; window list churn rebuilds the cards + shelf thumbs.
        glib::idle_add_local_once(|| {
            windows::reconcile_now();
            schedule_refresh_after_change();
        });
    });
    header.append(&close);
    card.append(&header);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.add_css_class("metis-task-view-card-preview");
    preview.set_hexpand(true);
    preview.set_vexpand(true);
    preview.set_overflow(gtk::Overflow::Hidden);
    preview.set_can_target(false);

    let thumb = window_thumbs::thumb_or_icon_widget(thumbs, w, 48);
    thumb.set_halign(gtk::Align::Fill);
    thumb.set_valign(gtk::Align::Fill);
    thumb.set_hexpand(true);
    thumb.set_vexpand(true);
    thumb.set_can_target(false);
    preview.append(&thumb);
    card.append(&preview);

    // Click = activate; press-and-drag past the DnD threshold = move to workspace.
    // GestureDrag's drag-begin fires on button-down, so we only enter drag mode
    // once offset exceeds gtk-dnd-drag-threshold.
    let drag_armed = Rc::new(Cell::new(false));

    let gesture = gtk::GestureDrag::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Bubble);

    let tv_begin = tv.clone();
    let armed_begin = drag_armed.clone();
    gesture.connect_drag_begin(move |g, _, _| {
        armed_begin.set(false);
        tv_begin.selected.set(index);
        refresh_selection(&tv_begin);
        if let Some((x, y)) = gesture_surface_pos(g) {
            tv_begin.pointer.set((x, y));
        }
    });

    let drag_title = w.title.clone();
    let drag_app_id = w.app_id.clone();
    let drag_thumb = thumbs.and_then(|t| t.textures.get(&w.id)).cloned();
    let window_id = w.id;
    let tv_update = tv.clone();
    let card_drag = card.clone();
    let armed_update = drag_armed.clone();
    gesture.connect_drag_update(move |g, ox, oy| {
        if !armed_update.get() {
            let threshold = drag_threshold_px();
            if ox.hypot(oy) < threshold {
                return;
            }
            armed_update.set(true);
            APP_DRAG_ACTIVE.with(|c| c.set(true));
            card_drag.add_css_class("dragging");
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some((x, y)) = gesture_surface_pos(g) {
                tv_update.pointer.set((x, y));
            }
            show_drag_ghost(
                &tv_update,
                drag_title.as_str(),
                drag_app_id.as_deref(),
                drag_thumb.as_ref(),
            );
        }
        let Some((x, y)) = gesture_surface_pos(g) else {
            return;
        };
        tv_update.pointer.set((x, y));
        position_drag_ghost(&tv_update, x, y);
        update_shelf_drop_hover(&tv_update, x, y);
    });

    let tv_end = tv.clone();
    let card_end = card.clone();
    let armed_end = drag_armed.clone();
    gesture.connect_drag_end(move |g, _, _| {
        card_end.remove_css_class("dragging");
        if armed_end.get() {
            let drop_ws = gesture_surface_pos(g)
                .or_else(|| Some(tv_end.pointer.get()))
                .and_then(|(x, y)| workspace_under_point(&tv_end, x, y));
            if let Some(workspace) = drop_ws {
                if let Err(err) = crate::compositor::move_window_to_workspace(window_id, workspace)
                {
                    tracing::warn!(window_id, workspace, %err, "task view move failed");
                } else {
                    REFRESH_PENDING.with(|c| c.set(true));
                }
            }
            // Idle so a shelf click-released sees APP_DRAG_ACTIVE first.
            glib::idle_add_local_once(|| {
                finish_app_drag();
            });
        } else {
            // Short click: bring the selected app to front.
            activate_selected();
        }
    });

    let tv_cancel = tv.clone();
    let card_cancel = card.clone();
    let armed_cancel = drag_armed;
    gesture.connect_cancel(move |_, _| {
        card_cancel.remove_css_class("dragging");
        if armed_cancel.get() {
            clear_drag_ghost(&tv_cancel);
            glib::idle_add_local_once(|| {
                finish_app_drag();
            });
        }
    });
    card.add_controller(gesture);

    card
}

fn drag_threshold_px() -> f64 {
    gtk::Settings::default()
        .map(|s| f64::from(s.gtk_dnd_drag_threshold().max(1)))
        .unwrap_or(8.0)
}

const DRAG_HOT_X: f64 = 80.0;
const DRAG_HOT_Y: f64 = 24.0;

/// Surface-local pointer for the active gesture (stable while the card moves).
fn gesture_surface_pos(gesture: &gtk::GestureDrag) -> Option<(f64, f64)> {
    gesture.current_event()?.position()
}

fn show_drag_ghost(
    tv: &Rc<TaskView>,
    title: &str,
    app_id: Option<&str>,
    thumb: Option<&gdk::Texture>,
) {
    clear_drag_ghost(tv);
    let ghost = build_drag_preview(title, app_id, thumb);
    ghost.set_can_target(false);
    ghost.set_sensitive(false);
    let (x, y) = tv.pointer.get();
    tv.drag_layer.put(&ghost, x - DRAG_HOT_X, y - DRAG_HOT_Y);
    *tv.drag_ghost.borrow_mut() = Some(ghost.upcast());
}

fn clear_drag_ghost(tv: &TaskView) {
    if let Some(ghost) = tv.drag_ghost.borrow_mut().take() {
        tv.drag_layer.remove(&ghost);
    }
}

fn position_drag_ghost(tv: &TaskView, x: f64, y: f64) {
    let Some(ghost) = tv.drag_ghost.borrow().clone() else {
        return;
    };
    tv.drag_layer.move_(&ghost, x - DRAG_HOT_X, y - DRAG_HOT_Y);
}

fn clear_shelf_drop_hover(tv: &TaskView) {
    let mut child = tv.shelf.first_child();
    while let Some(tile) = child {
        tile.remove_css_class("drop-hover");
        child = tile.next_sibling();
    }
}

fn update_shelf_drop_hover(tv: &TaskView, x: f64, y: f64) {
    clear_shelf_drop_hover(tv);
    if let Some(tile) = shelf_tile_under_point(tv, x, y) {
        tile.add_css_class("drop-hover");
    }
}

fn shelf_tile_under_point(tv: &TaskView, x: f64, y: f64) -> Option<gtk::Widget> {
    let target = tv.window.pick(x, y, gtk::PickFlags::DEFAULT)?;
    let mut w: Option<gtk::Widget> = Some(target);
    while let Some(widget) = w {
        if widget.has_css_class("metis-task-view-shelf-tile") {
            return Some(widget);
        }
        w = widget.parent();
    }
    None
}

fn workspace_under_point(tv: &TaskView, x: f64, y: f64) -> Option<u32> {
    let tile = shelf_tile_under_point(tv, x, y)?;
    let name = tile.widget_name();
    name.strip_prefix("ws-")?.parse().ok()
}

/// Floating mini-card that tracks the pointer during an app → workspace drag.
fn build_drag_preview(title: &str, app_id: Option<&str>, thumb: Option<&gdk::Texture>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("metis-task-view-drag-preview");
    root.set_size_request(160, 110);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("metis-task-view-drag-preview-header");
    let icon = gtk::Image::from_gicon(&applications::resolve_icon_for_app_id(app_id));
    icon.set_pixel_size(14);
    header.append(&icon);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("metis-task-view-drag-preview-title");
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Start);
    header.append(&label);
    root.append(&header);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.add_css_class("metis-task-view-drag-preview-body");
    preview.set_hexpand(true);
    preview.set_vexpand(true);
    if let Some(tex) = thumb {
        let pic = gtk::Picture::for_paintable(tex);
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.set_can_shrink(true);
        pic.set_hexpand(true);
        pic.set_vexpand(true);
        preview.append(&pic);
    } else {
        let big = gtk::Image::from_gicon(&applications::resolve_icon_for_app_id(app_id));
        big.set_pixel_size(40);
        big.set_halign(gtk::Align::Center);
        big.set_valign(gtk::Align::Center);
        big.set_hexpand(true);
        big.set_vexpand(true);
        preview.append(&big);
    }
    root.append(&preview);
    root
}

fn build_shelf_tile(
    tv: &Rc<TaskView>,
    workspace: u32,
    active: u32,
    ws_thumbs: Option<&WorkspaceThumbSet>,
) -> gtk::Box {
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 4);
    tile.add_css_class("metis-task-view-shelf-tile");
    tile.set_widget_name(&format!("ws-{workspace}"));
    if workspace == active {
        tile.add_css_class("active");
    }
    tile.set_size_request(SHELF_W, -1);
    tile.set_can_target(true);
    tile.set_valign(gtk::Align::Start);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview.add_css_class("metis-task-view-shelf-preview");
    preview.set_size_request(SHELF_W, SHELF_H);
    preview.set_overflow(gtk::Overflow::Hidden);
    preview.set_can_target(true);
    preview.set_hexpand(false);
    preview.set_vexpand(false);

    if let Some(tex) = ws_thumbs.and_then(|t| t.textures.get(&workspace)) {
        let pic = gtk::Picture::for_paintable(tex);
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.set_can_shrink(true);
        pic.add_css_class("metis-task-view-shelf-thumb");
        pic.set_can_target(false);
        preview.append(&pic);
    } else {
        let label = gtk::Label::new(Some(&workspace.to_string()));
        label.add_css_class("metis-task-view-shelf-fallback");
        label.set_hexpand(true);
        label.set_vexpand(true);
        label.set_halign(gtk::Align::Center);
        label.set_valign(gtk::Align::Center);
        preview.append(&label);
    }
    tile.append(&preview);

    let name = gtk::Label::new(Some(&format!("{} {workspace}", metis_i18n::tr("Desktop"))));
    name.add_css_class("metis-task-view-shelf-label");
    name.set_halign(gtk::Align::Center);
    name.set_valign(gtk::Align::Start);
    tile.append(&name);

    let out_click = tv.output.borrow().clone();
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.set_propagation_phase(gtk::PropagationPhase::Bubble);
    click.connect_released(move |_, n_press, _, _| {
        if n_press != 1 || APP_DRAG_ACTIVE.with(|c| c.get()) {
            return;
        }
        switch_and_dismiss(out_click.clone(), workspace);
    });
    tile.add_controller(click);

    tile
}

fn switch_and_dismiss(output: Option<String>, workspace: u32) {
    tracing::info!(?output, workspace, "task view switch workspace");
    if let Some(o) = output {
        crate::services::dispatch_workspace(Some(o), workspace);
    } else {
        crate::services::dispatch_workspace(None, workspace);
    }
    dismiss();
}

fn refresh_selection(tv: &TaskView) {
    let sel = tv.selected.get();
    for (i, card) in tv.cards.borrow().iter().enumerate() {
        if i == sel {
            card.add_css_class("selected");
        } else {
            card.remove_css_class("selected");
        }
    }
}

fn wire_keyboard(tv: &Rc<TaskView>) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tv_key = tv.clone();
    controller.connect_key_pressed(move |_, key, _, mods| {
        use gdk::Key;
        if key == Key::Escape {
            dismiss();
            return glib::Propagation::Stop;
        }
        if key == Key::Return || key == Key::KP_Enter || key == Key::space {
            activate_selected();
            return glib::Propagation::Stop;
        }
        if key == Key::Tab || key == Key::ISO_Left_Tab {
            let forward = !mods.contains(gdk::ModifierType::SHIFT_MASK) && key != Key::ISO_Left_Tab;
            let n = tv_key.windows.borrow().len();
            if n > 0 {
                let cur = tv_key.selected.get();
                let next = if forward {
                    (cur + 1) % n
                } else {
                    cur.checked_sub(1).unwrap_or(n - 1)
                };
                tv_key.selected.set(next);
                refresh_selection(&tv_key);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    tv.window.add_controller(controller);
}

fn gdk_monitor_for_output(connector: Option<&str>) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    let n = monitors.n_items();
    if let Some(name) = connector {
        for i in 0..n {
            let Ok(monitor) = monitors.item(i)?.downcast::<gdk::Monitor>() else {
                continue;
            };
            if monitor
                .connector()
                .map(|c| c.eq_ignore_ascii_case(name))
                .unwrap_or(false)
            {
                return Some(monitor);
            }
        }
    }
    for i in 0..n {
        let Ok(monitor) = monitors.item(i)?.downcast::<gdk::Monitor>() else {
            continue;
        };
        return Some(monitor);
    }
    None
}
