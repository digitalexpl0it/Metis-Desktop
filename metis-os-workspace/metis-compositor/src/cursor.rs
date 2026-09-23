//! XCursor theme loading for the DRM backend's software/hardware cursor.
//!
//! The nested winit backend draws the host cursor, so this is only used by the
//! standalone DRM session, which must paint its own pointer. We load the named
//! `default` cursor from the active XCursor theme once and hand its RGBA frames
//! to the renderer (cached as `MemoryRenderBuffer`s in `UdevState`).

use std::io::Read;

use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

/// A loaded pointer cursor (one or more animation frames at various sizes).
pub struct XCursor {
    images: Vec<Image>,
    size: u32,
}

impl XCursor {
    /// Load the `default` pointer from `theme` at nominal `size`, falling back to
    /// `left_ptr` and finally a generated arrow if the theme has neither.
    pub fn load(theme: &str, size: u32) -> Self {
        let theme_obj = CursorTheme::load(theme);
        let images = load_named(&theme_obj, "default")
            .or_else(|| load_named(&theme_obj, "left_ptr"))
            .unwrap_or_else(fallback_images);
        XCursor {
            images: if images.is_empty() {
                fallback_images()
            } else {
                images
            },
            size: size.max(1),
        }
    }

    /// Pick the frame nearest the requested size for the given animation time.
    pub fn frame(&self, millis: u32) -> &Image {
        let nearest = self
            .images
            .iter()
            .min_by_key(|img| (self.size as i32 - img.size as i32).abs())
            .expect("cursor always has at least one frame");
        let frames: Vec<&Image> = self
            .images
            .iter()
            .filter(|img| img.width == nearest.width && img.height == nearest.height)
            .collect();
        let total: u32 = frames.iter().map(|f| f.delay).sum();
        if total == 0 {
            return frames[0];
        }
        let mut t = millis % total;
        for f in &frames {
            if t < f.delay {
                return f;
            }
            t -= f.delay;
        }
        frames[0]
    }

    /// Load a named cursor from `theme` (e.g. `ew-resize`) and pick a frame.
    /// Falls back to [`Self::frame`] when the name is missing from the theme.
    pub fn frame_named(&self, theme: &str, name: &str, millis: u32) -> Image {
        let theme_obj = CursorTheme::load(theme);
        if let Some(images) = load_named(&theme_obj, name) {
            return pick_frame(&images, self.size, millis);
        }
        self.frame(millis).clone()
    }

    /// Resize cursor for `edge`: try every standard XCursor alias in `theme`, then
    /// synthesize a directional shape. Unlike [`Self::frame_named`], this never falls
    /// back to the default pointer — that fallback looked identical to client cursors
    /// on the DRM backend and made edge hover appear broken.
    pub fn frame_resize(&self, theme: &str, edge: crate::grabs::ResizeEdge, millis: u32) -> Image {
        let theme_obj = CursorTheme::load(theme);
        for name in resize_cursor_names(edge) {
            if let Some(images) = load_named(&theme_obj, name) {
                return pick_frame(&images, self.size, millis);
            }
        }
        synthesize_resize_image(edge, self.size)
    }
}

/// Standard XCursor names to try for a resize edge (most themes only ship a subset).
pub fn resize_cursor_names(edge: crate::grabs::ResizeEdge) -> &'static [&'static str] {
    use crate::grabs::ResizeEdge;
    if edge == ResizeEdge::TOP_LEFT || edge == ResizeEdge::BOTTOM_RIGHT {
        &[
            "nesw-resize",
            "size_fdiag",
            "bd_double_arrow",
            "top_right_corner",
        ]
    } else if edge == ResizeEdge::TOP_RIGHT || edge == ResizeEdge::BOTTOM_LEFT {
        &[
            "nwse-resize",
            "size_bdiag",
            "fd_double_arrow",
            "top_left_corner",
        ]
    } else if edge.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
        &[
            "ew-resize",
            "col-resize",
            "size_hor",
            "h_double_arrow",
            "left_side",
            "right_side",
        ]
    } else if edge.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
        &[
            "ns-resize",
            "row-resize",
            "size_ver",
            "v_double_arrow",
            "top_side",
            "bottom_side",
        ]
    } else {
        &["default"]
    }
}

/// Map a hovered resize edge to the primary standard XCursor name.
#[allow(dead_code)]
pub fn resize_cursor_name(edge: crate::grabs::ResizeEdge) -> &'static str {
    resize_cursor_names(edge)[0]
}

fn pick_frame(images: &[Image], size: u32, millis: u32) -> Image {
    let nearest = images
        .iter()
        .min_by_key(|img| (size as i32 - img.size as i32).abs())
        .expect("cursor always has at least one frame");
    let frames: Vec<&Image> = images
        .iter()
        .filter(|img| img.width == nearest.width && img.height == nearest.height)
        .collect();
    let total: u32 = frames.iter().map(|f| f.delay).sum();
    if total == 0 {
        return (*frames[0]).clone();
    }
    let mut t = millis % total;
    for f in &frames {
        if t < f.delay {
            return (*f).clone();
        }
        t -= f.delay;
    }
    (*frames[0]).clone()
}

fn load_named(theme: &CursorTheme, name: &str) -> Option<Vec<Image>> {
    let path = theme.load_icon(name)?;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .read_to_end(&mut data)
        .ok()?;
    let images = parse_xcursor(&data)?;
    if images.is_empty() {
        None
    } else {
        Some(images)
    }
}

/// Draw a compositor-owned resize cursor when the active theme has no matching icon.
fn synthesize_resize_image(edge: crate::grabs::ResizeEdge, size: u32) -> Image {
    use crate::grabs::ResizeEdge;

    let n = size.clamp(16, 96);
    let mut px = vec![0u8; (n * n * 4) as usize];
    let cx = (n / 2) as i32;
    let cy = (n / 2) as i32;
    let arm = (n as i32 / 2 - 3).max(4);
    let head = 3i32;

    let set = |px: &mut [u8], x: i32, y: i32| {
        if x < 0 || y < 0 || x >= n as i32 || y >= n as i32 {
            return;
        }
        let i = ((y as u32 * n + x as u32) * 4) as usize;
        px[i] = 255;
        px[i + 1] = 255;
        px[i + 2] = 255;
        px[i + 3] = 255;
    };

    let plot = |px: &mut [u8], x: i32, y: i32| {
        set(px, x, y);
        set(px, x - 1, y);
        set(px, x + 1, y);
        set(px, x, y - 1);
        set(px, x, y + 1);
    };

    let arrow_h = |px: &mut [u8], tip_x: i32, y: i32, dir: i32| {
        for dx in 0..=head {
            plot(px, tip_x - dir * dx, y - dx);
            plot(px, tip_x - dir * dx, y + dx);
        }
    };

    let arrow_v = |px: &mut [u8], x: i32, tip_y: i32, dir: i32| {
        for dy in 0..=head {
            plot(px, x - dy, tip_y - dir * dy);
            plot(px, x + dy, tip_y - dir * dy);
        }
    };

    if edge.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT)
        && !edge.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM)
    {
        for x in (cx - arm)..=(cx + arm) {
            plot(&mut px, x, cy);
        }
        arrow_h(&mut px, cx - arm, cy, -1);
        arrow_h(&mut px, cx + arm, cy, 1);
    } else if edge.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM)
        && !edge.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT)
    {
        for y in (cy - arm)..=(cy + arm) {
            plot(&mut px, cx, y);
        }
        arrow_v(&mut px, cx, cy - arm, -1);
        arrow_v(&mut px, cx, cy + arm, 1);
    } else if edge == ResizeEdge::TOP_LEFT || edge == ResizeEdge::BOTTOM_RIGHT {
        for i in -arm..=arm {
            plot(&mut px, cx + i, cy - i);
        }
        arrow_h(&mut px, cx + arm, cy - arm, 1);
        arrow_v(&mut px, cx + arm, cy - arm, -1);
        arrow_h(&mut px, cx - arm, cy + arm, -1);
        arrow_v(&mut px, cx - arm, cy + arm, 1);
    } else if edge == ResizeEdge::TOP_RIGHT || edge == ResizeEdge::BOTTOM_LEFT {
        for i in -arm..=arm {
            plot(&mut px, cx + i, cy + i);
        }
        arrow_h(&mut px, cx - arm, cy - arm, -1);
        arrow_v(&mut px, cx - arm, cy - arm, -1);
        arrow_h(&mut px, cx + arm, cy + arm, 1);
        arrow_v(&mut px, cx + arm, cy + arm, 1);
    }

    Image {
        size: n,
        width: n,
        height: n,
        xhot: n / 2,
        yhot: n / 2,
        delay: 0,
        pixels_rgba: px,
        pixels_argb: Vec::new(),
    }
}

/// A generated 24×24 top-left arrow used when no theme cursor is available, so
/// the pointer is always visible. White fill with a 1px black outline.
fn fallback_images() -> Vec<Image> {
    const N: u32 = 24;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let inside = |x: i32, y: i32| -> bool {
        // Classic arrow polygon (approximate), pointing up-left.
        // Filled triangle from the hotspot with a small tail.
        y >= x && x >= 0 && y < (N as i32) && x < (N as i32) && y - x < 14 && y < 18
    };
    for y in 0..N as i32 {
        for x in 0..N as i32 {
            let i = ((y as u32 * N + x as u32) * 4) as usize;
            if inside(x, y) {
                // Outline if a neighbour is outside the shape.
                let edge = !inside(x - 1, y)
                    || !inside(x + 1, y)
                    || !inside(x, y - 1)
                    || !inside(x, y + 1);
                let (r, g, b) = if edge { (0, 0, 0) } else { (255, 255, 255) };
                px[i] = r;
                px[i + 1] = g;
                px[i + 2] = b;
                px[i + 3] = 255;
            }
        }
    }
    vec![Image {
        size: N,
        width: N,
        height: N,
        xhot: 0,
        yhot: 0,
        delay: 0,
        pixels_rgba: px,
        pixels_argb: Vec::new(),
    }]
}
