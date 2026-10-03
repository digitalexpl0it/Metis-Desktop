//! Subprocess probe + isolated encode worker for VAAPI/NVENC.
//!
//! libva / CUDA / FFmpeg can abort or segfault the calling process (missing
//! encode entrypoints, driver bugs). The compositor never runs that code
//! in-process: it probes with this binary, then encodes through `worker` mode.
//!
//! Usage:
//! - `metis-encode-probe <backend> <codec> <width> <height> <render_node>`
//!   Exit 0 = open OK, 1 = unavailable / error, 2 = bad arguments.
//! - `metis-encode-probe worker <fd>` — serve encode requests on socket `fd`
//!   (spawned by `metis_encode::open_isolated_encoder`).

use metis_encode::{EncoderBackend, EncoderConfig, FfmpegHwEncoder, RudpCodec};

/// Exit without running C `atexit` handlers. NVIDIA's `libnvcuvid` registers
/// one that joins an internal thread and deadlocks when a CUDA context was
/// ever created (the probe then "times out" even though NVENC opened fine).
/// Rust state is already dropped by the caller; stdout is unused.
fn hard_exit(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stderr().flush();
    // SAFETY: `_exit` is always safe to call; it never returns.
    unsafe { libc::_exit(code) }
}

fn usage_exit(msg: &str) -> ! {
    eprintln!("metis-encode-probe: {msg}");
    eprintln!(
        "usage: metis-encode-probe <vaapi|nvenc> <h264|hevc> <width> <height> <render_node>\n       \
         metis-encode-probe worker <fd>"
    );
    std::process::exit(2);
}

fn init_worker_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("worker") {
        let fd = args
            .get(1)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or_else(|| usage_exit("worker requires a socket fd"));
        init_worker_logging();
        // run_worker drops its encoder (CUDA / VA contexts) before returning.
        hard_exit(metis_encode::run_worker(fd));
    }
    probe_main(&args);
}

fn probe_main(args: &[String]) -> ! {
    let mut args = args.iter().map(String::as_str);
    let backend = match args.next() {
        Some("vaapi") | Some("auto") => EncoderBackend::Vaapi,
        Some("nvenc") => EncoderBackend::Nvenc,
        other => usage_exit(&format!("unknown backend {other:?}")),
    };
    let codec = match args.next() {
        Some("hevc") | Some("h265") => RudpCodec::Hevc,
        Some("h264") | Some("avc") => RudpCodec::H264,
        Some("av1") => RudpCodec::Av1,
        other => usage_exit(&format!("unknown codec {other:?}")),
    };
    let width = match args.next().and_then(|s| s.parse::<u32>().ok()) {
        Some(w) if w > 0 => w,
        _ => usage_exit("width required"),
    };
    let height = match args.next().and_then(|s| s.parse::<u32>().ok()) {
        Some(h) if h > 0 => h,
        _ => usage_exit("height required"),
    };
    let Some(render_node) = args.next() else {
        usage_exit("render_node path required");
    };

    let cfg = EncoderConfig {
        backend,
        codec,
        width,
        height,
        fps_hint: 30,
        bitrate_kbps: 2_000,
    };

    let code = match FfmpegHwEncoder::open(&cfg, render_node) {
        // Drop (closes the codec + hw contexts) before exit, not after.
        Ok(enc) => {
            drop(enc);
            0
        }
        Err(err) => {
            eprintln!("metis-encode-probe: {err}");
            1
        }
    };
    hard_exit(code);
}
