//! Per-window PNG thumbnails for Task View (compositor-rendered, not screen crops).
//!
//! A single output screenshot cannot show buried/maximized clients correctly —
//! every near-fullscreen crop is just whatever is currently on top. Instead we
//! render each window's own surfaces into an offscreen buffer on the GL thread
//! and write `$XDG_RUNTIME_DIR/metis/thumbs/{id}.png`.

use std::path::PathBuf;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen, Texture};
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};

use crate::render::CLEAR_COLOR;
use crate::state::MetisState;

const MAX_EDGE_PX: i32 = 320;
const MAX_QUEUE: usize = 24;

pub fn thumb_dir() -> PathBuf {
    metis_protocol::runtime_dir().join("thumbs")
}

pub fn thumb_path(id: u32) -> PathBuf {
    thumb_dir().join(format!("{id}.png"))
}

impl MetisState {
    pub(crate) fn queue_window_thumb(&mut self, id: u32) {
        if self.windows.get(id).is_none() {
            return;
        }
        if self.windows.is_minimized(id) {
            return;
        }
        let q = &mut self.pending_window_thumbs;
        if q.iter().any(|&x| x == id) {
            return;
        }
        q.push_back(id);
        while q.len() > MAX_QUEUE {
            q.pop_front();
        }
        self.schedule_redraw();
    }

    pub(crate) fn has_pending_window_thumbs(&self) -> bool {
        !self.pending_window_thumbs.is_empty()
    }

    pub(crate) fn existing_window_thumbs(&self, ids: &[u32]) -> Vec<metis_protocol::WindowThumb> {
        ids.iter()
            .filter_map(|&id| {
                let path = thumb_path(id);
                path.is_file().then(|| metis_protocol::WindowThumb {
                    id,
                    path: path.to_string_lossy().into_owned(),
                })
            })
            .collect()
    }
}

pub(crate) fn process_pending_window_thumbs(state: &mut MetisState, renderer: &mut GlesRenderer) {
    if state.session_is_locked() {
        state.pending_window_thumbs.clear();
        return;
    }
    let ids: Vec<u32> = state.pending_window_thumbs.drain(..).collect();
    if ids.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(thumb_dir());
    for id in ids {
        if let Err(err) = render_window_thumb(state, renderer, id) {
            tracing::debug!(id, %err, "window thumb capture failed");
        }
    }
}

fn render_window_thumb(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    id: u32,
) -> Result<(), String> {
    if state.windows.is_minimized(id) {
        return Err("minimized".into());
    }
    let record = state
        .windows
        .get(id)
        .cloned()
        .ok_or_else(|| "unknown window".to_string())?;
    let window = record.window;

    let geo = window.geometry();
    let mut width = geo.size.w.max(1);
    let mut height = geo.size.h.max(1);
    if (width < 8 || height < 8)
        && let Some(r) = state.windows.target_rect(id)
    {
        width = r.width.max(1);
        height = r.height.max(1);
    }
    if width < 8 || height < 8 {
        return Err("degenerate geometry".into());
    }

    let scale_down = (MAX_EDGE_PX as f64) / (width.max(height) as f64);
    let scale_factor = if scale_down < 1.0 { scale_down } else { 1.0 };
    let out_w = ((width as f64) * scale_factor).round().max(1.0) as i32;
    let out_h = ((height as f64) * scale_factor).round().max(1.0) as i32;
    let output_scale = Scale::from(scale_factor);
    let size_phys: Size<i32, Physical> = Size::from((out_w, out_h));
    let size_buf: Size<i32, smithay::utils::Buffer> = Size::from((out_w, out_h));

    // Place window geometry origin at (0,0) in the offscreen target.
    let loc = Point::<i32, smithay::utils::Logical>::from((-geo.loc.x, -geo.loc.y))
        .to_physical_precise_round(output_scale);
    let elems = AsRenderElements::<GlesRenderer>::render_elements::<
        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
    >(&window, renderer, loc, output_scale, 1.0);

    if elems.is_empty() {
        return Err("no render elements".into());
    }

    let mut offscreen =
        Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size_buf)
            .map_err(|err| format!("offscreen: {err:?}"))?;
    let mut framebuffer = renderer
        .bind(&mut offscreen)
        .map_err(|err| format!("bind: {err:?}"))?;

    let mut damage_tracker = OutputDamageTracker::new(size_phys, output_scale, Transform::Normal);
    damage_tracker
        .render_output(renderer, &mut framebuffer, 0, &elems, CLEAR_COLOR)
        .map_err(|err| format!("render: {err:?}"))?;

    let region = Rectangle::from_size(size_buf);
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .map_err(|err| format!("copy: {err:?}"))?;
    let map_size = mapping.size();
    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|err| format!("map: {err:?}"))?;

    // Smithay GLES Abgr8888 readback is R,G,B,A bytes — same as image::RgbaImage.
    // Mapping width may include stride padding; crop to the requested thumb size.
    let src_w = map_size.w.max(1) as usize;
    let src_h = map_size.h.max(1) as usize;
    let dst_w = out_w as usize;
    let dst_h = out_h as usize;
    let mut rgba = Vec::with_capacity(dst_w * dst_h * 4);
    for y in 0..dst_h.min(src_h) {
        let row = y * src_w * 4;
        let end = row + dst_w.min(src_w) * 4;
        if end > pixels.len() {
            break;
        }
        rgba.extend_from_slice(&pixels[row..end]);
        // Pad short rows (should not happen for tight packs).
        while rgba.len() < (y + 1) * dst_w * 4 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    while rgba.len() < dst_w * dst_h * 4 {
        rgba.push(0);
    }

    let path = thumb_path(id);
    let img = image::RgbaImage::from_raw(out_w as u32, out_h as u32, rgba)
        .ok_or_else(|| "rgba buffer size mismatch".to_string())?;
    img.save(&path)
        .map_err(|err| format!("png write {path:?}: {err}"))?;
    Ok(())
}
