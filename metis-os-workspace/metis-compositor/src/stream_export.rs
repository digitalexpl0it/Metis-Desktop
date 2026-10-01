//! Phase 1 RUDP host capture: export the primary output's composited frame as
//! dmabuf plane FDs for a future encode worker — **no PipeWire / portal**.
//!
//! Scanout still owns the KMS buffer. When armed, we run a second
//! `OutputStack` compose into a dedicated GBM dmabuf pool (same pattern as
//! [`crate::image_capture`]), then publish an [`ExportedFrame`] on a
//! latest-wins slot + wake channel (never blocks the render thread).
//!
//! Guardrails:
//! - Prefer hardware-tiled GBM modifiers (`Modifier::Invalid` / render-node
//!   formats); do not force LINEAR as the only layout.
//! - Do not `sync.wait()` on the render thread — export an explicit fence FD
//!   (and keep [`SyncPoint`] for the consumer when export is unavailable).
//! - Pool slots use [`SlotReleaseGuard`] so dropped / superseded frames free
//!   their BO immediately under latest-wins backpressure.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufAllocator};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Allocator, Buffer as AllocBuffer, Fourcc, Modifier};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::drm::DrmNode;
use smithay::backend::renderer::Bind;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::output::Output;
use smithay::utils::{Physical, Point, Scale, Size, Transform};

use crate::render::CLEAR_COLOR;
use crate::state::MetisState;

const POOL_SIZE: usize = 3;
/// Wake channel depth — payload lives in `latest` (true latest-wins).
const WAKE_CAP: usize = 1;
/// Synthetic encode latency in the debug consumer (proves slot RAII under load).
const DEBUG_ENCODE_SLEEP: Duration = Duration::from_millis(6);

/// One composited primary-output frame for the RUDP encode worker.
///
/// Plane FDs are owned duplicates — safe to send across threads. Dropping the
/// frame closes the FDs and releases the pool slot via [`SlotReleaseGuard`].
pub struct ExportedFrame {
    pub seq: u64,
    pub output_name: String,
    pub width: u32,
    pub height: u32,
    /// Bytes per row of plane 0 (GBM pitch).
    pub stride: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub fds: Vec<OwnedFd>,
    pub offsets: Vec<u32>,
    pub strides: Vec<u32>,
    /// Full-frame for Phase 1; damage rects arrive in a later phase.
    pub damage_full: bool,
    /// Explicit sync fence FD when the GLES sync point is exportable.
    /// Prefer awaiting this on the encode thread (never on the render path).
    pub fence: Option<OwnedFd>,
    /// Fallback sync point when `fence` is `None` — call [`SyncPoint::wait`]
    /// on the consumer thread before reading the dmabuf.
    pub sync: SyncPoint,
    /// RAII: clears `busy_slots[index]` when the last clone is dropped.
    _slot_guard: Option<Arc<SlotReleaseGuard>>,
}

impl std::fmt::Debug for ExportedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportedFrame")
            .field("seq", &self.seq)
            .field("output_name", &self.output_name)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("fourcc", &self.fourcc)
            .field("modifier", &self.modifier)
            .field("planes", &self.fds.len())
            .field("damage_full", &self.damage_full)
            .field("has_fence", &self.fence.is_some())
            .field("slot", &self._slot_guard.as_ref().map(|g| g.slot_index))
            .finish()
    }
}

impl ExportedFrame {
    /// Block until GPU work for this frame is complete (encode / debug thread only).
    pub fn wait_ready(&self) {
        if let Some(fence) = &self.fence {
            // Native sync_file: poll until readable (signalled).
            wait_sync_file(fence);
            return;
        }
        let _ = self.sync.wait();
    }
}

fn wait_sync_file(fd: &OwnedFd) {
    use std::os::fd::AsRawFd;
    let mut pollfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // -1 timeout: wait until signalled (encode thread only).
    loop {
        let rc = unsafe { libc::poll(&mut pollfd, 1, -1) };
        if rc >= 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            tracing::debug!(?err, "stream export: sync_file poll failed");
            break;
        }
    }
}

struct SlotReleaseGuard {
    slot_index: usize,
    pool: Arc<Mutex<PoolState>>,
}

impl Drop for SlotReleaseGuard {
    fn drop(&mut self) {
        let Ok(mut pool) = self.pool.lock() else {
            return;
        };
        if let Some(busy) = pool.busy.get_mut(self.slot_index) {
            *busy = false;
        }
    }
}

struct PoolSlot {
    dmabuf: Dmabuf,
    width: u32,
    height: u32,
}

struct PoolState {
    slots: Vec<PoolSlot>,
    busy: Vec<bool>,
    fourcc: Fourcc,
}

impl PoolState {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            busy: Vec::new(),
            fourcc: Fourcc::Xrgb8888,
        }
    }

    fn ensure(
        &mut self,
        gbm: &GbmDevice<DrmDeviceFd>,
        width: u32,
        height: u32,
        preferred_modifiers: &[Modifier],
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("zero-sized stream export buffer".into());
        }
        let needs_rebuild = self.slots.len() != POOL_SIZE
            || self
                .slots
                .iter()
                .any(|s| s.width != width || s.height != height);
        if !needs_rebuild {
            return Ok(());
        }
        self.slots.clear();
        self.busy.clear();
        let mut allocator =
            DmabufAllocator(GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING));

        // Prefer driver-native / tiled modifiers. Never allocate with LINEAR alone.
        let mut mods: Vec<Modifier> = Vec::new();
        mods.push(Modifier::Invalid);
        for m in preferred_modifiers {
            if *m != Modifier::Invalid && !mods.contains(m) {
                mods.push(*m);
            }
        }
        // LINEAR last-resort only (CPU readback / exotic paths) — not preferred.
        if !mods.contains(&Modifier::Linear) {
            mods.push(Modifier::Linear);
        }

        for _ in 0..POOL_SIZE {
            let dmabuf = allocator
                .create_buffer(width, height, self.fourcc, &mods)
                .or_else(|_| allocator.create_buffer(width, height, Fourcc::Argb8888, &mods))
                .map_err(|e| format!("stream export dmabuf allocate: {e}"))?;
            self.fourcc = AllocBuffer::format(&dmabuf).code;
            let got_mod = AllocBuffer::format(&dmabuf).modifier;
            self.slots.push(PoolSlot {
                dmabuf,
                width,
                height,
            });
            self.busy.push(false);
            tracing::debug!(?got_mod, "stream export: pool slot allocated");
        }
        tracing::info!(
            width,
            height,
            fourcc = ?self.fourcc,
            modifier = ?AllocBuffer::format(&self.slots[0].dmabuf).modifier,
            pool = POOL_SIZE,
            "stream export: GBM dmabuf pool ready (tiled-preferring)"
        );
        Ok(())
    }

    /// Acquire a free pool slot. Returns `None` if all slots are busy (skip frame).
    fn acquire(pool: &Arc<Mutex<PoolState>>) -> Option<(Dmabuf, Arc<SlotReleaseGuard>)> {
        let mut state = pool.lock().ok()?;
        let idx = state.busy.iter().position(|&b| !b)?;
        state.busy[idx] = true;
        let dmabuf = state.slots.get(idx)?.dmabuf.clone();
        drop(state);
        let guard = Arc::new(SlotReleaseGuard {
            slot_index: idx,
            pool: Arc::clone(pool),
        });
        Some((dmabuf, guard))
    }
}

/// Lock-free publish path from the DRM render thread to a consumer.
///
/// Frames land in `latest` (replacing any unread frame — its
/// [`SlotReleaseGuard`] runs on drop). A capacity-1 wake channel notifies the
/// consumer without ever blocking the producer.
pub struct StreamExportHub {
    enabled: AtomicBool,
    seq: AtomicU64,
    latest: Mutex<Option<ExportedFrame>>,
    wake_tx: Mutex<Option<SyncSender<()>>>,
    pool: Arc<Mutex<PoolState>>,
    debug_join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Default for StreamExportHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamExportHub {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            latest: Mutex::new(None),
            wake_tx: Mutex::new(None),
            pool: Arc::new(Mutex::new(PoolState::new())),
            debug_join: Mutex::new(None),
        }
    }

    pub fn is_armed(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Arm export and return a wake receiver. Read frames via [`Self::take_latest`].
    pub fn arm(&self) -> Receiver<()> {
        let (tx, rx) = mpsc::sync_channel(WAKE_CAP);
        if let Ok(mut slot) = self.wake_tx.lock() {
            *slot = Some(tx);
        }
        if let Ok(mut latest) = self.latest.lock() {
            *latest = None;
        }
        self.enabled.store(true, Ordering::SeqCst);
        tracing::info!("stream export: armed (latest-wins + slot RAII, wake cap {WAKE_CAP})");
        rx
    }

    pub fn disarm(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.wake_tx.lock() {
            *slot = None;
        }
        if let Ok(mut latest) = self.latest.lock() {
            *latest = None;
        }
        tracing::info!("stream export: disarmed");
    }

    /// Take the newest unread frame, if any.
    pub fn take_latest(&self) -> Option<ExportedFrame> {
        self.latest.lock().ok()?.take()
    }

    /// Start a background consumer that logs capture FPS and applies synthetic
    /// encode latency (Phase 1 proof of latest-wins + slot release).
    pub fn arm_debug_consumer(self: &Arc<Self>) {
        let wake = self.arm();
        let hub = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name("metis-stream-export".into())
            .spawn(move || debug_consumer_loop(hub, wake))
            .ok();
        if let Ok(mut slot) = self.debug_join.lock() {
            *slot = handle;
        }
    }

    fn try_publish(&self, frame: ExportedFrame) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        {
            let Ok(mut latest) = self.latest.lock() else {
                return;
            };
            // Replacing drops the stale frame → SlotReleaseGuard frees its BO.
            *latest = Some(frame);
        }
        let tx = {
            let Ok(guard) = self.wake_tx.lock() else {
                return;
            };
            guard.clone()
        };
        let Some(tx) = tx else {
            return;
        };
        match tx.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {
                self.enabled.store(false, Ordering::Relaxed);
            }
        }
    }
}

fn debug_consumer_loop(hub: Arc<StreamExportHub>, wake: Receiver<()>) {
    tracing::info!("stream export: debug consumer started");
    let mut last_log = Instant::now();
    let mut frame_count = 0u32;
    let mut last_meta: Option<(u32, u32, u64, u64, usize)> = None;
    while let Ok(()) | Err(RecvTimeoutError::Timeout) = wake.recv_timeout(Duration::from_secs(2)) {
        while let Some(frame) = hub.take_latest() {
            frame.wait_ready();
            frame_count = frame_count.saturating_add(1);
            last_meta = Some((
                frame.width,
                frame.height,
                frame.seq,
                frame.modifier,
                frame.fds.len(),
            ));
            // Synthetic encoder latency — proves backpressure frees pool slots.
            std::thread::sleep(DEBUG_ENCODE_SLEEP);
            // Frame drop → SlotReleaseGuard returns the BO to the pool.
        }
        if last_log.elapsed() >= Duration::from_secs(1) {
            if let Some((w, h, seq, modifier, planes)) = last_meta {
                tracing::info!(
                    fps = frame_count,
                    width = w,
                    height = h,
                    seq,
                    modifier,
                    planes,
                    "stream export: capture rate"
                );
            }
            frame_count = 0;
            last_log = Instant::now();
        }
    }
    tracing::info!("stream export: debug consumer stopped");
}

type PlaneExport = (Vec<OwnedFd>, Vec<u32>, Vec<u32>);

fn dup_planes(dmabuf: &Dmabuf) -> Result<PlaneExport, String> {
    let mut fds = Vec::with_capacity(dmabuf.num_planes());
    for handle in dmabuf.handles() {
        let owned = handle
            .try_clone_to_owned()
            .map_err(|e| format!("dup dmabuf plane fd: {e}"))?;
        fds.push(owned);
    }
    let offsets: Vec<u32> = dmabuf.offsets().collect();
    let strides: Vec<u32> = dmabuf.strides().collect();
    if fds.is_empty() || fds.len() != offsets.len() || fds.len() != strides.len() {
        return Err("dmabuf plane metadata mismatch".into());
    }
    Ok((fds, offsets, strides))
}

fn preferred_modifiers_for_export(state: &MetisState) -> Vec<Modifier> {
    let Some(udev) = state.udev.as_ref() else {
        return vec![Modifier::Invalid];
    };
    let preferred_codes = [
        Fourcc::Xrgb8888,
        Fourcc::Argb8888,
        Fourcc::Xbgr8888,
        Fourcc::Abgr8888,
    ];
    let mut mods = Vec::new();
    for fmt in udev.capture_dmabuf_formats.iter() {
        if preferred_codes.contains(&fmt.code) && !mods.contains(&fmt.modifier) {
            mods.push(fmt.modifier);
        }
    }
    if mods.is_empty() {
        mods.push(Modifier::Invalid);
    }
    mods
}

/// Second-pass compose of `output` into the export pool; publish if armed.
///
/// Call only after a successful local DRM `render_frame` for this output.
/// No-ops when disarmed, on non-primary outputs, or when GBM is unavailable.
/// Never blocks on GPU sync — fence / [`SyncPoint`] travel with the frame.
pub fn maybe_export_frame(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    render_node: DrmNode,
    output: &Output,
) {
    if !state.stream_export.is_armed() {
        return;
    }
    if output.name().as_str() != state.primary_key() {
        return;
    }

    let Some(gbm) = state
        .udev
        .as_ref()
        .and_then(|u| u.gbm_for_render_node(render_node))
        .cloned()
    else {
        tracing::debug!("stream export: no GBM for render node — skip");
        return;
    };

    let mode = match output.current_mode() {
        Some(m) => m,
        None => return,
    };
    let width = mode.size.w.max(0) as u32;
    let height = mode.size.h.max(0) as u32;
    if width == 0 || height == 0 {
        return;
    }

    let mods = preferred_modifiers_for_export(state);
    {
        let Ok(mut pool) = state.stream_export.pool.lock() else {
            return;
        };
        if let Err(err) = pool.ensure(&gbm, width, height, &mods) {
            tracing::warn!(%err, "stream export: pool ensure failed");
            return;
        }
    }

    let size_phys: Size<i32, Physical> = mode.size;
    let output_scale = Scale::from(output.current_scale().fractional_scale());
    let render_origin: Point<i32, Physical> = state
        .space
        .output_geometry(output)
        .map(|g| g.loc.to_physical_precise_round(output_scale))
        .unwrap_or_default();

    let mut elements = state.build_render_elements(
        renderer,
        render_origin,
        output_scale,
        crate::night_light::RenderTargetInfo {
            size: size_phys,
            output_name: Some(output.name().as_str()),
            skip_night_light: true,
        },
        &["metis-screenshot", "metis-screenshot-record"],
        true,
    );
    let mut cursor = state.build_cursor_elements(renderer, output, output_scale);
    if !cursor.is_empty() {
        cursor.append(&mut elements);
        elements = cursor;
    }

    let Some((mut dmabuf, slot_guard)) = PoolState::acquire(&state.stream_export.pool) else {
        tracing::debug!("stream export: pool exhausted — skip frame");
        return;
    };

    let sync = {
        let mut framebuffer = match renderer.bind(&mut dmabuf) {
            Ok(fb) => fb,
            Err(err) => {
                tracing::warn!(?err, "stream export: dmabuf bind failed");
                return;
            }
        };
        let mut damage_tracker =
            OutputDamageTracker::new(size_phys, output_scale, Transform::Normal);
        match damage_tracker.render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR) {
            Ok(r) => r.sync,
            Err(err) => {
                tracing::warn!(?err, "stream export: render_output failed");
                return;
            }
        }
        // Intentionally no sync.wait() here — fence travels with the frame.
    };

    let fence = sync.export();
    let (fds, offsets, strides) = match dup_planes(&dmabuf) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, "stream export: plane export failed");
            return;
        }
    };
    let fmt = AllocBuffer::format(&dmabuf);
    let stride = strides.first().copied().unwrap_or(width.saturating_mul(4));
    let seq = state.stream_export.seq.fetch_add(1, Ordering::Relaxed) + 1;
    let frame = ExportedFrame {
        seq,
        output_name: output.name().to_string(),
        width,
        height,
        stride,
        fourcc: fmt.code as u32,
        modifier: fmt.modifier.into(),
        fds,
        offsets,
        strides,
        damage_full: true,
        fence,
        sync,
        _slot_guard: Some(slot_guard),
    };
    state.stream_export.try_publish(frame);
}

/// Bootstrap from `METIS_STREAM_EXPORT=1` (debug consumer). Safe to call once
/// after DRM init.
pub fn maybe_arm_from_env(hub: &Arc<StreamExportHub>) {
    match std::env::var("METIS_STREAM_EXPORT") {
        Ok(v) if matches!(v.as_str(), "1" | "true" | "yes" | "debug") => {
            hub.arm_debug_consumer();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame(seq: u64) -> ExportedFrame {
        ExportedFrame {
            seq,
            output_name: "eDP-1".into(),
            width: 100,
            height: 100,
            stride: 400,
            fourcc: 0,
            modifier: 0,
            fds: Vec::new(),
            offsets: Vec::new(),
            strides: vec![400],
            damage_full: true,
            fence: None,
            sync: SyncPoint::signaled(),
            _slot_guard: None,
        }
    }

    #[test]
    fn try_publish_latest_wins() {
        let hub = StreamExportHub::new();
        let _wake = hub.arm();
        for seq in 1..=5u64 {
            hub.try_publish(test_frame(seq));
        }
        let got = hub.take_latest().expect("one frame queued");
        assert_eq!(got.seq, 5);
        assert!(hub.take_latest().is_none());
        hub.disarm();
    }

    #[test]
    fn disarmed_skips_publish_path() {
        let hub = StreamExportHub::new();
        assert!(!hub.is_armed());
        hub.try_publish(test_frame(1));
        assert!(hub.take_latest().is_none());
    }

    #[test]
    fn slot_release_guard_clears_busy() {
        let pool = Arc::new(Mutex::new(PoolState {
            slots: Vec::new(),
            busy: vec![true, true],
            fourcc: Fourcc::Xrgb8888,
        }));
        {
            let guard = Arc::new(SlotReleaseGuard {
                slot_index: 0,
                pool: Arc::clone(&pool),
            });
            assert!(pool.lock().unwrap().busy[0]);
            drop(guard);
        }
        assert!(!pool.lock().unwrap().busy[0]);
        assert!(pool.lock().unwrap().busy[1]);
    }

    #[test]
    fn latest_wins_drops_stale_slot_guard() {
        let pool = Arc::new(Mutex::new(PoolState {
            slots: Vec::new(),
            busy: vec![true, true],
            fourcc: Fourcc::Xrgb8888,
        }));
        let hub = StreamExportHub::new();
        let _wake = hub.arm();

        let mut a = test_frame(1);
        a._slot_guard = Some(Arc::new(SlotReleaseGuard {
            slot_index: 0,
            pool: Arc::clone(&pool),
        }));
        let mut b = test_frame(2);
        b._slot_guard = Some(Arc::new(SlotReleaseGuard {
            slot_index: 1,
            pool: Arc::clone(&pool),
        }));

        hub.try_publish(a);
        assert!(pool.lock().unwrap().busy[0]);
        hub.try_publish(b); // evicts seq=1 → frees slot 0
        assert!(!pool.lock().unwrap().busy[0]);
        assert!(pool.lock().unwrap().busy[1]);

        let got = hub.take_latest().expect("latest frame");
        assert_eq!(got.seq, 2);
        drop(got);
        assert!(!pool.lock().unwrap().busy[1]);
        hub.disarm();
    }
}
