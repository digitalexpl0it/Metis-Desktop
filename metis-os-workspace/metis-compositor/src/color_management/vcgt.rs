//! Parser for the ICC `vcgt` (Video Card Gamma Table) tag.
//!
//! `vcgt` is the per-channel calibration ramp that display-profiling tools
//! (DisplayCAL, ArgyllCMS, colord) bake into an `.icc` profile. It is exactly the
//! correction curve that belongs in the GPU's per-CRTC gamma ramp, so it is what
//! [`crate::output_gamma`] uploads for hardware colour calibration.
//!
//! This is a self-contained, dependency-free, bounds-checked parser: it never
//! panics on malformed input (returns [`VcgtError`]) and returns `Ok(None)` when
//! the profile simply has no `vcgt` tag (full gamut correction via a 3D LUT is a
//! later stage). All ICC integers are big-endian.

use std::ops::Range;

/// Per-channel gamma ramps parsed from a `vcgt` tag. Values are 16-bit
/// (`0..=65535`) regardless of the tag's native entry size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GammaRamps {
    pub r: Vec<u16>,
    pub g: Vec<u16>,
    pub b: Vec<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum VcgtError {
    #[error("ICC data too small for the header + tag table")]
    Truncated,
    #[error("vcgt tag data is malformed or truncated")]
    MalformedTag,
    #[error("unsupported vcgt table encoding (channels={channels}, entry_size={entry_size})")]
    Unsupported { channels: u16, entry_size: u16 },
    #[error("unknown vcgt gamma type {0}")]
    UnknownType(u32),
}

/// Number of samples synthesised for a formula-type `vcgt` before resampling to
/// the CRTC's gamma length.
const FORMULA_SAMPLES: usize = 256;

/// Parse the `vcgt` tag from a full ICC profile.
///
/// - `Ok(Some(ramps))` — a usable calibration ramp was found.
/// - `Ok(None)` — the profile is well-formed but carries no `vcgt` tag.
/// - `Err(_)` — the ICC header/tag table or the `vcgt` tag is malformed.
pub fn parse_vcgt(icc: &[u8]) -> Result<Option<GammaRamps>, VcgtError> {
    match find_vcgt_tag(icc)? {
        Some(range) => parse_vcgt_tag(&icc[range]).map(Some),
        None => Ok(None),
    }
}

/// Locate the `vcgt` tag's byte range within the ICC tag table.
fn find_vcgt_tag(icc: &[u8]) -> Result<Option<Range<usize>>, VcgtError> {
    // 128-byte header, then a u32 tag count at offset 128.
    if icc.len() < 132 {
        return Err(VcgtError::Truncated);
    }
    let count = u32_at(icc, 128).map_err(|_| VcgtError::Truncated)? as usize;
    let table_end = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(132))
        .ok_or(VcgtError::Truncated)?;
    if icc.len() < table_end {
        return Err(VcgtError::Truncated);
    }
    for i in 0..count {
        let base = 132 + i * 12;
        if &icc[base..base + 4] == b"vcgt" {
            let off = u32_at(icc, base + 4)? as usize;
            let size = u32_at(icc, base + 8)? as usize;
            let end = off.checked_add(size).ok_or(VcgtError::MalformedTag)?;
            // A vcgt tag has at least the 12-byte type header.
            if size < 12 || end > icc.len() {
                return Err(VcgtError::MalformedTag);
            }
            return Ok(Some(off..end));
        }
    }
    Ok(None)
}

fn parse_vcgt_tag(tag: &[u8]) -> Result<GammaRamps, VcgtError> {
    // tag[0..4] = 'vcgt' type signature, tag[4..8] reserved, tag[8..12] = gamma type.
    if tag.len() < 12 || &tag[0..4] != b"vcgt" {
        return Err(VcgtError::MalformedTag);
    }
    match u32_at(tag, 8)? {
        0 => parse_table(tag),
        1 => parse_formula(tag),
        other => Err(VcgtError::UnknownType(other)),
    }
}

/// Table encoding: channels (u16), entries-per-channel (u16), bytes-per-entry
/// (u16), then `channels * entries * entry_size` bytes in R, G, B channel order.
fn parse_table(tag: &[u8]) -> Result<GammaRamps, VcgtError> {
    let channels = u16_at(tag, 12)?;
    let entry_count = u16_at(tag, 14)? as usize;
    let entry_size = u16_at(tag, 16)?;
    if entry_count == 0 || !matches!(channels, 1 | 3) || !matches!(entry_size, 1 | 2) {
        return Err(VcgtError::Unsupported {
            channels,
            entry_size,
        });
    }
    const DATA_START: usize = 18;
    let total = (channels as usize)
        .checked_mul(entry_count)
        .and_then(|n| n.checked_mul(entry_size as usize))
        .ok_or(VcgtError::MalformedTag)?;
    if tag.len() < DATA_START + total {
        return Err(VcgtError::MalformedTag);
    }

    let read_channel = |ch: usize| -> Result<Vec<u16>, VcgtError> {
        let mut out = Vec::with_capacity(entry_count);
        for e in 0..entry_count {
            let pos = DATA_START + (ch * entry_count + e) * entry_size as usize;
            let val = if entry_size == 1 {
                // Scale 8-bit to 16-bit so the full range maps (0->0, 255->65535).
                u16::from(*tag.get(pos).ok_or(VcgtError::MalformedTag)?) * 257
            } else {
                u16_at(tag, pos)?
            };
            out.push(val);
        }
        Ok(out)
    };

    if channels == 1 {
        let r = read_channel(0)?;
        Ok(GammaRamps {
            g: r.clone(),
            b: r.clone(),
            r,
        })
    } else {
        Ok(GammaRamps {
            r: read_channel(0)?,
            g: read_channel(1)?,
            b: read_channel(2)?,
        })
    }
}

/// Formula encoding: nine `s15Fixed16` values — (gamma, min, max) per channel —
/// sampled into a ramp: `out = min + (max - min) * (x ^ gamma)`.
fn parse_formula(tag: &[u8]) -> Result<GammaRamps, VcgtError> {
    let mut v = [0f64; 9];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = s15f16_at(tag, 12 + i * 4)?;
    }
    let [
        r_gamma,
        r_min,
        r_max,
        g_gamma,
        g_min,
        g_max,
        b_gamma,
        b_min,
        b_max,
    ] = v;
    Ok(GammaRamps {
        r: formula_ramp(r_gamma, r_min, r_max),
        g: formula_ramp(g_gamma, g_min, g_max),
        b: formula_ramp(b_gamma, b_min, b_max),
    })
}

fn formula_ramp(gamma: f64, min: f64, max: f64) -> Vec<u16> {
    (0..FORMULA_SAMPLES)
        .map(|i| {
            let x = i as f64 / (FORMULA_SAMPLES as f64 - 1.0);
            // A non-positive gamma is meaningless; treat it as linear.
            let curved = if gamma > 0.0 { x.powf(gamma) } else { x };
            let y = (min + (max - min) * curved).clamp(0.0, 1.0);
            (y * 65535.0).round() as u16
        })
        .collect()
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, VcgtError> {
    let b = d.get(at..at + 2).ok_or(VcgtError::MalformedTag)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, VcgtError> {
    let b = d.get(at..at + 4).ok_or(VcgtError::MalformedTag)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn s15f16_at(d: &[u8], at: usize) -> Result<f64, VcgtError> {
    Ok(u32_at(d, at)? as i32 as f64 / 65536.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal ICC blob carrying a single tag (`sig`) whose data is
    /// `tag_data`, placed right after the tag table.
    fn icc_with_tag(sig: &[u8; 4], tag_data: &[u8]) -> Vec<u8> {
        let tag_count = 1u32;
        let data_off = 132 + 12; // header + count + one 12-byte tag entry
        let mut buf = vec![0u8; data_off];
        buf[128..132].copy_from_slice(&tag_count.to_be_bytes());
        buf[132..136].copy_from_slice(sig);
        buf[136..140].copy_from_slice(&(data_off as u32).to_be_bytes());
        buf[140..144].copy_from_slice(&(tag_data.len() as u32).to_be_bytes());
        buf.extend_from_slice(tag_data);
        buf
    }

    fn s15f16(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32 as u32).to_be_bytes()
    }

    #[test]
    fn parses_16bit_table() {
        // channels=3, entries=2, entry_size=2; R=[0,65535] G=[0,32768] B=[0,65535].
        let mut data = Vec::new();
        data.extend_from_slice(b"vcgt");
        data.extend_from_slice(&[0, 0, 0, 0]); // reserved
        data.extend_from_slice(&0u32.to_be_bytes()); // type = table
        data.extend_from_slice(&3u16.to_be_bytes()); // channels
        data.extend_from_slice(&2u16.to_be_bytes()); // entries
        data.extend_from_slice(&2u16.to_be_bytes()); // entry size
        for v in [0u16, 65535, 0, 32768, 0, 65535] {
            data.extend_from_slice(&v.to_be_bytes());
        }
        let icc = icc_with_tag(b"vcgt", &data);
        let ramps = parse_vcgt(&icc).unwrap().expect("vcgt present");
        assert_eq!(ramps.r, vec![0, 65535]);
        assert_eq!(ramps.g, vec![0, 32768]);
        assert_eq!(ramps.b, vec![0, 65535]);
    }

    #[test]
    fn parses_8bit_single_channel_table() {
        // channels=1 replicates the same ramp to all three outputs.
        let mut data = Vec::new();
        data.extend_from_slice(b"vcgt");
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes()); // channels
        data.extend_from_slice(&3u16.to_be_bytes()); // entries
        data.extend_from_slice(&1u16.to_be_bytes()); // entry size (u8)
        data.extend_from_slice(&[0u8, 128, 255]);
        let icc = icc_with_tag(b"vcgt", &data);
        let ramps = parse_vcgt(&icc).unwrap().expect("vcgt present");
        assert_eq!(ramps.r, vec![0, 128 * 257, 65535]);
        assert_eq!(ramps.r, ramps.g);
        assert_eq!(ramps.g, ramps.b);
    }

    #[test]
    fn parses_linear_formula() {
        // gamma=1, min=0, max=1 for all channels -> identity ramp.
        let mut data = Vec::new();
        data.extend_from_slice(b"vcgt");
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&1u32.to_be_bytes()); // type = formula
        for _ in 0..3 {
            data.extend_from_slice(&s15f16(1.0)); // gamma
            data.extend_from_slice(&s15f16(0.0)); // min
            data.extend_from_slice(&s15f16(1.0)); // max
        }
        let icc = icc_with_tag(b"vcgt", &data);
        let ramps = parse_vcgt(&icc).unwrap().expect("vcgt present");
        assert_eq!(ramps.r.len(), FORMULA_SAMPLES);
        assert_eq!(ramps.r.first(), Some(&0));
        assert_eq!(ramps.r.last(), Some(&65535));
        // Midpoint of a linear ramp is ~50%.
        let mid = ramps.r[FORMULA_SAMPLES / 2];
        assert!((mid as i32 - 32768).abs() < 200, "mid={mid}");
    }

    #[test]
    fn no_vcgt_tag_is_none() {
        let icc = icc_with_tag(b"desc", &[0u8; 16]);
        assert_eq!(parse_vcgt(&icc).unwrap(), None);
    }

    #[test]
    fn truncated_icc_errors() {
        assert!(matches!(parse_vcgt(&[0u8; 8]), Err(VcgtError::Truncated)));
    }

    #[test]
    fn malformed_tag_offset_errors() {
        // Tag table claims a vcgt tag whose data runs past the end of the blob.
        let mut buf = vec![0u8; 132 + 12];
        buf[128..132].copy_from_slice(&1u32.to_be_bytes());
        buf[132..136].copy_from_slice(b"vcgt");
        buf[136..140].copy_from_slice(&1_000_000u32.to_be_bytes()); // bogus offset
        buf[140..144].copy_from_slice(&64u32.to_be_bytes());
        assert!(matches!(parse_vcgt(&buf), Err(VcgtError::MalformedTag)));
    }

    #[test]
    fn does_not_panic_on_random_bytes() {
        // Fuzz-ish: never panic, whatever the bytes say.
        for len in [0usize, 1, 64, 131, 132, 200] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 % 256) as u8).collect();
            let _ = parse_vcgt(&bytes);
        }
    }
}
