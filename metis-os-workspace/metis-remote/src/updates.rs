//! Software updates: PackageKit (`pkcon`) primary, distro CLI fallback, Flatpak, fwupd.
//!
//! Check/list run unprivileged. Refresh/apply for PackageKit fallbacks escalate via
//! `pkexec metis-remote pk-updates-*`. Flatpak/fwupd use their own polkit when needed.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::SyncSender;
use std::thread;

use serde::{Deserialize, Serialize};

use crate::pkhelpers::{require_root, run_pkexec};
use metis_config::{UpdateSources, UpdatesConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSourceKind {
    PackageKit,
    Flatpak,
    Firmware,
    Distro,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateItem {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub source: UpdateSourceKind,
    /// PackageKit security / important when detectable.
    #[serde(default)]
    pub security: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateSnapshot {
    #[serde(default)]
    pub packages: Vec<UpdateItem>,
    #[serde(default)]
    pub flatpaks: Vec<UpdateItem>,
    #[serde(default)]
    pub firmware: Vec<UpdateItem>,
    #[serde(default)]
    pub reboot_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UpdateSnapshot {
    pub fn total_count(&self) -> usize {
        self.packages.len() + self.flatpaks.len() + self.firmware.len()
    }

    pub fn is_empty(&self) -> bool {
        self.total_count() == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UpdateProgressEvent {
    Log {
        line: String,
    },
    Progress {
        percent: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    Phase {
        name: String,
    },
    /// dpkg stopped on a modified `/etc` file — the updater should ask the user
    /// Keep mine vs Use package version, then call [`resolve_conffile_conflict`].
    ConffileConflict {
        package: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_path: Option<String>,
    },
    Finished {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        reboot_required: bool,
    },
}

/// How to finish a pending dpkg conffile prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfFileChoice {
    /// Keep the locally modified file (`--force-confold`).
    KeepLocal,
    /// Install the package maintainer's file (`--force-confnew`).
    UsePackage,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum UpdatesError {
    #[error("package manager is busy")]
    Busy,
    #[error("{0}")]
    Message(String),
}

fn command_exists(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Soft list commands must not hang the machine; `fwupdmgr` / `flatpak` can
/// block on D-Bus for minutes and stall pointer motion system-wide.
const CHECK_TIMEOUT_SECS: u64 = 25;

fn command_output(bin: &str, args: &[&str]) -> Result<std::process::Output, UpdatesError> {
    let output = if command_exists("timeout") {
        let mut argv = Vec::with_capacity(args.len() + 2);
        argv.push(CHECK_TIMEOUT_SECS.to_string());
        argv.push(bin.to_string());
        argv.extend(args.iter().map(|s| (*s).to_string()));
        Command::new("timeout")
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| UpdatesError::Message(format!("timeout {bin}: {e}")))?
    } else {
        Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| UpdatesError::Message(format!("{bin}: {e}")))?
    };
    // GNU timeout exits 124 when the child is killed for exceeding the limit.
    if output.status.code() == Some(124) {
        return Err(UpdatesError::Message(format!(
            "{bin} timed out after {CHECK_TIMEOUT_SECS}s"
        )));
    }
    Ok(output)
}

fn packagekit_available() -> bool {
    command_exists("pkcon")
}

fn distro_id() -> String {
    let Ok(text) = std::fs::read_to_string("/etc/os-release") else {
        return String::new();
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("ID=") {
            return rest.trim().trim_matches('"').to_ascii_lowercase();
        }
    }
    String::new()
}

fn package_manager_busy() -> bool {
    // Unprivileged processes cannot probe dpkg locks via O_WRONLY. Rely on
    // pkcon/apt exit text ("busy"/"locked") instead.
    false
}

pub fn reboot_required() -> bool {
    Path::new("/var/run/reboot-required").is_file() || Path::new("/run/reboot-required").is_file()
}

/// Full check across enabled sources (no elevation for list; may soft-fail per source).
pub fn check(sources: &UpdateSources) -> UpdateSnapshot {
    check_inner(sources, false)
}

/// Background poll: PackageKit / distro list only.
///
/// Flatpak `remote-ls --updates` and `fwupdmgr get-updates` talk to system
/// daemons and routinely peg D-Bus / disk for long enough to make the whole
/// session (including pointer motion) feel stuck. Those sources run only on
/// manual checks and after apply.
pub fn check_background(sources: &UpdateSources) -> UpdateSnapshot {
    check_inner(sources, true)
}

fn check_inner(sources: &UpdateSources, background: bool) -> UpdateSnapshot {
    let mut snap = UpdateSnapshot {
        reboot_required: reboot_required(),
        ..Default::default()
    };

    if sources.packagekit {
        match check_packagekit() {
            Ok((items, backend)) => {
                snap.packages = items;
                snap.backend = Some(backend);
            }
            Err(UpdatesError::Busy) => {
                snap.error = Some("Package manager is busy".into());
            }
            Err(UpdatesError::Message(err)) => {
                if snap.error.is_none() {
                    snap.error = Some(err);
                }
            }
        }
    }

    if !background && sources.flatpak {
        match check_flatpak() {
            Ok(items) => snap.flatpaks = items,
            Err(err) => {
                if snap.error.is_none() {
                    snap.error = Some(err.to_string());
                }
            }
        }
    }

    if !background && sources.fwupd {
        match check_fwupd() {
            Ok(items) => snap.firmware = items,
            Err(err) => {
                if snap.error.is_none() {
                    snap.error = Some(err.to_string());
                }
            }
        }
    }

    snap.reboot_required = reboot_required() || snap.reboot_required;
    snap
}

pub fn check_from_config(cfg: &UpdatesConfig) -> UpdateSnapshot {
    check(&cfg.sources)
}

pub fn check_background_from_config(cfg: &UpdatesConfig) -> UpdateSnapshot {
    check_background(&cfg.sources)
}

fn check_packagekit() -> Result<(Vec<UpdateItem>, String), UpdatesError> {
    if package_manager_busy() {
        return Err(UpdatesError::Busy);
    }
    if packagekit_available() {
        let items = pkcon_get_updates()?;
        return Ok((items, "packagekit".into()));
    }
    let items = distro_list_updates()?;
    Ok((items, format!("distro:{}", distro_id())))
}

fn pkcon_get_updates() -> Result<Vec<UpdateItem>, UpdatesError> {
    let output = command_output("pkcon", &["get-updates", "-p"])?;
    // pkcon exits 5 when no updates; still OK.
    let code = output.status.code().unwrap_or(1);
    if !output.status.success() && code != 5 {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.to_ascii_lowercase().contains("busy") || err.to_ascii_lowercase().contains("locked")
        {
            return Err(UpdatesError::Busy);
        }
        // Fall through to parse stdout anyway; some versions write errors but list packages.
        if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            return Err(UpdatesError::Message(if err.trim().is_empty() {
                format!("pkcon get-updates exited {code}")
            } else {
                err.trim().to_string()
            }));
        }
    }
    Ok(parse_pkcon_updates(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_pkcon_updates(text: &str) -> Vec<UpdateItem> {
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Transaction:") || line.starts_with("Results:") {
            continue;
        }
        // Formats vary: "Installed  foo-1.0-1.x86_64 (repo) summary"
        // or "foo;1.0;x86_64;repo"
        if let Some((name, ver)) = line.split_once(';') {
            let name = name.trim();
            if name.is_empty() || name.contains(' ') {
                continue;
            }
            let version = ver
                .split(';')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            items.push(UpdateItem {
                id: name.to_string(),
                name: name.to_string(),
                version: version.map(str::to_string),
                summary: None,
                source: UpdateSourceKind::PackageKit,
                security: false,
            });
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let security = lower.contains("security");
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        // Skip status words
        let start = if matches!(
            tokens[0],
            "Installed" | "Available" | "Updating" | "Downloading" | "Interesting"
        ) {
            1
        } else {
            0
        };
        if start >= tokens.len() {
            continue;
        }
        let pkg = tokens[start];
        let (name, version) = split_nevra(pkg);
        if name.is_empty() {
            continue;
        }
        let summary = if tokens.len() > start + 1 {
            Some(tokens[start + 1..].join(" "))
        } else {
            None
        };
        items.push(UpdateItem {
            id: name.clone(),
            name,
            version,
            summary,
            source: UpdateSourceKind::PackageKit,
            security,
        });
    }
    items
}

fn split_nevra(pkg: &str) -> (String, Option<String>) {
    // foo-1.2.3-1.x86_64 or foo_1.2.3
    let bare = pkg.trim_end_matches([',', ')']);
    if let Some((name, rest)) = bare.rsplit_once('-') {
        // Heuristic: version often starts with digit
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return (name.to_string(), Some(rest.to_string()));
        }
    }
    (bare.to_string(), None)
}

fn distro_list_updates() -> Result<Vec<UpdateItem>, UpdatesError> {
    let id = distro_id();
    if id == "fedora" || id == "rhel" || id == "centos" || command_exists("dnf") {
        return dnf_check_update();
    }
    if id == "arch" || id == "manjaro" || command_exists("checkupdates") {
        return pacman_checkupdates();
    }
    // Debian/Ubuntu default
    apt_list_upgradable()
}

fn apt_list_upgradable() -> Result<Vec<UpdateItem>, UpdatesError> {
    let output = Command::new("apt")
        .args(["list", "--upgradable"])
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| UpdatesError::Message(format!("apt: {e}")))?;
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // name/suite version arch [upgradable from: …]
        if !line.contains("upgradable") {
            continue;
        }
        let Some(name_suite) = line.split_whitespace().next() else {
            continue;
        };
        let name = name_suite.split('/').next().unwrap_or(name_suite);
        let version = line.split_whitespace().nth(1).map(str::to_string);
        items.push(UpdateItem {
            id: name.to_string(),
            name: name.to_string(),
            version,
            summary: None,
            source: UpdateSourceKind::Distro,
            security: line.to_ascii_lowercase().contains("security"),
        });
    }
    Ok(items)
}

fn dnf_check_update() -> Result<Vec<UpdateItem>, UpdatesError> {
    let output = Command::new("dnf")
        .args(["check-update", "-q"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| UpdatesError::Message(format!("dnf: {e}")))?;
    // exit 100 = updates available
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Last metadata") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let version = parts.next().map(str::to_string);
        items.push(UpdateItem {
            id: name.to_string(),
            name: name.to_string(),
            version,
            summary: None,
            source: UpdateSourceKind::Distro,
            security: false,
        });
    }
    Ok(items)
}

fn pacman_checkupdates() -> Result<Vec<UpdateItem>, UpdatesError> {
    let bin = if command_exists("checkupdates") {
        "checkupdates"
    } else {
        return Ok(Vec::new());
    };
    let output = Command::new(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| UpdatesError::Message(format!("{bin}: {e}")))?;
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let _old = parts.next();
        let new = parts.next().map(str::to_string);
        items.push(UpdateItem {
            id: name.to_string(),
            name: name.to_string(),
            version: new,
            summary: None,
            source: UpdateSourceKind::Distro,
            security: false,
        });
    }
    Ok(items)
}

fn check_flatpak() -> Result<Vec<UpdateItem>, UpdatesError> {
    if !command_exists("flatpak") {
        return Ok(Vec::new());
    }
    let output = command_output(
        "flatpak",
        &[
            "remote-ls",
            "--updates",
            "--columns=application,name,version",
        ],
    )?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.trim().is_empty() {
            return Ok(Vec::new());
        }
        // No remotes configured is fine.
        if err.to_ascii_lowercase().contains("no remotes") {
            return Ok(Vec::new());
        }
    }
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.is_empty() {
            continue;
        }
        let id = cols[0].trim();
        if id.is_empty() || id == "Application" {
            continue;
        }
        let name = cols.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());
        let version = cols
            .get(2)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        items.push(UpdateItem {
            id: id.to_string(),
            name: name.unwrap_or(id).to_string(),
            version,
            summary: None,
            source: UpdateSourceKind::Flatpak,
            security: false,
        });
    }
    Ok(items)
}

fn check_fwupd() -> Result<Vec<UpdateItem>, UpdatesError> {
    if !command_exists("fwupdmgr") {
        return Ok(Vec::new());
    }
    let output = command_output("fwupdmgr", &["get-updates", "--json"])?;
    if !output.status.success() {
        // No devices / no updates often exits non-zero.
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_fwupd_json(&text)
}

fn parse_fwupd_json(text: &str) -> Result<Vec<UpdateItem>, UpdatesError> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    let devices = v
        .get("Devices")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    for dev in devices {
        let name = dev
            .get("Name")
            .and_then(|n| n.as_str())
            .unwrap_or("Firmware")
            .to_string();
        let id = dev
            .get("DeviceId")
            .and_then(|n| n.as_str())
            .unwrap_or(&name)
            .to_string();
        let version = dev
            .get("Releases")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|rel| rel.get("Version"))
            .and_then(|ver| ver.as_str())
            .map(str::to_string);
        let summary = dev
            .get("Releases")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|rel| rel.get("Description"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        items.push(UpdateItem {
            id,
            name,
            version,
            summary,
            source: UpdateSourceKind::Firmware,
            security: true,
        });
    }
    Ok(items)
}

fn emit(tx: &Option<SyncSender<UpdateProgressEvent>>, ev: UpdateProgressEvent) {
    let Some(tx) = tx else {
        return;
    };
    // Never drop terminal / phase / percent / conffile events — only skip surplus logs.
    match &ev {
        UpdateProgressEvent::Finished { .. }
        | UpdateProgressEvent::Phase { .. }
        | UpdateProgressEvent::Progress { .. }
        | UpdateProgressEvent::ConffileConflict { .. } => {
            let _ = tx.send(ev);
        }
        UpdateProgressEvent::Log { .. } => {
            let _ = tx.try_send(ev);
        }
    }
}

/// Soft metadata refresh for listing — never elevates / never prompts polkit.
///
/// Matches GNOME Software: checking for updates is unprivileged. Stale caches
/// are fine; install still refreshes under elevation when needed.
pub fn refresh(
    sources: &UpdateSources,
    progress: Option<SyncSender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    if package_manager_busy() {
        return Err(UpdatesError::Busy);
    }
    if sources.packagekit && packagekit_available() {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: "Refreshing package metadata".into(),
            },
        );
        // Ignore auth / network failures — GetUpdates still works with a stale cache.
        let _ = run_streaming("pkcon", &["refresh", "--noninteractive"], &progress);
    }
    if sources.flatpak && command_exists("flatpak") {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: "Refreshing Flatpak remotes".into(),
            },
        );
        let _ = run_streaming("flatpak", &["update", "--appstream", "-y"], &progress);
    }
    if sources.fwupd && command_exists("fwupdmgr") {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: "Refreshing firmware metadata".into(),
            },
        );
        // May require auth; never escalate from the soft path.
        let _ = run_streaming("fwupdmgr", &["refresh", "--force"], &progress);
    }
    Ok(())
}

/// Which updates to install. `All` covers every enabled source; `Selected`
/// installs only the listed ids (empty list for a source skips that source).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateApplyScope {
    #[default]
    All,
    Selected {
        packages: Vec<String>,
        flatpaks: Vec<String>,
        firmware: Vec<String>,
    },
}

impl UpdateApplyScope {
    pub fn selected_count(&self) -> Option<usize> {
        match self {
            Self::All => None,
            Self::Selected {
                packages,
                flatpaks,
                firmware,
            } => Some(packages.len() + flatpaks.len() + firmware.len()),
        }
    }

    pub fn is_empty_selection(&self) -> bool {
        matches!(self.selected_count(), Some(0))
    }
}

/// Apply all enabled update sources. Streams progress events.
pub fn apply(
    sources: &UpdateSources,
    progress: Option<SyncSender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    apply_scope(sources, &UpdateApplyScope::All, progress)
}

/// Apply a subset (or all) of pending updates. Streams progress events.
pub fn apply_scope(
    sources: &UpdateSources,
    scope: &UpdateApplyScope,
    progress: Option<SyncSender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    if package_manager_busy() {
        return Err(UpdatesError::Busy);
    }
    if scope.is_empty_selection() {
        emit(
            &progress,
            UpdateProgressEvent::Finished {
                ok: false,
                error: Some("No updates selected".into()),
                reboot_required: reboot_required(),
            },
        );
        return Err(UpdatesError::Message("No updates selected".into()));
    }

    let mut ok = true;
    let mut last_err = None;

    let (do_packages, package_ids) = match scope {
        UpdateApplyScope::All => (sources.packagekit, None),
        UpdateApplyScope::Selected { packages, .. } => (
            sources.packagekit && !packages.is_empty(),
            Some(packages.as_slice()),
        ),
    };
    if do_packages {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: if package_ids.is_some() {
                    "Installing selected system updates".into()
                } else {
                    "Installing system updates".into()
                },
            },
        );
        let r = if packagekit_available() {
            // Do not soft-refresh first — `pkcon refresh` can block for minutes
            // on D-Bus/network and freezes pointer motion session-wide. PackageKit
            // update already brings metadata as needed under one auth prompt.
            let mut args = vec![
                "update".to_string(),
                "--noninteractive".to_string(),
                "-y".to_string(),
            ];
            if let Some(ids) = package_ids {
                args.extend(ids.iter().cloned());
            }
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            run_streaming("pkcon", &argv, &progress)
        } else {
            // Single elevation: refresh indexes + upgrade inside pk-updates-apply.
            escalate_apply(package_ids.unwrap_or(&[]), &progress)
        };
        if let Err(e) = r {
            ok = false;
            last_err = Some(e.to_string());
            emit(
                &progress,
                UpdateProgressEvent::Log {
                    line: format!("System updates failed: {e}"),
                },
            );
        }
    }

    let (do_flatpak, flatpak_ids) = match scope {
        UpdateApplyScope::All => (sources.flatpak, None),
        UpdateApplyScope::Selected { flatpaks, .. } => (
            sources.flatpak && !flatpaks.is_empty(),
            Some(flatpaks.as_slice()),
        ),
    };
    if do_flatpak && command_exists("flatpak") {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: if flatpak_ids.is_some() {
                    "Updating selected Flatpak apps".into()
                } else {
                    "Updating Flatpak apps".into()
                },
            },
        );
        let mut args = vec![
            "update".to_string(),
            "-y".to_string(),
            "--noninteractive".to_string(),
        ];
        if let Some(ids) = flatpak_ids {
            args.extend(ids.iter().cloned());
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(e) = run_streaming("flatpak", &argv, &progress) {
            ok = false;
            last_err = Some(e.to_string());
        }
    }

    let (do_fwupd, firmware_ids) = match scope {
        UpdateApplyScope::All => (sources.fwupd, None),
        UpdateApplyScope::Selected { firmware, .. } => (
            sources.fwupd && !firmware.is_empty(),
            Some(firmware.as_slice()),
        ),
    };
    if do_fwupd && command_exists("fwupdmgr") {
        emit(
            &progress,
            UpdateProgressEvent::Phase {
                name: if firmware_ids.is_some() {
                    "Updating selected firmware".into()
                } else {
                    "Updating firmware".into()
                },
            },
        );
        let r = if let Some(ids) = firmware_ids {
            let mut all_ok = true;
            let mut err = None;
            for id in ids {
                let args = ["update", id.as_str(), "--assume-yes", "--no-reboot-check"];
                if let Err(e) = run_streaming("fwupdmgr", &args, &progress) {
                    let msg = e.to_string();
                    if !msg.to_ascii_lowercase().contains("no updates") {
                        all_ok = false;
                        err = Some(msg);
                    }
                }
            }
            if all_ok {
                Ok(())
            } else {
                Err(UpdatesError::Message(
                    err.unwrap_or_else(|| "firmware update failed".into()),
                ))
            }
        } else if let Err(e) = run_streaming(
            "fwupdmgr",
            &["update", "--assume-yes", "--no-reboot-check"],
            &progress,
        ) {
            let msg = e.to_string();
            if msg.to_ascii_lowercase().contains("no updates") {
                Ok(())
            } else {
                Err(UpdatesError::Message(msg))
            }
        } else {
            Ok(())
        };
        if let Err(e) = r {
            ok = false;
            last_err = Some(e.to_string());
        }
    }

    let reboot = reboot_required();
    emit(
        &progress,
        UpdateProgressEvent::Finished {
            ok,
            error: last_err.clone(),
            reboot_required: reboot,
        },
    );
    if ok {
        Ok(())
    } else {
        Err(UpdatesError::Message(
            last_err.unwrap_or_else(|| "update failed".into()),
        ))
    }
}

fn escalate_apply(
    packages: &[String],
    progress: &Option<SyncSender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    emit(
        progress,
        UpdateProgressEvent::Log {
            line: if packages.is_empty() {
                "Elevating to install system updates…".into()
            } else {
                format!(
                    "Elevating to install {} selected package(s)…",
                    packages.len()
                )
            },
        },
    );
    let mut argv = vec!["pk-updates-apply".to_string()];
    argv.extend(packages.iter().cloned());
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let output =
        run_pkexec(&refs, std::time::Duration::from_secs(3600)).map_err(UpdatesError::Message)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stdout.lines() {
        emit(
            progress,
            UpdateProgressEvent::Log {
                line: line.to_string(),
            },
        );
        if let Some(pct) = parse_percent_line(line) {
            emit(
                progress,
                UpdateProgressEvent::Progress {
                    percent: pct,
                    item: None,
                },
            );
        }
    }
    for line in stderr.lines() {
        emit(
            progress,
            UpdateProgressEvent::Log {
                line: format!("! {line}"),
            },
        );
    }
    if !output.status.success() {
        let combined = format!("{stdout}\n{stderr}");
        emit_conffile_if_present(progress, &combined);
        return Err(UpdatesError::Message(if stderr.trim().is_empty() {
            format!("apply exited {}", output.status)
        } else {
            stderr.trim().to_string()
        }));
    }
    Ok(())
}

fn run_streaming(
    bin: &str,
    args: &[&str],
    progress: &Option<SyncSender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Helps apt backends under PackageKit avoid interactive conffile
        // prompts when the daemon inherits the client environment.
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("NEEDRESTART_MODE", "a")
        .env("UCF_FORCE_CONFFOLD", "1")
        .spawn()
        .map_err(|e| UpdatesError::Message(format!("{bin}: {e}")))?;

    // PackageKit / flatpak often write progress with CR updates or on stderr.
    // Reading only newline-delimited stdout left the UI stuck at 0% until exit
    // (and could deadlock if stderr filled while we blocked on stdout).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (line_tx, line_rx) = std::sync::mpsc::channel::<StreamLine>();

    let t_out = stdout.map(|pipe| {
        let tx = line_tx.clone();
        thread::spawn(move || drain_progress_pipe(pipe, false, tx))
    });
    let t_err = stderr.map(|pipe| {
        let tx = line_tx.clone();
        thread::spawn(move || drain_progress_pipe(pipe, true, tx))
    });
    drop(line_tx);

    let mut log_budget = 0u32;
    let mut err_budget = 0u32;
    let mut transcript = String::new();
    while let Ok(chunk) = line_rx.recv() {
        let line = chunk.text;
        if transcript.len() < 32_768 {
            transcript.push_str(&line);
            transcript.push('\n');
        }
        if let Some(pct) = parse_percent_line(&line) {
            emit(
                progress,
                UpdateProgressEvent::Progress {
                    percent: pct,
                    item: extract_item_name(&line),
                },
            );
        } else if let Some(name) = extract_item_name(&line) {
            // Status line without a percent — still surfaces activity in the UI.
            emit(
                progress,
                UpdateProgressEvent::Progress {
                    percent: 0,
                    item: Some(name),
                },
            );
        }
        if chunk.from_stderr {
            err_budget += 1;
            if err_budget <= 20 || err_budget.is_multiple_of(10) {
                emit(
                    progress,
                    UpdateProgressEvent::Log {
                        line: format!("! {line}"),
                    },
                );
            }
        } else {
            log_budget += 1;
            if log_budget <= 40 || log_budget.is_multiple_of(25) {
                emit(progress, UpdateProgressEvent::Log { line });
            }
        }
    }

    if let Some(t) = t_out {
        let _ = t.join();
    }
    if let Some(t) = t_err {
        let _ = t.join();
    }

    let status = child
        .wait()
        .map_err(|e| UpdatesError::Message(format!("{bin} wait: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        emit_conffile_if_present(progress, &transcript);
        Err(UpdatesError::Message(format!("{bin} exited with {status}")))
    }
}

struct StreamLine {
    text: String,
    from_stderr: bool,
}

/// Split on `\n` and `\r` so CR-style progress ("Downloading… 42%\r") is visible.
fn drain_progress_pipe(pipe: impl Read, from_stderr: bool, tx: std::sync::mpsc::Sender<StreamLine>) {
    let mut reader = pipe;
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) => {
                    if !buf.is_empty() {
                        let text = String::from_utf8_lossy(&buf).trim().to_string();
                        if !text.is_empty() {
                            let _ = tx.send(StreamLine { text, from_stderr });
                        }
                    }
                    return;
                }
                Ok(_) => {
                    if byte[0] == b'\n' || byte[0] == b'\r' {
                        break;
                    }
                    buf.push(byte[0]);
                    // Cap runaway lines (ANSI / binary noise).
                    if buf.len() >= 4096 {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        let text = String::from_utf8_lossy(&buf).trim().to_string();
        if !text.is_empty() {
            let _ = tx.send(StreamLine { text, from_stderr });
        }
    }
}

fn parse_percent_line(line: &str) -> Option<u32> {
    // "Downloading… 42%" or "Percentage: 42"
    if let Some(idx) = line.find('%') {
        let before = &line[..idx];
        let num: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return num.parse().ok();
    }
    None
}

fn extract_item_name(line: &str) -> Option<String> {
    for prefix in ["Updating ", "Downloading ", "Installing "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name = rest.split_whitespace().next()?;
            return Some(name.trim_matches(|c| c == ':' || c == '.').to_string());
        }
    }
    None
}

/// Root: refresh package indexes for the host distro.
pub fn refresh_as_root() -> Result<(), String> {
    require_root()?;
    let id = distro_id();
    let status = if id == "fedora" || id == "rhel" || id == "centos" || command_exists("dnf") {
        Command::new("dnf")
            .args(["makecache"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .status()
    } else if id == "arch" || id == "manjaro" || command_exists("pacman") {
        Command::new("pacman").args(["-Sy"]).status()
    } else {
        Command::new("apt-get")
            .args(["update"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .status()
    }
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("refresh exited {status}"))
    }
}

/// Root: apply distro package upgrades (refresh indexes first).
///
/// Empty `packages` upgrades everything available; otherwise only the named
/// packages are upgraded (`apt-get install --only-upgrade`, `dnf upgrade`,
/// `pacman -S`).
///
/// Conffile prompts are **not** auto-answered here — stdin is null under
/// pkexec, so a conflict fails the transaction and the Metis updater asks
/// Keep mine / Use package version, then calls [`configure_pending_as_root`].
pub fn apply_as_root(packages: &[String]) -> Result<(), String> {
    require_root()?;
    // Fold refresh into apply so Install prompts once, not twice.
    let _ = refresh_as_root();
    let id = distro_id();
    let status = if id == "fedora" || id == "rhel" || id == "centos" || command_exists("dnf") {
        let mut cmd = Command::new("dnf");
        cmd.arg("-y").arg("upgrade");
        for pkg in packages {
            cmd.arg(pkg);
        }
        cmd.status()
    } else if id == "arch" || id == "manjaro" || command_exists("pacman") {
        if packages.is_empty() {
            Command::new("pacman")
                .args(["-Syu", "--noconfirm"])
                .status()
        } else {
            let mut cmd = Command::new("pacman");
            cmd.args(["-S", "--noconfirm", "--needed"]);
            for pkg in packages {
                cmd.arg(pkg);
            }
            cmd.status()
        }
    } else if packages.is_empty() {
        apt_noninteractive(&["-y", "upgrade"], AptConfPolicy::Prompt)
    } else {
        let mut args = vec![
            "-y".to_string(),
            "--only-upgrade".to_string(),
            "install".to_string(),
        ];
        args.extend(packages.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        apt_noninteractive(&refs, AptConfPolicy::Prompt)
    }
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apply exited {status}"))
    }
}

/// Finish packages left unconfigured after a conffile prompt (Keep / Use package).
pub fn configure_pending_as_root(choice: ConfFileChoice) -> Result<(), String> {
    require_root()?;
    let force = match choice {
        ConfFileChoice::KeepLocal => "--force-confold",
        ConfFileChoice::UsePackage => "--force-confnew",
    };
    let status = Command::new("dpkg")
        .args([force, "--configure", "-a"])
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("UCF_FORCE_CONFFOLD", "1")
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("dpkg --configure exited {status}"));
    }
    // Clear any dependency skew left by the interrupted transaction.
    let policy = match choice {
        ConfFileChoice::KeepLocal => AptConfPolicy::KeepLocal,
        ConfFileChoice::UsePackage => AptConfPolicy::UsePackage,
    };
    let fix = apt_noninteractive(&["-y", "-f", "install"], policy).map_err(|e| e.to_string())?;
    if fix.success() {
        Ok(())
    } else {
        Err(format!("apt-get -f install exited {fix}"))
    }
}

/// Unprivileged: elevate and finish a pending conffile conflict.
pub fn resolve_conffile_conflict(choice: ConfFileChoice) -> Result<(), String> {
    let mode = match choice {
        ConfFileChoice::KeepLocal => "keep",
        ConfFileChoice::UsePackage => "package",
    };
    let output = run_pkexec(
        &["pk-updates-configure", mode],
        std::time::Duration::from_secs(600),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(if err.trim().is_empty() {
            format!("configure exited {}", output.status)
        } else {
            err.trim().to_string()
        })
    }
}

#[derive(Clone, Copy)]
enum AptConfPolicy {
    /// Fail into a Metis dialog when a conffile conflicts (interactive updater).
    Prompt,
    KeepLocal,
    UsePackage,
}

/// `apt-get` with debconf noninteractive. Force-conf* only when the user (or
/// auto-apply) already chose Keep / Use package — never silently for Prompt.
fn apt_noninteractive(
    args: &[&str],
    conf: AptConfPolicy,
) -> std::io::Result<std::process::ExitStatus> {
    let mut cmd = Command::new("apt-get");
    match conf {
        AptConfPolicy::Prompt => {}
        AptConfPolicy::KeepLocal => {
            cmd.args([
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confold",
            ]);
        }
        AptConfPolicy::UsePackage => {
            cmd.args([
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confnew",
            ]);
        }
    }
    cmd.args(args);
    cmd.env("DEBIAN_FRONTEND", "noninteractive");
    cmd.env("NEEDRESTART_MODE", "a");
    if matches!(conf, AptConfPolicy::KeepLocal) {
        cmd.env("UCF_FORCE_CONFFOLD", "1");
    }
    cmd.status()
}

fn emit_conffile_if_present(progress: &Option<SyncSender<UpdateProgressEvent>>, text: &str) {
    if let Some((package, config_path)) = parse_conffile_conflict(text) {
        emit(
            progress,
            UpdateProgressEvent::ConffileConflict {
                package,
                config_path,
            },
        );
    }
}

fn is_conffile_prompt_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("conffile prompt")
        || t.contains("end of file on stdin at conffile")
        || (t.contains("configuration file")
            && t.contains("what would you like to do about it"))
}

fn parse_conffile_conflict(text: &str) -> Option<(String, Option<String>)> {
    if !is_conffile_prompt_error(text) {
        return None;
    }
    let mut config_path = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("Configuration file ")
            .or_else(|| trimmed.strip_prefix("configuration file "))
        {
            let path = rest
                .trim()
                .trim_matches(|c| c == '«' || c == '»' || c == '\'' || c == '"')
                .trim();
            if path.starts_with('/') {
                config_path = Some(path.to_string());
            }
        }
    }
    let mut package = None;
    for line in text.lines() {
        // May be glued onto the prompt line: "… [default=N] ? dpkg: error processing package foo (--configure):"
        if let Some(idx) = line.find("dpkg: error processing package ") {
            let rest = &line[idx + "dpkg: error processing package ".len()..];
            let name = rest.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() {
                package = Some(name.to_string());
                break;
            }
        }
    }
    Some((
        package.unwrap_or_else(|| "system package".into()),
        config_path,
    ))
}

/// Unprivileged wrappers that escalate for distro fallback refresh/apply.
pub fn refresh_privileged() -> Result<(), String> {
    let output = run_pkexec(&["pk-updates-refresh"], std::time::Duration::from_secs(600))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(crate::pkhelpers::pkexec_failure_message(&output, ""))
    }
}

pub fn apply_privileged(packages: &[String]) -> Result<(), String> {
    let mut argv = vec!["pk-updates-apply".to_string()];
    argv.extend(packages.iter().cloned());
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let output = run_pkexec(&refs, std::time::Duration::from_secs(3600))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(crate::pkhelpers::pkexec_failure_message(&output, ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pkcon_semicolon() {
        let items = parse_pkcon_updates("bash;5.2;amd64;ubuntu\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "bash");
    }

    #[test]
    fn parse_percent() {
        assert_eq!(parse_percent_line("Downloading foo 42%"), Some(42));
        assert_eq!(parse_percent_line("Percentage:	87%"), Some(87));
        assert_eq!(parse_percent_line("Downloading foo 42%\r"), Some(42));
    }

    #[test]
    fn parse_conffile_from_apt_log() {
        let log = r#"
Configuration file «/etc/apparmor.d/abstractions/libvirt-qemu»
 ==> Modified (by you or by a script) since installation.
 ==> Package distributor has shipped an updated version.
   What would you like to do about it ?
*** libvirt-qemu (Y/I/N/O/D/Z) [default=N] ? dpkg: error processing package libvirt-daemon-driver-qemu (--configure):
 end of file on stdin at conffile prompt
"#;
        let (pkg, path) = parse_conffile_conflict(log).expect("detect");
        assert_eq!(pkg, "libvirt-daemon-driver-qemu");
        assert_eq!(
            path.as_deref(),
            Some("/etc/apparmor.d/abstractions/libvirt-qemu")
        );
    }
}
