//! Annotation editor for a captured PNG.
//!
//! The canvas keeps two layers: the pixel `image` (mutated only by destructive
//! operations such as crop and pixelate) and a vector list of `Annotation`s that
//! is re-rendered every frame. Export re-runs the exact same Cairo drawing code
//! against an offscreen surface, so the saved file always matches the preview.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

use crate::icons::{self, Glyph};
use crate::{Cli, ocr, pin, theme};

const PALETTE: [(&str, (f64, f64, f64)); 6] = [
    ("Red", (0.95, 0.25, 0.21)),
    ("Amber", (1.0, 0.72, 0.11)),
    ("Green", (0.24, 0.79, 0.44)),
    ("Blue", (0.24, 0.60, 0.98)),
    ("White", (1.0, 1.0, 1.0)),
    ("Black", (0.06, 0.07, 0.09)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pen,
    Highlighter,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Pixelate,
    Crop,
    Ocr,
}

impl Tool {
    const ALL: [Tool; 9] = [
        Tool::Pen,
        Tool::Highlighter,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Text,
        Tool::Pixelate,
        Tool::Crop,
        Tool::Ocr,
    ];

    fn glyph(self) -> Glyph {
        match self {
            Tool::Pen => Glyph::Pen,
            Tool::Highlighter => Glyph::Highlighter,
            Tool::Arrow => Glyph::Arrow,
            Tool::Rect => Glyph::Rect,
            Tool::Ellipse => Glyph::Ellipse,
            Tool::Text => Glyph::Text,
            Tool::Pixelate => Glyph::Pixelate,
            Tool::Crop => Glyph::Crop,
            Tool::Ocr => Glyph::Ocr,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Tool::Pen => "Pen — freehand line",
            Tool::Highlighter => "Highlighter — translucent freehand",
            Tool::Arrow => "Arrow — drag from tail to tip",
            Tool::Rect => "Rectangle — drag to size",
            Tool::Ellipse => "Ellipse — drag to size",
            Tool::Text => "Text — drag a box, then type directly on the image",
            Tool::Pixelate => "Pixelate — drag over what to hide",
            Tool::Crop => "Crop — drag to keep that region",
            Tool::Ocr => "Extract all text into a selectable results view",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Pen | Tool::Highlighter => "Drag on the image to draw",
            Tool::Arrow => "Drag from the tail to the arrow tip",
            Tool::Rect | Tool::Ellipse => "Drag to size the shape",
            Tool::Text => "Drag a text box, then type; move it or resize its corner",
            Tool::Pixelate => "Drag over the region to obscure",
            Tool::Crop => "Drag the region to keep",
            Tool::Ocr => "Extract all text from the image",
        }
    }

    fn is_freehand(self) -> bool {
        matches!(self, Tool::Pen | Tool::Highlighter)
    }
}

#[derive(Clone)]
struct Annotation {
    tool: Tool,
    points: Vec<(f64, f64)>,
    color: (f64, f64, f64),
    width: f64,
    text: String,
}

impl Annotation {
    fn span(&self) -> ((f64, f64), (f64, f64)) {
        let first = self.points.first().copied().unwrap_or((0.0, 0.0));
        let last = self.points.last().copied().unwrap_or(first);
        (first, last)
    }
}

/// One reversible edit. Vector edits only store the previous annotation list;
/// full image copies are kept exclusively for crop and pixelate, which cannot be
/// replayed from vectors.
enum Step {
    Annotations(Vec<Annotation>),
    Image(image::RgbaImage, Vec<Annotation>),
}

const MAX_HISTORY: usize = 40;

struct State {
    path: PathBuf,
    image: image::RgbaImage,
    surface: cairo::ImageSurface,
    annotations: Vec<Annotation>,
    active: Option<Annotation>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    tool: Tool,
    color: (f64, f64, f64),
    width: f64,
    selected_text: Option<usize>,
}

impl State {
    fn rebuild_surface(&mut self) -> Result<(), String> {
        self.surface = surface_from_image(&self.image)?;
        Ok(())
    }

    fn record(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn push_annotation(&mut self, annotation: Annotation) {
        let previous = self.annotations.clone();
        self.annotations.push(annotation);
        self.record(Step::Annotations(previous));
    }
}

pub fn show(app: &gtk::Application, cli: Cli) {
    theme::install();
    let Some(path) = cli.path else {
        return;
    };
    let image = match image::open(&path) {
        Ok(image) => image.into_rgba8(),
        Err(error) => {
            show_error(app, &format!("Could not open {}: {error}", path.display()));
            return;
        }
    };
    let surface = match surface_from_image(&image) {
        Ok(surface) => surface,
        Err(error) => {
            show_error(app, &error);
            return;
        }
    };

    let state = Rc::new(RefCell::new(State {
        path,
        image,
        surface,
        annotations: Vec::new(),
        active: None,
        undo: Vec::new(),
        redo: Vec::new(),
        tool: Tool::Pen,
        color: PALETTE[0].1,
        width: 4.0,
        selected_text: None,
    }));

    let (image_width, image_height) = {
        let state = state.borrow();
        (state.image.width() as i32, state.image.height() as i32)
    };
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Metis Screenshot")
        .default_width((image_width + 64).clamp(760, 1500))
        .default_height((image_height + 168).clamp(520, 950))
        .build();
    window.add_css_class("metis-screenshot-root");
    if crate::running_under_metis() {
        // Metis draws server-side decorations; keeping GTK's headerbar as well
        // would stack two titlebars on the same window.
        window.add_css_class("metis-screenshot-ssd");
        window.set_decorated(false);
        window.set_titlebar(gtk::Widget::NONE);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let status = gtk::Label::new(Some(Tool::Pen.hint()));
    status.add_css_class("metis-shot-status");
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

    let view = Rc::new(Cell::new((1.0_f64, 0.0_f64, 0.0_f64)));
    let caret_visible = Rc::new(Cell::new(true));
    let canvas = gtk::DrawingArea::new();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_focusable(true);
    canvas.set_draw_func({
        let state = state.clone();
        let view = view.clone();
        let caret_visible = caret_visible.clone();
        move |_, context, width, height| {
            let state = state.borrow();
            let image_width = state.image.width() as f64;
            let image_height = state.image.height() as f64;
            if image_width < 1.0 || image_height < 1.0 {
                return;
            }
            // Fit without upscaling so 1:1 captures stay pixel exact.
            let scale = (width as f64 / image_width)
                .min(height as f64 / image_height)
                .clamp(0.02, 1.0);
            let offset_x = ((width as f64 - image_width * scale) / 2.0).floor();
            let offset_y = ((height as f64 - image_height * scale) / 2.0).floor();
            view.set((scale, offset_x, offset_y));

            context.save().ok();
            context.translate(offset_x, offset_y);
            context.scale(scale, scale);
            if context.set_source_surface(&state.surface, 0.0, 0.0).is_ok() {
                let _ = context.paint();
            }
            for annotation in state.annotations.iter().chain(state.active.iter()) {
                draw_annotation(context, annotation);
            }
            if let Some(annotation) = state.active.as_ref().filter(|item| item.tool == Tool::Text) {
                draw_text_selection(context, annotation, false);
            }
            if let Some(index) = state.selected_text
                && let Some(annotation) = state.annotations.get(index)
            {
                draw_text_selection(context, annotation, caret_visible.get());
            }
            context.restore().ok();
        }
    });
    {
        let state = state.clone();
        let canvas = canvas.clone();
        let caret_visible = caret_visible.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if canvas.root().is_none() {
                return glib::ControlFlow::Break;
            }
            if state.borrow().selected_text.is_some() {
                caret_visible.set(!caret_visible.get());
                canvas.queue_draw();
            } else {
                caret_visible.set(true);
            }
            glib::ControlFlow::Continue
        });
    }

    let stage = gtk::Frame::new(None);
    stage.add_css_class("metis-shot-stage");
    stage.set_child(Some(&canvas));
    stage.set_hexpand(true);
    stage.set_vexpand(true);

    let info = gtk::Label::new(None);
    info.add_css_class("metis-shot-status");
    refresh_info(&info, &state.borrow());

    let feedback = Feedback {
        status: status.clone(),
        info: info.clone(),
    };
    let toolbar = build_toolbar(&window, &state, &canvas, &status, &info);
    let footer = build_footer(&window, &state, &canvas, &feedback);

    install_canvas_controllers(&canvas, state.clone(), view.clone(), feedback.clone());
    install_shortcuts(&window, &state, &canvas, &feedback, caret_visible.clone());

    root.append(&toolbar);
    root.append(&stage);
    root.append(&footer);
    window.set_child(Some(&root));
    window.present();
}

/// The two labels the canvas updates: transient feedback and the persistent
/// file/size readout.
#[derive(Clone)]
struct Feedback {
    status: gtk::Label,
    info: gtk::Label,
}

fn refresh_info(info: &gtk::Label, state: &State) {
    let name = state
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    info.set_text(&format!(
        "{name}  ·  {} × {}",
        state.image.width(),
        state.image.height()
    ));
}

fn build_toolbar(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    canvas: &gtk::DrawingArea,
    status: &gtk::Label,
    info: &gtk::Label,
) -> gtk::Box {
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.add_css_class("metis-shot-bar");

    let mut group: Option<gtk::ToggleButton> = None;
    for tool in Tool::ALL {
        // Split the drawing tools from the ones that rewrite pixels.
        if tool == Tool::Pixelate {
            toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        }
        let button = gtk::ToggleButton::new();
        button.add_css_class("metis-shot-tool");
        button.set_child(Some(&icons::image(tool.glyph(), 20)));
        button.set_tooltip_text(Some(tool.tooltip()));
        match &group {
            Some(first) => button.set_group(Some(first)),
            None => group = Some(button.clone()),
        }
        button.set_active(tool == Tool::Pen);
        let state = state.clone();
        let status = status.clone();
        let canvas = canvas.clone();
        let window = window.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.tool = tool;
                if tool != Tool::Text {
                    state.selected_text = None;
                }
            }
            if tool == Tool::Ocr {
                run_ocr(&window, &state, &status);
            } else {
                set_status(&status, tool.hint(), false);
            }
            canvas.queue_draw();
        });
        toolbar.append(&button);
    }

    toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    toolbar.append(&build_style_button(state, canvas));

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    info.set_margin_end(6);
    info.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    toolbar.append(info);
    toolbar
}

/// Colour and stroke width live behind one swatch button so the tool row stays
/// a single uncluttered strip.
fn build_style_button(state: &Rc<RefCell<State>>, canvas: &gtk::DrawingArea) -> gtk::MenuButton {
    let preview = gtk::DrawingArea::new();
    preview.set_content_width(18);
    preview.set_content_height(18);
    preview.set_draw_func({
        let state = state.clone();
        move |_, context, width, height| {
            let (r, g, b) = state.borrow().color;
            let radius = (width.min(height) as f64) / 2.0 - 3.0;
            context.arc(
                width as f64 / 2.0,
                height as f64 / 2.0,
                radius.max(2.0),
                0.0,
                std::f64::consts::TAU,
            );
            context.set_source_rgb(r, g, b);
            let _ = context.fill_preserve();
            context.set_source_rgba(1.0, 1.0, 1.0, 0.35);
            context.set_line_width(1.0);
            let _ = context.stroke();
        }
    });

    let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    popover_box.set_margin_top(10);
    popover_box.set_margin_bottom(10);
    popover_box.set_margin_start(10);
    popover_box.set_margin_end(10);

    let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let mut swatch_group: Option<gtk::ToggleButton> = None;
    for (index, (name, color)) in PALETTE.iter().enumerate() {
        let swatch = gtk::ToggleButton::new();
        swatch.add_css_class("metis-shot-swatch");
        swatch.set_tooltip_text(Some(name));
        let dot = gtk::DrawingArea::new();
        dot.set_content_width(18);
        dot.set_content_height(18);
        dot.set_can_target(false);
        let color = *color;
        dot.set_draw_func(move |_, context, width, height| {
            context.arc(
                width as f64 / 2.0,
                height as f64 / 2.0,
                (width.min(height) as f64) / 2.0 - 1.0,
                0.0,
                std::f64::consts::TAU,
            );
            context.set_source_rgb(color.0, color.1, color.2);
            let _ = context.fill();
        });
        swatch.set_child(Some(&dot));
        match &swatch_group {
            Some(first) => swatch.set_group(Some(first)),
            None => swatch_group = Some(swatch.clone()),
        }
        swatch.set_active(index == 0);
        let state = state.clone();
        let preview = preview.clone();
        let canvas = canvas.clone();
        swatch.connect_toggled(move |swatch| {
            if !swatch.is_active() {
                return;
            }
            state.borrow_mut().color = color;
            preview.queue_draw();
            canvas.queue_draw();
        });
        swatches.append(&swatch);
    }
    popover_box.append(&swatches);

    let width_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    width_row.append(&gtk::Label::new(Some("Size")));
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 24.0, 1.0);
    scale.set_value(state.borrow().width);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale.set_size_request(180, -1);
    {
        let state = state.clone();
        let canvas = canvas.clone();
        scale.connect_value_changed(move |scale| {
            state.borrow_mut().width = scale.value();
            canvas.queue_draw();
        });
    }
    width_row.append(&scale);
    popover_box.append(&width_row);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&popover_box));
    let button = gtk::MenuButton::new();
    button.add_css_class("metis-shot-action");
    button.add_css_class("flat");
    button.set_tooltip_text(Some("Colour and stroke size"));
    button.set_child(Some(&preview));
    button.set_popover(Some(&popover));
    button
}

fn build_footer(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    canvas: &gtk::DrawingArea,
    feedback: &Feedback,
) -> gtk::Box {
    let status = &feedback.status;
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    footer.append(status);

    let history = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    for (glyph, tooltip, kind) in [
        (Glyph::Undo, "Undo (Ctrl+Z)", HistoryAction::Undo),
        (Glyph::Redo, "Redo (Ctrl+Shift+Z)", HistoryAction::Redo),
        (Glyph::Trash, "Remove all annotations", HistoryAction::Clear),
    ] {
        let button = icon_button(glyph, None, tooltip);
        button.add_css_class("flat");
        let state = state.clone();
        let canvas = canvas.clone();
        let feedback = feedback.clone();
        button.connect_clicked(move |_| {
            let message = {
                let mut state = state.borrow_mut();
                let message = match kind {
                    HistoryAction::Undo => {
                        if step_history(&mut state, true) {
                            "Undid last edit"
                        } else {
                            "Nothing to undo"
                        }
                    }
                    HistoryAction::Redo => {
                        if step_history(&mut state, false) {
                            "Redid last edit"
                        } else {
                            "Nothing to redo"
                        }
                    }
                    HistoryAction::Clear => {
                        if state.annotations.is_empty() {
                            "No annotations to remove"
                        } else {
                            let previous = std::mem::take(&mut state.annotations);
                            state.selected_text = None;
                            state.record(Step::Annotations(previous));
                            "Removed all annotations"
                        }
                    }
                };
                // Undoing a crop restores the previous size.
                refresh_info(&feedback.info, &state);
                message
            };
            set_status(&feedback.status, message, false);
            canvas.queue_draw();
        });
        history.append(&button);
    }
    footer.append(&history);
    footer.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let copy = icon_button(
        Glyph::Copy,
        Some("Copy"),
        "Copy the image to the clipboard (Ctrl+C)",
    );
    connect_export_action(&copy, state.clone(), status.clone(), ExportAction::Copy);
    footer.append(&copy);

    let save = icon_button(
        Glyph::Save,
        Some("Save"),
        "Overwrite the capture file (Ctrl+S)",
    );
    connect_export_action(&save, state.clone(), status.clone(), ExportAction::Save);
    footer.append(&save);

    let save_as = icon_button(Glyph::SaveAs, Some("Save As"), "Save to another file");
    {
        let state = state.clone();
        let status = status.clone();
        let window = window.clone();
        save_as.connect_clicked(move |_| save_as_dialog(&window, state.clone(), status.clone()));
    }
    footer.append(&save_as);

    let pin_button = icon_button(
        Glyph::Pin,
        Some("Pin"),
        "Pin the image on top of the desktop",
    );
    {
        let state = state.clone();
        let status = status.clone();
        pin_button.connect_clicked(move |_| {
            let output = temporary_png_path();
            let result = export_png(&state.borrow(), &output).and_then(|()| pin::spawn(&output));
            match result {
                Ok(()) => set_status(&status, "Pinned screenshot", false),
                Err(error) => set_status(&status, &error, true),
            }
        });
    }
    footer.append(&pin_button);

    let done = icon_button(Glyph::Check, Some("Done"), "Close the editor (Esc)");
    done.add_css_class("suggested");
    {
        let window = window.clone();
        done.connect_clicked(move |_| window.close());
    }
    footer.append(&done);
    footer
}

#[derive(Clone, Copy)]
enum HistoryAction {
    Undo,
    Redo,
    Clear,
}

fn icon_button(glyph: Glyph, label: Option<&str>, tooltip: &str) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    content.append(&icons::image(glyph, 18));
    if let Some(label) = label {
        content.append(&gtk::Label::new(Some(label)));
    }
    let button = gtk::Button::new();
    button.add_css_class("metis-shot-action");
    button.set_child(Some(&content));
    button.set_tooltip_text(Some(tooltip));
    button
}

enum DragAction {
    Draw,
    CreateText,
    MoveText {
        index: usize,
        press: (f64, f64),
        start: ((f64, f64), (f64, f64)),
        before: Vec<Annotation>,
    },
    ResizeText {
        index: usize,
        before: Vec<Annotation>,
    },
}

fn install_canvas_controllers(
    canvas: &gtk::DrawingArea,
    state: Rc<RefCell<State>>,
    view: Rc<Cell<(f64, f64, f64)>>,
    feedback: Feedback,
) {
    let origin = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let action = Rc::new(RefCell::new(None::<DragAction>));
    let drag = gtk::GestureDrag::new();
    {
        let state = state.clone();
        let view = view.clone();
        let origin = origin.clone();
        let action = action.clone();
        let canvas = canvas.clone();
        drag.connect_drag_begin(move |_, x, y| {
            origin.set((x, y));
            let point = widget_to_image(&view, x, y);
            let mut state = state.borrow_mut();
            if state.tool == Tool::Text {
                let selected_handle = state.selected_text.filter(|&index| {
                    state
                        .annotations
                        .get(index)
                        .is_some_and(|item| text_resize_handle_hit(item, point))
                });
                if let Some(index) = selected_handle {
                    *action.borrow_mut() = Some(DragAction::ResizeText {
                        index,
                        before: state.annotations.clone(),
                    });
                    return;
                }
                if let Some(index) = hit_text_box(&state.annotations, point) {
                    state.selected_text = Some(index);
                    let start = state.annotations[index].span();
                    *action.borrow_mut() = Some(DragAction::MoveText {
                        index,
                        press: point,
                        start,
                        before: state.annotations.clone(),
                    });
                    canvas.grab_focus();
                    canvas.queue_draw();
                    return;
                }
                state.selected_text = None;
                state.active = Some(Annotation {
                    tool: Tool::Text,
                    points: vec![point, point],
                    color: state.color,
                    width: state.width,
                    text: String::new(),
                });
                *action.borrow_mut() = Some(DragAction::CreateText);
                canvas.grab_focus();
                canvas.queue_draw();
                return;
            }
            if state.tool == Tool::Ocr {
                return;
            }
            let annotation = Annotation {
                tool: state.tool,
                points: vec![point],
                color: state.color,
                width: state.width,
                text: String::new(),
            };
            state.active = Some(annotation);
            *action.borrow_mut() = Some(DragAction::Draw);
            drop(state);
            canvas.queue_draw();
        });
    }
    {
        let state = state.clone();
        let view = view.clone();
        let origin = origin.clone();
        let action = action.clone();
        let canvas = canvas.clone();
        // GestureDrag reports offsets from the press point, not widget
        // coordinates: add the origin back or every shape is drawn near 0,0.
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let (start_x, start_y) = origin.get();
            let point = widget_to_image(&view, start_x + offset_x, start_y + offset_y);
            let mut state = state.borrow_mut();
            match action.borrow().as_ref() {
                Some(DragAction::Draw | DragAction::CreateText) => {
                    if let Some(active) = state.active.as_mut() {
                        extend(active, point);
                    }
                }
                Some(DragAction::MoveText {
                    index,
                    press,
                    start,
                    ..
                }) => {
                    if let Some(annotation) = state.annotations.get_mut(*index) {
                        let dx = point.0 - press.0;
                        let dy = point.1 - press.1;
                        annotation.points = vec![
                            ((start.0).0 + dx, (start.0).1 + dy),
                            ((start.1).0 + dx, (start.1).1 + dy),
                        ];
                    }
                }
                Some(DragAction::ResizeText { index, .. }) => {
                    if let Some(annotation) = state.annotations.get_mut(*index) {
                        if annotation.points.len() < 2 {
                            annotation.points.push(point);
                        } else {
                            annotation.points[1] = point;
                        }
                        enforce_text_box_size(annotation);
                    }
                }
                None => {}
            }
            drop(state);
            canvas.queue_draw();
        });
    }
    {
        let canvas = canvas.clone();
        let action = action.clone();
        drag.connect_drag_end(move |_, offset_x, offset_y| {
            let (start_x, start_y) = origin.get();
            let point = widget_to_image(&view, start_x + offset_x, start_y + offset_y);
            let completed = action.borrow_mut().take();
            match completed {
                Some(DragAction::Draw) => {
                    let finished = {
                        let mut state = state.borrow_mut();
                        if let Some(active) = state.active.as_mut() {
                            extend(active, point);
                        }
                        state.active.take()
                    };
                    if let Some(annotation) = finished {
                        finish(annotation, &state, &feedback);
                    }
                }
                Some(DragAction::CreateText) => {
                    let mut state = state.borrow_mut();
                    if let Some(mut annotation) = state.active.take() {
                        extend(&mut annotation, point);
                        enforce_text_box_size(&mut annotation);
                        let previous = state.annotations.clone();
                        state.annotations.push(annotation);
                        state.selected_text = Some(state.annotations.len() - 1);
                        state.record(Step::Annotations(previous));
                        set_status(
                            &feedback.status,
                            "Type directly. Drag the box to move it; drag the corner to resize.",
                            false,
                        );
                        canvas.grab_focus();
                    }
                }
                Some(DragAction::MoveText { before, .. })
                | Some(DragAction::ResizeText { before, .. }) => {
                    state.borrow_mut().record(Step::Annotations(before));
                }
                None => {}
            }
            canvas.queue_draw();
        });
    }
    canvas.add_controller(drag);
    canvas.set_cursor(gtk::gdk::Cursor::from_name("crosshair", None).as_ref());
}

/// Freehand tools accumulate every sample; shapes only ever track the drag
/// origin and the live end point.
fn extend(annotation: &mut Annotation, point: (f64, f64)) {
    if annotation.tool.is_freehand() || annotation.points.len() < 2 {
        annotation.points.push(point);
    } else {
        annotation.points[1] = point;
    }
}

fn finish(annotation: Annotation, state: &Rc<RefCell<State>>, feedback: &Feedback) {
    let status = &feedback.status;
    match annotation.tool {
        Tool::Ocr | Tool::Text => {}
        Tool::Crop => {
            let mut state = state.borrow_mut();
            let Some(rect) = region(&state.image, &annotation) else {
                set_status(status, "Drag a larger region to crop", true);
                return;
            };
            match flatten(&state) {
                Ok(flattened) => {
                    let cropped =
                        image::imageops::crop_imm(&flattened, rect.0, rect.1, rect.2, rect.3)
                            .to_image();
                    let previous_image = std::mem::replace(&mut state.image, cropped);
                    let previous_annotations = std::mem::take(&mut state.annotations);
                    state.selected_text = None;
                    if let Err(error) = state.rebuild_surface() {
                        set_status(status, &error, true);
                        return;
                    }
                    state.record(Step::Image(previous_image, previous_annotations));
                    refresh_info(&feedback.info, &state);
                    let message = format!("Cropped to {} × {}", rect.2, rect.3);
                    drop(state);
                    set_status(status, &message, false);
                }
                Err(error) => set_status(status, &error, true),
            }
        }
        Tool::Pixelate => {
            let mut state = state.borrow_mut();
            let Some(rect) = region(&state.image, &annotation) else {
                set_status(status, "Drag a larger region to pixelate", true);
                return;
            };
            let previous_image = state.image.clone();
            let previous_annotations = state.annotations.clone();
            let block = (annotation.width * 2.5).round().max(6.0) as u32;
            pixelate(&mut state.image, rect, block);
            if let Err(error) = state.rebuild_surface() {
                state.image = previous_image;
                let _ = state.rebuild_surface();
                set_status(status, &error, true);
                return;
            }
            state.record(Step::Image(previous_image, previous_annotations));
            drop(state);
            set_status(status, "Pixelated region", false);
        }
        _ => {
            if annotation.points.len() < 2 {
                return;
            }
            state.borrow_mut().push_annotation(annotation);
        }
    }
}

fn install_shortcuts(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    canvas: &gtk::DrawingArea,
    feedback: &Feedback,
    caret_visible: Rc<Cell<bool>>,
) {
    let keys = gtk::EventControllerKey::new();
    let state = state.clone();
    let canvas = canvas.clone();
    let feedback = feedback.clone();
    let status = feedback.status.clone();
    let window_ref = window.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
        let text_selected = state.borrow().selected_text.is_some();
        if text_selected && !control {
            let edit = match key {
                gtk::gdk::Key::BackSpace => Some(TextEdit::Backspace),
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => Some(TextEdit::Insert('\n')),
                gtk::gdk::Key::Delete => Some(TextEdit::DeleteBox),
                _ => key
                    .to_unicode()
                    .filter(|character| !character.is_control())
                    .map(TextEdit::Insert),
            };
            if let Some(edit) = edit {
                edit_selected_text(&state, edit);
                caret_visible.set(true);
                canvas.queue_draw();
                return glib::Propagation::Stop;
            }
        }
        match key {
            gtk::gdk::Key::Escape => {
                if state.borrow().selected_text.is_some() {
                    state.borrow_mut().selected_text = None;
                    canvas.queue_draw();
                } else {
                    window_ref.close();
                }
                glib::Propagation::Stop
            }
            gtk::gdk::Key::z | gtk::gdk::Key::Z if control => {
                if !history_shortcut(&state, &feedback, !shift) {
                    set_status(&status, "Nothing to change", false);
                }
                canvas.queue_draw();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::y | gtk::gdk::Key::Y if control => {
                history_shortcut(&state, &feedback, false);
                canvas.queue_draw();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::c | gtk::gdk::Key::C if control => {
                run_export(&state, &status, ExportAction::Copy);
                glib::Propagation::Stop
            }
            gtk::gdk::Key::s | gtk::gdk::Key::S if control => {
                run_export(&state, &status, ExportAction::Save);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}

enum TextEdit {
    Insert(char),
    Backspace,
    DeleteBox,
}

fn edit_selected_text(state: &Rc<RefCell<State>>, edit: TextEdit) {
    let mut state = state.borrow_mut();
    let Some(index) = state.selected_text else {
        return;
    };
    let previous = state.annotations.clone();
    match edit {
        TextEdit::Insert(character) => {
            if let Some(annotation) = state.annotations.get_mut(index) {
                annotation.text.push(character);
            }
        }
        TextEdit::Backspace => {
            if let Some(annotation) = state.annotations.get_mut(index) {
                annotation.text.pop();
            }
        }
        TextEdit::DeleteBox => {
            if index < state.annotations.len() {
                state.annotations.remove(index);
                state.selected_text = None;
            }
        }
    }
    state.record(Step::Annotations(previous));
}

fn history_shortcut(state: &Rc<RefCell<State>>, feedback: &Feedback, undo: bool) -> bool {
    let mut state = state.borrow_mut();
    let moved = step_history(&mut state, undo);
    refresh_info(&feedback.info, &state);
    moved
}

fn step_history(state: &mut State, undo: bool) -> bool {
    let step = if undo {
        state.undo.pop()
    } else {
        state.redo.pop()
    };
    let Some(step) = step else {
        return false;
    };
    let inverse = match step {
        Step::Annotations(previous) => {
            Step::Annotations(std::mem::replace(&mut state.annotations, previous))
        }
        Step::Image(image, annotations) => {
            let previous_image = std::mem::replace(&mut state.image, image);
            let previous_annotations = std::mem::replace(&mut state.annotations, annotations);
            if let Err(error) = state.rebuild_surface() {
                tracing::warn!(%error, "unable to rebuild canvas after history step");
            }
            Step::Image(previous_image, previous_annotations)
        }
    };
    if undo {
        state.redo.push(inverse);
    } else {
        state.undo.push(inverse);
    }
    if state
        .selected_text
        .is_some_and(|index| index >= state.annotations.len())
    {
        state.selected_text = None;
    }
    true
}

fn enforce_text_box_size(annotation: &mut Annotation) {
    let ((x0, y0), (x1, y1)) = annotation.span();
    let direction_x = if x1 < x0 { -1.0 } else { 1.0 };
    let direction_y = if y1 < y0 { -1.0 } else { 1.0 };
    let width = (x1 - x0).abs().max(180.0);
    let height = (y1 - y0).abs().max(64.0);
    annotation.points = vec![
        (x0, y0),
        (x0 + width * direction_x, y0 + height * direction_y),
    ];
}

fn text_box(annotation: &Annotation) -> (f64, f64, f64, f64) {
    let ((x0, y0), (x1, y1)) = annotation.span();
    let width = (x1 - x0).abs().max(180.0);
    let height = (y1 - y0).abs().max(64.0);
    (x0.min(x1), y0.min(y1), width, height)
}

fn hit_text_box(annotations: &[Annotation], point: (f64, f64)) -> Option<usize> {
    annotations
        .iter()
        .enumerate()
        .rev()
        .find(|(_, annotation)| {
            if annotation.tool != Tool::Text {
                return false;
            }
            let (x, y, width, height) = text_box(annotation);
            point.0 >= x && point.0 <= x + width && point.1 >= y && point.1 <= y + height
        })
        .map(|(index, _)| index)
}

fn text_resize_handle_hit(annotation: &Annotation, point: (f64, f64)) -> bool {
    let (x, y, width, height) = text_box(annotation);
    let radius = 14.0;
    (point.0 - (x + width)).abs() <= radius && (point.1 - (y + height)).abs() <= radius
}

fn draw_text_selection(context: &cairo::Context, annotation: &Annotation, show_caret: bool) {
    let (x, y, width, height) = text_box(annotation);
    context.save().ok();
    context.set_source_rgba(0.20, 0.75, 1.0, 0.95);
    context.set_line_width(1.5);
    context.set_dash(&[5.0, 4.0], 0.0);
    context.rectangle(x, y, width, height);
    let _ = context.stroke();
    context.set_dash(&[], 0.0);
    context.arc(x + width, y + height, 6.0, 0.0, std::f64::consts::TAU);
    let _ = context.fill();
    if show_caret {
        draw_text_caret(context, annotation);
    }
    context.restore().ok();
}

fn draw_text_caret(context: &cairo::Context, annotation: &Annotation) {
    let size = (annotation.width * 6.0).max(18.0);
    let (x, y, width, height) = text_box(annotation);
    let content_width = (width - 16.0).max(1.0);
    context.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    context.set_font_size(size);
    let lines = wrap_text_lines(context, &annotation.text, content_width);
    let line_height = size * 1.22;
    let max_lines = ((height - 12.0) / line_height).floor().max(1.0) as usize;
    let Some(line) = lines.get(lines.len().saturating_sub(1).min(max_lines - 1)) else {
        return;
    };
    let advance = context
        .text_extents(line)
        .map(|extents| extents.x_advance())
        .unwrap_or(0.0);
    let line_index = lines.len().saturating_sub(1).min(max_lines - 1);
    let caret_x = (x + 8.0 + advance).min(x + width - 6.0);
    let caret_top = y + 7.0 + line_index as f64 * line_height;
    context.set_dash(&[], 0.0);
    context.set_source_rgba(0.20, 0.75, 1.0, 1.0);
    context.set_line_width(2.0);
    context.move_to(caret_x, caret_top);
    context.line_to(caret_x, (caret_top + size * 1.12).min(y + height - 5.0));
    let _ = context.stroke();
}

fn run_ocr(window: &gtk::ApplicationWindow, state: &Rc<RefCell<State>>, status: &gtk::Label) {
    set_status(status, "Extracting text from the full image…", false);
    let image = match flatten(&state.borrow()) {
        Ok(image) => image,
        Err(error) => {
            set_status(status, &error, true);
            return;
        }
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(ocr::run_image(&image));
    });
    let window = window.clone();
    let status = status.clone();
    glib::timeout_add_local(
        std::time::Duration::from_millis(50),
        move || match receiver.try_recv() {
            Ok(Ok(text)) => {
                set_status(
                    &status,
                    "Text extracted — select any passage or copy everything",
                    false,
                );
                show_ocr_results(&window, &text);
                glib::ControlFlow::Break
            }
            Ok(Err(error)) => {
                set_status(&status, &error, true);
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                set_status(&status, "OCR worker stopped unexpectedly", true);
                glib::ControlFlow::Break
            }
        },
    );
}

fn show_ocr_results(parent: &gtk::ApplicationWindow, text: &str) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .title("Extracted Text")
        .default_width(620)
        .default_height(520)
        .build();
    window.add_css_class("metis-screenshot-root");
    if crate::running_under_metis() {
        window.add_css_class("metis-screenshot-ssd");
        window.set_decorated(false);
        window.set_titlebar(gtk::Widget::NONE);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(14);
    root.set_margin_bottom(14);
    root.set_margin_start(14);
    root.set_margin_end(14);
    let heading = gtk::Label::new(Some("Extracted text"));
    heading.set_xalign(0.0);
    heading.add_css_class("title-3");
    root.append(&heading);
    let hint = gtk::Label::new(Some(
        "Drag to highlight text, then copy the selection—or copy all detected text.",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-shot-status");
    root.append(&hint);

    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(text);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_editable(false);
    view.set_cursor_visible(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(14);
    view.set_right_margin(14);
    view.set_top_margin(12);
    view.set_bottom_margin(12);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&view)
        .build();
    scroll.add_css_class("metis-shot-text-results");
    scroll.set_vexpand(true);
    root.append(&scroll);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let copy_selection = gtk::Button::with_label("Copy Selection");
    copy_selection.add_css_class("metis-shot-action");
    {
        let buffer = buffer.clone();
        copy_selection.connect_clicked(move |_| {
            if let Some((start, end)) = buffer.selection_bounds() {
                copy_text(&buffer.text(&start, &end, false));
            }
        });
    }
    let copy_all = gtk::Button::with_label("Copy All");
    copy_all.add_css_class("metis-shot-action");
    copy_all.add_css_class("suggested");
    {
        let text = text.to_string();
        copy_all.connect_clicked(move |_| copy_text(&text));
    }
    let close = gtk::Button::with_label("Close");
    close.add_css_class("metis-shot-action");
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    actions.append(&copy_selection);
    actions.append(&copy_all);
    actions.append(&close);
    root.append(&actions);
    window.set_child(Some(&root));
    window.present();
    view.grab_focus();
}

fn copy_text(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

fn draw_annotation(context: &cairo::Context, annotation: &Annotation) {
    let (r, g, b) = annotation.color;
    let ((x0, y0), (x1, y1)) = annotation.span();
    context.set_line_cap(cairo::LineCap::Round);
    context.set_line_join(cairo::LineJoin::Round);
    context.set_dash(&[], 0.0);

    match annotation.tool {
        Tool::Pen => {
            context.set_source_rgba(r, g, b, 1.0);
            context.set_line_width(annotation.width);
            trace_path(context, &annotation.points);
            let _ = context.stroke();
        }
        Tool::Highlighter => {
            context.set_source_rgba(r, g, b, 0.32);
            context.set_line_width(annotation.width * 4.0);
            context.set_line_cap(cairo::LineCap::Square);
            trace_path(context, &annotation.points);
            let _ = context.stroke();
        }
        Tool::Arrow => {
            let head = (annotation.width * 4.0).max(14.0);
            let angle = (y1 - y0).atan2(x1 - x0);
            context.set_source_rgba(r, g, b, 1.0);
            context.set_line_width(annotation.width);
            context.move_to(x0, y0);
            context.line_to(x1 - head * 0.8 * angle.cos(), y1 - head * 0.8 * angle.sin());
            let _ = context.stroke();
            context.move_to(x1, y1);
            for spread in [0.42, -0.42] {
                context.line_to(
                    x1 - head * (angle + spread).cos(),
                    y1 - head * (angle + spread).sin(),
                );
            }
            context.close_path();
            let _ = context.fill();
        }
        Tool::Rect => {
            context.set_source_rgba(r, g, b, 1.0);
            context.set_line_width(annotation.width);
            context.rectangle(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
            let _ = context.stroke();
        }
        Tool::Ellipse => {
            let (rx, ry) = ((x1 - x0).abs() / 2.0, (y1 - y0).abs() / 2.0);
            if rx < 0.5 || ry < 0.5 {
                return;
            }
            context.set_source_rgba(r, g, b, 1.0);
            context.save().ok();
            context.translate((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            context.scale(rx, ry);
            context.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
            context.restore().ok();
            context.set_line_width(annotation.width);
            let _ = context.stroke();
        }
        Tool::Text => {
            if annotation.text.is_empty() {
                return;
            }
            let size = (annotation.width * 6.0).max(18.0);
            let (x, y, width, height) = text_box(annotation);
            context.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            context.set_font_size(size);
            context.save().ok();
            context.rectangle(x, y, width, height);
            context.clip();
            draw_wrapped_text(
                context,
                &annotation.text,
                (x + 8.0, y + 6.0, width - 16.0, height - 12.0),
                size,
                annotation.color,
            );
            context.restore().ok();
        }
        Tool::Pixelate | Tool::Crop | Tool::Ocr => {
            // Only ever drawn as the live selection preview; the committed
            // result is baked into the pixel layer.
            context.set_source_rgba(r, g, b, 0.9);
            context.set_line_width(1.5);
            context.set_dash(&[6.0, 4.0], 0.0);
            context.rectangle(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
            let _ = context.stroke_preserve();
            context.set_source_rgba(r, g, b, 0.12);
            let _ = context.fill();
            context.set_dash(&[], 0.0);
        }
    }
}

fn draw_wrapped_text(
    context: &cairo::Context,
    text: &str,
    bounds: (f64, f64, f64, f64),
    size: f64,
    color: (f64, f64, f64),
) {
    let (x, y, width, height) = bounds;
    let line_height = size * 1.22;
    let mut cursor_y = y + size;
    for line in wrap_text_lines(context, text, width) {
        if cursor_y > y + height {
            return;
        }
        if !line.is_empty() {
            draw_text_line(context, &line, x, cursor_y, size, color);
        }
        cursor_y += line_height;
    }
}

/// Character-aware wrapping preserves repeated/trailing spaces and explicit
/// newlines while the user types, so the caret always follows the real buffer.
fn wrap_text_lines(context: &cairo::Context, text: &str, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for character in text.chars() {
        if character == '\n' {
            lines.push(std::mem::take(&mut line));
            continue;
        }
        let mut candidate = line.clone();
        candidate.push(character);
        let advance = context
            .text_extents(&candidate)
            .map(|extents| extents.x_advance())
            .unwrap_or(0.0);
        if advance > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        line.push(character);
    }
    lines.push(line);
    lines
}

fn draw_text_line(
    context: &cairo::Context,
    text: &str,
    x: f64,
    y: f64,
    size: f64,
    color: (f64, f64, f64),
) {
    context.move_to(x, y);
    context.set_source_rgba(0.0, 0.0, 0.0, 0.55);
    context.set_line_width((size / 8.0).max(2.0));
    context.text_path(text);
    let _ = context.stroke_preserve();
    context.set_source_rgba(color.0, color.1, color.2, 1.0);
    let _ = context.fill();
}

fn trace_path(context: &cairo::Context, points: &[(f64, f64)]) {
    let Some(&(x, y)) = points.first() else {
        return;
    };
    context.move_to(x, y);
    if points.len() == 1 {
        context.line_to(x + 0.01, y);
        return;
    }
    for &(x, y) in &points[1..] {
        context.line_to(x, y);
    }
}

fn widget_to_image(view: &Cell<(f64, f64, f64)>, x: f64, y: f64) -> (f64, f64) {
    let (scale, offset_x, offset_y) = view.get();
    let scale = if scale.abs() < f64::EPSILON {
        1.0
    } else {
        scale
    };
    ((x - offset_x) / scale, (y - offset_y) / scale)
}

/// Clamp a drag to a pixel rectangle inside the image, or `None` when the drag
/// was too small to be a deliberate selection.
fn region(image: &image::RgbaImage, annotation: &Annotation) -> Option<(u32, u32, u32, u32)> {
    let ((x0, y0), (x1, y1)) = annotation.span();
    let left = x0.min(x1).max(0.0).round() as u32;
    let top = y0.min(y1).max(0.0).round() as u32;
    let right = (x0.max(x1).round() as i64).clamp(0, image.width() as i64) as u32;
    let bottom = (y0.max(y1).round() as i64).clamp(0, image.height() as i64) as u32;
    if left >= image.width() || top >= image.height() {
        return None;
    }
    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    if width < 4 || height < 4 {
        return None;
    }
    Some((left, top, width, height))
}

fn pixelate(image: &mut image::RgbaImage, rect: (u32, u32, u32, u32), block: u32) {
    let (left, top, width, height) = rect;
    let block = block.max(2);
    let mut y = top;
    while y < top + height {
        let mut x = left;
        while x < left + width {
            let cell_w = block.min(left + width - x);
            let cell_h = block.min(top + height - y);
            let count = u64::from(cell_w) * u64::from(cell_h);
            if count == 0 {
                x += block;
                continue;
            }
            let mut totals = [0u64; 4];
            for py in y..y + cell_h {
                for px in x..x + cell_w {
                    let pixel = image.get_pixel(px, py).0;
                    for (total, channel) in totals.iter_mut().zip(pixel) {
                        *total += u64::from(channel);
                    }
                }
            }
            let average = image::Rgba([
                (totals[0] / count) as u8,
                (totals[1] / count) as u8,
                (totals[2] / count) as u8,
                (totals[3] / count) as u8,
            ]);
            for py in y..y + cell_h {
                for px in x..x + cell_w {
                    image.put_pixel(px, py, average);
                }
            }
            x += block;
        }
        y += block;
    }
}

fn surface_from_image(image: &image::RgbaImage) -> Result<cairo::ImageSurface, String> {
    let width = image.width() as i32;
    let height = image.height() as i32;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|error| format!("allocate canvas: {error}"))?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface
            .data()
            .map_err(|error| format!("lock canvas: {error}"))?;
        for y in 0..image.height() {
            let row = y as usize * stride;
            for x in 0..image.width() {
                let pixel = image.get_pixel(x, y).0;
                let alpha = pixel[3] as u32;
                // Cairo's ARGB32 is premultiplied and native-endian, which is
                // B, G, R, A byte order on the little-endian targets Metis ships.
                let premultiply = |channel: u8| ((channel as u32 * alpha + 127) / 255) as u8;
                let index = row + x as usize * 4;
                data[index] = premultiply(pixel[2]);
                data[index + 1] = premultiply(pixel[1]);
                data[index + 2] = premultiply(pixel[0]);
                data[index + 3] = alpha as u8;
            }
        }
    }
    surface.mark_dirty();
    Ok(surface)
}

fn image_from_surface(surface: &mut cairo::ImageSurface) -> Result<image::RgbaImage, String> {
    let width = surface.width() as u32;
    let height = surface.height() as u32;
    let stride = surface.stride() as usize;
    let data = surface
        .data()
        .map_err(|error| format!("read canvas: {error}"))?;
    let mut output = image::RgbaImage::new(width, height);
    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let index = row + x as usize * 4;
            let alpha = data[index + 3];
            let restore = |channel: u8| match u32::from(alpha) {
                0 => 0,
                alpha => ((u32::from(channel) * 255 + alpha / 2) / alpha).min(255) as u8,
            };
            output.put_pixel(
                x,
                y,
                image::Rgba([
                    restore(data[index + 2]),
                    restore(data[index + 1]),
                    restore(data[index]),
                    alpha,
                ]),
            );
        }
    }
    Ok(output)
}

/// Render the pixel layer plus every annotation into one surface, so exports and
/// crops always use exactly what the canvas shows.
fn composite(state: &State) -> Result<cairo::ImageSurface, String> {
    let surface = cairo::ImageSurface::create(
        cairo::Format::ARgb32,
        state.image.width() as i32,
        state.image.height() as i32,
    )
    .map_err(|error| format!("allocate export canvas: {error}"))?;
    {
        let context =
            cairo::Context::new(&surface).map_err(|error| format!("draw export: {error}"))?;
        context
            .set_source_surface(&state.surface, 0.0, 0.0)
            .map_err(|error| format!("draw export source: {error}"))?;
        context
            .paint()
            .map_err(|error| format!("paint export: {error}"))?;
        for annotation in &state.annotations {
            draw_annotation(&context, annotation);
        }
    }
    surface.flush();
    Ok(surface)
}

fn flatten(state: &State) -> Result<image::RgbaImage, String> {
    let mut surface = composite(state)?;
    image_from_surface(&mut surface)
}

#[derive(Clone, Copy)]
enum ExportAction {
    Copy,
    Save,
}

fn connect_export_action(
    button: &gtk::Button,
    state: Rc<RefCell<State>>,
    status: gtk::Label,
    action: ExportAction,
) {
    button.connect_clicked(move |_| run_export(&state, &status, action));
}

fn run_export(state: &Rc<RefCell<State>>, status: &gtk::Label, action: ExportAction) {
    let output = match action {
        ExportAction::Copy => temporary_png_path(),
        ExportAction::Save => state.borrow().path.clone(),
    };
    if let Err(error) = export_png(&state.borrow(), &output) {
        set_status(status, &error, true);
        return;
    }
    match action {
        ExportAction::Copy => match copy_png(&output) {
            Ok(()) => set_status(status, "Copied image to clipboard", false),
            Err(error) => set_status(
                status,
                &format!("{error} — saved a copy to {}", output.display()),
                true,
            ),
        },
        ExportAction::Save => set_status(status, &format!("Saved {}", output.display()), false),
    }
}

fn save_as_dialog(window: &gtk::ApplicationWindow, state: Rc<RefCell<State>>, status: gtk::Label) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Save annotated screenshot");
    if let Some(name) = state
        .borrow()
        .path
        .file_name()
        .and_then(|name| name.to_str())
    {
        dialog.set_initial_name(Some(name));
    }
    dialog.save(
        Some(window),
        gio::Cancellable::NONE,
        move |result| match result.and_then(|file| {
            file.path().ok_or_else(|| {
                glib::Error::new(
                    gio::IOErrorEnum::InvalidArgument,
                    "A local file is required",
                )
            })
        }) {
            Ok(path) => match export_png(&state.borrow(), &path) {
                Ok(()) => set_status(&status, &format!("Saved {}", path.display()), false),
                Err(error) => set_status(&status, &error, true),
            },
            Err(error) if error.matches(gtk::DialogError::Dismissed) => {}
            Err(error) => set_status(&status, &format!("Save failed: {error}"), true),
        },
    );
}

fn export_png(state: &State, path: &Path) -> Result<(), String> {
    let flattened = flatten(state)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    flattened
        .save(path)
        .map_err(|error| format!("save PNG: {error}"))
}

fn temporary_png_path() -> PathBuf {
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("metis");
    let _ = std::fs::create_dir_all(&root);
    root.join(format!("screenshot-{}.png", std::process::id()))
}

fn copy_png(path: &Path) -> Result<(), String> {
    let status = std::process::Command::new("wl-copy")
        .args(["-t", "image/png"])
        .arg(path)
        .status()
        .map_err(|error| format!("wl-copy unavailable: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("wl-copy failed".into())
    }
}

fn set_status(label: &gtk::Label, message: &str, failed: bool) {
    label.set_text(message);
    label.set_tooltip_text(Some(message));
    if failed {
        label.add_css_class("warn");
    } else {
        label.remove_css_class("warn");
    }
}

fn show_error(app: &gtk::Application, message: &str) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Metis Screenshot")
        .default_width(480)
        .build();
    window.add_css_class("metis-screenshot-root");
    if crate::running_under_metis() {
        window.add_css_class("metis-screenshot-ssd");
        window.set_decorated(false);
        window.set_titlebar(gtk::Widget::NONE);
    }
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_margin_top(24);
    label.set_margin_bottom(24);
    label.set_margin_start(24);
    label.set_margin_end(24);
    window.set_child(Some(&label));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn annotation(tool: Tool, points: Vec<(f64, f64)>) -> Annotation {
        Annotation {
            tool,
            points,
            color: (1.0, 0.0, 0.0),
            width: 3.0,
            text: String::new(),
        }
    }

    fn canvas(size: u32) -> State {
        let image = image::RgbaImage::from_pixel(size, size, image::Rgba(WHITE));
        let surface = surface_from_image(&image).expect("build surface");
        State {
            path: PathBuf::from("/tmp/metis-editor-test.png"),
            image,
            surface,
            annotations: Vec::new(),
            active: None,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Rect,
            color: (1.0, 0.0, 0.0),
            width: 3.0,
            selected_text: None,
        }
    }

    #[test]
    fn widget_coordinates_map_back_through_the_fit_transform() {
        let view = Cell::new((0.5, 20.0, 10.0));
        assert_eq!(widget_to_image(&view, 20.0, 10.0), (0.0, 0.0));
        assert_eq!(widget_to_image(&view, 120.0, 110.0), (200.0, 200.0));
    }

    #[test]
    fn shapes_keep_the_origin_and_track_the_live_end_point() {
        let mut shape = annotation(Tool::Rect, vec![(1.0, 1.0)]);
        extend(&mut shape, (5.0, 5.0));
        extend(&mut shape, (9.0, 7.0));
        assert_eq!(shape.points, vec![(1.0, 1.0), (9.0, 7.0)]);
    }

    #[test]
    fn freehand_keeps_every_sample() {
        let mut stroke = annotation(Tool::Pen, vec![(1.0, 1.0)]);
        extend(&mut stroke, (2.0, 2.0));
        extend(&mut stroke, (3.0, 3.0));
        assert_eq!(stroke.points.len(), 3);
    }

    #[test]
    fn text_boxes_have_a_usable_minimum_and_can_be_hit() {
        let mut text = annotation(Tool::Text, vec![(20.0, 30.0), (21.0, 31.0)]);
        enforce_text_box_size(&mut text);
        assert_eq!(text_box(&text), (20.0, 30.0, 180.0, 64.0));
        assert_eq!(hit_text_box(&[text.clone()], (50.0, 50.0)), Some(0));
        assert!(text_resize_handle_hit(&text, (200.0, 94.0)));
        assert_eq!(hit_text_box(&[text], (5.0, 5.0)), None);
    }

    #[test]
    fn typing_updates_the_selected_text_box_and_is_undoable() {
        let state = Rc::new(RefCell::new(canvas(240)));
        {
            let mut state = state.borrow_mut();
            state
                .annotations
                .push(annotation(Tool::Text, vec![(10.0, 10.0), (200.0, 80.0)]));
            state.selected_text = Some(0);
        }
        edit_selected_text(&state, TextEdit::Insert('M'));
        edit_selected_text(&state, TextEdit::Insert('e'));
        assert_eq!(state.borrow().annotations[0].text, "Me");
        assert!(step_history(&mut state.borrow_mut(), true));
        assert_eq!(state.borrow().annotations[0].text, "M");
    }

    /// Guards the drag-offset regression: shapes must land under the pointer
    /// rather than collapsing towards the image origin.
    #[test]
    fn export_draws_shapes_where_they_were_dragged() {
        let mut state = canvas(40);
        state
            .annotations
            .push(annotation(Tool::Rect, vec![(8.0, 8.0), (30.0, 30.0)]));
        let flattened = flatten(&state).expect("flatten");

        let edge = flattened.get_pixel(8, 8).0;
        assert!(
            edge[0] > 200 && edge[1] < 90,
            "rectangle edge should be red, got {edge:?}"
        );
        assert_eq!(flattened.get_pixel(20, 20).0, WHITE, "interior stays clear");
        assert_eq!(flattened.get_pixel(2, 2).0, WHITE, "origin stays clear");
    }

    #[test]
    fn round_tripping_through_cairo_preserves_pixels() {
        let mut source = image::RgbaImage::from_pixel(4, 4, image::Rgba([12, 200, 90, 255]));
        source.put_pixel(1, 2, image::Rgba([255, 0, 0, 255]));
        let state = State {
            image: source.clone(),
            surface: surface_from_image(&source).expect("build surface"),
            ..canvas(4)
        };
        assert_eq!(flatten(&state).expect("flatten"), source);
    }

    #[test]
    fn tiny_drags_are_not_treated_as_a_region() {
        let image = image::RgbaImage::new(40, 40);
        assert!(
            region(
                &image,
                &annotation(Tool::Crop, vec![(5.0, 5.0), (7.0, 6.0)])
            )
            .is_none()
        );
        assert_eq!(
            region(
                &image,
                &annotation(Tool::Crop, vec![(30.0, 30.0), (5.0, 5.0)])
            ),
            Some((5, 5, 25, 25)),
            "a drag up-left still yields a positive rectangle"
        );
        assert_eq!(
            region(
                &image,
                &annotation(Tool::Crop, vec![(20.0, 20.0), (99.0, 99.0)])
            ),
            Some((20, 20, 20, 20)),
            "regions are clamped to the image"
        );
    }

    #[test]
    fn pixelate_flattens_each_block_to_one_colour() {
        let mut image = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        image.put_pixel(0, 0, image::Rgba([100, 100, 100, 255]));
        image.put_pixel(1, 1, image::Rgba([100, 100, 100, 255]));
        pixelate(&mut image, (0, 0, 4, 4), 4);
        assert_eq!(image.get_pixel(0, 0), image.get_pixel(3, 3));
        assert_eq!(
            image.get_pixel(0, 0).0[0],
            12,
            "block averages its 16 pixels"
        );
        assert_eq!(
            image.get_pixel(5, 5).0,
            [0, 0, 0, 255],
            "pixels outside the region are untouched"
        );
    }

    #[test]
    fn undo_restores_the_previous_annotation_list_then_redo_replays_it() {
        let mut state = canvas(16);
        state.push_annotation(annotation(Tool::Arrow, vec![(1.0, 1.0), (9.0, 9.0)]));
        assert_eq!(state.annotations.len(), 1);
        assert!(step_history(&mut state, true));
        assert!(state.annotations.is_empty());
        assert!(step_history(&mut state, false));
        assert_eq!(state.annotations.len(), 1);
        assert!(!step_history(&mut state, false));
    }
}
