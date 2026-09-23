//! Persistent ext-image-copy-capture session for live ScreenCast frames.

use std::time::{Duration, Instant};

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output::WlOutput, wl_registry::WlRegistry, wl_shm::Format, wl_shm::WlShm},
};
use wayland_protocols::ext::{
    image_capture_source::v1::client::{
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
        ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
        ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
    },
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};

use metis_capture::dmabuf::{modifiers_from_array, parse_dev_t};
use metis_capture::{
    BufferFormat, CaptureOptions, DmabufBuffer, DmabufOffer, Frame, ShmBuffer, prefer_shm_format,
};

enum CaptureMode {
    #[allow(dead_code)]
    OneShot,
    Continuous,
}

struct SessionState {
    constraints: BufferFormat,
    needs_allocate: bool,
    frame_ready: bool,
    shm: Option<ShmBuffer>,
    dmabuf: Option<DmabufBuffer>,
    dmabuf_offer: DmabufOffer,
    dmabuf_failed: bool,
    _source: ExtImageCaptureSourceV1,
    session: ExtImageCopyCaptureSessionV1,
    frame_pending: bool,
}

#[derive(Debug, Clone, Default)]
struct OutputInfo {
    output: Option<WlOutput>,
    name: String,
    description: String,
}

struct AppState {
    shm: Option<WlShm>,
    dmabuf_global: Option<ZwpLinuxDmabufV1>,
    copy_manager: Option<ExtImageCopyCaptureManagerV1>,
    source_manager: Option<ExtOutputImageCaptureSourceManagerV1>,
    outputs: Vec<OutputInfo>,
    options: CaptureOptions,
    session: Option<SessionState>,
    result: Option<Result<Frame, String>>,
    mode: CaptureMode,
}

impl AppState {
    fn fail(&mut self, msg: impl Into<String>) {
        if self.result.is_none() {
            self.result = Some(Err(msg.into()));
        }
        if let Some(session) = self.session.take() {
            session.session.destroy();
        }
    }

    fn start_capture(&mut self, qh: &QueueHandle<Self>) {
        let Some(output) = self.resolve_output() else {
            self.fail("no wl_output for capture selection");
            return;
        };
        let Some(source_manager) = self.source_manager.as_ref() else {
            self.fail("ext output capture source manager missing");
            return;
        };
        let Some(copy_manager) = self.copy_manager.as_ref() else {
            self.fail("ext image copy capture manager missing");
            return;
        };

        let source = source_manager.create_source(&output, qh, ());
        let wl_options = if self.options.draw_cursor {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let session = copy_manager.create_session(&source, wl_options, qh, ());

        self.session = Some(SessionState {
            constraints: BufferFormat {
                format: Format::Argb8888,
                width: 0,
                height: 0,
                stride: 0,
            },
            needs_allocate: false,
            frame_ready: false,
            shm: None,
            dmabuf: None,
            dmabuf_offer: DmabufOffer::default(),
            dmabuf_failed: false,
            _source: source,
            session,
            frame_pending: false,
        });
    }

    fn resolve_output(&self) -> Option<WlOutput> {
        resolve_output(&self.outputs, &self.options)
    }

    fn on_session_done(&mut self, qh: &QueueHandle<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Err(msg) =
            validate_buffer_constraints(session.constraints.width, session.constraints.height)
        {
            self.fail(msg);
            return;
        }
        session.constraints.stride =
            effective_stride(session.constraints.width, session.constraints.stride);

        if session.dmabuf.is_none()
            && !session.dmabuf_failed
            && session.dmabuf_offer.is_usable()
            && let Some(dmabuf_global) = self.dmabuf_global.as_ref()
        {
            match DmabufBuffer::allocate(
                dmabuf_global,
                qh,
                &session.dmabuf_offer,
                session.constraints.width,
                session.constraints.height,
            ) {
                Ok(buffer) => session.dmabuf = Some(buffer),
                Err(err) => {
                    session.dmabuf_failed = true;
                    tracing::warn!(%err, "dmabuf capture allocation failed; falling back to shm");
                }
            }
        }

        if session.dmabuf.is_none() && session.shm.is_none() {
            let Some(shm_global) = self.shm.as_ref() else {
                self.fail("neither usable dmabuf nor wl_shm is available");
                return;
            };
            match ShmBuffer::new(shm_global, qh, session.constraints) {
                Ok(buf) => session.shm = Some(buf),
                Err(err) => {
                    self.fail(err);
                    return;
                }
            }
        }

        self.request_frame(qh);
    }

    fn request_frame(&mut self, qh: &QueueHandle<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.frame_pending {
            return;
        }
        let frame = session.session.create_frame(qh, ());
        if let Some(dmabuf) = session.dmabuf.as_ref() {
            frame.attach_buffer(&dmabuf.buffer);
        } else if let Some(shm) = session.shm.as_ref() {
            frame.attach_buffer(&shm.buffer);
        } else {
            return;
        }
        frame.damage_buffer(
            0,
            0,
            session.constraints.width as i32,
            session.constraints.height as i32,
        );
        frame.capture();
        session.frame_pending = true;
    }

    fn on_frame_ready(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let frame = if let Some(dmabuf) = session.dmabuf.as_ref() {
            let planes = match dmabuf.export_planes() {
                Ok(planes) => planes,
                Err(err) => {
                    session.dmabuf_failed = true;
                    session.dmabuf = None;
                    self.fail(format!("export dmabuf capture planes: {err}"));
                    return;
                }
            };
            Frame {
                width: dmabuf.width,
                height: dmabuf.height,
                stride: dmabuf.stride,
                shm_format: Format::Argb8888,
                data: dmabuf.pixels().map_or_else(Vec::new, <[u8]>::to_vec),
                dmabuf: Some(planes),
            }
        } else if let Some(shm) = session.shm.as_ref() {
            Frame {
                width: shm.format.width,
                height: shm.format.height,
                stride: shm.format.stride,
                shm_format: shm.format.format,
                data: shm.pixels().to_vec(),
                dmabuf: None,
            }
        } else {
            self.fail("missing capture buffer on frame ready");
            return;
        };
        if matches!(self.mode, CaptureMode::OneShot) {
            if let Some(session) = self.session.take() {
                session.session.destroy();
            }
            self.result = Some(Ok(frame));
            return;
        }
        session.frame_pending = false;
        self.result = Some(Ok(frame));
    }

    fn tick(&mut self, qh: &QueueHandle<Self>) {
        if self.result.is_some() {
            return;
        }
        let needs_allocate = self
            .session
            .as_ref()
            .is_some_and(|session| session.needs_allocate);
        let frame_ready = self
            .session
            .as_ref()
            .is_some_and(|session| session.frame_ready);

        if needs_allocate {
            if let Some(session) = self.session.as_mut() {
                session.needs_allocate = false;
            }
            self.on_session_done(qh);
        }
        if frame_ready {
            if let Some(session) = self.session.as_mut() {
                session.frame_ready = false;
            }
            self.on_frame_ready();
        }
    }
}

fn bind_all_outputs(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<AppState>,
) -> Vec<OutputInfo> {
    let registry = globals.registry();
    let globals_list = globals.contents().clone_list();
    let mut outputs = Vec::new();
    for g in globals_list
        .into_iter()
        .filter(|g| g.interface == "wl_output")
    {
        let version = g.version.min(4);
        let idx = outputs.len();
        let output: WlOutput = registry.bind(g.name, version, qh, idx);
        outputs.push(OutputInfo {
            output: Some(output),
            name: String::new(),
            description: String::new(),
        });
    }
    outputs
}

/// Reject zero-sized capture buffers from the compositor.
pub(crate) fn validate_buffer_constraints(width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0 || height == 0 {
        Err("invalid capture buffer size from compositor")
    } else {
        Ok(())
    }
}

/// Default stride to tightly packed RGBA when the compositor omits it.
pub(crate) fn effective_stride(width: u32, stride: u32) -> u32 {
    if stride == 0 {
        width.saturating_mul(4)
    } else {
        stride
    }
}

/// Pure output selection used by [`resolve_output`] (index into `outputs`).
pub(crate) fn select_output_index(
    names: &[(String, String)],
    connector: Option<&str>,
    output_index: usize,
) -> Option<usize> {
    if let Some(want) = connector.map(str::trim).filter(|s| !s.is_empty()) {
        let want_l = want.to_ascii_lowercase();
        if let Some(idx) = names.iter().position(|(name, description)| {
            name.eq_ignore_ascii_case(want) || description.to_ascii_lowercase().contains(&want_l)
        }) {
            return Some(idx);
        }
    }
    if names.is_empty() {
        None
    } else if output_index < names.len() {
        Some(output_index)
    } else {
        Some(0)
    }
}

fn resolve_output(outputs: &[OutputInfo], options: &CaptureOptions) -> Option<WlOutput> {
    let names: Vec<(String, String)> = outputs
        .iter()
        .map(|o| (o.name.clone(), o.description.clone()))
        .collect();
    let idx = select_output_index(&names, options.connector.as_deref(), options.output_index)?;
    outputs.get(idx).and_then(|o| o.output.clone())
}

/// Live capture handle — keeps an ext-image-copy session open across frames.
pub struct CaptureSession {
    _conn: Connection,
    queue: wayland_client::EventQueue<AppState>,
    state: AppState,
}

impl CaptureSession {
    pub fn open(options: CaptureOptions) -> Result<Self, String> {
        let conn = Connection::connect_to_env()
            .map_err(|err| format!("connect to WAYLAND_DISPLAY: {err}"))?;
        let (globals, mut queue) = registry_queue_init::<AppState>(&conn)
            .map_err(|err| format!("registry init: {err}"))?;
        let qh = queue.handle();

        let shm = globals.bind(&qh, 1..=1, ()).ok();
        let dmabuf_global = globals.bind(&qh, 3..=5, ()).ok();
        let copy_manager = globals.bind(&qh, 1..=1, ()).ok();
        let source_manager = globals.bind(&qh, 1..=1, ()).ok();
        let outputs = bind_all_outputs(&globals, &qh);

        let mut state = AppState {
            shm,
            dmabuf_global,
            copy_manager,
            source_manager,
            outputs,
            options,
            session: None,
            result: None,
            mode: CaptureMode::Continuous,
        };

        if state.copy_manager.is_none() || state.source_manager.is_none() {
            return Err(
                "compositor does not expose ext-image-copy-capture (rebuild metis-compositor)"
                    .into(),
            );
        }
        if state.outputs.is_empty() {
            return Err("no wl_output available for capture".into());
        }

        // Round-trip so wl_output.name / description events arrive before selection.
        queue
            .roundtrip(&mut state)
            .map_err(|err| format!("wayland roundtrip: {err}"))?;

        state.start_capture(&qh);

        let mut session = Self {
            _conn: conn,
            queue,
            state,
        };
        session.wait_until_ready(Duration::from_secs(8))?;
        Ok(session)
    }

    fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while self.state.session.is_none() && Instant::now() < deadline {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|err| format!("wayland dispatch: {err}"))?;
            self.state.tick(&self.queue.handle());
            if self.state.result.is_some() {
                break;
            }
        }
        if self.state.session.is_some() {
            Ok(())
        } else {
            self.state
                .result
                .take()
                .unwrap_or(Err("capture session setup timed out".into()))
                .map(|_| ())
        }
    }

    pub fn capture_next_frame(&mut self) -> Result<Frame, String> {
        self.state.result = None;
        let qh = self.queue.handle();
        self.state.request_frame(&qh);

        let deadline = Instant::now() + Duration::from_millis(500);
        while self.state.result.is_none() && Instant::now() < deadline {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|err| format!("wayland dispatch: {err}"))?;
            self.state.tick(&qh);
        }

        match self.state.result.take() {
            Some(result) => result,
            None => Err("capture frame timed out".into()),
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        if let Some(session) = self.state.session.take() {
            session.session.destroy();
        }
    }
}

fn prefer_shm_format_local(current: Format, offered: Format) -> Format {
    prefer_shm_format(current, offered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_constraints_reject_zero_size() {
        assert!(validate_buffer_constraints(1920, 1080).is_ok());
        assert_eq!(
            validate_buffer_constraints(0, 1080).unwrap_err(),
            "invalid capture buffer size from compositor"
        );
        assert!(validate_buffer_constraints(1920, 0).is_err());
    }

    #[test]
    fn effective_stride_defaults_to_rgba_pitch() {
        assert_eq!(effective_stride(100, 0), 400);
        assert_eq!(effective_stride(100, 512), 512);
    }

    #[test]
    fn select_output_prefers_connector_match_then_index() {
        let outputs = vec![
            ("eDP-1".into(), "Built-in".into()),
            ("HDMI-A-1".into(), "External HDMI".into()),
        ];
        assert_eq!(select_output_index(&outputs, Some("hdmi-a-1"), 0), Some(1));
        assert_eq!(select_output_index(&outputs, Some("External"), 0), Some(1));
        assert_eq!(select_output_index(&outputs, None, 1), Some(1));
        assert_eq!(select_output_index(&outputs, None, 99), Some(0));
        assert_eq!(select_output_index(&[], None, 0), None);
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for AppState {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(AppState: ignore WlShm);
wayland_client::delegate_noop!(AppState: ignore ZwpLinuxDmabufV1);
wayland_client::delegate_noop!(AppState: ignore ExtOutputImageCaptureSourceManagerV1);
wayland_client::delegate_noop!(AppState: ignore ExtImageCopyCaptureManagerV1);
wayland_client::delegate_noop!(AppState: ignore ExtImageCaptureSourceV1);
wayland_client::delegate_noop!(AppState: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(AppState: ignore wayland_client::protocol::wl_buffer::WlBuffer);

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            if let Some(session) = state.session.as_mut() {
                session.dmabuf_failed = true;
            }
            tracing::warn!("linux-dmabuf buffer creation failed");
        }
    }
}

impl Dispatch<WlOutput, usize> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        &idx: &usize,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output::Event;
        let Some(info) = state.outputs.get_mut(idx) else {
            return;
        };
        match event {
            Event::Name { name } => info.name = name,
            Event::Description { description } => info.description = description,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &ExtImageCopyCaptureSessionV1,
        event: <ExtImageCopyCaptureSessionV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        let Some(session) = state.session.as_mut() else {
            return;
        };
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height, .. } => {
                session.constraints.width = width;
                session.constraints.height = height;
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat {
                format: WEnum::Value(fmt),
                ..
            } => {
                session.constraints.format =
                    prefer_shm_format_local(session.constraints.format, fmt);
            }
            ext_image_copy_capture_session_v1::Event::DmabufDevice { device, .. } => {
                session.dmabuf_offer.device = parse_dev_t(&device);
            }
            ext_image_copy_capture_session_v1::Event::DmabufFormat {
                format, modifiers, ..
            } => {
                session
                    .dmabuf_offer
                    .formats
                    .push((format, modifiers_from_array(&modifiers)));
            }
            ext_image_copy_capture_session_v1::Event::Done => {
                session.needs_allocate = true;
            }
            ext_image_copy_capture_session_v1::Event::Stopped => {
                state.fail("capture session stopped");
            }
            _ => {}
        }
        if state.result.is_none()
            && state
                .session
                .as_ref()
                .is_some_and(|session| session.needs_allocate)
        {
            if let Some(session) = state.session.as_mut() {
                session.needs_allocate = false;
            }
            state.on_session_done(qhandle);
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &ExtImageCopyCaptureFrameV1,
        event: <ExtImageCopyCaptureFrameV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => {
                if let Some(session) = state.session.as_mut() {
                    session.frame_pending = false;
                    session.frame_ready = true;
                }
            }
            ext_image_copy_capture_frame_v1::Event::Failed { .. } => {
                state.fail("capture frame failed");
            }
            _ => {}
        }
        if state.result.is_none()
            && state
                .session
                .as_ref()
                .is_some_and(|session| session.frame_ready)
        {
            if let Some(session) = state.session.as_mut() {
                session.frame_ready = false;
            }
            state.on_frame_ready();
        }
    }
}
