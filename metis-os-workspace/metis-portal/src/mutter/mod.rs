//! Mutter RemoteDesktop + ScreenCast D-Bus shims for gnome-remote-desktop.
//!
//! GRD refuses to bind RDP port 3389 until both `org.gnome.Mutter.RemoteDesktop`
//! and `org.gnome.Mutter.ScreenCast` are available on the session bus.

pub mod clipboard;
mod eis;

use std::collections::HashMap;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use zbus::zvariant::OwnedFd;

use ashpd::PortalError;
use zbus::fdo;
use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, Value};

use metis_capture::CaptureOptions;

use crate::capture::{CaptureHub, spawn_screencast_pump};
use crate::compositor_ipc;
use crate::pipewire::PipeWireHub;

const REMOTE_DESKTOP_NAME: &str = "org.gnome.Mutter.RemoteDesktop";
const REMOTE_DESKTOP_PATH: &str = "/org/gnome/Mutter/RemoteDesktop";
const SCREEN_CAST_NAME: &str = "org.gnome.Mutter.ScreenCast";
const SCREEN_CAST_PATH: &str = "/org/gnome/Mutter/ScreenCast";

#[derive(Clone)]
struct MutterHub {
    next_session: Arc<AtomicU32>,
    next_stream: Arc<AtomicU32>,
    pipewire: Arc<PipeWireHub>,
    capture: Arc<CaptureHub>,
    rd_sessions: Arc<Mutex<HashMap<String, String>>>,
    sc_streams: Arc<Mutex<HashMap<String, StreamSlot>>>,
    clipboard_sessions: Arc<Mutex<Vec<clipboard::ClipboardSession>>>,
}

struct StreamSlot {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    pump: Option<std::thread::JoinHandle<()>>,
}

impl MutterHub {
    fn new(pipewire: Arc<PipeWireHub>, capture: Arc<CaptureHub>) -> Self {
        Self {
            next_session: Arc::new(AtomicU32::new(1)),
            next_stream: Arc::new(AtomicU32::new(1)),
            pipewire,
            capture,
            rd_sessions: Arc::new(Mutex::new(HashMap::new())),
            sc_streams: Arc::new(Mutex::new(HashMap::new())),
            clipboard_sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn alloc_session_id(&self) -> String {
        self.next_session
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    fn alloc_stream_id(&self) -> String {
        self.next_stream.fetch_add(1, Ordering::Relaxed).to_string()
    }

    async fn capture_size(&self, connector: Option<&str>) -> (u32, u32) {
        self.capture.output_size(connector).await
    }
}

struct RemoteDesktopRoot {
    hub: MutterHub,
    conn: zbus::Connection,
}

#[interface(name = "org.gnome.Mutter.RemoteDesktop")]
impl RemoteDesktopRoot {
    #[zbus(property)]
    fn version(&self) -> i32 {
        1
    }

    #[zbus(property)]
    fn supported_device_types(&self) -> u32 {
        // keyboard | pointer | touchscreen
        1 | 2 | 4
    }

    async fn create_session(&self) -> fdo::Result<OwnedObjectPath> {
        let id = self.hub.alloc_session_id();
        let path = format!("{REMOTE_DESKTOP_PATH}/Session/{id}");
        let session_id = format!("metis-rd-{id}");
        self.hub
            .rd_sessions
            .lock()
            .map_err(|_| fdo::Error::Failed("session map lock".into()))?
            .insert(path.clone(), session_id.clone());

        let clipboard = clipboard::ClipboardSession::new(self.conn.clone(), path.clone());
        if let Ok(mut list) = self.hub.clipboard_sessions.lock() {
            list.push(clipboard.clone());
        }
        let iface = RemoteDesktopSession {
            _hub: self.hub.clone(),
            _conn: self.conn.clone(),
            session_id,
            _path: path.clone(),
            started: false,
            clipboard,
        };
        self.conn
            .object_server()
            .at(path.as_str(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        OwnedObjectPath::try_from(path.as_str()).map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

struct RemoteDesktopSession {
    _hub: MutterHub,
    _conn: zbus::Connection,
    session_id: String,
    _path: String,
    started: bool,
    clipboard: clipboard::ClipboardSession,
}

#[interface(name = "org.gnome.Mutter.RemoteDesktop.Session")]
impl RemoteDesktopSession {
    #[zbus(property)]
    fn session_id(&self) -> &str {
        &self.session_id
    }

    #[zbus(property)]
    fn caps_lock_state(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn num_lock_state(&self) -> bool {
        false
    }

    async fn start(&mut self) -> fdo::Result<()> {
        self.started = true;
        Ok(())
    }

    async fn stop(&mut self) -> fdo::Result<()> {
        self.started = false;
        Ok(())
    }

    #[zbus(name = "EnableClipboard")]
    async fn enable_clipboard(&mut self, options: HashMap<&str, Value<'_>>) -> fdo::Result<()> {
        self.clipboard
            .enable(&options)
            .map_err(fdo::Error::Failed)?;
        Ok(())
    }

    #[zbus(name = "DisableClipboard")]
    async fn disable_clipboard(&mut self) -> fdo::Result<()> {
        self.clipboard.disable().map_err(fdo::Error::Failed)?;
        Ok(())
    }

    #[zbus(name = "SetSelection")]
    async fn set_selection(&mut self, options: HashMap<&str, Value<'_>>) -> fdo::Result<()> {
        self.clipboard
            .set_selection(&options)
            .map_err(fdo::Error::Failed)?;
        Ok(())
    }

    #[zbus(name = "SelectionWrite")]
    async fn selection_write(&mut self, serial: u32) -> fdo::Result<zbus::zvariant::OwnedFd> {
        self.clipboard
            .selection_write(serial)
            .map(zbus::zvariant::OwnedFd::from)
            .map_err(fdo::Error::Failed)
    }

    #[zbus(name = "SelectionWriteDone")]
    async fn selection_write_done(&mut self, serial: u32, success: bool) -> fdo::Result<()> {
        self.clipboard
            .selection_write_done(serial, success)
            .map_err(fdo::Error::Failed)?;
        Ok(())
    }

    #[zbus(name = "SelectionRead")]
    async fn selection_read(&mut self, mime_type: &str) -> fdo::Result<zbus::zvariant::OwnedFd> {
        self.clipboard
            .selection_read(mime_type)
            .map(zbus::zvariant::OwnedFd::from)
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "ConnectToEIS")]
    async fn connect_to_eis(&mut self, _options: HashMap<&str, Value<'_>>) -> fdo::Result<OwnedFd> {
        tracing::info!(session = %self.session_id, "mutter shim: ConnectToEIS");
        let fd = eis::client_fd().map_err(fdo::Error::Failed)?;
        Ok(fd.into())
    }
}

struct ScreenCastRoot {
    hub: MutterHub,
    conn: zbus::Connection,
}

#[interface(name = "org.gnome.Mutter.ScreenCast")]
impl ScreenCastRoot {
    #[zbus(property)]
    fn version(&self) -> i32 {
        4
    }

    async fn create_session(
        &self,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        let rd_id = properties
            .get("remote-desktop-session-id")
            .and_then(|v| v.downcast_ref::<&str>().ok())
            .map(str::to_string);
        let id = self.hub.alloc_session_id();
        let path = format!("{SCREEN_CAST_PATH}/Session/{id}");
        let iface = ScreenCastSession {
            hub: self.hub.clone(),
            conn: self.conn.clone(),
            path: path.clone(),
            _remote_desktop_session_id: rd_id,
        };
        self.conn
            .object_server()
            .at(path.as_str(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        OwnedObjectPath::try_from(path.as_str()).map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

struct ScreenCastSession {
    hub: MutterHub,
    conn: zbus::Connection,
    path: String,
    _remote_desktop_session_id: Option<String>,
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Session")]
impl ScreenCastSession {
    async fn start(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn record_monitor(
        &self,
        connector: &str,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        self.record_stream(properties, Some(connector.to_string()))
            .await
    }

    async fn record_virtual(
        &self,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        self.record_stream(properties, None).await
    }

    async fn record_window(
        &self,
        _properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        Err(fdo::Error::NotSupported(
            "single-window capture not implemented".into(),
        ))
    }

    async fn record_area(
        &self,
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
        properties: HashMap<&str, Value<'_>>,
    ) -> fdo::Result<OwnedObjectPath> {
        self.record_stream(properties, None).await
    }
}

impl ScreenCastSession {
    async fn record_stream(
        &self,
        properties: HashMap<&str, Value<'_>>,
        connector: Option<String>,
    ) -> fdo::Result<OwnedObjectPath> {
        let stream_id = self.hub.alloc_stream_id();
        let path = format!("{}/Stream/{stream_id}", self.path);
        let (width, height) = self.hub.capture_size(connector.as_deref()).await;
        let mapping_id = mapping_id_from_properties(&properties);
        eis::register_viewport(eis::Viewport {
            mapping_id: mapping_id.clone(),
            x: 0,
            y: 0,
            width,
            height,
            scale: 1.0,
        });
        let iface = ScreenCastStream {
            hub: self.hub.clone(),
            conn: self.conn.clone(),
            stream_path: path.clone(),
            width,
            height,
            mapping_id,
            connector,
            node_id: None,
        };
        self.conn
            .object_server()
            .at(path.as_str(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        OwnedObjectPath::try_from(path.as_str()).map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

struct ScreenCastStream {
    hub: MutterHub,
    conn: zbus::Connection,
    stream_path: String,
    width: u32,
    height: u32,
    mapping_id: String,
    connector: Option<String>,
    node_id: Option<u32>,
}

fn mapping_id_from_properties(properties: &HashMap<&str, Value<'_>>) -> String {
    for key in ["mapping-id", "mapping_id"] {
        if let Some(id) = properties
            .get(key)
            .and_then(|v| v.downcast_ref::<&str>().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return id.to_string();
        }
    }
    "metis:0".to_string()
}

#[interface(name = "org.gnome.Mutter.ScreenCast.Stream")]
impl ScreenCastStream {
    #[zbus(property)]
    fn parameters(&self) -> HashMap<String, Value<'_>> {
        HashMap::from([
            (
                "size".to_string(),
                Value::from((self.width as i32, self.height as i32)),
            ),
            ("position".to_string(), Value::from((0_i32, 0_i32))),
            (
                "mapping-id".to_string(),
                Value::from(self.mapping_id.as_str()),
            ),
        ])
    }

    async fn start(&mut self) -> fdo::Result<()> {
        if self.node_id.is_some() {
            return Ok(());
        }
        let (width, height) = self.hub.capture_size(self.connector.as_deref()).await;
        self.width = width;
        self.height = height;

        tracing::info!(
            stream = %self.stream_path,
            mapping_id = %self.mapping_id,
            connector = self.connector.as_deref().unwrap_or("primary"),
            width = self.width,
            height = self.height,
            "mutter shim: starting ScreenCast stream"
        );
        eis::update_viewport(eis::Viewport {
            mapping_id: self.mapping_id.clone(),
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
            scale: 1.0,
        });
        let handle = self
            .hub
            .pipewire
            .create_stream_with_mapping(self.width, self.height, &self.mapping_id)
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        self.node_id = Some(handle.node_id);

        if let Err(message) =
            compositor_ipc::begin_capture_overlay(Some("gnome-remote-desktop".into()))
        {
            tracing::warn!(%message, "BeginCaptureOverlay rejected");
        }

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pump = spawn_screencast_pump(
            Arc::clone(&self.hub.pipewire),
            handle.node_id,
            CaptureOptions {
                draw_cursor: true,
                connector: self.connector.clone(),
                output_index: 0,
            },
            Arc::clone(&cancel),
        );
        self.hub
            .sc_streams
            .lock()
            .map_err(|_| fdo::Error::Failed("stream map lock".into()))?
            .insert(
                self.stream_path.clone(),
                StreamSlot {
                    cancel,
                    pump: Some(pump),
                },
            );

        self.conn
            .emit_signal(
                None::<&str>,
                self.stream_path.as_str(),
                "org.gnome.Mutter.ScreenCast.Stream",
                "PipeWireStreamAdded",
                &(handle.node_id,),
            )
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    async fn stop(&mut self) -> fdo::Result<()> {
        if let Some(mut slot) = self
            .hub
            .sc_streams
            .lock()
            .map_err(|_| fdo::Error::Failed("stream map lock".into()))?
            .remove(&self.stream_path)
        {
            slot.cancel.store(true, Ordering::Relaxed);
            if let Some(handle) = slot.pump.take() {
                let _ = handle.join();
            }
        }
        if let Some(node) = self.node_id.take() {
            self.hub.pipewire.destroy_stream(node);
        }
        eis::unregister_viewport(&self.mapping_id);
        compositor_ipc::end_capture_overlay(Some("gnome-remote-desktop".into()));
        Ok(())
    }
}

/// Register Mutter-compatible D-Bus services needed by gnome-remote-desktop.
pub async fn serve(
    conn: &zbus::Connection,
    pipewire: Arc<PipeWireHub>,
    capture: Arc<CaptureHub>,
) -> Result<(), PortalError> {
    tracing::info!("mutter shim: registering RemoteDesktop + ScreenCast D-Bus APIs");
    let hub = MutterHub::new(pipewire, capture);
    crate::compositor_events::spawn_clipboard_listener(Arc::clone(&hub.clipboard_sessions));

    conn.object_server()
        .at(
            REMOTE_DESKTOP_PATH,
            RemoteDesktopRoot {
                hub: hub.clone(),
                conn: conn.clone(),
            },
        )
        .await
        .map_err(|e| PortalError::Failed(format!("mutter RemoteDesktop: {e}")))?;

    conn.object_server()
        .at(
            SCREEN_CAST_PATH,
            ScreenCastRoot {
                hub,
                conn: conn.clone(),
            },
        )
        .await
        .map_err(|e| PortalError::Failed(format!("mutter ScreenCast: {e}")))?;

    for name in [REMOTE_DESKTOP_NAME, SCREEN_CAST_NAME] {
        match conn.request_name(name).await {
            Ok(()) => tracing::info!(%name, "mutter shim: owning D-Bus name"),
            Err(err) => {
                tracing::warn!(%name, %err, "mutter shim: could not own D-Bus name");
                return Err(PortalError::Failed(format!("could not own {name}: {err}")));
            }
        }
    }

    Ok(())
}
