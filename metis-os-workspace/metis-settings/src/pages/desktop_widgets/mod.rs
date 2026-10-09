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

mod configure;
mod list;
mod options;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetInstance, DesktopWidgetKind, DesktopWidgetsConfig, find_widget_extension,
    load_desktop_widgets_config, save_desktop_widgets_config,
};

use crate::gtk_cb::OptFn0Cell;
use crate::pages::appearance_common::{color_dialog_button, hex_to_rgba, rgba_to_hex};
use crate::ui;
use metis_i18n::tr;

use configure::open_configure_dialog;
use list::{AddChoice, build_add_choices, instance_row};

/// Coalesce slider writes so dragging opacity doesn't storm the shell with
/// full config reloads / atomic renames.
pub(crate) const CHROME_SAVE_DEBOUNCE_MS: u64 = 180;

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

pub(crate) fn mutate_from_disk(
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
pub(crate) fn mutate_from_disk_debounced(
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
