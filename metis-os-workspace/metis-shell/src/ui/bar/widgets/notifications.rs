//! Legacy notifications bell — opens the Notification Center (Phase 13).
//! Kept so existing `bar.json` entries with `notifications` still work; the
//! default layout no longer includes this widget (badge lives on the clock).

use std::rc::Rc;

use gtk::prelude::*;

use crate::services::{BarNotification, do_not_disturb, notification_count, register_refresh};
use crate::ui::icons::{self, names};

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

/// Collapsed preview budgets (GTK `Label::lines` is unreliable for wrap+hexpand).
const COLLAPSED_TITLE_CHARS: usize = 40;
const COLLAPSED_BODY_CHARS: usize = 96;

/// Whether title/body are long enough that a clamp + expand control helps.
pub(crate) fn notification_needs_expand(title: &str, message: &str) -> bool {
    title.chars().count() > COLLAPSED_TITLE_CHARS
        || message.chars().count() > COLLAPSED_BODY_CHARS
        || title.contains('\n')
        || message.contains('\n')
}

fn ellipsize_chars(s: &str, max: usize) -> String {
    let mut iter = s.chars();
    let head: String = iter.by_ref().take(max).collect();
    if iter.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn apply_expand_state(
    title: &gtk::Label,
    message: Option<&gtk::Label>,
    full_title: &str,
    full_message: &str,
    expanded: bool,
) {
    if expanded {
        title.set_label(full_title);
        title.set_ellipsize(gtk::pango::EllipsizeMode::None);
        title.set_wrap(true);
        title.set_lines(0);
        if let Some(msg) = message {
            msg.set_label(full_message);
            msg.set_ellipsize(gtk::pango::EllipsizeMode::None);
            msg.set_wrap(true);
            msg.set_lines(0);
            msg.queue_resize();
        }
    } else {
        title.set_label(&ellipsize_chars(full_title, COLLAPSED_TITLE_CHARS));
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_wrap(false);
        title.set_lines(1);
        if let Some(msg) = message {
            msg.set_label(&ellipsize_chars(full_message, COLLAPSED_BODY_CHARS));
            msg.set_ellipsize(gtk::pango::EllipsizeMode::End);
            msg.set_wrap(true);
            msg.set_lines(2);
            msg.queue_resize();
        }
    }
    title.queue_resize();
}

/// Three-dot control that swaps truncated ↔ full title/body. Returns `None` when
/// the content is short enough to show in full.
pub(crate) fn build_expand_toggle(
    title: &gtk::Label,
    message: Option<&gtk::Label>,
) -> Option<gtk::Button> {
    let full_title = title.label().to_string();
    let full_message = message.map(|m| m.label().to_string()).unwrap_or_default();
    if !notification_needs_expand(&full_title, &full_message) {
        return None;
    }

    apply_expand_state(title, message, &full_title, &full_message, false);

    let expand = gtk::Button::from_icon_name("view-more-horizontal-symbolic");
    expand.add_css_class("flat");
    expand.add_css_class("metis-notif-expand");
    expand.set_tooltip_text(Some(&metis_i18n::tr("Expand")));
    expand.set_valign(gtk::Align::Start);
    expand.set_focus_on_click(false);

    let expanded = Rc::new(std::cell::Cell::new(false));
    {
        let title = title.clone();
        let message = message.cloned();
        let expanded = expanded.clone();
        let full_title = full_title.clone();
        let full_message = full_message.clone();
        expand.connect_clicked(move |btn| {
            let next = !expanded.get();
            expanded.set(next);
            apply_expand_state(&title, message.as_ref(), &full_title, &full_message, next);
            // Resize card / list row so the panel grows with the expanded body.
            let mut walk = btn.parent();
            while let Some(parent) = walk {
                parent.queue_resize();
                walk = parent.parent();
            }
            btn.set_tooltip_text(Some(&if next {
                metis_i18n::tr("Collapse")
            } else {
                metis_i18n::tr("Expand")
            }));
        });
    }

    Some(expand)
}

/// Per-card dismiss (X) for Notification Center / toast title rows.
pub(crate) fn build_dismiss_button<F>(on_dismiss: F) -> gtk::Button
where
    F: Fn() + 'static,
{
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat");
    close.add_css_class("metis-notif-dismiss");
    close.set_tooltip_text(Some(&metis_i18n::tr("Dismiss")));
    close.set_valign(gtk::Align::Start);
    close.set_focus_on_click(false);
    close.connect_clicked(move |_| on_dismiss());
    close
}

/// True when `widget` is (or is inside) a button — used to ignore card-level
/// default-action clicks that land on expand / dismiss / Open controls.
pub(crate) fn widget_is_buttonish(widget: &impl IsA<gtk::Widget>) -> bool {
    let mut current = Some(widget.clone().upcast::<gtk::Widget>());
    while let Some(node) = current {
        if node.downcast_ref::<gtk::Button>().is_some()
            || node.downcast_ref::<gtk::ToggleButton>().is_some()
            || node.downcast_ref::<gtk::MenuButton>().is_some()
        {
            return true;
        }
        current = node.parent();
    }
    false
}

/// Left-edge kind strip (colour set by `.metis-notif-card-* .metis-notif-accent`).
pub(crate) fn notif_kind_accent() -> gtk::Box {
    let accent = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(false)
        .vexpand(true)
        .build();
    accent.add_css_class("metis-notif-accent");
    accent
}

/// Icon for toast / notification-center cards: prefer the app's icon, then the
/// freedesktop `app_icon`, then a kind glyph. Kind colour tints the glyph only;
/// cards use a solid fill with border + shadow.
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
