//! Freedesktop notification daemon (`org.freedesktop.Notifications`).
//!
//! Runs a `zbus` server on a dedicated background thread (its own current-thread
//! tokio runtime) and forwards every incoming `Notify` into the in-bar
//! notification store. Because that store is `thread_local` to the GTK main
//! thread, the daemon hands notifications across an `mpsc` channel that the bar
//! drains on the UI thread.
//!
//! The bus name is requested with the replace flags so Metis takes over from any
//! previously running daemon (dunst/mako). If another daemon later reclaims the
//! name, Metis simply stops receiving — acceptable for a desktop shell.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zbus::fdo::RequestNameFlags;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value};

use super::notifications::{BarNotification, NotificationKind, NotifyOutgoing};

/// Events delivered from the D-Bus daemon thread to the GTK main thread.
#[derive(Debug, Clone)]
pub enum NotifyIncoming {
    /// A new (or replaced) notification to show.
    Show(BarNotification),
    /// Sending app called `CloseNotification` — drop it from the in-bar store.
    Closed { id: u32 },
}

/// Incoming notifications + the outgoing action/close channel handed to the bar.
pub struct NotifyChannels {
    /// Notifications delivered by the daemon, drained on the GTK main thread.
    pub incoming: Receiver<NotifyIncoming>,
    /// Sent by the UI when the user clicks an action or dismisses a card; the
    /// daemon turns these into `ActionInvoked` / `NotificationClosed` signals.
    pub actions: UnboundedSender<NotifyOutgoing>,
}

struct NotifyServer {
    tx: Mutex<Sender<NotifyIncoming>>,
    /// Outgoing signal channel so `CloseNotification` can emit `NotificationClosed`.
    out_tx: UnboundedSender<NotifyOutgoing>,
    next_id: AtomicU32,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotifyServer {
    /// Implements `org.freedesktop.Notifications.Notify`. Returns the assigned
    /// notification id (echoes `replaces_id` when non-zero, per spec).
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed).max(1)
        };

        let kind = urgency_kind(&hints);
        let title = if summary.trim().is_empty() {
            app_name.clone()
        } else {
            summary
        };
        let note = BarNotification {
            id,
            app_name,
            kind,
            title,
            message: body,
            actions: parse_actions(&actions),
            desktop_entry: hint_str(&hints, "desktop-entry"),
            suppress_sound: hint_bool(&hints, "suppress-sound"),
            sound_name: hint_str(&hints, "sound-name"),
            sound_file: hint_str(&hints, "sound-file"),
            expire_ms: expire_timeout,
        };
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(NotifyIncoming::Show(note));
        }
        id
    }

    /// `CloseNotification` — drop from the UI store and emit `NotificationClosed`
    /// with reason 3 (closed by a call to CloseNotification), per the spec.
    async fn close_notification(&self, id: u32) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(NotifyIncoming::Closed { id });
        }
        let _ = self.out_tx.send(NotifyOutgoing::Closed { id, reason: 3 });
    }

    /// `GetCapabilities` — advertise what Metis renders. `actions` is what makes
    /// apps attach buttons; `sound` lets them request a tone on arrival.
    fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".to_string(),
            "actions".to_string(),
            "sound".to_string(),
            "persistence".to_string(),
        ]
    }

    /// `GetServerInformation` — (name, vendor, version, spec_version).
    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "Metis".to_string(),
            "metis".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }
}

/// Map the freedesktop `urgency` hint (byte: 0 low, 1 normal, 2 critical) to a
/// Metis notification kind. Anything missing/odd is treated as a normal alert.
fn urgency_kind(hints: &HashMap<String, OwnedValue>) -> NotificationKind {
    let urgency = hints.get("urgency").and_then(|v| match &**v {
        Value::U8(b) => Some(*b),
        _ => None,
    });
    match urgency {
        Some(0) => NotificationKind::Information,
        Some(2) => NotificationKind::Error,
        _ => NotificationKind::Notification,
    }
}

/// The `actions` array alternates `[key, label, key, label, ...]`. Pair them up,
/// dropping any trailing unpaired entry.
fn parse_actions(actions: &[String]) -> Vec<(String, String)> {
    actions
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

/// Read a string-valued hint (handles both `Str` and `OwnedValue`-wrapped str).
fn hint_str(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    hints.get(key).and_then(|v| match &**v {
        Value::Str(s) => {
            let s = s.as_str();
            (!s.is_empty()).then(|| s.to_string())
        }
        _ => None,
    })
}

/// Read a bool-valued hint, defaulting to `false` when missing or wrong type.
fn hint_bool(hints: &HashMap<String, OwnedValue>, key: &str) -> bool {
    matches!(hints.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

/// Start the notification daemon on a background thread and return the receiver
/// the bar polls on the GTK main thread.
pub fn spawn_notification_service() -> NotifyChannels {
    let (tx, rx) = channel::<NotifyIncoming>();
    // Outgoing action/close channel. One sender goes to the UI (via the returned
    // struct), one to the daemon's `CloseNotification`, and the receiver lives in
    // `run()`. `unbounded_channel` does not need a runtime to be created.
    let (out_tx, out_rx) = unbounded_channel::<NotifyOutgoing>();
    let ui_out_tx = out_tx.clone();

    let nested = std::env::var("METIS_NESTED")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let disabled = std::env::var("METIS_NO_NOTIFY")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if nested || disabled {
        tracing::info!("notify: skipped (nested dev session or METIS_NO_NOTIFY)");
        drop(tx);
        // Drop the daemon's receiver so UI sends fail gracefully (debug-logged).
        drop(out_rx);
        return NotifyChannels {
            incoming: rx,
            actions: ui_out_tx,
        };
    }
    if let Err(err) = std::thread::Builder::new()
        .name("metis-notify-dbus".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(%err, "notify: failed to build runtime");
                    return;
                }
            };
            if let Err(err) = rt.block_on(run(tx, out_tx, out_rx)) {
                tracing::warn!(%err, "notify: dbus service stopped");
            }
        })
    {
        tracing::error!(%err, "notify: failed to spawn dbus thread");
    }
    NotifyChannels {
        incoming: rx,
        actions: ui_out_tx,
    }
}

const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";

async fn run(
    tx: Sender<NotifyIncoming>,
    out_tx: UnboundedSender<NotifyOutgoing>,
    mut out_rx: UnboundedReceiver<NotifyOutgoing>,
) -> zbus::Result<()> {
    let server = NotifyServer {
        tx: Mutex::new(tx),
        out_tx,
        next_id: AtomicU32::new(1),
    };
    let conn = zbus::connection::Builder::session()?
        .serve_at(NOTIFY_PATH, server)?
        .build()
        .await?;

    let flags = RequestNameFlags::ReplaceExisting | RequestNameFlags::AllowReplacement;
    conn.request_name_with_flags(NOTIFY_IFACE, flags).await?;
    tracing::info!("notify: acquired org.freedesktop.Notifications");

    // Drain outgoing UI interactions and emit the spec-required signals back to
    // the originating apps. This loop also keeps the connection (and thus the
    // service) alive for the process lifetime: the server holds a sender clone,
    // so `recv()` never returns `None`.
    while let Some(msg) = out_rx.recv().await {
        let result = match msg {
            NotifyOutgoing::Action { id, key } => {
                conn.emit_signal(
                    None::<&str>,
                    NOTIFY_PATH,
                    NOTIFY_IFACE,
                    "ActionInvoked",
                    &(id, key),
                )
                .await
            }
            NotifyOutgoing::Closed { id, reason } => {
                conn.emit_signal(
                    None::<&str>,
                    NOTIFY_PATH,
                    NOTIFY_IFACE,
                    "NotificationClosed",
                    &(id, reason),
                )
                .await
            }
        };
        if let Err(err) = result {
            tracing::warn!(%err, "notify: failed to emit signal");
        }
    }
    Ok(())
}
