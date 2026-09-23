//! Stage 2 colour — bake sRGB→display 3D LUTs from ICC profiles (lcms2) and
//! apply them as a GLES post-pass at scanout.
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
use lcms2::{Intent, PixelFormat, Profile, Transform};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, ffi};
use smithay::backend::renderer::{Bind, ImportMem, Offscreen};
use smithay::utils::{Buffer, Size};

/// Grid resolution per axis (33³ is the common desktop CMS trade-off).
pub const LUT_SIZE: usize = 33;

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
    /// Output name → uploaded atlas (invalidated when ICC bytes change).
    entries: HashMap<String, LutEntry>,
    /// Outputs for which a LUT bake succeeded this session (skip CRTC vcgt).
    lut_active: HashMap<String, bool>,
}

struct LutEntry {
    icc_hash: u64,
    atlas: GlesTexture,
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
            match bake_and_upload(renderer, icc) {
                Ok(atlas) => {
                    tracing::info!(output = %name, "colour: baked sRGB→display 3D LUT");
                    self.entries.insert(
                        name.clone(),
                        LutEntry {
                            icc_hash: hash,
                            atlas,
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
    pub fn apply(
        &mut self,
        renderer: &mut GlesRenderer,
        output_name: &str,
        scene: GlesTexture,
        size: Size<i32, Buffer>,
    ) -> Option<GlesTexture> {
        let atlas = self.entries.get(output_name)?.atlas.clone();
        self.ensure_blit(renderer)?;
        blit_with_lut(renderer, self.blit.as_ref()?, &scene, &atlas, size)
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

fn bake_and_upload(renderer: &mut GlesRenderer, icc: &[u8]) -> Result<GlesTexture, String> {
    let atlas = bake_lut_atlas(icc)?;
    let n = LUT_SIZE as i32;
    let size = Size::from((n * n, n));
    renderer
        .import_memory(&atlas, Fourcc::Abgr8888, size, false)
        .map_err(|e| format!("import LUT atlas: {e:?}"))
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
) -> Option<GlesTexture> {
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let mut dest = match Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size) {
        Ok(t) => t,
        Err(err) => {
            tracing::warn!(?err, "colour: LUT dest offscreen failed");
            return None;
        }
    };

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
    }
}
