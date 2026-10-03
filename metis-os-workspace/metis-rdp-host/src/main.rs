//! Metis-native RDP host: portal ScreenCast → PipeWire → FreeRDP shadow.
//!
//! Spawned by `metis-remote native enable`. Captures the Wayland session via
//! xdg-desktop-portal ScreenCast (metis-portal) and serves RDP on TCP 3389 so
//! Metis Viewer / FreeRDP clients can connect. GRD remains the product default.

mod capture;
mod framebuf;
mod input;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::framebuf::FrameBuffer;
use crate::input::InputBridge;

const DEFAULT_PORT: u16 = 3389;

#[link(name = "metis_shadow", kind = "static")]
unsafe extern "C" {
    fn metis_rdp_server_run(
        shm_path: *const libc::c_char,
        input_path: *const libc::c_char,
        port: u16,
        password: *const libc::c_char,
    ) -> libc::c_int;
    fn metis_rdp_server_stop();
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("metis")
}

fn password_from_env_or_file() -> String {
    if let Ok(p) = std::env::var("METIS_RDP_PASSWORD") {
        return p;
    }
    let path = runtime_dir().join("rdp-password");
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    // Trusted host path — skip portal consent when Settings enabled sharing.
    // metis-portal also auto-approves when remote.json.enabled for this app.
    if std::env::var_os("METIS_PORTAL_AUTO_APPROVE").is_none() {
        // SAFETY: single-threaded set before any other threads spawn.
        unsafe {
            std::env::set_var("METIS_PORTAL_AUTO_APPROVE", "1");
        }
    }

    let port = std::env::var("METIS_RDP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir).context("create runtime dir")?;
    let shm_path = dir.join("rdp-frames.shm");
    let input_path = dir.join("rdp-input.sock");

    let frames = Arc::new(Mutex::new(
        FrameBuffer::create(&shm_path, 1920, 1080).context("create frame SHM")?,
    ));
    let input = InputBridge::start(&input_path).context("input bridge")?;
    let _capture = capture::CaptureSession::start(Arc::clone(&frames))
        .await
        .context("start ScreenCast capture")?;

    let password = password_from_env_or_file();
    let shm_c = std::ffi::CString::new(shm_path.to_string_lossy().as_bytes())?;
    let input_c = std::ffi::CString::new(input_path.to_string_lossy().as_bytes())?;
    let pass_c = std::ffi::CString::new(password.as_str())?;

    tracing::info!(
        %port,
        shm = %shm_path.display(),
        "metis-rdp-host: starting FreeRDP shadow (portal capture)"
    );

    let server = std::thread::Builder::new()
        .name("metis-rdp-shadow".into())
        .spawn(move || unsafe {
            metis_rdp_server_run(shm_c.as_ptr(), input_c.as_ptr(), port, pass_c.as_ptr())
        })?;

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("metis-rdp-host: shutting down");
    unsafe {
        metis_rdp_server_stop();
    }
    let _ = server.join();
    drop(input);
    Ok(())
}
