pub(crate) mod alarms;
pub(crate) mod calendar;
pub(crate) mod stopwatch;
pub(crate) mod timer;
pub(crate) mod world;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration as StdDuration;

use chrono::{Datelike, Local, Timelike};
use gtk::prelude::*;

use crate::config::{ClockConfig, ClocksConfig, alarm_sound_canberra_id, load_clocks_config};
use crate::services::{do_not_disturb, notification_count, register_refresh};
use crate::ui::icons::{self, names};

/// Shared, persisted clock state (world clocks + alarms) used by clock pages.
#[derive(Clone)]
pub(crate) struct Store(pub(crate) Rc<RefCell<ClocksConfig>>);

impl Store {
    pub(crate) fn save(&self) {
        let _ = crate::config::save_clocks_config(&self.0.borrow());
    }
    pub(crate) fn borrow(&self) -> std::cell::Ref<'_, ClocksConfig> {
        self.0.borrow()
    }
    pub(crate) fn borrow_mut(&self) -> std::cell::RefMut<'_, ClocksConfig> {
        self.0.borrow_mut()
    }
}

/// Raise a notification in Metis's notification store (+ toast when not DND).
pub(crate) fn notify(title: &str, body: &str) {
    crate::services::push_notification(crate::services::BarNotification::internal(
        crate::services::NotificationKind::Notification,
        title,
        body,
    ));
}

/// Best-effort alarm sound; degrades silently if no player/sound is present.
#[allow(dead_code)]
pub(crate) fn play_alarm_sound() {
    play_alarm_sound_id("alarm-clock-elapsed");
}

/// Play a specific libcanberra event sound, falling back to a bundled oga file.
pub(crate) fn play_alarm_sound_id(canberra_id: &str) {
    if std::process::Command::new("canberra-gtk-play")
        .args(["-i", canberra_id])
        .spawn()
        .is_ok()
    {
        return;
    }
    let _ = std::process::Command::new("paplay")
        .arg("/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga")
        .spawn();
}

pub struct ClockWidget {
    root: gtk::Button,
    /// Keeps the notification badge refresh hook alive for `register_refresh`.
    /// Without this, the `Rc` is dropped at the end of `new` and the Weak in the
    /// notify store is pruned — the badge then freezes at whatever count was
    /// painted on the last construction-time refresh (often after a bar rebuild).
    _notif_refresh: Rc<dyn Fn()>,
}

impl ClockWidget {
    pub fn new(config: &ClockConfig, compact: bool) -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-clock");
        if compact {
            root.add_css_class("metis-bar-clock-compact");
        }

        let time_label = gtk::Label::new(None);
        time_label.add_css_class("metis-bar-clock-time");
        let date_label = gtk::Label::new(None);
        date_label.add_css_class("metis-bar-clock-date");

        // Bell + count: shown only while unread notifications exist.
        let bell_icon = icons::image(names::notification(do_not_disturb()));
        bell_icon.add_css_class("metis-bar-clock-bell");
        let bell_overlay = gtk::Overlay::new();
        bell_overlay.add_css_class("metis-bar-notif-overlay");
        bell_overlay.set_child(Some(&bell_icon));
        let badge = gtk::Label::builder().label("").build();
        badge.add_css_class("metis-bar-notif-badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::Start);
        bell_overlay.add_overlay(&badge);
        let bell_wrap = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        bell_wrap.add_css_class("metis-bar-clock-bell-wrap");
        bell_wrap.append(&bell_overlay);
        bell_wrap.set_visible(false);

        let content = gtk::Box::builder()
            .orientation(if compact {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            })
            .spacing(if compact { 2 } else { 8 })
            .build();

        if compact {
            let icon = icons::image("preferences-system-time-symbolic");
            icon.add_css_class("metis-bar-clock-icon");
            content.append(&icon);
            content.append(&bell_wrap);
            root.set_tooltip_text(Some(&metis_i18n::tr("Clock & notifications")));
        } else {
            content.append(&time_label);
            content.append(&date_label);
            content.append(&bell_wrap);
        }

        root.set_child(Some(&content));

        root.connect_clicked(|_| {
            crate::ui::notification_center::toggle();
        });

        let badge_refresh = badge.clone();
        let bell_wrap_refresh = bell_wrap.clone();
        let bell_icon_refresh = bell_icon.clone();
        let refresh: Rc<dyn Fn()> = Rc::new(move || {
            let total = notification_count();
            let dnd = do_not_disturb();
            icons::set_icon(&bell_icon_refresh, names::notification(dnd));
            if total == 0 {
                badge_refresh.set_visible(false);
                bell_wrap_refresh.set_visible(false);
                return;
            }
            bell_wrap_refresh.set_visible(true);
            // Cap the badge so a huge backlog stays readable in the strip.
            let label = if total > 99 {
                "99+".to_string()
            } else {
                total.to_string()
            };
            badge_refresh.set_label(&label);
            badge_refresh.set_visible(true);
        });
        register_refresh(refresh.clone());
        refresh();

        // Alarm tick even when the Notification Center is closed. Reloads
        // `clock.json` each minute so alarms edited in the center are picked up.
        let seed_tz = config.timezones.clone();
        let last_minute = Rc::new(std::cell::Cell::new(Local::now().timestamp() / 60));
        glib::timeout_add_local(StdDuration::from_secs(1), {
            let last_minute = last_minute.clone();
            let time_label = time_label.clone();
            let date_label = date_label.clone();
            let root_for_tip = root.clone();
            let cfg = config.clone();
            let seed_tz = seed_tz.clone();
            move || {
                if compact {
                    update_bar_tooltip(&root_for_tip, &cfg);
                } else {
                    update_bar_labels(&time_label, &date_label, &cfg);
                }
                let minute = Local::now().timestamp() / 60;
                if minute != last_minute.get() {
                    last_minute.set(minute);
                    let store = Store(Rc::new(RefCell::new(load_clocks_config(&seed_tz))));
                    check_alarms(&store);
                }
                glib::ControlFlow::Continue
            }
        });

        if compact {
            update_bar_tooltip(&root, config);
        } else {
            update_bar_labels(&time_label, &date_label, config);
        }

        Self {
            root,
            _notif_refresh: refresh,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }
}

fn update_bar_labels(time_label: &gtk::Label, date_label: &gtk::Label, config: &ClockConfig) {
    let now = Local::now();
    time_label.set_label(&now.format(&config.time_format).to_string());
    date_label.set_label(&now.format(&config.date_format).to_string());
}

fn update_bar_tooltip(root: &gtk::Button, config: &ClockConfig) {
    let now = Local::now();
    let tip = format!(
        "{}\n{}",
        now.format(&config.time_format),
        now.format(&config.date_format)
    );
    root.set_tooltip_text(Some(&tip));
}

fn check_alarms(store: &Store) {
    let now = Local::now();
    let weekday = now.weekday().num_days_from_sunday() as u8;
    let hour = now.hour() as u8;
    let minute = now.minute() as u8;
    let due: Vec<(String, &'static str)> = store
        .borrow()
        .alarms
        .iter()
        .filter(|a| a.enabled && a.hour == hour && a.minute == minute)
        .filter(|a| a.days.is_empty() || a.days.contains(&weekday))
        .map(|a| {
            let label = if a.label.is_empty() {
                metis_i18n::tr("Alarm %1").replace("%1", &format!("{:02}:{:02}", a.hour, a.minute))
            } else {
                a.label.clone()
            };
            (label, alarm_sound_canberra_id(a.sound.as_deref()))
        })
        .collect();
    for (label, sound) in due {
        notify(&label, &metis_i18n::tr("Metis alarm"));
        play_alarm_sound_id(sound);
    }
}
