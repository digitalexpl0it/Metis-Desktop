//! Background software-update checks and shared snapshot for the edge bar /
//! updater window.

use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::time::Duration;

use gtk::glib;
use metis_config::{load_updates_config, save_updates_config, UpdateSources, UpdatesConfig};
use metis_remote::{
    updates_apply, updates_check_from_config, updates_refresh, UpdateProgressEvent, UpdateSnapshot,
};

thread_local! {
    static SNAPSHOT: RefCell<UpdateSnapshot> = RefCell::new(UpdateSnapshot::default());
    static REFRESHES: RefCell<Vec<std::rc::Rc<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static LAST_NOTIFIED_COUNT: Cell<usize> = const { Cell::new(0) };
    static CHECK_GEN: Cell<u64> = const { Cell::new(0) };
}

pub fn snapshot() -> UpdateSnapshot {
    SNAPSHOT.with(|s| s.borrow().clone())
}

pub fn pending_count() -> usize {
    SNAPSHOT.with(|s| s.borrow().total_count())
}

pub fn register_refresh(cb: std::rc::Rc<dyn Fn()>) {
    REFRESHES.with(|r| r.borrow_mut().push(cb));
}

fn fire_refresh() {
    REFRESHES.with(|r| {
        for cb in r.borrow().iter() {
            cb();
        }
    });
}

fn set_snapshot(snap: UpdateSnapshot) {
    SNAPSHOT.with(|s| *s.borrow_mut() = snap);
    fire_refresh();
}

/// Start periodic checks (call once from the bar init path).
pub fn spawn_updates_service() {
    glib::timeout_add_local_once(Duration::from_secs(45), || {
        request_check(false);
    });

    glib::timeout_add_local(Duration::from_secs(60), || {
        let cfg = load_updates_config();
        if !cfg.enabled || cfg.is_snoozed() {
            fire_refresh();
            return glib::ControlFlow::Continue;
        }
        let interval = Duration::from_secs(u64::from(cfg.check_interval_hours.max(1)) * 3600);
        let due = cfg
            .last_check
            .map(|t| {
                let elapsed = chrono::Local::now().signed_duration_since(t);
                elapsed.to_std().unwrap_or(Duration::ZERO) >= interval
            })
            .unwrap_or(true);
        if due {
            request_check(false);
        }
        glib::ControlFlow::Continue
    });
}

/// Kick a background check. When `manual` is true, ignore snooze for listing.
pub fn request_check(manual: bool) {
    let mut cfg = load_updates_config();
    if !manual && (!cfg.enabled || cfg.is_snoozed()) {
        fire_refresh();
        return;
    }
    let gen = CHECK_GEN.with(|g| {
        let n = g.get() + 1;
        g.set(n);
        n
    });
    let sources = cfg.sources.clone();
    let (tx, rx) = mpsc::channel::<Result<UpdateSnapshot, String>>();
    std::thread::spawn(move || {
        let _ = updates_refresh(&sources, None);
        let snap = updates_check_from_config(&UpdatesConfig {
            sources,
            ..Default::default()
        });
        let _ = tx.send(Ok(snap));
    });

    glib::timeout_add_local(Duration::from_millis(200), move || {
        match rx.try_recv() {
            Ok(Ok(snap)) => {
                if CHECK_GEN.with(|g| g.get()) != gen {
                    return glib::ControlFlow::Break;
                }
                let count = snap.total_count();
                let err = snap.error.clone();
                cfg.last_check = Some(chrono::Local::now());
                cfg.last_error = err;
                let _ = save_updates_config(&cfg);
                maybe_notify(&cfg, count);
                set_snapshot(snap.clone());
                if cfg.auto_install_security && !manual {
                    let has_security = snap.packages.iter().any(|p| p.security);
                    if has_security {
                        tracing::info!("auto-installing PackageKit security updates");
                        let sources = UpdateSources {
                            packagekit: true,
                            flatpak: false,
                            fwupd: false,
                        };
                        std::thread::spawn(move || {
                            let _ = updates_refresh(&sources, None);
                            let _ = updates_apply(&sources, None);
                        });
                    }
                }
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                tracing::warn!(%err, "updates check failed");
                cfg.last_error = Some(err);
                let _ = save_updates_config(&cfg);
                fire_refresh();
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn maybe_notify(cfg: &UpdatesConfig, count: usize) {
    if !cfg.notify_on_available || cfg.is_snoozed() || count == 0 {
        LAST_NOTIFIED_COUNT.with(|c| c.set(count));
        return;
    }
    let prev = LAST_NOTIFIED_COUNT.with(|c| {
        let prev = c.get();
        c.set(count);
        prev
    });
    if count <= prev {
        return;
    }
    let mut note = crate::services::BarNotification::internal(
        crate::services::NotificationKind::Notification,
        metis_i18n::tr("Software updates available"),
        metis_i18n::tr("%1 update(s) ready to install.").replace("%1", &count.to_string()),
    );
    note.actions = vec![
        ("default".into(), metis_i18n::tr("Install")),
        ("later".into(), metis_i18n::tr("Later")),
    ];
    note.sound_name = Some("message-new-instant".into());
    crate::ui::bar::emit_updates_notification(note);
}

pub fn handle_notification_action(key: &str) -> bool {
    match key {
        "default" | "install" => {
            crate::ui::updater::show();
            true
        }
        "later" => {
            snooze_hours(24);
            true
        }
        _ => false,
    }
}

pub fn snooze_hours(hours: i64) {
    let mut cfg = load_updates_config();
    cfg.snooze_hours(hours);
    let _ = save_updates_config(&cfg);
    fire_refresh();
}

pub fn snooze_tonight() {
    let mut cfg = load_updates_config();
    cfg.snooze_until_tonight();
    let _ = save_updates_config(&cfg);
    fire_refresh();
}

pub fn snooze_one_day() {
    let mut cfg = load_updates_config();
    cfg.snooze_one_day();
    let _ = save_updates_config(&cfg);
    fire_refresh();
}

pub fn is_visible() -> bool {
    let cfg = load_updates_config();
    !cfg.is_snoozed() && pending_count() > 0
}

/// Run apply on a background thread; progress events arrive on `on_event` (GTK thread).
pub fn start_apply(on_event: std::rc::Rc<dyn Fn(UpdateProgressEvent)>) {
    let cfg = load_updates_config();
    let sources = cfg.sources.clone();
    let (tx, rx) = mpsc::channel::<UpdateProgressEvent>();
    std::thread::spawn(move || {
        let _ = updates_refresh(&sources, Some(tx.clone()));
        let _ = updates_apply(&sources, Some(tx));
    });
    glib::timeout_add_local(Duration::from_millis(100), move || {
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    let done = matches!(ev, UpdateProgressEvent::Finished { .. });
                    on_event(ev);
                    if done {
                        request_check(true);
                        return glib::ControlFlow::Break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    request_check(true);
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
}
