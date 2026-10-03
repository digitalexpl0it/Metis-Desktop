//! Wave A/B: Anvil-style [`MultiRenderer`] present for hybrid outputs.
//!
//! When the frame does not need GLES-only blur / HDR / colour post-pass, hybrid
//! CRTCs composite through `GpuManager::renderer(primary, target, format)` with a
//! generic element stack (surfaces, textured SSD, wallpaper, overlays, cursor).
//! Wave B uploads decoration CPU caches via [`ImportMem`] into MultiRenderer.
//! Blur + HDR/`TextureShaderElement` stay on [`crate::cross_gpu`] transfer.

use smithay::{
    backend::{
        allocator::Fourcc,
        drm::{DrmDeviceFd, DrmNode},
        renderer::{
            Color32F, ImportAll, ImportMem,
            element::{
                AsRenderElements, Kind,
                memory::MemoryRenderBufferRenderElement,
                solid::SolidColorRenderElement,
                surface::WaylandSurfaceRenderElement,
                texture::{TextureBuffer, TextureRenderElement},
                utils::CropRenderElement,
            },
            gles::GlesRenderer,
            multigpu::{MultiRenderer, MultiTexture, gbm::GbmGlesBackend},
        },
    },
    desktop::layer_map_for_output,
    input::pointer::CursorImageStatus,
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::shell::wlr_layer::Layer,
};

/// Cached hybrid MultiRenderer wallpaper upload (invalidated with GPU/context resets).
pub struct HybridWallpaperCache {
    generation: u64,
    size: Size<i32, Physical>,
    buffer: TextureBuffer<MultiTexture>,
}

use crate::cross_gpu::{self, TransferFrameResult};
use crate::night_light::RenderTargetInfo;
use crate::render::CLEAR_COLOR;
use crate::state::MetisState;
use crate::udev::{MetisGpuManager, UdevOutputId};

// Anvil-style hybrid stack (no Blur / HdrEncode — those stay on the GLES transfer path).
smithay::backend::renderer::element::render_elements! {
    pub HybridOutputStack<R> where R: ImportAll + ImportMem;
    Wallpaper=TextureRenderElement<R::TextureId>,
    Surface=WaylandSurfaceRenderElement<R>,
    Deco=crate::decoration::DecorationElement<R>,
    Overlay=SolidColorRenderElement,
    CropSurface=CropRenderElement<WaylandSurfaceRenderElement<R>>,
    CropDeco=CropRenderElement<crate::decoration::DecorationElement<R>>,
    CursorMemory=MemoryRenderBufferRenderElement<R>,
}

/// MultiRenderer over the seat's GBM GLES backends (render GPU → scanout GPU).
pub type UdevMultiRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

const SNAP_OVERLAY_COLOR: [f32; 4] = [0.36, 0.56, 0.96, 0.30];

/// True when Wave A should skip MultiRenderer and use full-frame transfer (blur/HDR/LUT).
pub fn hybrid_needs_gles_transfer(state: &MetisState, output: &Output, id: UdevOutputId) -> bool {
    let output_name = output.name();
    let skip_underlay = state.output_has_fullscreen(Some(output_name.as_str()));

    let blur_wanted = state.blur.enabled
        && !skip_underlay
        && !state.splash_overlay_visible()
        && state.wallpaper.texture().is_some()
        && !state
            .bar_auto_hidden
            .get(output_name.as_str())
            .copied()
            .unwrap_or(false);
    if blur_wanted {
        return true;
    }

    let (hdr_active, _) = state
        .udev
        .as_ref()
        .and_then(|u| u.surface(id))
        .map(|s| (s.hdr_active, s.hdr_transfer))
        .unwrap_or((false, crate::hdr_encode::HdrTransfer::Pq));
    let mode = state.hdr_content_mode_for_output(Some(output_name.as_str()));
    let passthrough = mode.allows_passthrough(hdr_active);
    let wants_hdr_encode = hdr_active && !passthrough;
    let wants_lut = state.color_lut.lut_owns_output(output_name.as_str());
    wants_hdr_encode || wants_lut
}

/// Composite on `primary_gpu` and present on `target_node` via MultiRenderer.
pub fn try_multirenderer_frame(
    state: &mut MetisState,
    gpus: &mut MetisGpuManager,
    primary_gpu: DrmNode,
    target_node: DrmNode,
    id: UdevOutputId,
    output: &Output,
) -> Result<Option<TransferFrameResult>, String> {
    if primary_gpu == target_node {
        return Ok(None);
    }
    if hybrid_needs_gles_transfer(state, output, id) {
        return Err("hybrid MultiRenderer skipped — blur/HDR/LUT needs GLES transfer".into());
    }

    let copy_format = Fourcc::Abgr8888;
    let mut renderer: UdevMultiRenderer<'_> = gpus
        .renderer(&primary_gpu, &target_node, copy_format)
        .map_err(|e| {
            cross_gpu::record_multi_fail();
            format!("hybrid MultiRenderer: {e:?}")
        })?;

    let scale = Scale::from(output.current_scale().fractional_scale());
    let size: Size<i32, Physical> = output
        .current_mode()
        .map(|m| m.size)
        .ok_or_else(|| "hybrid MultiRenderer: output has no mode".to_string())?;
    let origin: Point<i32, Physical> = state
        .space
        .output_geometry(output)
        .map(|g| g.loc.to_physical_precise_round(scale))
        .unwrap_or_default();

    let elements = build_hybrid_elements(
        state,
        &mut renderer,
        origin,
        scale,
        RenderTargetInfo {
            size,
            output_name: Some(output.name().as_str()),
            skip_night_light: false,
        },
        output,
    )?;

    crate::output_vrr::prepare_vrr_for_render(state, id);
    crate::output_hdr::maybe_log_scanout_format(state, id);

    let Some(surface) = state.udev.as_mut().and_then(|udev| udev.surface_mut(id)) else {
        cross_gpu::record_multi_fail();
        return Err("hybrid MultiRenderer: surface gone".into());
    };

    match surface.drm_output.render_frame(
        &mut renderer,
        &elements,
        CLEAR_COLOR,
        smithay::backend::drm::compositor::FrameFlags::DEFAULT,
    ) {
        Ok(res) => {
            cross_gpu::record_multi_ok();
            tracing::debug!(
                output = %output.name(),
                ?primary_gpu,
                ?target_node,
                stats = ?cross_gpu::CrossGpuStats::snapshot(),
                "hybrid MultiRenderer frame presented"
            );
            Ok(Some(TransferFrameResult {
                empty: res.is_empty,
                states: res.states,
            }))
        }
        Err(err) => {
            cross_gpu::record_multi_fail();
            Err(format!("hybrid MultiRenderer render_frame: {err:?}"))
        }
    }
}

fn build_hybrid_elements<'a>(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'a>,
    render_origin: Point<i32, Physical>,
    output_scale: Scale<f64>,
    target: RenderTargetInfo<'_>,
    output: &Output,
) -> Result<Vec<HybridOutputStack<UdevMultiRenderer<'a>>>, String> {
    if state.lock.locked || state.protocol_lock.is_locked() {
        return Err("hybrid MultiRenderer: session locked — use transfer/local".into());
    }

    let mut render_elements: Vec<HybridOutputStack<UdevMultiRenderer<'a>>> = Vec::new();

    // Snap preview
    if let Some((rect, _)) = state.snap_preview {
        if state.last_snap_rect != Some(rect) {
            state.last_snap_rect = Some(rect);
            state.snap_overlay_commit.increment();
        }
        let geo = Rectangle::<i32, Logical>::new(
            Point::from((rect.x, rect.y)),
            Size::from((rect.width.max(1), rect.height.max(1))),
        )
        .to_physical(1);
        let geo = Rectangle::new(geo.loc - render_origin, geo.size);
        render_elements.push(HybridOutputStack::Overlay(SolidColorRenderElement::new(
            state.snap_overlay_id.clone(),
            geo,
            state.snap_overlay_commit,
            Color32F::from(SNAP_OVERLAY_COLOR),
            Kind::Unspecified,
        )));
    }

    // Layer-shell (bar, etc.)
    let layer_outputs: Vec<Output> = state.space.outputs().cloned().collect();
    let mut upper_layers = Vec::new();
    let mut lower_layers = Vec::new();
    for out in &layer_outputs {
        let out_origin = state
            .space
            .output_geometry(out)
            .map(|g| g.loc)
            .unwrap_or_default();
        let map = layer_map_for_output(out);
        for surface in map.layers().rev() {
            let Some(geo) = map.layer_geometry(surface) else {
                continue;
            };
            let loc =
                (geo.loc + out_origin).to_physical_precise_round(output_scale) - render_origin;
            let elems = AsRenderElements::<UdevMultiRenderer<'a>>::render_elements::<
                WaylandSurfaceRenderElement<UdevMultiRenderer<'a>>,
            >(surface, renderer, loc, output_scale, 1.0);
            let target_vec = if matches!(surface.layer(), Layer::Background | Layer::Bottom) {
                &mut lower_layers
            } else {
                &mut upper_layers
            };
            target_vec.extend(elems.into_iter().map(HybridOutputStack::Surface));
        }
    }
    render_elements.extend(upper_layers);

    // Textured SSD (Wave B): same stacking as the GLES path — overlay chrome
    // below the bar, then each window immediately followed by its below-chrome.
    let deco_specs = state.decoration_specs();
    state.decorations.begin_frame(&deco_specs);
    let deco_by_id: std::collections::HashMap<u32, crate::decoration::WindowDeco> =
        deco_specs.into_iter().map(|w| (w.id, w)).collect();

    for spec in deco_by_id.values().filter(|w| w.overlay) {
        if let Some(record) = state.windows.get(spec.id) {
            let win_scale = state.window_output_scale(&record.window, output_scale);
            let decos = state.decorations.window_elements(renderer, spec, win_scale);
            render_elements.extend(decos.into_iter().map(HybridOutputStack::Deco));
        }
    }

    {
        let stack_ids: std::collections::HashSet<u32> = state
            .space
            .elements()
            .filter_map(|w| state.windows.id_for_window(w))
            .collect();
        for (id, spec) in &deco_by_id {
            if spec.overlay || stack_ids.contains(id) {
                continue;
            }
            tracing::warn!(
                id,
                "hybrid deco: window chrome not matched to a stacked window — drawing on top"
            );
            if let Some(record) = state.windows.get(*id) {
                let win_scale = state.window_output_scale(&record.window, output_scale);
                let decos = state.decorations.window_elements(renderer, spec, win_scale);
                render_elements.extend(decos.into_iter().map(HybridOutputStack::Deco));
            }
        }
    }

    // Windows top-to-bottom, each followed by its own chrome (GLES parity).
    let stacking: Vec<_> = state.space.elements().cloned().collect();
    for window in stacking.iter().rev() {
        let id = state.windows.id_for_window(window);
        let win_scale = state.window_output_scale(window, output_scale);
        let elem_loc = state.space.element_location(window).unwrap_or_default();
        let geo_off = if window.x11_surface().is_some() {
            Point::from((0, 0))
        } else {
            window.geometry().loc
        };
        let mut loc = (elem_loc - geo_off).to_physical_precise_round(win_scale) - render_origin;
        let mut alpha = 1.0f32;
        if let Some(id) = id {
            let nudge = state.scroll_render_nudge(id);
            if nudge != 0 {
                loc = Point::from((loc.x + nudge, loc.y));
            }
            if let Some((_, genie_alpha)) = state.minimize_genie_render(id) {
                alpha = genie_alpha;
            }
        }
        let clip = id.and_then(|id| {
            if state.is_minimize_genie_active(id) {
                state.minimize_genie_render(id).map(|(r, _)| {
                    Rectangle::new(
                        Point::from((r.x, r.y)).to_physical_precise_round(win_scale)
                            - render_origin,
                        Size::from((r.width.max(1), r.height.max(1)))
                            .to_physical_precise_round(win_scale),
                    )
                })
            } else {
                state
                    .scroll_window_clip(id, win_scale)
                    .map(|c| Rectangle::new(c.loc - render_origin, c.size))
            }
        });
        let elems = AsRenderElements::<UdevMultiRenderer<'a>>::render_elements::<
            WaylandSurfaceRenderElement<UdevMultiRenderer<'a>>,
        >(window, renderer, loc, win_scale, alpha);
        if let Some(clip) = clip {
            for e in elems {
                if let Some(c) = CropRenderElement::from_element(e, win_scale, clip) {
                    render_elements.push(HybridOutputStack::CropSurface(c));
                }
            }
        } else {
            render_elements.extend(elems.into_iter().map(HybridOutputStack::Surface));
        }
        if let Some(id) = id
            && let Some(spec) = deco_by_id.get(&id).filter(|s| !s.overlay)
        {
            let decos = state.decorations.window_elements(renderer, spec, win_scale);
            if let Some(clip) = clip {
                for d in decos {
                    if let Some(c) = CropRenderElement::from_element(d, win_scale, clip) {
                        render_elements.push(HybridOutputStack::CropDeco(c));
                    }
                }
            } else {
                render_elements.extend(decos.into_iter().map(HybridOutputStack::Deco));
            }
        }
    }

    render_elements.extend(lower_layers);

    // Wallpaper from CPU pixels (avoid GLES-only texture cache).
    let skip_underlay = state.output_has_fullscreen(target.output_name);
    if !skip_underlay {
        state.wallpaper.poll_decode();
        state.wallpaper.request_decode_if_needed();
        if let Some(wp) = wallpaper_element(state, renderer, render_origin)? {
            render_elements.push(HybridOutputStack::Wallpaper(wp));
        }
    }

    // Cursor
    render_elements.splice(
        0..0,
        hybrid_cursor_elements(state, renderer, output, output_scale)?,
    );

    // Night light / battery dim as solid overlays (same as GLES path helpers).
    let nl = crate::night_light::night_light_element(state, &target);
    if let Some(elem) = nl {
        render_elements.insert(0, HybridOutputStack::Overlay(elem));
    }
    if let Some(elem) = crate::battery_dim::battery_dim_element(state, &target) {
        render_elements.insert(0, HybridOutputStack::Overlay(elem));
    }

    Ok(render_elements)
}

fn wallpaper_element<'a>(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'a>,
    render_origin: Point<i32, Physical>,
) -> Result<Option<TextureRenderElement<MultiTexture>>, String> {
    let size = state.wallpaper.full_size();
    if size.w <= 0 || size.h <= 0 || state.wallpaper.cpu_pixels_ref().is_none() {
        return Ok(None);
    }
    let expected = (size.w as usize)
        .saturating_mul(size.h as usize)
        .saturating_mul(4);
    let pixels_gen = state.wallpaper.cpu_pixels_gen();
    let reuse = state
        .hybrid_wallpaper_cache
        .as_ref()
        .is_some_and(|c| c.generation == pixels_gen && c.size == size);
    if !reuse {
        let buffer = {
            let Some(rgba) = state.wallpaper.cpu_pixels_ref() else {
                return Ok(None);
            };
            if rgba.len() != expected {
                return Ok(None);
            }
            let texture = renderer
                .import_memory(rgba, Fourcc::Abgr8888, (size.w, size.h).into(), false)
                .map_err(|e| format!("hybrid wallpaper import: {e:?}"))?;
            TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None)
        };
        state.hybrid_wallpaper_cache = Some(HybridWallpaperCache {
            generation: pixels_gen,
            size,
            buffer,
        });
    }
    let Some(cache) = state.hybrid_wallpaper_cache.as_ref() else {
        return Ok(None);
    };
    let loc: Point<f64, Physical> = Point::from((-render_origin.x as f64, -render_origin.y as f64));
    Ok(Some(TextureRenderElement::from_texture_buffer(
        loc,
        &cache.buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )))
}

fn hybrid_cursor_elements<'a>(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'a>,
    output: &Output,
    scale: Scale<f64>,
) -> Result<Vec<HybridOutputStack<UdevMultiRenderer<'a>>>, String> {
    let mut out = Vec::new();
    let Some(geo) = state.space.output_geometry(output) else {
        return Ok(out);
    };
    let Some(pointer) = state.seat.get_pointer() else {
        return Ok(out);
    };
    let loc = pointer.current_location();
    if !geo.to_f64().contains(loc) {
        return Ok(out);
    }
    if matches!(state.cursor_status, CursorImageStatus::Hidden)
        || state.active_pointer_lock_suppresses_cursor()
    {
        return Ok(out);
    }
    let over_bar = state.metis_bar_ui_hit(loc);
    let local = loc - geo.loc.to_f64();
    let millis = state.start_time.elapsed().as_millis() as u32;
    let Some(udev) = state.udev.as_mut() else {
        return Ok(out);
    };
    let image = if !over_bar {
        if let Some(edge) = state.hover_cursor {
            udev.cursor.frame_resize(&udev.cursor_theme, edge, millis)
        } else {
            udev.cursor.frame(millis).clone()
        }
    } else {
        udev.cursor.frame(millis).clone()
    };
    let buffer = match udev.pointer_buffers.iter().find(|(i, _)| *i == image) {
        Some((_, buf)) => buf.clone(),
        None => {
            let buf = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                &image.pixels_rgba,
                Fourcc::Argb8888,
                (image.width as i32, image.height as i32),
                1,
                Transform::Normal,
                None,
            );
            udev.pointer_buffers.push((image.clone(), buf.clone()));
            buf
        }
    };
    let hotspot: Point<f64, Physical> = Point::from((image.xhot as f64, image.yhot as f64));
    let pos = local.to_physical(scale) - hotspot;
    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        pos,
        &buffer,
        None,
        None,
        None,
        Kind::Cursor,
    ) {
        out.push(HybridOutputStack::CursorMemory(elem));
    }
    Ok(out)
}
