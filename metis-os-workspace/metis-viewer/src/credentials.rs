//! Viewer host passwords via the freedesktop Secret Service.
//!
//! Host metadata stays in `viewer.json`; secrets never do.
//!
//! Never call Secret Service APIs on the GTK main thread — `oo7`/D-Bus can
//! stall the UI for seconds (looks like password typing “stutter”).

use std::collections::HashMap;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gtk::prelude::EditableExt;
use metis_config::{ViewerHost, ViewerProtocol};
use metis_secrets::VIEWER_PASSWORD;

/// Session cache so Edit / Connect do not re-hit the keyring every time.
/// Shared across keyring worker threads and the GTK thread.
fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stable Secret Service account id for a saved host endpoint.
pub fn secret_account(host: &ViewerHost) -> String {
    secret_account_parts(host.protocol, &host.host, host.port, &host.username)
}

pub fn secret_account_parts(
    protocol: ViewerProtocol,
    host: &str,
    port: u16,
    username: &str,
) -> String {
    let proto = match protocol {
        ViewerProtocol::Rdp => "rdp",
        ViewerProtocol::Rudp => "rudp",
    };
    format!(
        "viewer:{proto}:{}:{}:{}",
        host.trim(),
        port,
        username.trim()
    )
}

fn with_runtime<T>(f: impl FnOnce(&tokio::runtime::Runtime) -> T) -> Option<T> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    Some(f(&rt))
}

fn cache_get(account: &str) -> Option<String> {
    cache().lock().ok().and_then(|c| c.get(account).cloned())
}

fn cache_set(account: &str, password: &str) {
    let Ok(mut c) = cache().lock() else {
        return;
    };
    if password.is_empty() {
        c.remove(account);
    } else {
        c.insert(account.to_string(), password.to_string());
    }
}

fn cache_remove(account: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(account);
    }
}

/// Cached lookup only — never blocks on D-Bus.
pub fn cached_password(account: &str) -> Option<String> {
    cache_get(account)
}

/// Blocking Secret Service read. Call only from a worker thread.
pub fn load_password_blocking(account: &str) -> Option<String> {
    if let Some(cached) = cache_get(account) {
        return Some(cached);
    }
    let loaded = with_runtime(|rt| {
        rt.block_on(async { metis_secrets::get(account, VIEWER_PASSWORD).await })
            .ok()
            .flatten()
    })
    .flatten()?;
    cache_set(account, &loaded);
    Some(loaded)
}

pub fn store_password_blocking(account: &str, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Ok(());
    }
    cache_set(account, password);
    with_runtime(|rt| {
        rt.block_on(async { metis_secrets::store(account, VIEWER_PASSWORD, password).await })
            .map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("could not start keyring runtime".into()))
}

pub fn delete_password(account: &str) {
    cache_remove(account);
    let account = account.to_string();
    let _ = std::thread::Builder::new()
        .name("metis-viewer-keyring-del".into())
        .spawn(move || {
            let _ = with_runtime(|rt| {
                rt.block_on(async { metis_secrets::delete(&account, VIEWER_PASSWORD).await })
            });
        });
}

/// Persist password for `new_account`. Empty password keeps / migrates the
/// previous secret when the endpoint identity changed.
pub fn save_host_password(
    new_account: &str,
    password: &str,
    previous_account: Option<&str>,
) -> Result<(), String> {
    let new = new_account.to_string();
    let prev = previous_account.map(str::to_string);
    let password = password.to_string();
    // Keep the session cache in sync immediately so Edit/Connect feel instant.
    if !password.is_empty() {
        cache_set(new_account, &password);
        if let Some(prev) = previous_account
            && prev != new_account
        {
            cache_remove(prev);
        }
    } else if let Some(prev) = previous_account
        && prev != new_account
        && let Some(existing) = cache_get(prev)
    {
        cache_set(new_account, &existing);
        cache_remove(prev);
    }
    let _ = std::thread::Builder::new()
        .name("metis-viewer-keyring-save".into())
        .spawn(move || {
            if !password.is_empty() {
                let _ = store_password_blocking(&new, &password);
                if let Some(prev) = prev.as_deref()
                    && prev != new
                {
                    delete_password_sync(prev);
                }
                return;
            }
            if let Some(prev) = prev.as_deref()
                && prev != new
                && let Some(existing) = load_password_blocking(prev)
            {
                let _ = store_password_blocking(&new, &existing);
                delete_password_sync(prev);
            }
        });
    Ok(())
}

fn delete_password_sync(account: &str) {
    cache_remove(account);
    let _ = with_runtime(|rt| {
        rt.block_on(async { metis_secrets::delete(account, VIEWER_PASSWORD).await })
    });
}

/// Fill `pass_entry` from cache/keyring without blocking the UI or clobbering
/// keystrokes (`password_dirty`).
pub fn fill_password_async(
    pass_entry: gtk::PasswordEntry,
    account: String,
    password_dirty: std::rc::Rc<std::cell::Cell<bool>>,
    expected_account: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    suppress_dirty: std::rc::Rc<std::cell::Cell<bool>>,
) {
    let apply = |account: &str, pw: &str| {
        if password_dirty.get() {
            return;
        }
        if expected_account.borrow().as_deref() != Some(account) {
            return;
        }
        if !pass_entry.text().is_empty() {
            return;
        }
        suppress_dirty.set(true);
        pass_entry.set_text(pw);
        suppress_dirty.set(false);
    };

    if let Some(pw) = cache_get(&account) {
        apply(&account, &pw);
        return;
    }

    let (tx, rx) = mpsc::channel::<Option<String>>();
    let account_for_worker = account.clone();
    let _ = std::thread::Builder::new()
        .name("metis-viewer-keyring-load".into())
        .spawn(move || {
            let _ = tx.send(load_password_blocking(&account_for_worker));
        });

    let pass = pass_entry;
    let dirty = password_dirty;
    let expected = expected_account;
    let suppress = suppress_dirty;
    glib::timeout_add_local(Duration::from_millis(16), move || match rx.try_recv() {
        Ok(Some(pw)) => {
            if !dirty.get()
                && expected.borrow().as_deref() == Some(account.as_str())
                && pass.text().is_empty()
            {
                suppress.set(true);
                pass.set_text(&pw);
                suppress.set(false);
            }
            glib::ControlFlow::Break
        }
        Ok(None) => glib::ControlFlow::Break,
        Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}
