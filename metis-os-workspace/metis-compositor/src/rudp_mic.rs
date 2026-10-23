//! Viewer → host microphone uplink for Metis Remote: Opus decode → Pulse playback.
//!
//! Demand-gated on authenticated mic enables (`allow_mic_uplink` + Viewer
//! `MicEnable`). Failures leave the host in video + downlink-audio mode (never
//! take down Quinn / encode). Playback is on the **default sink** (v1 — echo
//! cancellation is out of scope; see USER_GUIDE).
//!
//! Session lock and `allow_mic_uplink=false` stop inject and clear
//! [`EngineShared::mic_inject_ready`] so the host can revoke `MicReady`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use metis_protocol::{RUDP_MIC_CHANNELS, RUDP_MIC_FRAME_SAMPLES, RUDP_MIC_SAMPLE_RATE};
use metis_rudp_host::engine::EngineShared;
use opus::{Channels, Decoder};

const IDLE_POLL: Duration = Duration::from_millis(20);
const FRAME_BYTES_F32: usize =
    RUDP_MIC_FRAME_SAMPLES * RUDP_MIC_CHANNELS as usize * std::mem::size_of::<f32>();

/// Spawn a supervisor that starts/stops Pulse inject when mic demand changes.
pub fn spawn_mic_supervisor(
    stop: Arc<AtomicBool>,
    shared: Arc<EngineShared>,
    session_locked: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("metis-rudp-mic-sup".into())
        .spawn(move || mic_supervisor_loop(stop, shared, session_locked))
        .unwrap_or_else(|err| {
            tracing::warn!(%err, "rudp mic: failed to spawn supervisor");
            std::thread::spawn(|| {})
        })
}

fn mic_supervisor_loop(
    stop: Arc<AtomicBool>,
    shared: Arc<EngineShared>,
    session_locked: Arc<AtomicBool>,
) {
    tracing::info!("rudp mic: supervisor started");
    let mut inject_stop: Option<Arc<AtomicBool>> = None;
    let mut inject_join: Option<JoinHandle<()>> = None;

    while !stop.load(Ordering::Relaxed) {
        let demand = shared.mic.demand.load(Ordering::SeqCst);
        let allow = shared.allow_mic_uplink.load(Ordering::Relaxed);
        let locked = session_locked.load(Ordering::Relaxed);
        let want = demand > 0 && allow && !locked;
        let running = inject_join.is_some();

        if want && !running {
            let istop = Arc::new(AtomicBool::new(false));
            let shared2 = Arc::clone(&shared);
            let istop2 = Arc::clone(&istop);
            match std::thread::Builder::new()
                .name("metis-rudp-mic".into())
                .spawn(move || inject_loop(istop2, shared2))
            {
                Ok(handle) => {
                    inject_stop = Some(istop);
                    inject_join = Some(handle);
                }
                Err(err) => {
                    tracing::warn!(%err, "rudp mic: spawn inject failed");
                    shared.mic_inject_ready.store(false, Ordering::Relaxed);
                }
            }
        } else if !want && running {
            if let Some(s) = inject_stop.take() {
                s.store(true, Ordering::SeqCst);
            }
            if let Some(h) = inject_join.take() {
                let _ = h.join();
            }
            shared.mic_inject_ready.store(false, Ordering::Relaxed);
            if let Ok(mut slot) = shared.mic.latest.lock() {
                *slot = None;
            }
        }
        std::thread::sleep(IDLE_POLL);
    }

    if let Some(s) = inject_stop.take() {
        s.store(true, Ordering::SeqCst);
    }
    if let Some(h) = inject_join.take() {
        let _ = h.join();
    }
    shared.mic_inject_ready.store(false, Ordering::Relaxed);
    tracing::info!("rudp mic: supervisor stopped");
}

fn inject_loop(stop: Arc<AtomicBool>, shared: Arc<EngineShared>) {
    match run_inject_session(&stop, &shared) {
        Ok(()) => {}
        Err(err) => {
            tracing::warn!(%err, "rudp mic: inject session failed — mic soft-fail");
            shared.mic_inject_ready.store(false, Ordering::Relaxed);
        }
    }
}

fn run_inject_session(stop: &AtomicBool, shared: &EngineShared) -> Result<(), String> {
    use libpulse_binding::def::BufferAttr;
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;

    let channels = RUDP_MIC_CHANNELS;
    let rate = RUDP_MIC_SAMPLE_RATE;
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
        tlength: frame_bytes * 2,
        prebuf: frame_bytes,
        minreq: frame_bytes,
        fragsize: u32::MAX,
    };

    // Play on the default sink (null-sink routing is a future Settings option).
    let simple = Simple::new(
        None,
        "Metis Remote Mic",
        Direction::Playback,
        None,
        "Remote microphone",
        &spec,
        None,
        Some(&attr),
    )
    .map_err(|e| format!("pulse simple playback: {e}"))?;

    let mut decoder =
        Decoder::new(rate, Channels::Mono).map_err(|e| format!("opus decoder: {e}"))?;

    tracing::info!(
        rate,
        channels,
        "rudp mic: Pulse inject ready (default sink)"
    );
    shared.mic_inject_ready.store(true, Ordering::Relaxed);

    let mut pcm_i16 = vec![0i16; RUDP_MIC_FRAME_SAMPLES * 4]; // Opus max frame headroom
    let mut pcm_f32 = vec![0u8; FRAME_BYTES_F32];
    let mut last_seq: Option<u32> = None;

    while !stop.load(Ordering::Relaxed) {
        let pkt = shared.mic.latest.lock().ok().and_then(|mut g| g.take());
        let Some(pkt) = pkt else {
            // Keep the Pulse stream alive with silence so the device does not underrun-stall.
            pcm_f32.fill(0);
            let _ = simple.write(&pcm_f32);
            std::thread::sleep(IDLE_POLL);
            continue;
        };
        if let Some(last) = last_seq
            && pkt.seq <= last
            && !(last > 0xFFFF_0000 && pkt.seq < 0x1000)
        {
            continue;
        }
        last_seq = Some(pkt.seq);

        let samples_per_ch = match decoder.decode(&pkt.opus, &mut pcm_i16, false) {
            Ok(n) => n,
            Err(err) => {
                tracing::debug!(%err, "rudp mic: opus decode failed");
                continue;
            }
        };
        let total = samples_per_ch * channels as usize;
        // Write one 20 ms frame at a time (pad/truncate to expected size).
        let want = RUDP_MIC_FRAME_SAMPLES * channels as usize;
        pcm_f32.fill(0);
        for (i, &s) in pcm_i16.iter().take(total.min(want)).enumerate() {
            let f = (s as f32) / 32768.0;
            let off = i * 4;
            if off + 4 <= pcm_f32.len() {
                pcm_f32[off..off + 4].copy_from_slice(&f.to_le_bytes());
            }
        }
        if let Err(err) = simple.write(&pcm_f32) {
            return Err(format!("pulse write: {err}"));
        }
    }

    shared.mic_inject_ready.store(false, Ordering::Relaxed);
    Ok(())
}

/// Apply a Viewer mic enable/disable on the compositor (demand counter).
pub fn apply_mic_enable(shared: &EngineShared, enabled: bool) {
    if enabled {
        shared.mic.demand.fetch_add(1, Ordering::SeqCst);
    } else {
        let _ = shared
            .mic
            .demand
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |d| {
                Some(d.saturating_sub(1))
            });
    }
}

/// Push one demuxed Opus packet for Pulse inject.
pub fn push_mic_packet(shared: &EngineShared, seq: u32, pts_us: i64, opus: Vec<u8>) {
    if shared.mic.demand.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !shared.allow_mic_uplink.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut slot) = shared.mic.latest.lock() {
        *slot = Some(metis_rudp_host::engine::RudpMicPacket { seq, pts_us, opus });
    }
}
