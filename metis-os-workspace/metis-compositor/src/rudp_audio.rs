//! Host → client system audio for Metis Remote: Pulse sink-monitor → Opus.
//!
//! Demand-gated on authenticated RUDP sessions. Failures leave the host in
//! video-only mode (never take down Quinn / encode).

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use metis_protocol::{
    RUDP_AUDIO_CHANNELS, RUDP_AUDIO_FRAME_SAMPLES, RUDP_AUDIO_SAMPLE_RATE, encode_audio_datagram,
};

const FRAME_BYTES_F32: usize =
    RUDP_AUDIO_FRAME_SAMPLES * RUDP_AUDIO_CHANNELS as usize * std::mem::size_of::<f32>();
const OPUS_BITRATE: i32 = 128_000;
const IDLE_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct RudpAudioPacket {
    pub seq: u32,
    /// Presentation timestamp (retained for future A/V sync).
    #[allow(dead_code)]
    pub pts_us: i64,
    /// Encoded Quinn datagram (MRUA header + Opus).
    pub datagram: Vec<u8>,
}

/// Shared with the Quinn pump.
pub struct RudpAudioShared {
    pub latest: Mutex<Option<RudpAudioPacket>>,
    /// True after a successful Pulse+Opus open (host may advertise AudioReady).
    pub streaming: AtomicBool,
    seq: AtomicU32,
}

impl RudpAudioShared {
    pub fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            streaming: AtomicBool::new(false),
            seq: AtomicU32::new(0),
        }
    }
}

/// Spawn a supervisor that starts/stops capture when `active_sessions` changes.
pub fn spawn_audio_supervisor(
    stop: Arc<AtomicBool>,
    active_sessions: Arc<AtomicUsize>,
    audio: Arc<RudpAudioShared>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("metis-rudp-audio-sup".into())
        .spawn(move || audio_supervisor_loop(stop, active_sessions, audio))
        .unwrap_or_else(|err| {
            tracing::warn!(%err, "rudp audio: failed to spawn supervisor");
            // Return a no-op join handle by spawning an empty thread.
            std::thread::spawn(|| {})
        })
}

fn audio_supervisor_loop(
    stop: Arc<AtomicBool>,
    active_sessions: Arc<AtomicUsize>,
    audio: Arc<RudpAudioShared>,
) {
    tracing::info!("rudp audio: supervisor started");
    let mut capture_stop: Option<Arc<AtomicBool>> = None;
    let mut capture_join: Option<JoinHandle<()>> = None;

    while !stop.load(Ordering::Relaxed) {
        let sessions = active_sessions.load(Ordering::SeqCst);
        let running = capture_join.is_some();
        if sessions > 0 && !running {
            let cstop = Arc::new(AtomicBool::new(false));
            let audio2 = Arc::clone(&audio);
            let cstop2 = Arc::clone(&cstop);
            match std::thread::Builder::new()
                .name("metis-rudp-audio".into())
                .spawn(move || capture_encode_loop(cstop2, audio2))
            {
                Ok(handle) => {
                    capture_stop = Some(cstop);
                    capture_join = Some(handle);
                }
                Err(err) => {
                    tracing::warn!(%err, "rudp audio: spawn capture failed");
                }
            }
        } else if sessions == 0 && running {
            if let Some(s) = capture_stop.take() {
                s.store(true, Ordering::SeqCst);
            }
            if let Some(h) = capture_join.take() {
                let _ = h.join();
            }
            audio.streaming.store(false, Ordering::Relaxed);
            if let Ok(mut slot) = audio.latest.lock() {
                *slot = None;
            }
        }
        std::thread::sleep(IDLE_POLL);
    }

    if let Some(s) = capture_stop.take() {
        s.store(true, Ordering::SeqCst);
    }
    if let Some(h) = capture_join.take() {
        let _ = h.join();
    }
    audio.streaming.store(false, Ordering::Relaxed);
    tracing::info!("rudp audio: supervisor stopped");
}

fn capture_encode_loop(stop: Arc<AtomicBool>, audio: Arc<RudpAudioShared>) {
    match run_capture_session(&stop, &audio) {
        Ok(()) => {}
        Err(err) => {
            tracing::warn!(%err, "rudp audio: capture session failed — video-only");
            audio.streaming.store(false, Ordering::Relaxed);
        }
    }
}

fn run_capture_session(stop: &AtomicBool, audio: &RudpAudioShared) -> Result<(), String> {
    use libpulse_binding::def::BufferAttr;
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;
    use opus::{Application, Bitrate, Channels, Encoder};

    let monitor = resolve_monitor_source()
        .ok_or_else(|| "no default sink monitor (is PipeWire/Pulse running?)".to_string())?;

    let channels = RUDP_AUDIO_CHANNELS;
    let rate = RUDP_AUDIO_SAMPLE_RATE;
    let spec = Spec {
        format: Format::F32le,
        channels,
        rate,
    };
    if !spec.is_valid() {
        return Err("invalid pulse sample spec".into());
    }

    let frame_bytes = FRAME_BYTES_F32 as u32;
    let attr = BufferAttr {
        maxlength: frame_bytes * 8,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: frame_bytes,
    };

    let simple = Simple::new(
        None,
        "Metis Remote",
        Direction::Record,
        Some(&monitor),
        "Desktop audio",
        &spec,
        None,
        Some(&attr),
    )
    .map_err(|e| format!("pulse simple connect: {e}"))?;

    let mut encoder = Encoder::new(rate, Channels::Stereo, Application::Audio)
        .map_err(|e| format!("opus encoder: {e}"))?;
    encoder
        .set_bitrate(Bitrate::Bits(OPUS_BITRATE))
        .map_err(|e| format!("opus bitrate: {e}"))?;

    tracing::info!(%monitor, rate, channels, "rudp audio: capturing sink monitor");
    audio.streaming.store(true, Ordering::Relaxed);

    let mut pcm_bytes = vec![0u8; FRAME_BYTES_F32];
    let mut pcm_i16 = vec![0i16; RUDP_AUDIO_FRAME_SAMPLES * channels as usize];
    let mut opus_buf = vec![0u8; 4000];
    let started = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        if let Err(err) = simple.read(&mut pcm_bytes) {
            return Err(format!("pulse read: {err}"));
        }
        let samples = RUDP_AUDIO_FRAME_SAMPLES * channels as usize;
        let (chunks, _) = pcm_bytes.as_chunks::<4>();
        for (i, chunk) in chunks.iter().enumerate().take(samples) {
            let f = f32::from_le_bytes(*chunk);
            let clamped = f.clamp(-1.0, 1.0);
            pcm_i16[i] = (clamped * 32767.0) as i16;
        }
        let n = encoder
            .encode(&pcm_i16, &mut opus_buf)
            .map_err(|e| format!("opus encode: {e}"))?;
        if n == 0 {
            continue;
        }
        let seq = audio.seq.fetch_add(1, Ordering::Relaxed);
        let pts_us = started.elapsed().as_micros() as i64;
        let datagram = encode_audio_datagram(seq, pts_us, &opus_buf[..n])
            .map_err(|e| format!("audio datagram: {e}"))?;
        if let Ok(mut slot) = audio.latest.lock() {
            *slot = Some(RudpAudioPacket {
                seq,
                pts_us,
                datagram,
            });
        }
    }

    audio.streaming.store(false, Ordering::Relaxed);
    Ok(())
}

fn resolve_monitor_source() -> Option<String> {
    let out = std::process::Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sink = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sink.is_empty() {
        return None;
    }
    Some(format!("{sink}.monitor"))
}
