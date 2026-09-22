use std::cell::RefCell;
use std::rc::Rc;

use tokio::sync::mpsc::UnboundedSender;

/// A notification plus how many identical copies have arrived (for de-duped
/// grouping with a count badge).
#[derive(Debug, Clone)]
pub struct NotificationEntry {
    /// Process-local id for store management (dismiss/grouping). Distinct from
    /// the freedesktop `BarNotification::id`, which can be 0 for internal alerts
    /// and is only meaningful for the `ActionInvoked` round-trip.
    pub uid: u64,
    pub notification: BarNotification,
    pub count: u32,
}

/// A message sent back to the freedesktop daemon thread when the user interacts
/// with a notification (clicks an action button or dismisses a card), so the
/// daemon can emit the spec-required `ActionInvoked` / `NotificationClosed`
/// signals to the originating application.
#[derive(Debug, Clone)]
pub enum NotifyOutgoing {
    /// User triggered an action; `key` is the action identifier the app sent.
    Action { id: u32, key: String },
    /// Notification was closed; `reason` follows the spec (1 expired, 2 dismissed
    /// by user, 3 closed by `CloseNotification`, 4 undefined).
    Closed { id: u32, reason: u32 },
}

thread_local! {
    /// Notifications raised at runtime (timers, alarms, calendar reminders),
    /// newest-first, with identical messages coalesced into a single entry.
    static RUNTIME: RefCell<Vec<NotificationEntry>> = const { RefCell::new(Vec::new()) };
    /// Monotonic source of process-local entry ids (see `NotificationEntry::uid`).
    static NEXT_UID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    /// Repaint hooks installed by each bar's notifications widget (one per output
    /// in a multi-monitor session). Weak so a torn-down bar's hook drops itself.
    static REFRESH: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    /// Outgoing channel back to the D-Bus daemon thread for action/close signals.
    /// Set once at startup from the GTK main thread by `set_action_sender`.
    static ACTION_TX: RefCell<Option<UnboundedSender<NotifyOutgoing>>> = const { RefCell::new(None) };
    /// Do Not Disturb — suppresses toasts; list still accumulates.
    static DND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn do_not_disturb() -> bool {
    DND.with(|d| d.get())
}

pub fn set_do_not_disturb(on: bool) {
    DND.with(|d| d.set(on));
}

/// Register the outgoing channel used to notify the freedesktop daemon thread of
/// action clicks / dismissals. Called once at startup on the GTK main thread.
pub fn set_action_sender(tx: UnboundedSender<NotifyOutgoing>) {
    ACTION_TX.with(|cell| *cell.borrow_mut() = Some(tx));
}

fn send_outgoing(msg: NotifyOutgoing) {
    ACTION_TX.with(|cell| {
        if let Some(tx) = cell.borrow().as_ref() {
            if let Err(err) = tx.send(msg) {
                tracing::debug!(%err, "notify: outgoing channel closed");
            }
        }
    });
}

/// Tell the originating app that the user invoked `key` on notification `id`.
/// Real D-Bus notifications carry a non-zero id; runtime/demo notifications use
/// id 0 and are silently ignored here (nothing to signal back to).
pub fn invoke_action(id: u32, key: &str) {
    if id == 0 {
        if crate::services::updates_handle_notification_action(key) {
            return;
        }
        return;
    }
    send_outgoing(NotifyOutgoing::Action {
        id,
        key: key.to_string(),
    });
}

/// Tell the originating app that notification `id` was closed. `reason` follows
/// the freedesktop spec (2 = dismissed by the user).
pub fn close_notification(id: u32, reason: u32) {
    if id == 0 {
        return;
    }
    send_outgoing(NotifyOutgoing::Closed { id, reason });
}

/// Best-effort notification sound, honouring the sender's `sound-file` /
/// `sound-name` hints and falling back to the freedesktop default. Mirrors the
/// `canberra-gtk-play` -> `paplay` fallback used for alarms. Degrades silently
/// when no player or sound is available. Callers must gate this on Do Not
/// Disturb and the `suppress-sound` hint themselves.
pub fn play_notification_sound(note: &BarNotification) {
    if let Some(file) = note.sound_file.as_deref().filter(|f| !f.is_empty()) {
        if std::process::Command::new("canberra-gtk-play")
            .args(["-f", file])
            .spawn()
            .is_ok()
        {
            return;
        }
        if std::process::Command::new("paplay")
            .arg(file)
            .spawn()
            .is_ok()
        {
            return;
        }
    }

    let name = note
        .sound_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or("message-new-instant");
    if std::process::Command::new("canberra-gtk-play")
        .args(["-i", name])
        .spawn()
        .is_ok()
    {
        return;
    }
    let _ = std::process::Command::new("paplay")
        .arg("/usr/share/sounds/freedesktop/stereo/message.oga")
        .spawn();
}

const RUNTIME_CAP: usize = 50;

/// Register a callback the runtime queue invokes whenever it changes so every bar
/// can repaint its notification badge and list. Each bar registers its own hook;
/// dead hooks (from rebuilt/removed bars) are pruned on the next register/fire.
pub fn register_refresh(cb: Rc<dyn Fn()>) {
    REFRESH.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.push(Rc::downgrade(&cb));
    });
}

fn fire_refresh() {
    // Collect live callbacks first so we don't hold the REFRESH borrow while a
    // callback runs (a callback may re-enter via register_refresh).
    let callbacks: Vec<Rc<dyn Fn()>> = REFRESH.with(|r| {
        let mut list = r.borrow_mut();
        list.retain(|w| w.strong_count() > 0);
        list.iter().filter_map(std::rc::Weak::upgrade).collect()
    });
    for cb in callbacks {
        cb();
    }
}

/// Re-run all registered notification UI refresh hooks (badge, lists, DND).
pub fn notify_store_changed() {
    fire_refresh();
}

/// Push a notification into the in-bar notification popup. Identical
/// notifications (same kind/title/message) are coalesced: the existing entry's
/// count is bumped and it moves to the top instead of stacking duplicates.
pub fn push_notification(notification: BarNotification) {
    RUNTIME.with(|r| {
        let mut list = r.borrow_mut();
        if let Some(pos) = list
            .iter()
            .position(|e| e.notification.content_eq(&notification))
        {
            let mut entry = list.remove(pos);
            entry.count = entry.count.saturating_add(1);
            // Refresh the carried notification so a re-sent firmware/update note
            // keeps its latest D-Bus id and action set while reusing the uid.
            entry.notification = notification;
            list.insert(0, entry);
        } else {
            let uid = NEXT_UID.with(|c| {
                let next = c.get();
                c.set(next.wrapping_add(1).max(1));
                next
            });
            list.insert(
                0,
                NotificationEntry {
                    uid,
                    notification,
                    count: 1,
                },
            );
            list.truncate(RUNTIME_CAP);
        }
    });
    fire_refresh();
}

/// Remove all runtime notifications.
pub fn clear_notifications() {
    let closed: Vec<u32> = RUNTIME.with(|r| {
        r.borrow()
            .iter()
            .map(|e| e.notification.id)
            .filter(|id| *id != 0)
            .collect()
    });
    RUNTIME.with(|r| r.borrow_mut().clear());
    for id in closed {
        close_notification(id, 2);
    }
    fire_refresh();
}

/// Remove every store entry that carries freedesktop notification `id` (used when
/// the sending app calls `CloseNotification`).
pub fn dismiss_notification_by_dbus_id(id: u32) {
    if id == 0 {
        return;
    }
    let removed = RUNTIME.with(|r| {
        let mut list = r.borrow_mut();
        let before = list.len();
        list.retain(|e| e.notification.id != id);
        list.len() != before
    });
    if removed {
        fire_refresh();
    }
}

/// Remove a single notification by its process-local `uid` (see
/// `NotificationEntry::uid`). Used when the user acts on or dismisses a card so
/// it disappears from the bar list regardless of its freedesktop id.
pub fn dismiss_notification(uid: u64) {
    let removed = RUNTIME.with(|r| {
        let mut list = r.borrow_mut();
        let before = list.len();
        list.retain(|e| e.uid != uid);
        list.len() != before
    });
    if removed {
        fire_refresh();
    }
}

/// Snapshot of the runtime notification queue (newest first), grouped.
pub fn runtime_notifications() -> Vec<NotificationEntry> {
    RUNTIME.with(|r| r.borrow().clone())
}

/// Total number of notifications including coalesced duplicates.
pub fn notification_count() -> u32 {
    RUNTIME.with(|r| r.borrow().iter().map(|e| e.count).sum())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Error,
    Notification,
    Success,
    Information,
    Payment,
}

impl NotificationKind {
    pub fn css_suffix(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Notification => "notify",
            Self::Success => "success",
            Self::Information => "info",
            Self::Payment => "payment",
        }
    }

    #[allow(dead_code)] // notification icon fallback API
    pub fn icon_glyph(self) -> &'static str {
        match self {
            Self::Error => "!",
            Self::Notification => "🔔",
            Self::Success => "✓",
            Self::Information => "i",
            Self::Payment => "$",
        }
    }

    /// Symbolic icon name that matches the notification's nature.
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Error => "dialog-error-symbolic",
            Self::Notification => "alarm-symbolic",
            Self::Success => "emblem-ok-symbolic",
            Self::Information => "dialog-information-symbolic",
            Self::Payment => "emblem-synchronizing-symbolic",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BarNotification {
    /// Freedesktop notification id assigned by the daemon (0 for internal/runtime
    /// or demo notifications that have no app to signal back to).
    pub id: u32,
    /// Sending application name (`app_name` from `Notify`).
    pub app_name: String,
    pub kind: NotificationKind,
    pub title: String,
    pub message: String,
    /// Labeled actions the sender provided as `(key, label)` pairs. The
    /// conventional `default` action (invoked by clicking the card body) is kept
    /// here too if present.
    pub actions: Vec<(String, String)>,
    /// `desktop-entry` hint — the `.desktop` id of the owning app, used to render
    /// a single "Open" button when no explicit actions are supplied.
    pub desktop_entry: Option<String>,
    /// `suppress-sound` hint: the sender asked that no sound be played.
    pub suppress_sound: bool,
    /// `sound-name` hint: a freedesktop sound theme id to play on arrival.
    pub sound_name: Option<String>,
    /// `sound-file` hint: an explicit audio file path to play on arrival.
    pub sound_file: Option<String>,
    /// Requested on-screen lifetime in ms (`expire_timeout`): `-1` = server
    /// default, `0` = never auto-expire. Drives toast auto-dismiss timing.
    pub expire_ms: i32,
}

impl BarNotification {
    /// Build an internal (non-D-Bus) notification with default action/sound
    /// fields. Used by timers, alarms, calendar reminders and demo seeds.
    pub fn internal(
        kind: NotificationKind,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            app_name: "Metis".to_string(),
            kind,
            title: title.into(),
            message: message.into(),
            actions: Vec::new(),
            desktop_entry: None,
            suppress_sound: false,
            sound_name: None,
            sound_file: None,
            expire_ms: -1,
        }
    }

    /// Whether two notifications carry the same user-visible content. Used for
    /// dedup/grouping so distinct ids, actions and sound hints do not prevent
    /// coalescing of otherwise-identical messages.
    pub fn content_eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.title == other.title && self.message == other.message
    }

    /// Non-`default` actions as `(key, label)` — these become explicit buttons.
    pub fn labeled_actions(&self) -> impl Iterator<Item = &(String, String)> {
        self.actions.iter().filter(|(key, _)| key != "default")
    }

    /// Whether a conventional `default` action is present (invoked by clicking
    /// the card body).
    pub fn has_default_action(&self) -> bool {
        self.actions.iter().any(|(key, _)| key == "default")
    }

    /// Resolved on-screen lifetime for a toast banner. Honours an explicit
    /// positive `expire_ms`; server-default (`-1`) and "never" (`0`) both map to
    /// a sensible ~5s banner so the overlay never lingers forever.
    pub fn toast_duration_ms(&self) -> u64 {
        if self.expire_ms > 0 {
            (self.expire_ms as u64).clamp(2000, 30_000)
        } else {
            5000
        }
    }
}
