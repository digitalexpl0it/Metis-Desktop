//! Non-classic pack layouts (Bracket / Ledger / Mosaic / Ladder / Plaza / Crest / Chip).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{KeyboardMode, LayerShell};

use crate::gtk_cb::OptFn0Cell;
use crate::services::{AppEntry, applications};

use super::chrome::{
    CHIP_ICON_SIZE, GRID_ICON_SIZE, activate_or_launch_settings, app_image, app_row,
    attach_pin_target, build_user_header, clear_box, rail_button, refresh_user_header,
    restore_search_focus, run_detached, wire_vertical_scroll,
};
use super::pin::PinContext;
use super::register_menu_popover;

pub(crate) fn install_pack_layout(button: &gtk::Button, menu_cfg: &metis_config::MenuConfig) {
    let style = menu_cfg.style;
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    panel.add_css_class("metis-bar-dropdown-panel");
    panel.add_css_class("metis-menu-panel");
    panel.add_css_class(style.css_class());

    let overlay = gtk::Overlay::new();
    let tip = gtk::Label::new(None);
    tip.add_css_class("metis-menu-tooltip-label");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);

    let user_header_widgets: Option<(gtk::Image, gtk::Label)> =
        if style == metis_config::MenuStyle::Crest || menu_cfg.show_user_header {
            let (header, avatar, name) = if style == metis_config::MenuStyle::Crest {
                build_crest_header(menu_cfg)
            } else {
                build_user_header(menu_cfg)
            };
            panel.append(&header);
            Some((avatar, name))
        } else {
            None
        };

    let search = gtk::SearchEntry::builder()
        .placeholder_text(metis_i18n::tr("Search applications…"))
        .hexpand(true)
        .build();
    search.add_css_class("metis-menu-search");
    if style.search_on_top() {
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_row.add_css_class("metis-menu-search-row");
        search_row.append(&search);
        panel.append(&search_row);
    }

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.add_css_class("metis-menu-body");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let apps_host = gtk::Box::new(gtk::Orientation::Vertical, 4);
    apps_host.set_hexpand(true);
    apps_host.set_vexpand(true);

    let category_list = gtk::ListBox::new();
    category_list.set_selection_mode(gtk::SelectionMode::Browse);
    category_list.add_css_class("metis-menu-categories");
    let selected_category = Rc::new(RefCell::new("frequent".to_string()));

    if style.uses_categories() {
        let cat_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .width_request(if style == metis_config::MenuStyle::Ladder {
                140
            } else {
                160
            })
            .child(&category_list)
            .build();
        cat_scroll.add_css_class("metis-menu-scroll");
        wire_vertical_scroll(&cat_scroll, &cat_scroll);
        for (id, label, _) in applications::MENU_CATEGORY_DEFS {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(id);
            let lbl = gtk::Label::new(Some(&metis_i18n::tr(label)));
            lbl.set_xalign(0.0);
            lbl.set_margin_start(10);
            lbl.set_margin_end(10);
            lbl.set_margin_top(8);
            lbl.set_margin_bottom(8);
            row.set_child(Some(&lbl));
            category_list.append(&row);
        }
        if let Some(row) = category_list.row_at_index(0) {
            category_list.select_row(Some(&row));
        }
        body.append(&cat_scroll);
        if style == metis_config::MenuStyle::Ladder {
            let div = gtk::Separator::new(gtk::Orientation::Vertical);
            div.add_css_class("metis-menu-divider");
            body.append(&div);
        }
    }

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list_box.add_css_class("metis-menu-list");
    list_box.set_hexpand(true);

    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(if style.dense_grid() { 5 } else { 4 })
        .max_children_per_line(if style.dense_grid() { 6 } else { 5 })
        .row_spacing(6)
        .column_spacing(6)
        .valign(gtk::Align::Start)
        .build();
    grid.add_css_class("metis-menu-app-grid");
    if style.dense_grid() {
        grid.add_css_class("metis-menu-app-grid-dense");
    }

    let apps_scroll = if style.uses_app_grid() {
        let wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrap.set_valign(gtk::Align::Start);
        wrap.append(&grid);
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&wrap)
            .build()
    } else {
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&list_box)
            .build()
    };
    apps_scroll.add_css_class("metis-menu-scroll");
    wire_vertical_scroll(&apps_scroll, &apps_scroll);
    apps_host.append(&apps_scroll);
    body.append(&apps_host);
    panel.append(&body);

    let mut user_header_widgets = user_header_widgets;
    if style == metis_config::MenuStyle::Plaza {
        let (footer, avatar, name) = build_plaza_footer(&overlay, &tip, menu_cfg);
        panel.append(&footer);
        if user_header_widgets.is_none() {
            user_header_widgets = Some((avatar, name));
        }
    }

    overlay.set_child(Some(&panel));
    overlay.add_overlay(&tip);
    let pin_ctx = PinContext::new(&overlay);
    {
        let pin_ctx = pin_ctx.clone();
        let overlay_for_pick = overlay.clone();
        let dismiss = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        dismiss.connect_pressed(move |_, _, x, y| {
            if !pin_ctx.panel.is_visible() {
                return;
            }
            if let Some(target) = overlay_for_pick.pick(x, y, gtk::PickFlags::DEFAULT) {
                let mut node = Some(target);
                while let Some(w) = node {
                    if w == pin_ctx.panel {
                        return;
                    }
                    node = w.parent();
                }
            }
            pin_ctx.dismiss();
        });
        overlay.add_controller(dismiss);
    }

    let rebuild_slot: OptFn0Cell = Rc::new(RefCell::new(None));
    let refresh: Rc<dyn Fn()> = {
        let slot = rebuild_slot.clone();
        Rc::new(move || {
            if let Some(f) = slot.borrow().clone() {
                f();
            }
        })
    };
    pin_ctx.set_refresh(refresh);

    let rebuild: Rc<dyn Fn()> = {
        let list_box = list_box.clone();
        let grid = grid.clone();
        let search = search.clone();
        let pin_ctx = pin_ctx.clone();
        let selected_category = selected_category.clone();
        Rc::new(move || {
            let keep = search.has_focus();
            let query = search.text().to_string();
            let apps = applications::list_apps();
            pin_ctx.dismiss();
            pin_ctx.clear_targets();
            clear_box(&list_box);
            while let Some(child) = grid.first_child() {
                grid.remove(&child);
            }
            let filtered = if style.uses_categories() {
                applications::apps_in_category(&apps, &selected_category.borrow(), &query)
            } else if query.trim().is_empty() {
                apps.clone()
            } else {
                applications::search_in(&apps, &query)
            };
            if style.uses_app_grid() {
                let icon = if style.dense_grid() {
                    CHIP_ICON_SIZE
                } else {
                    GRID_ICON_SIZE
                };
                for entry in &filtered {
                    grid.append(&grid_tile(entry, icon, &pin_ctx));
                }
            } else {
                for entry in &filtered {
                    list_box.append(&app_row(entry, &pin_ctx));
                }
                if filtered.is_empty() {
                    let empty = gtk::Label::new(Some(&metis_i18n::tr("No apps found.")));
                    empty.add_css_class("metis-menu-empty");
                    empty.set_halign(gtk::Align::Start);
                    list_box.append(&empty);
                }
            }
            restore_search_focus(&search, keep);
        })
    };
    *rebuild_slot.borrow_mut() = Some(rebuild.clone());
    applications::register_refresh(rebuild.clone());

    if style.uses_categories() {
        let selected_category = selected_category.clone();
        let rebuild = rebuild.clone();
        category_list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            selected_category.replace(row.widget_name().to_string());
            rebuild();
        });
    }

    let search_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let search_changed = {
        let rebuild = rebuild.clone();
        let search_debounce = search_debounce.clone();
        search.connect_search_changed(move |_| {
            if let Some(id) = search_debounce.borrow_mut().take() {
                id.remove();
            }
            let rebuild = rebuild.clone();
            let debounce_slot = search_debounce.clone();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
                *debounce_slot.borrow_mut() = None;
                rebuild();
                glib::ControlFlow::Break
            });
            *search_debounce.borrow_mut() = Some(id);
        })
    };

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&overlay)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.add_css_class("metis-menu-popover");
    popover.set_parent(button);
    register_menu_popover(&popover);
    search.set_key_capture_widget(Some(&popover));

    {
        let btn = button.clone();
        let rebuild = rebuild.clone();
        let search = search.clone();
        let user_header_widgets = user_header_widgets.clone();
        popover.connect_map(move |popover| {
            btn.add_css_class("metis-bar-dropdown-active");
            if let Some(window) = popover.root().and_downcast::<gtk::Window>() {
                window.set_keyboard_mode(KeyboardMode::Exclusive);
            }
            if let Some((avatar, name)) = user_header_widgets.as_ref() {
                refresh_user_header(avatar, name);
            }
            search.block_signal(&search_changed);
            search.set_text("");
            search.unblock_signal(&search_changed);
            applications::invalidate_app_cache();
            let rebuild = rebuild.clone();
            glib::idle_add_local_once(move || rebuild());
            search.grab_focus();
        });
    }
    {
        let btn = button.clone();
        let pin_ctx = pin_ctx.clone();
        popover.connect_unmap(move |popover| {
            pin_ctx.dismiss();
            btn.remove_css_class("metis-bar-dropdown-active");
            if let Some(window) = popover.root().and_downcast::<gtk::Window>() {
                window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
        });
    }
    crate::ui::bar::dropdown::register(&popover);

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            crate::ui::bar::dropdown::clear_trigger_highlight(&popover);
            glib::idle_add_local_once(move || popover.popdown());
            return;
        }
        crate::ui::bar::dropdown::close_all();
        glib::idle_add_local_once(move || popover.popup());
    });
}

pub(crate) fn build_crest_header(
    cfg: &metis_config::MenuConfig,
) -> (gtk::Box, gtk::Image, gtk::Label) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    col.add_css_class("metis-menu-crest-header");
    col.set_halign(gtk::Align::Center);
    let avatar = gtk::Image::new();
    avatar.add_css_class("metis-menu-crest-avatar");
    avatar.set_pixel_size(72);
    let name = gtk::Label::builder()
        .label(cfg.resolved_display_name())
        .halign(gtk::Align::Center)
        .build();
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    col.append(&avatar);
    col.append(&name);
    (col, avatar, name)
}

pub(crate) fn build_plaza_footer(
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
    cfg: &metis_config::MenuConfig,
) -> (gtk::Box, gtk::Image, gtk::Label) {
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("metis-menu-plaza-footer");
    footer.set_hexpand(true);
    let avatar = gtk::Image::new();
    avatar.set_pixel_size(28);
    avatar.add_css_class("metis-menu-user-avatar");
    let name = gtk::Label::new(Some(&cfg.resolved_display_name()));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    footer.append(&avatar);
    footer.append(&name);
    footer.append(&rail_button(
        overlay,
        tip,
        "preferences-system-symbolic",
        &metis_i18n::tr("Settings"),
        || {
            crate::ui::bar::dropdown::close_all();
            activate_or_launch_settings();
        },
    ));
    footer.append(&rail_button(
        overlay,
        tip,
        "system-shutdown-symbolic",
        &metis_i18n::tr("Shut Down"),
        || {
            run_detached("systemctl", &["poweroff"]);
            crate::ui::bar::dropdown::request_close_all();
        },
    ));
    (footer, avatar, name)
}

pub(crate) fn grid_tile(entry: &AppEntry, icon_size: i32, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let tile = gtk::Button::builder().has_frame(false).build();
    tile.add_css_class("metis-menu-tile");
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_halign(gtk::Align::Center);
    vbox.append(&app_image(entry, icon_size));
    let label = gtk::Label::new(Some(&entry.name));
    label.add_css_class("metis-menu-tile-label");
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(10);
    vbox.append(&label);
    tile.set_child(Some(&vbox));
    {
        let entry = entry.clone();
        tile.connect_clicked(move |_| {
            crate::ui::bar::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&tile, &entry.id, pin_ctx);
    tile
}
