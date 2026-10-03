//! Metis-native RDP host via `metis-rdp-host` (portal ScreenCast + FreeRDP).
//!
//! Captures the Wayland session through xdg-desktop-portal / metis-portal and
//! serves RDP on TCP 3389 for Metis Viewer. GRD stays the product default.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use metis_config::{RemoteBackend, load_remote_config, save_remote_config};

use crate::host::{hostname, lan_addresses};

const DEFAULT_PORT: u16 = 3389;
const HOST_NAMES: &[&str] = &["metis-rdp-host"];

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match path.metadata() {
        Ok(meta) if meta.is_file() => meta.permissions().mode() & 0o111 != 0,
        _ => false,
    }
}

/// Absolute path to `metis-rdp-host`, if installed.
pub fn resolve_host_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in HOST_NAMES {
            let path = dir.join(name);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    for dir in ["/usr/local/bin", "/usr/bin"] {
        for name in HOST_NAMES {
            let path = Path::new(dir).join(name);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

pub fn install_hint() -> &'static str {
    "Rebuild/install Metis so metis-rdp-host is next to metis-remote (run-metis.sh --install-session)"
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("metis")
}

fn pid_path() -> PathBuf {
    runtime_dir().join("native-rdp.pid")
}

fn read_pid() -> Option<u32> {
    let text = fs::read_to_string(pid_path()).ok()?;
    text.trim().parse().ok()
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn write_pid(pid: u32) -> Result<(), String> {
    let dir = runtime_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let path = pid_path();
    fs::write(&path, format!("{pid}\n")).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn clear_pid() {
    let _ = fs::remove_file(pid_path());
}

pub fn is_running() -> bool {
    match read_pid() {
        Some(pid) if pid_alive(pid) => true,
        Some(_) => {
            clear_pid();
            false
        }
        None => Command::new("pgrep")
            .args(["-x", "metis-rdp-host"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NativeRdpStatus {
    pub installed: bool,
    pub binary: Option<String>,
    pub running: bool,
    pub backend_selected: bool,
    pub config_enabled: bool,
    pub port: u16,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub firewall_applied: bool,
    pub firewall_backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub install_hint: String,
}

pub fn status() -> NativeRdpStatus {
    let cfg = load_remote_config();
    let bin = resolve_host_binary();
    let installed = bin.is_some();
    let fw = crate::firewall::status();
    let selected = matches!(cfg.backend, RemoteBackend::MetisNative);
    NativeRdpStatus {
        installed,
        binary: bin.as_ref().map(|p| p.display().to_string()),
        running: is_running(),
        backend_selected: selected,
        config_enabled: cfg.enabled && selected,
        port: DEFAULT_PORT,
        hostname: hostname(),
        addresses: lan_addresses(),
        firewall_applied: fw.applied,
        firewall_backend: fw.backend,
        error: if installed {
            None
        } else {
            Some(format!("metis-rdp-host not found — {}", install_hint()))
        },
        install_hint: install_hint().into(),
    }
}

fn start_host() -> Result<(), String> {
    let bin = resolve_host_binary().ok_or_else(|| install_hint().to_string())?;
    if is_running() {
        return Ok(());
    }

    let child = Command::new(&bin)
        .env("METIS_PORTAL_AUTO_APPROVE", "1")
        .env("METIS_RDP_PORT", DEFAULT_PORT.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;

    let pid = child.id();
    write_pid(pid)?;
    std::mem::forget(child);
    tracing::info!(%pid, binary = %bin.display(), "native RDP: started metis-rdp-host");
    Ok(())
}

fn stop_host() -> Result<(), String> {
    if let Some(pid) = read_pid() {
        if pid_alive(pid) {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            std::thread::sleep(std::time::Duration::from_millis(400));
            if pid_alive(pid) {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        clear_pid();
        return Ok(());
    }
    let _ = Command::new("pkill")
        .args(["-x", "metis-rdp-host"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// Select MetisNative backend, start portal RDP host, apply LAN firewall if configured.
pub fn enable() -> Result<(), String> {
    start_host()?;
    let mut cfg = load_remote_config();
    cfg.backend = RemoteBackend::MetisNative;
    cfg.enabled = true;
    save_remote_config(&cfg).map_err(|e| e.to_string())?;

    let lan_only = cfg.lan_only;
    std::thread::Builder::new()
        .name("metis-remote-native-fw".into())
        .spawn(move || {
            if lan_only {
                if let Err(err) = crate::firewall::apply() {
                    tracing::warn!(%err, "native RDP: LAN firewall apply failed");
                }
            } else if let Err(err) = crate::firewall::clear() {
                tracing::warn!(%err, "native RDP: firewall clear failed");
            }
        })
        .ok();
    Ok(())
}

/// Stop host and clear MetisNative backend preference (restore GRD default).
pub fn disable(kill: bool) -> Result<(), String> {
    if kill {
        stop_host()?;
    }
    let mut cfg = load_remote_config();
    if matches!(cfg.backend, RemoteBackend::MetisNative) {
        cfg.backend = RemoteBackend::GnomeRdp;
        cfg.enabled = false;
    }
    save_remote_config(&cfg).map_err(|e| e.to_string())?;
    Ok(())
}

/// Pause listen (session lock) while keeping `remote.json.enabled`.
pub fn pause() -> Result<(), String> {
    let cfg = load_remote_config();
    if !matches!(cfg.backend, RemoteBackend::MetisNative) {
        return Ok(());
    }
    stop_host()
}

/// Resume if config still wants MetisNative sharing.
pub fn resume() -> Result<(), String> {
    let cfg = load_remote_config();
    if !cfg.enabled || !matches!(cfg.backend, RemoteBackend::MetisNative) {
        return Ok(());
    }
    start_host()?;
    if cfg.lan_only {
        let _ = crate::firewall::apply();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hint_mentions_host() {
        assert!(install_hint().contains("metis-rdp-host"));
    }

    #[test]
    fn host_names_non_empty() {
        assert!(!HOST_NAMES.is_empty());
    }
}
