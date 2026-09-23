//! Shell-side cache of the compositor's open windows, driven by the compositor
//! event stream (`WindowOpened`/`WindowClosed`/`WindowMetadata`/`WindowFocused`/
//! `WindowMinimized`) with a periodic `ListWindows` reconcile as a safety net.
//! The taskbar widget reads this cache to render running apps.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use metis_protocol::{CompositorEvent, WindowInfo};

const FOCUS_MRU_CAP: usize = 32;

#[derive(Debug, Clone, Default)]
pub struct WindowsSnapshot {
    pub windows: Vec<WindowInfo>,
    pub focused: Option<u32>,
    /// Most-recently-focused window ids (front = most recent).
    pub focus_mru: Vec<u32>,
}

thread_local! {
    static STORE: RefCell<WindowsSnapshot> = RefCell::new(WindowsSnapshot::default());
    /// Repaint hooks installed by each bar's tasks widget (one per output in a
    /// multi-monitor session). Weak so a torn-down bar's hook drops itself.
    static REFRESH: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    /// Coalesce bursty window events (Firefox/Text Editor title churn) into one
    /// idle repaint so the dock is not torn down repeatedly in a single frame.
    static REFRESH_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

/// Register a callback invoked whenever the window cache changes so every bar's
/// taskbar can repaint. Each bar registers its own hook; dead hooks (from
/// rebuilt/removed bars) are pruned on the next register/fire.
pub fn register_refresh(cb: Rc<dyn Fn()>) {
    REFRESH.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.push(Rc::downgrade(&cb));
    });
}

/// Repaint every bar's task dock (e.g. after `taskbar_pinned` changes live).
pub fn refresh_taskbars() {
    fire_refresh();
}

fn fire_refresh() {
    if REFRESH_SCHEDULED.with(|c| c.get()) {
        return;
    }
    REFRESH_SCHEDULED.with(|c| c.set(true));
    glib::idle_add_local_once(|| {
        REFRESH_SCHEDULED.with(|c| c.set(false));
        // Collect live callbacks first so we don't hold the REFRESH borrow while a
        // callback runs (a callback may re-enter via register_refresh).
        let callbacks: Vec<Rc<dyn Fn()>> = REFRESH.with(|r| {
            let mut list = r.borrow_mut();
            list.retain(|w| w.strong_count() > 0);
            list.iter().filter_map(std::rc::Weak::upgrade).collect()
        });
        for cb in callbacks {
            cb();
        }
    });
}

/// Current snapshot of known windows.
pub fn snapshot() -> WindowsSnapshot {
    STORE.with(|s| s.borrow().clone())
}

fn note_focus_mru(mru: &mut Vec<u32>, id: u32) {
    mru.retain(|&x| x != id);
    mru.insert(0, id);
    if mru.len() > FOCUS_MRU_CAP {
        mru.truncate(FOCUS_MRU_CAP);
    }
}

/// Windows on `output` (if known) and `workspace`, ordered by focus MRU then
/// remaining windows in snapshot order. Minimized windows are included (Alt+Tab
/// should still reach them).
pub fn windows_for_output_workspace(output: Option<&str>, workspace: u32) -> Vec<WindowInfo> {
    let snap = snapshot();
    filter_by_output_workspace(&snap.windows, output, workspace, &snap.focus_mru)
}

fn filter_by_output_workspace(
    windows: &[WindowInfo],
    output: Option<&str>,
    workspace: u32,
    focus_mru: &[u32],
) -> Vec<WindowInfo> {
    let mut filtered: Vec<WindowInfo> = windows
        .iter()
        .filter(|w| {
            let out_ok = match output {
                Some(o) if !o.is_empty() => w.output.is_empty() || w.output == o,
                _ => true,
            };
            let ws_ok = w.workspace == 0 || w.workspace == workspace;
            out_ok && ws_ok
        })
        .cloned()
        .collect();

    let mut ordered = Vec::with_capacity(filtered.len());
    for &id in focus_mru {
        if let Some(pos) = filtered.iter().position(|w| w.id == id) {
            ordered.push(filtered.remove(pos));
        }
    }
    ordered.append(&mut filtered);
    ordered
}

/// Best-effort output name for the switcher/overview: focused window's output,
/// else any known window output, else empty (all outputs).
pub fn focused_output_name() -> Option<String> {
    let snap = snapshot();
    if let Some(fid) = snap.focused
        && let Some(w) = snap.windows.iter().find(|w| w.id == fid)
        && !w.output.is_empty()
    {
        return Some(w.output.clone());
    }
    snap.windows
        .iter()
        .find(|w| !w.output.is_empty())
        .map(|w| w.output.clone())
}

/// Replace the cache from an authoritative `ListWindows` response (initial seed
/// and slow reconcile). Best-effort: a failed IPC leaves the cache untouched.
pub fn reconcile_now() {
    match crate::compositor::list_windows() {
        Ok(windows) => {
            let list_focus = windows.iter().find(|w| w.focused).map(|w| w.id);
            STORE.with(|s| {
                let mut store = s.borrow_mut();
                // Focus is authoritative from the event stream (`WindowFocused`),
                // not from this snapshot. `list_windows` derives focus from live
                // keyboard focus, which is `None` whenever the pointer is in the
                // shell UI (bar, start menu, dock) — so recomputing it here would
                // clear the dock highlight every reconcile. Keep our event-driven
                // focus, only falling back to the list when we have none or the
                // tracked window has gone away.
                let focused = match store.focused {
                    Some(fid) if windows.iter().any(|w| w.id == fid) => Some(fid),
                    _ => list_focus,
                };
                store.windows = windows;
                store.focused = focused;
                for w in &mut store.windows {
                    w.focused = Some(w.id) == focused;
                }
                let live: Vec<u32> = store.windows.iter().map(|w| w.id).collect();
                store.focus_mru.retain(|id| live.contains(id));
                if let Some(fid) = focused {
                    note_focus_mru(&mut store.focus_mru, fid);
                }
            });
            fire_refresh();
        }
        Err(err) => tracing::debug!(%err, "list_windows reconcile failed"),
    }
}

/// Fold a compositor event into the window cache, repainting on any change.
/// Non-window events are ignored.
pub fn apply_event(evt: &CompositorEvent) {
    match evt {
        CompositorEvent::WindowOpened { id, app_id, .. }
        | CompositorEvent::WindowMetadata { id, app_id, .. } => {
            tracing::info!(id = *id, ?app_id, "windows: applying window event");
        }
        CompositorEvent::WindowClosed { id }
        | CompositorEvent::WindowFocused { id }
        | CompositorEvent::WindowMinimized { id, .. } => {
            tracing::info!(id = *id, "windows: applying window event");
        }
        _ => {}
    }
    let changed = STORE.with(|s| {
        let mut store = s.borrow_mut();
        match evt {
            CompositorEvent::WindowOpened {
                id,
                title,
                app_id,
                suggested_rect,
            } => {
                if let Some(w) = store.windows.iter_mut().find(|w| w.id == *id) {
                    w.title = title.clone();
                    w.app_id = app_id.clone();
                } else {
                    store.windows.push(WindowInfo {
                        id: *id,
                        title: title.clone(),
                        app_id: app_id.clone(),
                        rect: *suggested_rect,
                        fullscreen: false,
                        minimized: false,
                        focused: false,
                        output: String::new(),
                        workspace: 0,
                    });
                }
                true
            }
            CompositorEvent::WindowClosed { id } => {
                let before = store.windows.len();
                store.windows.retain(|w| w.id != *id);
                store.focus_mru.retain(|x| x != id);
                if store.focused == Some(*id) {
                    store.focused = None;
                }
                store.windows.len() != before
            }
            CompositorEvent::WindowMetadata { id, title, app_id } => {
                if let Some(w) = store.windows.iter_mut().find(|w| w.id == *id) {
                    let app_changed = w.app_id != *app_id;
                    w.title = title.clone();
                    w.app_id = app_id.clone();
                    // Title-only updates (tab switches, document edits) must not
                    // rebuild the dock — that destroys task buttons while popovers
                    // are open and can corrupt GTK layout.
                    app_changed
                } else {
                    false
                }
            }
            CompositorEvent::WindowFocused { id } => {
                // The compositor re-emits focus on every click into a window,
                // even when it was already focused. Ignore no-op focus changes
                // so the dock doesn't rebuild (and re-enumerate every installed
                // app) on each click — but still refresh MRU so Alt+Tab order
                // stays accurate if the id was already focused.
                note_focus_mru(&mut store.focus_mru, *id);
                if store.focused == Some(*id) {
                    false
                } else {
                    store.focused = Some(*id);
                    for w in &mut store.windows {
                        w.focused = w.id == *id;
                    }
                    true
                }
            }
            CompositorEvent::WindowMinimized { id, minimized } => {
                if let Some(w) = store.windows.iter_mut().find(|w| w.id == *id) {
                    w.minimized = *minimized;
                    true
                } else {
                    false
                }
            }
            CompositorEvent::WindowFullscreen {
                id,
                fullscreen,
                output,
            } => {
                if let Some(w) = store.windows.iter_mut().find(|w| w.id == *id) {
                    w.fullscreen = *fullscreen;
                    if !output.is_empty() {
                        w.output.clone_from(output);
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    });
    match evt {
        CompositorEvent::WindowOpened {
            app_id: Some(app_id),
            ..
        }
        | CompositorEvent::WindowMetadata {
            app_id: Some(app_id),
            ..
        } => {
            crate::services::launch_pending::clear_for_window(app_id);
        }
        _ => {}
    }
    if changed {
        fire_refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_protocol::PixelRect;

    fn win(id: u32, output: &str, workspace: u32) -> WindowInfo {
        WindowInfo {
            id,
            title: format!("w{id}"),
            app_id: Some(format!("app.{id}")),
            rect: PixelRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            fullscreen: false,
            minimized: false,
            focused: false,
            output: output.into(),
            workspace,
        }
    }

    #[test]
    fn mru_orders_filtered_windows() {
        let windows = vec![
            win(1, "metis-0", 1),
            win(2, "metis-0", 1),
            win(3, "metis-0", 1),
            win(4, "metis-0", 2),
            win(5, "metis-1", 1),
        ];
        let mru = vec![3, 1, 9];
        let ordered = filter_by_output_workspace(&windows, Some("metis-0"), 1, &mru);
        assert_eq!(
            ordered.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }

    #[test]
    fn note_focus_mru_moves_to_front() {
        let mut mru = vec![1, 2, 3];
        note_focus_mru(&mut mru, 2);
        assert_eq!(mru, vec![2, 1, 3]);
        note_focus_mru(&mut mru, 4);
        assert_eq!(mru, vec![4, 2, 1, 3]);
    }
}
