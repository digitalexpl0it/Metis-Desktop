//! Metis Remote (RUDP) microphone uplink — Opus datagrams (Viewer → host).
//!
//! Distinct magic from downlink audio (`MRUA`) and video (`MRUV`) so either
//! peer can demux on the first bytes. Framing matches downlink (20-byte header
//! + Opus); default format is 48 kHz mono, 20 ms frames.

/// Magic bytes `MRUM` (Metis RUDP Mic uplink).
pub const RUDP_MIC_MAGIC: [u8; 4] = *b"MRUM";
pub const RUDP_MIC_DATAGRAM_VERSION: u8 = 1;

/// Fixed mic datagram header size (payload follows).
pub const RUDP_MIC_HEADER_LEN: usize = 20;

/// Reuse the downlink Opus codec id so one decoder table covers both directions.
pub use crate::rudp_audio::AUDIO_CODEC_OPUS;

pub const RUDP_MIC_SAMPLE_RATE: u32 = 48_000;
/// Mic uplink is mono (viewer capture → host playback).
pub const RUDP_MIC_CHANNELS: u8 = 1;
/// Opus frame duration in milliseconds (20 ms @ 48 kHz = 960 samples).
pub const RUDP_MIC_FRAME_MS: u32 = 20;
pub const RUDP_MIC_FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicDatagram {
    pub codec: u8,
    pub seq: u32,
    pub pts_us: i64,
    pub payload: Vec<u8>,
}

/// Encode one Opus packet into a Quinn datagram body (Viewer → host).
pub fn encode_mic_datagram(seq: u32, pts_us: i64, opus: &[u8]) -> Result<Vec<u8>, String> {
    if opus.len() > u16::MAX as usize {
        return Err("mic payload too large".into());
    }
    let mut out = Vec::with_capacity(RUDP_MIC_HEADER_LEN + opus.len());
    out.extend_from_slice(&RUDP_MIC_MAGIC);
    out.push(RUDP_MIC_DATAGRAM_VERSION);
    out.push(AUDIO_CODEC_OPUS);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&pts_us.to_le_bytes());
    out.extend_from_slice(&(opus.len() as u16).to_le_bytes());
    out.extend_from_slice(opus);
    Ok(out)
}

/// Decode one mic uplink datagram. Returns `Err` on bad magic/version/truncation.
pub fn try_decode_mic_datagram(buf: &[u8]) -> Result<MicDatagram, String> {
    if buf.len() < RUDP_MIC_HEADER_LEN {
        return Err("mic datagram truncated".into());
    }
    if buf[0..4] != RUDP_MIC_MAGIC {
        return Err("bad mic magic".into());
    }
    let version = buf[4];
    if version != RUDP_MIC_DATAGRAM_VERSION {
        return Err(format!("unsupported mic datagram version {version}"));
    }
    let codec = buf[5];
    let seq = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
    let pts_us = i64::from_le_bytes([
        buf[10], buf[11], buf[12], buf[13], buf[14], buf[15], buf[16], buf[17],
    ]);
    let payload_len = u16::from_le_bytes([buf[18], buf[19]]) as usize;
    if buf.len() < RUDP_MIC_HEADER_LEN + payload_len {
        return Err("mic payload truncated".into());
    }
    Ok(MicDatagram {
        codec,
        seq,
        pts_us,
        payload: buf[RUDP_MIC_HEADER_LEN..RUDP_MIC_HEADER_LEN + payload_len].to_vec(),
    })
}

/// True when `buf` starts with the mic magic (safe on short slices).
pub fn is_mic_datagram(buf: &[u8]) -> bool {
    buf.len() >= 4 && buf[0..4] == RUDP_MIC_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_datagram_roundtrip() {
        let opus = vec![9u8, 8, 7, 6];
        let bytes = encode_mic_datagram(7, 2_000_000, &opus).expect("enc");
        assert!(is_mic_datagram(&bytes));
        assert!(!crate::is_audio_datagram(&bytes));
        let dec = try_decode_mic_datagram(&bytes).expect("dec");
        assert_eq!(dec.seq, 7);
        assert_eq!(dec.pts_us, 2_000_000);
        assert_eq!(dec.codec, AUDIO_CODEC_OPUS);
        assert_eq!(dec.payload, opus);
    }

    #[test]
    fn mic_magic_distinct_from_downlink() {
        assert_ne!(RUDP_MIC_MAGIC, crate::RUDP_AUDIO_MAGIC);
    }
}
