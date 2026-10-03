//! Metis Remote (RUDP) audio wire format — Opus datagrams (no FEC).
//!
//! Distinct magic from video (`MRUV`) so clients can demux on the first bytes.

/// Magic bytes `MRUA` (Metis RUDP Audio).
pub const RUDP_AUDIO_MAGIC: [u8; 4] = *b"MRUA";
pub const RUDP_AUDIO_DATAGRAM_VERSION: u8 = 1;

/// Fixed audio datagram header size (payload follows).
pub const RUDP_AUDIO_HEADER_LEN: usize = 20;

pub const AUDIO_CODEC_OPUS: u8 = 1;

pub const RUDP_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const RUDP_AUDIO_CHANNELS: u8 = 2;
/// Opus frame duration in milliseconds (20 ms @ 48 kHz = 960 samples/channel).
pub const RUDP_AUDIO_FRAME_MS: u32 = 20;
pub const RUDP_AUDIO_FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDatagram {
    pub codec: u8,
    pub seq: u32,
    pub pts_us: i64,
    pub payload: Vec<u8>,
}

/// Encode one Opus packet into a Quinn datagram body.
pub fn encode_audio_datagram(seq: u32, pts_us: i64, opus: &[u8]) -> Result<Vec<u8>, String> {
    if opus.len() > u16::MAX as usize {
        return Err("audio payload too large".into());
    }
    let mut out = Vec::with_capacity(RUDP_AUDIO_HEADER_LEN + opus.len());
    out.extend_from_slice(&RUDP_AUDIO_MAGIC);
    out.push(RUDP_AUDIO_DATAGRAM_VERSION);
    out.push(AUDIO_CODEC_OPUS);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&pts_us.to_le_bytes());
    out.extend_from_slice(&(opus.len() as u16).to_le_bytes());
    out.extend_from_slice(opus);
    Ok(out)
}

/// Decode one audio datagram. Returns `Err` on bad magic/version/truncation.
pub fn try_decode_audio_datagram(buf: &[u8]) -> Result<AudioDatagram, String> {
    if buf.len() < RUDP_AUDIO_HEADER_LEN {
        return Err("audio datagram truncated".into());
    }
    if buf[0..4] != RUDP_AUDIO_MAGIC {
        return Err("bad audio magic".into());
    }
    let version = buf[4];
    if version != RUDP_AUDIO_DATAGRAM_VERSION {
        return Err(format!("unsupported audio datagram version {version}"));
    }
    let codec = buf[5];
    let seq = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
    let pts_us = i64::from_le_bytes([
        buf[10], buf[11], buf[12], buf[13], buf[14], buf[15], buf[16], buf[17],
    ]);
    let payload_len = u16::from_le_bytes([buf[18], buf[19]]) as usize;
    if buf.len() < RUDP_AUDIO_HEADER_LEN + payload_len {
        return Err("audio payload truncated".into());
    }
    Ok(AudioDatagram {
        codec,
        seq,
        pts_us,
        payload: buf[RUDP_AUDIO_HEADER_LEN..RUDP_AUDIO_HEADER_LEN + payload_len].to_vec(),
    })
}

/// True when `buf` starts with the audio magic (safe on short slices).
pub fn is_audio_datagram(buf: &[u8]) -> bool {
    buf.len() >= 4 && buf[0..4] == RUDP_AUDIO_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_datagram_roundtrip() {
        let opus = vec![1u8, 2, 3, 4, 5];
        let bytes = encode_audio_datagram(42, 1_000_000, &opus).expect("enc");
        assert!(is_audio_datagram(&bytes));
        let dec = try_decode_audio_datagram(&bytes).expect("dec");
        assert_eq!(dec.seq, 42);
        assert_eq!(dec.pts_us, 1_000_000);
        assert_eq!(dec.codec, AUDIO_CODEC_OPUS);
        assert_eq!(dec.payload, opus);
    }
}
