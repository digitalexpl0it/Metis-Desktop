//! Export, clipboard, and image compositing helpers.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

use super::draw::draw_annotation;
use super::set_status;
use super::state::{Annotation, State};

/// Clamp a drag to a pixel rectangle inside the image, or `None` when the drag
/// was too small to be a deliberate selection.
pub(crate) fn region(
    image: &image::RgbaImage,
    annotation: &Annotation,
) -> Option<(u32, u32, u32, u32)> {
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

pub(crate) fn pixelate(image: &mut image::RgbaImage, rect: (u32, u32, u32, u32), block: u32) {
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

pub(crate) fn image_from_surface(
    surface: &mut cairo::ImageSurface,
) -> Result<image::RgbaImage, String> {
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
pub(crate) fn composite(state: &State) -> Result<cairo::ImageSurface, String> {
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

pub(crate) fn flatten(state: &State) -> Result<image::RgbaImage, String> {
    let mut surface = composite(state)?;
    image_from_surface(&mut surface)
}

#[derive(Clone, Copy)]
pub(crate) enum ExportAction {
    Copy,
    Save,
}

pub(crate) fn connect_export_action(
    button: &gtk::Button,
    state: Rc<RefCell<State>>,
    status: gtk::Label,
    action: ExportAction,
) {
    button.connect_clicked(move |_| run_export(&state, &status, action));
}

pub(crate) fn run_export(state: &Rc<RefCell<State>>, status: &gtk::Label, action: ExportAction) {
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

pub(crate) fn save_as_dialog(
    window: &gtk::ApplicationWindow,
    state: Rc<RefCell<State>>,
    status: gtk::Label,
) {
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

pub(crate) fn export_png(state: &State, path: &Path) -> Result<(), String> {
    let flattened = flatten(state)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    flattened
        .save(path)
        .map_err(|error| format!("save PNG: {error}"))
}

pub(crate) fn temporary_png_path() -> PathBuf {
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("metis");
    let _ = std::fs::create_dir_all(&root);
    root.join(format!("screenshot-{}.png", std::process::id()))
}

pub(crate) fn copy_png(path: &Path) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;

    use super::super::canvas::{
        TextEdit, edit_selected_text, enforce_text_box_size, extend, hit_text_box, step_history,
        text_box, text_resize_handle_hit, widget_to_image,
    };
    use super::super::state::{Annotation, State, Tool, surface_from_image};
    use super::{flatten, pixelate, region};

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
