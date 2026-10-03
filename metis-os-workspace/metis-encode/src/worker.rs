//! Encode worker process body (`metis-encode-probe worker <fd>`).
//!
//! Owns the FFmpeg / libva / CUDA state so a driver crash kills only this
//! process. The compositor talks to it via [`crate::process::ProcessEncoder`].

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use crate::ipc::{self, FrameMeta, Message, PacketMeta, RecvError, RecvLimits, Reply, Request};
use crate::types::{EncodeInput, EncodedPacket};
use crate::{HwEncoder, open_encoder};

fn send_reply(sock: &UnixStream, reply: &Reply, blob: &[u8]) -> std::io::Result<()> {
    let bytes = ipc::encode_message(reply, blob)?;
    ipc::send_message(sock.as_raw_fd(), &bytes, &[])
}

fn send_packets(sock: &UnixStream, packets: Vec<EncodedPacket>) -> std::io::Result<()> {
    let total: usize = packets.iter().map(|p| p.data.len()).sum();
    let mut blob = Vec::with_capacity(total);
    let mut metas = Vec::with_capacity(packets.len());
    for p in packets {
        let len =
            u32::try_from(p.data.len()).map_err(|_| std::io::Error::other("packet too large"))?;
        blob.extend_from_slice(&p.data);
        metas.push(PacketMeta {
            seq: p.seq,
            codec: p.codec,
            is_keyframe: p.is_keyframe,
            pts_us: p.pts_us,
            damage_full: p.damage_full,
            damage: p.damage,
            len,
        });
    }
    send_reply(sock, &Reply::Packets { packets: metas }, &blob)
}

fn encode_one(
    enc: &mut dyn HwEncoder,
    meta: &FrameMeta,
    fds: &[OwnedFd],
) -> Result<Vec<EncodedPacket>, String> {
    if fds.is_empty() || fds.len() != meta.planes as usize {
        return Err(format!(
            "expected {} plane fds, received {}",
            meta.planes,
            fds.len()
        ));
    }
    if meta.offsets.len() != fds.len() || meta.strides.len() != fds.len() {
        return Err("plane metadata length mismatch".into());
    }
    let borrowed: Vec<_> = fds.iter().map(|f| f.as_fd()).collect();
    let input = EncodeInput {
        seq: meta.seq,
        width: meta.width,
        height: meta.height,
        stride: meta.stride,
        fourcc: meta.fourcc,
        modifier: meta.modifier,
        fds: &borrowed,
        offsets: &meta.offsets,
        strides: &meta.strides,
        damage_full: meta.damage_full,
        damage: &meta.damage,
    };
    if meta.keyframe {
        enc.request_keyframe();
    }
    enc.submit(&input).map_err(|e| e.to_string())?;
    enc.drain().map_err(|e| e.to_string())
}

/// Serve encode requests on `fd` until the peer closes it. Returns an exit code.
pub fn run_worker(fd: RawFd) -> i32 {
    if fd < 0 {
        eprintln!("metis-encode worker: invalid socket fd {fd}");
        return 2;
    }
    // SAFETY: the parent dup2'd a socketpair end onto `fd` before exec; we own it.
    let sock = UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let forever = RecvLimits::default();

    // Fault injection for crash-isolation tests: abort after N frames.
    let abort_after: Option<u64> = std::env::var("METIS_ENCODE_WORKER_ABORT_AFTER")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut frames_seen: u64 = 0;

    let mut encoder: Option<Box<dyn HwEncoder>> = None;
    loop {
        let msg: Message<Request> = match ipc::recv_message(&sock, &forever) {
            Ok(m) => m,
            Err(RecvError::Closed) => return 0,
            Err(err) => {
                tracing::error!(%err, "metis-encode worker: receive failed — exiting");
                return 1;
            }
        };
        match msg.header {
            Request::Init { cfg, render_node } => {
                if encoder.is_some() {
                    let _ = send_reply(
                        &sock,
                        &Reply::Error {
                            message: "already initialised".into(),
                            fatal: true,
                        },
                        &[],
                    );
                    return 1;
                }
                match open_encoder(&cfg, &render_node) {
                    Ok(enc) => {
                        let info = enc.info().clone();
                        encoder = Some(enc);
                        if send_reply(&sock, &Reply::Ready { info }, &[]).is_err() {
                            return 1;
                        }
                    }
                    Err(err) => {
                        let _ = send_reply(
                            &sock,
                            &Reply::Error {
                                message: err.to_string(),
                                fatal: true,
                            },
                            &[],
                        );
                        return 1;
                    }
                }
            }
            Request::Frame(meta) => {
                frames_seen += 1;
                if abort_after.is_some_and(|n| frames_seen > n) {
                    std::process::abort();
                }
                let Some(enc) = encoder.as_mut() else {
                    let _ = send_reply(
                        &sock,
                        &Reply::Error {
                            message: "frame before init".into(),
                            fatal: true,
                        },
                        &[],
                    );
                    return 1;
                };
                // `msg.fds` drop at end of this arm closes the worker's dmabuf dups.
                let sent = match encode_one(enc.as_mut(), &meta, &msg.fds) {
                    Ok(packets) => send_packets(&sock, packets),
                    Err(message) => send_reply(
                        &sock,
                        &Reply::Error {
                            message,
                            fatal: false,
                        },
                        &[],
                    ),
                };
                if sent.is_err() {
                    return 1;
                }
            }
            Request::Flush => {
                let packets = encoder
                    .as_mut()
                    .map(|e| e.flush().unwrap_or_default())
                    .unwrap_or_default();
                if send_packets(&sock, packets).is_err() {
                    return 1;
                }
            }
        }
    }
}
