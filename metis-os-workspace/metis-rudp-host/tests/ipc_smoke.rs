//! Spawn the real `metis-rudp-host` binary against a private Unix socket and
//! drive the compositor side of the IPC contract: Config → Status(listening) →
//! media / layout accepted → Shutdown → clean exit.

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use metis_protocol::{
    CompToHost, HostToComp, IpcStreamInfo, RUDP_HOST_IPC_VERSION, RudpMonitorInfo,
    read_host_ipc_frame, write_host_ipc_frame,
};
use metis_rudp_host::ipc_sock;

fn scratch_dir(tag: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("metis-rudp-host-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn host_connects_reports_status_and_exits_on_shutdown() {
    let scratch = scratch_dir("smoke");
    // Short path: AF_UNIX paths are limited to ~108 bytes.
    let sock_dir = std::env::temp_dir().join(format!("mrh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).expect("sock dir");
    let sock = sock_dir.join("h.sock");
    let listener = UnixListener::bind(&sock).expect("bind");

    let mut child = Command::new(env!("CARGO_BIN_EXE_metis-rudp-host"))
        .arg("--socket")
        .arg(&sock)
        // Keep the generated TLS identity out of the real ~/.config/metis.
        .env("HOME", &scratch)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn host");

    let (mut stream, _) = listener.accept().expect("host connects");
    ipc_sock::verify_peer(&stream).expect("peer uid");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");

    write_host_ipc_frame(
        &mut stream,
        &CompToHost::Config {
            ipc_version: RUDP_HOST_IPC_VERSION,
            bind: "127.0.0.1:0".into(),
            allowed_users: vec!["nobody-test".into()],
            lan_only: true,
            unlock_session_on_auth: false,
            stream_quality: "medium".into(),
            allow_mic_uplink: false,
        },
    )
    .expect("send config");
    write_host_ipc_frame(
        &mut stream,
        &CompToHost::DesktopLayout {
            generation: 1,
            monitors: vec![RudpMonitorInfo {
                id: "DP-1".into(),
                name: "DP-1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
                primary: true,
            }],
        },
    )
    .expect("send layout");
    write_host_ipc_frame(
        &mut stream,
        &CompToHost::StreamAnnounce {
            streams: vec![IpcStreamInfo {
                stream_id: 0,
                output_id: "DP-1".into(),
                width: 1920,
                height: 1080,
                codec: "h264".into(),
            }],
        },
    )
    .expect("send streams");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut listening = false;
    while Instant::now() < deadline {
        match read_host_ipc_frame::<_, HostToComp>(&mut stream) {
            Ok(Some(HostToComp::Status {
                listening: true,
                fingerprint,
                error: None,
                ..
            })) => {
                assert!(!fingerprint.is_empty(), "host reports its TLS fingerprint");
                listening = true;
                break;
            }
            Ok(Some(HostToComp::Status { error: Some(e), .. })) => {
                panic!("host failed to listen: {e}")
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    assert!(listening, "host never reported a listening Status");

    write_host_ipc_frame(&mut stream, &CompToHost::Shutdown).expect("shutdown");
    let exit_deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() > exit_deadline {
            let _ = child.kill();
            panic!("host did not exit after Shutdown");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "host exit status: {status}");
    let _ = std::fs::remove_dir_all(&sock_dir);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn host_exits_when_compositor_socket_closes() {
    let scratch = scratch_dir("eof");
    let sock_dir = std::env::temp_dir().join(format!("mrh-eof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).expect("sock dir");
    let sock = sock_dir.join("h.sock");
    let listener = UnixListener::bind(&sock).expect("bind");

    let mut child = Command::new(env!("CARGO_BIN_EXE_metis-rudp-host"))
        .arg("--socket")
        .arg(&sock)
        .env("HOME", &scratch)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn host");
    let (mut stream, _) = listener.accept().expect("host connects");
    write_host_ipc_frame(
        &mut stream,
        &CompToHost::Config {
            ipc_version: RUDP_HOST_IPC_VERSION,
            bind: "127.0.0.1:0".into(),
            allowed_users: vec!["nobody-test".into()],
            lan_only: true,
            unlock_session_on_auth: false,
            stream_quality: "medium".into(),
            allow_mic_uplink: false,
        },
    )
    .expect("send config");
    // Compositor "crashes": drop the socket without Shutdown.
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("orphaned host kept running after compositor socket closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&sock_dir);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn host_refuses_empty_allowlist() {
    let scratch = scratch_dir("empty");
    let sock_dir = std::env::temp_dir().join(format!("mrh-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).expect("sock dir");
    let sock = sock_dir.join("h.sock");
    let listener = UnixListener::bind(&sock).expect("bind");
    let mut child = Command::new(env!("CARGO_BIN_EXE_metis-rudp-host"))
        .arg("--socket")
        .arg(&sock)
        .env("HOME", &scratch)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn host");
    let (mut stream, _) = listener.accept().expect("host connects");
    write_host_ipc_frame(
        &mut stream,
        &CompToHost::Config {
            ipc_version: RUDP_HOST_IPC_VERSION,
            bind: "127.0.0.1:0".into(),
            allowed_users: Vec::new(),
            lan_only: true,
            unlock_session_on_auth: false,
            stream_quality: "medium".into(),
            allow_mic_uplink: false,
        },
    )
    .expect("send config");
    let status = child.wait().expect("wait");
    assert!(!status.success(), "empty allowlist must be refused");
    let _ = std::fs::remove_dir_all(&sock_dir);
    let _ = std::fs::remove_dir_all(&scratch);
}
