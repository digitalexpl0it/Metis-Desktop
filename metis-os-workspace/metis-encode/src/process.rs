//! Compositor side of the isolated encode worker.
//!
//! All FFmpeg / libva / CUDA code runs in `metis-encode-probe worker`. A driver
//! segfault, abort, or hang there costs one encoder restart — never the
//! compositor (release builds use `panic = "abort"`, so in-process isolation
//! is impossible for C crashes).

use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::HwEncoder;
use crate::ipc::{self, FrameMeta, Message, RecvError, RecvLimits, Reply, Request};
use crate::probe::probe_binary_path;
use crate::types::{
    EncodeError, EncodeInput, EncodeResult, EncodedPacket, EncoderConfig, EncoderInfo,
};

/// Worst case for Init: per-codec probes (3 s each, up to 4) plus the real open.
const INIT_TIMEOUT: Duration = Duration::from_secs(20);
/// One frame round-trip. A healthy hardware encode is single-digit ms.
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const SEND_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_GRACE: Duration = Duration::from_millis(300);

/// [`HwEncoder`] backed by a child process.
pub struct ProcessEncoder {
    sock: UnixStream,
    child: Option<Child>,
    info: EncoderInfo,
    pending: Vec<EncodedPacket>,
    keyframe_next: bool,
    healthy: bool,
    cancel: Option<Arc<AtomicBool>>,
}

fn set_send_timeout(sock: &UnixStream, timeout: Duration) {
    if let Err(err) = sock.set_write_timeout(Some(timeout)) {
        tracing::warn!(%err, "metis-encode: could not set worker send timeout");
    }
}

fn spawn_worker(child_end: &UnixStream) -> EncodeResult<Child> {
    let bin = probe_binary_path().ok_or_else(|| {
        EncodeError::Unavailable(
            "metis-encode-probe not found (install it next to metis-compositor)".into(),
        )
    })?;
    let child_fd: RawFd = child_end.as_raw_fd();
    let target = ipc::WORKER_SOCKET_FD;
    let mut cmd = Command::new(&bin);
    cmd.arg("worker")
        .arg(target.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    // SAFETY: only async-signal-safe calls (dup2, fcntl, close_range, prctl,
    // getppid) between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if child_fd == target {
                let flags = libc::fcntl(target, libc::F_GETFD);
                if flags < 0 || libc::fcntl(target, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(child_fd, target) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Mark every other fd (DRM master, libinput, Wayland clients…)
            // close-on-exec so the worker never inherits them. CLOEXEC rather
            // than close keeps std's exec-error pipe working. Failure on
            // pre-5.11 kernels only means fewer fds are scrubbed.
            const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
            let _ = libc::syscall(
                libc::SYS_close_range,
                (target + 1) as libc::c_uint,
                libc::c_uint::MAX,
                CLOSE_RANGE_CLOEXEC,
            );
            // Worker must never outlive the compositor.
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL as libc::c_ulong,
                0,
                0,
                0,
            ) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() == 1 {
                return Err(std::io::Error::other("parent exited before worker start"));
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|err| EncodeError::Unavailable(format!("spawn {}: {err}", bin.display())))
}

fn recv_err_to_encode(err: RecvError, what: &str) -> EncodeError {
    match err {
        RecvError::Cancelled => EncodeError::Cancelled,
        other => EncodeError::Worker(format!("{what}: {other}")),
    }
}

impl ProcessEncoder {
    /// Spawn a worker and open an encoder in it. Blocks up to ~20 s (probe
    /// ladder); returns early with [`EncodeError::Cancelled`] when `cancel` is set.
    pub fn spawn(
        cfg: &EncoderConfig,
        render_node: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> EncodeResult<Self> {
        let (parent_end, child_end) = UnixStream::pair()?;
        let child = spawn_worker(&child_end)?;
        drop(child_end);
        set_send_timeout(&parent_end, SEND_TIMEOUT);

        let mut this = Self {
            sock: parent_end,
            child: Some(child),
            info: EncoderInfo {
                backend: cfg.backend,
                codec: cfg.codec,
                encoder_name: String::new(),
                width: cfg.width,
                height: cfg.height,
            },
            pending: Vec::new(),
            keyframe_next: true,
            healthy: true,
            cancel,
        };

        let init = Request::Init {
            cfg: cfg.clone(),
            render_node: render_node.to_string(),
        };
        this.send(&init, &[])?;
        let limits = RecvLimits::within(INIT_TIMEOUT, this.cancel.clone());
        let reply: Message<Reply> = ipc::recv_message(&this.sock, &limits)
            .map_err(|e| this.fail(recv_err_to_encode(e, "init")))?;
        match reply.header {
            Reply::Ready { info } => {
                tracing::info!(
                    backend = ?info.backend,
                    codec = ?info.codec,
                    encoder = %info.encoder_name,
                    width = info.width,
                    height = info.height,
                    "metis-encode: isolated encode worker ready"
                );
                this.info = info;
                Ok(this)
            }
            Reply::Error { message, .. } => Err(this.fail(EncodeError::Unavailable(message))),
            Reply::Packets { .. } => Err(this.fail(EncodeError::Worker(
                "unexpected packets before ready".into(),
            ))),
        }
    }

    fn fail(&mut self, err: EncodeError) -> EncodeError {
        self.healthy = false;
        self.kill_child();
        err
    }

    fn send(&mut self, req: &Request, fds: &[RawFd]) -> EncodeResult<()> {
        let bytes = ipc::encode_message(req, &[])?;
        ipc::send_message(self.sock.as_raw_fd(), &bytes, fds)
            .map_err(|e| self.fail(EncodeError::Worker(format!("send: {e}"))))
    }

    fn round_trip(&mut self, req: &Request, fds: &[RawFd]) -> EncodeResult<Vec<EncodedPacket>> {
        if !self.healthy {
            return Err(EncodeError::Worker("worker is not running".into()));
        }
        self.send(req, fds)?;
        let limits = RecvLimits::within(FRAME_TIMEOUT, self.cancel.clone());
        let reply: Message<Reply> = ipc::recv_message(&self.sock, &limits)
            .map_err(|e| self.fail(recv_err_to_encode(e, "frame")))?;
        match reply.header {
            Reply::Packets { packets } => {
                let mut out = Vec::with_capacity(packets.len());
                let mut offset = 0usize;
                for meta in packets {
                    let end = offset
                        .checked_add(meta.len as usize)
                        .filter(|end| *end <= reply.blob.len())
                        .ok_or_else(|| {
                            self.fail(EncodeError::Worker("packet blob out of range".into()))
                        })?;
                    out.push(EncodedPacket {
                        seq: meta.seq,
                        codec: meta.codec,
                        is_keyframe: meta.is_keyframe,
                        data: reply.blob[offset..end].to_vec(),
                        pts_us: meta.pts_us,
                        damage_full: meta.damage_full,
                        damage: meta.damage,
                    });
                    offset = end;
                }
                Ok(out)
            }
            Reply::Error { message, fatal } => {
                if fatal {
                    Err(self.fail(EncodeError::Worker(message)))
                } else {
                    Err(EncodeError::Ffmpeg(message))
                }
            }
            Reply::Ready { .. } => Err(self.fail(EncodeError::Worker("unexpected ready".into()))),
        }
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl HwEncoder for ProcessEncoder {
    fn info(&self) -> &EncoderInfo {
        &self.info
    }

    fn submit(&mut self, frame: &EncodeInput<'_>) -> EncodeResult<()> {
        if frame.fds.is_empty() || frame.fds.len() > ipc::MAX_FDS {
            return Err(EncodeError::InvalidInput(format!(
                "unsupported plane count {}",
                frame.fds.len()
            )));
        }
        let meta = FrameMeta {
            seq: frame.seq,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            fourcc: frame.fourcc,
            modifier: frame.modifier,
            offsets: frame.offsets.to_vec(),
            strides: frame.strides.to_vec(),
            damage_full: frame.damage_full,
            damage: frame.damage.to_vec(),
            keyframe: std::mem::take(&mut self.keyframe_next),
            planes: frame.fds.len() as u32,
        };
        let fds: Vec<RawFd> = frame.fds.iter().map(|f| f.as_raw_fd()).collect();
        match self.round_trip(&Request::Frame(meta), &fds) {
            Ok(packets) => {
                self.pending.extend(packets);
                Ok(())
            }
            Err(err) => {
                // Retry the IDR on the next frame if this one never encoded.
                self.keyframe_next = true;
                Err(err)
            }
        }
    }

    fn drain(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        Ok(std::mem::take(&mut self.pending))
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        let mut out = std::mem::take(&mut self.pending);
        if self.healthy {
            out.extend(self.round_trip(&Request::Flush, &[])?);
        }
        Ok(out)
    }

    fn request_keyframe(&mut self) {
        self.keyframe_next = true;
    }

    fn is_healthy(&self) -> bool {
        self.healthy
    }
}

impl Drop for ProcessEncoder {
    fn drop(&mut self) {
        // Closing the socket makes a healthy worker exit on its own.
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
        let Some(mut child) = self.child.take() else {
            return;
        };
        let deadline = Instant::now() + EXIT_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }
}

/// Open an encoder in an isolated worker process (the only path the compositor
/// should use).
pub fn open_isolated_encoder(
    cfg: &EncoderConfig,
    render_node: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> EncodeResult<Box<dyn HwEncoder>> {
    Ok(Box::new(ProcessEncoder::spawn(cfg, render_node, cancel)?))
}
