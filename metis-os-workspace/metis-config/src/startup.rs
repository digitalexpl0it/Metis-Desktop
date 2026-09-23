//! Session startup applications (`~/.config/metis/startup.json`).
//!
//! Empty by default. Entries are desktop app ids only (no free-form command
//! lines). The compositor resolves Exec from XDG `.desktop` files and spawns
//! argv-only after the shell is up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::widget_ext::is_safe_launch_id;

const MAX_ENTRIES: usize = 64;
const MAX_DELAY_SECS: u32 = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartupEntry {
    /// Desktop file id, e.g. `firefox.desktop` or `org.mozilla.firefox`.
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Extra delay after the compositor's base startup wait (0–120 s).
    #[serde(default)]
    pub delay_seconds: u32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartupConfig {
    /// Master switch — when false, no startup apps launch.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub entries: Vec<StartupEntry>,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            entries: Vec::new(),
        }
    }
}

pub fn startup_config_path() -> PathBuf {
    super::config_dir().join("startup.json")
}

pub fn load_startup_config() -> StartupConfig {
    let path = startup_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<StartupConfig>(&text) {
            Ok(cfg) => return sanitize_startup_config(cfg),
            Err(err) => tracing::warn!(%err, "startup.json parse failed — using defaults"),
        }
    }
    StartupConfig::default()
}

pub fn save_startup_config(cfg: &StartupConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let clean = sanitize_startup_config(cfg.clone());
    let json = serde_json::to_string_pretty(&clean).map_err(std::io::Error::other)?;
    std::fs::write(startup_config_path(), json)
}

/// Drop invalid ids, clamp delays, dedupe by id (first wins), cap list length.
pub fn sanitize_startup_config(mut cfg: StartupConfig) -> StartupConfig {
    let mut seen = std::collections::HashSet::new();
    cfg.entries.retain(|e| {
        let id = e.id.trim();
        if !is_safe_launch_id(id) {
            return false;
        }
        if !seen.insert(id.to_string()) {
            return false;
        }
        true
    });
    for e in &mut cfg.entries {
        e.id = e.id.trim().to_string();
        e.delay_seconds = e.delay_seconds.min(MAX_DELAY_SECS);
    }
    if cfg.entries.len() > MAX_ENTRIES {
        cfg.entries.truncate(MAX_ENTRIES);
    }
    cfg
}

/// Resolve a desktop app id to an argv vector (field codes stripped). No shell.
pub fn resolve_desktop_launch_argv(id: &str) -> Option<Vec<String>> {
    let id = id.trim();
    if !is_safe_launch_id(id) {
        return None;
    }
    let path = find_desktop_file(id)?;
    let exec = read_desktop_exec(&path)?;
    let argv = clean_exec_argv(&exec);
    if argv.is_empty() {
        return None;
    }
    if is_stub_exec(&argv[0]) {
        return None;
    }
    Some(argv)
}

fn find_desktop_file(id: &str) -> Option<PathBuf> {
    let candidates = desktop_name_candidates(id);
    for dir in applications_dirs() {
        for name in &candidates {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
            // Flatpak / vendor subdirs: applications/foo/bar.desktop
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let nested = p.join(name);
                        if nested.is_file() {
                            return Some(nested);
                        }
                    }
                }
            }
        }
    }
    None
}

fn desktop_name_candidates(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(id.to_string());
    if !id.ends_with(".desktop") {
        out.push(format!("{id}.desktop"));
    }
    out
}

fn applications_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("XDG_DATA_HOME") {
        if !home.is_empty() {
            dirs.push(PathBuf::from(home).join("applications"));
        }
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    // Common Flatpak export roots (also often on XDG_DATA_DIRS in Metis sessions).
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs
}

fn read_desktop_exec(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_desktop = false;
    let mut exec = None;
    let mut try_exec = None;
    let mut hidden = false;
    let mut no_display = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_desktop = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_desktop {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Exec=") {
            exec = Some(unescape_desktop(rest));
        } else if let Some(rest) = line.strip_prefix("TryExec=") {
            try_exec = Some(unescape_desktop(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("Hidden=") {
            hidden = rest.eq_ignore_ascii_case("true");
        } else if let Some(rest) = line.strip_prefix("NoDisplay=") {
            no_display = rest.eq_ignore_ascii_case("true");
        }
    }
    if hidden || no_display {
        // Still allow explicit user startup picks of NoDisplay helpers; only skip Hidden.
        if hidden {
            return None;
        }
    }
    if let Some(te) = try_exec
        && !try_exec_ok(&te)
    {
        return None;
    }
    exec.filter(|s| !s.trim().is_empty())
}

fn unescape_desktop(s: &str) -> String {
    // Desktop Entry spec: \s \n \t \r \\
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn try_exec_ok(te: &str) -> bool {
    let te = te.trim();
    if te.is_empty() {
        return true;
    }
    if te.contains('/') {
        return Path::new(te).is_file();
    }
    binary_on_path(te)
}

fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

fn clean_exec_argv(exec: &str) -> Vec<String> {
    split_exec(exec)
        .into_iter()
        .filter(|tok| !(tok.len() == 2 && tok.starts_with('%')))
        .collect()
}

/// Minimal argv split for Exec= lines (handles double quotes; no shell expansion).
fn split_exec(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quote = !in_quote,
            '\\' if in_quote => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_stub_exec(program: &str) -> bool {
    matches!(program, "false" | "/usr/bin/false" | "/bin/false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_bad_ids() {
        let cfg = StartupConfig {
            enabled: true,
            entries: vec![
                StartupEntry {
                    id: "firefox.desktop".into(),
                    enabled: true,
                    delay_seconds: 999,
                },
                StartupEntry {
                    id: "../evil".into(),
                    enabled: true,
                    delay_seconds: 0,
                },
                StartupEntry {
                    id: "firefox.desktop".into(),
                    enabled: false,
                    delay_seconds: 0,
                },
            ],
        };
        let clean = sanitize_startup_config(cfg);
        assert_eq!(clean.entries.len(), 1);
        assert_eq!(clean.entries[0].id, "firefox.desktop");
        assert_eq!(clean.entries[0].delay_seconds, MAX_DELAY_SECS);
    }

    #[test]
    fn split_exec_strips_field_codes() {
        let argv = clean_exec_argv("firefox %u --new-window");
        assert_eq!(argv, vec!["firefox", "--new-window"]);
    }
}
