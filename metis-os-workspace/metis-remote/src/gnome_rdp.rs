//! gnome-remote-desktop session-sharing RDP via grdctl + systemd user unit.

use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use crate::{RemoteStatus, host};

const GRDCTL: &str = "grdctl";
const SYSTEMD_UNIT: &str = "gnome-remote-desktop.service";
const HEADLESS_UNIT: &str = "gnome-remote-desktop-headless.service";
const DEFAULT_PORT: u16 = 3389;

fn bin_on_path(name: &str) -> bool {
    for dir in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
        if std::path::Path::new(&format!("{dir}/{name}")).is_file() {
            return true;
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join(name).is_file() {
                return true;
            }
        }
    }
    false
}

pub fn grdctl_available() -> bool {
    bin_on_path("grdctl")
}

pub fn mutter_apis_available() -> bool {
    let Ok(output) = Command::new("busctl").args(["--user", "list"]).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.contains("org.gnome.Mutter.RemoteDesktop") && text.contains("org.gnome.Mutter.ScreenCast")
}

fn grdctl(args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new(GRDCTL);
    cmd.args(args);
    cmd.output()
        .map_err(|e| format!("failed to run grdctl: {e}"))
}

fn systemctl(args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new("systemctl");
    cmd.arg("--user");
    cmd.args(args);
    cmd.output()
        .map_err(|e| format!("failed to run systemctl --user: {e}"))
}

fn run_ok(output: Output, ctx: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        stderr.trim().to_string()
    };
    if detail.is_empty() {
        Err(format!("{ctx} failed"))
    } else {
        Err(format!("{ctx}: {detail}"))
    }
}

fn wait_for_daemon(max_wait: Duration) {
    let step = Duration::from_millis(200);
    let mut elapsed = Duration::ZERO;
    while elapsed < max_wait {
        if daemon_active() {
            return;
        }
        thread::sleep(step);
        elapsed += step;
    }
}

fn daemon_active() -> bool {
    systemctl(&["is-active", "--quiet", SYSTEMD_UNIT])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rdp_port_listening(port: u16) -> bool {
    let output = Command::new("ss").args(["-tln"]).output().ok();
    let Some(output) = output else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let needle = format!(":{port} ");
    text.lines().any(|line| line.contains(&needle))
}

pub fn status_snapshot() -> RemoteStatus {
    let available = grdctl_available();
    if !available {
        return RemoteStatus {
            available: false,
            running: false,
            rdp_enabled: false,
            port: DEFAULT_PORT,
            password_set: false,
            username: None,
            hostname: host::hostname(),
            addresses: host::lan_addresses(),
            backend: "gnome-rdp".into(),
            config_enabled: false,
            lan_only: true,
            firewall_applied: false,
            firewall_backend: String::new(),
            firewall_detail: None,
            error: Some(
                "Install gnome-remote-desktop (Ubuntu: sudo apt install gnome-remote-desktop)"
                    .into(),
            ),
        };
    }

    let running = daemon_active();
    let text = grdctl(&["status"])
        .ok()
        .filter(|o| o.status.success() || !o.stdout.is_empty())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();

    let parsed = parse_grdctl_status(&text);
    let mut error = parsed.error;
    if !mutter_apis_available() {
        error = Some(
            "Mutter capture APIs unavailable — restart the Metis session so metis-portal can register them"
                .into(),
        );
    }

    RemoteStatus {
        available: true,
        running: running || parsed.rdp_enabled,
        rdp_enabled: parsed.rdp_enabled,
        port: parsed.port,
        password_set: parsed.password_set,
        username: parsed.username,
        hostname: host::hostname(),
        addresses: host::lan_addresses(),
        backend: "gnome-rdp".into(),
        config_enabled: false,
        lan_only: true,
        firewall_applied: false,
        firewall_backend: String::new(),
        firewall_detail: None,
        error,
    }
}

struct ParsedStatus {
    rdp_enabled: bool,
    port: u16,
    password_set: bool,
    username: Option<String>,
    error: Option<String>,
}

fn parse_grdctl_status(text: &str) -> ParsedStatus {
    let mut in_rdp = false;
    let mut rdp_enabled = false;
    let mut port = DEFAULT_PORT;
    let mut username: Option<String> = None;
    let mut password_set = false;
    let mut error: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("RDP:") {
            in_rdp = true;
            continue;
        }
        if in_rdp && line.contains(':') {
            let Some((key, val)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim();
            match key {
                "Status" => {
                    rdp_enabled = val.eq_ignore_ascii_case("enabled");
                }
                "Port" => {
                    if let Ok(p) = val.parse::<u16>() {
                        port = p;
                    }
                }
                "Username" => {
                    if !val.is_empty()
                        && !val.eq_ignore_ascii_case("(empty)")
                        && !val.eq_ignore_ascii_case("(hidden)")
                    {
                        username = Some(val.to_string());
                    }
                }
                "Password" => {
                    password_set = (!val.is_empty() && !val.eq_ignore_ascii_case("(empty)"))
                        || val.eq_ignore_ascii_case("(hidden)");
                }
                "TLS certificate" if val.is_empty() => {
                    error = Some(
                        "RDP TLS certificate is missing — disable and re-enable session sharing"
                            .into(),
                    );
                }
                "TLS fingerprint" if val.eq_ignore_ascii_case("(null)") && error.is_none() => {
                    error = Some(
                        "RDP TLS certificate is not configured — disable and re-enable sharing"
                            .into(),
                    );
                }
                _ => {}
            }
        }
    }

    if text.contains("certificate is invalid") {
        error =
            Some("RDP TLS certificate is not configured — toggle sharing off and on again".into());
    }

    ParsedStatus {
        rdp_enabled,
        port,
        password_set,
        username,
        error,
    }
}

fn grdctl_user_active() -> bool {
    Command::new("busctl")
        .args(["--user", "list"])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains("org.gnome.RemoteDesktop.User")
        })
        .unwrap_or(false)
}

pub fn enable_sharing() -> Result<(), String> {
    if !mutter_apis_available() {
        return Err(
            "Desktop capture is unavailable — log out and back in to Metis, then try again".into(),
        );
    }
    ensure_tls_cert()?;
    // Session sharing must use gnome-remote-desktop.service, not the headless unit.
    let _ = systemctl(&["stop", HEADLESS_UNIT]);
    let _ = systemctl(&["disable", "--now", HEADLESS_UNIT]);
    let _ = grdctl(&["rdp", "disable"]);
    run_ok(
        systemctl(&["enable", "--now", SYSTEMD_UNIT])?,
        "start gnome-remote-desktop",
    )?;
    wait_for_daemon(Duration::from_secs(5));
    if !wait_for_grd_user_bus(Duration::from_secs(8)) {
        return Err(
            "gnome-remote-desktop did not start — log out and back in to Metis, then try again"
                .into(),
        );
    }
    run_ok(
        grdctl(&["rdp", "set-port", &DEFAULT_PORT.to_string()])?,
        "set RDP port",
    )?;
    run_ok(
        grdctl(&["rdp", "disable-view-only"])?,
        "enable remote control",
    )?;
    run_ok(grdctl(&["rdp", "enable"])?, "enable RDP")?;
    if !rdp_port_listening(DEFAULT_PORT) {
        run_ok(
            systemctl(&["restart", SYSTEMD_UNIT])?,
            "restart gnome-remote-desktop",
        )?;
        wait_for_daemon(Duration::from_secs(5));
        let _ = wait_for_grd_user_bus(Duration::from_secs(8));
    }
    let _ = systemctl(&["disable", "--now", HEADLESS_UNIT]);
    if !wait_for_rdp_port(DEFAULT_PORT, Duration::from_secs(15)) {
        return Err(
            "RDP did not start on port 3389 — ensure metis-portal is running (log out and back in to Metis)"
                .into(),
        );
    }
    Ok(())
}

fn wait_for_rdp_port(port: u16, max_wait: Duration) -> bool {
    let step = Duration::from_millis(300);
    let mut elapsed = Duration::ZERO;
    while elapsed < max_wait {
        if rdp_port_listening(port) {
            return true;
        }
        thread::sleep(step);
        elapsed += step;
    }
    false
}

fn wait_for_grd_user_bus(max_wait: Duration) -> bool {
    let step = Duration::from_millis(200);
    let mut elapsed = Duration::ZERO;
    while elapsed < max_wait {
        if grdctl_user_active() {
            return true;
        }
        thread::sleep(step);
        elapsed += step;
    }
    false
}

pub fn disable_sharing() -> Result<(), String> {
    let _ = grdctl(&["rdp", "disable"]);
    let _ = systemctl(&["stop", SYSTEMD_UNIT]);
    let _ = systemctl(&["stop", HEADLESS_UNIT]);
    let _ = systemctl(&["disable", "--now", HEADLESS_UNIT]);
    Ok(())
}

/// Stop accepting RDP connections without clearing `remote.json` or credentials.
pub fn pause_sharing() -> Result<(), String> {
    let _ = grdctl(&["rdp", "disable"]);
    Ok(())
}

/// Re-enable RDP after [`pause_sharing`] (daemon must already be running).
pub fn resume_sharing() -> Result<(), String> {
    if !mutter_apis_available() {
        return Err(
            "Desktop capture is unavailable — unlock and ensure metis-portal is running".into(),
        );
    }
    ensure_tls_cert()?;
    if !daemon_active() {
        run_ok(
            systemctl(&["enable", "--now", SYSTEMD_UNIT])?,
            "start gnome-remote-desktop",
        )?;
        wait_for_daemon(Duration::from_secs(5));
        let _ = wait_for_grd_user_bus(Duration::from_secs(8));
    }
    run_ok(
        grdctl(&["rdp", "set-port", &DEFAULT_PORT.to_string()])?,
        "set RDP port",
    )?;
    run_ok(
        grdctl(&["rdp", "disable-view-only"])?,
        "enable remote control",
    )?;
    run_ok(grdctl(&["rdp", "enable"])?, "enable RDP")?;
    let _ = wait_for_rdp_port(DEFAULT_PORT, Duration::from_secs(10));
    Ok(())
}

pub fn set_credentials(username: &str, password: &str) -> Result<(), String> {
    if !grdctl_available() {
        return Err("grdctl not found — install gnome-remote-desktop".into());
    }
    ensure_tls_cert()?;
    if !daemon_active() {
        run_ok(
            systemctl(&["start", SYSTEMD_UNIT])?,
            "start gnome-remote-desktop",
        )?;
        wait_for_daemon(Duration::from_secs(5));
    }
    run_ok(
        grdctl(&["rdp", "set-credentials", username, password])?,
        "set RDP credentials",
    )
}

fn tls_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".local/share/gnome-remote-desktop")
}

fn ensure_tls_cert() -> Result<(), String> {
    let dir = tls_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create TLS dir: {e}"))?;
    let cert = dir.join("tls.crt");
    let key = dir.join("tls.key");

    if !cert.is_file() || !key.is_file() {
        generate_tls_cert(&cert, &key)?;
    }

    let cert_s = cert.to_string_lossy();
    let key_s = key.to_string_lossy();
    run_ok(
        grdctl(&["rdp", "set-tls-cert", &cert_s])?,
        "set RDP TLS certificate",
    )?;
    run_ok(grdctl(&["rdp", "set-tls-key", &key_s])?, "set RDP TLS key")?;
    Ok(())
}

fn generate_tls_cert(cert: &std::path::Path, key: &std::path::Path) -> Result<(), String> {
    if bin_on_path("winpr-makecert") {
        let dir = cert.parent().ok_or("cert path has no parent")?;
        let status = Command::new("winpr-makecert")
            .args(["-silent", "-rdp", "-path", &dir.to_string_lossy(), "tls"])
            .status()
            .map_err(|e| format!("winpr-makecert failed: {e}"))?;
        if status.success() && cert.is_file() && key.is_file() {
            return Ok(());
        }
    }

    if !bin_on_path("openssl") {
        return Err(
            "openssl not found — install openssl to generate the RDP TLS certificate".into(),
        );
    }

    let host = host::hostname();
    let primary_ip = host::lan_addresses().into_iter().next();
    let mut san = format!("DNS:{host},DNS:localhost,IP:127.0.0.1");
    if let Some(ip) = primary_ip {
        san.push_str(&format!(",IP:{ip}"));
    }

    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:4096",
            "-sha256",
            "-days",
            "3650",
            "-nodes",
            "-keyout",
            &key.to_string_lossy(),
            "-out",
            &cert.to_string_lossy(),
            "-subj",
            &format!("/CN={host}"),
            "-addext",
            &format!("subjectAltName={san}"),
        ])
        .status()
        .map_err(|e| format!("openssl failed: {e}"))?;

    if !status.success() {
        return Err("openssl could not generate RDP TLS certificate".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enabled_status() {
        let text = r#"RDP:
	Status: enabled
	Port: 3389
	Username: drose
	Password: ***
"#;
        let p = parse_grdctl_status(text);
        assert!(p.rdp_enabled);
        assert_eq!(p.port, 3389);
        assert!(p.password_set);
        assert_eq!(p.username.as_deref(), Some("drose"));
    }

    #[test]
    fn parse_disabled_empty_creds() {
        let text = r#"RDP:
	Status: disabled
	Port: 3389
	Username: (empty)
	Password: (empty)
"#;
        let p = parse_grdctl_status(text);
        assert!(!p.rdp_enabled);
        assert!(!p.password_set);
    }

    #[test]
    fn parse_hidden_credentials() {
        let text = r#"RDP:
	Status: enabled
	Port: 3389
	Username: (hidden)
	Password: (hidden)
	TLS certificate: /path/to/cert
	TLS fingerprint: ab:cd
"#;
        let p = parse_grdctl_status(text);
        assert!(p.rdp_enabled);
        assert!(p.password_set);
        assert!(p.username.is_none());
    }

    #[test]
    fn parse_missing_tls() {
        let text = r#"RDP:
	Status: enabled
	Port: 3389
	Username: (hidden)
	Password: (hidden)
	TLS certificate: 
	TLS fingerprint: (null)
"#;
        let p = parse_grdctl_status(text);
        assert!(p.error.is_some());
    }
}
