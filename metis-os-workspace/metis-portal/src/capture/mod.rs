//! Screen capture for the Metis portal backend.

mod pump;
mod session;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ashpd::PortalError;
use metis_capture::{CaptureOptions, capture_output_frame, capture_png, frame_to_rgba, write_png};

use crate::pipewire::PipeWireHub;

pub use metis_capture::Frame;
pub use pump::spawn_screencast_pump;

#[derive(Debug, Clone)]
pub struct CapturedPng {
    pub path: PathBuf,
}

pub struct CaptureHub {
    _pipewire: Arc<PipeWireHub>,
}

impl CaptureHub {
    pub fn new(pipewire: Arc<PipeWireHub>) -> Self {
        Self {
            _pipewire: pipewire,
        }
    }

    pub async fn screenshot_png(&self) -> Result<PathBuf, PortalError> {
        capture_fullscreen_png()
            .await
            .map(|c| c.path)
            .map_err(PortalError::Failed)
    }

    pub async fn output_size(&self, connector: Option<&str>) -> (u32, u32) {
        let connector = connector.map(str::to_string);
        match tokio::task::spawn_blocking(move || {
            capture_output_frame(CaptureOptions {
                draw_cursor: true,
                connector,
                ..Default::default()
            })
        })
        .await
        {
            Ok(Ok(frame)) => (frame.width, frame.height),
            _ => (1920, 1080),
        }
    }
}

pub async fn capture_fullscreen_png() -> Result<CapturedPng, String> {
    let path = screenshot_path();
    capture_png(
        CaptureOptions {
            draw_cursor: true,
            ..Default::default()
        },
        None,
        &path,
    )?;
    Ok(CapturedPng { path })
}

#[allow(dead_code)]
pub fn save_frame_png(frame: &Frame, path: &Path) -> Result<(), String> {
    let rgba = frame_to_rgba(frame);
    write_png(path, frame.width, frame.height, &rgba)
}

fn screenshot_path() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Path::new(&base).join(format!("metis-screenshot-{millis}.png"))
}
