//! Workspace mini-desktop thumbnails for Task View shelf tiles.
//!
//! Renders wallpaper + non-minimized windows for one (output, workspace) into
//! `$XDG_RUNTIME_DIR/metis/thumbs/ws-{output}-{id}.png`, including windows that
//! are currently unmapped because their workspace is inactive.

use std::path::PathBuf;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen, Texture};
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::render::{CLEAR_COLOR, OutputStack};
use crate::state::MetisState;
use crate::window_thumb::thumb_dir;

const MAX_EDGE_PX: i32 = 320;
const MAX_QUEUE: usize = 16;

pub fn workspace_thumb_path(output: &str, workspace: u32) -> PathBuf {
    thumb_dir().join(format!("ws-{}-{workspace}.png", sanitize_output(output)))
}

fn sanitize_output(output: &str) -> String {
    output
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl MetisState {
    pub(crate) fn queue_workspace_thumb(&mut self, output: String, workspace: u32) {
        if output.is_empty() || workspace == 0 {
            return;
        }
        let q = &mut self.pending_workspace_thumbs;
        if q.iter().any(|(o, w)| o == &output && *w == workspace) {
            return;
        }
        q.push_back((output, workspace));
        while q.len() > MAX_QUEUE {
            q.pop_front();
        }
        self.schedule_redraw();
    }

    pub(crate) fn has_pending_workspace_thumbs(&self) -> bool {
        !self.pending_workspace_thumbs.is_empty()
    }

    pub(crate) fn existing_workspace_thumbs(
        &self,
        output: &str,
        workspaces: &[u32],
    ) -> Vec<metis_protocol::WorkspaceThumb> {
        workspaces
            .iter()
            .filter_map(|&workspace| {
                let path = workspace_thumb_path(output, workspace);
                path.is_file().then(|| metis_protocol::WorkspaceThumb {
                    workspace,
                    path: path.to_string_lossy().into_owned(),
                })
            })
            .collect()
    }
}

pub(crate) fn process_pending_workspace_thumbs(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
) {
    if state.session_is_locked() {
        state.pending_workspace_thumbs.clear();
        return;
    }
    let jobs: Vec<(String, u32)> = state.pending_workspace_thumbs.drain(..).collect();
    if jobs.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(thumb_dir());
    let mut retry: Vec<(String, u32)> = Vec::new();
    for (output, workspace) in jobs {
        match render_workspace_thumb(state, renderer, &output, workspace) {
            Ok(()) => {}
            Err(err) => {
                tracing::debug!(%output, workspace, %err, "workspace thumb capture failed");
                // Wallpaper still decoding/uploading — try again next frame
                // instead of leaving a charcoal CLEAR_COLOR PNG on disk.
                if err.contains("wallpaper not ready")
                    && (state.wallpaper.upload_pending()
                        || state.wallpaper.cpu_pixels_ref().is_some())
                {
                    retry.push((output, workspace));
                }
            }
        }
    }
    for (output, workspace) in retry {
        state.queue_workspace_thumb(output, workspace);
    }
}

fn find_output<'a>(state: &'a MetisState, name: &str) -> Option<&'a Output> {
    state.space.outputs().find(|o| o.name() == name)
}

fn render_workspace_thumb(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    output_name: &str,
    workspace: u32,
) -> Result<(), String> {
    let output = find_output(state, output_name)
        .cloned()
        .ok_or_else(|| format!("unknown output {output_name}"))?;
    let out_geo = state
        .space
        .output_geometry(&output)
        .ok_or_else(|| "output has no geometry".to_string())?;
    let out_w = out_geo.size.w.max(1);
    let out_h = out_geo.size.h.max(1);

    let scale_down = (MAX_EDGE_PX as f64) / (out_w.max(out_h) as f64);
    let scale_factor = if scale_down < 1.0 { scale_down } else { 1.0 };
    let thumb_w = ((out_w as f64) * scale_factor).round().max(1.0) as i32;
    let thumb_h = ((out_h as f64) * scale_factor).round().max(1.0) as i32;
    let output_scale = Scale::from(scale_factor);
    let size_phys: Size<i32, Physical> = Size::from((thumb_w, thumb_h));
    let size_buf: Size<i32, smithay::utils::Buffer> = Size::from((thumb_w, thumb_h));

    // Match the live desktop path: poll → ensure upload → place at the
    // negative of this output's origin so the full-desktop texture crops.
    state.wallpaper.poll_decode();
    if state.wallpaper.enabled() {
        if state.wallpaper.buffer_ref().is_none() {
            state.wallpaper.ensure(renderer);
        }
        if state.wallpaper.buffer_ref().is_none() {
            // Prefer a CPU crop over a missing PNG / charcoal clear — empty
            // desktops must still show wallpaper in Task View.
            if let Some(img) = cpu_wallpaper_thumb(
                state,
                out_geo.loc.x,
                out_geo.loc.y,
                out_w,
                out_h,
                thumb_w,
                thumb_h,
            ) {
                let path = workspace_thumb_path(output_name, workspace);
                img.save(&path)
                    .map_err(|err| format!("png write {path:?}: {err}"))?;
                return Ok(());
            }
            state.wallpaper.request_decode_if_needed();
            return Err("wallpaper not ready".into());
        }
    }

    // Front-to-back: topmost window first (drawn on top), wallpaper last.
    // Stack order must match the live desktop (`space.elements`), not the
    // unordered window-id map — otherwise focusing an already-open app never
    // changes which window appears on top in the shelf thumb.
    let mut elems: Vec<OutputStack> = Vec::new();
    let ids = state.workspace_thumb_stack_order(output_name, workspace);
    for id in ids.into_iter().rev() {
        if state.windows.is_minimized(id) {
            continue;
        }
        let Some(record) = state.windows.get(id).cloned() else {
            continue;
        };
        let Some(body) = state
            .current_window_body_rect(id)
            .or_else(|| state.windows.target_rect(id))
        else {
            continue;
        };
        let local = Point::<i32, Logical>::from((body.x - out_geo.loc.x, body.y - out_geo.loc.y));
        let geo_off = record.window.geometry().loc;
        let loc = (local - geo_off).to_physical_precise_round(output_scale);
        let win_elems = AsRenderElements::<GlesRenderer>::render_elements::<
            smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
        >(&record.window, renderer, loc, output_scale, 1.0);
        elems.extend(win_elems.into_iter().map(OutputStack::Surface));
    }

    // Same placement as `render.rs` live output — empty workspaces still get
    // wallpaper; never leave CLEAR_COLOR as the only layer when wallpaper is on.
    if state.wallpaper.enabled() {
        let wallpaper_origin: Point<f64, Physical> =
            Point::from((-out_geo.loc.x as f64, -out_geo.loc.y as f64));
        let wp = state.wallpaper.render_elements_at(wallpaper_origin);
        if wp.is_empty() {
            return Err("wallpaper not ready".into());
        }
        elems.extend(wp.into_iter().map(OutputStack::Wallpaper));
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

    let src_w = map_size.w.max(1) as usize;
    let src_h = map_size.h.max(1) as usize;
    let dst_w = thumb_w as usize;
    let dst_h = thumb_h as usize;
    let mut rgba = Vec::with_capacity(dst_w * dst_h * 4);
    for y in 0..dst_h.min(src_h) {
        let row = y * src_w * 4;
        let end = row + dst_w.min(src_w) * 4;
        if end > pixels.len() {
            break;
        }
        rgba.extend_from_slice(&pixels[row..end]);
        while rgba.len() < (y + 1) * dst_w * 4 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    while rgba.len() < dst_w * dst_h * 4 {
        rgba.push(0);
    }

    let path = workspace_thumb_path(output_name, workspace);
    let img = image::RgbaImage::from_raw(thumb_w as u32, thumb_h as u32, rgba)
        .ok_or_else(|| "rgba buffer size mismatch".to_string())?;
    img.save(&path)
        .map_err(|err| format!("png write {path:?}: {err}"))?;
    Ok(())
}

/// Crop this output's region from the CPU wallpaper and downscale to the thumb.
/// Used when the GL buffer is missing (e.g. after multi-GPU cache invalidate)
/// so empty workspaces still get a real desktop preview.
fn cpu_wallpaper_thumb(
    state: &MetisState,
    origin_x: i32,
    origin_y: i32,
    out_w: i32,
    out_h: i32,
    thumb_w: i32,
    thumb_h: i32,
) -> Option<image::RgbaImage> {
    let pixels = state.wallpaper.cpu_pixels_ref()?;
    let full = state.wallpaper.full_size();
    let fw = full.w.max(1) as u32;
    let fh = full.h.max(1) as u32;
    let expected = (fw as usize).saturating_mul(fh as usize).saturating_mul(4);
    if pixels.len() != expected {
        return None;
    }
    let full_img = image::RgbaImage::from_raw(fw, fh, pixels.to_vec())?;
    let x = origin_x.clamp(0, full.w.saturating_sub(1)) as u32;
    let y = origin_y.clamp(0, full.h.saturating_sub(1)) as u32;
    let cw = (out_w as u32).min(fw.saturating_sub(x)).max(1);
    let ch = (out_h as u32).min(fh.saturating_sub(y)).max(1);
    let cropped = image::imageops::crop_imm(&full_img, x, y, cw, ch).to_image();
    Some(image::imageops::resize(
        &cropped,
        thumb_w.max(1) as u32,
        thumb_h.max(1) as u32,
        image::imageops::FilterType::Triangle,
    ))
}
