//! Local user account helpers (list is unprivileged; mutations escalate via pkexec).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::pkhelpers::{
    pkexec_failure_message, require_root, run_pkexec, take_password_file, validate_username,
    write_password_file,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub display_name: String,
    pub home: String,
    pub shell: String,
    pub is_admin: bool,
    /// True when this row is the session user (`$USER` / `PKEXEC_UID` caller).
    #[serde(default)]
    pub is_current: bool,
}

fn escalate(args: &[&str]) -> Result<(), String> {
    escalate_pkexec(args, None)
}

/// Run `pkexec metis-remote …`. When `password_file` is set, the helper reads
/// the secret from that path (not from pkexec's stdin — piping stdin breaks
/// GUI PolicyKit agents).
fn escalate_pkexec(args: &[&str], password_file: Option<&Path>) -> Result<(), String> {
    let mut owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    if let Some(path) = password_file {
        owned.push("--password-file".into());
        owned.push(path.display().to_string());
    }
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let output = run_pkexec(&refs, std::time::Duration::from_secs(90))?;
    // Best-effort cleanup if auth failed before the root helper ran.
    if let Some(path) = password_file {
        let _ = std::fs::remove_file(path);
    }
    if output.status.success() {
        return Ok(());
    }
    Err(pkexec_failure_message(&output, ""))
}

fn current_username() -> Option<String> {
    std::env::var("USER").ok().filter(|s| !s.is_empty())
}

fn user_in_admin_group(user: &str) -> bool {
    let Ok(output) = Command::new("id").args(["-nG", user]).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let groups = String::from_utf8_lossy(&output.stdout);
    groups.split_whitespace().any(|g| g == "sudo" || g == "admin" || g == "wheel")
}

/// List local human users (UID ≥ 1000, exclude nobody). Unprivileged.
pub fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let passwd = std::fs::read_to_string("/etc/passwd")
        .map_err(|e| format!("read /etc/passwd: {e}"))?;
    let current = current_username();
    let mut out = Vec::new();
    for line in passwd.lines() {
        let mut parts = line.split(':');
        let Some(name) = parts.next() else { continue };
        let _ = parts.next(); // password
        let Some(uid_s) = parts.next() else { continue };
        let Some(gid_s) = parts.next() else { continue };
        let gecos = parts.next().unwrap_or("");
        let home = parts.next().unwrap_or("");
        let shell = parts.next().unwrap_or("");
        let Ok(uid) = uid_s.parse::<u32>() else { continue };
        let Ok(gid) = gid_s.parse::<u32>() else { continue };
        if uid < 1000 || name == "nobody" {
            continue;
        }
        // Skip nologin/false shells typically used for system service accounts
        // that still got a high UID on some setups — keep real login shells.
        if shell.ends_with("nologin") || shell.ends_with("/false") {
            continue;
        }
        let display = gecos
            .split(',')
            .next()
            .unwrap_or(gecos)
            .trim()
            .to_string();
        let display_name = if display.is_empty() {
            name.to_string()
        } else {
            display
        };
        out.push(AccountInfo {
            is_admin: user_in_admin_group(name),
            is_current: current.as_deref() == Some(name),
            username: name.to_string(),
            uid,
            gid,
            display_name,
            home: home.to_string(),
            shell: shell.to_string(),
        });
    }
    out.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(out)
}

/// Root: print JSON list (for `pk-accounts-list`).
pub fn accounts_list_as_root() -> Result<(), String> {
    // Readable without root, but keep behind polkit for a stable Settings IPC.
    let list = list_accounts()?;
    let json = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

pub fn set_display_name(user: &str, name: &str) -> Result<(), String> {
    validate_username(user)?;
    let name = name.trim();
    if name.len() > 256 || name.contains(['\n', '\r', ':', '\0']) {
        return Err("invalid display name".into());
    }
    escalate(&["pk-accounts-set-name", user, name])
}

pub fn set_display_name_as_root(user: &str, name: &str) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    let name = name.trim();
    if name.len() > 256 || name.contains(['\n', '\r', ':', '\0']) {
        return Err("invalid display name".into());
    }
    let status = Command::new("usermod")
        .args(["-c", name, user])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("usermod failed: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&status.stderr);
        Err(if err.trim().is_empty() {
            format!("usermod exited with {}", status.status)
        } else {
            err.trim().to_string()
        })
    }
}

pub fn set_password(user: &str, password: &str) -> Result<(), String> {
    validate_username(user)?;
    if password.is_empty() || password.contains('\0') {
        return Err("password must not be empty".into());
    }
    let path = write_password_file(password)?;
    escalate_pkexec(&["pk-accounts-set-password", user], Some(&path))
}

pub fn set_password_as_root(user: &str, password_file: Option<&str>) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    let mut password = if let Some(path) = password_file {
        take_password_file(path)?
    } else {
        let mut password = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut password)
            .map_err(|e| format!("read password: {e}"))?;
        password.trim_end_matches(['\r', '\n']).to_string()
    };
    let result = apply_password(user, &password);
    password.zeroize();
    result
}

fn apply_password(user: &str, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("password on stdin must not be empty".into());
    }
    let line = format!("{user}:{password}\n");
    let mut child = Command::new("chpasswd")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("chpasswd failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("write chpasswd: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("chpasswd wait: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(if err.trim().is_empty() {
            format!("chpasswd exited with {}", output.status)
        } else {
            err.trim().to_string()
        })
    }
}

pub fn set_admin(user: &str, admin: bool) -> Result<(), String> {
    validate_username(user)?;
    escalate(&[
        "pk-accounts-set-admin",
        user,
        if admin { "true" } else { "false" },
    ])
}

pub fn set_admin_as_root(user: &str, admin: bool) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    if current_username().as_deref() == Some(user) && !admin {
        // Allow demoting self only if another admin exists — keep simple: refuse.
        return Err("refusing to remove admin from the current user".into());
    }
    let status = if admin {
        Command::new("usermod")
            .args(["-aG", "sudo", user])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
    } else {
        // Prefer gpasswd -d; fall back to deluser.
        let st = Command::new("gpasswd")
            .args(["-d", user, "sudo"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match st {
            Ok(s) if s.success() => Ok(s),
            _ => Command::new("deluser")
                .args([user, "sudo"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status(),
        }
    }
    .map_err(|e| format!("usermod/gpasswd failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("admin group update exited with {status}"))
    }
}

pub fn add_user(user: &str, display_name: &str, admin: bool, password: &str) -> Result<(), String> {
    validate_username(user)?;
    if password.is_empty() {
        return Err("password must not be empty".into());
    }
    let path = write_password_file(password)?;
    let mut owned: Vec<String> = vec!["pk-accounts-add".into(), user.into()];
    if admin {
        owned.push("--admin".into());
    }
    let name = display_name.trim();
    if !name.is_empty() {
        owned.push("--name".into());
        owned.push(name.into());
    }
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    escalate_pkexec(&refs, Some(&path))
}

pub fn add_user_as_root(
    user: &str,
    display_name: Option<&str>,
    admin: bool,
    password_file: Option<&str>,
) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    let mut password = if let Some(path) = password_file {
        take_password_file(path)?
    } else {
        // Legacy: password on stdin (avoid for GUI pkexec — agent cannot attach).
        let mut password = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut password)
            .map_err(|e| format!("read password: {e}"))?;
        let trimmed = password.trim_end_matches(['\r', '\n']).to_string();
        password.zeroize();
        trimmed
    };
    if password.is_empty() {
        return Err("password must not be empty".into());
    }

    // Refuse colliding names (also catch partially-created accounts).
    if std::path::Path::new("/etc/passwd")
        .exists()
        .then(|| std::fs::read_to_string("/etc/passwd").ok())
        .flatten()
        .is_some_and(|p| p.lines().any(|l| l.split(':').next() == Some(user)))
    {
        password.zeroize();
        return Err(format!("user '{user}' already exists"));
    }

    let mut cmd = Command::new("useradd");
    cmd.args(["-m", "-s", "/bin/bash"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if admin {
        // Prefer sudo; fall back without -G if the group is missing.
        cmd.args(["-G", "sudo"]);
    }
    if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
        if name.contains(['\n', '\r', ':', '\0']) {
            password.zeroize();
            return Err("invalid display name".into());
        }
        cmd.args(["-c", name]);
    }
    cmd.arg(user);
    let output = cmd
        .output()
        .map_err(|e| format!("useradd failed: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        // Retry without -G sudo when the group is absent.
        if admin && err.to_ascii_lowercase().contains("group") {
            let mut cmd = Command::new("useradd");
            cmd.args(["-m", "-s", "/bin/bash"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
                cmd.args(["-c", name]);
            }
            cmd.arg(user);
            let output = cmd
                .output()
                .map_err(|e| format!("useradd failed: {e}"))?;
            if !output.status.success() {
                password.zeroize();
                let err = String::from_utf8_lossy(&output.stderr);
                return Err(if err.trim().is_empty() {
                    format!("useradd exited with {}", output.status)
                } else {
                    err.trim().to_string()
                });
            }
            let _ = set_admin_as_root(user, true);
        } else {
            password.zeroize();
            return Err(if err.trim().is_empty() {
                format!("useradd exited with {}", output.status)
            } else {
                err.trim().to_string()
            });
        }
    }
    let result = apply_password(user, &password);
    password.zeroize();
    result
}

pub fn remove_user(user: &str) -> Result<(), String> {
    validate_username(user)?;
    escalate(&["pk-accounts-remove", user])
}

pub fn remove_user_as_root(user: &str) -> Result<(), String> {
    require_root()?;
    validate_username(user)?;
    if current_username().as_deref() == Some(user) {
        return Err("refusing to remove the current user".into());
    }
    if user == "root" {
        return Err("refusing to remove root".into());
    }
    let output = Command::new("userdel")
        .args(["-r", user])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("userdel failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(if err.trim().is_empty() {
            format!("userdel exited with {}", output.status)
        } else {
            err.trim().to_string()
        })
    }
}
