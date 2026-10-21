//! Layer-shell prompt to unlock `metis-secretsd` via `org.metis.Secrets1`.
//!
//! zbus (tokio) must not run on the GLib main context. D-Bus work runs on a
//! worker thread with a short-lived current-thread Tokio runtime; results return
//! via `mpsc` + `glib::timeout_add_local` (same pattern as updates apply).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::gdk::Key;
use gtk::glib::{self, Propagation};
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_i18n::tr;

thread_local! {
    static OVERLAY: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// After the bar is up, check whether the vault is locked and prompt once.
/// Also listen for Secret Service `UnlockPromptNeeded` (libsecret Unlock path).
pub fn maybe_prompt_at_startup() {
    spawn_unlock_prompt_listener();
    glib::timeout_add_seconds_local_once(3, || {
        let (tx, rx) = mpsc::sync_channel::<bool>(1);
        std::thread::Builder::new()
            .name("metis-secrets-check".into())
            .spawn(move || {
                let _ = tx.send(vault_is_locked_blocking());
            })
            .ok();
        glib::timeout_add_local(Duration::from_millis(100), move || match rx.try_recv() {
            Ok(true) => {
                ask();
                glib::ControlFlow::Break
            }
            Ok(false) => glib::ControlFlow::Break,
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    });
}

/// Background thread: show the unlock UI when secretsd emits `UnlockPromptNeeded`.
fn spawn_unlock_prompt_listener() {
    let (tx, rx) = mpsc::sync_channel::<()>(4);
    std::thread::Builder::new()
        .name("metis-secrets-prompt".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                let Ok(conn) = zbus::Connection::session().await else {
                    return;
                };
                let Ok(proxy) = zbus::Proxy::new(
                    &conn,
                    "org.freedesktop.secrets",
                    "/org/freedesktop/secrets",
                    "org.metis.Secrets1",
                )
                .await
                else {
                    return;
                };
                let Ok(mut stream) = proxy.receive_signal("UnlockPromptNeeded").await else {
                    return;
                };
                use futures_util::StreamExt;
                while stream.next().await.is_some() {
                    let _ = tx.send(());
                }
            });
        })
        .ok();
    glib::timeout_add_local(Duration::from_millis(250), move || match rx.try_recv() {
        Ok(()) => {
            ask();
            glib::ControlFlow::Continue
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

fn with_tokio_rt<T>(f: impl std::future::Future<Output = T>) -> Option<T> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    Some(rt.block_on(f))
}

fn vault_is_locked_blocking() -> bool {
    with_tokio_rt(vault_is_locked()).unwrap_or(false)
}

fn unlock_with_password_blocking(password: String) -> Result<(), String> {
    with_tokio_rt(unlock_with_password(password))
        .unwrap_or_else(|| Err("failed to start Tokio runtime for secrets unlock".into()))
}

async fn vault_is_locked() -> bool {
    let Ok(conn) = zbus::Connection::session().await else {
        return false;
    };
    let Ok(proxy) = zbus::Proxy::new(
        &conn,
        "org.freedesktop.secrets",
        "/org/freedesktop/secrets",
        "org.metis.Secrets1",
    )
    .await
    else {
        return false;
    };
    proxy.get_property::<bool>("Locked").await.unwrap_or(false)
}

async fn unlock_with_password(password: String) -> Result<(), String> {
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.secrets",
        "/org/freedesktop/secrets",
        "org.metis.Secrets1",
    )
    .await
    .map_err(|e| e.to_string())?;
    proxy
        .call_method("UnlockWithPassword", &(password,))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Show the unlock dialog (idempotent — replaces any existing overlay).
pub fn ask() {
    dismiss();

    let window = gtk::Window::builder()
        .title(tr("Unlock keyring"))
        .decorated(false)
        .build();
    window.add_css_class("metis-session-confirm-window");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-secrets-unlock"));
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_exclusive_zone(-1);
        window.set_margin(edge, 0);
    }

    let chrome = gtk::Overlay::new();
    chrome.add_css_class("metis-session-confirm-root");

    let backdrop = gtk::Button::new();
    backdrop.set_has_frame(false);
    backdrop.add_css_class("metis-session-confirm-backdrop");
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    chrome.set_child(Some(&backdrop));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.add_css_class("metis-session-confirm-card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);

    let icon = gtk::Image::from_icon_name("dialog-password-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("metis-session-confirm-icon");
    icon.set_halign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&tr("Unlock Metis keyring")));
    title.add_css_class("metis-session-confirm-title");
    title.set_halign(gtk::Align::Center);

    let detail = gtk::Label::new(Some(&tr(
        "Enter your login password to unlock saved passwords and portal secrets.",
    )));
    detail.add_css_class("metis-session-confirm-detail");
    detail.set_wrap(true);
    detail.set_justify(gtk::Justification::Center);
    detail.set_max_width_chars(40);
    detail.set_halign(gtk::Align::Center);

    let entry = gtk::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    entry.set_hexpand(true);
    entry.set_width_chars(28);

    let error = gtk::Label::new(None);
    error.add_css_class("metis-session-confirm-detail");
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_visible(false);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::Center);
    buttons.add_css_class("metis-session-confirm-actions");

    let later = gtk::Button::with_label(&tr("Later"));
    later.add_css_class("metis-session-confirm-cancel");

    let unlock = gtk::Button::with_label(&tr("Unlock"));
    unlock.add_css_class("metis-session-confirm-ok");
    unlock.add_css_class("suggested-action");

    buttons.append(&later);
    buttons.append(&unlock);

    card.append(&icon);
    card.append(&title);
    card.append(&detail);
    card.append(&entry);
    card.append(&error);
    card.append(&buttons);
    chrome.add_overlay(&card);
    window.set_child(Some(&chrome));

    let win_later = window.clone();
    later.connect_clicked(move |_| {
        win_later.close();
    });
    let win_backdrop = window.clone();
    backdrop.connect_clicked(move |_| {
        win_backdrop.close();
    });

    let try_unlock = Rc::new({
        let window = window.clone();
        let entry = entry.clone();
        let error = error.clone();
        let unlock_btn = unlock.clone();
        move || {
            let password = entry.text().to_string();
            if password.is_empty() {
                error.set_text(&tr("Password required."));
                error.set_visible(true);
                return;
            }
            unlock_btn.set_sensitive(false);
            let window = window.clone();
            let error = error.clone();
            let unlock_btn = unlock_btn.clone();
            let (tx, rx) = mpsc::sync_channel::<Result<(), String>>(1);
            std::thread::Builder::new()
                .name("metis-secrets-unlock".into())
                .spawn(move || {
                    let _ = tx.send(unlock_with_password_blocking(password));
                })
                .ok();
            glib::timeout_add_local(Duration::from_millis(100), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    window.close();
                    load_ssh_keys_after_unlock();
                    glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    tracing::warn!(%err, "secrets unlock failed");
                    error.set_text(&tr("Could not unlock. Check your password."));
                    error.set_visible(true);
                    unlock_btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    error.set_text(&tr("Could not unlock. Check your password."));
                    error.set_visible(true);
                    unlock_btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
            });
        }
    });

    let try_click = try_unlock.clone();
    unlock.connect_clicked(move |_| try_click());

    let try_activate = try_unlock.clone();
    entry.connect_activate(move |_| try_activate());

    let key = gtk::EventControllerKey::new();
    let win_esc = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == Key::Escape {
            win_esc.close();
            Propagation::Stop
        } else {
            Propagation::Proceed
        }
    });
    window.add_controller(key);

    window.connect_close_request(|win| {
        OVERLAY.with(|cell| {
            if cell
                .borrow()
                .as_ref()
                .is_some_and(|existing| existing == win)
            {
                *cell.borrow_mut() = None;
            }
        });
        Propagation::Proceed
    });

    OVERLAY.with(|cell| *cell.borrow_mut() = Some(window.clone()));
    window.present();
    entry.grab_focus();
}

fn dismiss() {
    OVERLAY.with(|cell| {
        if let Some(win) = cell.borrow_mut().take() {
            win.close();
        }
    });
}

/// Pipe vault-stored OpenSSH private keys into `ssh-add` (Phase 22 §C).
fn load_ssh_keys_after_unlock() {
    if std::env::var_os("SSH_AUTH_SOCK").is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("metis-ssh-add".into())
        .spawn(|| {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let keys = rt
                .block_on(metis_secrets::list_ssh_key_secrets())
                .unwrap_or_default();
            for (label, pem) in keys {
                if pem.trim().is_empty() {
                    continue;
                }
                let mut child = match std::process::Command::new("ssh-add")
                    .arg("-")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(err) => {
                        tracing::debug!(%err, "ssh-add not available");
                        return;
                    }
                };
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(pem.as_bytes());
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        tracing::info!(%label, "loaded SSH key into agent");
                    }
                    Ok(_) => tracing::warn!(%label, "ssh-add failed for vault key"),
                    Err(err) => tracing::warn!(%err, %label, "ssh-add wait failed"),
                }
            }
        })
        .ok();
}
