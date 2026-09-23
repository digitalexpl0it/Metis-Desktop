//! Optional RustDesk backend for `metis-remote` (Wave 4a).
//!
//! GRD/RDP remains the default Metis host. This module only detects a system or
//! Flatpak RustDesk install and starts/stops it argv-only — no shell, no
//! credential plumbing (RustDesk manages its own ID/password).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use metis_config::{RemoteBackend, load_remote_config, save_remote_config};

const FLATPAK_ID: &str = "com.rustdesk.RustDesk";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    Missing,
    Path(PathBuf),
    Flatpak,
}

impl Install {
    pub fn is_installed(&self) -> bool {
        !matches!(self, Self::Missing)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Missing => "not_installed",
            Self::Path(_) => "system",
            Self::Flatpak => "flatpak",
        }
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match path.metadata() {
        Ok(meta) if meta.is_file() => meta.permissions().mode() & 0o111 != 0,
        _ => false,
    }
}

/// Detect system binary or Flatpak app (argv-only probes).
pub fn detect() -> Install {
    for dir in ["/usr/bin", "/usr/local/bin"] {
        let path = Path::new(dir).join("rustdesk");
        if is_executable(&path) {
            return Install::Path(path);
        }
    }
    if flatpak_has(FLATPAK_ID) {
        return Install::Flatpak;
    }
    Install::Missing
}

fn flatpak_has(app_id: &str) -> bool {
    Command::new("flatpak")
        .args(["info", app_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn pgrep_rustdesk() -> bool {
    // argv-only: fixed pattern, no shell.
    Command::new("pgrep")
        .args(["-x", "rustdesk"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RustDeskStatus {
    pub installed: bool,
    pub install: String,
    pub running: bool,
    pub backend_selected: bool,
    pub config_enabled: bool,
    pub firewall_applied: bool,
    pub firewall_backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn status() -> RustDeskStatus {
    let cfg = load_remote_config();
    let install = detect();
    let fw = crate::firewall::status_rustdesk();
    RustDeskStatus {
        installed: install.is_installed(),
        install: install.label().into(),
        running: pgrep_rustdesk(),
        backend_selected: matches!(cfg.backend, RemoteBackend::RustDesk),
        config_enabled: cfg.enabled && matches!(cfg.backend, RemoteBackend::RustDesk),
        firewall_applied: fw.applied,
        firewall_backend: fw.backend,
        error: None,
    }
}

/// Mark RustDesk as the active backend and start the UI/daemon if installed.
pub fn enable() -> Result<(), String> {
    let install = detect();
    if !install.is_installed() {
        return Err(
            "RustDesk is not installed (system package or Flatpak com.rustdesk.RustDesk)".into(),
        );
    }
    start(&install)?;
    let mut cfg = load_remote_config();
    cfg.backend = RemoteBackend::RustDesk;
    cfg.enabled = true;
    save_remote_config(&cfg).map_err(|e| e.to_string())?;
    if cfg.lan_only {
        std::thread::spawn(|| {
            if let Err(err) = crate::firewall::apply_rustdesk() {
                tracing::warn!(%err, "RustDesk LAN firewall apply failed");
            }
        });
    }
    Ok(())
}

/// Stop preferring RustDesk; leave the process running unless `kill` is true.
pub fn disable(kill: bool) -> Result<(), String> {
    let mut cfg = load_remote_config();
    cfg.enabled = false;
    if matches!(cfg.backend, RemoteBackend::RustDesk) {
        cfg.backend = RemoteBackend::GnomeRdp;
    }
    save_remote_config(&cfg).map_err(|e| e.to_string())?;
    if kill {
        let _ = Command::new("pkill")
            .args(["-x", "rustdesk"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    std::thread::spawn(|| {
        if let Err(err) = crate::firewall::clear_rustdesk() {
            tracing::warn!(%err, "RustDesk firewall clear failed");
        }
    });
    Ok(())
}

fn start(install: &Install) -> Result<(), String> {
    if pgrep_rustdesk() {
        return Ok(());
    }
    match install {
        Install::Missing => Err("RustDesk is not installed".into()),
        Install::Path(path) => {
            Command::new(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("failed to start {}: {e}", path.display()))?;
            Ok(())
        }
        Install::Flatpak => {
            Command::new("flatpak")
                .args(["run", FLATPAK_ID])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("failed to start Flatpak RustDesk: {e}"))?;
            Ok(())
        }
    }
}
