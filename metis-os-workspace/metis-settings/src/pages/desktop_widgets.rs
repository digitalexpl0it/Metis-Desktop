//! Desktop widgets: enable the wallpaper widget layer, edit mode, chrome
//! (fill + border), and manage instances. Persists to `desktop-widgets.json`;
//! the shell live-reloads.
//!
//! Geometry is owned by the shell while the user drags/resizes. Every Settings
//! write reloads from disk first so toggles cannot clobber positions the shell
//! already saved.
//!
//! Chrome: global defaults under `chrome`, optional per-instance overrides
//! (`None` = inherit).
//!
//! Instance list is compact (icon + summary + Locked / Configure / Remove);
//! per-widget options open in a modal dialog so Add stays near the top.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetChromeOverride, DesktopWidgetInstance, DesktopWidgetKind, DesktopWidgetView,
    DesktopWidgetsConfig, EqualizerBarShape, EqualizerColorMode, EqualizerVizStyle,
    WidgetExtSettingType, discover_widget_extensions, find_widget_extension,
    load_desktop_widgets_config, load_menu_config, save_desktop_widgets_config,
};

use crate::gtk_cb::OptFn0Cell;
use crate::pages::appearance_common::{
    color_dialog_button, font_picker_button, hex_to_rgba, rgba_to_hex,
};
use crate::ui;
use metis_i18n::tr;

/// Coalesce slider writes so dragging opacity doesn't storm the shell with
/// full config reloads / atomic renames.
const CHROME_SAVE_DEBOUNCE_MS: u64 = 180;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("desktop_widgets");
    let cfg = Rc::new(RefCell::new(load_desktop_widgets_config()));
    let chrome_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let (panel_card, panel_body) =
        ui::section_with_icon(&tr("Desktop widgets"), "view-grid-symbolic");

    let enabled = gtk::Switch::new();
    enabled.set_active(cfg.borrow().enabled);
    enabled.set_halign(gtk::Align::End);
    panel_body.append(&ui::row_with_icon(
        "preferences-desktop-wallpaper-symbolic",
        &tr("Show desktop widgets"),
        &enabled,
    ));

    let edit_mode = gtk::Switch::new();
    edit_mode.set_active(cfg.borrow().edit_mode);
    edit_mode.set_halign(gtk::Align::End);
    panel_body.append(&ui::row_with_icon(
        "document-edit-symbolic",
        &tr("Edit mode (move / resize)"),
        &edit_mode,
    ));

    let hint = gtk::Label::new(Some(&tr(
        "Widgets float over the wallpaper (not classic desktop icons). Off by \
         default. In edit mode, drag the title bar to move and the corner handle \
         to resize. Chrome below is the default look; each widget can override it \
         from its Configure dialog.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    panel_body.append(&hint);
    content.append(&panel_card);

    // ---- Global chrome defaults ----
    let (chrome_card, chrome_body) =
        ui::section_with_icon(&tr("Default look"), "preferences-color-symbolic");
    {
        let chrome = cfg.borrow().chrome.clone();

        let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        opacity.set_value(chrome.background_opacity as f64);
        opacity.set_size_request(200, -1);
        opacity.set_draw_value(true);
        opacity.set_digits(2);
        ui::forward_wheel_to_page_scroller(&opacity);
        chrome_body.append(&ui::row_with_icon(
            "preferences-color-symbolic",
            &tr("Background opacity"),
            &opacity,
        ));

        let bg_theme = gtk::CheckButton::with_label(&tr("Theme colour"));
        bg_theme.set_active(chrome.background_color.is_empty());
        let bg_color = color_dialog_button();
        if !chrome.background_color.is_empty() {
            bg_color.set_rgba(&hex_to_rgba(&chrome.background_color));
        }
        bg_color.set_sensitive(!chrome.background_color.is_empty());
        let bg_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bg_row.append(&bg_theme);
        bg_row.append(bg_color.upcast_ref());
        chrome_body.append(&ui::row_with_icon(
            "color-select-symbolic",
            &tr("Background colour"),
            &bg_row,
        ));

        let border_w = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 12.0, 0.5);
        border_w.set_value(chrome.border_width as f64);
        border_w.set_size_request(200, -1);
        border_w.set_draw_value(true);
        border_w.set_digits(1);
        ui::forward_wheel_to_page_scroller(&border_w);
        chrome_body.append(&ui::row_with_icon(
            "object-select-symbolic",
            &tr("Border width (0 = none)"),
            &border_w,
        ));

        let border_theme = gtk::CheckButton::with_label(&tr("Theme colour"));
        border_theme.set_active(chrome.border_color.is_empty());
        let border_color = color_dialog_button();
        if !chrome.border_color.is_empty() {
            border_color.set_rgba(&hex_to_rgba(&chrome.border_color));
        }
        border_color.set_sensitive(!chrome.border_color.is_empty());
        let border_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        border_row.append(&border_theme);
        border_row.append(border_color.upcast_ref());
        chrome_body.append(&ui::row_with_icon(
            "color-select-symbolic",
            &tr("Border colour"),
            &border_row,
        ));

        let chrome_hint = gtk::Label::new(Some(&tr(
            "Opacity 0 clears the fill; set border width to 0 to hide the edge. \
             Theme colour follows the active Appearance surface / text tint.",
        )));
        chrome_hint.set_xalign(0.0);
        chrome_hint.set_wrap(true);
        chrome_hint.add_css_class("metis-settings-hint");
        chrome_body.append(&chrome_hint);

        {
            let cfg = cfg.clone();
            let chrome_debounce = chrome_debounce.clone();
            opacity.connect_value_changed(move |s| {
                let v = s.value() as f32;
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    disk.chrome.background_opacity = v.clamp(0.0, 1.0);
                });
            });
        }
        {
            let cfg = cfg.clone();
            let bg_color = bg_color.clone();
            bg_theme.connect_toggled(move |btn| {
                let use_theme = btn.is_active();
                bg_color.set_sensitive(!use_theme);
                mutate_from_disk(&cfg, |disk| {
                    disk.chrome.background_color = if use_theme {
                        String::new()
                    } else {
                        rgba_to_hex(&bg_color.rgba())
                    };
                });
            });
        }
        {
            let cfg = cfg.clone();
            let bg_theme = bg_theme.clone();
            bg_color.connect_rgba_notify(move |btn| {
                if bg_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    disk.chrome.background_color = hex;
                });
            });
        }
        {
            let cfg = cfg.clone();
            let chrome_debounce = chrome_debounce.clone();
            border_w.connect_value_changed(move |s| {
                let v = s.value() as f32;
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    disk.chrome.border_width = v.clamp(0.0, 12.0);
                });
            });
        }
        {
            let cfg = cfg.clone();
            let border_color = border_color.clone();
            border_theme.connect_toggled(move |btn| {
                let use_theme = btn.is_active();
                border_color.set_sensitive(!use_theme);
                mutate_from_disk(&cfg, |disk| {
                    disk.chrome.border_color = if use_theme {
                        String::new()
                    } else {
                        rgba_to_hex(&border_color.rgba())
                    };
                });
            });
        }
        {
            let cfg = cfg.clone();
            let border_theme = border_theme.clone();
            border_color.connect_rgba_notify(move |btn| {
                if border_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    disk.chrome.border_color = hex;
                });
            });
        }
    }
    content.append(&chrome_card);

    // ---- Compact instance list ----
    let (list_card, list_body) =
        ui::section_with_icon(&tr("Widgets on this desktop"), "view-list-symbolic");

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_row.set_halign(gtk::Align::Fill);
    add_row.set_hexpand(true);
    add_row.add_css_class("metis-widget-add-row");
    let (add_choices, add_labels) = build_add_choices();
    let add_choices = Rc::new(add_choices);
    let label_refs: Vec<&str> = add_labels.iter().map(|s| s.as_str()).collect();
    let kind_dd = gtk::DropDown::from_strings(&label_refs);
    // Default to Folders (skip Placeholder at index 0).
    let folders_idx = add_choices
        .iter()
        .position(|c| matches!(c, AddChoice::Builtin(DesktopWidgetKind::Folders)))
        .unwrap_or(0) as u32;
    kind_dd.set_selected(folders_idx);
    kind_dd.set_hexpand(true);
    let add_btn = gtk::Button::with_label(&tr("Add widget"));
    add_btn.add_css_class("suggested-action");
    add_row.append(&kind_dd);
    add_row.append(&add_btn);
    list_body.append(&add_row);

    let empty = gtk::Label::new(Some(&tr(
        "No widgets yet. Pick a type above and click Add widget. JSON extensions \
         install under ~/.local/share/metis/widgets/.",
    )));
    empty.set_xalign(0.0);
    empty.add_css_class("metis-settings-hint");
    list_body.append(&empty);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("metis-widget-list");
    list_body.append(&list);

    content.append(&list_card);

    let refresh_list: OptFn0Cell = Rc::new(RefCell::new(None));
    {
        let cfg = cfg.clone();
        let list = list.clone();
        let empty = empty.clone();
        let refresh_slot = refresh_list.clone();
        let chrome_debounce = chrome_debounce.clone();
        let scroller = scroller.clone();
        let refresh = Rc::new(move || {
            let scroll_y = scroller.vadjustment().value();
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let instances = cfg.borrow().instances.clone();
            empty.set_visible(instances.is_empty());
            list.set_visible(!instances.is_empty());
            for (idx, inst) in instances.iter().enumerate() {
                let row = instance_row(
                    inst,
                    idx,
                    cfg.clone(),
                    refresh_slot.clone(),
                    chrome_debounce.clone(),
                );
                list.append(&row);
            }
            let scroller = scroller.clone();
            glib::idle_add_local_once(move || {
                let vadj = scroller.vadjustment();
                let max = (vadj.upper() - vadj.page_size()).max(vadj.lower());
                vadj.set_value(scroll_y.clamp(vadj.lower(), max));
            });
        });
        *refresh_list.borrow_mut() = Some(refresh.clone());
        refresh();
    }

    {
        let cfg = cfg.clone();
        enabled.connect_active_notify(move |sw| {
            mutate_from_disk(&cfg, |disk| {
                disk.enabled = sw.is_active();
            });
        });
    }
    {
        let cfg = cfg.clone();
        edit_mode.connect_active_notify(move |sw| {
            mutate_from_disk(&cfg, |disk| {
                disk.edit_mode = sw.is_active();
            });
        });
    }
    {
        let cfg = cfg.clone();
        let refresh_list = refresh_list.clone();
        let chrome_debounce = chrome_debounce.clone();
        let add_btn_ref = add_btn.clone();
        let add_choices = add_choices.clone();
        add_btn.connect_clicked(move |_| {
            let idx = kind_dd.selected() as usize;
            let choice = add_choices
                .get(idx)
                .cloned()
                .unwrap_or(AddChoice::Builtin(DesktopWidgetKind::Folders));
            let mut new_id = None;
            mutate_from_disk(&cfg, |disk| {
                let inst = match &choice {
                    AddChoice::Builtin(kind) => DesktopWidgetInstance::new(*kind),
                    AddChoice::Extension(ext_id) => {
                        if let Some(ext) = find_widget_extension(ext_id) {
                            DesktopWidgetInstance::new_extension(&ext.manifest)
                        } else {
                            let mut i = DesktopWidgetInstance::new(DesktopWidgetKind::Extension);
                            i.extension_id = ext_id.clone();
                            i
                        }
                    }
                };
                new_id = Some(inst.id.clone());
                disk.instances.push(inst);
            });
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
            if let Some(id) = new_id {
                let parent = add_btn_ref
                    .root()
                    .and_then(|r| r.downcast::<gtk::Window>().ok());
                open_configure_dialog(
                    parent.as_ref(),
                    &id,
                    cfg.clone(),
                    refresh_list.clone(),
                    chrome_debounce.clone(),
                );
            }
        });
    }

    scroller.upcast()
}

fn kind_icon(kind: DesktopWidgetKind) -> &'static str {
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
enum AddChoice {
    Builtin(DesktopWidgetKind),
    Extension(String),
}

fn build_add_choices() -> (Vec<AddChoice>, Vec<String>) {
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

fn instance_subtitle(inst: &DesktopWidgetInstance) -> String {
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

fn instance_row(
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

fn open_configure_dialog(
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

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    {
        let dialog = dialog.clone();
        let refresh_list = refresh_list.clone();
        close_btn.connect_clicked(move |_| {
            // Subtitle / pins summary may have changed while editing.
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
            dialog.close();
        });
    }
    {
        let refresh_list = refresh_list.clone();
        dialog.connect_close_request(move |_| {
            if let Some(refresh) = refresh_list.borrow().as_ref() {
                refresh();
            }
            glib::Propagation::Proceed
        });
    }

    dialog.present();
}

fn fill_configure_body(
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

fn extension_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let id_label = gtk::Label::new(Some(&format!("id: {}", inst.extension_id)));
    id_label.set_xalign(0.0);
    id_label.set_selectable(true);
    id_label.add_css_class("metis-settings-hint");
    col.append(&id_label);

    let path_hint = if let Some(ext) = find_widget_extension(&inst.extension_id) {
        format!("Pack: {}", ext.root.display())
    } else {
        format!(
            "Pack missing — place under ~/.local/share/metis/widgets/{}/",
            if inst.extension_id.is_empty() {
                "<id>"
            } else {
                &inst.extension_id
            }
        )
    };
    let path_l = gtk::Label::new(Some(&path_hint));
    path_l.set_xalign(0.0);
    path_l.set_wrap(true);
    path_l.set_selectable(true);
    path_l.add_css_class("metis-settings-hint");
    col.append(&path_l);

    let Some(ext) = find_widget_extension(&inst.extension_id) else {
        return col.upcast();
    };
    if ext.manifest.settings_schema.is_empty() {
        let none = gtk::Label::new(Some(&tr("This extension has no settings.")));
        none.set_xalign(0.0);
        none.add_css_class("metis-settings-hint");
        col.append(&none);
        return col.upcast();
    }

    for setting in &ext.manifest.settings_schema {
        let key = setting.key.clone();
        let label = if setting.label.is_empty() {
            setting.key.clone()
        } else {
            setting.label.clone()
        };
        let current = inst
            .extension_settings
            .get(&key)
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        match setting.setting_type {
            WidgetExtSettingType::Bool => {
                let sw = gtk::Switch::new();
                sw.set_active(current.as_bool().unwrap_or(false));
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                sw.connect_state_set(move |_, on| {
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings
                                .insert(key.clone(), serde_json::Value::Bool(on));
                        }
                    });
                    glib::Propagation::Proceed
                });
                col.append(&ui::row(&label, &sw));
            }
            WidgetExtSettingType::Number => {
                let entry = gtk::Entry::new();
                let text = match &current {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => "0".into(),
                };
                entry.set_text(&text);
                entry.set_input_purpose(gtk::InputPurpose::Number);
                entry.set_hexpand(true);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                entry.connect_changed(move |e| {
                    let raw = e.text().to_string();
                    let val = raw
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::String(raw));
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings.insert(key.clone(), val);
                        }
                    });
                });
                col.append(&ui::row(&label, &entry));
            }
            WidgetExtSettingType::String => {
                let entry = gtk::Entry::new();
                let text = match &current {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                entry.set_text(&text);
                entry.set_hexpand(true);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                entry.connect_changed(move |e| {
                    let text = e.text().to_string();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings
                                .insert(key.clone(), serde_json::Value::String(text));
                        }
                    });
                });
                col.append(&ui::row(&label, &entry));
            }
        }
    }
    col.upcast()
}

fn equalizer_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    // Style — changing it rebuilds the option rows for that style.
    {
        let labels: Vec<&str> = EqualizerVizStyle::all().iter().map(|s| s.label()).collect();
        let dd = gtk::DropDown::from_strings(&labels);
        let selected = EqualizerVizStyle::all()
            .iter()
            .position(|s| *s == inst.viz_style)
            .unwrap_or(1) as u32;
        dd.set_selected(selected);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let rebuild_body = rebuild_body.clone();
            dd.connect_selected_notify(move |dd| {
                let style = EqualizerVizStyle::all()
                    .get(dd.selected() as usize)
                    .copied()
                    .unwrap_or(EqualizerVizStyle::Bars);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.viz_style = style;
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        col.append(&ui::row_with_icon(
            "multimedia-equalizer-symbolic",
            &tr("Style"),
            &dd,
        ));
    }

    // Colour mode — rebuild so solid / gradient pickers show correctly.
    {
        let labels: Vec<&str> = EqualizerColorMode::all()
            .iter()
            .map(|m| m.label())
            .collect();
        let dd = gtk::DropDown::from_strings(&labels);
        let selected = EqualizerColorMode::all()
            .iter()
            .position(|m| *m == inst.color_mode)
            .unwrap_or(1) as u32;
        dd.set_selected(selected);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let rebuild_body = rebuild_body.clone();
            dd.connect_selected_notify(move |dd| {
                let mode = EqualizerColorMode::all()
                    .get(dd.selected() as usize)
                    .copied()
                    .unwrap_or(EqualizerColorMode::Multi);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.color_mode = mode;
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        col.append(&ui::row_with_icon(
            "preferences-color-symbolic",
            &tr("Colour mode"),
            &dd,
        ));
    }

    match inst.color_mode {
        EqualizerColorMode::Solid => {
            let color = color_dialog_button();
            color.set_rgba(&hex_to_rgba(&inst.solid_color));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                color.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.solid_color = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Solid colour"),
                &color,
            ));
        }
        EqualizerColorMode::Multi => {
            let start = color_dialog_button();
            start.set_rgba(&hex_to_rgba(&inst.gradient_start));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                start.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.gradient_start = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Gradient start"),
                &start,
            ));

            let end = color_dialog_button();
            end.set_rgba(&hex_to_rgba(&inst.gradient_end));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                end.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.gradient_end = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Gradient end"),
                &end,
            ));
        }
        EqualizerColorMode::Theme => {
            let hint = gtk::Label::new(Some(&tr(
                "Uses the active Appearance accent and secondary colours.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
    }

    // Density — useful for all styles.
    {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 16.0, 96.0, 1.0);
        scale.set_value(inst.bar_count as f64);
        scale.set_draw_value(true);
        scale.set_digits(0);
        scale.set_hexpand(true);
        ui::forward_wheel_to_page_scroller(&scale);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                let n = s.value().round() as u32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.bar_count = n.clamp(16, 96);
                    }
                });
            });
        }
        let density_label = match inst.viz_style {
            EqualizerVizStyle::Bars => "Bars / density",
            EqualizerVizStyle::SpectrumLines => "Lines / density",
            EqualizerVizStyle::NeonWave => "Points / density",
            EqualizerVizStyle::Radial => "Rays / density",
        };
        col.append(&ui::row_with_icon(
            "view-continuous-symbolic",
            density_label,
            &scale,
        ));
    }

    // Style-specific controls.
    match inst.viz_style {
        EqualizerVizStyle::Bars => {
            {
                let labels: Vec<&str> =
                    EqualizerBarShape::all().iter().map(|s| s.label()).collect();
                let dd = gtk::DropDown::from_strings(&labels);
                let selected = EqualizerBarShape::all()
                    .iter()
                    .position(|s| *s == inst.bar_shape)
                    .unwrap_or(0) as u32;
                dd.set_selected(selected);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    dd.connect_selected_notify(move |dd| {
                        let shape = EqualizerBarShape::all()
                            .get(dd.selected() as usize)
                            .copied()
                            .unwrap_or(EqualizerBarShape::Segmented);
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.bar_shape = shape;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "view-list-bullet-symbolic",
                    &tr("Bar shape"),
                    &dd,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.bar_gradient);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.bar_gradient = on;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "color-select-symbolic",
                    &tr("Bar height gradient"),
                    &sw,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.show_peaks);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    let rebuild_body = rebuild_body.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.show_peaks = on;
                            }
                        });
                        if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                            rebuild();
                        }
                    });
                }
                col.append(&ui::row_with_icon("go-top-symbolic", &tr("Peak caps"), &sw));
            }
            if inst.show_peaks {
                let color = color_dialog_button();
                color.set_rgba(&hex_to_rgba(&inst.peak_color));
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    color.connect_rgba_notify(move |btn| {
                        let hex = rgba_to_hex(&btn.rgba());
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.peak_color = hex;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "color-select-symbolic",
                    &tr("Peak colour"),
                    &color,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.show_reflection);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.show_reflection = on;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "object-flip-vertical-symbolic",
                    &tr("Bar reflection"),
                    &sw,
                ));
            }
        }
        EqualizerVizStyle::SpectrumLines => {
            let hint = gtk::Label::new(Some(&tr(
                "Spectrum lines use the colour mode above across the frequency range.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
        EqualizerVizStyle::NeonWave => {
            let sw = gtk::Switch::new();
            sw.set_active(inst.show_reflection);
            sw.set_halign(gtk::Align::End);
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                sw.connect_active_notify(move |sw| {
                    let on = sw.is_active();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.show_reflection = on;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "object-flip-vertical-symbolic",
                &tr("Mirror wave"),
                &sw,
            ));
        }
        EqualizerVizStyle::Radial => {
            let hint = gtk::Label::new(Some(&tr(
                "Rays radiate from the centre. Colour mode tints around the ring.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
    }

    let hint = gtk::Label::new(Some(&tr(
        "Listens to the default audio output (PipeWire/Pulse monitor). \
         Play music or a movie — silent sinks show a quiet idle decay.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    col.append(&hint);

    col.upcast()
}

fn text_style_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.append(&font_row(inst, cfg.clone()));

    // Text colour
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let use_theme = gtk::CheckButton::with_label(&tr("Theme colour"));
        use_theme.set_active(inst.text_color.trim().is_empty());
        let color = color_dialog_button();
        if !inst.text_color.trim().is_empty() {
            color.set_rgba(&hex_to_rgba(&inst.text_color));
        }
        color.set_sensitive(!inst.text_color.trim().is_empty());
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let color = color.clone();
            let rebuild_body = rebuild_body.clone();
            use_theme.connect_toggled(move |btn| {
                let theme = btn.is_active();
                color.set_sensitive(!theme);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.text_color = if theme {
                            String::new()
                        } else {
                            rgba_to_hex(&color.rgba())
                        };
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let use_theme = use_theme.clone();
            color.connect_rgba_notify(move |btn| {
                if use_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.text_color = hex;
                    }
                });
            });
        }
        row.append(&use_theme);
        row.append(color.upcast_ref());
        col.append(&ui::row_with_icon(
            "color-select-symbolic",
            &tr("Text colour"),
            &row,
        ));
    }

    // Accent (progress bars on System; optional highlight elsewhere).
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let use_theme = gtk::CheckButton::with_label(&tr("Theme accent"));
        use_theme.set_active(inst.accent_color.trim().is_empty());
        let color = color_dialog_button();
        if !inst.accent_color.trim().is_empty() {
            color.set_rgba(&hex_to_rgba(&inst.accent_color));
        }
        color.set_sensitive(!inst.accent_color.trim().is_empty());
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let color = color.clone();
            let rebuild_body = rebuild_body.clone();
            use_theme.connect_toggled(move |btn| {
                let theme = btn.is_active();
                color.set_sensitive(!theme);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.accent_color = if theme {
                            String::new()
                        } else {
                            rgba_to_hex(&color.rgba())
                        };
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let use_theme = use_theme.clone();
            color.connect_rgba_notify(move |btn| {
                if use_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.accent_color = hex;
                    }
                });
            });
        }
        row.append(&use_theme);
        row.append(color.upcast_ref());
        let accent_label = match inst.kind {
            DesktopWidgetKind::System => "Bar accent",
            _ => "Accent colour",
        };
        col.append(&ui::row_with_icon(
            "preferences-color-symbolic",
            accent_label,
            &row,
        ));
    }

    let hint = gtk::Label::new(Some(&tr(
        "Font picks family, weight, and size. Text colour tints labels and icons; \
         accent colours the System progress fills.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    col.append(&hint);

    col.upcast()
}

fn font_row(inst: &DesktopWidgetInstance, cfg: Rc<RefCell<DesktopWidgetsConfig>>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let use_theme = gtk::CheckButton::with_label(&tr("Theme font"));
    use_theme.set_active(inst.font.trim().is_empty());

    let font_btn = font_picker_button();
    if !inst.font.trim().is_empty() {
        font_btn.set_font_desc(&gtk::pango::FontDescription::from_string(&inst.font));
    }
    font_btn.set_sensitive(!inst.font.trim().is_empty());

    {
        let cfg = cfg.clone();
        let id = inst.id.clone();
        let font_btn = font_btn.clone();
        use_theme.connect_toggled(move |btn| {
            let theme = btn.is_active();
            font_btn.set_sensitive(!theme);
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    if theme {
                        inst.font.clear();
                    } else if inst.font.trim().is_empty() {
                        let desc = font_btn.font_desc().unwrap_or_default();
                        inst.font = desc.to_string();
                    }
                }
            });
        });
    }
    {
        let cfg = cfg.clone();
        let id = inst.id.clone();
        let use_theme = use_theme.clone();
        font_btn.connect_font_desc_notify(move |btn| {
            if use_theme.is_active() {
                return;
            }
            let desc = btn.font_desc().unwrap_or_default();
            let font = desc.to_string();
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.font = font;
                }
            });
        });
    }

    row.append(&use_theme);
    row.append(font_btn.upcast_ref());
    ui::row_with_icon("font-x-generic-symbolic", &tr("Font"), &row).upcast()
}

fn view_mode_row(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
) -> gtk::Widget {
    let labels: Vec<&str> = DesktopWidgetView::all().iter().map(|v| v.label()).collect();
    let dd = gtk::DropDown::from_strings(&labels);
    let selected = DesktopWidgetView::all()
        .iter()
        .position(|v| *v == inst.view)
        .unwrap_or(0) as u32;
    dd.set_selected(selected);

    let id = inst.id.clone();
    {
        let cfg = cfg.clone();
        dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let view = DesktopWidgetView::all()
                .get(idx)
                .copied()
                .unwrap_or(DesktopWidgetView::Grid);
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.view = view;
                }
            });
        });
    }

    ui::row_with_icon("view-grid-symbolic", &tr("View"), &dd).upcast()
}

fn instance_chrome_overrides(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let expander = gtk::Expander::new(Some(&tr("Look overrides (optional)")));
    expander.set_expanded(!inst.chrome.is_empty());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.set_margin_top(6);
    expander.set_child(Some(&body));

    let id = inst.id.clone();

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Background opacity"));
        enable.set_active(inst.chrome.background_opacity.is_some());
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        scale.set_value(inst.chrome.background_opacity.unwrap_or(0.4) as f64);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_digits(2);
        scale.set_sensitive(inst.chrome.background_opacity.is_some());
        ui::forward_wheel_to_page_scroller(&scale);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let scale = scale.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                scale.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_opacity =
                            if on { Some(scale.value() as f32) } else { None };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                if !enable.is_active() {
                    return;
                }
                let v = s.value() as f32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_opacity = Some(v.clamp(0.0, 1.0));
                    }
                });
            });
        }
        row.append(&enable);
        row.append(&scale);
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Background colour"));
        let has = inst
            .chrome
            .background_color
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        enable.set_active(has);
        let color = color_dialog_button();
        if let Some(hex) = inst
            .chrome
            .background_color
            .as_ref()
            .filter(|c| !c.is_empty())
        {
            color.set_rgba(&hex_to_rgba(hex));
        }
        color.set_sensitive(has);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let color = color.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                color.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_color = if on {
                            Some(rgba_to_hex(&color.rgba()))
                        } else {
                            None
                        };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            color.connect_rgba_notify(move |btn| {
                if !enable.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_color = Some(hex);
                    }
                });
            });
        }
        row.append(&enable);
        row.append(color.upcast_ref());
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Border width"));
        enable.set_active(inst.chrome.border_width.is_some());
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 12.0, 0.5);
        scale.set_value(inst.chrome.border_width.unwrap_or(1.0) as f64);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_digits(1);
        scale.set_sensitive(inst.chrome.border_width.is_some());
        ui::forward_wheel_to_page_scroller(&scale);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let scale = scale.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                scale.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_width =
                            if on { Some(scale.value() as f32) } else { None };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                if !enable.is_active() {
                    return;
                }
                let v = s.value() as f32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_width = Some(v.clamp(0.0, 12.0));
                    }
                });
            });
        }
        row.append(&enable);
        row.append(&scale);
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Border colour"));
        let has = inst
            .chrome
            .border_color
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        enable.set_active(has);
        let color = color_dialog_button();
        if let Some(hex) = inst.chrome.border_color.as_ref().filter(|c| !c.is_empty()) {
            color.set_rgba(&hex_to_rgba(hex));
        }
        color.set_sensitive(has);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let color = color.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                color.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_color = if on {
                            Some(rgba_to_hex(&color.rgba()))
                        } else {
                            None
                        };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            color.connect_rgba_notify(move |btn| {
                if !enable.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_color = Some(hex);
                    }
                });
            });
        }
        row.append(&enable);
        row.append(color.upcast_ref());
        body.append(&row);
    }

    let clear = gtk::Button::with_label(&tr("Clear all overrides"));
    {
        let cfg = cfg.clone();
        let id = id.clone();
        let rebuild_body = rebuild_body.clone();
        clear.connect_clicked(move |_| {
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.chrome = DesktopWidgetChromeOverride::default();
                }
            });
            if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                rebuild();
            }
        });
    }
    body.append(&clear);

    expander.upcast()
}

/// Re-read `desktop-widgets.json`, apply `f`, write back, and ask the shell to
/// reload. Preserves geometry the shell saved after drag/resize.
fn mutate_from_disk(
    cfg: &RefCell<DesktopWidgetsConfig>,
    f: impl FnOnce(&mut DesktopWidgetsConfig),
) {
    let mut disk = load_desktop_widgets_config();
    f(&mut disk);
    *cfg.borrow_mut() = disk.clone();
    if let Err(err) = save_desktop_widgets_config(&disk) {
        tracing::warn!(%err, "failed to save desktop-widgets.json");
    }
    crate::runtime::send_widgets("reload-desktop-widgets");
}

/// Like [`mutate_from_disk`], but coalesces rapid calls (opacity / border sliders).
fn mutate_from_disk_debounced(
    cfg: &Rc<RefCell<DesktopWidgetsConfig>>,
    pending: &Rc<RefCell<Option<glib::SourceId>>>,
    f: impl FnOnce(&mut DesktopWidgetsConfig) + 'static,
) {
    f(&mut cfg.borrow_mut());
    let snapshot = cfg.borrow().clone();

    if let Some(id) = pending.borrow_mut().take() {
        id.remove();
    }
    let cfg = Rc::clone(cfg);
    let pending_timer = Rc::clone(pending);
    let pending_slot = Rc::clone(pending);
    let source =
        glib::timeout_add_local(Duration::from_millis(CHROME_SAVE_DEBOUNCE_MS), move || {
            *pending_timer.borrow_mut() = None;
            let mut disk = load_desktop_widgets_config();
            disk.chrome = snapshot.chrome.clone();
            for inst in &mut disk.instances {
                if let Some(src) = snapshot.instances.iter().find(|i| i.id == inst.id) {
                    let (x, y, w, h, output) =
                        (inst.x, inst.y, inst.w, inst.h, inst.output.clone());
                    *inst = src.clone();
                    inst.x = x;
                    inst.y = y;
                    inst.w = w;
                    inst.h = h;
                    inst.output = output;
                }
            }
            *cfg.borrow_mut() = disk.clone();
            if let Err(err) = save_desktop_widgets_config(&disk) {
                tracing::warn!(%err, "failed to save desktop-widgets.json");
            }
            crate::runtime::send_widgets("reload-desktop-widgets");
            glib::ControlFlow::Break
        });
    *pending_slot.borrow_mut() = Some(source);
}
