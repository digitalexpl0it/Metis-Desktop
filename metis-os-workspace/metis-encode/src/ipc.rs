//! Wire protocol between the compositor and the isolated encode worker.
//!
//! Transport: `AF_UNIX` `SOCK_STREAM` socketpair. Dmabuf plane FDs travel as
//! `SCM_RIGHTS` ancillary data attached to the first byte of a message.
//!
//! Framing: `[u32 LE body_len][u32 LE json_len][json][blob]` where `blob` carries
//! raw encoded bitstream bytes (never JSON-encoded).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::types::{DamageRect, EncoderConfig, EncoderInfo, RudpCodec};

/// Hard cap on one message body (rejects garbage / runaway sizes).
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// Max dmabuf planes carried per frame.
pub const MAX_FDS: usize = 4;
/// Fd number the worker receives its socket on (`metis-encode-probe worker 3`).
pub const WORKER_SOCKET_FD: RawFd = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Init {
        cfg: EncoderConfig,
        render_node: String,
    },
    Frame(FrameMeta),
    Flush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameMeta {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub offsets: Vec<u32>,
    pub strides: Vec<u32>,
    pub damage_full: bool,
    pub damage: Vec<DamageRect>,
    pub keyframe: bool,
    pub planes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ready { info: EncoderInfo },
    Packets { packets: Vec<PacketMeta> },
    Error { message: String, fatal: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketMeta {
    pub seq: u64,
    pub codec: RudpCodec,
    pub is_keyframe: bool,
    pub pts_us: i64,
    pub damage_full: bool,
    pub damage: Vec<DamageRect>,
    pub len: u32,
}

/// One decoded message: JSON header, binary blob, and any received FDs.
pub struct Message<T> {
    pub header: T,
    pub blob: Vec<u8>,
    pub fds: Vec<OwnedFd>,
}

/// Why a receive did not produce a message.
#[derive(Debug)]
pub enum RecvError {
    /// Peer closed the socket (worker exited / crashed).
    Closed,
    TimedOut,
    Cancelled,
    Io(io::Error),
    Protocol(String),
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "peer closed the connection"),
            Self::TimedOut => write!(f, "timed out"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Protocol(m) => write!(f, "protocol: {m}"),
        }
    }
}

/// Encode a message body (without the outer length prefix).
pub fn encode_message<T: Serialize>(header: &T, blob: &[u8]) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(header).map_err(io::Error::other)?;
    let json_len = u32::try_from(json.len()).map_err(|_| io::Error::other("json too large"))?;
    let body_len = 4usize
        .checked_add(json.len())
        .and_then(|n| n.checked_add(blob.len()))
        .filter(|n| *n <= MAX_BODY)
        .ok_or_else(|| io::Error::other("message too large"))?;
    let mut out = Vec::with_capacity(4 + body_len);
    out.extend_from_slice(&(body_len as u32).to_le_bytes());
    out.extend_from_slice(&json_len.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(blob);
    Ok(out)
}

fn decode_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<(T, Vec<u8>), RecvError> {
    if body.len() < 4 {
        return Err(RecvError::Protocol("short body".into()));
    }
    let json_len = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    let json_end = 4usize
        .checked_add(json_len)
        .filter(|end| *end <= body.len())
        .ok_or_else(|| RecvError::Protocol("json length exceeds body".into()))?;
    let header = serde_json::from_slice(&body[4..json_end])
        .map_err(|e| RecvError::Protocol(format!("json: {e}")))?;
    Ok((header, body[json_end..].to_vec()))
}

/// Send one message, attaching `fds` (SCM_RIGHTS) to its first byte.
///
/// Blocking; bounded by the socket's `SO_SNDTIMEO` when set.
pub fn send_message(sock: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::other("too many fds"));
    }
    let mut sent = 0usize;
    if !fds.is_empty() {
        sent = sendmsg_with_fds(sock, bytes, fds)?;
    }
    while sent < bytes.len() {
        let rest = &bytes[sent..];
        // SAFETY: valid buffer slice; MSG_NOSIGNAL avoids SIGPIPE killing us.
        let n = unsafe {
            libc::send(
                sock,
                rest.as_ptr() as *const libc::c_void,
                rest.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "socket closed"));
        }
        sent += n as usize;
    }
    Ok(())
}

fn sendmsg_with_fds(sock: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let fd_bytes = std::mem::size_of_val(fds);
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(fd_bytes as u32) } as usize;
    let mut cmsg_buf = vec![0u8; space];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    // SAFETY: zeroed msghdr is a valid initial state.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    // SAFETY: msg_control points at `space` bytes, enough for one cmsg of fds.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::other("CMSG_FIRSTHDR null"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_bytes as u32) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr() as *const u8, libc::CMSG_DATA(cmsg), fd_bytes);
    }
    loop {
        // SAFETY: msg fully initialised above.
        let n = unsafe { libc::sendmsg(sock, &msg, libc::MSG_NOSIGNAL) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "socket closed"));
        }
        return Ok(n as usize);
    }
}

/// Deadline / cancellation for a blocking receive. `None` deadline = wait forever.
#[derive(Clone, Default)]
pub struct RecvLimits {
    pub deadline: Option<Instant>,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl RecvLimits {
    pub fn within(timeout: Duration, cancel: Option<Arc<AtomicBool>>) -> Self {
        Self {
            deadline: Some(Instant::now() + timeout),
            cancel,
        }
    }
}

/// Wait until `sock` is readable, honouring the deadline + cancel flag.
fn wait_readable(sock: RawFd, limits: &RecvLimits) -> Result<(), RecvError> {
    loop {
        if limits
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            return Err(RecvError::Cancelled);
        }
        let slice_ms = match limits.deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(RecvError::TimedOut);
                }
                left.as_millis().min(100) as i32
            }
            None if limits.cancel.is_some() => 100,
            None => -1,
        };
        let mut pfd = libc::pollfd {
            fd: sock,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = if slice_ms < 0 { -1 } else { slice_ms.max(1) };
        // SAFETY: one valid pollfd.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(RecvError::Io(err));
        }
        if rc > 0 {
            // POLLIN, POLLHUP, or POLLERR — let recvmsg report which.
            return Ok(());
        }
    }
}

/// Read exactly `buf.len()` bytes, collecting any SCM_RIGHTS fds into `fds`.
fn recv_exact(
    sock: RawFd,
    buf: &mut [u8],
    fds: &mut Vec<OwnedFd>,
    limits: &RecvLimits,
) -> Result<(), RecvError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        wait_readable(sock, limits)?;
        let n = recvmsg_collect(sock, &mut buf[filled..], fds)?;
        if n == 0 {
            return Err(RecvError::Closed);
        }
        filled += n;
    }
    Ok(())
}

fn recvmsg_collect(
    sock: RawFd,
    buf: &mut [u8],
    fds: &mut Vec<OwnedFd>,
) -> Result<usize, RecvError> {
    let fd_bytes = MAX_FDS * std::mem::size_of::<RawFd>();
    // SAFETY: size computation only.
    let space = unsafe { libc::CMSG_SPACE(fd_bytes as u32) } as usize;
    let mut cmsg_buf = vec![0u8; space];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    // SAFETY: zeroed msghdr is a valid initial state.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;

    let n = loop {
        // SAFETY: msg initialised; MSG_CMSG_CLOEXEC keeps received fds out of exec'd children.
        let n = unsafe { libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n < 0 {
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Err(RecvError::TimedOut),
                _ => return Err(RecvError::Io(err)),
            }
        }
        break n as usize;
    };

    // Take ownership of every received fd first so nothing leaks on error paths.
    // SAFETY: walking the control buffer the kernel just filled.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg);
                let header = libc::CMSG_LEN(0) as usize;
                let payload = ((*cmsg).cmsg_len as usize).saturating_sub(header);
                let count = payload / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let mut raw: RawFd = -1;
                    std::ptr::copy_nonoverlapping(
                        data.add(i * std::mem::size_of::<RawFd>()),
                        &mut raw as *mut RawFd as *mut u8,
                        std::mem::size_of::<RawFd>(),
                    );
                    if raw >= 0 {
                        fds.push(OwnedFd::from_raw_fd(raw));
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(RecvError::Protocol(
            "ancillary data truncated (too many fds)".into(),
        ));
    }
    if fds.len() > MAX_FDS {
        return Err(RecvError::Protocol("too many fds".into()));
    }
    Ok(n)
}

/// Receive one full message.
pub fn recv_message<T: for<'de> Deserialize<'de>>(
    sock: &impl AsRawFd,
    limits: &RecvLimits,
) -> Result<Message<T>, RecvError> {
    let raw = sock.as_raw_fd();
    let mut fds = Vec::new();
    let mut len_buf = [0u8; 4];
    recv_exact(raw, &mut len_buf, &mut fds, limits)?;
    let body_len = u32::from_le_bytes(len_buf) as usize;
    if !(4..=MAX_BODY).contains(&body_len) {
        return Err(RecvError::Protocol(format!("bad body length {body_len}")));
    }
    let mut body = vec![0u8; body_len];
    recv_exact(raw, &mut body, &mut fds, limits)?;
    let (header, blob) = decode_body(&body)?;
    Ok(Message { header, blob, fds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    #[test]
    fn roundtrip_with_fd() {
        let (a, b) = UnixStream::pair().expect("pair");
        let file = tempfile_fd();
        let bytes = encode_message(&Request::Flush, b"xyz").expect("encode");
        send_message(a.as_raw_fd(), &bytes, &[file.as_raw_fd()]).expect("send");
        let msg: Message<Request> =
            recv_message(&b, &RecvLimits::within(Duration::from_secs(2), None)).expect("recv");
        assert!(matches!(msg.header, Request::Flush));
        assert_eq!(msg.blob, b"xyz");
        assert_eq!(msg.fds.len(), 1);
    }

    #[test]
    fn closed_peer_reports_closed() {
        let (a, b) = UnixStream::pair().expect("pair");
        drop(a);
        let res: Result<Message<Request>, _> =
            recv_message(&b, &RecvLimits::within(Duration::from_secs(1), None));
        assert!(matches!(res, Err(RecvError::Closed)));
    }

    #[test]
    fn timeout_and_cancel() {
        let (_a, b) = UnixStream::pair().expect("pair");
        let res: Result<Message<Request>, _> =
            recv_message(&b, &RecvLimits::within(Duration::from_millis(50), None));
        assert!(matches!(res, Err(RecvError::TimedOut)));
        let cancel = Arc::new(AtomicBool::new(true));
        let res: Result<Message<Request>, _> = recv_message(
            &b,
            &RecvLimits {
                deadline: None,
                cancel: Some(cancel),
            },
        );
        assert!(matches!(res, Err(RecvError::Cancelled)));
    }

    #[test]
    fn rejects_oversized_length() {
        let (mut a, b) = UnixStream::pair().expect("pair");
        a.write_all(&u32::MAX.to_le_bytes()).expect("write");
        let res: Result<Message<Request>, _> =
            recv_message(&b, &RecvLimits::within(Duration::from_secs(1), None));
        assert!(matches!(res, Err(RecvError::Protocol(_))));
    }

    fn tempfile_fd() -> OwnedFd {
        let (x, _y) = UnixStream::pair().expect("pair");
        OwnedFd::from(x)
    }
}
