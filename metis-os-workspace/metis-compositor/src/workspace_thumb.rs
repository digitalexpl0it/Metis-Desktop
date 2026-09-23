//! Workspace mini-desktop thumbnails for Task View shelf tiles.
//!
//! Renders wallpaper + non-minimized windows for one (output, workspace) into
//! `$XDG_RUNTIME_DIR/metis/thumbs/ws-{output}-{id}.png`, including windows that
//! are currently unmapped because their workspace is inactive.

use std::path::PathBuf;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Kind};
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
    for (output, workspace) in jobs {
        if let Err(err) = render_workspace_thumb(state, renderer, &output, workspace) {
            tracing::debug!(%output, workspace, %err, "workspace thumb capture failed");
        }
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

    state.wallpaper.poll_decode();
    state.wallpaper.ensure(renderer);

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

    // Wallpaper texture is 1:1 with the virtual desktop (scale 1). Crop this
    // output's region; OutputDamageTracker scale maps it into the thumb.
    if let Some(buffer) = state.wallpaper.buffer_ref() {
        let src = Rectangle::<f64, Logical>::new(
            Point::from((out_geo.loc.x as f64, out_geo.loc.y as f64)),
            Size::from((out_w as f64, out_h as f64)),
        );
        let dst = Size::<i32, Logical>::from((out_w, out_h));
        let wp = TextureRenderElement::from_texture_buffer(
            Point::from((0.0, 0.0)),
            buffer,
            None,
            Some(src),
            Some(dst),
            Kind::Unspecified,
        );
        elems.push(OutputStack::Wallpaper(wp));
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
