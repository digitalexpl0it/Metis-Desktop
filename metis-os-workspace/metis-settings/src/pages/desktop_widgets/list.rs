//! Compact desktop-widget instance list helpers.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetInstance, DesktopWidgetKind, DesktopWidgetsConfig, discover_widget_extensions,
    load_menu_config,
};

use crate::gtk_cb::OptFn0Cell;
use metis_i18n::tr;

use super::configure::open_configure_dialog;
use super::mutate_from_disk;

pub(crate) fn kind_icon(kind: DesktopWidgetKind) -> &'static str {
    match kind {
        DesktopWidgetKind::Folders => "folder-symbolic",
        DesktopWidgetKind::Apps => "view-app-grid-symbolic",
        DesktopWidgetKind::Clock => "preferences-system-time-symbolic",
        DesktopWidgetKind::System => "utilities-system-monitor-symbolic",
        DesktopWidgetKind::Weather => "weather-few-clouds-symbolic",
        DesktopWidgetKind::Equalizer => "multimedia-equalizer-symbolic",
        DesktopWidgetKind::Placeholder => "view-grid-symbolic",
        DesktopWidgetKind::Extension => "application-x-addon-symbolic",
    }
}

#[derive(Clone)]
pub(crate) enum AddChoice {
    Builtin(DesktopWidgetKind),
    Extension(String),
}

pub(crate) fn build_add_choices() -> (Vec<AddChoice>, Vec<String>) {
    let mut choices = Vec::new();
    let mut labels = Vec::new();
    for kind in DesktopWidgetKind::addable() {
        choices.push(AddChoice::Builtin(*kind));
        labels.push(kind.label().to_string());
    }
    for ext in discover_widget_extensions() {
        choices.push(AddChoice::Extension(ext.manifest.id.clone()));
        labels.push(format!("{} (extension)", ext.manifest.name));
    }
    (choices, labels)
}

pub(crate) fn instance_subtitle(inst: &DesktopWidgetInstance) -> String {
    let geo = format!("{}×{} @ ({}, {})", inst.w, inst.h, inst.x, inst.y);
    let detail = match inst.kind {
        DesktopWidgetKind::Folders => {
            let path = if inst.path.trim().is_empty() {
                "~/Desktop"
            } else {
                inst.path.as_str()
            };
            path.to_string()
        }
        DesktopWidgetKind::Apps => {
            if inst.pins.is_empty() {
                let n = load_menu_config().pinned.len();
                format!("Following start menu ({n})")
            } else {
                format!("{} dedicated pin(s)", inst.pins.len())
            }
        }
        DesktopWidgetKind::Equalizer => inst.viz_style.label().to_string(),
        DesktopWidgetKind::Clock | DesktopWidgetKind::Weather => {
            if inst.font.trim().is_empty() {
                "Theme font".into()
            } else {
                inst.font.clone()
            }
        }
        DesktopWidgetKind::Extension => {
            if inst.extension_id.is_empty() {
                "Extension".into()
            } else {
                inst.extension_id.clone()
            }
        }
        _ => String::new(),
    };
    if detail.is_empty() {
        if inst.output.is_empty() {
            geo
        } else {
            format!("{geo} · {}", inst.output)
        }
    } else if inst.output.is_empty() {
        format!("{detail} · {geo}")
    } else {
        format!("{detail} · {geo} · {}", inst.output)
    }
}

pub(crate) fn instance_row(
    inst: &DesktopWidgetInstance,
    index: usize,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    refresh_list: OptFn0Cell,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-widget-list-row");
    if index % 2 == 1 {
        row.add_css_class("metis-widget-list-row-alt");
    }
    row.set_hexpand(true);

    let icon = gtk::Image::from_icon_name(kind_icon(inst.kind));
    icon.set_pixel_size(22);
    icon.add_css_class("metis-widget-list-icon");
    row.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&inst.display_title()));
    title.set_xalign(0.0);
    title.add_css_class("metis-widget-list-title");
    text.append(&title);

    let subtitle = gtk::Label::new(Some(&instance_subtitle(inst)));
    subtitle.set_xalign(0.0);
    subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    subtitle.add_css_class("metis-widget-list-subtitle");
    text.append(&subtitle);
    row.append(&text);

    let locked = gtk::CheckButton::with_label(&tr("Locked"));
    locked.set_active(inst.locked);
    locked.set_valign(gtk::Align::Center);
    let id = inst.id.clone();
    {
        let cfg = cfg.clone();
        locked.connect_toggled(move |btn| {
            let locked = btn.is_active();
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.locked = locked;
                }
            });
        });
    }
    row.append(&locked);

    let configure = gtk::Button::from_icon_name("preferences-system-symbolic");
    configure.set_tooltip_text(Some(&tr("Configure")));
    configure.add_css_class("flat");
    configure.add_css_class("metis-widget-configure-btn");
    configure.set_valign(gtk::Align::Center);
    let id = inst.id.clone();
    {
        let cfg = cfg.clone();
        let refresh_list = refresh_list.clone();
        let chrome_debounce = chrome_debounce.clone();
        configure.connect_clicked(move |btn| {
            let parent = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            open_configure_dialog(
                parent.as_ref(),
                &id,
                cfg.clone(),
                refresh_list.clone(),
                chrome_debounce.clone(),
            );
        });
    }
    row.append(&configure);

    let remove = gtk::Button::with_label(&tr("Remove"));
    remove.add_css_class("destructive-action");
    remove.set_valign(gtk::Align::Center);
    let id = inst.id.clone();
    {
        let cfg = cfg.clone();
        let refresh_list = refresh_list.clone();
        remove.connect_clicked(move |_| {
            mutate_from_disk(&cfg, |disk| {
                disk.instances.retain(|i| i.id != id);
            });
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
        });
    }
    row.append(&remove);

    row.upcast()
}
