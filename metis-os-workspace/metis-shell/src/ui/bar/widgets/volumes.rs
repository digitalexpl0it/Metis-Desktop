//! Edge-bar removable volumes: USB / SD / optical / ISO icons next to the tray.
//!
//! Left-click opens (or mounts then opens). Right-click shows Open / Mount /
//! Unmount / Eject. The context popover follows the bar's transient-popover
//! pattern so opening it never resizes the icon or the edge bar.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::services::{self, VolumeEntry, VolumeKind};

thread_local! {
    /// Only one volumes context menu at a time (buttons rebuild on mount changes).
    static VOLUME_MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

pub struct VolumesWidget {
    root: gtk::Box,
}

impl VolumesWidget {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("metis-bar-volumes");
        root.set_visible(false);
        // Never let volume icons expand the pill when a popover is open.
        root.set_overflow(gtk::Overflow::Hidden);
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Center);

        let widget = Rc::new(RefCell::new(VolumesWidgetInner { root: root.clone() }));

        let refresh = {
            let widget = widget.clone();
            Rc::new(move || {
                dismiss_volume_menu();
                widget.borrow().rebuild();
            }) as Rc<dyn Fn()>
        };
        services::register_volumes_refresh(refresh.clone());
        refresh();

        Self { root }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }
}

struct VolumesWidgetInner {
    root: gtk::Box,
}

impl VolumesWidgetInner {
    fn rebuild(&self) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        let entries = services::volumes_snapshot();
        self.root.set_visible(!entries.is_empty());
        for entry in entries {
            self.root.append(&build_volume_button(&entry));
        }
    }
}

fn build_volume_button(entry: &VolumeEntry) -> gtk::Button {
    let btn = gtk::Button::builder().has_frame(false).build();
    btn.add_css_class("metis-bar-widget");
    btn.add_css_class("metis-bar-sys-icon");
    btn.add_css_class("metis-bar-volume-item");
    btn.set_tooltip_text(Some(&entry.tooltip));
    // Keep the button's allocation fixed across hover / :checked (popover open).
    btn.set_valign(gtk::Align::Center);
    btn.set_halign(gtk::Align::Center);
    btn.set_overflow(gtk::Overflow::Hidden);

    let icon = gtk::Image::from_icon_name(services::volumes_icon_name(entry.kind));
    icon.set_pixel_size(18);
    if entry.kind == VolumeKind::Locked
        && let Some(display) = gdk::Display::default()
    {
        let theme = gtk::IconTheme::for_display(&display);
        if !theme.has_icon("drive-harddisk-encrypted-symbolic") {
            icon.set_icon_name(Some("changes-prevent-symbolic"));
        }
    }
    btn.set_child(Some(&icon));

    let id = entry.id.clone();
    btn.connect_clicked(move |_| {
        dismiss_volume_menu();
        services::volumes_activate(&id);
    });

    let entry = entry.clone();
    let btn_for_menu = btn.clone();
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gdk::BUTTON_SECONDARY);
    gesture.connect_released(move |g, _, _x, _y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        show_context_menu(&btn_for_menu, &entry);
    });
    btn.add_controller(gesture);

    btn
}

fn show_context_menu(anchor: &gtk::Button, entry: &VolumeEntry) {
    dismiss_volume_menu();
    super::super::dropdown::close_all();

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
    box_.add_css_class("metis-bar-dropdown-panel");
    box_.add_css_class("metis-bar-volumes-menu-panel");
    box_.set_margin_top(6);
    box_.set_margin_bottom(6);
    box_.set_margin_start(6);
    box_.set_margin_end(6);

    let title = gtk::Label::new(Some(&entry.label));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(28);
    title.set_width_chars(1);
    title.set_hexpand(false);
    title.add_css_class("metis-bar-section-title");
    title.add_css_class("metis-bar-volumes-menu-title");
    title.set_margin_bottom(4);
    box_.append(&title);

    if entry.mount_path.is_some() {
        let open = menu_btn(&metis_i18n::tr("Open"));
        let id = entry.id.clone();
        open.connect_clicked(move |_| {
            dismiss_volume_menu();
            if let Some(path) = services::volumes_snapshot()
                .into_iter()
                .find(|e| e.id == id)
                .and_then(|e| e.mount_path)
            {
                services::open_in_file_manager(&path);
            }
        });
        box_.append(&open);
    }

    if entry.needs_mount {
        let label = if entry.is_encrypted_locked {
            metis_i18n::tr("Unlock…")
        } else {
            metis_i18n::tr("Mount")
        };
        let mount = menu_btn(&label);
        let id = entry.id.clone();
        mount.connect_clicked(move |_| {
            dismiss_volume_menu();
            services::volumes_mount(&id);
        });
        box_.append(&mount);
    }

    if entry.can_unmount {
        let unmount = menu_btn(&metis_i18n::tr("Unmount"));
        let id = entry.id.clone();
        unmount.connect_clicked(move |_| {
            dismiss_volume_menu();
            services::volumes_unmount(&id);
        });
        box_.append(&unmount);
    }

    if entry.can_eject {
        let eject = menu_btn(&metis_i18n::tr("Eject"));
        let id = entry.id.clone();
        eject.connect_clicked(move |_| {
            dismiss_volume_menu();
            services::volumes_eject(&id);
        });
        box_.append(&eject);
    }

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(&box_)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.add_css_class("metis-bar-volumes-menu");
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

    VOLUME_MENU.with(|cell| *cell.borrow_mut() = Some(popover.clone()));

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        let weak = weak.clone();
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade()
                && p.parent().is_some()
            {
                p.unparent();
            }
            VOLUME_MENU.with(|cell| {
                if cell
                    .borrow()
                    .as_ref()
                    .is_some_and(|cur| weak.upgrade().as_ref() == Some(cur))
                {
                    *cell.borrow_mut() = None;
                }
            });
        });
    });

    glib::idle_add_local_once(move || popover.popup());
}

fn dismiss_volume_menu() {
    VOLUME_MENU.with(|cell| {
        if let Some(p) = cell.borrow_mut().take() {
            p.popdown();
            if p.parent().is_some() {
                p.unparent();
            }
        }
    });
}

fn menu_btn(label: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(label);
    btn.add_css_class("flat");
    btn.add_css_class("metis-bar-volumes-menu-item");
    btn.set_halign(gtk::Align::Fill);
    btn
}
