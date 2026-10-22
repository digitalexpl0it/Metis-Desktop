//! Compositor side of the `metis-rudp-host` link (Phase 23 §D).
//!
//! The compositor spawns `metis-rudp-host`, accepts its connection on a
//! private Unix socket (`0700` dir, `0600` socket, `SO_PEERCRED` UID check) and
//! then:
//!
//! - **forwards** desktop layout, pointer / session lock state, the output
//!   stream table, clipboard, system audio and every encoded access unit the
//!   frame pipeline produces ([`CompToHost`]);
//! - **applies** what the host reports back ([`HostToComp`]): session demand
//!   (drives export + encode), the union of output subscriptions, input /
//!   clipboard-set events (into the existing calloop channel), PAM-unlock
//!   requests and listener status.
//!
//! The host is *not trusted with the session*: unlock requests are honoured
//! only while an authenticated session exists and the compositor's own
//! `unlock_session_on_auth` policy is on, and the calloop handler still checks
//! the session owner. If the host dies it is restarted with bounded back-off
//! ([`RESTART_BACKOFF`]) and then left off — the desktop never notices.

use std::net::SocketAddr;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use metis_protocol::{
    CompToHost, HostToComp, IpcEncodedAudio, IpcEncodedVideo, IpcStreamInfo, RUDP_HOST_IPC_VERSION,
    codec_to_str, read_host_ipc_frame, write_host_ipc_frame,
};
use metis_rudp_host::engine::EngineShared;
use metis_rudp_host::ipc_sock;
use smithay::reexports::calloop;

use crate::rudp_host::{NetStatusHandle, RudpInputEvent, record_net_status};
use crate::stream_export::StreamExportHub;

/// Back-off between host restarts; after the last entry Remote stays off for
/// this session (re-enable in Settings to retry).
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
/// A host that stayed up this long resets the failure count.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);
/// How long a freshly spawned host may take to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A host that stops draining the socket is treated as dead (also bounds how
/// long `RudpHostSystem::shutdown` can block calloop on a wedged host).
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Grace period after `Shutdown` before the child is killed.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// Locate `metis-rudp-host`: `METIS_RUDP_HOST_BIN`, next to the compositor
/// binary, then `PATH`.
pub fn host_binary_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("METIS_RUDP_HOST_BIN").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("metis-rudp-host");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    let path_var = std::env::var("PATH").ok()?;
    path_var
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("metis-rudp-host"))
        .find(|c| c.is_file())
}

/// Everything the supervisor needs from the compositor.
pub struct IpcHostParams {
    pub bind: SocketAddr,
    pub allowed_users: Vec<String>,
    pub lan_only: bool,
    pub unlock_session_on_auth: bool,
    pub shared: Arc<EngineShared>,
    pub hub: Arc<StreamExportHub>,
    pub input_tx: calloop::channel::Sender<RudpInputEvent>,
    pub status: NetStatusHandle,
    pub stop: Arc<AtomicBool>,
}

/// Handle to the host-process supervisor thread.
pub struct IpcHost {
    policy_tx: Sender<CompToHost>,
    join: Option<JoinHandle<()>>,
}

fn config_msg(
    bind: SocketAddr,
    allowed_users: Vec<String>,
    lan_only: bool,
    unlock_session_on_auth: bool,
) -> CompToHost {
    CompToHost::Config {
        ipc_version: RUDP_HOST_IPC_VERSION,
        bind: bind.to_string(),
        allowed_users,
        lan_only,
        unlock_session_on_auth,
    }
}

impl IpcHost {
    /// Validate the install and start the supervisor thread.
    pub fn spawn(params: IpcHostParams) -> Result<Self, String> {
        let bin = host_binary_path().ok_or_else(|| {
            "metis-rudp-host not found (install it next to metis-compositor, or set \
             METIS_RUDP_INPROCESS=1 for the emergency in-process path)"
                .to_string()
        })?;
        // Fail fast (and visibly) when the runtime dir is unusable.
        ipc_sock::prepare_ipc_dir()?;
        let (policy_tx, policy_rx) = mpsc::channel();
        let config = config_msg(
            params.bind,
            params.allowed_users.clone(),
            params.lan_only,
            params.unlock_session_on_auth,
        );
        let join = std::thread::Builder::new()
            .name("metis-rudp-ipc".into())
            .spawn(move || supervise(params, bin, config, policy_rx))
            .map_err(|e| format!("spawn rudp ipc supervisor: {e}"))?;
        Ok(Self {
            policy_tx,
            join: Some(join),
        })
    }

    /// Live policy change (same bind): forwarded as a fresh `Config`.
    pub fn update_policy(
        &self,
        bind: SocketAddr,
        allowed_users: Vec<String>,
        lan_only: bool,
        unlock_session_on_auth: bool,
    ) {
        let _ = self.policy_tx.send(config_msg(
            bind,
            allowed_users,
            lan_only,
            unlock_session_on_auth,
        ));
    }

    /// Join the supervisor (the shared stop flag must already be set). Bounded:
    /// the child gets [`EXIT_GRACE`] and is then killed.
    pub fn shutdown(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Kills and reaps the host child on drop (never leaves a zombie / orphan).
struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    /// `Some(status text)` once the child has exited.
    fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(err) => Some(format!("wait failed: {err}")),
        }
    }

    /// Wait up to `grace` for a clean exit, then kill.
    fn finish(mut self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if self.exited().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Drop kills + reaps.
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Remove the socket file when the session ends.
struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn supervise(
    params: IpcHostParams,
    bin: PathBuf,
    mut config: CompToHost,
    policy_rx: Receiver<CompToHost>,
) {
    let mut failures = 0usize;
    while !params.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let result = run_session(&params, &bin, &mut config, &policy_rx);
        reset_remote_state(&params);
        if params.stop.load(Ordering::Relaxed) {
            break;
        }
        if started.elapsed() >= HEALTHY_AFTER {
            failures = 0;
        }
        let reason = result.err().unwrap_or_else(|| "host stopped".to_string());
        if failures >= RESTART_BACKOFF.len() {
            tracing::error!(
                %reason,
                "rudp host: metis-rudp-host keeps failing — Metis Remote stays off for this \
                 session (desktop unaffected; re-enable in Settings to retry)"
            );
            record_net_status(
                &params.status,
                false,
                "",
                &params.bind.to_string(),
                Some(reason),
            );
            break;
        }
        let delay = RESTART_BACKOFF[failures];
        failures += 1;
        tracing::warn!(
            %reason,
            failures,
            retry_in_ms = delay.as_millis() as u64,
            "rudp host: metis-rudp-host exited — restarting after back-off"
        );
        let wake = Instant::now() + delay;
        while Instant::now() < wake && !params.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    tracing::info!("rudp host: ipc supervisor stopped");
}

/// Drop everything the dead host left behind: no sessions ⇒ no export demand.
fn reset_remote_state(params: &IpcHostParams) {
    params.shared.active_sessions.store(0, Ordering::SeqCst);
    params.hub.set_subscribed_outputs(Vec::<String>::new());
    params.shared.packet_outbox.clear();
}

/// Poll the non-blocking listener until the host connects.
fn accept_host(
    listener: &UnixListener,
    child: &mut ChildGuard,
    stop: &AtomicBool,
) -> Result<UnixStream, String> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(format!("accept: {e}")),
        }
        if stop.load(Ordering::Relaxed) {
            return Err("stopped while waiting for host".into());
        }
        if let Some(status) = child.exited() {
            return Err(format!("host exited before connecting ({status})"));
        }
        if Instant::now() >= deadline {
            return Err("host did not connect in time".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_session(
    params: &IpcHostParams,
    bin: &Path,
    config: &mut CompToHost,
    policy_rx: &Receiver<CompToHost>,
) -> Result<(), String> {
    let sock_path = ipc_sock::prepare_ipc_dir()?;
    let listener = ipc_sock::bind_listener(&sock_path)?;
    let _socket_file = SocketFile(sock_path.clone());
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("listener nonblocking: {e}"))?;

    let child = Command::new(bin)
        .arg("--socket")
        .arg(&sock_path)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    let mut child = ChildGuard { child };
    tracing::info!(host = %bin.display(), pid = child.child.id(), "rudp host: spawned metis-rudp-host");

    let stream = accept_host(&listener, &mut child, &params.stop)?;
    drop(listener);
    stream
        .set_nonblocking(false)
        .map_err(|e| format!("stream blocking: {e}"))?;
    ipc_sock::verify_peer(&stream)?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|e| format!("ipc write timeout: {e}"))?;
    let reader_stream = stream.try_clone().map_err(|e| format!("ipc clone: {e}"))?;
    let mut writer = stream;

    let dead = Arc::new(AtomicBool::new(false));
    let reader = {
        let shared = Arc::clone(&params.shared);
        let hub = Arc::clone(&params.hub);
        let input_tx = params.input_tx.clone();
        let status = Arc::clone(&params.status);
        let dead = Arc::clone(&dead);
        std::thread::Builder::new()
            .name("metis-rudp-ipc-r".into())
            .spawn(move || reader_loop(reader_stream, shared, hub, input_tx, status, dead))
            .map_err(|e| format!("spawn ipc reader: {e}"))?
    };

    let outcome = forward_loop(params, &mut writer, config, policy_rx, &mut child, &dead);

    if params.stop.load(Ordering::Relaxed) {
        let _ = write_host_ipc_frame(&mut writer, &CompToHost::Shutdown);
    }
    // Unblock the reader and let the host see EOF.
    let _ = writer.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    child.finish(EXIT_GRACE);
    outcome
}

/// Last values pushed to the host, so only changes go on the wire.
struct Forwarded {
    layout_gen: u64,
    pointer_locked: Option<bool>,
    session_locked: Option<bool>,
    streams_gen: u64,
    audio_seq: Option<u32>,
    audio_streaming: Option<bool>,
    clipboard_serial: Option<u64>,
}

impl Default for Forwarded {
    fn default() -> Self {
        Self {
            layout_gen: u64::MAX,
            pointer_locked: None,
            session_locked: None,
            streams_gen: u64::MAX,
            audio_seq: None,
            audio_streaming: None,
            clipboard_serial: None,
        }
    }
}

fn send(writer: &mut UnixStream, msg: &CompToHost) -> Result<(), String> {
    write_host_ipc_frame(writer, msg)
}

/// Push whatever changed since the last pass (cheap atomics when idle).
fn forward_state(
    shared: &EngineShared,
    writer: &mut UnixStream,
    st: &mut Forwarded,
) -> Result<(), String> {
    let layout_gen = shared.layout.generation();
    if layout_gen != st.layout_gen {
        let (generation, monitors) = shared.layout.snapshot();
        send(
            writer,
            &CompToHost::DesktopLayout {
                generation,
                monitors,
            },
        )?;
        st.layout_gen = layout_gen;
    }
    let pointer = shared.pointer_locked.load(Ordering::Relaxed);
    if st.pointer_locked != Some(pointer) {
        send(writer, &CompToHost::PointerLocked { locked: pointer })?;
        st.pointer_locked = Some(pointer);
    }
    let locked = shared.session_locked.load(Ordering::Relaxed);
    if st.session_locked != Some(locked) {
        send(writer, &CompToHost::SessionLocked { locked })?;
        st.session_locked = Some(locked);
    }
    forward_streams(shared, writer, st)?;

    let streaming = shared.audio.streaming.load(Ordering::Relaxed);
    if st.audio_streaming != Some(streaming) {
        send(writer, &CompToHost::AudioState { streaming })?;
        st.audio_streaming = Some(streaming);
    }
    if streaming {
        let pkt = shared.audio.latest.lock().ok().and_then(|g| g.clone());
        if let Some(pkt) = pkt
            && st.audio_seq != Some(pkt.seq)
        {
            st.audio_seq = Some(pkt.seq);
            send(
                writer,
                &CompToHost::EncodedAudio(IpcEncodedAudio {
                    seq: pkt.seq,
                    pts_us: pkt.pts_us,
                    datagram: pkt.datagram,
                }),
            )?;
        }
    } else {
        st.audio_seq = None;
    }

    let clip = shared.clipboard_out.lock().ok().and_then(|g| g.clone());
    if let Some(clip) = clip
        && st.clipboard_serial != Some(clip.serial)
    {
        st.clipboard_serial = Some(clip.serial);
        send(
            writer,
            &CompToHost::ClipboardOut {
                serial: clip.serial,
                mime: clip.mime,
                text: clip.text,
            },
        )?;
    }
    Ok(())
}

/// Send the stream table when it changed (must precede that stream's media).
fn forward_streams(
    shared: &EngineShared,
    writer: &mut UnixStream,
    st: &mut Forwarded,
) -> Result<(), String> {
    let generation = shared.streams.generation();
    if generation == st.streams_gen {
        return Ok(());
    }
    let streams = shared
        .streams
        .snapshot()
        .into_iter()
        .map(|s| IpcStreamInfo {
            stream_id: s.stream_id,
            output_id: s.output_id,
            width: s.width,
            height: s.height,
            codec: codec_to_str(s.codec).to_string(),
        })
        .collect();
    send(writer, &CompToHost::StreamAnnounce { streams })?;
    st.streams_gen = generation;
    Ok(())
}

fn forward_packet(
    shared: &EngineShared,
    writer: &mut UnixStream,
    st: &mut Forwarded,
    out: metis_rudp_host::engine::OutboundPacket,
) -> Result<(), String> {
    // `publish` bumps the generation before `push`, so this guarantees the
    // announce precedes the first access unit of a (re)sized stream.
    forward_streams(shared, writer, st)?;
    let p = out.packet;
    send(
        writer,
        &CompToHost::EncodedVideo(IpcEncodedVideo {
            stream_id: out.stream_id,
            output_id: out.output_id,
            width: out.width,
            height: out.height,
            seq: p.seq,
            pts_us: p.pts_us,
            codec: codec_to_str(p.codec).to_string(),
            is_keyframe: p.is_keyframe,
            damage_full: p.damage_full,
            damage: p.damage,
            data: p.data,
        }),
    )
}

fn forward_loop(
    params: &IpcHostParams,
    writer: &mut UnixStream,
    config: &mut CompToHost,
    policy_rx: &Receiver<CompToHost>,
    child: &mut ChildGuard,
    dead: &AtomicBool,
) -> Result<(), String> {
    send(writer, config)?;
    let shared = &params.shared;
    let mut st = Forwarded::default();
    loop {
        if params.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if dead.load(Ordering::Relaxed) {
            return Err("ipc connection to host lost".into());
        }
        if let Some(status) = child.exited() {
            return Err(format!("host exited ({status})"));
        }
        while let Ok(new_config) = policy_rx.try_recv() {
            *config = new_config;
            send(writer, config)?;
        }
        forward_state(shared, writer, &mut st)?;

        let wait = if shared.active_sessions.load(Ordering::Relaxed) > 0 {
            Duration::from_millis(4)
        } else {
            Duration::from_millis(40)
        };
        if let Some(first) = shared.packet_outbox.pop_wait(wait) {
            forward_packet(shared, writer, &mut st, first)?;
            while let Some(next) = shared.packet_outbox.pop() {
                forward_packet(shared, writer, &mut st, next)?;
            }
        }
    }
}

fn reader_loop(
    mut stream: UnixStream,
    shared: Arc<EngineShared>,
    hub: Arc<StreamExportHub>,
    input_tx: calloop::channel::Sender<RudpInputEvent>,
    status: Arc<Mutex<crate::rudp_host::NetStatus>>,
    dead: Arc<AtomicBool>,
) {
    loop {
        match read_host_ipc_frame::<_, HostToComp>(&mut stream) {
            Ok(Some(msg)) => apply_host_msg(&shared, &hub, &input_tx, &status, msg),
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(%err, "rudp host: ipc read failed");
                break;
            }
        }
    }
    dead.store(true, Ordering::SeqCst);
}

fn apply_host_msg(
    shared: &EngineShared,
    hub: &StreamExportHub,
    input_tx: &calloop::channel::Sender<RudpInputEvent>,
    status: &NetStatusHandle,
    msg: HostToComp,
) {
    let authenticated = shared.active_sessions.load(Ordering::Relaxed) > 0;
    match msg {
        HostToComp::SessionDemand {
            active_sessions,
            session_joins,
        } => {
            shared
                .active_sessions
                .store(active_sessions as usize, Ordering::SeqCst);
            shared.session_joins.store(session_joins, Ordering::SeqCst);
        }
        HostToComp::SubscribeUnion { output_ids, gained } => {
            if !output_ids.is_empty() {
                shared.streams.retain(&output_ids);
            }
            // Empty union (no sessions / no layout yet) ⇒ hub exports primary only.
            hub.set_subscribed_outputs(output_ids);
            if gained {
                hub.request_full_frame();
            }
        }
        HostToComp::Input { event } => {
            // Input only means something while a client is authenticated.
            if authenticated {
                let _ = input_tx.send(event.into());
            }
        }
        HostToComp::UnlockPamSession { username } => {
            if authenticated && shared.unlock_session_on_auth.load(Ordering::Relaxed) {
                let _ = input_tx.send(RudpInputEvent::UnlockPamSession { username });
            } else {
                tracing::warn!("rudp host: ignoring unlock request (policy off / no session)");
            }
        }
        HostToComp::RefreshVideo => {
            let _ = input_tx.send(RudpInputEvent::RefreshVideo);
        }
        HostToComp::Status {
            listening,
            fingerprint,
            bind,
            error,
        } => record_net_status(status, listening, &fingerprint, &bind, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_rudp_host::engine::{NullLink, RudpLayoutShared};

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
    fn demand_and_unlock_policy_are_enforced_by_compositor() {
        let shared = shared();
        let hub = Arc::new(StreamExportHub::new());
        let status: NetStatusHandle = Arc::new(Mutex::new(Default::default()));
        let (input_tx, _input_rx) = calloop::channel::channel::<RudpInputEvent>();

        apply_host_msg(
            &shared,
            &hub,
            &input_tx,
            &status,
            HostToComp::SessionDemand {
                active_sessions: 2,
                session_joins: 5,
            },
        );
        assert_eq!(shared.active_sessions.load(Ordering::SeqCst), 2);
        assert_eq!(shared.session_joins.load(Ordering::SeqCst), 5);

        // Unlock is ignored while policy is off even with a live session.
        apply_host_msg(
            &shared,
            &hub,
            &input_tx,
            &status,
            HostToComp::UnlockPamSession {
                username: "alice".into(),
            },
        );
        assert!(!shared.unlock_session_on_auth.load(Ordering::Relaxed));
    }

    #[test]
    fn status_message_is_recorded() {
        let shared = shared();
        let hub = Arc::new(StreamExportHub::new());
        let status: NetStatusHandle = Arc::new(Mutex::new(Default::default()));
        let (input_tx, _input_rx) = calloop::channel::channel::<RudpInputEvent>();
        apply_host_msg(
            &shared,
            &hub,
            &input_tx,
            &status,
            HostToComp::Status {
                listening: true,
                fingerprint: "ab:cd".into(),
                bind: "0.0.0.0:7843".into(),
                error: None,
            },
        );
        let got = status.lock().map(|g| g.clone()).unwrap_or_default();
        assert!(got.listening);
        assert_eq!(got.fingerprint, "ab:cd");
    }

    #[test]
    fn config_message_carries_policy() {
        let bind: SocketAddr = "127.0.0.1:7843".parse().expect("addr");
        match config_msg(bind, vec!["alice".into()], true, false) {
            CompToHost::Config {
                bind,
                allowed_users,
                unlock_session_on_auth,
                ..
            } => {
                assert_eq!(bind, "127.0.0.1:7843");
                assert_eq!(allowed_users, vec!["alice".to_string()]);
                assert!(!unlock_session_on_auth);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
