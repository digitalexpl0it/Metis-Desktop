//! Unified DRM colour post-pass: optional Stage 2 3D-LUT + optional HDR encode.
//!
//! Pipeline: composite elements → (prefer 10-bit / float offscreen) → LUT blit →
//! PQ or HLG encode (when HDR active) → single fullscreen scanout element.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::color_lut::ColorLutRuntime;
use crate::hdr_encode::{HDR_CLEAR, HdrEncodeRuntime, HdrTransfer, REFERENCE_WHITE_NITS};
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
    let mut scene = composite_offscreen(renderer, elements, size, scale, size_buf)?;

    if wants_lut
        && let Some(mapped) = lut_runtime.apply(renderer, output_name, scene.clone(), size_buf)
    {
        scene = mapped;
    }

    if wants_hdr_encode {
        hdr_runtime.ensure_program(renderer, hdr_transfer);
        let program = match hdr_transfer {
            HdrTransfer::Pq => hdr_runtime.pq_program.clone()?,
            HdrTransfer::Hlg => hdr_runtime.hlg_program.clone()?,
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
        let encoded = TextureShaderElement::new(
            inner,
            program,
            vec![Uniform::new("reference_white", REFERENCE_WHITE_NITS)],
        );
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
