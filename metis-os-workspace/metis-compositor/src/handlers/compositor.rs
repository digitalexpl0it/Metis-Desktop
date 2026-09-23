use crate::grabs::resize_grab;
use crate::handlers::{handle_layer_commit, xdg_shell};
use crate::state::{ClientState, MetisState};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::{
        calloop::Interest,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer, wl_surface::WlSurface},
        },
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface,
            with_states,
        },
        dmabuf::get_dmabuf,
        drm_syncobj::DrmSyncobjCachedState,
        seat::WaylandFocus,
        shell::wlr_layer::{Anchor, LayerSurfaceCachedState, LayerSurfaceData},
        shm::{ShmHandler, ShmState},
    },
};

impl CompositorHandler for MetisState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(state) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &state.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        // Work around a smithay + gtk4-layer-shell teardown crash. When a
        // gtk4-layer-shell window is destroyed (e.g. removing a per-output edge
        // bar after a "Show bar on" change) the toolkit issues
        // `zwlr_layer_surface_v1.destroy()` — which makes smithay reset the
        // layer surface's cached state to defaults (size 0×0, no anchors) — and
        // then a trailing `wl_surface.attach(null); commit`. Smithay's
        // layer-shell pre-commit hook validates that reset state, sees width 0
        // without left/right anchors, and posts an `invalid_size` protocol
        // error, disconnecting the shell.
        //
        // We register our own pre-commit hook here, on the bare surface before
        // any role (and thus before smithay's layer hook) exists, so it runs
        // first. It only touches surfaces that already carry the layer role and
        // only when they are in that degenerate teardown state, repairing the
        // pending anchors so the unmap commit validates cleanly. Well-behaved
        // layer surfaces never commit a zero dimension without the matching
        // anchors, so this is a no-op during normal operation.
        add_pre_commit_hook::<Self, _>(surface, |_state, _dh, surface| {
            with_states(surface, |states| {
                if states.data_map.get::<LayerSurfaceData>().is_none() {
                    return;
                }
                let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
                let pending = guard.pending();
                if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                    pending.anchor |= Anchor::LEFT | Anchor::RIGHT;
                }
                if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                    pending.anchor |= Anchor::TOP | Anchor::BOTTOM;
                }
            });
        });

        // Buffer-readiness gate for dmabuf clients (games / Proton). When a
        // client commits a dmabuf we must not sample it until the GPU work that
        // produced it has finished, or we scan out a half-drawn frame (tearing /
        // glitching). Prefer the client's explicit `linux-drm-syncobj-v1` acquire
        // fence when present; otherwise fall back to the dmabuf's implicit sync.
        // The commit is held via a calloop blocker until the fence signals.
        add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            let mut acquire_point = None;
            let maybe_dmabuf = with_states(surface, |surface_data| {
                acquire_point.clone_from(
                    &surface_data
                        .cached_state
                        .get::<DrmSyncobjCachedState>()
                        .pending()
                        .acquire_point,
                );
                surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            let Some(dmabuf) = maybe_dmabuf else {
                return;
            };
            // Explicit sync: wait on the client-supplied acquire timeline point.
            if let Some(acquire_point) = acquire_point
                && let Ok((blocker, source)) = acquire_point.generate_blocker()
                && let Some(client) = surface.client()
            {
                let res = state.loop_handle.insert_source(source, move |_, _, data| {
                    let dh = data.display_handle.clone();
                    data.client_compositor_state(&client)
                        .blocker_cleared(data, &dh);
                    Ok(())
                });
                if res.is_ok() {
                    add_blocker(surface, blocker);
                    return;
                }
            }
            // Implicit sync fallback: block on the dmabuf becoming readable.
            if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ)
                && let Some(client) = surface.client()
            {
                let res = state.loop_handle.insert_source(source, move |_, _, data| {
                    let dh = data.display_handle.clone();
                    data.client_compositor_state(&client)
                        .blocker_cleared(data, &dh);
                    Ok(())
                });
                if res.is_ok() {
                    add_blocker(surface, blocker);
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        if let Some(udev) = self.udev.as_mut()
            && let Some(gpus) = udev.gpus.as_mut()
            && let Err(err) = gpus.early_import(udev.render_node, surface)
        {
            tracing::debug!(?err, "early client-buffer import failed");
        }
        on_commit_buffer_handler::<MetisState>(surface);
        let mut committed_id = None;
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|w| w.wl_surface().is_some_and(|s| *s == root))
            {
                window.on_commit();
                committed_id = self.windows.id_for_window(window);
            }
        }
        // After the client settles its committed size, re-anchor auto-hide
        // (maximized / edge-snapped) windows so the screen-edge gap is kept even
        // when the app refuses to shrink to the snapped footprint.
        if let Some(id) = committed_id {
            self.reclamp_auto_hide(id);
            self.reclamp_maximized_geometry(id);
            self.sync_toplevel_fullscreen_from_client(id);
            let _ = self.maybe_register_capture_overlay(id);
        }

        xdg_shell::handle_commit(&mut self.popups, &self.space, surface);
        resize_grab::handle_commit(&mut self.space, surface);
        handle_layer_commit(self, surface);

        // Flag damage on every commit. Arm only the output the window sits on so
        // a game on one monitor does not composite every display each frame.
        if let Some(id) = committed_id {
            self.schedule_redraw_for_window(id);
        } else {
            self.schedule_redraw();
        }

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            // Send the initial configure on the client's first commit so it can
            // attach a buffer immediately, instead of waiting (potentially
            // forever) for an unrelated layout pass to place the window.
            if let Some(id) = self.windows.id_for_surface(&root) {
                self.ensure_initial_configure(id);
            }
            // If an XWayland toplevel just associated its surface, index it and
            // re-deliver keyboard focus (fixes games that map + get focus before
            // their wl_surface exists — pointer works, Esc/keys don't). Resolved
            // via the space-element match, since `id_for_surface` is precisely
            // what is still missing on this first association commit.
            if let Some(id) = committed_id {
                self.note_surface_committed_for_focus(id, &root);
            }
            self.try_activate_committed_window(&root);
        }
    }
}

impl BufferHandler for MetisState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for MetisState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
