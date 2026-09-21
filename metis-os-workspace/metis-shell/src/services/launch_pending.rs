//! Single-flight app launches: suppress duplicate starts while an app is still
//! mapping its first window, with dock/toast feedback hooks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::applications::AppEntry;

/// Ignore further launches for this long if no matching window appears.
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(8);
/// Show a "Starting…" toast only if the app is still pending after this delay.
pub const TOAST_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct PendingLaunch {
    /// Lowercased identity keys used to match Wayland `app_id` / StartupWMClass.
    pub keys: Vec<String>,
    pub started: Instant,
}

/// Pure pending map (unit-tested without GLib).
#[derive(Debug, Default)]
pub struct PendingMap {
    by_id: HashMap<String, PendingLaunch>,
}

impl PendingMap {
    pub fn is_pending(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn pending_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.by_id.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Insert a pending launch. Returns `false` if `id` is already pending.
    pub fn try_begin(&mut self, id: &str, keys: Vec<String>) -> bool {
        if self.by_id.contains_key(id) {
            return false;
        }
        self.by_id.insert(
            id.to_string(),
            PendingLaunch {
                keys,
                started: Instant::now(),
            },
        );
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.by_id.remove(id).is_some()
    }

    /// Clear any pending launch whose keys match `app_id`. Returns cleared desktop ids.
    pub fn clear_for_window(&mut self, app_id: &str) -> Vec<String> {
        let needle = normalize_key(app_id);
        if needle.is_empty() {
            return Vec::new();
        }
        let mut cleared = Vec::new();
        self.by_id.retain(|id, pending| {
            if keys_match(&pending.keys, &needle) {
                cleared.push(id.clone());
                false
            } else {
                true
            }
        });
        cleared
    }

    /// Drop entries older than `timeout`. Returns cleared desktop ids.
    pub fn clear_expired(&mut self, timeout: Duration, now: Instant) -> Vec<String> {
        let mut cleared = Vec::new();
        self.by_id.retain(|id, pending| {
            if now.duration_since(pending.started) >= timeout {
                cleared.push(id.clone());
                false
            } else {
                true
            }
        });
        cleared
    }
}

/// Build match keys for a desktop entry (lowercased).
pub fn match_keys_for_entry(entry: &AppEntry) -> Vec<String> {
    let mut keys = Vec::new();
    push_key(&mut keys, &entry.id);
    if let Some(base) = entry.id.strip_suffix(".desktop") {
        push_key(&mut keys, base);
        if let Some(last) = base.rsplit('.').next() {
            push_key(&mut keys, last);
        }
    }
    if let Some(wm) = entry.wm_class.as_deref() {
        push_key(&mut keys, wm);
    }
    if let Some(fp) = entry.flatpak_id.as_deref() {
        push_key(&mut keys, fp);
    }
    keys
}

fn normalize_key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn push_key(keys: &mut Vec<String>, raw: &str) {
    let k = normalize_key(raw);
    if !k.is_empty() && !keys.contains(&k) {
        keys.push(k);
    }
}

fn keys_match(keys: &[String], needle: &str) -> bool {
    keys.iter().any(|k| k == needle)
}

thread_local! {
    static STORE: RefCell<PendingMap> = RefCell::new(PendingMap::default());
    static LISTENERS: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static REFRESH_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

/// Register a callback when the pending set changes (dock pulse).
pub fn register_refresh(cb: Rc<dyn Fn()>) {
    LISTENERS.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.push(Rc::downgrade(&cb));
    });
}

fn fire_refresh() {
    if REFRESH_SCHEDULED.with(|c| c.get()) {
        return;
    }
    REFRESH_SCHEDULED.with(|c| c.set(true));
    glib::idle_add_local_once(|| {
        REFRESH_SCHEDULED.with(|c| c.set(false));
        let callbacks: Vec<Rc<dyn Fn()>> = LISTENERS.with(|r| {
            let mut list = r.borrow_mut();
            list.retain(|w| w.strong_count() > 0);
            list.iter().filter_map(std::rc::Weak::upgrade).collect()
        });
        for cb in callbacks {
            cb();
        }
    });
}

pub fn is_pending(id: &str) -> bool {
    STORE.with(|s| s.borrow().is_pending(id))
}

/// Sorted pending desktop ids (for dock signature).
pub fn pending_ids() -> Vec<String> {
    STORE.with(|s| s.borrow().pending_ids())
}

/// Begin a gated launch. Returns `false` if this desktop id is already pending.
pub fn try_begin_launch(entry: &AppEntry) -> bool {
    let keys = match_keys_for_entry(entry);
    let started = STORE.with(|s| s.borrow_mut().try_begin(&entry.id, keys));
    if !started {
        tracing::debug!(id = %entry.id, "launch suppressed — already starting");
        return false;
    }
    fire_refresh();
    schedule_toast(entry.id.clone(), entry.name.clone());
    schedule_timeout(entry.id.clone());
    true
}

/// Clear pending state for `id` (e.g. spawn failed).
pub fn clear_id(id: &str) {
    let removed = STORE.with(|s| s.borrow_mut().remove(id));
    if removed {
        fire_refresh();
    }
}

/// Match a newly mapped window's `app_id` against pending launches.
pub fn clear_for_window(app_id: &str) {
    let cleared = STORE.with(|s| s.borrow_mut().clear_for_window(app_id));
    if !cleared.is_empty() {
        tracing::debug!(?cleared, app_id, "cleared pending launches for window");
        fire_refresh();
    }
}

fn schedule_timeout(_id: String) {
    let ms = PENDING_TIMEOUT.as_millis() as u64;
    glib::timeout_add_local_once(Duration::from_millis(ms), move || {
        let cleared = STORE.with(|s| {
            s.borrow_mut()
                .clear_expired(PENDING_TIMEOUT, Instant::now())
        });
        if !cleared.is_empty() {
            tracing::debug!(?cleared, "pending launches timed out");
            fire_refresh();
        }
    });
}

fn schedule_toast(id: String, name: String) {
    let ms = TOAST_DELAY.as_millis() as u64;
    glib::timeout_add_local_once(Duration::from_millis(ms), move || {
        let still = STORE.with(|s| s.borrow().is_pending(&id));
        if !still {
            return;
        }
        let mut note = crate::services::BarNotification::internal(
            crate::services::NotificationKind::Information,
            format!("Starting {name}"),
            "Opening the application…",
        );
        note.suppress_sound = true;
        note.expire_ms = 2500;
        crate::ui::toast::show(&note);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, wm: Option<&str>, flatpak: Option<&str>) -> AppEntry {
        AppEntry {
            id: id.into(),
            name: name.into(),
            exec: "true".into(),
            icon: None,
            keywords: Vec::new(),
            wm_class: wm.map(str::to_string),
            flatpak_id: flatpak.map(str::to_string),
            categories: Vec::new(),
        }
    }

    #[test]
    fn rejects_duplicate_begin() {
        let mut map = PendingMap::default();
        let e = entry("firefox.desktop", "Firefox", Some("firefox"), None);
        let keys = match_keys_for_entry(&e);
        assert!(map.try_begin(&e.id, keys.clone()));
        assert!(!map.try_begin(&e.id, keys));
        assert!(map.is_pending("firefox.desktop"));
    }

    #[test]
    fn clears_by_app_id_wm_class() {
        let mut map = PendingMap::default();
        let e = entry(
            "org.mozilla.firefox.desktop",
            "Firefox",
            Some("firefox"),
            None,
        );
        assert!(map.try_begin(&e.id, match_keys_for_entry(&e)));
        let cleared = map.clear_for_window("firefox");
        assert_eq!(cleared, vec!["org.mozilla.firefox.desktop".to_string()]);
        assert!(!map.is_pending(&e.id));
    }

    #[test]
    fn clears_by_flatpak_id() {
        let mut map = PendingMap::default();
        let e = entry(
            "com.valvesoftware.Steam.desktop",
            "Steam",
            None,
            Some("com.valvesoftware.Steam"),
        );
        assert!(map.try_begin(&e.id, match_keys_for_entry(&e)));
        assert!(map.clear_for_window("unrelated").is_empty());
        let cleared = map.clear_for_window("com.valvesoftware.Steam");
        assert_eq!(cleared.len(), 1);
    }

    #[test]
    fn clear_expired_drops_old() {
        let mut map = PendingMap::default();
        let e = entry("a.desktop", "A", None, None);
        assert!(map.try_begin(&e.id, match_keys_for_entry(&e)));
        // Force an old timestamp.
        if let Some(p) = map.by_id.get_mut("a.desktop") {
            p.started = Instant::now() - Duration::from_secs(30);
        }
        let cleared = map.clear_expired(PENDING_TIMEOUT, Instant::now());
        assert_eq!(cleared, vec!["a.desktop".to_string()]);
    }

    #[test]
    fn match_keys_include_basename() {
        let e = entry(
            "org.gnome.Nautilus.desktop",
            "Files",
            Some("org.gnome.Nautilus"),
            None,
        );
        let keys = match_keys_for_entry(&e);
        assert!(keys.iter().any(|k| k == "org.gnome.nautilus.desktop"));
        assert!(keys.iter().any(|k| k == "org.gnome.nautilus"));
        assert!(keys.iter().any(|k| k == "nautilus"));
    }
}
