//! Canvas input controllers and keyboard shortcuts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use super::export::{ExportAction, flatten, pixelate, region, run_export};
use super::state::{Annotation, State, Step, Tool};
use super::{Feedback, refresh_info, set_status};

pub(crate) enum DragAction {
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

pub(crate) fn install_canvas_controllers(
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
pub(crate) fn extend(annotation: &mut Annotation, point: (f64, f64)) {
    if annotation.tool.is_freehand() || annotation.points.len() < 2 {
        annotation.points.push(point);
    } else {
        annotation.points[1] = point;
    }
}

pub(crate) fn finish(annotation: Annotation, state: &Rc<RefCell<State>>, feedback: &Feedback) {
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

pub(crate) fn install_shortcuts(
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

pub(crate) enum TextEdit {
    Insert(char),
    Backspace,
    DeleteBox,
}

pub(crate) fn edit_selected_text(state: &Rc<RefCell<State>>, edit: TextEdit) {
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

pub(crate) fn history_shortcut(
    state: &Rc<RefCell<State>>,
    feedback: &Feedback,
    undo: bool,
) -> bool {
    let mut state = state.borrow_mut();
    let moved = step_history(&mut state, undo);
    refresh_info(&feedback.info, &state);
    moved
}

pub(crate) fn step_history(state: &mut State, undo: bool) -> bool {
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

pub(crate) fn enforce_text_box_size(annotation: &mut Annotation) {
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

pub(crate) fn text_box(annotation: &Annotation) -> (f64, f64, f64, f64) {
    let ((x0, y0), (x1, y1)) = annotation.span();
    let width = (x1 - x0).abs().max(180.0);
    let height = (y1 - y0).abs().max(64.0);
    (x0.min(x1), y0.min(y1), width, height)
}

pub(crate) fn hit_text_box(annotations: &[Annotation], point: (f64, f64)) -> Option<usize> {
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

pub(crate) fn text_resize_handle_hit(annotation: &Annotation, point: (f64, f64)) -> bool {
    let (x, y, width, height) = text_box(annotation);
    let radius = 14.0;
    (point.0 - (x + width)).abs() <= radius && (point.1 - (y + height)).abs() <= radius
}

pub(crate) fn widget_to_image(view: &Cell<(f64, f64, f64)>, x: f64, y: f64) -> (f64, f64) {
    let (scale, offset_x, offset_y) = view.get();
    let scale = if scale.abs() < f64::EPSILON {
        1.0
    } else {
        scale
    };
    ((x - offset_x) / scale, (y - offset_y) / scale)
}
