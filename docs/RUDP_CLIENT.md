# Building a Metis Remote (RUDP) client

This document describes how to implement a **Metis Remote** client that connects
to a Metis host’s Quinn/RUDP stream. It is the integration guide for future
**Windows** and **macOS** clients (and any third-party client). Metis does not
ship those desktop clients yet; keep this file in sync when the wire protocol
or reference crates change.

Metis Remote is **not** RDP. Classic RDP (GNOME Remote Desktop / `metis-rdp-host`)
uses TCP 3389 and FreeRDP. This guide covers **RUDP only**.

**Source of truth (code):**

| Concern | Crate / path |
|--------|----------------|
| Control + video/audio wire | [`metis-protocol`](../metis-os-workspace/metis-protocol/) |
| Reference Quinn session | [`metis-rudp-client`](../metis-os-workspace/metis-rudp-client/) |
| Reference decode (FFmpeg) | [`metis-decode`](../metis-os-workspace/metis-decode/) |
| Linux UI reference | [`metis-viewer`](../metis-os-workspace/metis-viewer/) `rudp_session.rs` |
| Host | [`metis-rudp-host`](../metis-os-workspace/metis-rudp-host/) process (Quinn / TLS / PAM), spawned by the compositor (`rudp_host_ipc.rs`) |
| Smoke CLI | [`metis-rudp-smoke`](../metis-os-workspace/metis-rudp-smoke/) |

Prefer **embedding `metis-rudp-client` + `metis-decode`** in a Rust UI (egui,
iced, winit, etc.) over reimplementing Quinn framing. Reimplement only if you
must use another language — then treat `metis-protocol` + this doc as the
contract and verify against `metis-rudp-smoke` / Metis Viewer.

---

## 1. Scope and defaults

| Item | Value |
|------|--------|
| Transport | QUIC (Quinn) over **UDP** |
| Default port | **7843** (configurable in `~/.config/metis/rudp.json` on the host) |
| TLS | Host self-signed cert; client **TOFU**-pins SHA-256 fingerprint |
| Auth | Local **PAM** username + password (allowlisted on host) |
| Protocol version | `RUDP_PROTOCOL_VERSION` = **4** (`metis-protocol`) — stream quality; hosts still accept Hello **3..=4** |
| First-party Linux client | Metis Viewer → protocol **Metis Remote** |

Host enablement and firewall: [User Guide — Metis Remote](USER_GUIDE.md#metis-remote-rudp).

> **Host process (Phase 23 §D).** The QUIC endpoint you connect to is the
> separate `metis-rudp-host` process, not the compositor itself; the compositor
> feeds it encoded video / audio over a private Unix socket
> (`metis_protocol::rudp_host_ipc`, internal — not part of this contract).
> Datagram layouts, TOFU fingerprint and PAM auth are unchanged from v3. If the
> host process crashes the compositor restarts it with back-off; clients just
> see a dropped connection and reconnect. `METIS_RUDP_INPROCESS=1` (emergency,
> one release) runs the identical engine inside the compositor.

---

## 2. Recommended client architecture

```text
┌─────────────────────────────────────────┐
│  Platform UI (Win / macOS / Linux)      │
│  present RGBA · capture input · audio   │
└─────────────────┬───────────────────────┘
                  │
┌─────────────────▼───────────────────────┐
│  metis-rudp-client                      │
│  connect · TOFU · PAM · events · input  │
└─────────────────┬───────────────────────┘
                  │
┌─────────────────▼───────────────────────┐
│  metis-decode  →  RGBA frames           │
│  metis-protocol (types / framing)       │
└─────────────────────────────────────────┘
```

Do **not** link FreeRDP, `metis-remote` host helpers, or GRD into an RUDP-only
client.

---

## 3. Session ladder

1. Resolve `host:port` → `SocketAddr` (DNS allowed; Viewer uses
   `metis_rudp_client::resolve_host_port`).
2. Open a Quinn client connection with a custom TLS verifier:
   - Unknown cert → interactive Trust / Decline (or auto-pin in smoke tools).
   - Fingerprint = SHA-256 of cert DER, colon-hex lowercase
     (`metis_config::fingerprint_cert_der`).
   - Pin store: `known_hosts` under the Metis config dir
     (`~/.config/metis/rudp/known_hosts` on Linux; use the same relative layout
     under the platform config dir on Windows/macOS via `directories`).
3. Open a **bidirectional control stream**.
4. Client → host: `Hello { protocol: 4, username }` (hosts accept **3..=4**).
   with `Reject { reason: protocol }`.
5. Host → client: `AuthChallenge { nonce }` (nonce may be unused by simple
   password clients today; still wait for the message).
6. Client → host: `AuthResponse { password }` (never log or persist).
7. Host → client: `SessionOk { session_id }` **or** `Reject { reason, detail? }`.
8. After `SessionOk`, host sends `DesktopLayout { monitors }`, then client should
   send `SubscribeOutputs { output_ids }` (empty / omit → **primary only**; max
   **4** ids). Host replies with one `VideoReady` per subscribed stream
   (`stream_id`, `output_id`, size, codec), optionally `AudioReady`, then media.
   Host may also send `PointerLock`, `ClipboardSet`, and (protocol ≥ 4)
   `StreamQuality`. Layout is re-sent on hotplug / primary change.
9. Client (protocol ≥ 4) may send `SetStreamQuality { quality }` anytime after
   auth (`performance` / `high` / `medium` / `low`). The host applies encode
   prefs live and replies with `StreamQuality` to all v4 sessions. Does **not**
   rewrite host `rudp.json` when sent from a client.
10. Client sends input / clipboard on the control stream; reads media from
   uni-stream + datagrams demuxed by `stream_id`.

Reject reasons (`snake_case`): `not_allowed`, `auth_failed`, `protocol`,
`rate_limited`.

The host **emits `rate_limited`** when a peer IP exceeds the auth failure budget
(5 failures / 60s → 60s lockout) or when session/handshake caps are saturated.
Clients should surface this distinctly from `auth_failed` and back off.

**Session lock (host behaviour clients should know):**

- When `rudp.json` `unlock_session_on_auth` is **true** (default), authenticating
  as the Metis session owner unlocks the Metis PAM lock. When **false**, the
  lock stays until local unlock.
- While the session is locked (Metis PAM or `ext-session-lock`), the host
  **pauses video and audio** (control stream stays up; input/clipboard remain
  gated). Media resumes after unlock with a keyframe.
- Other allowlisted users never unlock the Metis PAM lock. Third-party
  `ext-session-lock` lockers are never cleared by RUDP.

---

## 4. Control stream framing

Each message:

```text
u32 big-endian length | UTF-8 JSON body  (max 64 KiB)
```

JSON uses a `type` tag in `snake_case` (serde adjacently tagged). Helpers:
`encode_rudp_frame` / `try_decode_rudp_frame` in `metis-protocol`.

### Messages (summary)

| Direction | `type` | Notes |
|-----------|--------|--------|
| C→H | `hello` | `protocol`, `username` |
| H→C | `auth_challenge` | `nonce` |
| C→H | `auth_response` | `password` |
| H→C | `session_ok` | `session_id` |
| H→C | `reject` | `reason`, optional `detail` |
| either | `keepalive` | |
| H→C | `desktop_layout` | `monitors[]` with `id`, `name`, `x`, `y`, `width`, `height`, `scale`, `primary` |
| C→H | `subscribe_outputs` | `output_ids` (empty → primary; max 4) |
| H→C | `video_ready` | `width`, `height`, `codec`, `stream_id`, `output_id` |
| H→C | `audio_ready` | `sample_rate`, `channels`, `codec` (`opus`) |
| C→H | `pointer_absolute` | compositor **logical** desktop `x`,`y` (add monitor origin from layout) |
| C→H | `pointer_relative` | `dx`,`dy` logical pixels |
| C→H | `pointer_button` | Linux **evdev** button code (e.g. `BTN_LEFT` = `0x110`) |
| C→H | `pointer_scroll` | logical pixel deltas |
| C→H | `key` | Linux **evdev** keycode + `pressed` |
| H→C | `pointer_lock` | prefer relative mouse when `locked` |
| either | `clipboard_set` | UTF-8 `text`, `mime`, `serial`; max ~48 KiB text |
| C→H | `set_stream_quality` | protocol ≥ 4; `quality`: `performance`/`high`/`medium`/`low` |
| H→C | `stream_quality` | protocol ≥ 4; current encode preset (after SessionOk + on change) |

Map OS key/button codes to evdev on the client (or maintain a table). Absolute
pointer coordinates are the host’s full virtual desktop logical space, not
window-local.

**Cursor UX (Metis Viewer):** the host **omits** its seat cursor from RUDP video
frames. Clients should show the local OS pointer over the stream (do not use
`cursor: none`) so the user sees one responsive cursor. Absolute motion is mapped
1:1 into host logical space — local mouse acceleration affects travel across the
widget; host `input.json` accel does not apply to injected motion. On
`pointer_lock` with `locked=true`, prefer relative motion (games/CAD).

**Keyboard:** `key.keycode` is a Linux **evdev** `KEY_*` value (ESC = 1, A = 30).
The Metis host converts to XKB (`+ 8`) before seat inject — do **not** send
XKB keycodes on the wire.

---

## 5. Video media

### Codecs

| Wire `u8` | Name |
|-----------|------|
| 1 | H.264 |
| 2 | HEVC |
| 3 | AV1 |

Host advertises the active name in `VideoReady.codec`. Minimum viable client:
**software H.264** decode. HEVC/AV1 are optional with fallback messaging if
unsupported.

**Multi-monitor:** each subscribed output has its own `stream_id`. Open one
decoder (or window) per `VideoReady`. Map pointer from window-local pixels into
global logical space using that monitor’s `x`/`y` from `DesktopLayout`.

### Keyframes (reliable)

Host opens a **unidirectional** stream and sends length-prefixed
`ReliableAccessUnit` bodies (same `u32` BE length prefix as control). These are
keyframes (IDR). AU body version **2**: byte 3 is `stream_id`. Parse with
`ReliableAccessUnit` helpers in `metis-protocol`.

### Deltas (+ FEC) — datagrams

Quinn **unreliable datagrams**, magic **`MRUV`**, little-endian header
(`RUDP_DATAGRAM_HEADER_LEN` = 32), then payload. Datagram version **2**: header
byte 7 is `stream_id`. Flags include key / FEC / damage-full. Default datagram
budget ~1200 bytes. Reassemble with `try_reassemble_media` (Reed–Solomon when
FEC shards present), demuxing by `stream_id` before decode.

Deliver complete access units to the decoder (`metis-decode` or equivalent) →
RGBA for presentation.

---

## 6. Audio media

Optional. After `AudioReady` (typically 48 kHz stereo Opus, 20 ms frames):

- Datagrams magic **`MRUA`**, header 20 bytes + Opus payload.
- Demux: if datagram starts with `MRUA`, treat as audio; `MRUV` as video.
- Decode Opus → PCM; play via platform audio (Linux Viewer uses **cpal**).

No microphone uplink in v1.

---

## 7. Reference API (`metis-rudp-client`)

```rust
use metis_rudp_client::{
    RudpClientConfig, SessionEvent, TofuMode, connect, resolve_host_port,
};

let addr = resolve_host_port(&host, port)?;
let session = connect(RudpClientConfig {
    addr,
    host_key: format!("{host}:{port}"),
    username,
    password,
    tofu: TofuMode::Interactive, // or Auto for tests
}).await?;

while let Some(ev) = session.recv_event().await {
    match ev {
        SessionEvent::DesktopLayout { monitors } => { /* pick outputs; SubscribeOutputs */ }
        SessionEvent::VideoReady { width, height, codec, stream_id, output_id } => { /* decoder */ }
        SessionEvent::AccessUnit(au) => { /* demux on au.stream_id; decode au.data */ }
        SessionEvent::AudioReady { .. } | SessionEvent::AudioPacket { .. } => { /* … */ }
        SessionEvent::PointerLock { locked } => { /* relative vs absolute */ }
        SessionEvent::ClipboardSet { text, serial, .. } => { /* set OS clipboard */ }
        SessionEvent::Disconnected => break,
    }
}

session.send_input(RudpControlMsg::PointerAbsolute { x, y }).await?;
// Multi-monitor: pick up to RUDP_MAX_SUBSCRIBED_OUTPUTS (4) layout ids; empty = primary.
session.subscribe_outputs(vec!["DP-1".into(), "DP-2".into()]).await?;
```

Interactive TOFU surfaces `ClientError::TofuUnknown` / `TofuMismatch`; call
`pin_host` / `clear_host_pin` after user confirmation (see Viewer).

---

## 8. Platform notes (Windows / macOS)

| Topic | Guidance |
|-------|----------|
| UI toolkit | Prefer winit + egui/iced/wgpu — not GTK/FreeRDP |
| FFmpeg | Soft decode via system or bundled dylib/DLL; HW (D3D11VA / VideoToolbox) later |
| Config / pins | `%LOCALAPPDATA%\metis\rudp\known_hosts` (Win), `~/Library/Application Support/metis/rudp/` (macOS), or `directories` crate |
| Firewall | Host UDP 7843 (or configured port); client ephemeral UDP |
| Packaging | Ship exe + FFmpeg libs + runtime; no RDP stack |
| Auth UX | Username + password → host PAM; document that this is not Windows AD login |

---

## 9. Compliance checklist

A client is considered compatible when it can:

- [ ] Complete TOFU + PAM against an enabled Metis Remote host (protocol **v4**; v3 still accepted)
- [ ] Honour `DesktopLayout` + `SubscribeOutputs`; demux video by `stream_id`
- [ ] Decode H.264 access units to a visible frame
- [ ] Send absolute pointer + key events that move/type on an unlocked session
- [ ] Honour `Reject` / disconnect cleanly; never persist the password
- [ ] (Optional) Play Opus audio; sync text clipboard both ways
- [ ] (Optional) Handle HEVC/AV1 and FEC loss recovery
- [ ] (Optional, v4) Send/receive `SetStreamQuality` / `StreamQuality`

Validate with: host Settings → Metis Remote enabled, then
`metis-rudp-smoke` and/or Metis Viewer on Linux against the same host.

---

## 10. Keeping this document current

Update **this file in the same PR** when you change any of:

- `RUDP_PROTOCOL_VERSION` or `RudpControlMsg` variants
- Video/audio datagram magic, header layout, or codec IDs
- TOFU fingerprint algorithm or `known_hosts` format
- Auth / session-lock host behaviour that clients must mirror

Bump a one-line note in `CHANGELOG.md` under Metis Remote when the client
contract changes in a breaking way.
