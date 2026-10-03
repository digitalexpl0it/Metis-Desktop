//! Wave A/B/C: Anvil-style [`MultiRenderer`] present for hybrid outputs.
//!
//! Hybrid CRTCs composite through `GpuManager::renderer(primary, target, format)`
//! with textured SSD, wallpaper cache, blur, and HDR decode. When Stage-2 LUT or
//! HDR encode is needed, the stack is composited on a same-node MultiRenderer
//! offscreen, colour-posted on the primary GLES context, then presented as one
//! element to the target. [`crate::cross_gpu`] remains a hard-fail fallback only.

use smithay::{
    backend::{
        allocator::Fourcc,
        drm::{DrmDeviceFd, DrmNode},
        renderer::{
            Bind, Color32F, ImportMem, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Kind,
                memory::MemoryRenderBufferRenderElement,
                solid::SolidColorRenderElement,
                surface::WaylandSurfaceRenderElement,
                texture::{TextureBuffer, TextureRenderElement},
                utils::CropRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
            multigpu::{MultiRenderer, MultiTexture, gbm::GbmGlesBackend},
        },
    },
    desktop::layer_map_for_output,
    input::pointer::CursorImageStatus,
    output::Output,
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::shell::wlr_layer::Layer,
};

use crate::cross_gpu::{self, TransferFrameResult};
use crate::hybrid_shader::HybridTexShaderElement;
use crate::night_light::RenderTargetInfo;
use crate::output_colour::{HybridColourPass, apply_hybrid_colour_post_pass};
use crate::render::{CLEAR_COLOR, bar_layer_rect};
use crate::state::MetisState;
use crate::udev::{MetisGpuManager, UdevOutputId};

/// Cached hybrid MultiRenderer wallpaper upload (invalidated with GPU/context resets).
pub struct HybridWallpaperCache {
    generation: u64,
    size: Size<i32, Physical>,
    /// Cloned into blur / wallpaper elements; kept beside `buffer` for shader draws.
    pub texture: MultiTexture,
    buffer: TextureBuffer<MultiTexture>,
}

/// MultiRenderer over the seat's GBM GLES backends (render GPU → scanout GPU).
pub type UdevMultiRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

smithay::backend::renderer::element::render_elements! {
    pub HybridOutputStack<='a, UdevMultiRenderer<'a>>;
    Wallpaper=TextureRenderElement<MultiTexture>,
    Surface=WaylandSurfaceRenderElement<UdevMultiRenderer<'a>>,
    Deco=crate::decoration::DecorationElement<UdevMultiRenderer<'a>>,
    Overlay=SolidColorRenderElement,
    CropSurface=CropRenderElement<WaylandSurfaceRenderElement<UdevMultiRenderer<'a>>>,
    CropDeco=CropRenderElement<crate::decoration::DecorationElement<UdevMultiRenderer<'a>>>,
    CursorMemory=MemoryRenderBufferRenderElement<UdevMultiRenderer<'a>>,
    Shader=HybridTexShaderElement,
}

const SNAP_OVERLAY_COLOR: [f32; 4] = [0.36, 0.56, 0.96, 0.30];

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

    let copy_format = Fourcc::Abgr8888;
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

    let (hdr_active, hdr_transfer) = state
        .udev
        .as_ref()
        .and_then(|u| u.surface(id))
        .map(|s| (s.hdr_active, s.hdr_transfer))
        .unwrap_or((false, crate::hdr_encode::HdrTransfer::Pq));
    let output_name = output.name();
    let mode = state.hdr_content_mode_for_output(Some(output_name.as_str()));
    let passthrough = mode.allows_passthrough(hdr_active);
    let wants_colour =
        state.color_lut.lut_owns_output(output_name.as_str()) || (hdr_active && !passthrough);

    crate::output_vrr::prepare_vrr_for_render(state, id);
    crate::output_hdr::maybe_log_scanout_format(state, id);

    if wants_colour {
        present_with_colour_post(
            state,
            gpus,
            primary_gpu,
            target_node,
            id,
            output,
            copy_format,
            origin,
            scale,
            size,
            hdr_active,
            hdr_transfer,
            passthrough,
        )
    } else {
        present_direct(
            state,
            gpus,
            primary_gpu,
            target_node,
            id,
            output,
            copy_format,
            origin,
            scale,
            size,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn present_direct(
    state: &mut MetisState,
    gpus: &mut MetisGpuManager,
    primary_gpu: DrmNode,
    target_node: DrmNode,
    id: UdevOutputId,
    output: &Output,
    copy_format: Fourcc,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    size: Size<i32, Physical>,
) -> Result<Option<TransferFrameResult>, String> {
    let mut renderer: UdevMultiRenderer<'_> = gpus
        .renderer(&primary_gpu, &target_node, copy_format)
        .map_err(|e| {
            cross_gpu::record_multi_fail();
            format!("hybrid MultiRenderer: {e:?}")
        })?;

    sync_colour_profiles(state, &mut renderer);

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

    present_elements(
        state,
        id,
        output,
        primary_gpu,
        target_node,
        &mut renderer,
        &elements,
        CLEAR_COLOR,
    )
}

#[allow(clippy::too_many_arguments)]
fn present_with_colour_post(
    state: &mut MetisState,
    gpus: &mut MetisGpuManager,
    primary_gpu: DrmNode,
    target_node: DrmNode,
    id: UdevOutputId,
    output: &Output,
    copy_format: Fourcc,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    size: Size<i32, Physical>,
    hdr_active: bool,
    hdr_transfer: crate::hdr_encode::HdrTransfer,
    passthrough: bool,
) -> Result<Option<TransferFrameResult>, String> {
    let output_name = output.name();

    // Same-node Multi: Offscreen/Bind stay on primary; other GPUs still import.
    let (pass, clear) = {
        let mut multi_same: UdevMultiRenderer<'_> = gpus
            .renderer(&primary_gpu, &primary_gpu, copy_format)
            .map_err(|e| {
                cross_gpu::record_multi_fail();
                format!("hybrid MultiRenderer same-node: {e:?}")
            })?;

        sync_colour_profiles(state, &mut multi_same);

        let elements = build_hybrid_elements(
            state,
            &mut multi_same,
            origin,
            scale,
            RenderTargetInfo {
                size,
                output_name: Some(output_name.as_str()),
                skip_night_light: false,
            },
            output,
        )?;

        let scene = composite_hybrid_offscreen(&mut multi_same, &elements, size, scale)
            .ok_or_else(|| {
                cross_gpu::record_multi_fail();
                "hybrid colour: offscreen composite failed".to_string()
            })?;

        let multi_ctx = Renderer::context_id(&multi_same);
        let gles: &mut GlesRenderer = multi_same.as_mut();
        apply_hybrid_colour_post_pass(
            &mut state.color_lut,
            &mut state.hdr_encode,
            gles,
            multi_ctx,
            scene,
            output_name.as_str(),
            size,
            hdr_active,
            hdr_transfer,
            passthrough,
        )
        .ok_or_else(|| {
            cross_gpu::record_multi_fail();
            "hybrid colour: post-pass failed".to_string()
        })?
    };

    let mut renderer: UdevMultiRenderer<'_> = gpus
        .renderer(&primary_gpu, &target_node, copy_format)
        .map_err(|e| {
            cross_gpu::record_multi_fail();
            format!("hybrid MultiRenderer present: {e:?}")
        })?;

    let elements: Vec<HybridOutputStack<'_>> = match pass {
        HybridColourPass::Texture(tex) => vec![HybridOutputStack::Wallpaper(tex)],
        HybridColourPass::Shader(shader) => vec![HybridOutputStack::Shader(shader)],
    };

    present_elements(
        state,
        id,
        output,
        primary_gpu,
        target_node,
        &mut renderer,
        &elements,
        clear,
    )
}

#[allow(clippy::too_many_arguments)]
fn present_elements<'a>(
    state: &mut MetisState,
    id: UdevOutputId,
    output: &Output,
    primary_gpu: DrmNode,
    target_node: DrmNode,
    renderer: &mut UdevMultiRenderer<'a>,
    elements: &[HybridOutputStack<'a>],
    clear: [f32; 4],
) -> Result<Option<TransferFrameResult>, String> {
    let Some(surface) = state.udev.as_mut().and_then(|udev| udev.surface_mut(id)) else {
        cross_gpu::record_multi_fail();
        return Err("hybrid MultiRenderer: surface gone".into());
    };

    match surface.drm_output.render_frame(
        renderer,
        elements,
        clear,
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

fn sync_colour_profiles(state: &mut MetisState, renderer: &mut UdevMultiRenderer<'_>) {
    if state.color_mgmt.profiles_dirty {
        let profiles = state.color_mgmt.profile_map().clone();
        let gles: &mut GlesRenderer = renderer.as_mut();
        state.color_lut.sync_profiles(gles, &profiles);
        state.color_mgmt.profiles_dirty = false;
        crate::output_gamma::apply_output_gamma(state);
    }
}

fn composite_hybrid_offscreen<'a>(
    renderer: &mut UdevMultiRenderer<'a>,
    elements: &[HybridOutputStack<'a>],
    size: Size<i32, Physical>,
    scale: Scale<f64>,
) -> Option<GlesTexture> {
    let size_buf: Size<i32, Buffer> = Size::from((size.w, size.h));
    for format in [Fourcc::Abgr16161616f, Fourcc::Abgr2101010, Fourcc::Abgr8888] {
        let mut offscreen =
            match Offscreen::<GlesTexture>::create_buffer(renderer, format, size_buf) {
                Ok(buf) => buf,
                Err(_) => continue,
            };
        let rendered = {
            let mut framebuffer = match renderer.bind(&mut offscreen) {
                Ok(fb) => fb,
                Err(_) => continue,
            };
            let mut damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
            damage_tracker
                .render_output(renderer, &mut framebuffer, 0, elements, CLEAR_COLOR)
                .is_ok()
        };
        if rendered {
            return Some(offscreen);
        }
    }
    tracing::warn!("hybrid colour: offscreen composite failed for all formats");
    None
}

fn build_hybrid_elements<'a>(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'a>,
    render_origin: Point<i32, Physical>,
    output_scale: Scale<f64>,
    target: RenderTargetInfo<'_>,
    output: &Output,
) -> Result<Vec<HybridOutputStack<'a>>, String> {
    if state.lock.locked || state.protocol_lock.is_locked() {
        return Err("hybrid MultiRenderer: session locked — use transfer/local".into());
    }

    // Ensure blur / HDR decode programs on the primary GLES context.
    {
        let gles: &mut GlesRenderer = renderer.as_mut();
        state.blur.ensure_program(gles);
    }

    let mut render_elements: Vec<HybridOutputStack<'a>> = Vec::new();

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

    // Textured SSD
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

    // Windows top-to-bottom (+ HDR decode when needed)
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

        let decoded = if clip.is_none()
            && state.should_decode_hdr_surfaces(target.output_name)
            && let Some(tf) = state.window_hdr_transfer(window)
        {
            let gles: &mut GlesRenderer = renderer.as_mut();
            state
                .hdr_encode
                .try_decode_window_hybrid(gles, window, loc, win_scale, alpha, tf)
        } else {
            None
        };

        if let Some(decoded) = decoded {
            render_elements.push(HybridOutputStack::Shader(decoded));
        } else {
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

    // Blur (above wallpaper) + wallpaper
    let skip_underlay = state.output_has_fullscreen(target.output_name);
    if !skip_underlay {
        state.wallpaper.poll_decode();
        state.wallpaper.request_decode_if_needed();
        ensure_hybrid_wallpaper(state, renderer)?;

        let draw_blur = state.blur.enabled
            && !state.splash_overlay_visible()
            && state.hybrid_wallpaper_cache.is_some()
            && !state
                .bar_auto_hidden
                .get(output.name().as_str())
                .copied()
                .unwrap_or(false);

        if draw_blur {
            let bar_rects: Vec<Rectangle<i32, Physical>> = state
                .space
                .outputs()
                .filter_map(|out| {
                    if state
                        .bar_auto_hidden
                        .get(&out.name())
                        .copied()
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    let out_origin = state.space.output_geometry(out)?.loc.to_physical(1);
                    let local = bar_layer_rect(out)?;
                    Some(Rectangle::new(
                        local.loc + out_origin - render_origin,
                        local.size,
                    ))
                })
                .collect();

            if let Some(cache) = state.hybrid_wallpaper_cache.as_ref() {
                let tex_size = cache.texture.size();
                let texture = cache.texture.clone();
                let radius = state.blur.radius;
                let enabled = state.blur.enabled;
                // Re-borrow mutably for hybrid_element after cloning texture.
                let _ = (enabled, radius);
                for r in bar_rects {
                    let rect = state.blur.confine_to_pill(r);
                    if let Some(elem) = state.blur.hybrid_element(rect, texture.clone(), tex_size) {
                        render_elements.push(HybridOutputStack::Shader(elem));
                    }
                }
            }
        }

        if let Some(wp) = wallpaper_element_from_cache(state, render_origin) {
            render_elements.push(HybridOutputStack::Wallpaper(wp));
        }
    }

    // Cursor
    render_elements.splice(
        0..0,
        hybrid_cursor_elements(state, renderer, output, output_scale)?,
    );

    // Night light / battery dim
    if let Some(elem) = crate::night_light::night_light_element(state, &target) {
        render_elements.insert(0, HybridOutputStack::Overlay(elem));
    }
    if let Some(elem) = crate::battery_dim::battery_dim_element(state, &target) {
        render_elements.insert(0, HybridOutputStack::Overlay(elem));
    }

    Ok(render_elements)
}

fn ensure_hybrid_wallpaper(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'_>,
) -> Result<(), String> {
    let size = state.wallpaper.full_size();
    if size.w <= 0 || size.h <= 0 || state.wallpaper.cpu_pixels_ref().is_none() {
        return Ok(());
    }
    let expected = (size.w as usize)
        .saturating_mul(size.h as usize)
        .saturating_mul(4);
    let pixels_gen = state.wallpaper.cpu_pixels_gen();
    let reuse = state
        .hybrid_wallpaper_cache
        .as_ref()
        .is_some_and(|c| c.generation == pixels_gen && c.size == size);
    if reuse {
        return Ok(());
    }
    let (texture, buffer) = {
        let Some(rgba) = state.wallpaper.cpu_pixels_ref() else {
            return Ok(());
        };
        if rgba.len() != expected {
            return Ok(());
        }
        let texture = renderer
            .import_memory(rgba, Fourcc::Abgr8888, (size.w, size.h).into(), false)
            .map_err(|e| format!("hybrid wallpaper import: {e:?}"))?;
        let buffer =
            TextureBuffer::from_texture(renderer, texture.clone(), 1, Transform::Normal, None);
        (texture, buffer)
    };
    state.hybrid_wallpaper_cache = Some(HybridWallpaperCache {
        generation: pixels_gen,
        size,
        texture,
        buffer,
    });
    Ok(())
}

fn wallpaper_element_from_cache(
    state: &MetisState,
    render_origin: Point<i32, Physical>,
) -> Option<TextureRenderElement<MultiTexture>> {
    let cache = state.hybrid_wallpaper_cache.as_ref()?;
    let loc: Point<f64, Physical> = Point::from((-render_origin.x as f64, -render_origin.y as f64));
    Some(TextureRenderElement::from_texture_buffer(
        loc,
        &cache.buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    ))
}

fn hybrid_cursor_elements<'a>(
    state: &mut MetisState,
    renderer: &mut UdevMultiRenderer<'a>,
    output: &Output,
    scale: Scale<f64>,
) -> Result<Vec<HybridOutputStack<'a>>, String> {
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
