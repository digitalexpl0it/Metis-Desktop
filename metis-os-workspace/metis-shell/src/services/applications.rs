//! Installed-application enumeration and app-menu state for the launcher popover.
//!
//! Apps are discovered through `gio::AppInfo` (the freedesktop `.desktop` index).
//! Launch frequency and pinned apps persist to `menu.json` via `metis-config`.
//! Launching routes through the compositor (`launch_program`) so children inherit
//! the nested Wayland environment and are tracked for cleanup.

use gio::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use metis_config::{load_menu_config, save_menu_config};

/// A single launchable application resolved from a `.desktop` entry.
#[derive(Clone)]
pub struct AppEntry {
    /// Desktop file id, e.g. `firefox.desktop`. Stable key for pinning/frequency.
    pub id: String,
    pub name: String,
    /// Command line with `.desktop` field codes stripped, ready for `launch_program`.
    pub exec: String,
    pub icon: Option<gio::Icon>,
    pub keywords: Vec<String>,
    /// `StartupWMClass` from the desktop entry, used to map a running window's
    /// Wayland `app_id` back to its launcher entry/icon.
    pub wm_class: Option<String>,
    /// Flatpak application id (`X-Flatpak` desktop key), e.g.
    /// `com.valvesoftware.Steam`. Flatpak windows commonly report this as their
    /// Wayland `app_id`, so it is another handle for window→entry matching.
    pub flatpak_id: Option<String>,
    /// Freedesktop Categories= tokens from the `.desktop` file.
    pub categories: Vec<String>,
}

thread_local! {
    static APP_CACHE: RefCell<Option<Vec<AppEntry>>> = const { RefCell::new(None) };
    /// Repaint hooks for widgets that enumerate installed apps (menu, taskbar).
    static REFRESH: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static APP_MONITOR_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Drop the in-process app index so the next read rescans `.desktop` entries.
pub fn invalidate_app_cache() {
    APP_CACHE.with(|cache| *cache.borrow_mut() = None);
}

/// Register a callback invoked when the freedesktop app index changes (install,
/// remove, `.desktop` edit). Each consumer registers its own hook; dead hooks from
/// rebuilt bars are pruned on the next register/fire.
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
    tracing::debug!("desktop app index changed, refreshing app list");
    invalidate_app_cache();
    fire_refresh();
}

/// Watch `GAppInfoMonitor` so newly installed apps appear without restarting the
/// shell. Safe to call more than once; only the first call wires the monitor.
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

/// Enumerate all visible installed applications, sorted alphabetically by name.
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

    let desktop = info.downcast_ref::<gio::DesktopAppInfo>();
    let keywords = desktop
        .map(|desktop| {
            desktop
                .keywords()
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let wm_class = desktop
        .and_then(|desktop| desktop.startup_wm_class())
        .map(|s| s.to_string());
    let flatpak_id = desktop
        .and_then(|desktop| desktop.string("X-Flatpak"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let categories = desktop
        .and_then(|desktop| desktop.categories())
        .map(|s| {
            s.split(';')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(AppEntry {
        id,
        name,
        exec,
        icon: info.icon(),
        keywords,
        wm_class,
        flatpak_id,
        categories,
    })
}

/// Strip freedesktop Exec field codes (`%U`, `%f`, `%i`, ...) so the residual
/// command line can be spawned directly by the compositor.
fn clean_exec(exec: &str) -> String {
    metis_protocol::split_command_line(exec)
        .into_iter()
        .filter(|tok| !(tok.len() == 2 && tok.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Snap ships placeholder `.desktop` stubs (`Exec=/usr/bin/false`) alongside the
/// real launcher — drop them so the menu does not list non-startable entries.
fn is_stub_exec(exec: &str) -> bool {
    matches!(
        exec.split_whitespace().next(),
        Some("false" | "/usr/bin/false" | "/bin/false")
    )
}

/// Apps the user has launched at least once, ordered by descending launch count
/// (ties broken alphabetically). Drives the "Frequent Apps" section.
pub fn frequent_from(apps: &[AppEntry], limit: usize) -> Vec<AppEntry> {
    let counts = load_menu_config().launch_counts;
    let mut scored: Vec<(u32, AppEntry)> = apps
        .iter()
        .filter_map(|e| counts.get(&e.id).copied().map(|c| (c, e.clone())))
        .filter(|(c, _)| *c > 0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

/// Apps the user has launched at least once, ordered by descending launch count
/// (ties broken alphabetically). Drives the "Frequent Apps" section.
#[allow(dead_code)] // convenience wrapper over `frequent_from`
pub fn frequent(limit: usize) -> Vec<AppEntry> {
    frequent_from(&list_apps(), limit)
}

/// Case-insensitive search over app name and keywords, ranked by launch count.
pub fn search_in(apps: &[AppEntry], query: &str) -> Vec<AppEntry> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let counts = load_menu_config().launch_counts;
    let mut matches: Vec<AppEntry> = apps
        .iter()
        .filter(|e| {
            e.name.to_lowercase().contains(&needle)
                || e.keywords
                    .iter()
                    .any(|k| k.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect();
    matches.sort_by(|a, b| {
        let starts_a = a.name.to_lowercase().starts_with(&needle);
        let starts_b = b.name.to_lowercase().starts_with(&needle);
        starts_b
            .cmp(&starts_a)
            .then_with(|| {
                counts
                    .get(&b.id)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&counts.get(&a.id).copied().unwrap_or(0))
            })
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    matches
}

/// Case-insensitive search over app name and keywords, ranked by launch count.
#[allow(dead_code)] // convenience wrapper over `search_in`
pub fn search(query: &str) -> Vec<AppEntry> {
    search_in(&list_apps(), query)
}

/// The user's pinned apps, in saved order, resolved to current `AppEntry`s.
pub fn pinned_entries_from(apps: &[AppEntry]) -> Vec<AppEntry> {
    let pinned = load_menu_config().pinned;
    pinned
        .iter()
        .filter_map(|id| apps.iter().find(|e| &e.id == id).cloned())
        .collect()
}

/// The user's pinned apps, in saved order, resolved to current `AppEntry`s.
#[allow(dead_code)] // convenience wrapper over `pinned_entries_from`
pub fn pinned_entries() -> Vec<AppEntry> {
    pinned_entries_from(&list_apps())
}

/// Main menu category groups (id, translated label key, matching desktop tokens).
/// Special ids: `frequent`, `all`.
pub const MENU_CATEGORY_DEFS: &[(&str, &str, &[&str])] = &[
    ("frequent", "Frequent", &[]),
    ("all", "All", &[]),
    (
        "internet",
        "Internet",
        &["Network", "WebBrowser", "Email", "InstantMessaging"],
    ),
    ("office", "Office", &["Office", "WordProcessor", "Spreadsheet"]),
    ("graphics", "Graphics", &["Graphics", "Photography", "2DGraphics"]),
    (
        "audiovideo",
        "Audio & Video",
        &["AudioVideo", "Audio", "Video", "Player", "Recorder"],
    ),
    (
        "development",
        "Development",
        &["Development", "IDE", "TextEditor"],
    ),
    ("games", "Games", &["Game"]),
    ("education", "Education", &["Education"]),
    ("science", "Science", &["Science"]),
    ("settings", "Settings", &["Settings", "DesktopSettings", "System"]),
    ("utilities", "Utilities", &["Utility", "Accessories", "Core"]),
];

/// Apps matching a category id (`frequent` / `all` / Freedesktop group).
pub fn apps_in_category<'a>(apps: &'a [AppEntry], category_id: &str, query: &str) -> Vec<AppEntry> {
    let base: Vec<AppEntry> = match category_id {
        "frequent" => frequent_from(apps, FREQUENT_CATEGORY_LIMIT),
        "all" => apps.to_vec(),
        id => {
            let tokens = MENU_CATEGORY_DEFS
                .iter()
                .find(|(cid, _, _)| *cid == id)
                .map(|(_, _, t)| *t)
                .unwrap_or(&[]);
            apps.iter()
                .filter(|e| {
                    tokens.iter().any(|tok| {
                        e.categories
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(tok))
                    })
                })
                .cloned()
                .collect()
        }
    };
    if query.trim().is_empty() {
        base
    } else {
        search_in(&base, query)
    }
}

const FREQUENT_CATEGORY_LIMIT: usize = 24;

/// Toggle an app's pinned state, persisting the change. Returns the new state.
pub fn toggle_pin(id: &str) -> bool {
    let mut cfg = load_menu_config();
    let now_pinned = if let Some(pos) = cfg.pinned.iter().position(|p| p == id) {
        cfg.pinned.remove(pos);
        false
    } else {
        cfg.pinned.push(id.to_string());
        true
    };
    if let Err(err) = save_menu_config(&cfg) {
        tracing::warn!(%err, "failed to persist menu.json after pin toggle");
    }
    // Apps desktop widgets watch menu.json in the widgets process.
    now_pinned
}

/// Fallback icon name used when a window's `app_id` matches no installed app.
pub const FALLBACK_ICON_NAME: &str = "application-x-executable-symbolic";

/// Paint the best available icon for `entry` onto `image` at `size` pixels.
pub fn set_app_icon(image: &gtk::Image, entry: &AppEntry, size: i32) {
    image.set_pixel_size(size);
    if let Some(icon) = &entry.icon {
        if paint_gicon(image, icon, size) {
            return;
        }
    }
    if entry
        .id
        .strip_suffix(".desktop")
        .unwrap_or(&entry.id)
        .eq_ignore_ascii_case("metis-settings")
        && paint_file_icon(image, metis_settings_icon_path())
    {
        return;
    }
    image.set_from_icon_name(Some(FALLBACK_ICON_NAME));
}

fn paint_gicon(image: &gtk::Image, icon: &gio::Icon, size: i32) -> bool {
    if let Some(file_icon) = icon.downcast_ref::<gio::FileIcon>() {
        if let Some(path) = file_icon.file().path() {
            return paint_file_icon(image, Some(path));
        }
    }
    if let Some(themed) = icon.downcast_ref::<gio::ThemedIcon>() {
        let names: Vec<String> = themed
            .names()
            .iter()
            .flat_map(|name| {
                let name = name.as_str();
                if name.ends_with("-symbolic") {
                    vec![name.to_string()]
                } else {
                    vec![name.to_string(), format!("{name}-symbolic")]
                }
            })
            .collect();
        if paint_themed_names(image, &names, size) {
            return true;
        }
    }
    image.set_from_gicon(icon);
    true
}

fn paint_themed_names(image: &gtk::Image, names: &[String], size: i32) -> bool {
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let theme = gtk::IconTheme::for_display(&display);
    for name in names {
        if !theme.has_icon(name) {
            continue;
        }
        let paintable = theme.lookup_icon(
            name,
            &[],
            size,
            1,
            gtk::TextDirection::Ltr,
            gtk::IconLookupFlags::empty(),
        );
        image.set_from_paintable(Some(&paintable));
        return true;
    }
    false
}

fn paint_file_icon(image: &gtk::Image, path: Option<std::path::PathBuf>) -> bool {
    let Some(path) = path else {
        return false;
    };
    match gtk::gdk::Texture::from_filename(&path) {
        Ok(texture) => {
            image.set_from_paintable(Some(&texture));
            true
        }
        Err(err) => {
            tracing::debug!(%err, path = %path.display(), "failed to load app icon file");
            false
        }
    }
}

/// Resolve a running window's Wayland `app_id` to its launcher entry. The
/// Wayland `app_id` is usually reverse-DNS (e.g. `org.gnome.Calculator`), which
/// matches a desktop file basename, but some apps report their `StartupWMClass`
/// instead — both are matched case-insensitively. Falls back to a direct
/// `.desktop` lookup so `OnlyShowIn=GNOME` apps still resolve under Metis.
pub fn resolve_entry_for_app_id(app_id: &str) -> Option<AppEntry> {
    resolve_entry_for_id(app_id)
}

/// Resolve a window's `app_id` to a displayable icon, falling back to a generic
/// application glyph when no match (or no `app_id`) is found.
pub fn resolve_icon_for_app_id(app_id: Option<&str>) -> gio::Icon {
    app_id
        .and_then(resolve_entry_for_app_id)
        .and_then(|e| e.icon)
        .or_else(|| {
            app_id.and_then(|id| {
                id.eq_ignore_ascii_case("metis-settings")
                    .then(metis_settings_icon_path)
                    .flatten()
                    .map(|path| gio::FileIcon::new(&gio::File::for_path(path)).upcast())
            })
        })
        .unwrap_or_else(|| gio::ThemedIcon::new(FALLBACK_ICON_NAME).upcast())
}

fn metis_settings_icon_path() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        candidates.push(
            std::path::PathBuf::from(data_home)
                .join("icons/hicolor/256x256/apps/metis-settings.png"),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            std::path::PathBuf::from(home)
                .join(".local/share/icons/hicolor/256x256/apps/metis-settings.png"),
        );
    }
    candidates.push(std::path::PathBuf::from(
        "/usr/share/icons/hicolor/256x256/apps/metis-settings.png",
    ));
    candidates.push(std::path::PathBuf::from(
        "/usr/local/share/icons/hicolor/256x256/apps/metis-settings.png",
    ));
    candidates.into_iter().find(|path| path.is_file())
}

fn matches_app_id(entry: &AppEntry, needle: &str) -> bool {
    let base = entry
        .id
        .strip_suffix(".desktop")
        .unwrap_or(&entry.id)
        .to_lowercase();
    base == needle
        || entry.id.to_lowercase() == needle
        || entry
            .wm_class
            .as_deref()
            .is_some_and(|w| w.to_lowercase() == needle)
        || entry
            .flatpak_id
            .as_deref()
            .is_some_and(|f| f.to_lowercase() == needle)
}

/// Desktop-file id variants to try with `DesktopAppInfo::new`.
fn desktop_id_candidates(id: &str) -> Vec<String> {
    let raw = id.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = vec![raw.to_string()];
    if !raw.ends_with(".desktop") {
        out.push(format!("{raw}.desktop"));
    }
    let base = raw.strip_suffix(".desktop").unwrap_or(raw);
    if base != raw {
        out.push(base.to_string());
    }
    out
}

/// Wayland `app_id` values that do not match the `.desktop` basename.
///
/// GNOME Terminal's process reports `gnome-terminal-server`; pinning that string
/// used to leave a dock icon that could not resolve an Exec line.
fn app_id_launch_aliases(id: &str) -> Vec<String> {
    let lower = id.trim().to_lowercase();
    let mut out = Vec::new();
    match lower.as_str() {
        "gnome-terminal-server" | "gnome-terminal" | "gnome-terminal.desktop" => {
            out.push("org.gnome.Terminal.desktop".into());
            out.push("org.gnome.Terminal".into());
        }
        "gnome-terminal-preferences" | "org.gnome.terminal.preferences" => {
            out.push("org.gnome.Terminal.Preferences.desktop".into());
        }
        _ => {}
    }
    if let Some(base) = lower.strip_suffix("-server") {
        if !base.is_empty() {
            out.push(format!("{base}.desktop"));
            out.push(base.to_string());
        }
    }
    out
}

fn entry_from_desktop_candidate(candidate: &str) -> Option<AppEntry> {
    let info = gio::DesktopAppInfo::new(candidate)?;
    entry_from_info(info.upcast())
}

/// Resolve a pin / Wayland app id to a launchable entry.
///
/// Tries the visible app index first, then `DesktopAppInfo::new` so apps that
/// declare `OnlyShowIn=GNOME` (e.g. GNOME Terminal) still resolve under Metis
/// even when `AppInfo::all().should_show()` hides them from the menu. Also maps
/// known Wayland `app_id` aliases (`gnome-terminal-server` → Terminal).
pub fn resolve_entry_for_id(id: &str) -> Option<AppEntry> {
    let needle = id.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    if let Some(entry) = list_apps().into_iter().find(|e| matches_app_id(e, &needle)) {
        return Some(entry);
    }
    for candidate in desktop_id_candidates(id) {
        if let Some(entry) = entry_from_desktop_candidate(&candidate) {
            return Some(entry);
        }
    }
    for alias in app_id_launch_aliases(&needle) {
        if let Some(entry) = list_apps()
            .into_iter()
            .find(|e| matches_app_id(e, &alias.to_lowercase()))
        {
            return Some(entry);
        }
        for candidate in desktop_id_candidates(&alias) {
            if let Some(entry) = entry_from_desktop_candidate(&candidate) {
                return Some(entry);
            }
        }
    }
    None
}

/// Launch by desktop / pin id (resolves Exec even for OnlyShowIn=GNOME apps).
pub fn launch_id(id: &str) {
    let Some(entry) = resolve_entry_for_id(id) else {
        tracing::warn!(id, "no desktop entry to launch for pin/app id");
        return;
    };
    launch(&entry);
}

/// Force another instance (taskbar "New window") — bypasses the pending gate.
pub fn launch_id_new_window(id: &str) {
    let Some(entry) = resolve_entry_for_id(id) else {
        tracing::warn!(id, "no desktop entry to launch for pin/app id");
        return;
    };
    launch_new_window(&entry);
}

/// Record a launch (bumping its frequency) and spawn the app via the compositor.
/// Duplicate clicks while the app is still starting are ignored.
pub fn launch(entry: &AppEntry) {
    if !crate::services::launch_pending::try_begin_launch(entry) {
        return;
    }
    spawn_tracked(entry);
}

/// Spawn without single-flight suppression (explicit "New window").
pub fn launch_new_window(entry: &AppEntry) {
    spawn_tracked(entry);
}

fn spawn_tracked(entry: &AppEntry) {
    let mut cfg = load_menu_config();
    *cfg.launch_counts.entry(entry.id.clone()).or_insert(0) += 1;
    if let Err(err) = save_menu_config(&cfg) {
        tracing::warn!(%err, "failed to persist menu.json after launch");
    }
    if let Err(err) = crate::compositor::launch_program(&entry.exec) {
        tracing::warn!(%err, exec = %entry.exec, "failed to launch application");
        crate::services::launch_pending::clear_id(&entry.id);
    }
}

/// Return true if `program` is an executable on `$PATH`.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
            && std::fs::metadata(&candidate)
                .map(|m| {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                })
                .unwrap_or(false)
    })
}

/// Resolve the command that opens Steam Big Picture / gamepad UI, or `None` when
/// Steam is not installed. Prefers a native `steam` on `$PATH`; otherwise falls
/// back to a discovered Flatpak Steam (`com.valvesoftware.Steam`). Callers gate
/// UI (e.g. the Big Picture launcher) on this returning `Some`, so non-gaming
/// setups are unaffected.
pub fn steam_big_picture_command() -> Option<String> {
    if on_path("steam") {
        return Some("steam -gamepadui".to_string());
    }
    metis_gaming::flatpak_steam_launch_command("-gamepadui")
}
