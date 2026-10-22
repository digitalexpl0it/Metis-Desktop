//! `metis-rudp-host` — Metis Remote network edge as its own process.
//!
//! Spawned by `metis-compositor` (`--socket <path>`); never started by hand in
//! a normal session. It connects back to the compositor's private Unix socket,
//! receives the policy (`Config`), desktop layout and encoded media, binds
//! Quinn, authenticates clients with PAM and sends input / unlock / output
//! subscriptions back. It dies when the socket closes, so it can never outlive
//! the session that owns it.
#![deny(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metis_protocol::{
    CompToHost, HostToComp, RUDP_HOST_IPC_VERSION, RudpHostInput, codec_from_str,
    read_host_ipc_frame, write_host_ipc_frame,
};
use metis_rudp_host::engine::{
    EngineShared, HostLink, MediaPacket, OutboundPacket, RudpAudioPacket, RudpClipboardOut,
    RudpLayoutShared, StreamInfo, run_quinn_runtime,
};
use metis_rudp_host::ipc_sock;

/// Outbound IPC queue depth; a stuck compositor must not grow host memory.
const OUT_QUEUE: usize = 8192;
/// How long to wait for the compositor's socket / first `Config`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A compositor that stops draining the socket is treated as dead.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// [`HostLink`] that forwards engine callbacks to the compositor over IPC.
struct IpcLink {
    tx: SyncSender<HostToComp>,
    last_joins: AtomicU64,
}

impl IpcLink {
    fn send(&self, msg: HostToComp) {
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!("rudp host: ipc queue full — dropping message");
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

impl HostLink for IpcLink {
    fn input(&self, event: RudpHostInput) {
        self.send(HostToComp::Input { event });
    }

    fn unlock_pam_session(&self, username: String) {
        self.send(HostToComp::UnlockPamSession { username });
    }

    fn subscribe_union(&self, output_ids: Vec<String>, gained: bool) {
        self.send(HostToComp::SubscribeUnion { output_ids, gained });
    }

    fn session_demand(&self, sessions: usize, joins: u64) {
        let prev = self.last_joins.swap(joins, Ordering::Relaxed);
        self.send(HostToComp::SessionDemand {
            active_sessions: u32::try_from(sessions).unwrap_or(u32::MAX),
            session_joins: joins,
        });
        if joins != prev && sessions > 0 {
            // A client joined: make the compositor repaint + emit a full frame
            // even when the desktop is idle.
            self.send(HostToComp::RefreshVideo);
        }
    }

    fn set_stream_quality(&self, quality: String) {
        self.send(HostToComp::SetStreamQuality { quality });
    }

    fn status(&self, listening: bool, fingerprint: &str, bind: SocketAddr, err: Option<String>) {
        self.send(HostToComp::Status {
            listening,
            fingerprint: fingerprint.to_string(),
            bind: bind.to_string(),
            error: err,
        });
    }
}

struct Args {
    socket: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => {
                socket = Some(PathBuf::from(
                    it.next().ok_or("--socket needs a path argument")?,
                ));
            }
            "-h" | "--help" => {
                println!(
                    "metis-rudp-host --socket <path>\n\
                     Metis Remote network host; spawned by metis-compositor."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        socket: socket.unwrap_or_else(metis_protocol::rudp_host_ipc_path),
    })
}

/// Harden the process: no core dumps / ptrace of password memory, and die with
/// the compositor even if the socket EOF is somehow missed.
fn harden_process() {
    // SAFETY: prctl with these option values only reads its integer arguments.
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            tracing::debug!("rudp host: PR_SET_DUMPABLE failed");
        }
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
            tracing::debug!("rudp host: PR_SET_PDEATHSIG failed");
        }
    }
}

fn connect(path: &PathBuf) -> Result<UnixStream, String> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(err) if Instant::now() < deadline => {
                tracing::debug!(%err, "rudp host: compositor socket not ready");
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(format!("connect {}: {err}", path.display())),
        }
    }
}

fn parse_bind(raw: &str) -> Result<SocketAddr, String> {
    raw.parse::<SocketAddr>()
        .map_err(|e| format!("invalid bind {raw:?}: {e}"))
}

fn stream_info_from_ipc(s: metis_protocol::IpcStreamInfo) -> StreamInfo {
    StreamInfo {
        stream_id: s.stream_id,
        output_id: s.output_id,
        width: s.width,
        height: s.height,
        codec: codec_from_str(&s.codec),
    }
}

/// Apply one compositor message to the engine. Returns `false` on `Shutdown`.
fn apply(shared: &Arc<EngineShared>, bind: SocketAddr, msg: CompToHost) -> bool {
    match msg {
        CompToHost::Config {
            ipc_version,
            bind: new_bind,
            allowed_users,
            unlock_session_on_auth,
            stream_quality,
            ..
        } => {
            if ipc_version != RUDP_HOST_IPC_VERSION {
                tracing::warn!(ipc_version, "rudp host: ipc version mismatch");
            }
            if new_bind != bind.to_string() {
                // The compositor restarts the host on a bind change; a live
                // Config never rebinds.
                tracing::warn!(%new_bind, "rudp host: bind change ignored (restart required)");
            }
            shared.set_policy(allowed_users, unlock_session_on_auth);
            shared.set_stream_quality_str(&stream_quality);
        }
        CompToHost::StreamQuality { quality } => {
            shared.set_stream_quality_str(&quality);
        }
        CompToHost::DesktopLayout { monitors, .. } => {
            shared.layout.publish(monitors);
        }
        CompToHost::SessionLocked { locked } => {
            shared.session_locked.store(locked, Ordering::Relaxed);
        }
        CompToHost::PointerLocked { locked } => {
            shared.pointer_locked.store(locked, Ordering::Relaxed);
        }
        CompToHost::StreamAnnounce { streams } => {
            shared
                .streams
                .replace(streams.into_iter().map(stream_info_from_ipc).collect());
        }
        CompToHost::EncodedVideo(v) => {
            shared.packet_outbox.push(OutboundPacket {
                stream_id: v.stream_id,
                output_id: v.output_id,
                width: v.width,
                height: v.height,
                packet: MediaPacket {
                    seq: v.seq,
                    pts_us: v.pts_us,
                    codec: codec_from_str(&v.codec),
                    is_keyframe: v.is_keyframe,
                    damage_full: v.damage_full,
                    damage: v.damage,
                    data: v.data,
                },
            });
        }
        CompToHost::EncodedAudio(a) => {
            if let Ok(mut slot) = shared.audio.latest.lock() {
                *slot = Some(RudpAudioPacket {
                    seq: a.seq,
                    pts_us: a.pts_us,
                    datagram: a.datagram,
                });
            }
        }
        CompToHost::AudioState { streaming } => {
            shared.audio.streaming.store(streaming, Ordering::Relaxed);
            if !streaming && let Ok(mut slot) = shared.audio.latest.lock() {
                *slot = None;
            }
        }
        CompToHost::ClipboardOut { serial, mime, text } => {
            shared.store_clipboard(RudpClipboardOut { serial, mime, text });
        }
        CompToHost::Shutdown => return false,
    }
    true
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    harden_process();

    let stream = connect(&args.socket)?;
    ipc_sock::verify_peer(&stream)?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|e| format!("ipc write timeout: {e}"))?;
    let mut reader = stream.try_clone().map_err(|e| format!("ipc clone: {e}"))?;
    let mut writer = stream;

    // First frame must be `Config`.
    let first: CompToHost = read_host_ipc_frame(&mut reader)?
        .ok_or_else(|| "compositor closed the socket before Config".to_string())?;
    let CompToHost::Config {
        ipc_version,
        bind,
        allowed_users,
        unlock_session_on_auth,
        stream_quality,
        ..
    } = first
    else {
        return Err("first ipc message was not Config".into());
    };
    if ipc_version != RUDP_HOST_IPC_VERSION {
        return Err(format!(
            "ipc version mismatch: compositor {ipc_version}, host {RUDP_HOST_IPC_VERSION}"
        ));
    }
    if allowed_users.is_empty() {
        return Err("refused — allowed_users is empty".into());
    }
    let bind_addr = parse_bind(&bind)?;

    let (tx, rx) = mpsc::sync_channel::<HostToComp>(OUT_QUEUE);
    let link = Arc::new(IpcLink {
        tx,
        last_joins: AtomicU64::new(0),
    });
    let shared = Arc::new(EngineShared::new(
        link,
        Arc::new(RudpLayoutShared::new()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        allowed_users,
        unlock_session_on_auth,
    ));
    shared.set_stream_quality_str(&stream_quality);
    let stop = Arc::new(AtomicBool::new(false));
    // Last writer error (surfaced when Quinn exits because IPC died).
    let ipc_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let writer_stop = Arc::clone(&stop);
    let writer_err = Arc::clone(&ipc_error);
    std::thread::Builder::new()
        .name("metis-rudp-ipc-w".into())
        .spawn(move || {
            while let Ok(msg) = rx.recv() {
                if let Err(err) = write_host_ipc_frame(&mut writer, &msg) {
                    if let Ok(mut slot) = writer_err.lock() {
                        *slot = Some(err);
                    }
                    writer_stop.store(true, Ordering::SeqCst);
                    return;
                }
            }
        })
        .map_err(|e| format!("spawn ipc writer: {e}"))?;

    let reader_stop = Arc::clone(&stop);
    let reader_shared = Arc::clone(&shared);
    let reader_err = Arc::clone(&ipc_error);
    std::thread::Builder::new()
        .name("metis-rudp-ipc-r".into())
        .spawn(move || {
            loop {
                match read_host_ipc_frame::<_, CompToHost>(&mut reader) {
                    Ok(Some(msg)) => {
                        if !apply(&reader_shared, bind_addr, msg) {
                            break;
                        }
                    }
                    Ok(None) => {
                        tracing::info!("rudp host: compositor closed the socket — exiting");
                        break;
                    }
                    Err(err) => {
                        if let Ok(mut slot) = reader_err.lock() {
                            *slot = Some(err);
                        }
                        break;
                    }
                }
            }
            reader_stop.store(true, Ordering::SeqCst);
        })
        .map_err(|e| format!("spawn ipc reader: {e}"))?;

    tracing::info!(
        bind = %bind_addr,
        socket = %args.socket.display(),
        "rudp host: connected to compositor"
    );
    let result = run_quinn_runtime(bind_addr, Arc::clone(&stop), shared);
    stop.store(true, Ordering::SeqCst);
    if let Ok(slot) = ipc_error.lock()
        && let Some(err) = slot.as_ref()
    {
        tracing::warn!(%err, "rudp host: ipc error");
    }
    result
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "rudp host: fatal");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_protocol::{IpcEncodedVideo, codec_to_str};
    use metis_rudp_host::engine::NullLink;

    fn shared() -> Arc<EngineShared> {
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
    fn apply_routes_state_and_media() {
        let shared = shared();
        let bind: SocketAddr = "127.0.0.1:7843".parse().expect("addr");
        assert!(apply(
            &shared,
            bind,
            CompToHost::PointerLocked { locked: true }
        ));
        assert!(shared.pointer_locked.load(Ordering::Relaxed));
        assert!(apply(
            &shared,
            bind,
            CompToHost::StreamAnnounce {
                streams: vec![metis_protocol::IpcStreamInfo {
                    stream_id: 1,
                    output_id: "DP-1".into(),
                    width: 640,
                    height: 480,
                    codec: codec_to_str(metis_protocol::CODEC_HEVC).into(),
                }]
            }
        ));
        let snap = shared.streams.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].codec, metis_protocol::CODEC_HEVC);
        assert!(apply(
            &shared,
            bind,
            CompToHost::EncodedVideo(IpcEncodedVideo {
                stream_id: 1,
                output_id: "DP-1".into(),
                codec: "hevc".into(),
                is_keyframe: true,
                data: vec![1, 2, 3],
                ..Default::default()
            })
        ));
        let pkt = shared.packet_outbox.pop().expect("queued");
        assert_eq!(pkt.packet.data, vec![1, 2, 3]);
        assert_eq!(pkt.packet.codec, metis_protocol::CODEC_HEVC);
        assert!(!apply(&shared, bind, CompToHost::Shutdown));
    }

    #[test]
    fn config_updates_policy_without_rebind() {
        let shared = shared();
        let bind: SocketAddr = "127.0.0.1:7843".parse().expect("addr");
        assert!(apply(
            &shared,
            bind,
            CompToHost::Config {
                ipc_version: RUDP_HOST_IPC_VERSION,
                bind: "127.0.0.1:7843".into(),
                allowed_users: vec!["bob".into()],
                lan_only: true,
                unlock_session_on_auth: true,
                stream_quality: "high".into(),
            }
        ));
        assert_eq!(
            shared
                .allowed_users
                .read()
                .map(|g| g.clone())
                .unwrap_or_default(),
            vec!["bob".to_string()]
        );
        assert!(shared.unlock_session_on_auth.load(Ordering::Relaxed));
    }

    #[test]
    fn parse_bind_rejects_garbage() {
        assert!(parse_bind("not an addr").is_err());
        assert!(parse_bind("0.0.0.0:7843").is_ok());
    }
}
