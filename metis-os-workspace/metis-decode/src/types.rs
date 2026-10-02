//! Public decode types (no FFmpeg in the API surface).

use thiserror::Error;

pub use metis_encode::RudpCodec;

#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Packed RGBA8 (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts_us: i64,
}

#[derive(Debug, Clone)]
pub struct DecoderInfo {
    pub codec: RudpCodec,
    pub decoder_name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("decoder unavailable: {0}")]
    Unavailable(String),
    #[error("invalid decode input: {0}")]
    InvalidInput(String),
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
}

pub type DecodeResult<T> = Result<T, DecodeError>;
