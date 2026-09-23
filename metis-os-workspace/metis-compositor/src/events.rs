use std::io::{ErrorKind, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use metis_protocol::{CompositorEvent, EVENT_SUBSCRIBER_CAP, SlidingWindow};

/// Broadcast compositor events to subscribed shell clients (newline-delimited JSON).
#[derive(Clone, Default)]
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>,
}

impl EventBus {
    /// Subscribe if under [`EVENT_SUBSCRIBER_CAP`]. Returns `false` when full.
    pub fn try_subscribe(&self, stream: std::os::unix::net::UnixStream) -> bool {
        // Non-blocking: a stalled shell/portal reader must never freeze the
        // compositor (ClipboardChanged after screenshots previously could).
        let _ = stream.set_nonblocking(true);
        let Ok(mut subs) = self.subscribers.lock() else {
            return false;
        };
        if subs.len() >= EVENT_SUBSCRIBER_CAP {
            return false;
        }
        subs.push(stream);
        true
    }

    pub fn emit(&self, event: &CompositorEvent) {
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        let mut payload = line;
        payload.push('\n');
        let bytes = payload.as_bytes();

        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain_mut(|stream| write_event_nonblocking(stream, bytes));
        }
    }

    pub fn prune_dead(&self) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|stream| stream.peer_addr().is_ok());
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().map(|s| s.len()).unwrap_or(0)
    }
}

/// Best-effort non-blocking write. Drop the subscriber on hard errors; keep it
/// on WouldBlock so a briefly busy reader is not permanently removed.
fn write_event_nonblocking(stream: &mut std::os::unix::net::UnixStream, bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset < bytes.len() {
        match stream.write(&bytes[offset..]) {
            Ok(0) => return false,
            Ok(n) => offset += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                tracing::debug!(
                    "event subscriber temporarily busy; dropping event for that client"
                );
                return true;
            }
            Err(_) => return false,
        }
    }
    true
}

pub fn init_events_listener(
    bus: &EventBus,
) -> Result<std::os::unix::net::UnixListener, std::io::Error> {
    let _ = metis_protocol::ensure_runtime_dir()?;
    let path = metis_protocol::events_socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    metis_protocol::set_mode(&path, 0o600)?;
    listener.set_nonblocking(true)?;
    tracing::info!(path = ?path, "compositor event socket ready");
    let _ = bus;
    Ok(listener)
}

pub fn accept_event_subscribers(
    listener: &std::os::unix::net::UnixListener,
    bus: &EventBus,
    subscribe_limit: &mut SlidingWindow,
) {
    let now = Instant::now();
    loop {
        match metis_protocol::accept_same_euid(listener) {
            Ok(metis_protocol::AcceptPeer::Ready(stream)) => {
                if bus.subscriber_count() >= EVENT_SUBSCRIBER_CAP {
                    tracing::warn!(
                        cap = EVENT_SUBSCRIBER_CAP,
                        "events: subscriber cap reached; dropping connection"
                    );
                    drop(stream);
                    continue;
                }
                if !subscribe_limit.try_admit(now) {
                    tracing::warn!("events: subscribe rate limited");
                    drop(stream);
                    continue;
                }
                if bus.try_subscribe(stream) {
                    tracing::info!("shell subscribed to compositor events");
                } else {
                    tracing::warn!(
                        cap = EVENT_SUBSCRIBER_CAP,
                        "events: subscriber cap reached; dropping connection"
                    );
                }
            }
            Ok(metis_protocol::AcceptPeer::Rejected) => {
                tracing::warn!("events: rejected subscriber from foreign UID (SO_PEERCRED)");
            }
            Ok(metis_protocol::AcceptPeer::WouldBlock) => break,
            Err(e) => {
                tracing::warn!("event subscriber accept error: {e}");
                break;
            }
        }
    }
}
