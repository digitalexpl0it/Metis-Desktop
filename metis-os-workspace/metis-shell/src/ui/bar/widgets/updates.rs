//! Ephemeral edge-bar software-updates indicator.
//!
//! Hidden when there are no pending updates or updates are snoozed. Primary
//! click opens the updater; right-click offers snooze / open.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::services;
use crate::ui::icons;

thread_local! {
    static MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

pub struct UpdatesWidget {
    root: gtk::Button,
    _refresh: Rc<dyn Fn()>,
}

impl UpdatesWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().has_frame(false).build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-sys-icon");
        root.add_css_class("metis-bar-updates");
        root.set_tooltip_text(Some(&metis_i18n::tr("Software updates")));
        root.set_visible(false);

        let icon = icons::image("software-update-available-symbolic");
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
            dismiss_menu();
            crate::ui::updater::show();
        });

        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_SECONDARY);
        let btn_for_menu = root.clone();
        gesture.connect_released(move |g, _, _x, _y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            show_context_menu(&btn_for_menu);
        });
        root.add_controller(gesture);

        let refresh: Rc<dyn Fn()> = {
            let root = root.clone();
            let badge = badge.clone();
            Rc::new(move || {
                let visible = services::updates_is_visible();
                root.set_visible(visible);
                if !visible {
                    badge.set_label("");
                    badge.set_visible(false);
                    return;
                }
                let count = services::updates_pending_count();
                let label = if count > 99 {
                    "99+".to_string()
                } else {
                    count.to_string()
                };
                badge.set_label(&label);
                badge.set_visible(count > 0);
                root.set_tooltip_text(Some(
                    &metis_i18n::tr("%1 software update(s) available")
                        .replace("%1", &count.to_string()),
                ));
            })
        };
        services::register_updates_refresh(refresh.clone());
        refresh();

        Self {
            root,
            _refresh: refresh,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }
}

fn dismiss_menu() {
    MENU.with(|m| {
        if let Some(p) = m.borrow_mut().take() {
            p.popdown();
            p.unparent();
        }
    });
}

fn show_context_menu(anchor: &gtk::Button) {
    dismiss_menu();
    super::super::dropdown::close_all();

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
    box_.add_css_class("metis-bar-volumes-menu-panel");
    box_.set_margin_top(6);
    box_.set_margin_bottom(6);
    box_.set_margin_start(6);
    box_.set_margin_end(6);

    let title = gtk::Label::new(Some(&metis_i18n::tr("Software updates")));
    title.set_xalign(0.0);
    title.add_css_class("metis-bar-section-title");
    title.set_margin_bottom(4);
    box_.append(&title);

    let open = menu_btn(&metis_i18n::tr("Open updater"));
    open.connect_clicked(|_| {
        dismiss_menu();
        crate::ui::updater::show();
    });
    box_.append(&open);

    let s1 = menu_btn(&metis_i18n::tr("Snooze 1 hour"));
    s1.connect_clicked(|_| {
        dismiss_menu();
        services::updates_snooze_hours(1);
    });
    box_.append(&s1);

    let tonight = menu_btn(&metis_i18n::tr("Snooze until tonight"));
    tonight.connect_clicked(|_| {
        dismiss_menu();
        services::updates_snooze_tonight();
    });
    box_.append(&tonight);

    let day = menu_btn(&metis_i18n::tr("Snooze 1 day"));
    day.connect_clicked(|_| {
        dismiss_menu();
        services::updates_snooze_one_day();
    });
    box_.append(&day);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(&box_)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(anchor);
    super::super::dropdown::register(&popover);

    {
        let btn = anchor.clone();
        popover.connect_map(move |_| {
            btn.add_css_class("metis-bar-dropdown-active");
        });
    }
    {
        let btn = anchor.clone();
        popover.connect_unmap(move |_| {
            btn.remove_css_class("metis-bar-dropdown-active");
        });
    }

    MENU.with(|m| *m.borrow_mut() = Some(popover.clone()));
    popover.popup();
}

fn menu_btn(label: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(label);
    btn.add_css_class("flat");
    btn.add_css_class("metis-bar-menu-item");
    btn.set_halign(gtk::Align::Fill);
    btn
}
