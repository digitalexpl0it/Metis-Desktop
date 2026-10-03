//! Receive FreeRDP input events (JSON datagrams) and inject into the compositor.

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use metis_protocol::{CompositorCommand, send_compositor_command};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputMsg {
    Abs { x: u32, y: u32 },
    Rel { dx: i32, dy: i32 },
    Button { button: u32, pressed: bool },
    Scroll { dx: i32, dy: i32 },
    Key { keycode: u32, pressed: bool },
}

pub struct InputBridge {
    path: PathBuf,
    join: Option<JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl InputBridge {
    pub fn start(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(path);
        let sock = UnixDatagram::bind(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        sock.set_nonblocking(false)?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_t = std::sync::Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("metis-rdp-input".into())
            .spawn(move || input_loop(sock, stop_t))?;
        Ok(Self {
            path: path.to_path_buf(),
            join: Some(join),
            stop,
        })
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InputBridge {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Unblock recv with a dummy datagram.
        if let Ok(s) = UnixDatagram::unbound() {
            let _ = s.send_to(b"{}", &self.path);
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn input_loop(sock: UnixDatagram, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let mut buf = [0u8; 512];
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let n = match sock.recv(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            continue;
        }
        let Ok(msg) = serde_json::from_slice::<InputMsg>(&buf[..n]) else {
            continue;
        };
        let cmd = match msg {
            InputMsg::Abs { x, y } => CompositorCommand::InjectRemotePointerAbsolute {
                x: f64::from(x),
                y: f64::from(y),
            },
            InputMsg::Rel { dx, dy } => CompositorCommand::InjectRemotePointerRelative {
                dx: f64::from(dx),
                dy: f64::from(dy),
            },
            InputMsg::Button { button, pressed } => {
                CompositorCommand::InjectRemotePointerButton { button, pressed }
            }
            InputMsg::Scroll { dx, dy } => CompositorCommand::InjectRemotePointerScroll {
                dx: f64::from(dx),
                dy: f64::from(dy),
            },
            InputMsg::Key { keycode, pressed } => {
                CompositorCommand::InjectRemoteKey { keycode, pressed }
            }
        };
        if let Err(err) = send_compositor_command(&cmd) {
            tracing::debug!(%err, "rdp host: inject failed");
        }
    }
}
