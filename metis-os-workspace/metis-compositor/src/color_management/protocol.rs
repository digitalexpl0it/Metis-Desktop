//! `wp_color_management_v1` global and object dispatch (protocol version 1).

use std::ffi::CString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::Mutex;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1, wp_color_management_surface_feedback_v1,
    wp_color_management_surface_v1, wp_color_manager_v1, wp_image_description_creator_icc_v1,
    wp_image_description_creator_params_v1, wp_image_description_info_v1, wp_image_description_v1,
};
use smithay::reexports::wayland_server::protocol::wl_surface;
use smithay::reexports::wayland_server::{
    Client, DataInit, DisplayHandle, New, Resource, WEnum, backend::GlobalId,
};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::color_management::{
    DescriptionKind, DescriptionRecord, NamedPrimaries, NamedTransferFunction,
};
use crate::state::MetisState;

/// User data for the `wp_color_manager_v1` global.
#[derive(Debug, Default, Clone, Copy)]
pub struct ColorManagerGlobalData;

/// Registered `wp_color_manager_v1` global (absent when the protocol is disabled).
#[derive(Debug)]
pub struct ColorManagementState {
    /// Keeps the Wayland global alive for the compositor lifetime.
    #[allow(dead_code)]
    global: Option<GlobalId>,
}

/// Whether to expose `wp_color_management_v1` to clients.
///
/// **Opt-in** (`METIS_COLOR_MGMT=1`). The request handlers are hardened against
/// panics (every `New<>` is initialised, all input validated) and no longer
/// leak description records, but advertising the global to Chromium/Ozone still
/// destabilises the session: a Chromium client (Cursor) reliably triggers heap
/// corruption in the compositor (`malloc_consolidate(): unaligned fastbin
/// chunk`), taking the whole session down.
///
/// Root cause (reproduced deterministically in a nested `--session` under gdb):
/// the corruption is a use-after-free of a wayland `ObjectData` `Arc` inside
/// `wayland-backend`'s **server/sys** `resource_dispatcher`, **not** in Metis
/// code. It fires when Chromium destroys a `wp_image_description_v1` and
/// immediately reuses the freed protocol id for a `wp_image_description_info_v1`
/// in the same dispatch batch. None of our `unsafe` (the ICC memfd path) runs
/// in the crashing trace — only safe handlers (`get_information` → parametric
/// info events).
///
/// Reconfirmed 2026-07-25: crates.io `wayland-backend` **0.3.16** /
/// `wayland-server` **0.31.14** only fix **client/sys** races; `server_impl` is
/// unchanged, so a dependency bump alone does not clear this path. Metis still
/// locks 0.3.15 / 0.31.13 via smithay. Tracked upstream as
/// <https://github.com/Smithay/wayland-rs/issues/949>. The global stays disabled
/// by default until a **server-side** fix lands and Chromium retest passes — then
/// flip this gate to default-on (optional `METIS_COLOR_MGMT=0` opt-out). Rejected
/// workarounds: Chromium blacklist, disabling `get_information`, or dropping
/// `use_system_lib` only for colour management.
///
/// Per-output ICC `vcgt` / Stage 2 3D-LUT ([`crate::output_gamma`],
/// [`crate::color_lut`]) are independent of this global and remain active.
/// Parse the opt-in gate used by [`color_protocol_enabled`].
///
/// `None` / empty / non-truthy values keep `wp_color_management_v1` off.
pub(crate) fn color_protocol_enabled_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        )
    })
}

pub fn color_protocol_enabled() -> bool {
    color_protocol_enabled_from(std::env::var("METIS_COLOR_MGMT").ok().as_deref())
}

impl ColorManagementState {
    pub fn new(display: &DisplayHandle) -> Self {
        let global = if color_protocol_enabled() {
            tracing::info!(
                "registering wp_color_management_v1 (METIS_COLOR_MGMT=1; experimental — can destabilise Chromium/the DRM session)"
            );
            Some(
                display.create_global::<MetisState, wp_color_manager_v1::WpColorManagerV1, _>(
                    1,
                    ColorManagerGlobalData,
                ),
            )
        } else {
            tracing::info!(
                "wp_color_management_v1 disabled — set METIS_COLOR_MGMT=1 to enable (experimental)"
            );
            None
        };
        Self { global }
    }

    #[allow(dead_code)] // accessor for global lifetime / future teardown hooks
    pub fn global(&self) -> Option<GlobalId> {
        self.global.clone()
    }
}

fn send_manager_support_events(manager: &wp_color_manager_v1::WpColorManagerV1) {
    use wp_color_manager_v1::{Feature, Primaries, RenderIntent, TransferFunction};

    manager.supported_intent(RenderIntent::Perceptual);
    manager.supported_feature(Feature::IccV2V4);
    manager.supported_feature(Feature::Parametric);
    manager.supported_tf_named(TransferFunction::Srgb);
    manager.supported_tf_named(TransferFunction::Gamma22);
    manager.supported_tf_named(TransferFunction::St2084Pq);
    manager.supported_tf_named(TransferFunction::Hlg);
    manager.supported_primaries_named(Primaries::Srgb);
    manager.supported_primaries_named(Primaries::Bt2020);
    manager.done();
}

impl GlobalDispatch2<wp_color_manager_v1::WpColorManagerV1, MetisState> for ColorManagerGlobalData {
    fn bind(
        &self,
        _state: &mut MetisState,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_color_manager_v1::WpColorManagerV1>,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        let manager = data_init.init(resource, ColorManagerGlobalData);
        send_manager_support_events(&manager);
    }
}

fn init_ready_image_description(
    state: &mut MetisState,
    data_init: &mut DataInit<'_, MetisState>,
    id: New<wp_image_description_v1::WpImageDescriptionV1>,
    record_id: u64,
) {
    let desc = data_init.init(id, ImageDescriptionData { record_id });
    state
        .color_mgmt
        .register_description_object(desc.id(), record_id);
    desc.ready(record_id as u32);
}

/// Initialise a `New` image-description object with inert user data without
/// marking it ready.
///
/// wayland-rs **panics** (aborting the compositor, which is the DRM session) if
/// a request carrying a `New<>` returns without initialising it. Creation
/// requests that cannot be satisfied (missing ICC data, missing parametric
/// fields, an unsupported feature) must therefore still initialise the object
/// here; the caller then raises the protocol error the spec mandates, which
/// terminates only the offending client.
fn init_inert_image_description(
    state: &mut MetisState,
    data_init: &mut DataInit<'_, MetisState>,
    id: New<wp_image_description_v1::WpImageDescriptionV1>,
) {
    let record_id = state
        .color_mgmt
        .alloc_description(DescriptionKind::SrgbParametric, false);
    let desc = data_init.init(id, ImageDescriptionData { record_id });
    // Register the inert object too so its `destroyed` handler reclaims the
    // record; the caller raises a protocol error immediately after, but the
    // object still lives until the client tears it down.
    state
        .color_mgmt
        .register_description_object(desc.id(), record_id);
}

impl Dispatch2<wp_color_manager_v1::WpColorManagerV1, MetisState> for ColorManagerGlobalData {
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_color_manager_v1::WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_color_manager_v1::Request::Destroy => {}
            wp_color_manager_v1::Request::GetOutput { id, output } => {
                let Some(output) = Output::from_resource(&output) else {
                    // Unknown/dead output: still initialise the New (an
                    // uninitialised New would panic and abort the compositor).
                    // The inert object simply reports the default description.
                    let _ = data_init.init(
                        id,
                        OutputColorManagementData {
                            output_name: String::new(),
                        },
                    );
                    return;
                };
                let output_cm = data_init.init(
                    id,
                    OutputColorManagementData {
                        output_name: output.name(),
                    },
                );
                state
                    .color_mgmt
                    .register_output_object(output_cm.id(), output.name());
                output_cm.image_description_changed();
            }
            wp_color_manager_v1::Request::GetSurface { id, surface } => {
                if state.color_mgmt.surface_has_color_mgmt(&surface.id()) {
                    // Initialise the New before faulting the client (an
                    // uninitialised New would panic and abort the compositor).
                    let _ = data_init.init(
                        id,
                        SurfaceColorManagementData {
                            surface: surface.clone(),
                        },
                    );
                    resource.post_error(
                        wp_color_manager_v1::Error::SurfaceExists,
                        "the surface already has a color management object".to_string(),
                    );
                    return;
                }
                state.color_mgmt.register_color_surface(surface.id());
                let _surface_cm = data_init.init(id, SurfaceColorManagementData { surface });
            }
            wp_color_manager_v1::Request::GetSurfaceFeedback { id, surface } => {
                let _feedback = data_init.init(id, SurfaceFeedbackData { surface });
            }
            wp_color_manager_v1::Request::CreateIccCreator { obj } => {
                let _creator = data_init.init(obj, IccCreatorData::default());
            }
            wp_color_manager_v1::Request::CreateParametricCreator { obj } => {
                let _creator = data_init.init(obj, ParametricCreatorData::default());
            }
            wp_color_manager_v1::Request::CreateWindowsScrgb { image_description } => {
                // We do not advertise the windows_scrgb feature, so per spec this
                // request must raise `unsupported_feature`. Initialise the New
                // object first (an uninitialised New would panic and abort the
                // compositor), then fault the client cleanly.
                init_inert_image_description(state, data_init, image_description);
                resource.post_error(
                    wp_color_manager_v1::Error::UnsupportedFeature,
                    "windows_scrgb is not supported".to_string(),
                );
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct OutputColorManagementData {
    output_name: String,
}

impl Dispatch2<wp_color_management_output_v1::WpColorManagementOutputV1, MetisState>
    for OutputColorManagementData
{
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        _resource: &wp_color_management_output_v1::WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_color_management_output_v1::Request::Destroy => {}
            wp_color_management_output_v1::Request::GetImageDescription { image_description } => {
                let kind = state.color_mgmt.profile_for_output(&self.output_name);
                // Chromium/Electron always call get_information; parametric answers are required.
                let record_id = state.color_mgmt.alloc_description(kind, true);
                init_ready_image_description(state, data_init, image_description, record_id);
            }
            _ => {}
        }
    }

    fn destroyed(
        &self,
        state: &mut MetisState,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        resource: &wp_color_management_output_v1::WpColorManagementOutputV1,
    ) {
        state.color_mgmt.unregister_output_object(&resource.id());
    }
}

#[derive(Debug, Clone, Copy)]
struct ImageDescriptionData {
    record_id: u64,
}

impl Dispatch2<wp_image_description_v1::WpImageDescriptionV1, MetisState> for ImageDescriptionData {
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_image_description_v1::WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_image_description_v1::Request::Destroy => {}
            wp_image_description_v1::Request::GetInformation { information } => {
                let Some(record) = state.color_mgmt.description(self.record_id) else {
                    // Stale record; initialise the New so it can't panic and stop.
                    let _ = data_init.init(
                        information,
                        ImageDescriptionInfoData {
                            record_id: self.record_id,
                        },
                    );
                    return;
                };
                if !record.allow_information {
                    // Initialise the New before faulting the client (an
                    // uninitialised New would panic and abort the compositor).
                    let _ = data_init.init(
                        information,
                        ImageDescriptionInfoData {
                            record_id: self.record_id,
                        },
                    );
                    resource.post_error(
                        wp_image_description_v1::Error::NoInformation,
                        "get_information not allowed on this image description".to_string(),
                    );
                    return;
                }
                let info = data_init.init(
                    information,
                    ImageDescriptionInfoData {
                        record_id: self.record_id,
                    },
                );
                send_image_description_info(state, &info, record);
            }
            _ => {}
        }
    }

    fn destroyed(
        &self,
        state: &mut MetisState,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        resource: &wp_image_description_v1::WpImageDescriptionV1,
    ) {
        state
            .color_mgmt
            .unregister_description_object(&resource.id());
    }
}

#[derive(Debug, Clone, Copy)]
struct ImageDescriptionInfoData {
    /// Dispatch user data; ties the info object to its description record.
    #[allow(dead_code)]
    record_id: u64,
}

impl Dispatch2<wp_image_description_info_v1::WpImageDescriptionInfoV1, MetisState>
    for ImageDescriptionInfoData
{
    fn request(
        &self,
        _state: &mut MetisState,
        _client: &Client,
        _resource: &wp_image_description_info_v1::WpImageDescriptionInfoV1,
        _request: wp_image_description_info_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, MetisState>,
    ) {
    }
}

/// sRGB / BT.709 primaries and D65 white (CIE 1931 xy × 1_000_000).
const SRGB_PRIMARIES_XY: [i32; 8] = [
    640_000, 330_000, 300_000, 600_000, 150_000, 60_000, 312_700, 329_000,
];

/// Typical SDR display luminance (cd/m²). `min_lum` is scaled ×10_000 per protocol.
const SDR_MIN_LUM_SCALED: u32 = 0;
const SDR_MAX_LUM: u32 = 80;
const SDR_REFERENCE_LUM: u32 = 80;

fn send_parametric_info(
    info: &wp_image_description_info_v1::WpImageDescriptionInfoV1,
    tf: Option<NamedTransferFunction>,
    primaries: Option<NamedPrimaries>,
) {
    use wp_color_manager_v1::{Primaries, TransferFunction};

    let named_p = match primaries.unwrap_or(NamedPrimaries::Srgb) {
        NamedPrimaries::Srgb => Primaries::Srgb,
        NamedPrimaries::Bt2020 => Primaries::Bt2020,
    };
    let named_tf = match tf.unwrap_or(NamedTransferFunction::Srgb) {
        NamedTransferFunction::Srgb => TransferFunction::Srgb,
        NamedTransferFunction::Gamma22 => TransferFunction::Gamma22,
        NamedTransferFunction::St2084Pq => TransferFunction::St2084Pq,
        NamedTransferFunction::Hlg => TransferFunction::Hlg,
    };

    let [r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y] = match primaries.unwrap_or(NamedPrimaries::Srgb) {
        NamedPrimaries::Srgb => SRGB_PRIMARIES_XY,
        NamedPrimaries::Bt2020 => BT2020_PRIMARIES_XY,
    };
    let (min_lum, max_lum, ref_lum) = match tf.unwrap_or(NamedTransferFunction::Srgb) {
        NamedTransferFunction::St2084Pq | NamedTransferFunction::Hlg => (0u32, 1000u32, 203u32),
        _ => (SDR_MIN_LUM_SCALED, SDR_MAX_LUM, SDR_REFERENCE_LUM),
    };

    info.primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    info.primaries_named(named_p);
    info.tf_named(named_tf);
    info.luminances(min_lum, max_lum, ref_lum);
    info.target_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    info.target_luminance(min_lum, max_lum);
}

/// BT.2020 / BT.2100 primaries and D65 (CIE 1931 xy × 1_000_000).
const BT2020_PRIMARIES_XY: [i32; 8] = [
    708_000, 292_000, 170_000, 797_000, 131_000, 46_000, 312_700, 329_000,
];

fn send_srgb_parametric_info(info: &wp_image_description_info_v1::WpImageDescriptionInfoV1) {
    send_parametric_info(
        info,
        Some(NamedTransferFunction::Srgb),
        Some(NamedPrimaries::Srgb),
    );
}

fn send_image_description_info(
    _state: &MetisState,
    info: &wp_image_description_info_v1::WpImageDescriptionInfoV1,
    record: &DescriptionRecord,
) {
    match &record.kind {
        DescriptionKind::Icc(icc) => match sealed_memfd("metis-icc", icc) {
            Some(fd) => {
                info.icc_file(fd.as_fd(), icc.len() as u32);
            }
            _ => {
                tracing::warn!("ICC memfd failed — falling back to sRGB parametric info");
                send_srgb_parametric_info(info);
            }
        },
        DescriptionKind::SrgbParametric => send_srgb_parametric_info(info),
        DescriptionKind::Parametric { tf, primaries } => {
            send_parametric_info(info, *tf, *primaries);
        }
    }
    info.done();
}

#[derive(Debug, Default)]
struct IccCreatorData {
    icc: Mutex<Option<Arc<[u8]>>>,
}

impl Dispatch2<wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1, MetisState>
    for IccCreatorData
{
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
        request: wp_image_description_creator_icc_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_image_description_creator_icc_v1::Request::SetIccFile {
                icc_profile,
                offset,
                length,
            } => {
                if let Some(bytes) = read_icc_fd(icc_profile, offset, length)
                    && let Ok(mut slot) = self.icc.lock()
                {
                    *slot = Some(bytes);
                }
            }
            wp_image_description_creator_icc_v1::Request::Create { image_description } => {
                let icc = self.icc.lock().ok().and_then(|guard| guard.clone());
                let Some(icc) = icc else {
                    // `create` before a successful `set_icc_file`: spec requires
                    // the `incomplete_set` protocol error. Initialise the New
                    // first so it can't panic, then fault the client.
                    init_inert_image_description(state, data_init, image_description);
                    resource.post_error(
                        wp_image_description_creator_icc_v1::Error::IncompleteSet,
                        "create called before a valid set_icc_file".to_string(),
                    );
                    return;
                };
                let record_id = state
                    .color_mgmt
                    .alloc_description(DescriptionKind::Icc(icc), false);
                init_ready_image_description(state, data_init, image_description, record_id);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Default)]
struct ParametricCreatorData {
    tf: Mutex<Option<wp_color_manager_v1::TransferFunction>>,
    primaries: Mutex<Option<wp_color_manager_v1::Primaries>>,
}

impl
    Dispatch2<wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1, MetisState>
    for ParametricCreatorData
{
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_image_description_creator_params_v1::Request::SetTfNamed {
                tf: WEnum::Value(tf),
            } => {
                if let Ok(mut slot) = self.tf.lock() {
                    *slot = Some(tf);
                }
            }
            wp_image_description_creator_params_v1::Request::SetPrimariesNamed {
                primaries: WEnum::Value(primaries),
            } => {
                if let Ok(mut slot) = self.primaries.lock() {
                    *slot = Some(primaries);
                }
            }
            wp_image_description_creator_params_v1::Request::Create { image_description } => {
                let tf = self.tf.lock().ok().and_then(|guard| *guard);
                let primaries = self.primaries.lock().ok().and_then(|guard| *guard);
                if tf.is_none() || primaries.is_none() {
                    // `create` before both tf + primaries were set: spec requires
                    // the `incomplete_set` protocol error. Initialise the New
                    // first so it can't panic, then fault the client.
                    init_inert_image_description(state, data_init, image_description);
                    resource.post_error(
                        wp_image_description_creator_params_v1::Error::IncompleteSet,
                        "create called before set_tf_named and set_primaries_named".to_string(),
                    );
                    return;
                }
                let named_tf = tf.and_then(map_transfer_function);
                let named_primaries = primaries.and_then(map_primaries);
                let record_id = state.color_mgmt.alloc_description(
                    DescriptionKind::Parametric {
                        tf: named_tf,
                        primaries: named_primaries,
                    },
                    true,
                );
                init_ready_image_description(state, data_init, image_description, record_id);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct SurfaceColorManagementData {
    surface: wl_surface::WlSurface,
}

impl Dispatch2<wp_color_management_surface_v1::WpColorManagementSurfaceV1, MetisState>
    for SurfaceColorManagementData
{
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_color_management_surface_v1::Request::Destroy => {}
            wp_color_management_surface_v1::Request::SetImageDescription {
                image_description,
                render_intent,
            } => {
                use wp_color_manager_v1::RenderIntent;

                if !matches!(render_intent, WEnum::Value(RenderIntent::Perceptual)) {
                    resource.post_error(
                        wp_color_management_surface_v1::Error::RenderIntent,
                        "unsupported rendering intent".to_string(),
                    );
                    return;
                }
                let Some(record_id) = state
                    .color_mgmt
                    .description_id_for_object(&image_description.id())
                else {
                    resource.post_error(
                        wp_color_management_surface_v1::Error::ImageDescription,
                        "invalid image description".to_string(),
                    );
                    return;
                };
                state
                    .color_mgmt
                    .set_surface_description(&self.surface.id(), record_id);
            }
            wp_color_management_surface_v1::Request::UnsetImageDescription => {
                state
                    .color_mgmt
                    .clear_surface_description(&self.surface.id());
            }
            _ => {}
        }
    }

    fn destroyed(
        &self,
        state: &mut MetisState,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        _resource: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
    ) {
        state
            .color_mgmt
            .unregister_color_surface(&self.surface.id());
        state
            .color_mgmt
            .clear_surface_description(&self.surface.id());
    }
}

#[derive(Debug, Clone)]
struct SurfaceFeedbackData {
    surface: wl_surface::WlSurface,
}

impl
    Dispatch2<
        wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
        MetisState,
    > for SurfaceFeedbackData
{
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        match request {
            wp_color_management_surface_feedback_v1::Request::Destroy => {}
            wp_color_management_surface_feedback_v1::Request::GetPreferred {
                image_description,
            } => {
                let record_id = preferred_description_for_surface(state, &self.surface, false);
                init_ready_image_description(state, data_init, image_description, record_id);
            }
            wp_color_management_surface_feedback_v1::Request::GetPreferredParametric {
                image_description,
            } => {
                let record_id = preferred_description_for_surface(state, &self.surface, true);
                init_ready_image_description(state, data_init, image_description, record_id);
            }
            _ => {
                let _ = resource;
            }
        }
    }
}

fn preferred_description_for_surface(
    state: &mut MetisState,
    surface: &wl_surface::WlSurface,
    parametric_only: bool,
) -> u64 {
    let output_name = state
        .windows
        .id_for_surface(surface)
        .and_then(|id| state.output_for_window(id))
        .map(|output| output.name())
        .or_else(|| state.primary_output().map(|output| output.name()))
        .unwrap_or_default();
    let kind = if parametric_only {
        DescriptionKind::SrgbParametric
    } else {
        state.color_mgmt.profile_for_output(&output_name)
    };
    state.color_mgmt.alloc_description(kind, true)
}

fn read_icc_fd(fd: OwnedFd, offset: u32, length: u32) -> Option<Arc<[u8]>> {
    let mut file = std::fs::File::from(fd);
    file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
    let len = length.min(32 * 1024 * 1024) as usize;
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(Arc::from(buf.into_boxed_slice()))
}

fn sealed_memfd(name: &str, data: &[u8]) -> Option<OwnedFd> {
    let c_name = CString::new(name).ok()?;
    let fd =
        unsafe { libc::memfd_create(c_name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return None;
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(data).ok()?;
    let _ = file.flush();
    unsafe {
        libc::fcntl(
            file.as_raw_fd(),
            libc::F_ADD_SEALS,
            libc::F_SEAL_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW,
        )
    };
    Some(file.into())
}

fn map_transfer_function(
    tf: wp_color_manager_v1::TransferFunction,
) -> Option<NamedTransferFunction> {
    use wp_color_manager_v1::TransferFunction as Tf;
    match tf {
        Tf::Srgb => Some(NamedTransferFunction::Srgb),
        Tf::Gamma22 => Some(NamedTransferFunction::Gamma22),
        Tf::St2084Pq => Some(NamedTransferFunction::St2084Pq),
        Tf::Hlg => Some(NamedTransferFunction::Hlg),
        _ => None,
    }
}

fn map_primaries(p: wp_color_manager_v1::Primaries) -> Option<NamedPrimaries> {
    use wp_color_manager_v1::Primaries as P;
    match p {
        P::Srgb => Some(NamedPrimaries::Srgb),
        P::Bt2020 => Some(NamedPrimaries::Bt2020),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default path must not advertise `wp_color_management_v1` — Chromium/Ozone
    /// binding the global triggers a wayland-rs server ObjectData UAF
    /// (`docs/upstream/wayland-rs-server-objectdata-uaf.md`).
    #[test]
    fn wp_color_management_v1_default_off() {
        assert!(
            !color_protocol_enabled_from(None),
            "unset METIS_COLOR_MGMT must keep color management disabled"
        );
        assert!(!color_protocol_enabled_from(Some("")));
        assert!(!color_protocol_enabled_from(Some("0")));
        assert!(!color_protocol_enabled_from(Some("false")));
        assert!(!color_protocol_enabled_from(Some("off")));
        // Opt-in still works when explicitly requested.
        assert!(color_protocol_enabled_from(Some("1")));
        assert!(color_protocol_enabled_from(Some("true")));
    }
}
