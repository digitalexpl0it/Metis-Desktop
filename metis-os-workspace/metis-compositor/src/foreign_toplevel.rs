//! `ext-foreign-toplevel-list-v1` projection of [`crate::windows::WindowRegistry`].
//!
//! Metis shell/taskbar stays on `metis-protocol` IPC. This global is a read-only
//! Wayland export for third-party list clients (no activate/close).

use smithay::wayland::foreign_toplevel_list::{
    ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState,
};

use crate::state::MetisState;
use crate::surface_scale::foreign_toplevel_identifier;

impl ForeignToplevelListHandler for MetisState {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevel_list_state
    }
}

impl MetisState {
    /// Announce a ready window on the foreign-toplevel list (idempotent).
    pub(crate) fn foreign_toplevel_announce(&mut self, id: u32) {
        if self
            .windows
            .get(id)
            .is_some_and(|r| r.foreign_toplevel.is_some())
        {
            return;
        }
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let title = record.title.clone();
        let app_id = record.app_id.clone().unwrap_or_default();
        let identifier = foreign_toplevel_identifier(id);
        let handle = self
            .foreign_toplevel_list_state
            .new_toplevel_with_identifier::<MetisState>(title, app_id, identifier);
        self.windows.set_foreign_toplevel(id, Some(handle));
        self.foreign_mgmt_announce(id);
    }

    /// Push title/app_id changes to an existing foreign handle.
    pub(crate) fn foreign_toplevel_sync_metadata(&mut self, id: u32) {
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let title = record.title.clone();
        let app_id = record.app_id.clone().unwrap_or_default();
        let Some(handle) = record.foreign_toplevel.clone() else {
            // Not announced yet (not ready) — nothing to sync.
            return;
        };
        handle.send_title(&title);
        handle.send_app_id(&app_id);
        handle.send_done();
        self.foreign_mgmt_sync(id);
    }

    /// Close the foreign handle when the window leaves the registry.
    pub(crate) fn foreign_toplevel_close(&mut self, handle: Option<ForeignToplevelHandle>) {
        let Some(handle) = handle else {
            return;
        };
        self.foreign_toplevel_list_state.remove_toplevel(&handle);
    }

    /// Close list + management handles for `id`.
    pub(crate) fn foreign_toplevel_close_id(
        &mut self,
        id: u32,
        handle: Option<ForeignToplevelHandle>,
    ) {
        self.foreign_mgmt_close(id);
        self.foreign_toplevel_close(handle);
    }
}
