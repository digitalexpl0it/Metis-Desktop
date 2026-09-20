use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::imageops::{FilterType, Triangle};
use image::{Rgba, RgbaImage};
use metis_config::{BackgroundKind, GradientDirection};
use smithay::backend::{
    allocator::Fourcc,
    renderer::{
        element::{
            texture::{TextureBuffer, TextureRenderElement},
            Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        ImportMem, Texture,
    },
};
use smithay::utils::{Physical, Point, Size, Transform};

/// Result handed back from the decode worker thread.
struct DecodeOutput {
    /// One full-framebuffer RGBA buffer (`full_size`) composed of every output's
    /// own cover-cropped image, each blitted at its global origin.
    pixels: Vec<u8>,
    /// Sources the worker loaded from disk this pass, keyed by path, so the main
    /// thread can cache them and skip re-reading on later (resize) decodes.
    sources: HashMap<PathBuf, Arc<RgbaImage>>,
}

/// How the desktop background is produced. `Image` reads `path`; the others are
/// generated procedurally at the output resolution.
#[derive(Clone, PartialEq)]
enum BackgroundMode {
    Image,
    Solid([u8; 3]),
    Gradient {
        a: [u8; 3],
        b: [u8; 3],
        dir: GradientDirection,
    },
}

/// One logical output's slot in the full framebuffer: where it sits (global
/// physical origin) and how big it is. The wallpaper is composed by cropping
/// each output's image to `size` and blitting at `origin`, so every monitor is
/// cover-filled independently rather than one image stretched across them all.
#[derive(Clone, PartialEq)]
pub struct OutputRegion {
    pub name: String,
    pub origin: Point<i32, Physical>,
    pub size: Size<i32, Physical>,
}

/// Crossfade length when Settings (or IPC) swaps image / solid / gradient.
const WALLPAPER_FADE_MS: u64 = 280;

pub struct Wallpaper {
    /// Background used for any output without a per-output override.
    default_mode: BackgroundMode,
    default_path: PathBuf,
    /// Per-output image overrides (output name → image path), from `wallpaper.json`.
    overrides: HashMap<String, PathBuf>,
    /// Current output layout the framebuffer is composed for.
    regions: Vec<OutputRegion>,
    /// Size of the whole virtual desktop (all outputs) in physical pixels.
    full_size: Size<i32, Physical>,
    buffer: Option<TextureBuffer<GlesTexture>>,
    /// The raw texture backing `buffer`, kept so the bar's backdrop blur can
    /// sample the wallpaper region behind the bar (TextureBuffer hides it).
    texture: Option<GlesTexture>,
    /// Previous GPU wallpaper kept on screen while the next compose uploads, then
    /// crossfaded out. Set only for config swaps (`apply_config`), not layout
    /// resize (which snaps once the new buffer is ready).
    outgoing: Option<TextureBuffer<GlesTexture>>,
    /// When the incoming texture finished uploading and the fade started.
    fade_start: Option<Instant>,
    /// Next successful decode should stash the current buffer into `outgoing`
    /// and fade rather than dropping it (Settings background changes).
    fade_on_next_upload: bool,
    /// Decoded RGBA pixels (CPU) ready for a fast GPU upload during render.
    cpu_pixels: Option<Vec<u8>>,
    /// Full-resolution sources kept in memory (keyed by path) so resizes only
    /// re-scale (cheap) instead of re-reading and re-decoding from disk. Shared
    /// across outputs, so two displays showing the same image only read it once.
    sources: HashMap<PathBuf, Arc<RgbaImage>>,
    /// `(generation, result)` — the generation tag lets us ignore stale worker
    /// output after a layout invalidation without dropping the slot mid-flight.
    decode_slot: Arc<Mutex<(u64, Option<DecodeOutput>)>>,
    decode_generation: u64,
    decode_thread: Option<JoinHandle<()>>,
    /// When set, a (re)decode is due once this instant passes — debounces the
    /// burst of resize events emitted while maximizing/restoring the window.
    redecode_at: Option<Instant>,
}

/// True only when `METIS_NO_WALLPAPER` is set to a non-empty value. An explicit
/// empty value (`METIS_NO_WALLPAPER=`) means "enable wallpaper".
fn wallpaper_disabled() -> bool {
    std::env::var("METIS_NO_WALLPAPER")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

impl Wallpaper {
    pub fn new() -> Self {
        let (default_mode, default_path, overrides) = resolve_config();
        if default_path.is_file() {
            tracing::info!(path = %default_path.display(), "wallpaper configured");
        }
        Self {
            default_mode,
            default_path,
            overrides,
            regions: Vec::new(),
            full_size: Size::from((0, 0)),
            buffer: None,
            texture: None,
            outgoing: None,
            fade_start: None,
            fade_on_next_upload: false,
            cpu_pixels: None,
            sources: HashMap::new(),
            decode_slot: Arc::new(Mutex::new((0, None))),
            decode_generation: 0,
            decode_thread: None,
            redecode_at: None,
        }
    }

    pub fn enabled(&self) -> bool {
        if wallpaper_disabled() {
            return false;
        }
        let default_ok = match self.default_mode {
            BackgroundMode::Image => self.default_path.is_file(),
            BackgroundMode::Solid(_) | BackgroundMode::Gradient { .. } => true,
        };
        // Per-output overrides can carry the desktop on their own even if the
        // global image is missing, so honour those too.
        default_ok || !self.overrides.is_empty()
    }

    /// Resolve a single output's background: its per-output override if present,
    /// otherwise the global default.
    fn region_background(&self, name: &str) -> (BackgroundMode, PathBuf) {
        if let Some(path) = self.overrides.get(name) {
            return (BackgroundMode::Image, path.clone());
        }
        (self.default_mode.clone(), self.default_path.clone())
    }

    /// Re-read `wallpaper.json` and switch the background at runtime. Keeps the
    /// current GPU texture on screen while the next image/solid/gradient composes,
    /// then crossfades — clearing here caused a multi-second black flash in Settings.
    pub fn apply_config(&mut self) {
        let (mode, path, overrides) = resolve_config();
        if mode == self.default_mode && path == self.default_path && overrides == self.overrides {
            return;
        }
        tracing::info!("wallpaper: applying new background config");
        self.default_mode = mode;
        self.default_path = path;
        self.overrides = overrides;
        // Keep decoded sources that are still referenced so switching back (or
        // re-applying the same picture) skips the multi‑MB PNG decode.
        let mut keep = HashSet::new();
        if matches!(self.default_mode, BackgroundMode::Image) {
            keep.insert(self.default_path.clone());
        }
        for p in self.overrides.values() {
            keep.insert(p.clone());
        }
        self.sources.retain(|p, _| keep.contains(p));
        // Soft invalidate + request a fade when the new framebuffer uploads.
        self.decode_generation = self.decode_generation.wrapping_add(1);
        self.cpu_pixels = None;
        self.fade_on_next_upload = true;
        // Drop any in-progress fade so we always blend from the currently shown
        // wallpaper into the newly requested one.
        self.outgoing = None;
        self.fade_start = None;
    }

    pub fn invalidate(&mut self) {
        self.buffer = None;
        self.texture = None;
        self.outgoing = None;
        self.fade_start = None;
        self.fade_on_next_upload = false;
        self.cpu_pixels = None;
        // Bump the generation so any in-flight worker's result is ignored on poll.
        // Never drop `decode_slot` or join here — that orphaned workers and/or
        // blocked the main loop for the full decode on every resize burst.
        self.decode_generation = self.decode_generation.wrapping_add(1);
    }

    /// Drop only context-bound GL objects while retaining the composed CPU image.
    /// Used when DRM rendering switches to another GPU context.
    pub fn invalidate_gpu_cache(&mut self) {
        self.buffer = None;
        self.texture = None;
        self.outgoing = None;
        self.fade_start = None;
    }

    /// Set the output layout (full framebuffer size + per-output regions) the
    /// wallpaper composes for. Schedules a debounced re-decode when it changes,
    /// collapsing the burst of resize/hotplug events into a single decode.
    ///
    /// Keeps the previous GPU texture on screen until the new compose uploads —
    /// clearing it here caused a visible black flash on every HDMI plug.
    pub fn set_layout(&mut self, full_size: Size<i32, Physical>, regions: Vec<OutputRegion>) {
        if self.full_size == full_size && self.regions == regions {
            return;
        }
        self.full_size = full_size;
        self.regions = regions;
        // Soft invalidate: drop CPU pixels so a new decode is scheduled, but
        // leave `buffer` / `texture` so the last frame keeps painting.
        self.decode_generation = self.decode_generation.wrapping_add(1);
        self.cpu_pixels = None;
        let at = Instant::now() + Duration::from_millis(120);
        self.redecode_at = Some(self.redecode_at.map_or(at, |prev| prev.min(at)));
    }

    /// Drive debounced decoding, result polling, and wallpaper crossfade from the
    /// compositor heartbeat. Returns true while the wallpaper still needs a frame
    /// rendered (decode pending/running, pixels awaiting GPU upload, or fade).
    pub fn tick_decode(&mut self) -> bool {
        if !self.enabled() {
            self.redecode_at = None;
            self.outgoing = None;
            self.fade_start = None;
            return false;
        }
        if let Some(at) = self.redecode_at {
            if Instant::now() >= at {
                // start_async_decode clears redecode_at on success or re-arms it
                // for a short retry while a previous worker is still composing —
                // never drop the schedule here, or the wallpaper never lands.
                self.start_async_decode();
            }
        }
        self.poll_decode();
        self.tick_fade();
        // Only schedule frames when a re-decode is queued, decoded pixels still
        // need a GPU upload, or a crossfade is in progress. Polling while a
        // worker is running caused a 60fps render spin that blocked the nested
        // compositor during startup.
        self.redecode_at.is_some()
            || (self.cpu_pixels.is_some() && self.buffer.is_none())
            || self.fade_start.is_some()
    }

    fn tick_fade(&mut self) {
        let Some(start) = self.fade_start else {
            return;
        };
        if start.elapsed() >= Duration::from_millis(WALLPAPER_FADE_MS) {
            self.outgoing = None;
            self.fade_start = None;
        }
    }

    fn fade_alpha(&self) -> Option<f32> {
        let start = self.fade_start?;
        let t = (start.elapsed().as_secs_f32()
            / Duration::from_millis(WALLPAPER_FADE_MS).as_secs_f32())
        .clamp(0.0, 1.0);
        // Ease-out quad — snappy start, soft settle.
        Some(1.0 - (1.0 - t) * (1.0 - t))
    }

    /// Compose the full-desktop wallpaper on a background thread (one cover-crop
    /// per output, blitted into the shared framebuffer) so init/resize stay
    /// responsive.
    pub fn start_async_decode(&mut self) {
        if !self.enabled() || self.cpu_pixels.is_some() {
            self.redecode_at = None;
            return;
        }
        if let Some(handle) = self.decode_thread.as_ref() {
            if !handle.is_finished() {
                // A worker is still composing the previous layout. Retry shortly
                // rather than dropping the request, so the latest layout still
                // gets decoded once the worker frees up.
                self.redecode_at = Some(Instant::now() + Duration::from_millis(30));
                return;
            }
        }
        if let Some(handle) = self.decode_thread.take() {
            let _ = handle.join();
        }
        if self.full_size.w <= 0 || self.full_size.h <= 0 || self.regions.is_empty() {
            self.redecode_at = None;
            return;
        }
        self.redecode_at = None;

        let full = self.full_size;
        let generation = self.decode_generation;
        // Resolve every region's (mode, path) up front so the worker is fully
        // self-contained and never touches `self`.
        let jobs: Vec<(OutputRegion, BackgroundMode, PathBuf)> = self
            .regions
            .iter()
            .map(|r| {
                let (mode, path) = self.region_background(&r.name);
                (r.clone(), mode, path)
            })
            .collect();
        let cached = self.sources.clone();
        let slot = Arc::clone(&self.decode_slot);
        let slot_worker = Arc::clone(&slot);

        tracing::debug!(
            width = full.w,
            height = full.h,
            outputs = jobs.len(),
            "composing wallpaper"
        );
        let handle = std::thread::Builder::new()
            .name("metis-wallpaper-decode".into())
            .spawn(move || {
                let fw = full.w.max(0) as usize;
                let fh = full.h.max(0) as usize;
                let mut buf = vec![0u8; fw * fh * 4];
                let mut new_sources: HashMap<PathBuf, Arc<RgbaImage>> = HashMap::new();

                for (region, mode, path) in jobs {
                    let rw = region.size.w.max(0) as u32;
                    let rh = region.size.h.max(0) as u32;
                    if rw == 0 || rh == 0 {
                        continue;
                    }
                    let pixels = match mode {
                        BackgroundMode::Image => {
                            let source = cached
                                .get(&path)
                                .or_else(|| new_sources.get(&path))
                                .cloned()
                                .or_else(|| {
                                    load_image_cached(&path).map(|arc| {
                                        new_sources.insert(path.clone(), arc.clone());
                                        arc
                                    })
                                });
                            match source {
                                Some(src) => cover_crop_rgba(&src, rw, rh),
                                None => vec![0u8; (rw as usize) * (rh as usize) * 4],
                            }
                        }
                        BackgroundMode::Solid(rgb) => gen_solid(rgb, rw, rh).into_raw(),
                        BackgroundMode::Gradient { a, b, dir } => {
                            gen_gradient(a, b, dir, rw, rh).into_raw()
                        }
                    };
                    blit(
                        &mut buf,
                        fw,
                        fh,
                        region.origin.x,
                        region.origin.y,
                        &pixels,
                        rw as usize,
                        rh as usize,
                    );
                }

                if let Ok(mut guard) = slot_worker.lock() {
                    if guard.0 == generation {
                        guard.1 = Some(DecodeOutput {
                            pixels: buf,
                            sources: new_sources,
                        });
                    }
                }
            })
            .ok();

        if let Some(handle) = handle {
            if let Ok(mut guard) = self.decode_slot.lock() {
                guard.0 = generation;
                guard.1 = None;
            }
            self.decode_thread = Some(handle);
        }
    }

    /// Pull the composed framebuffer from the worker thread when ready.
    pub fn poll_decode(&mut self) {
        if self.cpu_pixels.is_some() {
            return;
        }
        if let Ok(mut guard) = self.decode_slot.lock() {
            if guard.0 == self.decode_generation {
                if let Some(out) = guard.1.take() {
                    tracing::info!(
                        width = self.full_size.w,
                        height = self.full_size.h,
                        "wallpaper composed"
                    );
                    for (path, src) in out.sources {
                        self.sources.insert(path, src);
                    }
                    self.cpu_pixels = Some(out.pixels);
                    if self.fade_on_next_upload {
                        // Keep painting the previous wallpaper until the new
                        // texture uploads, then crossfade.
                        if let Some(prev) = self.buffer.take() {
                            self.outgoing = Some(prev);
                        }
                        self.texture = None;
                        self.fade_on_next_upload = false;
                        self.fade_start = None;
                    } else {
                        // Layout / hotplug: snap once the new buffer is ready
                        // (previous soft-invalidate already kept it until now).
                        self.buffer = None;
                        self.texture = None;
                        self.outgoing = None;
                        self.fade_start = None;
                    }
                }
            }
        }
        if let Some(handle) = self.decode_thread.take() {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                self.decode_thread = Some(handle);
            }
        }
    }

    pub fn ensure(&mut self, renderer: &mut GlesRenderer) {
        if !self.enabled() || self.buffer.is_some() {
            return;
        }
        if self.full_size.w <= 0 || self.full_size.h <= 0 {
            return;
        }

        self.poll_decode();

        let Some(rgba) = self.cpu_pixels.as_ref() else {
            return;
        };

        let w = self.full_size.w;
        let h = self.full_size.h;

        // Import the texture explicitly (rather than letting TextureBuffer own it)
        // so we can also hand the GlesTexture to the bar backdrop-blur element.
        match renderer.import_memory(rgba, Fourcc::Abgr8888, (w, h).into(), false) {
            Ok(texture) => {
                let buf = TextureBuffer::from_texture(
                    renderer,
                    texture.clone(),
                    1,
                    Transform::Normal,
                    None,
                );
                tracing::info!(width = w, height = h, "wallpaper ready");
                self.texture = Some(texture);
                self.buffer = Some(buf);
                if self.outgoing.is_some() {
                    self.fade_start = Some(Instant::now());
                }
            }
            Err(err) => tracing::warn!(?err, "failed to upload wallpaper texture"),
        }
    }

    /// The wallpaper texture and its size, for sampling behind the bar (blur).
    pub fn texture(&self) -> Option<(GlesTexture, Size<i32, smithay::utils::Buffer>)> {
        let texture = self.texture.as_ref()?;
        Some((texture.clone(), texture.size()))
    }

    /// Borrow the uploaded wallpaper buffer (Task View workspace thumbs).
    pub fn buffer_ref(&self) -> Option<&TextureBuffer<GlesTexture>> {
        self.buffer.as_ref()
    }

    pub fn render_element(&self) -> Option<TextureRenderElement<GlesTexture>> {
        self.render_element_at(Point::from((0.0, 0.0)))
    }

    /// Wallpaper element with its top-left placed at `loc` (physical). The
    /// texture spans the whole virtual desktop, so the DRM backend offsets it by
    /// the negative of each output's origin to slice the per-output framebuffer.
    ///
    /// Prefer [`render_elements_at`] when a config crossfade may be active.
    pub fn render_element_at(
        &self,
        loc: Point<f64, Physical>,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        self.render_elements_at(loc).into_iter().next()
    }

    /// Zero, one, or two wallpaper elements for `loc`. During a Settings
    /// background crossfade this returns the fading-in texture first (on top)
    /// and the previous wallpaper beneath it. Higher stack index = further back
    /// in the scene (`render.rs` pushes later = more background).
    pub fn render_elements_at(
        &self,
        loc: Point<f64, Physical>,
    ) -> Vec<TextureRenderElement<GlesTexture>> {
        let mut elems = Vec::with_capacity(2);
        let fade = self.fade_alpha();
        if let Some(buffer) = self.buffer.as_ref() {
            let alpha = if self.outgoing.is_some() {
                fade.or(Some(0.0))
            } else {
                None
            };
            elems.push(TextureRenderElement::from_texture_buffer(
                loc,
                buffer,
                alpha,
                None,
                None,
                Kind::Unspecified,
            ));
        }
        if let Some(prev) = self.outgoing.as_ref() {
            // Keep the previous wallpaper fully opaque under the fade-in.
            elems.push(TextureRenderElement::from_texture_buffer(
                loc,
                prev,
                None,
                None,
                None,
                Kind::Unspecified,
            ));
        } else if elems.is_empty() {
            // Nothing uploaded yet — callers fall back to the desktop underlay.
        }
        elems
    }

    /// True while a background decode is running and the texture is not uploaded yet.
    pub fn decode_in_flight(&self) -> bool {
        self.enabled()
            && self.buffer.is_none()
            && self.outgoing.is_none()
            && (self.decode_thread.is_some() || self.redecode_at.is_some())
    }
}

fn load_image_cached(path: &std::path::Path) -> Option<Arc<RgbaImage>> {
    if let Some((w, h, rgba)) = metis_config::load_wallpaper_rgba_cache(path) {
        if let Some(img) = RgbaImage::from_raw(w, h, rgba) {
            return Some(Arc::new(img));
        }
    }
    match image::open(path) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            if let Err(err) =
                metis_config::store_wallpaper_rgba_cache(path, rgba.width(), rgba.height(), &rgba)
            {
                tracing::debug!(path = %path.display(), %err, "wallpaper rgba cache write failed");
            }
            Some(Arc::new(rgba))
        }
        Err(_) => {
            tracing::warn!(path = %path.display(), "failed to open wallpaper");
            None
        }
    }
}

fn cover_crop_rgba(rgba: &image::RgbaImage, out_w: u32, out_h: u32) -> Vec<u8> {
    cover_crop_rgba_filter(rgba, out_w, out_h, Triangle)
}

/// Cover-crop: crop the source to the output aspect first, then resize once.
/// (The previous path resized the whole image up/down then cropped — much heavier
/// for ultrawide / mismatched aspects.)
fn cover_crop_rgba_filter(
    rgba: &image::RgbaImage,
    out_w: u32,
    out_h: u32,
    filter: FilterType,
) -> Vec<u8> {
    let (iw, ih) = (rgba.width(), rgba.height());
    if iw == 0 || ih == 0 || out_w == 0 || out_h == 0 {
        return vec![0; (out_w as usize) * (out_h as usize) * 4];
    }
    if iw == out_w && ih == out_h {
        return rgba.as_raw().clone();
    }

    let src_aspect = iw as f32 / ih as f32;
    let dst_aspect = out_w as f32 / out_h as f32;
    let (cx, cy, cw, ch) = if src_aspect > dst_aspect {
        // Source wider than output — crop sides.
        let cw = ((ih as f32 * dst_aspect).round() as u32).clamp(1, iw);
        let cx = (iw - cw) / 2;
        (cx, 0, cw, ih)
    } else {
        // Source taller — crop top/bottom.
        let ch = ((iw as f32 / dst_aspect).round() as u32).clamp(1, ih);
        let cy = (ih - ch) / 2;
        (0, cy, iw, ch)
    };

    let cropped = if cx == 0 && cy == 0 && cw == iw && ch == ih {
        None
    } else {
        Some(image::imageops::crop_imm(rgba, cx, cy, cw, ch).to_image())
    };
    let src = cropped.as_ref().unwrap_or(rgba);
    if src.width() == out_w && src.height() == out_h {
        return src.as_raw().clone();
    }
    image::imageops::resize(src, out_w, out_h, filter).into_raw()
}

/// Copy a `src_w × src_h` RGBA block into `dst` (a `dst_w × dst_h` RGBA buffer)
/// at `(ox, oy)`, clipping to the destination bounds. Copies row by row so a
/// region near the edge of the framebuffer is partially blitted rather than
/// skipped. Within Metis's tiled layout every region lands fully in-bounds.
#[allow(clippy::too_many_arguments)]
fn blit(
    dst: &mut [u8],
    dst_w: usize,
    dst_h: usize,
    ox: i32,
    oy: i32,
    src: &[u8],
    src_w: usize,
    src_h: usize,
) {
    if src_w == 0 || src_h == 0 {
        return;
    }
    let x0 = ox.max(0);
    let x1 = (ox + src_w as i32).min(dst_w as i32);
    if x1 <= x0 {
        return;
    }
    let copy_w = (x1 - x0) as usize;
    let src_col0 = (x0 - ox) as usize;
    for row in 0..src_h {
        let dy = oy + row as i32;
        if dy < 0 || dy as usize >= dst_h {
            continue;
        }
        let s = (row * src_w + src_col0) * 4;
        let d = (dy as usize * dst_w + x0 as usize) * 4;
        if s + copy_w * 4 <= src.len() && d + copy_w * 4 <= dst.len() {
            dst[d..d + copy_w * 4].copy_from_slice(&src[s..s + copy_w * 4]);
        }
    }
}

/// Resolve the full wallpaper config: the global default background plus the set
/// of valid per-output image overrides (entries whose file is missing are
/// dropped, so a stale path silently falls back to the global background).
fn resolve_config() -> (BackgroundMode, PathBuf, HashMap<String, PathBuf>) {
    let (mode, path) = resolve_background();
    let overrides = metis_config::load_wallpaper_config()
        .per_output
        .into_iter()
        .filter_map(|(name, raw)| {
            let p = PathBuf::from(raw);
            p.is_file().then_some((name, p))
        })
        .collect();
    (mode, path, overrides)
}

/// Build the active background mode (and image path, if any) from `wallpaper.json`.
fn resolve_background() -> (BackgroundMode, PathBuf) {
    let cfg = metis_config::load_wallpaper_config();
    match cfg.kind {
        BackgroundKind::Image => (BackgroundMode::Image, resolve_path().unwrap_or_default()),
        BackgroundKind::Solid => (
            BackgroundMode::Solid(metis_config::parse_hex_rgb(&cfg.color)),
            PathBuf::new(),
        ),
        BackgroundKind::Gradient => (
            BackgroundMode::Gradient {
                a: metis_config::parse_hex_rgb(&cfg.gradient_start),
                b: metis_config::parse_hex_rgb(&cfg.gradient_end),
                dir: cfg.gradient_direction,
            },
            PathBuf::new(),
        ),
    }
}

fn gen_solid(rgb: [u8; 3], w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w.max(1), h.max(1), Rgba([rgb[0], rgb[1], rgb[2], 255]))
}

fn gen_gradient(a: [u8; 3], b: [u8; 3], dir: GradientDirection, w: u32, h: u32) -> RgbaImage {
    let w = w.max(1);
    let h = h.max(1);
    let mut img = RgbaImage::new(w, h);
    let wf = (w.saturating_sub(1)).max(1) as f32;
    let hf = (h.saturating_sub(1)).max(1) as f32;
    let lerp = |c0: u8, c1: u8, t: f32| (c0 as f32 + (c1 as f32 - c0 as f32) * t).round() as u8;
    for y in 0..h {
        let yt = y as f32 / hf;
        for x in 0..w {
            let xt = x as f32 / wf;
            let t = match dir {
                GradientDirection::Vertical => yt,
                GradientDirection::VerticalReverse => 1.0 - yt,
                GradientDirection::Horizontal => xt,
                GradientDirection::HorizontalReverse => 1.0 - xt,
                GradientDirection::Diagonal => (xt + yt) * 0.5,
                GradientDirection::DiagonalReverse => ((1.0 - xt) + yt) * 0.5,
            }
            .clamp(0.0, 1.0);
            img.put_pixel(
                x,
                y,
                Rgba([
                    lerp(a[0], b[0], t),
                    lerp(a[1], b[1], t),
                    lerp(a[2], b[2], t),
                    255,
                ]),
            );
        }
    }
    img
}

pub fn resolve_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("METIS_WALLPAPER") {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Some(path);
        }
        tracing::warn!(path = %path.display(), "METIS_WALLPAPER is not a file");
    }

    // User's explicit selection from the settings app (wallpaper.json).
    if let Some(path) = metis_config::load_wallpaper_config().path {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(dirs) = directories::ProjectDirs::from("com", "metis", "metis") {
        for name in ["wallpaper.jpg", "wallpaper.png", "wallpaper.webp"] {
            let path = dirs.config_dir().join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    metis_config::default_wallpaper_path()
}
