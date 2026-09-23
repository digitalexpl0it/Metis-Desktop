//! Notifications card for the Notification Center panel.
//!
//! Header (title, DND, Clear all, collapse chevron) always stays visible.
//! The notification list collapses when empty or when the user toggles the chevron.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::services::{
    BarNotification, NotificationEntry, NotificationKind, clear_notifications, do_not_disturb,
    notify_store_changed, register_refresh, runtime_notifications, set_do_not_disturb,
};
use crate::ui::bar::widgets::build_action_row;

const SLIDE_MS: u32 = 120;

pub struct NotificationsCard {
    root: gtk::Widget,
    scrolled: gtk::ScrolledWindow,
    refresh: Rc<dyn Fn()>,
}

impl NotificationsCard {
    pub fn new() -> Self {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        card.add_css_class("metis-nc-card");

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();

        let collapse_btn = gtk::ToggleButton::new();
        collapse_btn.set_icon_name("pan-down-symbolic");
        collapse_btn.add_css_class("metis-nc-collapse");
        collapse_btn.set_tooltip_text(Some(&metis_i18n::tr("Collapse")));
        collapse_btn.set_active(true);

        let title = gtk::Label::builder()
            .label(metis_i18n::tr("Notifications"))
            .hexpand(true)
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("metis-nc-card-title");

        let dnd_label = gtk::Label::builder()
            .label(metis_i18n::tr("DND"))
            .halign(gtk::Align::End)
            .build();
        dnd_label.add_css_class("metis-notif-dnd-label");

        let dnd_switch = gtk::Switch::new();
        dnd_switch.set_active(do_not_disturb());
        dnd_switch.set_valign(gtk::Align::Center);
        dnd_switch.add_css_class("metis-nc-switch");

        let clear_btn = gtk::Button::with_label(&metis_i18n::tr("Clear all"));
        clear_btn.add_css_class("metis-notif-clear");
        clear_btn.add_css_class("metis-nc-btn");

        header.append(&collapse_btn);
        header.append(&title);
        header.append(&dnd_label);
        header.append(&dnd_switch);
        header.append(&clear_btn);
        card.append(&header);

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();

        let (notif_max, _) = crate::ui::notification_center::scroll_budgets();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(80)
            .max_content_height(notif_max)
            .propagate_natural_height(true)
            .overlay_scrolling(false)
            .child(&list)
            .build();
        scrolled.add_css_class("metis-notif-scrolled");
        scrolled.add_css_class("metis-nc-scrolled");

        let body = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(SLIDE_MS)
            .reveal_child(true)
            .child(&scrolled)
            .build();
        card.append(&body);

        // User-collapsed vs auto-empty: empty forces body closed; non-empty
        // respects the chevron.
        let user_expanded = Rc::new(Cell::new(true));

        let refresh: Rc<dyn Fn()> = {
            let list = list.clone();
            let clear_btn = clear_btn.clone();
            let body = body.clone();
            let collapse_btn = collapse_btn.clone();
            let user_expanded = user_expanded.clone();
            Rc::new(move || {
                let entries = runtime_notifications();
                let empty = entries.is_empty();
                clear_btn.set_sensitive(!empty);
                fill_list(&list, &entries);
                let show = !empty && user_expanded.get();
                body.set_reveal_child(show);
                collapse_btn.set_sensitive(!empty);
                if empty {
                    collapse_btn.set_active(false);
                    collapse_btn.set_icon_name("pan-end-symbolic");
                } else {
                    collapse_btn.set_active(user_expanded.get());
                    collapse_btn.set_icon_name(if user_expanded.get() {
                        "pan-down-symbolic"
                    } else {
                        "pan-end-symbolic"
                    });
                }
            })
        };

        collapse_btn.connect_toggled({
            let body = body.clone();
            let user_expanded = user_expanded.clone();
            let collapse_btn = collapse_btn.clone();
            move |btn| {
                if !btn.is_sensitive() {
                    return;
                }
                let expanded = btn.is_active();
                user_expanded.set(expanded);
                body.set_reveal_child(expanded);
                collapse_btn.set_icon_name(if expanded {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
                collapse_btn.set_tooltip_text(Some(&if expanded {
                    metis_i18n::tr("Collapse")
                } else {
                    metis_i18n::tr("Expand")
                }));
            }
        });

        dnd_switch.connect_state_set({
            let refresh = refresh.clone();
            move |_, state| {
                set_do_not_disturb(state);
                refresh();
                notify_store_changed();
                glib::Propagation::Proceed
            }
        });

        clear_btn.connect_clicked({
            let list = list.clone();
            move |_| animate_clear(&list)
        });

        register_refresh(refresh.clone());
        refresh();

        Self {
            root: card.upcast(),
            scrolled,
            refresh,
        }
    }

    pub fn root(&self) -> &gtk::Widget {
        &self.root
    }

    pub fn refresh(&self) {
        (self.refresh)();
    }

    /// Cap the notification list height for the current monitor.
    pub fn set_list_max_height(&self, max_h: i32) {
        self.scrolled.set_max_content_height(max_h.max(80));
    }
}

fn animate_clear(list: &gtk::Box) {
    // Clear the store first so the bar badge drops immediately. A delayed clear
    // after the slide animation left the badge stuck whenever the clock refresh
    // hook had been pruned (or the panel closed mid-animation).
    clear_notifications();

    let mut child = list.first_child();
    while let Some(c) = child {
        let next = c.next_sibling();
        if let Ok(rev) = c.clone().downcast::<gtk::Revealer>() {
            rev.set_reveal_child(false);
        }
        child = next;
    }
}

fn fill_list(list: &gtk::Box, entries: &[NotificationEntry]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    if entries.is_empty() {
        let empty = gtk::Label::builder()
            .label(metis_i18n::tr("No new notifications"))
            .halign(gtk::Align::Start)
            .build();
        empty.add_css_class("metis-notif-empty");
        list.append(&empty);
        return;
    }
    for entry in entries {
        let card = build_notification_card(entry);
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .transition_duration(SLIDE_MS)
            .reveal_child(true)
            .child(&card)
            .build();
        list.append(&revealer);
    }
}

fn build_notification_card(entry: &NotificationEntry) -> gtk::Box {
    let notif = &entry.notification;
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .hexpand(true)
        .build();
    card.add_css_class("metis-notif-card");
    card.add_css_class(&format!("metis-notif-card-{}", notif.kind.css_suffix()));

    let icon = gtk::Image::from_icon_name(notif.kind.icon_name());
    icon.add_css_class("metis-notif-icon");
    icon.set_valign(gtk::Align::Start);
    card.append(&icon);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();

    let title = gtk::Label::builder()
        .label(&notif.title)
        .halign(gtk::Align::Fill)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(28)
        .build();
    title.add_css_class("metis-notif-title");

    let message = gtk::Label::builder()
        .label(&notif.message)
        .halign(gtk::Align::Fill)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(28)
        .build();
    message.add_css_class("metis-notif-message");

    text.append(&title);
    text.append(&message);

    let uid = entry.uid;
    let dismiss = move || {
        glib::idle_add_local_once(move || crate::services::dismiss_notification(uid));
    };

    if let Some(row) = build_action_row(notif, dismiss) {
        text.append(&row);
    }

    card.append(&text);

    if notif.has_default_action() {
        card.add_css_class("metis-notif-card-clickable");
        let gesture = gtk::GestureClick::new();
        let id = notif.id;
        gesture.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            crate::services::invoke_action(id, "default");
            crate::services::close_notification(id, 2);
            dismiss();
        });
        card.add_controller(gesture);
    }

    if entry.count > 1 {
        let count = gtk::Label::new(Some(&format!("{}", entry.count)));
        count.add_css_class("metis-notif-count");
        count.set_valign(gtk::Align::Start);
        card.append(&count);
    }

    card
}

pub fn seed_demo_notifications() {
    use crate::services::push_notification;
    let demos = [
        (
            NotificationKind::Success,
            "Workspace saved",
            "Layout stored to disk.",
        ),
        (
            NotificationKind::Payment,
            "Payment received",
            "Invoice #1042 was paid.",
        ),
        (
            NotificationKind::Error,
            "Sync failed",
            "Could not reach the calendar server.",
        ),
        (
            NotificationKind::Notification,
            "New message",
            "Ping from Metis Core.",
        ),
        (
            NotificationKind::Notification,
            "New message",
            "Ping from Metis Core.",
        ),
    ];
    for (kind, title, message) in demos {
        push_notification(BarNotification::internal(kind, title, message));
    }
}
