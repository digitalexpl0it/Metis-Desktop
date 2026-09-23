//! Native ext-image-copy-capture screenshot client.

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

use crate::dmabuf::DmabufPlanes;
use crate::shm::{BufferFormat, ShmBuffer};

#[derive(Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub shm_format: Format,
    pub data: Vec<u8>,
    /// When present, PipeWire can push `SPA_DATA_DmaBuf` instead of MemFd.
    pub dmabuf: Option<DmabufPlanes>,
}

#[derive(Debug, Clone, Default)]
pub struct CaptureOptions {
    pub draw_cursor: bool,
    /// Zero-based index into the connected `wl_output` list (used when
    /// [`Self::connector`] is `None` or does not match).
    pub output_index: usize,
    /// Prefer the output whose `wl_output.name` (or description) matches this
    /// connector string (e.g. `HDMI-A-1`, `eDP-1`). Case-insensitive.
    pub connector: Option<String>,
}

struct SessionState {
    constraints: BufferFormat,
    needs_allocate: bool,
    frame_ready: bool,
    shm: Option<ShmBuffer>,
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

pub struct AppState {
    shm: Option<WlShm>,
    copy_manager: Option<ExtImageCopyCaptureManagerV1>,
    source_manager: Option<ExtOutputImageCaptureSourceManagerV1>,
    outputs: Vec<OutputInfo>,
    options: CaptureOptions,
    session: Option<SessionState>,
    result: Option<Result<Frame, String>>,
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
        let options = if self.options.draw_cursor {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let session = copy_manager.create_session(&source, options, qh, ());

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
        if session.constraints.width == 0 || session.constraints.height == 0 {
            self.fail("invalid capture buffer size from compositor");
            return;
        }
        if session.constraints.stride == 0 {
            session.constraints.stride = session.constraints.width * 4;
        }

        let Some(shm_global) = self.shm.as_ref() else {
            self.fail("wl_shm missing");
            return;
        };

        if session.shm.is_none() {
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
        let Some(shm) = session.shm.as_ref() else {
            return;
        };
        let frame = session.session.create_frame(qh, ());
        frame.attach_buffer(&shm.buffer);
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
        let Some(session) = self.session.take() else {
            return;
        };
        let Some(shm) = session.shm else {
            self.fail("missing shm buffer on frame ready");
            return;
        };
        session.session.destroy();
        self.result = Some(Ok(Frame {
            width: shm.format.width,
            height: shm.format.height,
            stride: shm.format.stride,
            shm_format: shm.format.format,
            data: shm.pixels().to_vec(),
            dmabuf: None,
        }));
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

pub fn capture_output_frame(options: CaptureOptions) -> Result<Frame, String> {
    let conn =
        Connection::connect_to_env().map_err(|err| format!("connect to WAYLAND_DISPLAY: {err}"))?;
    let (globals, mut event_queue) =
        registry_queue_init::<AppState>(&conn).map_err(|err| format!("registry init: {err}"))?;
    let qh = event_queue.handle();

    let shm = globals.bind(&qh, 1..=1, ()).ok();
    let copy_manager = globals.bind(&qh, 1..=1, ()).ok();
    let source_manager = globals.bind(&qh, 1..=1, ()).ok();
    let outputs = bind_all_outputs(&globals, &qh);

    let mut state = AppState {
        shm,
        copy_manager,
        source_manager,
        outputs,
        options,
        session: None,
        result: None,
    };

    if state.copy_manager.is_none() || state.source_manager.is_none() {
        return Err(
            "compositor does not expose ext-image-copy-capture (rebuild metis-compositor)".into(),
        );
    }
    if state.outputs.is_empty() {
        return Err("no wl_output available for capture".into());
    }

    // Round-trip so wl_output.name / description events arrive before selection.
    event_queue
        .roundtrip(&mut state)
        .map_err(|err| format!("wayland roundtrip: {err}"))?;

    state.start_capture(&qh);

    let deadline = Instant::now() + Duration::from_secs(8);
    while state.result.is_none() && Instant::now() < deadline {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|err| format!("wayland dispatch: {err}"))?;
        state.tick(&qh);
    }

    match state.result {
        Some(result) => result,
        None => Err("capture timed out".to_string()),
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

fn resolve_output(outputs: &[OutputInfo], options: &CaptureOptions) -> Option<WlOutput> {
    if let Some(want) = options
        .connector
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let want_l = want.to_ascii_lowercase();
        if let Some(info) = outputs.iter().find(|o| {
            o.name.eq_ignore_ascii_case(want)
                || o.description.to_ascii_lowercase().contains(&want_l)
        }) {
            return info.output.clone();
        }
    }
    outputs
        .get(options.output_index)
        .or_else(|| outputs.first())
        .and_then(|o| o.output.clone())
}

pub fn prefer_shm_format(current: Format, offered: Format) -> Format {
    fn rank(format: Format) -> u8 {
        match format {
            Format::Argb8888 => 0,
            Format::Xrgb8888 => 1,
            Format::Abgr8888 => 2,
            Format::Xbgr8888 => 3,
            _ => 4,
        }
    }
    if rank(offered) < rank(current) {
        offered
    } else {
        current
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
wayland_client::delegate_noop!(AppState: ignore ExtOutputImageCaptureSourceManagerV1);
wayland_client::delegate_noop!(AppState: ignore ExtImageCopyCaptureManagerV1);
wayland_client::delegate_noop!(AppState: ignore ExtImageCaptureSourceV1);
wayland_client::delegate_noop!(AppState: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(AppState: ignore wayland_client::protocol::wl_buffer::WlBuffer);

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
                session.constraints.format = prefer_shm_format(session.constraints.format, fmt);
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
