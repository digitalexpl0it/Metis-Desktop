//! Installed-application enumeration for Settings (freedesktop `.desktop` index).
//! Mirrors the shell's app list closely enough for decoration override matching
//! via `StartupWMClass` / Flatpak id / desktop basename.

use std::cell::RefCell;
use std::rc::Rc;

use gio::prelude::*;

/// A launchable application resolved from a `.desktop` entry.
#[derive(Clone)]
pub struct AppEntry {
    /// Desktop file id, e.g. `firefox.desktop`.
    pub id: String,
    pub name: String,
    pub icon: Option<gio::Icon>,
    /// Themmed icon name from the desktop file (when not a path), e.g. `org.xfce.thunar`.
    pub icon_name: Option<String>,
    /// First token of the cleaned Exec= line (binary basename).
    pub exec_bin: Option<String>,
    /// `StartupWMClass` from the desktop entry.
    pub wm_class: Option<String>,
    /// Flatpak application id (`X-Flatpak`).
    pub flatpak_id: Option<String>,
}

impl AppEntry {
    /// Candidate Wayland/X11 `app_id` keys for writing `decorations.json` overrides.
    ///
    /// Includes desktop basename, Exec binary, StartupWMClass, Flatpak id, and
    /// icon name (plus its last dotted component). Needed because some desktop
    /// files (e.g. File Manager Settings → `thunar-settings`) open a dialog owned
    /// by another process whose live `app_id` / WM_CLASS is `Thunar` /
    /// `org.xfce.Thunar`, not the desktop basename.
    pub fn decoration_candidates(&self) -> Vec<String> {
        let mut ids = Vec::new();
        let push = |ids: &mut Vec<String>, raw: &str| {
            let k = raw.trim().to_ascii_lowercase();
            if !k.is_empty() {
                ids.push(k);
            }
        };

        if let Some(wm) = &self.wm_class {
            push(&mut ids, wm);
        }
        if let Some(fp) = &self.flatpak_id {
            push(&mut ids, fp);
        }
        let base = self.id.trim_end_matches(".desktop").trim();
        push(&mut ids, base);
        if let Some(bin) = &self.exec_bin {
            // Skip wrapper executables that never appear as window app_ids.
            let b = bin.to_ascii_lowercase();
            if !matches!(
                b.as_str(),
                "env" | "flatpak" | "snap" | "sh" | "bash" | "python3" | "python"
            ) {
                push(&mut ids, bin);
            }
        }
        // Only reverse-DNS icon names make useful app_id candidates. Generic
        // theme icons (`package-x-generic`, `preferences-desktop-locale`, `0`)
        // polluted decorations.json when every icon token was stored.
        if let Some(icon) = &self.icon_name
            && looks_like_app_id_token(icon)
        {
            push(&mut ids, icon);
        }

        ids.sort();
        ids.dedup();
        ids
    }
}

/// True for reverse-DNS-ish tokens safe to treat as Wayland `app_id` aliases.
fn looks_like_app_id_token(raw: &str) -> bool {
    let s = raw.trim();
    if s.is_empty() || s.contains('/') {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    // Reject themed FreeDesktop icon names and numeric junk.
    if lower.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    const GENERIC: &[&str] = &[
        "application",
        "application-x-executable",
        "package-x-generic",
        "preferences-desktop",
        "preferences-system",
        "multimedia-volume-control",
        "printer",
        "utilities-terminal",
        "system-file-manager",
        "image-x-generic",
        "text-x-generic",
        "folder",
        "desktop",
        "emblem",
    ];
    if GENERIC
        .iter()
        .any(|g| lower == *g || lower.starts_with(&format!("{g}-")))
    {
        return false;
    }
    // Prefer reverse-DNS (`org.xfce.thunar`), hyphenated ids (`transmission-gtk`),
    // or plain app names used as icons/WM class (`transmission`, `shotwell`).
    lower.contains('.')
        || lower.contains('-')
        || (lower.len() >= 4 && lower.chars().all(|c| c.is_ascii_alphanumeric()))
}

thread_local! {
    static APP_CACHE: RefCell<Option<Vec<AppEntry>>> = const { RefCell::new(None) };
    static REFRESH: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static APP_MONITOR_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn invalidate_app_cache() {
    APP_CACHE.with(|cache| *cache.borrow_mut() = None);
}

pub fn register_refresh(cb: Rc<dyn Fn()>) {
    REFRESH.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.push(Rc::downgrade(&cb));
    });
}

fn fire_refresh() {
    let callbacks: Vec<Rc<dyn Fn()>> = REFRESH.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.iter().filter_map(std::rc::Weak::upgrade).collect()
    });
    for cb in callbacks {
        cb();
    }
}

fn on_app_index_changed() {
    invalidate_app_cache();
    fire_refresh();
}

/// Watch `GAppInfoMonitor` so newly installed apps appear without restarting.
pub fn watch_app_index() {
    APP_MONITOR_ARMED.with(|armed| {
        if armed.get() {
            return;
        }
        armed.set(true);
        let monitor = gio::AppInfoMonitor::get();
        monitor.connect_changed(|_| on_app_index_changed());
    });
}

pub fn list_apps() -> Vec<AppEntry> {
    APP_CACHE.with(|cache| {
        if let Some(entries) = cache.borrow().as_ref() {
            return entries.clone();
        }
        let entries = list_apps_uncached();
        *cache.borrow_mut() = Some(entries.clone());
        entries
    })
}

fn list_apps_uncached() -> Vec<AppEntry> {
    let mut entries: Vec<AppEntry> = gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(entry_from_info)
        .collect();
    entries.sort_by_key(|a| a.name.to_lowercase());
    entries
}

fn entry_from_info(info: gio::AppInfo) -> Option<AppEntry> {
    let id = info.id()?.to_string();
    let name = info.name().to_string();
    if name.is_empty() {
        return None;
    }
    let exec = info
        .commandline()
        .map(|cmd| clean_exec(&cmd.to_string_lossy()))
        .filter(|s| !s.is_empty())?;
    if is_stub_exec(&exec) {
        return None;
    }

    let desktop = info.downcast_ref::<gio_unix::DesktopAppInfo>();
    let wm_class = desktop
        .and_then(|desktop| desktop.startup_wm_class())
        .map(|s| s.to_string());
    let flatpak_id = desktop
        .and_then(|desktop| desktop.string("X-Flatpak"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let icon_name = desktop
        .and_then(|desktop| desktop.string("Icon"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains('/'));
    let exec_bin = exec
        .split_whitespace()
        .next()
        .map(|tok| {
            std::path::Path::new(tok)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(tok)
                .to_string()
        })
        .filter(|s| !s.is_empty());

    Some(AppEntry {
        id,
        name,
        icon: info.icon(),
        icon_name,
        exec_bin,
        wm_class,
        flatpak_id,
    })
}

fn clean_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|tok| !(tok.len() == 2 && tok.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_stub_exec(exec: &str) -> bool {
    matches!(
        exec.split_whitespace().next(),
        Some("false" | "/usr/bin/false" | "/bin/false")
    )
}
