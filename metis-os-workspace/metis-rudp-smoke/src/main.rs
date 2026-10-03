//! Metis Remote Phase 4–8 smoke client (thin CLI over `metis-rudp-client`).
//!
//! Usage:
//!   metis-rudp-smoke <host:port> --user <name>
//!   metis-rudp-smoke <host:port> --user <name> --video-secs 5
//!   metis-rudp-smoke <host:port> --user <name> --input-smoke
//!   Password on stdin (or METIS_RUDP_PASSWORD).
//!   --tofu-reset clears the known_hosts pin for this host.

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use metis_protocol::RudpControlMsg;
use metis_rudp_client::{
    ClientError, RudpClientConfig, SessionEvent, TofuMode, clear_host_pin, connect,
};
use zeroize::Zeroize;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_rudp_smoke=info,warn".into()),
        )
        .init();

    let code = match run(std::env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("metis-rudp-smoke: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut host_port = None;
    let mut user = None;
    let mut tofu_reset = false;
    let mut video_secs: Option<u64> = None;
    let mut input_smoke = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--user" | "-u" => {
                i += 1;
                user = args.get(i).cloned();
            }
            "--video-secs" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or_else(|| "--video-secs needs a number".to_string())?;
                let n: u64 = raw
                    .parse()
                    .map_err(|_| format!("invalid --video-secs '{raw}'"))?;
                video_secs = Some(n.max(1));
            }
            "--input-smoke" => input_smoke = true,
            "--tofu-reset" => tofu_reset = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other if !other.starts_with('-') && host_port.is_none() => {
                host_port = Some(other.to_string());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }

    let host_port = host_port.ok_or_else(|| {
        "usage: metis-rudp-smoke <host:port> --user <name> [--video-secs N] [--input-smoke] [--tofu-reset]"
            .to_string()
    })?;
    let user = user.ok_or_else(|| "--user <name> required".to_string())?;

    if tofu_reset {
        let removed = clear_host_pin(&host_port).map_err(|e| e.to_string())?;
        eprintln!(
            "TOFU: {}",
            if removed {
                format!("cleared pin for {host_port}")
            } else {
                "no pin to clear".into()
            }
        );
    }

    let addr: SocketAddr = metis_rudp_client::resolve_endpoint(&host_port)
        .map_err(|e| format!("invalid host:port '{host_port}': {e}"))?;

    let mut password = if let Ok(p) = std::env::var("METIS_RUDP_PASSWORD") {
        p
    } else {
        read_password_prompt().map_err(|e| format!("read password: {e}"))?
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio: {e}"))?;

    let result = rt.block_on(async {
        smoke_session(addr, &host_port, &user, &password, video_secs, input_smoke).await
    });
    password.zeroize();
    result.map_err(|e| e.to_string())
}

async fn smoke_session(
    addr: SocketAddr,
    host_key: &str,
    user: &str,
    password: &str,
    video_secs: Option<u64>,
    input_smoke: bool,
) -> Result<(), ClientError> {
    let mut session = connect(RudpClientConfig {
        addr,
        host_key: host_key.to_string(),
        username: user.to_string(),
        password: password.to_string(),
        tofu: TofuMode::Auto,
    })
    .await?;
    println!("SessionOk {}", session.session_id());

    // Drain any immediate PointerLock.
    while let Ok(ev) = tokio::time::timeout(Duration::from_millis(80), session.recv_event()).await {
        match ev {
            Some(SessionEvent::PointerLock { locked }) => {
                println!("PointerLock locked={locked}");
            }
            Some(SessionEvent::Disconnected) | None => break,
            Some(_) => break,
        }
    }

    if input_smoke {
        const BTN_LEFT: u32 = 0x110;
        const KEY_A: u32 = 30;
        session
            .send_input(RudpControlMsg::PointerAbsolute { x: 100.0, y: 100.0 })
            .await?;
        session
            .send_input(RudpControlMsg::PointerButton {
                button: BTN_LEFT,
                pressed: true,
            })
            .await?;
        session
            .send_input(RudpControlMsg::PointerButton {
                button: BTN_LEFT,
                pressed: false,
            })
            .await?;
        session
            .send_input(RudpControlMsg::Key {
                keycode: KEY_A,
                pressed: true,
            })
            .await?;
        session
            .send_input(RudpControlMsg::Key {
                keycode: KEY_A,
                pressed: false,
            })
            .await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        println!("input-smoke: sent absolute + button + key");
    }

    let Some(secs) = video_secs else {
        return Ok(());
    };

    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut video_ready = false;
    let mut aus = 0u64;
    let mut bytes = 0u64;
    let mut keyframes = 0u64;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(200)),
            session.recv_event(),
        )
        .await
        {
            Ok(Some(SessionEvent::VideoReady {
                width,
                height,
                codec,
            })) => {
                println!("VideoReady {width}x{height} codec={codec}");
                video_ready = true;
            }
            Ok(Some(SessionEvent::PointerLock { locked })) => {
                println!("PointerLock locked={locked}");
            }
            Ok(Some(SessionEvent::AccessUnit(au))) => {
                aus += 1;
                bytes += au.data.len() as u64;
                if au.keyframe {
                    keyframes += 1;
                }
            }
            Ok(Some(SessionEvent::Disconnected)) | Ok(None) => break,
            Err(_) => {}
        }
    }

    println!("video: aus={aus} keyframes={keyframes} bytes={bytes} video_ready={video_ready}");
    if aus == 0 {
        return Err(ClientError::msg(
            "no video access units received (is Metis Remote enabled with an active DRM session?)",
        ));
    }
    Ok(())
}

/// Prompt on stderr, read one line from stdin with echo disabled (TTY only).
/// Uses `read_line` (not `read_to_string`) so Enter finishes the prompt —
/// waiting for EOF made the CLI look hung after password entry.
fn read_password_prompt() -> std::io::Result<String> {
    eprint!("Password: ");
    let _ = std::io::stderr().flush();

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let saved = save_and_disable_tty_echo(fd);
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(term) = saved {
        // SAFETY: restore the exact termios we captured before disabling echo.
        unsafe {
            let _ = libc::tcsetattr(fd, libc::TCSANOW, &term);
        }
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn save_and_disable_tty_echo(fd: i32) -> Option<libc::termios> {
    // SAFETY: termios on a valid fd; failure → leave echo alone.
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut term) != 0 {
            return None;
        }
        let saved = term;
        term.c_lflag &= !libc::ECHO;
        if libc::tcsetattr(fd, libc::TCSANOW, &term) != 0 {
            return None;
        }
        Some(saved)
    }
}

fn print_help() {
    println!(
        "metis-rudp-smoke — Metis Remote auth + video + input smoke test\n\n\
         Usage:\n  \
         metis-rudp-smoke <host:port> --user <name> [--video-secs N] [--input-smoke] [--tofu-reset]\n\n\
         Password: TTY prompt (no echo) or METIS_RUDP_PASSWORD.\n\
         --video-secs N  receive/reassemble video for N seconds (requires host encode).\n\
         --input-smoke   send a short pointer/keyboard sequence after SessionOk.\n\
         TOFU pins live in ~/.config/metis/rudp/known_hosts."
    );
}
