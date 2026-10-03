//! Metis Remote Viewer audio: Opus decode + cpal playback.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use metis_protocol::{RUDP_AUDIO_CHANNELS, RUDP_AUDIO_SAMPLE_RATE};
use opus::{Channels, Decoder};

const MAX_QUEUED_SAMPLES: usize = RUDP_AUDIO_SAMPLE_RATE as usize * RUDP_AUDIO_CHANNELS as usize; // ~1s

struct PlaybackInner {
    decoder: Decoder,
    /// Interleaved f32 PCM awaiting the output callback.
    pcm: VecDeque<f32>,
    last_seq: Option<u32>,
}

/// Owns the cpal output stream for one RUDP session.
pub struct RudpAudioPlayback {
    inner: Arc<Mutex<PlaybackInner>>,
    _stream: cpal::Stream,
}

impl RudpAudioPlayback {
    pub fn start(sample_rate: u32, channels: u8) -> Result<Self, String> {
        if channels != RUDP_AUDIO_CHANNELS {
            return Err(format!("unsupported channel count {channels}"));
        }
        let rate = if sample_rate == 0 {
            RUDP_AUDIO_SAMPLE_RATE
        } else {
            sample_rate
        };
        let decoder =
            Decoder::new(rate, Channels::Stereo).map_err(|e| format!("opus decoder: {e}"))?;

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default audio output device".to_string())?;
        let config = cpal::StreamConfig {
            channels: u16::from(channels),
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let inner = Arc::new(Mutex::new(PlaybackInner {
            decoder,
            pcm: VecDeque::new(),
            last_seq: None,
        }));
        let cb_inner = Arc::clone(&inner);
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let Ok(mut g) = cb_inner.lock() else {
                        data.fill(0.0);
                        return;
                    };
                    for sample in data.iter_mut() {
                        *sample = g.pcm.pop_front().unwrap_or(0.0);
                    }
                },
                |err| {
                    tracing::warn!(%err, "rudp audio: cpal stream error");
                },
                None,
            )
            .map_err(|e| format!("cpal output stream: {e}"))?;
        stream.play().map_err(|e| format!("cpal play: {e}"))?;

        tracing::info!(rate, channels, "rudp audio: playback started");
        Ok(Self {
            inner,
            _stream: stream,
        })
    }

    pub fn push_packet(&self, seq: u32, opus: &[u8]) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if let Some(last) = g.last_seq
            && seq <= last
            && !(last > 0xFFFF_0000 && seq < 0x1000)
        {
            // Drop late / duplicate (allow wrap near u32 max).
            return;
        }
        g.last_seq = Some(seq);

        let mut pcm_i16 = vec![0i16; 5760 * RUDP_AUDIO_CHANNELS as usize]; // max Opus frame
        let samples_per_ch = match g.decoder.decode(opus, &mut pcm_i16, false) {
            Ok(n) => n,
            Err(err) => {
                tracing::debug!(%err, "rudp audio: opus decode failed");
                return;
            }
        };
        let total = samples_per_ch * RUDP_AUDIO_CHANNELS as usize;
        for &s in pcm_i16.iter().take(total) {
            if g.pcm.len() >= MAX_QUEUED_SAMPLES {
                g.pcm.pop_front();
            }
            g.pcm.push_back(s as f32 / 32768.0);
        }
    }
}
