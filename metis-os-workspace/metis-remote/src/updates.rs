//! Software updates: PackageKit (`pkcon`) primary, distro CLI fallback, Flatpak, fwupd.
//!
//! Check/list run unprivileged. Refresh/apply for PackageKit fallbacks escalate via
//! `pkexec metis-remote pk-updates-*`. Flatpak/fwupd use their own polkit when needed.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

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
    Log { line: String },
    Progress {
        percent: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    Phase { name: String },
    Finished {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        reboot_required: bool,
    },
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
    Path::new("/var/run/reboot-required").is_file()
        || Path::new("/run/reboot-required").is_file()
}

/// Full check across enabled sources (no elevation for list; may soft-fail per source).
pub fn check(sources: &UpdateSources) -> UpdateSnapshot {
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

    if sources.flatpak {
        match check_flatpak() {
            Ok(items) => snap.flatpaks = items,
            Err(err) => {
                if snap.error.is_none() {
                    snap.error = Some(err.to_string());
                }
            }
        }
    }

    if sources.fwupd {
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
    let output = Command::new("pkcon")
        .args(["get-updates", "-p"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| UpdatesError::Message(format!("pkcon: {e}")))?;
    // pkcon exits 5 when no updates; still OK.
    let code = output.status.code().unwrap_or(1);
    if !output.status.success() && code != 5 {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.to_ascii_lowercase().contains("busy")
            || err.to_ascii_lowercase().contains("locked")
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
    Ok(parse_pkcon_updates(&String::from_utf8_lossy(&output.stdout)))
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
            let version = ver.split(';').next().map(str::trim).filter(|s| !s.is_empty());
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
    let output = Command::new("flatpak")
        .args([
            "remote-ls",
            "--updates",
            "--columns=application,name,version",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| UpdatesError::Message(format!("flatpak: {e}")))?;
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
        let version = cols.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
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
    let output = Command::new("fwupdmgr")
        .args(["get-updates", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| UpdatesError::Message(format!("fwupdmgr: {e}")))?;
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

fn emit(tx: &Option<Sender<UpdateProgressEvent>>, ev: UpdateProgressEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(ev);
    }
}

/// Refresh metadata (PackageKit / apt update). Escalates for distro fallback.
pub fn refresh(sources: &UpdateSources, progress: Option<Sender<UpdateProgressEvent>>) -> Result<(), UpdatesError> {
    if package_manager_busy() {
        return Err(UpdatesError::Busy);
    }
    if sources.packagekit {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Refreshing package metadata".into(),
        });
        if packagekit_available() {
            run_streaming(
                "pkcon",
                &["refresh", "--noninteractive"],
                &progress,
            )?;
        } else {
            escalate_refresh(&progress)?;
        }
    }
    if sources.flatpak && command_exists("flatpak") {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Refreshing Flatpak remotes".into(),
        });
        let _ = run_streaming("flatpak", &["update", "--appstream", "-y"], &progress);
    }
    if sources.fwupd && command_exists("fwupdmgr") {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Refreshing firmware metadata".into(),
        });
        let _ = run_streaming("fwupdmgr", &["refresh", "--force"], &progress);
    }
    Ok(())
}

fn escalate_refresh(progress: &Option<Sender<UpdateProgressEvent>>) -> Result<(), UpdatesError> {
    emit(progress, UpdateProgressEvent::Log {
        line: "Elevating to refresh package indexes…".into(),
    });
    let output = run_pkexec(
        &["pk-updates-refresh"],
        std::time::Duration::from_secs(600),
    )
    .map_err(UpdatesError::Message)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        emit(progress, UpdateProgressEvent::Log {
            line: line.to_string(),
        });
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(UpdatesError::Message(if err.trim().is_empty() {
            format!("refresh exited {}", output.status)
        } else {
            err.trim().to_string()
        }));
    }
    Ok(())
}

/// Apply all enabled update sources. Streams progress events.
pub fn apply(sources: &UpdateSources, progress: Option<Sender<UpdateProgressEvent>>) -> Result<(), UpdatesError> {
    if package_manager_busy() {
        return Err(UpdatesError::Busy);
    }

    let mut ok = true;
    let mut last_err = None;

    if sources.packagekit {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Installing system updates".into(),
        });
        let r = if packagekit_available() {
            run_streaming(
                "pkcon",
                &["update", "--noninteractive", "-y"],
                &progress,
            )
        } else {
            escalate_apply(&progress)
        };
        if let Err(e) = r {
            ok = false;
            last_err = Some(e.to_string());
            emit(&progress, UpdateProgressEvent::Log {
                line: format!("System updates failed: {e}"),
            });
        }
    }

    if sources.flatpak && command_exists("flatpak") {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Updating Flatpak apps".into(),
        });
        if let Err(e) = run_streaming(
            "flatpak",
            &["update", "-y", "--noninteractive"],
            &progress,
        ) {
            ok = false;
            last_err = Some(e.to_string());
        }
    }

    if sources.fwupd && command_exists("fwupdmgr") {
        emit(&progress, UpdateProgressEvent::Phase {
            name: "Updating firmware".into(),
        });
        if let Err(e) = run_streaming(
            "fwupdmgr",
            &["update", "--assume-yes", "--no-reboot-check"],
            &progress,
        ) {
            // No updates is not fatal.
            let msg = e.to_string();
            if !msg.to_ascii_lowercase().contains("no updates") {
                ok = false;
                last_err = Some(msg);
            }
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

fn escalate_apply(progress: &Option<Sender<UpdateProgressEvent>>) -> Result<(), UpdatesError> {
    emit(progress, UpdateProgressEvent::Log {
        line: "Elevating to install system updates…".into(),
    });
    let output = run_pkexec(
        &["pk-updates-apply"],
        std::time::Duration::from_secs(3600),
    )
    .map_err(UpdatesError::Message)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        emit(progress, UpdateProgressEvent::Log {
            line: line.to_string(),
        });
        if let Some(pct) = parse_percent_line(line) {
            emit(progress, UpdateProgressEvent::Progress {
                percent: pct,
                item: None,
            });
        }
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(UpdatesError::Message(if err.trim().is_empty() {
            format!("apply exited {}", output.status)
        } else {
            err.trim().to_string()
        }));
    }
    Ok(())
}

fn run_streaming(
    bin: &str,
    args: &[&str],
    progress: &Option<Sender<UpdateProgressEvent>>,
) -> Result<(), UpdatesError> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| UpdatesError::Message(format!("{bin}: {e}")))?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(pct) = parse_percent_line(&line) {
                emit(progress, UpdateProgressEvent::Progress {
                    percent: pct,
                    item: extract_item_name(&line),
                });
            }
            emit(progress, UpdateProgressEvent::Log { line });
        }
    }
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            emit(progress, UpdateProgressEvent::Log {
                line: format!("! {line}"),
            });
        }
    }
    let status = child
        .wait()
        .map_err(|e| UpdatesError::Message(format!("{bin} wait: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(UpdatesError::Message(format!(
            "{bin} exited with {status}"
        )))
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
        Command::new("pacman")
            .args(["-Sy"])
            .status()
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

/// Root: apply distro package upgrades.
pub fn apply_as_root() -> Result<(), String> {
    require_root()?;
    let id = distro_id();
    let status = if id == "fedora" || id == "rhel" || id == "centos" || command_exists("dnf") {
        Command::new("dnf")
            .args(["-y", "upgrade"])
            .status()
    } else if id == "arch" || id == "manjaro" || command_exists("pacman") {
        Command::new("pacman")
            .args(["-Syu", "--noconfirm"])
            .status()
    } else {
        Command::new("apt-get")
            .args(["-y", "upgrade"])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .status()
    }
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apply exited {status}"))
    }
}

/// Unprivileged wrappers that escalate for distro fallback refresh/apply.
pub fn refresh_privileged() -> Result<(), String> {
    let output = run_pkexec(
        &["pk-updates-refresh"],
        std::time::Duration::from_secs(600),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(crate::pkhelpers::pkexec_failure_message(&output, ""))
    }
}

pub fn apply_privileged() -> Result<(), String> {
    let output = run_pkexec(
        &["pk-updates-apply"],
        std::time::Duration::from_secs(3600),
    )?;
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
    }
}
