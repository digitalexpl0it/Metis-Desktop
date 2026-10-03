//! Custom GLES texture shaders on Smithay [`MultiRenderer`] (Wave C).
//!
//! `MultiFrame` exposes `AsMut<GlesFrame>`, so we can call
//! `GlesFrame::render_texture_from_to` with a [`GlesTexProgram`] while the
//! element stack remains typed for [`crate::hybrid_multi::UdevMultiRenderer`].

use smithay::{
    backend::{
        drm::DrmDeviceFd,
        renderer::{
            Frame, Renderer, RendererSuper,
            element::{Element, Id, Kind, RenderElement},
            gles::{GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
            multigpu::{Error as MultiError, MultiTexture, gbm::GbmGlesBackend},
            utils::CommitCounter,
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform, user_data::UserDataMap},
};

use crate::hybrid_multi::UdevMultiRenderer;

type UdevGbm = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;
type UdevMultiError = MultiError<UdevGbm, UdevGbm>;

/// Texture + custom GLES program drawn through MultiFrame → GlesFrame.
#[derive(Debug)]
pub struct HybridTexShaderElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    src: Rectangle<f64, Buffer>,
    texture: MultiTexture,
    program: GlesTexProgram,
    uniforms: Vec<Uniform<'static>>,
    alpha: f32,
    transform: Transform,
    kind: Kind,
}

impl HybridTexShaderElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Id,
        commit: CommitCounter,
        geometry: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        texture: MultiTexture,
        program: GlesTexProgram,
        uniforms: Vec<Uniform<'_>>,
        alpha: f32,
        kind: Kind,
    ) -> Self {
        Self {
            id,
            commit,
            geometry,
            src,
            texture,
            program,
            uniforms: uniforms.into_iter().map(|u| u.into_owned()).collect(),
            alpha,
            transform: Transform::Normal,
            kind,
        }
    }

    /// Wrap a primary-GPU [`GlesTexture`] already holding scene pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn from_gles_texture(
        renderer: &GlesRenderer,
        id: Id,
        commit: CommitCounter,
        geometry: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        texture: GlesTexture,
        program: GlesTexProgram,
        uniforms: Vec<Uniform<'_>>,
        alpha: f32,
        kind: Kind,
    ) -> Option<Self> {
        let multi =
            MultiTexture::from_native_texture::<UdevGbm>(&Renderer::context_id(renderer), texture)?;
        Some(Self::new(
            id, commit, geometry, src, multi, program, uniforms, alpha, kind,
        ))
    }

    /// Wrap an existing [`MultiTexture`] (e.g. hybrid wallpaper cache).
    #[allow(clippy::too_many_arguments)]
    pub fn from_multi_texture(
        id: Id,
        commit: CommitCounter,
        geometry: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        texture: MultiTexture,
        program: GlesTexProgram,
        uniforms: Vec<Uniform<'_>>,
        alpha: f32,
        kind: Kind,
    ) -> Self {
        Self::new(
            id, commit, geometry, src, texture, program, uniforms, alpha, kind,
        )
    }
}

impl Element for HybridTexShaderElement {
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
        self.transform
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }
}

impl<'a> RenderElement<UdevMultiRenderer<'a>> for HybridTexShaderElement {
    fn draw(
        &self,
        frame: &mut <UdevMultiRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), UdevMultiError> {
        let gles_frame: &mut GlesFrame<'_, '_> = frame.as_mut();
        let render_id = Frame::context_id(gles_frame);
        let Some(texture) = self.texture.get::<UdevGbm>(&render_id) else {
            tracing::warn!("hybrid shader: MultiTexture missing primary GLES entry");
            return Ok(());
        };
        gles_frame
            .render_texture_from_to(
                &texture,
                src,
                dst,
                damage,
                opaque_regions,
                self.transform,
                self.alpha,
                Some(&self.program),
                &self.uniforms,
            )
            .map_err(MultiError::Render)
    }
}
