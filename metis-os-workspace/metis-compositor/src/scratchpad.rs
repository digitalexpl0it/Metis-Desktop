//! Global i3-style scratchpad stash (not minimize, not workspace tile stash).
//!
//! Windows in the stash are removed from the tiling/scroll grid and unmapped
//! without `TileMode::Minimized` / genie FX. Toggle (Super+U) sends the focused
//! desk window in, or shows/hides the most-recent stash entry.

use metis_config::is_metis_chrome_app_id;
use metis_protocol::{CompositorEvent, PixelRect};

use crate::focus::KeyboardFocusTarget;
use crate::state::MetisState;

#[derive(Debug, Clone)]
pub(crate) struct ScratchpadEntry {
    pub id: u32,
    pub saved_rect: PixelRect,
    pub was_floating: bool,
    pub output: String,
    pub workspace: u32,
    /// Currently remapped as a floating window on the active desk.
    pub shown: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ScratchpadState {
    /// Ordered stash; last entry is most-recently used for toggle-with-no-focus.
    entries: Vec<ScratchpadEntry>,
}

impl ScratchpadState {
    pub fn contains(&self, id: u32) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    pub fn is_hidden(&self, id: u32) -> bool {
        self.entries.iter().any(|e| e.id == id && !e.shown)
    }

    pub fn is_shown(&self, id: u32) -> bool {
        self.entries.iter().any(|e| e.id == id && e.shown)
    }

    pub fn count(&self) -> u32 {
        self.entries.len() as u32
    }

    pub fn remove(&mut self, id: u32) -> Option<ScratchpadEntry> {
        let pos = self.entries.iter().position(|e| e.id == id)?;
        Some(self.entries.remove(pos))
    }

    fn touch_mru(&mut self, id: u32) {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            let entry = self.entries.remove(pos);
            self.entries.push(entry);
        }
    }

    fn mru_id(&self) -> Option<u32> {
        self.entries.last().map(|e| e.id)
    }
}

impl MetisState {
    pub(crate) fn scratchpad_eligible(&self, id: u32) -> bool {
        let Some(rec) = self.windows.get(id) else {
            return false;
        };
        if !rec.ready {
            return false;
        }
        if self.window_is_capture_overlay(id) {
            return false;
        }
        if let Some(app) = rec.app_id.as_deref()
            && is_metis_chrome_app_id(app)
        {
            return false;
        }
        true
    }

    /// Super+U / `ToggleScratchpad`: send focused desk window, or hide/show MRU.
    pub fn toggle_scratchpad(&mut self) {
        if self.session_is_locked() {
            return;
        }
        if let Some(id) = self.focused_window_id() {
            if !self.scratchpad.contains(id) {
                self.send_to_scratchpad(id);
                return;
            }
            if self.scratchpad.is_shown(id) {
                self.hide_scratchpad(id);
            } else {
                self.show_scratchpad(id);
            }
            return;
        }
        let Some(id) = self.scratchpad.mru_id() else {
            return;
        };
        if self.scratchpad.is_shown(id) {
            self.hide_scratchpad(id);
        } else {
            self.show_scratchpad(id);
        }
    }

    /// `SetScratchpad { scratchpad: true }` — stash and hide.
    /// `SetScratchpad { scratchpad: false }` — show and leave the stash (desk float).
    pub fn set_scratchpad(&mut self, id: u32, scratchpad: bool) {
        if scratchpad {
            self.send_to_scratchpad(id);
        } else if self.scratchpad.contains(id) {
            self.show_scratchpad(id);
            // Pop from stash so it stays on the desk like a normal float.
            if let Some(entry) = self.scratchpad.remove(id) {
                let _ = entry;
                self.emit_scratchpad_changed(id, false);
            }
        }
    }

    pub fn send_to_scratchpad(&mut self, id: u32) {
        if !self.scratchpad_eligible(id) {
            return;
        }
        if self.scratchpad.is_hidden(id) {
            return;
        }
        // Already shown from stash → hide again.
        if self.scratchpad.is_shown(id) {
            self.hide_scratchpad(id);
            return;
        }

        let Some(rec) = self.windows.get(id).cloned() else {
            return;
        };
        let saved_rect = rec.target_rect;
        let was_floating = self.floating.contains(&id);
        let output = rec.output.clone();
        let workspace = rec.workspace;

        // Drop minimize chrome if any — scratchpad is a separate path.
        if self.windows.is_minimized(id) {
            self.windows.set_minimized(id, false);
        }
        if self.minimize_genie_fx.contains_key(&id) {
            self.minimize_genie_fx.remove(&id);
        }

        let desk_key = self.desk_key_for_window(id);
        self.remove_app_tile_everywhere(id);
        self.floating.remove(&id);
        self.clear_auto_hide(id);

        if let Some(toplevel) = rec.wl_toplevel() {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.size = None;
                state.fullscreen_output = None;
            });
            toplevel.send_pending_configure();
        }
        self.windows.set_maximized(id, false);
        self.windows.set_fullscreen(id, false);
        self.space.unmap_elem(&rec.window);

        if self.focused_window_id() == Some(id) {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
            }
        }

        self.scratchpad.entries.push(ScratchpadEntry {
            id,
            saved_rect,
            was_floating,
            output,
            workspace,
            shown: false,
        });
        self.scratchpad.touch_mru(id);

        self.auto_reflow_grid_apps(&desk_key, self.focused_window_id(), true);
        self.refresh_all_scroll_offsets();
        self.reposition_scroll_windows();
        self.persist_layout();
        self.mark_session_snapshot_dirty();
        self.emit_scratchpad_changed(id, true);
        self.schedule_redraw();
    }

    pub fn hide_scratchpad(&mut self, id: u32) {
        let Some(pos) = self.scratchpad.entries.iter().position(|e| e.id == id) else {
            return;
        };
        if !self.scratchpad.entries[pos].shown {
            return;
        }
        let Some(rec) = self.windows.get(id).cloned() else {
            return;
        };
        // Refresh saved place from current geometry.
        self.scratchpad.entries[pos].saved_rect = rec.target_rect;
        self.scratchpad.entries[pos].output = rec.output.clone();
        self.scratchpad.entries[pos].workspace = rec.workspace;
        self.scratchpad.entries[pos].was_floating = self.floating.contains(&id);
        self.scratchpad.entries[pos].shown = false;
        self.scratchpad.touch_mru(id);

        let desk_key = self.desk_key_for_window(id);
        self.remove_app_tile_everywhere(id);
        self.floating.remove(&id);
        self.clear_auto_hide(id);
        self.space.unmap_elem(&rec.window);

        if self.focused_window_id() == Some(id) {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
            }
        }

        self.auto_reflow_grid_apps(&desk_key, self.focused_window_id(), true);
        self.refresh_all_scroll_offsets();
        self.reposition_scroll_windows();
        self.mark_session_snapshot_dirty();
        self.emit_scratchpad_changed(id, true);
        self.schedule_redraw();
    }

    pub fn show_scratchpad(&mut self, id: u32) {
        let Some(pos) = self.scratchpad.entries.iter().position(|e| e.id == id) else {
            return;
        };
        if self.scratchpad.entries[pos].shown {
            // Already visible — raise + focus.
            self.focus_scratchpad_window(id);
            return;
        }

        let saved = self.scratchpad.entries[pos].saved_rect;
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        let ws = self.active_workspace_for(&key);

        self.windows.set_output(id, key.clone());
        self.windows.set_workspace(id, ws);
        self.floating.insert(id);
        self.remove_app_tile_everywhere(id);

        let rect = if saved.width > 32 && saved.height > 32 {
            saved
        } else {
            default_scratchpad_float_rect(self, &key)
        };
        let body = self.restore_body_for_window(id, rect);
        self.windows.set_target_rect(id, body);
        self.windows.set_placement_chosen(id, true);

        self.scratchpad.entries[pos].shown = true;
        self.scratchpad.entries[pos].output = key;
        self.scratchpad.entries[pos].workspace = ws;
        self.scratchpad.touch_mru(id);

        self.apply_window_rect(id);
        self.focus_scratchpad_window(id);
        self.mark_session_snapshot_dirty();
        self.emit_scratchpad_changed(id, true);
        self.schedule_redraw();
    }

    fn focus_scratchpad_window(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.note_window_focus(id);
        self.raise_stacking_window(id, true);
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
        }
        self.event_bus.emit(&CompositorEvent::WindowFocused { id });
    }

    pub(crate) fn on_scratchpad_window_destroyed(&mut self, id: u32) {
        if self.scratchpad.remove(id).is_some() {
            self.emit_scratchpad_changed(id, false);
        }
    }

    /// Auto-send on first map when `scratchpad.json` prefers this app id.
    pub(crate) fn maybe_auto_scratchpad(&mut self, id: u32) {
        if self.scratchpad.contains(id) {
            return;
        }
        let Some(app_id) = self.windows.get(id).and_then(|r| r.app_id.clone()) else {
            return;
        };
        if is_metis_chrome_app_id(&app_id) {
            return;
        }
        let cfg = metis_config::load_scratchpad_config();
        if cfg.prefers(&app_id) {
            self.send_to_scratchpad(id);
        }
    }

    fn emit_scratchpad_changed(&mut self, id: u32, in_scratchpad: bool) {
        self.event_bus.emit(&CompositorEvent::WindowScratchpad {
            id,
            scratchpad: in_scratchpad,
        });
        self.event_bus.emit(&CompositorEvent::ScratchpadChanged {
            count: self.scratchpad.count(),
        });
    }

    /// Patch WindowInfo list: drop hidden stash members; mark shown members.
    pub(crate) fn annotate_window_infos(&self, windows: &mut Vec<metis_protocol::WindowInfo>) {
        windows.retain(|w| !self.scratchpad.is_hidden(w.id));
        for w in windows.iter_mut() {
            w.scratchpad = self.scratchpad.contains(w.id);
        }
    }
}

fn default_scratchpad_float_rect(state: &MetisState, output_key: &str) -> PixelRect {
    let (x, y, w, h) = state
        .output_by_name(output_key)
        .map(|o| {
            let geo = state.space.output_geometry(&o).unwrap_or_else(|| {
                smithay::utils::Rectangle::new(
                    smithay::utils::Point::from((0, 0)),
                    smithay::utils::Size::from((1920, 1080)),
                )
            });
            (geo.loc.x, geo.loc.y, geo.size.w, geo.size.h)
        })
        .unwrap_or((0, 0, 1920, 1080));
    let width = (w / 2).max(640);
    let height = (h / 2).max(480);
    PixelRect {
        x: x + (w - width) / 2,
        y: y + (h - height) / 2,
        width,
        height,
    }
}
