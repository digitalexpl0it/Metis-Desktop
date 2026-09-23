//! Server-side window decorations (titlebar + border + window controls).
//!
//! Metis forces SSD on every toplevel (see the `XdgDecorationHandler` impl), so
//! the compositor is responsible for drawing the chrome around each tiled app
//! window. We draw it with cheap `SolidColorRenderElement`s for the titlebar,
//! border edges and the three macOS-style control buttons, plus a cached
//! `fontdue`-rasterized texture for the window title.
//!
//! The decoration rects live in the gap between the tile frame (full cell) and
//! the client surface (inset by titlebar + border via `app_tile_body_rect`), so
//! they never overlap the client buffer.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use fontdue::Font;
use metis_grid::{APP_TILE_HEADER_PX, PixelRect};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Id, Kind, render_elements};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, ImportMem};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};

// Muted, desaturated control colors tuned to the dark slate theme rather than the
// stock bright "traffic light" palette — they read as part of the chrome, not as
// neon dots.
const BTN_CLOSE: [f32; 4] = [0.74, 0.37, 0.36, 1.0];
const BTN_MIN: [f32; 4] = [0.74, 0.58, 0.34, 1.0];
const BTN_MAX: [f32; 4] = [0.40, 0.62, 0.46, 1.0];
/// Traffic-light buttons desaturate to a flat gray when the window is unfocused.
const BTN_INACTIVE: [f32; 4] = [0.30, 0.32, 0.37, 1.0];
/// Glyph (×, +, −) drawn over a focused button — a light translucent symbol so it
/// stays legible on the muted button fills.
const BTN_GLYPH: [f32; 4] = [0.92, 0.94, 0.97, 0.85];
/// Supersample factor for button textures so the circle/glyph edges are smooth.
const BTN_SS: i32 = 3;

const BTN_SIZE: i32 = 17;
const BTN_GAP: i32 = 9;
const BTN_RIGHT_PAD: i32 = 12;
const TITLE_LEFT_PAD: i32 = 12;
const TITLE_FONT_PX: f32 = 14.0;
/// Padding baked around the title text to form its opaque rounded "pill". The pill
/// is part of the (always-opaque) title texture, so the window name stays solid and
/// legible even when the titlebar background opacity is turned down.
const TITLE_PILL_PAD_X: i32 = 9;
const TITLE_PILL_PAD_Y: i32 = 3;

/// Radius of the rounded top corners on the server-side titlebar.
const CORNER_RADIUS_PX: i32 = 10;
/// Supersample factor for the titlebar texture so the rounded corners are smooth.
const TITLEBAR_SS: i32 = 2;

/// Soft drop shadow cast behind every framed window, drawn as an outer ring that
/// lives *entirely outside* the frame: the alpha peaks at the window edge and fades
/// to zero `MARGIN` px outward, so no part of it ever overlaps (and darkens) the
/// client or a translucent titlebar. It is 9-sliced from a single cached texture, so
/// it costs one shared texture plus eight stretched quads per window regardless of
/// size (no per-resize re-rasterization). `ALPHA` is the peak opacity at the edge.
const SHADOW_MARGIN: i32 = 14;
const SHADOW_ALPHA: f32 = 0.34;

/// The decoration colors, derived from the active Metis theme (`themes/*.json`) so
/// the server-side chrome tracks light/dark mode like the rest of the DE. Refreshed
/// live (~1s) alongside the titlebar opacity.
#[derive(Clone, PartialEq)]
struct Palette {
    titlebar_active: [f32; 3],
    titlebar_inactive: [f32; 3],
    border_active: [f32; 4],
    border_inactive: [f32; 4],
    text_active: [f32; 4],
    text_inactive: [f32; 4],
    /// Theme accent stops (the full `accent` array) used for the focused window's
    /// title-pill border when the pill-border mode is `accent`.
    accent: Vec<[f32; 3]>,
}

impl Default for Palette {
    fn default() -> Self {
        load_palette()
    }
}

/// Load the active theme tokens the same way the shell resolves them. The
/// compositor can't query GTK's system-theme setting, so `System` falls back to
/// the dark tokens (an explicit light/dark preference is honored exactly).
fn load_active_theme_tokens() -> metis_config::ThemeTokens {
    use metis_config::ThemeMode;
    match metis_config::load_theme_preference().unwrap_or(ThemeMode::Dark) {
        ThemeMode::Light => metis_config::load_theme_tokens("light"),
        ThemeMode::Dark | ThemeMode::System => metis_config::load_theme_tokens("dark"),
    }
}

/// Build the decoration palette from the active theme: the titlebar uses the
/// raised/base surface tokens, the frame the border token, and the title text the
/// text / muted-text tokens.
fn load_palette() -> Palette {
    let t = load_active_theme_tokens();
    let tb_active = hex_rgb(&t.surface_raised);
    let tb_inactive = hex_rgb(&t.surface);
    let border = hex_rgb(&t.border);
    let border_inactive = [
        lerp(border[0], tb_inactive[0], 0.5),
        lerp(border[1], tb_inactive[1], 0.5),
        lerp(border[2], tb_inactive[2], 0.5),
        1.0,
    ];
    let text = hex_rgb(&t.text);
    let muted = hex_rgb(&t.text_muted);
    let accent: Vec<[f32; 3]> = t.accent.iter().map(|h| hex_rgb(h)).collect();
    Palette {
        titlebar_active: tb_active,
        titlebar_inactive: tb_inactive,
        border_active: [border[0], border[1], border[2], 1.0],
        border_inactive,
        text_active: [text[0], text[1], text[2], 1.0],
        text_inactive: [muted[0], muted[1], muted[2], 1.0],
        accent,
    }
}

/// Parse a `#rrggbb` color into linear-ish `[r, g, b]` in 0..1 (sRGB bytes / 255,
/// matching how the GL pipeline treats the rest of the chrome). Falls back to a
/// dark slate on malformed input.
fn hex_rgb(hex: &str) -> [f32; 3] {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return [0.13, 0.14, 0.17];
    }
    let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    [parse(0), parse(2), parse(4)]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

render_elements! {
    pub DecorationElement<=GlesRenderer>;
    Solid=SolidColorRenderElement,
    Text=TextureRenderElement<GlesTexture>,
}

/// One window's decoration geometry + identity, gathered before drawing.
#[derive(Clone)]
pub struct WindowDeco {
    pub id: u32,
    /// Full tile frame in monitor-logical coordinates. For a normal window this is
    /// the client grown by the chrome; for an `overlay` window it is the client
    /// rect itself (the titlebar overlays its top strip).
    pub frame: PixelRect,
    pub title: String,
    pub focused: bool,
    /// When true this is an auto-hide window's revealed titlebar: draw only the
    /// titlebar + title + controls (no surrounding border) and render it *above*
    /// the client surface as a translucent overlay.
    pub overlay: bool,
    /// Slide progress for overlay titlebars: 0 = hidden above the client top edge,
    /// 1 = fully shown. Ignored when `overlay` is false.
    pub overlay_reveal: f32,
    /// When true the overlay is a compact top-right control strip (for tabbed
    /// browsers) instead of a full-width titlebar over the client's tab row.
    pub overlay_compact: bool,
}

/// Render elements for a frame, split by stacking layer relative to the client
/// surfaces: `below` chrome draws behind the clients (it only fills the reserved
/// gaps), `overlay` chrome draws on top of them (the auto-hide titlebar reveal).
pub struct DecoElements {
    /// Normal chrome, grouped per window id. The renderer interleaves each
    /// window's chrome directly beneath that window's own surface (and above the
    /// windows stacked below it), so an overlapping window can never hide a
    /// lower window's titlebar.
    pub below: HashMap<u32, Vec<DecorationElement>>,
    /// Auto-hide reveal chrome (drawn above all clients as a translucent strip).
    pub overlay: Vec<DecorationElement>,
}

/// Hit-test regions for a window's controls, in monitor-logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoControl {
    Close,
    Minimize,
    Maximize,
    Titlebar,
}

#[derive(Clone)]
struct CachedTitle {
    text: String,
    color: [f32; 4],
    pill: [f32; 4],
    /// Pill-border stops (1 = flat, >1 = left→right gradient) and stroke width, so
    /// the texture re-rasterizes when the configured border changes.
    border: Vec<[f32; 3]>,
    border_px: f32,
    width: i32,
    height: i32,
    buffer: TextureBuffer<GlesTexture>,
}

/// A cached, anti-aliased control-button texture (rounded circle + glyph). Keyed
/// per `(window, role)` so each window owns a distinct buffer id; re-rasterized
/// only when the window's focus state flips.
#[derive(Clone)]
struct CachedButton {
    focused: bool,
    buffer: TextureBuffer<GlesTexture>,
}

/// A cached titlebar-background texture with rounded top corners. Re-rasterized
/// when the window's width, opacity, focus, or overlay mode changes (the corners
/// need fully transparent pixels, so a flat `SolidColorRenderElement` can't be
/// used).
#[derive(Clone)]
struct CachedTitlebar {
    width: i32,
    header: i32,
    /// Total frame height — the denominator for the vertical border gradient, so the
    /// titlebar ring samples the same top→bottom ramp as the side edges.
    frame_height: i32,
    /// Border thickness baked into the ring (runtime-configurable).
    border_px: i32,
    alpha: f32,
    focused: bool,
    overlay: bool,
    /// Border gradient stops baked into the ring (1 = flat, >1 = top→bottom ramp).
    border: Vec<[f32; 3]>,
    buffer: TextureBuffer<GlesTexture>,
}

/// Cached vertical-gradient side-border edge texture (left + right share it). The
/// frame's left/right edges below the titlebar are colored by a top→bottom gradient
/// matching the titlebar ring and bottom edge. Re-rasterized when the window height,
/// focus, or border stops change.
#[derive(Clone)]
struct CachedBorder {
    height: i32,
    frame_height: i32,
    focused: bool,
    stops: Vec<[f32; 3]>,
    /// One buffer per side (`[left, right]`) so each quad has a distinct element id.
    bufs: Vec<TextureBuffer<GlesTexture>>,
}

/// Outcome of a throttled config refresh: whether to re-damage and/or relayout.
#[derive(Default, Clone, Copy)]
pub struct DecoRefresh {
    /// Chrome appearance changed — caller should flag damage / redraw.
    pub damage: bool,
    /// Border thickness changed — caller should re-apply window rects so the client
    /// body inset tracks the new frame width.
    pub relayout: bool,
}

/// Persistent decoration resources owned by `MetisState`.
pub struct DecorationRuntime {
    font: Option<Font>,
    ids: HashMap<(u32, u8), Id>,
    titles: HashMap<u32, CachedTitle>,
    buttons: HashMap<(u32, u8), CachedButton>,
    titlebars: HashMap<u32, CachedTitlebar>,
    borders: HashMap<u32, CachedBorder>,
    /// Single shared drop-shadow texture (black, premultiplied), rasterized once
    /// and 9-sliced for every window. `None` until the first frame imports it.
    /// Used for the straight edges and the (square) bottom corners.
    shadow_tex: Option<GlesTexture>,
    /// Rounded top-corner shadow textures (left + right). The window's titlebar has
    /// rounded top corners, so the shadow there hugs the arc instead of a square
    /// corner. Fixed size (`MARGIN + CORNER_RADIUS`), rasterized once.
    shadow_corner_tl: Option<GlesTexture>,
    shadow_corner_tr: Option<GlesTexture>,
    /// Per-window texture buffers wrapping the shadow textures — one per slice so
    /// each quad has a stable, distinct element id for damage tracking.
    shadow_bufs: HashMap<u32, Vec<TextureBuffer<GlesTexture>>>,
    commit: CommitCounter,
    last_sig: u64,
    /// Configurable titlebar background opacity (title text + buttons stay
    /// opaque). Read from `bar.json` and refreshed live, mirroring the bar blur.
    titlebar_alpha: f32,
    /// Theme-derived chrome colors, refreshed live so light/dark switches apply.
    palette: Palette,
    /// Title-pill border appearance (mode/color/gradient/width) from `bar.json`,
    /// refreshed live alongside the titlebar opacity.
    pill_border: metis_config::TitlebarPillBorder,
    /// Window frame border appearance + thickness from `bar.json`, refreshed live.
    /// The thickness is mirrored into `metis_grid`'s client inset.
    window_border: metis_config::WindowBorder,
    last_config_check: Instant,
    /// Output scale used while building render elements for the current window.
    build_scale: Scale<f64>,
}

impl Default for DecorationRuntime {
    fn default() -> Self {
        Self {
            font: load_font(),
            ids: HashMap::new(),
            titles: HashMap::new(),
            buttons: HashMap::new(),
            titlebars: HashMap::new(),
            borders: HashMap::new(),
            shadow_tex: None,
            shadow_corner_tl: None,
            shadow_corner_tr: None,
            shadow_bufs: HashMap::new(),
            commit: CommitCounter::default(),
            last_sig: 0,
            titlebar_alpha: read_titlebar_opacity(),
            palette: load_palette(),
            pill_border: read_pill_border(),
            window_border: read_window_border(),
            last_config_check: Instant::now(),
            build_scale: Scale::from(1.0),
        }
    }
}

/// Read the titlebar background opacity from `~/.config/metis/bar.json`, clamped
/// to a sane range. Defaults are supplied by `metis-config`.
fn read_titlebar_opacity() -> f32 {
    metis_config::load_bar_config()
        .titlebar_opacity
        .clamp(0.0, 1.0)
}

/// Read the title-pill border appearance from `~/.config/metis/bar.json`. Defaults
/// are supplied by `metis-config`.
fn read_pill_border() -> metis_config::TitlebarPillBorder {
    metis_config::load_bar_config().titlebar_pill_border
}

/// Read the window frame border appearance + thickness from `~/.config/metis/bar.json`,
/// mirroring the thickness into `metis_grid` so the client body inset matches the
/// drawn border. Defaults are supplied by `metis-config`.
fn read_window_border() -> metis_config::WindowBorder {
    let wb = metis_config::load_bar_config().window_border;
    metis_grid::set_app_tile_border_px(wb.width_px.round() as i32);
    wb
}

impl DecorationRuntime {
    /// Force a full decoration re-damage after output scale or geometry changes.
    pub fn invalidate_all(&mut self) {
        self.last_sig = 0;
        self.commit.increment();
    }

    /// Drop cached title / button / titlebar GL textures (e.g. after a connector
    /// disconnect). The next frame re-rasterizes them so SSD chrome does not keep
    /// a hollow titlebar with missing controls.
    pub fn clear_texture_caches(&mut self) {
        self.titles.clear();
        self.buttons.clear();
        self.titlebars.clear();
        self.borders.clear();
        self.shadow_bufs.clear();
        self.shadow_tex = None;
        self.shadow_corner_tl = None;
        self.shadow_corner_tr = None;
        self.commit.increment();
    }

    fn prune_dead_windows(&mut self, live: &std::collections::HashSet<u32>) {
        self.titles.retain(|id, _| live.contains(id));
        self.ids.retain(|(id, _), _| live.contains(id));
        self.buttons.retain(|(id, _), _| live.contains(id));
        self.titlebars.retain(|id, _| live.contains(id));
        self.borders.retain(|id, _| live.contains(id));
        self.shadow_bufs.retain(|id, _| live.contains(id));
    }

    /// Prune stale caches and bump the damage commit when decoration geometry
    /// changed since the last frame.
    pub fn begin_frame(&mut self, windows: &[WindowDeco]) {
        let live: std::collections::HashSet<u32> = windows.iter().map(|w| w.id).collect();
        self.prune_dead_windows(&live);
        let sig = signature(windows) ^ self.titlebar_alpha.to_bits() as u64;
        if sig != self.last_sig {
            self.last_sig = sig;
            self.commit.increment();
        }
    }

    fn phys(&self, r: PixelRect) -> Rectangle<i32, Physical> {
        Rectangle::<i32, Logical>::new(
            Point::from((r.x, r.y)),
            Size::from((r.width.max(1), r.height.max(1))),
        )
        .to_physical_precise_round(self.build_scale)
    }

    fn logical_point(&self, x: i32, y: i32) -> Point<i32, Physical> {
        Point::<i32, Logical>::from((x, y)).to_physical_precise_round(self.build_scale)
    }

    /// Build render elements for one decorated window at `output_scale`.
    pub fn window_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
        output_scale: Scale<f64>,
    ) -> Vec<DecorationElement> {
        self.build_scale = output_scale;
        let frame = w.frame;
        if frame.width <= 2 || frame.height <= APP_TILE_HEADER_PX {
            return Vec::new();
        }

        let mut out = Vec::new();
        let titlebar_rgb = if w.focused {
            self.palette.titlebar_active
        } else {
            self.palette.titlebar_inactive
        };
        let border_stops: Vec<[f32; 3]> = if w.focused {
            resolve_border_stops(
                self.window_border.mode,
                &self.window_border.color,
                &self.window_border.gradient,
                &self.palette,
            )
        } else {
            vec![[
                self.palette.border_inactive[0],
                self.palette.border_inactive[1],
                self.palette.border_inactive[2],
            ]]
        };

        let header = APP_TILE_HEADER_PX.min(frame.height);
        let b = metis_grid::app_tile_border_px();
        let chrome = if w.overlay {
            overlay_chrome_rect(frame, w.overlay_reveal, w.overlay_compact)
        } else {
            PixelRect {
                x: frame.x,
                y: frame.y,
                width: frame.width,
                height: header,
            }
        };
        let bar_x = chrome.x;
        let bar_y = chrome.y;
        let bar_w = chrome.width;
        let titlebar_alpha = if w.overlay {
            self.titlebar_alpha * metis_grid::ease_out_cubic(w.overlay_reveal.clamp(0.0, 1.0))
        } else {
            self.titlebar_alpha
        };
        let commit = self.commit;

        if !w.overlay && b > 0 {
            for elem in self.border_edge_elements(renderer, w, b, header, &border_stops) {
                out.push(elem);
            }
            let t_bottom = if frame.height > 0 {
                (frame.height - b) as f32 / frame.height as f32
            } else {
                1.0
            };
            let bc = sample_gradient(&border_stops, t_bottom);
            out.push(self.solid(
                w.id,
                3,
                PixelRect {
                    x: frame.x,
                    y: frame.y + frame.height - b,
                    width: frame.width,
                    height: b,
                },
                [bc[0], bc[1], bc[2], 1.0],
                commit,
            ));
        }

        let cy = bar_y + (header - BTN_SIZE) / 2;
        let close_x = frame.x + frame.width - BTN_RIGHT_PAD - BTN_SIZE;
        let max_x = close_x - (BTN_GAP + BTN_SIZE);
        let min_x = max_x - (BTN_GAP + BTN_SIZE);
        // Smithay paints the *first* element on top — push fill last so buttons
        // and title sit above the titlebar background.
        for (role, kind, x) in [
            (4u8, DecoControl::Close, close_x),
            (5u8, DecoControl::Maximize, max_x),
            (6u8, DecoControl::Minimize, min_x),
        ] {
            if let Some(elem) = self.button_element(renderer, w, role, kind, x, cy) {
                out.push(elem);
            }
        }

        if !w.overlay_compact {
            let tx = frame.x + TITLE_LEFT_PAD;
            let max_text_w = (min_x - BTN_GAP - tx).max(0);
            if max_text_w > 8
                && let Some(elem) = self.title_element(renderer, w, tx, max_text_w, header, bar_y)
            {
                out.push(elem);
            }
        }

        if let Some(elem) = self.titlebar_element(
            renderer,
            w,
            bar_w,
            header,
            frame.height,
            b,
            titlebar_rgb,
            titlebar_alpha,
            &border_stops,
            bar_x,
            bar_y,
            w.overlay,
        ) {
            out.push(elem);
        }

        if !w.overlay {
            for elem in self.shadow_elements(renderer, w) {
                out.push(elem);
            }
        }

        out
    }

    /// Throttled re-read of `bar.json` + the active theme (~1s) so a Settings app
    /// changing the titlebar opacity / borders / light-dark theme is picked up live.
    /// Returns whether the caller should re-damage and/or relayout windows.
    pub fn maybe_refresh(&mut self) -> DecoRefresh {
        if self.last_config_check.elapsed() < Duration::from_secs(1) {
            return DecoRefresh::default();
        }
        self.last_config_check = Instant::now();
        let mut out = DecoRefresh::default();
        let alpha = read_titlebar_opacity();
        if (alpha - self.titlebar_alpha).abs() > f32::EPSILON {
            self.titlebar_alpha = alpha;
            out.damage = true;
        }
        let pill_border = read_pill_border();
        if pill_border != self.pill_border {
            self.pill_border = pill_border;
            // The cached title textures bake in the old pill stroke; drop them.
            self.titles.clear();
            self.commit.increment();
            out.damage = true;
        }
        let window_border = read_window_border();
        if window_border != self.window_border {
            // A thickness change must also relayout (the client body inset changed);
            // `read_window_border` already pushed the new width into `metis_grid`.
            let width_changed =
                (window_border.width_px - self.window_border.width_px).abs() > f32::EPSILON;
            self.window_border = window_border;
            // The titlebar ring + side edges bake the frame stroke/width; drop them.
            self.titlebars.clear();
            self.borders.clear();
            self.commit.increment();
            out.damage = true;
            out.relayout |= width_changed;
        }
        let palette = load_palette();
        if palette != self.palette {
            self.palette = palette;
            // The cached textures (titlebar / title / buttons / borders) bake in the
            // old colors, so drop them; bump the commit so solid elements re-damage.
            self.titles.clear();
            self.buttons.clear();
            self.titlebars.clear();
            self.borders.clear();
            self.commit.increment();
            out.damage = true;
        }
        out
    }

    /// Build the render elements for every decorated window. Front-to-back order
    /// within the returned vec does not matter — decoration rects never overlap.
    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        windows: &[WindowDeco],
        output_scale: Scale<f64>,
    ) -> DecoElements {
        self.begin_frame(windows);

        let mut below: HashMap<u32, Vec<DecorationElement>> = HashMap::new();
        let mut overlay = Vec::new();
        for w in windows {
            let elems = self.window_elements(renderer, w, output_scale);
            if w.overlay {
                overlay.extend(elems);
            } else {
                below.insert(w.id, elems);
            }
        }
        DecoElements { below, overlay }
    }

    fn solid(
        &mut self,
        window_id: u32,
        role: u8,
        rect: PixelRect,
        color: [f32; 4],
        commit: CommitCounter,
    ) -> DecorationElement {
        let id = self
            .ids
            .entry((window_id, role))
            .or_insert_with(Id::new)
            .clone();
        let geo = self.phys(rect);
        DecorationElement::Solid(SolidColorRenderElement::new(
            id,
            geo,
            commit,
            Color32F::from(color),
            Kind::Unspecified,
        ))
    }

    /// Build (or reuse) a rounded control-button texture and place it at `(x, cy)`.
    fn button_element(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
        role: u8,
        kind: DecoControl,
        x: i32,
        cy: i32,
    ) -> Option<DecorationElement> {
        let needs_render = self
            .buttons
            .get(&(w.id, role))
            .map(|c| c.focused != w.focused)
            .unwrap_or(true);

        if needs_render {
            let (pixels, pw, ph) = rasterize_button(kind, w.focused)?;
            let texture = renderer
                .import_memory(&pixels, Fourcc::Abgr8888, Size::from((pw, ph)), false)
                .ok()?;
            let buffer =
                TextureBuffer::from_texture(renderer, texture, BTN_SS, Transform::Normal, None);
            self.buttons.insert(
                (w.id, role),
                CachedButton {
                    focused: w.focused,
                    buffer,
                },
            );
        }

        let cached = self.buttons.get(&(w.id, role))?;
        let src = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((BTN_SIZE as f64, BTN_SIZE as f64)),
        );
        let loc = self.logical_point(x, cy);
        Some(DecorationElement::Text(
            TextureRenderElement::from_texture_buffer(
                loc.to_f64(),
                &cached.buffer,
                None,
                Some(src),
                Some(Size::from((BTN_SIZE, BTN_SIZE))),
                Kind::Unspecified,
            ),
        ))
    }

    /// Build (or reuse) the titlebar-background texture (rounded top corners) and
    /// place it across the top of the frame. Re-rasterized only when the window's
    /// width, opacity, or focus changes.
    #[allow(clippy::too_many_arguments)]
    fn titlebar_element(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
        width: i32,
        header: i32,
        frame_height: i32,
        border_px: i32,
        color: [f32; 3],
        alpha: f32,
        border: &[[f32; 3]],
        bar_x: i32,
        bar_y: i32,
        overlay: bool,
    ) -> Option<DecorationElement> {
        if width <= 0 || header <= 0 {
            return None;
        }
        let needs_render = self
            .titlebars
            .get(&w.id)
            .map(|c| {
                c.width != width
                    || c.header != header
                    || c.frame_height != frame_height
                    || c.border_px != border_px
                    || c.focused != w.focused
                    || c.overlay != overlay
                    || c.border.as_slice() != border
                    || (c.alpha - alpha).abs() > f32::EPSILON
            })
            .unwrap_or(true);

        if needs_render {
            let (pixels, pw, ph) = rasterize_titlebar(
                color,
                alpha,
                border,
                width,
                header,
                frame_height,
                border_px,
                overlay,
            )?;
            let texture = renderer
                .import_memory(&pixels, Fourcc::Abgr8888, Size::from((pw, ph)), false)
                .ok()?;
            let buffer = TextureBuffer::from_texture(
                renderer,
                texture,
                TITLEBAR_SS,
                Transform::Normal,
                None,
            );
            self.titlebars.insert(
                w.id,
                CachedTitlebar {
                    width,
                    header,
                    frame_height,
                    border_px,
                    alpha,
                    focused: w.focused,
                    overlay,
                    border: border.to_vec(),
                    buffer,
                },
            );
        }

        let cached = self.titlebars.get(&w.id)?;
        let src = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((width as f64, header as f64)),
        );
        let loc = self.logical_point(bar_x, bar_y);
        Some(DecorationElement::Text(
            TextureRenderElement::from_texture_buffer(
                loc.to_f64(),
                &cached.buffer,
                None,
                Some(src),
                Some(Size::from((width, header))),
                Kind::Unspecified,
            ),
        ))
    }

    /// Build (or reuse) the shared vertical-gradient side-border texture and place it
    /// as the window's left and right edges (below the titlebar). Re-rasterized only
    /// when the window height, focus, or border stops change.
    fn border_edge_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
        b: i32,
        header: i32,
        stops: &[[f32; 3]],
    ) -> Vec<DecorationElement> {
        let frame = w.frame;
        let height = frame.height - header;
        if b <= 0 || height <= 0 {
            return Vec::new();
        }
        let needs_render = self
            .borders
            .get(&w.id)
            .map(|c| {
                c.height != height
                    || c.frame_height != frame.height
                    || c.focused != w.focused
                    || c.stops.as_slice() != stops
            })
            .unwrap_or(true);

        if needs_render {
            let Some((pixels, pw, ph)) =
                rasterize_border_edge(stops, b, height, header, frame.height)
            else {
                return Vec::new();
            };
            let texture = match renderer.import_memory(
                &pixels,
                Fourcc::Abgr8888,
                Size::from((pw, ph)),
                false,
            ) {
                Ok(t) => t,
                Err(_) => return Vec::new(),
            };
            let bufs = (0..2)
                .map(|_| {
                    TextureBuffer::from_texture(
                        renderer,
                        texture.clone(),
                        1,
                        Transform::Normal,
                        None,
                    )
                })
                .collect::<Vec<_>>();
            self.borders.insert(
                w.id,
                CachedBorder {
                    height,
                    frame_height: frame.height,
                    focused: w.focused,
                    stops: stops.to_vec(),
                    bufs,
                },
            );
        }

        let Some(cached) = self.borders.get(&w.id) else {
            return Vec::new();
        };
        let src = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((b as f64, height as f64)),
        );
        let positions = [
            (frame.x, frame.y + header),
            (frame.x + frame.width - b, frame.y + header),
        ];
        let mut out = Vec::with_capacity(2);
        for (i, (x, y)) in positions.iter().enumerate() {
            let Some(buf) = cached.bufs.get(i) else {
                continue;
            };
            let loc = self.logical_point(*x, *y);
            out.push(DecorationElement::Text(
                TextureRenderElement::from_texture_buffer(
                    loc.to_f64(),
                    buf,
                    None,
                    Some(src),
                    Some(Size::from((b, height))),
                    Kind::Unspecified,
                ),
            ));
        }
        out
    }

    fn title_element(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
        x: i32,
        max_w: i32,
        header: i32,
        bar_y: i32,
    ) -> Option<DecorationElement> {
        let font = self.font.as_ref()?;
        let color = if w.focused {
            self.palette.text_active
        } else {
            self.palette.text_inactive
        };
        // Opaque pill behind the title. Derive it from this window's titlebar shade
        // but nudge it toward white-on-dark / black-on-light so the chip is clearly
        // visible against the titlebar (and solid over the wallpaper when the
        // titlebar opacity is turned down). Always alpha 1.0.
        let tb = if w.focused {
            self.palette.titlebar_active
        } else {
            self.palette.titlebar_inactive
        };
        let pill = pill_base(tb);
        // Thin border ringing the pill. The focused window draws the configured
        // pill-border (accent gradient / solid / custom gradient) as a crisp pop;
        // unfocused windows always fall back to a muted slate stroke so background
        // titles stay quiet.
        let border = if w.focused {
            resolve_border_stops(
                self.pill_border.mode,
                &self.pill_border.color,
                &self.pill_border.gradient,
                &self.palette,
            )
        } else {
            vec![[
                self.palette.border_inactive[0],
                self.palette.border_inactive[1],
                self.palette.border_inactive[2],
            ]]
        };
        let border_px = self.pill_border.width_px.clamp(0.0, 8.0);

        let needs_render = self
            .titles
            .get(&w.id)
            .map(|c| {
                c.text != w.title
                    || c.color != color
                    || c.pill != pill
                    || c.border != border
                    || (c.border_px - border_px).abs() > f32::EPSILON
            })
            .unwrap_or(true);

        if needs_render {
            if let Some((pixels, tw, th)) = rasterize(
                font,
                &w.title,
                TITLE_FONT_PX,
                color,
                pill,
                &border,
                border_px,
            ) {
                match renderer.import_memory(&pixels, Fourcc::Abgr8888, Size::from((tw, th)), false)
                {
                    Ok(texture) => {
                        let buffer = TextureBuffer::from_texture(
                            renderer,
                            texture,
                            1,
                            Transform::Normal,
                            None,
                        );
                        self.titles.insert(
                            w.id,
                            CachedTitle {
                                text: w.title.clone(),
                                color,
                                pill,
                                border,
                                border_px,
                                width: tw,
                                height: th,
                                buffer,
                            },
                        );
                    }
                    _ => {
                        return None;
                    }
                }
            } else {
                self.titles.remove(&w.id);
                return None;
            }
        }

        let cached = self.titles.get(&w.id)?;
        let (tw, th) = (cached.width, cached.height);
        let draw_w = tw.min(max_w);
        let y = bar_y + (header - th) / 2;
        let src = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((draw_w as f64, th as f64)),
        );
        let loc = self.logical_point(x, y);
        Some(DecorationElement::Text(
            TextureRenderElement::from_texture_buffer(
                loc.to_f64(),
                &cached.buffer,
                None,
                Some(src),
                Some(Size::from((draw_w, th))),
                Kind::Unspecified,
            ),
        ))
    }

    /// Build the drop-shadow quads for one window. The shadow is an outer ring that
    /// never overlaps the frame interior: straight edges + (square) bottom corners
    /// come from one shared radial texture, while the two top corners use dedicated
    /// rounded-corner textures so the shadow hugs the titlebar's rounded top corners.
    /// All textures are imported once; each window owns one `TextureBuffer` per piece
    /// so every quad has a stable, distinct element id for damage tracking.
    fn shadow_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        w: &WindowDeco,
    ) -> Vec<DecorationElement> {
        let m = SHADOW_MARGIN;
        let r = CORNER_RADIUS_PX;
        let f = w.frame;
        // Need room for the rounded top corners (2*r wide) plus the straight runs.
        if m <= 0 || f.width <= 2 * r || f.height <= r + m {
            return Vec::new();
        }

        // Ensure the three shared shadow textures exist (radial edge tex + the two
        // rounded top-corner textures). Any failure simply skips the shadow.
        if self.shadow_tex.is_none() {
            let Some((px, pw, ph)) = rasterize_shadow() else {
                return Vec::new();
            };
            match renderer.import_memory(&px, Fourcc::Abgr8888, Size::from((pw, ph)), false) {
                Ok(t) => self.shadow_tex = Some(t),
                Err(_) => return Vec::new(),
            }
        }
        if self.shadow_corner_tl.is_none() {
            let Some((px, pw, ph)) = rasterize_shadow_corner(false) else {
                return Vec::new();
            };
            match renderer.import_memory(&px, Fourcc::Abgr8888, Size::from((pw, ph)), false) {
                Ok(t) => self.shadow_corner_tl = Some(t),
                Err(_) => return Vec::new(),
            }
        }
        if self.shadow_corner_tr.is_none() {
            let Some((px, pw, ph)) = rasterize_shadow_corner(true) else {
                return Vec::new();
            };
            match renderer.import_memory(&px, Fourcc::Abgr8888, Size::from((pw, ph)), false) {
                Ok(t) => self.shadow_corner_tr = Some(t),
                Err(_) => return Vec::new(),
            }
        }
        let (Some(edge), Some(ctl), Some(ctr)) = (
            self.shadow_tex.clone(),
            self.shadow_corner_tl.clone(),
            self.shadow_corner_tr.clone(),
        ) else {
            return Vec::new();
        };

        // Ensure this window's slice buffers exist. Index → source texture is fixed:
        // 0 = rounded TL, 1 = rounded TR, 2..8 = radial (edges + square bottom corners).
        self.shadow_bufs.entry(w.id).or_insert_with(|| {
            let sources = [&ctl, &ctr, &edge, &edge, &edge, &edge, &edge, &edge];
            sources
                .iter()
                .map(|t| {
                    TextureBuffer::from_texture(renderer, (*t).clone(), 1, Transform::Normal, None)
                })
                .collect::<Vec<_>>()
        });
        let Some(bufs) = self.shadow_bufs.get(&w.id) else {
            return Vec::new();
        };

        let s2 = m + r; // rounded-corner texture side
        let tail = m + 2; // far edge/corner offset in the radial texture

        // (src_x, src_y, src_w, src_h, dst_x, dst_y, dst_w, dst_h). Index order must
        // match the `sources` mapping above. Every dst sits outside the frame.
        type ShadowSlice = (i32, i32, i32, i32, i32, i32, i32, i32);
        let slices: [ShadowSlice; 8] = [
            // Rounded top corners (full corner texture, unstretched).
            (0, 0, s2, s2, f.x - m, f.y - m, s2, s2),
            (0, 0, s2, s2, f.x + f.width - r, f.y - m, s2, s2),
            // Top + bottom edges (vertical fade), between the corners.
            (m, 0, 2, m, f.x + r, f.y - m, f.width - 2 * r, m),
            (m, tail, 2, m, f.x, f.y + f.height, f.width, m),
            // Left + right edges (horizontal fade), below the rounded top corners.
            (0, m, m, 2, f.x - m, f.y + r, m, f.height - r),
            (tail, m, m, 2, f.x + f.width, f.y + r, m, f.height - r),
            // Square bottom corners (radial fade).
            (0, tail, m, m, f.x - m, f.y + f.height, m, m),
            (tail, tail, m, m, f.x + f.width, f.y + f.height, m, m),
        ];

        let mut out = Vec::with_capacity(8);
        for (i, (sx, sy, sw, sh, dx, dy, dw, dh)) in slices.iter().enumerate() {
            let src = Rectangle::<f64, Logical>::new(
                Point::from((*sx as f64, *sy as f64)),
                Size::from((*sw as f64, *sh as f64)),
            );
            let loc = self.logical_point(*dx, *dy);
            out.push(DecorationElement::Text(
                TextureRenderElement::from_texture_buffer(
                    loc.to_f64(),
                    &bufs[i],
                    None,
                    Some(src),
                    Some(Size::from((*dw, *dh))),
                    Kind::Unspecified,
                ),
            ));
        }
        out
    }
}

/// Chrome strip for a sliding overlay titlebar at `reveal` progress (0..1).
pub fn overlay_chrome_rect(frame: PixelRect, reveal: f32, compact: bool) -> PixelRect {
    let header = APP_TILE_HEADER_PX.min(frame.height.max(1));
    let t = metis_grid::ease_out_cubic(reveal.clamp(0.0, 1.0));
    let y = frame.y - ((header as f32) * (1.0 - t)).round() as i32;
    if compact {
        let w = metis_grid::OVERLAY_CONTROLS_WIDTH_PX.min(frame.width.max(1));
        PixelRect {
            x: frame.x + frame.width - w,
            y,
            width: w,
            height: header,
        }
    } else {
        PixelRect {
            x: frame.x,
            y,
            width: frame.width.max(1),
            height: header,
        }
    }
}

/// Compute hit-test rects (monitor-logical) for a window's controls, given its
/// full tile frame. Returns `(control, rect)` pairs in priority order (buttons
/// before the general titlebar drag region).
pub fn control_hitboxes(frame: PixelRect, compact: bool) -> Vec<(DecoControl, PixelRect)> {
    let header = APP_TILE_HEADER_PX.min(frame.height);
    let close_x = frame.x + frame.width - BTN_RIGHT_PAD - BTN_SIZE;
    let max_x = close_x - (BTN_GAP + BTN_SIZE);
    let min_x = max_x - (BTN_GAP + BTN_SIZE);
    let hit = |x: i32| PixelRect {
        x: x - BTN_GAP / 2,
        y: frame.y,
        width: BTN_SIZE + BTN_GAP,
        height: header,
    };
    let titlebar = if compact {
        let strip_w = metis_grid::OVERLAY_CONTROLS_WIDTH_PX.min(frame.width.max(1));
        PixelRect {
            x: frame.x + frame.width - strip_w,
            y: frame.y,
            width: strip_w,
            height: header,
        }
    } else {
        PixelRect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: header,
        }
    };
    vec![
        (DecoControl::Close, hit(close_x)),
        (DecoControl::Maximize, hit(max_x)),
        (DecoControl::Minimize, hit(min_x)),
        (DecoControl::Titlebar, titlebar),
    ]
}

/// Rasterize `text` onto an opaque rounded "pill": a flat solid plate of `pill`
/// color ringed by a thin stroke `border_px` wide. The stroke is a flat color when
/// `border` has one stop, or a left→right gradient across its stops. The pill keeps
/// the window name solid/legible no matter how translucent the titlebar background
/// is. Returns premultiplied RGBA `(pixels, width, height)`, or `None` when the
/// string is empty/too small.
fn rasterize(
    font: &Font,
    text: &str,
    font_px: f32,
    color: [f32; 4],
    pill: [f32; 4],
    border: &[[f32; 3]],
    border_px: f32,
) -> Option<(Vec<u8>, i32, i32)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let pad_x = TITLE_PILL_PAD_X.max(0);
    let pad_y = TITLE_PILL_PAD_Y.max(0);
    // Two-pass: measure, then draw. The text band is sized to the glyph extents; the
    // baseline sits at ~font size, and the whole thing is inset by the pill padding.
    let text_h = (font_px * 1.4).ceil() as i32;
    let baseline = pad_y + font_px.round() as i32;

    let mut pen_x = 0f32;
    let mut placements: Vec<(fontdue::Metrics, Vec<u8>, i32)> = Vec::new();
    for ch in text.chars().take(256) {
        let (metrics, bitmap) = font.rasterize(ch, font_px);
        placements.push((metrics, bitmap, pen_x.round() as i32));
        pen_x += metrics.advance_width;
    }
    let text_w = pen_x.ceil() as i32;
    if text_w <= 0 || text_h <= 0 {
        return None;
    }
    let width = (text_w + 2 * pad_x).min(4096);
    let height = (text_h + 2 * pad_y).min(256);
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // 1) Flat solid pill plate ringed by a thin border (premultiplied). Radius =
    //    half-height for a full capsule, capped so narrow pills stay sane. The outer
    //    edge is anti-aliased via the signed-distance coverage; pixels within
    //    `TITLE_PILL_BORDER_PX` of that edge take the border color, the interior the
    //    flat fill.
    let r = (height as f32 * 0.5).min(width as f32 * 0.5);
    let (pr, pg, pb, pa) = (pill[0], pill[1], pill[2], pill[3].clamp(0.0, 1.0));
    let bw = border_px.max(0.0);
    let wf = width as f32;
    let hf = height as f32;
    // Precompute the per-column border color so a horizontal gradient stroke costs a
    // single lerp per column rather than per pixel.
    let grad: Vec<[f32; 3]> = (0..width)
        .map(|x| {
            let t = if width > 1 {
                x as f32 / (width as f32 - 1.0)
            } else {
                0.0
            };
            sample_gradient(border, t)
        })
        .collect();
    for y in 0..height {
        let py = y as f32 + 0.5;
        for x in 0..width {
            let px = x as f32 + 0.5;
            // Depth inside the rounded shape (>= 0 inside, used for both the outer
            // anti-aliased coverage and the inner border ring).
            let depth = pill_depth(px, py, wf, hf, r);
            let cov = aa(depth);
            if cov <= 0.0 {
                continue;
            }
            // 1 within `bw` of the edge (the border ring), fading to 0 in the fill.
            let bf = aa(bw - depth);
            let bc = grad[x as usize];
            let rr = bc[0] * bf + pr * (1.0 - bf);
            let rg = bc[1] * bf + pg * (1.0 - bf);
            let rb = bc[2] * bf + pb * (1.0 - bf);
            let a = pa * cov;
            let idx = ((y * width + x) * 4) as usize;
            pixels[idx] = (rr.clamp(0.0, 1.0) * a * 255.0) as u8;
            pixels[idx + 1] = (rg.clamp(0.0, 1.0) * a * 255.0) as u8;
            pixels[idx + 2] = (rb.clamp(0.0, 1.0) * a * 255.0) as u8;
            pixels[idx + 3] = (a * 255.0) as u8;
        }
    }

    // 2) Text composited over the pill with source-over (premultiplied) blending.
    let (cr, cg, cb, ca) = (color[0], color[1], color[2], color[3]);
    for (metrics, bitmap, pen) in &placements {
        let gx = pad_x + pen + metrics.xmin;
        let gy = baseline - metrics.ymin - metrics.height as i32;
        for row in 0..metrics.height as i32 {
            let py = gy + row;
            if py < 0 || py >= height {
                continue;
            }
            for col in 0..metrics.width as i32 {
                let px = gx + col;
                if px < 0 || px >= width {
                    continue;
                }
                let sa = bitmap[(row * metrics.width as i32 + col) as usize] as f32 / 255.0 * ca;
                if sa <= 0.0 {
                    continue;
                }
                let idx = ((py * width + px) * 4) as usize;
                let inv = 1.0 - sa;
                let dr = pixels[idx] as f32 / 255.0;
                let dg = pixels[idx + 1] as f32 / 255.0;
                let db = pixels[idx + 2] as f32 / 255.0;
                let da = pixels[idx + 3] as f32 / 255.0;
                pixels[idx] = (((cr * sa + dr * inv).clamp(0.0, 1.0)) * 255.0) as u8;
                pixels[idx + 1] = (((cg * sa + dg * inv).clamp(0.0, 1.0)) * 255.0) as u8;
                pixels[idx + 2] = (((cb * sa + db * inv).clamp(0.0, 1.0)) * 255.0) as u8;
                pixels[idx + 3] = (((sa + da * inv).clamp(0.0, 1.0)) * 255.0) as u8;
            }
        }
    }
    Some((pixels, width, height))
}

/// Resolve focused-window border stops from a configured mode: `Accent` follows the
/// theme accent gradient, `Solid` is a single parsed color, and `Gradient` parses the
/// configured stops. Always returns at least one stop (falling back to the theme
/// accent / a primary cyan when a list is empty). Shared by the title pill and the
/// window frame border.
fn resolve_border_stops(
    mode: metis_config::BorderMode,
    color: &str,
    gradient: &[String],
    palette: &Palette,
) -> Vec<[f32; 3]> {
    let accent_fallback = || {
        if palette.accent.is_empty() {
            vec![[0.0, 0.95, 1.0]]
        } else {
            palette.accent.clone()
        }
    };
    match mode {
        metis_config::BorderMode::Accent => accent_fallback(),
        metis_config::BorderMode::Solid => vec![hex_rgb(color)],
        metis_config::BorderMode::Gradient => {
            let stops: Vec<[f32; 3]> = gradient.iter().map(|h| hex_rgb(h)).collect();
            if stops.is_empty() {
                accent_fallback()
            } else {
                stops
            }
        }
    }
}

/// Sample a gradient (`stops`) at parameter `t` in `[0, 1]`, linearly interpolating
/// between the two bracketing stops. A single stop yields a flat color.
fn sample_gradient(stops: &[[f32; 3]], t: f32) -> [f32; 3] {
    match stops.len() {
        0 => [0.0, 0.0, 0.0],
        1 => stops[0],
        n => {
            let scaled = t.clamp(0.0, 1.0) * (n as f32 - 1.0);
            let i = scaled.floor() as usize;
            if i >= n - 1 {
                return stops[n - 1];
            }
            let f = scaled - i as f32;
            [
                lerp(stops[i][0], stops[i + 1][0], f),
                lerp(stops[i][1], stops[i + 1][1], f),
                lerp(stops[i][2], stops[i + 1][2], f),
            ]
        }
    }
}

/// Opaque flat-plate color for the title pill: a dark well on dark titlebars, a
/// light-grey one on light titlebars, so the window name sits in a solid chip that
/// stays legible over a translucent titlebar. Returns alpha 1.0.
fn pill_base(rgb: [f32; 3]) -> [f32; 4] {
    let lum = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    if lum < 0.5 {
        [0.11, 0.11, 0.13, 1.0]
    } else {
        [0.82, 0.82, 0.84, 1.0]
    }
}

/// Depth (px) inside a rounded rectangle (corner radius `r`) at pixel center
/// `(px, py)` within a `w`×`h` box: positive inside (distance from the nearest edge),
/// negative outside. Standard rounded-box SDF, sign-flipped so larger = deeper in.
/// `aa(pill_depth(..))` gives the anti-aliased outer coverage.
fn pill_depth(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    let dx = (px - w * 0.5).abs() - (w * 0.5 - r);
    let dy = (py - h * 0.5).abs() - (h * 0.5 - r);
    let ax = dx.max(0.0);
    let ay = dy.max(0.0);
    let outside = (ax * ax + ay * ay).sqrt();
    let inside = dx.max(dy).min(0.0);
    r - (outside + inside)
}

/// Rasterize the titlebar background. A normal titlebar gets rounded TOP corners
/// (the bottom edge stays square where it meets the client) plus an opaque border
/// ring along the top / left / right edges, so the window's border visually wraps
/// around and continues *under* the titlebar instead of stopping at it. The
/// auto-hide reveal `overlay` uses the same top-corner rounding (no border ring)
/// so maximized/snap chrome matches windowed SSD, still floating over the client.
/// `color` is the straight titlebar RGB dimmed by `alpha`; `border` is the
/// (opaque) frame stroke, sampled as a top→bottom gradient over the full
/// `frame_height` (one stop = flat) so the ring lines up with the side/bottom
/// border gradient. Pixels outside the rounded corners are fully transparent.
/// Returns premultiplied RGBA at `TITLEBAR_SS`× scale.
#[allow(clippy::too_many_arguments)]
fn rasterize_titlebar(
    color: [f32; 3],
    alpha: f32,
    border: &[[f32; 3]],
    width: i32,
    header: i32,
    frame_height: i32,
    border_px: i32,
    overlay: bool,
) -> Option<(Vec<u8>, i32, i32)> {
    let ss = TITLEBAR_SS.max(1);
    let w = width * ss;
    let h = header * ss;
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut pixels = vec![0u8; (w * h * 4) as usize];

    let r = (CORNER_RADIUS_PX * ss) as f32;
    let a = alpha.clamp(0.0, 1.0);

    // Overlay: rounded top corners, flat fill, no border ring.
    if overlay {
        for y in 0..h {
            for x in 0..w {
                let outer = aa(edge_dist(x as f32 + 0.5, y as f32 + 0.5, w as f32, r));
                if outer <= 0.0 {
                    continue;
                }
                let pa = a * outer;
                let idx = ((y * w + x) * 4) as usize;
                pixels[idx] = (color[0] * pa * 255.0) as u8;
                pixels[idx + 1] = (color[1] * pa * 255.0) as u8;
                pixels[idx + 2] = (color[2] * pa * 255.0) as u8;
                pixels[idx + 3] = (pa * 255.0) as u8;
            }
        }
        return Some((pixels, w, h));
    }

    let bw = (border_px.max(0) * ss) as f32;
    let denom = (frame_height.max(1) * ss) as f32;
    for y in 0..h {
        // Border color at this row: sample the frame's top→bottom gradient. The
        // titlebar occupies the top `header` px of the frame, so t = y / frame_height.
        let bc = sample_gradient(border, y as f32 / denom);
        for x in 0..w {
            // Distance to the nearest bordered edge (top/left/right), following the
            // rounded top corners; bottom edge is open (meets the client body).
            let ed = edge_dist(x as f32 + 0.5, y as f32 + 0.5, w as f32, r);
            let outer = aa(ed);
            if outer <= 0.0 {
                continue;
            }
            // Opaque border within `bw` of the edge; translucent fill inside that.
            let bf = aa(bw - ed);
            let fill_a = alpha;
            let pa = (bf + fill_a * (1.0 - bf)) * outer;
            let blend = |fg: f32, bg: f32| (fg * bf + bg * fill_a * (1.0 - bf)) * outer;
            let idx = ((y * w + x) * 4) as usize;
            pixels[idx] = (blend(bc[0], color[0]) * 255.0) as u8;
            pixels[idx + 1] = (blend(bc[1], color[1]) * 255.0) as u8;
            pixels[idx + 2] = (blend(bc[2], color[2]) * 255.0) as u8;
            pixels[idx + 3] = (pa * 255.0) as u8;
        }
    }
    Some((pixels, w, h))
}

/// Rasterize the shared vertical-gradient side-border edge: a `b`×`height` opaque
/// premultiplied RGBA strip whose rows are colored by sampling `stops` at
/// `t = (header + row) / frame_height`, so the left/right edges continue the frame's
/// top→bottom gradient below the titlebar. A single stop yields a flat color.
fn rasterize_border_edge(
    stops: &[[f32; 3]],
    b: i32,
    height: i32,
    header: i32,
    frame_height: i32,
) -> Option<(Vec<u8>, i32, i32)> {
    if b <= 0 || height <= 0 || frame_height <= 0 {
        return None;
    }
    let mut pixels = vec![0u8; (b * height * 4) as usize];
    for row in 0..height {
        let t = (header + row) as f32 / frame_height as f32;
        let c = sample_gradient(stops, t);
        let (cr, cg, cb) = (
            (c[0].clamp(0.0, 1.0) * 255.0) as u8,
            (c[1].clamp(0.0, 1.0) * 255.0) as u8,
            (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        );
        for col in 0..b {
            let idx = ((row * b + col) * 4) as usize;
            pixels[idx] = cr;
            pixels[idx + 1] = cg;
            pixels[idx + 2] = cb;
            pixels[idx + 3] = 255;
        }
    }
    Some((pixels, b, height))
}

/// Signed distance (px, positive = inside) from the nearest *bordered* edge of the
/// titlebar — top, left and right — with the top corners rounded to radius `r`.
/// The bottom edge is intentionally excluded so no border is drawn where the
/// titlebar meets the client body. Values <= 0 are outside the rounded shape.
fn edge_dist(px: f32, py: f32, w: f32, r: f32) -> f32 {
    if r > 0.0 && py < r {
        if px < r {
            let d = ((px - r).powi(2) + (py - r).powi(2)).sqrt();
            return r - d;
        }
        if px > w - r {
            let d = ((px - (w - r)).powi(2) + (py - r).powi(2)).sqrt();
            return r - d;
        }
    }
    px.min(w - px).min(py)
}

/// Rasterize the shared drop-shadow texture: a black field that peaks at a small 2px
/// center and fades smoothly to zero across `SHADOW_MARGIN` px in every direction.
/// 9-slicing this with an `m`-wide border maps the center to the window edge (peak)
/// and the borders to the outward fade, so the ring sits entirely outside the frame.
/// Premultiplied RGBA (rgb is always 0; only the alpha channel carries the shape).
fn rasterize_shadow() -> Option<(Vec<u8>, i32, i32)> {
    let m = SHADOW_MARGIN;
    if m <= 0 {
        return None;
    }
    let side = 2 * m + 2;
    let mf = m as f32;
    let (cmin, cmax) = (m as f32, (m + 2) as f32);
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            // Distance from the central 2px peak rect (0 inside it).
            let dx = (cmin - px).max(px - cmax).max(0.0);
            let dy = (cmin - py).max(py - cmax).max(0.0);
            let d = (dx * dx + dy * dy).sqrt();
            let t = (d / mf).clamp(0.0, 1.0);
            let fall = 1.0 - t;
            let a = SHADOW_ALPHA * fall * fall;
            if a <= 0.0 {
                continue;
            }
            let av = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            let idx = ((y * side + x) * 4) as usize;
            // Premultiplied black: rgb stays 0, alpha carries the falloff.
            pixels[idx + 3] = av;
        }
    }
    Some((pixels, side, side))
}

/// Rasterize a rounded top-corner shadow tile (size `MARGIN + CORNER_RADIUS`). The
/// window silhouette in this tile is a rounded rectangle whose top-left corner is
/// rounded to `CORNER_RADIUS` (matching the titlebar); the alpha peaks along that
/// silhouette and fades to zero `MARGIN` px outward, so the shadow wraps the rounded
/// corner. `mirror_x` flips it horizontally for the top-right corner. Premultiplied
/// black (only the alpha channel carries the shape).
fn rasterize_shadow_corner(mirror_x: bool) -> Option<(Vec<u8>, i32, i32)> {
    let m = SHADOW_MARGIN;
    let r = CORNER_RADIUS_PX;
    let s2 = m + r;
    if s2 <= 0 || m <= 0 {
        return None;
    }
    let mf = m as f32;
    let rf = r as f32;
    // Frame edges sit at x=m, y=m; the rounded corner's arc is centered at (m+r,m+r).
    let (cx, cy) = ((m + r) as f32, (m + r) as f32);
    let mut pixels = vec![0u8; (s2 * s2 * 4) as usize];
    for y in 0..s2 {
        for x in 0..s2 {
            // Sample mirrored (write straight) to produce the right-corner variant.
            let sx = if mirror_x { s2 - 1 - x } else { x };
            let px = sx as f32 + 0.5;
            let py = y as f32 + 0.5;
            // Distance from the window silhouette: inside the arc/edges => <= 0.
            let outward = if px < cx && py < cy {
                let dxc = cx - px;
                let dyc = cy - py;
                (dxc * dxc + dyc * dyc).sqrt() - rf
            } else {
                (mf - px).max(mf - py).max(0.0)
            };
            // Inside the silhouette (outward < 0) draw NOTHING: that area sits under
            // the window's rounded corner, and painting shadow there shows through a
            // translucent titlebar as a wedge. The straight edges already keep all
            // shadow outside the frame; match that so the corner is consistent.
            let a = if outward <= 0.0 {
                0.0
            } else {
                let t = (outward / mf).clamp(0.0, 1.0);
                let f = 1.0 - t;
                SHADOW_ALPHA * f * f
            };
            if a <= 0.0 {
                continue;
            }
            let av = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            let idx = ((y * s2 + x) * 4) as usize;
            pixels[idx + 3] = av;
        }
    }
    Some((pixels, s2, s2))
}

/// Rasterize a control button: an anti-aliased filled circle (traffic-light color
/// when focused, gray when not) with a dark glyph (× close, + maximize, − minimize)
/// drawn only on focused buttons. Returns premultiplied RGBA at `BTN_SS`× scale.
fn rasterize_button(kind: DecoControl, focused: bool) -> Option<(Vec<u8>, i32, i32)> {
    let n = BTN_SIZE * BTN_SS;
    if n <= 0 {
        return None;
    }
    let circle = if focused {
        match kind {
            DecoControl::Close => BTN_CLOSE,
            DecoControl::Maximize => BTN_MAX,
            DecoControl::Minimize => BTN_MIN,
            DecoControl::Titlebar => return None,
        }
    } else {
        BTN_INACTIVE
    };

    let mut pixels = vec![0u8; (n * n * 4) as usize];
    let center = n as f32 / 2.0;
    let radius = center - BTN_SS as f32 * 0.5;
    let half_len = radius * 0.46;
    let half_thick = BTN_SS as f32 * 0.7;

    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let circle_cov = aa(radius - dist);
            if circle_cov <= 0.0 {
                continue;
            }

            let mut rgb = [circle[0], circle[1], circle[2]];
            if focused {
                let g = glyph_coverage(kind, dx, dy, half_len, half_thick);
                if g > 0.0 {
                    let ga = g * BTN_GLYPH[3];
                    rgb[0] = BTN_GLYPH[0] * ga + rgb[0] * (1.0 - ga);
                    rgb[1] = BTN_GLYPH[1] * ga + rgb[1] * (1.0 - ga);
                    rgb[2] = BTN_GLYPH[2] * ga + rgb[2] * (1.0 - ga);
                }
            }

            let a = circle_cov;
            let idx = ((y * n + x) * 4) as usize;
            pixels[idx] = (rgb[0] * a * 255.0) as u8;
            pixels[idx + 1] = (rgb[1] * a * 255.0) as u8;
            pixels[idx + 2] = (rgb[2] * a * 255.0) as u8;
            pixels[idx + 3] = (a * 255.0) as u8;
        }
    }
    Some((pixels, n, n))
}

/// Coverage (0–1) of a button glyph at offset `(dx, dy)` from the button center.
fn glyph_coverage(kind: DecoControl, dx: f32, dy: f32, len: f32, thick: f32) -> f32 {
    // A single bar running `along` an axis with thickness measured `across` it.
    let bar = |along: f32, across: f32| aa(thick - across.abs()) * aa(len - along.abs() + 0.5);
    match kind {
        DecoControl::Minimize => bar(dx, dy),
        DecoControl::Maximize => bar(dx, dy).max(bar(dy, dx)),
        DecoControl::Close => {
            // Rotate 45° so the two bars form an ×.
            let inv = std::f32::consts::FRAC_1_SQRT_2;
            let u = (dx + dy) * inv;
            let v = (dx - dy) * inv;
            bar(u, v).max(bar(v, u))
        }
        DecoControl::Titlebar => 0.0,
    }
}

/// 1px-wide anti-aliased edge: maps a signed distance (px) to coverage in [0, 1].
fn aa(edge: f32) -> f32 {
    (edge + 0.5).clamp(0.0, 1.0)
}

/// FNV-1a over the decoration-relevant state so we only re-damage on change.
fn signature(windows: &[WindowDeco]) -> u64 {
    let mut h = 1469598103934665603u64;
    let mut mix = |v: i64| {
        h ^= v as u64;
        h = h.wrapping_mul(1099511628211);
    };
    for w in windows {
        mix(w.id as i64);
        mix(w.frame.x as i64);
        mix(w.frame.y as i64);
        mix(w.frame.width as i64);
        mix(w.frame.height as i64);
        mix(w.focused as i64);
        mix(w.overlay as i64);
        mix(w.overlay_compact as i64);
        mix((w.overlay_reveal * 4096.0) as i64);
        for b in w.title.as_bytes() {
            mix(*b as i64);
        }
        mix(-1);
    }
    h
}

/// Load a UI font for titles: ask fontconfig, then fall back to common paths
/// (including CJK / Arabic Noto families for Phase 8 locale coverage).
pub(crate) fn load_font() -> Option<Font> {
    load_font_with_data().map(|(font, _)| font)
}

/// Like [`load_font`], but also returns the raw TTF/OTF bytes for rustybuzz.
pub(crate) fn load_font_with_data() -> Option<(Font, Vec<u8>)> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for query in ["sans", "Noto Sans", "Noto Sans CJK", "Noto Sans Arabic"] {
        if let Ok(out) = std::process::Command::new("fc-match")
            .args(["-f", "%{file}", query])
            .output()
            && out.status.success()
        {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                candidates.push(p.into());
            }
        }
    }
    candidates.extend(
        [
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ]
        .iter()
        .map(std::path::PathBuf::from),
    );

    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(font) = Font::from_bytes(bytes.clone(), fontdue::FontSettings::default())
        {
            tracing::info!(path = %path.display(), "decoration: loaded title font");
            return Some((font, bytes));
        }
    }
    tracing::warn!("decoration: no title font found; titles will be blank");
    None
}
