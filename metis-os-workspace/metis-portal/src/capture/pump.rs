//! ScreenCast frame pump — Wayland capture → PipeWire.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use wayland_client::protocol::wl_shm::Format;

use metis_capture::CaptureOptions;
use metis_capture::dmabuf::format_is_bgr_order;

use crate::capture::session::CaptureSession;
use crate::pipewire::PipeWireHub;

/// Convert compositor SHM pixels to PipeWire BGRx (gnome-remote-desktop's preferred layout).
fn frame_to_bgrx(format: Format, data: &[u8], width: u32, height: u32, stride: u32) -> Vec<u8> {
    match format {
        Format::Abgr8888 | Format::Xbgr8888 => rgba_to_bgrx(data, width, height, stride),
        _ => bgra_to_bgrx(data, width, height, stride),
    }
}

/// Wayland ARGB8888 / XRGB8888 SHM is B,G,R,(A|X) in memory on little-endian.
fn bgra_to_bgrx(data: &[u8], width: u32, height: u32, stride: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let src_stride = stride as usize;
    let dst_stride = w * 4;
    let mut out = vec![0u8; dst_stride * h];
    for y in 0..h {
        let src_row = y * src_stride;
        let dst_row = y * dst_stride;
        for x in 0..w {
            let si = src_row + x * 4;
            let di = dst_row + x * 4;
            if si + 3 >= data.len() || di + 3 >= out.len() {
                continue;
            }
            out[di] = data[si];
            out[di + 1] = data[si + 1];
            out[di + 2] = data[si + 2];
            // PipeWire BGRx expects an opaque pixel; XRGB padding may be zero.
            out[di + 3] = 255;
        }
    }
    out
}

/// ABGR8888 / XBGR8888 SHM is R,G,B,(A|X) in memory on little-endian.
fn rgba_to_bgrx(data: &[u8], width: u32, height: u32, stride: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let src_stride = stride as usize;
    let dst_stride = w * 4;
    let mut out = vec![0u8; dst_stride * h];
    for y in 0..h {
        let src_row = y * src_stride;
        let dst_row = y * dst_stride;
        for x in 0..w {
            let si = src_row + x * 4;
            let di = dst_row + x * 4;
            if si + 3 >= data.len() || di + 3 >= out.len() {
                continue;
            }
            out[di] = data[si + 2];
            out[di + 1] = data[si + 1];
            out[di + 2] = data[si];
            out[di + 3] = 255;
        }
    }
    out
}

pub fn spawn_screencast_pump(
    pipewire: Arc<PipeWireHub>,
    node_id: u32,
    options: CaptureOptions,
    cancel: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("metis-screencast".into())
        .spawn(move || {
            let session = match CaptureSession::open(options) {
                Ok(s) => s,
                Err(err) => {
                    tracing::error!(%err, "screencast capture session failed");
                    pipewire.destroy_stream(node_id);
                    return;
                }
            };
            let mut session = session;
            let frame_interval = Duration::from_millis(33);
            let mut frames_sent = 0u64;
            while !cancel.load(Ordering::Relaxed) {
                let start = std::time::Instant::now();
                match session.capture_next_frame() {
                    Ok(frame) => {
                        let dmabuf_fourcc = frame.dmabuf.as_ref().map(|planes| planes.fourcc);
                        if frame.dmabuf.is_none() && frame.data.iter().all(|&b| b == 0) {
                            tracing::warn!("screencast frame all zeros — compositor may not be rendering");
                            if elapsed_under(start, frame_interval) {
                                thread::sleep(frame_interval - start.elapsed());
                            }
                            continue;
                        }

                        let mut sent = false;
                        if let Some(planes) = frame.dmabuf {
                            match pipewire.push_dmabuf_frame(node_id, planes) {
                                Ok(()) => sent = true,
                                Err(err) => {
                                    tracing::debug!(%err, "pipewire dmabuf push unavailable; using MemFd fallback");
                                }
                            }
                        }

                        if !sent && !frame.data.is_empty() {
                            let pixels = match dmabuf_fourcc {
                                Some(fourcc) if format_is_bgr_order(fourcc) => bgra_to_bgrx(
                                    &frame.data,
                                    frame.width,
                                    frame.height,
                                    frame.stride,
                                ),
                                Some(_) => rgba_to_bgrx(
                                    &frame.data,
                                    frame.width,
                                    frame.height,
                                    frame.stride,
                                ),
                                None => frame_to_bgrx(
                                    frame.shm_format,
                                    &frame.data,
                                    frame.width,
                                    frame.height,
                                    frame.stride,
                                ),
                            };
                            match pipewire.push_frame(node_id, pixels) {
                                Ok(()) => sent = true,
                                Err(err) => tracing::warn!(%err, "pipewire push_frame failed"),
                            }
                        }

                        if sent {
                            frames_sent += 1;
                            if frames_sent == 1 {
                                tracing::info!(
                                    node_id,
                                    width = frame.width,
                                    height = frame.height,
                                    "screencast first frame pushed to pipewire"
                                );
                            }
                        } else if frame.data.is_empty() {
                            tracing::warn!(
                                "pipewire rejected dmabuf and capture buffer was not mappable"
                            );
                        }
                    }
                    Err(err) => tracing::warn!(%err, "screencast frame capture failed"),
                }
                if elapsed_under(start, frame_interval) {
                    thread::sleep(frame_interval - start.elapsed());
                }
            }
            if frames_sent == 0 {
                tracing::warn!(node_id, "screencast pump ended without sending any frames");
            }
            pipewire.destroy_stream(node_id);
        })
        .expect("spawn screencast pump thread")
}

fn elapsed_under(start: std::time::Instant, interval: Duration) -> bool {
    start.elapsed() < interval
}
