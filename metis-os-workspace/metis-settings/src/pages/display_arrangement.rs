//! macOS-style draggable monitor arrangement preview for Settings → Display.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{load_outputs_config, output_prefs, save_outputs_config, OutputsConfig};
use metis_protocol::OutputInfo;

const CANVAS_MIN_H: i32 = 220;
const PAD: f64 = 20.0;
const SNAP_PX: f64 = 28.0;
const TAP_THRESHOLD_PX: f64 = 6.0;

const BLOCK_COLORS: &[&str] = &[
    "metis-display-block-0",
    "metis-display-block-1",
    "metis-display-block-2",
    "metis-display-block-3",
];

#[derive(Clone)]
struct BlockState {
    name: String,
    logical_x: i32,
    logical_y: i32,
    width: i32,
    height: i32,
    label: String,
    primary: bool,
    color_idx: usize,
}

pub struct ArrangementCanvas {
    root: gtk::Box,
    /// Viewport that owns the allocation used for layout; keeps GtkFixed's
    /// child bounding-box from locking the Settings window min-width.
    ///
    /// Plain `Box` (not `ScrolledWindow`): GTK's built-in scroll controller on a
    /// `ScrolledWindow` always `Stop`s wheel events even with Never/Never
    /// policy, which fights the Settings page scroller on GTK 4.22.
    viewport: gtk::Box,
    canvas: gtk::Fixed,
    hint: gtk::Label,
    cfg: Rc<RefCell<OutputsConfig>>,
    outputs: Rc<RefCell<Vec<OutputInfo>>>,
    selected_name: Rc<RefCell<Option<String>>>,
    on_select: Rc<dyn Fn(usize)>,
    on_pending_changed: Rc<dyn Fn(bool)>,
    blocks: Rc<RefCell<Vec<BlockState>>>,
    committed_blocks: Rc<RefCell<Vec<BlockState>>>,
    block_widgets: Rc<RefCell<Vec<gtk::Frame>>>,
    scale: Rc<RefCell<f64>>,
    origin: Rc<RefCell<(i32, i32)>>,
    pending: Rc<RefCell<bool>>,
    draggable: Rc<RefCell<bool>>,
    trial_backup: Rc<RefCell<Option<OutputsConfig>>>,
    canvas_size: Rc<RefCell<(f64, f64)>>,
    resize_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    /// Suppresses viewport-driven `recompute_layout` while a tile is dragged —
    /// otherwise GtkFixed reflow fights GestureDrag and rubber-bands.
    dragging: Cell<bool>,
}

impl ArrangementCanvas {
    pub fn new(
        cfg: Rc<RefCell<OutputsConfig>>,
        outputs: Rc<RefCell<Vec<OutputInfo>>>,
        selected_name: Rc<RefCell<Option<String>>>,
        on_select: Rc<dyn Fn(usize)>,
        on_pending_changed: Rc<dyn Fn(bool)>,
    ) -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        root.add_css_class("metis-display-arrangement");

        let hint = gtk::Label::new(None);
        hint.set_xalign(0.0);
        hint.add_css_class("metis-settings-hint");
        // Cap the hint's reported min-width so a long sentence can't lock the
        // Settings window wider than the sidebar + content default.
        hint.set_wrap(true);
        hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        hint.set_width_chars(28);
        hint.set_max_width_chars(56);
        root.append(&hint);

        let canvas = gtk::Fixed::builder()
            .height_request(CANVAS_MIN_H)
            .hexpand(true)
            .build();
        canvas.add_css_class("metis-display-arrangement-canvas");
        canvas.set_hexpand(true);
        canvas.set_overflow(gtk::Overflow::Hidden);

        // GtkFixed's minimum size is the child bounding box. Two side-by-side
        // monitor tiles easily report ~700–900px and freeze the window from
        // shrinking. The viewport takes allocation; Fixed lays out inside it.
        let viewport = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .height_request(CANVAS_MIN_H)
            .build();
        viewport.set_overflow(gtk::Overflow::Hidden);
        viewport.set_size_request(200, CANVAS_MIN_H);
        viewport.add_css_class("metis-display-arrangement-viewport");
        viewport.append(&canvas);
        root.append(&viewport);

        let this = Rc::new(Self {
            root,
            viewport: viewport.clone(),
            canvas,
            hint,
            cfg,
            outputs,
            selected_name,
            on_select,
            on_pending_changed,
            blocks: Rc::new(RefCell::new(Vec::new())),
            committed_blocks: Rc::new(RefCell::new(Vec::new())),
            block_widgets: Rc::new(RefCell::new(Vec::new())),
            scale: Rc::new(RefCell::new(1.0)),
            origin: Rc::new(RefCell::new((0, 0))),
            pending: Rc::new(RefCell::new(false)),
            draggable: Rc::new(RefCell::new(false)),
            trial_backup: Rc::new(RefCell::new(None)),
            canvas_size: Rc::new(RefCell::new((480.0, CANVAS_MIN_H as f64))),
            resize_debounce: Rc::new(RefCell::new(None)),
            dragging: Cell::new(false),
        });
        {
            let this_w = this.clone();
            let on_alloc = {
                let this_w = this_w.clone();
                move |widget: &gtk::Box| {
                    if this_w.dragging.get() {
                        return;
                    }
                    let alloc = widget.allocation();
                    // Ignore sub-pixel / scrollbar jitter — a recompute_layout
                    // storm while the page scrolls feels like a lock-up on the
                    // arrangement card.
                    if alloc.width() > 0 && alloc.height() > 0 {
                        this_w
                            .schedule_layout_for_size(alloc.width() as f64, alloc.height() as f64);
                    }
                }
            };
            let on_width = Rc::new(on_alloc);
            this.viewport
                .connect_notify_local(Some("width"), move |widget, _| {
                    on_width(widget);
                });
            // Height is fixed via size_request — don't re-layout on height notify.
            let this_w = this.clone();
            this.root.connect_map(move |_| {
                this_w.refresh_layout();
            });
        }
        // Ensure wheel/touchpad over the preview scrolls the Settings page —
        // Capture on tiles alone is not enough if a child claims the sequence.
        crate::ui::forward_wheel_to_page_scroller(&this.viewport);
        crate::ui::forward_wheel_to_page_scroller(&this.canvas);
        wire_canvas_drag(&this);
        this.rebuild_blocks();
        this
    }

    fn schedule_layout_for_size(self: &Rc<Self>, width: f64, height: f64) {
        if self.dragging.get() {
            return;
        }
        let prev = *self.canvas_size.borrow();
        // Ignore tiny allocation jitter (scrollbar show/hide, subpixel rounding).
        if (prev.0 - width).abs() < 8.0 && (prev.1 - height).abs() < 8.0 {
            return;
        }
        *self.canvas_size.borrow_mut() = (width, height);

        let mut debounce = self.resize_debounce.borrow_mut();
        if let Some(id) = debounce.take() {
            id.remove();
        }
        let this = self.clone();
        let id = glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            *this.resize_debounce.borrow_mut() = None;
            if this.dragging.get() || this.block_widgets.borrow().is_empty() {
                return glib::ControlFlow::Break;
            }
            this.recompute_layout();
            glib::ControlFlow::Break
        });
        *debounce = Some(id);
    }

    fn canvas_dims(self: &Rc<Self>) -> (f64, f64) {
        self.sync_canvas_size_from_allocation();
        *self.canvas_size.borrow()
    }

    /// Pick up the viewport (or parent) width so layout works before the first
    /// `width` notify, and after rebuilds while the Display stack page is shown.
    fn sync_canvas_size_from_allocation(self: &Rc<Self>) {
        let alloc = self.viewport.allocation();
        let mut width = alloc.width().max(0) as f64;
        let height = alloc
            .height()
            .max(self.viewport.height_request())
            .max(CANVAS_MIN_H) as f64;
        if width < 120.0 {
            if let Some(parent) = self.viewport.parent() {
                let palloc = parent.allocation();
                if palloc.width() > 0 {
                    width = palloc.width() as f64;
                }
            }
        }
        if width >= 120.0 && height >= 80.0 {
            *self.canvas_size.borrow_mut() = (width, height);
        }
    }

    pub fn refresh_layout(self: &Rc<Self>) {
        self.sync_canvas_size_from_allocation();
        self.recompute_layout();
    }

    fn recompute_layout(self: &Rc<Self>) {
        if self.dragging.get() {
            return;
        }
        let blocks = self.blocks.borrow().clone();
        if blocks.is_empty() {
            return;
        }
        let (canvas_w, canvas_h) = self.canvas_dims();
        // The Display page lives in a hidden stack child at build time — skip until
        // GTK has given the canvas a real allocation (see connect_map/size_allocate).
        if canvas_w < 120.0 || canvas_h < 80.0 {
            return;
        }
        let (scale, min_x, min_y) = fit_scale(&blocks, canvas_w, canvas_h);
        *self.scale.borrow_mut() = scale;
        *self.origin.borrow_mut() = (min_x, min_y);

        for (block, widget) in blocks.iter().zip(self.block_widgets.borrow().iter()) {
            let (cw, ch) = block_canvas_size(block, scale);
            widget.set_size_request(cw.round().max(1.0) as i32, ch.round().max(1.0) as i32);
            let (cx, cy) = logical_to_canvas(block.logical_x, block.logical_y, min_x, min_y, scale);
            let (cx, cy) = clamp_canvas_point(cx, cy, cw, ch, canvas_w, canvas_h);
            self.canvas.move_(widget, cx, cy);
        }
    }

    pub fn widget(self: &Rc<Self>) -> &gtk::Box {
        &self.root
    }

    #[allow(dead_code)] // handy for tests / future layout asserts
    pub fn output_count(&self) -> usize {
        self.outputs.borrow().len()
    }

    /// Stable identity of the monitors currently drawn (order matters).
    pub fn output_names(&self) -> Vec<String> {
        self.blocks
            .borrow()
            .iter()
            .map(|b| b.name.clone())
            .collect()
    }

    pub fn block_name(&self, index: usize) -> Option<String> {
        self.blocks.borrow().get(index).map(|b| b.name.clone())
    }

    pub fn has_pending(&self) -> bool {
        *self.pending.borrow()
    }

    pub fn in_trial(&self) -> bool {
        self.trial_backup.borrow().is_some()
    }

    /// Apply pending layout to disk (not yet confirmed). `force` allows saving
    /// when only other display fields (resolution, etc.) changed.
    pub fn begin_trial(self: &Rc<Self>, force: bool) -> bool {
        if self.in_trial() {
            return false;
        }
        if !*self.pending.borrow() && !force {
            return false;
        }
        // Shift arrangement so the top-left of the bounding box is (0, 0). Stops
        // a bad drag from parking the primary output at e.g. (3084, 514).
        self.normalize_origin_into_cfg();
        let backup = load_outputs_config();
        let cfg = self.cfg.borrow().clone();
        if let Err(err) = save_outputs_config(&cfg) {
            tracing::warn!(%err, "failed to save output layout");
            return false;
        }
        *self.trial_backup.borrow_mut() = Some(backup);
        self.set_pending(false);
        true
    }

    /// Write block positions into cfg with the desktop origin at (0, 0).
    fn normalize_origin_into_cfg(self: &Rc<Self>) {
        let mut blocks = self.blocks.borrow_mut();
        if blocks.is_empty() {
            return;
        }
        let min_x = blocks.iter().map(|b| b.logical_x).min().unwrap_or(0);
        let min_y = blocks.iter().map(|b| b.logical_y).min().unwrap_or(0);
        if min_x != 0 || min_y != 0 {
            for b in blocks.iter_mut() {
                b.logical_x -= min_x;
                b.logical_y -= min_y;
            }
        }
        let mut cfg = self.cfg.borrow_mut();
        for b in blocks.iter() {
            let entry = cfg.outputs.entry(b.name.clone()).or_default();
            entry.layout_x = Some(b.logical_x);
            entry.layout_y = Some(b.logical_y);
        }
    }

    /// User accepted the trial arrangement in the confirmation dialog.
    pub fn confirm_trial(self: &Rc<Self>) {
        if !self.in_trial() {
            return;
        }
        *self.committed_blocks.borrow_mut() = self.blocks.borrow().clone();
        *self.trial_backup.borrow_mut() = None;
    }

    /// User rejected the trial arrangement or the confirmation timer expired.
    pub fn cancel_trial(self: &Rc<Self>) {
        let Some(backup) = self.trial_backup.borrow_mut().take() else {
            return;
        };
        *self.cfg.borrow_mut() = backup.clone();
        if let Err(err) = save_outputs_config(&backup) {
            tracing::warn!(%err, "failed to restore output layout");
        }
        *self.blocks.borrow_mut() = self.committed_blocks.borrow().clone();
        self.reposition_widgets();
    }

    /// Discard in-memory edits and restore the last committed preview.
    pub fn revert_layout(self: &Rc<Self>) {
        if self.in_trial() {
            self.cancel_trial();
            return;
        }
        *self.cfg.borrow_mut() = load_outputs_config();
        *self.blocks.borrow_mut() = self.committed_blocks.borrow().clone();
        self.set_pending(false);
        self.reposition_widgets();
    }

    fn set_pending(self: &Rc<Self>, dirty: bool) {
        if *self.pending.borrow() == dirty {
            return;
        }
        *self.pending.borrow_mut() = dirty;
        let cb = self.on_pending_changed.clone();
        glib::idle_add_local_once(move || cb(dirty));
    }

    /// Full rebuild when the output list changes.
    pub fn rebuild_blocks(self: &Rc<Self>) {
        while let Some(child) = self.canvas.first_child() {
            self.canvas.remove(&child);
        }
        self.block_widgets.borrow_mut().clear();
        if !self.in_trial() {
            self.set_pending(false);
        }

        let blocks = {
            let list = self.outputs.borrow();
            if list.is_empty() {
                self.hint.set_label(
                    "No displays detected — start a Metis session or click Detect displays.",
                );
                *self.draggable.borrow_mut() = false;
                return;
            }

            let can_arrange = list.len() >= 2 && !self.in_trial();
            *self.draggable.borrow_mut() = can_arrange;
            if self.in_trial() {
                self.hint.set_label(
                    "Confirm the new arrangement in the dialog. Changes revert automatically if you do not accept.",
                );
            } else if can_arrange {
                self.hint.set_label(
                    "Drag displays to match their physical positions, then click Save display settings \
         at the bottom of the page. This controls how the pointer moves between screens.",
                );
            } else {
                self.hint.set_label(
                    "Single display preview. Connect another monitor to arrange relative positions.",
                );
            }

            let mut blocks = build_blocks(&list, &self.cfg.borrow());
            // New HDMI outputs often report the same origin as the primary until
            // the user arranges them — untangle so boxes aren't stacked, and mark
            // the auto-placement as pending so Save persists it.
            if untangle_overlaps(&mut blocks, None) && !self.in_trial() {
                let mut cfg = self.cfg.borrow_mut();
                for b in &blocks {
                    let entry = cfg.outputs.entry(b.name.clone()).or_default();
                    entry.layout_x = Some(b.logical_x);
                    entry.layout_y = Some(b.logical_y);
                }
                drop(cfg);
                self.set_pending(true);
            }
            blocks
        };
        if blocks.is_empty() {
            return;
        }

        *self.blocks.borrow_mut() = blocks.clone();
        if !self.in_trial() {
            *self.committed_blocks.borrow_mut() = blocks.clone();
        }

        let sel = self.selected_index();
        let draggable = *self.draggable.borrow();
        let mut widgets = Vec::with_capacity(blocks.len());
        for (idx, block) in blocks.iter().enumerate() {
            let widget = build_block_widget(block, idx == sel);
            // Drag is handled once on the Fixed canvas (stable coords). Per-tile
            // GestureDrag rubber-bands because offsets are in the moving child's space.
            if !draggable {
                wire_select(self, &widget, idx);
            }
            widgets.push(widget.clone());
            self.canvas.put(&widget, PAD, PAD);
        }
        *self.block_widgets.borrow_mut() = widgets;
        let this = self.clone();
        glib::idle_add_local_once(move || {
            this.refresh_layout();
            // One more pass after GTK sizes the new block widgets.
            glib::idle_add_local_once(move || this.refresh_layout());
        });
    }

    pub fn set_selected(self: &Rc<Self>, index: usize) {
        let name = self.blocks.borrow().get(index).map(|b| b.name.clone());
        if let Some(name) = name {
            *self.selected_name.borrow_mut() = Some(name);
        }
        for (idx, widget) in self.block_widgets.borrow().iter().enumerate() {
            if idx == index {
                widget.add_css_class("metis-display-block-selected");
            } else {
                widget.remove_css_class("metis-display-block-selected");
            }
        }
    }

    fn selected_index(self: &Rc<Self>) -> usize {
        let name = self.selected_name.borrow().clone();
        let blocks = self.blocks.borrow();
        if let Some(ref n) = name {
            if let Some(idx) = blocks.iter().position(|b| &b.name == n) {
                return idx;
            }
        }
        0
    }

    fn reposition_widgets(self: &Rc<Self>) {
        self.recompute_layout();
    }

    /// Refresh block positions from the latest compositor output list (after apply).
    pub fn sync_positions(self: &Rc<Self>) {
        let blocks = {
            let list = self.outputs.borrow();
            build_blocks(&list, &self.cfg.borrow())
        };
        let widget_count = self.block_widgets.borrow().len();
        if blocks.len() != widget_count {
            self.rebuild_blocks();
            return;
        }
        *self.blocks.borrow_mut() = blocks.clone();
        if !self.in_trial() {
            *self.committed_blocks.borrow_mut() = blocks.clone();
        }
        self.reposition_widgets();
    }
}

fn build_blocks(list: &[OutputInfo], cfg: &OutputsConfig) -> Vec<BlockState> {
    let primary = configured_primary_name(cfg, list);
    list.iter()
        .enumerate()
        .map(|(i, out)| {
            let prefs = output_prefs(cfg, &out.name);
            let (logical_x, logical_y) = if list.len() >= 2 {
                match (prefs.layout_x, prefs.layout_y) {
                    (Some(x), Some(y)) => (x, y),
                    _ => (out.rect.x, out.rect.y),
                }
            } else {
                (0, 0)
            };
            BlockState {
                name: out.name.clone(),
                logical_x,
                logical_y,
                width: out.rect.width.max(1),
                height: out.rect.height.max(1),
                label: short_label(out, i),
                primary: primary.as_deref() == Some(out.name.as_str()),
                color_idx: i % BLOCK_COLORS.len(),
            }
        })
        .collect()
}

fn configured_primary_name(cfg: &OutputsConfig, list: &[OutputInfo]) -> Option<String> {
    if let Some(ref name) = cfg.primary_output {
        if list.iter().any(|o| o.name == *name && o.enabled) {
            return Some(name.clone());
        }
    }
    list.iter()
        .find(|o| o.primary)
        .map(|o| o.name.clone())
        .or_else(|| list.first().map(|o| o.name.clone()))
}

fn short_label(out: &OutputInfo, index: usize) -> String {
    let name = if !out.make.is_empty() || !out.model.is_empty() {
        format!("{} {}", out.make.trim(), out.model.trim())
            .trim()
            .to_string()
    } else {
        out.name.clone()
    };
    if name.is_empty() {
        format!("Display {}", index + 1)
    } else {
        name
    }
}

fn fit_scale(blocks: &[BlockState], canvas_w: f64, canvas_h: f64) -> (f64, i32, i32) {
    let (min_x, min_y, max_x, max_y) = bounds(blocks);
    let bw = (max_x - min_x).max(1) as f64;
    let bh = (max_y - min_y).max(1) as f64;
    let inner_w = (canvas_w - PAD * 2.0).max(1.0);
    let inner_h = (canvas_h - PAD * 2.0).max(1.0);
    let scale = (inner_w / bw).min(inner_h / bh).max(0.01);
    (scale, min_x, min_y)
}

fn block_canvas_size(block: &BlockState, scale: f64) -> (f64, f64) {
    (
        (block.width as f64 * scale).max(1.0),
        (block.height as f64 * scale).max(1.0),
    )
}

fn clamp_canvas_point(
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
    canvas_w: f64,
    canvas_h: f64,
) -> (f64, f64) {
    let max_x = (canvas_w - cw - PAD).max(PAD);
    let max_y = (canvas_h - ch - PAD).max(PAD);
    (cx.clamp(PAD, max_x), cy.clamp(PAD, max_y))
}

fn bounds(blocks: &[BlockState]) -> (i32, i32, i32, i32) {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for b in blocks {
        min_x = min_x.min(b.logical_x);
        min_y = min_y.min(b.logical_y);
        max_x = max_x.max(b.logical_x + b.width);
        max_y = max_y.max(b.logical_y + b.height);
    }
    (min_x, min_y, max_x, max_y)
}

fn logical_to_canvas(x: i32, y: i32, min_x: i32, min_y: i32, scale: f64) -> (f64, f64) {
    let cx = PAD + (x - min_x) as f64 * scale;
    let cy = PAD + (y - min_y) as f64 * scale;
    (cx, cy)
}

fn canvas_to_logical(cx: f64, cy: f64, min_x: i32, min_y: i32, scale: f64) -> (i32, i32) {
    let x = min_x + ((cx - PAD) / scale).round() as i32;
    let y = min_y + ((cy - PAD) / scale).round() as i32;
    (x, y)
}

fn build_block_widget(block: &BlockState, selected: bool) -> gtk::Frame {
    let frame = gtk::Frame::builder().build();
    frame.add_css_class("metis-display-block");
    frame.add_css_class(BLOCK_COLORS[block.color_idx]);
    if selected {
        frame.add_css_class("metis-display-block-selected");
    }
    if block.primary {
        frame.add_css_class("metis-display-block-primary");
    }

    let col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();

    if block.primary {
        let bar = gtk::Box::builder().height_request(6).build();
        bar.add_css_class("metis-display-block-menubar");
        col.append(&bar);
    }

    let label = gtk::Label::new(Some(&block.label));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    label.set_max_width_chars(14);
    label.add_css_class("metis-display-block-label");
    col.append(&label);

    frame.set_child(Some(&col));
    frame
}

fn wire_select(canvas: &Rc<ArrangementCanvas>, widget: &gtk::Frame, index: usize) {
    let gesture = gtk::GestureClick::new();
    gesture.connect_pressed({
        let canvas = canvas.clone();
        move |_, _, _, _| {
            canvas.set_selected(index);
            (canvas.on_select)(index);
        }
    });
    widget.add_controller(gesture);
}

/// Drag tiles via a gesture on the Fixed canvas — not on the tiles themselves.
///
/// `GtkGestureDrag` offsets are in the widget the gesture is attached to. When
/// that widget is the tile being `move_`'d, the coordinate space moves under the
/// pointer every frame and the drag rubber-bands. The canvas stays put, so
/// `start_origin + offset` tracks the pointer smoothly.
fn wire_canvas_drag(canvas: &Rc<ArrangementCanvas>) {
    let drag_index: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let start_origin = Rc::new(RefCell::new((0.0_f64, 0.0_f64)));
    let last_pos = Rc::new(RefCell::new((0.0_f64, 0.0_f64)));
    let tile_size = Rc::new(RefCell::new((1.0_f64, 1.0_f64)));
    let drag_total = Rc::new(RefCell::new((0.0_f64, 0.0_f64)));

    let drag = gtk::GestureDrag::builder()
        .button(1)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();

    drag.connect_drag_begin({
        let canvas = canvas.clone();
        let drag_index = drag_index.clone();
        let start_origin = start_origin.clone();
        let last_pos = last_pos.clone();
        let tile_size = tile_size.clone();
        let drag_total = drag_total.clone();
        move |gesture, x, y| {
            drag_index.set(None);
            if !*canvas.draggable.borrow() || canvas.in_trial() {
                return;
            }
            let Some(index) = hit_block_index(&canvas, x, y) else {
                return;
            };
            // Claim so child buttons / labels don't steal the sequence.
            gesture.set_state(gtk::EventSequenceState::Claimed);

            if let Some(id) = canvas.resize_debounce.borrow_mut().take() {
                id.remove();
            }
            canvas.dragging.set(true);
            drag_index.set(Some(index));
            drag_total.replace((0.0, 0.0));

            let widgets = canvas.block_widgets.borrow();
            let Some(widget) = widgets.get(index) else {
                return;
            };
            widget.add_css_class("metis-display-block-dragging");
            let alloc = widget.allocation();
            let origin = (alloc.x() as f64, alloc.y() as f64);
            start_origin.replace(origin);
            last_pos.replace(origin);
            let (bw, bh) = canvas
                .blocks
                .borrow()
                .get(index)
                .map(|b| block_canvas_size(b, *canvas.scale.borrow()))
                .unwrap_or((alloc.width() as f64, alloc.height() as f64));
            tile_size.replace((bw.max(1.0), bh.max(1.0)));

            canvas.set_selected(index);
            (canvas.on_select)(index);
        }
    });

    drag.connect_drag_update({
        let canvas = canvas.clone();
        let drag_index = drag_index.clone();
        let start_origin = start_origin.clone();
        let last_pos = last_pos.clone();
        let tile_size = tile_size.clone();
        let drag_total = drag_total.clone();
        move |_, offset_x, offset_y| {
            let Some(index) = drag_index.get() else {
                return;
            };
            drag_total.replace((offset_x, offset_y));
            let (ox, oy) = *start_origin.borrow();
            let (tw, th) = *tile_size.borrow();
            let (canvas_w, canvas_h) = *canvas.canvas_size.borrow();
            let (nx, ny) = clamp_canvas_point(
                ox + offset_x,
                oy + offset_y,
                tw,
                th,
                canvas_w.max(120.0),
                canvas_h.max(80.0),
            );
            last_pos.replace((nx, ny));
            if let Some(widget) = canvas.block_widgets.borrow().get(index) {
                canvas.canvas.move_(widget, nx, ny);
            }
        }
    });

    drag.connect_drag_end({
        let canvas = canvas.clone();
        let drag_index = drag_index.clone();
        let last_pos = last_pos.clone();
        let tile_size = tile_size.clone();
        let drag_total = drag_total.clone();
        move |_, _, _| {
            let Some(index) = drag_index.take() else {
                canvas.dragging.set(false);
                return;
            };
            if let Some(widget) = canvas.block_widgets.borrow().get(index) {
                widget.remove_css_class("metis-display-block-dragging");
            }
            canvas.dragging.set(false);

            if canvas.in_trial() {
                return;
            }

            let (ox, oy) = *drag_total.borrow();
            if ox.hypot(oy) < TAP_THRESHOLD_PX {
                // Click without a real drag — selection already applied on begin.
                return;
            }

            let (mut cx, mut cy) = *last_pos.borrow();
            let (tw, th) = *tile_size.borrow();
            snap_canvas_position(
                index,
                &mut cx,
                &mut cy,
                &canvas.blocks.borrow(),
                *canvas.scale.borrow(),
                *canvas.origin.borrow(),
            );

            let (canvas_w, canvas_h) = *canvas.canvas_size.borrow();
            let (cx, cy) =
                clamp_canvas_point(cx, cy, tw, th, canvas_w.max(120.0), canvas_h.max(80.0));

            let (min_x, min_y) = *canvas.origin.borrow();
            let scale = (*canvas.scale.borrow()).max(0.01);
            let (logical_x, logical_y) = canvas_to_logical(cx, cy, min_x, min_y, scale);

            {
                let mut blocks = canvas.blocks.borrow_mut();
                if let Some(b) = blocks.get_mut(index) {
                    b.logical_x = logical_x;
                    b.logical_y = logical_y;
                }
                untangle_overlaps(&mut blocks, Some(index));
                let min_x = blocks.iter().map(|b| b.logical_x).min().unwrap_or(0);
                let min_y = blocks.iter().map(|b| b.logical_y).min().unwrap_or(0);
                if min_x != 0 || min_y != 0 {
                    for b in blocks.iter_mut() {
                        b.logical_x -= min_x;
                        b.logical_y -= min_y;
                    }
                }
                let mut c = canvas.cfg.borrow_mut();
                for block in blocks.iter() {
                    let entry = c.outputs.entry(block.name.clone()).or_default();
                    entry.layout_x = Some(block.logical_x);
                    entry.layout_y = Some(block.logical_y);
                }
            }

            canvas.set_pending(true);
            canvas.recompute_layout();
        }
    });

    canvas.canvas.add_controller(drag);
}

fn hit_block_index(canvas: &ArrangementCanvas, x: f64, y: f64) -> Option<usize> {
    // Prefer the top-most tile when they overlap (later children paint above).
    for (i, widget) in canvas.block_widgets.borrow().iter().enumerate().rev() {
        let alloc = widget.allocation();
        let left = alloc.x() as f64;
        let top = alloc.y() as f64;
        let right = left + alloc.width() as f64;
        let bottom = top + alloc.height() as f64;
        if x >= left && x < right && y >= top && y < bottom {
            return Some(i);
        }
    }
    None
}

fn snap_canvas_position(
    moved_idx: usize,
    cx: &mut f64,
    cy: &mut f64,
    blocks: &[BlockState],
    scale: f64,
    origin: (i32, i32),
) {
    let (min_x, min_y) = origin;
    let moved = &blocks[moved_idx];
    let mw = moved.width as f64 * scale;
    let mh = moved.height as f64 * scale;

    for (i, other) in blocks.iter().enumerate() {
        if i == moved_idx {
            continue;
        }
        let (ox, oy) = logical_to_canvas(other.logical_x, other.logical_y, min_x, min_y, scale);
        let ow = other.width as f64 * scale;
        let oh = other.height as f64 * scale;

        if (*cx - (ox + ow)).abs() < SNAP_PX {
            *cx = ox + ow;
        }
        if ((*cx + mw) - ox).abs() < SNAP_PX {
            *cx = ox - mw;
        }
        if (*cy - (oy + oh)).abs() < SNAP_PX {
            *cy = oy + oh;
        }
        if ((*cy + mh) - oy).abs() < SNAP_PX {
            *cy = oy - mh;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rects_overlap(ax: i32, ay: i32, aw: i32, ah: i32, bx: i32, by: i32, bw: i32, bh: i32) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

/// Push overlapping monitors apart. Prefers moving `prefer_move` (the tile the
/// user just dragged); otherwise moves non-primary outputs. Separation follows
/// the shallowest overlap axis in the direction of the centers — never the old
/// "always shove right" rule that made top/bottom/swap arrangements impossible.
fn untangle_overlaps(blocks: &mut [BlockState], prefer_move: Option<usize>) -> bool {
    if blocks.len() < 2 {
        return false;
    }
    let mut changed = false;
    for _ in 0..64 {
        let mut progressed = false;
        for i in 0..blocks.len() {
            for j in 0..blocks.len() {
                if i == j {
                    continue;
                }
                let (ax, ay, aw, ah) = (
                    blocks[i].logical_x,
                    blocks[i].logical_y,
                    blocks[i].width,
                    blocks[i].height,
                );
                let (bx, by, bw, bh) = (
                    blocks[j].logical_x,
                    blocks[j].logical_y,
                    blocks[j].width,
                    blocks[j].height,
                );
                if !rects_overlap(ax, ay, aw, ah, bx, by, bw, bh) {
                    continue;
                }

                let overlap_x = (ax + aw).min(bx + bw) - ax.max(bx);
                let overlap_y = (ay + ah).min(by + bh) - ay.max(by);
                if overlap_x <= 0 || overlap_y <= 0 {
                    continue;
                }

                // Which block to move: prefer the dragged tile; else the
                // non-primary; else the higher index.
                let move_i = match prefer_move {
                    Some(p) if p == i => true,
                    Some(p) if p == j => false,
                    _ if blocks[i].primary != blocks[j].primary => !blocks[i].primary,
                    _ => i > j,
                };
                let (mi, oi) = if move_i { (i, j) } else { (j, i) };

                let (mx, my, mw, mh) = (
                    blocks[mi].logical_x,
                    blocks[mi].logical_y,
                    blocks[mi].width,
                    blocks[mi].height,
                );
                let (ox, oy, ow, oh) = (
                    blocks[oi].logical_x,
                    blocks[oi].logical_y,
                    blocks[oi].width,
                    blocks[oi].height,
                );
                let mcx = mx as i64 + mw as i64 / 2;
                let mcy = my as i64 + mh as i64 / 2;
                let ocx = ox as i64 + ow as i64 / 2;
                let ocy = oy as i64 + oh as i64 / 2;

                let pair_overlap_x = (mx + mw).min(ox + ow) - mx.max(ox);
                let pair_overlap_y = (my + mh).min(oy + oh) - my.max(oy);

                if pair_overlap_x <= pair_overlap_y {
                    if mcx <= ocx {
                        blocks[mi].logical_x = ox - mw;
                    } else {
                        blocks[mi].logical_x = ox + ow;
                    }
                } else if mcy <= ocy {
                    blocks[mi].logical_y = oy - mh;
                } else {
                    blocks[mi].logical_y = oy + oh;
                }
                progressed = true;
                changed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    changed
}
