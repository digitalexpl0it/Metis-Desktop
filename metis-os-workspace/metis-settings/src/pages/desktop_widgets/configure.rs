//! Per-instance Configure modal for desktop widgets.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetInstance, DesktopWidgetKind, DesktopWidgetsConfig, load_menu_config,
};

use crate::gtk_cb::OptFn0Cell;
use crate::ui;
use metis_i18n::tr;

use super::list::kind_icon;
use super::mutate_from_disk;
use super::options::{
    equalizer_options, extension_options, instance_chrome_overrides, text_style_options,
    view_mode_row,
};

pub(crate) fn open_configure_dialog(
    parent: Option<&gtk::Window>,
    id: &str,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    refresh_list: OptFn0Cell,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
) {
    let Some(inst) = cfg.borrow().instances.iter().find(|i| i.id == id).cloned() else {
        return;
    };

    let mut builder = gtk::Window::builder()
        .title(format!("{} widget", inst.display_title()))
        .modal(true)
        .decorated(false)
        .resizable(true)
        .default_width(480)
        .default_height(520);
    if let Some(parent) = parent {
        builder = builder.transient_for(parent);
    }
    let dialog = builder.build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-widget-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.set_margin_bottom(12);
    let header_icon = gtk::Image::from_icon_name(kind_icon(inst.kind));
    header_icon.set_pixel_size(24);
    header_icon.add_css_class("metis-widget-list-icon");
    header.append(&header_icon);
    let heading = gtk::Label::new(Some(&format!("{} settings", inst.display_title())));
    heading.set_xalign(0.0);
    heading.set_hexpand(true);
    heading.add_css_class("metis-settings-section-title");
    header.append(&heading);
    let close_btn = gtk::Button::with_label(&tr("Done"));
    close_btn.add_css_class("suggested-action");
    header.append(&close_btn);
    outer.append(&header);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .min_content_height(360)
        .overlay_scrolling(false)
        .build();
    scroll.set_kinetic_scrolling(false);
    ui::wire_vertical_scroll(&scroll);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_end(4);
    scroll.set_child(Some(&body));
    outer.append(&scroll);

    let rebuild_body: OptFn0Cell = Rc::new(RefCell::new(None));
    {
        let cfg = cfg.clone();
        let id = id.to_string();
        let body = body.clone();
        let chrome_debounce = chrome_debounce.clone();
        let rebuild_slot = rebuild_body.clone();
        let rebuild = Rc::new(move || {
            while let Some(child) = body.first_child() {
                body.remove(&child);
            }
            let Some(inst) = cfg.borrow().instances.iter().find(|i| i.id == id).cloned() else {
                return;
            };
            fill_configure_body(
                &body,
                &inst,
                cfg.clone(),
                chrome_debounce.clone(),
                rebuild_slot.clone(),
            );
        });
        *rebuild_body.borrow_mut() = Some(rebuild.clone());
        rebuild();
    }

    // Own sheet host so Pick a Color / Font land on this modal (not behind it
    // on the main Settings overlay).
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&ui::dialog_sheet(&outer)));
    crate::dialog::install(&overlay);
    dialog.set_child(Some(&overlay));

    {
        let dialog = dialog.clone();
        let refresh_list = refresh_list.clone();
        close_btn.connect_clicked(move |_| {
            // Subtitle / pins summary may have changed while editing.
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
            crate::dialog::uninstall();
            dialog.close();
        });
    }
    {
        let refresh_list = refresh_list.clone();
        dialog.connect_close_request(move |_| {
            crate::dialog::uninstall();
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
            glib::Propagation::Proceed
        });
    }

    dialog.present();
}

pub(crate) fn fill_configure_body(
    body: &gtk::Box,
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    rebuild_body: OptFn0Cell,
) {
    let geo = gtk::Label::new(Some(&format!(
        "Size {}×{} at ({}, {}){}",
        inst.w,
        inst.h,
        inst.x,
        inst.y,
        if inst.output.is_empty() {
            String::new()
        } else {
            format!(" on {}", inst.output)
        }
    )));
    geo.set_xalign(0.0);
    geo.set_wrap(true);
    geo.add_css_class("metis-settings-hint");
    geo.set_margin_bottom(4);
    body.append(&geo);

    {
        let show_title = gtk::CheckButton::with_label(&tr("Show title"));
        show_title.set_active(inst.show_title);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            show_title.connect_toggled(move |btn| {
                let on = btn.is_active();
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.show_title = on;
                    }
                });
            });
        }
        body.append(&show_title);
    }

    match inst.kind {
        DesktopWidgetKind::Folders => {
            let path_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let path_entry = gtk::Entry::new();
            path_entry.set_text(&inst.path);
            path_entry.set_placeholder_text(Some(&tr("~/Desktop")));
            path_entry.set_hexpand(true);
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                path_entry.connect_activate(move |entry| {
                    let path = entry.text().to_string();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.path = if path.trim().is_empty() {
                                "~/Desktop".into()
                            } else {
                                path
                            };
                        }
                    });
                });
            }
            let apply = gtk::Button::with_label(&tr("Set path"));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                let path_entry = path_entry.clone();
                apply.connect_clicked(move |_| {
                    let path = path_entry.text().to_string();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.path = if path.trim().is_empty() {
                                "~/Desktop".into()
                            } else {
                                path
                            };
                        }
                    });
                });
            }
            path_row.append(&path_entry);
            path_row.append(&apply);
            body.append(&path_row);
            body.append(&view_mode_row(inst, cfg.clone()));
        }
        DesktopWidgetKind::Apps => {
            let menu_count = load_menu_config().pinned.len();
            let pins_hint = if inst.pins.is_empty() {
                format!(
                    "Following start-menu pins live ({menu_count}). \
                     Import below to freeze a dedicated copy on this widget."
                )
            } else {
                format!(
                    "{} dedicated pin(s) on this widget (not live-synced).",
                    inst.pins.len()
                )
            };
            let pins_hint = gtk::Label::new(Some(&pins_hint));
            pins_hint.set_xalign(0.0);
            pins_hint.set_wrap(true);
            pins_hint.add_css_class("metis-settings-hint");
            body.append(&pins_hint);
            body.append(&view_mode_row(inst, cfg.clone()));

            let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let import = gtk::Button::with_label(&tr("Import start-menu pins"));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                let rebuild_body = rebuild_body.clone();
                import.connect_clicked(move |_| {
                    let menu_pins = load_menu_config().pinned;
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            for pin in &menu_pins {
                                if !inst.pins.iter().any(|p| p.eq_ignore_ascii_case(pin)) {
                                    inst.pins.push(pin.clone());
                                }
                            }
                        }
                    });
                    if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                        rebuild();
                    }
                });
            }
            btn_row.append(&import);

            if !inst.pins.is_empty() {
                let clear = gtk::Button::with_label(&tr("Follow start menu again"));
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    let rebuild_body = rebuild_body.clone();
                    clear.connect_clicked(move |_| {
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.pins.clear();
                            }
                        });
                        if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                            rebuild();
                        }
                    });
                }
                btn_row.append(&clear);
            }
            body.append(&btn_row);
        }
        DesktopWidgetKind::Clock | DesktopWidgetKind::Weather | DesktopWidgetKind::System => {
            body.append(&text_style_options(inst, cfg.clone(), rebuild_body.clone()));
        }
        DesktopWidgetKind::Equalizer => {
            body.append(&equalizer_options(
                inst,
                cfg.clone(),
                chrome_debounce.clone(),
                rebuild_body.clone(),
            ));
        }
        DesktopWidgetKind::Extension => {
            body.append(&extension_options(inst, cfg.clone()));
        }
        _ => {}
    }

    body.append(&instance_chrome_overrides(
        inst,
        cfg,
        chrome_debounce,
        rebuild_body,
    ));
}
