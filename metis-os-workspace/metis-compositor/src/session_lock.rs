//! `ext-session-lock-v1` for optional third-party lockers (swaylock, gtklock, …).
//!
//! Metis’s compositor PAM lock ([`crate::lock`]) remains the default for
//! Super+L / idle / Settings. Only one lock owner is allowed per cycle: a
//! protocol lock is refused while PAM lock is active, and vice versa.

use std::collections::HashMap;

use smithay::{
    backend::renderer::{
        Color32F,
        element::{
            Id, Kind,
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{WindowSurfaceType, utils::under_from_surface_tree},
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{
            Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};

use crate::focus::KeyboardFocusTarget;
use crate::render::OutputStack;
use crate::state::MetisState;

/// Runtime state for an active (or pending) protocol session lock.
#[derive(Debug, Default)]
pub enum ProtocolLock {
    #[default]
    Unlocked,
    Locked {
        lock: ExtSessionLockV1,
        /// Per-output lock surfaces keyed by output name.
        surfaces: HashMap<String, LockSurface>,
        blank_id: Id,
        blank_commit: CommitCounter,
    },
}

impl ProtocolLock {
    pub fn is_locked(&self) -> bool {
        match self {
            Self::Unlocked => false,
            Self::Locked { lock, .. } => lock.is_alive(),
        }
    }

    fn surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        match self {
            Self::Locked { surfaces, .. } => surfaces.get(&output.name()),
            Self::Unlocked => None,
        }
    }
}

impl MetisState {
    /// True when either the Metis PAM lock or a protocol locker owns the session.
    pub fn session_is_locked(&self) -> bool {
        self.lock.locked || self.protocol_lock.is_locked()
    }

    /// Drop a dead protocol lock (client exited without `unlock_and_destroy`).
    pub(crate) fn protocol_lock_reap_dead(&mut self) {
        if let ProtocolLock::Locked { lock, .. } = &self.protocol_lock
            && !lock.is_alive()
        {
            tracing::warn!("protocol lock client died — unlocking session");
            self.protocol_lock_finish_unlock();
        }
    }

    fn protocol_lock_begin(&mut self, confirmation: SessionLocker) {
        if self.lock.locked {
            tracing::info!("refusing protocol lock while Metis PAM lock is active");
            return;
        }
        if self.protocol_lock.is_locked() {
            tracing::info!("refusing protocol lock — already locked by a locker");
            return;
        }

        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
        }

        let lock = confirmation.ext_session_lock().clone();
        confirmation.lock();
        self.protocol_lock = ProtocolLock::Locked {
            lock,
            surfaces: HashMap::new(),
            blank_id: Id::new(),
            blank_commit: CommitCounter::default(),
        };
        tracing::info!("protocol session lock active");
        crate::lock::spawn_metis_remote(&["pause"]);
        self.damaged = true;
        self.request_redraw();
    }

    fn protocol_lock_finish_unlock(&mut self) {
        if matches!(self.protocol_lock, ProtocolLock::Unlocked) {
            return;
        }
        self.protocol_lock = ProtocolLock::Unlocked;
        tracing::info!("protocol session lock released");
        crate::lock::spawn_metis_remote(&["resume"]);
        self.damaged = true;
        self.request_redraw();
    }

    pub(crate) fn configure_protocol_lock_surface(surface: &LockSurface, output: &Output) {
        let Some(mode) = output.current_mode() else {
            return;
        };
        let scale = output.current_scale().fractional_scale();
        let transform = output.current_transform();
        let logical: Size<i32, Logical> = transform
            .transform_size(mode.size.to_f64().to_logical(scale))
            .to_i32_round();
        surface.with_pending_state(|state| {
            state.size = Some(Size::from((
                logical.w.max(1) as u32,
                logical.h.max(1) as u32,
            )));
        });
        surface.send_configure();
    }

    fn protocol_lock_new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            tracing::warn!("protocol lock surface: no Output for WlOutput");
            return;
        };
        let ProtocolLock::Locked { lock, surfaces, .. } = &mut self.protocol_lock else {
            tracing::warn!("protocol lock surface while unlocked — ignoring");
            return;
        };
        if lock.client() != surface.wl_surface().client() {
            tracing::debug!("ignoring lock surface from unrelated client");
            return;
        }
        Self::configure_protocol_lock_surface(&surface, &output);
        surfaces.insert(output.name(), surface);
        self.damaged = true;
        self.request_redraw();
    }

    /// Blank + protocol lock surface(s) for the current render target.
    pub(crate) fn build_protocol_lock_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        render_origin: Point<i32, Physical>,
        target: &crate::night_light::RenderTargetInfo<'_>,
        output_scale: Scale<f64>,
    ) -> Vec<OutputStack> {
        let size = target.size;
        if size.w <= 0 || size.h <= 0 {
            return Vec::new();
        }

        let mut elems = Vec::new();
        let output = target
            .output_name
            .and_then(|name| self.output_by_name(name));
        let out_origin = output
            .as_ref()
            .and_then(|o| self.space.output_geometry(o))
            .map(|g| g.loc.to_physical_precise_round(output_scale))
            .unwrap_or_default();
        let loc = out_origin - render_origin;

        if let Some(ref out) = output
            && let Some(surface) = self.protocol_lock_surface_for_output(out)
        {
            let surface_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    loc,
                    output_scale,
                    1.0,
                    Kind::ScanoutCandidate,
                );
            elems.extend(surface_elems.into_iter().map(OutputStack::Surface));
        }

        let (blank_id, blank_commit) = match &self.protocol_lock {
            ProtocolLock::Locked {
                blank_id,
                blank_commit,
                ..
            } => (blank_id.clone(), *blank_commit),
            ProtocolLock::Unlocked => (Id::new(), CommitCounter::default()),
        };
        // Smithay draws front-to-back: blank goes last so it sits behind the locker.
        elems.push(OutputStack::Overlay(SolidColorRenderElement::new(
            blank_id,
            Rectangle::from_size(size),
            blank_commit,
            Color32F::from([0.0, 0.0, 0.0, 1.0]),
            Kind::Unspecified,
        )));
        elems
    }

    /// Hit-test protocol lock surfaces only (global logical coords).
    pub(crate) fn protocol_lock_surface_at(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let ProtocolLock::Locked { surfaces, .. } = &self.protocol_lock else {
            return None;
        };
        for output in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            if !geo.to_f64().contains(pos) {
                continue;
            }
            let Some(lock_surface) = surfaces.get(&output.name()) else {
                continue;
            };
            let rel = pos - geo.loc.to_f64();
            if let Some((surface, loc)) = under_from_surface_tree(
                lock_surface.wl_surface(),
                rel,
                (0, 0),
                WindowSurfaceType::ALL,
            ) {
                return Some((surface, loc.to_f64() + geo.loc.to_f64()));
            }
        }
        None
    }

    /// Prefer the lock surface on the output under the pointer for keyboard focus.
    pub(crate) fn protocol_lock_keyboard_focus(&self) -> Option<KeyboardFocusTarget> {
        let ProtocolLock::Locked { surfaces, .. } = &self.protocol_lock else {
            return None;
        };
        let pointer_loc = self.seat.get_pointer().map(|p| p.current_location());
        if let Some(pos) = pointer_loc
            && let Some(output) = self.space.outputs().find(|o| {
                self.space
                    .output_geometry(o)
                    .is_some_and(|g| g.to_f64().contains(pos))
            })
            && let Some(s) = surfaces.get(&output.name())
        {
            return Some(KeyboardFocusTarget::LockSurface(s.wl_surface().clone()));
        }
        surfaces
            .values()
            .next()
            .map(|s| KeyboardFocusTarget::LockSurface(s.wl_surface().clone()))
    }

    pub(crate) fn protocol_lock_surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        self.protocol_lock.surface_for_output(output)
    }

    pub(crate) fn send_protocol_lock_frames(&self, output: &Output, time: std::time::Duration) {
        let Some(surface) = self.protocol_lock_surface_for_output(output) else {
            return;
        };
        smithay::desktop::utils::send_frames_surface_tree(
            surface.wl_surface(),
            output,
            time,
            Some(std::time::Duration::from_millis(16)),
            |_, _| Some(output.clone()),
        );
    }
}

impl SessionLockHandler for MetisState {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.protocol_lock_begin(confirmation);
    }

    fn unlock(&mut self) {
        self.protocol_lock_finish_unlock();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        self.protocol_lock_new_surface(surface, output);
    }
}
