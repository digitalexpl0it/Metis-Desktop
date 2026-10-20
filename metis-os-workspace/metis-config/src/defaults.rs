//! Default applications via `xdg-settings` / `xdg-mime` (mimeapps.list).

use std::path::PathBuf;
use std::process::Command;

use crate::menu::{self, load_menu_config, save_menu_config};

/// Well-known default-application roles shown in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAppRole {
    Browser,
    FileManager,
    Terminal,
    Mail,
    Images,
    Audio,
    Video,
    Pdf,
}

impl DefaultAppRole {
    pub const ALL: &[DefaultAppRole] = &[
        Self::Browser,
        Self::FileManager,
        Self::Terminal,
        Self::Mail,
        Self::Images,
        Self::Audio,
        Self::Video,
        Self::Pdf,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Browser => "Web browser",
            Self::FileManager => "File manager",
            Self::Terminal => "Terminal",
            Self::Mail => "Mail",
            Self::Images => "Images",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Pdf => "PDF",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Browser => "web-browser-symbolic",
            Self::FileManager => "system-file-manager-symbolic",
            Self::Terminal => "utilities-terminal-symbolic",
            Self::Mail => "mail-unread-symbolic",
            Self::Images => "image-x-generic-symbolic",
            Self::Audio => "audio-x-generic-symbolic",
            Self::Video => "video-x-generic-symbolic",
            Self::Pdf => "application-pdf-symbolic",
        }
    }

    /// Primary MIME / scheme used with `xdg-mime` / `xdg-settings`.
    fn mime(self) -> &'static str {
        match self {
            Self::Browser => "x-scheme-handler/http",
            Self::FileManager => "inode/directory",
            Self::Terminal => "x-scheme-handler/terminal",
            Self::Mail => "x-scheme-handler/mailto",
            Self::Images => "image/jpeg",
            Self::Audio => "audio/mpeg",
            Self::Video => "video/mp4",
            Self::Pdf => "application/pdf",
        }
    }

    fn extra_mimes(self) -> &'static [&'static str] {
        match self {
            Self::Browser => &["x-scheme-handler/https", "text/html"],
            Self::FileManager => &["inode/directory"],
            Self::Terminal => &[],
            Self::Mail => &[],
            Self::Images => &["image/png", "image/webp", "image/gif"],
            Self::Audio => &["audio/flac", "audio/ogg", "audio/x-wav"],
            Self::Video => &["video/webm", "video/x-matroska"],
            Self::Pdf => &["application/x-pdf"],
        }
    }
}

/// Desktop id currently registered as the default for `role`, if any.
pub fn query_default_app(role: DefaultAppRole) -> Option<String> {
    match role {
        DefaultAppRole::Browser => xdg_settings_get(&["get", "default-web-browser"]),
        DefaultAppRole::Mail => xdg_settings_get(&["get", "default-url-scheme-handler", "mailto"])
            .or_else(|| xdg_mime_query(role.mime())),
        DefaultAppRole::Terminal => {
            // Prefer Metis Menu preference (single source with menu rail), then mime.
            let cfg = load_menu_config();
            if let Some(term) = cfg.terminal.as_deref().filter(|s| !s.is_empty())
                && let Some(id) = desktop_id_for_executable(term)
            {
                return Some(id);
            }
            xdg_mime_query(role.mime()).or_else(query_gsettings_terminal_desktop)
        }
        DefaultAppRole::FileManager => {
            let cfg = load_menu_config();
            if let Some(fm) = cfg.file_manager.as_deref().filter(|s| !s.is_empty())
                && let Some(id) = desktop_id_for_executable(fm)
            {
                return Some(id);
            }
            xdg_mime_query(role.mime())
        }
        _ => xdg_mime_query(role.mime()),
    }
}

/// When Metis Menu sets a terminal / file-manager executable, mirror into
/// FreeDesktop defaults when a matching `.desktop` can be resolved.
pub fn sync_default_from_menu_executable(role: DefaultAppRole, exec: Option<&str>) {
    let Some(exec) = exec.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    if !matches!(role, DefaultAppRole::Terminal | DefaultAppRole::FileManager) {
        return;
    }
    if let Some(id) = desktop_id_for_executable(exec) {
        let _ = set_default_app(role, &id);
    }
}

/// Set the default application for `role` to `desktop_id` (e.g. `firefox.desktop`).
pub fn set_default_app(role: DefaultAppRole, desktop_id: &str) -> Result<(), String> {
    let id = normalize_desktop_id(desktop_id)?;
    match role {
        DefaultAppRole::Browser => {
            run_checked("xdg-settings", &["set", "default-web-browser", &id])?;
            for mime in role.extra_mimes() {
                let _ = run_checked("xdg-mime", &["default", &id, mime]);
            }
        }
        DefaultAppRole::Mail => {
            let _ = run_checked(
                "xdg-settings",
                &["set", "default-url-scheme-handler", "mailto", &id],
            );
            run_checked("xdg-mime", &["default", &id, role.mime()])?;
        }
        DefaultAppRole::Terminal => {
            let _ = run_checked("xdg-mime", &["default", &id, role.mime()]);
            sync_menu_terminal_from_desktop(&id)?;
            apply_gsettings_terminal(&id);
        }
        DefaultAppRole::FileManager => {
            run_checked("xdg-mime", &["default", &id, role.mime()])?;
            sync_menu_file_manager_from_desktop(&id)?;
        }
        _ => {
            run_checked("xdg-mime", &["default", &id, role.mime()])?;
            for mime in role.extra_mimes() {
                let _ = run_checked("xdg-mime", &["default", &id, mime]);
            }
        }
    }
    Ok(())
}

/// Desktop entries that can handle `role` (for Settings dropdowns).
pub fn candidate_apps_for_role(role: DefaultAppRole) -> Vec<(String, String)> {
    let mime = role.mime();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // `gio mime` lists handlers when available.
    if let Some(text) = run_stdout("gio", &["mime", mime]) {
        for line in text.lines() {
            let line = line.trim();
            if let Some(id) = line.strip_prefix("Default application:") {
                push_candidate(&mut out, &mut seen, id.trim());
            } else if let Some(id) = line.strip_prefix("Registered applications:") {
                for part in id.split(',') {
                    push_candidate(&mut out, &mut seen, part.trim());
                }
            } else if line.ends_with(".desktop") {
                push_candidate(&mut out, &mut seen, line);
            }
        }
    }

    // Fallback: scan applications for known categories.
    for (id, name) in scan_desktop_apps_for_role(role) {
        if seen.insert(id.clone()) {
            out.push((id, name));
        }
    }

    out.sort_by_key(|a| a.1.to_lowercase());
    out
}

fn push_candidate(
    out: &mut Vec<(String, String)>,
    seen: &mut std::collections::HashSet<String>,
    id: &str,
) {
    let Ok(id) = normalize_desktop_id(id) else {
        return;
    };
    if !seen.insert(id.clone()) {
        return;
    }
    let name = desktop_display_name(&id).unwrap_or_else(|| id.clone());
    out.push((id, name));
}

fn scan_desktop_apps_for_role(role: DefaultAppRole) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if desktop_matches_role(&text, role) {
                let name = desktop_name_from_text(&text).unwrap_or_else(|| id.clone());
                out.push((id, name));
            }
        }
    }
    out
}

fn desktop_matches_role(text: &str, role: DefaultAppRole) -> bool {
    let cats = desktop_categories(text);
    let mimes = desktop_mimetypes(text);
    match role {
        DefaultAppRole::Browser => {
            cats.iter().any(|c| c == "WebBrowser" || c == "Network")
                || mimes.iter().any(|m| m.contains("x-scheme-handler/http"))
        }
        DefaultAppRole::FileManager => {
            cats.iter().any(|c| c == "FileManager") || mimes.iter().any(|m| m == "inode/directory")
        }
        DefaultAppRole::Terminal => cats.iter().any(|c| c == "TerminalEmulator"),
        DefaultAppRole::Mail => {
            cats.iter().any(|c| c == "Email" || c == "Office")
                || mimes.iter().any(|m| m.contains("mailto"))
        }
        DefaultAppRole::Images => {
            cats.iter().any(|c| c == "Graphics" || c == "Viewer")
                || mimes.iter().any(|m| m.starts_with("image/"))
        }
        DefaultAppRole::Audio => {
            cats.iter().any(|c| c == "Audio" || c == "Player")
                || mimes.iter().any(|m| m.starts_with("audio/"))
        }
        DefaultAppRole::Video => {
            cats.iter().any(|c| c == "Video" || c == "Player")
                || mimes.iter().any(|m| m.starts_with("video/"))
        }
        DefaultAppRole::Pdf => mimes.iter().any(|m| m.contains("pdf")),
    }
}

fn desktop_categories(text: &str) -> Vec<String> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Categories=") {
            return rest
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

fn desktop_mimetypes(text: &str) -> Vec<String> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MimeType=") {
            return rest
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

fn desktop_name_from_text(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Name=") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.chars().filter(|c| !c.is_control()).take(128).collect());
            }
        }
    }
    None
}

fn desktop_display_name(id: &str) -> Option<String> {
    for dir in application_dirs() {
        let path = dir.join(id);
        if path.is_file()
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            return desktop_name_from_text(&text);
        }
    }
    None
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg).join("applications");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs.push(PathBuf::from("/usr/local/share/applications"));
    dirs.push(PathBuf::from("/usr/share/applications"));
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for part in data_dirs.split(':').filter(|s| !s.is_empty()) {
            let p = PathBuf::from(part).join("applications");
            if !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }
    dirs
}

fn normalize_desktop_id(raw: &str) -> Result<String, String> {
    let id = raw.trim();
    if id.is_empty() || id.len() > 256 {
        return Err("Invalid desktop id".into());
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("Invalid desktop id".into());
    }
    let id = if id.ends_with(".desktop") {
        id.to_string()
    } else {
        format!("{id}.desktop")
    };
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+')
    {
        return Err("Invalid desktop id".into());
    }
    Ok(id)
}

fn sync_menu_terminal_from_desktop(desktop_id: &str) -> Result<(), String> {
    let exec = desktop_exec_basename(desktop_id)
        .ok_or_else(|| format!("Could not resolve executable for {desktop_id}"))?;
    let mut cfg = load_menu_config();
    cfg.terminal = Some(exec);
    save_menu_config(&cfg).map_err(|e| e.to_string())
}

fn sync_menu_file_manager_from_desktop(desktop_id: &str) -> Result<(), String> {
    let exec = desktop_exec_basename(desktop_id)
        .ok_or_else(|| format!("Could not resolve executable for {desktop_id}"))?;
    let mut cfg = load_menu_config();
    cfg.file_manager = Some(exec);
    save_menu_config(&cfg).map_err(|e| e.to_string())
}

fn desktop_exec_basename(desktop_id: &str) -> Option<String> {
    for dir in application_dirs() {
        let path = dir.join(desktop_id);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Exec=") {
                let first = rest.split_whitespace().next()?;
                let base = PathBuf::from(first)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)?;
                if !base.is_empty() && !base.contains('/') {
                    return Some(base);
                }
            }
        }
    }
    None
}

fn desktop_id_for_executable(exec: &str) -> Option<String> {
    let needle = PathBuf::from(exec)
        .file_name()
        .and_then(|n| n.to_str())?
        .to_string();
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(rest) = text.lines().find_map(|l| l.strip_prefix("Exec=")) {
                let first = rest.split_whitespace().next().unwrap_or("");
                let base_owned = PathBuf::from(first)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string);
                if base_owned.as_deref() == Some(needle.as_str()) {
                    return path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(str::to_string);
                }
            }
        }
    }
    // Fall back to known terminal table.
    for (bin, _) in menu::KNOWN_TERMINALS {
        if *bin == needle {
            return Some(format!("{bin}.desktop"));
        }
    }
    None
}

fn apply_gsettings_terminal(desktop_id: &str) {
    if let Some(exec) = desktop_exec_basename(desktop_id) {
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.default-applications.terminal",
                "exec",
                &exec,
            ])
            .status();
    }
}

fn query_gsettings_terminal_desktop() -> Option<String> {
    let exec = run_stdout(
        "gsettings",
        &[
            "get",
            "org.gnome.desktop.default-applications.terminal",
            "exec",
        ],
    )?;
    let exec = exec.trim().trim_matches('\'').trim_matches('"');
    if exec.is_empty() {
        return None;
    }
    desktop_id_for_executable(exec)
}

fn xdg_settings_get(args: &[&str]) -> Option<String> {
    let text = run_stdout("xdg-settings", args)?;
    let id = text.trim();
    if id.is_empty() || id == "default" {
        return None;
    }
    normalize_desktop_id(id).ok()
}

fn xdg_mime_query(mime: &str) -> Option<String> {
    let text = run_stdout("xdg-mime", &["query", "default", mime])?;
    let id = text.trim();
    if id.is_empty() {
        return None;
    }
    normalize_desktop_id(id).ok()
}

fn run_stdout(bin: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(bin).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn run_checked(bin: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(if err.trim().is_empty() {
            format!("{bin} failed")
        } else {
            err.trim().chars().take(200).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_desktop_id_ok() {
        assert_eq!(normalize_desktop_id("firefox").unwrap(), "firefox.desktop");
        assert_eq!(
            normalize_desktop_id("org.mozilla.firefox.desktop").unwrap(),
            "org.mozilla.firefox.desktop"
        );
        assert!(normalize_desktop_id("../x").is_err());
        assert!(normalize_desktop_id("a/b.desktop").is_err());
    }

    #[test]
    fn role_mime_stable() {
        assert_eq!(DefaultAppRole::Browser.mime(), "x-scheme-handler/http");
        assert_eq!(DefaultAppRole::Pdf.mime(), "application/pdf");
    }
}
