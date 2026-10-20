use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ashpd::{
    MaybeAppID, PortalError, WindowIdentifierType,
    backend::{
        request::RequestImpl,
        screencast::{ScreencastImpl, SelectSourcesResponse},
        session::{CreateSessionResponse, SessionImpl},
    },
    desktop::{
        CreateSessionOptions, HandleToken,
        screencast::{
            CursorMode, SelectSourcesOptions, SourceType, StartCastOptions, StreamBuilder, Streams,
            StreamsBuilder,
        },
    },
};
use async_trait::async_trait;
use enumflags2::BitFlags;

use metis_capture::CaptureOptions;
use metis_protocol::{OutputInfo, RUDP_MAX_SUBSCRIBED_OUTPUTS};

use crate::capture::{CaptureHub, spawn_screencast_pump};
use crate::compositor_ipc;
use crate::pipewire::PipeWireHub;

/// Screen capture must not start while the session is locked — the framebuffer
/// shows lock UI, and refusing avoids leaking even that. Mirrors compositor
/// `process_pending_captures` / IPC `BeginCaptureOverlay` denylist policy.
pub(crate) fn should_refuse_cast_while_locked(session_locked: bool) -> bool {
    session_locked
}

struct CastSession {
    streams: Vec<u32>,
    cancel: Arc<AtomicBool>,
    /// One pump thread per PipeWire stream (multi-monitor).
    pumps: Vec<JoinHandle<()>>,
    /// From SelectSources — when true, start_cast emits one stream per output.
    multiple: bool,
}

impl CastSession {
    fn stop(&mut self, pipewire: &PipeWireHub) {
        self.cancel.store(true, Ordering::Relaxed);
        for handle in self.pumps.drain(..) {
            let _ = handle.join();
        }
        for node in self.streams.drain(..) {
            pipewire.destroy_stream(node);
        }
    }
}

pub struct MetisScreencast {
    capture: Arc<CaptureHub>,
    pipewire: Arc<PipeWireHub>,
    sessions: Mutex<HashMap<String, CastSession>>,
}

impl MetisScreencast {
    pub fn new(capture: Arc<CaptureHub>, pipewire: Arc<PipeWireHub>) -> Self {
        Self {
            capture,
            pipewire,
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl RequestImpl for MetisScreencast {
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl SessionImpl for MetisScreencast {
    async fn session_closed(&self, session_token: HandleToken) -> ashpd::backend::Result<()> {
        let key = session_token.to_string();
        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(mut session) = sessions.remove(&key)
        {
            session.stop(&self.pipewire);
        }
        compositor_ipc::end_capture_overlay(None);
        Ok(())
    }
}

#[async_trait]
impl ScreencastImpl for MetisScreencast {
    fn available_source_types(&self) -> BitFlags<SourceType> {
        SourceType::Monitor.into()
    }

    fn available_cursor_mode(&self) -> BitFlags<CursorMode> {
        CursorMode::Hidden | CursorMode::Embedded | CursorMode::Metadata
    }

    async fn create_session(
        &self,
        _token: HandleToken,
        session_token: HandleToken,
        _app_id: Option<MaybeAppID>,
        _options: CreateSessionOptions,
    ) -> ashpd::backend::Result<CreateSessionResponse> {
        self.sessions
            .lock()
            .map_err(|err| PortalError::Failed(format!("session map lock: {err}")))?
            .insert(
                session_token.to_string(),
                CastSession {
                    streams: Vec::new(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    pumps: Vec::new(),
                    multiple: false,
                },
            );
        Ok(CreateSessionResponse::new(session_token))
    }

    async fn select_sources(
        &self,
        session_token: HandleToken,
        _app_id: Option<MaybeAppID>,
        options: SelectSourcesOptions,
    ) -> ashpd::backend::Result<SelectSourcesResponse> {
        let multiple = options.is_multiple().unwrap_or(false);
        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(session) = sessions.get_mut(&session_token.to_string())
        {
            session.multiple = multiple;
        }
        Ok(SelectSourcesResponse {})
    }

    async fn start_cast(
        &self,
        session_token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        _options: StartCastOptions,
    ) -> ashpd::backend::Result<Streams> {
        tracing::info!(?app_id, "portal screencast start");
        let app_label = crate::consent::display_app_label(app_id.as_ref());
        if !crate::consent::request_capture_consent(
            crate::consent::CaptureKind::Screencast,
            &app_label,
        )
        .await
        {
            return Err(PortalError::Cancelled(
                "Screen share declined by user".into(),
            ));
        }
        let portal_app = compositor_ipc::portal_app_id(app_id);
        if let Err(message) = compositor_ipc::begin_capture_overlay(portal_app) {
            let locked = message.contains("session is locked");
            tracing::warn!(%message, locked, "BeginCaptureOverlay rejected");
            if should_refuse_cast_while_locked(locked) {
                return Err(PortalError::Failed(message));
            }
            return Err(PortalError::Failed(message));
        }

        let multiple = self
            .sessions
            .lock()
            .ok()
            .and_then(|g| g.get(&session_token.to_string()).map(|s| s.multiple))
            .unwrap_or(false);

        let targets = screencast_targets(multiple, &self.capture).await;
        if targets.is_empty() {
            return Err(PortalError::Failed(
                "no outputs available for ScreenCast".into(),
            ));
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let mut pw_streams = Vec::with_capacity(targets.len());
        let mut node_ids = Vec::with_capacity(targets.len());
        let mut pumps = Vec::with_capacity(targets.len());

        for (idx, target) in targets.iter().enumerate() {
            let stream = self
                .pipewire
                .create_stream(target.width, target.height)
                .map_err(|err| PortalError::Failed(format!("create PipeWire stream: {err}")))?;

            let Some(pump) = spawn_screencast_pump(
                Arc::clone(&self.pipewire),
                stream.node_id,
                CaptureOptions {
                    draw_cursor: true,
                    output_index: target.output_index,
                    connector: target.connector.clone(),
                },
                Arc::clone(&cancel),
            ) else {
                // Tear down streams already created for this cast.
                for nid in &node_ids {
                    self.pipewire.destroy_stream(*nid);
                }
                self.pipewire.destroy_stream(stream.node_id);
                return Err(PortalError::Failed(
                    "failed to spawn screencast pump thread".into(),
                ));
            };

            let pw_stream = StreamBuilder::new(stream.node_id)
                .source_type(SourceType::Monitor)
                .size((target.width as i32, target.height as i32))
                .position((target.x, target.y))
                .mapping_id(format!("metis:{idx}"))
                .build();

            node_ids.push(stream.node_id);
            pumps.push(pump);
            pw_streams.push(pw_stream);
        }

        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(session) = sessions.get_mut(&session_token.to_string())
        {
            session.streams.extend(node_ids);
            session.cancel = cancel;
            session.pumps = pumps;
        }

        tracing::info!(
            streams = pw_streams.len(),
            multiple,
            "portal screencast streams ready"
        );
        Ok(StreamsBuilder::new(pw_streams).build())
    }
}

#[derive(Debug, Clone)]
struct CastTarget {
    connector: Option<String>,
    output_index: usize,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

async fn screencast_targets(multiple: bool, capture: &CaptureHub) -> Vec<CastTarget> {
    if !multiple {
        let (width, height) = capture.output_size(None).await;
        return vec![CastTarget {
            connector: None,
            output_index: 0,
            x: 0,
            y: 0,
            width,
            height,
        }];
    }

    let outputs = compositor_ipc::list_outputs();
    let enabled: Vec<&OutputInfo> = outputs
        .iter()
        .filter(|o| o.enabled && !o.mirrored && o.rect.width > 0 && o.rect.height > 0)
        .take(RUDP_MAX_SUBSCRIBED_OUTPUTS)
        .collect();

    if enabled.is_empty() {
        let (width, height) = capture.output_size(None).await;
        return vec![CastTarget {
            connector: None,
            output_index: 0,
            x: 0,
            y: 0,
            width,
            height,
        }];
    }

    enabled
        .into_iter()
        .enumerate()
        .map(|(idx, o)| CastTarget {
            connector: Some(o.name.clone()),
            output_index: idx,
            x: o.rect.x,
            y: o.rect.y,
            width: o.rect.width.max(1) as u32,
            height: o.rect.height.max(1) as u32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuse_cast_while_locked_policy() {
        assert!(should_refuse_cast_while_locked(true));
        assert!(!should_refuse_cast_while_locked(false));
    }
}
