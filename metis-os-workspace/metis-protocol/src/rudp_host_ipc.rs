//! Compositor ↔ `metis-rudp-host` Unix-socket IPC (Phase 23 §D).
//!
//! The Metis Remote (RUDP) network edge — Quinn / TLS / PAM auth / datagram
//! fan-out — runs in its own `metis-rudp-host` process. The compositor keeps
//! frame capture + (isolated) encode and the calloop input inject path, and
//! talks to the host over a private Unix socket:
//!
//! - `$XDG_RUNTIME_DIR/metis/rudp-host.sock`, directory `0700`, socket `0600`,
//!   both ends verify the peer UID with `SO_PEERCRED`.
//! - Every message is one **frame**:
//!   `u32 LE header_len | u32 LE payload_len | header (UTF-8 JSON) | payload`.
//!   The JSON header carries the message; `payload` is a raw binary tail used
//!   only by [`CompToHost::EncodedVideo`] / [`CompToHost::EncodedAudio`] so
//!   encoded access units are never base64-inflated.
//!
//! The client-facing QUIC wire protocol is **unchanged** by this split.

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{RudpControlMsg, RudpDamageRect, RudpMonitorInfo};

/// Socket filename under `$XDG_RUNTIME_DIR/metis/`.
pub const RUDP_HOST_IPC_SOCK: &str = "rudp-host.sock";
/// Bumped on any incompatible change to the messages below.
pub const RUDP_HOST_IPC_VERSION: u32 = 1;
/// Upper bound for one JSON header (control-sized; hostile-peer guard).
pub const RUDP_HOST_IPC_MAX_HEADER: usize = 1024 * 1024;
/// Upper bound for one binary payload (one encoded access unit).
pub const RUDP_HOST_IPC_MAX_PAYLOAD: usize = 32 * 1024 * 1024;

/// Directory holding the IPC socket (`$XDG_RUNTIME_DIR/metis`, created `0700`
/// by the compositor).
pub fn rudp_host_ipc_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("metis")
}

/// Full path of the compositor ↔ host socket.
pub fn rudp_host_ipc_path() -> PathBuf {
    rudp_host_ipc_dir().join(RUDP_HOST_IPC_SOCK)
}

/// One output stream advertised to clients in `VideoReady`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcStreamInfo {
    pub stream_id: u8,
    pub output_id: String,
    pub width: u32,
    pub height: u32,
    /// `h264` / `hevc` / `av1` (see [`crate::codec_to_str`]).
    pub codec: String,
}

/// One encoded access unit. The bitstream itself travels as the frame
/// payload (not serialised into the JSON header).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpcEncodedVideo {
    pub stream_id: u8,
    pub output_id: String,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub pts_us: i64,
    /// `h264` / `hevc` / `av1`.
    pub codec: String,
    pub is_keyframe: bool,
    pub damage_full: bool,
    #[serde(default)]
    pub damage: Vec<RudpDamageRect>,
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// One pre-built audio datagram (`MRUA` header + Opus) for fan-out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpcEncodedAudio {
    pub seq: u32,
    pub pts_us: i64,
    #[serde(skip)]
    pub datagram: Vec<u8>,
}

/// Compositor → host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum CompToHost {
    /// First message after connect, re-sent whenever policy changes live
    /// (`ReloadRudp` with the same bind).
    Config {
        ipc_version: u32,
        /// Quinn bind address, e.g. `0.0.0.0:7843`.
        bind: String,
        allowed_users: Vec<String>,
        lan_only: bool,
        unlock_session_on_auth: bool,
    },
    /// Desktop topology; `generation` bumps on every change.
    DesktopLayout {
        generation: u64,
        monitors: Vec<RudpMonitorInfo>,
    },
    /// PAM / `ext-session-lock` owns the session — pause media.
    SessionLocked {
        locked: bool,
    },
    /// Wayland pointer lock active — clients switch to relative motion.
    PointerLocked {
        locked: bool,
    },
    /// Full snapshot of live output streams (replaces the previous one).
    StreamAnnounce {
        streams: Vec<IpcStreamInfo>,
    },
    EncodedVideo(IpcEncodedVideo),
    EncodedAudio(IpcEncodedAudio),
    /// System-audio capture state (gates `AudioReady`).
    AudioState {
        streaming: bool,
    },
    /// Latest-wins clipboard text for authenticated clients.
    ClipboardOut {
        serial: u64,
        mime: String,
        text: String,
    },
    /// Stop listening, close clients, exit.
    Shutdown,
}

/// Input / clipboard events forwarded from authenticated clients. Mirrors the
/// compositor's calloop `RudpInputEvent` (minus the compositor-only variants).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RudpHostInput {
    PointerAbsolute {
        x: f64,
        y: f64,
    },
    PointerRelative {
        dx: f64,
        dy: f64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
    },
    PointerScroll {
        dx: f64,
        dy: f64,
    },
    Key {
        keycode: u32,
        pressed: bool,
    },
    /// Viewer → host: install text on the Wayland clipboard.
    ClipboardSet {
        mime: String,
        text: String,
        serial: u64,
    },
}

impl RudpHostInput {
    /// Map a control-stream message onto an input event (`None` for
    /// everything that is not input).
    pub fn from_control(msg: RudpControlMsg) -> Option<Self> {
        match msg {
            RudpControlMsg::PointerAbsolute { x, y } => Some(Self::PointerAbsolute { x, y }),
            RudpControlMsg::PointerRelative { dx, dy } => Some(Self::PointerRelative { dx, dy }),
            RudpControlMsg::PointerButton { button, pressed } => {
                Some(Self::PointerButton { button, pressed })
            }
            RudpControlMsg::PointerScroll { dx, dy } => Some(Self::PointerScroll { dx, dy }),
            RudpControlMsg::Key { keycode, pressed } => Some(Self::Key { keycode, pressed }),
            RudpControlMsg::ClipboardSet { mime, text, serial } => {
                Some(Self::ClipboardSet { mime, text, serial })
            }
            _ => None,
        }
    }
}

/// Host → compositor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum HostToComp {
    /// Authenticated session count (export / encode demand) and a monotonic
    /// join counter — a change makes the frame thread emit an IDR everywhere.
    SessionDemand {
        active_sessions: u32,
        session_joins: u64,
    },
    /// Union of all sessions' output subscriptions (layout order). `gained`
    /// is set when a session started receiving an output (needs an IDR).
    SubscribeUnion {
        output_ids: Vec<String>,
        gained: bool,
    },
    Input {
        event: RudpHostInput,
    },
    /// Owner authenticated over RUDP and `unlock_session_on_auth` is on.
    UnlockPamSession {
        username: String,
    },
    /// A client joined / the encoder restarted — repaint + full frame.
    RefreshVideo,
    /// Listener state for Settings / logs. `error` is set when the host could
    /// not bind or load its TLS identity.
    Status {
        listening: bool,
        fingerprint: String,
        bind: String,
        error: Option<String>,
    },
}

/// A message that may carry a raw binary payload after its JSON header.
pub trait RudpHostIpcMessage: Serialize + DeserializeOwned {
    /// Binary tail written after the JSON header (empty for most messages).
    fn payload(&self) -> &[u8] {
        &[]
    }
    /// Attach the received tail to a freshly decoded message.
    fn set_payload(&mut self, _payload: Vec<u8>) {}
}

impl RudpHostIpcMessage for CompToHost {
    fn payload(&self) -> &[u8] {
        match self {
            Self::EncodedVideo(v) => &v.data,
            Self::EncodedAudio(a) => &a.datagram,
            _ => &[],
        }
    }

    fn set_payload(&mut self, payload: Vec<u8>) {
        match self {
            Self::EncodedVideo(v) => v.data = payload,
            Self::EncodedAudio(a) => a.datagram = payload,
            _ => {}
        }
    }
}

impl RudpHostIpcMessage for HostToComp {}

/// Serialise one frame into `out` (header + payload, see module docs).
pub fn encode_host_ipc_frame<T: RudpHostIpcMessage>(
    msg: &T,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let header =
        serde_json::to_vec(msg).map_err(|e| format!("rudp host ipc: encode header: {e}"))?;
    let payload = msg.payload();
    if header.len() > RUDP_HOST_IPC_MAX_HEADER {
        return Err("rudp host ipc: header too large".into());
    }
    if payload.len() > RUDP_HOST_IPC_MAX_PAYLOAD {
        return Err("rudp host ipc: payload too large".into());
    }
    out.reserve(8 + header.len() + payload.len());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    Ok(())
}

/// Write one frame (single `write_all`, so interleaved writers never split it
/// as long as the caller serialises access to `w`).
pub fn write_host_ipc_frame<W: Write, T: RudpHostIpcMessage>(
    w: &mut W,
    msg: &T,
) -> Result<(), String> {
    let mut buf = Vec::new();
    encode_host_ipc_frame(msg, &mut buf)?;
    w.write_all(&buf)
        .map_err(|e| format!("rudp host ipc: write: {e}"))?;
    w.flush().map_err(|e| format!("rudp host ipc: flush: {e}"))
}

/// Blocking read of one frame. `Ok(None)` on clean EOF at a frame boundary.
pub fn read_host_ipc_frame<R: Read, T: RudpHostIpcMessage>(r: &mut R) -> Result<Option<T>, String> {
    let mut prefix = [0u8; 8];
    match r.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(format!("rudp host ipc: read prefix: {e}")),
    }
    let header_len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    let payload_len = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]) as usize;
    if header_len == 0 || header_len > RUDP_HOST_IPC_MAX_HEADER {
        return Err(format!("rudp host ipc: bad header length {header_len}"));
    }
    if payload_len > RUDP_HOST_IPC_MAX_PAYLOAD {
        return Err(format!("rudp host ipc: bad payload length {payload_len}"));
    }
    let mut header = vec![0u8; header_len];
    r.read_exact(&mut header)
        .map_err(|e| format!("rudp host ipc: read header: {e}"))?;
    let mut msg: T = serde_json::from_slice(&header)
        .map_err(|e| format!("rudp host ipc: decode header: {e}"))?;
    if payload_len > 0 {
        let mut payload = vec![0u8; payload_len];
        r.read_exact(&mut payload)
            .map_err(|e| format!("rudp host ipc: read payload: {e}"))?;
        msg.set_payload(payload);
    }
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn config_roundtrip() {
        let msg = CompToHost::Config {
            ipc_version: RUDP_HOST_IPC_VERSION,
            bind: "0.0.0.0:7843".into(),
            allowed_users: vec!["alice".into()],
            lan_only: true,
            unlock_session_on_auth: false,
        };
        let mut buf = Vec::new();
        encode_host_ipc_frame(&msg, &mut buf).expect("encode");
        let got = read_host_ipc_frame::<_, CompToHost>(&mut Cursor::new(buf))
            .expect("decode")
            .expect("frame");
        match got {
            CompToHost::Config {
                bind,
                allowed_users,
                ..
            } => {
                assert_eq!(bind, "0.0.0.0:7843");
                assert_eq!(allowed_users, vec!["alice".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn video_payload_travels_as_binary_tail() {
        let msg = CompToHost::EncodedVideo(IpcEncodedVideo {
            stream_id: 2,
            output_id: "DP-1".into(),
            width: 1920,
            height: 1080,
            seq: 9,
            pts_us: 1234,
            codec: "h264".into(),
            is_keyframe: true,
            damage_full: true,
            damage: vec![RudpDamageRect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }],
            data: vec![0, 1, 2, 3, 255],
        });
        let mut buf = Vec::new();
        encode_host_ipc_frame(&msg, &mut buf).expect("encode");
        // Payload is raw, not base64 / JSON-array encoded in the header.
        assert!(buf.ends_with(&[0, 1, 2, 3, 255]));
        let got = read_host_ipc_frame::<_, CompToHost>(&mut Cursor::new(buf))
            .expect("decode")
            .expect("frame");
        match got {
            CompToHost::EncodedVideo(v) => {
                assert_eq!(v.data, vec![0, 1, 2, 3, 255]);
                assert_eq!(v.stream_id, 2);
                assert_eq!(v.damage.len(), 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn eof_at_boundary_is_none() {
        let got = read_host_ipc_frame::<_, HostToComp>(&mut Cursor::new(Vec::new()))
            .expect("no error on clean EOF");
        assert!(got.is_none());
    }

    #[test]
    fn rejects_oversized_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        assert!(read_host_ipc_frame::<_, HostToComp>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn input_from_control_maps_only_input() {
        assert_eq!(
            RudpHostInput::from_control(RudpControlMsg::Key {
                keycode: 30,
                pressed: true
            }),
            Some(RudpHostInput::Key {
                keycode: 30,
                pressed: true
            })
        );
        assert!(
            RudpHostInput::from_control(RudpControlMsg::SubscribeOutputs { output_ids: vec![] })
                .is_none()
        );
    }

    #[test]
    fn host_to_comp_roundtrip() {
        let msg = HostToComp::SubscribeUnion {
            output_ids: vec!["DP-1".into()],
            gained: true,
        };
        let mut buf = Vec::new();
        encode_host_ipc_frame(&msg, &mut buf).expect("encode");
        let got = read_host_ipc_frame::<_, HostToComp>(&mut Cursor::new(buf))
            .expect("decode")
            .expect("frame");
        assert!(matches!(
            got,
            HostToComp::SubscribeUnion { gained: true, .. }
        ));
    }
}
