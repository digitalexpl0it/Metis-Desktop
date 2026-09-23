//! Compositor event subscription for portal-side clipboard sync.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use metis_protocol::CompositorEvent;

use crate::mutter::clipboard::ClipboardSession;

pub fn spawn_clipboard_listener(sessions: Arc<Mutex<Vec<ClipboardSession>>>) {
    thread::Builder::new()
        .name("metis-portal-events".into())
        .spawn(move || listen_loop(sessions))
        .ok();
}

fn listen_loop(sessions: Arc<Mutex<Vec<ClipboardSession>>>) {
    loop {
        match connect_events_socket() {
            Ok(stream) => {
                tracing::info!("portal subscribed to compositor events");
                read_events(stream, &sessions);
            }
            Err(err) => {
                tracing::debug!(%err, "compositor event socket connect failed, retrying");
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn connect_events_socket() -> std::io::Result<UnixStream> {
    let path = metis_protocol::events_socket_path();
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(None)?;
    Ok(stream)
}

fn read_events(stream: UnixStream, sessions: &Arc<Mutex<Vec<ClipboardSession>>>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else {
            tracing::warn!("compositor event stream disconnected");
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(evt) = serde_json::from_str::<CompositorEvent>(&line) else {
            continue;
        };
        if let CompositorEvent::ClipboardChanged {
            mime,
            preview_text,
            image_path,
        } = evt
            && let Ok(list) = sessions.lock()
        {
            for session in list.iter() {
                session.on_local_clipboard_changed(
                    &mime,
                    preview_text.as_deref(),
                    image_path.as_deref(),
                );
            }
        }
    }
}
