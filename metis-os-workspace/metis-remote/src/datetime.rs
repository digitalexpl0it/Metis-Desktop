//! System date/time via `timedatectl` (status unprivileged; mutations via pkexec).

use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::pkhelpers::{pkexec_failure_message, require_root, run_pkexec};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateTimeStatus {
    pub timezone: String,
    pub ntp: bool,
    pub ntp_synchronized: bool,
    pub local_rtc: bool,
    /// Human-readable local clock from `date`.
    pub local_time: String,
}

fn escalate(args: &[&str]) -> Result<(), String> {
    let output = run_pkexec(args, std::time::Duration::from_secs(90))?;
    if output.status.success() {
        return Ok(());
    }
    Err(pkexec_failure_message(&output, ""))
}

fn timedatectl_show_map() -> Result<std::collections::HashMap<String, String>, String> {
    let output = Command::new("timedatectl")
        .args([
            "show",
            "--no-pager",
            "--property=Timezone",
            "--property=NTP",
            "--property=NTPSynchronized",
            "--property=LocalRTC",
        ])
        .output()
        .map_err(|e| format!("timedatectl show failed: {e}"))?;
    if !output.status.success() {
        // Older timedatectl without `show` — fall back to `status` parse.
        return timedatectl_status_fallback();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut map = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Ok(map)
}

fn timedatectl_status_fallback() -> Result<std::collections::HashMap<String, String>, String> {
    let output = Command::new("timedatectl")
        .arg("status")
        .output()
        .map_err(|e| format!("timedatectl status failed: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut map = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Time zone:") {
            let tz = rest.split_whitespace().next().unwrap_or("").to_string();
            map.insert("Timezone".into(), tz);
        } else if let Some(rest) = line.strip_prefix("System clock synchronized:") {
            map.insert(
                "NTPSynchronized".into(),
                if rest.trim().eq_ignore_ascii_case("yes") {
                    "yes".into()
                } else {
                    "no".into()
                },
            );
        } else if line.starts_with("NTP service:") || line.starts_with("Network time on:") {
            let on = line.to_ascii_lowercase().contains("active")
                || line.to_ascii_lowercase().contains(" yes");
            map.insert("NTP".into(), if on { "yes".into() } else { "no".into() });
        } else if let Some(rest) = line.strip_prefix("RTC in local TZ:") {
            map.insert(
                "LocalRTC".into(),
                if rest.trim().eq_ignore_ascii_case("yes") {
                    "yes".into()
                } else {
                    "no".into()
                },
            );
        }
    }
    Ok(map)
}

fn parse_bool(v: Option<&String>) -> bool {
    matches!(
        v.map(|s| s.as_str()),
        Some("yes" | "Yes" | "true" | "1" | "on")
    )
}

fn local_time_string() -> String {
    Command::new("date")
        .arg("+%d %B %Y, %I:%M %p")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "—".into())
}

pub fn status() -> Result<DateTimeStatus, String> {
    let map = timedatectl_show_map()?;
    Ok(DateTimeStatus {
        timezone: map.get("Timezone").cloned().unwrap_or_else(|| "UTC".into()),
        ntp: parse_bool(map.get("NTP")),
        ntp_synchronized: parse_bool(map.get("NTPSynchronized")),
        local_rtc: parse_bool(map.get("LocalRTC")),
        local_time: local_time_string(),
    })
}

pub fn status_as_root() -> Result<(), String> {
    let snap = status()?;
    let json = serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

pub fn set_ntp(on: bool) -> Result<(), String> {
    escalate(&["pk-datetime-set-ntp", if on { "true" } else { "false" }])
}

pub fn set_ntp_as_root(on: bool) -> Result<(), String> {
    require_root()?;
    let status = Command::new("timedatectl")
        .args(["set-ntp", if on { "true" } else { "false" }])
        .status()
        .map_err(|e| format!("timedatectl set-ntp failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("timedatectl set-ntp exited with {status}"))
    }
}

fn validate_timezone(tz: &str) -> Result<(), String> {
    if tz.is_empty() || tz.len() > 64 || tz.contains(['\n', '\r', '\0', ' ', ';']) {
        return Err("invalid timezone".into());
    }
    if !tz
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/' || c == '-' || c == '+')
    {
        return Err("invalid timezone characters".into());
    }
    let path = std::path::Path::new("/usr/share/zoneinfo").join(tz);
    if !path.is_file() {
        return Err(format!("unknown timezone '{tz}'"));
    }
    Ok(())
}

pub fn set_timezone(tz: &str) -> Result<(), String> {
    validate_timezone(tz)?;
    escalate(&["pk-datetime-set-timezone", tz])
}

pub fn set_timezone_as_root(tz: &str) -> Result<(), String> {
    require_root()?;
    validate_timezone(tz)?;
    let status = Command::new("timedatectl")
        .args(["set-timezone", tz])
        .status()
        .map_err(|e| format!("timedatectl set-timezone failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("timedatectl set-timezone exited with {status}"))
    }
}

/// `timedatectl set-time` argument, e.g. `2026-09-19 20:37:00`.
pub fn set_time(spec: &str) -> Result<(), String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.len() > 64 || spec.contains(['\n', '\r', '\0', ';', '|', '&']) {
        return Err("invalid time specification".into());
    }
    escalate(&["pk-datetime-set-time", spec])
}

pub fn set_time_as_root(spec: &str) -> Result<(), String> {
    require_root()?;
    let spec = spec.trim();
    if spec.is_empty() || spec.len() > 64 || spec.contains(['\n', '\r', '\0', ';', '|', '&']) {
        return Err("invalid time specification".into());
    }
    let status = Command::new("timedatectl")
        .args(["set-time", spec])
        .status()
        .map_err(|e| format!("timedatectl set-time failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("timedatectl set-time exited with {status}"))
    }
}
