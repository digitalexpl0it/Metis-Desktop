//! Editor tool, annotation, and undo state.

use std::path::PathBuf;

use gtk::cairo;

use crate::icons::Glyph;

pub(crate) const PALETTE: [(&str, (f64, f64, f64)); 6] = [
    ("Red", (0.95, 0.25, 0.21)),
    ("Amber", (1.0, 0.72, 0.11)),
    ("Green", (0.24, 0.79, 0.44)),
    ("Blue", (0.24, 0.60, 0.98)),
    ("White", (1.0, 1.0, 1.0)),
    ("Black", (0.06, 0.07, 0.09)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
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
    pub(crate) const ALL: [Tool; 9] = [
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

    pub(crate) fn glyph(self) -> Glyph {
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

    pub(crate) fn tooltip(self) -> &'static str {
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

    pub(crate) fn hint(self) -> &'static str {
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

    pub(crate) fn is_freehand(self) -> bool {
        matches!(self, Tool::Pen | Tool::Highlighter)
    }
}

#[derive(Clone)]
pub(crate) struct Annotation {
    pub(crate) tool: Tool,
    pub(crate) points: Vec<(f64, f64)>,
    pub(crate) color: (f64, f64, f64),
    pub(crate) width: f64,
    pub(crate) text: String,
}

impl Annotation {
    pub(crate) fn span(&self) -> ((f64, f64), (f64, f64)) {
        let first = self.points.first().copied().unwrap_or((0.0, 0.0));
        let last = self.points.last().copied().unwrap_or(first);
        (first, last)
    }
}

/// One reversible edit. Vector edits only store the previous annotation list;
/// full image copies are kept exclusively for crop and pixelate, which cannot be
/// replayed from vectors.
pub(crate) enum Step {
    Annotations(Vec<Annotation>),
    Image(image::RgbaImage, Vec<Annotation>),
}

pub(crate) const MAX_HISTORY: usize = 40;

pub(crate) struct State {
    pub(crate) path: PathBuf,
    pub(crate) image: image::RgbaImage,
    pub(crate) surface: cairo::ImageSurface,
    pub(crate) annotations: Vec<Annotation>,
    pub(crate) active: Option<Annotation>,
    pub(crate) undo: Vec<Step>,
    pub(crate) redo: Vec<Step>,
    pub(crate) tool: Tool,
    pub(crate) color: (f64, f64, f64),
    pub(crate) width: f64,
    pub(crate) selected_text: Option<usize>,
}

impl State {
    pub(crate) fn rebuild_surface(&mut self) -> Result<(), String> {
        self.surface = surface_from_image(&self.image)?;
        Ok(())
    }

    pub(crate) fn record(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub(crate) fn push_annotation(&mut self, annotation: Annotation) {
        let previous = self.annotations.clone();
        self.annotations.push(annotation);
        self.record(Step::Annotations(previous));
    }
}

pub(crate) fn surface_from_image(image: &image::RgbaImage) -> Result<cairo::ImageSurface, String> {
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
