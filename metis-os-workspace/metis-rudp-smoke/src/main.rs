//! Metis Remote Phase 4–8 smoke client (thin CLI over `metis-rudp-client`).
//!
//! Usage:
//!   metis-rudp-smoke <host:port> --user <name>
//!   metis-rudp-smoke <host:port> --user <name> --video-secs 5
//!   metis-rudp-smoke <host:port> --user <name> --input-smoke
//!   metis-rudp-smoke <host:port> --user <name> --monitor DP-2 --video-secs 5
//!   Password on stdin (or METIS_RUDP_PASSWORD).
//!   --tofu-reset clears the known_hosts pin for this host.

use std::collections::BTreeMap;
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
    let mut monitors: Vec<String> = Vec::new();
    let mut stream_quality: Option<String> = None;
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
            "--monitor" | "-m" => {
                i += 1;
                let name = args
                    .get(i)
                    .ok_or_else(|| "--monitor needs an output name".to_string())?;
                if !monitors.contains(name) {
                    monitors.push(name.clone());
                }
            }
            "--stream-quality" => {
                i += 1;
                let q = args
                    .get(i)
                    .ok_or_else(|| {
                        "--stream-quality needs performance|high|medium|low".to_string()
                    })?
                    .to_ascii_lowercase();
                if !matches!(q.as_str(), "performance" | "high" | "medium" | "low") {
                    return Err(format!("invalid --stream-quality '{q}'"));
                }
                stream_quality.replace(q);
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
        "usage: metis-rudp-smoke <host:port> --user <name> [--video-secs N] [--monitor NAME].. [--input-smoke] [--tofu-reset]"
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
        smoke_session(
            addr,
            &host_port,
            &user,
            &password,
            video_secs,
            input_smoke,
            &monitors,
            stream_quality.as_deref(),
        )
        .await
    });
    password.zeroize();
    result.map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn smoke_session(
    addr: SocketAddr,
    host_key: &str,
    user: &str,
    password: &str,
    video_secs: Option<u64>,
    input_smoke: bool,
    monitors: &[String],
    stream_quality: Option<&str>,
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
    if let Some(q) = stream_quality {
        session.set_stream_quality(q).await?;
        println!("SetStreamQuality {q}");
    }

    // Drain the post-auth burst: PointerLock + DesktopLayout (host sends both
    // immediately after SessionOk).
    let mut layout_seen = false;
    let mut video_ready = false;
    let mut streams: BTreeMap<u8, String> = BTreeMap::new();
    while let Ok(ev) = tokio::time::timeout(Duration::from_millis(250), session.recv_event()).await
    {
        match ev {
            Some(SessionEvent::PointerLock { locked }) => {
                println!("PointerLock locked={locked}");
            }
            Some(SessionEvent::StreamQuality { quality }) => {
                println!("StreamQuality {quality}");
            }
            Some(SessionEvent::DesktopLayout { monitors: layout }) => {
                layout_seen = true;
                print_layout(&layout);
                if !monitors.is_empty() {
                    if let Some(missing) = monitors
                        .iter()
                        .find(|want| !layout.iter().any(|m| &m.id == *want))
                    {
                        return Err(ClientError::msg(format!(
                            "--monitor {missing}: not in host desktop layout"
                        )));
                    }
                    session.subscribe_outputs(monitors.to_vec()).await?;
                    println!("SubscribeOutputs {}", monitors.join(","));
                }
            }
            Some(SessionEvent::VideoReady {
                width,
                height,
                codec,
                stream_id,
                output_id,
            }) => {
                println!(
                    "VideoReady stream={stream_id} output={output_id} {width}x{height} codec={codec}"
                );
                streams.insert(stream_id, output_id);
                video_ready = true;
            }
            Some(SessionEvent::Disconnected) | None => break,
            Some(_) => break,
        }
    }
    if !layout_seen && !monitors.is_empty() {
        return Err(ClientError::msg(
            "--monitor given but host sent no DesktopLayout (protocol < 3?)",
        ));
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
    let mut aus = 0u64;
    let mut bytes = 0u64;
    let mut keyframes = 0u64;
    let mut per_stream: BTreeMap<u8, u64> = BTreeMap::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(200)),
            session.recv_event(),
        )
        .await
        {
            Ok(Some(SessionEvent::DesktopLayout { monitors: layout })) => {
                print_layout(&layout);
            }
            Ok(Some(SessionEvent::VideoReady {
                width,
                height,
                codec,
                stream_id,
                output_id,
            })) => {
                println!(
                    "VideoReady stream={stream_id} output={output_id} {width}x{height} codec={codec}"
                );
                streams.insert(stream_id, output_id);
                video_ready = true;
            }
            Ok(Some(SessionEvent::PointerLock { locked })) => {
                println!("PointerLock locked={locked}");
            }
            Ok(Some(SessionEvent::AccessUnit(au))) => {
                aus += 1;
                bytes += au.data.len() as u64;
                *per_stream.entry(au.stream_id).or_default() += 1;
                if au.keyframe {
                    keyframes += 1;
                }
            }
            Ok(Some(
                SessionEvent::AudioReady { .. }
                | SessionEvent::AudioPacket { .. }
                | SessionEvent::ClipboardSet { .. }
                | SessionEvent::StreamQuality { .. },
            )) => {}
            Ok(Some(SessionEvent::Disconnected)) | Ok(None) => break,
            Err(_) => {}
        }
    }

    println!("video: aus={aus} keyframes={keyframes} bytes={bytes} video_ready={video_ready}");
    for (stream_id, n) in &per_stream {
        let output = streams.get(stream_id).map_or("?", String::as_str);
        println!("  stream {stream_id} ({output}): aus={n}");
    }
    if aus == 0 {
        return Err(ClientError::msg(
            "no video access units received (is Metis Remote enabled with an active DRM session?)",
        ));
    }
    Ok(())
}

fn print_layout(monitors: &[metis_protocol::RudpMonitorInfo]) {
    for m in monitors {
        println!(
            "Monitor {}{} {}x{}+{}+{} scale={}",
            m.id,
            if m.primary { " (primary)" } else { "" },
            m.width,
            m.height,
            m.x,
            m.y,
            m.scale
        );
    }
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
         metis-rudp-smoke <host:port> --user <name> [--video-secs N] [--monitor NAME].. [--stream-quality Q] [--input-smoke] [--tofu-reset]\n\n\
         Password: TTY prompt (no echo) or METIS_RUDP_PASSWORD.\n\
         --video-secs N  receive/reassemble video for N seconds (requires host encode).\n\
         --monitor NAME  subscribe to this output (repeatable, max 4; default: primary).\n\
         --stream-quality Q  performance|high|medium|low (protocol v4).\n\
         --input-smoke   send a short pointer/keyboard sequence after SessionOk.\n\
         TOFU pins live in ~/.config/metis/rudp/known_hosts."
    );
}
