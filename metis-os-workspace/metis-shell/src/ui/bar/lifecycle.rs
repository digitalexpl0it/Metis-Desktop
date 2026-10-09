//! Bar rebuild, config/theme watchers, service channels, and notifications.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use gtk::gio;
use gtk::prelude::*;

use crate::config::{BarConfig, load_bar_config};
use crate::services::{
    BarSnapshot, WeatherSnapshot, apply_event, last_weather_snapshot, refresh_taskbars,
    weather_refresh,
};

use super::autohide::{
    apply_bar_visibility, force_auto_hide_all, on_bar_edge_hover, on_bar_edge_leave,
    rearm_auto_hide_after_ui_dismiss,
};
use super::geometry::{
    apply_layer_geometry, configure_surface, layer_window_size, target_monitors,
};
use super::{
    BAR_POSITION, BARS, BarHandle, BarShell, REBUILD_SCHEDULED, build_bar, dropdown,
    ensure_bar_strip_geometry, remember_menu_layout, remount_bar_chrome, take_menu_layout_changed,
    widgets,
};

pub(crate) fn watch_monitors() {
    use gtk::gio::prelude::ListModelExt;
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    display
        .monitors()
        .connect_items_changed(move |_, _, _, _| rebuild_from_config());
}

/// Layer-shell surfaces do not receive outside-click events; the compositor
/// tells us to pop down when the pointer hits bare desktop.
pub(crate) fn watch_compositor_dismiss() {
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let path = metis_protocol::runtime_command_path();
        if let Ok(cmd) = std::fs::read_to_string(&path) {
            let parsed = match metis_protocol::parse_runtime_command(
                cmd.trim(),
                metis_protocol::BAR_RUNTIME_VERBS,
            ) {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!(%err, "rejected bar runtime command");
                    let _ = std::fs::remove_file(&path);
                    return glib::ControlFlow::Continue;
                }
            };
            if !metis_protocol::try_admit_runtime_command_dispatch() {
                tracing::warn!("bar runtime command dispatch rate limited");
                let _ = std::fs::remove_file(&path);
                return glib::ControlFlow::Continue;
            }
            let verb = parsed.verb;
            let arg = parsed.arg;
            match verb {
                "close-popovers" => {
                    // Sync close so auto-hide can re-arm on the same turn (idle
                    // popdown left `dropdown::is_open()` true and skipped hide).
                    // Task View stays open: it is sticky until Esc / activate /
                    // desktop click / backdrop click (not outside-click IPC).
                    dropdown::close_all();
                    crate::ui::dashboard::request_close();
                    crate::ui::notification_center::dismiss();
                    rearm_auto_hide_after_ui_dismiss();
                }
                "toggle-menu" => widgets::toggle_menu(),
                // Multimedia / hardware keys forwarded by the compositor
                // (`hw <action>`): volume, brightness, media transport, and the
                // display mirror/extend confirmation overlay.
                "hw" => {
                    let arg = arg.trim();
                    if let Some(mode) = arg.strip_prefix("osd-display") {
                        let mode = mode.trim();
                        let label = if mode == "mirror" {
                            "Mirror displays"
                        } else {
                            "Extend displays"
                        };
                        crate::ui::osd::show("video-display-symbolic", label, None, false);
                    } else if !crate::services::hardware::dispatch(arg) {
                        tracing::debug!(%arg, "unknown hardware key action");
                    }
                }
                "dismiss-screenshot" => crate::ui::screenshot::dismiss(),
                "window-switcher-next" | "task-view-next" => crate::ui::task_view::cycle_next(),
                "window-switcher-prev" | "task-view-prev" => crate::ui::task_view::cycle_prev(),
                "window-switcher-activate" | "task-view-activate" => {
                    crate::ui::task_view::activate_selected()
                }
                "dismiss-window-switcher" | "dismiss-task-view" | "dismiss-workspace-overview" => {
                    crate::ui::task_view::dismiss()
                }
                // Legacy verb: same as Super+Tab → open or cycle next.
                "workspace-overview" => crate::ui::task_view::cycle_next(),
                "reload-bar" => rebuild_from_config(),
                "reveal-edge-bar" | "bar-edge-hover" => on_bar_edge_hover(),
                "bar-edge-leave" => on_bar_edge_leave(),
                "reload-dashboard" => crate::ui::dashboard::on_dashboard_config_changed(),
                // Desktop widgets are isolated; Settings writes to command-widgets.
                "reload-desktop-widgets" => {
                    tracing::debug!("reload-desktop-widgets ignored in bar process");
                }
                "screenshot" => {
                    // Forms: `screenshot [CONNECTOR]`,
                    // `screenshot instant-full|window|record [CONNECTOR]`.
                    let mut parts = arg.split_whitespace();
                    let first = parts.next().unwrap_or("");
                    let (mode, connector) = match first {
                        "" => (crate::ui::screenshot::LaunchMode::Interactive, None),
                        "instant-full" => (
                            crate::ui::screenshot::LaunchMode::InstantFull,
                            parts.next().map(str::to_string),
                        ),
                        "window" => (
                            crate::ui::screenshot::LaunchMode::Window,
                            parts.next().map(str::to_string),
                        ),
                        "record" => (
                            crate::ui::screenshot::LaunchMode::Record,
                            parts.next().map(str::to_string),
                        ),
                        connector => (
                            crate::ui::screenshot::LaunchMode::Interactive,
                            Some(connector.to_string()),
                        ),
                    };
                    crate::ui::screenshot::show(mode, connector);
                }
                "reload-theme" => {
                    let _ = crate::ui::theme::init_theme();
                }
                "reload-graphics-profile" => {
                    // Compositor re-reads AppConfig on client spawn / animation
                    // checks; acknowledge so Settings does not leave a stale file.
                    tracing::debug!("graphics profile reload acknowledged");
                }
                "reload-weather" => {
                    if !crate::ui::onboarding::is_active() {
                        crate::services::weather::weather_refresh();
                    }
                }
                "reload-calendars" => crate::services::reload_calendars(),
                "reload-gaming" => {
                    let _ = metis_config::load_gaming_config();
                    let _ = crate::compositor::reload_gaming_config();
                }
                "reload-locale" => {
                    // Must recreate widgets — gettext reload alone leaves construction-time
                    // labels/tooltips in the old language. Do not go through
                    // `rebuild_from_config()`: that short-circuits to live geometry when
                    // bar.json is unchanged.
                    metis_i18n::reload();
                    let dir = if metis_i18n::is_rtl() {
                        gtk::TextDirection::Rtl
                    } else {
                        gtk::TextDirection::Ltr
                    };
                    gtk::Widget::set_default_direction(dir);
                    rebuild_for_locale();
                }
                "optimize-gaming" => {
                    // Require explicit `yes` (Settings confirm / metis-cmd --yes).
                    if arg.trim() == "yes"
                        || std::env::var_os("METIS_GAMING_OPTIMIZE_YES").is_some()
                    {
                        std::thread::spawn(|| {
                            let _ = metis_gaming::optimize_flatpak_gaming();
                            let _ = metis_gaming::ensure_steam_launcher();
                        });
                    } else {
                        tracing::warn!(
                            "optimize-gaming ignored without confirmation — use Settings → Gaming \
                             or: metis-cmd optimize-gaming --yes"
                        );
                    }
                }
                "show-onboarding" => crate::ui::onboarding::show(),
                "show-updater" => {
                    // Prefer any snapshot Settings just wrote before painting.
                    crate::services::updates_reload_snapshot_from_disk();
                    crate::ui::updater::show();
                }
                "reload-updates-snapshot" => {
                    crate::services::updates_reload_snapshot_from_disk();
                }
                "settings" => {
                    let program = if arg.trim().is_empty() {
                        "metis-settings".to_string()
                    } else {
                        format!("metis-settings --page {}", arg.trim())
                    };
                    if let Err(err) = crate::compositor::launch_program(&program) {
                        tracing::warn!(%err, "failed to launch metis-settings");
                    }
                }
                _ => {}
            }
            let _ = std::fs::remove_file(&path);
        }
        glib::ControlFlow::Continue
    });
}

/// Re-apply geometry/widgets to every existing bar in place (keeps the layer
/// surfaces — used for live theme/opacity/position edits that don't change the
/// set of outputs).
pub(crate) fn rebuild_bars_in_place(config: Rc<RefCell<BarConfig>>) {
    dropdown::close_all();
    // Closing CC before remount avoids a visible dash_host fighting the pill
    // for the fixed strip height/width after an edge change.
    crate::ui::dashboard::request_close();
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        let cfg = config.borrow();
        let (win_w, win_h) = layer_window_size(&cfg);
        for handle in bars.iter_mut() {
            configure_surface(&handle.outer, &handle.column, &handle.pill, &cfg);
            remount_bar_chrome(handle, &cfg);
            apply_layer_geometry(&handle.window, &cfg);
            let shell = BarShell {
                window: handle.window.clone(),
                outer: handle.outer.clone(),
                column: handle.column.clone(),
                host: handle.dash_host.clone(),
            };
            ensure_bar_strip_geometry(&shell);
            apply_bar_visibility(handle);
            handle.window.set_default_size(win_w, win_h);
            handle.outer.queue_resize();
            handle.column.queue_resize();
            handle.pill.queue_resize();
            while let Some(child) = handle.pill.first_child() {
                handle.pill.remove(&child);
            }
            handle.widget_refs =
                widgets::build(&handle.pill, config.clone(), handle.output.clone(), shell);
            rehydrate_widget_state(&handle.widget_refs);
        }
    });
    remember_menu_layout();
}

/// Re-apply cached service state after tearing down and rebuilding bar widgets.
pub(crate) fn rehydrate_widget_state(refs: &widgets::WidgetRefs) {
    if let Some(snapshot) = last_weather_snapshot() {
        refs.apply_weather(&snapshot);
    } else if !crate::ui::onboarding::is_active() {
        weather_refresh();
    }
    crate::services::sync_tray();
}

/// Whether a bar.json change requires destroying and recreating bar widgets.
fn needs_widget_rebuild(old: &BarConfig, new: &BarConfig) -> bool {
    old.widgets != new.widgets
        || old.position != new.position
        || old.displays != new.displays
        || old.clock != new.clock
        || old.workspace_count != new.workspace_count
}

/// Live-update geometry, CSS, and widget settings without closing popovers or
/// recreating widgets (opacity, tray mode, taskbar pins, margins, etc.).
fn apply_bars_live(config: Rc<RefCell<BarConfig>>) {
    let cfg = config.borrow().clone();
    BAR_POSITION.with(|p| p.set(cfg.position));
    let (win_w, win_h) = layer_window_size(&cfg);
    BARS.with(|bars| {
        for handle in bars.borrow_mut().iter_mut() {
            configure_surface(&handle.outer, &handle.column, &handle.pill, &cfg);
            apply_layer_geometry(&handle.window, &cfg);
            let shell = BarShell {
                window: handle.window.clone(),
                outer: handle.outer.clone(),
                column: handle.column.clone(),
                host: handle.dash_host.clone(),
            };
            ensure_bar_strip_geometry(&shell);
            apply_bar_visibility(handle);
            handle.window.set_default_size(win_w, win_h);
            handle.outer.queue_resize();
            handle.column.queue_resize();
            handle.pill.queue_resize();
            handle.widget_refs.apply_bar_config(&cfg);
            rehydrate_widget_state(&handle.widget_refs);
        }
    });
    refresh_taskbars();
}

/// Force-recreate edge-bar chrome (and dependent overlays) after a language change.
fn rebuild_for_locale() {
    if crate::ui::onboarding::is_active() {
        tracing::debug!("locale bar rebuild deferred — onboarding active");
        return;
    }
    let config = BARS.with(|bars| bars.borrow().first().map(|h| h.config.clone()));
    let Some(config) = config else {
        return;
    };
    rebuild_bars_in_place(config);
    crate::ui::theme::reload_stylesheet();
    crate::ui::notification_center::reload_for_locale();
    crate::ui::dashboard::reload_for_locale();
    // Desktop widgets process watches locale.json / its own command file.
    // Re-apply weather with freshly translated condition/hour strings.
    if let Some(snapshot) = last_weather_snapshot() {
        BARS.with(|bars| {
            for handle in bars.borrow().iter() {
                handle.widget_refs.apply_weather(&snapshot);
            }
        });
    }
}

/// Tear down all bars and rebuild from scratch for the current monitor set —
/// used when the number of target outputs changes (monitor hotplug or toggling
/// the `displays` option).
fn rebuild_all_bars(config: Rc<RefCell<BarConfig>>) {
    dropdown::close_all();
    let cfg = config.borrow().clone();
    // Build the new bars *before* destroying the old ones to avoid a one-frame
    // flash with no bar on screen. (The shell runs its own GLib main loop, so an
    // empty window set never quits it.)
    let new_handles: Vec<BarHandle> = target_monitors(&cfg)
        .iter()
        .map(|m| build_bar(config.clone(), m.as_ref()))
        .collect();
    let old = BARS.with(|bars| std::mem::replace(&mut *bars.borrow_mut(), new_handles));
    for handle in old {
        handle.window.destroy();
    }
    remember_menu_layout();
}

pub(crate) fn watch_bar_config() {
    let path = crate::config::bar_config_path();
    if !path.exists()
        && let Err(err) = crate::config::save_default_bar_config()
    {
        tracing::warn!(%err, "failed to create default bar.json");
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "bar.json file monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            // Re-reads bar.json and rebuilds in place, or recreates the surfaces if
            // the `displays` option changed the number of bars.
            rebuild_from_config();
        });
    });
}

pub(crate) fn watch_dashboard_config() {
    let path = crate::config::dashboard_config_path();
    if !path.exists()
        && let Err(err) = crate::config::save_default_dashboard_config()
    {
        tracing::warn!(%err, "failed to create default dashboard.json");
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "dashboard.json file monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            crate::ui::dashboard::on_dashboard_config_changed();
        });
    });
}

pub(crate) fn spawn_gaming_daemon() {
    if std::env::var_os("METIS_NO_GAMINGD").is_some() {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("metis-gamingd")))
        .filter(|p| p.is_file());
    let exe = exe.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("metis-gamingd"))
                .find(|p| p.is_file())
        })
    });
    let Some(exe) = exe else {
        tracing::debug!("metis-gamingd not found beside shell or on PATH");
        return;
    };
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => tracing::info!(path = %exe.display(), "spawned metis-gamingd"),
        Err(err) => tracing::warn!(%err, "failed to spawn metis-gamingd"),
    }
}

/// Live-reload the active theme when any `themes/*.json` changes. Mirrors
/// `watch_bar_config`: the GFileMonitor stays alive via the main-context source,
/// and the debounced callback re-runs `init_theme()` (which re-reads the active
/// mode + on-disk token file and re-applies the CssProvider).
pub(crate) fn watch_theme_files() {
    let dir = crate::config::config_dir().join("themes");
    if let Err(err) = crate::config::ensure_config_dirs() {
        tracing::warn!(%err, "failed to ensure themes dir");
    }
    let file = gio::File::for_path(&dir);
    let Ok(monitor) =
        file.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %dir.display(), "themes dir monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            let _ = crate::ui::theme::init_theme();
        });
    });
}

pub(crate) fn attach_weather_channel(rx: Receiver<WeatherSnapshot>) {
    glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
        while let Ok(snapshot) = rx.try_recv() {
            tracing::debug!(
                locations = snapshot.locations.len(),
                error = ?snapshot.error,
                "weather: UI received snapshot"
            );
            // Cache on the GTK thread too (worker already stores process-wide).
            crate::services::weather::remember_snapshot(&snapshot);
            BARS.with(|bars| {
                for handle in bars.borrow().iter() {
                    handle.widget_refs.apply_weather(&snapshot);
                }
            });
        }
        glib::ControlFlow::Continue
    });
}

pub(crate) fn attach_notification_channel(channels: crate::services::NotifyChannels) {
    let crate::services::NotifyChannels { incoming, actions } = channels;
    // Register the outgoing sender so popover/toast buttons can round-trip
    // ActionInvoked / NotificationClosed back to the originating apps.
    crate::services::set_action_sender(actions);

    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        while let Ok(event) = incoming.try_recv() {
            match event {
                crate::services::NotifyIncoming::Show(note) => {
                    let note = *note;
                    let dnd = widgets::do_not_disturb();
                    if !dnd {
                        if !note.suppress_sound {
                            crate::services::play_notification_sound(&note);
                        }
                        crate::ui::toast::show(&note);
                    }
                    crate::services::push_notification(note);
                }
                crate::services::NotifyIncoming::Closed { id } => {
                    crate::services::dismiss_notification_by_dbus_id(id);
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

pub(crate) fn attach_tray_channel(events: std::sync::mpsc::Receiver<crate::services::TrayEvent>) {
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(event) = events.try_recv() {
            apply_event(event);
        }
        glib::ControlFlow::Continue
    });
}

pub(crate) fn attach_poll_channel(rx: Receiver<BarSnapshot>) {
    let mut last = BarSnapshot::default();
    glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
        while let Ok(snapshot) = rx.try_recv() {
            if snapshot == last {
                continue;
            }
            last = snapshot.clone();
            check_bluetooth_battery_alerts(&snapshot.bluetooth);
            BARS.with(|bars| {
                for handle in bars.borrow().iter() {
                    handle.widget_refs.apply_snapshot(&snapshot);
                }
            });
        }
        glib::ControlFlow::Continue
    });
}

/// Charge level (inclusive) at or below which a connected Bluetooth device is
/// considered "low" and worth a charge reminder.
const BT_BATTERY_LOW: u8 = 20;
/// Charge level a device must climb back above before it can alert again, so a
/// reading hovering around the threshold can't spam repeated notifications.
const BT_BATTERY_CLEAR: u8 = 25;

thread_local! {
    /// Per-device (by MAC) latch: `true` once we've fired a low-battery alert,
    /// cleared when the device recharges past `BT_BATTERY_CLEAR` or disconnects.
    static BT_LOW_ALERTED: std::cell::RefCell<std::collections::HashMap<String, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Fire a one-shot charge reminder when a connected Bluetooth device's battery
/// drops to a low level. Uses hysteresis + a per-device latch so each low
/// episode notifies exactly once, and prunes state for disconnected devices.
fn check_bluetooth_battery_alerts(status: &crate::services::BluetoothStatus) {
    use crate::services::{BarNotification, NotificationKind};

    BT_LOW_ALERTED.with(|cell| {
        let mut latched = cell.borrow_mut();
        latched.retain(|addr, _| status.devices.iter().any(|d| &d.address == addr));

        for dev in &status.devices {
            let Some(pct) = dev.battery_percent else {
                continue;
            };
            // Don't nag to charge a device that's already charging.
            if dev.battery_charging == Some(true) {
                latched.insert(dev.address.clone(), false);
                continue;
            }
            let already = latched.get(&dev.address).copied().unwrap_or(false);
            if pct <= BT_BATTERY_LOW && !already {
                latched.insert(dev.address.clone(), true);
                let mut note = BarNotification::internal(
                    NotificationKind::Error,
                    format!("{} battery low", dev.name),
                    format!("{pct}% remaining — charge it soon."),
                );
                note.sound_name = Some("battery-low".to_string());
                emit_internal_notification(note);
            } else if pct >= BT_BATTERY_CLEAR && already {
                latched.insert(dev.address.clone(), false);
            }
        }
    });
}

/// Deliver a Metis-originated notification through the same path as incoming
/// D-Bus ones: play a sound and show a toast (unless Do Not Disturb is on), then
/// store it in the in-bar notification list.
fn emit_internal_notification(note: crate::services::BarNotification) {
    if !widgets::do_not_disturb() {
        if !note.suppress_sound {
            crate::services::play_notification_sound(&note);
        }
        crate::ui::toast::show(&note);
    }
    crate::services::push_notification(note);
}

/// Software-updates toast + notification-center card (Install / Later actions).
pub fn emit_updates_notification(note: crate::services::BarNotification) {
    emit_internal_notification(note);
}

/// Toast + notification-center card when a monitor is plugged or unplugged.
pub fn notify_display_hotplug(connected: bool, name: &str, make: &str, model: &str) {
    use crate::services::{BarNotification, NotificationKind};

    // DRM modeset around hotplug can make NetworkManager drop the association
    // briefly — never write `nmcli radio wifi off` from that flake.
    crate::services::suppress_wifi_radio_writes(20);

    let pretty = {
        let branded = format!("{} {}", make.trim(), model.trim())
            .trim()
            .to_string();
        if branded.is_empty()
            || branded.eq_ignore_ascii_case("unknown unknown")
            || branded.eq_ignore_ascii_case("unknown")
        {
            name.to_string()
        } else {
            branded
        }
    };

    let (title, message, osd_label) = if connected {
        (
            metis_i18n::tr("Display connected"),
            metis_i18n::tr("%1 is ready to use.").replace("%1", &pretty),
            metis_i18n::tr("Display connected"),
        )
    } else {
        (
            metis_i18n::tr("Display disconnected"),
            metis_i18n::tr("%1 was unplugged.").replace("%1", &pretty),
            metis_i18n::tr("Display disconnected"),
        )
    };

    crate::ui::osd::show("video-display-symbolic", &osd_label, None, false);

    let mut note = BarNotification::internal(NotificationKind::Notification, title, message);
    note.sound_name = Some(if connected {
        "device-added".to_string()
    } else {
        "device-removed".to_string()
    });
    emit_internal_notification(note);
}

/// Mirror a user audio change (volume/mic/mute) onto every bar immediately, so a
/// multi-monitor session doesn't wait for the pactl poll round-trip to update the
/// other displays' volume icons/sliders.
pub fn broadcast_audio(percent: u8, muted: bool, mic_percent: u8, mic_muted: bool) {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            handle
                .widget_refs
                .apply_volume_optimistic(percent, muted, mic_percent, mic_muted);
        }
    });
}

/// Close all bar dropdown popovers (e.g. before a bar surface rebuild).
pub fn close_popovers() {
    dropdown::request_close_all();
    crate::ui::notification_center::dismiss();
}

pub fn rebuild_from_config() {
    if crate::ui::onboarding::is_active() {
        tracing::debug!("bar rebuild deferred — onboarding active");
        return;
    }
    // Coalesce bursts: one settings change writes bar.json (which can emit several
    // file-change events) *and* sends a `reload-bar` runtime command, so multiple
    // rebuild triggers land within a few hundred ms. Collapsing them into a single
    // deferred rebuild avoids overlapping teardown/rebuild passes racing each
    // other (and the deferred surface present) while bars are being recreated.
    if REBUILD_SCHEDULED.with(|f| f.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(std::time::Duration::from_millis(80), || {
        REBUILD_SCHEDULED.with(|f| f.set(false));
        if crate::ui::onboarding::is_active() {
            tracing::debug!("bar rebuild deferred — onboarding active");
            return;
        }
        rebuild_from_config_now();
    });
}

/// Apply the current `bar.json` immediately. Called after onboarding dismisses
/// its overlay window so bar layer surfaces are not rebuilt concurrently.
pub fn apply_bar_config_now() {
    let (config, cur_count) = BARS.with(|bars| {
        let bars = bars.borrow();
        (bars.first().map(|h| h.config.clone()), bars.len())
    });
    let Some(config) = config else {
        return;
    };
    let old = config.borrow().clone();
    let new = load_bar_config();
    apply_config_diff(config, cur_count, &old, &new);
}

fn apply_config_diff(
    config: Rc<RefCell<BarConfig>>,
    cur_count: usize,
    old: &BarConfig,
    new: &BarConfig,
) {
    let enabling_auto_hide = new.auto_hide && !old.auto_hide;
    let menu_layout_changed = take_menu_layout_changed();
    *config.borrow_mut() = new.clone();
    let target_count = target_monitors(new).len();
    if target_count != cur_count {
        rebuild_all_bars(config.clone());
    } else if menu_layout_changed || needs_widget_rebuild(old, new) {
        rebuild_bars_in_place(config.clone());
    } else {
        apply_bars_live(config.clone());
    }
    crate::ui::theme::reload_stylesheet();
    // Toggle ON from Settings: hide immediately so the user sees the effect
    // without waiting for a pointer leave (and despite a stale pointer_over).
    if enabling_auto_hide {
        force_auto_hide_all();
    }
}

fn rebuild_from_config_now() {
    let (config, cur_count) = BARS.with(|bars| {
        let bars = bars.borrow();
        (bars.first().map(|h| h.config.clone()), bars.len())
    });
    let Some(config) = config else {
        return;
    };
    let old = config.borrow().clone();
    let new = load_bar_config();
    apply_config_diff(config, cur_count, &old, &new);
    crate::ui::dashboard::on_bar_config_changed();
    crate::ui::notification_center::on_bar_config_changed();
}
