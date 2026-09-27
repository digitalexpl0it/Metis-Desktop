//! Legacy notifications bell — opens the Notification Center (Phase 13).
//! Kept so existing `bar.json` entries with `notifications` still work; the
//! default layout no longer includes this widget (badge lives on the clock).

use gtk::prelude::*;

use crate::services::{BarNotification, do_not_disturb, notification_count, register_refresh};
use crate::ui::icons::{self, names};
use std::rc::Rc;

pub struct NotificationsWidget {
    root: gtk::Button,
    /// See [`ClockWidget::_notif_refresh`] — same Weak-hook lifetime bug.
    _notif_refresh: Rc<dyn Fn()>,
}

impl NotificationsWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-notifications");
        root.add_css_class("metis-bar-sys-icon");
        root.set_tooltip_text(Some(&metis_i18n::tr("Notifications")));

        let icon = icons::image(names::notification(do_not_disturb()));
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("metis-bar-notif-overlay");
        overlay.set_child(Some(&icon));

        let badge = gtk::Label::builder().label("").build();
        badge.add_css_class("metis-bar-notif-badge");
        badge.set_visible(false);
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::Start);
        overlay.add_overlay(&badge);
        root.set_child(Some(&overlay));

        root.connect_clicked(|_| {
            crate::ui::notification_center::toggle();
        });

        let refresh: Rc<dyn Fn()> = {
            let badge = badge.clone();
            let icon = icon.clone();
            Rc::new(move || {
                icons::set_icon(&icon, names::notification(do_not_disturb()));
                let total = notification_count();
                if do_not_disturb() || total == 0 {
                    badge.set_label("");
                    badge.set_visible(false);
                } else {
                    let label = if total > 99 {
                        "99+".to_string()
                    } else {
                        total.to_string()
                    };
                    badge.set_label(&label);
                    badge.set_visible(true);
                }
            })
        };
        register_refresh(refresh.clone());
        refresh();

        Self {
            root,
            _notif_refresh: refresh,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, _notifications: &[BarNotification]) {
        // Runtime store drives refresh hooks.
    }
}

/// Build a row of buttons for a notification using the detection rule:
/// labeled actions become one button each; otherwise a `desktop-entry` becomes a
/// single "Open" button. Returns `None` when the notification has neither.
pub(crate) fn build_action_row<F>(notif: &BarNotification, on_done: F) -> Option<gtk::Box>
where
    F: Fn() + Clone + 'static,
{
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    row.add_css_class("metis-notif-actions");
    row.set_margin_top(4);

    let mut any = false;
    for (key, label) in notif.labeled_actions() {
        let button = gtk::Button::with_label(label);
        button.add_css_class("metis-notif-action");
        let id = notif.id;
        let key = key.clone();
        let on_done = on_done.clone();
        button.connect_clicked(move |_| {
            crate::services::invoke_action(id, &key);
            crate::services::close_notification(id, 2);
            crate::services::dismiss_notification_by_dbus_id(id);
            on_done();
        });
        row.append(&button);
        any = true;
    }

    if !any && let Some(entry) = notif.desktop_entry.clone() {
        let button = gtk::Button::with_label(&metis_i18n::tr("Open"));
        button.add_css_class("metis-notif-action");
        button.add_css_class("suggested-action");
        let id = notif.id;
        let on_done = on_done.clone();
        button.connect_clicked(move |_| {
            open_desktop_entry(&entry);
            crate::services::close_notification(id, 2);
            crate::services::dismiss_notification_by_dbus_id(id);
            on_done();
        });
        row.append(&button);
        any = true;
    }

    any.then_some(row)
}

/// Focus a running window for `desktop_entry` when possible; otherwise launch.
pub(crate) fn open_desktop_entry(entry: &str) {
    use crate::services::applications::{self, entry_matches_app_id};
    use crate::services::windows;

    if let Some(app) = applications::resolve_entry_for_id(entry) {
        let snap = windows::snapshot();
        let mut matches: Vec<u32> = snap
            .windows
            .iter()
            .filter(|w| {
                w.app_id
                    .as_deref()
                    .is_some_and(|aid| entry_matches_app_id(&app, aid))
            })
            .map(|w| w.id)
            .collect();
        if !matches.is_empty() {
            // Prefer the most recently focused matching window.
            matches.sort_by_key(|id| {
                snap.focus_mru
                    .iter()
                    .position(|m| m == id)
                    .unwrap_or(usize::MAX)
            });
            let id = matches[0];
            if let Err(err) = crate::compositor::activate_window(id) {
                tracing::warn!(%err, id, "notify: failed to focus running app");
            } else {
                return;
            }
        }
        applications::launch_id(&app.id);
        return;
    }

    // Last resort: GIO launch (no window match without a resolved entry).
    use gio::prelude::*;
    let candidates = [entry.to_string(), format!("{entry}.desktop")];
    for id in candidates {
        if let Some(app) = gio_unix::DesktopAppInfo::new(&id) {
            match app.launch(&[], None::<&gio::AppLaunchContext>) {
                Ok(()) => return,
                Err(err) => tracing::warn!(%err, desktop = %id, "notify: failed to launch app"),
            }
        }
    }
    tracing::warn!(desktop = %entry, "notify: no .desktop entry found to open");
}

/// Icon for toast / notification-center cards: prefer the app's icon, then the
/// freedesktop `app_icon`, then a kind glyph. No circular glow — kind tint is
/// a soft gradient on the card background.
pub(crate) fn notif_icon_badge(note: &BarNotification) -> gtk::Box {
    use crate::services::applications;

    let wrap = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .build();
    wrap.add_css_class("metis-notif-icon-wrap");

    let image = gtk::Image::new();
    image.set_pixel_size(28);
    image.add_css_class("metis-notif-icon");

    let mut used_app = false;
    if let Some(de) = note.desktop_entry.as_deref()
        && let Some(entry) = applications::resolve_entry_for_id(de)
    {
        applications::set_app_icon(&image, &entry, 28);
        used_app = true;
    } else if let Some(icon) = note.app_icon.as_deref() {
        if icon.contains('/') {
            image.set_from_file(Some(std::path::Path::new(icon)));
            used_app = true;
        } else {
            image.set_icon_name(Some(icon));
            used_app = !icon.contains("dialog-") && !icon.contains("emblem-");
        }
    } else if note.app_name != "Metis"
        && let Some(entry) = applications::resolve_entry_for_id(&note.app_name)
    {
        applications::set_app_icon(&image, &entry, 28);
        used_app = true;
    } else {
        image.set_icon_name(Some(note.kind.icon_name()));
    }

    if used_app {
        image.add_css_class("metis-notif-icon-app");
    }
    wrap.append(&image);
    wrap
}
