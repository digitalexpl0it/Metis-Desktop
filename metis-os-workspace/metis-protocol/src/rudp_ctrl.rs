//! Metis Remote (RUDP) control-plane messages (Quinn reliable stream).
//!
//! Framing: big-endian `u32` length + UTF-8 JSON body (max 64 KiB).

use serde::{Deserialize, Serialize};

pub const RUDP_PROTOCOL_VERSION: u32 = 1;
pub const RUDP_MAX_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    #[test]
    fn frame_roundtrip() {
        let msg = RudpControlMsg::Hello {
            protocol: RUDP_PROTOCOL_VERSION,
            username: "alice".into(),
        };
        let bytes = encode_rudp_frame(&msg).expect("enc");
        let (decoded, n) = try_decode_rudp_frame(&bytes)
            .expect("dec")
            .expect("complete");
        assert_eq!(n, bytes.len());
        assert_eq!(decoded, msg);
    }
}
