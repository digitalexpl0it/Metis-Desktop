//! `zwlr_foreign_toplevel_management_v1` for third-party docks.
//!
//! Complements read-only [`crate::foreign_toplevel`] (`ext-foreign-toplevel-list-v1`).
//! Metis shell/taskbar stays on `metis-protocol` IPC. Same-euid bind filter.

use std::collections::HashMap;
use std::sync::Arc;

use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use smithay::reexports::wayland_server::{
    Client, DataInit, DisplayHandle, New, Resource, Weak, backend::ClientId, backend::GlobalId,
};
use smithay::wayland::{Dispatch2, GlobalDispatch2};

use crate::state::MetisState;

const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug)]
pub struct ForeignToplevelMgmtState {
    #[allow(dead_code)]
    global: GlobalId,
    managers: Vec<Weak<ZwlrForeignToplevelManagerV1>>,
    /// Per window id → live handle instances (one per manager client).
    handles: HashMap<u32, Vec<Weak<ZwlrForeignToplevelHandleV1>>>,
}

#[derive(Clone)]
pub struct ManagerGlobalData {
    client_filter: Arc<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl std::fmt::Debug for ManagerGlobalData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagerGlobalData")
    }
}

#[derive(Debug, Clone)]
struct ManagerData;

#[derive(Debug, Clone)]
struct HandleData {
    window_id: u32,
}

impl ForeignToplevelMgmtState {
    pub fn new(dh: &DisplayHandle) -> Self {
        let dh_for_filter = dh.clone();
        let client_filter =
            Arc::new(
                move |client: &Client| match client.get_credentials(&dh_for_filter) {
                    Ok(creds) => creds.uid == unsafe { libc::geteuid() },
                    Err(_) => false,
                },
            );
        let global = dh.create_global::<MetisState, ZwlrForeignToplevelManagerV1, _>(
            PROTOCOL_VERSION,
            ManagerGlobalData { client_filter },
        );
        Self {
            global,
            managers: Vec::new(),
            handles: HashMap::new(),
        }
    }

    fn retain_live_managers(&mut self) {
        self.managers.retain(|m| m.upgrade().is_ok());
    }

    fn retain_live_handles(&mut self, id: u32) {
        if let Some(list) = self.handles.get_mut(&id) {
            list.retain(|h| h.upgrade().is_ok());
            if list.is_empty() {
                self.handles.remove(&id);
            }
        }
    }
}

impl MetisState {
    /// Announce a window to all bound foreign-toplevel managers.
    pub(crate) fn foreign_mgmt_announce(&mut self, id: u32) {
        self.foreign_toplevel_mgmt.retain_live_managers();
        let managers: Vec<_> = self
            .foreign_toplevel_mgmt
            .managers
            .iter()
            .filter_map(|m| m.upgrade().ok())
            .collect();
        for manager in managers {
            self.foreign_mgmt_send_toplevel_to_manager(id, &manager);
        }
    }

    fn foreign_mgmt_send_toplevel_to_manager(
        &mut self,
        id: u32,
        manager: &ZwlrForeignToplevelManagerV1,
    ) {
        let Some(client) = manager.client() else {
            return;
        };
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, MetisState>(
            &self.display_handle,
            manager.version(),
            HandleData { window_id: id },
        ) else {
            return;
        };
        manager.toplevel(&handle);
        self.foreign_mgmt_send_initial(&handle, id);
        self.foreign_toplevel_mgmt
            .handles
            .entry(id)
            .or_default()
            .push(handle.downgrade());
    }

    fn foreign_mgmt_send_initial(&self, handle: &ZwlrForeignToplevelHandleV1, id: u32) {
        let Some(record) = self.windows.get(id) else {
            return;
        };
        handle.title(record.title.clone());
        handle.app_id(record.app_id.clone().unwrap_or_default());
        self.foreign_mgmt_send_state(handle, id);
        handle.done();
    }

    fn foreign_mgmt_send_state(&self, handle: &ZwlrForeignToplevelHandleV1, id: u32) {
        use zwlr_foreign_toplevel_handle_v1::State as S;
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let mut states: Vec<u32> = Vec::new();
        if record.minimized {
            states.push(S::Minimized as u32);
        }
        if record.maximized {
            states.push(S::Maximized as u32);
        }
        if record.fullscreen {
            states.push(S::Fullscreen as u32);
        }
        if self.focused_window_id() == Some(id) {
            states.push(S::Activated as u32);
        }
        let mut bytes = Vec::with_capacity(states.len() * 4);
        for s in states {
            bytes.extend_from_slice(&s.to_ne_bytes());
        }
        handle.state(bytes);
    }

    /// Push title / app_id / state to management clients.
    pub(crate) fn foreign_mgmt_sync(&mut self, id: u32) {
        self.foreign_toplevel_mgmt.retain_live_handles(id);
        let handles: Vec<_> = self
            .foreign_toplevel_mgmt
            .handles
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|h| h.upgrade().ok())
            .collect();
        for handle in handles {
            let Some(record) = self.windows.get(id) else {
                continue;
            };
            handle.title(record.title.clone());
            handle.app_id(record.app_id.clone().unwrap_or_default());
            self.foreign_mgmt_send_state(&handle, id);
            handle.done();
        }
    }

    /// Close management handles for a destroyed window.
    pub(crate) fn foreign_mgmt_close(&mut self, id: u32) {
        if let Some(list) = self.foreign_toplevel_mgmt.handles.remove(&id) {
            for weak in list {
                if let Ok(handle) = weak.upgrade() {
                    handle.closed();
                }
            }
        }
    }
}

impl GlobalDispatch2<ZwlrForeignToplevelManagerV1, MetisState> for ManagerGlobalData {
    fn can_view(&self, client: &Client) -> bool {
        (self.client_filter)(client)
    }

    fn bind(
        &self,
        state: &mut MetisState,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        data_init: &mut DataInit<'_, MetisState>,
    ) {
        let manager = data_init.init(resource, ManagerData);
        state
            .foreign_toplevel_mgmt
            .managers
            .push(manager.downgrade());
        let ids: Vec<u32> = state.windows.ids();
        for id in ids {
            if state
                .windows
                .get(id)
                .is_some_and(|r| r.foreign_toplevel.is_some())
            {
                state.foreign_mgmt_send_toplevel_to_manager(id, &manager);
            }
        }
    }
}

impl Dispatch2<ZwlrForeignToplevelManagerV1, MetisState> for ManagerData {
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, MetisState>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            resource.finished();
            state
                .foreign_toplevel_mgmt
                .managers
                .retain(|m| m.upgrade().ok().as_ref() != Some(resource));
        }
    }

    fn destroyed(
        &self,
        state: &mut MetisState,
        _client: ClientId,
        resource: &ZwlrForeignToplevelManagerV1,
    ) {
        state
            .foreign_toplevel_mgmt
            .managers
            .retain(|m| m.upgrade().ok().as_ref() != Some(resource));
    }
}

impl Dispatch2<ZwlrForeignToplevelHandleV1, MetisState> for HandleData {
    fn request(
        &self,
        state: &mut MetisState,
        _client: &Client,
        resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, MetisState>,
    ) {
        let id = self.window_id;
        if state.windows.get(id).is_none() {
            return;
        }
        match request {
            zwlr_foreign_toplevel_handle_v1::Request::SetMaximized => {
                state.set_maximized(id, true);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::UnsetMaximized => {
                state.set_maximized(id, false);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::SetMinimized => {
                state.minimize_by_id(id);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::UnsetMinimized => {
                state.activate_window_by_id(id);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::Activate { .. } => {
                state.activate_window_by_id(id);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::Close => {
                state.close_window(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::SetRectangle { .. } => {
                // Icon geometry hint — accepted no-op.
            }
            zwlr_foreign_toplevel_handle_v1::Request::SetFullscreen { .. } => {
                state.set_fullscreen(id, true, None);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::UnsetFullscreen => {
                state.set_fullscreen(id, false, None);
                state.foreign_mgmt_sync(id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::Destroy => {
                if let Some(list) = state.foreign_toplevel_mgmt.handles.get_mut(&id) {
                    list.retain(|h| h.upgrade().ok().as_ref() != Some(resource));
                }
            }
            _ => {}
        }
    }
}
