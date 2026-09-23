mod compositor;
mod layer_shell;
mod xdg_shell;

pub use layer_shell::handle_layer_commit;

use std::os::unix::io::OwnedFd;

use crate::clipboard::{MetisSelectionUserData, serve_compositor_selection};
use crate::state::MetisState;
use smithay::wayland::selection::data_device::current_data_device_selection_userdata;
use smithay::wayland::selection::primary_selection::current_primary_selection_userdata;

use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::pointer::{Focus, PointerHandle};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::{
    ConstraintRemove, PointerConstraintsHandler, with_pointer_constraint,
};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};

use crate::focus::KeyboardFocusTarget;

impl SeatHandler for MetisState {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocusTarget>) {
        let dh = &self.display_handle;
        let client = focused
            .and_then(|target| target.wl_surface())
            .and_then(|surface| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}

impl MetisState {
    /// Point the seat's clipboard + primary-selection devices at the client under
    /// `loc` so paste (right/middle-click, Shift+Insert) works even when keyboard
    /// focus was previously elsewhere.
    pub(crate) fn sync_selection_focus_at(&mut self, loc: Point<f64, Logical>) {
        let under = self.pointer_target_at(loc);
        self.sync_selection_focus_from_target(&under);
    }

    /// Align data-device and primary-selection focus with an already-resolved
    /// pointer target (avoids a second hit-test that can disagree with motion).
    pub(crate) fn sync_selection_focus_from_target(
        &mut self,
        under: &Option<(WlSurface, Point<f64, Logical>)>,
    ) {
        let dh = &self.display_handle;
        let client = under
            .as_ref()
            .and_then(|(surface, _)| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, &self.seat, client.clone());
        set_primary_focus(dh, &self.seat, client);
    }
}

impl SelectionHandler for MetisState {
    type SelectionUserData = MetisSelectionUserData;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if ty == SelectionTarget::Clipboard
            && let Some(ref source) = source
        {
            self.queue_clipboard_capture(source.mime_types());
        }
        if let Some(xwm) = self.xwm.as_mut()
            && let Err(err) = xwm.new_selection(ty, source.map(|s| s.mime_types()))
        {
            tracing::warn!(?err, ?ty, "failed to mirror Wayland selection to XWayland");
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &Self::SelectionUserData,
    ) {
        let fd = match serve_compositor_selection(user_data, &mime_type, fd) {
            Ok(()) => return,
            Err(fd) => fd,
        };

        let compositor_owned = match ty {
            SelectionTarget::Clipboard => {
                current_data_device_selection_userdata(&self.seat).is_some()
            }
            SelectionTarget::Primary => current_primary_selection_userdata(&self.seat).is_some(),
        };
        if compositor_owned {
            if !user_data.has_payload() {
                // XWayland advertised Wayland mimes; bytes live on the X11 side.
                if let Some(xwm) = self.xwm.as_mut()
                    && let Err(err) = xwm.send_selection(ty, mime_type, fd)
                {
                    tracing::warn!(?err, ?ty, "failed to read X11 selection for Wayland paste");
                }
                return;
            }
            tracing::warn!(
                ?ty,
                %mime_type,
                "compositor selection: unsupported mime for recall payload"
            );
            return;
        }

        if let Some(xwm) = self.xwm.as_mut()
            && let Err(err) = xwm.send_selection(ty, mime_type, fd)
        {
            tracing::warn!(?err, ?ty, "failed to send Wayland selection to XWayland");
        }
    }
}

impl PrimarySelectionHandler for MetisState {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl DataDeviceHandler for MetisState {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl DndGrabHandler for MetisState {}
impl WaylandDndGrabHandler for MetisState {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let start_data = ptr.grab_start_data().unwrap();
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                source.cancel();
            }
        }
        let _ = icon;
    }
}

impl OutputHandler for MetisState {}

impl smithay::wayland::drm_syncobj::DrmSyncobjHandler for MetisState {
    fn drm_syncobj_state(&mut self) -> Option<&mut smithay::wayland::drm_syncobj::DrmSyncobjState> {
        self.drm_syncobj_state.as_mut()
    }
}

impl PointerConstraintsHandler for MetisState {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        use smithay::reexports::wayland_server::Resource;
        self.pointer_constraint_phases.insert(
            surface.id(),
            crate::state::PointerConstraintPhase::NeverActivated,
        );
        self.trace_game_pointer(surface, pointer, "new pointer constraint", None);
        let mut activated = false;
        if pointer.current_focus().as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |constraint| {
                if let Some(constraint) = constraint {
                    constraint.activate();
                    self.pointer_constraint_phases
                        .insert(surface.id(), crate::state::PointerConstraintPhase::Active);
                    activated = true;
                }
            });
        }
        if activated {
            self.trace_game_pointer_at(
                surface,
                "new constraint activated (surface already focused)",
                None,
                Some(crate::state::PointerConstraintPhase::Active),
                true,
                true,
            );
        }
    }

    fn remove_constraint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        removal: ConstraintRemove,
    ) {
        use smithay::reexports::wayland_server::Resource;
        // Pointer-leave only deactivates the constraint; motion re-activates it
        // on re-entry. Tear-down below is for the client destroying it.
        if matches!(removal, ConstraintRemove::PointerLeave(_)) {
            self.trace_game_pointer(
                surface,
                pointer,
                "pointer constraint deactivated (leave)",
                None,
            );
            return;
        }
        let surface_id = surface.id();
        self.trace_game_pointer(surface, pointer, "pointer constraint removed", None);
        self.pointer_constraint_phases.remove(&surface_id);
        if self.last_pointer_motion_surface.as_ref() == Some(&surface_id) {
            self.last_pointer_motion_surface = None;
        }
        let should_restore =
            with_pointer_constraint(surface, pointer, |constraint| constraint.is_none());
        if should_restore
            && let Some((hint_surface, hint_location)) = self.cursor_position_hint.take()
            && let Some(origin) = self.surface_space_origin(&hint_surface)
        {
            let restore = origin + hint_location;
            self.trace_game_pointer_at(
                surface,
                "restoring cursor from position hint",
                Some(restore),
                None,
                false,
                false,
            );
            pointer.set_location(restore);
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        use smithay::reexports::wayland_server::Resource;
        if with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|c| c.is_active())
        }) {
            self.cursor_position_hint = Some((surface.clone(), location));
            // Hint is for unlock restore only. Do not log every update at INFO —
            // Proton floods these during mouse-look.
            tracing::debug!(
                ?location,
                surface_id = ?surface.id(),
                "cursor position hint updated"
            );
        }
    }
}

smithay::delegate_dispatch2!(MetisState);
