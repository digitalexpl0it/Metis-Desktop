//! Unified DRM colour post-pass: optional Stage 2 3D-LUT + optional HDR encode.
//!
//! Pipeline: composite elements → (prefer 10-bit / float offscreen) → LUT blit →
//! PQ or HLG encode (when HDR active) → single fullscreen scanout element.

use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::multigpu::{MultiTexture, gbm::GbmGlesBackend};
use smithay::backend::renderer::{Bind, Offscreen, Renderer};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

type UdevGbm = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;

use crate::color_lut::ColorLutRuntime;
use crate::hdr_encode::{HDR_CLEAR, HdrEncodeRuntime, HdrTransfer};
use crate::render::{CLEAR_COLOR, OutputStack};

/// Result of the colour post-pass ready for `render_frame`.
pub struct ColourPassResult {
    pub elements: Vec<OutputStack>,
    pub clear: [f32; 4],
}

/// Composite `elements`, optionally apply the output LUT, optionally HDR-encode.
///
/// Returns `None` when neither LUT nor HDR is active (caller scans out `elements`
/// directly). On GL failure falls back toward a simpler path and may return
/// `None` so the caller can use the original stack.
#[allow(clippy::too_many_arguments)]
pub fn apply_colour_post_pass(
    lut_runtime: &mut ColorLutRuntime,
    hdr_runtime: &mut HdrEncodeRuntime,
    renderer: &mut GlesRenderer,
    elements: &[OutputStack],
    output_name: &str,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    hdr_active: bool,
    hdr_transfer: HdrTransfer,
    // When true, skip SDR→HDR encode (client already provided PQ/HLG content).
    hdr_passthrough: bool,
    // Scene is Rec.709 linear (mixed HDR float path); encode skips sRGB EOTF.
    scene_linear: bool,
    content_max_nits: f32,
) -> Option<ColourPassResult> {
    let wants_lut = lut_runtime.lut_owns_output(output_name);
    let wants_hdr_encode = hdr_active && !hdr_passthrough;
    if !wants_lut && !wants_hdr_encode {
        // HDR output with pass-through: still need a single fullscreen element if
        // we only wanted to skip encode — return None so the original stack scans out.
        return None;
    }
    if size.w <= 0 || size.h <= 0 {
        return None;
    }

    let size_buf: Size<i32, Buffer> = Size::from((size.w, size.h));
    let scene = composite_offscreen(renderer, elements, size, scale, size_buf)?;
    apply_colour_post_pass_scene(
        lut_runtime,
        hdr_runtime,
        renderer,
        scene,
        output_name,
        size,
        hdr_active,
        hdr_transfer,
        hdr_passthrough,
        scene_linear,
        content_max_nits,
    )
}

/// Apply LUT / HDR encode to an already-composited scene texture.
///
/// Used by the GLES path after offscreen composite and by the hybrid Multi path
/// after same-node Multi offscreen composite.
#[allow(clippy::too_many_arguments)]
pub fn apply_colour_post_pass_scene(
    lut_runtime: &mut ColorLutRuntime,
    hdr_runtime: &mut HdrEncodeRuntime,
    renderer: &mut GlesRenderer,
    mut scene: GlesTexture,
    output_name: &str,
    size: Size<i32, Physical>,
    hdr_active: bool,
    hdr_transfer: HdrTransfer,
    hdr_passthrough: bool,
    scene_linear: bool,
    content_max_nits: f32,
) -> Option<ColourPassResult> {
    let wants_lut = lut_runtime.lut_owns_output(output_name);
    let wants_hdr_encode = hdr_active && !hdr_passthrough;
    if !wants_lut && !wants_hdr_encode {
        return None;
    }
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let size_buf: Size<i32, Buffer> = Size::from((size.w, size.h));

    if wants_lut
        && let Some(mapped) = lut_runtime.apply(renderer, output_name, scene.clone(), size_buf)
    {
        scene = mapped;
    }

    if wants_hdr_encode {
        let encoded = hdr_runtime.encode_scene_element(
            renderer,
            scene,
            size,
            hdr_transfer,
            scene_linear,
            content_max_nits,
        )?;
        return Some(ColourPassResult {
            elements: vec![OutputStack::HdrEncode(encoded)],
            clear: HDR_CLEAR,
        });
    }

    // LUT-only: scan out the corrected texture.
    let buffer = TextureBuffer::from_texture(renderer, scene, 1, Transform::Normal, None);
    let src_rect = Rectangle::<f64, Logical>::new(
        Point::from((0.0, 0.0)),
        Size::from((size.w as f64, size.h as f64)),
    );
    let tex = TextureRenderElement::from_texture_buffer(
        Point::<f64, Physical>::from((0.0, 0.0)),
        &buffer,
        None,
        Some(src_rect),
        Some(Size::from((size.w, size.h))),
        Kind::Unspecified,
    );
    Some(ColourPassResult {
        elements: vec![OutputStack::Wallpaper(tex)],
        clear: CLEAR_COLOR,
    })
}

/// Hybrid Multi present result after colour post: one fullscreen MultiTexture
/// element (LUT-only) or a hybrid shader element (HDR encode).
pub enum HybridColourPass {
    Texture(TextureRenderElement<smithay::backend::renderer::multigpu::MultiTexture>),
    Shader(crate::hybrid_shader::HybridTexShaderElement),
}

/// Run LUT / HDR encode on a primary-GPU scene and wrap for Multi present.
#[allow(clippy::too_many_arguments)]
pub fn apply_hybrid_colour_post_pass(
    lut_runtime: &mut ColorLutRuntime,
    hdr_runtime: &mut HdrEncodeRuntime,
    renderer: &mut GlesRenderer,
    multi_context_id: smithay::backend::renderer::ContextId<MultiTexture>,
    scene: GlesTexture,
    output_name: &str,
    size: Size<i32, Physical>,
    hdr_active: bool,
    hdr_transfer: HdrTransfer,
    hdr_passthrough: bool,
    scene_linear: bool,
    content_max_nits: f32,
) -> Option<(HybridColourPass, [f32; 4])> {
    let wants_lut = lut_runtime.lut_owns_output(output_name);
    let wants_hdr_encode = hdr_active && !hdr_passthrough;
    if !wants_lut && !wants_hdr_encode {
        return None;
    }
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let size_buf: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut scene = scene;

    if wants_lut
        && let Some(mapped) = lut_runtime.apply(renderer, output_name, scene.clone(), size_buf)
    {
        scene = mapped;
    }

    if wants_hdr_encode {
        let elem = hdr_runtime.encode_scene_hybrid(
            renderer,
            scene,
            size,
            hdr_transfer,
            scene_linear,
            content_max_nits,
        )?;
        return Some((HybridColourPass::Shader(elem), HDR_CLEAR));
    }

    let multi =
        MultiTexture::from_native_texture::<UdevGbm>(&Renderer::context_id(renderer), scene)?;
    let src_rect = Rectangle::<f64, Logical>::new(
        Point::from((0.0, 0.0)),
        Size::from((size.w as f64, size.h as f64)),
    );
    let tex = TextureRenderElement::from_static_texture(
        Id::new(),
        multi_context_id,
        Point::<f64, Physical>::from((0.0, 0.0)),
        multi,
        1,
        Transform::Normal,
        None,
        Some(src_rect),
        Some(Size::from((size.w, size.h))),
        None,
        Kind::Unspecified,
    );
    Some((HybridColourPass::Texture(tex), CLEAR_COLOR))
}

fn composite_offscreen(
    renderer: &mut GlesRenderer,
    elements: &[OutputStack],
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    size_buf: Size<i32, Buffer>,
) -> Option<GlesTexture> {
    // Prefer higher bit-depth intermediates when the GLES context supports them.
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
    tracing::warn!("colour: offscreen composite failed for all formats");
    None
}
