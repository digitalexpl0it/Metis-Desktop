//! Metis Remote (RUDP) network engine: Quinn listener, TLS identity, PAM
//! auth, per-session output subscriptions and encoded-packet fan-out.
//!
//! This is the part of Metis Remote that faces the network. Phase 23 §D runs
//! it in the standalone `metis-rudp-host` process; the compositor keeps frame
//! capture + (isolated) encode and talks to the engine through [`HostLink`] /
//! [`EngineShared`]. The `METIS_RUDP_INPROCESS=1` emergency fallback drives the
//! very same engine on a compositor thread, so both modes share one code path.
//!
//! Multi-monitor (protocol v3): the compositor publishes the desktop topology
//! into [`RudpLayoutShared`]; every session gets `DesktopLayout` after auth and
//! on change, and picks outputs with `SubscribeOutputs` (default: primary). The
//! union of all sessions' subscriptions is reported through
//! [`HostLink::subscribe_union`]; each output has its own encoder +
//! `stream_id` and its packets reach only the sessions subscribed to it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use metis_protocol::{
    MediaDatagramParams, RUDP_AUDIO_CHANNELS, RUDP_AUDIO_SAMPLE_RATE, RUDP_DEFAULT_DATAGRAM_BUDGET,
    RUDP_MAX_SUBSCRIBED_OUTPUTS, RUDP_PROTOCOL_VERSION, ReliableAccessUnit, RudpControlMsg,
    RudpDamageRect, RudpHostInput, RudpMonitorInfo, RudpRejectReason, build_media_datagrams,
    codec_to_str, encode_rudp_frame, try_decode_rudp_frame,
};
use quinn::Endpoint;
use tokio::sync::RwLock as AsyncRwLock;
use zeroize::Zeroize;

use crate::pam_auth::{pam_check, pam_service};
use crate::rudp_auth_guard::{AuthAdmitDeny, AuthGuard, AuthSessionSlot, InFlightAuthGuard};
use crate::rudp_identity;

/// Default QUIC listen port for Metis RUDP.
pub const DEFAULT_RUDP_PORT: u16 = 7843;

/// Upper bound for one reliable write to a client (control / keyframe).
const PUMP_WRITE_TIMEOUT: Duration = Duration::from_millis(750);

/// Encoded AU outbox depth (all streams) — absorbs keyframe write stalls
/// without dropping every intervening delta (latest-wins would). Sized for
/// [`RUDP_MAX_SUBSCRIBED_OUTPUTS`] streams at the old single-stream depth.
pub const PACKET_OUTBOX_CAP: usize = 16 * RUDP_MAX_SUBSCRIBED_OUTPUTS;

/// Codec-agnostic encoded access unit (bitstream + timing + damage).
#[derive(Debug, Clone)]
pub struct MediaPacket {
    pub seq: u64,
    pub pts_us: i64,
    /// Wire codec byte (`metis_protocol::CODEC_*`).
    pub codec: u8,
    pub is_keyframe: bool,
    pub damage_full: bool,
    pub damage: Vec<RudpDamageRect>,
    pub data: Vec<u8>,
}

/// One encoded access unit tagged with the output stream it belongs to.
#[derive(Debug, Clone)]
pub struct OutboundPacket {
    /// Demux key advertised in `VideoReady` / carried on the wire.
    pub stream_id: u8,
    /// Output name (also the `output_id` in `VideoReady` / `DesktopLayout`).
    pub output_id: String,
    /// Encode dimensions of this packet's stream.
    pub width: u32,
    pub height: u32,
    pub packet: MediaPacket,
}

/// Bounded encoded-packet queue between the producer (frame thread, or the IPC
/// reader in the host process) and the Quinn pump / IPC forwarder.
///
/// Push never blocks. Under pressure, oldest non-keyframes are dropped first
/// (preferring the incoming packet's own stream so one busy monitor cannot
/// starve another); an incoming keyframe always gets a slot (dropping the
/// oldest packet if the queue is all keyframes). An incoming delta is dropped
/// only when the queue is full of keyframes.
pub struct PacketOutbox {
    inner: Mutex<VecDeque<OutboundPacket>>,
    ready: Condvar,
}

impl Default for PacketOutbox {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketOutbox {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(PACKET_OUTBOX_CAP)),
            ready: Condvar::new(),
        }
    }

    /// Index of the oldest droppable delta, same stream first.
    fn evict_candidate(q: &VecDeque<OutboundPacket>, stream_id: u8) -> Option<usize> {
        q.iter()
            .position(|p| !p.packet.is_keyframe && p.stream_id == stream_id)
            .or_else(|| q.iter().position(|p| !p.packet.is_keyframe))
    }

    pub fn push(&self, pkt: OutboundPacket) {
        let Ok(mut q) = self.inner.lock() else {
            return;
        };
        let queued = if q.len() < PACKET_OUTBOX_CAP {
            q.push_back(pkt);
            true
        } else if pkt.packet.is_keyframe {
            if let Some(idx) = Self::evict_candidate(&q, pkt.stream_id) {
                q.remove(idx);
            } else {
                let _ = q.pop_front();
            }
            q.push_back(pkt);
            true
        } else if let Some(idx) = Self::evict_candidate(&q, pkt.stream_id) {
            q.remove(idx);
            q.push_back(pkt);
            true
        } else {
            // All keyframes — drop the incoming delta.
            false
        };
        drop(q);
        if queued {
            self.ready.notify_one();
        }
    }

    pub fn pop(&self) -> Option<OutboundPacket> {
        self.inner.lock().ok()?.pop_front()
    }

    /// Like [`Self::pop`], but waits up to `timeout` for a packet.
    pub fn pop_wait(&self, timeout: Duration) -> Option<OutboundPacket> {
        let mut q = self.inner.lock().ok()?;
        if let Some(p) = q.pop_front() {
            return Some(p);
        }
        let (mut q, _) = self.ready.wait_timeout(q, timeout).ok()?;
        q.pop_front()
    }

    pub fn clear(&self) {
        if let Ok(mut q) = self.inner.lock() {
            q.clear();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    #[cfg(test)]
    fn seqs(&self) -> Vec<u64> {
        self.inner
            .lock()
            .map(|q| q.iter().map(|p| p.packet.seq).collect())
            .unwrap_or_default()
    }
}

/// Latest-wins outbound clipboard payload for authenticated sessions.
#[derive(Debug, Clone)]
pub struct RudpClipboardOut {
    pub serial: u64,
    pub mime: String,
    pub text: String,
}

/// Desktop topology snapshot shared between the compositor / IPC reader
/// (writer) and the Quinn runtime (reader). Whoever owns the topology calls
/// [`Self::publish`] whenever outputs, geometry, scale or the primary output
/// change; the engine re-sends `DesktopLayout` to every session when
/// [`Self::generation`] moves.
#[derive(Debug, Default)]
pub struct RudpLayoutShared {
    /// 0 until the first non-empty publish; bumped on every change.
    generation: AtomicU64,
    monitors: Mutex<Vec<RudpMonitorInfo>>,
}

impl RudpLayoutShared {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the layout. Returns `true` (and bumps the generation) only when
    /// it differs from the current snapshot, so callers may publish freely.
    pub fn publish(&self, monitors: Vec<RudpMonitorInfo>) -> bool {
        let Ok(mut cur) = self.monitors.lock() else {
            return false;
        };
        if *cur == monitors {
            return false;
        }
        *cur = monitors;
        self.generation.fetch_add(1, Ordering::SeqCst);
        true
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Consistent `(generation, monitors)` pair.
    pub fn snapshot(&self) -> (u64, Vec<RudpMonitorInfo>) {
        match self.monitors.lock() {
            Ok(guard) => (self.generation.load(Ordering::SeqCst), guard.clone()),
            Err(_) => (0, Vec::new()),
        }
    }

    pub fn monitors(&self) -> Vec<RudpMonitorInfo> {
        self.monitors.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// One Opus datagram (MRUA header + payload) ready for fan-out.
#[derive(Debug, Clone)]
pub struct RudpAudioPacket {
    pub seq: u32,
    /// Presentation timestamp (retained for future A/V sync).
    pub pts_us: i64,
    /// Encoded Quinn datagram (MRUA header + Opus).
    pub datagram: Vec<u8>,
}

/// Latest system-audio packet, written by the capture side (compositor) or the
/// IPC reader (host process) and read by the Quinn pump.
#[derive(Default)]
pub struct RudpAudioShared {
    pub latest: Mutex<Option<RudpAudioPacket>>,
    /// True after a successful Pulse+Opus open (engine may advertise AudioReady).
    pub streaming: AtomicBool,
    seq: AtomicU32,
}

impl RudpAudioShared {
    pub fn new() -> Self {
        Self::default()
    }

    /// Next capture sequence number.
    pub fn next_seq(&self) -> u32 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }
}

/// Callbacks from the engine into whoever owns the desktop session (the
/// compositor in-process, or the IPC writer in `metis-rudp-host`). All methods
/// must be cheap and non-blocking — they run on Quinn / pump tasks.
pub trait HostLink: Send + Sync {
    /// Authenticated client input / clipboard-set.
    fn input(&self, event: RudpHostInput);
    /// Owner authenticated and `unlock_session_on_auth` is on.
    fn unlock_pam_session(&self, username: String);
    /// Union of all sessions' output subscriptions changed.
    fn subscribe_union(&self, output_ids: Vec<String>, gained: bool);
    /// Authenticated session count / join counter changed (pump tick).
    fn session_demand(&self, _sessions: usize, _joins: u64) {}
    /// Listener state after bind / identity load.
    fn status(
        &self,
        _listening: bool,
        _fingerprint: &str,
        _bind: SocketAddr,
        _err: Option<String>,
    ) {
    }
}

/// [`HostLink`] that drops everything (compositor-side mirror in IPC mode).
pub struct NullLink;

impl HostLink for NullLink {
    fn input(&self, _event: RudpHostInput) {}
    fn unlock_pam_session(&self, _username: String) {}
    fn subscribe_union(&self, _output_ids: Vec<String>, _gained: bool) {}
}

/// State shared between the producer side (frame pipeline / IPC reader), the
/// audio supervisor and the Quinn runtime.
pub struct EngineShared {
    /// Encoded access units waiting for fan-out / IPC forwarding.
    pub packet_outbox: PacketOutbox,
    /// Per-output stream ids + advertised encode parameters.
    pub streams: StreamTable,
    /// Desktop topology.
    pub layout: Arc<RudpLayoutShared>,
    /// True while a Wayland pointer lock is active (advertised as `PointerLock`).
    pub pointer_locked: Arc<AtomicBool>,
    /// True while Metis PAM lock or `ext-session-lock` owns the session.
    pub session_locked: Arc<AtomicBool>,
    /// Authenticated sessions; > 0 ⇒ export + encode demand.
    pub active_sessions: AtomicUsize,
    /// Monotonic join counter — a change makes the frame thread emit an IDR
    /// on every stream.
    pub session_joins: AtomicU64,
    /// Latest-wins clipboard text for fan-out to authenticated clients.
    pub clipboard_out: Mutex<Option<RudpClipboardOut>>,
    pub clipboard_serial: AtomicU64,
    pub audio: RudpAudioShared,
    /// Live allowlist of local PAM usernames.
    pub allowed_users: StdRwLock<Vec<String>>,
    /// Live: unlock Metis PAM lock after owner auth.
    pub unlock_session_on_auth: AtomicBool,
    /// Owner of the desktop session.
    pub link: Arc<dyn HostLink>,
    /// A session joined / left / changed its subscription — re-resolve.
    subs_dirty: AtomicBool,
    /// Layout generation the subscriptions were last resolved against.
    subs_layout_gen: AtomicU64,
}

impl EngineShared {
    pub fn new(
        link: Arc<dyn HostLink>,
        layout: Arc<RudpLayoutShared>,
        pointer_locked: Arc<AtomicBool>,
        session_locked: Arc<AtomicBool>,
        allowed_users: Vec<String>,
        unlock_session_on_auth: bool,
    ) -> Self {
        Self {
            packet_outbox: PacketOutbox::new(),
            streams: StreamTable::new(),
            layout,
            pointer_locked,
            session_locked,
            active_sessions: AtomicUsize::new(0),
            session_joins: AtomicU64::new(0),
            clipboard_out: Mutex::new(None),
            clipboard_serial: AtomicU64::new(0),
            audio: RudpAudioShared::new(),
            allowed_users: StdRwLock::new(allowed_users),
            unlock_session_on_auth: AtomicBool::new(unlock_session_on_auth),
            link,
            subs_dirty: AtomicBool::new(true),
            // Force the first resolve even when the layout is still empty.
            subs_layout_gen: AtomicU64::new(u64::MAX),
        }
    }

    /// Replace the live policy (allowlist + unlock-on-auth).
    pub fn set_policy(&self, allowed_users: Vec<String>, unlock_session_on_auth: bool) {
        if let Ok(mut guard) = self.allowed_users.write() {
            *guard = allowed_users;
        }
        self.unlock_session_on_auth
            .store(unlock_session_on_auth, Ordering::Relaxed);
    }

    /// Push local clipboard text to connected RUDP clients (latest-wins).
    /// No-op when nobody is authenticated.
    pub fn push_clipboard_text(&self, mime: &str, text: &str) {
        if self.active_sessions.load(Ordering::Relaxed) == 0 {
            return;
        }
        let trimmed = metis_protocol::truncate_clipboard_text(text);
        if trimmed.is_empty() {
            return;
        }
        let serial = self.clipboard_serial.fetch_add(1, Ordering::Relaxed) + 1;
        let mime = if mime.is_empty() {
            "text/plain;charset=utf-8".into()
        } else {
            mime.to_string()
        };
        self.store_clipboard(RudpClipboardOut {
            serial,
            mime,
            text: trimmed.to_string(),
        });
    }

    /// Install a clipboard payload (serial chosen by the caller).
    pub fn store_clipboard(&self, clip: RudpClipboardOut) {
        if let Ok(mut slot) = self.clipboard_out.lock() {
            *slot = Some(clip);
        }
    }
}

/// Counts one authenticated session for as long as it lives (incl. task cancel).
struct SessionGuard(Arc<EngineShared>);

impl SessionGuard {
    fn new(video: &Arc<EngineShared>) -> Self {
        video.active_sessions.fetch_add(1, Ordering::SeqCst);
        video.session_joins.fetch_add(1, Ordering::SeqCst);
        video.subs_dirty.store(true, Ordering::SeqCst);
        Self(Arc::clone(video))
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.active_sessions.fetch_sub(1, Ordering::SeqCst);
        self.0.subs_dirty.store(true, Ordering::SeqCst);
    }
}

/// Suppress reconnect-storm spam for the same PAM user + peer IP.
const CONNECT_NOTIFY_DEBOUNCE: Duration = Duration::from_secs(5);

fn sanitize_notify_user(username: &str) -> String {
    let trimmed: String = username
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(64)
        .collect();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        "remote user".into()
    } else {
        trimmed.to_string()
    }
}

fn should_notify_connect(username: &str, peer: SocketAddr) -> bool {
    static LAST: Mutex<Option<(String, IpAddr, Instant)>> = Mutex::new(None);
    let Ok(mut guard) = LAST.lock() else {
        return true;
    };
    let now = Instant::now();
    let ip = peer.ip();
    if let Some((prev_user, prev_ip, at)) = guard.as_ref()
        && prev_user == username
        && *prev_ip == ip
        && now.duration_since(*at) < CONNECT_NOTIFY_DEBOUNCE
    {
        return false;
    }
    *guard = Some((username.to_string(), ip, now));
    true
}

/// Desktop notification via `notify-send` so Metis Notification Center picks it up.
/// Runs off-thread so Quinn/auth never blocks on D-Bus.
fn notify_rudp_client(username: &str, peer: SocketAddr, connected: bool) {
    let user = sanitize_notify_user(username);
    let peer_s = peer.to_string();
    let title = "Metis Remote".to_string();
    let body = if connected {
        format!("{user} connected from {peer_s}")
    } else {
        format!("{user} disconnected ({peer_s})")
    };
    let _ = std::thread::Builder::new()
        .name("metis-rudp-notify".into())
        .spawn(move || {
            use std::process::{Command, Stdio};
            let ok = Command::new("notify-send")
                .args([
                    "-a",
                    "Metis Remote",
                    "-u",
                    "normal",
                    "--icon=network-transmit-receive",
                    "--hint=string:desktop-entry:metis-settings",
                    &title,
                    &body,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                tracing::debug!(
                    %body,
                    "rudp host: notify-send failed (Notification Center may be unavailable)"
                );
            }
        });
}

/// Install the `ring` rustls provider (idempotent).
pub fn ensure_rustls_provider() {
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        // Already installed by another component — fine.
    }
}

/// What the host advertises for one output stream in `VideoReady`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub stream_id: u8,
    pub output_id: String,
    pub width: u32,
    pub height: u32,
    /// Wire codec byte (`metis_protocol::CODEC_*`) of the encoder actually
    /// running (may differ from the preference after a fallback).
    pub codec: u8,
}

#[derive(Default)]
struct StreamTableInner {
    ids: HashMap<String, u8>,
    infos: HashMap<String, StreamInfo>,
}

/// Host-global output → `stream_id` registry plus the live encode parameters
/// of every running output stream. Ids are the lowest free value in `0..=255`
/// and are recycled once no session subscribes to an output any more.
pub struct StreamTable {
    inner: Mutex<StreamTableInner>,
    /// Bumped when an advertised [`StreamInfo`] changes (drives `VideoReady`).
    generation: AtomicU64,
}

impl Default for StreamTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamTable {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StreamTableInner::default()),
            generation: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn alloc_id(inner: &mut StreamTableInner, output: &str) -> Option<u8> {
        if let Some(id) = inner.ids.get(output) {
            return Some(*id);
        }
        let used: HashSet<u8> = inner.ids.values().copied().collect();
        let id = (0..=u8::MAX).find(|id| !used.contains(id))?;
        inner.ids.insert(output.to_string(), id);
        Some(id)
    }

    /// Record the live encode parameters for `output`; returns its stream id.
    pub fn publish(&self, output: &str, width: u32, height: u32, codec: u8) -> Option<u8> {
        let mut inner = self.inner.lock().ok()?;
        let stream_id = Self::alloc_id(&mut inner, output)?;
        let info = StreamInfo {
            stream_id,
            output_id: output.to_string(),
            width,
            height,
            codec,
        };
        if inner.infos.get(output) != Some(&info) {
            inner.infos.insert(output.to_string(), info);
            drop(inner);
            self.bump();
        }
        Some(stream_id)
    }

    /// Forget streams for outputs outside `keep` (ids become reusable).
    pub fn retain(&self, keep: &[String]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let before = inner.ids.len() + inner.infos.len();
        inner.ids.retain(|name, _| keep.iter().any(|k| k == name));
        inner.infos.retain(|name, _| keep.iter().any(|k| k == name));
        let changed = before != inner.ids.len() + inner.infos.len();
        drop(inner);
        if changed {
            self.bump();
        }
    }

    /// Replace the whole table with `infos` (IPC mirror in the host process).
    /// Bumps the generation only when something changed.
    pub fn replace(&self, infos: Vec<StreamInfo>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let mut next_ids = HashMap::new();
        let mut next_infos = HashMap::new();
        for info in infos {
            next_ids.insert(info.output_id.clone(), info.stream_id);
            next_infos.insert(info.output_id.clone(), info);
        }
        if inner.ids == next_ids && inner.infos == next_infos {
            return;
        }
        inner.ids = next_ids;
        inner.infos = next_infos;
        drop(inner);
        self.bump();
    }

    /// Live streams, ordered by `stream_id`.
    pub fn snapshot(&self) -> Vec<StreamInfo> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut infos: Vec<StreamInfo> = inner.infos.values().cloned().collect();
        infos.sort_by_key(|i| i.stream_id);
        infos
    }
}

/// Longest output id accepted from a client (connector names are short).
const MAX_OUTPUT_ID_LEN: usize = 128;
/// Output ids examined per `SubscribeOutputs` (bounds work for hostile input).
const MAX_REQUESTED_OUTPUT_IDS: usize = 16;

/// Normalise a client `SubscribeOutputs` list: drop empty / oversized / duplicate
/// ids and bound its length. Empty → `None` (primary only).
fn sanitize_subscription(output_ids: Vec<String>) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for id in output_ids {
        if out.len() >= MAX_REQUESTED_OUTPUT_IDS {
            break;
        }
        if id.is_empty() || id.len() > MAX_OUTPUT_ID_LEN || out.contains(&id) {
            continue;
        }
        out.push(id);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Outputs a session actually receives: the requested ids that exist in the
/// layout (capped at [`RUDP_MAX_SUBSCRIBED_OUTPUTS`]); empty / all-unknown →
/// the primary output. Empty only while the layout itself is empty.
fn resolve_subscription(requested: Option<&[String]>, monitors: &[RudpMonitorInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(requested) = requested {
        for id in requested {
            if out.len() >= RUDP_MAX_SUBSCRIBED_OUTPUTS {
                break;
            }
            if monitors.iter().any(|m| &m.id == id) && !out.contains(id) {
                out.push(id.clone());
            }
        }
    }
    if out.is_empty()
        && let Some(primary) = monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| monitors.first())
    {
        out.push(primary.id.clone());
    }
    out
}

/// Union of every session's effective outputs, in layout order.
fn union_subscriptions(per_session: &[Vec<String>], monitors: &[RudpMonitorInfo]) -> Vec<String> {
    monitors
        .iter()
        .filter(|m| per_session.iter().any(|s| s.contains(&m.id)))
        .map(|m| m.id.clone())
        .collect()
}

#[derive(Default)]
struct SessionSubscription {
    /// Client request; `None` ⇒ primary only (the default until the client
    /// sends `SubscribeOutputs`).
    requested: Option<Vec<String>>,
    /// Resolved against the current layout.
    effective: Vec<String>,
}

/// Bind Quinn on `bind` and serve until `stop` is set. Blocks the calling
/// thread; call it from a dedicated OS thread (compositor in-process mode) or
/// the `metis-rudp-host` main thread.
pub fn run_quinn_runtime(
    bind: SocketAddr,
    stop: Arc<AtomicBool>,
    video: Arc<EngineShared>,
) -> Result<(), String> {
    ensure_rustls_provider();
    let (server_config, fingerprint) = match rudp_identity::load_or_create_server_config() {
        Ok(v) => v,
        Err(err) => {
            video.link.status(false, "", bind, Some(err.clone()));
            return Err(err);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .thread_name("metis-rudp-tok")
        .build()
        .map_err(|e| format!("rudp tokio runtime: {e}"))?;

    let result = runtime.block_on(async move {
        let endpoint = match Endpoint::server(server_config, bind) {
            Ok(ep) => ep,
            Err(e) => {
                let err = format!("rudp bind {bind}: {e}");
                video
                    .link
                    .status(false, &fingerprint, bind, Some(err.clone()));
                return Err(err);
            }
        };
        tracing::info!(
            local = %endpoint.local_addr().unwrap_or(bind),
            %fingerprint,
            "rudp host: Quinn listening"
        );
        video.link.status(true, &fingerprint, bind, None);

        let sessions: SessionRegistry = Arc::new(AsyncRwLock::new(HashMap::new()));
        let auth_guard = Arc::new(Mutex::new(AuthGuard::default()));
        let pump_stop = Arc::clone(&stop);
        let pump_sessions = Arc::clone(&sessions);
        let pump_video = Arc::clone(&video);
        tokio::spawn(async move {
            video_pump_loop(pump_stop, pump_sessions, pump_video).await;
        });

        connection_broker_loop(
            endpoint,
            BrokerCtx {
                stop,
                sessions,
                video,
                auth_guard,
            },
        )
        .await;
        Ok::<(), String>(())
    });
    // Plain drop waits forever for spawn_blocking (PAM) tasks; shutdown runs
    // on calloop via the host's shutdown path, so it must be bounded.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

struct VideoSession {
    conn: quinn::Connection,
    control_send: tokio::sync::Mutex<quinn::SendStream>,
    video_send: tokio::sync::Mutex<quinn::SendStream>,
    audio_ready_sent: AtomicBool,
    warned_no_dgram: AtomicBool,
    /// Layout generation last delivered to this client (0 = none yet).
    layout_gen_sent: AtomicU64,
    /// [`StreamTable`] generation the `VideoReady` set was last synced at
    /// (`u64::MAX` = never).
    ready_epoch: AtomicU64,
    /// `VideoReady` already delivered, keyed by output id.
    announced: Mutex<HashMap<String, StreamInfo>>,
    subs: Mutex<SessionSubscription>,
}

impl VideoSession {
    fn new(
        conn: quinn::Connection,
        control_send: quinn::SendStream,
        video_send: quinn::SendStream,
        layout_gen_sent: u64,
    ) -> Self {
        Self {
            conn,
            control_send: tokio::sync::Mutex::new(control_send),
            video_send: tokio::sync::Mutex::new(video_send),
            audio_ready_sent: AtomicBool::new(false),
            warned_no_dgram: AtomicBool::new(false),
            layout_gen_sent: AtomicU64::new(layout_gen_sent),
            ready_epoch: AtomicU64::new(u64::MAX),
            announced: Mutex::new(HashMap::new()),
            subs: Mutex::new(SessionSubscription::default()),
        }
    }

    /// Record a client `SubscribeOutputs` (resolved on the next pump tick).
    fn set_requested(&self, output_ids: Vec<String>) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.requested = sanitize_subscription(output_ids);
        }
    }

    /// Re-resolve against `monitors`. Returns `true` when the session gained an
    /// output it did not receive before (needs a keyframe).
    fn refresh_effective(&self, monitors: &[RudpMonitorInfo]) -> bool {
        let Ok(mut subs) = self.subs.lock() else {
            return false;
        };
        let next = resolve_subscription(subs.requested.as_deref(), monitors);
        let gained = next.iter().any(|id| !subs.effective.contains(id));
        subs.effective = next;
        gained
    }

    fn effective(&self) -> Vec<String> {
        self.subs
            .lock()
            .map(|s| s.effective.clone())
            .unwrap_or_default()
    }

    fn is_subscribed(&self, output_id: &str) -> bool {
        self.subs
            .lock()
            .is_ok_and(|s| s.effective.iter().any(|id| id == output_id))
    }

    /// Media for `output_id` may be sent: subscribed **and** `VideoReady` for
    /// it already reached this client (so decoders exist before the first AU).
    fn accepts_media(&self, output_id: &str) -> bool {
        self.is_subscribed(output_id)
            && self
                .announced
                .lock()
                .is_ok_and(|a| a.contains_key(output_id))
    }
}

type SessionRegistry = Arc<AsyncRwLock<HashMap<String, Arc<VideoSession>>>>;

struct BrokerCtx {
    stop: Arc<AtomicBool>,
    sessions: SessionRegistry,
    video: Arc<EngineShared>,
    auth_guard: Arc<Mutex<AuthGuard>>,
}

async fn connection_broker_loop(endpoint: Endpoint, ctx: BrokerCtx) {
    let BrokerCtx {
        stop,
        sessions,
        video,
        auth_guard,
    } = ctx;
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
        let sessions = Arc::clone(&sessions);
        let video = Arc::clone(&video);
        let auth_guard = Arc::clone(&auth_guard);
        tokio::spawn(async move {
            match connecting.await {
                Ok(conn) => {
                    let peer = conn.remote_address();
                    let peer_ip = peer.ip();
                    let now = Instant::now();
                    let admit = auth_guard
                        .lock()
                        .ok()
                        .map(|mut g| g.try_begin_auth(peer_ip, now));
                    match admit {
                        Some(Ok(())) => {}
                        Some(Err(AuthAdmitDeny::RateLimited)) => {
                            tracing::warn!(%peer, "rudp host: auth rate limited");
                            if let Ok((mut send, _)) = conn.accept_bi().await {
                                let _ = write_control_reject(
                                    &mut send,
                                    RudpRejectReason::RateLimited,
                                    None,
                                )
                                .await;
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                            conn.close(0u32.into(), b"rate limited");
                            return;
                        }
                        Some(Err(AuthAdmitDeny::CapExceeded)) | None => {
                            tracing::warn!(%peer, "rudp host: auth/session cap — closing");
                            conn.close(0u32.into(), b"busy");
                            return;
                        }
                    }
                    let _in_flight = InFlightAuthGuard::new(Arc::clone(&auth_guard));
                    tracing::info!(%peer, "rudp host: TLS connected — starting auth");
                    match authenticate_connection(&conn, &video, &auth_guard, peer_ip).await {
                        Ok((session_id, username, mut control_send, mut control_recv)) => {
                            let session_ok = auth_guard
                                .lock()
                                .ok()
                                .is_some_and(|mut g| g.try_begin_session());
                            if !session_ok {
                                tracing::warn!(%peer, "rudp host: session cap after PAM");
                                let _ = write_control_reject(
                                    &mut control_send,
                                    RudpRejectReason::RateLimited,
                                    Some("too many sessions".into()),
                                )
                                .await;
                                tokio::time::sleep(Duration::from_millis(200)).await;
                                conn.close(0u32.into(), b"busy");
                                return;
                            }
                            let session_slot = AuthSessionSlot::new(Arc::clone(&auth_guard));
                            // In-flight slot no longer needed once we hold a session slot.
                            _in_flight.disarm();

                            tracing::info!(
                                %peer,
                                %session_id,
                                %username,
                                "rudp host: session authenticated"
                            );
                            if should_notify_connect(&username, peer) {
                                notify_rudp_client(&username, peer, true);
                            }
                            if video.unlock_session_on_auth.load(Ordering::Relaxed) {
                                video.link.unlock_pam_session(username.clone());
                            }
                            // Advertise current pointer-lock so the client can pick
                            // absolute vs relative before the next pump tick.
                            let locked = video.pointer_locked.load(Ordering::Relaxed);
                            let _ = write_control_msg(
                                &mut control_send,
                                &RudpControlMsg::PointerLock { locked },
                            )
                            .await;
                            // Topology right after SessionOk so the client can pick
                            // outputs before any VideoReady.
                            let (layout_gen, layout_monitors) = video.layout.snapshot();
                            let mut layout_gen_sent = 0;
                            if !layout_monitors.is_empty()
                                && write_control_msg(
                                    &mut control_send,
                                    &RudpControlMsg::DesktopLayout {
                                        monitors: layout_monitors,
                                    },
                                )
                                .await
                                .is_ok()
                            {
                                layout_gen_sent = layout_gen;
                            }
                            let video_send = match conn.open_uni().await {
                                Ok(s) => s,
                                Err(err) => {
                                    tracing::warn!(%peer, %err, "rudp host: open_uni failed");
                                    conn.close(0u32.into(), b"video stream failed");
                                    return;
                                }
                            };
                            let session = Arc::new(VideoSession::new(
                                conn.clone(),
                                control_send,
                                video_send,
                                layout_gen_sent,
                            ));
                            {
                                let mut guard = sessions.write().await;
                                guard.insert(session_id.clone(), Arc::clone(&session));
                            }
                            // Demand on (frame thread starts export + IDR); off on drop.
                            let _session_count = SessionGuard::new(&video);
                            let _auth_slot = session_slot;
                            let ctrl_link = Arc::clone(&video.link);
                            let ctrl_session = Arc::clone(&session);
                            let ctrl_video = Arc::clone(&video);
                            // Control reads: keepalives + Phase 7 input → calloop,
                            // `SubscribeOutputs` → this session's subscription.
                            tokio::spawn(async move {
                                let mut buf = Vec::new();
                                let mut tmp = [0u8; 2048];
                                while let Ok(Some(n)) = control_recv.read(&mut tmp).await {
                                    buf.extend_from_slice(&tmp[..n]);
                                    while let Ok(Some((msg, n))) = try_decode_rudp_frame(&buf) {
                                        buf.drain(..n);
                                        if let RudpControlMsg::SubscribeOutputs { output_ids } = msg
                                        {
                                            ctrl_session.set_requested(output_ids);
                                            ctrl_video.subs_dirty.store(true, Ordering::SeqCst);
                                            continue;
                                        }
                                        if let Some(ev) = RudpHostInput::from_control(msg) {
                                            ctrl_link.input(ev);
                                        }
                                    }
                                }
                            });
                            conn.closed().await;
                            sessions.write().await.remove(&session_id);
                            // After the removal so the next resolve excludes this session.
                            video.subs_dirty.store(true, Ordering::SeqCst);
                            tracing::info!(
                                %peer,
                                %session_id,
                                %username,
                                "rudp host: client disconnected"
                            );
                            notify_rudp_client(&username, peer, false);
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

/// Re-resolve every session's outputs against the current layout and push the
/// union into the export hub. Cheap no-op unless something changed.
fn sync_subscriptions(snap: &[Arc<VideoSession>], video: &EngineShared) {
    let layout_gen = video.layout.generation();
    let dirty = video.subs_dirty.swap(false, Ordering::SeqCst);
    if !dirty && layout_gen == video.subs_layout_gen.load(Ordering::Relaxed) {
        return;
    }
    video.subs_layout_gen.store(layout_gen, Ordering::Relaxed);
    let monitors = video.layout.monitors();
    let mut gained = false;
    for session in snap {
        gained |= session.refresh_effective(&monitors);
    }
    let per_session: Vec<Vec<String>> = snap.iter().map(|s| s.effective()).collect();
    let union = union_subscriptions(&per_session, &monitors);
    if !union.is_empty() {
        video.streams.retain(&union);
    }
    // Empty union (no sessions / no layout yet) ⇒ hub exports primary only.
    video.link.subscribe_union(union, gained);
    if gained {
        // A session started receiving an output: every stream needs an IDR.
        video.session_joins.fetch_add(1, Ordering::SeqCst);
    }
    // Re-evaluate which `VideoReady` each session still needs.
    video.streams.bump();
}

/// Send `DesktopLayout` to every session whose copy is stale.
async fn broadcast_layout(snap: &[Arc<VideoSession>], layout: &RudpLayoutShared) {
    let generation = layout.generation();
    if generation == 0
        || snap
            .iter()
            .all(|s| s.layout_gen_sent.load(Ordering::Relaxed) == generation)
    {
        return;
    }
    let (generation, monitors) = layout.snapshot();
    if monitors.is_empty() {
        return;
    }
    let msg = RudpControlMsg::DesktopLayout { monitors };
    for session in snap {
        if session.layout_gen_sent.load(Ordering::Relaxed) == generation {
            continue;
        }
        let mut send = session.control_send.lock().await;
        if write_control_msg(&mut send, &msg).await.is_ok() {
            session.layout_gen_sent.store(generation, Ordering::Relaxed);
        }
    }
}

/// Deliver `VideoReady` for every subscribed stream a session has not seen (or
/// whose size / codec changed). Late joiners and re-subscribers get the full set.
async fn announce_streams(snap: &[Arc<VideoSession>], video: &EngineShared) {
    let epoch = video.streams.generation();
    if snap
        .iter()
        .all(|s| s.ready_epoch.load(Ordering::Relaxed) == epoch)
    {
        return;
    }
    let infos = video.streams.snapshot();
    for session in snap {
        if session.ready_epoch.load(Ordering::Relaxed) == epoch {
            continue;
        }
        // Forget announcements for outputs this session no longer receives so a
        // later re-subscribe re-sends `VideoReady`.
        let wanted = session.effective();
        if let Ok(mut announced) = session.announced.lock() {
            announced.retain(|output, _| wanted.contains(output));
        }
        let mut all_ok = true;
        for info in &infos {
            if !wanted.contains(&info.output_id) {
                continue;
            }
            let already = session
                .announced
                .lock()
                .is_ok_and(|a| a.get(&info.output_id) == Some(info));
            if already {
                continue;
            }
            let msg = RudpControlMsg::VideoReady {
                width: info.width,
                height: info.height,
                codec: codec_to_str(info.codec).to_string(),
                stream_id: info.stream_id,
                output_id: info.output_id.clone(),
            };
            let mut send = session.control_send.lock().await;
            if write_control_msg(&mut send, &msg).await.is_ok() {
                if let Ok(mut announced) = session.announced.lock() {
                    announced.insert(info.output_id.clone(), info.clone());
                }
            } else {
                all_ok = false;
            }
        }
        if all_ok {
            session.ready_epoch.store(epoch, Ordering::Relaxed);
        }
    }
}

async fn video_pump_loop(
    stop: Arc<AtomicBool>,
    sessions: SessionRegistry,
    video: Arc<EngineShared>,
) {
    tracing::info!("rudp host: video pump started");
    let mut last_demand: Option<(usize, u64)> = None;
    let mut last_audio_seq: Option<u32> = None;
    let mut last_pointer_lock: Option<bool> = None;
    let mut last_clipboard_serial: Option<u64> = None;

    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(4)).await;

        let sessions_now = video.active_sessions.load(Ordering::Relaxed);
        if sessions_now == 0 {
            video.packet_outbox.clear();
        }

        let snap: Vec<Arc<VideoSession>> = {
            let guard = sessions.read().await;
            guard.values().cloned().collect()
        };

        // Output subscriptions + topology (also while locked: clients learn of
        // hot-plug immediately, media stays paused below).
        sync_subscriptions(&snap, &video);
        // Report demand *after* the resolve above so a gained-output join bump
        // travels with the same tick.
        let demand = (sessions_now, video.session_joins.load(Ordering::SeqCst));
        if last_demand != Some(demand) {
            video.link.session_demand(demand.0, demand.1);
            last_demand = Some(demand);
        }
        broadcast_layout(&snap, &video.layout).await;

        let locked = video.pointer_locked.load(Ordering::Relaxed);
        if last_pointer_lock != Some(locked) {
            let msg = RudpControlMsg::PointerLock { locked };
            for session in &snap {
                let mut send = session.control_send.lock().await;
                let _ = write_control_msg(&mut send, &msg).await;
            }
            last_pointer_lock = Some(locked);
        }

        // Pause media while the desktop session is locked (RDP parity).
        if video.session_locked.load(Ordering::Relaxed) {
            video.packet_outbox.clear();
            continue;
        }

        let clipboard = video.clipboard_out.lock().ok().and_then(|g| g.clone());
        if let Some(clip) = clipboard
            && last_clipboard_serial != Some(clip.serial)
        {
            let msg = RudpControlMsg::ClipboardSet {
                mime: clip.mime.clone(),
                text: clip.text.clone(),
                serial: clip.serial,
            };
            for session in &snap {
                let mut send = session.control_send.lock().await;
                let _ = write_control_msg(&mut send, &msg).await;
            }
            last_clipboard_serial = Some(clip.serial);
        }

        announce_streams(&snap, &video).await;

        // System audio (host → client Opus datagrams).
        let audio_streaming = video.audio.streaming.load(Ordering::Relaxed);
        if audio_streaming {
            let msg = RudpControlMsg::AudioReady {
                sample_rate: RUDP_AUDIO_SAMPLE_RATE,
                channels: RUDP_AUDIO_CHANNELS,
                codec: "opus".into(),
            };
            for session in &snap {
                if session.audio_ready_sent.load(Ordering::Relaxed) {
                    continue;
                }
                let mut send = session.control_send.lock().await;
                if write_control_msg(&mut send, &msg).await.is_ok() {
                    session.audio_ready_sent.store(true, Ordering::Relaxed);
                }
            }
            if let Ok(guard) = video.audio.latest.lock()
                && let Some(pkt) = guard.clone()
                && last_audio_seq != Some(pkt.seq)
            {
                last_audio_seq = Some(pkt.seq);
                for session in &snap {
                    if session.conn.datagram_send_buffer_space() == 0 {
                        continue;
                    }
                    match session
                        .conn
                        .send_datagram(Bytes::from(pkt.datagram.clone()))
                    {
                        Ok(()) => {}
                        Err(quinn::SendDatagramError::UnsupportedByPeer) => {
                            if !session.warned_no_dgram.swap(true, Ordering::Relaxed) {
                                tracing::warn!(
                                    "rudp host: peer does not support datagrams — audio dropped"
                                );
                            }
                        }
                        Err(quinn::SendDatagramError::Disabled) => {}
                        Err(quinn::SendDatagramError::TooLarge) => {
                            tracing::debug!("rudp host: audio datagram too large");
                        }
                        Err(err) => {
                            tracing::debug!(%err, "rudp host: audio send_datagram failed");
                        }
                    }
                }
            }
        }

        if snap.is_empty() {
            continue;
        }

        // Drain the outbox — keyframe writes may take hundreds of ms, so the
        // frame thread must be able to queue deltas without overwriting.
        while let Some(out) = video.packet_outbox.pop() {
            let pkt = &out.packet;
            let codec = pkt.codec;
            // Only sessions subscribed to this output (and told about its stream).
            let targets: Vec<&Arc<VideoSession>> = snap
                .iter()
                .filter(|s| s.accepts_media(&out.output_id))
                .collect();
            if targets.is_empty() {
                continue;
            }

            if pkt.is_keyframe {
                let au = ReliableAccessUnit {
                    frame_seq: pkt.seq,
                    pts_us: pkt.pts_us,
                    codec,
                    stream_id: out.stream_id,
                    damage_full: pkt.damage_full,
                    width: out.width,
                    height: out.height,
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
                for session in targets {
                    let mut send = session.video_send.lock().await;
                    // A client that stops reading must not stall the pump for everyone.
                    match tokio::time::timeout(PUMP_WRITE_TIMEOUT, send.write_all(&framed)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => tracing::debug!(%err, "rudp host: keyframe write failed"),
                        Err(_) => {
                            tracing::warn!(
                                "rudp host: keyframe write timed out — closing slow client"
                            );
                            session.conn.close(0u32.into(), b"client too slow");
                        }
                    }
                }
                continue;
            }

            for session in targets {
                let max_dg = session
                    .conn
                    .max_datagram_size()
                    .unwrap_or(RUDP_DEFAULT_DATAGRAM_BUDGET);
                let stats = session.conn.stats();
                let sent = stats.path.sent_packets.max(1);
                let loss_ratio = stats.path.lost_packets as f64 / sent as f64;
                let dgrams = match build_media_datagrams(
                    &pkt.data,
                    MediaDatagramParams {
                        frame_seq: pkt.seq,
                        pts_us: pkt.pts_us,
                        codec,
                        stream_id: out.stream_id,
                        damage_full: pkt.damage_full,
                        max_datagram: max_dg,
                        loss_ratio,
                    },
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
    }
    tracing::info!("rudp host: video pump stopped");
}

async fn authenticate_connection(
    conn: &quinn::Connection,
    video: &Arc<EngineShared>,
    auth_guard: &Arc<Mutex<AuthGuard>>,
    peer_ip: IpAddr,
) -> Result<(String, String, quinn::SendStream, quinn::RecvStream), String> {
    // Client opens the control bi-stream.
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(15), conn.accept_bi())
        .await
        .map_err(|_| "auth: timed out waiting for control stream".to_string())?
        .map_err(|e| format!("auth: accept_bi: {e}"))?;

    let hello = read_control_msg(&mut recv).await?;
    let username = match hello {
        RudpControlMsg::Hello { protocol, username } => {
            if protocol != RUDP_PROTOCOL_VERSION {
                let _ = auth_fail_backoff(auth_guard, peer_ip).await;
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
            let _ = auth_fail_backoff(auth_guard, peer_ip).await;
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
        let guard = video
            .allowed_users
            .read()
            .map_err(|_| "allowlist lock poisoned".to_string())?;
        guard.iter().any(|u| u == &username)
    };
    if !allowed {
        let _ = auth_fail_backoff(auth_guard, peer_ip).await;
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
    let mut password = match response {
        RudpControlMsg::AuthResponse { password } => password,
        other => {
            let _ = auth_fail_backoff(auth_guard, peer_ip).await;
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
    let password_for_pam = password.clone();
    let ok = tokio::task::spawn_blocking(move || {
        let result = pam_check(&service, &user_for_pam, &password_for_pam);
        let mut pw = password_for_pam;
        pw.zeroize();
        result
    })
    .await
    .map_err(|e| format!("pam join: {e}"))?;
    password.zeroize();

    if !ok {
        let _ = auth_fail_backoff(auth_guard, peer_ip).await;
        write_control_reject(&mut send, RudpRejectReason::AuthFailed, None).await;
        return Err("pam authentication failed".into());
    }

    if let Ok(mut g) = auth_guard.lock() {
        g.record_success(peer_ip);
    }

    let session_id = format!("s-{nonce}");
    write_control_msg(
        &mut send,
        &RudpControlMsg::SessionOk {
            session_id: session_id.clone(),
        },
    )
    .await?;
    Ok((session_id, username, send, recv))
}

async fn auth_fail_backoff(auth_guard: &Arc<Mutex<AuthGuard>>, peer_ip: IpAddr) -> Duration {
    let delay = auth_guard
        .lock()
        .ok()
        .map(|mut g| g.record_failure(peer_ip, Instant::now()))
        .unwrap_or(Duration::from_millis(400));
    tokio::time::sleep(delay).await;
    delay
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
    let _ = write_control_msg(send, &RudpControlMsg::Reject { reason, detail }).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use metis_protocol::{CODEC_AV1, CODEC_H264};

    fn test_pkt(seq: u64, key: bool) -> OutboundPacket {
        test_pkt_on(0, seq, key)
    }

    fn test_pkt_on(stream_id: u8, seq: u64, key: bool) -> OutboundPacket {
        OutboundPacket {
            stream_id,
            output_id: format!("OUT-{stream_id}"),
            width: 1920,
            height: 1080,
            packet: MediaPacket {
                seq,
                codec: CODEC_H264,
                is_keyframe: key,
                data: vec![seq as u8],
                pts_us: seq as i64,
                damage_full: key,
                damage: Vec::new(),
            },
        }
    }

    fn monitor(id: &str, x: i32, primary: bool) -> RudpMonitorInfo {
        RudpMonitorInfo {
            id: id.into(),
            name: id.into(),
            x,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary,
        }
    }

    fn test_shared() -> Arc<EngineShared> {
        Arc::new(EngineShared::new(
            Arc::new(NullLink),
            Arc::new(RudpLayoutShared::new()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            vec!["alice".into()],
            false,
        ))
    }

    #[test]
    fn session_guard_counts_and_releases() {
        let video = test_shared();
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
    fn packet_outbox_preserves_deltas_under_backpressure() {
        let box_ = PacketOutbox::new();
        box_.push(test_pkt(1, true));
        for seq in 2..=PACKET_OUTBOX_CAP as u64 {
            box_.push(test_pkt(seq, false));
        }
        assert_eq!(box_.len(), PACKET_OUTBOX_CAP);
        // Consumer still blocked — more deltas must not wipe the IDR.
        let newest = PACKET_OUTBOX_CAP as u64 + 1;
        box_.push(test_pkt(newest, false));
        let seqs = box_.seqs();
        assert_eq!(seqs[0], 1, "keyframe must remain at head");
        assert!(seqs.contains(&newest), "newest delta kept after drop");
        assert_eq!(seqs.len(), PACKET_OUTBOX_CAP);
        // Pop all — IDR then remaining deltas in order.
        let first = box_.pop().expect("idr");
        assert!(first.packet.is_keyframe);
        assert_eq!(first.packet.seq, 1);
        let mut rest = Vec::new();
        while let Some(p) = box_.pop() {
            rest.push(p.packet.seq);
        }
        assert!(!rest.is_empty());
        assert!(rest.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn packet_outbox_keyframe_evicts_delta_when_full() {
        let box_ = PacketOutbox::new();
        for seq in 1..=PACKET_OUTBOX_CAP as u64 {
            box_.push(test_pkt(seq, false));
        }
        box_.push(test_pkt(99, true));
        let seqs = box_.seqs();
        assert_eq!(seqs.len(), PACKET_OUTBOX_CAP);
        assert!(seqs.contains(&99));
        assert!(seqs.iter().any(|&s| {
            // One of the original deltas was dropped to make room.
            (1..=PACKET_OUTBOX_CAP as u64).contains(&s)
        }));
    }

    #[test]
    fn packet_outbox_evicts_same_stream_first() {
        let box_ = PacketOutbox::new();
        // Stream 1 fills most of the queue, stream 0 has one old delta at the head.
        box_.push(test_pkt_on(0, 1, false));
        for seq in 2..=PACKET_OUTBOX_CAP as u64 {
            box_.push(test_pkt_on(1, seq, false));
        }
        assert_eq!(box_.len(), PACKET_OUTBOX_CAP);
        box_.push(test_pkt_on(1, 1000, false));
        let seqs = box_.seqs();
        assert!(
            seqs.contains(&1),
            "stream 0's delta survives stream 1 pressure"
        );
        assert!(!seqs.contains(&2), "oldest stream-1 delta was evicted");
        assert!(seqs.contains(&1000));
    }

    #[test]
    fn packet_outbox_pop_wait_times_out_and_wakes() {
        let box_ = Arc::new(PacketOutbox::new());
        assert!(box_.pop_wait(Duration::from_millis(5)).is_none());
        let producer = Arc::clone(&box_);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            producer.push(test_pkt(7, true));
        });
        let got = box_.pop_wait(Duration::from_secs(2));
        let _ = handle.join();
        assert_eq!(got.map(|p| p.packet.seq), Some(7));
    }

    #[test]
    fn sanitize_subscription_filters_and_bounds() {
        assert_eq!(sanitize_subscription(vec![]), None);
        assert_eq!(sanitize_subscription(vec![String::new()]), None);
        assert_eq!(
            sanitize_subscription(vec!["DP-1".into(), "DP-1".into(), "x".repeat(500)]),
            Some(vec!["DP-1".to_string()])
        );
        let many: Vec<String> = (0..100).map(|i| format!("O-{i}")).collect();
        let got = sanitize_subscription(many).expect("some");
        assert_eq!(got.len(), MAX_REQUESTED_OUTPUT_IDS);
    }

    #[test]
    fn resolve_subscription_defaults_to_primary() {
        let monitors = vec![monitor("DP-1", 0, false), monitor("DP-2", 1920, true)];
        assert_eq!(resolve_subscription(None, &monitors), vec!["DP-2"]);
        assert_eq!(
            resolve_subscription(Some(&["nope".to_string()]), &monitors),
            vec!["DP-2"],
            "unknown ids fall back to primary"
        );
        assert_eq!(
            resolve_subscription(Some(&["DP-1".to_string()]), &monitors),
            vec!["DP-1"]
        );
        assert!(resolve_subscription(None, &[]).is_empty());
        // No `primary` flag → first monitor.
        let flat = vec![monitor("A", 0, false), monitor("B", 10, false)];
        assert_eq!(resolve_subscription(None, &flat), vec!["A"]);
    }

    #[test]
    fn resolve_subscription_caps_at_max() {
        let monitors: Vec<RudpMonitorInfo> = (0..6)
            .map(|i| monitor(&format!("M-{i}"), i * 100, i == 0))
            .collect();
        let req: Vec<String> = (0..6).map(|i| format!("M-{i}")).collect();
        let got = resolve_subscription(Some(&req), &monitors);
        assert_eq!(got.len(), RUDP_MAX_SUBSCRIBED_OUTPUTS);
        assert_eq!(got[0], "M-0");
    }

    #[test]
    fn union_follows_layout_order_and_dedups() {
        let monitors = vec![
            monitor("DP-1", 0, true),
            monitor("DP-2", 1920, false),
            monitor("DP-3", 3840, false),
        ];
        let per_session = vec![
            vec!["DP-3".to_string(), "DP-1".to_string()],
            vec!["DP-1".to_string()],
        ];
        assert_eq!(
            union_subscriptions(&per_session, &monitors),
            vec!["DP-1", "DP-3"]
        );
        assert!(union_subscriptions(&[], &monitors).is_empty());
    }

    #[test]
    fn layout_publish_bumps_generation_only_on_change() {
        let layout = RudpLayoutShared::new();
        assert_eq!(layout.generation(), 0);
        assert!(layout.publish(vec![monitor("DP-1", 0, true)]));
        assert_eq!(layout.generation(), 1);
        assert!(!layout.publish(vec![monitor("DP-1", 0, true)]));
        assert_eq!(layout.generation(), 1);
        assert!(layout.publish(vec![monitor("DP-1", 0, true), monitor("DP-2", 1920, false)]));
        let (generation, monitors) = layout.snapshot();
        assert_eq!(generation, 2);
        assert_eq!(monitors.len(), 2);
    }

    #[test]
    fn stream_table_assigns_stable_recyclable_ids() {
        let table = StreamTable::new();
        assert_eq!(table.publish("DP-1", 1920, 1080, CODEC_H264), Some(0));
        assert_eq!(table.publish("DP-2", 2560, 1440, CODEC_H264), Some(1));
        let generation = table.generation();
        assert_eq!(
            table.publish("DP-1", 1920, 1080, CODEC_H264),
            Some(0),
            "id is stable"
        );
        assert_eq!(table.generation(), generation, "unchanged info: no bump");
        assert_eq!(table.publish("DP-1", 1280, 720, CODEC_H264), Some(0));
        assert!(table.generation() > generation, "resize re-advertises");
        let snap = table.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].output_id, "DP-2");
        assert_eq!((snap[1].width, snap[1].height), (2560, 1440));
        // Dropping DP-1 frees id 0 for the next output.
        table.retain(&["DP-2".to_string()]);
        assert_eq!(table.publish("HDMI-A-1", 800, 600, CODEC_AV1), Some(0));
    }

    #[test]
    fn stream_table_replace_mirrors_and_bumps_once() {
        let table = StreamTable::new();
        let infos = vec![StreamInfo {
            stream_id: 3,
            output_id: "DP-1".into(),
            width: 1920,
            height: 1080,
            codec: CODEC_H264,
        }];
        table.replace(infos.clone());
        let generation = table.generation();
        assert_eq!(table.snapshot(), infos);
        table.replace(infos);
        assert_eq!(
            table.generation(),
            generation,
            "identical snapshot: no bump"
        );
        table.replace(Vec::new());
        assert!(table.snapshot().is_empty());
        assert!(table.generation() > generation);
    }

    #[test]
    fn session_subscription_accepts_media_needs_announce() {
        // `VideoSession` needs a live Quinn connection, so exercise the
        // subscription state machine directly.
        let mut subs = SessionSubscription::default();
        let monitors = vec![monitor("DP-1", 0, true), monitor("DP-2", 1920, false)];
        subs.effective = resolve_subscription(subs.requested.as_deref(), &monitors);
        assert_eq!(subs.effective, vec!["DP-1"]);
        subs.requested = sanitize_subscription(vec!["DP-2".into(), "DP-1".into()]);
        let next = resolve_subscription(subs.requested.as_deref(), &monitors);
        assert!(
            next.iter().any(|id| !subs.effective.contains(id)),
            "gained DP-2"
        );
    }

    #[test]
    fn sanitize_notify_user_strips_controls() {
        assert_eq!(sanitize_notify_user("alice"), "alice");
        assert_eq!(sanitize_notify_user("\0bob\n"), "bob");
        assert_eq!(sanitize_notify_user("   "), "remote user");
        assert_eq!(sanitize_notify_user(&"x".repeat(80)).len(), 64);
    }

    #[test]
    fn set_policy_updates_allowlist_and_unlock_flag() {
        let video = test_shared();
        video.set_policy(vec!["bob".into()], true);
        assert_eq!(
            video
                .allowed_users
                .read()
                .map(|g| g.clone())
                .unwrap_or_default(),
            vec!["bob".to_string()]
        );
        assert!(video.unlock_session_on_auth.load(Ordering::Relaxed));
    }

    #[test]
    fn clipboard_push_requires_active_session() {
        let video = test_shared();
        video.push_clipboard_text("text/plain", "hi");
        assert!(
            video
                .clipboard_out
                .lock()
                .map(|g| g.is_none())
                .unwrap_or(false)
        );
        let _guard = SessionGuard::new(&video);
        video.push_clipboard_text("", "hi");
        let got = video.clipboard_out.lock().ok().and_then(|g| g.clone());
        assert_eq!(
            got.map(|c| (c.serial, c.mime)),
            Some((1, "text/plain;charset=utf-8".to_string()))
        );
    }
}
