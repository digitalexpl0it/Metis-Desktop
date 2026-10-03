//! xdg ScreenCast client → PipeWire frames → [`crate::framebuf::FrameBuffer`].

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow};
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use enumflags2::BitFlags;
use pipewire as pw;
use pw::properties::properties;
use pw::spa::pod::Pod;

use crate::framebuf::FrameBuffer;

struct CaptureData {
    format: pw::spa::param::video::VideoInfoRaw,
    frames: Arc<Mutex<FrameBuffer>>,
}

pub struct CaptureSession {
    _join: JoinHandle<()>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl CaptureSession {
    pub async fn start(frames: Arc<Mutex<FrameBuffer>>) -> Result<Self> {
        let proxy = Screencast::new()
            .await
            .context("ScreenCast portal unavailable")?;
        let session = proxy
            .create_session(Default::default())
            .await
            .context("ScreenCast CreateSession")?;
        proxy
            .select_sources(
                &session,
                ashpd::desktop::screencast::SelectSourcesOptions::default()
                    .set_sources(Some(BitFlags::from(SourceType::Monitor)))
                    .set_cursor_mode(Some(CursorMode::Embedded))
                    .set_persist_mode(Some(PersistMode::DoNot))
                    .set_multiple(Some(false)),
            )
            .await
            .context("ScreenCast SelectSources")?
            .response()
            .context("ScreenCast SelectSources response")?;
        let streams = proxy
            .start(&session, None, Default::default())
            .await
            .context("ScreenCast Start")?
            .response()
            .context("ScreenCast Start response")?;
        let stream = streams
            .streams()
            .first()
            .ok_or_else(|| anyhow!("ScreenCast returned no streams"))?;
        let node_id = stream.pipe_wire_node_id();
        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .context("OpenPipeWireRemote")?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("metis-rdp-pw".into())
            .spawn(move || {
                if let Err(err) = run_pipewire(fd, node_id, frames, stop_t) {
                    tracing::error!(%err, "rdp host: PipeWire capture ended");
                }
            })
            .context("spawn PipeWire thread")?;
        // Keep the portal session alive for the lifetime of the capture thread.
        std::mem::forget(session);
        Ok(Self { _join: join, stop })
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn run_pipewire(
    fd: OwnedFd,
    node_id: u32,
    frames: Arc<Mutex<FrameBuffer>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("pw MainLoop")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("pw Context")?;
    let core = context.connect_fd_rc(fd, None).context("pw connect_fd")?;

    let data = CaptureData {
        format: Default::default(),
        frames,
    };

    let stream = pw::stream::StreamBox::new(
        &core,
        "metis-rdp-host",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .context("pw Stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if user_data.format.parse(param).is_ok() {
                tracing::info!(
                    w = user_data.format.size().width,
                    h = user_data.format.size().height,
                    "rdp host: PipeWire video format"
                );
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let w = user_data.format.size().width;
            let h = user_data.format.size().height;
            if w == 0 || h == 0 {
                return;
            }
            let stride = w.saturating_mul(4);
            let size = datas[0].chunk().size() as usize;
            let pixels = match datas[0].data() {
                Some(d) if size > 0 && d.len() >= size => d[..size].to_vec(),
                _ => return,
            };
            if let Ok(mut fb) = user_data.frames.lock() {
                let _ = fb.publish_bgrx(w, h, stride, &pixels);
            }
        })
        .register()
        .context("pw listener")?;

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA,
        ),
    );
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| anyhow!("serialize format pod: {e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).context("format pod")?];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("pw stream connect")?;

    tracing::info!(node_id, "rdp host: PipeWire stream connected");
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = mainloop.loop_().iterate(pw::loop_::Timeout::Finite(
            std::time::Duration::from_millis(50),
        ));
    }
    let _ = stream.disconnect();
    Ok(())
}
