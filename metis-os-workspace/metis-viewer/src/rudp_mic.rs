//! Metis Remote Viewer mic uplink: cpal capture → Opus → `MRUM` datagrams.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytes::Bytes;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use metis_protocol::{
    RUDP_MIC_CHANNELS, RUDP_MIC_FRAME_SAMPLES, RUDP_MIC_SAMPLE_RATE, encode_mic_datagram,
};
use opus::{Application, Bitrate, Channels, Encoder};

const OPUS_BITRATE: i32 = 64_000;

/// Owns the cpal input stream; encodes Opus and invokes `on_datagram`.
pub struct RudpMicCapture {
    stop: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

impl RudpMicCapture {
    /// Start capturing from the default input device at 48 kHz mono.
    ///
    /// `on_datagram` is called from the audio callback thread — keep it cheap
    /// (e.g. `session.send_datagram`).
    pub fn start(on_datagram: Arc<dyn Fn(Bytes) + Send + Sync>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no default microphone".to_string())?;
        let config = cpal::StreamConfig {
            channels: u16::from(RUDP_MIC_CHANNELS),
            sample_rate: cpal::SampleRate(RUDP_MIC_SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let mut encoder = Encoder::new(RUDP_MIC_SAMPLE_RATE, Channels::Mono, Application::Voip)
            .map_err(|e| format!("opus encoder: {e}"))?;
        encoder
            .set_bitrate(Bitrate::Bits(OPUS_BITRATE))
            .map_err(|e| format!("opus bitrate: {e}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_cb = Arc::clone(&stop);
        let seq = Arc::new(AtomicU32::new(1));
        let started = Instant::now();
        let pending = Arc::new(Mutex::new(Vec::<f32>::new()));
        let encoder = Arc::new(Mutex::new(encoder));

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    if stop_cb.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(mut buf) = pending.lock() else {
                        return;
                    };
                    buf.extend_from_slice(data);
                    while buf.len() >= RUDP_MIC_FRAME_SAMPLES {
                        let frame: Vec<f32> = buf.drain(..RUDP_MIC_FRAME_SAMPLES).collect();
                        let mut pcm_i16 = vec![0i16; RUDP_MIC_FRAME_SAMPLES];
                        for (i, &s) in frame.iter().enumerate() {
                            pcm_i16[i] = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                        let Ok(mut enc) = encoder.lock() else {
                            return;
                        };
                        let mut opus_buf = vec![0u8; 4000];
                        let n = match enc.encode(&pcm_i16, &mut opus_buf) {
                            Ok(n) => n,
                            Err(err) => {
                                tracing::debug!(%err, "rudp mic: opus encode failed");
                                continue;
                            }
                        };
                        if n == 0 {
                            continue;
                        }
                        let s = seq.fetch_add(1, Ordering::Relaxed);
                        let pts_us = started.elapsed().as_micros() as i64;
                        match encode_mic_datagram(s, pts_us, &opus_buf[..n]) {
                            Ok(dgram) => on_datagram(Bytes::from(dgram)),
                            Err(err) => tracing::debug!(%err, "rudp mic: datagram encode failed"),
                        }
                    }
                },
                |err| {
                    tracing::warn!(%err, "rudp mic: cpal input error");
                },
                None,
            )
            .map_err(|e| format!("cpal input stream: {e}"))?;
        stream.play().map_err(|e| format!("cpal play: {e}"))?;

        tracing::info!(
            rate = RUDP_MIC_SAMPLE_RATE,
            channels = RUDP_MIC_CHANNELS,
            "rudp mic: capture started"
        );
        Ok(Self {
            stop,
            _stream: stream,
        })
    }
}

impl Drop for RudpMicCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
