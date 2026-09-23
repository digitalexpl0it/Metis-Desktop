use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_capture::{CaptureOptions, capture_output_frame, crop_rgba, frame_to_rgba};
use metis_grid::PixelRect;

use crate::{Cli, theme};

pub fn show(app: &gtk::Application, cli: Cli) {
    theme::install();
    let window = gtk::Window::builder()
        .application(app)
        .title("Recording")
        .build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_namespace(Some("metis-screenshot-record"));
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_margin(Edge::Bottom, 28);

    let pill = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    pill.add_css_class("metis-screenshot-pill");
    pill.set_halign(gtk::Align::Center);
    let state_label = gtk::Label::new(Some("● Recording"));
    pill.append(&state_label);
    let stop = gtk::Button::with_label("Stop");
    stop.add_css_class("suggested-action");
    pill.append(&stop);
    window.set_child(Some(&pill));
    window.present();

    let stopped = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = std::sync::mpsc::channel();
    {
        let stopped = stopped.clone();
        std::thread::spawn(move || {
            let result = record_loop(cli, stopped);
            let _ = sender.send(result);
        });
    }
    {
        let stopped = stopped.clone();
        let state_label = state_label.clone();
        let stop_button = stop.clone();
        stop.connect_clicked(move |_| {
            stopped.store(true, Ordering::Relaxed);
            state_label.set_text("Saving MP4…");
            stop_button.set_sensitive(false);
        });
    }
    glib::timeout_add_local(Duration::from_millis(250), move || {
        match receiver.try_recv() {
            Ok(Ok(path)) => {
                println!("{}", path.display());
                state_label.set_text("Saved MP4 to Videos/Metis");
                let window = window.clone();
                glib::timeout_add_local_once(Duration::from_millis(1200), move || window.close());
                glib::ControlFlow::Break
            }
            Ok(Err(error)) => {
                eprintln!("metis-screenshot record: {error}");
                state_label.set_text("Recording failed — check logs");
                let window = window.clone();
                glib::timeout_add_local_once(Duration::from_millis(2200), move || window.close());
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn record_loop(cli: Cli, stopped: Arc<AtomicBool>) -> Result<std::path::PathBuf, String> {
    let output = video_path()?;
    let encoder = mp4_encoder();
    let mut command = std::process::Command::new("ffmpeg");
    command.args([
        "-y",
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-framerate",
        "12",
        "-i",
        "-",
        "-an",
        "-vf",
        "pad=ceil(iw/2)*2:ceil(ih/2)*2",
        "-c:v",
        encoder,
    ]);
    if encoder == "libx264" {
        command.args(["-preset", "veryfast", "-crf", "23"]);
    } else {
        command.args(["-q:v", "5"]);
    }
    let mut child = command
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(&output)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("ffmpeg is required for MP4 recording: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ffmpeg stdin unavailable".to_string())?;
    let options = CaptureOptions {
        connector: cli.connector,
        output_index: 0,
        draw_cursor: true,
    };
    while !stopped.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        let frame = capture_output_frame(options.clone())?;
        let mut rgba = frame_to_rgba(&frame);
        let (mut width, mut height) = (frame.width, frame.height);
        if let Some(crop) = cli.crop {
            rgba = crop_rgba(
                &rgba,
                width,
                height,
                PixelRect {
                    x: crop.x,
                    y: crop.y,
                    width: crop.width,
                    height: crop.height,
                },
            )?;
            width = crop.width as u32;
            height = crop.height as u32;
        }
        let png = encode_png(width, height, &rgba)?;
        stdin
            .write_all(&png)
            .map_err(|error| format!("write frame to ffmpeg: {error}"))?;
        let elapsed = started.elapsed();
        if elapsed < Duration::from_millis(83) {
            std::thread::sleep(Duration::from_millis(83) - elapsed);
        }
    }
    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| format!("wait for ffmpeg: {error}"))?;
    if status.success() {
        Ok(output)
    } else {
        Err("ffmpeg failed to encode the recording".into())
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("PNG header: {error}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("PNG frame: {error}"))?;
    writer
        .finish()
        .map_err(|error| format!("PNG finish: {error}"))?;
    Ok(bytes)
}

fn video_path() -> Result<std::path::PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is unset".to_string())?;
    let directory = std::path::PathBuf::from(home).join("Videos/Metis");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create video directory: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock: {error}"))?
        .as_secs();
    Ok(directory.join(format!("metis-{timestamp}.mp4")))
}

/// Prefer H.264 for broad hardware/player compatibility. The built-in MPEG-4
/// Part 2 encoder is available in minimal ffmpeg builds and remains a valid MP4
/// fallback when a distribution omits libx264.
fn mp4_encoder() -> &'static str {
    let supports_x264 = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains("libx264"));
    if supports_x264 { "libx264" } else { "mpeg4" }
}
