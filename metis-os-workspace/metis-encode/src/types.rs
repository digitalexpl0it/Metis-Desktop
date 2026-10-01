//! Public encode types (no FFmpeg / Smithay in the API surface).

use std::os::fd::BorrowedFd;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_BITRATE_KBPS: u32 = 25_000;
pub const DEFAULT_FPS_HINT: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EncoderBackend {
    #[default]
    Auto,
    Vaapi,
    Nvenc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RudpCodec {
    #[default]
    Hevc,
    H264,
}

impl RudpCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hevc => "hevc",
            Self::H264 => "h264",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncoderConfig {
    pub backend: EncoderBackend,
    pub codec: RudpCodec,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps_hint: u32,
    #[serde(default = "default_bitrate")]
    pub bitrate_kbps: u32,
}

fn default_fps() -> u32 {
    DEFAULT_FPS_HINT
}

fn default_bitrate() -> u32 {
    DEFAULT_BITRATE_KBPS
}

impl EncoderConfig {
    pub fn sanitize(mut self) -> Self {
        if self.width == 0 {
            self.width = 1;
        }
        if self.height == 0 {
            self.height = 1;
        }
        if self.fps_hint == 0 {
            self.fps_hint = DEFAULT_FPS_HINT;
        }
        if self.bitrate_kbps < 500 {
            self.bitrate_kbps = DEFAULT_BITRATE_KBPS;
        }
        self
    }
}

/// One composited frame ready for hardware encode (dmabuf planes, no mmap).
pub struct EncodeInput<'a> {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub fds: &'a [BorrowedFd<'a>],
    pub offsets: &'a [u32],
    pub strides: &'a [u32],
}

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub seq: u64,
    pub codec: RudpCodec,
    pub is_keyframe: bool,
    pub data: Vec<u8>,
    pub pts_us: i64,
}

#[derive(Debug, Clone)]
pub struct EncoderInfo {
    pub backend: EncoderBackend,
    pub codec: RudpCodec,
    pub encoder_name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("encoder unavailable: {0}")]
    Unavailable(String),
    #[error("invalid encode input: {0}")]
    InvalidInput(String),
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type EncodeResult<T> = Result<T, EncodeError>;
