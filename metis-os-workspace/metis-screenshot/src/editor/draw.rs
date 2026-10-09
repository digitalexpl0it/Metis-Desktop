//! Annotation drawing and OCR results UI.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

use crate::ocr;

use super::canvas::text_box;
use super::export::flatten;
use super::set_status;
use super::state::{Annotation, State, Tool};

pub(crate) fn draw_text_selection(
    context: &cairo::Context,
    annotation: &Annotation,
    show_caret: bool,
) {
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

pub(crate) fn draw_text_caret(context: &cairo::Context, annotation: &Annotation) {
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

pub(crate) fn run_ocr(
    window: &gtk::ApplicationWindow,
    state: &Rc<RefCell<State>>,
    status: &gtk::Label,
) {
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

pub(crate) fn show_ocr_results(parent: &gtk::ApplicationWindow, text: &str) {
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

pub(crate) fn copy_text(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

pub(crate) fn draw_annotation(context: &cairo::Context, annotation: &Annotation) {
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

pub(crate) fn draw_wrapped_text(
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
pub(crate) fn wrap_text_lines(context: &cairo::Context, text: &str, width: f64) -> Vec<String> {
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

pub(crate) fn draw_text_line(
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

pub(crate) fn trace_path(context: &cairo::Context, points: &[(f64, f64)]) {
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
