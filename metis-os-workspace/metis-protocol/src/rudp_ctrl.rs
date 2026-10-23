//! Metis Remote (RUDP) control-plane messages (Quinn reliable stream).
//!
//! Framing: big-endian `u32` length + UTF-8 JSON body (max 64 KiB).

use serde::{Deserialize, Serialize};

/// Protocol version 5: mic uplink (`MicEnable` / `MicReady` + `MRUM` datagrams).
/// Hosts accept Hello protocol in
/// [`RUDP_PROTOCOL_MIN`]..=[`RUDP_PROTOCOL_VERSION`]. Mic control is gated on
/// Hello ≥ 5 (older clients never see mic messages).
pub const RUDP_PROTOCOL_VERSION: u32 = 5;
/// Oldest Hello protocol this host still accepts (v3 multi-monitor clients).
pub const RUDP_PROTOCOL_MIN: u32 = 3;
pub const RUDP_MAX_FRAME: usize = 64 * 1024;
/// Max UTF-8 bytes in a [`RudpControlMsg::ClipboardSet`] text payload so the
/// JSON frame stays under [`RUDP_MAX_FRAME`].
pub const RUDP_CLIPBOARD_MAX_BYTES: usize = 48 * 1024;
/// Cap concurrent subscribed outputs per session (encode cost bound).
pub const RUDP_MAX_SUBSCRIBED_OUTPUTS: usize = 4;

/// One monitor in a [`RudpControlMsg::DesktopLayout`] advertisement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RudpMonitorInfo {
    /// Stable id (output name / connector), also used in [`RudpControlMsg::SubscribeOutputs`].
    pub id: String,
    /// Human-readable name (may equal `id`).
    pub name: String,
    /// Logical desktop origin X.
    pub x: i32,
    /// Logical desktop origin Y.
    pub y: i32,
    /// Logical width in pixels.
    pub width: u32,
    /// Logical height in pixels.
    pub height: u32,
    /// Output scale factor.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Whether this is the primary output.
    #[serde(default)]
    pub primary: bool,
}

fn default_scale() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RudpControlMsg {
    Hello {
        protocol: u32,
        username: String,
    },
    AuthChallenge {
        nonce: String,
    },
    AuthResponse {
        password: String,
    },
    SessionOk {
        session_id: String,
    },
    Reject {
        reason: RudpRejectReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Keepalive,
    /// Host advertises the virtual desktop topology after SessionOk (and on change).
    DesktopLayout {
        monitors: Vec<RudpMonitorInfo>,
    },
    /// Host advertises encode dimensions + codec for one output stream.
    VideoReady {
        width: u32,
        height: u32,
        /// `"h264"`, `"hevc"`, or `"av1"`.
        codec: String,
        /// Demux key for datagrams / reliable AUs (0..=255).
        stream_id: u8,
        /// Output id from [`DesktopLayout`] / [`RudpMonitorInfo::id`].
        output_id: String,
    },
    /// Client → host: which outputs to encode (empty → primary only). Cap
    /// [`RUDP_MAX_SUBSCRIBED_OUTPUTS`].
    SubscribeOutputs {
        output_ids: Vec<String>,
    },
    /// Host advertises system-audio stream parameters (host → client Opus).
    AudioReady {
        sample_rate: u32,
        channels: u8,
        /// `"opus"`.
        codec: String,
    },
    /// Client → host: absolute pointer in compositor logical desktop coords.
    PointerAbsolute {
        x: f64,
        y: f64,
    },
    /// Client → host: relative pointer delta (logical pixels).
    PointerRelative {
        dx: f64,
        dy: f64,
    },
    /// Client → host: pointer button (Linux evdev code, e.g. BTN_LEFT=0x110).
    PointerButton {
        button: u32,
        pressed: bool,
    },
    /// Client → host: scroll delta in logical pixels.
    PointerScroll {
        dx: f64,
        dy: f64,
    },
    /// Client → host: keyboard key (evdev keycode).
    Key {
        keycode: u32,
        pressed: bool,
    },
    /// Host → client: Wayland pointer lock active (prefer relative mouse).
    PointerLock {
        locked: bool,
    },
    /// Either direction: replace the peer clipboard with this UTF-8 text.
    /// Cap `text` with [`truncate_clipboard_text`] before send.
    ClipboardSet {
        /// e.g. `"text/plain;charset=utf-8"`.
        mime: String,
        text: String,
        /// Monotonic id so receivers can drop echoes of their own set.
        #[serde(default)]
        serial: u64,
    },
    /// Client → host (protocol ≥ 4): request encode quality preset.
    /// `quality` is `performance` / `high` / `medium` / `low`.
    SetStreamQuality {
        quality: String,
    },
    /// Host → client (protocol ≥ 4): current encode quality (after SessionOk
    /// and after any change from Settings or another client).
    StreamQuality {
        quality: String,
    },
    /// Client → host (protocol ≥ 5): request mic uplink on/off. Default off.
    /// Host may reply with [`Self::MicEnable`] `{ enabled: false }` when
    /// disallowed / locked, or with [`Self::MicReady`] when inject is armed.
    MicEnable {
        enabled: bool,
    },
    /// Host → client (protocol ≥ 5): Pulse inject path ready; client may send
    /// `MRUM` Opus datagrams at these parameters.
    MicReady {
        sample_rate: u32,
        channels: u8,
        /// `"opus"`.
        codec: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RudpRejectReason {
    NotAllowed,
    AuthFailed,
    Protocol,
    RateLimited,
}

impl RudpRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAllowed => "not_allowed",
            Self::AuthFailed => "auth_failed",
            Self::Protocol => "protocol",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// Truncate UTF-8 clipboard text to [`RUDP_CLIPBOARD_MAX_BYTES`] on a char boundary.
pub fn truncate_clipboard_text(text: &str) -> &str {
    if text.len() <= RUDP_CLIPBOARD_MAX_BYTES {
        return text;
    }
    let mut end = RUDP_CLIPBOARD_MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Encode one control message to length-prefixed bytes.
pub fn encode_rudp_frame(msg: &RudpControlMsg) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(msg).map_err(|e| format!("rudp encode: {e}"))?;
    if body.len() > RUDP_MAX_FRAME {
        return Err("rudp frame too large".into());
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode one control message from a complete length-prefixed buffer
/// (`len` BE u32 + body). Returns `None` if more bytes are needed.
pub fn try_decode_rudp_frame(buf: &[u8]) -> Result<Option<(RudpControlMsg, usize)>, String> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > RUDP_MAX_FRAME {
        return Err("rudp frame too large".into());
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let msg: RudpControlMsg =
        serde_json::from_slice(&buf[4..4 + len]).map_err(|e| format!("rudp decode: {e}"))?;
    Ok(Some((msg, 4 + len)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: RudpControlMsg) {
        let bytes = encode_rudp_frame(&msg).expect("enc");
        let (decoded, n) = try_decode_rudp_frame(&bytes)
            .expect("dec")
            .expect("complete");
        assert_eq!(n, bytes.len());
        assert_eq!(decoded, msg);
    }

    #[test]
    fn frame_roundtrip() {
        roundtrip(RudpControlMsg::Hello {
            protocol: RUDP_PROTOCOL_VERSION,
            username: "alice".into(),
        });
    }

    #[test]
    fn input_and_pointer_lock_roundtrip() {
        roundtrip(RudpControlMsg::PointerAbsolute {
            x: 100.5,
            y: 200.25,
        });
        roundtrip(RudpControlMsg::PointerRelative { dx: -1.5, dy: 2.0 });
        roundtrip(RudpControlMsg::PointerButton {
            button: 0x110,
            pressed: true,
        });
        roundtrip(RudpControlMsg::PointerScroll { dx: 0.0, dy: -30.0 });
        roundtrip(RudpControlMsg::Key {
            keycode: 30,
            pressed: false,
        });
        roundtrip(RudpControlMsg::PointerLock { locked: true });
        roundtrip(RudpControlMsg::DesktopLayout {
            monitors: vec![RudpMonitorInfo {
                id: "DP-1".into(),
                name: "DP-1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
                primary: true,
            }],
        });
        roundtrip(RudpControlMsg::VideoReady {
            width: 1920,
            height: 1080,
            codec: "hevc".into(),
            stream_id: 0,
            output_id: "DP-1".into(),
        });
        roundtrip(RudpControlMsg::SubscribeOutputs {
            output_ids: vec!["DP-1".into(), "HDMI-A-1".into()],
        });
        roundtrip(RudpControlMsg::AudioReady {
            sample_rate: 48_000,
            channels: 2,
            codec: "opus".into(),
        });
        roundtrip(RudpControlMsg::SetStreamQuality {
            quality: "performance".into(),
        });
        roundtrip(RudpControlMsg::StreamQuality {
            quality: "high".into(),
        });
        roundtrip(RudpControlMsg::ClipboardSet {
            mime: "text/plain;charset=utf-8".into(),
            text: "hello clipboard".into(),
            serial: 7,
        });
        roundtrip(RudpControlMsg::MicEnable { enabled: true });
        roundtrip(RudpControlMsg::MicReady {
            sample_rate: 48_000,
            channels: 1,
            codec: "opus".into(),
        });
    }

    #[test]
    fn truncate_clipboard_respects_char_boundary() {
        let s = "é".repeat(RUDP_CLIPBOARD_MAX_BYTES);
        let t = truncate_clipboard_text(&s);
        assert!(t.len() <= RUDP_CLIPBOARD_MAX_BYTES);
        assert!(t.is_char_boundary(t.len()));
    }

    /// CI-safe fuzz smoke (Phase 23 §E) — never panics on garbage.
    #[test]
    fn decode_garbage_does_not_panic() {
        for sample in [
            &[][..],
            &[0u8; 8],
            b"not a frame",
            &[0xff, 0xff, 0xff, 0xff, 1, 2, 3, 4],
        ] {
            let _ = try_decode_rudp_frame(sample);
        }
    }
}
