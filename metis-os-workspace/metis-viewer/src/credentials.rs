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

fn protocol_tag(protocol: ViewerProtocol) -> &'static str {
    match protocol {
        ViewerProtocol::Rdp => "rdp",
        ViewerProtocol::Rudp => "rudp",
    }
}

/// Per-card Secret Service account id (protocol + host + port + user + label).
///
/// Label is included so two saved cards for the same server keep separate
/// passwords. Unlabeled cards keep the legacy account shape.
pub fn secret_account(host: &ViewerHost) -> String {
    secret_account_parts(
        host.protocol,
        &host.host,
        host.port,
        &host.username,
        &host.label,
    )
}

/// Pre-label account id shared by every card on an endpoint (migration only).
pub fn secret_account_legacy(host: &ViewerHost) -> String {
    secret_account_parts(host.protocol, &host.host, host.port, &host.username, "")
}

pub fn secret_account_parts(
    protocol: ViewerProtocol,
    host: &str,
    port: u16,
    username: &str,
    label: &str,
) -> String {
    let proto = protocol_tag(protocol);
    let host = host.trim();
    let user = username.trim();
    let label = label.trim();
    if label.is_empty() {
        format!("viewer:{proto}:{host}:{port}:{user}")
    } else {
        format!("viewer:{proto}:{host}:{port}:{user}:label:{label}")
    }
}

/// Copy a pre-label shared endpoint password onto the oldest labeled card for
/// that endpoint (last in `hosts`, since newer cards are prepended). Newer
/// cards on the same server keep an empty secret until the user sets one.
///
/// Call from a worker thread — touches the Secret Service.
pub fn migrate_legacy_passwords(hosts: &[ViewerHost]) {
    use std::collections::HashMap;
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.set(()).is_err() {
        return;
    }

    let mut by_legacy: HashMap<String, Vec<&ViewerHost>> = HashMap::new();
    for host in hosts {
        let legacy = secret_account_legacy(host);
        by_legacy.entry(legacy).or_default().push(host);
    }

    for (legacy, group) in by_legacy {
        // Oldest card is last in recent order.
        let Some(owner) = group.last() else {
            continue;
        };
        let primary = secret_account(owner);
        if primary == legacy {
            continue;
        }
        if cache_get(&primary).is_some() || load_password_blocking(&primary).is_some() {
            continue;
        }
        let Some(pw) = load_password_blocking(&legacy) else {
            continue;
        };
        let _ = store_password_blocking(&primary, &pw);
    }
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

/// Delete the removed card's keyring secret only when no remaining card still
/// needs it. Never wipe a legacy shared endpoint secret while another card for
/// the same host/port/user/protocol remains.
///
/// Call after the card has been removed from `viewer.json`, passing the hosts
/// that are still saved.
pub fn delete_host_password_if_unshared(removed: &ViewerHost, remaining: &[ViewerHost]) {
    let account = secret_account(removed);
    let legacy = secret_account_legacy(removed);
    let endpoint_still_used = remaining.iter().any(|h| secret_account_legacy(h) == legacy);
    let account_still_used = remaining.iter().any(|h| secret_account(h) == account);

    if !account_still_used {
        if account == legacy {
            // Unlabeled card → shared legacy key. Keep it if siblings remain.
            if !endpoint_still_used {
                delete_password(&account);
            }
        } else {
            delete_password(&account);
        }
    }

    // Drop an orphan legacy entry only when this was the last card on the endpoint.
    if legacy != account && !endpoint_still_used {
        delete_password(&legacy);
    }
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
