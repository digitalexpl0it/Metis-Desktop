//! Pin/Unpin sheet drawn inside the Start menu overlay (same Wayland surface).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::services::applications;

pub(crate) const PIN_SHEET_WIDTH: i32 = 180;

/// Pin/Unpin menu drawn inside the Start menu's `GtkOverlay` (same surface).
/// A nested `GtkPopover` would be a separate Wayland surface the compositor
/// stacks *behind* the translucent menu — same pitfall as rail tooltips.
///
/// Right-clicks are handled on the overlay itself so (x, y) are already in
/// overlay space. Per-row `translate_coordinates` failed for FlowBox tiles and
/// for the second open (stale margins made the clamp snap to top-left).
pub(crate) struct PinContext {
    overlay: gtk::Overlay,
    pub(crate) panel: gtk::Box,
    /// Live row/tile widgets → desktop id, for overlay `pick` → pin target.
    targets: RefCell<HashMap<usize, String>>,
    refresh: RefCell<Option<Rc<dyn Fn()>>>,
}

impl PinContext {
    pub(crate) fn new(overlay: &gtk::Overlay) -> Rc<Self> {
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

    pub(crate) fn set_refresh(&self, refresh: Rc<dyn Fn()>) {
        *self.refresh.borrow_mut() = Some(refresh);
    }

    pub(crate) fn clear_targets(&self) {
        self.targets.borrow_mut().clear();
    }

    pub(crate) fn register_target(&self, widget: &impl IsA<gtk::Widget>, id: &str) {
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

    pub(crate) fn dismiss(&self) {
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
        if let Some(child) = item.child()
            && let Ok(lbl) = child.downcast::<gtk::Label>()
        {
            lbl.set_halign(gtk::Align::Start);
            lbl.set_xalign(0.0);
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
