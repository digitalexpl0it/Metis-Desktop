//! Metis Remote (RUDP) video wire format — datagram shards + reliable AUs.
//!
//! Datagrams are little-endian fixed headers + payload. Reliable access units
//! on a host-opened uni-stream use the same length-prefix framing as control
//! (`u32` BE length + body).

use reed_solomon_erasure::galois_8::ReedSolomon;

/// Magic bytes `MRUV` (Metis RUDP Video).
pub const RUDP_VIDEO_MAGIC: [u8; 4] = *b"MRUV";
pub const RUDP_VIDEO_DATAGRAM_VERSION: u8 = 1;
pub const RUDP_VIDEO_AU_VERSION: u8 = 1;

/// Fixed datagram header size (payload follows).
pub const RUDP_DATAGRAM_HEADER_LEN: usize = 32;

/// Default max datagram used when Quinn has not yet advertised a size.
pub const RUDP_DEFAULT_DATAGRAM_BUDGET: usize = 1200;

pub const FLAG_KEY: u8 = 0x01;
pub const FLAG_FEC: u8 = 0x02;
pub const FLAG_DAMAGE_FULL: u8 = 0x04;

pub const CODEC_H264: u8 = 1;
pub const CODEC_HEVC: u8 = 2;

/// Physical dirty rectangle on a reliable AU (compact wire form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RudpDamageRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatagramHeader {
    pub version: u8,
    pub flags: u8,
    pub codec: u8,
    pub frame_seq: u64,
    pub pts_us: i64,
    pub shard_i: u16,
    pub shard_n: u16,
    pub fec_m: u16,
    pub payload_len: u16,
}

impl DatagramHeader {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&RUDP_VIDEO_MAGIC);
        out.push(self.version);
        out.push(self.flags);
        out.push(self.codec);
        out.push(0);
        out.extend_from_slice(&self.frame_seq.to_le_bytes());
        out.extend_from_slice(&self.pts_us.to_le_bytes());
        out.extend_from_slice(&self.shard_i.to_le_bytes());
        out.extend_from_slice(&self.shard_n.to_le_bytes());
        out.extend_from_slice(&self.fec_m.to_le_bytes());
        out.extend_from_slice(&self.payload_len.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize), String> {
        if buf.len() < RUDP_DATAGRAM_HEADER_LEN {
            return Err("datagram header truncated".into());
        }
        if buf[0..4] != RUDP_VIDEO_MAGIC {
            return Err("bad datagram magic".into());
        }
        let version = buf[4];
        if version != RUDP_VIDEO_DATAGRAM_VERSION {
            return Err(format!("unsupported datagram version {version}"));
        }
        let payload_len = u16::from_le_bytes([buf[30], buf[31]]) as usize;
        if buf.len() < RUDP_DATAGRAM_HEADER_LEN + payload_len {
            return Err("datagram payload truncated".into());
        }
        Ok((
            Self {
                version,
                flags: buf[5],
                codec: buf[6],
                frame_seq: u64::from_le_bytes(buf[8..16].try_into().unwrap_or([0; 8])),
                pts_us: i64::from_le_bytes(buf[16..24].try_into().unwrap_or([0; 8])),
                shard_i: u16::from_le_bytes([buf[24], buf[25]]),
                shard_n: u16::from_le_bytes([buf[26], buf[27]]),
                fec_m: u16::from_le_bytes([buf[28], buf[29]]),
                payload_len: payload_len as u16,
            },
            RUDP_DATAGRAM_HEADER_LEN + payload_len,
        ))
    }
}

/// Payload budget for one datagram (header excluded).
pub fn datagram_payload_budget(max_datagram: usize) -> usize {
    let capped = max_datagram.min(RUDP_DEFAULT_DATAGRAM_BUDGET);
    capped.saturating_sub(RUDP_DATAGRAM_HEADER_LEN).max(64)
}

/// Split `data` into equal-size shards that fit in `max_payload` (pad last).
pub fn shard_payload(data: &[u8], max_payload: usize) -> Vec<Vec<u8>> {
    let max_payload = max_payload.max(1);
    if data.is_empty() {
        return vec![vec![0u8; max_payload.min(1)]];
    }
    let k = data.len().div_ceil(max_payload).max(1);
    let shard_len = data.len().div_ceil(k).max(1);
    let mut shards = Vec::with_capacity(k);
    for i in 0..k {
        let start = i * shard_len;
        let mut shard = vec![0u8; shard_len];
        if start < data.len() {
            let end = (start + shard_len).min(data.len());
            let n = end - start;
            shard[..n].copy_from_slice(&data[start..end]);
        }
        shards.push(shard);
    }
    shards
}

/// Adaptive parity count from Quinn loss ratio and data-shard count `k`.
pub fn fec_parity_count(k: usize, loss_ratio: f64) -> usize {
    if k == 0 {
        return 0;
    }
    if k == 1 {
        return 1;
    }
    if loss_ratio < 0.02 {
        ((k as f64) / 10.0).ceil() as usize
    } else {
        ((k as f64) / 4.0).ceil() as usize
    }
    .clamp(
        if loss_ratio < 0.02 { 1 } else { 2 },
        if loss_ratio < 0.02 { 4 } else { 8 },
    )
}

/// Append `m` Reed-Solomon parity shards (same length as data shards).
pub fn apply_fec(data_shards: &[Vec<u8>], m: usize) -> Result<Vec<Vec<u8>>, String> {
    let k = data_shards.len();
    if k == 0 {
        return Err("no data shards".into());
    }
    if m == 0 {
        return Ok(data_shards.to_vec());
    }
    let shard_len = data_shards[0].len();
    if data_shards.iter().any(|s| s.len() != shard_len) {
        return Err("unequal shard lengths".into());
    }
    let rs = ReedSolomon::new(k, m).map_err(|e| format!("reed-solomon init: {e}"))?;
    let mut shards: Vec<Vec<u8>> = data_shards.to_vec();
    for _ in 0..m {
        shards.push(vec![0u8; shard_len]);
    }
    let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| s.as_mut_slice()).collect();
    rs.encode(&mut refs)
        .map_err(|e| format!("reed-solomon encode: {e}"))?;
    Ok(shards)
}

/// Build wire datagrams for one encoded AU (data + FEC).
pub fn build_frame_datagrams(
    data: &[u8],
    frame_seq: u64,
    pts_us: i64,
    codec: u8,
    damage_full: bool,
    max_datagram: usize,
    loss_ratio: f64,
) -> Result<Vec<Vec<u8>>, String> {
    let budget = datagram_payload_budget(max_datagram);
    let data_shards = shard_payload(data, budget);
    let k = data_shards.len();
    let m = fec_parity_count(k, loss_ratio);
    let all = apply_fec(&data_shards, m)?;
    let mut out = Vec::with_capacity(all.len());
    for (i, shard) in all.iter().enumerate() {
        let mut flags = 0u8;
        if damage_full {
            flags |= FLAG_DAMAGE_FULL;
        }
        if i >= k {
            flags |= FLAG_FEC;
        }
        let header = DatagramHeader {
            version: RUDP_VIDEO_DATAGRAM_VERSION,
            flags,
            codec,
            frame_seq,
            pts_us,
            shard_i: i as u16,
            shard_n: k as u16,
            fec_m: m as u16,
            payload_len: shard.len() as u16,
        };
        let mut packet = Vec::with_capacity(RUDP_DATAGRAM_HEADER_LEN + shard.len());
        header.encode(&mut packet);
        packet.extend_from_slice(shard);
        out.push(packet);
    }
    Ok(out)
}

/// Reassembled access unit from datagrams (after optional FEC recover).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReassembledFrame {
    pub frame_seq: u64,
    pub pts_us: i64,
    pub codec: u8,
    pub damage_full: bool,
    pub data: Vec<u8>,
}

/// Collect shards for one `frame_seq` and recover payload.
///
/// `slots[i]` is `Some(payload)` when shard `i` (0..k+m) arrived.
/// Payloads are expected to be the wrapped form from [`wrap_au_payload`].
pub fn try_reassemble(
    frame_seq: u64,
    pts_us: i64,
    codec: u8,
    damage_full: bool,
    shard_n: usize,
    fec_m: usize,
    slots: &[Option<Vec<u8>>],
) -> Result<Option<ReassembledFrame>, String> {
    let total = shard_n.saturating_add(fec_m);
    if shard_n == 0 || slots.len() < total {
        return Ok(None);
    }
    let present: usize = slots.iter().take(total).filter(|s| s.is_some()).count();
    if present < shard_n {
        return Ok(None);
    }

    let shard_len = slots
        .iter()
        .take(total)
        .find_map(|s| s.as_ref().map(|v| v.len()))
        .ok_or_else(|| "no shard payloads".to_string())?;

    let mut working: Vec<Option<Vec<u8>>> = (0..total)
        .map(|i| {
            slots.get(i).and_then(|s| {
                s.as_ref().map(|v| {
                    let mut copy = v.clone();
                    if copy.len() < shard_len {
                        copy.resize(shard_len, 0);
                    } else if copy.len() > shard_len {
                        copy.truncate(shard_len);
                    }
                    copy
                })
            })
        })
        .collect();

    if fec_m > 0 && working.iter().take(shard_n).any(Option::is_none) {
        let rs = ReedSolomon::new(shard_n, fec_m).map_err(|e| format!("reed-solomon init: {e}"))?;
        rs.reconstruct(&mut working)
            .map_err(|e| format!("reed-solomon reconstruct: {e}"))?;
    }

    let mut data = Vec::with_capacity(shard_n * shard_len);
    for (i, slot) in working.iter().enumerate().take(shard_n) {
        let shard = slot
            .as_ref()
            .ok_or_else(|| format!("missing data shard {i} after recover"))?;
        data.extend_from_slice(shard);
    }

    Ok(Some(ReassembledFrame {
        frame_seq,
        pts_us,
        codec,
        damage_full,
        data,
    }))
}

/// Prefix AU bytes with `u32` LE length so reassembly can trim padding.
pub fn wrap_au_payload(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

pub fn unwrap_au_payload(buf: &[u8]) -> Result<Vec<u8>, String> {
    if buf.len() < 4 {
        return Err("au payload too short".into());
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return Err("au payload truncated".into());
    }
    Ok(buf[4..4 + len].to_vec())
}

/// Reliable access-unit on the video uni-stream (length-prefixed body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReliableAccessUnit {
    pub frame_seq: u64,
    pub pts_us: i64,
    pub codec: u8,
    pub damage_full: bool,
    pub width: u32,
    pub height: u32,
    pub damage: Vec<RudpDamageRect>,
    pub data: Vec<u8>,
}

impl ReliableAccessUnit {
    pub fn encode_body(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(RUDP_VIDEO_AU_VERSION);
        let mut flags = FLAG_KEY;
        if self.damage_full {
            flags |= FLAG_DAMAGE_FULL;
        }
        body.push(flags);
        body.push(self.codec);
        body.push(0);
        body.extend_from_slice(&self.frame_seq.to_le_bytes());
        body.extend_from_slice(&self.pts_us.to_le_bytes());
        body.extend_from_slice(&self.width.to_le_bytes());
        body.extend_from_slice(&self.height.to_le_bytes());
        let n = self.damage.len().min(32) as u16;
        body.extend_from_slice(&n.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        for r in self.damage.iter().take(32) {
            body.extend_from_slice(&r.x.to_le_bytes());
            body.extend_from_slice(&r.y.to_le_bytes());
            body.extend_from_slice(&r.w.to_le_bytes());
            body.extend_from_slice(&r.h.to_le_bytes());
        }
        body.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        body.extend_from_slice(&self.data);
        body
    }

    pub fn encode_framed(&self) -> Vec<u8> {
        let body = self.encode_body();
        let mut out = Vec::with_capacity(4 + body.len());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn decode_body(body: &[u8]) -> Result<Self, String> {
        if body.len() < 32 {
            return Err("AU body truncated".into());
        }
        if body[0] != RUDP_VIDEO_AU_VERSION {
            return Err(format!("unsupported AU version {}", body[0]));
        }
        let flags = body[1];
        let codec = body[2];
        let frame_seq = u64::from_le_bytes(body[4..12].try_into().unwrap_or([0; 8]));
        let pts_us = i64::from_le_bytes(body[12..20].try_into().unwrap_or([0; 8]));
        let width = u32::from_le_bytes(body[20..24].try_into().unwrap_or([0; 4]));
        let height = u32::from_le_bytes(body[24..28].try_into().unwrap_or([0; 4]));
        let damage_count = u16::from_le_bytes([body[28], body[29]]) as usize;
        let mut offset = 32;
        let mut damage = Vec::with_capacity(damage_count.min(32));
        for _ in 0..damage_count.min(32) {
            if body.len() < offset + 16 {
                return Err("AU damage truncated".into());
            }
            damage.push(RudpDamageRect {
                x: i32::from_le_bytes(body[offset..offset + 4].try_into().unwrap_or([0; 4])),
                y: i32::from_le_bytes(body[offset + 4..offset + 8].try_into().unwrap_or([0; 4])),
                w: i32::from_le_bytes(body[offset + 8..offset + 12].try_into().unwrap_or([0; 4])),
                h: i32::from_le_bytes(body[offset + 12..offset + 16].try_into().unwrap_or([0; 4])),
            });
            offset += 16;
        }
        if body.len() < offset + 4 {
            return Err("AU data len truncated".into());
        }
        let data_len =
            u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
        offset += 4;
        if body.len() < offset + data_len {
            return Err("AU data truncated".into());
        }
        Ok(Self {
            frame_seq,
            pts_us,
            codec,
            damage_full: (flags & FLAG_DAMAGE_FULL) != 0,
            width,
            height,
            damage,
            data: body[offset..offset + data_len].to_vec(),
        })
    }

    /// Decode one length-prefixed AU from a buffer; returns bytes consumed.
    pub fn try_decode_framed(buf: &[u8]) -> Result<Option<(Self, usize)>, String> {
        if buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if len > 16 * 1024 * 1024 {
            return Err("AU frame too large".into());
        }
        if buf.len() < 4 + len {
            return Ok(None);
        }
        let au = Self::decode_body(&buf[4..4 + len])?;
        Ok(Some((au, 4 + len)))
    }
}

pub fn codec_from_str(s: &str) -> u8 {
    match s {
        "h264" => CODEC_H264,
        _ => CODEC_HEVC,
    }
}

pub fn codec_to_str(c: u8) -> &'static str {
    match c {
        CODEC_H264 => "h264",
        _ => "hevc",
    }
}

/// Build datagrams with length-prefixed AU so reassembly can trim pad.
pub fn build_media_datagrams(
    au_data: &[u8],
    frame_seq: u64,
    pts_us: i64,
    codec: u8,
    damage_full: bool,
    max_datagram: usize,
    loss_ratio: f64,
) -> Result<Vec<Vec<u8>>, String> {
    let wrapped = wrap_au_payload(au_data);
    build_frame_datagrams(
        &wrapped,
        frame_seq,
        pts_us,
        codec,
        damage_full,
        max_datagram,
        loss_ratio,
    )
}

/// Reassemble and unwrap length-prefixed AU payload.
pub fn try_reassemble_media(
    frame_seq: u64,
    pts_us: i64,
    codec: u8,
    damage_full: bool,
    shard_n: usize,
    fec_m: usize,
    slots: &[Option<Vec<u8>>],
) -> Result<Option<ReassembledFrame>, String> {
    let Some(mut frame) =
        try_reassemble(frame_seq, pts_us, codec, damage_full, shard_n, fec_m, slots)?
    else {
        return Ok(None);
    };
    frame.data = unwrap_au_payload(&frame.data)?;
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_and_fec_roundtrip_with_loss() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let dgrams =
            build_media_datagrams(&data, 42, 1000, CODEC_HEVC, false, 1200, 0.0).expect("build");
        assert!(dgrams.len() > 1);

        let (hdr0, _) = DatagramHeader::decode(&dgrams[0]).expect("hdr");
        let k = hdr0.shard_n as usize;
        let m = hdr0.fec_m as usize;
        let mut slots: Vec<Option<Vec<u8>>> = vec![None; k + m];
        // Drop one data shard; keep parities.
        for (i, d) in dgrams.iter().enumerate() {
            let (h, total) = DatagramHeader::decode(d).expect("d");
            assert_eq!(h.frame_seq, 42);
            if i == 0 {
                continue; // lose first data shard
            }
            slots[h.shard_i as usize] = Some(d[RUDP_DATAGRAM_HEADER_LEN..total].to_vec());
        }
        let got = try_reassemble_media(42, 1000, CODEC_HEVC, false, k, m, &slots)
            .expect("re")
            .expect("complete");
        assert_eq!(got.data, data);
    }

    #[test]
    fn reliable_au_roundtrip() {
        let au = ReliableAccessUnit {
            frame_seq: 7,
            pts_us: 12345,
            codec: CODEC_H264,
            damage_full: true,
            width: 1920,
            height: 1080,
            damage: vec![RudpDamageRect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }],
            data: vec![9, 8, 7, 6],
        };
        let framed = au.encode_framed();
        let (decoded, n) = ReliableAccessUnit::try_decode_framed(&framed)
            .expect("dec")
            .expect("complete");
        assert_eq!(n, framed.len());
        assert_eq!(decoded, au);
    }

    #[test]
    fn fec_parity_policy() {
        assert_eq!(fec_parity_count(1, 0.0), 1);
        assert_eq!(fec_parity_count(10, 0.0), 1);
        assert_eq!(fec_parity_count(40, 0.0), 4);
        assert_eq!(fec_parity_count(10, 0.1), 3);
    }
}
