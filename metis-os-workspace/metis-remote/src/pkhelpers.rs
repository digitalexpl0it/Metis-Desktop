//! Privileged helpers intended to run only under `pkexec` (Phase 15 §B).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use zeroize::Zeroize;

/// Packages Metis may install via Polkit (onboarding + gaming health fixes).
pub const APT_ALLOWLIST: &[&str] = &[
    "gnome-remote-desktop",
    "flatpak",
    "gamemode",
    "bluez",
    "bluetooth",
    "cups",
    "system-config-printer",
    "gnome-keyring",
    "mesa-vulkan-drivers",
    "mesa-vulkan-drivers:i386",
    "pipewire-audio",
    "steam-installer",
    "steam-devices",
    "nftables",
    "pkexec",
    "polkitd",
];

pub(crate) fn require_root() -> Result<(), String> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if uid == "0" {
        Ok(())
    } else {
        Err("this command must run as root (via pkexec)".into())
    }
}

fn package_allowed(pkg: &str) -> bool {
    APT_ALLOWLIST.contains(&pkg)
}

/// `apt-get install -y -- <allowlisted packages…>` — root only.
pub fn apt_install(packages: &[String]) -> Result<(), String> {
    require_root()?;
    if packages.is_empty() {
        return Ok(());
    }
    for pkg in packages {
        if pkg.is_empty()
            || pkg.contains(['/', ' ', '\0', ';', '|', '&', '$', '`', '\n', '\r'])
            || !package_allowed(pkg)
        {
            return Err(format!(
                "package '{pkg}' is not on the Metis allowlist (refusing apt-get)"
            ));
        }
    }
    let status = Command::new("apt-get")
        .args(["install", "-y", "--"])
        .args(packages)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .map_err(|e| format!("apt-get failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apt-get exited with {status}"))
    }
}

/// Guided NVIDIA install: fixed `ubuntu-drivers install` argv only (no free-form shell).
/// Best-effort matching `libnvidia-gl-<series>:i386` when a series can be detected.
pub fn ubuntu_drivers_install() -> Result<(), String> {
    require_root()?;
    let ubuntu_drivers = if Path::new("/usr/bin/ubuntu-drivers").is_file() {
        "/usr/bin/ubuntu-drivers"
    } else {
        "ubuntu-drivers"
    };
    let status = Command::new(ubuntu_drivers)
        .arg("install")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .map_err(|e| format!("ubuntu-drivers failed: {e}"))?;
    if !status.success() {
        return Err(format!("ubuntu-drivers install exited with {status}"));
    }
    // Best-effort i386 GL for Proton; series detection may fail until reboot.
    if let Some(series) = detect_nvidia_series_digits() {
        let pkg = format!("libnvidia-gl-{series}:i386");
        let _ = Command::new("apt-get")
            .args(["install", "-y", "--", &pkg])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .status();
    } else {
        tracing::warn!(
            "could not detect NVIDIA driver series for i386 GL — install libnvidia-gl-*:i386 after reboot if Proton needs it"
        );
    }
    Ok(())
}

fn detect_nvidia_series_digits() -> Option<String> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Package}\n", "nvidia-driver-*"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let pkg = line.trim();
        if let Some(rest) = pkg.strip_prefix("nvidia-driver-") {
            let series: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !series.is_empty() && series.len() <= 4 {
                return Some(series);
            }
        }
    }
    None
}

/// Validate a Unix username (no path separators / control chars).
pub fn validate_username(user: &str) -> Result<(), String> {
    if user.is_empty() || user.len() > 64 {
        return Err("invalid username".into());
    }
    if !user
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err("username contains invalid characters".into());
    }
    Ok(())
}

/// `usermod -aG input <user>` — root only.
pub fn add_input_group(user: &str) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    let status = Command::new("usermod")
        .args(["-aG", "input", user])
        .status()
        .map_err(|e| format!("usermod failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("usermod exited with {status}"))
    }
}

/// Prefer the packaged binary so pkexec cannot escalate a writable cwd copy.
pub fn privileged_exe() -> PathBuf {
    const INSTALLED: &str = "/usr/bin/metis-remote";
    if Path::new(INSTALLED).is_file() {
        return Path::new(INSTALLED).to_path_buf();
    }
    std::env::current_exe().unwrap_or_else(|_| Path::new("metis-remote").to_path_buf())
}

fn metis_polkit_agent_running() -> bool {
    let Ok(dir) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(exe) = fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        if exe
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "metis-polkit-agent")
        {
            return true;
        }
    }
    false
}

fn resolve_metis_polkit_agent() -> Option<PathBuf> {
    if let Ok(override_bin) = std::env::var("METIS_POLKIT_AGENT_BIN") {
        let p = PathBuf::from(override_bin);
        if p.is_file() {
            return Some(p);
        }
    }
    const INSTALLED: &[&str] = &[
        "/usr/libexec/metis-polkit-agent",
        "/usr/bin/metis-polkit-agent",
        "/usr/local/bin/metis-polkit-agent",
    ];
    for path in INSTALLED {
        let p = Path::new(path);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    std::env::current_exe().ok().and_then(|exe| {
        let sibling = exe.parent()?.join("metis-polkit-agent");
        sibling.is_file().then_some(sibling)
    })
}

/// Ensure the Metis PolicyKit authentication agent is running.
///
/// Without an agent, `pkexec` falls back to `pkttyagent` (needs a TTY) and
/// Settings background threads hang or fail with "Not authorized".
pub fn ensure_polkit_agent() {
    if std::env::var_os("METIS_NO_POLKIT_AGENT").is_some() {
        return;
    }
    if metis_polkit_agent_running() {
        return;
    }
    stop_third_party_polkit_agents();
    let Some(bin) = resolve_metis_polkit_agent() else {
        tracing::warn!(
            "metis-polkit-agent not found — install Metis or set METIS_POLKIT_AGENT_BIN"
        );
        return;
    };
    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GTK_A11Y", "none")
        .env("NO_AT_BRIDGE", "1")
        .env("GSK_RENDERER", "cairo");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(_) => {
            tracing::info!(path = %bin.display(), "started metis-polkit-agent");
            std::thread::sleep(Duration::from_millis(400));
        }
        Err(err) => {
            tracing::warn!(%err, path = %bin.display(), "failed to start metis-polkit-agent");
        }
    }
}

fn stop_third_party_polkit_agents() {
    let Ok(dir) = fs::read_dir("/proc") else {
        return;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let Ok(exe) = fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        let exe_s = exe.to_string_lossy();
        if exe_s.contains("polkit-gnome-authentication-agent")
            || exe_s.contains("polkit-kde-authentication-agent")
            || exe_s.contains("polkit-mate-authentication-agent")
            || exe_s.contains("lxqt-policykit-agent")
        {
            tracing::info!(%pid, path = %exe_s, "stopping third-party PolicyKit agent");
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
    std::thread::sleep(Duration::from_millis(300));
}

/// Run `pkexec <privileged_exe> <args…>` with an in-process timeout.
///
/// Do **not** wrap with the `timeout` binary — polkit then sees `timeout` as
/// the subject, and some agents mishandle auth. Stdin is always `/dev/null`
/// so a GUI agent can attach (never pipe secrets into pkexec).
pub fn run_pkexec(args: &[&str], wait: Duration) -> Result<std::process::Output, String> {
    ensure_polkit_agent();
    let bin = privileged_exe();
    let child = Command::new("pkexec")
        .arg(&bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start pkexec ({e}) — install policykit-1 / pkexec"))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(wait) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("pkexec wait failed: {e}")),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
            std::thread::sleep(Duration::from_millis(500));
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            Err(
                "Timed out waiting for admin approval. The Metis PolicyKit dialog \
                 should appear — if it does not, ensure metis-polkit-agent is running."
                    .into(),
            )
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("pkexec worker thread disconnected".into())
        }
    }
}

/// Format a failed `pkexec` output into a user-facing error.
pub fn pkexec_failure_message(output: &std::process::Output, context: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };
    if detail.is_empty() {
        format!(
            "Admin approval failed or was cancelled{context}. \
             Enter your account password in the PolicyKit dialog \
             (Authorize alone is not enough)."
        )
    } else if detail.to_ascii_lowercase().contains("not authorized") {
        format!(
            "{detail} — authentication failed. Use your user password in the \
             PolicyKit dialog (wrong password, empty password, or a broken agent \
             all produce this message)."
        )
    } else {
        detail
    }
}

/// Write a one-shot password file under `$XDG_RUNTIME_DIR` (mode 0600).
///
/// Prefer this over piping secrets into `pkexec`'s stdin — a piped stdin can
/// prevent the GUI authentication agent from attaching cleanly.
pub fn write_password_file(password: &str) -> Result<PathBuf, String> {
    if password.is_empty() || password.contains('\0') {
        return Err("password must not be empty".into());
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let path = dir.join(format!(
        "metis-pw-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("create password file: {e}"))?;
    file.write_all(password.as_bytes())
        .map_err(|e| format!("write password file: {e}"))?;
    if !password.ends_with('\n') {
        file.write_all(b"\n")
            .map_err(|e| format!("write password file: {e}"))?;
    }
    file.sync_all()
        .map_err(|e| format!("sync password file: {e}"))?;
    Ok(path)
}

/// Read + delete a password file written by [`write_password_file`] (root only).
pub fn take_password_file(path: &str) -> Result<String, String> {
    require_root()?;
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("password file must be an absolute path".into());
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("password file path must not contain ..".into());
    }
    let meta = fs::metadata(path).map_err(|e| format!("stat password file: {e}"))?;
    if !meta.is_file() {
        return Err("password file is not a regular file".into());
    }
    if meta.len() > 4096 {
        return Err("password file too large".into());
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        let _ = fs::remove_file(path);
        return Err("password file permissions too open (expected 0600)".into());
    }
    let file_uid = meta.uid();
    let caller_uid = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    if file_uid != 0 && caller_uid.is_some_and(|u| u != file_uid) {
        let _ = fs::remove_file(path);
        return Err("password file owner does not match caller".into());
    }
    let mut raw = fs::read_to_string(path).map_err(|e| format!("read password file: {e}"))?;
    let _ = fs::remove_file(path);
    let trimmed = raw.trim_end_matches(['\r', '\n']).to_string();
    raw.zeroize();
    if trimmed.is_empty() {
        return Err("password file was empty".into());
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_includes_gaming_packages() {
        assert!(APT_ALLOWLIST.contains(&"mesa-vulkan-drivers"));
        assert!(APT_ALLOWLIST.contains(&"mesa-vulkan-drivers:i386"));
        assert!(APT_ALLOWLIST.contains(&"steam-devices"));
    }

    #[test]
    fn rejects_unknown_package() {
        assert!(!package_allowed("evil-package"));
        assert!(!package_allowed("mesa-vulkan-drivers;rm -rf /"));
    }
}
