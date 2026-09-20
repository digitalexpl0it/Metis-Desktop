//! Resolve the logind / systemd session id for RegisterAuthenticationAgent.

use std::collections::HashMap;
use std::fs;
use std::process::Command;

use zbus::zvariant::OwnedValue;

/// Prefer `XDG_SESSION_ID`, then logind `GetSessionByPID`, then `/proc/self/sessionid`.
pub fn resolve_session_id() -> Result<String, String> {
    if let Ok(id) = std::env::var("XDG_SESSION_ID") {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }

    if let Ok(id) = session_id_from_logind() {
        return Ok(id);
    }

    if let Ok(raw) = fs::read_to_string("/proc/self/sessionid") {
        let id = raw.trim().to_string();
        // "4294967295" means "no session" on Linux.
        if !id.is_empty() && id != "4294967295" {
            return Ok(id);
        }
    }

    // Last resort: `loginctl list-sessions` for this user (active).
    if let Ok(id) = session_id_from_loginctl() {
        return Ok(id);
    }

    Err(
        "could not resolve XDG_SESSION_ID — set it in the session environment \
         (compositor should export it via systemd/dbus activation env)"
            .into(),
    )
}

fn session_id_from_logind() -> Result<String, String> {
    let uid = unsafe { libc::getuid() };
    let rt = tokio::runtime::Handle::try_current().ok();
    // We may be called before the tokio runtime is entered — use a tiny blocking
    // runtime when needed.
    let fut = async {
        let conn = zbus::Connection::system()
            .await
            .map_err(|e| format!("logind bus: {e}"))?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await
        .map_err(|e| format!("logind proxy: {e}"))?;
        let path: zbus::zvariant::OwnedObjectPath = proxy
            .call("GetSessionByPID", &(std::process::id()))
            .await
            .map_err(|e| format!("GetSessionByPID: {e}"))?;
        let session = zbus::Proxy::new(
            &conn,
            "org.freedesktop.login1",
            path.as_str(),
            "org.freedesktop.login1.Session",
        )
        .await
        .map_err(|e| format!("session proxy: {e}"))?;
        let id: String = session
            .get_property("Id")
            .await
            .map_err(|e| format!("session Id: {e}"))?;
        let _ = uid;
        Ok(id)
    };
    if let Some(handle) = rt {
        handle.block_on(fut)
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio: {e}"))?
            .block_on(fut)
    }
}

fn session_id_from_loginctl() -> Result<String, String> {
    let user = std::env::var("USER").unwrap_or_default();
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .map_err(|e| format!("loginctl: {e}"))?;
    if !output.status.success() {
        return Err("loginctl list-sessions failed".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // SESSION UID USER SEAT TTY
        if parts.len() >= 3 && (user.is_empty() || parts[2] == user) {
            return Ok(parts[0].to_string());
        }
    }
    Err("no loginctl session for user".into())
}

/// Build a unix-session Subject map (also used if we need to re-register).
#[allow(dead_code)]
pub fn session_subject_details(session_id: &str) -> HashMap<String, OwnedValue> {
    let mut details = HashMap::new();
    details.insert(
        "session-id".to_string(),
        zbus::zvariant::OwnedValue::try_from(zbus::zvariant::Value::new(session_id.to_string()))
            .expect("session-id value"),
    );
    details
}
