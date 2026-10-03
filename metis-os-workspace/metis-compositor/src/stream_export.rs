//! Phase 1–5 RUDP host capture: export the primary output's composited frame as
//! dmabuf plane FDs for the encode worker — **no PipeWire / portal**.
//!
//! Scanout still owns the KMS buffer. When armed, we run a second
//! `OutputStack` compose into a dedicated GBM dmabuf pool (same pattern as
//! [`crate::image_capture`]), then publish an [`ExportedFrame`] on a
//! latest-wins slot + wake channel (never blocks the render thread).
//!
//! Phase 5: a **persistent** [`OutputDamageTracker`] classifies scene damage
//! (skip empty / sparse vs full). Compose into the rotating pool remains
//! **full-frame** — partial GLES redraw into a fresh BO would leave undefined
//! pixels.
//!
//! Guardrails:
//! - Armed is not enough: nothing is composed until a consumer sets demand
//!   ([`StreamExportHub::set_demand`]) — enabling Metis Remote with no client
//!   connected adds zero GPU work to the session.
//! - LINEAR GBM buffers only for the export pool (CPU-read by the isolated
//!   encode worker); CCS/tiled BOs as GLES targets have killed the DRM session
//!   on Intel.
//! - Do not `sync.wait()` on the render thread — export an explicit fence FD
//!   (and keep [`SyncPoint`] for the consumer when export is unavailable).
//! - Pool slots use [`SlotReleaseGuard`] so dropped / superseded frames free
//!   their BO immediately under latest-wins backpressure.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metis_encode::DamageRect;
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
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};

use crate::render::CLEAR_COLOR;
use crate::state::MetisState;

const POOL_SIZE: usize = 3;
/// Wake channel depth — payload lives in `latest` (true latest-wins).
const WAKE_CAP: usize = 1;
/// Synthetic encode latency in the debug consumer (proves slot RAII under load).
const DEBUG_ENCODE_SLEEP: Duration = Duration::from_millis(6);
/// Damage area / output area at or above this → `damage_full` (gaming / CAD).
pub const DAMAGE_FULL_THRESHOLD: f64 = 0.35;
/// More than this many rects → treat as full-frame (metadata cap).
pub const DAMAGE_RECT_CAP: usize = 32;

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
    /// True when damage covers a large fraction of the output (or forced full).
    pub damage_full: bool,
    /// Sparse physical dirty rects when `!damage_full`; empty when full.
    pub damage: Vec<DamageRect>,
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
            .field("damage_rects", &self.damage.len())
            .field("has_fence", &self.fence.is_some())
            .field("slot", &self._slot_guard.as_ref().map(|g| g.slot_index))
            .finish()
    }
}

impl ExportedFrame {
    /// Block (encode / debug thread only) until GPU work for this frame is
    /// complete or `timeout` elapses. Returns `false` on timeout / error — the
    /// caller must drop the frame rather than read a half-rendered buffer.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        if let Some(fence) = &self.fence {
            return wait_sync_file(fence, timeout);
        }
        self.sync.wait().is_ok()
    }
}

/// Classify scene damage for export / encode metadata.
///
/// Empty `rects` → caller should skip export. More than [`DAMAGE_RECT_CAP`]
/// rects or coverage ≥ [`DAMAGE_FULL_THRESHOLD`] → `damage_full` with empty
/// rect list.
pub fn classify_export_damage(
    rects: Vec<DamageRect>,
    output_w: i32,
    output_h: i32,
) -> (bool, Vec<DamageRect>) {
    if rects.is_empty() {
        return (false, rects);
    }
    if rects.len() > DAMAGE_RECT_CAP {
        return (true, Vec::new());
    }
    let output_area = (output_w.max(0) as u64)
        .saturating_mul(output_h.max(0) as u64)
        .max(1);
    let damaged: u64 = rects.iter().map(|r| r.area()).sum();
    let coverage = damaged as f64 / output_area as f64;
    if coverage >= DAMAGE_FULL_THRESHOLD {
        (true, Vec::new())
    } else {
        (false, rects)
    }
}

fn rects_from_smithay(rects: &[Rectangle<i32, Physical>]) -> Vec<DamageRect> {
    rects
        .iter()
        .map(|r| DamageRect {
            x: r.loc.x,
            y: r.loc.y,
            w: r.size.w,
            h: r.size.h,
        })
        .collect()
}

/// Poll a sync_file until signalled. A wedged GPU must not wedge the encode
/// thread (compositor shutdown joins it), so the wait is bounded.
fn wait_sync_file(fd: &OwnedFd, timeout: Duration) -> bool {
    use std::os::fd::AsRawFd;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::warn!(
                ?timeout,
                "stream export: GPU fence not signalled — dropping frame"
            );
            return false;
        }
        let mut pollfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        // SAFETY: one valid pollfd on an fd we own.
        let rc = unsafe { libc::poll(&mut pollfd, 1, ms) };
        if rc > 0 {
            return pollfd.revents & (libc::POLLERR | libc::POLLNVAL) == 0;
        }
        if rc == 0 {
            continue;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            tracing::debug!(?err, "stream export: sync_file poll failed");
            return false;
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

/// Persistent scene-damage tracker (separate from full-frame pool compose).
struct ExportDamageState {
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    tracker: OutputDamageTracker,
    /// Next `damage_output` age: 0 after reset, then 1.
    first: bool,
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
        _preferred_modifiers: &[Modifier],
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

        // Stream export feeds remote encode (VAAPI/NVENC), not local scanout.
        // LINEAR ONLY — never pass Modifier::Invalid. GBM treats Invalid as "driver
        // pick" and often returns I915_y_tiled; composing into that BO with GLES has
        // aborted the DRM session (blank screen after wallpaper).
        let linear = [Modifier::Linear];

        for _ in 0..POOL_SIZE {
            let dmabuf = allocator
                .create_buffer(width, height, self.fourcc, &linear)
                .or_else(|_| allocator.create_buffer(width, height, Fourcc::Argb8888, &linear))
                .map_err(|e| {
                    format!("stream export dmabuf allocate (LINEAR required for RUDP): {e}")
                })?;
            self.fourcc = AllocBuffer::format(&dmabuf).code;
            let got_mod = AllocBuffer::format(&dmabuf).modifier;
            if got_mod != Modifier::Linear {
                return Err(format!(
                    "stream export: expected LINEAR dmabuf, got {got_mod:?} — refusing tiled export"
                ));
            }
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
            "stream export: GBM dmabuf pool ready (LINEAR only for encode)"
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
    /// A consumer actually wants frames (e.g. an authenticated RUDP client).
    /// Armed-but-idle costs nothing: no second compose, no pool BOs touched.
    demand: AtomicBool,
    /// Next export ignores scene damage and publishes a full frame (client
    /// join / encoder restart — the decoder needs a complete picture).
    force_full: AtomicBool,
    seq: AtomicU64,
    latest: Mutex<Option<ExportedFrame>>,
    wake_tx: Mutex<Option<SyncSender<()>>>,
    pool: Arc<Mutex<PoolState>>,
    damage: Mutex<Option<ExportDamageState>>,
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
            demand: AtomicBool::new(false),
            force_full: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            latest: Mutex::new(None),
            wake_tx: Mutex::new(None),
            pool: Arc::new(Mutex::new(PoolState::new())),
            damage: Mutex::new(None),
            debug_join: Mutex::new(None),
        }
    }

    pub fn is_armed(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Armed **and** a consumer is waiting — only then compose export frames.
    pub fn wants_frames(&self) -> bool {
        self.is_armed() && self.demand.load(Ordering::Relaxed)
    }

    /// Start / stop producing frames. Rising edge forces a full frame.
    pub fn set_demand(&self, wanted: bool) {
        let was = self.demand.swap(wanted, Ordering::SeqCst);
        if wanted && !was {
            self.reset_damage_tracker();
            self.force_full.store(true, Ordering::SeqCst);
            tracing::info!("stream export: consumer attached — producing frames");
        } else if !wanted && was {
            if let Ok(mut latest) = self.latest.lock() {
                *latest = None;
            }
            tracing::info!("stream export: no consumer — export idle");
        }
    }

    /// Ask for the next export to be a full frame even without scene damage.
    pub fn request_full_frame(&self) {
        self.force_full.store(true, Ordering::SeqCst);
    }

    /// True when the render loop should export even if the local frame was empty.
    pub fn forced_frame_pending(&self) -> bool {
        self.wants_frames() && self.force_full.load(Ordering::Relaxed)
    }

    fn reset_damage_tracker(&self) {
        if let Ok(mut slot) = self.damage.lock() {
            *slot = None;
        }
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
        self.reset_damage_tracker();
        self.demand.store(false, Ordering::SeqCst);
        self.force_full.store(false, Ordering::SeqCst);
        self.enabled.store(true, Ordering::SeqCst);
        tracing::info!("stream export: armed (latest-wins + slot RAII, wake cap {WAKE_CAP})");
        rx
    }

    pub fn disarm(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.demand.store(false, Ordering::SeqCst);
        self.force_full.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.wake_tx.lock() {
            *slot = None;
        }
        if let Ok(mut latest) = self.latest.lock() {
            *latest = None;
        }
        self.reset_damage_tracker();
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
        self.set_demand(true);
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
            if !frame.wait_ready(Duration::from_millis(500)) {
                continue;
            }
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

fn preferred_modifiers_for_export(_state: &MetisState) -> Vec<Modifier> {
    // Allocation insists on LINEAR alone; keep the helper for API stability.
    vec![Modifier::Linear]
}

/// Second-pass compose of `output` into the export pool; publish if armed.
///
/// Call only after a successful local DRM `render_frame` for this output.
/// No-ops when disarmed, on non-primary outputs, or when GBM is unavailable.
/// Never blocks on GPU sync — fence / [`SyncPoint`] travel with the frame.
///
/// Skips publish when the persistent damage tracker reports no scene change,
/// unless a full frame was requested ([`StreamExportHub::request_full_frame`]).
/// Does nothing at all until a consumer signals demand.
pub fn maybe_export_frame(
    state: &mut MetisState,
    renderer: &mut GlesRenderer,
    render_node: DrmNode,
    output: &Output,
) {
    if !state.stream_export.wants_frames() {
        return;
    }
    let forced = state.stream_export.force_full.load(Ordering::Relaxed);
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
    let transform = Transform::Normal;
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

    // Scene damage (persistent tracker) — skip before pool acquire when idle.
    let (damage_full, damage) = {
        let Ok(mut slot) = state.stream_export.damage.lock() else {
            return;
        };
        let need_new = match slot.as_ref() {
            None => true,
            Some(s) => s.size != size_phys || s.scale != output_scale || s.transform != transform,
        };
        if need_new {
            *slot = Some(ExportDamageState {
                size: size_phys,
                scale: output_scale,
                transform,
                tracker: OutputDamageTracker::new(size_phys, output_scale, transform),
                first: true,
            });
        }
        let Some(dmg) = slot.as_mut() else {
            return;
        };
        let age = if dmg.first { 0 } else { 1 };
        let classified = match dmg.tracker.damage_output(age, &elements) {
            Ok((None, _)) => {
                dmg.first = false;
                None
            }
            Ok((Some(rects), _)) if rects.is_empty() => {
                dmg.first = false;
                None
            }
            Ok((Some(rects), _)) => {
                dmg.first = false;
                Some(classify_export_damage(
                    rects_from_smithay(rects),
                    size_phys.w,
                    size_phys.h,
                ))
            }
            Err(err) => {
                tracing::warn!(?err, "stream export: damage_output failed — full frame");
                dmg.first = false;
                Some((true, Vec::new()))
            }
        };
        match (classified, forced) {
            (_, true) => (true, Vec::new()),
            (None, false) => return,
            (Some(v), false) => v,
        }
    };

    let Some((mut dmabuf, slot_guard)) = PoolState::acquire(&state.stream_export.pool) else {
        tracing::debug!("stream export: pool exhausted — skip frame");
        return;
    };

    // Full-frame compose into the pool BO (age 0 / throwaway tracker).
    let sync = {
        let mut framebuffer = match renderer.bind(&mut dmabuf) {
            Ok(fb) => fb,
            Err(err) => {
                tracing::warn!(?err, "stream export: dmabuf bind failed");
                return;
            }
        };
        let mut compose_tracker =
            OutputDamageTracker::new(size_phys, output_scale, Transform::Normal);
        match compose_tracker.render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR) {
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
        damage_full,
        damage,
        fence,
        sync,
        _slot_guard: Some(slot_guard),
    };
    if forced {
        state
            .stream_export
            .force_full
            .store(false, Ordering::SeqCst);
    }
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
            damage: Vec::new(),
            fence: None,
            sync: SyncPoint::signaled(),
            _slot_guard: None,
        }
    }

    #[test]
    fn classify_empty_is_not_full() {
        let (full, rects) = classify_export_damage(Vec::new(), 100, 100);
        assert!(!full);
        assert!(rects.is_empty());
    }

    #[test]
    fn classify_sparse_below_threshold() {
        // 10x10 = 100 of 10000 → 1%
        let (full, rects) = classify_export_damage(
            vec![DamageRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            }],
            100,
            100,
        );
        assert!(!full);
        assert_eq!(rects.len(), 1);
    }

    #[test]
    fn classify_high_coverage_is_full() {
        // 60x60 = 3600 of 10000 → 36%
        let (full, rects) = classify_export_damage(
            vec![DamageRect {
                x: 0,
                y: 0,
                w: 60,
                h: 60,
            }],
            100,
            100,
        );
        assert!(full);
        assert!(rects.is_empty());
    }

    #[test]
    fn classify_too_many_rects_is_full() {
        let rects: Vec<_> = (0..DAMAGE_RECT_CAP + 1)
            .map(|i| DamageRect {
                x: i as i32,
                y: 0,
                w: 1,
                h: 1,
            })
            .collect();
        let (full, out) = classify_export_damage(rects, 1920, 1080);
        assert!(full);
        assert!(out.is_empty());
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
    fn demand_gates_frames_and_forces_full() {
        let hub = StreamExportHub::new();
        let _wake = hub.arm();
        assert!(hub.is_armed());
        assert!(!hub.wants_frames(), "armed alone must not compose");
        assert!(!hub.forced_frame_pending());
        hub.set_demand(true);
        assert!(hub.wants_frames());
        assert!(
            hub.forced_frame_pending(),
            "consumer attach forces a full frame"
        );
        hub.try_publish(test_frame(1));
        hub.set_demand(false);
        assert!(!hub.wants_frames());
        assert!(hub.take_latest().is_none(), "demand off drops queued frame");
        hub.disarm();
        assert!(!hub.forced_frame_pending());
    }

    #[test]
    fn fence_wait_is_bounded() {
        // An unsignalled pipe read end stands in for a wedged GPU fence.
        let mut fds = [0i32; 2];
        // SAFETY: plain pipe(2) into a local array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: fresh fds we own.
        let (read, _write) = unsafe {
            use std::os::fd::FromRawFd;
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        let started = Instant::now();
        assert!(!wait_sync_file(&read, Duration::from_millis(50)));
        assert!(started.elapsed() < Duration::from_secs(1));
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
