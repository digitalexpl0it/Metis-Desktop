//! Session crash-recovery preference (`~/.config/metis/session.json`) and the
//! on-disk snapshot schema (`$XDG_STATE_HOME/metis/session-snapshot.json`).
//!
//! See [`docs/decisions/session-restore.md`](../../../docs/decisions/session-restore.md).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::widget_ext::is_safe_launch_id;
use crate::{config_dir, ensure_config_dirs};

/// Current snapshot schema version.
pub const SESSION_SNAPSHOT_VERSION: u32 = 1;

/// Preference: relaunch windows after an unclean compositor restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// When true (default), an unclean compositor exit supervised by
    /// `metis-session` relaunches apps from the last snapshot.
    #[serde(default = "default_true")]
    pub restore_on_crash: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            restore_on_crash: true,
        }
    }
}

pub fn session_config_path() -> PathBuf {
    config_dir().join("session.json")
}

pub fn load_session_config() -> SessionConfig {
    let path = session_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<SessionConfig>(&text) {
            Ok(cfg) => return sanitize_session_config(cfg),
            Err(err) => tracing::warn!(%err, "session.json parse failed — using defaults"),
        }
    }
    SessionConfig::default()
}

pub fn save_session_config(cfg: &SessionConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let clean = sanitize_session_config(cfg.clone());
    crate::persist::write_json_atomic(&session_config_path(), &clean)
}

pub fn sanitize_session_config(cfg: SessionConfig) -> SessionConfig {
    cfg
}

/// Per-output desk state captured for restore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutputSnapshot {
    pub name: String,
    #[serde(default = "default_workspace")]
    pub active_workspace: u32,
    /// Workspace number → layout kind (`free` / `grid` / `scroll`).
    #[serde(default)]
    pub layout_kind: Vec<(u32, String)>,
}

fn default_workspace() -> u32 {
    1
}

/// One window slot to relaunch and place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWindowSnapshot {
    /// Desktop file id when known (`firefox.desktop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_id: Option<String>,
    /// Wayland / X11 app id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_hint: Option<String>,
    pub output: String,
    #[serde(default = "default_workspace")]
    pub workspace: u32,
    #[serde(default)]
    pub floating: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<SessionTileRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<SessionPixelRect>,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub minimized: bool,
    /// Window belonged to the global scratchpad stash when the snapshot was taken.
    #[serde(default)]
    pub scratchpad: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTileRect {
    pub col: u32,
    pub row: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPixelRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Full session snapshot written under `$XDG_STATE_HOME/metis/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default = "snapshot_version")]
    pub version: u32,
    /// Unix seconds when the snapshot was last written.
    #[serde(default)]
    pub saved_at: u64,
    /// When false, restore must not consume this file (clean logout).
    #[serde(default = "default_true")]
    pub eligible: bool,
    #[serde(default)]
    pub outputs: Vec<SessionOutputSnapshot>,
    #[serde(default)]
    pub windows: Vec<SessionWindowSnapshot>,
}

fn snapshot_version() -> u32 {
    SESSION_SNAPSHOT_VERSION
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            version: SESSION_SNAPSHOT_VERSION,
            saved_at: 0,
            eligible: false,
            outputs: Vec::new(),
            windows: Vec::new(),
        }
    }
}

/// `$XDG_STATE_HOME/metis` (or `~/.local/state/metis`).
pub fn metis_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("metis");
    }
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".local/state/metis"))
        .unwrap_or_else(|| PathBuf::from(".local/state/metis"))
}

pub fn session_snapshot_path() -> PathBuf {
    metis_state_dir().join("session-snapshot.json")
}

pub fn session_clean_exit_marker_path() -> PathBuf {
    metis_state_dir().join("session-clean-exit")
}

pub fn load_session_snapshot() -> Option<SessionSnapshot> {
    let path = session_snapshot_path();
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<SessionSnapshot>(&text) {
        Ok(snap) => Some(sanitize_session_snapshot(snap)),
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "session-snapshot.json parse failed");
            None
        }
    }
}

pub fn save_session_snapshot(snap: &SessionSnapshot) -> std::io::Result<()> {
    let path = session_snapshot_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let clean = sanitize_session_snapshot(snap.clone());
    crate::persist::write_json_atomic(&path, &clean)
}

/// Mark the on-disk snapshot ineligible so a later cold login cannot restore it.
pub fn mark_session_snapshot_ineligible() {
    let Some(mut snap) = load_session_snapshot() else {
        return;
    };
    if !snap.eligible {
        return;
    }
    snap.eligible = false;
    if let Err(err) = save_session_snapshot(&snap) {
        tracing::warn!(%err, "failed to mark session snapshot ineligible");
    }
}

pub fn write_session_clean_exit_marker() {
    let path = session_clean_exit_marker_path();
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(%err, "failed to create state dir for session-clean-exit");
        return;
    }
    if let Err(err) = std::fs::write(&path, b"1\n") {
        tracing::warn!(%err, path = %path.display(), "failed to write session-clean-exit");
    }
}

pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Drop unsafe ids, clamp workspaces, drop chrome-only slots.
pub fn sanitize_session_snapshot(mut snap: SessionSnapshot) -> SessionSnapshot {
    snap.version = SESSION_SNAPSHOT_VERSION;
    snap.outputs.retain(|o| !o.name.trim().is_empty());
    for o in &mut snap.outputs {
        o.name = o.name.trim().to_string();
        o.active_workspace = o.active_workspace.clamp(1, 12);
        o.layout_kind
            .retain(|(ws, kind)| (1..=12).contains(ws) && is_layout_kind_name(kind));
    }
    snap.windows.retain(|w| {
        if w.output.trim().is_empty() {
            return false;
        }
        let app = w.app_id.as_deref().unwrap_or("").trim();
        let desk = w.desktop_id.as_deref().unwrap_or("").trim();
        if app.is_empty() && desk.is_empty() {
            return false;
        }
        if is_metis_chrome_app_id(app) {
            return false;
        }
        if !desk.is_empty() && !is_safe_launch_id(desk) {
            return false;
        }
        true
    });
    for w in &mut snap.windows {
        w.output = w.output.trim().to_string();
        w.workspace = w.workspace.clamp(1, 12);
        if let Some(id) = w.desktop_id.as_mut() {
            *id = id.trim().to_string();
        }
        if let Some(id) = w.app_id.as_mut() {
            *id = id.trim().to_string();
        }
        if let Some(t) = w.title_hint.as_mut() {
            *t = t.trim().to_string();
            if t.is_empty() {
                w.title_hint = None;
            }
        }
        if let Some(r) = w.rect
            && (r.width <= 0 || r.height <= 0)
        {
            w.rect = None;
        }
        if let Some(t) = w.tile
            && (t.w == 0 || t.h == 0)
        {
            w.tile = None;
        }
    }
    if snap.windows.len() > 64 {
        snap.windows.truncate(64);
    }
    snap
}

fn is_layout_kind_name(kind: &str) -> bool {
    matches!(kind, "free" | "grid" | "scroll")
}

/// Shell / widgets chrome that must never be relaunched from a snapshot.
pub fn is_metis_chrome_app_id(app_id: &str) -> bool {
    let lower = app_id.trim().to_lowercase();
    lower.is_empty()
        || lower == "metis-shell"
        || lower == "metis-desktop-widgets"
        || lower.starts_with("metis-shell")
        || lower.contains("desktop-widgets")
}

/// Resolve a Wayland/X11 `app_id` to a launchable `.desktop` id (no GTK).
pub fn resolve_desktop_id_for_app_id(app_id: &str) -> Option<String> {
    let app_id = app_id.trim();
    if app_id.is_empty() || is_metis_chrome_app_id(app_id) {
        return None;
    }
    for candidate in desktop_id_candidates_for_app(app_id) {
        if crate::startup::find_desktop_file_for_id(&candidate).is_some() {
            return Some(normalize_desktop_id(&candidate));
        }
    }
    scan_desktop_files_for_wm_class(app_id)
}

fn desktop_id_candidates_for_app(app_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(app_id.to_string());
    if !app_id.ends_with(".desktop") {
        out.push(format!("{app_id}.desktop"));
    }
    let lower = app_id.to_lowercase();
    match lower.as_str() {
        "gnome-terminal-server" | "gnome-terminal" => {
            out.push("org.gnome.Terminal.desktop".into());
            out.push("org.gnome.Terminal".into());
        }
        _ => {}
    }
    if let Some(base) = lower.strip_suffix("-server")
        && !base.is_empty()
    {
        out.push(format!("{base}.desktop"));
        out.push(base.to_string());
    }
    out
}

fn normalize_desktop_id(id: &str) -> String {
    let id = id.trim();
    if id.ends_with(".desktop") {
        id.to_string()
    } else {
        format!("{id}.desktop")
    }
}

fn scan_desktop_files_for_wm_class(app_id: &str) -> Option<String> {
    let needle = app_id.to_lowercase();
    for dir in crate::startup::applications_dirs_for_lookup() {
        if let Some(found) = scan_dir_for_wm_class(&dir, &needle) {
            return Some(found);
        }
    }
    None
}

fn scan_dir_for_wm_class(dir: &Path, needle: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = scan_dir_for_wm_class(&path, needle) {
                return Some(found);
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        if desktop_matches_wm_class(&path, needle) {
            let name = path.file_name()?.to_string_lossy().to_string();
            return Some(name);
        }
    }
    None
}

fn desktop_matches_wm_class(path: &Path, needle: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut in_desktop = false;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if stem == *needle {
        return true;
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_desktop = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_desktop {
            continue;
        }
        for key in ["StartupWMClass=", "WMClass="] {
            if let Some(rest) = line.strip_prefix(key)
                && rest.trim().eq_ignore_ascii_case(needle)
            {
                return true;
            }
        }
    }
    false
}

/// Unique desktop ids from a snapshot, preserving first-seen order.
pub fn unique_desktop_ids_for_restore(snap: &SessionSnapshot) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for w in &snap.windows {
        let id = w
            .desktop_id
            .clone()
            .or_else(|| w.app_id.as_deref().and_then(resolve_desktop_id_for_app_id));
        let Some(id) = id else {
            continue;
        };
        if !is_safe_launch_id(&id) {
            continue;
        }
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

/// Whether `startup_id` is already covered by a restore launch list.
pub fn startup_id_covered_by_restore(startup_id: &str, restore_ids: &[String]) -> bool {
    let a = normalize_desktop_id(startup_id).to_lowercase();
    restore_ids
        .iter()
        .any(|r| normalize_desktop_id(r).to_lowercase() == a)
}

/// Match a newly mapped window to the next unused snapshot slot.
pub fn match_restore_slot(
    pending: &mut [SessionWindowSnapshot],
    claimed: &mut [bool],
    app_id: Option<&str>,
    desktop_hint: Option<&str>,
) -> Option<SessionWindowSnapshot> {
    let app = app_id.map(str::trim).filter(|s| !s.is_empty());
    let desk = desktop_hint.map(str::trim).filter(|s| !s.is_empty());
    if app.is_none() && desk.is_none() {
        return None;
    }
    for (i, slot) in pending.iter().enumerate() {
        if claimed.get(i).copied().unwrap_or(true) {
            continue;
        }
        if slot_matches(slot, app, desk) {
            claimed[i] = true;
            return Some(slot.clone());
        }
    }
    None
}

fn slot_matches(slot: &SessionWindowSnapshot, app: Option<&str>, desk: Option<&str>) -> bool {
    if let (Some(a), Some(sa)) = (app, slot.app_id.as_deref())
        && a.eq_ignore_ascii_case(sa)
    {
        return true;
    }
    if let (Some(d), Some(sd)) = (desk, slot.desktop_id.as_deref())
        && normalize_desktop_id(d).eq_ignore_ascii_case(&normalize_desktop_id(sd))
    {
        return true;
    }
    if let (Some(a), Some(sd)) = (app, slot.desktop_id.as_deref()) {
        let cand = normalize_desktop_id(a);
        if cand.eq_ignore_ascii_case(&normalize_desktop_id(sd)) {
            return true;
        }
        let stem = sd.strip_suffix(".desktop").unwrap_or(sd);
        if a.eq_ignore_ascii_case(stem) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_session_config_idempotent() {
        let once = sanitize_session_config(SessionConfig::default());
        let twice = sanitize_session_config(once.clone());
        assert_eq!(once, twice);
        assert!(once.restore_on_crash);
    }

    #[test]
    fn snapshot_round_trip() {
        let snap = SessionSnapshot {
            version: SESSION_SNAPSHOT_VERSION,
            saved_at: 123,
            eligible: true,
            outputs: vec![SessionOutputSnapshot {
                name: "DP-1".into(),
                active_workspace: 2,
                layout_kind: vec![(2, "grid".into())],
            }],
            windows: vec![SessionWindowSnapshot {
                desktop_id: Some("firefox.desktop".into()),
                app_id: Some("firefox".into()),
                title_hint: Some("Mozilla Firefox".into()),
                output: "DP-1".into(),
                workspace: 2,
                floating: false,
                tile: Some(SessionTileRect {
                    col: 0,
                    row: 0,
                    w: 6,
                    h: 4,
                }),
                rect: None,
                fullscreen: false,
                maximized: false,
                minimized: false,
                scratchpad: false,
            }],
        };
        let json = serde_json::to_string_pretty(&snap).expect("serialize");
        let back: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            sanitize_session_snapshot(back),
            sanitize_session_snapshot(snap)
        );
    }

    #[test]
    fn sanitize_drops_chrome_and_bad_ids() {
        let snap = SessionSnapshot {
            version: 1,
            saved_at: 0,
            eligible: true,
            outputs: vec![],
            windows: vec![
                SessionWindowSnapshot {
                    desktop_id: None,
                    app_id: Some("metis-shell".into()),
                    title_hint: None,
                    output: "DP-1".into(),
                    workspace: 1,
                    floating: true,
                    tile: None,
                    rect: None,
                    fullscreen: false,
                    maximized: false,
                    minimized: false,
                    scratchpad: false,
                },
                SessionWindowSnapshot {
                    desktop_id: Some("../evil".into()),
                    app_id: Some("x".into()),
                    title_hint: None,
                    output: "DP-1".into(),
                    workspace: 1,
                    floating: true,
                    tile: None,
                    rect: None,
                    fullscreen: false,
                    maximized: false,
                    minimized: false,
                    scratchpad: false,
                },
                SessionWindowSnapshot {
                    desktop_id: Some("kitty.desktop".into()),
                    app_id: Some("kitty".into()),
                    title_hint: None,
                    output: "DP-1".into(),
                    workspace: 99,
                    floating: true,
                    tile: None,
                    rect: None,
                    fullscreen: false,
                    maximized: false,
                    minimized: false,
                    scratchpad: false,
                },
            ],
        };
        let clean = sanitize_session_snapshot(snap);
        assert_eq!(clean.windows.len(), 1);
        assert_eq!(
            clean.windows[0].desktop_id.as_deref(),
            Some("kitty.desktop")
        );
        assert_eq!(clean.windows[0].workspace, 12);
    }

    #[test]
    fn unique_desktop_ids_preserve_order() {
        let snap = SessionSnapshot {
            version: 1,
            saved_at: 0,
            eligible: true,
            outputs: vec![],
            windows: vec![
                SessionWindowSnapshot {
                    desktop_id: Some("a.desktop".into()),
                    app_id: Some("a".into()),
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
                },
                SessionWindowSnapshot {
                    desktop_id: Some("b.desktop".into()),
                    app_id: Some("b".into()),
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
                },
                SessionWindowSnapshot {
                    desktop_id: Some("a.desktop".into()),
                    app_id: Some("a".into()),
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
                },
            ],
        };
        assert_eq!(
            unique_desktop_ids_for_restore(&snap),
            vec!["a.desktop".to_string(), "b.desktop".to_string()]
        );
    }

    #[test]
    fn match_n_windows_same_app_id() {
        let mut pending = vec![
            SessionWindowSnapshot {
                desktop_id: Some("kitty.desktop".into()),
                app_id: Some("kitty".into()),
                title_hint: None,
                output: "DP-1".into(),
                workspace: 1,
                floating: true,
                tile: None,
                rect: Some(SessionPixelRect {
                    x: 10,
                    y: 10,
                    width: 800,
                    height: 600,
                }),
                fullscreen: false,
                maximized: false,
                minimized: false,
                scratchpad: false,
            },
            SessionWindowSnapshot {
                desktop_id: Some("kitty.desktop".into()),
                app_id: Some("kitty".into()),
                title_hint: None,
                output: "DP-1".into(),
                workspace: 2,
                floating: true,
                tile: None,
                rect: Some(SessionPixelRect {
                    x: 20,
                    y: 20,
                    width: 800,
                    height: 600,
                }),
                fullscreen: false,
                maximized: false,
                minimized: false,
                scratchpad: false,
            },
        ];
        let mut claimed = vec![false, false];
        let first =
            match_restore_slot(&mut pending, &mut claimed, Some("kitty"), None).expect("first");
        assert_eq!(first.workspace, 1);
        let second =
            match_restore_slot(&mut pending, &mut claimed, Some("kitty"), None).expect("second");
        assert_eq!(second.workspace, 2);
        assert!(match_restore_slot(&mut pending, &mut claimed, Some("kitty"), None).is_none());
    }

    #[test]
    fn startup_dedupe() {
        let restore = vec![
            "firefox.desktop".into(),
            "org.gnome.Nautilus.desktop".into(),
        ];
        assert!(startup_id_covered_by_restore("firefox", &restore));
        assert!(startup_id_covered_by_restore("firefox.desktop", &restore));
        assert!(!startup_id_covered_by_restore("kitty.desktop", &restore));
    }

    #[test]
    fn chrome_detection() {
        assert!(is_metis_chrome_app_id("metis-shell"));
        assert!(is_metis_chrome_app_id("metis-desktop-widgets"));
        assert!(!is_metis_chrome_app_id("firefox"));
    }
}
