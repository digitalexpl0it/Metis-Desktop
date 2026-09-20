//! Desktop sharing orchestration for Metis (gnome-remote-desktop session-sharing RDP,
//! optional RustDesk backend).

mod accounts;
mod datetime;
mod firewall;
mod gnome_rdp;
mod host;
mod pkhelpers;
mod rustdesk;

pub use accounts::{
    accounts_list_as_root, add_user, add_user_as_root, list_accounts, remove_user,
    remove_user_as_root, set_admin, set_admin_as_root, set_display_name, set_display_name_as_root,
    set_password as set_account_password, set_password_as_root as set_account_password_as_root,
    AccountInfo,
};
pub use datetime::{
    set_ntp, set_ntp_as_root, set_time, set_time_as_root, set_timezone, set_timezone_as_root,
    status as datetime_status, status_as_root as datetime_status_as_root, DateTimeStatus,
};
pub use firewall::FirewallStatus;
pub use gnome_rdp::{
    disable_sharing, enable_sharing, pause_sharing, resume_sharing, set_credentials,
    status_snapshot,
};
pub use host::{hostname, lan_addresses};
pub use pkhelpers::{
    add_input_group, apt_install, ensure_polkit_agent, privileged_exe, ubuntu_drivers_install,
    validate_username, APT_ALLOWLIST,
};
pub use rustdesk::RustDeskStatus;

use metis_config::{load_remote_config, save_remote_config};
use zeroize::Zeroize;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RemoteStatus {
    pub available: bool,
    pub running: bool,
    pub rdp_enabled: bool,
    pub port: u16,
    pub password_set: bool,
    pub username: Option<String>,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub backend: String,
    pub config_enabled: bool,
    /// From `remote.json` — Metis should restrict TCP 3389 to private ranges.
    #[serde(default = "default_true")]
    pub lan_only: bool,
    /// Whether LAN-only firewall rules appear applied on this host.
    #[serde(default)]
    pub firewall_applied: bool,
    /// `nft`, `ufw`, or empty.
    #[serde(default)]
    pub firewall_backend: String,
    /// Last firewall apply/clear detail (shown under Security, not as a share error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firewall_detail: Option<String>,
    pub error: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Read live status from gnome-remote-desktop and merge with `remote.json`.
pub fn status() -> RemoteStatus {
    let cfg = load_remote_config();
    let fw = firewall::status();
    let mut snap = status_snapshot();
    snap.config_enabled = cfg.enabled;
    snap.lan_only = cfg.lan_only;
    snap.firewall_applied = fw.applied;
    snap.firewall_backend = fw.backend;
    if snap.hostname.is_empty() {
        snap.hostname = hostname();
    }
    if snap.addresses.is_empty() {
        snap.addresses = lan_addresses();
    }
    if snap.config_enabled && snap.lan_only && !snap.firewall_applied {
        snap.firewall_detail = cfg.firewall_last_error.clone().or_else(|| {
            Some(
                "LAN-only firewall is not applied yet — RDP may be reachable beyond your LAN. \
                 Turning LAN only on (while sharing) applies rules automatically; \
                 a PolicyKit password dialog may appear. Or use Retry under Security."
                    .into(),
            )
        });
    } else {
        snap.firewall_detail = cfg.firewall_last_error.clone();
    }
    snap
}

/// Enable sharing per `remote.json` (starts headless daemon + RDP).
///
/// Returns once RDP is up and config is saved. LAN firewall apply (pkexec) runs
/// in the background so Settings never sticks on "Starting…" waiting for admin.
pub fn enable() -> Result<(), String> {
    let mut cfg = load_remote_config();
    if !gnome_rdp::grdctl_available() {
        return Err(
            "gnome-remote-desktop is not installed (install the gnome-remote-desktop package)"
                .into(),
        );
    }
    let snap = status_snapshot();
    if !snap.password_set {
        return Err("Set RDP credentials before enabling remote desktop".into());
    }
    enable_sharing()?;
    cfg.enabled = true;
    save_remote_config(&cfg).map_err(|e| e.to_string())?;

    let lan_only = cfg.lan_only;
    std::thread::Builder::new()
        .name("metis-remote-fw-enable".into())
        .spawn(move || {
            if lan_only {
                if let Err(err) = firewall::apply() {
                    tracing::warn!(%err, "LAN-only firewall apply failed — RDP may be reachable beyond the LAN");
                }
            } else if let Err(err) = firewall::clear() {
                tracing::warn!(%err, "firewall clear on enable(lan_only=false) failed");
            }
        })
        .ok();
    Ok(())
}

/// Disable RDP and clear `enabled` in config.
///
/// Returns as soon as RDP listen is off and config is saved so Settings can
/// update immediately. Stopping the daemon and clearing firewall rules runs
/// in the background (firewall clear may need pkexec).
pub fn disable() -> Result<(), String> {
    let mut cfg = load_remote_config();
    // Instant: stop accepting connections before anything else.
    if gnome_rdp::grdctl_available() {
        let _ = pause_sharing();
    }
    cfg.enabled = false;
    save_remote_config(&cfg).map_err(|e| e.to_string())?;

    std::thread::Builder::new()
        .name("metis-remote-disable".into())
        .spawn(|| {
            if gnome_rdp::grdctl_available() {
                if let Err(err) = disable_sharing() {
                    tracing::warn!(%err, "background disable_sharing failed");
                }
            }
            match firewall::status() {
                fw if fw.applied => {
                    if let Err(err) = firewall::clear() {
                        tracing::warn!(%err, "firewall clear after disable failed");
                    }
                }
                _ => {
                    // Ensure persisted flag is cleared even if live probe was stale.
                    let mut cfg = load_remote_config();
                    if cfg.firewall_applied {
                        cfg.firewall_applied = false;
                        cfg.firewall_backend.clear();
                        let _ = save_remote_config(&cfg);
                    }
                }
            }
        })
        .map_err(|e| format!("spawn disable cleanup: {e}"))?;
    Ok(())
}

/// Pause RDP listen while keeping `remote.json.enabled` (used on session lock).
pub fn pause() -> Result<(), String> {
    if !gnome_rdp::grdctl_available() {
        return Ok(());
    }
    pause_sharing()
}

/// Resume RDP if config still wants sharing (used on session unlock).
pub fn resume() -> Result<(), String> {
    let cfg = load_remote_config();
    if !cfg.enabled || !gnome_rdp::grdctl_available() {
        return Ok(());
    }
    let snap = status_snapshot();
    if !snap.password_set {
        return Ok(());
    }
    resume_sharing()?;
    if cfg.lan_only {
        let _ = firewall::apply();
    }
    Ok(())
}

/// Persist and optionally re-apply LAN-only firewall when sharing is active.
///
/// Config is saved immediately; pkexec firewall work runs in the background so
/// the Settings toggle never hangs.
pub fn set_lan_only(lan_only: bool) -> Result<(), String> {
    if lan_only {
        // Fail fast (no pkexec) when nothing can enforce rules.
        firewall::enforceable_backend()?;
    }
    let mut cfg = load_remote_config();
    cfg.lan_only = lan_only;
    save_remote_config(&cfg).map_err(|e| e.to_string())?;
    let sharing_on = cfg.enabled;
    std::thread::Builder::new()
        .name("metis-remote-fw-lan".into())
        .spawn(move || {
            if sharing_on {
                if lan_only {
                    if let Err(err) = firewall::apply() {
                        tracing::warn!(%err, "LAN-only firewall apply failed");
                    }
                } else if let Err(err) = firewall::clear() {
                    tracing::warn!(%err, "LAN-only firewall clear failed");
                }
            } else if !lan_only {
                let _ = firewall::clear();
            }
        })
        .ok();
    Ok(())
}

pub fn firewall_apply() -> Result<FirewallStatus, String> {
    firewall::apply()
}

pub fn firewall_clear() -> Result<FirewallStatus, String> {
    firewall::clear()
}

pub fn firewall_status() -> FirewallStatus {
    firewall::status()
}

pub fn firewall_apply_as_root() -> Result<FirewallStatus, String> {
    firewall::apply_as_root()
}

pub fn firewall_clear_as_root() -> Result<FirewallStatus, String> {
    firewall::clear_as_root()
}

pub fn rustdesk_status() -> RustDeskStatus {
    rustdesk::status()
}

pub fn rustdesk_enable() -> Result<(), String> {
    rustdesk::enable()
}

pub fn rustdesk_disable(kill: bool) -> Result<(), String> {
    rustdesk::disable(kill)
}

pub fn firewall_rustdesk_apply() -> Result<FirewallStatus, String> {
    firewall::apply_rustdesk()
}

pub fn firewall_rustdesk_clear() -> Result<FirewallStatus, String> {
    firewall::clear_rustdesk()
}

pub fn firewall_rustdesk_status() -> FirewallStatus {
    firewall::status_rustdesk()
}

pub fn firewall_rustdesk_apply_as_root() -> Result<FirewallStatus, String> {
    firewall::apply_rustdesk_as_root()
}

pub fn firewall_rustdesk_clear_as_root() -> Result<FirewallStatus, String> {
    firewall::clear_rustdesk_as_root()
}

/// Set RDP username/password via grdctl (headless store).
pub fn set_password(username: &str, password: &str) -> Result<(), String> {
    if username.trim().is_empty() {
        return Err("Username must not be empty".into());
    }
    if password.is_empty() {
        return Err("Password must not be empty".into());
    }
    let mut owned = password.to_string();
    let result = set_credentials(username.trim(), &owned);
    owned.zeroize();
    result
}

/// Called from metis-session when `remote.json` has enabled + auto_start.
pub fn autostart_from_config() -> Result<(), String> {
    let cfg = load_remote_config();
    if !cfg.enabled || !cfg.auto_start {
        return Ok(());
    }
    if !gnome_rdp::grdctl_available() {
        tracing::warn!("remote autostart skipped: gnome-remote-desktop not installed");
        return Ok(());
    }
    let snap = status_snapshot();
    if !snap.password_set {
        tracing::warn!("remote autostart skipped: RDP credentials not set");
        return Ok(());
    }
    enable_sharing().map_err(|e| {
        tracing::warn!(%e, "remote autostart failed");
        e
    })?;
    if cfg.lan_only {
        if let Err(err) = firewall::apply() {
            tracing::warn!(%err, "remote autostart: LAN-only firewall apply failed");
        }
    }
    Ok(())
}
