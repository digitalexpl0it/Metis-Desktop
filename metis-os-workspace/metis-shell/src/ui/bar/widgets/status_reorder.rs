//! On-bar drag reorder for optional status icons: live drop gap + persist order.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::config::{
    BarWidgetId, load_bar_config, rebuild_bar_widgets, save_bar_config, split_bar_widget_layout,
};

pub type StatusPeers = Rc<RefCell<Vec<(BarWidgetId, gtk::Widget)>>>;
pub type DropSlotCell = Rc<RefCell<Option<gtk::Box>>>;

/// Drag-threshold reorder with a highlighted drop gap (click still opens popovers).
pub fn register(
    widget: &gtk::Widget,
    id: BarWidgetId,
    peers: &StatusPeers,
    drop_slot: &DropSlotCell,
    horizontal: bool,
) {
    if !id.is_status_optional() {
        return;
    }
    peers.borrow_mut().push((id, widget.clone()));

    let armed = Rc::new(Cell::new(false));
    // GestureDrag begin (x,y) are widget-local; ox/oy are deltas from that point.
    let drag_origin = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let last_xy = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let gesture = gtk::GestureDrag::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let armed_begin = armed.clone();
    let origin_begin = drag_origin.clone();
    gesture.connect_drag_begin(move |_, x, y| {
        armed_begin.set(false);
        origin_begin.set((x, y));
    });

    let armed_update = armed.clone();
    let widget_update = widget.clone();
    let origin_update = drag_origin.clone();
    let last_update = last_xy.clone();
    let peers_update = peers.clone();
    let slot_update = drop_slot.clone();
    gesture.connect_drag_update(move |g, ox, oy| {
        let (sx, sy) = origin_update.get();
        // Prefer parent space (same as place_drop_slot); fall back to root.
        let xy = widget_update
            .parent()
            .and_then(|p| widget_pos_in(&widget_update, &p, sx + ox, sy + oy))
            .or_else(|| widget_pos_in_root(&widget_update, sx + ox, sy + oy));
        if let Some(xy) = xy {
            last_update.set(xy);
        }
        let threshold = gtk::Settings::default()
            .map(|s| f64::from(s.gtk_dnd_drag_threshold().max(1)))
            .unwrap_or(8.0);
        if !armed_update.get() {
            if ox.hypot(oy) < threshold {
                return;
            }
            armed_update.set(true);
            widget_update.add_css_class("metis-bar-status-dragging");
            g.set_state(gtk::EventSequenceState::Claimed);
        }
        let (x, y) = last_update.get();
        let axis = if horizontal { x } else { y };
        place_drop_slot(
            &widget_update,
            id,
            &peers_update.borrow(),
            &slot_update,
            axis,
            horizontal,
        );
    });

    let armed_end = armed.clone();
    let widget_end = widget.clone();
    let peers_end = peers.clone();
    let slot_end = drop_slot.clone();
    let last_end = last_xy.clone();
    gesture.connect_drag_end(move |_, _, _| {
        widget_end.remove_css_class("metis-bar-status-dragging");
        if !armed_end.get() {
            clear_drop_slot(&slot_end);
            return;
        }
        let (x, y) = last_end.get();
        let axis = if horizontal { x } else { y };
        place_drop_slot(
            &widget_end,
            id,
            &peers_end.borrow(),
            &slot_end,
            axis,
            horizontal,
        );
        commit_drop(&widget_end, &slot_end);
        persist_order_from_peers(&peers_end.borrow(), horizontal);
        clear_drop_slot(&slot_end);
    });

    let armed_cancel = armed;
    let widget_cancel = widget.clone();
    let slot_cancel = drop_slot.clone();
    gesture.connect_cancel(move |_, _| {
        widget_cancel.remove_css_class("metis-bar-status-dragging");
        armed_cancel.set(false);
        clear_drop_slot(&slot_cancel);
    });

    widget.add_controller(gesture);
}

/// Map a point in `widget` local coordinates into `target`'s coordinate space.
fn widget_pos_in(
    widget: &gtk::Widget,
    target: &impl IsA<gtk::Widget>,
    lx: f64,
    ly: f64,
) -> Option<(f64, f64)> {
    let point = widget.compute_point(target, &gtk::graphene::Point::new(lx as f32, ly as f32))?;
    Some((f64::from(point.x()), f64::from(point.y())))
}

fn widget_pos_in_root(widget: &gtk::Widget, lx: f64, ly: f64) -> Option<(f64, f64)> {
    let root = widget.root()?;
    widget_pos_in(widget, &root, lx, ly)
}

fn axis_center_in(widget: &gtk::Widget, target: &impl IsA<gtk::Widget>, horizontal: bool) -> f64 {
    let Some(bounds) = widget.compute_bounds(target) else {
        return 0.0;
    };
    if horizontal {
        f64::from(bounds.x()) + f64::from(bounds.width()) * 0.5
    } else {
        f64::from(bounds.y()) + f64::from(bounds.height()) * 0.5
    }
}

fn ensure_drop_slot(slot_cell: &RefCell<Option<gtk::Box>>, horizontal: bool) -> gtk::Box {
    if let Some(slot) = slot_cell.borrow().clone() {
        return slot;
    }
    let orientation = if horizontal {
        gtk::Orientation::Horizontal
    } else {
        gtk::Orientation::Vertical
    };
    let slot = gtk::Box::new(orientation, 0);
    slot.add_css_class("metis-bar-status-drop-slot");
    slot.set_can_target(false);
    *slot_cell.borrow_mut() = Some(slot.clone());
    slot
}

fn clear_drop_slot(slot_cell: &RefCell<Option<gtk::Box>>) {
    if let Some(slot) = slot_cell.borrow_mut().take()
        && let Some(parent) = slot.parent()
        && let Ok(box_) = parent.downcast::<gtk::Box>()
    {
        box_.remove(&slot);
    }
}

fn place_drop_slot(
    dragged: &gtk::Widget,
    dragged_id: BarWidgetId,
    peers: &[(BarWidgetId, gtk::Widget)],
    slot_cell: &RefCell<Option<gtk::Box>>,
    axis_pos: f64,
    horizontal: bool,
) {
    let Some(parent) = dragged.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
        return;
    };
    let mut others: Vec<(BarWidgetId, gtk::Widget)> = peers
        .iter()
        .filter(|(pid, w)| *pid != dragged_id && w.is_visible())
        .cloned()
        .collect();
    others.sort_by(|a, b| {
        axis_center_in(&a.1, &parent, horizontal)
            .partial_cmp(&axis_center_in(&b.1, &parent, horizontal))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut idx = others.len();
    for (i, (_, w)) in others.iter().enumerate() {
        if axis_pos < axis_center_in(w, &parent, horizontal) {
            idx = i;
            break;
        }
    }

    let slot = ensure_drop_slot(slot_cell, horizontal);
    if let Some(p) = slot.parent()
        && let Ok(box_) = p.downcast::<gtk::Box>()
    {
        box_.remove(&slot);
    }

    // Insert after the previous peer (or before the first). Skip the dragged
    // widget itself when walking left so a left-edge drop lands at the start.
    let sibling = if idx == 0 {
        let mut before = others
            .first()
            .map(|(_, w)| w.prev_sibling())
            .unwrap_or_else(|| dragged.prev_sibling());
        while before.as_ref().is_some_and(|s| s == dragged) {
            before = before.and_then(|s| s.prev_sibling());
        }
        before
    } else {
        Some(others[idx - 1].1.clone().upcast())
    };
    parent.insert_child_after(&slot, sibling.as_ref());
}

fn commit_drop(dragged: &gtk::Widget, slot_cell: &RefCell<Option<gtk::Box>>) {
    let Some(slot) = slot_cell.borrow().clone() else {
        return;
    };
    let Some(parent) = dragged.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
        return;
    };
    let sibling = slot.prev_sibling();
    if let Some(p) = slot.parent()
        && let Ok(box_) = p.downcast::<gtk::Box>()
    {
        box_.remove(&slot);
    }
    parent.reorder_child_after(dragged, sibling.as_ref());
}

/// Persist status order from the pill layout. Hidden auto-hide icons keep their
/// relative place from the previous config. Does **not** send `reload-bar`
/// (avoids double rebuild blanking icons); the file monitor rebuilds once and
/// rehydrate restores poller state.
fn persist_order_from_peers(peers: &[(BarWidgetId, gtk::Widget)], _horizontal: bool) {
    let mut cfg = load_bar_config();
    let (_, old_status, _) = split_bar_widget_layout(&cfg.widgets);

    // Walk the parent box's children — not geometry. Right after
    // `reorder_child_after`, `compute_bounds` is often still stale, so sorting
    // by axis made drag look correct until logout (disk never updated).
    let Some((_, first)) = peers.first() else {
        return;
    };
    let Some(parent) = first.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
        return;
    };
    let peer_ptrs: HashSet<usize> = peers.iter().map(|(_, w)| w.as_ptr() as usize).collect();
    let mut visible_ids = Vec::new();
    let mut child = parent.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        let ptr = widget.as_ptr() as usize;
        if !peer_ptrs.contains(&ptr) {
            continue;
        }
        if let Some((id, _)) = peers.iter().find(|(_, w)| w.as_ptr() as usize == ptr)
            && widget.is_visible()
        {
            visible_ids.push(*id);
        }
    }
    if visible_ids.is_empty() {
        return;
    }
    let visible_set: HashSet<BarWidgetId> = visible_ids.iter().copied().collect();

    let mut vis_iter = visible_ids.into_iter();
    let mut new_status = Vec::with_capacity(old_status.len());
    for id in &old_status {
        if visible_set.contains(id) {
            if let Some(next) = vis_iter.next() {
                new_status.push(next);
            }
        } else {
            new_status.push(*id);
        }
    }
    new_status.extend(vis_iter);

    if new_status == old_status {
        return;
    }
    cfg.widgets = rebuild_bar_widgets(&new_status);
    if let Err(err) = save_bar_config(&cfg) {
        tracing::warn!(%err, "failed to save bar.json after status reorder");
    }
}
