//! Annotation editor for a captured PNG.
//!
//! The canvas keeps two layers: the pixel `image` (mutated only by destructive
//! operations such as crop and pixelate) and a vector list of `Annotation`s that
//! is re-rendered every frame. Export re-runs the exact same Cairo drawing code
//! against an offscreen surface, so the saved file always matches the preview.

mod canvas;
mod draw;
mod export;
mod state;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::icons::{self, Glyph};
use crate::{Cli, pin, theme};

use canvas::{install_canvas_controllers, install_shortcuts, step_history};
use draw::{draw_annotation, draw_text_selection, run_ocr};
use export::{ExportAction, connect_export_action, export_png, save_as_dialog, temporary_png_path};
use state::{PALETTE, State, Step, Tool, surface_from_image};

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
pub(crate) struct Feedback {
    pub(crate) status: gtk::Label,
    pub(crate) info: gtk::Label,
}

pub(crate) fn refresh_info(info: &gtk::Label, state: &State) {
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

pub(crate) fn build_toolbar(
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
pub(crate) fn build_style_button(
    state: &Rc<RefCell<State>>,
    canvas: &gtk::DrawingArea,
) -> gtk::MenuButton {
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

pub(crate) fn build_footer(
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
pub(crate) enum HistoryAction {
    Undo,
    Redo,
    Clear,
}

pub(crate) fn icon_button(glyph: Glyph, label: Option<&str>, tooltip: &str) -> gtk::Button {
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

pub(crate) fn set_status(label: &gtk::Label, message: &str, failed: bool) {
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
