//! Freedesktop icon theme preference (`icons.json`) plus brand / per-app overrides
//! and path-safe pack install into `~/.local/share/icons`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::widget_ext::is_safe_icon_name;

/// Max unpacked size for a user-installed icon pack (~256 MiB).
const MAX_EXTRACT_BYTES: u64 = 256 * 1024 * 1024;
/// Max archive members.
const MAX_ARCHIVE_ENTRIES: usize = 50_000;

/// Persisted icon / branding preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IconsConfig {
    /// Freedesktop icon theme directory name (e.g. `Papirus-Dark`).
    /// `None` / empty = leave the session default alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Optional path to a custom edge-bar launcher brand image (PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_launcher: Option<String>,
    /// Optional path to a custom Metis Settings app icon (PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_settings: Option<String>,
    /// Per desktop-id icon override: themed name or absolute path under the
    /// user's home / XDG data dirs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_icons: BTreeMap<String, String>,
}

/// A discovered Freedesktop icon theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconThemeInfo {
    /// Directory name used as `gtk-icon-theme-name` (e.g. `Papirus-Dark`).
    pub id: String,
    /// Display name from `index.theme` `Name=` (falls back to `id`).
    pub name: String,
    /// Absolute path to the theme root.
    pub path: PathBuf,
    /// True when the theme lives under `~/.local/share/icons` (removable).
    pub user_installed: bool,
}

pub fn icons_config_path() -> PathBuf {
    super::config_dir().join("icons.json")
}

pub fn load_icons_config() -> IconsConfig {
    let path = icons_config_path();
    if path.exists()
        && let Ok(text) = fs::read_to_string(&path)
        && let Ok(mut cfg) = serde_json::from_str::<IconsConfig>(&text)
    {
        normalize_icons_config(&mut cfg);
        return cfg;
    }
    IconsConfig::default()
}

pub fn save_icons_config(cfg: &IconsConfig) -> io::Result<()> {
    let mut clean = cfg.clone();
    normalize_icons_config(&mut clean);
    super::ensure_config_dirs()?;
    crate::persist::write_json_atomic(&icons_config_path(), &clean)
}

fn normalize_icons_config(cfg: &mut IconsConfig) {
    if let Some(theme) = cfg.theme.take() {
        let t = theme.trim();
        cfg.theme = if t.is_empty() || !is_safe_icon_theme_id(t) {
            None
        } else {
            Some(t.to_string())
        };
    }
    cfg.brand_launcher = cfg
        .brand_launcher
        .take()
        .and_then(|p| sanitize_user_icon_path(&p));
    cfg.brand_settings = cfg
        .brand_settings
        .take()
        .and_then(|p| sanitize_user_icon_path(&p));
    cfg.app_icons.retain(|id, icon| {
        is_safe_desktop_id(id)
            && (is_safe_icon_name(icon) || sanitize_user_icon_path(icon).is_some())
    });
    for icon in cfg.app_icons.values_mut() {
        if let Some(path) = sanitize_user_icon_path(icon) {
            *icon = path;
        } else {
            *icon = icon.trim().to_string();
        }
    }
}

/// Theme directory / gsettings id (stricter than symbolic glyph names).
pub fn is_safe_icon_theme_id(name: &str) -> bool {
    is_safe_icon_name(name)
}

fn is_safe_desktop_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id.len() > 256 {
        return false;
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+')
}

/// Allow absolute icon paths only under the user's home or XDG data dirs.
pub fn sanitize_user_icon_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 1024 {
        return None;
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return None;
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let allowed_roots = user_icon_path_roots();
    let canon = path.canonicalize().ok().unwrap_or(path.clone());
    if allowed_roots.iter().any(|root| canon.starts_with(root)) {
        Some(canon.to_string_lossy().into_owned())
    } else {
        None
    }
}

fn user_icon_path_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(xdg));
    }
    if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(cfg));
    }
    roots.push(super::config_dir());
    roots
}

/// Scan system + user icon directories for valid Freedesktop themes.
pub fn discover_icon_themes() -> Vec<IconThemeInfo> {
    let mut themes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (dir, user) in icon_search_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let index = path.join("index.theme");
            if !index.is_file() {
                continue;
            }
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !is_safe_icon_theme_id(&id) || !seen.insert(id.clone()) {
                continue;
            }
            // Skip cursor-only packs (DMZ, etc.) — they share index.theme but
            // have no application icons, so Metis falls back to Adwaita holes.
            if !looks_like_application_icon_theme(&path, &index) {
                continue;
            }
            let name = parse_index_theme_name(&index).unwrap_or_else(|| id.clone());
            themes.push(IconThemeInfo {
                id,
                name,
                path,
                user_installed: user,
            });
        }
    }
    themes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    themes
}

fn icon_search_dirs() -> Vec<(PathBuf, bool)> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push((PathBuf::from(home).join(".local/share/icons"), true));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg).join("icons");
        if !dirs.iter().any(|(d, _)| d == &p) {
            dirs.push((p, true));
        }
    }
    dirs.push((PathBuf::from("/usr/local/share/icons"), false));
    dirs.push((PathBuf::from("/usr/share/icons"), false));
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for part in data_dirs.split(':').filter(|s| !s.is_empty()) {
            let p = PathBuf::from(part).join("icons");
            if !dirs.iter().any(|(d, _)| d == &p) {
                dirs.push((p, false));
            }
        }
    }
    dirs
}

fn parse_index_theme_name(index: &Path) -> Option<String> {
    let text = fs::read_to_string(index).ok()?;
    let mut in_icon_theme = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_icon_theme = line.eq_ignore_ascii_case("[Icon Theme]");
            continue;
        }
        if !in_icon_theme {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Name=") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.chars().filter(|c| !c.is_control()).take(128).collect());
            }
        }
    }
    None
}

/// True when `path` looks like an application/icon theme (not cursors-only).
fn looks_like_application_icon_theme(path: &Path, index: &Path) -> bool {
    let text = fs::read_to_string(index).unwrap_or_default();
    let lower = text.to_ascii_lowercase();
    // Explicit cursor theme sections / comments.
    if lower.contains("[cursor theme]") {
        return false;
    }
    if lower.contains("cursor theme")
        && !lower.contains("directories=")
        && !has_icon_size_dirs(path)
    {
        return false;
    }
    if let Some(dirs) = parse_index_directories(&text) {
        let has_non_cursor = dirs.iter().any(|d| {
            let d = d.trim();
            !d.is_empty() && !d.eq_ignore_ascii_case("cursors")
        });
        if has_non_cursor {
            return true;
        }
        if !dirs.is_empty() {
            // Directories= only lists cursors.
            return false;
        }
    }
    has_icon_size_dirs(path)
}

fn parse_index_directories(text: &str) -> Option<Vec<String>> {
    let mut in_icon_theme = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_icon_theme = line.eq_ignore_ascii_case("[Icon Theme]");
            continue;
        }
        if !in_icon_theme {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Directories=") {
            return Some(
                rest.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            );
        }
    }
    None
}

fn has_icon_size_dirs(path: &Path) -> bool {
    for name in [
        "scalable", "symbolic", "16x16", "22x22", "24x24", "32x32", "48x48", "64x64",
    ] {
        if path.join(name).is_dir() {
            return true;
        }
    }
    false
}

/// Apply icon theme to session gsettings (non-Metis GTK apps).
pub fn apply_session_icon_theme(theme: &str) {
    let theme = theme.trim();
    if theme.is_empty() || !is_safe_icon_theme_id(theme) {
        return;
    }
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "icon-theme", theme])
        .status();
}

/// Sync gsettings from `icons.json` when a theme is configured.
pub fn sync_session_icon_theme_from_config() {
    if let Some(theme) = load_icons_config().theme {
        apply_session_icon_theme(&theme);
    }
}

/// Resolved brand launcher path if configured and present on disk.
pub fn resolve_brand_launcher_path() -> Option<PathBuf> {
    load_icons_config()
        .brand_launcher
        .as_deref()
        .and_then(sanitize_user_icon_path)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Resolved Settings brand icon path if configured and present.
pub fn resolve_brand_settings_path() -> Option<PathBuf> {
    load_icons_config()
        .brand_settings
        .as_deref()
        .and_then(sanitize_user_icon_path)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Per-app override: themed name or absolute file path.
pub fn resolve_app_icon_override(desktop_id: &str) -> Option<String> {
    let cfg = load_icons_config();
    let key = desktop_id.trim();
    let bare = key.strip_suffix(".desktop").unwrap_or(key);
    cfg.app_icons
        .get(key)
        .or_else(|| cfg.app_icons.get(bare))
        .cloned()
        .or_else(|| {
            let with = format!("{bare}.desktop");
            cfg.app_icons.get(&with).cloned()
        })
}

pub fn user_icons_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("icons");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/icons")
}

/// Install a zip/tar(.gz) icon pack into `~/.local/share/icons/<Name>/`.
pub fn install_icon_theme_archive(archive: &Path) -> Result<IconThemeInfo, String> {
    if !archive.is_file() {
        return Err("Archive not found".into());
    }
    let ext = archive
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let staging = std::env::temp_dir().join(format!(
        "metis-icon-install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| {
        match ext.as_str() {
            "zip" => extract_zip(archive, &staging)?,
            "gz" | "tgz" => extract_tar_gz(archive, &staging)?,
            "tar" => extract_tar(archive, &staging)?,
            _ => {
                // `.tar.gz` often reports extension `gz` already; also try by name.
                let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
                    extract_tar_gz(archive, &staging)?;
                } else if name.ends_with(".tar") {
                    extract_tar(archive, &staging)?;
                } else {
                    return Err("Unsupported archive (use .zip, .tar, or .tar.gz)".into());
                }
            }
        }
        let theme_root = find_theme_root(&staging)?;
        let id = theme_root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid theme directory name".to_string())?
            .to_string();
        if !is_safe_icon_theme_id(&id) {
            return Err("Theme directory name is not a valid icon theme id".into());
        }
        let dest_parent = user_icons_dir();
        fs::create_dir_all(&dest_parent).map_err(|e| e.to_string())?;
        let dest = dest_parent.join(&id);
        if dest.exists() {
            return Err(format!("Theme “{id}” is already installed"));
        }
        // Move staging theme root into place (rename; fallback copy).
        if fs::rename(&theme_root, &dest).is_err() {
            copy_dir_recursive(&theme_root, &dest)?;
        }
        let name = parse_index_theme_name(&dest.join("index.theme")).unwrap_or_else(|| id.clone());
        Ok(IconThemeInfo {
            id,
            name,
            path: dest,
            user_installed: true,
        })
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Remove a **user-installed** icon theme. Never touches `/usr/share/icons`.
pub fn remove_user_icon_theme(theme_id: &str) -> Result<(), String> {
    if !is_safe_icon_theme_id(theme_id) {
        return Err("Invalid theme id".into());
    }
    let dest = user_icons_dir().join(theme_id);
    let canon = dest
        .canonicalize()
        .map_err(|_| "Theme not found".to_string())?;
    let user_root = user_icons_dir()
        .canonicalize()
        .unwrap_or_else(|_| user_icons_dir());
    if !canon.starts_with(&user_root) {
        return Err("Refusing to remove a system icon theme".into());
    }
    if !canon.join("index.theme").is_file() {
        return Err("Not a valid icon theme directory".into());
    }
    fs::remove_dir_all(&canon).map_err(|e| e.to_string())?;
    // If the removed theme was selected, clear the preference.
    let mut cfg = load_icons_config();
    if cfg.theme.as_deref() == Some(theme_id) {
        cfg.theme = None;
        let _ = save_icons_config(&cfg);
    }
    Ok(())
}

fn find_theme_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join("index.theme").is_file() {
        return Ok(staging.to_path_buf());
    }
    let mut found = Vec::new();
    collect_theme_roots(staging, &mut found, 0)?;
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err("Archive has no index.theme".into()),
        _ => Err("Archive contains multiple icon themes; extract a single theme pack".into()),
    }
}

fn collect_theme_roots(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), String> {
    if depth > 6 {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("index.theme").is_file() {
            out.push(path);
        } else {
            collect_theme_roots(&path, out, depth + 1)?;
        }
    }
    Ok(())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err("Archive has too many entries".into());
    }
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // zip 9: never materialise archive symlinks (RUSTSEC-2026-0368 family).
        if entry.is_symlink() {
            continue;
        }
        let name = entry.name().map_err(|e| e.to_string())?;
        let Some(rel) = sanitized_archive_relpath(name.as_ref()) else {
            continue;
        };
        let out_path = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = fs::File::create(&out_path).map_err(|e| e.to_string())?;
        let written = io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        total = total.saturating_add(written);
        if total > MAX_EXTRACT_BYTES {
            return Err("Archive expands beyond the size limit".into());
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    let dec = flate2::read::GzDecoder::new(file);
    extract_tar_reader(dec, dest)
}

fn extract_tar(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    extract_tar_reader(file, dest)
}

fn extract_tar_reader<R: Read>(reader: R, dest: &Path) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err("Archive has too many entries".into());
        }
        let path = entry.path().map_err(|e| e.to_string())?;
        let Some(rel) = sanitized_archive_relpath(&path.to_string_lossy()) else {
            continue;
        };
        let out_path = dest.join(&rel);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = fs::File::create(&out_path).map_err(|e| e.to_string())?;
        let written = io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        total = total.saturating_add(written);
        if total > MAX_EXTRACT_BYTES {
            return Err("Archive expands beyond the size limit".into());
        }
    }
    Ok(())
}

/// Reject absolute paths, `..`, and Windows drive tricks.
fn sanitized_archive_relpath(name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.contains('\0') || name.starts_with('/') {
        return None;
    }
    // Zip often prefixes `./`; strip only that, not every leading slash (already rejected).
    let name = name.strip_prefix("./").unwrap_or(name.as_str());
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    let path = Path::new(name);
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_id_validation() {
        assert!(is_safe_icon_theme_id("Papirus-Dark"));
        assert!(is_safe_icon_theme_id("breeze"));
        assert!(!is_safe_icon_theme_id("../evil"));
        assert!(!is_safe_icon_theme_id("a/b"));
        assert!(!is_safe_icon_theme_id(""));
    }

    #[test]
    fn archive_path_sanitizer() {
        assert_eq!(
            sanitized_archive_relpath("Papirus/index.theme"),
            Some(PathBuf::from("Papirus/index.theme"))
        );
        assert!(sanitized_archive_relpath("../etc/passwd").is_none());
        assert!(sanitized_archive_relpath("/etc/passwd").is_none());
        assert!(sanitized_archive_relpath("foo/../../x").is_none());
    }

    #[test]
    fn parse_index_name() {
        let dir = tempfile_dir();
        let index = dir.join("index.theme");
        fs::write(
            &index,
            "[Icon Theme]\nName=My Pack\nComment=test\nInherits=hicolor\n",
        )
        .unwrap();
        assert_eq!(parse_index_theme_name(&index).as_deref(), Some("My Pack"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metis-icons-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
