//! System-audio spectrum for the Equalizer desktop widget.
//!
//! Captures the PulseAudio / PipeWire-pulse **default sink monitor**, runs a
//! short FFT, and publishes smoothed bands. Refcounted so multiple Equalizer
//! cards share one capture thread.
//!
//! Latency: small Pulse fragsize + overlapping FFT hops (~11 ms) so the widget
//! tracks audio tightly instead of waiting on a default ~1s server buffer.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use realfft::RealFftPlanner;

const SAMPLE_RATE: u32 = 44_100;
const FFT_SIZE: usize = 2048;
/// Hop between FFT frames — smaller = smoother / lower latency.
const HOP_SIZE: usize = 512;
const MAX_BANDS: usize = 96;
const ATTACK: f32 = 0.55;
const RELEASE: f32 = 0.22;
const PEAK_DECAY: f32 = 0.06;

#[derive(Debug, Clone)]
pub struct VizFrame {
    pub bands: Vec<f32>,
    pub peaks: Vec<f32>,
    pub active: bool,
}

impl Default for VizFrame {
    fn default() -> Self {
        Self {
            bands: vec![0.0; 48],
            peaks: vec![0.0; 48],
            active: false,
        }
    }
}

struct VizState {
    frame: Mutex<VizFrame>,
    stop: AtomicBool,
    refs: AtomicUsize,
}

static VIZ: OnceLock<Arc<VizState>> = OnceLock::new();

fn state() -> Arc<VizState> {
    VIZ.get_or_init(|| {
        Arc::new(VizState {
            frame: Mutex::new(VizFrame::default()),
            stop: AtomicBool::new(true),
            refs: AtomicUsize::new(0),
        })
    })
    .clone()
}

/// Retain the shared capture service (idempotent spawn).
pub fn ensure_audio_viz() {
    let st = state();
    let prev = st.refs.fetch_add(1, Ordering::SeqCst);
    if prev == 0 {
        st.stop.store(false, Ordering::SeqCst);
        let st2 = st.clone();
        if let Err(err) = thread::Builder::new()
            .name("metis-audio-viz".into())
            .spawn(move || capture_loop(st2))
        {
            tracing::warn!(%err, "audio viz: failed to spawn capture thread");
            st.refs.fetch_sub(1, Ordering::SeqCst);
            st.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// Release one retain; stops capture when the last Equalizer widget closes.
pub fn release_audio_viz() {
    let st = state();
    let prev = st.refs.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        st.stop.store(true, Ordering::SeqCst);
    } else if prev == 0 {
        st.refs.store(0, Ordering::SeqCst);
    }
}

/// Latest smoothed frame, resampled to `bar_count` bands (16–96).
pub fn audio_viz_frame(bar_count: u32) -> VizFrame {
    let n = bar_count.clamp(16, 96) as usize;
    let st = state();
    let Ok(guard) = st.frame.lock() else {
        return VizFrame {
            bands: vec![0.0; n],
            peaks: vec![0.0; n],
            active: false,
        };
    };
    let src = &guard.bands;
    let src_peaks = &guard.peaks;
    if src.is_empty() {
        return VizFrame {
            bands: vec![0.0; n],
            peaks: vec![0.0; n],
            active: false,
        };
    }
    let mut bands = vec![0.0_f32; n];
    let mut peaks = vec![0.0_f32; n];
    for i in 0..n {
        let t = i as f32 / n as f32;
        let idx = (t * (src.len() - 1) as f32).round() as usize;
        bands[i] = src[idx.min(src.len() - 1)];
        peaks[i] = src_peaks[idx.min(src_peaks.len() - 1)];
    }
    VizFrame {
        bands,
        peaks,
        active: guard.active,
    }
}

fn capture_loop(st: Arc<VizState>) {
    tracing::info!("audio viz: capture thread started");
    loop {
        if st.stop.load(Ordering::SeqCst) {
            break;
        }
        match run_pulse_session(&st) {
            Ok(()) => break,
            Err(err) => {
                tracing::warn!(%err, "audio viz: pulse session failed — retrying");
                decay_idle(&st);
                for _ in 0..20 {
                    if st.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    decay_idle(&st);
    tracing::info!("audio viz: capture thread stopped");
}

fn decay_idle(st: &VizState) {
    if let Ok(mut frame) = st.frame.lock() {
        for b in &mut frame.bands {
            *b *= 0.7;
        }
        for p in &mut frame.peaks {
            *p *= 0.85;
        }
        frame.active = false;
    }
}

fn run_pulse_session(st: &VizState) -> Result<(), String> {
    use libpulse_binding::def::BufferAttr;
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;

    let monitor = resolve_monitor_source()
        .ok_or_else(|| "no default sink monitor (is PipeWire/Pulse running?)".to_string())?;

    let spec = Spec {
        format: Format::F32le,
        channels: 1,
        rate: SAMPLE_RATE,
    };
    if !spec.is_valid() {
        return Err("invalid pulse sample spec".into());
    }

    // Keep server-side buffering tight — default Pulse Simple latency is often
    // ~1s, which made the equalizer feel a beat behind the music.
    let bytes_per_hop = (HOP_SIZE * 4) as u32;
    let bytes_fft = (FFT_SIZE * 4) as u32;
    let attr = BufferAttr {
        maxlength: bytes_fft * 4,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: bytes_per_hop,
    };

    let simple = Simple::new(
        None,
        "Metis Equalizer",
        Direction::Record,
        Some(&monitor),
        "Desktop visualizer",
        &spec,
        None,
        Some(&attr),
    )
    .map_err(|e| format!("pulse simple connect: {e}"))?;

    tracing::info!(%monitor, "audio viz: recording sink monitor (low-latency)");

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut indata = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();

    let mut bands = vec![0.0_f32; MAX_BANDS];
    let mut peaks = vec![0.0_f32; MAX_BANDS];
    // Ring buffer of mono f32 samples for overlapping FFT windows.
    let mut ring = vec![0.0_f32; FFT_SIZE];
    let mut hop_bytes = vec![0u8; HOP_SIZE * 4];

    while !st.stop.load(Ordering::SeqCst) {
        if let Err(err) = simple.read(&mut hop_bytes) {
            return Err(format!("pulse read: {err}"));
        }

        // Shift left by HOP_SIZE and append new samples.
        ring.copy_within(HOP_SIZE.., 0);
        for (i, chunk) in hop_bytes.as_chunks::<4>().0.iter().enumerate() {
            if i >= HOP_SIZE {
                break;
            }
            ring[FFT_SIZE - HOP_SIZE + i] =
                f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        for i in 0..FFT_SIZE {
            let w = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos());
            indata[i] = ring[i] * w;
        }

        if fft
            .process_with_scratch(&mut indata, &mut spectrum, &mut scratch)
            .is_err()
        {
            continue;
        }

        let nyquist = SAMPLE_RATE as f32 / 2.0;
        let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
        let mut energy = 0.0_f32;

        for b in 0..MAX_BANDS {
            let t0 = b as f32 / MAX_BANDS as f32;
            let t1 = (b + 1) as f32 / MAX_BANDS as f32;
            let f0 = 40.0 * (nyquist / 40.0).powf(t0);
            let f1 = 40.0 * (nyquist / 40.0).powf(t1);
            let i0 = ((f0 / bin_hz) as usize).clamp(1, spectrum.len() - 1);
            let i1 = ((f1 / bin_hz) as usize).clamp(i0 + 1, spectrum.len());

            let mut mag = 0.0_f32;
            for c in &spectrum[i0..i1] {
                mag += (c.re * c.re + c.im * c.im).sqrt();
            }
            mag /= (i1 - i0) as f32;
            let level = (mag * 0.1).ln_1p().clamp(0.0, 1.0);
            energy += level;

            let prev = bands[b];
            let next = if level > prev {
                prev + (level - prev) * ATTACK
            } else {
                prev + (level - prev) * RELEASE
            };
            bands[b] = next;
            peaks[b] = (peaks[b] - PEAK_DECAY).max(next);
        }

        let active = energy > 0.015;
        if let Ok(mut frame) = st.frame.lock() {
            frame.bands.clone_from(&bands);
            frame.peaks.clone_from(&peaks);
            frame.active = active;
        }
    }

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
