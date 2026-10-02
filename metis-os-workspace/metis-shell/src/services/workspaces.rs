use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Workspace strip for the Metis compositor session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceSnapshot {
    pub workspaces: Vec<Workspace>,
    pub active_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub id: u32,
    pub name: String,
}

/// Active workspace per output (output name → 1-based workspace id). Each output
/// owns an independent set of workspaces, so the bar on every monitor tracks its
/// own active workspace.
fn active_map() -> &'static Mutex<HashMap<String, u32>> {
    static MAP: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ephemeral remote-session workspace id per output (`count + 1` while a
/// dedicated FreeRDP client is active on that output).
fn ephemeral_map() -> &'static Mutex<HashMap<String, u32>> {
    static MAP: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn workspace_count() -> u32 {
    crate::config::load_bar_config()
        .workspace_count
        .clamp(1, 12)
}

/// Ephemeral remote desk id for an output, if the compositor currently advertises
/// one. `None` for unbound bars falls back to any known ephemeral id.
pub fn ephemeral_remote_for(output: Option<&str>) -> Option<u32> {
    let map = ephemeral_map().lock().ok()?;
    match output {
        Some(o) if !o.is_empty() => map.get(o).copied(),
        _ => map.values().next().copied(),
    }
}

/// Record (or clear) the ephemeral remote workspace for an output.
pub fn set_ephemeral_remote(output: &str, ephemeral: Option<u32>) {
    if output.is_empty() {
        return;
    }
    let Ok(mut map) = ephemeral_map().lock() else {
        return;
    };
    match ephemeral {
        Some(id) if id > 0 => {
            map.insert(output.to_string(), id);
        }
        _ => {
            map.remove(output);
        }
    }
}

/// The active workspace on a given output. `None` (a bar not bound to a specific
/// output, i.e. single-monitor sessions) falls back to any known active value.
pub fn active_workspace_for(output: Option<&str>) -> u32 {
    let map = match active_map().lock() {
        Ok(m) => m,
        Err(_) => return 1,
    };
    let count = workspace_count();
    let id = match output {
        Some(o) if !o.is_empty() => map.get(o).copied().unwrap_or(1),
        _ => map.values().next().copied().unwrap_or(1),
    };
    if ephemeral_remote_for(output) == Some(id) {
        return id;
    }
    id.clamp(1, count)
}

/// Build a snapshot for an output. Permanent desks are `1..=count`; when that
/// output has an ephemeral remote session desk, it is appended after them.
pub fn workspace_snapshot_for(output: Option<&str>) -> WorkspaceSnapshot {
    let count = workspace_count();
    let active_id = active_workspace_for(output);
    let mut workspaces: Vec<Workspace> = (1..=count)
        .map(|id| Workspace {
            id,
            name: format!("Desktop {id}"),
        })
        .collect();
    if let Some(eid) = ephemeral_remote_for(output) {
        workspaces.push(Workspace {
            id: eid,
            name: metis_i18n::tr("Remote session"),
        });
    }
    WorkspaceSnapshot {
        workspaces,
        active_id,
    }
}

/// Global snapshot (output-agnostic), used by the background poller and as a
/// fallback. The active id reflects any output's current workspace.
pub fn workspace_snapshot() -> WorkspaceSnapshot {
    workspace_snapshot_for(None)
}

/// Switch a specific output to workspace `id`. `output` is the compositor output
/// name (`None` lets the compositor target the output under the pointer).
pub fn dispatch_workspace(output: Option<String>, id: u32) {
    let count = workspace_count();
    let allowed = (1..=count).contains(&id) || ephemeral_remote_for(output.as_deref()) == Some(id);
    if !allowed {
        return;
    }
    // Optimistic local update for snappy dot feedback; the compositor's
    // `WorkspaceChanged` event is authoritative and reconciles this.
    if let Some(o) = output.as_deref() {
        set_active_workspace(o, id);
    }
    if let Err(err) = crate::compositor::switch_workspace(output.clone(), id) {
        tracing::warn!(?output, id, %err, "failed to switch workspace");
    }
}

/// Reconcile an output's active workspace from a compositor `WorkspaceChanged`
/// event (or an optimistic local update).
pub fn set_active_workspace(output: &str, id: u32) {
    if output.is_empty() {
        return;
    }
    if let Ok(mut map) = active_map().lock() {
        map.insert(output.to_string(), id.max(1));
    }
}
