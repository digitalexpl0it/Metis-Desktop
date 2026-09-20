//! LAN-only firewall helpers for RDP port 3389.
//!
//! When `remote.json` has `lan_only: true`, Metis applies an idempotent nftables
//! (preferred) or ufw rule set that accepts TCP 3389 only from private /
//! loopback / link-local ranges and drops other inbound traffic to that port.
//!
//! Unprivileged `nft list` / `ufw status` often cannot see rules (permission
//! denied). After a successful apply/clear we therefore persist
//! `firewall_applied` in `remote.json` so Settings status stays honest.

use std::path::Path;
use std::process::{Command, Output};

use metis_config::{load_remote_config, save_remote_config};

const NFT_TABLE: &str = "metis_rdp";
const UFW_COMMENT: &str = "metis-rdp-lan-only";
const PORT: u16 = 3389;

const LAN_V4: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8",
];
const LAN_V6: &[&str] = &["fe80::/10", "::1/128"];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
pub struct FirewallStatus {
    /// Whether Metis believes LAN-only rules are present.
    pub applied: bool,
    /// Backend that owns the rules: `nft`, `ufw`, or empty when none.
    pub backend: String,
    /// Human-readable detail / last error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

fn run(cmd: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {cmd}: {e}"))
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn resolve_bin(name: &str) -> Option<String> {
    // Fixed locations only — never `sh -c command -v` (Phase 15 §B).
    for dir in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
        let candidate = format!("{dir}/{name}");
        if Path::new(&candidate).is_file() {
            return Some(candidate);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

fn nft_bin() -> Option<String> {
    resolve_bin("nft")
}

fn ufw_bin() -> Option<String> {
    resolve_bin("ufw")
}

fn nft_available() -> bool {
    nft_bin().is_some()
}

fn ufw_available() -> bool {
    ufw_bin().is_some()
}

/// `ufw status` reports "Status: active" only when the firewall is enabled.
fn ufw_is_active() -> bool {
    let Some(ufw) = ufw_bin() else {
        return false;
    };
    let Ok(output) = run(&ufw, &["status"]) else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    text.contains("status: active")
}

fn nft_table_present() -> bool {
    let Some(nft) = nft_bin() else {
        return false;
    };
    run(&nft, &["list", "table", "inet", NFT_TABLE])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ufw_rules_present() -> bool {
    let Some(ufw) = ufw_bin() else {
        return false;
    };
    let Ok(output) = run(&ufw, &["status", "numbered"]) else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.contains(UFW_COMMENT)
}

fn persist(applied: bool, backend: &str) {
    let mut cfg = load_remote_config();
    cfg.firewall_applied = applied;
    cfg.firewall_backend = if applied {
        backend.to_string()
    } else {
        String::new()
    };
    if applied {
        cfg.firewall_last_error = None;
    }
    if let Err(err) = save_remote_config(&cfg) {
        tracing::warn!(%err, "failed to persist firewall_applied in remote.json");
    }
}

fn persist_error(err: &str) {
    let mut cfg = load_remote_config();
    cfg.firewall_applied = false;
    cfg.firewall_backend.clear();
    cfg.firewall_last_error = Some(err.to_string());
    if let Err(e) = save_remote_config(&cfg) {
        tracing::warn!(%e, "failed to persist firewall_last_error");
    }
}

fn preferred_backend_label() -> String {
    if nft_available() {
        "nft".into()
    } else if ufw_available() && ufw_is_active() {
        "ufw".into()
    } else {
        String::new()
    }
}

/// Which backend we can use to enforce LAN-only rules (no pkexec).
pub fn enforceable_backend() -> Result<&'static str, String> {
    if nft_available() {
        return Ok("nft");
    }
    if ufw_available() {
        if ufw_is_active() {
            return Ok("ufw");
        }
        return Err(
            "ufw is installed but inactive — run `sudo ufw enable`, or install nftables \
             (`sudo apt install nftables`) so Metis can restrict RDP to the LAN"
                .into(),
        );
    }
    Err(
        "Neither nftables (`nft`) nor an active ufw is available — install nftables \
         (recommended) or enable ufw to enforce LAN-only RDP"
            .into(),
    )
}

/// Live probe only — may return false negatives when not root.
fn probe_live() -> Option<FirewallStatus> {
    if nft_available() && nft_table_present() {
        return Some(FirewallStatus {
            applied: true,
            backend: "nft".into(),
            detail: None,
        });
    }
    // Only treat ufw rules as live if ufw itself is active.
    if ufw_available() && ufw_is_active() && ufw_rules_present() {
        return Some(FirewallStatus {
            applied: true,
            backend: "ufw".into(),
            detail: None,
        });
    }
    None
}

/// Report whether LAN-only rules appear active (live probe, else persisted).
pub fn status() -> FirewallStatus {
    if let Some(live) = probe_live() {
        // Keep remote.json in sync when we can see rules.
        persist(true, &live.backend);
        return live;
    }
    let cfg = load_remote_config();
    if cfg.firewall_applied {
        let backend = if cfg.firewall_backend.is_empty() {
            preferred_backend_label()
        } else {
            cfg.firewall_backend
        };
        return FirewallStatus {
            applied: true,
            backend,
            detail: None,
        };
    }
    FirewallStatus {
        applied: false,
        backend: String::new(),
        detail: None,
    }
}

/// Apply LAN-only rules. Escalates via `pkexec` when not root.
pub fn apply() -> Result<FirewallStatus, String> {
    // Idempotent fast path: already applied — skip another pkexec round-trip.
    let current = status();
    if current.applied {
        return Ok(current);
    }
    // Clear a stale failure so Settings shows "Applying…" instead of the old error.
    {
        let mut cfg = load_remote_config();
        if cfg.firewall_last_error.take().is_some() {
            let _ = save_remote_config(&cfg);
        }
    }
    let backend = match enforceable_backend() {
        Ok(b) => b,
        Err(err) => {
            persist_error(&err);
            return Err(err);
        }
    };
    let result = if is_root() {
        match apply_as_root() {
            Ok(s) => s,
            Err(err) => {
                persist_error(&err);
                return Err(err);
            }
        }
    } else {
        match escalate(&["firewall", "apply-as-root"]) {
            Ok(()) => FirewallStatus {
                applied: true,
                backend: backend.to_string(),
                detail: None,
            },
            Err(err) => {
                let msg = if err.contains("pkexec")
                    || err.contains("cancelled")
                    || err.contains("denied")
                {
                    format!(
                        "{err}. No password dialog? Metis needs a PolicyKit agent \
                         (e.g. install `policykit-1-gnome` or `mate-polkit`) in the session, \
                         or run: pkexec metis-remote firewall apply"
                    )
                } else {
                    err
                };
                persist_error(&msg);
                return Err(msg);
            }
        }
    };
    if result.applied {
        persist(true, &result.backend);
    }
    Ok(result)
}

/// Clear LAN-only rules. Escalates via `pkexec` when not root.
///
/// No-ops (no pkexec) when Metis rules are not present — otherwise disable
/// would hang on a PolicyKit prompt even when there is nothing to clear.
pub fn clear() -> Result<FirewallStatus, String> {
    let current = status();
    if !current.applied {
        persist(false, "");
        return Ok(FirewallStatus {
            applied: false,
            backend: String::new(),
            detail: current.detail,
        });
    }
    let result = if is_root() {
        clear_as_root()?
    } else {
        escalate(&["firewall", "clear-as-root"])?;
        FirewallStatus {
            applied: false,
            backend: String::new(),
            detail: None,
        }
    };
    persist(false, "");
    Ok(result)
}

/// Privileged entry used under `pkexec` / already-root.
pub fn apply_as_root() -> Result<FirewallStatus, String> {
    if !is_root() {
        return Err("firewall apply-as-root requires root".into());
    }
    // Prefer nftables — works without requiring a separately enabled ufw service.
    if nft_available() {
        apply_nft()?;
        return Ok(FirewallStatus {
            applied: true,
            backend: "nft".into(),
            detail: None,
        });
    }
    if ufw_available() {
        if !ufw_is_active() {
            return Err(
                "ufw is installed but inactive — run `sudo ufw enable`, or install nftables \
                 (`sudo apt install nftables`)"
                    .into(),
            );
        }
        apply_ufw()?;
        return Ok(FirewallStatus {
            applied: true,
            backend: "ufw".into(),
            detail: None,
        });
    }
    Err(
        "Neither nftables (`nft`) nor an active ufw is available — install nftables \
         (recommended) or enable ufw to enforce LAN-only RDP"
            .into(),
    )
}

/// Privileged clear entry.
pub fn clear_as_root() -> Result<FirewallStatus, String> {
    if !is_root() {
        return Err("firewall clear-as-root requires root".into());
    }
    let mut cleared = false;
    // Always attempt clear when invoked as root — live probe may fail mid-clear.
    if nft_available() {
        if nft_table_present() {
            clear_nft()?;
            cleared = true;
        } else {
            // Table may exist but be unlistable inconsistently — try delete anyway.
            let _ = clear_nft();
        }
    }
    if ufw_available() && ufw_rules_present() {
        clear_ufw()?;
        cleared = true;
    }
    Ok(FirewallStatus {
        applied: false,
        backend: String::new(),
        detail: if cleared {
            None
        } else {
            Some("No Metis RDP firewall rules were present".into())
        },
    })
}

fn escalate(args: &[&str]) -> Result<(), String> {
    let output = crate::pkhelpers::run_pkexec(args, std::time::Duration::from_secs(45))?;
    if output.status.success() {
        return Ok(());
    }
    Err(crate::pkhelpers::pkexec_failure_message(
        &output,
        " for firewall changes",
    ))
}

fn apply_nft() -> Result<(), String> {
    let nft = nft_bin().ok_or_else(|| "nft not found".to_string())?;
    // Replace any previous table so re-apply is idempotent.
    let _ = run(&nft, &["delete", "table", "inet", NFT_TABLE]);

    let mut script = String::from("table inet metis_rdp {\n");
    script.push_str("  chain input {\n");
    script.push_str("    type filter hook input priority filter; policy accept;\n");
    for cidr in LAN_V4 {
        script.push_str(&format!("    tcp dport {PORT} ip saddr {cidr} accept\n"));
    }
    for cidr in LAN_V6 {
        script.push_str(&format!("    tcp dport {PORT} ip6 saddr {cidr} accept\n"));
    }
    script.push_str(&format!("    tcp dport {PORT} drop\n"));
    script.push_str("  }\n}\n");

    let status = Command::new(&nft)
        .arg("-f")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(script.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|e| format!("nft apply: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nft apply failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ))
    }
}

fn clear_nft() -> Result<(), String> {
    let nft = nft_bin().ok_or_else(|| "nft not found".to_string())?;
    let out = run(&nft, &["delete", "table", "inet", NFT_TABLE])?;
    if out.status.success()
        || String::from_utf8_lossy(&out.stderr).contains("No such file")
        || String::from_utf8_lossy(&out.stderr).contains("does not exist")
    {
        Ok(())
    } else {
        Err(format!(
            "nft clear failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn apply_ufw() -> Result<(), String> {
    let ufw = ufw_bin().ok_or_else(|| "ufw not found".to_string())?;
    let _ = clear_ufw();
    for cidr in LAN_V4 {
        let out = run(
            &ufw,
            &[
                "allow",
                "from",
                cidr,
                "to",
                "any",
                "port",
                &PORT.to_string(),
                "proto",
                "tcp",
                "comment",
                UFW_COMMENT,
            ],
        )?;
        if !out.status.success() {
            return Err(format!(
                "ufw allow {cidr}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    for cidr in LAN_V6 {
        let out = run(
            &ufw,
            &[
                "allow",
                "from",
                cidr,
                "to",
                "any",
                "port",
                &PORT.to_string(),
                "proto",
                "tcp",
                "comment",
                UFW_COMMENT,
            ],
        )?;
        if !out.status.success() {
            return Err(format!(
                "ufw allow {cidr}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    let out = run(
        &ufw,
        &[
            "deny",
            "in",
            "to",
            "any",
            "port",
            &PORT.to_string(),
            "proto",
            "tcp",
            "comment",
            UFW_COMMENT,
        ],
    )?;
    if !out.status.success() {
        return Err(format!(
            "ufw deny 3389: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn clear_ufw() -> Result<(), String> {
    let Some(ufw) = ufw_bin() else {
        return Ok(());
    };
    for _ in 0..32 {
        let Ok(output) = run(&ufw, &["status", "numbered"]) else {
            break;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(num) = text.lines().find_map(|line| {
            if !line.contains(UFW_COMMENT) {
                return None;
            }
            let trimmed = line.trim_start();
            let start = trimmed.strip_prefix('[')?;
            let (num, _) = start.split_once(']')?;
            Some(num.trim().to_string())
        }) else {
            break;
        };
        let mut child = Command::new(&ufw)
            .args(["delete", &num])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("ufw delete spawn: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"y\n");
        }
        let _ = child.wait();
    }
    Ok(())
}

// --- RustDesk ports (Wave 4a) ------------------------------------------------

const NFT_TABLE_RD: &str = "metis_rustdesk";
const UFW_COMMENT_RD: &str = "metis-rustdesk-lan-only";
/// TCP 21115–21119; UDP 21116 (relay / hole-punch).
const RD_TCP_PORTS: &str = "21115-21119";
const RD_UDP_PORT: u16 = 21116;

fn persist_rustdesk(applied: bool, backend: &str) {
    let mut cfg = load_remote_config();
    cfg.rustdesk_firewall_applied = applied;
    cfg.rustdesk_firewall_backend = if applied {
        backend.to_string()
    } else {
        String::new()
    };
    if applied {
        cfg.rustdesk_firewall_last_error = None;
    }
    if let Err(err) = save_remote_config(&cfg) {
        tracing::warn!(%err, "failed to persist rustdesk_firewall_applied");
    }
}

fn persist_rustdesk_error(err: &str) {
    let mut cfg = load_remote_config();
    cfg.rustdesk_firewall_applied = false;
    cfg.rustdesk_firewall_backend.clear();
    cfg.rustdesk_firewall_last_error = Some(err.to_string());
    if let Err(e) = save_remote_config(&cfg) {
        tracing::warn!(%e, "failed to persist rustdesk_firewall_last_error");
    }
}

/// Status of RustDesk LAN-only firewall rules.
pub fn status_rustdesk() -> FirewallStatus {
    if nft_table_exists(NFT_TABLE_RD) {
        return FirewallStatus {
            applied: true,
            backend: "nft".into(),
            detail: None,
        };
    }
    if ufw_has_comment(UFW_COMMENT_RD) {
        return FirewallStatus {
            applied: true,
            backend: "ufw".into(),
            detail: None,
        };
    }
    let cfg = load_remote_config();
    if cfg.rustdesk_firewall_applied {
        return FirewallStatus {
            applied: true,
            backend: if cfg.rustdesk_firewall_backend.is_empty() {
                preferred_backend_label()
            } else {
                cfg.rustdesk_firewall_backend
            },
            detail: None,
        };
    }
    FirewallStatus {
        applied: false,
        backend: String::new(),
        detail: cfg.rustdesk_firewall_last_error,
    }
}

fn nft_table_exists(table: &str) -> bool {
    let Some(nft) = nft_bin() else {
        return false;
    };
    run(&nft, &["list", "table", "inet", table])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ufw_has_comment(comment: &str) -> bool {
    let Some(ufw) = ufw_bin() else {
        return false;
    };
    run(&ufw, &["status"])
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(comment))
        .unwrap_or(false)
}

/// Apply LAN-only rules for RustDesk ports.
pub fn apply_rustdesk() -> Result<FirewallStatus, String> {
    let current = status_rustdesk();
    if current.applied {
        return Ok(current);
    }
    {
        let mut cfg = load_remote_config();
        if cfg.rustdesk_firewall_last_error.take().is_some() {
            let _ = save_remote_config(&cfg);
        }
    }
    let backend = enforceable_backend().inspect_err(|e| persist_rustdesk_error(e))?;
    let result = if is_root() {
        apply_rustdesk_as_root()?
    } else {
        escalate(&["firewall", "rustdesk-apply-as-root"]).map_err(|err| {
            let msg = err.to_string();
            persist_rustdesk_error(&msg);
            msg
        })?;
        FirewallStatus {
            applied: true,
            backend: backend.to_string(),
            detail: None,
        }
    };
    if result.applied {
        persist_rustdesk(true, &result.backend);
    }
    Ok(result)
}

/// Clear RustDesk LAN-only rules.
pub fn clear_rustdesk() -> Result<FirewallStatus, String> {
    let current = status_rustdesk();
    if !current.applied {
        persist_rustdesk(false, "");
        return Ok(FirewallStatus {
            applied: false,
            backend: String::new(),
            detail: current.detail,
        });
    }
    if is_root() {
        clear_rustdesk_as_root()?;
    } else {
        escalate(&["firewall", "rustdesk-clear-as-root"])?;
    }
    persist_rustdesk(false, "");
    Ok(FirewallStatus {
        applied: false,
        backend: String::new(),
        detail: None,
    })
}

pub fn apply_rustdesk_as_root() -> Result<FirewallStatus, String> {
    if !is_root() {
        return Err("firewall rustdesk-apply-as-root requires root".into());
    }
    let backend = enforceable_backend()?;
    match backend {
        "nft" => apply_nft_rustdesk()?,
        "ufw" => apply_ufw_rustdesk()?,
        other => return Err(format!("unsupported firewall backend: {other}")),
    }
    persist_rustdesk(true, backend);
    Ok(FirewallStatus {
        applied: true,
        backend: backend.to_string(),
        detail: None,
    })
}

pub fn clear_rustdesk_as_root() -> Result<FirewallStatus, String> {
    if !is_root() {
        return Err("firewall rustdesk-clear-as-root requires root".into());
    }
    let _ = clear_nft_rustdesk();
    let _ = clear_ufw_rustdesk();
    persist_rustdesk(false, "");
    Ok(FirewallStatus {
        applied: false,
        backend: String::new(),
        detail: None,
    })
}

fn apply_nft_rustdesk() -> Result<(), String> {
    let nft = nft_bin().ok_or_else(|| "nft not found".to_string())?;
    let _ = run(&nft, &["delete", "table", "inet", NFT_TABLE_RD]);
    let mut script = String::from("table inet metis_rustdesk {\n");
    script.push_str("  chain input {\n");
    script.push_str("    type filter hook input priority filter; policy accept;\n");
    for cidr in LAN_V4 {
        script.push_str(&format!(
            "    tcp dport {{ {RD_TCP_PORTS} }} ip saddr {cidr} accept\n"
        ));
        script.push_str(&format!(
            "    udp dport {RD_UDP_PORT} ip saddr {cidr} accept\n"
        ));
    }
    for cidr in LAN_V6 {
        script.push_str(&format!(
            "    tcp dport {{ {RD_TCP_PORTS} }} ip6 saddr {cidr} accept\n"
        ));
        script.push_str(&format!(
            "    udp dport {RD_UDP_PORT} ip6 saddr {cidr} accept\n"
        ));
    }
    script.push_str(&format!("    tcp dport {{ {RD_TCP_PORTS} }} drop\n"));
    script.push_str(&format!("    udp dport {RD_UDP_PORT} drop\n"));
    script.push_str("  }\n}\n");
    let status = Command::new(&nft)
        .arg("-f")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(script.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|e| format!("nft rustdesk apply: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nft rustdesk apply failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ))
    }
}

fn clear_nft_rustdesk() -> Result<(), String> {
    let nft = nft_bin().ok_or_else(|| "nft not found".to_string())?;
    let out = run(&nft, &["delete", "table", "inet", NFT_TABLE_RD])?;
    if out.status.success()
        || String::from_utf8_lossy(&out.stderr).contains("No such file")
        || String::from_utf8_lossy(&out.stderr).contains("does not exist")
    {
        Ok(())
    } else {
        Err(format!(
            "nft rustdesk clear failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn apply_ufw_rustdesk() -> Result<(), String> {
    let ufw = ufw_bin().ok_or_else(|| "ufw not found".to_string())?;
    let _ = clear_ufw_rustdesk();
    for cidr in LAN_V4.iter().chain(LAN_V6.iter()) {
        for proto_port in [("tcp", RD_TCP_PORTS), ("udp", "21116")] {
            let out = run(
                &ufw,
                &[
                    "allow",
                    "from",
                    cidr,
                    "to",
                    "any",
                    "port",
                    proto_port.1,
                    "proto",
                    proto_port.0,
                    "comment",
                    UFW_COMMENT_RD,
                ],
            )?;
            if !out.status.success() {
                return Err(format!(
                    "ufw rustdesk allow {cidr}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
    }
    Ok(())
}

fn clear_ufw_rustdesk() -> Result<(), String> {
    let Some(ufw) = ufw_bin() else {
        return Ok(());
    };
    for _ in 0..64 {
        let Ok(output) = run(&ufw, &["status", "numbered"]) else {
            break;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(num) = text.lines().find_map(|line| {
            if !line.contains(UFW_COMMENT_RD) {
                return None;
            }
            let trimmed = line.trim_start();
            let start = trimmed.strip_prefix('[')?;
            let (num, _) = start.split_once(']')?;
            Some(num.trim().to_string())
        }) else {
            break;
        };
        let mut child = Command::new(&ufw)
            .args(["delete", &num])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("ufw rustdesk delete spawn: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"y\n");
        }
        let _ = child.wait();
    }
    Ok(())
}
