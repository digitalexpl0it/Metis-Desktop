//! Test / CI encoder that accepts frames and emits empty keyframe markers.

use crate::HwEncoder;
use crate::types::{
    EncodeInput, EncodeResult, EncodedPacket, EncoderBackend, EncoderInfo, RudpCodec,
};

pub struct NullEncoder {
    info: EncoderInfo,
    pending: Vec<EncodedPacket>,
}

impl NullEncoder {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            info: EncoderInfo {
                backend: EncoderBackend::Auto,
                codec: RudpCodec::Hevc,
                encoder_name: "null".into(),
                width,
                height,
            },
            pending: Vec::new(),
        }
    }
}

impl HwEncoder for NullEncoder {
    fn info(&self) -> &EncoderInfo {
        &self.info
    }

    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()> {
        if frame.width == 0 || frame.height == 0 || frame.fds.is_empty() {
            return Err(crate::EncodeError::InvalidInput(
                "null encoder requires non-empty dmabuf planes".into(),
            ));
        }
        self.pending.push(EncodedPacket {
            seq: frame.seq,
            codec: self.info.codec,
            is_keyframe: true,
            data: Vec::new(),
            pts_us: frame.seq as i64 * 16_666,
        });
        Ok(())
    }

    fn drain(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        Ok(std::mem::take(&mut self.pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsFd, BorrowedFd};
    use std::os::unix::io::AsRawFd;

    #[test]
    fn null_encoder_roundtrip() {
        let mut enc = NullEncoder::new(64, 64);
        // Use stdin as a stand-in BorrowedFd (never read — only length checks).
        let stdin = std::io::stdin();
        let fd = stdin.as_fd();
        let fds: [BorrowedFd<'_>; 1] = [fd];
        let offsets = [0u32];
        let strides = [256u32];
        let input = EncodeInput {
            seq: 7,
            width: 64,
            height: 64,
            stride: 256,
            fourcc: 0,
            modifier: 0,
            fds: &fds,
            offsets: &offsets,
            strides: &strides,
        };
        enc.submit(&input).expect("submit");
        let pkts = enc.drain().expect("drain");
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0].seq, 7);
        assert!(pkts[0].is_keyframe);
        let _ = fd.as_raw_fd();
    }
}
