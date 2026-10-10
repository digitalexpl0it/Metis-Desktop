//! Stage 2 colour — bake display 3D LUTs from ICC profiles (lcms2) and apply
//! them as a GLES post-pass at scanout.
//!
//! Two atlases are cached per profile:
//! - **sRGB** — classic sRGB→display (display-referred SDR / HDR encode path)
//! - **linear** — Rec.709 linear→display primaries/white with γ=1 TRC so the
//!   Mixed float path stays scene-linear for BT.2390/PQ encode (no double-TRC)
//!
//! Smithay's custom texture shaders only expose one sampler (`tex`), so the LUT
//! is applied with a small dual-sampler blit via [`GlesRenderer::with_context`]
//! into a fresh offscreen, then handed to HDR encode or direct scanout.
//!
//! Atlas layout: width = `LUT_SIZE * LUT_SIZE`, height = `LUT_SIZE`. Texel
//! `(r + g * LUT_SIZE, b)` holds the display RGB for grid point `(r, g, b)`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bytemuck::{Pod, Zeroable};
use lcms2::{
    CIExyY, CIExyYTRIPLE, Intent, PixelFormat, Profile, Tag, TagSignature, ToneCurve, Transform,
    XYZ2xyY,
};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, ffi};
use smithay::backend::renderer::{Bind, ImportMem, Offscreen};
use smithay::utils::{Buffer, Size};

/// Grid resolution per axis (33³ is the common desktop CMS trade-off).
pub const LUT_SIZE: usize = 33;

/// Rec.709 / sRGB primaries (CIE xy) and D65 white.
const D65: CIExyY = CIExyY {
    x: 0.3127,
    y: 0.3290,
    Y: 1.0,
};
const REC709_PRIMARIES: CIExyYTRIPLE = CIExyYTRIPLE {
    Red: CIExyY {
        x: 0.64,
        y: 0.33,
        Y: 1.0,
    },
    Green: CIExyY {
        x: 0.30,
        y: 0.60,
        Y: 1.0,
    },
    Blue: CIExyY {
        x: 0.15,
        y: 0.06,
        Y: 1.0,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Rgb8 {
    r: u8,
    g: u8,
    b: u8,
}

/// Persistent LUT GL program + per-output atlas cache.
#[derive(Default)]
pub struct ColorLutRuntime {
    blit: Option<LutBlitProgram>,
    /// Output name → uploaded atlases (invalidated when ICC bytes change).
    entries: HashMap<String, LutEntry>,
    /// Outputs for which a LUT bake succeeded this session (skip CRTC vcgt).
    lut_active: HashMap<String, bool>,
    /// Cached probe: float / 10-bit offscreen available (scene-linear dest).
    scene_linear_formats: Option<bool>,
}

struct LutEntry {
    icc_hash: u64,
    atlas_srgb: GlesTexture,
    atlas_linear: GlesTexture,
}

struct LutBlitProgram {
    program: ffi::types::GLuint,
    loc_scene: ffi::types::GLint,
    loc_lut: ffi::types::GLint,
    loc_lut_size: ffi::types::GLint,
    attrib_pos: ffi::types::GLint,
}

impl ColorLutRuntime {
    /// Drop cached GL atlases / blit program (e.g. after VT resume when the
    /// GLES context may have lost resources). Next `sync_profiles` rebakes.
    pub fn invalidate_gl(&mut self) {
        self.entries.clear();
        self.lut_active.clear();
        self.blit = None;
        self.scene_linear_formats = None;
    }

    /// True when a GLES LUT is ready for `output_name` (vcgt should stay identity).
    pub fn lut_owns_output(&self, output_name: &str) -> bool {
        self.lut_active.get(output_name).copied().unwrap_or(false)
    }

    /// Sync baked LUTs with the ICC bytes currently loaded for each output.
    /// Drops entries whose profile disappeared; rebakes on hash change.
    pub fn sync_profiles(
        &mut self,
        renderer: &mut GlesRenderer,
        profiles: &HashMap<String, Option<std::sync::Arc<[u8]>>>,
    ) {
        self.lut_active.clear();
        let names: Vec<String> = profiles.keys().cloned().collect();
        for name in &names {
            let Some(Some(icc)) = profiles.get(name) else {
                self.entries.remove(name);
                continue;
            };
            let hash = hash_bytes(icc);
            if self.entries.get(name).is_some_and(|e| e.icc_hash == hash) {
                self.lut_active.insert(name.clone(), true);
                continue;
            }
            match bake_and_upload_pair(renderer, icc) {
                Ok((atlas_srgb, atlas_linear)) => {
                    tracing::info!(
                        output = %name,
                        "colour: baked sRGB + linear Rec.709→display 3D LUTs"
                    );
                    self.entries.insert(
                        name.clone(),
                        LutEntry {
                            icc_hash: hash,
                            atlas_srgb,
                            atlas_linear,
                        },
                    );
                    self.lut_active.insert(name.clone(), true);
                }
                Err(err) => {
                    tracing::warn!(output = %name, %err, "colour: LUT bake failed; Stage 1 vcgt only");
                    self.entries.remove(name);
                }
            }
        }
        self.entries.retain(|k, _| profiles.contains_key(k));
    }

    /// Apply the output's LUT to `scene` (offscreen). Returns a new texture or
    /// `None` when no LUT / GL blit fails (caller keeps `scene`).
    ///
    /// When `scene_linear` is true, uses the Rec.709-linear→display (γ=1) atlas
    /// and prefers a float/10-bit dest so highlights survive for HDR encode.
    pub fn apply(
        &mut self,
        renderer: &mut GlesRenderer,
        output_name: &str,
        scene: GlesTexture,
        size: Size<i32, Buffer>,
        scene_linear: bool,
    ) -> Option<GlesTexture> {
        let entry = self.entries.get(output_name)?;
        let atlas = if scene_linear {
            entry.atlas_linear.clone()
        } else {
            entry.atlas_srgb.clone()
        };
        self.ensure_blit(renderer)?;
        let prefer_wide = scene_linear && self.scene_linear_formats_ok(renderer);
        blit_with_lut(
            renderer,
            self.blit.as_ref()?,
            &scene,
            &atlas,
            size,
            prefer_wide,
        )
    }

    fn scene_linear_formats_ok(&mut self, renderer: &mut GlesRenderer) -> bool {
        if let Some(ok) = self.scene_linear_formats {
            return ok;
        }
        let size = Size::<i32, Buffer>::from((1, 1));
        let ok = [Fourcc::Abgr16161616f, Fourcc::Abgr2101010]
            .into_iter()
            .any(|format| Offscreen::<GlesTexture>::create_buffer(renderer, format, size).is_ok());
        self.scene_linear_formats = Some(ok);
        ok
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Bake a 33³ Abgr8888 atlas: sRGB grid → display RGB via lcms2.
pub fn bake_lut_atlas(icc: &[u8]) -> Result<Vec<u8>, String> {
    let display = Profile::new_icc(icc).map_err(|e| format!("icc: {e}"))?;
    let srgb = Profile::new_srgb();
    let transform = Transform::<Rgb8, Rgb8>::new(
        &srgb,
        PixelFormat::RGB_8,
        &display,
        PixelFormat::RGB_8,
        Intent::Perceptual,
    )
    .map_err(|e| format!("transform: {e}"))?;

    fill_atlas_from_transform(&transform)
}

/// Bake a 33³ Abgr8888 atlas: Rec.709 **linear** grid → display gamut/white
/// with γ=1 TRC (matrix/primaries only). Post-LUT samples stay scene-linear.
///
/// Prefer rewriting the display profile's R/G/B TRC tags to identity so lcms
/// keeps the profile's chromatic adaptation (MediaWhitePoint is often D50 PCS).
/// Fall back to a synthetic γ=1 RGB profile from ChromaticityTag + D65 when
/// TRC rewrite is unavailable (unusual LUT-based display ICCs).
pub fn bake_lut_atlas_linear(icc: &[u8]) -> Result<Vec<u8>, String> {
    let linear = ToneCurve::new(1.0);
    let curves = [&linear, &linear, &linear];
    let src = Profile::new_rgb(&D65, &REC709_PRIMARIES, &curves)
        .map_err(|e| format!("src linear Rec.709: {e}"))?;

    let mut display = Profile::new_icc(icc).map_err(|e| format!("icc: {e}"))?;
    let dest = if linearize_display_trcs(&mut display, &linear) {
        display
    } else {
        let primaries = display_primaries_xyy(&display)?;
        Profile::new_rgb(&D65, &primaries, &curves)
            .map_err(|e| format!("dest linear display: {e}"))?
    };

    let transform = Transform::<Rgb8, Rgb8>::new(
        &src,
        PixelFormat::RGB_8,
        &dest,
        PixelFormat::RGB_8,
        Intent::RelativeColorimetric,
    )
    .map_err(|e| format!("linear transform: {e}"))?;

    fill_atlas_from_transform(&transform)
}

fn linearize_display_trcs(display: &mut Profile, linear: &ToneCurve) -> bool {
    display.write_tag(TagSignature::RedTRCTag, Tag::ToneCurve(linear))
        && display.write_tag(TagSignature::GreenTRCTag, Tag::ToneCurve(linear))
        && display.write_tag(TagSignature::BlueTRCTag, Tag::ToneCurve(linear))
}

fn display_primaries_xyy(display: &Profile) -> Result<CIExyYTRIPLE, String> {
    if let Tag::CIExyYTRIPLE(prim) = display.read_tag(TagSignature::ChromaticityTag) {
        return Ok(*prim);
    }

    let red = colorant_xyy(display, TagSignature::RedColorantTag)?;
    let green = colorant_xyy(display, TagSignature::GreenColorantTag)?;
    let blue = colorant_xyy(display, TagSignature::BlueColorantTag)?;
    Ok(CIExyYTRIPLE {
        Red: red,
        Green: green,
        Blue: blue,
    })
}

fn colorant_xyy(display: &Profile, tag: TagSignature) -> Result<CIExyY, String> {
    match display.read_tag(tag) {
        Tag::CIEXYZ(xyz) => Ok(XYZ2xyY(xyz)),
        _ => Err(format!("missing colorant tag {tag:?}")),
    }
}

fn fill_atlas_from_transform(transform: &Transform<Rgb8, Rgb8>) -> Result<Vec<u8>, String> {
    let n = LUT_SIZE;
    let w = n * n;
    let h = n;
    let mut atlas = vec![0u8; w * h * 4];
    let max = (n - 1) as f32;

    for bi in 0..n {
        for gi in 0..n {
            for ri in 0..n {
                let src = Rgb8 {
                    r: ((ri as f32 / max) * 255.0).round() as u8,
                    g: ((gi as f32 / max) * 255.0).round() as u8,
                    b: ((bi as f32 / max) * 255.0).round() as u8,
                };
                let mut dst = [Rgb8 { r: 0, g: 0, b: 0 }];
                transform.transform_pixels(&[src], &mut dst);
                let x = ri + gi * n;
                let y = bi;
                let i = (y * w + x) * 4;
                // Abgr8888 little-endian in memory: R,G,B,A for GLES RGBA upload path
                // used by Smithay's Abgr8888 import (see fourcc_to_gl_formats).
                atlas[i] = dst[0].r;
                atlas[i + 1] = dst[0].g;
                atlas[i + 2] = dst[0].b;
                atlas[i + 3] = 255;
            }
        }
    }
    Ok(atlas)
}

fn bake_and_upload_pair(
    renderer: &mut GlesRenderer,
    icc: &[u8],
) -> Result<(GlesTexture, GlesTexture), String> {
    let atlas_srgb = bake_lut_atlas(icc)?;
    let atlas_linear = bake_lut_atlas_linear(icc)?;
    let n = LUT_SIZE as i32;
    let size = Size::from((n * n, n));
    let tex_srgb = renderer
        .import_memory(&atlas_srgb, Fourcc::Abgr8888, size, false)
        .map_err(|e| format!("import sRGB LUT atlas: {e:?}"))?;
    let tex_linear = renderer
        .import_memory(&atlas_linear, Fourcc::Abgr8888, size, false)
        .map_err(|e| format!("import linear LUT atlas: {e:?}"))?;
    Ok((tex_srgb, tex_linear))
}

impl ColorLutRuntime {
    fn ensure_blit(&mut self, renderer: &mut GlesRenderer) -> Option<()> {
        if self.blit.is_some() {
            return Some(());
        }
        match renderer.with_context(|gl| unsafe { compile_lut_blit(gl) }) {
            Ok(Ok(prog)) => {
                tracing::info!("colour: compiled 3D-LUT blit shader");
                self.blit = Some(prog);
                Some(())
            }
            Ok(Err(err)) => {
                tracing::warn!(%err, "colour: LUT blit shader compile failed");
                None
            }
            Err(err) => {
                tracing::warn!(?err, "colour: GL context unavailable for LUT blit");
                None
            }
        }
    }
}

unsafe fn compile_lut_blit(gl: &ffi::Gles2) -> Result<LutBlitProgram, String> {
    unsafe {
        let vs = r#"#version 100
attribute vec2 pos;
varying vec2 v_uv;
void main() {
    v_uv = pos * 0.5 + 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;
        let fs = r#"#version 100
precision highp float;
uniform sampler2D scene;
uniform sampler2D lut;
uniform float lut_size;
varying vec2 v_uv;

vec3 sample_lut(vec3 c) {
    float max_i = lut_size - 1.0;
    vec3 scaled = clamp(c, 0.0, 1.0) * max_i;
    vec3 base = floor(scaled);
    vec3 f = scaled - base;
    float atlas_w = lut_size * lut_size;
    float atlas_h = lut_size;

    vec3 c000 = texture2D(lut, vec2((base.x + base.y * lut_size + 0.5) / atlas_w, (base.z + 0.5) / atlas_h)).rgb;
    vec3 c100 = texture2D(lut, vec2((min(base.x + 1.0, max_i) + base.y * lut_size + 0.5) / atlas_w, (base.z + 0.5) / atlas_h)).rgb;
    vec3 c010 = texture2D(lut, vec2((base.x + min(base.y + 1.0, max_i) * lut_size + 0.5) / atlas_w, (base.z + 0.5) / atlas_h)).rgb;
    vec3 c110 = texture2D(lut, vec2((min(base.x + 1.0, max_i) + min(base.y + 1.0, max_i) * lut_size + 0.5) / atlas_w, (base.z + 0.5) / atlas_h)).rgb;
    vec3 c001 = texture2D(lut, vec2((base.x + base.y * lut_size + 0.5) / atlas_w, (min(base.z + 1.0, max_i) + 0.5) / atlas_h)).rgb;
    vec3 c101 = texture2D(lut, vec2((min(base.x + 1.0, max_i) + base.y * lut_size + 0.5) / atlas_w, (min(base.z + 1.0, max_i) + 0.5) / atlas_h)).rgb;
    vec3 c011 = texture2D(lut, vec2((base.x + min(base.y + 1.0, max_i) * lut_size + 0.5) / atlas_w, (min(base.z + 1.0, max_i) + 0.5) / atlas_h)).rgb;
    vec3 c111 = texture2D(lut, vec2((min(base.x + 1.0, max_i) + min(base.y + 1.0, max_i) * lut_size + 0.5) / atlas_w, (min(base.z + 1.0, max_i) + 0.5) / atlas_h)).rgb;

    vec3 c00 = mix(c000, c100, f.x);
    vec3 c10 = mix(c010, c110, f.x);
    vec3 c01 = mix(c001, c101, f.x);
    vec3 c11 = mix(c011, c111, f.x);
    vec3 c0 = mix(c00, c10, f.y);
    vec3 c1 = mix(c01, c11, f.y);
    return mix(c0, c1, f.z);
}

void main() {
    vec4 s = texture2D(scene, v_uv);
    gl_FragColor = vec4(sample_lut(s.rgb), s.a);
}
"#;

        let program = link_program(gl, vs, fs)?;
        let loc_scene = gl.GetUniformLocation(program, c"scene".as_ptr() as *const _);
        let loc_lut = gl.GetUniformLocation(program, c"lut".as_ptr() as *const _);
        let loc_lut_size = gl.GetUniformLocation(program, c"lut_size".as_ptr() as *const _);
        let attrib_pos = gl.GetAttribLocation(program, c"pos".as_ptr() as *const _);
        if loc_scene < 0 || loc_lut < 0 || loc_lut_size < 0 || attrib_pos < 0 {
            return Err("LUT blit missing uniform/attrib".into());
        }
        Ok(LutBlitProgram {
            program,
            loc_scene,
            loc_lut,
            loc_lut_size,
            attrib_pos,
        })
    }
}

unsafe fn link_program(
    gl: &ffi::Gles2,
    vs_src: &str,
    fs_src: &str,
) -> Result<ffi::types::GLuint, String> {
    unsafe {
        let vs = compile_shader(gl, ffi::VERTEX_SHADER, vs_src)?;
        let fs = compile_shader(gl, ffi::FRAGMENT_SHADER, fs_src)?;
        let program = gl.CreateProgram();
        gl.AttachShader(program, vs);
        gl.AttachShader(program, fs);
        gl.LinkProgram(program);
        gl.DeleteShader(vs);
        gl.DeleteShader(fs);
        let mut ok = 0;
        gl.GetProgramiv(program, ffi::LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0;
            gl.GetProgramiv(program, ffi::INFO_LOG_LENGTH, &mut len);
            let mut buf = vec![0u8; len.max(1) as usize];
            gl.GetProgramInfoLog(
                program,
                len,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
            );
            gl.DeleteProgram(program);
            return Err(String::from_utf8_lossy(&buf).into_owned());
        }
        Ok(program)
    }
}

unsafe fn compile_shader(
    gl: &ffi::Gles2,
    kind: ffi::types::GLenum,
    src: &str,
) -> Result<ffi::types::GLuint, String> {
    unsafe {
        let shader = gl.CreateShader(kind);
        let ptr = src.as_ptr() as *const ffi::types::GLchar;
        let len = src.len() as ffi::types::GLint;
        gl.ShaderSource(shader, 1, &ptr, &len);
        gl.CompileShader(shader);
        let mut ok = 0;
        gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0;
            gl.GetShaderiv(shader, ffi::INFO_LOG_LENGTH, &mut len);
            let mut buf = vec![0u8; len.max(1) as usize];
            gl.GetShaderInfoLog(
                shader,
                len,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
            );
            gl.DeleteShader(shader);
            return Err(String::from_utf8_lossy(&buf).into_owned());
        }
        Ok(shader)
    }
}

fn blit_with_lut(
    renderer: &mut GlesRenderer,
    blit: &LutBlitProgram,
    scene: &GlesTexture,
    atlas: &GlesTexture,
    size: Size<i32, Buffer>,
    prefer_wide: bool,
) -> Option<GlesTexture> {
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let formats: &[Fourcc] = if prefer_wide {
        &[Fourcc::Abgr16161616f, Fourcc::Abgr2101010, Fourcc::Abgr8888]
    } else {
        &[Fourcc::Abgr8888]
    };
    let mut dest = None;
    for &format in formats {
        match Offscreen::<GlesTexture>::create_buffer(renderer, format, size) {
            Ok(t) => {
                dest = Some(t);
                break;
            }
            Err(err) => {
                if format == Fourcc::Abgr8888 {
                    tracing::warn!(?err, "colour: LUT dest offscreen failed");
                    return None;
                }
            }
        }
    }
    let mut dest = dest?;

    {
        let mut fb = match renderer.bind(&mut dest) {
            Ok(fb) => fb,
            Err(err) => {
                tracing::warn!(?err, "colour: LUT dest bind failed");
                return None;
            }
        };
        let _ = &mut fb; // keep FBO bound for the blit
        let scene_id = scene.tex_id();
        let atlas_id = atlas.tex_id();
        let w = size.w;
        let h = size.h;
        let prog = blit.program;
        let loc_scene = blit.loc_scene;
        let loc_lut = blit.loc_lut;
        let loc_lut_size = blit.loc_lut_size;
        let attrib_pos = blit.attrib_pos;

        if let Err(err) = renderer.with_context(|gl| unsafe {
            // Fullscreen triangle strip in NDC.
            let verts: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            gl.Viewport(0, 0, w, h);
            gl.Disable(ffi::BLEND);
            gl.UseProgram(prog);
            gl.Uniform1i(loc_scene, 0);
            gl.Uniform1i(loc_lut, 1);
            gl.Uniform1f(loc_lut_size, LUT_SIZE as f32);

            gl.ActiveTexture(ffi::TEXTURE0);
            gl.BindTexture(ffi::TEXTURE_2D, scene_id);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);

            gl.ActiveTexture(ffi::TEXTURE1);
            gl.BindTexture(ffi::TEXTURE_2D, atlas_id);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_S,
                ffi::CLAMP_TO_EDGE as i32,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_T,
                ffi::CLAMP_TO_EDGE as i32,
            );

            gl.EnableVertexAttribArray(attrib_pos as ffi::types::GLuint);
            gl.VertexAttribPointer(
                attrib_pos as ffi::types::GLuint,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                verts.as_ptr() as *const _,
            );
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            gl.DisableVertexAttribArray(attrib_pos as ffi::types::GLuint);

            gl.ActiveTexture(ffi::TEXTURE1);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.UseProgram(0);
        }) {
            tracing::warn!(?err, "colour: LUT blit failed");
            return None;
        }
    }

    Some(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_srgb_lut_is_near_diagonal() {
        // Empty / invalid ICC should error.
        assert!(bake_lut_atlas(&[]).is_err());
        assert!(bake_lut_atlas_linear(&[]).is_err());
    }

    #[test]
    fn linear_bake_on_srgb_icc_is_near_identity() {
        let icc = Profile::new_srgb()
            .icc()
            .expect("sRGB profile should serialize");
        let atlas = bake_lut_atlas_linear(&icc).expect("linear bake");
        let n = LUT_SIZE;
        let w = n * n;
        let max = (n - 1) as f32;
        // Sample a few diagonal + mid points; Rec.709 linear → sRGB linearized
        // (same primaries/white) is the identity matrix.
        for &(ri, gi, bi) in &[
            (0, 0, 0),
            (n - 1, n - 1, n - 1),
            (n / 2, n / 2, n / 2),
            (8, 16, 24),
        ] {
            let x = ri + gi * n;
            let y = bi;
            let i = (y * w + x) * 4;
            let expect_r = ((ri as f32 / max) * 255.0).round() as i32;
            let expect_g = ((gi as f32 / max) * 255.0).round() as i32;
            let expect_b = ((bi as f32 / max) * 255.0).round() as i32;
            let dr = (atlas[i] as i32 - expect_r).abs();
            let dg = (atlas[i + 1] as i32 - expect_g).abs();
            let db = (atlas[i + 2] as i32 - expect_b).abs();
            assert!(
                dr <= 2 && dg <= 2 && db <= 2,
                "linear identity drift at ({ri},{gi},{bi}): got {} {} {}, expect ~{expect_r} {expect_g} {expect_b}",
                atlas[i],
                atlas[i + 1],
                atlas[i + 2]
            );
        }
    }

    #[test]
    fn srgb_bake_unchanged_shape() {
        let icc = Profile::new_srgb()
            .icc()
            .expect("sRGB profile should serialize");
        let atlas = bake_lut_atlas(&icc).expect("sRGB bake");
        assert_eq!(atlas.len(), LUT_SIZE * LUT_SIZE * LUT_SIZE * 4);
        // Black stays black; white stays white on sRGB→sRGB.
        assert_eq!(&atlas[0..4], &[0, 0, 0, 255]);
        let n = LUT_SIZE;
        let w = n * n;
        let white_i = ((n - 1) * w + (n - 1) + (n - 1) * n) * 4;
        // Last blue slice, last r+g cell.
        let x = (n - 1) + (n - 1) * n;
        let y = n - 1;
        let i = (y * w + x) * 4;
        assert_eq!(i, white_i);
        assert!(atlas[i] >= 250 && atlas[i + 1] >= 250 && atlas[i + 2] >= 250);
    }
}
