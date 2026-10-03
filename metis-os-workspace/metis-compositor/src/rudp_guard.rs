//! Crash-loop guard for Metis Remote (RUDP).
//!
//! While the host runs, a small marker file records what it is doing:
//! `starting` (first [`STABLE_AFTER`] after start), `streaming` (encoder
//! active) or `stable`. A clean host shutdown deletes it.
//!
//! On the next DRM session start, a leftover marker in `starting` or
//! `streaming` means the previous session died while Remote was the most
//! likely cause (compositor crash, GPU hang, power-cycle after a freeze). The
//! host is then switched off in `rudp.json` with an explanation Settings shows,
//! so a bad driver can never turn into a login crash loop. `stable` leftovers
//! (e.g. SIGTERM at shutdown hours into a session) are ignored.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Host uptime after which an unclean exit is no longer blamed on startup.
pub const STABLE_AFTER: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPhase {
    Starting,
    Stable,
    Streaming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    pid: u32,
    #[serde(default)]
    boot_id: String,
    phase: HostPhase,
    #[serde(default)]
    started_unix: u64,
}

fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("metis");
    }
    directories::BaseDirs::new()
        .map(|b| b.home_dir().join(".local/state/metis"))
        .unwrap_or_else(|| std::env::temp_dir().join("metis-state"))
}

fn marker_path() -> PathBuf {
    // Unit tests that spawn a host must never touch the real session marker.
    if cfg!(test) {
        return std::env::temp_dir()
            .join(format!("metis-rudp-guard-test-{}", std::process::id()))
            .join("rudp-host.json");
    }
    state_dir().join("rudp-host.json")
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_marker(path: &Path, marker: &Marker) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec(marker).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn read_marker(path: &Path) -> Option<Marker> {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Is `pid` a live process from this boot that looks like a Metis compositor?
fn pid_is_live_compositor(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|comm| comm.trim().starts_with("metis-composit"))
        .unwrap_or(false)
}

/// Live marker for a running host. Dropping it (clean shutdown) deletes the file.
pub struct HostMarker {
    path: PathBuf,
    state: Mutex<Marker>,
}

impl HostMarker {
    /// Record that the host is starting. Failure to write is logged, not fatal.
    pub fn create() -> Self {
        Self::create_at(marker_path())
    }

    fn create_at(path: PathBuf) -> Self {
        let marker = Marker {
            pid: std::process::id(),
            boot_id: boot_id(),
            phase: HostPhase::Starting,
            started_unix: unix_now(),
        };
        if let Err(err) = write_marker(&path, &marker) {
            tracing::warn!(%err, path = %path.display(), "rudp guard: could not write host marker");
        }
        Self {
            path,
            state: Mutex::new(marker),
        }
    }

    /// Persist a phase change (no-op when unchanged — at most a few writes per session).
    pub fn set_phase(&self, phase: HostPhase) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.phase == phase {
            return;
        }
        state.phase = phase;
        if let Err(err) = write_marker(&self.path, &state) {
            tracing::debug!(%err, "rudp guard: marker update failed");
        }
    }
}

impl Drop for HostMarker {
    fn drop(&mut self) {
        // Only remove our own marker (never another live instance's).
        let ours = read_marker(&self.path).is_none_or(|m| m.pid == std::process::id());
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Inspect a leftover marker from a previous session.
///
/// Returns a user-facing reason when the previous session ended uncleanly
/// while the host was starting or streaming. Consumes the marker either way
/// (except when it belongs to another live compositor).
fn check_marker_at(path: &Path) -> Option<String> {
    let marker = match read_marker(path) {
        Some(m) => m,
        None => {
            let _ = std::fs::remove_file(path);
            return None;
        }
    };
    if marker.pid == std::process::id() {
        return None;
    }
    if marker.boot_id == boot_id() && pid_is_live_compositor(marker.pid) {
        tracing::debug!(
            pid = marker.pid,
            "rudp guard: marker belongs to a live compositor"
        );
        return None;
    }
    let _ = std::fs::remove_file(path);
    match marker.phase {
        HostPhase::Stable => None,
        HostPhase::Starting => Some(
            "Metis Remote was turned off automatically: the previous desktop session ended \
             unexpectedly while Remote was starting. Turn it back on to try again."
                .into(),
        ),
        HostPhase::Streaming => Some(
            "Metis Remote was turned off automatically: the previous desktop session ended \
             unexpectedly while a remote client was connected. Turn it back on to try again."
                .into(),
        ),
    }
}

/// Session-start check. When the previous session crashed with Remote
/// starting / streaming, disables the host in `rudp.json` (with a reason for
/// Settings) and returns `true` — the caller must not start the host.
pub fn auto_disable_after_crash() -> bool {
    let Some(reason) = check_marker_at(&marker_path()) else {
        return false;
    };
    tracing::error!(%reason, "rudp guard: disabling Metis Remote after unclean session end");
    let mut cfg = metis_config::load_rudp_config();
    if cfg.enabled {
        cfg.enabled = false;
        cfg.auto_disabled_reason = Some(reason);
        if let Err(err) = metis_config::save_rudp_config(&cfg) {
            tracing::warn!(%err, "rudp guard: could not persist auto-disable");
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metis-rudp-guard-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("rudp-host.json")
    }

    fn stale(path: &Path, phase: HostPhase) {
        write_marker(
            path,
            &Marker {
                // pid 0 is never a live user process.
                pid: 0,
                boot_id: boot_id(),
                phase,
                started_unix: 1,
            },
        )
        .expect("write");
    }

    #[test]
    fn clean_drop_removes_marker() {
        let path = temp_path("drop");
        {
            let m = HostMarker::create_at(path.clone());
            assert!(path.exists());
            m.set_phase(HostPhase::Streaming);
            assert_eq!(
                read_marker(&path).map(|m| m.phase),
                Some(HostPhase::Streaming)
            );
        }
        assert!(!path.exists());
        assert!(check_marker_at(&path).is_none());
    }

    #[test]
    fn crash_while_starting_or_streaming_is_reported() {
        let path = temp_path("crash");
        stale(&path, HostPhase::Starting);
        assert!(check_marker_at(&path).is_some());
        assert!(!path.exists(), "marker consumed");
        stale(&path, HostPhase::Streaming);
        assert!(check_marker_at(&path).is_some());
    }

    #[test]
    fn stable_or_garbage_marker_is_ignored() {
        let path = temp_path("stable");
        stale(&path, HostPhase::Stable);
        assert!(check_marker_at(&path).is_none());
        assert!(!path.exists());
        std::fs::write(&path, b"not json").expect("write");
        assert!(check_marker_at(&path).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn own_live_marker_is_not_a_crash() {
        let path = temp_path("own");
        let _m = HostMarker::create_at(path.clone());
        assert!(check_marker_at(&path).is_none());
        assert!(path.exists());
    }
}
