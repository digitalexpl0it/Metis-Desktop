//! Compositor-side session snapshot + restore (maturity M1).
//!
//! Debounced writes of live window/desk state; on unclean restart (env
//! `METIS_SESSION_RESTORE=1`) queue relaunches and place windows as they map.
//! See `docs/decisions/session-restore.md`.

use std::time::{Duration, Instant};

use metis_config::{
    SessionOutputSnapshot, SessionPixelRect, SessionSnapshot, SessionTileRect,
    SessionWindowSnapshot, is_metis_chrome_app_id, load_session_config, load_session_snapshot,
    mark_session_snapshot_ineligible, match_restore_slot, resolve_desktop_id_for_app_id,
    save_session_snapshot, startup_id_covered_by_restore, unique_desktop_ids_for_restore,
    unix_now_secs, write_session_clean_exit_marker,
};
use metis_grid::{LayoutKind, PixelRect};

use crate::state::MetisState;

const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(750);
const RESTORE_MATCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub struct SessionRestoreRuntime {
    /// Debounced snapshot write deadline.
    dirty_deadline: Option<Instant>,
    /// Pending restore slots from the last unclean restart.
    pending: Vec<SessionWindowSnapshot>,
    claimed: Vec<bool>,
    /// When pending slots expire.
    match_deadline: Option<Instant>,
    /// Desktop ids already queued for restore launch (for startup.json dedupe).
    restore_launch_ids: Vec<String>,
    /// True once boot restore has been armed (or skipped) this process.
    boot_handled: bool,
}

impl SessionRestoreRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty_deadline = Some(Instant::now() + SNAPSHOT_DEBOUNCE);
    }

    pub fn restore_launch_ids(&self) -> &[String] {
        &self.restore_launch_ids
    }
}

impl MetisState {
    /// Call once from housekeeping after shell spawn: maybe load snapshot + queue apps.
    pub(crate) fn arm_session_restore_if_needed(&mut self) {
        if self.session_restore.boot_handled {
            return;
        }
        self.session_restore.boot_handled = true;

        let env_restore = std::env::var_os("METIS_SESSION_RESTORE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !env_restore {
            tracing::debug!("session restore skipped — METIS_SESSION_RESTORE unset");
            return;
        }
        let cfg = load_session_config();
        if !cfg.restore_on_crash {
            tracing::info!("session restore skipped — restore_on_crash disabled");
            return;
        }
        let Some(snap) = load_session_snapshot() else {
            tracing::info!("session restore skipped — no snapshot");
            return;
        };
        if !snap.eligible || snap.windows.is_empty() {
            tracing::info!(
                eligible = snap.eligible,
                windows = snap.windows.len(),
                "session restore skipped — snapshot not eligible"
            );
            return;
        }

        self.apply_output_restore(&snap);
        self.session_restore.restore_launch_ids = unique_desktop_ids_for_restore(&snap);
        self.session_restore.pending = snap.windows;
        self.session_restore.claimed = vec![false; self.session_restore.pending.len()];
        self.session_restore.match_deadline = Some(Instant::now() + RESTORE_MATCH_TIMEOUT);

        let now = Instant::now();
        let mut stagger_i = 0u32;
        for id in self.session_restore.restore_launch_ids.clone() {
            match metis_config::resolve_desktop_launch_argv(&id) {
                Some(argv) => {
                    let delay = Duration::from_millis(u64::from(stagger_i) * 150);
                    self.startup_apps_queue.push((now + delay, argv));
                    stagger_i = stagger_i.saturating_add(1);
                }
                None => {
                    tracing::warn!(%id, "session restore: desktop Exec unresolved — skipped");
                }
            }
        }
        tracing::info!(
            windows = self.session_restore.pending.len(),
            launches = stagger_i,
            "session restore armed after unclean compositor restart"
        );
    }

    fn apply_output_restore(&mut self, snap: &SessionSnapshot) {
        for out in &snap.outputs {
            let key = out.name.clone();
            if self.desk(&key).is_none() && self.output_by_name(&key).is_none() {
                tracing::debug!(%key, "session restore: output missing — layout skip");
                continue;
            }
            for (ws, kind_name) in &out.layout_kind {
                let kind = match kind_name.as_str() {
                    "grid" => LayoutKind::Grid,
                    "scroll" => LayoutKind::Scroll,
                    _ => LayoutKind::Free,
                };
                self.set_layout_kind_on(&key, *ws, kind);
            }
            let target = out.active_workspace.clamp(1, self.workspace_count());
            if self.active_workspace_for(&key) != target {
                self.switch_workspace(&key, target);
            }
        }
    }

    pub(crate) fn tick_session_restore(&mut self) {
        if let Some(deadline) = self.session_restore.dirty_deadline
            && Instant::now() >= deadline
        {
            self.session_restore.dirty_deadline = None;
            self.write_session_snapshot_now();
        }
        if let Some(deadline) = self.session_restore.match_deadline
            && Instant::now() >= deadline
        {
            let leftover = self
                .session_restore
                .claimed
                .iter()
                .enumerate()
                .filter(|(_, c)| !**c)
                .count();
            if leftover > 0 {
                tracing::warn!(
                    leftover,
                    "session restore: dropping unmatched window slots after timeout"
                );
            }
            self.session_restore.pending.clear();
            self.session_restore.claimed.clear();
            self.session_restore.match_deadline = None;
        }
    }

    pub(crate) fn mark_session_snapshot_dirty(&mut self) {
        self.session_restore.mark_dirty();
    }

    fn write_session_snapshot_now(&mut self) {
        let snap = self.build_session_snapshot();
        if let Err(err) = save_session_snapshot(&snap) {
            tracing::warn!(%err, "failed to write session-snapshot.json");
        } else {
            tracing::debug!(
                windows = snap.windows.len(),
                outputs = snap.outputs.len(),
                "wrote session-snapshot.json"
            );
        }
    }

    fn build_session_snapshot(&self) -> SessionSnapshot {
        let mut outputs = Vec::new();
        for (name, desk) in &self.desks {
            let mut layout_kind: Vec<(u32, String)> = desk
                .layout_kind
                .iter()
                .map(|(ws, k)| {
                    let s = match k {
                        LayoutKind::Free => "free",
                        LayoutKind::Grid => "grid",
                        LayoutKind::Scroll => "scroll",
                    };
                    (*ws, s.to_string())
                })
                .collect();
            layout_kind.sort_by_key(|(ws, _)| *ws);
            outputs.push(SessionOutputSnapshot {
                name: name.clone(),
                active_workspace: desk.active_workspace,
                layout_kind,
            });
        }
        outputs.sort_by(|a, b| a.name.cmp(&b.name));

        let mut windows = Vec::new();
        let mut ids = self.windows.ids();
        ids.sort_unstable();
        for id in ids {
            let Some(rec) = self.windows.get(id) else {
                continue;
            };
            if !rec.ready {
                continue;
            }
            let app_id = rec.app_id.clone().filter(|s| !s.trim().is_empty());
            if let Some(ref a) = app_id
                && is_metis_chrome_app_id(a)
            {
                continue;
            }
            let desktop_id = app_id.as_deref().and_then(resolve_desktop_id_for_app_id);
            if app_id.is_none() && desktop_id.is_none() {
                continue;
            }
            let floating = self.floating.contains(&id);
            let tile = if floating {
                None
            } else {
                self.find_app_tile(id).map(|(_, t)| SessionTileRect {
                    col: t.rect.col,
                    row: t.rect.row,
                    w: t.rect.w,
                    h: t.rect.h,
                })
            };
            let rect = {
                let r = rec.target_rect;
                if r.width > 0 && r.height > 0 {
                    Some(SessionPixelRect {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: r.height,
                    })
                } else {
                    None
                }
            };
            let title_hint = {
                let t = rec.title.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            };
            windows.push(SessionWindowSnapshot {
                desktop_id,
                app_id,
                title_hint,
                output: rec.output.clone(),
                workspace: rec.workspace,
                floating,
                tile,
                rect,
                fullscreen: rec.fullscreen,
                maximized: rec.maximized,
                minimized: rec.minimized,
                scratchpad: self.scratchpad.contains(id),
            });
        }

        SessionSnapshot {
            version: metis_config::SESSION_SNAPSHOT_VERSION,
            saved_at: unix_now_secs(),
            eligible: true,
            outputs,
            windows,
        }
    }

    /// Clean logout: stop supervisor restore and mark snapshot ineligible.
    pub(crate) fn prepare_clean_session_exit(&mut self) {
        write_session_clean_exit_marker();
        let mut snap = self.build_session_snapshot();
        snap.eligible = false;
        if let Err(err) = save_session_snapshot(&snap) {
            tracing::warn!(%err, "failed to write ineligible session snapshot on logout");
            mark_session_snapshot_ineligible();
        }
    }

    /// Try to place `id` from a pending restore slot. Returns true when handled.
    pub(crate) fn try_apply_session_restore_placement(
        &mut self,
        id: u32,
        app_id: Option<&str>,
    ) -> bool {
        if self.session_restore.pending.is_empty() {
            return false;
        }
        let desktop_hint = app_id.and_then(resolve_desktop_id_for_app_id);
        let Some(slot) = match_restore_slot(
            &mut self.session_restore.pending,
            &mut self.session_restore.claimed,
            app_id,
            desktop_hint.as_deref(),
        ) else {
            return false;
        };

        if !slot.output.is_empty() && self.output_by_name(&slot.output).is_some() {
            let current = self.desk_key_for_window(id);
            if current != slot.output {
                self.move_window_to_output_inner(id, &slot.output, false);
            }
        }
        self.windows.set_workspace(id, slot.workspace);

        if slot.floating {
            self.floating.insert(id);
            self.remove_app_tile_everywhere(id);
            if let Some(r) = slot.rect {
                let pix = PixelRect {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                };
                if r.width > 32 && r.height > 32 {
                    let body = self.restore_body_for_window(id, pix);
                    self.windows.set_target_rect(id, body);
                }
            }
        } else {
            self.floating.remove(&id);
            self.ensure_app_tile_for_window(id);
            if let Some(t) = slot.tile {
                self.apply_restored_tile_rect(id, t);
            }
        }

        self.windows.set_placement_chosen(id, true);

        // Defer FS/max/min/scratchpad until the window is ready (activate_window).
        if slot.fullscreen || slot.maximized || slot.minimized || slot.scratchpad {
            self.pending_session_restore_chrome.insert(
                id,
                (
                    slot.fullscreen,
                    slot.maximized,
                    slot.minimized,
                    slot.scratchpad,
                ),
            );
        }

        tracing::info!(
            id,
            app_id = app_id.unwrap_or(""),
            output = %slot.output,
            workspace = slot.workspace,
            floating = slot.floating,
            scratchpad = slot.scratchpad,
            "session restore: placed window from snapshot"
        );
        self.mark_session_snapshot_dirty();
        true
    }

    fn apply_restored_tile_rect(&mut self, id: u32, tile: SessionTileRect) {
        let key = self.desk_key_for_window(id);
        let tile_id = format!("app-{id}");
        let desk = self.desk_mut_or_default(&key);
        let apply = |t: &mut metis_grid::GridTile| {
            if t.id == tile_id {
                t.rect.col = tile.col;
                t.rect.row = tile.row;
                t.rect.w = tile.w.max(1);
                t.rect.h = tile.h.max(1);
            }
        };
        for t in &mut desk.layout.tiles {
            apply(t);
        }
        for tiles in desk.stashed_app_tiles.values_mut() {
            for t in tiles {
                apply(t);
            }
        }
    }

    pub(crate) fn apply_pending_session_restore_chrome(&mut self, id: u32) {
        let Some((fullscreen, maximized, minimized, scratchpad)) =
            self.pending_session_restore_chrome.remove(&id)
        else {
            return;
        };
        if fullscreen {
            self.set_fullscreen(id, true, None);
        } else if maximized {
            self.set_maximized(id, true);
        }
        if minimized {
            self.minimize_by_id(id);
        }
        if scratchpad {
            self.send_to_scratchpad(id);
        }
    }
}

/// Whether `startup.json` entry `id` should be skipped because restore already
/// launches it.
pub fn startup_covered_by_session_restore(runtime: &SessionRestoreRuntime, id: &str) -> bool {
    startup_id_covered_by_restore(id, runtime.restore_launch_ids())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_config::SessionWindowSnapshot;

    #[test]
    fn restore_launch_dedupe_helper() {
        let mut rt = SessionRestoreRuntime::new();
        rt.restore_launch_ids = vec!["firefox.desktop".into()];
        assert!(startup_covered_by_session_restore(&rt, "firefox"));
        assert!(!startup_covered_by_session_restore(&rt, "kitty.desktop"));
    }

    #[test]
    fn match_helper_smoke() {
        let mut pending = vec![SessionWindowSnapshot {
            desktop_id: Some("x.desktop".into()),
            app_id: Some("x".into()),
            title_hint: None,
            output: "o".into(),
            workspace: 1,
            floating: true,
            tile: None,
            rect: None,
            fullscreen: false,
            maximized: false,
            minimized: false,
            scratchpad: false,
        }];
        let mut claimed = vec![false];
        assert!(match_restore_slot(&mut pending, &mut claimed, Some("x"), None).is_some());
        assert!(claimed[0]);
    }
}
