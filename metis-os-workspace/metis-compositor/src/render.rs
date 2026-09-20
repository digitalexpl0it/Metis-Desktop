//! Shared render-element assembly used by both the nested winit backend and the
//! standalone DRM backend.
//!
//! [`MetisState::build_render_elements`] produces the full front-to-back element
//! stack for one render pass. Elements are positioned in *render-target-local*
//! coordinates: the caller passes `render_origin` (the global physical origin of
//! the target) and every element is offset by its negative. The winit backend
//! renders the whole virtual desktop into a single framebuffer and passes
//! `(0, 0)` (global coords); the DRM backend renders one framebuffer per output
//! and passes that output's global origin, so a half-scrolled column or an
//! off-output layer surface simply clips against the output bounds.

use std::collections::HashMap;

use smithay::{
    backend::renderer::{
        element::{
            solid::SolidColorRenderElement, surface::WaylandSurfaceRenderElement,
            texture::TextureRenderElement, utils::CropRenderElement, AsRenderElements, Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        Color32F,
    },
    desktop::{layer_map_for_output, Window},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::shell::wlr_layer::Layer,
};

use crate::night_light::{night_light_element, RenderTargetInfo};
use crate::state::MetisState;

smithay::backend::renderer::element::render_elements! {
    pub OutputStack<=GlesRenderer>;
    Wallpaper=TextureRenderElement<GlesTexture>,
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Deco=crate::decoration::DecorationElement,
    Blur=crate::blur::BlurElement,
    Overlay=SolidColorRenderElement,
    // Scroll-managed windows + their chrome, clipped to their own output so a
    // half-scrolled column never bleeds onto the adjacent display.
    CropSurface=CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    CropDeco=CropRenderElement<crate::decoration::DecorationElement>,
    // Software/hardware pointer for the DRM backend (named-theme cursor). The
    // winit backend uses the host cursor and never emits this.
    CursorMemory=smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<GlesRenderer>,
    // HDR H2 / Stage 2 colour: fullscreen post-pass blit (DRM only).
    HdrEncode=smithay::backend::renderer::gles::element::TextureShaderElement,
}

/// Clear color behind everything (matches the winit backend).
pub const CLEAR_COLOR: [f32; 4] = [0.08, 0.09, 0.11, 1.0];

/// Translucent fill for the snap-zone drop preview (accent blue @ ~30% alpha).
const SNAP_OVERLAY_COLOR: [f32; 4] = [0.36, 0.56, 0.96, 0.30];

/// On-screen rectangle of the Metis bar layer surface, used to position the
/// backdrop blur. Returned in the output's local physical coordinates; the
/// caller offsets by the output's global origin. `None` when the bar is not
/// (yet) mapped.
fn bar_layer_rect(output: &Output) -> Option<Rectangle<i32, Physical>> {
    let map = layer_map_for_output(output);
    for layer in map.layers() {
        if layer.namespace() == "metis-bar" {
            if let Some(geo) = map.layer_geometry(layer) {
                return Some(geo.to_physical(1));
            }
        }
    }
    None
}

impl MetisState {
    /// Assemble the full front-to-back render stack for a target whose global
    /// physical origin is `render_origin`, at `output_scale`. The first element
    /// in the returned vec ends up on top (smithay draws front-to-back).
    pub fn build_render_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        render_origin: Point<i32, Physical>,
        output_scale: Scale<f64>,
        target: RenderTargetInfo<'_>,
        exclude_layer_namespaces: &[&str],
        allow_blur: bool,
    ) -> Vec<OutputStack> {
        // Locked: draw ONLY the lock UI (Metis PAM chrome or a protocol locker
        // surface) and skip every normal client, layer, and decoration element.
        if self.lock.locked {
            return self.build_lock_elements(renderer, render_origin, &target, output_scale.x);
        }
        if self.protocol_lock.is_locked() {
            return self.build_protocol_lock_elements(
                renderer,
                render_origin,
                &target,
                output_scale,
            );
        }

        // Render-target-local physical origin of the full-desktop wallpaper.
        let wallpaper_origin: Point<f64, Physical> =
            Point::from((-render_origin.x as f64, -render_origin.y as f64));

        self.wallpaper.poll_decode();
        let skip_underlay = self.output_has_fullscreen(target.output_name);
        let wallpaper_elems = if skip_underlay {
            // Fullscreen game covers the output — skip wallpaper decode/upload and
            // the extra composite layer so Smithay can promote the game buffer to
            // the primary plane when formats match.
            Vec::new()
        } else {
            self.wallpaper.ensure(renderer);
            self.wallpaper.render_elements_at(wallpaper_origin)
        };

        // Bar backdrop-blur element per output (each output may carry its own
        // bar). Sampled from the wallpaper under the bar through a Gaussian
        // shader and drawn below the bar surface, above wallpaper/windows.
        let bar_rects: Vec<Rectangle<i32, Physical>> = self
            .space
            .outputs()
            .filter_map(|out| {
                // The auto-hidden bar keeps its full-size (CSS-slid) layer surface;
                // frosting behind it would paint a blurred band where no bar is.
                if self
                    .bar_auto_hidden
                    .get(&out.name())
                    .copied()
                    .unwrap_or(false)
                {
                    return None;
                }
                let out_origin = self.space.output_geometry(out)?.loc.to_physical(1);
                let local = bar_layer_rect(out)?;
                Some(Rectangle::new(
                    local.loc + out_origin - render_origin,
                    local.size,
                ))
            })
            .collect();
        if allow_blur {
            self.blur.ensure_program(renderer);
        }
        let draw_blur = allow_blur
            && self.blur.enabled
            && !skip_underlay
            && !self.splash_overlay_visible()
            && self.wallpaper.texture().is_some();
        let blur_elements: Vec<crate::blur::BlurElement> = if draw_blur {
            bar_rects
                .into_iter()
                .filter_map(|r| {
                    let rect = self.blur.confine_to_pill(r);
                    let (tex, tex_size) = self.wallpaper.texture()?;
                    self.blur.element(rect, tex, tex_size)
                })
                .collect()
        } else {
            Vec::new()
        };

        // Snap-zone drop preview (translucent fill at the destination), drawn on
        // top of everything during a titlebar drag. Bump the commit only when it
        // moves so the damage tracker treats it as one stable element.
        let snap_element = match self.snap_preview {
            Some((rect, _label)) => {
                if self.last_snap_rect != Some(rect) {
                    self.last_snap_rect = Some(rect);
                    self.snap_overlay_commit.increment();
                }
                let geo = Rectangle::<i32, Logical>::new(
                    Point::from((rect.x, rect.y)),
                    Size::from((rect.width.max(1), rect.height.max(1))),
                )
                .to_physical(1);
                let geo = Rectangle::new(geo.loc - render_origin, geo.size);
                Some(SolidColorRenderElement::new(
                    self.snap_overlay_id.clone(),
                    geo,
                    self.snap_overlay_commit,
                    Color32F::from(SNAP_OVERLAY_COLOR),
                    Kind::Unspecified,
                ))
            }
            None => {
                self.last_snap_rect = None;
                None
            }
        };

        // Server-side window decorations are built per-window below so each uses
        // the fractional scale of its output (matching client surface placement).
        let deco_specs = self.decoration_specs();
        self.decorations.begin_frame(&deco_specs);
        let deco_by_id: HashMap<u32, crate::decoration::WindowDeco> =
            deco_specs.into_iter().map(|w| (w.id, w)).collect();

        // Layer-shell surfaces from every output's layer map: background/bottom
        // render beneath windows, top/overlay above them (the Metis bar is Top).
        // Offset by output origin (to global) then by -render_origin (to local).
        let mut upper_layer_elems: Vec<OutputStack> = Vec::new();
        let mut lower_layer_elems: Vec<OutputStack> = Vec::new();
        let layer_outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for out in &layer_outputs {
            let out_origin = self
                .space
                .output_geometry(out)
                .map(|g| g.loc)
                .unwrap_or_default();
            let map = layer_map_for_output(out);
            for surface in map.layers().rev() {
                if exclude_layer_namespaces
                    .iter()
                    .any(|ns| surface.namespace() == *ns)
                {
                    continue;
                }
                let Some(geo) = map.layer_geometry(surface) else {
                    continue;
                };
                let loc =
                    (geo.loc + out_origin).to_physical_precise_round(output_scale) - render_origin;
                let elems = AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(surface, renderer, loc, output_scale, 1.0);
                let target = if matches!(surface.layer(), Layer::Background | Layer::Bottom) {
                    &mut lower_layer_elems
                } else {
                    &mut upper_layer_elems
                };
                target.extend(elems.into_iter().map(OutputStack::Surface));
            }
        }

        // smithay draws front-to-back: the FIRST element ends up on top, so the
        // wallpaper goes last (behind everything).
        let mut render_elements: Vec<OutputStack> = Vec::new();

        // Snap preview sits on top of all windows.
        if let Some(snap) = snap_element {
            render_elements.push(OutputStack::Overlay(snap));
        }
        // Top/overlay layer surfaces (the bar) above window chrome and clients.
        // Auto-hide titlebar overlays sit below the bar so a revealed titlebar
        // cannot paint through or above the edge bar strip.
        render_elements.extend(upper_layer_elems);
        // Auto-hide titlebar overlays sit below the bar so a revealed titlebar
        // cannot paint through or above the edge bar strip.
        for spec in deco_by_id.values().filter(|w| w.overlay) {
            if let Some(record) = self.windows.get(spec.id) {
                let win_scale = self.window_output_scale(&record.window, output_scale);
                let decos = self.decorations.window_elements(renderer, spec, win_scale);
                render_elements.extend(decos.into_iter().map(OutputStack::Deco));
            }
        }

        // Defensive: decorated windows not in the stacking list would otherwise
        // have no chrome drawn. Draw their decorations on top.
        {
            let stack_ids: std::collections::HashSet<u32> = self
                .space
                .elements()
                .filter_map(|w| self.windows.id_for_window(w))
                .collect();
            for (id, spec) in &deco_by_id {
                if spec.overlay || stack_ids.contains(id) {
                    continue;
                }
                tracing::warn!(
                    id,
                    "deco: window chrome not matched to a stacked window — drawing on top"
                );
                if let Some(record) = self.windows.get(*id) {
                    let win_scale = self.window_output_scale(&record.window, output_scale);
                    let decos = self.decorations.window_elements(renderer, spec, win_scale);
                    render_elements.extend(decos.into_iter().map(OutputStack::Deco));
                }
            }
        }

        // Windows top-to-bottom, each immediately followed by its own chrome so
        // an overlapping window can never hide a lower window's titlebar/border.
        // `space.elements()` yields bottom-to-top, so reverse it.
        let stacking: Vec<Window> = self.space.elements().cloned().collect();
        for window in stacking.iter().rev() {
            // Match smithay's `render_location`: the surface origin sits at the
            // mapped location minus the window-geometry offset (CSD shadow
            // margin), then offset to render-target-local coords.
            let id = self.windows.id_for_window(window);
            let win_scale = self.window_output_scale(window, output_scale);
            let elem_loc = self.space.element_location(window).unwrap_or_default();
            // X11 surfaces report size-only geometry (loc is typically 0). A stale
            // non-zero client geometry inset shifts borderless games off-screen —
            // ignore the offset for XWayland windows.
            let geo_off = if window.x11_surface().is_some() {
                Point::from((0, 0))
            } else {
                window.geometry().loc
            };
            // One-shot diagnostic when a fullscreen window is not flush at its
            // output origin. The persistent culprit for games like Hytale is a
            // *client-reported* window geometry with a negative origin (a stale
            // decoration inset) — fixed client-side by re-negotiating fullscreen,
            // not by the compositor's placement math (see the fullscreen relayout
            // nudge in the commit path).
            if let Some(id) = id {
                if self.windows.get(id).is_some_and(|r| r.fullscreen)
                    && !self.fs_offset_warned.contains(&id)
                {
                    let out_origin = self
                        .space
                        .outputs()
                        .filter_map(|o| self.space.output_geometry(o))
                        .find(|g| g.contains(elem_loc))
                        .map(|g| g.loc)
                        .unwrap_or_default();
                    if elem_loc != out_origin || geo_off.x != 0 || geo_off.y != 0 {
                        self.fs_offset_warned.insert(id);
                        tracing::info!(
                            id,
                            ?elem_loc,
                            ?geo_off,
                            ?out_origin,
                            buffer_bbox = ?window.bbox(),
                            "render: fullscreen window not flush at output origin"
                        );
                    }
                }
            }
            let mut loc = (elem_loc - geo_off).to_physical_precise_round(win_scale) - render_origin;
            let mut alpha = 1.0f32;
            if let Some(id) = id {
                let nudge = self.scroll_render_nudge(id);
                if nudge != 0 {
                    loc = Point::from((loc.x + nudge, loc.y));
                }
                if let Some((_, genie_alpha)) = self.minimize_genie_render(id) {
                    alpha = genie_alpha;
                }
            }
            // Scroll columns may overhang their display edge; clip them to their
            // own output (in local coords) so they never paint onto a neighbour.
            let clip = id.and_then(|id| {
                if self.is_minimize_genie_active(id) {
                    self.minimize_genie_render(id).map(|(r, _)| {
                        Rectangle::new(
                            Point::from((r.x, r.y)).to_physical_precise_round(win_scale)
                                - render_origin,
                            Size::from((r.width.max(1), r.height.max(1)))
                                .to_physical_precise_round(win_scale),
                        )
                    })
                } else {
                    self.scroll_window_clip(id, win_scale)
                        .map(|c| Rectangle::new(c.loc - render_origin, c.size))
                }
            });
            let elems = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(window, renderer, loc, win_scale, alpha);
            if let Some(clip) = clip {
                for e in elems {
                    if let Some(c) = CropRenderElement::from_element(e, win_scale, clip) {
                        render_elements.push(OutputStack::CropSurface(c));
                    }
                }
            } else {
                render_elements.extend(elems.into_iter().map(OutputStack::Surface));
            }
            if let Some(id) = id {
                if let Some(spec) = deco_by_id.get(&id).filter(|s| !s.overlay) {
                    let decos = self.decorations.window_elements(renderer, spec, win_scale);
                    if let Some(clip) = clip {
                        for d in decos {
                            if let Some(c) = CropRenderElement::from_element(d, win_scale, clip) {
                                render_elements.push(OutputStack::CropDeco(c));
                            }
                        }
                    } else {
                        render_elements.extend(decos.into_iter().map(OutputStack::Deco));
                    }
                }
            }
        }

        // Background/bottom layer surfaces beneath the windows.
        render_elements.extend(lower_layer_elems);
        // Below the windows, above the wallpaper.
        render_elements.extend(blur_elements.into_iter().map(OutputStack::Blur));
        if !wallpaper_elems.is_empty() {
            // Crossfade stacks incoming (with alpha) then outgoing beneath it.
            render_elements.extend(wallpaper_elems.into_iter().map(OutputStack::Wallpaper));
        } else if !skip_underlay {
            // Clean boot / slow wallpaper decode: keep a dark fill so the splash
            // logo never sits on an empty/white framebuffer.
            let size = target.size;
            render_elements.push(OutputStack::Overlay(SolidColorRenderElement::new(
                self.desktop_underlay_id.clone(),
                Rectangle::from_size(size),
                self.desktop_underlay_commit,
                Color32F::from([0.04, 0.05, 0.07, 1.0]),
                Kind::Unspecified,
            )));
        }

        // Night light / battery dim sit above the scene (cursor is drawn after).
        // Dim is inserted first so night-light warmth (if any) stacks above it.
        if let Some(dim) = crate::battery_dim::battery_dim_element(self, &target) {
            render_elements.insert(0, OutputStack::Overlay(dim));
        }
        if crate::night_light::should_render_night_light(self, &target) {
            if let Some(tint) = night_light_element(self, &target) {
                render_elements.insert(0, OutputStack::Overlay(tint));
            }
        }

        render_elements
    }
}
