//! Desktop background persisted to `~/.config/metis/wallpaper.json`.
//!
//! Supports three kinds of background: a picture (image file), a solid colour, or
//! a two-stop linear gradient. The settings app writes this file; the compositor
//! reads it on startup and applies changes live via the `ApplyBackground` IPC
//! command.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config_dir, ensure_config_dirs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    #[default]
    Image,
    Solid,
    Gradient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradientDirection {
    /// Top → bottom.
    #[default]
    Vertical,
    /// Bottom → top.
    VerticalReverse,
    /// Left → right.
    Horizontal,
    /// Right → left.
    HorizontalReverse,
    /// Top-left → bottom-right.
    Diagonal,
    /// Top-right → bottom-left.
    DiagonalReverse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperConfig {
    /// Which background style is active.
    #[serde(default)]
    pub kind: BackgroundKind,
    /// Absolute path to the selected picture (used when `kind == Image`).
    #[serde(default)]
    pub path: Option<String>,
    /// Solid background colour (`#rrggbb`), used when `kind == Solid`.
    #[serde(default = "default_solid")]
    pub color: String,
    /// Gradient start colour (`#rrggbb`), used when `kind == Gradient`.
    #[serde(default = "default_grad_start")]
    pub gradient_start: String,
    /// Gradient end colour (`#rrggbb`), used when `kind == Gradient`.
    #[serde(default = "default_grad_end")]
    pub gradient_end: String,
    /// Direction the gradient sweeps.
    #[serde(default)]
    pub gradient_direction: GradientDirection,
    /// Per-output image overrides, keyed by the compositor's output name
    /// (e.g. `metis-0`, `metis-1`). When an output appears here, that display
    /// shows the given picture instead of the global background. Outputs not
    /// listed fall back to the global `kind`/`path` above. Each display is
    /// always cover-cropped to its own resolution, so the same image on two
    /// differently sized monitors still fills each one correctly.
    #[serde(default)]
    pub per_output: HashMap<String, String>,
}

fn default_solid() -> String {
    "#1e1e2e".to_string()
}

fn default_grad_start() -> String {
    "#0ea5e9".to_string()
}

fn default_grad_end() -> String {
    "#7c3aed".to_string()
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            kind: BackgroundKind::default(),
            path: None,
            color: default_solid(),
            gradient_start: default_grad_start(),
            gradient_end: default_grad_end(),
            gradient_direction: GradientDirection::default(),
            per_output: HashMap::new(),
        }
    }
}

pub fn wallpaper_config_path() -> PathBuf {
    config_dir().join("wallpaper.json")
}

pub fn load_wallpaper_config() -> WallpaperConfig {
    let path = wallpaper_config_path();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    WallpaperConfig::default()
}

pub fn save_wallpaper_config(cfg: &WallpaperConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(wallpaper_config_path(), json)
}

/// On-disk RGBA cache for full wallpaper decodes (`~/.cache/metis/wallpaper-rgba/`).
/// Avoids re-decoding multi‑MB PNGs on every Settings click; the compositor and
/// Settings gallery warmer share this format.
const RGBA_CACHE_MAGIC: &[u8; 4] = b"MWP1";

fn cache_root() -> PathBuf {
    directories::ProjectDirs::from("com", "metis", "metis")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache/metis"))
                .unwrap_or_else(|_| PathBuf::from(".cache/metis"))
        })
        .join("wallpaper-rgba")
}

fn wallpaper_rgba_cache_key(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let len = meta.len();
    // Stable, path-independent key so renames that keep inode content still miss
    // correctly when mtime/size change.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= len;
    h = h.wrapping_mul(0x100000001b3);
    h ^= modified;
    Some(format!("{h:016x}"))
}

/// Cache file for a wallpaper source, if the path is readable.
pub fn wallpaper_rgba_cache_path(path: &Path) -> Option<PathBuf> {
    Some(cache_root().join(format!("{}.rgba", wallpaper_rgba_cache_key(path)?)))
}

/// Read a previously decoded RGBA wallpaper (`None` on miss / corruption).
pub fn load_wallpaper_rgba_cache(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let cache = wallpaper_rgba_cache_path(path)?;
    let bytes = std::fs::read(&cache).ok()?;
    if bytes.len() < 12 || &bytes[0..4] != RGBA_CACHE_MAGIC {
        let _ = std::fs::remove_file(&cache);
        return None;
    }
    let w = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let h = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let need = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if bytes.len() != 12 + need || w == 0 || h == 0 {
        let _ = std::fs::remove_file(&cache);
        return None;
    }
    Some((w, h, bytes[12..].to_vec()))
}

/// Persist a decoded RGBA wallpaper for fast compositor apply.
pub fn store_wallpaper_rgba_cache(path: &Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let Some(cache) = wallpaper_rgba_cache_path(path) else {
        return Ok(());
    };
    let need = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "size overflow"))?;
    if rgba.len() != need {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rgba length mismatch",
        ));
    }
    std::fs::create_dir_all(cache_root())?;
    let mut out = Vec::with_capacity(12 + need);
    out.extend_from_slice(RGBA_CACHE_MAGIC);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(rgba);
    let tmp = cache.with_extension("rgba.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, &cache)?;
    Ok(())
}

/// True when a valid RGBA cache entry already exists for `path`.
pub fn wallpaper_rgba_cache_fresh(path: &Path) -> bool {
    wallpaper_rgba_cache_path(path)
        .map(|p| p.is_file())
        .unwrap_or(false)
}

/// Directory where user-imported wallpapers are copied to.
pub fn wallpaper_store_dir() -> PathBuf {
    config_dir().join("wallpapers")
}

/// Image extensions treated as selectable wallpapers.
pub const WALLPAPER_IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Compile-time path to the workspace bundled wallpapers directory.
pub fn bundled_wallpaper_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets/wallpapers")
}

/// Candidate directories holding bundled wallpapers (compile-time bundle plus
/// installed FHS paths and exe-relative fallbacks).
///
/// Prefer packaged FHS dirs. The Cargo `assets/` tree is only consulted when no
/// system wallpaper dir exists — otherwise installed installs that still have
/// the source checkout list every wallpaper twice.
pub fn bundled_wallpaper_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let push = |p: PathBuf, dirs: &mut Vec<PathBuf>| {
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    // Packaged installs (`.deb` / `--install-session`).
    push(PathBuf::from("/usr/share/metis/wallpapers"), &mut dirs);
    push(
        PathBuf::from("/usr/local/share/metis/wallpapers"),
        &mut dirs,
    );
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        for rel in ["../share/metis/wallpapers", "../../share/metis/wallpapers"] {
            push(parent.join(rel), &mut dirs);
        }
    }
    // Dev / uninstalled: workspace assets + exe-relative asset trees.
    if dirs.is_empty() {
        push(bundled_wallpaper_dir(), &mut dirs);
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            for rel in [
                "assets/wallpapers",
                "../assets/wallpapers",
                "../../assets/wallpapers",
                "../../../assets/wallpapers",
            ] {
                push(parent.join(rel), &mut dirs);
            }
        }
    }
    dirs
}

/// Distro / desktop environment wallpaper trees (Ubuntu GNOME, KDE extras, …).
///
/// These are read in place — Metis never copies them into its store unless the
/// user explicitly imports a file.
pub fn system_wallpaper_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let push = |p: PathBuf, dirs: &mut Vec<PathBuf>| {
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    push(PathBuf::from("/usr/share/backgrounds"), &mut dirs);
    push(PathBuf::from("/usr/local/share/backgrounds"), &mut dirs);
    push(PathBuf::from("/usr/share/pixmaps/backgrounds"), &mut dirs);
    // Honour extra data dirs (Flatpak host, custom prefixes).
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for raw in data_dirs.split(':') {
        if raw.is_empty() {
            continue;
        }
        push(PathBuf::from(raw).join("backgrounds"), &mut dirs);
    }
    dirs
}

fn is_wallpaper_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| WALLPAPER_IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Collect image files from `dir` into `out`, skipping paths already in `seen`
/// (by canonical path **or** lowercase basename so FHS + build-tree copies of
/// `default.png` are not both listed).
pub fn collect_wallpaper_images(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    collect_wallpaper_images_depth(dir, out, seen, 0);
}

/// Like [`collect_wallpaper_images`], but also walks immediate subdirectories
/// up to `max_depth` (e.g. Ubuntu's `/usr/share/backgrounds/contest`).
pub fn collect_wallpaper_images_depth(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    max_depth: u32,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found = Vec::new();
    let mut subdirs = Vec::new();
    let mut seen_names: HashSet<String> = seen
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_ascii_lowercase())
        })
        .collect();
    // Also remember basenames already queued in `out` (same run, prior dirs).
    for p in out.iter() {
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            seen_names.insert(n.to_ascii_lowercase());
        }
    }
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if max_depth > 0 {
                subdirs.push(path);
            }
            continue;
        }
        if !path.is_file() || !is_wallpaper_image(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let name_key = name.to_ascii_lowercase();
        if !seen_names.insert(name_key) {
            continue;
        }
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen.insert(canon) {
            continue;
        }
        found.push(path);
    }
    found.sort();
    out.extend(found);
    subdirs.sort();
    for sub in subdirs {
        collect_wallpaper_images_depth(&sub, out, seen, max_depth - 1);
    }
}

/// All bundled wallpaper images, sorted by filename.
pub fn list_bundled_wallpapers() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for dir in bundled_wallpaper_dirs() {
        collect_wallpaper_images(&dir, &mut out, &mut seen);
    }
    out
}

/// First bundled wallpaper (typically `default.png`), if any exist.
pub fn default_wallpaper_path() -> Option<PathBuf> {
    list_bundled_wallpapers().into_iter().next()
}

/// Parse a `#rrggbb` hex colour into an RGB triplet, falling back to black.
pub fn parse_hex_rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6
        && let Ok(v) = u32::from_str_radix(h, 16)
    {
        return [(v >> 16) as u8, (v >> 8) as u8, v as u8];
    }
    [0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_skips_same_basename_from_later_dirs() {
        let root =
            std::env::temp_dir().join(format!("metis-wallpaper-dedupe-{}", std::process::id()));
        let a = root.join("a");
        let b = root.join("b");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("default.png"), b"a").unwrap();
        std::fs::write(b.join("default.png"), b"b").unwrap();
        std::fs::write(b.join("extra.png"), b"c").unwrap();

        let mut out = Vec::new();
        let mut seen = HashSet::new();
        collect_wallpaper_images(&a, &mut out, &mut seen);
        collect_wallpaper_images(&b, &mut out, &mut seen);

        let names: Vec<_> = out
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().any(|n| n == "default.png"));
        assert!(names.iter().any(|n| n == "extra.png"));
        // First dir wins for the duplicate name.
        assert_eq!(out[0].parent().unwrap(), a.as_path());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_depth_walks_one_subdir() {
        let root =
            std::env::temp_dir().join(format!("metis-wallpaper-depth-{}", std::process::id()));
        let contest = root.join("contest");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&contest).unwrap();
        std::fs::write(root.join("top.png"), b"a").unwrap();
        std::fs::write(contest.join("nested.png"), b"b").unwrap();

        let mut out = Vec::new();
        let mut seen = HashSet::new();
        collect_wallpaper_images_depth(&root, &mut out, &mut seen, 1);

        let names: Vec<_> = out
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        assert!(names.iter().any(|n| n == "top.png"));
        assert!(names.iter().any(|n| n == "nested.png"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
