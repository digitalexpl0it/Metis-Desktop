//! Phase 2–3 RUDP host: Quinn listener + hardware encode on an isolated
//! Tokio / worker thread pair — **never** on the Smithay calloop thread.
//!
//! Arms [`crate::stream_export::StreamExportHub`] on start and disarms on
//! [`Drop`]. Frame path: wake → `take_latest` → `wait_ready` → `metis-encode`
//! (VAAPI/NVENC, no mmap) → latest-wins outbound packet slot (Phase 6 sends).

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
    DEFAULT_BITRATE_KBPS, DEFAULT_FPS_HINT, EncodeInput, EncodedPacket, EncoderBackend,
    EncoderConfig, HwEncoder, NullEncoder, RudpCodec, open_encoder,
};
use metis_protocol::{
    RUDP_PROTOCOL_VERSION, RudpControlMsg, RudpRejectReason, encode_rudp_frame,
    try_decode_rudp_frame,
};
use quinn::Endpoint;
use std::sync::RwLock as StdRwLock;

use crate::pam_auth::{pam_check, pam_service};
use crate::rudp_identity;
use crate::stream_export::{ExportedFrame, StreamExportHub};

/// Default QUIC listen port for Metis RUDP.
pub const DEFAULT_RUDP_PORT: u16 = 7843;

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
            codec: RudpVideoCodec::Hevc,
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
    ) -> Result<Self, String> {
        ensure_rustls_provider();

        let stop = Arc::new(AtomicBool::new(false));
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let bytes_encoded = Arc::new(AtomicU64::new(0));
        let latest_packet = Arc::new(Mutex::new(None));
        let encode_error = Arc::new(Mutex::new(None));
        let encode_status = Arc::new(Mutex::new(None));
        let wake = hub.arm();

        let frame_ctx = FramePipelineCtx {
            hub: Arc::clone(&hub),
            wake,
            stop: Arc::clone(&stop),
            frames_encoded: Arc::clone(&frames_encoded),
            bytes_encoded: Arc::clone(&bytes_encoded),
            latest_packet: Arc::clone(&latest_packet),
            encode_error: Arc::clone(&encode_error),
            encode_status: Arc::clone(&encode_status),
            prefs: encode_prefs.clone(),
            render_node_path: render_node_path.clone(),
        };
        let frame_join = std::thread::Builder::new()
            .name("metis-rudp-frames".into())
            .spawn(move || frame_pipeline_loop(frame_ctx))
            .map_err(|e| format!("spawn rudp frame thread: {e}"))?;

        let allowed_users_live = Arc::new(StdRwLock::new(allowed_users.clone()));
        let quinn_stop = Arc::clone(&stop);
        let quinn_allowed = Arc::clone(&allowed_users_live);
        let quinn_join = std::thread::Builder::new()
            .name("metis-rudp-quinn".into())
            .spawn(move || {
                if let Err(err) = run_quinn_runtime(config, quinn_stop, quinn_allowed) {
                    tracing::error!(%err, "rudp host: Quinn runtime exited with error");
                }
            })
            .map_err(|e| format!("spawn rudp quinn thread: {e}"))?;

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
            "rudp host: started (Quinn + hardware encode + PAM auth)"
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

fn run_quinn_runtime(
    config: RudpHostConfig,
    stop: Arc<AtomicBool>,
    allowed_users: Arc<StdRwLock<Vec<String>>>,
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

    runtime.block_on(async move {
        let endpoint = Endpoint::server(server_config, config.bind)
            .map_err(|e| format!("rudp bind {}: {e}", config.bind))?;
        tracing::info!(
            local = %endpoint.local_addr().unwrap_or(config.bind),
            %fingerprint,
            "rudp host: Quinn listening"
        );

        connection_broker_loop(endpoint, stop, allowed_users).await;
        Ok::<(), String>(())
    })
}

async fn connection_broker_loop(
    endpoint: Endpoint,
    stop: Arc<AtomicBool>,
    allowed_users: Arc<StdRwLock<Vec<String>>>,
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
        tokio::spawn(async move {
            match connecting.await {
                Ok(conn) => {
                    let peer = conn.remote_address();
                    tracing::info!(%peer, "rudp host: TLS connected — starting auth");
                    match authenticate_connection(&conn, &allowed).await {
                        Ok(session_id) => {
                            tracing::info!(%peer, %session_id, "rudp host: session authenticated");
                            // Hold until peer closes; video/input arrive in later phases.
                            conn.closed().await;
                            tracing::info!(%peer, "rudp host: client disconnected");
                        }
                        Err(err) => {
                            tracing::warn!(%peer, %err, "rudp host: auth failed");
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

async fn authenticate_connection(
    conn: &quinn::Connection,
    allowed_users: &Arc<StdRwLock<Vec<String>>>,
) -> Result<String, String> {
    // Client opens the control bi-stream.
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(15), conn.accept_bi())
        .await
        .map_err(|_| "auth: timed out waiting for control stream".to_string())?
        .map_err(|e| format!("auth: accept_bi: {e}"))?;

    let hello = read_control_msg(&mut recv).await?;
    let username = match hello {
        RudpControlMsg::Hello { protocol, username } => {
            if protocol != RUDP_PROTOCOL_VERSION {
                let _ = write_control_msg(
                    &mut send,
                    &RudpControlMsg::Reject {
                        reason: RudpRejectReason::Protocol,
                        detail: Some(format!("unsupported protocol {protocol}")),
                    },
                )
                .await;
                return Err(format!("bad protocol {protocol}"));
            }
            username
        }
        other => {
            let _ = write_control_msg(
                &mut send,
                &RudpControlMsg::Reject {
                    reason: RudpRejectReason::Protocol,
                    detail: Some("expected hello".into()),
                },
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
        let _ = write_control_msg(
            &mut send,
            &RudpControlMsg::Reject {
                reason: RudpRejectReason::NotAllowed,
                detail: None,
            },
        )
        .await;
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
            let _ = write_control_msg(
                &mut send,
                &RudpControlMsg::Reject {
                    reason: RudpRejectReason::Protocol,
                    detail: Some("expected auth_response".into()),
                },
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
        let _ = write_control_msg(
            &mut send,
            &RudpControlMsg::Reject {
                reason: RudpRejectReason::AuthFailed,
                detail: None,
            },
        )
        .await;
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
    Ok(session_id)
}

async fn write_control_msg(
    send: &mut quinn::SendStream,
    msg: &RudpControlMsg,
) -> Result<(), String> {
    let bytes = encode_rudp_frame(msg)?;
    send.write_all(&bytes)
        .await
        .map_err(|e| format!("control write: {e}"))?;
    Ok(())
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

fn open_or_null(
    prefs: &RudpEncodePrefs,
    width: u32,
    height: u32,
    render_node: &str,
) -> Result<Box<dyn HwEncoder>, String> {
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
    open_encoder(&cfg, render_node).map_err(|e| e.to_string())
}

struct FramePipelineCtx {
    hub: Arc<StreamExportHub>,
    wake: Receiver<()>,
    stop: Arc<AtomicBool>,
    frames_encoded: Arc<AtomicU64>,
    bytes_encoded: Arc<AtomicU64>,
    latest_packet: Arc<Mutex<Option<EncodedPacket>>>,
    encode_error: Arc<Mutex<Option<String>>>,
    encode_status: Arc<Mutex<Option<String>>>,
    prefs: RudpEncodePrefs,
    render_node_path: PathBuf,
}

struct ActiveEncoder {
    enc: Box<dyn HwEncoder>,
    width: u32,
    height: u32,
}

fn frame_pipeline_loop(ctx: FramePipelineCtx) {
    tracing::info!("rudp host: frame pipeline started");
    let mut last_log = Instant::now();
    let mut fps = 0u32;
    let mut last: Option<(u64, u32, u32, usize)> = None;
    let mut encoder: Option<ActiveEncoder> = None;
    let render_node = ctx.render_node_path.to_string_lossy().into_owned();

    while !ctx.stop.load(Ordering::Relaxed) {
        match ctx.wake.recv_timeout(Duration::from_millis(16)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                while let Some(frame) = ctx.hub.take_latest() {
                    if ctx.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // GPU fence on this worker thread — never on calloop / Quinn poll.
                    frame.wait_ready();
                    match ensure_encoder(&mut encoder, &ctx, &render_node, &frame) {
                        Ok(()) => {
                            if let Err(err) = encode_frame(
                                encoder.as_mut().map(|a| &mut a.enc),
                                &frame,
                                &ctx.latest_packet,
                                &ctx.bytes_encoded,
                            ) {
                                tracing::warn!(%err, seq = frame.seq, "rudp encode submit/drain failed");
                                if let Ok(mut slot) = ctx.encode_error.lock() {
                                    *slot = Some(err);
                                }
                            } else {
                                ctx.frames_encoded.fetch_add(1, Ordering::Relaxed);
                                fps = fps.saturating_add(1);
                                last =
                                    Some((frame.seq, frame.width, frame.height, frame.fds.len()));
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "rudp encode: encoder unavailable");
                        }
                    }
                    // Drop frame → SlotReleaseGuard returns the pool BO.
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if last_log.elapsed() >= Duration::from_secs(1) {
            if let Some((seq, w, h, planes)) = last {
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
                    bytes = ctx.bytes_encoded.load(Ordering::Relaxed),
                    "rudp host: encode rate"
                );
            }
            fps = 0;
            last_log = Instant::now();
        }
    }

    if let Some(mut active) = encoder.take() {
        let _ = active.enc.flush();
    }
    tracing::info!("rudp host: frame pipeline stopped");
}

fn ensure_encoder(
    encoder: &mut Option<ActiveEncoder>,
    ctx: &FramePipelineCtx,
    render_node: &str,
    frame: &ExportedFrame,
) -> Result<(), String> {
    let need_new = match encoder {
        None => true,
        Some(active) => active.width != frame.width || active.height != frame.height,
    };
    if !need_new {
        return Ok(());
    }
    if let Some(mut old) = encoder.take() {
        let _ = old.enc.flush();
    }
    match open_or_null(&ctx.prefs, frame.width, frame.height, render_node) {
        Ok(enc) => {
            let info = enc.info();
            let label = format!(
                "{} / {} ({}x{})",
                backend_label(info.backend),
                info.codec.as_str(),
                info.width,
                info.height
            );
            tracing::info!(%label, "rudp encode: encoder ready");
            if let Ok(mut slot) = ctx.encode_status.lock() {
                *slot = Some(label);
            }
            if let Ok(mut slot) = ctx.encode_error.lock() {
                *slot = None;
            }
            *encoder = Some(ActiveEncoder {
                enc,
                width: frame.width,
                height: frame.height,
            });
            Ok(())
        }
        Err(err) => {
            if let Ok(mut slot) = ctx.encode_error.lock() {
                *slot = Some(err.clone());
            }
            if let Ok(mut slot) = ctx.encode_status.lock() {
                *slot = None;
            }
            Err(err)
        }
    }
}

fn encode_frame(
    encoder: Option<&mut Box<dyn HwEncoder>>,
    frame: &ExportedFrame,
    latest_packet: &Arc<Mutex<Option<EncodedPacket>>>,
    bytes_encoded: &Arc<AtomicU64>,
) -> Result<(), String> {
    let Some(enc) = encoder else {
        return Err("encoder not open".into());
    };
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
    };
    enc.submit(&input).map_err(|e| e.to_string())?;
    let packets = enc.drain().map_err(|e| e.to_string())?;
    for pkt in packets {
        bytes_encoded.fetch_add(pkt.data.len() as u64, Ordering::Relaxed);
        if let Ok(mut slot) = latest_packet.lock() {
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
pub fn maybe_start_for_session(
    hub: &Arc<StreamExportHub>,
    render_node_path: PathBuf,
) -> Option<RudpHostSystem> {
    match desired_host() {
        Some((config, users, lan_only, prefs)) => {
            match RudpHostSystem::spawn(
                config,
                Arc::clone(hub),
                users,
                lan_only,
                prefs,
                render_node_path,
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
        )
        .expect("spawn host");
        assert!(hub.is_armed());
        assert_eq!(host.allowed_users, vec!["alice".to_string()]);
        drop(host);
        assert!(!hub.is_armed());
        unsafe {
            std::env::remove_var("METIS_ENCODE_NULL");
        }
    }
}
