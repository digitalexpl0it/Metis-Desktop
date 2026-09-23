//! Per-output ICC profiles from `outputs.json` and the `wp_color_management_v1`
//! Wayland protocol (staging). Stage 1 applies `vcgt` via CRTC gamma; Stage 2
//! bakes a GLES 3D LUT ([`crate::color_lut`]). The protocol global stays opt-in
//! (`METIS_COLOR_MGMT=1`) until a **server-side** wayland-rs ObjectData UAF fix
//! lands (0.3.16 only touched client/sys — see [`protocol::color_protocol_enabled`]
//! and <https://github.com/Smithay/wayland-rs/issues/949>).

mod protocol;
pub(crate) mod vcgt;

use std::collections::HashMap;
use std::sync::Arc;

use metis_config::OutputsConfig;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::state::MetisState;

pub use protocol::ColorManagementState;

/// Named transfer function stored for parametric image descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedTransferFunction {
    Srgb,
    Gamma22,
    St2084Pq,
    Hlg,
}

/// Named primaries stored for parametric image descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedPrimaries {
    Srgb,
    Bt2020,
}

/// Runtime colour-management state (profiles + live protocol objects).
pub struct ColorManagementRuntime {
    /// Holds the `wp_color_manager_v1` global for Wayland object lifetime.
    #[allow(dead_code)]
    pub protocol: ColorManagementState,
    /// Output name → ICC bytes (`None` = sRGB parametric default).
    profiles: HashMap<String, Option<Arc<[u8]>>>,
    /// Set when profiles change so DRM render can rebake Stage 2 LUTs.
    pub profiles_dirty: bool,
    descriptions: HashMap<u64, DescriptionRecord>,
    next_description_id: u64,
    /// `wp_color_management_output_v1` → output name (for profile reload).
    output_objects: HashMap<ObjectId, String>,
    /// wl_surface id → already has a colour-management surface.
    color_surfaces: HashMap<ObjectId, ()>,
    /// Pending surface image description (protocol state; GPU still treats as sRGB).
    surface_descriptions: HashMap<ObjectId, u64>,
    /// `wp_image_description_v1` object id → description record id.
    description_objects: HashMap<ObjectId, u64>,
}

#[derive(Clone)]
pub(crate) enum DescriptionKind {
    SrgbParametric,
    Parametric {
        tf: Option<NamedTransferFunction>,
        primaries: Option<NamedPrimaries>,
    },
    Icc(Arc<[u8]>),
}

pub(crate) struct DescriptionRecord {
    pub kind: DescriptionKind,
    /// Whether `get_information` is allowed on this description.
    pub allow_information: bool,
}

impl ColorManagementRuntime {
    pub fn new(display: &DisplayHandle) -> Self {
        Self {
            protocol: ColorManagementState::new(display),
            profiles: HashMap::new(),
            profiles_dirty: true,
            descriptions: HashMap::new(),
            next_description_id: 1,
            output_objects: HashMap::new(),
            color_surfaces: HashMap::new(),
            surface_descriptions: HashMap::new(),
            description_objects: HashMap::new(),
        }
    }

    /// Returns the registered colour-management global, if any.
    #[allow(dead_code)] // kept for global lifetime / future teardown hooks
    pub fn global(&self) -> Option<GlobalId> {
        self.protocol.global()
    }

    pub fn load_profiles(&mut self, cfg: &OutputsConfig) {
        self.profiles.clear();
        for (name, prefs) in &cfg.outputs {
            let entry = prefs.color_profile.as_ref().and_then(|path| {
                let path = std::path::Path::new(path);
                if !path.is_file() {
                    tracing::warn!(output = %name, profile = %path.display(), "ICC profile path not found");
                    return None;
                }
                match std::fs::read(path) {
                    Ok(bytes) if !bytes.is_empty() => {
                        tracing::info!(output = %name, profile = %path.display(), size = bytes.len(), "loaded ICC profile");
                        Some(Arc::from(bytes.into_boxed_slice()))
                    }
                    Ok(_) => {
                        tracing::warn!(output = %name, profile = %path.display(), "ICC profile file is empty");
                        None
                    }
                    Err(err) => {
                        tracing::warn!(output = %name, profile = %path.display(), %err, "failed to read ICC profile");
                        None
                    }
                }
            });
            self.profiles.insert(name.clone(), entry);
        }
        self.profiles_dirty = true;
    }

    pub fn profile_for_output(&self, output_name: &str) -> DescriptionKind {
        match self.profiles.get(output_name) {
            Some(Some(icc)) => DescriptionKind::Icc(Arc::clone(icc)),
            _ => DescriptionKind::SrgbParametric,
        }
    }

    /// Raw ICC bytes assigned to `output_name`, if any. Used by the hardware
    /// gamma path ([`crate::output_gamma`]) to extract the `vcgt` calibration
    /// ramp. Returns `None` when the output has no profile.
    pub fn icc_bytes_for_output(&self, output_name: &str) -> Option<Arc<[u8]>> {
        self.profiles
            .get(output_name)
            .and_then(|entry| entry.as_ref())
            .map(Arc::clone)
    }

    /// Profile map for Stage 2 LUT baking (output name → ICC bytes).
    pub fn profile_map(&self) -> &HashMap<String, Option<Arc<[u8]>>> {
        &self.profiles
    }

    /// Surface image-description record id, if the client set one via
    /// `wp_color_management_surface_v1`. HDR TF hints feed Wave 3c pass-through.
    pub fn surface_description_id(&self, surface_id: &ObjectId) -> Option<u64> {
        self.surface_descriptions.get(surface_id).copied()
    }

    /// Named TF / primaries for a surface description, when parametric.
    pub fn surface_colour_hint(
        &self,
        surface_id: &ObjectId,
    ) -> Option<(Option<NamedTransferFunction>, Option<NamedPrimaries>)> {
        let record_id = self.surface_description_id(surface_id)?;
        let record = self.description(record_id)?;
        match &record.kind {
            DescriptionKind::Parametric { tf, primaries } => Some((*tf, *primaries)),
            DescriptionKind::SrgbParametric => Some((
                Some(NamedTransferFunction::Srgb),
                Some(NamedPrimaries::Srgb),
            )),
            DescriptionKind::Icc(_) => None,
        }
    }

    pub fn alloc_description(&mut self, kind: DescriptionKind, allow_information: bool) -> u64 {
        let id = self.next_description_id;
        self.next_description_id = self.next_description_id.saturating_add(1);
        self.descriptions.insert(
            id,
            DescriptionRecord {
                kind,
                allow_information,
            },
        );
        id
    }

    pub fn register_description_object(&mut self, object_id: ObjectId, record_id: u64) {
        self.description_objects.insert(object_id, record_id);
    }

    /// Drop the bookkeeping for a destroyed `wp_image_description_v1` object.
    ///
    /// Chromium/Ozone create and destroy image descriptions constantly (reusing
    /// the freed protocol ids for other interfaces). Without this cleanup the
    /// `descriptions`/`description_objects` maps grow without bound and pin the
    /// destroyed object's `alive` `Arc` forever.
    pub fn unregister_description_object(&mut self, object_id: &ObjectId) {
        if let Some(record_id) = self.description_objects.remove(object_id) {
            self.descriptions.remove(&record_id);
        }
    }

    pub fn description_id_for_object(&self, object_id: &ObjectId) -> Option<u64> {
        self.description_objects.get(object_id).copied()
    }

    pub fn description(&self, id: u64) -> Option<&DescriptionRecord> {
        self.descriptions.get(&id)
    }

    pub fn register_output_object(&mut self, object_id: ObjectId, output_name: String) {
        self.output_objects.insert(object_id, output_name);
    }

    pub fn unregister_output_object(&mut self, object_id: &ObjectId) {
        self.output_objects.remove(object_id);
    }

    pub fn register_color_surface(&mut self, surface_id: ObjectId) {
        self.color_surfaces.insert(surface_id, ());
    }

    pub fn unregister_color_surface(&mut self, surface_id: &ObjectId) {
        self.color_surfaces.remove(surface_id);
        self.surface_descriptions.remove(surface_id);
    }

    pub fn surface_has_color_mgmt(&self, surface_id: &ObjectId) -> bool {
        self.color_surfaces.contains_key(surface_id)
    }

    pub fn set_surface_description(&mut self, surface_id: &ObjectId, record_id: u64) {
        self.surface_descriptions
            .insert(surface_id.clone(), record_id);
    }

    pub fn clear_surface_description(&mut self, surface_id: &ObjectId) {
        self.surface_descriptions.remove(surface_id);
    }

    pub fn notify_output_profiles_changed(&self, state: &MetisState) {
        use smithay::reexports::wayland_protocols::wp::color_management::v1::server::wp_color_management_output_v1::WpColorManagementOutputV1;
        use smithay::reexports::wayland_server::Resource;

        for object_id in self.output_objects.keys() {
            if let Ok(output_cm) =
                WpColorManagementOutputV1::from_id(&state.display_handle, object_id.clone())
            {
                output_cm.image_description_changed();
            }
        }
    }
}

pub fn apply_color_profiles(state: &mut MetisState, cfg: &OutputsConfig) {
    state.color_mgmt.load_profiles(cfg);
    state.color_mgmt.notify_output_profiles_changed(state);
}
