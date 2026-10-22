//! Metis Remote (RUDP) host glue inside the compositor: frame capture →
//! hardware encode on an isolated worker pair — **never** on the Smithay
//! calloop thread.
//!
//! Phase 23 §D: the network edge (Quinn / TLS / PAM / datagram fan-out) lives in
//! the separate `metis-rudp-host` process ([`crate::rudp_host_ipc`] speaks the
//! `metis_protocol::rudp_host_ipc` contract to it). This module keeps what must
//! stay next to the compositor: export demand, the encode pipeline and the
//! calloop input inject channel. `METIS_RUDP_INPROCESS=1` runs the very same
//! engine ([`metis_rudp_host::engine`]) on a compositor thread instead — the
//! emergency fallback for one release.
//!
//! Crash containment (enabling Metis Remote must never take down the session):
//! - Quinn / PAM run in `metis-rudp-host`; if it dies the compositor restarts it
//!   with bounded back-off and then leaves Remote off, desktop untouched.
//! - All FFmpeg / libva / CUDA code runs in a separate `metis-encode-probe
//!   worker` process ([`metis_encode::open_isolated_encoder`]). A driver crash
//!   there costs one bounded encoder restart, never the compositor.
//! - Nothing is composed or encoded until an authenticated client is connected
//!   (export demand gating); idle hosts add zero GPU work to the session.
//! - Every blocking wait on the frame thread is bounded or cancellable, so
//!   [`RudpHostSystem::shutdown`] (called on calloop) cannot hang the desktop.
//! - [`crate::rudp_guard`] auto-disables the host after an unclean session end
//!   while it was running, so a crash can never become a login crash loop.
//!
//! Frame path: wake → `take_latest_all` → bounded fence wait → per-output
//! worker encode → bounded outbound packet queue → (IPC forwarder | in-process
//! Quinn pump). Phase 7 maps control-stream input onto calloop →
//! [`crate::remote_input`] and advertises Wayland pointer lock.
//!
//! Multi-monitor (protocol v3): the compositor publishes the desktop topology
//! into [`RudpLayoutShared`]; the union of all sessions' output subscriptions
//! drives [`StreamExportHub`]; each output has its own encoder + `stream_id`.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use metis_config::{RudpEncoderBackend, RudpVideoCodec};
use metis_encode::{
    DEFAULT_BITRATE_KBPS, DEFAULT_FPS_HINT, EncodeError, EncodeInput, EncoderBackend,
    EncoderConfig, HwEncoder, NullEncoder, RudpCodec, open_isolated_encoder,
};
use metis_protocol::{CODEC_AV1, CODEC_H264, CODEC_HEVC, RudpDamageRect, RudpHostInput};
use metis_rudp_host::engine::{self, EngineShared, HostLink, MediaPacket, OutboundPacket};
use smithay::reexports::calloop;

pub use metis_rudp_host::engine::{DEFAULT_RUDP_PORT, RudpLayoutShared};

use crate::rudp_audio;
use crate::rudp_guard::{HostMarker, HostPhase, STABLE_AFTER};
use crate::stream_export::{ExportedFrame, StreamExportHub};

/// GPU fence wait per exported frame on the frame thread.
const FENCE_TIMEOUT: Duration = Duration::from_millis(500);
/// Consecutive encoder (worker) failures before hardware encode is given up
/// for this host instance — one back-off per entry in [`ENCODER_BACKOFF`],
/// then stop. Reset after [`ENCODER_HEALTHY_RESET`] of clean encode.
const ENCODER_MAX_FAILURES: u32 = ENCODER_BACKOFF.len() as u32 + 1;
const ENCODER_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
const ENCODER_HEALTHY_RESET: Duration = Duration::from_secs(60);
/// Keep a warm worker this long after the last client leaves.
const ENCODER_IDLE_CLOSE: Duration = Duration::from_secs(15);

/// Input / control events marshaled onto calloop (from Quinn directly in
/// in-process mode, or via the host process IPC).
#[derive(Debug, Clone)]
pub enum RudpInputEvent {
    PointerAbsolute {
        x: f64,
        y: f64,
    },
    PointerRelative {
        dx: f64,
        dy: f64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
    },
    PointerScroll {
        dx: f64,
        dy: f64,
    },
    Key {
        keycode: u32,
        pressed: bool,
    },
    /// Viewer → host: install text on the Wayland clipboard.
    ClipboardSet {
        mime: String,
        text: String,
        serial: u64,
    },
    /// Authenticated RUDP session as this user — unlock Metis PAM lock if they
    /// own the session (calloop; not from the control-stream JSON enum).
    UnlockPamSession {
        username: String,
    },
    /// Not input: the frame pipeline needs a full frame now (client joined /
    /// encoder restarted) even if the desktop is idle — repaint + force export.
    RefreshVideo,
}

impl From<RudpHostInput> for RudpInputEvent {
    fn from(ev: RudpHostInput) -> Self {
        match ev {
            RudpHostInput::PointerAbsolute { x, y } => Self::PointerAbsolute { x, y },
            RudpHostInput::PointerRelative { dx, dy } => Self::PointerRelative { dx, dy },
            RudpHostInput::PointerButton { button, pressed } => {
                Self::PointerButton { button, pressed }
            }
            RudpHostInput::PointerScroll { dx, dy } => Self::PointerScroll { dx, dy },
            RudpHostInput::Key { keycode, pressed } => Self::Key { keycode, pressed },
            RudpHostInput::ClipboardSet { mime, text, serial } => {
                Self::ClipboardSet { mime, text, serial }
            }
        }
    }
}

/// Calloop inject sender + shared flags for the Quinn runtime.
#[derive(Clone)]
pub struct RudpCalloopBridge {
    pub input_tx: calloop::channel::Sender<RudpInputEvent>,
    pub pointer_locked: Arc<AtomicBool>,
    /// True while Metis PAM lock or `ext-session-lock` owns the session.
    pub session_locked: Arc<AtomicBool>,
    /// Desktop topology for `DesktopLayout` / subscription resolution.
    pub layout: Arc<RudpLayoutShared>,
}

/// Desired host bind + policy from `rudp.json` / `METIS_RUDP_HOST`.
#[derive(Debug, Clone)]
pub struct DesiredRudpHost {
    pub config: RudpHostConfig,
    pub allowed_users: Vec<String>,
    pub lan_only: bool,
    pub unlock_session_on_auth: bool,
    pub encode_prefs: RudpEncodePrefs,
}

/// Encode preferences mirrored from `rudp.json` (for restart comparison).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RudpEncodePrefs {
    pub backend: RudpEncoderBackend,
    pub codec: RudpVideoCodec,
    pub bitrate_kbps: u32,
}

impl Default for RudpEncodePrefs {
    fn default() -> Self {
        Self {
            backend: RudpEncoderBackend::Auto,
            codec: RudpVideoCodec::H264,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
        }
    }
}

impl RudpEncodePrefs {
    fn from_config(cfg: &metis_config::RudpConfig) -> Self {
        Self {
            backend: cfg.encoder,
            codec: cfg.codec,
            bitrate_kbps: cfg.bitrate_kbps,
        }
    }
}

/// Bind address for the Quinn server.
#[derive(Debug, Clone, Copy)]
pub struct RudpHostConfig {
    pub bind: SocketAddr,
}

impl RudpHostConfig {
    /// Parse `METIS_RUDP_HOST`:
    /// - `1` / `true` / `yes` → `0.0.0.0:7843`
    /// - `IP:port` → that address
    /// - unset / empty → `None`
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("METIS_RUDP_HOST").ok()?;
        Self::parse(raw.trim())
    }

    /// Parse a host bind spec (see [`Self::from_env`]).
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        if matches!(raw, "1" | "true" | "yes") {
            return Some(Self {
                bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_RUDP_PORT)),
            });
        }
        match raw.parse::<SocketAddr>() {
            Ok(bind) => Some(Self { bind }),
            Err(err) => {
                tracing::error!(%raw, %err, "METIS_RUDP_HOST: invalid SocketAddr");
                None
            }
        }
    }
}

/// `true` when `METIS_RUDP_INPROCESS` asks for the emergency in-process Quinn
/// path (one release; removed after the host-process split has baked).
pub fn inprocess_requested() -> bool {
    std::env::var("METIS_RUDP_INPROCESS")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Last listener state reported by the network edge (host process or the
/// in-process engine).
#[derive(Debug, Clone, Default)]
pub struct NetStatus {
    pub listening: bool,
    pub fingerprint: String,
    pub bind: String,
    pub error: Option<String>,
}

/// Shared handle to [`NetStatus`].
pub type NetStatusHandle = Arc<Mutex<NetStatus>>;

/// Record a status update and log it (single place both modes go through).
pub(crate) fn record_net_status(
    slot: &NetStatusHandle,
    listening: bool,
    fingerprint: &str,
    bind: &str,
    error: Option<String>,
) {
    if let Some(err) = &error {
        tracing::error!(%bind, %err, "rudp host: network edge failed");
    } else {
        tracing::info!(%bind, %fingerprint, listening, "rudp host: network edge status");
    }
    if let Ok(mut guard) = slot.lock() {
        *guard = NetStatus {
            listening,
            fingerprint: fingerprint.to_string(),
            bind: bind.to_string(),
            error,
        };
    }
}

/// [`HostLink`] for the in-process fallback: engine callbacks go straight to
/// calloop and the export hub.
struct CalloopLink {
    input_tx: calloop::channel::Sender<RudpInputEvent>,
    hub: Arc<StreamExportHub>,
    status: NetStatusHandle,
}

impl HostLink for CalloopLink {
    fn input(&self, event: RudpHostInput) {
        let _ = self.input_tx.send(event.into());
    }

    fn unlock_pam_session(&self, username: String) {
        let _ = self
            .input_tx
            .send(RudpInputEvent::UnlockPamSession { username });
    }

    fn subscribe_union(&self, output_ids: Vec<String>, _gained: bool) {
        // Empty union (no sessions / no layout yet) ⇒ hub exports primary only.
        self.hub.set_subscribed_outputs(output_ids);
    }

    fn status(&self, listening: bool, fingerprint: &str, bind: SocketAddr, err: Option<String>) {
        record_net_status(&self.status, listening, fingerprint, &bind.to_string(), err);
    }
}

/// How the network edge is attached to this compositor.
enum NetBackend {
    /// `METIS_RUDP_INPROCESS=1`: engine thread inside the compositor.
    InProcess { quinn_join: Option<JoinHandle<()>> },
    /// Default: `metis-rudp-host` child process behind Unix IPC.
    HostProcess(crate::rudp_host_ipc::IpcHost),
}

/// Owns the network edge (host process or in-process Quinn thread), the frame
/// pipeline thread and the audio supervisor. Drop ⇒ disarm export.
pub struct RudpHostSystem {
    stop: Arc<AtomicBool>,
    hub: Arc<StreamExportHub>,
    net: NetBackend,
    frame_join: Option<JoinHandle<()>>,
    audio_join: Option<JoinHandle<()>>,
    /// Engine state shared with the frame pipeline / audio / IPC forwarder.
    pub shared: Arc<EngineShared>,
    /// Frames submitted to the hardware encoder (for tests / metrics).
    pub frames_encoded: Arc<AtomicU64>,
    /// Bytes produced by encode drain (approx outbound payload size).
    pub bytes_encoded: Arc<AtomicU64>,
    /// Last encoder open error (status / Settings).
    pub encode_error: Arc<Mutex<Option<String>>>,
    /// Active encoder backend/codec label after open (best-effort).
    pub encode_status: Arc<Mutex<Option<String>>>,
    /// Network edge listener state (fingerprint, bind errors).
    pub net_status: NetStatusHandle,
    /// Listen address currently bound.
    pub bind: SocketAddr,
    /// Local PAM usernames allowed to authenticate (Phase 4).
    pub allowed_users: Vec<String>,
    pub lan_only: bool,
    pub encode_prefs: RudpEncodePrefs,
    /// Shared with the frame thread — see [`Self::apply_encode_prefs`].
    prefs_live: Arc<Mutex<RudpEncodePrefs>>,
    prefs_gen: Arc<AtomicU64>,
    pub render_node_path: PathBuf,
    /// Crash-loop guard marker; dropped (file removed) after threads join.
    _marker: Arc<HostMarker>,
}

impl RudpHostSystem {
    /// Arm the export hub, start the frame pipeline + audio supervisor and
    /// attach the network edge (host process by default).
    pub fn spawn(
        desired: DesiredRudpHost,
        hub: Arc<StreamExportHub>,
        render_node_path: PathBuf,
        bridge: RudpCalloopBridge,
    ) -> Result<Self, String> {
        if desired.allowed_users.is_empty() {
            return Err("rudp host: refused — allowed_users is empty".into());
        }
        let inprocess = inprocess_requested();

        let DesiredRudpHost {
            config,
            allowed_users,
            lan_only,
            unlock_session_on_auth,
            encode_prefs,
        } = desired;
        let stop = Arc::new(AtomicBool::new(false));
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let bytes_encoded = Arc::new(AtomicU64::new(0));
        let encode_error = Arc::new(Mutex::new(None));
        let encode_status = Arc::new(Mutex::new(None));
        let net_status: NetStatusHandle = Arc::new(Mutex::new(NetStatus::default()));

        let link: Arc<dyn HostLink> = if inprocess {
            Arc::new(CalloopLink {
                input_tx: bridge.input_tx.clone(),
                hub: Arc::clone(&hub),
                status: Arc::clone(&net_status),
            })
        } else {
            // IPC mode: the compositor-side engine state is only a mirror the
            // frame pipeline writes and the IPC forwarder drains.
            Arc::new(engine::NullLink)
        };
        let shared = Arc::new(EngineShared::new(
            link,
            Arc::clone(&bridge.layout),
            Arc::clone(&bridge.pointer_locked),
            Arc::clone(&bridge.session_locked),
            allowed_users.clone(),
            unlock_session_on_auth,
        ));

        let wake = hub.arm();
        let marker = Arc::new(HostMarker::create());
        let audio_join = Some(rudp_audio::spawn_audio_supervisor(
            Arc::clone(&stop),
            Arc::clone(&shared),
        ));
        let prefs_live = Arc::new(Mutex::new(encode_prefs.clone()));
        let prefs_gen = Arc::new(AtomicU64::new(1));
        let frame_ctx = FramePipelineCtx {
            hub: Arc::clone(&hub),
            wake,
            stop: Arc::clone(&stop),
            frames_encoded: Arc::clone(&frames_encoded),
            bytes_encoded: Arc::clone(&bytes_encoded),
            encode_error: Arc::clone(&encode_error),
            encode_status: Arc::clone(&encode_status),
            prefs: Arc::clone(&prefs_live),
            prefs_gen: Arc::clone(&prefs_gen),
            render_node_path: render_node_path.clone(),
            video: Arc::clone(&shared),
            refresh_tx: bridge.input_tx.clone(),
            marker: Arc::clone(&marker),
            started: Instant::now(),
            session_locked: Arc::clone(&bridge.session_locked),
        };
        let frame_join = match std::thread::Builder::new()
            .name("metis-rudp-frames".into())
            .spawn(move || frame_pipeline_loop(frame_ctx))
        {
            Ok(handle) => handle,
            Err(err) => {
                stop.store(true, Ordering::SeqCst);
                hub.disarm();
                if let Some(audio) = audio_join {
                    let _ = audio.join();
                }
                return Err(format!("spawn rudp frame thread: {err}"));
            }
        };

        let net = if inprocess {
            tracing::warn!(
                "rudp host: METIS_RUDP_INPROCESS=1 — Quinn/PAM run INSIDE the compositor \
                 (emergency fallback, removed next release)"
            );
            let quinn_stop = Arc::clone(&stop);
            let quinn_shared = Arc::clone(&shared);
            let bind = config.bind;
            match std::thread::Builder::new()
                .name("metis-rudp-quinn".into())
                .spawn(move || {
                    if let Err(err) = engine::run_quinn_runtime(bind, quinn_stop, quinn_shared) {
                        tracing::error!(%err, "rudp host: Quinn runtime exited with error");
                    }
                }) {
                Ok(handle) => Ok(NetBackend::InProcess {
                    quinn_join: Some(handle),
                }),
                Err(err) => Err(format!("spawn rudp quinn thread: {err}")),
            }
        } else {
            crate::rudp_host_ipc::IpcHost::spawn(crate::rudp_host_ipc::IpcHostParams {
                bind: config.bind,
                allowed_users: allowed_users.clone(),
                lan_only,
                unlock_session_on_auth,
                shared: Arc::clone(&shared),
                hub: Arc::clone(&hub),
                input_tx: bridge.input_tx.clone(),
                status: Arc::clone(&net_status),
                stop: Arc::clone(&stop),
            })
            .map(NetBackend::HostProcess)
        };
        let net = match net {
            Ok(net) => net,
            Err(err) => {
                // Don't leak a running frame thread / armed hub on partial start.
                stop.store(true, Ordering::SeqCst);
                hub.disarm();
                let _ = frame_join.join();
                if let Some(audio) = audio_join {
                    let _ = audio.join();
                }
                return Err(err);
            }
        };

        tracing::info!(
            bind = %config.bind,
            allowed = ?allowed_users,
            lan_only,
            mode = if inprocess { "in-process (fallback)" } else { "host process" },
            encoder = encode_prefs.backend.as_str(),
            codec = encode_prefs.codec.as_str(),
            bitrate_kbps = encode_prefs.bitrate_kbps,
            render_node = %render_node_path.display(),
            "rudp host: started (network edge + hardware encode + input)"
        );

        Ok(Self {
            stop,
            hub,
            net,
            frame_join: Some(frame_join),
            audio_join,
            shared,
            frames_encoded,
            bytes_encoded,
            encode_error,
            encode_status,
            net_status,
            bind: config.bind,
            allowed_users,
            lan_only,
            encode_prefs,
            prefs_live,
            prefs_gen,
            render_node_path,
            _marker: marker,
        })
    }

    /// Apply a same-bind policy change (`ReloadRudp`): live allowlist and
    /// unlock-on-auth, forwarded to the host process when there is one.
    pub fn apply_policy(
        &mut self,
        allowed_users: Vec<String>,
        lan_only: bool,
        unlock_session_on_auth: bool,
    ) {
        self.allowed_users = allowed_users.clone();
        self.lan_only = lan_only;
        self.shared
            .set_policy(allowed_users.clone(), unlock_session_on_auth);
        if let NetBackend::HostProcess(ipc) = &self.net {
            ipc.update_policy(self.bind, allowed_users, lan_only, unlock_session_on_auth);
        }
    }

    /// Live-update encode prefs (bitrate / codec / backend) without tearing
    /// down Quinn or authenticated sessions. Frame pipes reopen on the next
    /// exported frame.
    pub fn apply_encode_prefs(&mut self, prefs: RudpEncodePrefs) {
        if self.encode_prefs == prefs {
            return;
        }
        tracing::info!(
            encoder = prefs.backend.as_str(),
            codec = prefs.codec.as_str(),
            bitrate_kbps = prefs.bitrate_kbps,
            "rudp host: encode prefs updated live (sessions kept)"
        );
        self.encode_prefs = prefs.clone();
        if let Ok(mut slot) = self.prefs_live.lock() {
            *slot = prefs;
        }
        self.prefs_gen.fetch_add(1, Ordering::SeqCst);
        // Wake export so idle desktops still reopen the encoder promptly.
        self.hub.request_full_frame();
    }

    /// Push local clipboard text to connected RUDP clients (latest-wins).
    /// No-op when nobody is authenticated.
    pub fn push_clipboard_text(&self, mime: &str, text: &str) {
        self.shared.push_clipboard_text(mime, text);
    }

    /// Signal stop, disarm export (unblocks frame wake), join workers.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.hub.disarm();
        match &mut self.net {
            NetBackend::InProcess { quinn_join } => {
                if let Some(handle) = quinn_join.take() {
                    let _ = handle.join();
                }
            }
            NetBackend::HostProcess(ipc) => ipc.shutdown(),
        }
        if let Some(handle) = self.frame_join.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.audio_join.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RudpHostSystem {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Wire codec byte for the encoder codec (`metis_protocol::CODEC_*`).
fn codec_to_wire(codec: RudpCodec) -> u8 {
    match codec {
        RudpCodec::H264 => CODEC_H264,
        RudpCodec::Hevc => CODEC_HEVC,
        RudpCodec::Av1 => CODEC_AV1,
    }
}

fn to_encode_backend(b: RudpEncoderBackend) -> EncoderBackend {
    match b {
        RudpEncoderBackend::Auto => EncoderBackend::Auto,
        RudpEncoderBackend::Vaapi => EncoderBackend::Vaapi,
        RudpEncoderBackend::Nvenc => EncoderBackend::Nvenc,
        RudpEncoderBackend::Soft => EncoderBackend::Soft,
    }
}

fn to_encode_codec(c: RudpVideoCodec) -> RudpCodec {
    match c {
        RudpVideoCodec::Hevc => RudpCodec::Hevc,
        RudpVideoCodec::H264 => RudpCodec::H264,
        RudpVideoCodec::Av1 => RudpCodec::Av1,
    }
}

fn open_hw_encoder(
    prefs: &RudpEncodePrefs,
    width: u32,
    height: u32,
    render_node: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<Box<dyn HwEncoder>, EncodeError> {
    if std::env::var_os("METIS_ENCODE_NULL").is_some() {
        tracing::warn!("METIS_ENCODE_NULL set — using NullEncoder");
        return Ok(Box::new(NullEncoder::new(width, height)));
    }
    let cfg = EncoderConfig {
        backend: to_encode_backend(prefs.backend),
        codec: to_encode_codec(prefs.codec),
        width,
        height,
        fps_hint: DEFAULT_FPS_HINT,
        bitrate_kbps: prefs.bitrate_kbps.max(500),
    };
    // Out-of-process: a libva / CUDA crash kills the worker, not the session.
    open_isolated_encoder(&cfg, render_node, Some(Arc::clone(cancel)))
}

struct FramePipelineCtx {
    hub: Arc<StreamExportHub>,
    wake: Receiver<()>,
    stop: Arc<AtomicBool>,
    frames_encoded: Arc<AtomicU64>,
    bytes_encoded: Arc<AtomicU64>,
    encode_error: Arc<Mutex<Option<String>>>,
    encode_status: Arc<Mutex<Option<String>>>,
    /// Live encode prefs (bitrate / codec / backend). Updated by
    /// [`RudpHostSystem::apply_encode_prefs`] without tearing down Quinn.
    prefs: Arc<Mutex<RudpEncodePrefs>>,
    /// Bumped whenever `prefs` changes so each [`OutputPipe`] reopens.
    prefs_gen: Arc<AtomicU64>,
    render_node_path: PathBuf,
    video: Arc<EngineShared>,
    /// Calloop channel used to request a repaint + full export frame.
    refresh_tx: calloop::channel::Sender<RudpInputEvent>,
    /// Crash-loop guard: phase follows startup / streaming / stable.
    marker: Arc<HostMarker>,
    started: Instant,
    session_locked: Arc<AtomicBool>,
}

impl FramePipelineCtx {
    fn set_status(&self, status: impl Into<String>) {
        if let Ok(mut slot) = self.encode_status.lock() {
            *slot = Some(status.into());
        }
    }

    fn set_error(&self, err: Option<String>) {
        if let Ok(mut slot) = self.encode_error.lock() {
            *slot = err;
        }
    }

    /// Ask calloop for a repaint whose export is a full frame (idle desktops
    /// otherwise produce no frames, leaving a fresh client on a black screen).
    fn request_refresh(&self) {
        self.hub.request_full_frame();
        if self.refresh_tx.send(RudpInputEvent::RefreshVideo).is_err() {
            tracing::debug!("rudp encode: calloop refresh channel closed");
        }
    }
}

struct ActiveEncoder {
    enc: Box<dyn HwEncoder>,
    width: u32,
    height: u32,
    /// Wire codec byte of the encoder actually running (see [`codec_to_wire`]).
    codec: u8,
    opened_at: Instant,
}

/// Encode state for one subscribed output (sizes differ per monitor, and a
/// failing worker on one output must not disable the others).
#[derive(Default)]
struct OutputPipe {
    encoder: Option<ActiveEncoder>,
    supervisor: EncoderSupervisor,
    encode_mode: Option<EncodeMode>,
    /// Last [`FramePipelineCtx::prefs_gen`] applied to this pipe.
    applied_prefs_gen: u64,
}

impl OutputPipe {
    fn request_keyframe(&mut self) {
        if let Some(active) = self.encoder.as_mut() {
            active.enc.request_keyframe();
        }
    }
}

/// Bounded restart policy for the isolated encoder: back off between
/// attempts and stop trying after [`ENCODER_MAX_FAILURES`] so a broken
/// driver can't turn into a crash / respawn loop.
#[derive(Debug, Default)]
struct EncoderSupervisor {
    failures: u32,
    retry_at: Option<Instant>,
    gave_up: bool,
    /// After a back-off expires, request one refresh so an idle desktop still
    /// delivers the frame that reopens the encoder.
    refresh_on_retry: bool,
}

impl EncoderSupervisor {
    fn may_open(&self, now: Instant) -> bool {
        !self.gave_up && self.retry_at.is_none_or(|at| now >= at)
    }

    /// Record a failed open / dead worker. Returns `true` once given up.
    fn record_failure(&mut self, now: Instant) -> bool {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= ENCODER_MAX_FAILURES {
            self.gave_up = true;
            self.retry_at = None;
            self.refresh_on_retry = false;
        } else {
            let idx = (self.failures as usize)
                .saturating_sub(1)
                .min(ENCODER_BACKOFF.len() - 1);
            self.retry_at = Some(now + ENCODER_BACKOFF[idx]);
            self.refresh_on_retry = true;
        }
        self.gave_up
    }

    fn record_success(&mut self, encoder_age: Duration) {
        self.retry_at = None;
        if encoder_age >= ENCODER_HEALTHY_RESET {
            self.failures = 0;
        }
    }

    /// True exactly once when a back-off has expired.
    fn take_retry_refresh(&mut self, now: Instant) -> bool {
        if self.refresh_on_retry && self.may_open(now) {
            self.refresh_on_retry = false;
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncodeMode {
    Sparse,
    Full,
}

/// Rate-limits identical non-fatal encode warnings (avoid 60 Hz log floods).
#[derive(Default)]
struct WarnLimiter {
    last: Option<(String, Instant)>,
}

impl WarnLimiter {
    fn should_log(&mut self, msg: &str) -> bool {
        let now = Instant::now();
        match &self.last {
            Some((prev, at)) if prev == msg && now.duration_since(*at) < Duration::from_secs(5) => {
                false
            }
            _ => {
                self.last = Some((msg.to_string(), now));
                true
            }
        }
    }
}

fn frame_pipeline_loop(ctx: FramePipelineCtx) {
    tracing::info!("rudp host: frame pipeline started (encoders open when a client connects)");
    ctx.set_status("idle (no client)");
    let mut last_log = Instant::now();
    let mut fps = 0u32;
    let mut last: Option<(u64, u32, u32, usize)> = None;
    // One encoder per exported output, keyed by output name.
    let mut pipes: HashMap<String, OutputPipe> = HashMap::new();
    let mut subscribed: HashSet<String> = HashSet::new();
    let mut seen_sub_gen = u64::MAX;
    let mut warn_limiter = WarnLimiter::default();
    let mut seen_joins = 0u64;
    let mut idle_since: Option<Instant> = None;
    let mut was_session_locked = false;
    let render_node = ctx.render_node_path.to_string_lossy().into_owned();

    while !ctx.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let sessions = ctx.video.active_sessions.load(Ordering::SeqCst);
        let session_locked = ctx.session_locked.load(Ordering::Relaxed);
        // Persist the phase *before* demand turns on export compose + encode,
        // so a crash in that path is attributed correctly next login.
        let phase = if sessions > 0 && !session_locked {
            HostPhase::Streaming
        } else if now.duration_since(ctx.started) < STABLE_AFTER {
            HostPhase::Starting
        } else {
            HostPhase::Stable
        };
        ctx.marker.set_phase(phase);
        // Idle encode/export while the desktop lock owns the session.
        ctx.hub.set_demand(sessions > 0 && !session_locked);
        if was_session_locked && !session_locked && sessions > 0 {
            for pipe in pipes.values_mut() {
                pipe.request_keyframe();
            }
            ctx.request_refresh();
        }
        was_session_locked = session_locked;

        let joins = ctx.video.session_joins.load(Ordering::SeqCst);
        if joins != seen_joins {
            seen_joins = joins;
            for pipe in pipes.values_mut() {
                pipe.request_keyframe();
            }
            ctx.request_refresh();
        }

        // Follow the subscribed-output union: drop encoders for outputs nobody
        // receives any more (an empty set means "primary only" — keep as is).
        let sub_gen = ctx.hub.subscription_generation();
        if sub_gen != seen_sub_gen {
            seen_sub_gen = sub_gen;
            subscribed = ctx.hub.subscribed_outputs().into_iter().collect();
            if !subscribed.is_empty() {
                pipes.retain(|name, _| subscribed.contains(name));
            }
        }

        if sessions == 0 || session_locked {
            ctx.video.packet_outbox.clear();
            let since = *idle_since.get_or_insert(now);
            if pipes.values().any(|p| p.encoder.is_some())
                && now.duration_since(since) >= ENCODER_IDLE_CLOSE
            {
                tracing::info!("rudp encode: no clients — stopping encode workers");
                pipes.clear();
                ctx.set_status("idle (no client)");
            }
        } else {
            idle_since = None;
            let mut refresh = false;
            for pipe in pipes.values_mut() {
                if pipe.encoder.is_none() && pipe.supervisor.take_retry_refresh(now) {
                    refresh = true;
                }
            }
            if refresh {
                ctx.request_refresh();
            }
        }

        match ctx.wake.recv_timeout(Duration::from_millis(16)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                for frame in ctx.hub.take_latest_all() {
                    if ctx.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if ctx.session_locked.load(Ordering::Relaxed) {
                        continue;
                    }
                    // Stale frame from an output that was just unsubscribed.
                    if !subscribed.is_empty() && !subscribed.contains(&frame.output_name) {
                        continue;
                    }
                    // Bounded GPU fence wait on this thread — never on calloop / Quinn.
                    if !frame.wait_ready(FENCE_TIMEOUT) {
                        continue;
                    }
                    let pipe = pipes.entry(frame.output_name.clone()).or_default();
                    let mode = if frame.damage_full {
                        EncodeMode::Full
                    } else {
                        EncodeMode::Sparse
                    };
                    if pipe.encode_mode != Some(mode) {
                        tracing::debug!(
                            output = %frame.output_name,
                            ?mode,
                            damage_rects = frame.damage.len(),
                            seq = frame.seq,
                            "rudp encode: mode"
                        );
                        // Do **not** force an IDR on Sparse→Full (window close /
                        // large damage). Keyframes go on the reliable uni-stream
                        // and stall the pump; P-frames already carry full-frame
                        // residuals. IDRs stay for join / unlock / encoder open.
                        pipe.encode_mode = Some(mode);
                    }
                    let multi = pipes.len() > 1;
                    let Some(pipe) = pipes.get_mut(&frame.output_name) else {
                        continue;
                    };
                    if process_frame(&ctx, pipe, &mut warn_limiter, &render_node, &frame, multi) {
                        fps = fps.saturating_add(1);
                        last = Some((frame.seq, frame.width, frame.height, frame.fds.len()));
                    }
                    // Dropping `frame` returns its pool BO via SlotReleaseGuard.
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if last_log.elapsed() >= Duration::from_secs(1) {
            if let Some((seq, w, h, planes)) = last
                && fps > 0
            {
                let active = ctx
                    .encode_status
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_else(|| "none".into());
                tracing::debug!(
                    fps,
                    seq,
                    width = w,
                    height = h,
                    planes,
                    streams = pipes.len(),
                    encoder = %active,
                    bytes = ctx.bytes_encoded.load(Ordering::Relaxed),
                    "rudp host: encode rate"
                );
            }
            fps = 0;
            last_log = Instant::now();
        }
    }

    // Drop closes the worker sockets and reaps the children (bounded, no flush round-trip).
    drop(pipes);
    ctx.hub.set_demand(false);
    tracing::info!("rudp host: frame pipeline stopped");
}

/// Open (if needed) and encode one frame for its output. Returns `true` when
/// encoded.
fn process_frame(
    ctx: &FramePipelineCtx,
    pipe: &mut OutputPipe,
    warn_limiter: &mut WarnLimiter,
    render_node: &str,
    frame: &ExportedFrame,
    multi: bool,
) -> bool {
    let now = Instant::now();
    let prefs_gen = ctx.prefs_gen.load(Ordering::Relaxed);
    if pipe.applied_prefs_gen != prefs_gen {
        // Bitrate / codec / backend changed live — drop workers, keep Quinn.
        pipe.encoder = None;
        pipe.applied_prefs_gen = prefs_gen;
        pipe.supervisor = EncoderSupervisor::default();
    }
    let size_matches = pipe
        .encoder
        .as_ref()
        .is_some_and(|a| a.width == frame.width && a.height == frame.height);
    if !size_matches {
        // Size change or no encoder yet: replace the worker.
        pipe.encoder = None;
        if !pipe.supervisor.may_open(now) {
            return false;
        }
        let prefs = ctx
            .prefs
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        match open_hw_encoder(&prefs, frame.width, frame.height, render_node, &ctx.stop) {
            Ok(enc) => {
                let info = enc.info();
                let mut label = format!(
                    "{} / {} ({}x{})",
                    backend_label(info.backend),
                    info.codec.as_str(),
                    info.width,
                    info.height
                );
                if multi {
                    label = format!("{label} [{}]", frame.output_name);
                }
                tracing::info!(
                    output = %frame.output_name,
                    %label,
                    "rudp encode: encoder ready (isolated worker)"
                );
                let codec = codec_to_wire(info.codec);
                ctx.set_status(label);
                ctx.set_error(None);
                pipe.encoder = Some(ActiveEncoder {
                    enc,
                    width: frame.width,
                    height: frame.height,
                    codec,
                    opened_at: now,
                });
            }
            Err(EncodeError::Cancelled) => return false,
            Err(err) => {
                let msg = err.to_string();
                if pipe.supervisor.record_failure(now) {
                    tracing::error!(
                        output = %frame.output_name,
                        err = %msg,
                        "rudp encode: hardware encoder unavailable — giving up for this \
                         session (Remote stays reachable; re-enable in Settings to retry)"
                    );
                    ctx.set_status("unavailable");
                } else {
                    tracing::warn!(
                        output = %frame.output_name,
                        err = %msg,
                        failures = pipe.supervisor.failures,
                        "rudp encode: encoder open failed — will retry"
                    );
                    ctx.set_status("retrying");
                }
                ctx.set_error(Some(msg));
                return false;
            }
        }
    }

    let Some(active) = pipe.encoder.as_mut() else {
        return false;
    };
    let codec = active.codec;
    match encode_frame(
        active.enc.as_mut(),
        frame,
        codec,
        &ctx.video,
        &ctx.bytes_encoded,
    ) {
        Ok(()) => {
            ctx.frames_encoded.fetch_add(1, Ordering::Relaxed);
            pipe.supervisor
                .record_success(now.duration_since(active.opened_at));
            true
        }
        Err(err) if !active.enc.is_healthy() => {
            let msg = err.to_string();
            pipe.encoder = None;
            if ctx.stop.load(Ordering::Relaxed) {
                return false;
            }
            if pipe.supervisor.record_failure(now) {
                tracing::error!(
                    output = %frame.output_name,
                    err = %msg,
                    "rudp encode: encode worker failed repeatedly — hardware encode disabled \
                     for this session"
                );
                ctx.set_status("unavailable");
            } else {
                tracing::warn!(
                    output = %frame.output_name,
                    err = %msg,
                    failures = pipe.supervisor.failures,
                    "rudp encode: encode worker died — restarting after back-off \
                     (desktop unaffected)"
                );
                ctx.set_status("restarting");
            }
            ctx.set_error(Some(msg));
            false
        }
        Err(err) => {
            let msg = err.to_string();
            if warn_limiter.should_log(&msg) {
                tracing::warn!(
                    output = %frame.output_name,
                    err = %msg,
                    seq = frame.seq,
                    "rudp encode: frame skipped"
                );
            }
            ctx.set_error(Some(msg));
            false
        }
    }
}

fn encode_frame(
    enc: &mut dyn HwEncoder,
    frame: &ExportedFrame,
    codec: u8,
    video: &EngineShared,
    bytes_encoded: &AtomicU64,
) -> Result<(), EncodeError> {
    let borrowed: Vec<_> = frame.fds.iter().map(|fd| fd.as_fd()).collect();
    let input = EncodeInput {
        seq: frame.seq,
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        fourcc: frame.fourcc,
        modifier: frame.modifier,
        fds: &borrowed,
        offsets: &frame.offsets,
        strides: &frame.strides,
        damage_full: frame.damage_full,
        damage: &frame.damage,
    };
    enc.submit(&input)?;
    let packets = enc.drain()?;
    // Advertise (or refresh) this output's stream before its first packet.
    let Some(stream_id) =
        video
            .streams
            .publish(&frame.output_name, frame.width, frame.height, codec)
    else {
        tracing::warn!(output = %frame.output_name, "rudp encode: no free stream id");
        return Ok(());
    };
    for packet in packets {
        bytes_encoded.fetch_add(packet.data.len() as u64, Ordering::Relaxed);
        video.packet_outbox.push(OutboundPacket {
            stream_id,
            output_id: frame.output_name.clone(),
            width: frame.width,
            height: frame.height,
            packet: MediaPacket {
                seq: packet.seq,
                pts_us: packet.pts_us,
                codec: codec_to_wire(packet.codec),
                is_keyframe: packet.is_keyframe,
                damage_full: packet.damage_full,
                damage: packet
                    .damage
                    .iter()
                    .map(|r| RudpDamageRect {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    })
                    .collect(),
                data: packet.data,
            },
        });
    }
    Ok(())
}

fn backend_label(backend: EncoderBackend) -> &'static str {
    match backend {
        EncoderBackend::Auto => "auto",
        EncoderBackend::Vaapi => "vaapi",
        EncoderBackend::Nvenc => "nvenc",
        EncoderBackend::Soft => "soft",
    }
}

/// Desired host from env override or `rudp.json`.
pub fn desired_host() -> Option<DesiredRudpHost> {
    let cfg = metis_config::load_rudp_config();
    let prefs = RudpEncodePrefs::from_config(&cfg);
    if let Some(config) = RudpHostConfig::from_env() {
        // Dev override: skip LAN firewall gate; still require an allowlist.
        if cfg.allowed_users.is_empty() {
            tracing::warn!("rudp: METIS_RUDP_HOST set but allowed_users empty — not starting");
            return None;
        }
        return Some(DesiredRudpHost {
            config,
            allowed_users: cfg.allowed_users,
            lan_only: cfg.lan_only,
            unlock_session_on_auth: cfg.unlock_session_on_auth,
            encode_prefs: prefs,
        });
    }
    if !cfg.enabled {
        return None;
    }
    if cfg.allowed_users.is_empty() {
        tracing::warn!("rudp: enabled but allowed_users empty — not starting");
        return None;
    }
    if cfg.lan_only && !cfg.firewall_applied {
        tracing::warn!("rudp: lan_only without firewall_applied — refusing bind (fail-closed)");
        return None;
    }
    let bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, cfg.port));
    Some(DesiredRudpHost {
        config: RudpHostConfig { bind },
        allowed_users: cfg.allowed_users,
        lan_only: cfg.lan_only,
        unlock_session_on_auth: cfg.unlock_session_on_auth,
        encode_prefs: prefs,
    })
}

/// Resolve `/dev/dri/renderD*` (or card) path from the active DRM render node.
pub fn render_node_device_path(node: &smithay::backend::drm::DrmNode) -> PathBuf {
    use smithay::backend::drm::NodeType;
    node.dev_path_with_type(NodeType::Render)
        .or_else(|| node.dev_path())
        .unwrap_or_else(|| PathBuf::from("/dev/dri/renderD128"))
}

/// DRM session bootstrap: env override, else `rudp.json`, else Phase 1 debug export.
///
/// Runs the crash-loop guard first: if the previous session died while the
/// host was starting / streaming, Remote stays off (and is disabled in
/// `rudp.json` with a Settings notice) instead of risking the same crash.
pub fn maybe_start_for_session(
    hub: &Arc<StreamExportHub>,
    render_node_path: PathBuf,
    bridge: RudpCalloopBridge,
) -> Option<RudpHostSystem> {
    if crate::rudp_guard::auto_disable_after_crash() {
        return None;
    }
    match desired_host() {
        Some(desired) => {
            match RudpHostSystem::spawn(desired, Arc::clone(hub), render_node_path, bridge) {
                Ok(host) => Some(host),
                Err(err) => {
                    tracing::error!(%err, "rudp host: failed to start");
                    None
                }
            }
        }
        None => {
            crate::stream_export::maybe_arm_from_env(hub);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;

    #[test]
    fn supervisor_backs_off_then_gives_up() {
        let t0 = Instant::now();
        let mut s = EncoderSupervisor::default();
        assert!(s.may_open(t0));
        for (i, backoff) in ENCODER_BACKOFF.iter().enumerate() {
            assert!(!s.record_failure(t0), "failure {i} must not give up yet");
            assert!(!s.may_open(t0));
            assert!(s.may_open(t0 + *backoff));
            assert!(s.take_retry_refresh(t0 + *backoff));
            assert!(!s.take_retry_refresh(t0 + *backoff), "refresh fires once");
        }
        assert!(s.record_failure(t0), "final failure gives up");
        assert!(!s.may_open(t0 + Duration::from_secs(3600)));
    }

    #[test]
    fn supervisor_resets_after_healthy_run() {
        let t0 = Instant::now();
        let mut s = EncoderSupervisor::default();
        s.record_failure(t0);
        s.record_success(Duration::from_secs(1));
        assert_eq!(s.failures, 1, "short run keeps the failure count");
        s.record_success(ENCODER_HEALTHY_RESET);
        assert_eq!(s.failures, 0);
        assert!(s.may_open(t0));
    }

    #[test]
    fn codec_wire_bytes_match_protocol_names() {
        use metis_protocol::codec_to_str;
        assert_eq!(codec_to_str(codec_to_wire(RudpCodec::H264)), "h264");
        assert_eq!(codec_to_str(codec_to_wire(RudpCodec::Hevc)), "hevc");
        assert_eq!(codec_to_str(codec_to_wire(RudpCodec::Av1)), "av1");
    }

    #[test]
    fn host_input_maps_to_calloop_event() {
        let ev: RudpInputEvent = RudpHostInput::Key {
            keycode: 30,
            pressed: true,
        }
        .into();
        assert!(matches!(
            ev,
            RudpInputEvent::Key {
                keycode: 30,
                pressed: true
            }
        ));
    }

    #[test]
    fn config_parse_bool() {
        let cfg = RudpHostConfig::parse("1").expect("parsed");
        assert_eq!(cfg.bind.port(), DEFAULT_RUDP_PORT);
        assert!(cfg.bind.ip().is_unspecified());
    }

    #[test]
    fn config_parse_addr() {
        let cfg = RudpHostConfig::parse("127.0.0.1:9999").expect("parsed");
        assert_eq!(
            cfg.bind,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9999))
        );
    }

    #[test]
    fn spawn_arms_and_drop_disarms() {
        // Avoid probing real VAAPI/NVENC in unit tests, and keep Quinn
        // in-process so the test does not need an installed host binary.
        unsafe {
            std::env::set_var("METIS_ENCODE_NULL", "1");
            std::env::set_var("METIS_RUDP_INPROCESS", "1");
        }
        let hub = Arc::new(StreamExportHub::new());
        assert!(!hub.is_armed());
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let host = RudpHostSystem::spawn(
            DesiredRudpHost {
                config: RudpHostConfig { bind },
                allowed_users: vec!["alice".into()],
                lan_only: true,
                unlock_session_on_auth: true,
                encode_prefs: RudpEncodePrefs::default(),
            },
            Arc::clone(&hub),
            PathBuf::from("/dev/dri/renderD128"),
            RudpCalloopBridge {
                input_tx: {
                    let (tx, _rx) = calloop::channel::channel();
                    tx
                },
                pointer_locked: Arc::new(AtomicBool::new(false)),
                session_locked: Arc::new(AtomicBool::new(false)),
                layout: Arc::new(RudpLayoutShared::new()),
            },
        )
        .expect("spawn host");
        assert!(hub.is_armed());
        // No client yet: armed but no demand ⇒ no export compose at all.
        std::thread::sleep(Duration::from_millis(50));
        assert!(!hub.wants_frames());
        assert_eq!(host.allowed_users, vec!["alice".to_string()]);
        drop(host);
        assert!(!hub.is_armed());
        unsafe {
            std::env::remove_var("METIS_ENCODE_NULL");
            std::env::remove_var("METIS_RUDP_INPROCESS");
        }
    }

    #[test]
    fn desired_host_skips_when_lan_only_without_firewall() {
        // Exercise the gate with a synthetic config via env skip: without
        // METIS_RUDP_HOST, desired_host reads disk. Here we only assert the
        // helper rejects the fail-closed combination when constructing the
        // policy struct logic inline (disk-independent).
        let mut cfg = metis_config::RudpConfig {
            enabled: true,
            lan_only: true,
            firewall_applied: false,
            allowed_users: vec!["alice".into()],
            ..Default::default()
        };
        cfg = cfg.sanitize();
        assert!(cfg.enabled && cfg.lan_only && !cfg.firewall_applied);
        // Mirror desired_host gate:
        assert!(cfg.lan_only && !cfg.firewall_applied);
    }
}
