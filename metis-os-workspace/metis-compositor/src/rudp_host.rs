//! Phase 2–7 RUDP host: Quinn listener + hardware encode on an isolated
//! Tokio / worker thread pair — **never** on the Smithay calloop thread.
//!
//! Crash containment (enabling Metis Remote must never take down the session):
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
//! Frame path: wake → `take_latest` → bounded fence wait → worker encode →
//! latest-wins outbound packet slot. Phase 6 fans packets to authenticated
//! sessions (keyframes on uni-stream, deltas as FEC'd datagrams). Phase 7 maps
//! control-stream input onto calloop → [`crate::remote_input`] and advertises
//! Wayland pointer lock.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use metis_config::{RudpEncoderBackend, RudpVideoCodec};
use metis_encode::{
    DEFAULT_BITRATE_KBPS, DEFAULT_FPS_HINT, EncodeError, EncodeInput, EncodedPacket,
    EncoderBackend, EncoderConfig, HwEncoder, NullEncoder, RudpCodec, open_isolated_encoder,
};
use metis_protocol::{
    RUDP_DEFAULT_DATAGRAM_BUDGET, RUDP_PROTOCOL_VERSION, ReliableAccessUnit, RudpControlMsg,
    RudpDamageRect, RudpRejectReason, build_media_datagrams, codec_from_str, encode_rudp_frame,
    try_decode_rudp_frame,
};
use quinn::Endpoint;
use smithay::reexports::calloop;
use std::sync::RwLock as StdRwLock;
use tokio::sync::RwLock as AsyncRwLock;

use crate::pam_auth::{pam_check, pam_service};
use crate::rudp_guard::{HostMarker, HostPhase, STABLE_AFTER};
use crate::rudp_identity;
use crate::stream_export::{ExportedFrame, StreamExportHub};

/// Default QUIC listen port for Metis RUDP.
pub const DEFAULT_RUDP_PORT: u16 = 7843;

/// Upper bound for one reliable write to a client (control / keyframe).
const PUMP_WRITE_TIMEOUT: Duration = Duration::from_millis(750);
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

/// Input events from the Quinn control stream, marshaled onto calloop.
#[derive(Debug, Clone, Copy)]
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
    /// Not input: the frame pipeline needs a full frame now (client joined /
    /// encoder restarted) even if the desktop is idle — repaint + force export.
    RefreshVideo,
}

impl RudpInputEvent {
    fn from_control(msg: &RudpControlMsg) -> Option<Self> {
        match *msg {
            RudpControlMsg::PointerAbsolute { x, y } => Some(Self::PointerAbsolute { x, y }),
            RudpControlMsg::PointerRelative { dx, dy } => Some(Self::PointerRelative { dx, dy }),
            RudpControlMsg::PointerButton { button, pressed } => {
                Some(Self::PointerButton { button, pressed })
            }
            RudpControlMsg::PointerScroll { dx, dy } => Some(Self::PointerScroll { dx, dy }),
            RudpControlMsg::Key { keycode, pressed } => Some(Self::Key { keycode, pressed }),
            _ => None,
        }
    }
}

/// Calloop inject sender + shared pointer-lock flag for the Quinn runtime.
#[derive(Clone)]
pub struct RudpCalloopBridge {
    pub input_tx: calloop::channel::Sender<RudpInputEvent>,
    pub pointer_locked: Arc<AtomicBool>,
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

/// Owns the Quinn accept thread + frame pipeline thread. Drop ⇒ disarm export.
pub struct RudpHostSystem {
    stop: Arc<AtomicBool>,
    hub: Arc<StreamExportHub>,
    quinn_join: Option<JoinHandle<()>>,
    frame_join: Option<JoinHandle<()>>,
    /// Frames submitted to the hardware encoder (for tests / metrics).
    pub frames_encoded: Arc<AtomicU64>,
    /// Bytes produced by encode drain (approx outbound payload size).
    pub bytes_encoded: Arc<AtomicU64>,
    /// Latest encoded packet for Phase 6 datagram send (latest-wins).
    pub latest_packet: Arc<Mutex<Option<EncodedPacket>>>,
    /// Last encoder open error (status / Settings).
    pub encode_error: Arc<Mutex<Option<String>>>,
    /// Active encoder backend/codec label after open (best-effort).
    pub encode_status: Arc<Mutex<Option<String>>>,
    /// Listen address currently bound.
    pub bind: SocketAddr,
    /// Local PAM usernames allowed to authenticate (Phase 4).
    pub allowed_users: Vec<String>,
    /// Live allowlist shared with the Quinn auth task (updated on ReloadRudp).
    pub allowed_users_live: Arc<StdRwLock<Vec<String>>>,
    pub lan_only: bool,
    /// SHA-256 fingerprint of the persistent host cert.
    pub fingerprint: String,
    pub encode_prefs: RudpEncodePrefs,
    pub render_node_path: PathBuf,
    /// Shared with calloop: true when a Wayland pointer lock is active.
    pub pointer_locked: Arc<AtomicBool>,
    /// Crash-loop guard marker; dropped (file removed) after threads join.
    _marker: Arc<HostMarker>,
}

impl RudpHostSystem {
    /// Arm the export hub and start Quinn + frame workers on dedicated OS threads.
    pub fn spawn(
        config: RudpHostConfig,
        hub: Arc<StreamExportHub>,
        allowed_users: Vec<String>,
        lan_only: bool,
        encode_prefs: RudpEncodePrefs,
        render_node_path: PathBuf,
        bridge: RudpCalloopBridge,
    ) -> Result<Self, String> {
        ensure_rustls_provider();

        let stop = Arc::new(AtomicBool::new(false));
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let bytes_encoded = Arc::new(AtomicU64::new(0));
        let latest_packet = Arc::new(Mutex::new(None));
        let encode_error = Arc::new(Mutex::new(None));
        let encode_status = Arc::new(Mutex::new(None));
        let wake = hub.arm();

        let marker = Arc::new(HostMarker::create());
        let allowed_users_live = Arc::new(StdRwLock::new(allowed_users.clone()));
        let video = Arc::new(VideoShared {
            latest_packet: Arc::clone(&latest_packet),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            codec: AtomicU8::new(codec_to_u8(to_encode_codec(encode_prefs.codec))),
            active_sessions: AtomicUsize::new(0),
            session_joins: AtomicU64::new(0),
        });
        let quinn_stop = Arc::clone(&stop);
        let quinn_allowed = Arc::clone(&allowed_users_live);
        let quinn_video = Arc::clone(&video);
        let quinn_bridge = bridge.clone();
        let pointer_locked = Arc::clone(&bridge.pointer_locked);
        let frame_ctx = FramePipelineCtx {
            hub: Arc::clone(&hub),
            wake,
            stop: Arc::clone(&stop),
            frames_encoded: Arc::clone(&frames_encoded),
            bytes_encoded: Arc::clone(&bytes_encoded),
            encode_error: Arc::clone(&encode_error),
            encode_status: Arc::clone(&encode_status),
            prefs: encode_prefs.clone(),
            render_node_path: render_node_path.clone(),
            video: Arc::clone(&video),
            refresh_tx: bridge.input_tx.clone(),
            marker: Arc::clone(&marker),
            started: Instant::now(),
        };
        let frame_join = std::thread::Builder::new()
            .name("metis-rudp-frames".into())
            .spawn(move || frame_pipeline_loop(frame_ctx))
            .map_err(|e| {
                hub.disarm();
                format!("spawn rudp frame thread: {e}")
            })?;

        let quinn_join = match std::thread::Builder::new()
            .name("metis-rudp-quinn".into())
            .spawn(move || {
                if let Err(err) =
                    run_quinn_runtime(config, quinn_stop, quinn_allowed, quinn_video, quinn_bridge)
                {
                    tracing::error!(%err, "rudp host: Quinn runtime exited with error");
                }
            }) {
            Ok(handle) => handle,
            Err(err) => {
                // Don't leak a running frame thread / armed hub on partial start.
                stop.store(true, Ordering::SeqCst);
                hub.disarm();
                let _ = frame_join.join();
                return Err(format!("spawn rudp quinn thread: {err}"));
            }
        };

        // Ensure identity before logging fingerprint (Quinn thread also loads it).
        let fingerprint = match rudp_identity::load_or_create_server_config() {
            Ok((_, fp)) => fp,
            Err(err) => {
                tracing::warn!(%err, "rudp host: identity preview failed");
                String::new()
            }
        };

        tracing::info!(
            bind = %config.bind,
            allowed = ?allowed_users,
            lan_only,
            encoder = encode_prefs.backend.as_str(),
            codec = encode_prefs.codec.as_str(),
            bitrate_kbps = encode_prefs.bitrate_kbps,
            render_node = %render_node_path.display(),
            fingerprint = %fingerprint,
            "rudp host: started (Quinn + hardware encode + PAM auth + input)"
        );

        Ok(Self {
            stop,
            hub,
            quinn_join: Some(quinn_join),
            frame_join: Some(frame_join),
            frames_encoded,
            bytes_encoded,
            latest_packet,
            encode_error,
            encode_status,
            bind: config.bind,
            allowed_users,
            allowed_users_live,
            lan_only,
            fingerprint,
            encode_prefs,
            render_node_path,
            pointer_locked,
            _marker: marker,
        })
    }

    /// Signal stop, disarm export (unblocks frame wake), join workers.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.hub.disarm();
        if let Some(handle) = self.quinn_join.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.frame_join.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RudpHostSystem {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn ensure_rustls_provider() {
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        // Already installed by another component — fine.
    }
}

const CODEC_H264: u8 = 0;
const CODEC_HEVC: u8 = 1;

fn codec_to_u8(codec: RudpCodec) -> u8 {
    match codec {
        RudpCodec::H264 => CODEC_H264,
        RudpCodec::Hevc => CODEC_HEVC,
    }
}

fn codec_wire_name(raw: u8) -> &'static str {
    if raw == CODEC_HEVC { "hevc" } else { "h264" }
}

/// State shared by the frame pipeline thread and the Quinn runtime.
struct VideoShared {
    latest_packet: Arc<Mutex<Option<EncodedPacket>>>,
    /// Current encode size (0 until the first frame is encoded).
    width: AtomicU32,
    height: AtomicU32,
    /// Codec of the encoder actually running (may differ from the preference
    /// after a fallback) — advertised to clients in `VideoReady`.
    codec: AtomicU8,
    /// Authenticated sessions; > 0 ⇒ export + encode demand.
    active_sessions: AtomicUsize,
    /// Monotonic join counter — a change makes the frame thread emit an IDR.
    session_joins: AtomicU64,
}

/// Counts one authenticated session for as long as it lives (incl. task cancel).
struct SessionGuard(Arc<VideoShared>);

impl SessionGuard {
    fn new(video: &Arc<VideoShared>) -> Self {
        video.active_sessions.fetch_add(1, Ordering::SeqCst);
        video.session_joins.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(video))
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.active_sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

fn run_quinn_runtime(
    config: RudpHostConfig,
    stop: Arc<AtomicBool>,
    allowed_users: Arc<StdRwLock<Vec<String>>>,
    video: Arc<VideoShared>,
    bridge: RudpCalloopBridge,
) -> Result<(), String> {
    ensure_rustls_provider();
    let (server_config, fingerprint) = rudp_identity::load_or_create_server_config()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .thread_name("metis-rudp-tok")
        .build()
        .map_err(|e| format!("rudp tokio runtime: {e}"))?;

    let result = runtime.block_on(async move {
        let endpoint = Endpoint::server(server_config, config.bind)
            .map_err(|e| format!("rudp bind {}: {e}", config.bind))?;
        tracing::info!(
            local = %endpoint.local_addr().unwrap_or(config.bind),
            %fingerprint,
            "rudp host: Quinn listening"
        );

        let sessions: SessionRegistry = Arc::new(AsyncRwLock::new(HashMap::new()));
        let pump_stop = Arc::clone(&stop);
        let pump_sessions = Arc::clone(&sessions);
        let pump_video = Arc::clone(&video);
        let pump_lock = Arc::clone(&bridge.pointer_locked);
        tokio::spawn(async move {
            video_pump_loop(pump_stop, pump_sessions, pump_video, pump_lock).await;
        });

        connection_broker_loop(endpoint, stop, allowed_users, sessions, video, bridge).await;
        Ok::<(), String>(())
    });
    // Plain drop waits forever for spawn_blocking (PAM) tasks; shutdown runs
    // on calloop via RudpHostSystem::shutdown, so it must be bounded.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

struct VideoSession {
    conn: quinn::Connection,
    control_send: tokio::sync::Mutex<quinn::SendStream>,
    video_send: tokio::sync::Mutex<quinn::SendStream>,
    video_ready_sent: AtomicBool,
    warned_no_dgram: AtomicBool,
}

type SessionRegistry = Arc<AsyncRwLock<HashMap<String, Arc<VideoSession>>>>;

async fn connection_broker_loop(
    endpoint: Endpoint,
    stop: Arc<AtomicBool>,
    allowed_users: Arc<StdRwLock<Vec<String>>>,
    sessions: SessionRegistry,
    video: Arc<VideoShared>,
    bridge: RudpCalloopBridge,
) {
    while !stop.load(Ordering::Relaxed) {
        let incoming = tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            incoming = endpoint.accept() => incoming,
        };
        let Some(connecting) = incoming else {
            break;
        };
        let allowed = Arc::clone(&allowed_users);
        let sessions = Arc::clone(&sessions);
        let video = Arc::clone(&video);
        let bridge = bridge.clone();
        tokio::spawn(async move {
            match connecting.await {
                Ok(conn) => {
                    let peer = conn.remote_address();
                    tracing::info!(%peer, "rudp host: TLS connected — starting auth");
                    match authenticate_connection(&conn, &allowed).await {
                        Ok((session_id, mut control_send, mut control_recv)) => {
                            tracing::info!(%peer, %session_id, "rudp host: session authenticated");
                            // Advertise current pointer-lock so the client can pick
                            // absolute vs relative before the next pump tick.
                            let locked = bridge.pointer_locked.load(Ordering::Relaxed);
                            let _ = write_control_msg(
                                &mut control_send,
                                &RudpControlMsg::PointerLock { locked },
                            )
                            .await;
                            let video_send = match conn.open_uni().await {
                                Ok(s) => s,
                                Err(err) => {
                                    tracing::warn!(%peer, %err, "rudp host: open_uni failed");
                                    conn.close(0u32.into(), b"video stream failed");
                                    return;
                                }
                            };
                            let session = Arc::new(VideoSession {
                                conn: conn.clone(),
                                control_send: tokio::sync::Mutex::new(control_send),
                                video_send: tokio::sync::Mutex::new(video_send),
                                video_ready_sent: AtomicBool::new(false),
                                warned_no_dgram: AtomicBool::new(false),
                            });
                            {
                                let mut guard = sessions.write().await;
                                guard.insert(session_id.clone(), Arc::clone(&session));
                            }
                            // Demand on (frame thread starts export + IDR); off on drop.
                            let _session_count = SessionGuard::new(&video);
                            let input_tx = bridge.input_tx.clone();
                            // Control reads: keepalives + Phase 7 input → calloop.
                            tokio::spawn(async move {
                                let mut buf = Vec::new();
                                let mut tmp = [0u8; 2048];
                                while let Ok(Some(n)) = control_recv.read(&mut tmp).await {
                                    buf.extend_from_slice(&tmp[..n]);
                                    while let Ok(Some((msg, n))) = try_decode_rudp_frame(&buf) {
                                        buf.drain(..n);
                                        if let Some(ev) = RudpInputEvent::from_control(&msg) {
                                            let _ = input_tx.send(ev);
                                        }
                                    }
                                }
                            });
                            conn.closed().await;
                            sessions.write().await.remove(&session_id);
                            tracing::info!(%peer, %session_id, "rudp host: client disconnected");
                        }
                        Err(err) => {
                            tracing::warn!(%peer, %err, "rudp host: auth failed");
                            // Reject (if written) must reach the client before
                            // CONNECTION_CLOSE, or the Viewer only sees
                            // "read: connection lost".
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            conn.close(0u32.into(), b"auth failed");
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!(?err, "rudp host: handshake failed");
                }
            }
        });
    }
    endpoint.close(0u32.into(), b"rudp host shutdown");
}

async fn video_pump_loop(
    stop: Arc<AtomicBool>,
    sessions: SessionRegistry,
    video: Arc<VideoShared>,
    pointer_locked: Arc<AtomicBool>,
) {
    tracing::info!("rudp host: video pump started");
    let mut last_seq: Option<u64> = None;
    let mut last_ready = (0u32, 0u32, CODEC_H264);
    let mut last_pointer_lock: Option<bool> = None;

    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(4)).await;

        let locked = pointer_locked.load(Ordering::Relaxed);
        if last_pointer_lock != Some(locked) {
            let snap: Vec<Arc<VideoSession>> = {
                let guard = sessions.read().await;
                guard.values().cloned().collect()
            };
            let msg = RudpControlMsg::PointerLock { locked };
            for session in &snap {
                let mut send = session.control_send.lock().await;
                let _ = write_control_msg(&mut send, &msg).await;
            }
            last_pointer_lock = Some(locked);
        }

        let w = video.width.load(Ordering::Relaxed);
        let h = video.height.load(Ordering::Relaxed);
        let codec_raw = video.codec.load(Ordering::Relaxed);
        if w > 0 && h > 0 {
            let changed = (w, h, codec_raw) != last_ready;
            let snap: Vec<Arc<VideoSession>> = {
                let guard = sessions.read().await;
                guard.values().cloned().collect()
            };
            let msg = RudpControlMsg::VideoReady {
                width: w,
                height: h,
                codec: codec_wire_name(codec_raw).to_string(),
            };
            for session in &snap {
                // Late joiners need VideoReady too, not just size/codec changes.
                if !changed && session.video_ready_sent.load(Ordering::Relaxed) {
                    continue;
                }
                let mut send = session.control_send.lock().await;
                if write_control_msg(&mut send, &msg).await.is_ok() {
                    session.video_ready_sent.store(true, Ordering::Relaxed);
                }
            }
            last_ready = (w, h, codec_raw);
        }

        let packet = {
            let Ok(guard) = video.latest_packet.lock() else {
                continue;
            };
            guard.clone()
        };
        let Some(pkt) = packet else {
            continue;
        };
        if last_seq == Some(pkt.seq) {
            continue;
        }
        last_seq = Some(pkt.seq);

        let snap: Vec<Arc<VideoSession>> = {
            let guard = sessions.read().await;
            guard.values().cloned().collect()
        };
        if snap.is_empty() {
            continue;
        }

        let codec = match pkt.codec {
            RudpCodec::H264 => codec_from_str("h264"),
            RudpCodec::Hevc => codec_from_str("hevc"),
        };

        if pkt.is_keyframe {
            let au = ReliableAccessUnit {
                frame_seq: pkt.seq,
                pts_us: pkt.pts_us,
                codec,
                damage_full: pkt.damage_full,
                width: w,
                height: h,
                damage: pkt
                    .damage
                    .iter()
                    .map(|r| RudpDamageRect {
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    })
                    .collect(),
                data: pkt.data.clone(),
            };
            let framed = au.encode_framed();
            for session in &snap {
                let mut send = session.video_send.lock().await;
                // A client that stops reading must not stall the pump for everyone.
                match tokio::time::timeout(PUMP_WRITE_TIMEOUT, send.write_all(&framed)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => tracing::debug!(%err, "rudp host: keyframe write failed"),
                    Err(_) => {
                        tracing::warn!("rudp host: keyframe write timed out — closing slow client");
                        session.conn.close(0u32.into(), b"client too slow");
                    }
                }
            }
            continue;
        }

        for session in &snap {
            let max_dg = session
                .conn
                .max_datagram_size()
                .unwrap_or(RUDP_DEFAULT_DATAGRAM_BUDGET);
            let stats = session.conn.stats();
            let sent = stats.path.sent_packets.max(1);
            let loss_ratio = stats.path.lost_packets as f64 / sent as f64;
            let dgrams = match build_media_datagrams(
                &pkt.data,
                pkt.seq,
                pkt.pts_us,
                codec,
                pkt.damage_full,
                max_dg,
                loss_ratio,
            ) {
                Ok(d) => d,
                Err(err) => {
                    tracing::warn!(%err, "rudp host: shard/FEC failed");
                    continue;
                }
            };
            for d in dgrams {
                if session.conn.datagram_send_buffer_space() == 0 {
                    break;
                }
                match session.conn.send_datagram(Bytes::from(d)) {
                    Ok(()) => {}
                    Err(quinn::SendDatagramError::UnsupportedByPeer) => {
                        if !session.warned_no_dgram.swap(true, Ordering::Relaxed) {
                            tracing::warn!(
                                "rudp host: peer does not support datagrams — video deltas dropped"
                            );
                        }
                        break;
                    }
                    Err(quinn::SendDatagramError::Disabled) => break,
                    Err(quinn::SendDatagramError::TooLarge) => {
                        tracing::debug!("rudp host: datagram too large — skip shard");
                    }
                    Err(err) => {
                        tracing::debug!(%err, "rudp host: send_datagram failed");
                        break;
                    }
                }
            }
        }
    }
    tracing::info!("rudp host: video pump stopped");
}

async fn authenticate_connection(
    conn: &quinn::Connection,
    allowed_users: &Arc<StdRwLock<Vec<String>>>,
) -> Result<(String, quinn::SendStream, quinn::RecvStream), String> {
    // Client opens the control bi-stream.
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(15), conn.accept_bi())
        .await
        .map_err(|_| "auth: timed out waiting for control stream".to_string())?
        .map_err(|e| format!("auth: accept_bi: {e}"))?;

    let hello = read_control_msg(&mut recv).await?;
    let username = match hello {
        RudpControlMsg::Hello { protocol, username } => {
            if protocol != RUDP_PROTOCOL_VERSION {
                write_control_reject(
                    &mut send,
                    RudpRejectReason::Protocol,
                    Some(format!("unsupported protocol {protocol}")),
                )
                .await;
                return Err(format!("bad protocol {protocol}"));
            }
            username
        }
        other => {
            write_control_reject(
                &mut send,
                RudpRejectReason::Protocol,
                Some("expected hello".into()),
            )
            .await;
            return Err(format!("expected hello, got {other:?}"));
        }
    };

    let allowed = {
        let guard = allowed_users
            .read()
            .map_err(|_| "allowlist lock poisoned".to_string())?;
        guard.iter().any(|u| u == &username)
    };
    if !allowed {
        write_control_reject(&mut send, RudpRejectReason::NotAllowed, None).await;
        return Err(format!("user {username} not in allowlist"));
    }

    let nonce = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    write_control_msg(
        &mut send,
        &RudpControlMsg::AuthChallenge {
            nonce: nonce.clone(),
        },
    )
    .await?;

    let response = read_control_msg(&mut recv).await?;
    let password = match response {
        RudpControlMsg::AuthResponse { password } => password,
        other => {
            write_control_reject(
                &mut send,
                RudpRejectReason::Protocol,
                Some("expected auth_response".into()),
            )
            .await;
            return Err(format!("expected auth_response, got {other:?}"));
        }
    };

    let service = pam_service();
    let user_for_pam = username.clone();
    let ok = tokio::task::spawn_blocking(move || pam_check(&service, &user_for_pam, &password))
        .await
        .map_err(|e| format!("pam join: {e}"))?;

    if !ok {
        // Brief delay against online guessing.
        tokio::time::sleep(Duration::from_millis(400)).await;
        write_control_reject(&mut send, RudpRejectReason::AuthFailed, None).await;
        return Err("pam authentication failed".into());
    }

    let session_id = format!("s-{nonce}");
    write_control_msg(
        &mut send,
        &RudpControlMsg::SessionOk {
            session_id: session_id.clone(),
        },
    )
    .await?;
    Ok((session_id, send, recv))
}

async fn write_control_msg(
    send: &mut quinn::SendStream,
    msg: &RudpControlMsg,
) -> Result<(), String> {
    let bytes = encode_rudp_frame(msg)?;
    tokio::time::timeout(PUMP_WRITE_TIMEOUT, send.write_all(&bytes))
        .await
        .map_err(|_| "control write timed out".to_string())?
        .map_err(|e| format!("control write: {e}"))?;
    Ok(())
}

async fn write_control_reject(
    send: &mut quinn::SendStream,
    reason: RudpRejectReason,
    detail: Option<String>,
) {
    let _ = write_control_msg(
        send,
        &RudpControlMsg::Reject { reason, detail },
    )
    .await;
    // Half-close so the peer's read completes with the Reject frame.
    let _ = send.finish();
}

async fn read_control_msg(recv: &mut quinn::RecvStream) -> Result<RudpControlMsg, String> {
    let mut buf = Vec::with_capacity(256);
    let mut tmp = [0u8; 2048];
    loop {
        if let Some((msg, n)) = try_decode_rudp_frame(&buf)? {
            buf.drain(..n);
            return Ok(msg);
        }
        let n = tokio::time::timeout(Duration::from_secs(30), recv.read(&mut tmp))
            .await
            .map_err(|_| "auth: read timed out".to_string())?
            .map_err(|e| format!("control read: {e}"))?
            .ok_or_else(|| "control stream closed".to_string())?;
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > metis_protocol::RUDP_MAX_FRAME + 4 {
            return Err("control buffer overflow".into());
        }
    }
}

fn to_encode_backend(b: RudpEncoderBackend) -> EncoderBackend {
    match b {
        RudpEncoderBackend::Auto => EncoderBackend::Auto,
        RudpEncoderBackend::Vaapi => EncoderBackend::Vaapi,
        RudpEncoderBackend::Nvenc => EncoderBackend::Nvenc,
    }
}

fn to_encode_codec(c: RudpVideoCodec) -> RudpCodec {
    match c {
        RudpVideoCodec::Hevc => RudpCodec::Hevc,
        RudpVideoCodec::H264 => RudpCodec::H264,
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
    prefs: RudpEncodePrefs,
    render_node_path: PathBuf,
    video: Arc<VideoShared>,
    /// Calloop channel used to request a repaint + full export frame.
    refresh_tx: calloop::channel::Sender<RudpInputEvent>,
    /// Crash-loop guard: phase follows startup / streaming / stable.
    marker: Arc<HostMarker>,
    started: Instant,
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
    opened_at: Instant,
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
    tracing::info!("rudp host: frame pipeline started (encoder opens when a client connects)");
    ctx.set_status("idle (no client)");
    let mut last_log = Instant::now();
    let mut fps = 0u32;
    let mut last: Option<(u64, u32, u32, usize)> = None;
    let mut encoder: Option<ActiveEncoder> = None;
    let mut supervisor = EncoderSupervisor::default();
    let mut warn_limiter = WarnLimiter::default();
    let mut encode_mode: Option<EncodeMode> = None;
    let mut seen_joins = 0u64;
    let mut idle_since: Option<Instant> = None;
    let render_node = ctx.render_node_path.to_string_lossy().into_owned();

    while !ctx.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let sessions = ctx.video.active_sessions.load(Ordering::SeqCst);
        // Persist the phase *before* demand turns on export compose + encode,
        // so a crash in that path is attributed correctly next login.
        let phase = if sessions > 0 {
            HostPhase::Streaming
        } else if now.duration_since(ctx.started) < STABLE_AFTER {
            HostPhase::Starting
        } else {
            HostPhase::Stable
        };
        ctx.marker.set_phase(phase);
        ctx.hub.set_demand(sessions > 0);

        let joins = ctx.video.session_joins.load(Ordering::SeqCst);
        if joins != seen_joins {
            seen_joins = joins;
            if let Some(active) = encoder.as_mut() {
                active.enc.request_keyframe();
            }
            ctx.request_refresh();
        }

        if sessions == 0 {
            let since = *idle_since.get_or_insert(now);
            if encoder.is_some() && now.duration_since(since) >= ENCODER_IDLE_CLOSE {
                tracing::info!("rudp encode: no clients — stopping encode worker");
                encoder = None;
                encode_mode = None;
                ctx.set_status("idle (no client)");
            }
        } else {
            idle_since = None;
            if encoder.is_none() && supervisor.take_retry_refresh(now) {
                ctx.request_refresh();
            }
        }

        match ctx.wake.recv_timeout(Duration::from_millis(16)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                while let Some(frame) = ctx.hub.take_latest() {
                    if ctx.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Bounded GPU fence wait on this thread — never on calloop / Quinn.
                    if !frame.wait_ready(FENCE_TIMEOUT) {
                        continue;
                    }
                    let mode = if frame.damage_full {
                        EncodeMode::Full
                    } else {
                        EncodeMode::Sparse
                    };
                    if encode_mode != Some(mode) {
                        tracing::debug!(
                            ?mode,
                            damage_rects = frame.damage.len(),
                            seq = frame.seq,
                            "rudp encode: mode"
                        );
                        if encode_mode == Some(EncodeMode::Sparse)
                            && mode == EncodeMode::Full
                            && let Some(active) = encoder.as_mut()
                        {
                            active.enc.request_keyframe();
                        }
                        encode_mode = Some(mode);
                    }
                    if process_frame(
                        &ctx,
                        &mut encoder,
                        &mut supervisor,
                        &mut warn_limiter,
                        &render_node,
                        &frame,
                    ) {
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
                    encoder = %active,
                    ?encode_mode,
                    bytes = ctx.bytes_encoded.load(Ordering::Relaxed),
                    "rudp host: encode rate"
                );
            }
            fps = 0;
            last_log = Instant::now();
        }
    }

    // Drop closes the worker socket and reaps the child (bounded, no flush round-trip).
    drop(encoder);
    ctx.hub.set_demand(false);
    tracing::info!("rudp host: frame pipeline stopped");
}

/// Open (if needed) and encode one frame. Returns `true` when encoded.
fn process_frame(
    ctx: &FramePipelineCtx,
    encoder: &mut Option<ActiveEncoder>,
    supervisor: &mut EncoderSupervisor,
    warn_limiter: &mut WarnLimiter,
    render_node: &str,
    frame: &ExportedFrame,
) -> bool {
    let now = Instant::now();
    let size_matches = encoder
        .as_ref()
        .is_some_and(|a| a.width == frame.width && a.height == frame.height);
    if !size_matches {
        // Size change or no encoder yet: replace the worker.
        *encoder = None;
        if !supervisor.may_open(now) {
            return false;
        }
        match open_hw_encoder(
            &ctx.prefs,
            frame.width,
            frame.height,
            render_node,
            &ctx.stop,
        ) {
            Ok(enc) => {
                let info = enc.info();
                let label = format!(
                    "{} / {} ({}x{})",
                    backend_label(info.backend),
                    info.codec.as_str(),
                    info.width,
                    info.height
                );
                tracing::info!(%label, "rudp encode: encoder ready (isolated worker)");
                ctx.video
                    .codec
                    .store(codec_to_u8(info.codec), Ordering::Relaxed);
                ctx.set_status(label);
                ctx.set_error(None);
                *encoder = Some(ActiveEncoder {
                    enc,
                    width: frame.width,
                    height: frame.height,
                    opened_at: now,
                });
            }
            Err(EncodeError::Cancelled) => return false,
            Err(err) => {
                let msg = err.to_string();
                if supervisor.record_failure(now) {
                    tracing::error!(
                        err = %msg,
                        "rudp encode: hardware encoder unavailable — giving up for this \
                         session (Remote stays reachable; re-enable in Settings to retry)"
                    );
                    ctx.set_status("unavailable");
                } else {
                    tracing::warn!(
                        err = %msg,
                        failures = supervisor.failures,
                        "rudp encode: encoder open failed — will retry"
                    );
                    ctx.set_status("retrying");
                }
                ctx.set_error(Some(msg));
                return false;
            }
        }
    }

    let Some(active) = encoder.as_mut() else {
        return false;
    };
    match encode_frame(active.enc.as_mut(), frame, &ctx.video, &ctx.bytes_encoded) {
        Ok(()) => {
            ctx.frames_encoded.fetch_add(1, Ordering::Relaxed);
            supervisor.record_success(now.duration_since(active.opened_at));
            true
        }
        Err(err) if !active.enc.is_healthy() => {
            let msg = err.to_string();
            *encoder = None;
            if ctx.stop.load(Ordering::Relaxed) {
                return false;
            }
            if supervisor.record_failure(now) {
                tracing::error!(
                    err = %msg,
                    "rudp encode: encode worker failed repeatedly — hardware encode disabled \
                     for this session"
                );
                ctx.set_status("unavailable");
            } else {
                tracing::warn!(
                    err = %msg,
                    failures = supervisor.failures,
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
                tracing::warn!(err = %msg, seq = frame.seq, "rudp encode: frame skipped");
            }
            ctx.set_error(Some(msg));
            false
        }
    }
}

fn encode_frame(
    enc: &mut dyn HwEncoder,
    frame: &ExportedFrame,
    video: &VideoShared,
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
    video.width.store(frame.width, Ordering::Relaxed);
    video.height.store(frame.height, Ordering::Relaxed);
    for pkt in packets {
        bytes_encoded.fetch_add(pkt.data.len() as u64, Ordering::Relaxed);
        if let Ok(mut slot) = video.latest_packet.lock() {
            *slot = Some(pkt);
        }
    }
    Ok(())
}

fn backend_label(backend: EncoderBackend) -> &'static str {
    match backend {
        EncoderBackend::Auto => "auto",
        EncoderBackend::Vaapi => "vaapi",
        EncoderBackend::Nvenc => "nvenc",
    }
}

/// Desired host from env override or `rudp.json`.
pub fn desired_host() -> Option<(RudpHostConfig, Vec<String>, bool, RudpEncodePrefs)> {
    let cfg = metis_config::load_rudp_config();
    let prefs = RudpEncodePrefs::from_config(&cfg);
    if let Some(config) = RudpHostConfig::from_env() {
        return Some((config, cfg.allowed_users, cfg.lan_only, prefs));
    }
    if !cfg.enabled {
        return None;
    }
    let bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, cfg.port));
    Some((
        RudpHostConfig { bind },
        cfg.allowed_users,
        cfg.lan_only,
        prefs,
    ))
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
        Some((config, users, lan_only, prefs)) => {
            match RudpHostSystem::spawn(
                config,
                Arc::clone(hub),
                users,
                lan_only,
                prefs,
                render_node_path,
                bridge,
            ) {
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
    fn session_guard_counts_and_releases() {
        let video = Arc::new(VideoShared {
            latest_packet: Arc::new(Mutex::new(None)),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            codec: AtomicU8::new(CODEC_H264),
            active_sessions: AtomicUsize::new(0),
            session_joins: AtomicU64::new(0),
        });
        let a = SessionGuard::new(&video);
        let b = SessionGuard::new(&video);
        assert_eq!(video.active_sessions.load(Ordering::SeqCst), 2);
        drop(a);
        assert_eq!(video.active_sessions.load(Ordering::SeqCst), 1);
        drop(b);
        assert_eq!(video.active_sessions.load(Ordering::SeqCst), 0);
        assert_eq!(video.session_joins.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn codec_wire_names() {
        assert_eq!(codec_wire_name(codec_to_u8(RudpCodec::H264)), "h264");
        assert_eq!(codec_wire_name(codec_to_u8(RudpCodec::Hevc)), "hevc");
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
        // Avoid probing real VAAPI/NVENC in unit tests.
        unsafe {
            std::env::set_var("METIS_ENCODE_NULL", "1");
        }
        let hub = Arc::new(StreamExportHub::new());
        assert!(!hub.is_armed());
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let host = RudpHostSystem::spawn(
            RudpHostConfig { bind },
            Arc::clone(&hub),
            vec!["alice".into()],
            true,
            RudpEncodePrefs::default(),
            PathBuf::from("/dev/dri/renderD128"),
            RudpCalloopBridge {
                input_tx: {
                    let (tx, _rx) = calloop::channel::channel();
                    tx
                },
                pointer_locked: Arc::new(AtomicBool::new(false)),
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
        }
    }
}
