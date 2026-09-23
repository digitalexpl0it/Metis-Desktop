//! Standalone DRM/KMS + libseat + libinput backend.
//!
//! Runs Metis directly on the GPU/TTY as its own session. Stage G uses a Smithay
//! [`GpuManager`] and one [`BackendData`] per DRM device, so every seat GPU owns
//! its scanner, output manager, render node, notifier, and CRTC surfaces. Metis's
//! GLES-only custom elements are rendered by the output GPU's GLES renderer.
//! Cross-GPU outputs prefer a primary→secondary transfer (see [`crate::cross_gpu`])
//! so blur and the full stack composite on the primary GPU; on transfer failure
//! we fall back to the local renderer with blur disabled.

use std::collections::HashMap;
use std::time::Duration;

use input::DeviceCapability;
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, DrmSurface,
            NodeType,
            compositor::FrameFlags,
            exporter::gbm::{GbmFramebufferExporter, NodeFilter},
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma, ImportEgl, ImportMemWl,
            element::{
                Kind, RenderElementStates, default_primary_scanout_output_compare,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                utils::select_dmabuf_feedback,
            },
            gles::GlesRenderer,
            multigpu::{GpuManager, gbm::GbmGlesBackend},
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::utils::{
        OutputPresentationFeedback, surface_presentation_feedback_flags_from_states,
        surface_primary_scanout_output, update_surface_primary_scanout_output,
    },
    input::pointer::CursorImageStatus,
    output::{Mode as WlMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as DrmControlDevice, Mode, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_protocols::wp::{
            linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags,
            presentation_time::server::wp_presentation_feedback,
        },
        wayland_server::backend::GlobalId,
    },
    utils::{Clock, DeviceFd, Monotonic, Physical, Point, Scale, Time, Transform},
    wayland::{
        dmabuf::{
            DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState,
            ImportNotifier,
        },
        drm_syncobj::{DrmSyncobjState, supports_syncobj_eventfd},
        presentation::{PresentationState, Refresh},
    },
};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{DrmScanEvent, DrmScanner},
};
use xcursor::parser::Image as XCursorImage;

use crate::night_light::RenderTargetInfo;
use crate::render::{CLEAR_COLOR, OutputStack};
use crate::state::MetisState;

/// Color formats we ask the DRM compositor to consider, in preference order:
/// 10-bit first when available, falling back to plain 8-bit.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

/// Each queued frame carries the presentation feedback collected at render time
/// (`wp_presentation`). On vblank we hand this to the client with the real
/// scan-out timestamp so games can pace their frames accurately. `None` when a
/// frame carries no surfaces that requested feedback.
type FrameFeedback = Option<OutputPresentationFeedback>;
type MetisDrmOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    FrameFeedback,
    DrmDeviceFd,
>;
type MetisDrmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    FrameFeedback,
    DrmDeviceFd,
>;
type MetisGpuManager = GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdevOutputId {
    pub device: DrmNode,
    pub crtc: crtc::Handle,
}

/// Two dmabuf feedbacks per scan-out surface: the default render feedback and a
/// scanout-preferring feedback whose top tranche advertises the display's
/// directly-scannable plane formats. Sent per-surface so a fullscreen game that
/// is a primary-plane candidate is told to allocate directly-scannable buffers
/// (zero-copy direct scanout), while everything else keeps the render feedback.
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

/// Per-connector scan-out surface (one CRTC → one `Output`).
pub struct SurfaceData {
    pub device: DrmNode,
    pub render_node: DrmNode,
    pub output: Output,
    pub global: Option<GlobalId>,
    pub drm_output: MetisDrmOutput,
    pub connector: connector::Handle,
    /// Modes advertised by the connector when this output was connected.
    pub modes: Vec<Mode>,
    /// User turned this output off in Settings while the connector stays connected.
    pub user_disabled: bool,
    /// A frame is committed and awaiting its vblank; do not render again until
    /// `frame_submitted` clears this.
    pub queued: bool,
    /// Damage arrived (possibly while a frame was queued) and this surface needs
    /// to repaint at the next opportunity.
    pub pending: bool,
    /// Render/scanout dmabuf feedback for this display, `None` if the feedback
    /// could not be built (falls back to the default global feedback).
    pub dmabuf_feedback: Option<SurfaceDmabufFeedback>,
    /// Last applied `HDR_OUTPUT_METADATA` blob id (destroyed on clear/replace).
    pub hdr_metadata_blob: Option<u64>,
    /// HDR Colorspace / metadata currently applied on this connector.
    pub hdr_active: bool,
    /// Encode transfer matching the last applied HDR metadata (PQ vs HLG).
    pub hdr_transfer: crate::hdr_encode::HdrTransfer,
    /// Whether we already logged the negotiated primary-plane Fourcc.
    pub scanout_format_logged: bool,
}

/// All DRM/udev backend state, stored in `MetisState::udev`.
pub struct UdevState {
    pub session: LibSeatSession,
    pub loop_handle: LoopHandle<'static, MetisState>,
    /// Primary card node used for dmabuf globals and client GPU steering.
    pub node: DrmNode,
    /// Primary render node.
    pub render_node: DrmNode,
    /// Cached dmabuf formats for ScreenCast / image-copy-capture constraints
    /// (renderer may be briefly `None` while a frame is in flight).
    pub capture_dmabuf_formats: FormatSet,
    pub gpus: Option<MetisGpuManager>,
    pub backends: HashMap<DrmNode, BackendData>,
    pub dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    /// libinput context, retained so the session can suspend/resume it on VT
    /// switch.
    pub libinput: Option<Libinput>,
    /// Named-theme pointer cursor (DRM backend paints its own cursor).
    pub cursor: crate::cursor::XCursor,
    /// Theme name used to load [`Self::cursor`] (for on-demand resize cursors).
    pub cursor_theme: String,
    /// Cache of uploaded cursor frames, keyed by the source xcursor image.
    pub pointer_buffers: Vec<(XCursorImage, MemoryRenderBuffer)>,
    /// Cached mirror-source frame for the current damage-dispatch batch.
    pub mirror_batch: Option<crate::mirror::MirrorBatchCache>,
    /// `wp_presentation` global; hands out per-surface feedback objects and lets
    /// us report real scan-out timestamps on vblank.
    pub presentation: PresentationState,
    /// Monotonic clock used for presentation timestamps when the DRM event does
    /// not carry a hardware timestamp.
    pub clock: Clock<Monotonic>,
    /// Last GLES context used for compositor-owned textures. Switching nodes
    /// invalidates context-bound wallpaper and decoration caches.
    pub active_render_node: Option<DrmNode>,
}

pub struct BackendData {
    pub drm_output_manager: MetisDrmOutputManager,
    pub drm_scanner: DrmScanner,
    pub surfaces: HashMap<crtc::Handle, SurfaceData>,
    pub render_node: DrmNode,
    pub registration_token: RegistrationToken,
}

impl UdevState {
    pub fn surfaces(&self) -> impl Iterator<Item = &SurfaceData> {
        self.backends
            .values()
            .flat_map(|backend| backend.surfaces.values())
    }

    pub fn surfaces_mut(&mut self) -> impl Iterator<Item = &mut SurfaceData> {
        self.backends
            .values_mut()
            .flat_map(|backend| backend.surfaces.values_mut())
    }

    pub fn surface(&self, id: UdevOutputId) -> Option<&SurfaceData> {
        self.backends.get(&id.device)?.surfaces.get(&id.crtc)
    }

    pub fn surface_mut(&mut self, id: UdevOutputId) -> Option<&mut SurfaceData> {
        self.backends
            .get_mut(&id.device)?
            .surfaces
            .get_mut(&id.crtc)
    }

    pub fn output_id_by_name(&self, name: &str) -> Option<UdevOutputId> {
        self.backends.iter().find_map(|(device, backend)| {
            backend.surfaces.iter().find_map(|(crtc, surface)| {
                (surface.output.name() == name).then_some(UdevOutputId {
                    device: *device,
                    crtc: *crtc,
                })
            })
        })
    }

    pub fn output_id(&self, output: &Output) -> Option<UdevOutputId> {
        output.user_data().get::<UdevOutputId>().copied()
    }
}

#[derive(Debug, thiserror::Error)]
enum BackendError {
    #[error("no GPU found for seat")]
    NoGpu,
    #[error("failed to initialize libseat session: {0}")]
    Session(#[from] smithay::backend::session::libseat::Error),
    #[error("no device path for primary GPU node")]
    NoDevicePath,
    #[error("failed to open DRM device: {0}")]
    Open(String),
    #[error("EGL init failed: {0}")]
    Egl(#[from] smithay::backend::egl::Error),
    #[error("GLES renderer init failed: {0}")]
    Gles(#[from] smithay::backend::renderer::gles::GlesError),
}

/// Components produced by opening the primary GPU.
struct OpenedDevice {
    render_node: DrmNode,
    manager: MetisDrmOutputManager,
    drm_token: RegistrationToken,
    /// The DRM device fd, retained so we can set up `linux-drm-syncobj-v1`
    /// explicit sync against the GPU that imports client buffers.
    device_fd: DrmDeviceFd,
}

pub fn init_udev(
    event_loop: &mut EventLoop<'static, MetisState>,
    state: &mut MetisState,
) -> Result<(), Box<dyn std::error::Error>> {
    let loop_handle = event_loop.handle();

    // 1. Session: take control of the seat (DRM master + input) via libseat.
    let (session, session_notifier) = LibSeatSession::new().map_err(BackendError::Session)?;
    let seat_name = session.seat();
    tracing::info!(seat = %seat_name, "libseat session acquired");

    // 2. Pick the primary GPU (normalized to its card node for KMS).
    let node = pick_primary_gpu(&seat_name).ok_or(BackendError::NoGpu)?;
    tracing::info!(?node, "primary GPU");

    // 3. Build the renderer manager and open every GPU on this seat. Open the
    // primary first so display-only devices have a stable fallback.
    let mut gpus = GpuManager::new(GbmGlesBackend::default())
        .map_err(|e| BackendError::Open(format!("GPU manager: {e:?}")))?;
    let udev_backend = UdevBackend::new(&seat_name)
        .map_err(|e| BackendError::Open(format!("udev backend: {e}")))?;
    let mut devices: Vec<(DrmNode, std::path::PathBuf)> = udev_backend
        .device_list()
        .filter_map(|(id, path)| {
            DrmNode::from_dev_id(id)
                .ok()
                .map(|n| (to_primary_node(n), path.into()))
        })
        .collect();
    devices.sort_by_key(|(candidate, _)| *candidate != node);
    if !devices.iter().any(|(candidate, _)| *candidate == node) {
        let path = node.dev_path().ok_or(BackendError::NoDevicePath)?;
        devices.insert(0, (node, path));
    }

    let mut backends = HashMap::new();
    let mut primary_device_fd = None;
    for (device_node, path) in devices {
        match open_device(&loop_handle, &session, &mut gpus, device_node, &path) {
            Ok(opened) => {
                if device_node == node {
                    primary_device_fd = Some(opened.device_fd.clone());
                }
                backends.insert(
                    device_node,
                    BackendData {
                        drm_output_manager: opened.manager,
                        drm_scanner: DrmScanner::new(),
                        surfaces: HashMap::new(),
                        render_node: opened.render_node,
                        registration_token: opened.drm_token,
                    },
                );
            }
            Err(err) if device_node == node => return Err(err.into()),
            Err(err) => tracing::warn!(?device_node, ?err, "skipping secondary GPU"),
        }
    }
    let primary_render_node = backends
        .get(&node)
        .map(|backend| backend.render_node)
        .ok_or(BackendError::NoGpu)?;

    let mut udev = UdevState {
        session,
        loop_handle: loop_handle.clone(),
        node,
        render_node: primary_render_node,
        capture_dmabuf_formats: FormatSet::default(),
        gpus: Some(gpus),
        backends,
        dmabuf_state: None,
        libinput: None,
        cursor: {
            let (theme, size) = state.xcursor_config();
            crate::cursor::XCursor::load(theme, size)
        },
        cursor_theme: state.xcursor_config().0.to_string(),
        pointer_buffers: Vec::new(),
        mirror_batch: None,
        presentation: {
            let clock = Clock::<Monotonic>::new();
            PresentationState::new::<MetisState>(&state.display_handle, clock.id() as u32)
        },
        clock: Clock::<Monotonic>::new(),
        active_render_node: None,
    };

    // 4. dmabuf global from the primary renderer's formats so EGL/GPU clients
    //    (GTK) can submit hardware buffers; also bind wl_drm for legacy EGL.
    if let Some(gpus) = udev.gpus.as_mut()
        && let Ok(mut renderer) = gpus.single_renderer(&udev.render_node)
    {
        let renderer = renderer.as_mut();
        if let Err(err) = renderer.bind_wl_display(&state.display_handle) {
            tracing::info!(?err, "wl_drm (EGL) bind unavailable");
        }
        let dmabuf_formats = renderer.dmabuf_formats();
        udev.capture_dmabuf_formats = dmabuf_formats.clone();
        if let Ok(default_feedback) =
            DmabufFeedbackBuilder::new(udev.render_node.dev_id(), dmabuf_formats).build()
        {
            let mut dmabuf_state = DmabufState::new();
            let global = dmabuf_state.create_global_with_default_feedback::<MetisState>(
                &state.display_handle,
                &default_feedback,
            );
            udev.dmabuf_state = Some((dmabuf_state, global));
            tracing::info!("dmabuf global created");
        }
        let shm_formats = renderer.shm_formats();
        state.shm_state.update_formats(shm_formats);
    }

    // 5. libinput: feed real input devices into the shared, backend-agnostic
    //    `process_input_event`.
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        udev.session.clone().into(),
    );
    if libinput_context.udev_assign_seat(&seat_name).is_err() {
        tracing::warn!("failed to assign udev seat to libinput");
    }
    udev.libinput = Some(libinput_context.clone());
    let libinput_backend = LibinputInputBackend::new(libinput_context);
    loop_handle
        .insert_source(libinput_backend, move |mut event, _, state| {
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard)
                    && let Some(led_state) = state
                        .seat
                        .get_keyboard()
                        .map(|keyboard| keyboard.led_state())
                {
                    device.led_update(led_state.into());
                }
                if device.has_capability(DeviceCapability::Touch) {
                    state.ensure_touch_device();
                }
                state.input_runtime.on_device_added(device.clone());
                // Keyboards (and keypad hotplug) may arrive after seat setup —
                // re-evaluate Auto Num Lock when a device appears.
                let pref = state.input_runtime.cached().keyboard.num_lock;
                crate::device_input::apply_num_lock(state, pref);
            } else if let InputEvent::DeviceRemoved { device } = &event {
                state.input_runtime.on_device_removed(device);
            }
            if let Some(device) = crate::device_input::libinput_device_from_event(&event) {
                state.input_runtime.note_pointer_device(&device);
            }
            state.process_input_event(event);
        })
        .map_err(|e| BackendError::Open(format!("libinput source: {e}")))?;

    // 6. Register session (VT switch / suspend) events.
    loop_handle
        .insert_source(session_notifier, move |event, _, state| {
            state.on_session_event(event);
        })
        .map_err(|e| BackendError::Open(format!("session source: {e}")))?;

    // 6. Register udev hotplug source (GPU add/remove + connector changes).
    loop_handle
        .insert_source(udev_backend, move |event, _, state| {
            state.on_udev_event(event);
        })
        .map_err(|e| BackendError::Open(format!("udev source: {e}")))?;

    // The DRM backend is driven by the housekeeping heartbeat + vblank, not by a
    // host redraw request, so the redraw trigger is a no-op (damage is coalesced
    // by the 16ms tick below).
    state.set_redraw_trigger(std::rc::Rc::new(|| {}));

    // Steer spawned clients (games, XWayland, Vulkan apps) onto the same GPU the
    // compositor renders on. Resolved from the render node's PCI identity; None
    // for exotic nodes, in which case no GPU env is exported.
    state.client_gpu = crate::state::ClientGpuHint::from_render_node(&udev.render_node);
    if let Some(hint) = &state.client_gpu {
        tracing::info!(?hint, "client GPU steering env resolved");
    }
    // On a hybrid (Optimus) laptop the compositor renders on the iGPU that owns
    // the panel; detect a discrete GPU so games / Steam Big Picture can be
    // PRIME-offloaded onto it instead of being pinned to the weak iGPU.
    state.dgpu_offload = crate::state::DgpuOffload::detect(&udev.render_node);
    match &state.dgpu_offload {
        Some(offload) => tracing::info!(
            ?offload,
            "discrete GPU detected — game/launcher processes will be offloaded to it"
        ),
        None => tracing::info!("no discrete GPU detected; all clients use the display GPU"),
    }

    state.udev = Some(udev);

    // 6b. Explicit sync (`linux-drm-syncobj-v1`). Advertise it only when the
    //     primary GPU supports syncobj eventfd (otherwise we can't build the
    //     acquire-fence blocker). With it, NVIDIA + DXVK/VKD3D and modern
    //     XWayland negotiate explicit fences instead of implicit sync, which
    //     removes the tell-tale Proton stutter/glitching on this hardware.
    if primary_device_fd
        .as_ref()
        .is_some_and(supports_syncobj_eventfd)
    {
        let Some(device_fd) = primary_device_fd else {
            return Err(BackendError::NoGpu.into());
        };
        state.drm_syncobj_state = Some(DrmSyncobjState::new::<MetisState>(
            &state.display_handle,
            device_fd,
        ));
        tracing::info!("linux-drm-syncobj-v1 explicit sync enabled");
    } else {
        tracing::info!("GPU lacks syncobj eventfd; explicit sync disabled");
    }

    // 7. Bring up every currently-connected output.
    state.scan_connectors();

    // 8. Housekeeping heartbeat. This is NO LONGER the frame pacer — high-refresh
    //    rendering is vblank-driven (a damaged surface repaints on its next
    //    vblank; see `schedule_redraw` + `on_drm_vblank`). This tick only:
    //      * runs shared housekeeping (space refresh, client flush, cleanup), and
    //      * kicks the *first* frame out of an idle state (nothing queued, no
    //        vblank pending), bounding idle→first-paint latency to one tick.
    //    Because the fast path bypasses it, this interval no longer caps FPS.
    loop_handle.insert_source(
        Timer::from_duration(Duration::from_millis(16)),
        move |_, _, state| {
            state.tick_housekeeping();
            state.drm_dispatch_damage();
            TimeoutAction::ToDuration(Duration::from_millis(16))
        },
    )?;

    state.damaged = true;
    tracing::info!("DRM/udev backend initialized");
    Ok(())
}

/// Find the GPU to drive the session, normalized to its card (Primary) node so
/// it can be opened as DRM master.
///
/// `METIS_DRM_DEVICE` forces a choice. Otherwise we **rank all GPUs by whether
/// they actually have a connected output** (and prefer the `boot_vga` device on a
/// tie), rather than trusting udev's `primary_gpu()`. This is essential on hybrid
/// laptops: smithay's `primary_gpu()` often returns the discrete NVIDIA GPU,
/// whose KMS is flaky and which usually has *no* connected panel — the eDP is
/// wired to the Intel iGPU. Driving the GPU that owns the connected display gives
/// a stable session.
fn pick_primary_gpu(seat: &str) -> Option<DrmNode> {
    if let Ok(var) = std::env::var("METIS_DRM_DEVICE") {
        if let Ok(node) = DrmNode::from_path(&var) {
            tracing::info!(%var, "using METIS_DRM_DEVICE");
            return Some(to_primary_node(node));
        }
        tracing::warn!(%var, "METIS_DRM_DEVICE invalid — autodetecting");
    }

    let mut candidates: Vec<DrmNode> = all_gpus(seat)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|p| DrmNode::from_path(p).ok())
        .map(to_primary_node)
        .collect();

    // Ensure udev's notion of the primary GPU is at least in the running.
    if let Some(p) = primary_gpu(seat)
        .ok()
        .flatten()
        .and_then(|p| DrmNode::from_path(p).ok())
    {
        let p = to_primary_node(p);
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    }

    // Higher score wins: a connected output dominates; boot_vga breaks ties.
    let best = candidates.into_iter().max_by_key(gpu_rank);
    if let Some(node) = best {
        tracing::info!(
            ?node,
            has_output = gpu_has_connected_output(node),
            "selected primary GPU"
        );
        return Some(node);
    }
    None
}

/// Normalize a DRM node to its card (Primary) node for KMS/DRM-master use.
fn to_primary_node(node: DrmNode) -> DrmNode {
    node.node_with_type(NodeType::Primary)
        .and_then(|r| r.ok())
        .unwrap_or(node)
}

/// Rank a GPU for session use: connected output is worth far more than being the
/// boot VGA device, so an iGPU driving the panel beats an idle dGPU.
fn gpu_rank(node: &DrmNode) -> i32 {
    let mut score = 0;
    if gpu_has_connected_output(*node) {
        score += 100;
    }
    if gpu_is_boot_vga(*node) {
        score += 10;
    }
    score
}

/// The sysfs DRM card directory for a node (e.g. `/sys/class/drm/card2`), derived
/// from its device path (`/dev/dri/card2`).
fn gpu_sysfs_dir(node: DrmNode) -> Option<std::path::PathBuf> {
    let path = node.dev_path()?;
    let name = path.file_name()?.to_str()?;
    Some(std::path::PathBuf::from("/sys/class/drm").join(name))
}

/// True if any connector on this GPU reports `connected` (i.e. it owns a live
/// display). Reads `…/cardN-*/status` from sysfs.
fn gpu_has_connected_output(node: DrmNode) -> bool {
    let Some(dir) = gpu_sysfs_dir(node) else {
        return false;
    };
    let card = match dir.file_name().and_then(|n| n.to_str()) {
        Some(c) => c.to_string(),
        None => return false,
    };
    let Ok(entries) = std::fs::read_dir(
        dir.parent()
            .unwrap_or(std::path::Path::new("/sys/class/drm")),
    ) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Connectors are named like "card2-eDP-1"; skip the bare "card2" dir.
        if !name.starts_with(&format!("{card}-")) {
            continue;
        }
        if let Ok(status) = std::fs::read_to_string(entry.path().join("status"))
            && status.trim() == "connected"
        {
            return true;
        }
    }
    false
}

/// True if this GPU is the firmware boot VGA device (sysfs `boot_vga`).
fn gpu_is_boot_vga(node: DrmNode) -> bool {
    gpu_sysfs_dir(node)
        .and_then(|dir| std::fs::read_to_string(dir.join("device/boot_vga")).ok())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn open_device(
    loop_handle: &LoopHandle<'static, MetisState>,
    session: &LibSeatSession,
    gpus: &mut MetisGpuManager,
    node: DrmNode,
    path: &std::path::Path,
) -> Result<OpenedDevice, BackendError> {
    let mut session = session.clone();
    let fd = session
        .open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| BackendError::Open(format!("libseat open {path:?}: {e}")))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, drm_notifier) =
        DrmDevice::new(fd.clone(), true).map_err(|e| BackendError::Open(format!("drm: {e}")))?;
    let device_fd = fd.clone();
    let gbm = GbmDevice::new(fd).map_err(|e| BackendError::Open(format!("gbm: {e}")))?;

    // EGL + GLES renderer on this GPU.
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let render_node = EGLDevice::device_for_display(&egl_display)
        .ok()
        .and_then(|d| d.try_get_render_node().ok().flatten())
        .unwrap_or(node);
    // Register the GBM device with the manager; it owns the GLES context.
    gpus.as_mut()
        .add_node(render_node, gbm.clone())
        .map_err(BackendError::Egl)?;
    let mut renderer = gpus
        .single_renderer(&render_node)
        .map_err(|e| BackendError::Open(format!("renderer for {render_node}: {e:?}")))?;

    let render_formats = renderer
        .as_mut()
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect::<FormatSet>();

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), NodeFilter::from(Some(render_node)));

    let manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        SUPPORTED_FORMATS.iter().copied(),
        render_formats,
    );

    // VBlank / DRM error notifier.
    let drm_token = loop_handle
        .insert_source(drm_notifier, move |event, meta, state| match event {
            DrmEvent::VBlank(crtc) => state.on_drm_vblank(node, crtc, meta.take()),
            DrmEvent::Error(err) => tracing::warn!(?err, "DRM error"),
        })
        .map_err(|e| BackendError::Open(format!("drm notifier source: {e}")))?;

    Ok(OpenedDevice {
        render_node,
        manager,
        drm_token,
        device_fd,
    })
}

/// Build the render + scanout dmabuf feedbacks for a display surface. The
/// scanout tranche advertises the plane formats the display can scan out
/// directly, limited to formats we can also render from so there is always a
/// fallback path when a client buffer can't be promoted to a plane.
fn build_surface_dmabuf_feedback(
    render_node: DrmNode,
    render_formats: FormatSet,
    surface: &DrmSurface,
) -> Option<SurfaceDmabufFeedback> {
    let planes = surface.planes().clone();
    let planes_formats = surface
        .plane_info()
        .formats
        .iter()
        .copied()
        .chain(planes.overlay.into_iter().flat_map(|p| p.formats))
        .collect::<FormatSet>()
        .intersection(&render_formats)
        .copied()
        .collect::<FormatSet>();

    let builder = DmabufFeedbackBuilder::new(render_node.dev_id(), render_formats);
    let render_feedback = builder.clone().build().ok()?;
    let scanout_feedback = builder
        .add_preference_tranche(
            surface.device_fd().dev_id().ok()?,
            TrancheFlags::Scanout,
            planes_formats,
            // Feedback (and so scanout tranches) exists from dmabuf v4.
            4u32..=6,
        )
        .build()
        .ok()?;

    Some(SurfaceDmabufFeedback {
        render_feedback,
        scanout_feedback,
    })
}

/// Composite + scan out on the output's local GPU (non-transfer path).
#[allow(clippy::too_many_arguments)]
fn render_local_output_frame(
    state: &mut MetisState,
    gpus: &mut MetisGpuManager,
    render_node: DrmNode,
    primary_gpu: DrmNode,
    id: UdevOutputId,
    output: &Output,
    frame_states: &mut Option<RenderElementStates>,
    cross_gpu: bool,
) -> Result<bool, String> {
    let _ = primary_gpu;
    let mut renderer_guard = gpus
        .single_renderer(&render_node)
        .map_err(|err| format!("renderer unavailable for output: {err:?}"))?;
    let renderer = renderer_guard.as_mut();
    if cross_gpu {
        tracing::trace!(
            ?render_node,
            "using output GPU renderer; cross-GPU blur disabled"
        );
    }
    if let Some(s) = state.udev.as_mut().and_then(|u| u.surface_mut(id)) {
        s.pending = false;
    }

    let scale = Scale::from(output.current_scale().fractional_scale());
    let origin: Point<i32, Physical> = state
        .space
        .output_geometry(output)
        .map(|g| g.loc.to_physical_precise_round(scale))
        .unwrap_or_default();

    let mut elements = state.build_render_elements(
        renderer,
        origin,
        scale,
        RenderTargetInfo {
            size: output.current_mode().map(|m| m.size).unwrap_or_default(),
            output_name: Some(output.name().as_str()),
            skip_night_light: false,
        },
        &[],
        !cross_gpu,
    );

    let cursor = state.build_cursor_elements(renderer, output, scale);
    if !cursor.is_empty() {
        let mut stacked = cursor;
        stacked.append(&mut elements);
        elements = stacked;
    }

    crate::output_vrr::prepare_vrr_for_render(state, id);
    crate::output_hdr::maybe_log_scanout_format(state, id);

    let (hdr_active, hdr_transfer) = state
        .udev
        .as_ref()
        .and_then(|u| u.surface(id))
        .map(|s| (s.hdr_active, s.hdr_transfer))
        .unwrap_or((false, crate::hdr_encode::HdrTransfer::Pq));

    let output_name = output.name();
    if state.color_mgmt.profiles_dirty {
        let profiles = state.color_mgmt.profile_map().clone();
        state.color_lut.sync_profiles(renderer, &profiles);
        state.color_mgmt.profiles_dirty = false;
        crate::output_gamma::apply_output_gamma(state);
    }

    let (frame_elements, clear): (Vec<OutputStack>, [f32; 4]) = {
        state.refresh_hdr_content_flag();
        let passthrough = hdr_active && state.hdr_client_content_visible;
        if passthrough {
            tracing::trace!(
                output = %output_name,
                "HDR client content visible — skipping SDR→HDR encode (pass-through)"
            );
        }
        match crate::output_colour::apply_colour_post_pass(
            &mut state.color_lut,
            &mut state.hdr_encode,
            renderer,
            &elements,
            &output_name,
            output.current_mode().map(|m| m.size).unwrap_or_default(),
            scale,
            hdr_active,
            hdr_transfer,
            passthrough,
        ) {
            Some(pass) => (pass.elements, pass.clear),
            _ => (elements, CLEAR_COLOR),
        }
    };

    let surface = state
        .udev
        .as_mut()
        .and_then(|udev| udev.surface_mut(id))
        .ok_or_else(|| "surface gone during local render".to_string())?;
    match surface
        .drm_output
        .render_frame(renderer, &frame_elements, clear, FrameFlags::DEFAULT)
    {
        Ok(res) => {
            let empty = res.is_empty;
            *frame_states = Some(res.states);
            Ok(!empty)
        }
        Err(err) => Err(format!("{err:?}")),
    }
}

impl MetisState {
    /// Scan the device's connectors and bring up / tear down outputs.
    pub(crate) fn scan_connectors(&mut self) {
        let scans = {
            let Some(udev) = self.udev.as_mut() else {
                return;
            };
            let mut scans = Vec::new();
            for (node, backend) in &mut udev.backends {
                match backend
                    .drm_scanner
                    .scan_connectors(backend.drm_output_manager.device())
                {
                    Ok(scan) => scans.extend(scan.into_iter().map(|event| (*node, event))),
                    Err(err) => {
                        tracing::warn!(?node, ?err, "connector scan failed");
                    }
                }
            }
            scans
        };
        for (node, event) in scans {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_connected(node, connector, crtc),
                DrmScanEvent::Disconnected {
                    connector: _,
                    crtc: Some(crtc),
                } => self.connector_disconnected(node, crtc),
                _ => {}
            }
        }
        // Re-tile windows/layers after the output set changed.
        self.retile_outputs();
    }

    fn connector_connected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        if self
            .udev
            .as_ref()
            .and_then(|u| u.backends.get(&node))
            .map(|backend| backend.surfaces.contains_key(&crtc))
            .unwrap_or(true)
        {
            return;
        }

        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );

        let modes: Vec<Mode> = connector.modes().to_vec();
        // First pass: pick a mode from any existing prefs (or EDID preferred).
        let preliminary = {
            let cfg = self.output_runtime.cached().clone();
            let prefs = metis_config::output_prefs(&cfg, &name);
            crate::output_modes::pick_drm_mode_index(&modes, &prefs)
        };
        let Some(drm_mode) = modes.get(preliminary).copied() else {
            tracing::warn!(%name, "connector has no modes");
            return;
        };
        let preferred = drm_mode
            .mode_type()
            .contains(smithay::reexports::drm::control::ModeTypeFlags::PREFERRED);
        let mode_info = crate::output_modes::drm_mode_info(drm_mode, preferred);

        // Persist extend + preferred mode + right-of-primary layout for first
        // plugs; restore saved layout/mode on replug.
        let cfg = crate::output_prefs::persist_hotplug_connect(self, &name, &mode_info);
        let prefs = metis_config::output_prefs(&cfg, &name);
        let mode_id = crate::output_modes::pick_drm_mode_index(&modes, &prefs);
        let Some(drm_mode) = modes.get(mode_id).copied() else {
            tracing::warn!(%name, "connector has no modes");
            return;
        };
        let wl_mode = WlMode::from(drm_mode);

        let (make, model) = {
            let Some(udev) = self.udev.as_ref() else {
                tracing::warn!(%name, "connector setup: udev state missing");
                return;
            };
            let Some(backend) = udev.backends.get(&node) else {
                return;
            };
            let drm_device = backend.drm_output_manager.device();
            let info = display_info::for_connector(drm_device, connector.handle());
            (
                info.as_ref()
                    .and_then(|i| i.make())
                    .unwrap_or_else(|| "Unknown".into()),
                info.as_ref()
                    .and_then(|i| i.model())
                    .unwrap_or_else(|| "Unknown".into()),
            )
        };
        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));

        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make,
                model,
                serial_number: name.clone(),
            },
        );
        let global = output.create_global::<MetisState>(&self.display_handle);

        // Place the output using saved layout or auto-pack to the right of primary.
        let cfg = self.output_runtime.cached().clone();
        let position = crate::output_prefs::output_position_for_connect(self, &cfg, &name);
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), Some(Transform::Normal), None, Some(position));
        self.space.map_output(&output, position);

        let planes = {
            let Some(udev) = self.udev.as_ref() else {
                tracing::warn!(%name, "connector planes: udev state missing");
                self.space.unmap_output(&output);
                return;
            };
            udev.backends
                .get(&node)
                .and_then(|backend| backend.drm_output_manager.device().planes(&crtc).ok())
        };
        let drm_output = {
            let Some(udev) = self.udev.as_mut() else {
                tracing::warn!(%name, "connector DRM init: udev state missing");
                self.space.unmap_output(&output);
                return;
            };
            let UdevState { gpus, backends, .. } = udev;
            let Some(backend) = backends.get_mut(&node) else {
                self.space.unmap_output(&output);
                return;
            };
            let render_node = backend.render_node;
            let Some(gpus) = gpus.as_mut() else {
                tracing::error!("GPU manager unavailable for connector setup");
                self.space.unmap_output(&output);
                return;
            };
            let Ok(mut renderer) = gpus.single_renderer(&render_node) else {
                tracing::error!("no renderer for connector setup");
                self.space.unmap_output(&output);
                return;
            };
            backend
                .drm_output_manager
                .lock()
                .initialize_output::<GlesRenderer, crate::render::OutputStack>(
                    crtc,
                    drm_mode,
                    &[connector.handle()],
                    &output,
                    planes,
                    renderer.as_mut(),
                    &DrmOutputRenderElements::default(),
                )
        };

        let drm_output = match drm_output {
            Ok(o) => o,
            Err(err) => {
                tracing::warn!(?err, %name, "failed to initialize DRM output");
                self.space.unmap_output(&output);
                return;
            }
        };

        // Per-surface dmabuf feedback (render + scanout tranche) so fullscreen
        // clients can allocate directly-scannable buffers on this display.
        let dmabuf_feedback = match self.udev.as_mut() {
            Some(udev) => {
                let render_node = udev.backends.get(&node).map(|backend| backend.render_node);
                let render_formats = render_node.and_then(|render_node| {
                    udev.gpus
                        .as_mut()?
                        .single_renderer(&render_node)
                        .ok()
                        .map(|renderer| renderer.dmabuf_formats())
                });
                render_node
                    .zip(render_formats)
                    .and_then(|(render_node, formats)| {
                        drm_output.with_compositor(|compositor| {
                            build_surface_dmabuf_feedback(
                                render_node,
                                formats,
                                compositor.surface(),
                            )
                        })
                    })
            }
            None => {
                tracing::warn!(%name, "dmabuf feedback: udev state missing");
                None
            }
        };

        output
            .user_data()
            .insert_if_missing(|| UdevOutputId { device: node, crtc });
        let render_node = self
            .udev
            .as_ref()
            .and_then(|udev| udev.backends.get(&node))
            .map(|backend| backend.render_node)
            .unwrap_or(
                self.udev
                    .as_ref()
                    .map(|udev| udev.render_node)
                    .unwrap_or(node),
            );
        if self
            .udev
            .as_ref()
            .is_some_and(|udev| udev.render_node != render_node)
        {
            tracing::info!(
                output = %name,
                primary_gpu = ?self.udev.as_ref().map(|udev| udev.render_node),
                ?render_node,
                "cross-GPU output: will try primary→secondary transfer (local+no-blur fallback)"
            );
        }
        let Some(backend) = self
            .udev
            .as_mut()
            .and_then(|udev| udev.backends.get_mut(&node))
        else {
            self.space.unmap_output(&output);
            return;
        };
        backend.surfaces.insert(
            crtc,
            SurfaceData {
                device: node,
                render_node,
                output: output.clone(),
                global: Some(global),
                drm_output,
                connector: connector.handle(),
                modes,
                user_disabled: false,
                queued: false,
                pending: true,
                dmabuf_feedback,
                hdr_metadata_blob: None,
                hdr_active: false,
                hdr_transfer: crate::hdr_encode::HdrTransfer::default(),
                scanout_format_logged: false,
            },
        );
        tracing::info!(%name, ?position, "output connected");

        self.ensure_desk_for_output(&output);
        let cfg = self.output_runtime.cached().clone();
        crate::output_prefs::apply_outputs(self, &cfg);
        self.damaged = true;

        let make = output.physical_properties().make.clone();
        let model = output.physical_properties().model.clone();
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::OutputHotplug {
                connected: true,
                name: name.clone(),
                make,
                model,
            });
    }

    fn connector_disconnected(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let removed = self
            .udev
            .as_mut()
            .and_then(|u| u.backends.get_mut(&node))
            .and_then(|backend| backend.surfaces.remove(&crtc));
        if let Some(mut surface) = removed {
            let output = surface.output.clone();
            let name = output.name();
            let make = output.physical_properties().make.clone();
            let model = output.physical_properties().model.clone();
            // Remember live position/mode before unmap so the next plug restores
            // the last working arrangement (e.g. above primary after a drag).
            crate::output_prefs::persist_output_snapshot(self, &output);
            // Move windows off this output before it disappears — otherwise they
            // keep dead geometry and Metis SSD can lose title text / controls.
            if let Some(fallback) = self.fallback_output_key_excluding(&name) {
                let fallback = fallback.clone();
                self.evacuate_output(&name, &fallback);
            }
            if let Some(global) = surface.global.take() {
                self.display_handle.remove_global::<MetisState>(global);
            }
            self.space.unmap_output(&output);
            tracing::info!(output = %name, "output disconnected");
            // Refresh wallpaper/layout for the remaining outputs without forcing
            // a mode poke on every surviving connector.
            let cfg = self.output_runtime.cached().clone();
            crate::output_prefs::apply_outputs(self, &cfg);
            // Drop cached SSD textures so the next frame rebuilds title + buttons
            // against the remaining output's GL context.
            self.decorations.clear_texture_caches();
            self.decorations.invalidate_all();
            self.nudge_clients_after_output_change();
            self.damaged = true;
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::OutputHotplug {
                    connected: false,
                    name,
                    make,
                    model,
                });
        }
    }

    /// Damage-gated render dispatch from the heartbeat. Propagates the global
    /// `damaged` flag onto every surface, then renders each surface that needs a
    /// frame and is not already waiting on a vblank.
    pub(crate) fn drm_dispatch_damage(&mut self) {
        if self.udev.is_none() {
            return;
        }
        // While the screen is blanked (DPMS off) do no scan-out: page-flipping to
        // a powered-down connector can withhold the vblank and wedge the surface
        // as permanently `queued`. Still run housekeeping so client bookkeeping
        // keeps ticking; the wake path sets `damaged` to force a fresh repaint.
        if self.idle.is_blanked() {
            self.space.refresh();
            self.cleanup_destroyed_windows();
            self.popups.cleanup();
            let outputs: Vec<Output> = self.space.outputs().cloned().collect();
            for out in &outputs {
                smithay::desktop::layer_map_for_output(out).cleanup();
            }
            self.defer_client_flush = true;
            return;
        }
        if self.damaged {
            self.damaged = false;
            if let Some(udev) = self.udev.as_mut() {
                udev.mirror_batch = None;
                for surface in udev.surfaces_mut() {
                    if !surface.user_disabled {
                        surface.pending = true;
                    }
                }
            }
        }
        // While a portal screencast holds an image-copy session, keep repainting
        // so capture frames are produced even when the desktop is visually static.
        if (self.image_capture.screencast_active() || self.image_capture.has_pending())
            && let Some(udev) = self.udev.as_mut()
        {
            for surface in udev.surfaces_mut() {
                if !surface.user_disabled {
                    surface.pending = true;
                }
            }
        }
        let outputs: Vec<UdevOutputId> = self
            .udev
            .as_ref()
            .map(|u| {
                u.backends
                    .iter()
                    .flat_map(|(device, backend)| {
                        backend
                            .surfaces
                            .iter()
                            .filter(|(_, s)| !s.user_disabled && s.pending && !s.queued)
                            .map(|(crtc, _)| UdevOutputId {
                                device: *device,
                                crtc: *crtc,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        for id in outputs {
            self.render_surface(id);
        }

        // Housekeeping that the winit backend does in its Redraw handler.
        self.space.refresh();
        self.cleanup_destroyed_windows();
        self.popups.cleanup();
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for out in &outputs {
            smithay::desktop::layer_map_for_output(out).cleanup();
        }
        self.defer_client_flush = true;
    }

    /// VBlank: the queued frame scanned out. Recycle buffers, report the real
    /// scan-out time to `wp_presentation` clients, and repaint if more damage
    /// accumulated while the frame was in flight.
    pub(crate) fn on_drm_vblank(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        metadata: Option<DrmEventMetadata>,
    ) {
        let (still_pending, feedback, output, now, vrr_active) = {
            let Some(udev) = self.udev.as_mut() else {
                return;
            };
            let Some(surface) = udev
                .backends
                .get_mut(&node)
                .and_then(|backend| backend.surfaces.get_mut(&crtc))
            else {
                return;
            };
            surface.queued = false;
            // `frame_submitted` recycles the just-scanned-out buffer and returns
            // the presentation feedback we attached at queue time.
            let feedback = match surface.drm_output.frame_submitted() {
                Ok(user_data) => user_data.flatten(),
                Err(err) => {
                    tracing::warn!(?err, "frame_submitted failed");
                    None
                }
            };
            let vrr_active = surface.drm_output.with_compositor(|c| c.vrr_enabled());
            let output = surface.output.clone();
            let now = udev.clock.now();
            (surface.pending, feedback, output, now, vrr_active)
        };

        // Report the frame to clients that requested presentation feedback. Prefer
        // the hardware vblank timestamp/sequence from the DRM event; fall back to
        // the monotonic clock when the driver did not supply one.
        if let Some(mut feedback) = feedback {
            let tp = metadata.as_ref().and_then(|m| match m.time {
                DrmEventTime::Monotonic(tp) if !tp.is_zero() => Some(tp),
                _ => None,
            });
            let seq = metadata.as_ref().map(|m| m.sequence).unwrap_or(0);
            let (time, flags): (Time<Monotonic>, wp_presentation_feedback::Kind) = match tp {
                Some(tp) => (
                    tp.into(),
                    wp_presentation_feedback::Kind::Vsync
                        | wp_presentation_feedback::Kind::HwClock
                        | wp_presentation_feedback::Kind::HwCompletion,
                ),
                None => (now, wp_presentation_feedback::Kind::Vsync),
            };
            // With VRR the panel refreshes on flip, so the mode refresh is only
            // the *fastest* (minimum interval) — report it as Variable so clients
            // pace against the floor rather than assuming a fixed cadence.
            let refresh = output
                .current_mode()
                .filter(|m| m.refresh > 0)
                .map(|m| {
                    let interval = Duration::from_secs_f64(1_000.0 / m.refresh as f64);
                    if vrr_active {
                        Refresh::variable(interval)
                    } else {
                        Refresh::fixed(interval)
                    }
                })
                .unwrap_or(Refresh::Unknown);
            feedback.presented(time, refresh, seq as u64, flags);
        }

        if still_pending {
            self.render_surface(UdevOutputId { device: node, crtc });
        }
    }

    /// Record the primary scan-out output for every surface shown on `output`
    /// (so frame callbacks and feedback target the right output) and collect the
    /// `wp_presentation` feedback callbacks for this frame.
    fn build_presentation_feedback(
        &self,
        output: &Output,
        states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        for window in self.space.elements() {
            window.with_surfaces(|surface, surface_data| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    surface_data,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        let mut feedback = OutputPresentationFeedback::new(output);
        for window in self.space.elements() {
            if self.space.outputs_for_element(window).contains(output) {
                window.take_presentation_feedback(
                    &mut feedback,
                    surface_primary_scanout_output,
                    |surface, _| {
                        surface_presentation_feedback_flags_from_states(surface, None, states)
                    },
                );
            }
        }
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, surface_data| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    surface_data,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
            layer_surface.take_presentation_feedback(
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| surface_presentation_feedback_flags_from_states(surface, None, states),
            );
        }
        feedback
    }

    fn render_surface(&mut self, id: UdevOutputId) {
        let Some((output, render_node, user_disabled, primary_gpu)) =
            self.udev.as_ref().and_then(|udev| {
                udev.surface(id).map(|surface| {
                    (
                        surface.output.clone(),
                        surface.render_node,
                        surface.user_disabled,
                        udev.render_node,
                    )
                })
            })
        else {
            return;
        };
        if user_disabled {
            return;
        }

        let context_changed = self
            .udev
            .as_ref()
            .is_some_and(|udev| udev.active_render_node != Some(render_node));
        if context_changed {
            self.wallpaper.invalidate_gpu_cache();
            self.decorations.invalidate_all();
            self.clear_mirror_batch_cache();
            if let Some(udev) = self.udev.as_mut() {
                udev.active_render_node = Some(render_node);
            }
        }

        let Some(mut gpus) = self.udev.as_mut().and_then(|udev| udev.gpus.take()) else {
            return;
        };

        let cross_gpu = primary_gpu != render_node;
        let mut frame_states: Option<RenderElementStates> = None;
        let outcome: Result<bool, String> = if self.mirror_mode_active() {
            let mut renderer_guard = match gpus.single_renderer(&render_node) {
                Ok(renderer) => renderer,
                Err(err) => {
                    tracing::warn!(?render_node, ?err, "renderer unavailable for output");
                    if let Some(udev) = self.udev.as_mut() {
                        udev.gpus = Some(gpus);
                    }
                    return;
                }
            };
            let renderer = renderer_guard.as_mut();
            if let Some(s) = self.udev.as_mut().and_then(|u| u.surface_mut(id)) {
                s.pending = false;
            }
            let result = crate::mirror::render_mirror_surface(self, renderer, id, !cross_gpu);
            drop(renderer_guard);
            result
        } else if cross_gpu {
            if let Some(s) = self.udev.as_mut().and_then(|u| u.surface_mut(id)) {
                s.pending = false;
            }
            match crate::cross_gpu::try_transfer_frame(
                self,
                &mut gpus,
                primary_gpu,
                render_node,
                id,
                &output,
            ) {
                Ok(Some(xfer)) => {
                    frame_states = Some(xfer.states);
                    Ok(!xfer.empty)
                }
                Ok(None) => {
                    // Same GPU — should not happen when cross_gpu is true.
                    render_local_output_frame(
                        self,
                        &mut gpus,
                        render_node,
                        primary_gpu,
                        id,
                        &output,
                        &mut frame_states,
                        false,
                    )
                }
                Err(err) => {
                    tracing::warn!(
                        %err,
                        ?primary_gpu,
                        ?render_node,
                        output = %output.name(),
                        "cross-GPU transfer failed; falling back to local renderer (blur off)"
                    );
                    render_local_output_frame(
                        self,
                        &mut gpus,
                        render_node,
                        primary_gpu,
                        id,
                        &output,
                        &mut frame_states,
                        true, // cross_gpu: blur off
                    )
                }
            }
        } else {
            render_local_output_frame(
                self,
                &mut gpus,
                render_node,
                primary_gpu,
                id,
                &output,
                &mut frame_states,
                false,
            )
        };

        if self.image_capture.has_pending()
            || self.has_pending_window_thumbs()
            || self.has_pending_workspace_thumbs()
        {
            if render_node != primary_gpu {
                self.wallpaper.invalidate_gpu_cache();
                self.decorations.invalidate_all();
                self.clear_mirror_batch_cache();
                if let Some(udev) = self.udev.as_mut() {
                    udev.active_render_node = Some(primary_gpu);
                }
            }
            match gpus.single_renderer(&primary_gpu) {
                Ok(mut capture_renderer) => {
                    self.process_pending_captures(capture_renderer.as_mut());
                }
                Err(err) => {
                    tracing::warn!(?primary_gpu, ?err, "primary capture renderer unavailable");
                }
            }
        }
        if let Some(udev) = self.udev.as_mut() {
            udev.gpus = Some(gpus);
        }

        match outcome {
            Ok(rendered) => {
                if rendered {
                    // Collect presentation feedback (and record primary scan-out
                    // outputs) from the states captured during render, then attach
                    // it to the frame so `on_drm_vblank` can report the real
                    // scan-out time to clients.
                    let feedback = frame_states
                        .as_ref()
                        .map(|states| self.build_presentation_feedback(&output, states));
                    let Some(surface) = self.udev.as_mut().and_then(|udev| udev.surface_mut(id))
                    else {
                        return;
                    };
                    match surface.drm_output.queue_frame(feedback) {
                        Ok(()) => surface.queued = true,
                        Err(err) => tracing::warn!(?err, "queue_frame failed"),
                    }
                    // Deliver frame callbacks so clients paint their next frame.
                    let now = self.start_time.elapsed();
                    let out = output.clone();
                    self.space.elements().for_each(|window| {
                        window
                            .send_frame(&out, now, Some(Duration::ZERO), |_, _| Some(out.clone()));
                    });
                    self.send_layer_frames(&out, now);

                    if let Some(states) = frame_states.as_ref()
                        && self.output_scanout_promoted(&out, states)
                    {
                        tracing::trace!(
                            output = %out.name(),
                            scanout_promoted = true,
                            "direct primary-plane scanout"
                        );
                    }

                    // Per-surface dmabuf feedback: tell a surface that was scanned
                    // out directly (a fullscreen game on the primary plane) to keep
                    // allocating scannable buffers; everyone else gets the render
                    // feedback. Requires the render states captured this frame.
                    if let Some(states) = frame_states.as_ref()
                        && let Some(udev) = self.udev.as_ref()
                        && let Some(feedback) =
                            udev.surface(id).and_then(|s| s.dmabuf_feedback.as_ref())
                    {
                        for window in self.space.elements() {
                            window.send_dmabuf_feedback(
                                &out,
                                surface_primary_scanout_output,
                                |surface, _| {
                                    select_dmabuf_feedback(
                                        surface,
                                        states,
                                        &feedback.render_feedback,
                                        &feedback.scanout_feedback,
                                    )
                                },
                            );
                        }
                    }
                }
            }
            Err(err) => tracing::warn!(%err, "render_frame failed"),
        }
    }

    /// Build the pointer render element(s) for `output`, in output-local physical
    /// coordinates. Honors a client-supplied cursor surface (`set_cursor`), hides
    /// the pointer when the client requested it, and otherwise paints the named
    /// theme cursor. Returns empty when the pointer is not over this output.
    pub(crate) fn build_cursor_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        scale: Scale<f64>,
    ) -> Vec<OutputStack> {
        let mut out = Vec::new();
        let Some(geo) = self.space.output_geometry(output) else {
            return out;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return out;
        };
        let loc = pointer.current_location();
        if !geo.to_f64().contains(loc) {
            return out;
        }
        let over_bar = self.metis_bar_ui_hit(loc);
        // Output-local logical pointer position.
        let local = loc - geo.loc.to_f64();

        if matches!(self.cursor_status, CursorImageStatus::Hidden) {
            return out;
        }
        if self.active_pointer_lock_suppresses_cursor() {
            return out;
        }

        let millis = self.start_time.elapsed().as_millis() as u32;
        let udev = match self.udev.as_mut() {
            Some(u) => u,
            None => return out,
        };

        // Always paint a compositor-owned pointer on DRM. Client wl_pointer surfaces
        // were composited onto the primary plane; switching to the resize cursor on
        // the hardware cursor plane left those pixels behind, so the arrow never
        // appeared to change. The nested winit session already ignores client cursors.
        let image = if !over_bar {
            if let Some(edge) = self.hover_cursor {
                udev.cursor.frame_resize(&udev.cursor_theme, edge, millis)
            } else {
                udev.cursor.frame(millis).clone()
            }
        } else {
            udev.cursor.frame(millis).clone()
        };
        let buffer = match udev.pointer_buffers.iter().find(|(i, _)| *i == image) {
            Some((_, buf)) => buf.clone(),
            None => {
                let buf = MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    Fourcc::Argb8888,
                    (image.width as i32, image.height as i32),
                    1,
                    Transform::Normal,
                    None,
                );
                udev.pointer_buffers.push((image.clone(), buf.clone()));
                buf
            }
        };
        let hotspot: Point<f64, Physical> = Point::from((image.xhot as f64, image.yhot as f64));
        let pos = local.to_physical(scale) - hotspot;
        if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            pos,
            &buffer,
            None,
            None,
            None,
            Kind::Cursor,
        ) {
            out.push(OutputStack::CursorMemory(elem));
        }
        out
    }

    /// Re-pack mapped outputs left-to-right in global space so a disconnect never
    /// leaves a hole (and a reconnect never overlaps). Order is kept stable by
    /// current x then name, so the surviving outputs don't shuffle unexpectedly.
    fn repack_outputs(&mut self) {
        if self.mirror_mode_active() {
            let cfg = self.output_runtime.cached().clone();
            crate::mirror::apply_mirror_layout(self, &cfg);
            return;
        }
        let mut outputs: Vec<Output> = self
            .udev
            .as_ref()
            .map(|u| {
                u.surfaces()
                    .filter(|s| !s.user_disabled)
                    .map(|s| s.output.clone())
                    .collect()
            })
            .unwrap_or_else(|| self.space.outputs().cloned().collect());
        outputs.sort_by(|a, b| {
            let ax = self.space.output_geometry(a).map(|g| g.loc.x).unwrap_or(0);
            let bx = self.space.output_geometry(b).map(|g| g.loc.x).unwrap_or(0);
            ax.cmp(&bx).then_with(|| a.name().cmp(&b.name()))
        });
        let mut x = 0;
        for output in outputs {
            let width = self
                .space
                .output_geometry(&output)
                .map(|g| g.size.w)
                .or_else(|| output.current_mode().map(|m| m.size.w))
                .unwrap_or(0);
            let position = Point::from((x, 0));
            output.change_current_state(None, None, None, Some(position));
            self.space.map_output(&output, position);
            x += width;
        }
    }

    /// Re-apply window/layer geometry after the output set changed (connect /
    /// disconnect / hotplug). Mirrors the winit resize path.
    pub(crate) fn retile_outputs(&mut self) {
        self.repack_outputs();
        if let Some(first) = self
            .space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
        {
            self.monitor.width = first.size.w;
            self.monitor.height = first.size.h;
        }
        let (wp_full, wp_regions) = self.wallpaper_layout();
        self.wallpaper.set_layout(wp_full, wp_regions);
        if self.wallpaper.enabled() {
            self.wallpaper.start_async_decode();
        }
        let ids: Vec<u32> = self.windows.ids();
        for id in ids {
            self.apply_window_rect(id);
        }
        self.sync_all_app_windows();
        self.refresh_all_scroll_offsets();
        self.arrange_layers();
        self.emit_monitor_changed();
        self.damaged = true;
    }

    /// libseat session pause/resume (VT switch, suspend).
    pub(crate) fn on_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused (VT switch away / suspend)");
                if let Some(udev) = self.udev.as_mut() {
                    if let Some(li) = udev.libinput.as_mut() {
                        li.suspend();
                    }
                    for backend in udev.backends.values_mut() {
                        backend.drm_output_manager.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session resumed");
                if let Some(udev) = self.udev.as_mut() {
                    if let Some(li) = udev.libinput.as_mut()
                        && let Err(err) = li.resume()
                    {
                        tracing::warn!(?err, "failed to resume libinput");
                    }
                    for backend in udev.backends.values_mut() {
                        if let Err(err) = backend.drm_output_manager.lock().activate(false) {
                            tracing::warn!(?err, "failed to reactivate DRM after resume");
                        }
                    }
                    for surface in udev.surfaces_mut() {
                        surface.queued = false;
                        surface.pending = true;
                    }
                }
                // A VT switch resets CRTC gamma and connector HDR blobs; GL colour
                // resources may also be invalid after DRM pause — dirty + drop them
                // so the next frame rebakes LUT atlases and recompiles HDR encode.
                self.color_mgmt.profiles_dirty = true;
                self.color_lut.invalidate_gl();
                self.hdr_encode.invalidate_gl();
                crate::output_gamma::apply_output_gamma(self);
                crate::output_hdr::reapply_output_hdrs(self);
                self.damaged = true;
                self.drm_dispatch_damage();
            }
        }
    }

    /// Switch to virtual terminal `vt` (Ctrl+Alt+F<n>). Only meaningful under the
    /// DRM backend; a no-op (logged) when nested. Refused while the session is
    /// locked (Phase 15 §C) — use Ctrl+Alt+Backspace to quit instead.
    pub(crate) fn drm_change_vt(&mut self, vt: i32) {
        if self.session_is_locked() {
            tracing::warn!(vt, "refusing VT switch while session is locked");
            return;
        }
        if let Some(udev) = self.udev.as_mut()
            && let Err(err) = udev.session.change_vt(vt)
        {
            tracing::warn!(?err, vt, "failed to change VT");
        }
    }

    /// True when running under the standalone DRM backend.
    pub(crate) fn is_drm_backend(&self) -> bool {
        self.udev.is_some()
    }

    /// Safe quit for the standalone session (Ctrl+Alt+Backspace): tear down
    /// clients and stop the event loop, returning to the greeter.
    pub(crate) fn drm_quit(&mut self) {
        tracing::info!("safe-quit keybind — shutting down DRM session");
        self.end_compositor_session();
    }

    /// udev device add/remove (GPU hotplug).
    pub(crate) fn on_udev_event(&mut self, event: UdevEvent) {
        let primary = self.udev.as_ref().map(|u| u.node);
        match event {
            UdevEvent::Changed { .. } => self.scan_connectors(),
            UdevEvent::Removed { device_id } => {
                let node = DrmNode::from_dev_id(device_id).ok().map(to_primary_node);
                if node == primary {
                    tracing::error!("primary GPU removed — shutting down DRM session");
                    self.drm_quit();
                    return;
                }
                let Some(node) = node else {
                    return;
                };
                let removed = self
                    .udev
                    .as_mut()
                    .and_then(|udev| udev.backends.remove(&node));
                if let Some(backend) = removed {
                    for surface in backend.surfaces.values() {
                        self.space.unmap_output(&surface.output);
                    }
                    if let Some(udev) = self.udev.as_mut() {
                        if let Some(gpus) = udev.gpus.as_mut() {
                            gpus.as_mut().remove_node(&backend.render_node);
                        }
                        udev.loop_handle.remove(backend.registration_token);
                    }
                    self.retile_outputs();
                }
            }
            UdevEvent::Added { device_id, path } => {
                let Ok(node) = DrmNode::from_dev_id(device_id).map(to_primary_node) else {
                    return;
                };
                if self
                    .udev
                    .as_ref()
                    .is_some_and(|udev| udev.backends.contains_key(&node))
                {
                    self.scan_connectors();
                    return;
                }
                let Some(mut gpus) = self.udev.as_mut().and_then(|udev| udev.gpus.take()) else {
                    return;
                };
                let opened = {
                    let Some(udev) = self.udev.as_ref() else {
                        return;
                    };
                    open_device(&udev.loop_handle, &udev.session, &mut gpus, node, &path)
                };
                if let Some(udev) = self.udev.as_mut() {
                    udev.gpus = Some(gpus);
                    match opened {
                        Ok(opened) => {
                            udev.backends.insert(
                                node,
                                BackendData {
                                    drm_output_manager: opened.manager,
                                    drm_scanner: DrmScanner::new(),
                                    surfaces: HashMap::new(),
                                    render_node: opened.render_node,
                                    registration_token: opened.drm_token,
                                },
                            );
                        }
                        Err(err) => tracing::warn!(?node, ?err, "failed to open hotplug GPU"),
                    }
                }
                self.scan_connectors();
            }
        }
    }

    /// Turn off a connected DRM output without disconnecting the connector.
    pub(crate) fn udev_disable_output(&mut self, name: &str) -> bool {
        let Some(udev) = self.udev.as_mut() else {
            return false;
        };
        let id = udev.output_id_by_name(name).filter(|id| {
            udev.surface(*id)
                .is_some_and(|surface| !surface.user_disabled)
        });
        let Some(id) = id else {
            return false;
        };
        let (output, connector, device_node) = {
            let Some(surface) = udev.surface_mut(id) else {
                return false;
            };
            let output = surface.output.clone();
            if let Some(global) = surface.global.take() {
                self.display_handle.remove_global::<MetisState>(global);
            }
            self.space.unmap_output(&output);
            surface.user_disabled = true;
            surface.pending = false;
            surface.queued = false;
            (output, surface.connector, id.device)
        };
        // Blank the panel. Without this the last frame stays lit while the
        // pointer can no longer enter — feels like a "dead but still on" monitor.
        if let Some(backend) = self
            .udev
            .as_ref()
            .and_then(|u| u.backends.get(&device_node))
        {
            let device = backend.drm_output_manager.device();
            set_connector_dpms(device, connector, false, name);
        }
        let _ = output;
        tracing::info!(output = %name, "output disabled by user (unmapped + DPMS off)");
        true
    }

    /// Re-enable a user-disabled DRM output.
    pub(crate) fn udev_enable_output(&mut self, name: &str) -> bool {
        let Some(udev) = self.udev.as_mut() else {
            return false;
        };
        let id = udev.output_id_by_name(name).filter(|id| {
            udev.surface(*id)
                .is_some_and(|surface| surface.user_disabled)
        });
        let Some(id) = id else {
            return false;
        };
        let (output, connector, device_node) = {
            let Some(surface) = udev.surface_mut(id) else {
                return false;
            };
            let output = surface.output.clone();
            let global = output.create_global::<MetisState>(&self.display_handle);
            surface.global = Some(global);
            surface.user_disabled = false;
            surface.pending = true;
            (output, surface.connector, id.device)
        };
        if let Some(backend) = self
            .udev
            .as_ref()
            .and_then(|u| u.backends.get(&device_node))
        {
            let device = backend.drm_output_manager.device();
            set_connector_dpms(device, connector, true, name);
        }
        // Remap into the desktop. `apply_output_layout` only touches outputs
        // already in the space — without this, Active stays "on" in Settings
        // while the monitor never rejoins the pointer layout.
        let cfg = self.output_runtime.cached().clone();
        let pos = crate::output_prefs::output_position_for_connect(self, &cfg, name);
        output.change_current_state(None, None, None, Some(pos));
        self.space.map_output(&output, pos);
        tracing::info!(output = %name, ?pos, "output re-enabled by user (mapped + DPMS on)");
        true
    }

    /// Power every active connector on or off via its DRM `DPMS` property. Used
    /// by the idle blanker: `on == false` powers the panels down (backlight off)
    /// after the idle timeout, `on == true` wakes them and requests a repaint.
    ///
    /// No-op under the nested winit backend (there is no DRM device); the panel
    /// there is owned by the host compositor. User-disabled outputs are left
    /// alone — they are already off. This deliberately does **not** touch the
    /// `wl_output` globals or the CRTC mode, so clients never see the monitor
    /// "disconnect" and nothing reflows across a blank/wake cycle.
    pub(crate) fn set_outputs_dpms(&mut self, on: bool) {
        let Some(udev) = self.udev.as_ref() else {
            return;
        };
        for backend in udev.backends.values() {
            let device = backend.drm_output_manager.device();
            for surface in backend.surfaces.values() {
                if surface.user_disabled {
                    continue;
                }
                set_connector_dpms(device, surface.connector, on, &surface.output.name());
            }
        }
        // On wake, mark surfaces dirty so the heartbeat repaints once powered.
        if on {
            if let Some(udev) = self.udev.as_mut() {
                for surface in udev.surfaces_mut() {
                    if !surface.user_disabled {
                        surface.pending = true;
                    }
                }
            }
            self.damaged = true;
        }
    }
}

/// DRM `DPMS` connector-property value for the "on" state.
const DRM_MODE_DPMS_ON: u64 = 0;
/// DRM `DPMS` connector-property value for the "off" (powered down) state.
const DRM_MODE_DPMS_OFF: u64 = 3;

/// Set one connector's `DPMS` property. Mirrors Smithay's internal
/// `set_connector_state`; failures are logged and swallowed so a stubborn
/// connector can never take the session down (the worst case is a panel that
/// stays lit, and any input still wakes the rest of the pipeline).
fn set_connector_dpms<D: DrmControlDevice>(
    device: &D,
    conn: connector::Handle,
    on: bool,
    name: &str,
) {
    let props = match device.get_properties(conn) {
        Ok(props) => props,
        Err(err) => {
            tracing::warn!(output = %name, ?err, "dpms: get_properties failed");
            return;
        }
    };
    let (handles, _) = props.as_props_and_values();
    for handle in handles {
        let Ok(info) = device.get_property(*handle) else {
            continue;
        };
        if info.name().to_str().map(|n| n == "DPMS").unwrap_or(false) {
            let value = if on {
                DRM_MODE_DPMS_ON
            } else {
                DRM_MODE_DPMS_OFF
            };
            if let Err(err) = device.set_property(conn, *handle, value) {
                tracing::warn!(output = %name, on, ?err, "dpms: set_property failed");
            }
            return;
        }
    }
    tracing::debug!(output = %name, "dpms: connector has no DPMS property");
}

impl DmabufHandler for MetisState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self
            .udev
            .as_mut()
            .expect("dmabuf only active under DRM backend")
            .dmabuf_state
            .as_mut()
            .expect("dmabuf global initialized")
            .0
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        let primary = self.udev.as_ref().map(|udev| udev.render_node);
        let ok = primary
            .zip(self.udev.as_mut().and_then(|udev| udev.gpus.as_mut()))
            .and_then(|(primary, gpus)| gpus.single_renderer(&primary).ok())
            .map(|mut renderer| renderer.import_dmabuf(&dmabuf, None).is_ok())
            .unwrap_or(false);
        if ok {
            if let Some(primary) = primary {
                dmabuf.set_node(primary);
            }
            let _ = notifier.successful::<MetisState>();
        } else {
            notifier.failed();
        }
    }
}
