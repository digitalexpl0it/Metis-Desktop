//! Backdrop blur for the Metis edge bar.
//!
//! The bar reserves an exclusive zone, so the content directly behind it is the
//! wallpaper (windows reflow below the bar). We therefore implement the frosted
//! look by sampling the wallpaper texture under the bar's rectangle through a
//! Gaussian blur shader, drawn just beneath the bar surface. This avoids a true
//! framebuffer capture (and the transform/coordinate hazards that come with it),
//! while still giving a convincing translucent-blur bar over the wallpaper.
//!
//! Failure is always safe: if the wallpaper texture or shader program is
//! missing, the element simply isn't built and the bar renders normally.

use std::time::{Duration, Instant};

use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{
    Buffer, Physical, Point, Rectangle, Scale, Size, Transform, user_data::UserDataMap,
};

/// Custom texture shader: a 7x7 Gaussian sampled around each texel. The sampling
/// span scales with `blur_radius` (in texels). Mirrors Smithay's built-in
/// texture.frag header so the EXTERNAL/NO_ALPHA/DEBUG_FLAGS variants still link.
const BLUR_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;
uniform vec2 tex_size;
uniform float blur_radius;
// Stadium / rounded-rect mask: half short-axis for bar pill; 0 disables.
uniform float corner_radius;
// Dest (and sample-region) size in pixels for the SDF.
uniform vec2 rect_size;
// Top-left of the sample region in wallpaper texel space (src.loc).
uniform vec2 src_origin;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform float lift_linear;

float srgb_to_linear(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

/// Depth inside a rounded rect (positive inside). Matches CPU `pill_depth`.
float pill_depth(vec2 p, vec2 size, float r) {
    vec2 half_sz = size * 0.5;
    vec2 d = abs(p - half_sz) - (half_sz - vec2(r));
    float outside = length(max(d, 0.0));
    float inside = min(max(d.x, d.y), 0.0);
    return r - (outside + inside);
}

void main() {
    vec2 px = (blur_radius / 3.0) / tex_size;
    vec4 sum = vec4(0.0);
    float wsum = 0.0;
    for (int i = -3; i <= 3; i++) {
        for (int j = -3; j <= 3; j++) {
            float d2 = float(i * i + j * j);
            float w = exp(-d2 / 8.0);
            vec4 s = texture2D(tex, v_coords + vec2(float(i), float(j)) * px);
            if (lift_linear > 0.5) {
                s.rgb = vec3(
                    srgb_to_linear(s.r),
                    srgb_to_linear(s.g),
                    srgb_to_linear(s.b)
                );
            }
            sum += s * w;
            wsum += w;
        }
    }
    vec4 color = sum / wsum;

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

    // Soft stadium mask so frost follows the bar pill ends (CSS border-radius 999px).
    if (corner_radius > 0.5 && rect_size.x > 0.5 && rect_size.y > 0.5) {
        vec2 local = v_coords * tex_size - src_origin;
        float depth = pill_depth(local + vec2(0.5), rect_size, corner_radius);
        float cov = smoothstep(0.0, 1.0, depth);
        color *= cov;
    }

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Persistent blur configuration + GL resources, owned by `MetisState`.
pub struct BlurRuntime {
    pub enabled: bool,
    pub radius: f32,
    pub position: metis_config::BarPosition,
    id: Id,
    pub program: Option<GlesTexProgram>,
    commit: CommitCounter,
    last_sig: u64,
    last_check: Instant,
    /// Stable identity + commit for the full-screen lock-screen blur element, so
    /// its damage is tracked independently of the bar's backdrop blur.
    lock_id: Id,
    lock_commit: CommitCounter,
    last_lock_sig: u64,
}

impl Default for BlurRuntime {
    fn default() -> Self {
        let (enabled, radius, position) = read_bar_blur_config();
        Self {
            enabled,
            radius,
            position,
            id: Id::new(),
            program: None,
            commit: CommitCounter::default(),
            last_sig: 0,
            last_check: Instant::now(),
            lock_id: Id::new(),
            lock_commit: CommitCounter::default(),
            last_lock_sig: 0,
        }
    }
}

impl BlurRuntime {
    /// Throttled re-read of `bar.json` (~1s) so a Settings app toggling blur is
    /// picked up live. Returns `(changed, position_changed)`.
    pub fn maybe_refresh(&mut self) -> (bool, bool) {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return (false, false);
        }
        self.last_check = Instant::now();
        let (enabled, radius, position) = read_bar_blur_config();
        let position_changed = position != self.position;
        if enabled != self.enabled
            || (radius - self.radius).abs() > f32::EPSILON
            || position_changed
        {
            self.enabled = enabled;
            self.radius = radius;
            self.position = position;
            return (true, position_changed);
        }
        (false, false)
    }

    /// Shrink the bar's full layer rect down to just the visible pill, excluding
    /// the transparent drop-shadow padding baked into the surface, so the blur
    /// only frosts inside the bar (not the soft shadow around it).
    pub fn confine_to_pill(&self, rect: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
        let cfg = metis_config::load_bar_config();
        let pad = metis_config::bar::bar_layer_shadow_pad(&cfg);
        let side = metis_config::bar::bar_pill_side_inset(&cfg);
        let mut x = rect.loc.x;
        let mut y = rect.loc.y;
        let mut w = rect.size.w;
        let mut h = rect.size.h;
        match self.position {
            // Pill flush to the top; shadow pad sits below. Long edges: left/right.
            metis_config::BarPosition::Top => {
                h -= pad;
                x += side;
                w -= 2 * side;
            }
            // Pill flush to the bottom; shadow pad sits above. Long edges: left/right.
            metis_config::BarPosition::Bottom => {
                y += pad;
                h -= pad;
                x += side;
                w -= 2 * side;
            }
            // Pill flush to the left; shadow pad on the right. Long edges: top/bottom.
            metis_config::BarPosition::Left => {
                w -= pad;
                y += side;
                h -= 2 * side;
            }
            // Pill flush to the right; shadow pad on the left. Long edges: top/bottom.
            metis_config::BarPosition::Right => {
                x += pad;
                w -= pad;
                y += side;
                h -= 2 * side;
            }
        }
        Rectangle::new(Point::from((x, y)), Size::from((w.max(0), h.max(0))))
    }

    /// Lazily compile the blur shader using the renderer. Safe to call every
    /// frame; only compiles once.
    pub fn ensure_program(&mut self, renderer: &mut GlesRenderer) {
        if self.program.is_some() {
            return;
        }
        match renderer.compile_custom_texture_shader(
            BLUR_SHADER,
            &[
                UniformName::new("tex_size", UniformType::_2f),
                UniformName::new("blur_radius", UniformType::_1f),
                UniformName::new("lift_linear", UniformType::_1f),
                UniformName::new("corner_radius", UniformType::_1f),
                UniformName::new("rect_size", UniformType::_2f),
                UniformName::new("src_origin", UniformType::_2f),
            ],
        ) {
            Ok(program) => {
                tracing::info!("blur: compiled backdrop blur shader");
                self.program = Some(program);
            }
            Err(err) => tracing::warn!(?err, "blur: failed to compile shader; disabling"),
        }
    }

    /// Build a render element for the bar rect, or `None` if blur is unavailable.
    /// Bumps the commit counter when the rect/radius/texture changes so the
    /// damage tracker repaints only when needed.
    ///
    /// `rect` must already be [`Self::confine_to_pill`]'d. Applies a stadium
    /// corner mask (`min(w,h)/2`) so frost follows the GTK pill ends.
    pub fn element(
        &mut self,
        rect: Rectangle<i32, Physical>,
        texture: GlesTexture,
        tex_size: Size<i32, Buffer>,
        scene_linear: bool,
    ) -> Option<BlurElement> {
        if !self.enabled || rect.size.is_empty() || tex_size.is_empty() {
            return None;
        }
        let program = self.program.clone()?;
        let corner_radius = stadium_corner_radius(rect.size);

        let sig = signature(
            rect,
            self.radius,
            corner_radius,
            texture.tex_id(),
            tex_size,
            scene_linear,
        );
        if sig != self.last_sig {
            self.last_sig = sig;
            self.commit.increment();
        }

        let src = Rectangle::<f64, Buffer>::new(
            Point::from((rect.loc.x as f64, rect.loc.y as f64)),
            Size::from((rect.size.w as f64, rect.size.h as f64)),
        );

        Some(BlurElement {
            id: self.id.clone(),
            commit: self.commit,
            geometry: rect,
            src,
            texture,
            tex_w: tex_size.w as f32,
            tex_h: tex_size.h as f32,
            radius: self.radius,
            corner_radius,
            lift_linear: scene_linear,
            program,
        })
    }

    /// Hybrid MultiRenderer blur element sampling a [`MultiTexture`] wallpaper.
    /// Same stadium mask uniforms as [`Self::element`].
    pub fn hybrid_element(
        &mut self,
        rect: Rectangle<i32, Physical>,
        texture: smithay::backend::renderer::multigpu::MultiTexture,
        tex_size: Size<i32, Buffer>,
        scene_linear: bool,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        if !self.enabled || rect.size.is_empty() || tex_size.is_empty() {
            return None;
        }
        let program = self.program.clone()?;
        let corner_radius = stadium_corner_radius(rect.size);

        // MultiTexture has no GLES tex_id; hash size + geometry for damage.
        let sig = signature(rect, self.radius, corner_radius, 0, tex_size, scene_linear);
        if sig != self.last_sig {
            self.last_sig = sig;
            self.commit.increment();
        }

        let src = Rectangle::<f64, Buffer>::new(
            Point::from((rect.loc.x as f64, rect.loc.y as f64)),
            Size::from((rect.size.w as f64, rect.size.h as f64)),
        );

        Some(
            crate::hybrid_shader::HybridTexShaderElement::from_multi_texture(
                self.id.clone(),
                self.commit,
                rect,
                src,
                texture,
                program,
                blur_mask_uniforms(BlurMaskParams {
                    tex_w: tex_size.w as f32,
                    tex_h: tex_size.h as f32,
                    blur_radius: self.radius,
                    corner_radius,
                    rect_w: rect.size.w as f32,
                    rect_h: rect.size.h as f32,
                    src_x: rect.loc.x as f32,
                    src_y: rect.loc.y as f32,
                    scene_linear,
                }),
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            ),
        )
    }

    /// Build a full-target blur element for the compositor lock screen.
    ///
    /// Unlike [`Self::element`] (which conflates the bar's local rect with the
    /// texture-space region), the lock screen samples a *global* region of the
    /// desktop wallpaper texture: `geometry` is the render-target-local rectangle
    /// to fill, while `src` is the region of `texture` (in buffer coordinates) to
    /// sample — so a per-output DRM pass still reads that output's slice. Uses a
    /// stronger fixed `radius` than the bar for the frosted lock look. The shader
    /// program must already be compiled (via [`Self::ensure_program`]).
    pub fn lock_element(
        &mut self,
        geometry: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        texture: GlesTexture,
        tex_size: Size<i32, Buffer>,
        radius: f32,
    ) -> Option<BlurElement> {
        if geometry.size.is_empty() || tex_size.is_empty() {
            return None;
        }
        let program = self.program.clone()?;

        let sig = lock_signature(geometry, src, radius, texture.tex_id(), tex_size);
        if sig != self.last_lock_sig {
            self.last_lock_sig = sig;
            self.lock_commit.increment();
        }

        Some(BlurElement {
            id: self.lock_id.clone(),
            commit: self.lock_commit,
            geometry,
            src,
            texture,
            tex_w: tex_size.w as f32,
            tex_h: tex_size.h as f32,
            radius,
            // Full-output frost: no stadium mask.
            corner_radius: 0.0,
            lift_linear: false,
            program,
        })
    }
}

/// Half the short axis — matches shell CSS `border-radius: 999px` stadium pill.
fn stadium_corner_radius(size: Size<i32, Physical>) -> f32 {
    0.5 * (size.w.min(size.h).max(0) as f32)
}

struct BlurMaskParams {
    tex_w: f32,
    tex_h: f32,
    blur_radius: f32,
    corner_radius: f32,
    rect_w: f32,
    rect_h: f32,
    src_x: f32,
    src_y: f32,
    scene_linear: bool,
}

fn blur_mask_uniforms(p: BlurMaskParams) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("tex_size", [p.tex_w, p.tex_h]),
        Uniform::new("blur_radius", p.blur_radius),
        Uniform::new("lift_linear", if p.scene_linear { 1.0f32 } else { 0.0 }),
        Uniform::new("corner_radius", p.corner_radius),
        Uniform::new("rect_size", [p.rect_w, p.rect_h]),
        Uniform::new("src_origin", [p.src_x, p.src_y]),
    ]
}

fn lock_signature(
    geo: Rectangle<i32, Physical>,
    src: Rectangle<f64, Buffer>,
    radius: f32,
    tex_id: u32,
    tex: Size<i32, Buffer>,
) -> u64 {
    let mut h = 1469598103934665603u64;
    let mut mix = |v: i64| {
        h ^= v as u64;
        h = h.wrapping_mul(1099511628211);
    };
    mix(geo.loc.x as i64);
    mix(geo.loc.y as i64);
    mix(geo.size.w as i64);
    mix(geo.size.h as i64);
    mix(src.loc.x as i64);
    mix(src.loc.y as i64);
    mix(src.size.w as i64);
    mix(src.size.h as i64);
    mix(radius.to_bits() as i64);
    mix(tex_id as i64);
    mix(tex.w as i64);
    mix(tex.h as i64);
    h
}

fn signature(
    rect: Rectangle<i32, Physical>,
    radius: f32,
    corner_radius: f32,
    tex_id: u32,
    tex: Size<i32, Buffer>,
    scene_linear: bool,
) -> u64 {
    let mut h = 1469598103934665603u64;
    let mut mix = |v: i64| {
        h ^= v as u64;
        h = h.wrapping_mul(1099511628211);
    };
    mix(rect.loc.x as i64);
    mix(rect.loc.y as i64);
    mix(rect.size.w as i64);
    mix(rect.size.h as i64);
    mix(radius.to_bits() as i64);
    mix(corner_radius.to_bits() as i64);
    mix(tex_id as i64);
    mix(tex.w as i64);
    mix(tex.h as i64);
    mix(i64::from(scene_linear));
    h
}

/// A blurred copy of the wallpaper under the bar, drawn beneath the bar surface.
#[derive(Debug)]
pub struct BlurElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    src: Rectangle<f64, Buffer>,
    texture: GlesTexture,
    tex_w: f32,
    tex_h: f32,
    radius: f32,
    corner_radius: f32,
    lift_linear: bool,
    program: GlesTexProgram,
}

impl Element for BlurElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }
}

impl RenderElement<GlesRenderer> for BlurElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let uniforms = blur_mask_uniforms(BlurMaskParams {
            tex_w: self.tex_w,
            tex_h: self.tex_h,
            blur_radius: self.radius,
            corner_radius: self.corner_radius,
            rect_w: self.geometry.size.w as f32,
            rect_h: self.geometry.size.h as f32,
            src_x: self.src.loc.x as f32,
            src_y: self.src.loc.y as f32,
            scene_linear: self.lift_linear,
        });
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
            Some(&self.program),
            &uniforms,
        )
    }
}

/// Read the bar's blur settings (and position) from `~/.config/metis/bar.json`
/// via `metis-config`, which supplies the shell's defaults when fields/file are
/// missing. Radius is clamped to the shader's usable range.
fn read_bar_blur_config() -> (bool, f32, metis_config::BarPosition) {
    let cfg = metis_config::load_bar_config();
    (cfg.blur, cfg.blur_radius.clamp(1.0, 64.0), cfg.position)
}
