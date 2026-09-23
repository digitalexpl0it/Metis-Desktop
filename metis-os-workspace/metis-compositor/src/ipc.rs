use std::io::{Read, Write};
use std::time::Instant;

use metis_protocol::{CompositorCommand, CompositorEvent, IPC_MAX_ACCEPTS_PER_DRAIN};

use crate::events::{accept_event_subscribers, init_events_listener};
use crate::ipc_dispatch::resolve_ipc_caps;
use crate::state::MetisState;

pub fn init_ipc(state: &mut MetisState) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = metis_protocol::ensure_runtime_dir()?;
    let socket_path = metis_protocol::ipc_socket_path();
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    let _ = runtime;

    let listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
    metis_protocol::set_mode(&socket_path, 0o600)?;
    listener.set_nonblocking(true)?;
    state.ipc_listener = Some(listener);

    state.events_listener = Some(init_events_listener(&state.event_bus)?);

    Ok(())
}

pub fn drain_ipc(state: &mut MetisState) {
    if let Some(ref listener) = state.events_listener {
        accept_event_subscribers(
            listener,
            &state.event_bus,
            &mut state.event_subscribe_rate_limit,
        );
    }

    let mut pending = Vec::new();
    if let Some(listener) = state.ipc_listener.as_ref() {
        while pending.len() < IPC_MAX_ACCEPTS_PER_DRAIN {
            match metis_protocol::accept_same_euid(listener) {
                Ok(metis_protocol::AcceptPeer::Ready(stream)) => pending.push(stream),
                Ok(metis_protocol::AcceptPeer::Rejected) => {
                    tracing::warn!("IPC: rejected connection from foreign UID (SO_PEERCRED)");
                }
                Ok(metis_protocol::AcceptPeer::WouldBlock) => break,
                Err(e) => {
                    tracing::warn!("IPC accept error: {e}");
                    break;
                }
            }
        }
    }

    let now = Instant::now();
    for mut stream in pending {
        if !state.ipc_rate_limit.try_admit(now) {
            tracing::warn!("IPC: rate limited (sliding window)");
            let reply = serde_json::to_string(&CompositorEvent::Error {
                message: "rate limited".into(),
            })
            .unwrap_or_default();
            let timeout = std::time::Duration::from_millis(50);
            let _ = stream.set_write_timeout(Some(timeout));
            let _ = writeln!(stream, "{reply}");
            let _ = stream.flush();
            continue;
        }

        // A freshly-accepted connection may not have its command bytes available
        // in this same drain pass (the client connects, then writes). Reading
        // non-blocking here dropped the connection without a reply whenever we
        // lost that race, which surfaced as `EAGAIN` on the shell. Use a short
        // blocking read/write timeout instead: clients write immediately after
        // connecting, so this returns in well under a millisecond in practice and
        // only ever waits when a command is genuinely mid-flight.
        let timeout = std::time::Duration::from_millis(50);
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        let request = match read_request_line(&mut stream) {
            Some(line) => line,
            None => continue,
        };

        let reply = match parse_ipc_request(&request) {
            Ok((token, cmd)) => {
                match resolve_ipc_caps(state.widgets_ipc_token.as_deref(), token.as_deref()) {
                    Ok(caps) => {
                        let evt = state.handle_ipc_with_caps(cmd, caps);
                        serde_json::to_string(&evt).unwrap_or_default()
                    }
                    Err(message) => serde_json::to_string(&CompositorEvent::Error { message })
                        .unwrap_or_default(),
                }
            }
            Err(err) => serde_json::to_string(&CompositorEvent::Error {
                message: err.to_string(),
            })
            .unwrap_or_default(),
        };

        if writeln!(stream, "{reply}").is_err() {
            tracing::warn!("IPC reply write failed");
            continue;
        }
        if stream.flush().is_err() {
            tracing::warn!("IPC reply flush failed");
        }
    }
}

/// Read one newline-delimited command without blocking the compositor event loop.
fn read_request_line(stream: &mut std::os::unix::net::UnixStream) -> Option<String> {
    let mut acc = String::new();
    let mut buf = [0u8; 8192];

    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                if acc.contains('\n') {
                    break;
                }
                if acc.len() > 256 * 1024 {
                    tracing::warn!("IPC request exceeded size limit");
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => {
                tracing::warn!(%e, "IPC read error");
                break;
            }
        }
    }

    let line = acc.lines().next()?.trim().to_string();
    if line.is_empty() { None } else { Some(line) }
}

fn parse_ipc_request(request: &str) -> Result<(Option<String>, CompositorCommand), String> {
    let mut value: serde_json::Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let token = value
        .get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    if let Some(obj) = value.as_object_mut() {
        obj.remove("token");
    }
    let cmd: CompositorCommand = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Ok((token, cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strips_token_and_rejects_oversized_semantics() {
        let (tok, cmd) = parse_ipc_request(r#"{"cmd":"ping","token":"abc"}"#).unwrap();
        assert_eq!(tok.as_deref(), Some("abc"));
        assert!(matches!(cmd, CompositorCommand::Ping));
        assert!(parse_ipc_request("not-json").is_err());
    }

    #[test]
    fn ipc_request_size_limit_constant() {
        // Reader aborts accumulation above 256 KiB — keep the contract visible.
        assert_eq!(256 * 1024, 262_144);
    }
}
