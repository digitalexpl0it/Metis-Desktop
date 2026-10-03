//! Isolated encode worker smoke tests.
//!
//! The protocol / crash-isolation tests run everywhere (no GPU needed: the
//! worker fails Init or is aborted on purpose). Hardware tests are opt-in:
//! - VAAPI (Intel/AMD): `METIS_TEST_RENDER_NODE=/dev/dri/renderD12x`
//! - NVENC (NVIDIA): `METIS_TEST_NVENC=1` (optional `METIS_TEST_NVENC_NODE`)

use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use metis_encode::{
    EncodeError, EncodeInput, EncoderBackend, EncoderConfig, HwEncoder, ProcessEncoder, RudpCodec,
};

const XRGB8888: u32 = 0x3432_5258;

/// Tests mutate process env inherited by workers — run them one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn use_built_probe() {
    // SAFETY: tests in this binary only read this variable; set before any spawn.
    unsafe {
        std::env::set_var(
            "METIS_ENCODE_PROBE",
            env!("CARGO_BIN_EXE_metis-encode-probe"),
        )
    };
}

fn memfd_frame(width: u32, height: u32, stride: u32, frame: u32) -> OwnedFd {
    let name = c"metis-encode-test";
    // SAFETY: plain syscall; result checked.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw >= 0, "memfd_create failed");
    // SAFETY: fresh fd we own.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut buf = vec![0u8; (stride * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = (y * stride + x * 4) as usize;
            buf[i] = ((x + frame) & 0xff) as u8;
            buf[i + 1] = (y & 0xff) as u8;
            buf[i + 2] = ((x ^ y) & 0xff) as u8;
        }
    }
    let file = std::fs::File::from(fd);
    std::io::Write::write_all(&mut &file, &buf).expect("write frame");
    OwnedFd::from(file)
}

fn submit(
    enc: &mut dyn HwEncoder,
    fd: &OwnedFd,
    seq: u64,
    w: u32,
    h: u32,
    stride: u32,
) -> Result<(), EncodeError> {
    let fds = [fd.as_fd()];
    let input = EncodeInput {
        seq,
        width: w,
        height: h,
        stride,
        fourcc: XRGB8888,
        modifier: 0,
        fds: &fds,
        offsets: &[0],
        strides: &[stride],
        damage_full: true,
        damage: &[],
    };
    enc.submit(&input)
}

fn cfg(w: u32, h: u32, backend: EncoderBackend) -> EncoderConfig {
    EncoderConfig {
        backend,
        codec: RudpCodec::H264,
        width: w,
        height: h,
        fps_hint: 30,
        bitrate_kbps: 4_000,
    }
}

#[test]
fn bad_render_node_fails_cleanly() {
    let _g = serial();
    use_built_probe();
    let started = Instant::now();
    let res = ProcessEncoder::spawn(
        &cfg(64, 64, EncoderBackend::Vaapi),
        "/dev/dri/does-not-exist",
        None,
    );
    assert!(res.is_err());
    assert!(started.elapsed() < Duration::from_secs(25));
}

#[test]
fn cancel_aborts_init_quickly() {
    let _g = serial();
    use_built_probe();
    let cancel = Arc::new(AtomicBool::new(true));
    let started = Instant::now();
    let res = ProcessEncoder::spawn(
        &cfg(64, 64, EncoderBackend::Nvenc),
        "/dev/dri/renderD128",
        Some(cancel.clone()),
    );
    // Either the worker failed fast or we cancelled; must never hang.
    assert!(res.is_err() || cancel.load(Ordering::Relaxed));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel took {:?}",
        started.elapsed()
    );
}

fn hw_node() -> Option<String> {
    std::env::var("METIS_TEST_RENDER_NODE").ok()
}

#[test]
fn hardware_encode_produces_keyframe() {
    let Some(node) = hw_node() else {
        eprintln!("skipped: set METIS_TEST_RENDER_NODE");
        return;
    };
    encode_roundtrip(EncoderBackend::Vaapi, RudpCodec::H264, &node);
}

/// NVENC device: `METIS_TEST_NVENC_NODE`, else the first NVIDIA render node.
/// Opt in with `METIS_TEST_NVENC=1` (needs the proprietary driver + an FFmpeg
/// built with NVENC; run inside a desktop session that can see `/dev/nvidia*`).
fn nvenc_node() -> Option<String> {
    std::env::var_os("METIS_TEST_NVENC")?;
    if let Ok(node) = std::env::var("METIS_TEST_NVENC_NODE") {
        return Some(node);
    }
    Some(
        metis_encode::preferred_render_node("/dev/dri/renderD128", EncoderBackend::Nvenc)
            .to_string_lossy()
            .into_owned(),
    )
}

#[test]
fn nvenc_h264_encode_produces_keyframe() {
    let Some(node) = nvenc_node() else {
        eprintln!("skipped: set METIS_TEST_NVENC=1");
        return;
    };
    encode_roundtrip(EncoderBackend::Nvenc, RudpCodec::H264, &node);
}

#[test]
fn nvenc_hevc_encode_produces_keyframe() {
    let Some(node) = nvenc_node() else {
        eprintln!("skipped: set METIS_TEST_NVENC=1");
        return;
    };
    encode_roundtrip(EncoderBackend::Nvenc, RudpCodec::Hevc, &node);
}

/// Spawn a worker for exactly `backend`/`codec`, encode 10 frames (padded
/// stride, moving pattern), and require an IDR first with no silent fallback.
fn encode_roundtrip(backend: EncoderBackend, codec: RudpCodec, node: &str) {
    let _g = serial();
    use_built_probe();
    let (w, h, stride) = (640, 360, 640 * 4 + 64);
    let mut config = cfg(w, h, backend);
    config.codec = codec;
    let started = Instant::now();
    let mut enc = match ProcessEncoder::spawn(&config, node, None) {
        Ok(enc) => enc,
        Err(err) => panic!(
            "{backend:?}/{codec:?} on {node}: worker init failed after {:?}: {err}",
            started.elapsed()
        ),
    };
    let info = enc.info().clone();
    eprintln!(
        "{backend:?}/{codec:?} on {node}: opened {} in {:?}",
        info.encoder_name,
        started.elapsed()
    );
    assert_eq!(info.backend, backend, "fell back to another backend");
    assert_eq!(info.codec, codec, "fell back to another codec");

    let mut packets = Vec::new();
    let encode_started = Instant::now();
    for seq in 0..10u64 {
        let fd = memfd_frame(w, h, stride, seq as u32);
        submit(&mut enc, &fd, seq, w, h, stride).expect("submit");
        packets.extend(enc.drain().expect("drain"));
    }
    packets.extend(enc.flush().expect("flush"));
    let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
    eprintln!(
        "{backend:?}/{codec:?}: {} packets, {bytes} bytes, 10 frames in {:?}",
        packets.len(),
        encode_started.elapsed()
    );
    assert!(!packets.is_empty(), "no packets");
    assert!(packets[0].is_keyframe, "first packet must be IDR");
    assert!(packets.iter().all(|p| p.codec == codec));
    assert!(enc.is_healthy());
}

#[test]
fn worker_crash_is_contained() {
    let Some(node) = hw_node() else {
        eprintln!("skipped: set METIS_TEST_RENDER_NODE");
        return;
    };
    let _g = serial();
    use_built_probe();
    // SAFETY: test-only env, read by the child at startup.
    unsafe { std::env::set_var("METIS_ENCODE_WORKER_ABORT_AFTER", "2") };
    let (w, h, stride) = (320, 240, 320 * 4);
    let spawned = ProcessEncoder::spawn(&cfg(w, h, EncoderBackend::Vaapi), &node, None);
    unsafe { std::env::remove_var("METIS_ENCODE_WORKER_ABORT_AFTER") };
    let mut enc = spawned.expect("worker init");
    let fd = memfd_frame(w, h, stride, 0);
    submit(&mut enc, &fd, 0, w, h, stride).expect("frame 0");
    submit(&mut enc, &fd, 1, w, h, stride).expect("frame 1");
    let started = Instant::now();
    let err = submit(&mut enc, &fd, 2, w, h, stride).expect_err("worker aborted");
    assert!(matches!(err, EncodeError::Worker(_)), "got {err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!enc.is_healthy());
    // Further submits fail fast instead of blocking.
    assert!(submit(&mut enc, &fd, 3, w, h, stride).is_err());
}
