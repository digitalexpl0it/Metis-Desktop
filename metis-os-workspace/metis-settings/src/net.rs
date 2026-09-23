//! Thin `nmcli` wrappers for the Network settings page. Reads are blocking and
//! meant to run on a worker thread; mutations are fire-and-forget on a detached
//! thread (NetworkManager applies them and the next refresh reflects the result).

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// One-shot gate: `nmcli radio wifi off` is refused unless Settings armed it
/// from a real pointer press on the Wi-Fi switch (HDMI modeset must never kill Wi-Fi).
static WIFI_USER_OFF_ARMED: AtomicBool = AtomicBool::new(false);

/// Set by [`set_radio`] when an armed off is approved; consumed in [`detached`].
static WIFI_OFF_SPAWN_ALLOWED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq)]
pub struct WifiNet {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedConn {
    pub name: String,
    pub uuid: String,
    pub ctype: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EthDev {
    pub device: String,
    pub connection: Option<String>,
    pub connected: bool,
    pub ipv4: Ipv4,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ipv4 {
    pub method: String,
    pub addresses: String,
    pub gateway: String,
    pub dns: String,
    /// When true (NM `ipv4.ignore-auto-dns`), DHCP-provided DNS is ignored and
    /// only the manual `dns` list is used — i.e. a DNS override on an otherwise
    /// automatic connection.
    pub ignore_auto_dns: bool,
}

/// An active connection profile (e.g. the currently-joined Wi-Fi), with its
/// editable IPv4 config so the UI can offer a DNS override.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveConn {
    pub name: String,
    pub uuid: String,
    pub ipv4: Ipv4,
}

/// System proxy configuration, mirrored to/from GNOME's `org.gnome.system.proxy`
/// gsettings (honoured by GLib/GTK apps via the default `GProxyResolver`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxyConfig {
    /// "none", "manual", or "auto".
    pub mode: String,
    pub auto_url: String,
    pub http_host: String,
    pub http_port: u32,
    pub https_host: String,
    pub https_port: u32,
    pub socks_host: String,
    pub socks_port: u32,
    /// Comma-separated no-proxy host list.
    pub ignore_hosts: String,
    /// True when the gsettings proxy schema is available on this system.
    pub available: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetSnapshot {
    pub wifi_enabled: bool,
    pub wifi: Vec<WifiNet>,
    pub saved: Vec<SavedConn>,
    pub eth: Vec<EthDev>,
    /// The active Wi-Fi connection profile, if any (for the DNS override UI).
    pub active_wifi: Option<ActiveConn>,
    pub proxy: ProxyConfig,
    /// NetworkManager VPN / WireGuard profiles.
    pub vpn: Vec<VpnConn>,
}

/// Gather a full network snapshot (blocking — call on a worker thread).
pub fn load_snapshot() -> NetSnapshot {
    NetSnapshot {
        wifi_enabled: wifi_radio_enabled(),
        wifi: wifi_networks(),
        saved: saved_connections(),
        eth: ethernet_devices(),
        active_wifi: active_wifi_connection(),
        proxy: read_proxy(),
        vpn: list_vpn_connections(),
    }
}

pub fn wifi_radio_enabled() -> bool {
    capture(&["-t", "-f", "WIFI", "radio"], Duration::from_secs(3))
        .map(|s| {
            // nmcli may print trailing junk; treat any line that is exactly
            // "enabled" as on. Unknown/empty → leave as on (don't kill Wi‑Fi).
            s.lines()
                .map(str::trim)
                .any(|line| line.eq_ignore_ascii_case("enabled"))
                || s.trim().is_empty()
        })
        .unwrap_or(true)
}

fn wifi_networks() -> Vec<WifiNet> {
    let Some(text) = capture(
        &["-t", "-f", "ACTIVE,SSID,SIGNAL,SECURITY", "dev", "wifi"],
        Duration::from_secs(6),
    ) else {
        return Vec::new();
    };
    let mut nets: Vec<WifiNet> = Vec::new();
    for line in text.lines() {
        let f = split(line);
        if f.len() < 4 {
            continue;
        }
        let ssid = f[1].clone();
        if ssid.is_empty() {
            continue;
        }
        let active = f[0] == "yes";
        let signal = f[2].parse().unwrap_or(0);
        let sec = f[3].trim();
        let secured = !sec.is_empty() && sec != "--";
        if let Some(e) = nets.iter_mut().find(|n| n.ssid == ssid) {
            e.active = e.active || active;
            if signal > e.signal {
                e.signal = signal;
                e.secured = secured;
            }
            continue;
        }
        nets.push(WifiNet {
            ssid,
            signal,
            secured,
            active,
        });
    }
    nets.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    nets
}

pub fn saved_connections() -> Vec<SavedConn> {
    let Some(text) = capture(
        &["-t", "-f", "NAME,UUID,TYPE", "connection", "show"],
        Duration::from_secs(4),
    ) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let f = split(line);
            if f.len() < 3 || f[0].is_empty() {
                return None;
            }
            let ctype = f[2].as_str();
            // Wireless "Known networks" must not list ethernet, loopback, VPN, …
            if !is_wifi_connection_type(ctype) {
                return None;
            }
            Some(SavedConn {
                name: f[0].clone(),
                uuid: f[1].clone(),
                ctype: f[2].clone(),
            })
        })
        .collect()
}

/// NetworkManager Wi-Fi profile types (`nmcli connection show` TYPE column).
fn is_wifi_connection_type(ctype: &str) -> bool {
    matches!(
        ctype,
        "802-11-wireless" | "wifi" | "wireless" | "80211-wireless"
    )
}

fn ethernet_devices() -> Vec<EthDev> {
    let Some(text) = capture(
        &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "dev", "status"],
        Duration::from_secs(4),
    ) else {
        return Vec::new();
    };
    let mut devs = Vec::new();
    for line in text.lines() {
        let f = split(line);
        if f.len() < 4 || f[1] != "ethernet" {
            continue;
        }
        let connected = f[2].starts_with("connected");
        let conn = f[3].clone();
        let connection = (!conn.is_empty() && conn != "--").then_some(conn);
        let ipv4 = connection.as_deref().map(read_ipv4).unwrap_or_default();
        devs.push(EthDev {
            device: f[0].clone(),
            connection,
            connected,
            ipv4,
        });
    }
    devs
}

/// Read the IPv4 config of a saved connection (blocking).
pub fn read_ipv4(conn: &str) -> Ipv4 {
    let Some(text) = capture(
        &[
            "-t",
            "-f",
            "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns,ipv4.ignore-auto-dns",
            "connection",
            "show",
            conn,
        ],
        Duration::from_secs(4),
    ) else {
        return Ipv4::default();
    };
    let mut out = Ipv4::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim().to_string();
        match key.trim() {
            "ipv4.method" => out.method = val,
            "ipv4.addresses" => out.addresses = val,
            "ipv4.gateway" => out.gateway = val,
            "ipv4.dns" => out.dns = val,
            "ipv4.ignore-auto-dns" => out.ignore_auto_dns = val == "yes" || val == "true",
            _ => {}
        }
    }
    out
}

/// The currently-active Wi-Fi connection profile (name + uuid + IPv4), used to
/// offer a DNS override on the joined network. `None` when no Wi-Fi is up.
pub fn active_wifi_connection() -> Option<ActiveConn> {
    let text = capture(
        &[
            "-t",
            "-f",
            "NAME,UUID,TYPE",
            "connection",
            "show",
            "--active",
        ],
        Duration::from_secs(4),
    )?;
    for line in text.lines() {
        let f = split(line);
        if f.len() < 3 {
            continue;
        }
        if f[2].contains("wireless") {
            let name = f[0].clone();
            let ipv4 = read_ipv4(&name);
            return Some(ActiveConn {
                name,
                uuid: f[1].clone(),
                ipv4,
            });
        }
    }
    None
}

pub fn connect_wifi(ssid: String, password: Option<String>) {
    if let Err(err) = metis_config::validate_ssid(&ssid) {
        eprintln!("metis-settings: refusing Wi-Fi connect: {err}");
        return;
    }
    let mut args = vec![
        "dev".to_string(),
        "wifi".to_string(),
        "connect".to_string(),
        ssid,
    ];
    if let Some(pw) = password
        && !pw.is_empty()
    {
        args.push("password".to_string());
        args.push(pw);
    }
    detached(args, Duration::from_secs(30));
}

/// Arm the next Wi-Fi radio-off from a pointer press on the Settings switch.
pub fn arm_user_wifi_radio_toggle() {
    WIFI_USER_OFF_ARMED.store(true, Ordering::SeqCst);
}

/// Returns `false` when an unarmed radio-off was refused (caller should snap the
/// switch back on).
pub fn set_radio(on: bool) -> bool {
    if !on {
        if !WIFI_USER_OFF_ARMED.swap(false, Ordering::SeqCst) {
            eprintln!(
                "metis-settings: blocked nmcli radio wifi off (not user-armed; \
                 HDMI/modeset must never disable Wi-Fi)"
            );
            return false;
        }
        WIFI_OFF_SPAWN_ALLOWED.store(true, Ordering::SeqCst);
    } else {
        WIFI_USER_OFF_ARMED.store(false, Ordering::SeqCst);
    }
    detached(
        vec![
            "radio".into(),
            "wifi".into(),
            if on { "on" } else { "off" }.into(),
        ],
        Duration::from_secs(5),
    );
    true
}

pub fn forget(target: &str) {
    detached(
        vec!["connection".into(), "delete".into(), target.into()],
        Duration::from_secs(8),
    );
}

/// Switch a connection to automatic (DHCP). `dns_override` (comma-separated) is
/// applied as a manual DNS list with `ignore-auto-dns yes` so it overrides the
/// DHCP-provided servers; pass an empty string to use the DHCP DNS.
pub fn set_ipv4_dhcp(conn: &str, dns_override: &str) {
    let conn = conn.to_string();
    let dns = dns_override.trim().to_string();
    std::thread::spawn(move || {
        let ignore = if dns.is_empty() { "no" } else { "yes" };
        let _ = run_blocking(&[
            "connection",
            "modify",
            &conn,
            "ipv4.method",
            "auto",
            "ipv4.addresses",
            "",
            "ipv4.gateway",
            "",
            "ipv4.dns",
            &dns,
            "ipv4.ignore-auto-dns",
            ignore,
        ]);
        let _ = run_blocking(&["connection", "up", &conn]);
    });
}

/// Apply just a DNS override to a connection, preserving its current IPv4 method
/// (auto or manual). Empty `dns` clears the override and re-enables DHCP DNS.
pub fn set_dns_override(conn: &str, dns: &str) {
    let conn = conn.to_string();
    let dns = dns.trim().to_string();
    std::thread::spawn(move || {
        let ignore = if dns.is_empty() { "no" } else { "yes" };
        let _ = run_blocking(&[
            "connection",
            "modify",
            &conn,
            "ipv4.dns",
            &dns,
            "ipv4.ignore-auto-dns",
            ignore,
        ]);
        let _ = run_blocking(&["connection", "up", &conn]);
    });
}

pub fn set_ipv4_static(conn: &str, address: &str, gateway: &str, dns: &str) {
    let (conn, address, gateway, dns) = (
        conn.to_string(),
        address.to_string(),
        gateway.to_string(),
        dns.to_string(),
    );
    std::thread::spawn(move || {
        let _ = run_blocking(&[
            "connection",
            "modify",
            &conn,
            "ipv4.method",
            "manual",
            "ipv4.addresses",
            &address,
            "ipv4.gateway",
            &gateway,
            "ipv4.dns",
            &dns,
        ]);
        let _ = run_blocking(&["connection", "up", &conn]);
    });
}

// ---- VPN / WireGuard (NetworkManager) ------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnKind {
    OpenVpn,
    WireGuard,
    Other,
}

impl VpnKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenVpn => "OpenVPN",
            Self::WireGuard => "WireGuard",
            Self::Other => "VPN",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VpnConn {
    pub name: String,
    pub uuid: String,
    pub kind: VpnKind,
    pub active: bool,
    pub autoconnect: bool,
}

/// Fields for a simple WireGuard profile create dialog.
#[derive(Debug, Clone)]
pub struct WireGuardCreate {
    pub name: String,
    pub private_key: String,
    pub address: String,
    pub peer_public_key: String,
    pub endpoint: String,
    pub allowed_ips: String,
    pub dns: String,
}

/// Fields for a simple password-auth OpenVPN create dialog.
/// Full provider configs should still use Import… (`.ovpn`).
#[derive(Debug, Clone)]
pub struct OpenVpnCreate {
    pub name: String,
    pub gateway: String,
    pub username: String,
    pub password: String,
    pub ca_path: String,
    pub remember_password: bool,
}

fn vpn_kind_from_type(ctype: &str) -> Option<VpnKind> {
    let t = ctype.to_ascii_lowercase();
    if t == "wireguard" {
        Some(VpnKind::WireGuard)
    } else if t == "vpn" || t.starts_with("vpn") || t.contains("openvpn") {
        // NM OpenVPN plugin usually reports TYPE=vpn; some builds use vpn-openvpn.
        Some(if t.contains("openvpn") {
            VpnKind::OpenVpn
        } else {
            VpnKind::Other
        })
    } else {
        None
    }
}

fn refine_vpn_kind(uuid: &str, kind: VpnKind) -> VpnKind {
    if kind != VpnKind::Other {
        return kind;
    }
    // Prefer service-type when TYPE is the generic "vpn".
    let Some(text) = capture(
        &[
            "-t",
            "-f",
            "connection.id,vpn.service-type",
            "connection",
            "show",
            uuid,
        ],
        Duration::from_secs(3),
    ) else {
        return kind;
    };
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "vpn.service-type" {
            let v = val.to_ascii_lowercase();
            if v.contains("openvpn") {
                return VpnKind::OpenVpn;
            }
            if v.contains("wireguard") {
                return VpnKind::WireGuard;
            }
        }
    }
    kind
}

/// All saved VPN / WireGuard profiles, with active state.
pub fn list_vpn_connections() -> Vec<VpnConn> {
    let Some(text) = capture(
        &[
            "-t",
            "-f",
            "NAME,UUID,TYPE,DEVICE,AUTOCONNECT",
            "connection",
            "show",
        ],
        Duration::from_secs(4),
    ) else {
        return Vec::new();
    };
    let active_uuids = active_vpn_uuids();
    let mut out = Vec::new();
    for line in text.lines() {
        let f = split(line);
        if f.len() < 3 || f[0].is_empty() {
            continue;
        }
        let Some(mut kind) = vpn_kind_from_type(&f[2]) else {
            continue;
        };
        kind = refine_vpn_kind(&f[1], kind);
        // Prefer `--active` membership; DEVICE is a secondary hint when NM still
        // lists an iface on a profile that is mid-transition.
        let active =
            active_uuids.contains(&f[1]) || (f.len() >= 4 && !f[3].is_empty() && f[3] != "--");
        let autoconnect = f
            .get(4)
            .map(|s| {
                let s = s.trim().to_ascii_lowercase();
                s == "yes" || s == "true" || s == "1"
            })
            .unwrap_or(false);
        out.push(VpnConn {
            name: f[0].clone(),
            uuid: f[1].clone(),
            kind,
            active,
            autoconnect,
        });
    }
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn active_vpn_uuids() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let Some(text) = capture(
        &[
            "-t",
            "-f",
            "NAME,UUID,TYPE",
            "connection",
            "show",
            "--active",
        ],
        Duration::from_secs(4),
    ) else {
        return set;
    };
    for line in text.lines() {
        let f = split(line);
        if f.len() < 3 {
            continue;
        }
        if vpn_kind_from_type(&f[2]).is_some() {
            set.insert(f[1].clone());
        }
    }
    set
}

pub fn vpn_up(target: &str) -> Result<(), String> {
    vpn_toggle("up", target, None, Duration::from_secs(45))
}

/// Bring a VPN up, supplying a password via a temporary `passwd-file`.
/// When `remember` is true, also store the secret on the NM profile so later
/// connects (and other desktops using the same NM) do not prompt again.
pub fn vpn_up_with_password(target: &str, password: &str, remember: bool) -> Result<(), String> {
    let password = password.trim();
    if password.is_empty() {
        return Err("Password is required.".into());
    }
    vpn_toggle("up", target, Some(password), Duration::from_secs(45))?;
    if remember && let Err(e) = vpn_remember_password(target, password) {
        tracing::warn!(%e, "VPN connected but could not save password to profile");
    }
    Ok(())
}

pub fn vpn_down(target: &str) -> Result<(), String> {
    vpn_toggle("down", target, None, Duration::from_secs(20))
}

/// True when an `nmcli connection up` failure means NM needs a VPN password
/// (or other secret) that no agent provided.
pub fn vpn_secret_required(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("secrets were required")
        || e.contains("secret was required")
        || e.contains("no secrets")
        || e.contains("missing secret")
        || e.contains("password is required")
        || e.contains("password required")
        || e.contains("need password")
        || e.contains("authentication required")
        || (e.contains("password") && (e.contains("required") || e.contains("provide")))
}

fn vpn_toggle(
    action: &str,
    target: &str,
    password: Option<&str>,
    timeout: Duration,
) -> Result<(), String> {
    metis_config::validate_nm_id(target)?;
    let passwd_path = if action == "up" {
        password.and_then(|pw| write_vpn_passwd_file(pw).ok())
    } else {
        None
    };

    let mut args: Vec<&str> = vec!["connection", action, target];
    let passwd_owned;
    if let Some(ref path) = passwd_path {
        passwd_owned = path.to_string_lossy().into_owned();
        args.push("passwd-file");
        args.push(&passwd_owned);
    }

    let (stdout, stderr, ok) = run_capture_both(&args, timeout);
    if let Some(path) = passwd_path {
        let _ = std::fs::remove_file(path);
    }

    if ok {
        return Ok(());
    }
    let err = if stderr.is_empty() { stdout } else { stderr };
    let trimmed = err.trim();
    // Stale UI / race: disconnecting an already-down profile is success.
    if action == "down" && vpn_already_inactive(trimmed) {
        return Ok(());
    }
    let clean = trimmed.trim_start_matches("Error:").trim();
    if clean.is_empty() {
        Err(format!("Failed to {action} VPN connection."))
    } else {
        Err(clean.to_string())
    }
}

fn write_vpn_passwd_file(password: &str) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = std::env::temp_dir().join(format!(
        "metis-vpn-passwd-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("Could not create password file: {e}"))?;
    // OpenVPN / generic NM VPN plugins expect `vpn.secrets.password`.
    // A second common key covers certificate private-key passphrases.
    writeln!(file, "vpn.secrets.password:{password}")
        .and_then(|_| writeln!(file, "vpn.secrets.cert-pass:{password}"))
        .map_err(|e| format!("Could not write password file: {e}"))?;
    Ok(path)
}

/// Persist a VPN user password on the NetworkManager profile (system-owned).
pub fn vpn_remember_password(target: &str, password: &str) -> Result<(), String> {
    let secret = format!("password={password}");
    let (_o, e1, ok1) = run_capture_both(
        &["connection", "modify", target, "vpn.secrets", &secret],
        Duration::from_secs(8),
    );
    if !ok1 {
        // Older / alternate property form.
        let (_o, e2, ok2) = run_capture_both(
            &[
                "connection",
                "modify",
                target,
                "+vpn.secrets",
                &format!("password:{password}"),
            ],
            Duration::from_secs(8),
        );
        if !ok2 {
            let err = if !e1.trim().is_empty() {
                e1
            } else if !e2.trim().is_empty() {
                e2
            } else {
                "Could not save VPN password.".into()
            };
            return Err(err.trim_start_matches("Error:").trim().to_string());
        }
    }
    // Prefer storing in the connection instead of agent-only.
    let _ = run_capture_both(
        &[
            "connection",
            "modify",
            target,
            "+vpn.data",
            "password-flags=0",
        ],
        Duration::from_secs(5),
    );
    Ok(())
}

fn vpn_already_inactive(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("not an active connection")
        || e.contains("no active connection")
        || e.contains("not active")
}

/// Enable or disable NetworkManager autoconnect for a VPN profile.
///
/// Autoconnect is exclusive among Metis VPN profiles: turning it on for one
/// clears it on every other VPN / WireGuard connection. NM often will not bring
/// WireGuard up by itself at login (endpoint not reachable until Wi‑Fi is up),
/// so the shell also activates the chosen profile after the session starts.
pub fn vpn_set_autoconnect(uuid: &str, on: bool) -> Result<(), String> {
    if on {
        for other in list_vpn_connections() {
            if other.uuid == uuid || !other.autoconnect {
                continue;
            }
            let (_o, e, ok) = run_capture_both(
                &[
                    "connection",
                    "modify",
                    &other.uuid,
                    "connection.autoconnect",
                    "no",
                ],
                Duration::from_secs(8),
            );
            if !ok && !e.trim().is_empty() {
                tracing::warn!(uuid = %other.uuid, err = %e, "failed to clear VPN autoconnect");
            }
        }
        let (_o, e, ok) = run_capture_both(
            &[
                "connection",
                "modify",
                uuid,
                "connection.autoconnect",
                "yes",
                "connection.autoconnect-priority",
                "50",
            ],
            Duration::from_secs(8),
        );
        if !ok {
            return Err(if e.trim().is_empty() {
                "Could not enable autoconnect.".into()
            } else {
                e
            });
        }
        Ok(())
    } else {
        let (_o, e, ok) = run_capture_both(
            &[
                "connection",
                "modify",
                uuid,
                "connection.autoconnect",
                "no",
                "connection.autoconnect-priority",
                "0",
            ],
            Duration::from_secs(8),
        );
        if ok {
            Ok(())
        } else if e.trim().is_empty() {
            Err("Could not update autoconnect.".into())
        } else {
            Err(e)
        }
    }
}

pub fn vpn_delete(target: &str) {
    forget(target);
}

/// True when the NetworkManager OpenVPN plugin is installed (Debian/Ubuntu/Mint paths).
pub fn openvpn_plugin_present() -> bool {
    const CANDIDATES: &[&str] = &[
        "/usr/lib/NetworkManager/VPN/nm-openvpn-service.name",
        "/usr/lib/x86_64-linux-gnu/NetworkManager/VPN/nm-openvpn-service.name",
        "/usr/lib64/NetworkManager/VPN/nm-openvpn-service.name",
        "/usr/lib/NetworkManager/VPN/nm-openvpn-service",
        "/usr/lib/x86_64-linux-gnu/NetworkManager/VPN/nm-openvpn-service",
    ];
    CANDIDATES.iter().any(|p| std::path::Path::new(p).is_file())
}

/// Editable WireGuard fields (private key is never returned or required on edit).
#[derive(Debug, Clone, Default)]
pub struct WireGuardProfile {
    pub name: String,
    pub address: String,
    pub peer_public_key: String,
    pub endpoint: String,
    pub allowed_ips: String,
    pub dns: String,
}

/// Read a WireGuard profile for the edit dialog.
///
/// Peer details are not exposed by `nmcli -f wireguard.peers` (modify-only on
/// many NM builds), and `nmcli connection export` rejects native WireGuard
/// ("not VPN"). We read peers from NetworkManager's D-Bus `GetSettings`.
pub fn vpn_get_wireguard(uuid: &str) -> Option<WireGuardProfile> {
    let text = capture(
        &[
            "-t",
            "-f",
            "connection.id,ipv4.addresses,ipv4.dns",
            "connection",
            "show",
            uuid,
        ],
        Duration::from_secs(4),
    )?;
    let mut profile = WireGuardProfile::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "connection.id" => profile.name = val.to_string(),
            "ipv4.addresses" => {
                profile.address = val.split(',').next().unwrap_or(val).trim().to_string();
                if profile.address == "--" {
                    profile.address.clear();
                }
            }
            "ipv4.dns" => {
                profile.dns = val
                    .split([',', '|', ' '])
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "--")
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            _ => {}
        }
    }
    if profile.name.is_empty() {
        return None;
    }
    fill_wg_peers_from_nm_dbus(uuid, &mut profile);
    // Fallback for older layouts / classic plugin exports.
    if (profile.peer_public_key.is_empty() || profile.endpoint.is_empty())
        && let Some(export) = capture(&["connection", "export", uuid], Duration::from_secs(6))
    {
        fill_wg_profile_from_export(&export, &mut profile);
    }
    Some(profile)
}

/// Load the first WireGuard peer (public key, endpoint, allowed-ips) via
/// `busctl` JSON `GetSettings`. Never reads or stores the private key.
fn fill_wg_peers_from_nm_dbus(uuid: &str, profile: &mut WireGuardProfile) {
    let Some(path) = nm_settings_path_for_uuid(uuid) else {
        return;
    };
    let Some(raw) = run_stdout(
        "busctl",
        &[
            "--system",
            "call",
            "org.freedesktop.NetworkManager",
            &path,
            "org.freedesktop.NetworkManager.Settings.Connection",
            "GetSettings",
            "--json=pretty",
        ],
        Duration::from_secs(6),
    ) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(settings) = json.get("data").and_then(|d| d.get(0)) else {
        return;
    };
    let Some(peers) = settings
        .pointer("/wireguard/peers/data")
        .and_then(|v| v.as_array())
    else {
        return;
    };
    let Some(peer) = peers.first() else {
        return;
    };
    if profile.peer_public_key.is_empty()
        && let Some(pk) = busctl_variant_str(peer.get("public-key"))
    {
        profile.peer_public_key = pk;
    }
    if profile.endpoint.is_empty()
        && let Some(ep) = busctl_variant_str(peer.get("endpoint"))
    {
        profile.endpoint = ep;
    }
    if profile.allowed_ips.is_empty()
        && let Some(ips) = busctl_variant_str_array(peer.get("allowed-ips"))
    {
        profile.allowed_ips = ips.join(", ");
    }
}

fn nm_settings_path_for_uuid(uuid: &str) -> Option<String> {
    let raw = run_stdout(
        "busctl",
        &[
            "--system",
            "call",
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager/Settings",
            "org.freedesktop.NetworkManager.Settings",
            "GetConnectionByUuid",
            "s",
            uuid,
            "--json=pretty",
        ],
        Duration::from_secs(4),
    )?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("data")
        .and_then(|d| d.get(0))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn busctl_variant_str(v: Option<&serde_json::Value>) -> Option<String> {
    v?.get("data")?.as_str().map(str::to_string)
}

fn busctl_variant_str_array(v: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let arr = v?.get("data")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
    )
}

/// Parse NetworkManager keyfile or WireGuard `.conf` export text.
/// Never stores `PrivateKey` / `private-key` values.
fn fill_wg_profile_from_export(export: &str, profile: &mut WireGuardProfile) {
    let mut in_peer = false;

    for raw in export.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = &line[1..line.len() - 1];
            let lower = name.to_ascii_lowercase();
            in_peer = lower == "peer" || lower.starts_with("wireguard-peer.");
            if lower.starts_with("wireguard-peer.")
                && let Some(orig) = name.get("wireguard-peer.".len()..)
            {
                let pk = orig.trim_end_matches('=').trim();
                if !pk.is_empty() && profile.peer_public_key.is_empty() {
                    profile.peer_public_key = pk.to_string();
                }
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase().replace('_', "-");
        let val = v.trim();
        // Never capture private material.
        if key == "private-key"
            || key == "privatekey"
            || key == "preshared-key"
            || key == "presharedkey"
        {
            continue;
        }
        if !in_peer {
            if (key == "address" || key == "address1") && profile.address.is_empty() {
                profile.address = val.split(',').next().unwrap_or(val).trim().to_string();
            }
            if key == "dns" && profile.dns.is_empty() {
                profile.dns = val
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            continue;
        }
        match key.as_str() {
            "public-key" | "publickey" | "pubkey" => {
                if profile.peer_public_key.is_empty() {
                    profile.peer_public_key = val.to_string();
                }
            }
            "endpoint" => {
                if profile.endpoint.is_empty() {
                    profile.endpoint = val.to_string();
                }
            }
            "allowed-ips" | "allowedips" if profile.allowed_ips.is_empty() => {
                profile.allowed_ips = val
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            _ => {}
        }
    }
}

/// Apply simple WireGuard edits (name, address, peer endpoint/allowed-ips/pubkey, DNS).
/// Does not change the interface private key.
pub fn vpn_update_wireguard(uuid: &str, cfg: WireGuardProfile) -> Result<(), String> {
    let name = cfg.name.trim();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    if cfg.address.trim().is_empty() {
        return Err("Interface address (CIDR) is required.".into());
    }

    let (_o, e1, ok1) = run_capture_both(
        &[
            "connection",
            "modify",
            uuid,
            "connection.id",
            name,
            "ipv4.method",
            "manual",
            "ipv4.addresses",
            cfg.address.trim(),
        ],
        Duration::from_secs(15),
    );
    if !ok1 {
        return Err(if e1.trim().is_empty() {
            "Could not update WireGuard connection.".into()
        } else {
            e1
        });
    }

    // Peer update is optional when the dialog could not load an existing pubkey
    // (older NM builds hide peers from `connection show`).
    if !cfg.peer_public_key.trim().is_empty() {
        let allowed = if cfg.allowed_ips.trim().is_empty() {
            "0.0.0.0/0, ::/0"
        } else {
            cfg.allowed_ips.trim()
        };

        let mut peer_prop = format!(
            "pubkey={};allowed-ips={};",
            cfg.peer_public_key.trim(),
            allowed
        );
        if !cfg.endpoint.trim().is_empty() {
            peer_prop.push_str(&format!("endpoint={};", cfg.endpoint.trim()));
        }
        let (_o, e2, ok2) = run_capture_both(
            &["connection", "modify", uuid, "wireguard.peers", &peer_prop],
            Duration::from_secs(15),
        );
        if !ok2 {
            return Err(if e2.trim().is_empty() {
                "Could not update WireGuard peer.".into()
            } else {
                e2
            });
        }
    }

    if cfg.dns.trim().is_empty() {
        let _ = run_blocking(&[
            "connection",
            "modify",
            uuid,
            "ipv4.dns",
            "",
            "ipv4.ignore-auto-dns",
            "no",
        ]);
    } else {
        let (_o, e3, ok3) = run_capture_both(
            &[
                "connection",
                "modify",
                uuid,
                "ipv4.dns",
                cfg.dns.trim(),
                "ipv4.ignore-auto-dns",
                "yes",
            ],
            Duration::from_secs(10),
        );
        if !ok3 && !e3.trim().is_empty() {
            return Err(e3);
        }
    }
    Ok(())
}

/// Import an OpenVPN `.ovpn` file. Returns Ok(connection name hint) or an error
/// string suitable for the UI (missing plugin, bad file, …).
pub fn vpn_import_openvpn(path: &str) -> Result<String, String> {
    vpn_import("openvpn", path, "sudo apt install network-manager-openvpn")
}

/// Import a WireGuard `.conf` file.
///
/// NetworkManager requires the file basename (minus `.conf`) to be a valid Linux
/// interface name (≤15 chars, `[a-zA-Z0-9_-]`). Provider exports like Proton's
/// `Proton-US-FREE-79.conf` fail that check — we copy to a sanitized temp name,
/// import, then set the connection title to the original stem.
pub fn vpn_import_wireguard(path: &str) -> Result<String, String> {
    use std::path::{Path, PathBuf};

    let src = Path::new(path);
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("wg")
        .to_string();
    let friendly = stem.clone();
    let iface = sanitize_wg_iface_name(&stem);

    let (import_path, temp): (PathBuf, Option<PathBuf>) =
        if wg_iface_name_ok(&stem) && src.extension().and_then(|e| e.to_str()) == Some("conf") {
            (src.to_path_buf(), None)
        } else {
            let tmp = std::env::temp_dir().join(format!("{iface}.conf"));
            std::fs::copy(src, &tmp)
                .map_err(|e| format!("Could not prepare WireGuard config for import: {e}"))?;
            (tmp.clone(), Some(tmp))
        };

    let import_s = import_path.to_string_lossy().to_string();
    let result = vpn_import(
        "wireguard",
        &import_s,
        "ensure NetworkManager WireGuard support is available (NM ≥ 1.16)",
    );
    if let Some(tmp) = temp {
        let _ = std::fs::remove_file(tmp);
    }
    let imported = result?;

    // Prefer the provider filename as the displayed connection name when it
    // differs from the short interface name NM used at import.
    if friendly != imported && friendly != iface {
        let (_o, _e, ok) = run_capture_both(
            &[
                "connection",
                "modify",
                &imported,
                "connection.id",
                &friendly,
            ],
            Duration::from_secs(8),
        );
        if ok {
            return Ok(friendly);
        }
    }
    Ok(imported)
}

/// Linux IFNAMSIZ is 16 including NUL → 15 usable chars. NM WireGuard import
/// uses the `.conf` stem as the interface name.
fn wg_iface_name_ok(stem: &str) -> bool {
    if stem.is_empty() || stem.len() > 15 {
        return false;
    }
    let mut chars = stem.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    stem.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn sanitize_wg_iface_name(stem: &str) -> String {
    let mut out: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out = out.trim_matches('-').to_string();
    if out.is_empty() {
        out = "wg".into();
    }
    if !out
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        out = format!("wg{out}");
    }
    if out.len() > 15 {
        out.truncate(15);
        out = out.trim_end_matches('-').to_string();
    }
    if out.is_empty() { "wg0".into() } else { out }
}

fn vpn_import(kind: &str, path: &str, plugin_hint: &str) -> Result<String, String> {
    let (stdout, stderr, ok) = run_capture_both(
        &["connection", "import", "type", kind, "file", path],
        Duration::from_secs(30),
    );
    if ok {
        let name = stdout
            .lines()
            .find_map(|l| {
                l.strip_prefix("Connection '")
                    .and_then(|r| r.split('\'').next())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| path.to_string());
        return Ok(name);
    }
    let err = if stderr.is_empty() { stdout } else { stderr };
    let lower = err.to_ascii_lowercase();
    if lower.contains("unknown connection type")
        || lower.contains("plugin")
        || lower.contains("no vpn plugin")
        || (lower.contains("wireguard") && lower.contains("not"))
    {
        return Err(format!(
            "Could not import {kind} profile. {plugin_hint}.\n{err}"
        ));
    }
    Err(if err.trim().is_empty() {
        format!("Could not import {kind} profile.")
    } else {
        err
    })
}

/// Create a WireGuard connection from the simple Settings dialog fields.
pub fn vpn_create_wireguard(cfg: WireGuardCreate) -> Result<(), String> {
    let name = cfg.name.trim();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    if cfg.private_key.trim().is_empty() || cfg.peer_public_key.trim().is_empty() {
        return Err("Private key and peer public key are required.".into());
    }
    if cfg.address.trim().is_empty() {
        return Err("Interface address (CIDR) is required.".into());
    }
    let allowed = if cfg.allowed_ips.trim().is_empty() {
        "0.0.0.0/0, ::/0"
    } else {
        cfg.allowed_ips.trim()
    };

    // `connection add type wireguard` creates the iface profile; peer settings
    // use the wireguard.peers / ipv4 addressing properties.
    let (stdout, stderr, ok) = run_capture_both(
        &[
            "connection",
            "add",
            "type",
            "wireguard",
            "con-name",
            name,
            "ifname",
            name,
            "autoconnect",
            "no",
            "wireguard.private-key",
            cfg.private_key.trim(),
            "ipv4.method",
            "manual",
            "ipv4.addresses",
            cfg.address.trim(),
            "ipv6.method",
            "disabled",
        ],
        Duration::from_secs(20),
    );
    if !ok {
        let err = if stderr.is_empty() { stdout } else { stderr };
        return Err(if err.trim().is_empty() {
            "Could not create WireGuard connection (is WireGuard supported by NetworkManager?)."
                .into()
        } else {
            err
        });
    }

    // NM wireguard.peers format: "pubkey=…;allowed-ips=…;endpoint=…;"
    let mut peer_prop = format!(
        "pubkey={};allowed-ips={};",
        cfg.peer_public_key.trim(),
        allowed
    );
    if !cfg.endpoint.trim().is_empty() {
        peer_prop.push_str(&format!("endpoint={};", cfg.endpoint.trim()));
    }
    let (_o, e2, ok2) = run_capture_both(
        &["connection", "modify", name, "wireguard.peers", &peer_prop],
        Duration::from_secs(15),
    );
    if !ok2 {
        let _ = run_blocking(&["connection", "delete", name]);
        return Err(if e2.trim().is_empty() {
            "Could not set WireGuard peer.".into()
        } else {
            e2
        });
    }
    if !cfg.dns.trim().is_empty() {
        let _ = run_blocking(&[
            "connection",
            "modify",
            name,
            "ipv4.dns",
            cfg.dns.trim(),
            "ipv4.ignore-auto-dns",
            "yes",
        ]);
    }
    Ok(())
}

/// Create a password-auth OpenVPN connection (NetworkManager OpenVPN plugin).
pub fn vpn_create_openvpn(cfg: OpenVpnCreate) -> Result<(), String> {
    if !openvpn_plugin_present() {
        return Err(
            "OpenVPN plugin missing. Install with: sudo apt install network-manager-openvpn".into(),
        );
    }
    let name = cfg.name.trim();
    let gateway = cfg.gateway.trim();
    let username = cfg.username.trim();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    if gateway.is_empty() {
        return Err("Gateway (server host) is required.".into());
    }
    if username.is_empty() {
        return Err("Username is required.".into());
    }

    let mut data = format!("remote={gateway},username={username},connection-type=password");
    let ca = cfg.ca_path.trim();
    if !ca.is_empty() {
        if !std::path::Path::new(ca).is_file() {
            return Err(format!("CA certificate not found: {ca}"));
        }
        data.push_str(&format!(",ca={ca}"));
    }

    let (stdout, stderr, ok) = run_capture_both(
        &[
            "connection",
            "add",
            "type",
            "vpn",
            "vpn-type",
            "openvpn",
            "con-name",
            name,
            "autoconnect",
            "no",
            "vpn.data",
            &data,
        ],
        Duration::from_secs(20),
    );
    if !ok {
        let err = if stderr.is_empty() { stdout } else { stderr };
        return Err(if err.trim().is_empty() {
            "Could not create OpenVPN connection.".into()
        } else {
            err.trim_start_matches("Error:").trim().to_string()
        });
    }

    let password = cfg.password.trim();
    if !password.is_empty() {
        let secret = format!("password={password}");
        let (_o, e, ok) = run_capture_both(
            &["connection", "modify", name, "vpn.secrets", &secret],
            Duration::from_secs(8),
        );
        if !ok && !e.trim().is_empty() {
            tracing::warn!(%e, "OpenVPN created but password could not be stored");
        }
        if cfg.remember_password {
            let _ = run_capture_both(
                &[
                    "connection",
                    "modify",
                    name,
                    "+vpn.data",
                    "password-flags=0",
                ],
                Duration::from_secs(5),
            );
        }
    }
    Ok(())
}

/// Run nmcli capturing stdout+stderr and success bit.
fn run_capture_both(args: &[&str], timeout: Duration) -> (String, String, bool) {
    use std::io::Read;

    let mut child = match Command::new("nmcli")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => return (String::new(), err.to_string(), false),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                let _ = child.wait();
                return (stdout, stderr, status.success());
            }
            Ok(None) => {}
            Err(err) => return (String::new(), err.to_string(), false),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return (String::new(), "nmcli timed out".into(), false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---- proxy (GNOME gsettings) ---------------------------------------------

const PROXY_SCHEMA: &str = "org.gnome.system.proxy";

/// Read the system proxy config from gsettings. `available` is false when the
/// schema isn't installed (then the Proxy tab shows a hint instead of controls).
pub fn read_proxy() -> ProxyConfig {
    let Some(mode) = gsettings_get(PROXY_SCHEMA, "mode") else {
        return ProxyConfig::default();
    };
    ProxyConfig {
        mode: unquote(&mode),
        auto_url: gsettings_get(PROXY_SCHEMA, "autoconfig-url")
            .map(|s| unquote(&s))
            .unwrap_or_default(),
        ignore_hosts: gsettings_get(PROXY_SCHEMA, "ignore-hosts")
            .map(|s| parse_str_array(&s))
            .unwrap_or_default(),
        http_host: proxy_host("http"),
        http_port: proxy_port("http"),
        https_host: proxy_host("https"),
        https_port: proxy_port("https"),
        socks_host: proxy_host("socks"),
        socks_port: proxy_port("socks"),
        available: true,
    }
}

fn proxy_host(kind: &str) -> String {
    gsettings_get(&format!("{PROXY_SCHEMA}.{kind}"), "host")
        .map(|s| unquote(&s))
        .unwrap_or_default()
}

fn proxy_port(kind: &str) -> u32 {
    gsettings_get(&format!("{PROXY_SCHEMA}.{kind}"), "port")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Write the proxy config to gsettings (fire-and-forget on a worker thread).
pub fn set_proxy(cfg: ProxyConfig) {
    std::thread::spawn(move || {
        let _ = gsettings_set(PROXY_SCHEMA, "mode", &cfg.mode);
        let _ = gsettings_set(PROXY_SCHEMA, "autoconfig-url", &cfg.auto_url);
        let _ = gsettings_set(
            PROXY_SCHEMA,
            "ignore-hosts",
            &to_str_array(&cfg.ignore_hosts),
        );
        for (kind, host, port) in [
            ("http", &cfg.http_host, cfg.http_port),
            ("https", &cfg.https_host, cfg.https_port),
            ("socks", &cfg.socks_host, cfg.socks_port),
        ] {
            let schema = format!("{PROXY_SCHEMA}.{kind}");
            let _ = gsettings_set(&schema, "host", host);
            let _ = gsettings_set(&schema, "port", &port.to_string());
        }
    });
}

fn gsettings_get(schema: &str, key: &str) -> Option<String> {
    let out = Command::new("gsettings")
        .args(["get", schema, key])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gsettings_set(schema: &str, key: &str, value: &str) -> Option<()> {
    let status = Command::new("gsettings")
        .args(["set", schema, key, value])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    status.success().then_some(())
}

/// Strip the surrounding single quotes gsettings wraps string values in.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('\'').to_string()
}

/// Parse a gsettings string array (`['a', 'b']`) into a comma-separated list.
fn parse_str_array(s: &str) -> String {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| p.trim().trim_matches('\'').trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a comma-separated list as a gsettings string array literal.
fn to_str_array(csv: &str) -> String {
    let items: Vec<String> = csv
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("'{p}'"))
        .collect();
    format!("[{}]", items.join(", "))
}

// ---- internals -----------------------------------------------------------

fn detached(args: Vec<String>, timeout: Duration) {
    // Belt-and-suspenders: never spawn `radio wifi off` unless set_radio just
    // approved it (arm consumed there and this flag set).
    if args.len() >= 3
        && args[0] == "radio"
        && args[1] == "wifi"
        && args[2].eq_ignore_ascii_case("off")
        && !WIFI_OFF_SPAWN_ALLOWED.swap(false, Ordering::SeqCst)
    {
        eprintln!(
            "metis-settings: blocked orphan nmcli radio wifi off \
             (HDMI/modeset must never disable Wi-Fi)"
        );
        return;
    }
    std::thread::spawn(move || {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = run_with_timeout(&refs, timeout);
    });
}

fn run_blocking(args: &[&str]) -> Option<String> {
    run_with_timeout(args, Duration::from_secs(20))
}

fn capture(args: &[&str], timeout: Duration) -> Option<String> {
    run_with_timeout(args, timeout)
}

fn run_stdout(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read;

    let mut child = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                let _ = child.wait();
                if !status.success() {
                    return None;
                }
                return Some(stdout);
            }
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn run_with_timeout(args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new("nmcli")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
            }
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Split a terse (`-t`) nmcli line into fields, honoring `\:` / `\\` escapes.
fn split(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ':' => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    fields
}
