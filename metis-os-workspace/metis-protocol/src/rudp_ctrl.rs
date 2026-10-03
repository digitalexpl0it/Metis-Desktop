//! Metis Remote (RUDP) control-plane messages (Quinn reliable stream).
//!
//! Framing: big-endian `u32` length + UTF-8 JSON body (max 64 KiB).

use serde::{Deserialize, Serialize};

pub const RUDP_PROTOCOL_VERSION: u32 = 2;
pub const RUDP_MAX_FRAME: usize = 64 * 1024;
/// Max UTF-8 bytes in a [`RudpControlMsg::ClipboardSet`] text payload so the
/// JSON frame stays under [`RUDP_MAX_FRAME`].
pub const RUDP_CLIPBOARD_MAX_BYTES: usize = 48 * 1024;

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
    /// Host advertises encode dimensions + codec after SessionOk (Phase 6).
    VideoReady {
        width: u32,
        height: u32,
        /// `"h264"` or `"hevc"`.
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
        roundtrip(RudpControlMsg::VideoReady {
            width: 1920,
            height: 1080,
            codec: "hevc".into(),
        });
        roundtrip(RudpControlMsg::ClipboardSet {
            mime: "text/plain;charset=utf-8".into(),
            text: "hello clipboard".into(),
            serial: 7,
        });
    }

    #[test]
    fn truncate_clipboard_respects_char_boundary() {
        let s = "é".repeat(RUDP_CLIPBOARD_MAX_BYTES);
        let t = truncate_clipboard_text(&s);
        assert!(t.len() <= RUDP_CLIPBOARD_MAX_BYTES);
        assert!(t.is_char_boundary(t.len()));
    }
}
