//! Background software-update checks and shared snapshot for the edge bar /
//! updater window.
//!
//! The last snapshot is cached on disk so the bar icon survives restarts
//! without spawning anything. Automatic checks are PackageKit-only, run off
//! the GTK thread, and fire at most once per `check_interval_hours`. Flatpak /
//! fwupd are only probed on manual checks (Settings "Check now").

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::glib;
use metis_config::{UpdatesConfig, load_updates_config, save_updates_config};
use metis_remote::{
    ConfFileChoice, UpdateApplyScope, UpdateProgressEvent, UpdateSnapshot, updates_apply_scope,
    updates_check_background_from_config, updates_check_from_config,
    updates_resolve_conffile_conflict,
};

thread_local! {
    static SNAPSHOT: RefCell<UpdateSnapshot> = RefCell::new(UpdateSnapshot::default());
    /// One callback per live Updates bar widget. Replaced on re-register so bar
    /// rebuilds cannot accumulate hundreds of stale closures (main-thread jank).
    static REFRESHES: RefCell<Vec<std::rc::Rc<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    /// Optional overlay refresh — kept separate so bar rebuild caps cannot drop it.
    static UPDATER_REFRESH: RefCell<Option<std::rc::Rc<dyn Fn()>>> = const { RefCell::new(None) };
    static LAST_NOTIFIED_COUNT: Cell<usize> = const { Cell::new(0) };
    static CHECK_GEN: Cell<u64> = const { Cell::new(0) };
    static CHECK_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    static SERVICE_STARTED: Cell<bool> = const { Cell::new(false) };
    /// Wall-clock of the last completed check (any kind). Used to debounce.
    static LAST_CHECK_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// A forced re-check arrived while another check was running.
    static RECHECK_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Run the queued forced re-check once the in-flight check has finished.
fn drain_recheck_queue() {
    if RECHECK_QUEUED.replace(false) {
        run_check(false, true);
    }
}

/// Soft checks closer than this are skipped (cached snapshot wins).
const MIN_CHECK_GAP: Duration = Duration::from_secs(120);
/// First automatic check after login — keeps session start free of pkcon.
const FIRST_AUTO_CHECK_DELAY: Duration = Duration::from_secs(90);
/// How often we look at `last_check` to decide whether a check is due.
const AUTO_CHECK_POLL: Duration = Duration::from_secs(15 * 60);

fn snapshot_cache_path() -> PathBuf {
    glib::user_cache_dir()
        .join("metis")
        .join("updates-snapshot.json")
}

fn load_cached_snapshot() -> Option<UpdateSnapshot> {
    let text = std::fs::read_to_string(snapshot_cache_path()).ok()?;
    match serde_json::from_str(&text) {
        Ok(snap) => Some(snap),
        Err(err) => {
            tracing::warn!(%err, "updates snapshot cache unreadable — ignoring");
            None
        }
    }
}

fn save_cached_snapshot(snap: &UpdateSnapshot) {
    let path = snapshot_cache_path();
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, &path)
    })();
    if let Err(err) = result {
        tracing::warn!(%err, path = %path.display(), "could not cache updates snapshot");
    }
}

pub fn snapshot() -> UpdateSnapshot {
    SNAPSHOT.with(|s| s.borrow().clone())
}

pub fn pending_count() -> usize {
    SNAPSHOT.with(|s| s.borrow().total_count())
}

pub fn register_refresh(cb: std::rc::Rc<dyn Fn()>) {
    REFRESHES.with(|r| {
        let mut v = r.borrow_mut();
        // Multi-monitor: a few bars. Cap hard so rebuild leaks cannot grow forever.
        if v.len() >= 4 {
            v.clear();
        }
        v.push(cb);
    });
}

pub fn register_updater_refresh(cb: std::rc::Rc<dyn Fn()>) {
    UPDATER_REFRESH.with(|r| *r.borrow_mut() = Some(cb));
}

fn fire_refresh() {
    // Clone out first: a callback may rebuild the bar and re-register, which
    // would panic on a RefCell still borrowed by this loop.
    let bars: Vec<std::rc::Rc<dyn Fn()>> = REFRESHES.with(|r| r.borrow().clone());
    for cb in bars {
        cb();
    }
    let updater = UPDATER_REFRESH.with(|r| r.borrow().clone());
    if let Some(cb) = updater {
        cb();
    }
}

fn set_snapshot(snap: UpdateSnapshot) {
    save_cached_snapshot(&snap);
    SNAPSHOT.with(|s| *s.borrow_mut() = snap);
    fire_refresh();
}

/// Call once from the bar init path. Restores the cached snapshot (no
/// subprocesses), then schedules low-frequency PackageKit-only checks.
pub fn spawn_updates_service() {
    if SERVICE_STARTED.get() {
        return;
    }
    SERVICE_STARTED.set(true);
    if let Some(snap) = load_cached_snapshot() {
        SNAPSHOT.with(|s| *s.borrow_mut() = snap);
    }
    schedule_snooze_expiry_refresh(&load_updates_config());
    fire_refresh();

    glib::timeout_add_local_once(FIRST_AUTO_CHECK_DELAY, || {
        maybe_auto_check();
        glib::timeout_add_local(AUTO_CHECK_POLL, || {
            maybe_auto_check();
            glib::ControlFlow::Continue
        });
    });
}

/// Soft check when `check_interval_hours` have passed since the last one.
fn maybe_auto_check() {
    let cfg = load_updates_config();
    if !cfg.enabled {
        return;
    }
    let interval = chrono::Duration::hours(i64::from(cfg.check_interval_hours));
    let due = cfg
        .last_check
        .is_none_or(|at| chrono::Local::now().signed_duration_since(at) >= interval);
    // A restart with no cached list should repopulate the icon promptly.
    let empty_cache =
        SNAPSHOT.with(|s| s.borrow().total_count() == 0) && LAST_CHECK_AT.get().is_none();
    if due || empty_cache {
        request_check(false);
    }
}

/// Kick a background check.
///
/// - `manual == false`: PackageKit/distro list only, debounced, never Flatpak/fwupd.
/// - `manual == true`: full sources (Settings "Check now"). Still off the GTK thread.
pub fn request_check(manual: bool) {
    run_check(manual, false);
}

/// `force` (post-apply) bypasses the enabled flag and debounce, and queues
/// behind an in-flight check instead of being dropped.
fn run_check(manual: bool, force: bool) {
    let cfg = load_updates_config();
    if !manual && !force && !cfg.enabled {
        return;
    }
    if !manual
        && !force
        && let Some(at) = LAST_CHECK_AT.get()
        && at.elapsed() < MIN_CHECK_GAP
    {
        fire_refresh();
        return;
    }
    if CHECK_IN_FLIGHT.get() {
        if force {
            RECHECK_QUEUED.set(true);
        }
        return;
    }
    CHECK_IN_FLIGHT.set(true);
    let r#gen = CHECK_GEN.with(|g| {
        let n = g.get() + 1;
        g.set(n);
        n
    });
    let sources = cfg.sources.clone();
    let (tx, rx) = mpsc::channel::<Result<UpdateSnapshot, String>>();
    std::thread::spawn(move || {
        // Never call updates_refresh here — soft refresh shells out to pkcon /
        // flatpak / fwupd and can saturate the machine for minutes.
        let snap = if manual {
            updates_check_from_config(&UpdatesConfig {
                sources,
                ..Default::default()
            })
        } else {
            updates_check_background_from_config(&UpdatesConfig {
                sources,
                ..Default::default()
            })
        };
        let _ = tx.send(Ok(snap));
    });

    glib::timeout_add_local(Duration::from_millis(750), move || match rx.try_recv() {
        Ok(Ok(mut snap)) => {
            CHECK_IN_FLIGHT.set(false);
            LAST_CHECK_AT.set(Some(Instant::now()));
            if CHECK_GEN.with(|g| g.get()) != r#gen {
                drain_recheck_queue();
                return glib::ControlFlow::Break;
            }
            if !manual {
                // Background checks skip Flatpak / fwupd; keep what the last
                // full check found instead of silently dropping it.
                SNAPSHOT.with(|s| {
                    let prev = s.borrow();
                    snap.flatpaks = prev.flatpaks.clone();
                    snap.firmware = prev.firmware.clone();
                });
            }
            let count = snap.total_count();
            // Reload: Settings may have changed updates.json during the check.
            let mut cfg = load_updates_config();
            cfg.last_check = Some(chrono::Local::now());
            cfg.last_error = snap.error.clone();
            if let Err(err) = save_updates_config(&cfg) {
                tracing::warn!(%err, "could not save updates.json");
            }
            maybe_notify(&cfg, count);
            set_snapshot(snap);
            // Never auto-install from a soft check — that starts pkcon update
            // in the background and freezes the pointer under polkit.
            drain_recheck_queue();
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            CHECK_IN_FLIGHT.set(false);
            LAST_CHECK_AT.set(Some(Instant::now()));
            tracing::warn!(%err, "updates check failed");
            let mut cfg = load_updates_config();
            cfg.last_error = Some(err);
            if let Err(err) = save_updates_config(&cfg) {
                tracing::warn!(%err, "could not save updates.json");
            }
            fire_refresh();
            drain_recheck_queue();
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            CHECK_IN_FLIGHT.set(false);
            drain_recheck_queue();
            glib::ControlFlow::Break
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
        // Dismiss only. LAST_NOTIFIED_COUNT already stops a repeat toast for
        // the same set; hiding the bar icon is reserved for explicit snooze.
        "later" => true,
        _ => false,
    }
}

fn apply_snooze(set: impl FnOnce(&mut UpdatesConfig)) {
    let mut cfg = load_updates_config();
    set(&mut cfg);
    if let Err(err) = save_updates_config(&cfg) {
        tracing::warn!(%err, "could not save updates snooze");
    }
    schedule_snooze_expiry_refresh(&cfg);
    fire_refresh();
}

pub fn snooze_hours(hours: i64) {
    apply_snooze(|cfg| cfg.snooze_hours(hours));
}

pub fn snooze_tonight() {
    apply_snooze(UpdatesConfig::snooze_until_tonight);
}

pub fn snooze_one_day() {
    apply_snooze(UpdatesConfig::snooze_one_day);
}

/// Bar icon visibility. Only an explicit snooze (icon context menu) hides it.
pub fn is_visible() -> bool {
    pending_count() > 0 && !load_updates_config().is_snoozed()
}

/// Re-show the icon when a snooze ends (nothing else fires a refresh then).
fn schedule_snooze_expiry_refresh(cfg: &UpdatesConfig) {
    let Some(until) = cfg.snooze_until else {
        return;
    };
    let secs = until
        .signed_duration_since(chrono::Local::now())
        .num_seconds();
    if secs <= 0 {
        return;
    }
    let secs = u32::try_from(secs.saturating_add(1)).unwrap_or(u32::MAX);
    glib::timeout_add_seconds_local_once(secs, fire_refresh);
}

/// Apply pending updates (`scope` chooses all vs a subset).
pub fn start_apply_scope(
    scope: UpdateApplyScope,
    on_event: std::rc::Rc<dyn Fn(UpdateProgressEvent)>,
) {
    let cfg = load_updates_config();
    let sources = cfg.sources.clone();
    let (tx, rx) = mpsc::sync_channel::<UpdateProgressEvent>(64);
    let clear_all = matches!(scope, UpdateApplyScope::All);
    std::thread::spawn(move || {
        let _ = updates_apply_scope(&sources, &scope, Some(tx));
    });
    // Drain apply events promptly — a 64-deep channel + slow UI ticks made
    // large PackageKit runs look frozen even when progress was flowing.
    const MAX_PER_TICK: usize = 32;
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let mut processed = 0;
        loop {
            if processed >= MAX_PER_TICK {
                return glib::ControlFlow::Continue;
            }
            match rx.try_recv() {
                Ok(ev) => {
                    processed += 1;
                    let done = matches!(ev, UpdateProgressEvent::Finished { .. });
                    if matches!(ev, UpdateProgressEvent::Finished { ok: true, .. }) && clear_all {
                        // Apply covered every enabled source; the soft re-check
                        // below cannot see Flatpak / fwupd, so clear them here.
                        SNAPSHOT.with(|s| {
                            let mut snap = s.borrow_mut();
                            snap.flatpaks.clear();
                            snap.firmware.clear();
                        });
                    }
                    on_event(ev);
                    if done {
                        // Soft list only after apply — never kick Flatpak/fwupd here.
                        run_check(false, true);
                        return glib::ControlFlow::Break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    run_check(false, true);
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
}

/// Finish a pending dpkg conffile conflict (`keep` / `package`) via pkexec.
pub fn resolve_conffile(
    choice: ConfFileChoice,
    on_done: std::rc::Rc<dyn Fn(Result<(), String>)>,
) {
    let (tx, rx) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::spawn(move || {
        let _ = tx.send(updates_resolve_conffile_conflict(choice));
    });
    glib::timeout_add_local(Duration::from_millis(100), move || match rx.try_recv() {
        Ok(result) => {
            on_done(result);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            on_done(Err("configure cancelled".into()));
            glib::ControlFlow::Break
        }
    });
}
