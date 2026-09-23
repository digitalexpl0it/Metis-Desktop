use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

const DEFAULT_MAX_ENTRIES: usize = 50;
const DEFAULT_PAGE_SIZE: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub id: u64,
    pub mime: String,
    #[serde(default)]
    pub preview_text: Option<String>,
    #[serde(default)]
    pub image_path: Option<String>,
    #[serde(default)]
    pub favorited: bool,
}

#[derive(Debug, Clone)]
pub struct ClipboardPage {
    pub entries: Vec<ClipboardEntry>,
    pub page: usize,
    pub total_pages: usize,
    pub total_matching: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClipboardStore {
    #[serde(default = "default_max_entries")]
    max_entries: usize,
    #[serde(default = "default_page_size")]
    page_size: usize,
    #[serde(default)]
    private_mode: bool,
    #[serde(default)]
    active_id: Option<u64>,
    #[serde(default)]
    entries: Vec<ClipboardEntry>,
}

fn default_max_entries() -> usize {
    DEFAULT_MAX_ENTRIES
}

fn default_page_size() -> usize {
    DEFAULT_PAGE_SIZE
}

thread_local! {
    static ENTRIES: RefCell<Vec<ClipboardEntry>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
    static PRIVATE_MODE: RefCell<bool> = const { RefCell::new(false) };
    static PAGE_SIZE: RefCell<usize> = const { RefCell::new(DEFAULT_PAGE_SIZE) };
    static ACTIVE_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
    static REFRESH: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
}

pub fn register_refresh(f: Rc<dyn Fn()>) {
    REFRESH.with(|cell| cell.borrow_mut().push(Rc::downgrade(&f)));
}

fn notify_refresh() {
    glib::idle_add_local_once(|| {
        let callbacks: Vec<Rc<dyn Fn()>> = REFRESH.with(|cell| {
            let mut slot = cell.borrow_mut();
            slot.retain(|weak| weak.upgrade().is_some());
            slot.iter().filter_map(|weak| weak.upgrade()).collect()
        });
        for f in callbacks {
            f();
        }
    });
}

pub fn load_history() {
    let path = clipboard_json_path();
    let store: ClipboardStore = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let max_id = store.entries.iter().map(|e| e.id).max();
    ENTRIES.with(|cell| {
        *cell.borrow_mut() = store.entries;
    });
    PRIVATE_MODE.with(|m| *m.borrow_mut() = store.private_mode);
    PAGE_SIZE.with(|p| *p.borrow_mut() = store.page_size.max(1));
    ACTIVE_ID.with(|a| *a.borrow_mut() = store.active_id);
    if let Some(max) = max_id {
        NEXT_ID.with(|id| {
            *id.borrow_mut() = max.saturating_add(1);
        });
    }
}

fn set_active_entry_id(id: Option<u64>) {
    ACTIVE_ID.with(|a| *a.borrow_mut() = id);
}

fn persist_history() {
    let path = clipboard_json_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let entries = ENTRIES.with(|cell| cell.borrow().clone());
    let store = ClipboardStore {
        max_entries: DEFAULT_MAX_ENTRIES,
        page_size: PAGE_SIZE.with(|p| *p.borrow()),
        private_mode: PRIVATE_MODE.with(|m| *m.borrow()),
        active_id: ACTIVE_ID.with(|a| *a.borrow()),
        entries,
    };
    if let Ok(json) = serde_json::to_string_pretty(&store) {
        let _ = std::fs::write(path, json);
    }
}

fn persist_and_notify_active(id: Option<u64>) {
    set_active_entry_id(id);
    persist_history();
    notify_refresh();
}

pub fn clipboard_state_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "metis", "metis")
        .and_then(|d| d.state_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".local/state/metis"))
                .unwrap_or_else(|_| PathBuf::from(".local/state/metis"))
        })
}

fn clipboard_json_path() -> PathBuf {
    clipboard_state_dir().join("clipboard.json")
}

pub fn runtime_clipboard_entries() -> Vec<ClipboardEntry> {
    ENTRIES.with(|cell| cell.borrow().clone())
}

pub fn private_mode() -> bool {
    PRIVATE_MODE.with(|m| *m.borrow())
}

pub fn set_private_mode(on: bool) {
    PRIVATE_MODE.with(|m| *m.borrow_mut() = on);
    persist_history();
    notify_refresh();
}

pub fn page_size() -> usize {
    PAGE_SIZE.with(|p| *p.borrow())
}

pub fn set_page_size(size: usize) {
    PAGE_SIZE.with(|p| *p.borrow_mut() = size.clamp(10, 200));
    persist_history();
    notify_refresh();
}

pub fn active_entry_id() -> Option<u64> {
    ACTIVE_ID.with(|a| *a.borrow())
}

pub fn filtered_entries(search: &str, page: usize) -> ClipboardPage {
    let needle = search.trim().to_lowercase();
    let all = runtime_clipboard_entries();
    let matching: Vec<ClipboardEntry> = if needle.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|e| entry_matches(&needle, e))
            .collect()
    };
    let per_page = page_size();
    let total_matching = matching.len();
    let total_pages = if total_matching == 0 {
        1
    } else {
        total_matching.div_ceil(per_page)
    };
    let page = page.min(total_pages.saturating_sub(1));
    let start = page * per_page;
    let entries = matching.into_iter().skip(start).take(per_page).collect();
    ClipboardPage {
        entries,
        page,
        total_pages,
        total_matching,
    }
}

fn entry_matches(needle: &str, entry: &ClipboardEntry) -> bool {
    entry
        .preview_text
        .as_deref()
        .is_some_and(|t| t.to_lowercase().contains(needle))
        || entry.mime.to_lowercase().contains(needle)
}

fn trim_entries(entries: &mut Vec<ClipboardEntry>) {
    while entries.len() > DEFAULT_MAX_ENTRIES {
        let Some(pos) = entries.iter().rposition(|e| !e.favorited) else {
            break;
        };
        if let Some(path) = entries[pos].image_path.take() {
            let _ = std::fs::remove_file(path);
        }
        entries.remove(pos);
    }
}

pub fn apply_clipboard_event(mime: &str, preview_text: Option<String>, image_path: Option<String>) {
    if private_mode() {
        return;
    }
    if preview_text.as_deref().is_some_and(|t| t.is_empty()) && image_path.is_none() {
        return;
    }

    let active_id = ENTRIES.with(|cell| {
        let mut entries = cell.borrow_mut();
        if let Some(first) = entries.first()
            && first.mime == mime
            && first.preview_text == preview_text
            && first.image_path == image_path
        {
            return Some(first.id);
        }
        let id = NEXT_ID.with(|n| {
            let mut id = n.borrow_mut();
            let current = *id;
            *id = id.saturating_add(1);
            current
        });
        entries.insert(
            0,
            ClipboardEntry {
                id,
                mime: mime.to_string(),
                preview_text,
                image_path,
                favorited: false,
            },
        );
        trim_entries(&mut entries);
        Some(id)
    });
    if let Some(id) = active_id {
        persist_and_notify_active(Some(id));
    }
}

pub fn clear_history() {
    ENTRIES.with(|cell| {
        for entry in cell.borrow().iter() {
            if let Some(path) = entry.image_path.as_deref() {
                let _ = std::fs::remove_file(path);
            }
        }
        cell.borrow_mut().clear();
    });
    persist_and_notify_active(None);
}

pub fn delete_entry(id: u64) {
    ENTRIES.with(|cell| {
        let mut entries = cell.borrow_mut();
        if let Some(pos) = entries.iter().position(|e| e.id == id) {
            if let Some(path) = entries[pos].image_path.take() {
                let _ = std::fs::remove_file(path);
            }
            entries.remove(pos);
        }
    });
    if active_entry_id() == Some(id) {
        set_active_entry_id(None);
    }
    persist_history();
    notify_refresh();
}

pub fn toggle_favorite(id: u64) {
    ENTRIES.with(|cell| {
        if let Some(entry) = cell.borrow_mut().iter_mut().find(|e| e.id == id) {
            entry.favorited = !entry.favorited;
        }
    });
    persist_history();
    notify_refresh();
}

pub fn recall_entry(entry: &ClipboardEntry) -> Result<(), String> {
    if entry.preview_text.is_none() && entry.image_path.is_none() {
        return Err("empty clipboard entry".into());
    }
    if let Some(path) = entry.image_path.as_deref()
        && !Path::new(path).exists()
    {
        return Err("image no longer available".into());
    }

    let entry = entry.clone();
    glib::idle_add_local_once(move || {
        let result = recall_entry_now(&entry);
        if result.is_ok() {
            set_active_entry_id(Some(entry.id));
            notify_refresh();
        } else if let Err(err) = result {
            tracing::warn!(%err, "clipboard recall failed");
        }
    });
    Ok(())
}

fn recall_entry_now(entry: &ClipboardEntry) -> Result<(), String> {
    if let Some(text) = entry.preview_text.as_deref() {
        crate::compositor::client::set_clipboard(entry.mime.clone(), Some(text.to_string()), None)
            .map_err(|e| e.to_string())
    } else if let Some(path) = entry.image_path.as_deref() {
        let mime = effective_image_mime(entry, path);
        crate::compositor::client::set_clipboard(mime, None, Some(path.to_string()))
            .map_err(|e| e.to_string())
    } else {
        Err("empty clipboard entry".into())
    }
}

fn effective_image_mime(entry: &ClipboardEntry, path: &str) -> String {
    if entry.mime.starts_with("image/") {
        return entry.mime.clone();
    }
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png".into(),
        Some("jpg") | Some("jpeg") => "image/jpeg".into(),
        Some("webp") => "image/webp".into(),
        Some("bmp") => "image/bmp".into(),
        _ => entry.mime.clone(),
    }
}

impl Default for ClipboardStore {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            page_size: DEFAULT_PAGE_SIZE,
            private_mode: false,
            active_id: None,
            entries: Vec::new(),
        }
    }
}
