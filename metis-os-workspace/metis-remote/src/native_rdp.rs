//! Metis-native RDP host via `metis-rdp-host` (portal ScreenCast + FreeRDP).
//!
//! Captures the Wayland session through xdg-desktop-portal / metis-portal and
//! serves RDP on TCP 3389 for Metis Viewer. GRD stays the product default.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use metis_config::{RemoteBackend, load_remote_config, save_remote_config};
use zeroize::Zeroize;

use crate::host::{hostname, lan_addresses};

const DEFAULT_PORT: u16 = 3389;
const HOST_NAMES: &[&str] = &["metis-rdp-host"];
/// File name under `$XDG_RUNTIME_DIR/metis/` read by `metis-rdp-host`.
const PASSWORD_FILE: &str = "rdp-password";
/// Sanity cap for the RDP login name and password (bytes).
const MAX_CREDENTIAL_LEN: usize = 256;
const CREDENTIALS_REQUIRED: &str = "Set RDP credentials before enabling remote desktop";

fn is_executable(path: &Path) -> bool {
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
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    let path = pid_path();
    fs::write(&path, format!("{pid}\n")).map_err(|e| format!("write {}: {e}", path.display()))?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok(())
}

fn clear_pid() {
    let _ = fs::remove_file(pid_path());
}

// ── credentials (parity with GRD `set-credentials`) ──────────────────────────

/// `$XDG_RUNTIME_DIR/metis`, created 0700. Fails closed when the runtime dir is
/// missing — never falls back to `/tmp`.
fn secure_runtime_dir() -> Result<PathBuf, String> {
    let dir = crate::pkhelpers::require_xdg_runtime_dir()?.join("metis");
    ensure_private_dir(&dir)?;
    Ok(dir)
}

fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod {}: {e}", dir.display()))
}

/// `$XDG_RUNTIME_DIR/metis` without creating it (fail closed, no `/tmp` fallback).
fn existing_runtime_dir() -> Result<PathBuf, String> {
    Ok(crate::pkhelpers::require_xdg_runtime_dir()?.join("metis"))
}

fn validate_username(username: &str) -> Result<String, String> {
    let name = username.trim();
    if name.is_empty() {
        return Err("Username must not be empty".into());
    }
    if name.len() > MAX_CREDENTIAL_LEN || name.chars().any(char::is_control) {
        return Err("Username contains invalid characters or is too long".into());
    }
    Ok(name.to_string())
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("Password must not be empty".into());
    }
    if password.len() > MAX_CREDENTIAL_LEN {
        return Err("Password is too long".into());
    }
    if password.contains(['\0', '\r', '\n']) {
        return Err("Password must not contain newlines or NUL bytes".into());
    }
    Ok(())
}

/// Atomically write `password` (plus trailing newline) to `dir/rdp-password`, mode 0600.
fn write_password_in(dir: &Path, password: &str) -> Result<(), String> {
    validate_password(password)?;
    ensure_private_dir(dir)?;
    let final_path = dir.join(PASSWORD_FILE);
    let tmp_path = dir.join(format!("{PASSWORD_FILE}.{}.tmp", std::process::id()));
    // A stale temp from a crashed run would make `create_new` fail forever.
    let _ = fs::remove_file(&tmp_path);

    let mut payload = String::with_capacity(password.len() + 1);
    payload.push_str(password);
    payload.push('\n');
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| format!("create {}: {e}", tmp_path.display()))?;
        file.write_all(payload.as_bytes())
            .map_err(|e| format!("write {}: {e}", tmp_path.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync {}: {e}", tmp_path.display()))?;
        fs::rename(&tmp_path, &final_path)
            .map_err(|e| format!("install {}: {e}", final_path.display()))
    })();
    payload.zeroize();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Read the configured password (without trailing CR/LF); `None` if missing or empty.
/// Tightens a lax file mode to 0600 (file is ours; refuse to leave it readable).
fn read_password_in(dir: &Path) -> Result<Option<String>, String> {
    let path = dir.join(PASSWORD_FILE);
    let meta = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("stat {}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!(
            "{} is not a regular file (refusing to use it)",
            path.display()
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    }
    let mut raw = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value = raw.trim_end_matches(['\r', '\n']).to_string();
    raw.zeroize();
    Ok(if value.is_empty() { None } else { Some(value) })
}

/// True when a non-empty password file exists.
fn password_file_ready() -> bool {
    existing_runtime_dir()
        .and_then(|dir| read_password_in(&dir))
        .map(|pw| {
            pw.map(|mut p| {
                p.zeroize();
            })
            .is_some()
        })
        .unwrap_or(false)
}

/// Stored native username from `remote.json`, if non-empty.
fn configured_username() -> Option<String> {
    load_remote_config()
        .native_rdp_username
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Honest credential state for Settings / enable gating.
pub(crate) fn credentials_set() -> bool {
    configured_username().is_some() && password_file_ready()
}

/// Username persisted for the native host (no password).
pub(crate) fn username() -> Option<String> {
    configured_username()
}

/// Persist the native RDP login: password → `$XDG_RUNTIME_DIR/metis/rdp-password`
/// (0600), username → `remote.json` (never the password).
pub fn set_credentials(username: &str, password: &str) -> Result<(), String> {
    let name = validate_username(username)?;
    validate_password(password)?;
    let dir = secure_runtime_dir()?;
    write_password_in(&dir, password)?;

    let mut cfg = load_remote_config();
    if cfg.native_rdp_username.as_deref() != Some(name.as_str()) {
        cfg.native_rdp_username = Some(name);
        save_remote_config(&cfg).map_err(|e| e.to_string())?;
    }

    // A running host only reads the password at launch — restart so the new
    // login takes effect instead of silently keeping the old one.
    if is_running() {
        stop_host()?;
        if cfg.enabled && matches!(cfg.backend, RemoteBackend::MetisNative) {
            start_host()?;
        }
    }
    Ok(())
}

/// Fail closed unless both username and a non-empty password file exist.
/// Returns the username to hand to the host.
fn ensure_credentials() -> Result<String, String> {
    let Some(name) = configured_username() else {
        return Err(CREDENTIALS_REQUIRED.into());
    };
    let dir = secure_runtime_dir()?;
    match read_password_in(&dir)? {
        Some(mut pw) => {
            pw.zeroize();
            Ok(name)
        }
        None => Err(format!(
            "{CREDENTIALS_REQUIRED} (no password found at {})",
            dir.join(PASSWORD_FILE).display()
        )),
    }
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
    /// Username saved and a non-empty `rdp-password` file present.
    pub password_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
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
        password_set: credentials_set(),
        username: username(),
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
    // Never spawn an open (password-less) RDP server.
    let username = ensure_credentials()?;
    if is_running() {
        return Ok(());
    }

    let child = Command::new(&bin)
        .env("METIS_PORTAL_AUTO_APPROVE", "1")
        .env("METIS_RDP_PORT", DEFAULT_PORT.to_string())
        .env("METIS_RDP_USERNAME", &username)
        // The password file is authoritative; do not inherit an override.
        .env_remove("METIS_RDP_PASSWORD")
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
    // Same gate as GRD: refuse without credentials (even if a host is already up).
    ensure_credentials()?;
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

/// Stop host; keep MetisNative selected (default backend) with sharing off.
pub fn disable(kill: bool) -> Result<(), String> {
    if kill {
        stop_host()?;
    }
    let mut cfg = load_remote_config();
    if matches!(cfg.backend, RemoteBackend::MetisNative) {
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "metis-native-rdp-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn password_file_is_0600_and_roundtrips() {
        let dir = scratch_dir("roundtrip");
        write_password_in(&dir, "s3cret pass ").expect("write");
        let meta = fs::metadata(dir.join(PASSWORD_FILE)).expect("meta");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(&dir).expect("dir").permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            read_password_in(&dir).expect("read").as_deref(),
            Some("s3cret pass ")
        );
        // Overwrite is atomic and leaves no temp files behind.
        write_password_in(&dir, "other").expect("rewrite");
        assert_eq!(
            read_password_in(&dir).expect("read").as_deref(),
            Some("other")
        );
        let leftovers = fs::read_dir(&dir)
            .expect("ls")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_empty_password_reads_as_none() {
        let dir = scratch_dir("empty");
        ensure_private_dir(&dir).expect("dir");
        assert_eq!(read_password_in(&dir).expect("missing"), None);
        fs::write(dir.join(PASSWORD_FILE), "\n").expect("write");
        assert_eq!(read_password_in(&dir).expect("empty"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lax_password_mode_is_tightened() {
        let dir = scratch_dir("mode");
        ensure_private_dir(&dir).expect("dir");
        let path = dir.join(PASSWORD_FILE);
        fs::write(&path, "pw\n").expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(read_password_in(&dir).expect("read").as_deref(), Some("pw"));
        assert_eq!(
            fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn symlinked_password_file_is_refused() {
        let dir = scratch_dir("symlink");
        ensure_private_dir(&dir).expect("dir");
        let target = dir.join("elsewhere");
        fs::write(&target, "pw\n").expect("write");
        std::os::unix::fs::symlink(&target, dir.join(PASSWORD_FILE)).expect("symlink");
        assert!(read_password_in(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_validation_rejects_bad_input() {
        assert!(validate_username("  ").is_err());
        assert!(validate_username("bad\nname").is_err());
        assert_eq!(validate_username(" alice ").as_deref(), Ok("alice"));
        assert!(validate_password("").is_err());
        assert!(validate_password("a\nb").is_err());
        assert!(validate_password("a\0b").is_err());
        assert!(validate_password(&"x".repeat(MAX_CREDENTIAL_LEN + 1)).is_err());
        assert!(validate_password("ok pass").is_ok());
    }

    #[test]
    fn host_names_non_empty() {
        assert!(!HOST_NAMES.is_empty());
    }
}
