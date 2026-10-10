//! HDR encode at scanout — SDR desktop → PQ or HLG — and per-surface decode.
//!
//! When an output has HDR signaling active, the DRM path composites the usual
//! sRGB desktop into an offscreen buffer, then blits it through a texture
//! shader that:
//!   1. decodes sRGB → linear light
//!   2. converts Rec.709 / sRGB primaries → BT.2020
//!   3. scales by a reference-white luminance (BT.2408: 203 nits)
//!   4. encodes either SMPTE ST 2084 (PQ) or HLG (ARIB STD-B67)
//!
//! so the panel (already in HDR mode) displays SDR content at a sensible
//! brightness. PQ is preferred when EDID advertises ST.2084; HLG is used for
//! HLG-only panels.
//!
//! Mixed SDR+HDR (urgent #2):
//! - On an HDR output with float/10-bit intermediates: decode HDR windows to
//!   Rec.709 **scene-linear** (1.0 = 203 nits), lift SDR client windows to
//!   linear, composite, then tone-map + encode from linear.
//! - Otherwise (SDR panel, or 8-bit-only GL): decode HDR to display-referred
//!   sRGB (extended Reinhard + sRGB OETF) before the usual sRGB encode pass.
//!
//! Fullscreen / HDR-only on an HDR output still uses encode pass-through.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{
    AsRenderElements, Id, surface::WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::{
    GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::desktop::{LayerSurface, Window};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

/// BT.2408 reference white for mapping SDR peak to HDR (nits).
pub const REFERENCE_WHITE_NITS: f32 = 203.0;

/// Peak mastering luminance advertised on Metis HDR outputs (nits).
/// Keep in sync with [`crate::output_hdr`] `max_display_mastering_luminance`.
pub const OUTPUT_MASTERING_PEAK_NITS: f32 = 400.0;

/// Fallback content peak for decode tone-map when the output is not in HDR mode
/// (HDR client on an SDR panel). Typical PQ/HLG grade peak.
pub const DEFAULT_CONTENT_MAX_NITS: f32 = 1000.0;

/// Transfer function used for HDR encode + matching `HDR_OUTPUT_METADATA` EOTF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HdrTransfer {
    #[default]
    Pq,
    Hlg,
}

/// Placement and tone-map inputs for per-window PQ/HLG decode.
#[derive(Debug, Clone, Copy)]
pub struct HdrWindowDecodeOpts {
    pub place_at: Point<i32, Physical>,
    pub scale: Scale<f64>,
    pub alpha: f32,
    pub transfer: HdrTransfer,
    pub content_max_nits: f32,
    /// When true, decode to Rec.709 scene-linear (no tone-map / sRGB).
    pub scene_linear: bool,
}

/// Placement for per-window sRGB→linear lift (float scene path).
#[derive(Debug, Clone, Copy)]
pub struct HdrWindowLiftOpts {
    pub place_at: Point<i32, Physical>,
    pub scale: Scale<f64>,
    pub alpha: f32,
}

/// Placement for layer-shell sRGB→linear lift (float scene path).
#[derive(Debug, Clone, Copy)]
pub struct HdrLayerLiftOpts {
    pub place_at: Point<i32, Physical>,
    /// Layer geometry size in logical pixels (from `layer_map_for_output`).
    pub logical_size: Size<i32, Logical>,
    pub scale: Scale<f64>,
    pub alpha: f32,
}

/// Custom texture shader: sRGB → linear → BT.2020 → scale nits → ST.2084 PQ.
const PQ_ENCODE_SHADER: &str = r#"#version 100

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
uniform float reference_white;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float srgb_to_linear(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

vec3 rec709_to_bt2020(vec3 c) {
    return vec3(
        0.627404 * c.r + 0.329282 * c.g + 0.043314 * c.b,
        0.069097 * c.r + 0.919540 * c.g + 0.011362 * c.b,
        0.016391 * c.r + 0.088013 * c.g + 0.895595 * c.b
    );
}

float linear_to_pq(float y) {
    // ST 2084; y is luminance in [0, 1] where 1.0 = 10 000 nits.
    y = max(y, 0.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float ym = pow(y, m1);
    return pow((c1 + c2 * ym) / (1.0 + c3 * ym), m2);
}

void main() {
    vec4 srgb = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    srgb.a = 1.0;
#endif

    vec3 lin = vec3(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b)
    );
    lin = rec709_to_bt2020(lin);

    // Map SDR 1.0 → reference_white nits, then normalize to PQ's 10 000 nits.
    float scale = reference_white / 10000.0;
    vec3 pq = vec3(
        linear_to_pq(lin.r * scale),
        linear_to_pq(lin.g * scale),
        linear_to_pq(lin.b * scale)
    );

    vec4 color = vec4(pq, srgb.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Custom texture shader: sRGB → linear → BT.2020 → HLG OETF.
/// Linear is scaled so SDR peak ≈ reference_white on a 1000-nit HLG system
/// (BT.2408-style), then ARIB STD-B67 / Rec.2100 HLG OETF is applied.
const HLG_ENCODE_SHADER: &str = r#"#version 100

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
uniform float reference_white;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float srgb_to_linear(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

vec3 rec709_to_bt2020(vec3 c) {
    return vec3(
        0.627404 * c.r + 0.329282 * c.g + 0.043314 * c.b,
        0.069097 * c.r + 0.919540 * c.g + 0.011362 * c.b,
        0.016391 * c.r + 0.088013 * c.g + 0.895595 * c.b
    );
}

float linear_to_hlg(float x) {
    // Rec.2100 HLG OETF; x is scene-referred linear in [0, 1] (1.0 = peak).
    x = max(x, 0.0);
    float a = 0.17883277;
    float b = 1.0 - 4.0 * a;
    float c = 0.5 - a * log(4.0 * a);
    if (x <= 1.0 / 12.0) {
        return sqrt(3.0 * x);
    }
    return a * log(12.0 * x - b) + c;
}

void main() {
    vec4 srgb = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    srgb.a = 1.0;
#endif

    vec3 lin = vec3(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b)
    );
    lin = rec709_to_bt2020(lin);

    // Map SDR 1.0 → reference_white / 1000 peak (typical HLG system).
    float scale = reference_white / 1000.0;
    vec3 hlg = vec3(
        linear_to_hlg(lin.r * scale),
        linear_to_hlg(lin.g * scale),
        linear_to_hlg(lin.b * scale)
    );

    vec4 color = vec4(hlg, srgb.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Custom texture shader: PQ code values → linear → BT.2020→709 → tone-map → sRGB.
const PQ_DECODE_SHADER: &str = r#"#version 100

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
uniform float reference_white;
uniform float content_max_nits;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float pq_to_linear(float n) {
    // ST 2084 EOTF; n in [0,1] → luminance normalized to 10 000 nits.
    n = clamp(n, 0.0, 1.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float np = pow(n, 1.0 / m2);
    float num = max(np - c1, 0.0);
    float den = c2 - c3 * np;
    return pow(num / max(den, 1e-6), 1.0 / m1);
}

vec3 bt2020_to_rec709(vec3 c) {
    return vec3(
         1.6605 * c.r - 0.5876 * c.g - 0.0728 * c.b,
        -0.1246 * c.r + 1.1329 * c.g - 0.0083 * c.b,
        -0.0182 * c.r - 0.1006 * c.g + 1.1187 * c.b
    );
}

float linear_to_srgb(float c) {
    c = max(c, 0.0);
    if (c <= 0.0031308) {
        return 12.92 * c;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

void main() {
    vec4 pq = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    pq.a = 1.0;
#endif

    // PQ code → linear relative to 10 000 nits, then absolute nits.
    vec3 lin = vec3(
        pq_to_linear(pq.r),
        pq_to_linear(pq.g),
        pq_to_linear(pq.b)
    ) * 10000.0;

    // Extended Reinhard: mid-tones near reference white, soft shoulder to content peak.
    float rw = max(reference_white, 1.0);
    float cmax = max(content_max_nits, rw);
    float white_rel = cmax / rw;
    vec3 x = lin / rw;
    vec3 mapped = (x * (vec3(1.0) + x / (white_rel * white_rel))) / (vec3(1.0) + x);
    mapped = bt2020_to_rec709(mapped);
    mapped = clamp(mapped, 0.0, 1.0);

    vec3 srgb = vec3(
        linear_to_srgb(mapped.r),
        linear_to_srgb(mapped.g),
        linear_to_srgb(mapped.b)
    );

    vec4 color = vec4(srgb, pq.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Custom texture shader: HLG code values → linear → BT.2020→709 → tone-map → sRGB.
const HLG_DECODE_SHADER: &str = r#"#version 100

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
uniform float reference_white;
uniform float content_max_nits;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float hlg_to_linear(float x) {
    // Rec.2100 HLG inverse OETF; x in [0,1] → scene-referred linear [0,1].
    x = clamp(x, 0.0, 1.0);
    float a = 0.17883277;
    float b = 1.0 - 4.0 * a;
    float c = 0.5 - a * log(4.0 * a);
    if (x <= 0.5) {
        return (x * x) / 3.0;
    }
    return (exp((x - c) / a) + b) / 12.0;
}

vec3 bt2020_to_rec709(vec3 c) {
    return vec3(
         1.6605 * c.r - 0.5876 * c.g - 0.0728 * c.b,
        -0.1246 * c.r + 1.1329 * c.g - 0.0083 * c.b,
        -0.0182 * c.r - 0.1006 * c.g + 1.1187 * c.b
    );
}

float linear_to_srgb(float c) {
    c = max(c, 0.0);
    if (c <= 0.0031308) {
        return 12.92 * c;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

void main() {
    vec4 hlg = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    hlg.a = 1.0;
#endif

    vec3 scene = vec3(
        hlg_to_linear(hlg.r),
        hlg_to_linear(hlg.g),
        hlg_to_linear(hlg.b)
    );
    // Match encode: scene 1.0 ≈ 1000-nit HLG system peak.
    vec3 lin = scene * 1000.0;

    float rw = max(reference_white, 1.0);
    float cmax = max(content_max_nits, rw);
    float white_rel = cmax / rw;
    vec3 x = lin / rw;
    vec3 mapped = (x * (vec3(1.0) + x / (white_rel * white_rel))) / (vec3(1.0) + x);
    mapped = bt2020_to_rec709(mapped);
    mapped = clamp(mapped, 0.0, 1.0);

    vec3 srgb = vec3(
        linear_to_srgb(mapped.r),
        linear_to_srgb(mapped.g),
        linear_to_srgb(mapped.b)
    );

    vec4 color = vec4(srgb, hlg.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// PQ → Rec.709 scene-linear (1.0 = reference_white nits); keeps highlight headroom.
const PQ_DECODE_LINEAR_SHADER: &str = r#"#version 100

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
uniform float reference_white;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float pq_to_linear(float n) {
    n = clamp(n, 0.0, 1.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float np = pow(n, 1.0 / m2);
    float num = max(np - c1, 0.0);
    float den = c2 - c3 * np;
    return pow(num / max(den, 1e-6), 1.0 / m1);
}

vec3 bt2020_to_rec709(vec3 c) {
    return vec3(
         1.6605 * c.r - 0.5876 * c.g - 0.0728 * c.b,
        -0.1246 * c.r + 1.1329 * c.g - 0.0083 * c.b,
        -0.0182 * c.r - 0.1006 * c.g + 1.1187 * c.b
    );
}

void main() {
    vec4 pq = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    pq.a = 1.0;
#endif

    vec3 lin = vec3(
        pq_to_linear(pq.r),
        pq_to_linear(pq.g),
        pq_to_linear(pq.b)
    ) * 10000.0;

    float rw = max(reference_white, 1.0);
    vec3 mapped = bt2020_to_rec709(lin / rw);
    // Soft safety for pathological values; float FBOs keep >1 headroom.
    mapped = min(mapped, vec3(64.0));

    vec4 color = vec4(mapped, pq.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// HLG → Rec.709 scene-linear (1.0 = reference_white nits).
const HLG_DECODE_LINEAR_SHADER: &str = r#"#version 100

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
uniform float reference_white;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float hlg_to_linear(float x) {
    x = clamp(x, 0.0, 1.0);
    float a = 0.17883277;
    float b = 1.0 - 4.0 * a;
    float c = 0.5 - a * log(4.0 * a);
    if (x <= 0.5) {
        return (x * x) / 3.0;
    }
    return (exp((x - c) / a) + b) / 12.0;
}

vec3 bt2020_to_rec709(vec3 c) {
    return vec3(
         1.6605 * c.r - 0.5876 * c.g - 0.0728 * c.b,
        -0.1246 * c.r + 1.1329 * c.g - 0.0083 * c.b,
        -0.0182 * c.r - 0.1006 * c.g + 1.1187 * c.b
    );
}

void main() {
    vec4 hlg = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    hlg.a = 1.0;
#endif

    vec3 scene = vec3(
        hlg_to_linear(hlg.r),
        hlg_to_linear(hlg.g),
        hlg_to_linear(hlg.b)
    );
    vec3 lin = scene * 1000.0;

    float rw = max(reference_white, 1.0);
    vec3 mapped = bt2020_to_rec709(lin / rw);
    mapped = min(mapped, vec3(64.0));

    vec4 color = vec4(mapped, hlg.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Rec.709 scene-linear → BT.2390-style EETF → PQ (float scene path).
const PQ_ENCODE_LINEAR_SHADER: &str = r#"#version 100

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
uniform float reference_white;
uniform float content_max_nits;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

vec3 rec709_to_bt2020(vec3 c) {
    return vec3(
        0.627404 * c.r + 0.329282 * c.g + 0.043314 * c.b,
        0.069097 * c.r + 0.919540 * c.g + 0.011362 * c.b,
        0.016391 * c.r + 0.088013 * c.g + 0.895595 * c.b
    );
}

float linear_to_pq(float y) {
    y = max(y, 0.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float ym = pow(y, m1);
    return pow((c1 + c2 * ym) / (1.0 + c3 * ym), m2);
}

// BT.2390-10 EETF approximation (skip black lift). Input is scene-linear
// relative to reference_white (1.0 = rw nits); returns absolute PQ code.
float bt2390_eetf_pq(float x_rel, float rw, float peak) {
    float Y = max(x_rel, 0.0) * rw;
    float E = linear_to_pq(Y / 10000.0);
    float Ep = max(linear_to_pq(peak / 10000.0), 1e-6);
    float Es = linear_to_pq(rw / 10000.0);
    float E1 = E / Ep;
    float Es1 = Es / Ep;
    float KS = clamp(1.5 * Es1 - 0.5, 0.0, 0.99);
    float E2;
    if (E1 < KS) {
        E2 = E1;
    } else {
        float t = clamp((E1 - KS) / max(1.0 - KS, 1e-6), 0.0, 1.0);
        float t2 = t * t;
        float t3 = t2 * t;
        E2 = (2.0 * t3 - 3.0 * t2 + 1.0) * KS
           + (t3 - 2.0 * t2 + t) * (1.0 - KS)
           + (-2.0 * t3 + 3.0 * t2);
    }
    return clamp(E2 * Ep, 0.0, 1.0);
}

void main() {
    vec4 lin_in = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    lin_in.a = 1.0;
#endif

    float rw = max(reference_white, 1.0);
    float peak = max(content_max_nits, rw);
    vec3 c2020 = rec709_to_bt2020(max(lin_in.rgb, vec3(0.0)));
    vec3 pq = vec3(
        bt2390_eetf_pq(c2020.r, rw, peak),
        bt2390_eetf_pq(c2020.g, rw, peak),
        bt2390_eetf_pq(c2020.b, rw, peak)
    );

    vec4 color = vec4(pq, lin_in.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Rec.709 scene-linear → BT.2390-style EETF → HLG (float scene path).
const HLG_ENCODE_LINEAR_SHADER: &str = r#"#version 100

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
uniform float reference_white;
uniform float content_max_nits;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

vec3 rec709_to_bt2020(vec3 c) {
    return vec3(
        0.627404 * c.r + 0.329282 * c.g + 0.043314 * c.b,
        0.069097 * c.r + 0.919540 * c.g + 0.011362 * c.b,
        0.016391 * c.r + 0.088013 * c.g + 0.895595 * c.b
    );
}

float linear_to_pq(float y) {
    y = max(y, 0.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float ym = pow(y, m1);
    return pow((c1 + c2 * ym) / (1.0 + c3 * ym), m2);
}

float pq_to_linear(float n) {
    n = clamp(n, 0.0, 1.0);
    float m1 = 2610.0 / 16384.0;
    float m2 = 2523.0 / 32.0;
    float c1 = 3424.0 / 4096.0;
    float c2 = 2413.0 / 128.0;
    float c3 = 2392.0 / 128.0;
    float np = pow(n, 1.0 / m2);
    float num = max(np - c1, 0.0);
    float den = c2 - c3 * np;
    return pow(num / max(den, 1e-6), 1.0 / m1);
}

float linear_to_hlg(float x) {
    x = max(x, 0.0);
    float a = 0.17883277;
    float b = 1.0 - 4.0 * a;
    float c = 0.5 - a * log(4.0 * a);
    if (x <= 1.0 / 12.0) {
        return sqrt(3.0 * x);
    }
    return a * log(12.0 * x - b) + c;
}

float bt2390_eetf_pq(float x_rel, float rw, float peak) {
    float Y = max(x_rel, 0.0) * rw;
    float E = linear_to_pq(Y / 10000.0);
    float Ep = max(linear_to_pq(peak / 10000.0), 1e-6);
    float Es = linear_to_pq(rw / 10000.0);
    float E1 = E / Ep;
    float Es1 = Es / Ep;
    float KS = clamp(1.5 * Es1 - 0.5, 0.0, 0.99);
    float E2;
    if (E1 < KS) {
        E2 = E1;
    } else {
        float t = clamp((E1 - KS) / max(1.0 - KS, 1e-6), 0.0, 1.0);
        float t2 = t * t;
        float t3 = t2 * t;
        E2 = (2.0 * t3 - 3.0 * t2 + 1.0) * KS
           + (t3 - 2.0 * t2 + t) * (1.0 - KS)
           + (-2.0 * t3 + 3.0 * t2);
    }
    return clamp(E2 * Ep, 0.0, 1.0);
}

void main() {
    vec4 lin_in = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    lin_in.a = 1.0;
#endif

    float rw = max(reference_white, 1.0);
    float peak = max(content_max_nits, rw);
    vec3 c2020 = rec709_to_bt2020(max(lin_in.rgb, vec3(0.0)));
    // EETF in PQ, then linear nits → HLG OETF (1000-nit system).
    vec3 pq = vec3(
        bt2390_eetf_pq(c2020.r, rw, peak),
        bt2390_eetf_pq(c2020.g, rw, peak),
        bt2390_eetf_pq(c2020.b, rw, peak)
    );
    vec3 nits = vec3(
        pq_to_linear(pq.r),
        pq_to_linear(pq.g),
        pq_to_linear(pq.b)
    ) * 10000.0;
    vec3 hlg = vec3(
        linear_to_hlg(nits.r / 1000.0),
        linear_to_hlg(nits.g / 1000.0),
        linear_to_hlg(nits.b / 1000.0)
    );

    vec4 color = vec4(hlg, lin_in.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// sRGB → Rec.709 linear (peak 1.0) for SDR clients in the float scene path.
const SDR_LIFT_SHADER: &str = r#"#version 100

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

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

float srgb_to_linear(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

void main() {
    vec4 srgb = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    srgb.a = 1.0;
#endif

    vec3 lin = vec3(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b)
    );

    vec4 color = vec4(lin, srgb.a) * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Persistent HDR encode/decode GL resources, owned by `MetisState`.
#[derive(Default)]
pub struct HdrEncodeRuntime {
    pub pq_program: Option<GlesTexProgram>,
    pub hlg_program: Option<GlesTexProgram>,
    pub pq_linear_program: Option<GlesTexProgram>,
    pub hlg_linear_program: Option<GlesTexProgram>,
    pub pq_decode_program: Option<GlesTexProgram>,
    pub hlg_decode_program: Option<GlesTexProgram>,
    pub pq_decode_linear_program: Option<GlesTexProgram>,
    pub hlg_decode_linear_program: Option<GlesTexProgram>,
    pub sdr_lift_program: Option<GlesTexProgram>,
    /// Cached probe: float or 10-bit offscreen available for scene-linear path.
    scene_linear_formats: Option<bool>,
}

impl HdrEncodeRuntime {
    /// Drop compiled shaders so they recompile after VT resume / context loss.
    pub fn invalidate_gl(&mut self) {
        self.pq_program = None;
        self.hlg_program = None;
        self.pq_linear_program = None;
        self.hlg_linear_program = None;
        self.pq_decode_program = None;
        self.hlg_decode_program = None;
        self.pq_decode_linear_program = None;
        self.hlg_decode_linear_program = None;
        self.sdr_lift_program = None;
        self.scene_linear_formats = None;
    }

    /// True when the GLES context can allocate float or 10-bit offscreens.
    pub fn scene_linear_formats_ok(&mut self, renderer: &mut GlesRenderer) -> bool {
        if let Some(ok) = self.scene_linear_formats {
            return ok;
        }
        let size = Size::<i32, Buffer>::from((1, 1));
        let ok = [Fourcc::Abgr16161616f, Fourcc::Abgr2101010]
            .into_iter()
            .any(|format| Offscreen::<GlesTexture>::create_buffer(renderer, format, size).is_ok());
        if !ok {
            tracing::info!("hdr: no float/10-bit offscreen; mixed HDR stays display-referred sRGB");
        }
        self.scene_linear_formats = Some(ok);
        ok
    }

    pub fn ensure_program(&mut self, renderer: &mut GlesRenderer, transfer: HdrTransfer) {
        match transfer {
            HdrTransfer::Pq => {
                if self.pq_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    PQ_ENCODE_SHADER,
                    &[UniformName::new("reference_white", UniformType::_1f)],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled SDR→BT.2020→PQ encode shader");
                        self.pq_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "hdr: failed to compile PQ encode shader; leaving SDR")
                    }
                }
            }
            HdrTransfer::Hlg => {
                if self.hlg_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    HLG_ENCODE_SHADER,
                    &[UniformName::new("reference_white", UniformType::_1f)],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled SDR→BT.2020→HLG encode shader");
                        self.hlg_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(
                            ?err,
                            "hdr: failed to compile HLG encode shader; leaving SDR"
                        )
                    }
                }
            }
        }
    }

    pub fn ensure_linear_encode_program(
        &mut self,
        renderer: &mut GlesRenderer,
        transfer: HdrTransfer,
    ) {
        match transfer {
            HdrTransfer::Pq => {
                if self.pq_linear_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    PQ_ENCODE_LINEAR_SHADER,
                    &[
                        UniformName::new("reference_white", UniformType::_1f),
                        UniformName::new("content_max_nits", UniformType::_1f),
                    ],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled linear→BT.2020→PQ encode shader");
                        self.pq_linear_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "hdr: failed to compile linear PQ encode shader")
                    }
                }
            }
            HdrTransfer::Hlg => {
                if self.hlg_linear_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    HLG_ENCODE_LINEAR_SHADER,
                    &[
                        UniformName::new("reference_white", UniformType::_1f),
                        UniformName::new("content_max_nits", UniformType::_1f),
                    ],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled linear→BT.2020→HLG encode shader");
                        self.hlg_linear_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "hdr: failed to compile linear HLG encode shader")
                    }
                }
            }
        }
    }

    pub fn ensure_decode_program(
        &mut self,
        renderer: &mut GlesRenderer,
        transfer: HdrTransfer,
        scene_linear: bool,
    ) {
        if scene_linear {
            match transfer {
                HdrTransfer::Pq => {
                    if self.pq_decode_linear_program.is_some() {
                        return;
                    }
                    match renderer.compile_custom_texture_shader(
                        PQ_DECODE_LINEAR_SHADER,
                        &[UniformName::new("reference_white", UniformType::_1f)],
                    ) {
                        Ok(program) => {
                            tracing::info!("hdr: compiled PQ→linear decode shader");
                            self.pq_decode_linear_program = Some(program);
                        }
                        Err(err) => {
                            tracing::warn!(?err, "hdr: failed to compile PQ linear decode shader")
                        }
                    }
                }
                HdrTransfer::Hlg => {
                    if self.hlg_decode_linear_program.is_some() {
                        return;
                    }
                    match renderer.compile_custom_texture_shader(
                        HLG_DECODE_LINEAR_SHADER,
                        &[UniformName::new("reference_white", UniformType::_1f)],
                    ) {
                        Ok(program) => {
                            tracing::info!("hdr: compiled HLG→linear decode shader");
                            self.hlg_decode_linear_program = Some(program);
                        }
                        Err(err) => {
                            tracing::warn!(?err, "hdr: failed to compile HLG linear decode shader")
                        }
                    }
                }
            }
            return;
        }
        match transfer {
            HdrTransfer::Pq => {
                if self.pq_decode_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    PQ_DECODE_SHADER,
                    &[
                        UniformName::new("reference_white", UniformType::_1f),
                        UniformName::new("content_max_nits", UniformType::_1f),
                    ],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled PQ→sRGB decode shader");
                        self.pq_decode_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "hdr: failed to compile PQ decode shader")
                    }
                }
            }
            HdrTransfer::Hlg => {
                if self.hlg_decode_program.is_some() {
                    return;
                }
                match renderer.compile_custom_texture_shader(
                    HLG_DECODE_SHADER,
                    &[
                        UniformName::new("reference_white", UniformType::_1f),
                        UniformName::new("content_max_nits", UniformType::_1f),
                    ],
                ) {
                    Ok(program) => {
                        tracing::info!("hdr: compiled HLG→sRGB decode shader");
                        self.hlg_decode_program = Some(program);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "hdr: failed to compile HLG decode shader")
                    }
                }
            }
        }
    }

    pub fn ensure_lift_program(&mut self, renderer: &mut GlesRenderer) {
        if self.sdr_lift_program.is_some() {
            return;
        }
        match renderer.compile_custom_texture_shader(SDR_LIFT_SHADER, &[]) {
            Ok(program) => {
                tracing::info!("hdr: compiled sRGB→linear lift shader");
                self.sdr_lift_program = Some(program);
            }
            Err(err) => tracing::warn!(?err, "hdr: failed to compile SDR lift shader"),
        }
    }

    fn decode_program(&self, transfer: HdrTransfer, scene_linear: bool) -> Option<GlesTexProgram> {
        match (transfer, scene_linear) {
            (HdrTransfer::Pq, true) => self.pq_decode_linear_program.clone(),
            (HdrTransfer::Hlg, true) => self.hlg_decode_linear_program.clone(),
            (HdrTransfer::Pq, false) => self.pq_decode_program.clone(),
            (HdrTransfer::Hlg, false) => self.hlg_decode_program.clone(),
        }
    }

    fn linear_encode_program(&self, transfer: HdrTransfer) -> Option<GlesTexProgram> {
        match transfer {
            HdrTransfer::Pq => self.pq_linear_program.clone(),
            HdrTransfer::Hlg => self.hlg_linear_program.clone(),
        }
    }

    /// Offscreen formats for window staging before a texture shader blit.
    fn window_offscreen_formats(scene_linear: bool) -> &'static [Fourcc] {
        if scene_linear {
            &[Fourcc::Abgr16161616f, Fourcc::Abgr2101010, Fourcc::Abgr8888]
        } else {
            &[Fourcc::Abgr2101010, Fourcc::Abgr8888]
        }
    }

    fn render_window_offscreen(
        renderer: &mut GlesRenderer,
        window: &Window,
        scale: Scale<f64>,
        alpha: f32,
        formats: &[Fourcc],
    ) -> Option<(GlesTexture, Size<i32, Physical>)> {
        let geo = window.geometry();
        let width = geo.size.w.max(1);
        let height = geo.size.h.max(1);
        let size_phys: Size<i32, Physical> =
            Size::<i32, Logical>::from((width, height)).to_physical_precise_round(scale);
        if size_phys.w <= 0 || size_phys.h <= 0 {
            return None;
        }
        if size_phys.w > 7680 || size_phys.h > 4320 {
            tracing::warn!(
                w = size_phys.w,
                h = size_phys.h,
                "hdr: refusing window offscreen larger than 8K"
            );
            return None;
        }

        let size_buf: Size<i32, Buffer> = Size::from((size_phys.w, size_phys.h));
        let loc =
            Point::<i32, Logical>::from((-geo.loc.x, -geo.loc.y)).to_physical_precise_round(scale);
        let elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(window, renderer, loc, scale, alpha);
        if elems.is_empty() {
            return None;
        }

        for format in formats {
            let mut offscreen =
                match Offscreen::<GlesTexture>::create_buffer(renderer, *format, size_buf) {
                    Ok(buf) => buf,
                    Err(_) => continue,
                };
            let ok = {
                let mut framebuffer = match renderer.bind(&mut offscreen) {
                    Ok(fb) => fb,
                    Err(_) => continue,
                };
                let mut damage_tracker =
                    OutputDamageTracker::new(size_phys, scale, Transform::Normal);
                damage_tracker
                    .render_output(renderer, &mut framebuffer, 0, &elems, DECODE_CLEAR)
                    .is_ok()
            };
            if ok {
                return Some((offscreen, size_phys));
            }
        }
        None
    }

    /// Render `window` to an offscreen buffer, run PQ/HLG decode, and return
    /// a texture element placed at `opts.place_at` (render-target local).
    pub fn try_decode_window_element(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        opts: HdrWindowDecodeOpts,
    ) -> Option<TextureShaderElement> {
        self.ensure_decode_program(renderer, opts.transfer, opts.scene_linear);
        let program = self.decode_program(opts.transfer, opts.scene_linear)?;
        let (scene, size_phys) = Self::render_window_offscreen(
            renderer,
            window,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(opts.scene_linear),
        )?;

        let buffer = TextureBuffer::from_texture(renderer, scene, 1, Transform::Normal, None);
        let src_rect = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        let inner = TextureRenderElement::from_texture_buffer(
            Point::<f64, Physical>::from((opts.place_at.x as f64, opts.place_at.y as f64)),
            &buffer,
            None,
            Some(src_rect),
            Some(Size::from((size_phys.w, size_phys.h))),
            Kind::Unspecified,
        );
        let uniforms = if opts.scene_linear {
            vec![Uniform::new("reference_white", REFERENCE_WHITE_NITS)]
        } else {
            let content_max = opts.content_max_nits.max(REFERENCE_WHITE_NITS);
            vec![
                Uniform::new("reference_white", REFERENCE_WHITE_NITS),
                Uniform::new("content_max_nits", content_max),
            ]
        };
        Some(TextureShaderElement::new(inner, program, uniforms))
    }

    /// Hybrid MultiRenderer variant: same offscreen decode setup, wrapped for
    /// [`crate::hybrid_shader::HybridTexShaderElement`].
    pub fn try_decode_window_hybrid(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        opts: HdrWindowDecodeOpts,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        self.ensure_decode_program(renderer, opts.transfer, opts.scene_linear);
        let program = self.decode_program(opts.transfer, opts.scene_linear)?;
        let (scene, size_phys) = Self::render_window_offscreen(
            renderer,
            window,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(opts.scene_linear),
        )?;
        let geometry = Rectangle::new(opts.place_at, size_phys);
        let src = Rectangle::<f64, Buffer>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        let uniforms = if opts.scene_linear {
            vec![Uniform::new("reference_white", REFERENCE_WHITE_NITS)]
        } else {
            let content_max = opts.content_max_nits.max(REFERENCE_WHITE_NITS);
            vec![
                Uniform::new("reference_white", REFERENCE_WHITE_NITS),
                Uniform::new("content_max_nits", content_max),
            ]
        };
        crate::hybrid_shader::HybridTexShaderElement::from_gles_texture(
            renderer,
            Id::new(),
            CommitCounter::default(),
            geometry,
            src,
            scene,
            program,
            uniforms,
            opts.alpha,
            Kind::Unspecified,
        )
    }

    /// Wrap an existing sRGB texture element with the SDR→linear lift shader.
    pub fn wrap_lift_element(
        &mut self,
        renderer: &mut GlesRenderer,
        inner: TextureRenderElement<GlesTexture>,
    ) -> Option<TextureShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        Some(TextureShaderElement::new(inner, program, vec![]))
    }

    /// Wrap a MultiTexture with the SDR→linear lift shader (hybrid wallpaper).
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_lift_hybrid(
        &mut self,
        renderer: &mut GlesRenderer,
        id: Id,
        commit: CommitCounter,
        geometry: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        texture: smithay::backend::renderer::multigpu::MultiTexture,
        alpha: f32,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        Some(
            crate::hybrid_shader::HybridTexShaderElement::from_multi_texture(
                id,
                commit,
                geometry,
                src,
                texture,
                program,
                vec![],
                alpha,
                Kind::Unspecified,
            ),
        )
    }

    /// Lift an SDR window to Rec.709 linear for the float scene path.
    pub fn try_lift_window_element(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        opts: HdrWindowLiftOpts,
    ) -> Option<TextureShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        let (scene, size_phys) = Self::render_window_offscreen(
            renderer,
            window,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(true),
        )?;
        let buffer = TextureBuffer::from_texture(renderer, scene, 1, Transform::Normal, None);
        let src_rect = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        let inner = TextureRenderElement::from_texture_buffer(
            Point::<f64, Physical>::from((opts.place_at.x as f64, opts.place_at.y as f64)),
            &buffer,
            None,
            Some(src_rect),
            Some(Size::from((size_phys.w, size_phys.h))),
            Kind::Unspecified,
        );
        Some(TextureShaderElement::new(inner, program, vec![]))
    }

    /// Hybrid MultiRenderer variant of [`Self::try_lift_window_element`].
    pub fn try_lift_window_hybrid(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        opts: HdrWindowLiftOpts,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        let (scene, size_phys) = Self::render_window_offscreen(
            renderer,
            window,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(true),
        )?;
        let geometry = Rectangle::new(opts.place_at, size_phys);
        let src = Rectangle::<f64, Buffer>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        crate::hybrid_shader::HybridTexShaderElement::from_gles_texture(
            renderer,
            Id::new(),
            CommitCounter::default(),
            geometry,
            src,
            scene,
            program,
            vec![],
            opts.alpha,
            Kind::Unspecified,
        )
    }

    fn render_layer_offscreen(
        renderer: &mut GlesRenderer,
        surface: &LayerSurface,
        logical_size: Size<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        formats: &[Fourcc],
    ) -> Option<(GlesTexture, Size<i32, Physical>)> {
        let size_phys: Size<i32, Physical> = logical_size.to_physical_precise_round(scale);
        if size_phys.w <= 0 || size_phys.h <= 0 {
            return None;
        }
        if size_phys.w > 7680 || size_phys.h > 4320 {
            tracing::warn!(
                w = size_phys.w,
                h = size_phys.h,
                "hdr: refusing layer offscreen larger than 8K"
            );
            return None;
        }

        let size_buf: Size<i32, Buffer> = Size::from((size_phys.w, size_phys.h));
        let loc = Point::<i32, Physical>::from((0, 0));
        let elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(surface, renderer, loc, scale, alpha);
        if elems.is_empty() {
            return None;
        }

        for format in formats {
            let mut offscreen =
                match Offscreen::<GlesTexture>::create_buffer(renderer, *format, size_buf) {
                    Ok(buf) => buf,
                    Err(_) => continue,
                };
            let ok = {
                let mut framebuffer = match renderer.bind(&mut offscreen) {
                    Ok(fb) => fb,
                    Err(_) => continue,
                };
                let mut damage_tracker =
                    OutputDamageTracker::new(size_phys, scale, Transform::Normal);
                damage_tracker
                    .render_output(renderer, &mut framebuffer, 0, &elems, DECODE_CLEAR)
                    .is_ok()
            };
            if ok {
                return Some((offscreen, size_phys));
            }
        }
        None
    }

    /// Lift a layer-shell surface (bar, overlays) to Rec.709 linear.
    pub fn try_lift_layer_element(
        &mut self,
        renderer: &mut GlesRenderer,
        surface: &LayerSurface,
        opts: HdrLayerLiftOpts,
    ) -> Option<TextureShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        let (scene, size_phys) = Self::render_layer_offscreen(
            renderer,
            surface,
            opts.logical_size,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(true),
        )?;
        let buffer = TextureBuffer::from_texture(renderer, scene, 1, Transform::Normal, None);
        let src_rect = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        let inner = TextureRenderElement::from_texture_buffer(
            Point::<f64, Physical>::from((opts.place_at.x as f64, opts.place_at.y as f64)),
            &buffer,
            None,
            Some(src_rect),
            Some(Size::from((size_phys.w, size_phys.h))),
            Kind::Unspecified,
        );
        Some(TextureShaderElement::new(inner, program, vec![]))
    }

    /// Hybrid MultiRenderer variant of [`Self::try_lift_layer_element`].
    pub fn try_lift_layer_hybrid(
        &mut self,
        renderer: &mut GlesRenderer,
        surface: &LayerSurface,
        opts: HdrLayerLiftOpts,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        self.ensure_lift_program(renderer);
        let program = self.sdr_lift_program.clone()?;
        let (scene, size_phys) = Self::render_layer_offscreen(
            renderer,
            surface,
            opts.logical_size,
            opts.scale,
            opts.alpha,
            Self::window_offscreen_formats(true),
        )?;
        let geometry = Rectangle::new(opts.place_at, size_phys);
        let src = Rectangle::<f64, Buffer>::new(
            Point::from((0.0, 0.0)),
            Size::from((size_phys.w as f64, size_phys.h as f64)),
        );
        crate::hybrid_shader::HybridTexShaderElement::from_gles_texture(
            renderer,
            Id::new(),
            CommitCounter::default(),
            geometry,
            src,
            scene,
            program,
            vec![],
            opts.alpha,
            Kind::Unspecified,
        )
    }

    /// Build the HDR encode texture-shader element for a composited scene.
    pub fn encode_scene_element(
        &mut self,
        renderer: &mut GlesRenderer,
        scene: GlesTexture,
        size: Size<i32, Physical>,
        transfer: HdrTransfer,
        scene_linear: bool,
        content_max_nits: f32,
    ) -> Option<TextureShaderElement> {
        let (program, uniforms) = if scene_linear {
            self.ensure_linear_encode_program(renderer, transfer);
            let program = self.linear_encode_program(transfer)?;
            let content_max = content_max_nits.max(REFERENCE_WHITE_NITS);
            (
                program,
                vec![
                    Uniform::new("reference_white", REFERENCE_WHITE_NITS),
                    Uniform::new("content_max_nits", content_max),
                ],
            )
        } else {
            self.ensure_program(renderer, transfer);
            let program = match transfer {
                HdrTransfer::Pq => self.pq_program.clone()?,
                HdrTransfer::Hlg => self.hlg_program.clone()?,
            };
            (
                program,
                vec![Uniform::new("reference_white", REFERENCE_WHITE_NITS)],
            )
        };
        let buffer = TextureBuffer::from_texture(renderer, scene, 1, Transform::Normal, None);
        let src_rect = Rectangle::<f64, Logical>::new(
            Point::from((0.0, 0.0)),
            Size::from((size.w as f64, size.h as f64)),
        );
        let inner = TextureRenderElement::from_texture_buffer(
            Point::<f64, Physical>::from((0.0, 0.0)),
            &buffer,
            None,
            Some(src_rect),
            Some(Size::from((size.w, size.h))),
            Kind::Unspecified,
        );
        Some(TextureShaderElement::new(inner, program, uniforms))
    }

    /// Hybrid encode wrap for a primary-GPU scene texture.
    pub fn encode_scene_hybrid(
        &mut self,
        renderer: &mut GlesRenderer,
        scene: GlesTexture,
        size: Size<i32, Physical>,
        transfer: HdrTransfer,
        scene_linear: bool,
        content_max_nits: f32,
    ) -> Option<crate::hybrid_shader::HybridTexShaderElement> {
        let (program, uniforms) = if scene_linear {
            self.ensure_linear_encode_program(renderer, transfer);
            let program = self.linear_encode_program(transfer)?;
            let content_max = content_max_nits.max(REFERENCE_WHITE_NITS);
            (
                program,
                vec![
                    Uniform::new("reference_white", REFERENCE_WHITE_NITS),
                    Uniform::new("content_max_nits", content_max),
                ],
            )
        } else {
            self.ensure_program(renderer, transfer);
            let program = match transfer {
                HdrTransfer::Pq => self.pq_program.clone()?,
                HdrTransfer::Hlg => self.hlg_program.clone()?,
            };
            (
                program,
                vec![Uniform::new("reference_white", REFERENCE_WHITE_NITS)],
            )
        };
        let geometry = Rectangle::new(Point::from((0, 0)), size);
        let src = Rectangle::<f64, Buffer>::new(
            Point::from((0.0, 0.0)),
            Size::from((size.w as f64, size.h as f64)),
        );
        crate::hybrid_shader::HybridTexShaderElement::from_gles_texture(
            renderer,
            Id::new(),
            CommitCounter::default(),
            geometry,
            src,
            scene,
            program,
            uniforms,
            1.0,
            Kind::Unspecified,
        )
    }
}

/// Clear colour for the final HDR scanout pass (fullscreen encode element covers it).
pub const HDR_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Transparent clear when rendering a single HDR window for decode.
const DECODE_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
