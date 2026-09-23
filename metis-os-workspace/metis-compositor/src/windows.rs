use std::collections::HashMap;

use metis_grid::PixelRect;
use metis_protocol::WindowInfo;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::xwayland::X11Surface;
use smithay::{desktop::Window, wayland::shell::xdg::ToplevelSurface};

/// The shell surface backing a managed window. Native clients drive an
/// `xdg_toplevel`; XWayland clients drive an `X11Surface`. Both are mapped into
/// the same `Space<Window>` and share Metis's registry, decoration, placement,
/// grab and IPC machinery — only the "tell the client its geometry / state"
/// leaf calls differ, so they are funneled through helpers on [`MetisState`].
#[derive(Clone)]
pub enum WindowSurface {
    Wayland(ToplevelSurface),
    X11(Box<X11Surface>),
}

impl WindowSurface {
    pub fn wayland(&self) -> Option<&ToplevelSurface> {
        match self {
            WindowSurface::Wayland(toplevel) => Some(toplevel),
            WindowSurface::X11(_) => None,
        }
    }

    pub fn x11(&self) -> Option<&X11Surface> {
        match self {
            WindowSurface::X11(surface) => Some(surface.as_ref()),
            WindowSurface::Wayland(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct WindowRecord {
    pub id: u32,
    pub window: Window,
    pub surface: WindowSurface,
    pub title: String,
    pub app_id: Option<String>,
    pub target_rect: PixelRect,
    pub restore_rect: Option<PixelRect>,
    /// Pre-snap floating geometry is in `restore_rect`; this flag is set while the
    /// window occupies a snap/maximize layout so a titlebar drag can restore size.
    pub snapped: bool,
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    /// Virtual workspace this window lives on (1-based), scoped to its `output`.
    /// A window is mapped into `Space` only while its output's active workspace
    /// equals this value.
    pub workspace: u32,
    /// Name of the output (monitor) this window belongs to. Empty until assigned
    /// at registration. Per-output workspaces key off `(output, workspace)`.
    pub output: String,
    /// True after the first buffer commit; probe toplevels that never commit are dropped quietly.
    pub ready: bool,
    /// True once [`MetisState::place_new_window`] has decided float-vs-tile for this
    /// window. Prevents layout toggles (which clear `floating`) from re-centering.
    pub placement_chosen: bool,
    /// When true, Metis draws server-side titlebar/border chrome and insets the
    /// client surface. When false, the client owns its decorations (Chrome, Cursor,
    /// GTK headerbars, …) and is mapped to the full tile footprint.
    pub uses_ssd: bool,
    /// Set once the client binds `xdg-decoration` and we have exchanged a mode.
    pub decoration_negotiated: bool,
    /// Set when the client creates an `xdg-decoration` object (before mode pick).
    pub decoration_bound: bool,
    /// True when entering fullscreen while maximized — restored on fullscreen exit.
    pub pre_fullscreen_maximized: bool,
    /// XWayland windows are managed as floating surfaces (no grid tiling/snap):
    /// they carry no `xdg_toplevel` state and are positioned via `configure`.
    pub is_x11: bool,
}

impl WindowRecord {
    /// The `xdg_toplevel` for native Wayland windows, `None` for XWayland.
    pub fn wl_toplevel(&self) -> Option<&ToplevelSurface> {
        self.surface.wayland()
    }

    /// The backing `X11Surface` for XWayland windows, `None` for native Wayland.
    pub fn x11(&self) -> Option<&X11Surface> {
        self.surface.x11()
    }
}

pub struct WindowRegistry {
    next_id: u32,
    by_id: HashMap<u32, WindowRecord>,
    // Keyed on the surface's globally-unique `ObjectId` — NOT `protocol_id()`, which
    // is only unique per client connection, so surfaces from two different clients
    // (shell, terminal, settings, …) can share a number and clobber each other.
    surface_to_id: HashMap<ObjectId, u32>,
    // XWayland windows are keyed by their stable X11 window id: their `wl_surface`
    // may not be associated yet at registration (it arrives via xwayland-shell),
    // and X11 protocol callbacks (`XwmHandler`) only carry the `X11Surface`.
    x11_to_id: HashMap<u32, u32>,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            by_id: HashMap::new(),
            surface_to_id: HashMap::new(),
            x11_to_id: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn insert_record(
        &mut self,
        id: u32,
        window: Window,
        surface: WindowSurface,
        title: String,
        app_id: Option<String>,
    ) {
        let is_x11 = matches!(surface, WindowSurface::X11(_));
        let rect = PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        self.by_id.insert(
            id,
            WindowRecord {
                id,
                window,
                surface,
                title,
                app_id,
                target_rect: rect,
                restore_rect: None,
                snapped: false,
                fullscreen: false,
                maximized: false,
                minimized: false,
                workspace: 1,
                output: String::new(),
                ready: false,
                placement_chosen: false,
                uses_ssd: true,
                decoration_negotiated: false,
                decoration_bound: false,
                pre_fullscreen_maximized: false,
                is_x11,
            },
        );
    }

    pub fn register(&mut self, window: Window, title: String, app_id: Option<String>) -> u32 {
        let id = self.alloc_id();
        let toplevel = window.toplevel().unwrap().clone();
        self.surface_to_id.insert(toplevel.wl_surface().id(), id);
        self.insert_record(id, window, WindowSurface::Wayland(toplevel), title, app_id);
        id
    }

    /// Register an XWayland toplevel. Keyed by the X11 window id (stable and
    /// available in every `XwmHandler` callback); the `wl_surface` is also indexed
    /// when already associated so the shared commit/activation path can find it.
    pub fn register_x11(
        &mut self,
        window: Window,
        x11: X11Surface,
        title: String,
        app_id: Option<String>,
    ) -> u32 {
        let id = self.alloc_id();
        self.x11_to_id.insert(x11.window_id(), id);
        if let Some(surface) = x11.wl_surface() {
            self.surface_to_id.insert(surface.id(), id);
        }
        self.insert_record(id, window, WindowSurface::X11(Box::new(x11)), title, app_id);
        id
    }

    /// Index an XWayland window's `wl_surface` once it has been associated (it may
    /// arrive after `register_x11`). Safe to call repeatedly.
    pub fn index_x11_surface(&mut self, x11_window: u32, surface_id: ObjectId) {
        if let Some(id) = self.x11_to_id.get(&x11_window).copied() {
            self.surface_to_id.insert(surface_id, id);
        }
    }

    pub fn id_for_x11_window(&self, x11_window: u32) -> Option<u32> {
        self.x11_to_id.get(&x11_window).copied()
    }

    pub fn decoration_bound(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.decoration_bound)
    }

    pub fn set_decoration_bound(&mut self, id: u32, bound: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.decoration_bound = bound;
        }
    }

    pub fn uses_ssd(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.uses_ssd)
    }

    pub fn set_uses_ssd(&mut self, id: u32, uses_ssd: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.uses_ssd = uses_ssd;
        }
    }

    pub fn decoration_negotiated(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.decoration_negotiated)
    }

    pub fn set_decoration_negotiated(&mut self, id: u32, negotiated: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.decoration_negotiated = negotiated;
        }
    }

    pub fn placement_chosen(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.placement_chosen)
    }

    pub fn set_placement_chosen(&mut self, id: u32, chosen: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.placement_chosen = chosen;
        }
    }

    pub fn set_ready(&mut self, id: u32, ready: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.ready = ready;
        }
    }

    pub fn is_ready(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.ready)
    }

    pub fn set_metadata(&mut self, id: u32, title: String, app_id: Option<String>) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.title = title;
            record.app_id = app_id;
        }
    }

    pub fn get(&self, id: u32) -> Option<&WindowRecord> {
        self.by_id.get(&id)
    }

    pub fn ids(&self) -> Vec<u32> {
        self.by_id.keys().copied().collect()
    }

    pub fn id_for_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> Option<u32> {
        self.surface_to_id.get(&surface.id()).copied()
    }

    pub fn id_for_window(&self, window: &Window) -> Option<u32> {
        if let Some(toplevel) = window.toplevel() {
            return self.id_for_surface(toplevel.wl_surface());
        }
        if let Some(x11) = window.x11_surface() {
            if let Some(id) = self.x11_to_id.get(&x11.window_id()).copied() {
                return Some(id);
            }
            if let Some(surface) = x11.wl_surface() {
                return self.id_for_surface(&surface);
            }
        }
        None
    }

    pub fn set_target_rect(&mut self, id: u32, rect: PixelRect) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.target_rect = rect;
        }
    }

    pub fn target_rect(&self, id: u32) -> Option<PixelRect> {
        self.by_id.get(&id).map(|r| r.target_rect)
    }

    pub fn set_rect(&mut self, id: u32, rect: PixelRect) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.target_rect = rect;
        }
    }

    pub fn set_fullscreen(&mut self, id: u32, enabled: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.fullscreen = enabled;
            if enabled {
                record.maximized = false;
            }
        }
    }

    pub fn set_maximized(&mut self, id: u32, enabled: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.maximized = enabled;
            if enabled {
                record.fullscreen = false;
            }
        }
    }

    pub fn set_minimized(&mut self, id: u32, enabled: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.minimized = enabled;
        }
    }

    pub fn set_restore_rect(&mut self, id: u32, rect: PixelRect) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.restore_rect = Some(rect);
        }
    }

    pub fn take_restore_rect(&mut self, id: u32) -> Option<PixelRect> {
        self.by_id.get_mut(&id).and_then(|r| r.restore_rect.take())
    }

    pub fn set_pre_fullscreen_maximized(&mut self, id: u32, maximized: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.pre_fullscreen_maximized = maximized;
        }
    }

    pub fn take_pre_fullscreen_maximized(&mut self, id: u32) -> bool {
        self.by_id
            .get_mut(&id)
            .map(|r| {
                let v = r.pre_fullscreen_maximized;
                r.pre_fullscreen_maximized = false;
                v
            })
            .unwrap_or(false)
    }

    pub fn set_snapped(&mut self, id: u32, snapped: bool) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.snapped = snapped;
        }
    }

    pub fn is_snapped(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.snapped)
    }

    pub fn is_minimized(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.minimized)
    }

    pub fn set_workspace(&mut self, id: u32, workspace: u32) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.workspace = workspace;
        }
    }

    pub fn workspace(&self, id: u32) -> Option<u32> {
        self.by_id.get(&id).map(|r| r.workspace)
    }

    pub fn set_output(&mut self, id: u32, output: String) {
        if let Some(record) = self.by_id.get_mut(&id) {
            record.output = output;
        }
    }

    pub fn output_name(&self, id: u32) -> Option<String> {
        self.by_id.get(&id).map(|r| r.output.clone())
    }

    /// Snapshot of ready windows. `focused` is left `false` here (the registry
    /// has no seat access); the caller patches the focused id. `output` is the
    /// name of the monitor the window lives on (per-output workspaces).
    pub fn list(&self) -> Vec<WindowInfo> {
        self.by_id
            .values()
            .filter(|r| r.ready)
            .map(|r| WindowInfo {
                id: r.id,
                title: r.title.clone(),
                app_id: r.app_id.clone(),
                rect: r.target_rect,
                fullscreen: r.fullscreen,
                minimized: r.minimized,
                focused: false,
                output: r.output.clone(),
                workspace: r.workspace,
            })
            .collect()
    }

    pub fn unregister(&mut self, id: u32) -> Option<WindowRecord> {
        let record = self.by_id.remove(&id)?;
        // Drop every surface-index entry pointing at this id (an XWayland window's
        // `wl_surface` may have been indexed after registration).
        self.surface_to_id.retain(|_, mapped| *mapped != id);
        self.x11_to_id.retain(|_, mapped| *mapped != id);
        Some(record)
    }
}
