//! Edge bar: position, per-display placement, workspace + layout behaviour,
//! tray mode, distance from the screen edge, opacity, backdrop blur, ambient
//! tint, and the
//! bar's pill border. All fields live in `bar.json`, which the shell watches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    BarWidgetId, disable_status_widget, enable_status_widget, rebuild_bar_widgets,
    split_bar_widget_layout,
};

use crate::gtk_cb::OptFn0Cell;
use crate::pages::appearance_common::{
    color_dialog_button, hex_to_rgba, rgba_to_hex, set_stops, update_bar,
};
use crate::{runtime, ui};
use metis_i18n::tr;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("edgebar");

    let bar = metis_config::load_bar_config();
    let (bar_card, bar_body) =
        ui::section_with_icon(&tr("Edge bar"), "preferences-system-symbolic");

    let position_dd = {
        let __dd_labels = [tr("Top"), tr("Bottom"), tr("Left"), tr("Right")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    position_dd.set_selected(bar_position_to_index(bar.position));
    bar_body.append(&ui::row_with_icon(
        "view-paged-symbolic",
        &tr("Position"),
        &position_dd,
    ));

    let displays_dd = {
        let __dd_labels = [tr("All displays"), tr("Primary display only")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    displays_dd.set_selected(bar_displays_to_index(bar.displays));
    bar_body.append(&ui::row_with_icon(
        "video-display-symbolic",
        &tr("Show bar on"),
        &displays_dd,
    ));

    let workspace_mode_dd = {
        let __dd_labels = [tr("Independent per display"), tr("Linked across displays")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    workspace_mode_dd.set_selected(workspace_mode_to_index(bar.workspace_mode));
    workspace_mode_dd.set_tooltip_text(Some(&tr(
        "Independent: each monitor keeps its own workspaces.\n\
         Linked: switching a workspace moves every monitor at once.",
    )));
    bar_body.append(&ui::row_with_icon(
        "view-grid-symbolic",
        &tr("Workspaces"),
        &workspace_mode_dd,
    ));

    let default_layout_dd = {
        let __dd_labels = [tr("Free desktop"), tr("Grid tiling"), tr("Scrolling")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    default_layout_dd.set_selected(default_layout_to_index(bar.default_layout));
    default_layout_dd.set_tooltip_text(Some(&tr("Applies to every workspace right away.\n\
         Toggle the active workspace with Super + \\ (cycles free → grid → scroll).")));
    bar_body.append(&ui::row_with_icon(
        "view-dual-symbolic",
        &tr("New workspace layout"),
        &default_layout_dd,
    ));

    let tray_mode_dd = {
        let __dd_labels = [tr("Collapsed (popup list)"), tr("Pinned on bar")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    tray_mode_dd.set_selected(tray_icon_mode_to_index(bar.tray_icon_mode));
    tray_mode_dd.set_tooltip_text(Some(&tr(
        "Collapsed: one tray button opens a popover with all background app icons.\n\
         Pinned: tray icons stay visible on the bar, left of the tray button.",
    )));
    bar_body.append(&ui::row_with_icon(
        "view-more-horizontal-symbolic",
        &tr("System tray icons"),
        &tray_mode_dd,
    ));

    let edge_margin = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 64.0, 1.0);
    edge_margin.set_value(bar.margin_top as f64);
    edge_margin.set_size_request(200, -1);
    edge_margin.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&edge_margin);
    bar_body.append(&ui::row_with_icon(
        "view-fullscreen-symbolic",
        &tr("Distance from edge"),
        &edge_margin,
    ));

    let length = gtk::Scale::with_range(gtk::Orientation::Horizontal, 40.0, 100.0, 1.0);
    length.set_value(bar.length_percent as f64);
    length.set_size_request(200, -1);
    length.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&length);
    bar_body.append(&ui::row_with_icon(
        "object-select-symbolic",
        &tr("Bar length"),
        &length,
    ));
    let length_hint = gtk::Label::new(Some(&tr(
        "Percent of the screen edge. The bar stays centered (100% = full edge).",
    )));
    length_hint.set_xalign(0.0);
    length_hint.set_wrap(true);
    length_hint.add_css_class("metis-settings-hint");
    bar_body.append(&length_hint);

    let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 1.0, 0.01);
    opacity.set_value(bar.opacity as f64);
    opacity.set_size_request(200, -1);
    opacity.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&opacity);
    bar_body.append(&ui::row_with_icon(
        "display-brightness-symbolic",
        &tr("Opacity"),
        &opacity,
    ));

    let blur = gtk::Switch::new();
    blur.set_active(bar.blur);
    blur.set_halign(gtk::Align::End);
    blur.set_valign(gtk::Align::Center);
    bar_body.append(&ui::row_with_icon(
        "weather-fog-symbolic",
        &tr("Backdrop blur"),
        &blur,
    ));

    let blur_radius = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 40.0, 1.0);
    blur_radius.set_value(bar.blur_radius as f64);
    blur_radius.set_size_request(200, -1);
    blur_radius.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&blur_radius);
    bar_body.append(&ui::row_with_icon(
        "weather-fog-symbolic",
        &tr("Blur radius"),
        &blur_radius,
    ));

    let blur_hint = gtk::Label::new(Some(&tr(
        "Blur frosts the wallpaper behind the bar — it needs a wallpaper set and \
         the bar opacity below 1.0 to show through. Changes apply within ~1s.",
    )));
    blur_hint.set_xalign(0.0);
    blur_hint.set_wrap(true);
    blur_hint.add_css_class("metis-settings-hint");
    bar_body.append(&blur_hint);

    let ambient = gtk::Switch::new();
    ambient.set_active(bar.ambient_tint);
    ambient.set_halign(gtk::Align::End);
    ambient.set_valign(gtk::Align::Center);
    bar_body.append(&ui::row_with_icon(
        "colorimeter-colorhug-symbolic",
        &tr("Ambient tint"),
        &ambient,
    ));
    let ambient_hint = gtk::Label::new(Some(&tr(
        "Tints Control Center and Notification Center frost slightly from the \
         wallpaper. Themes stay the source of truth — this is only a subtle lift.",
    )));
    ambient_hint.set_xalign(0.0);
    ambient_hint.set_wrap(true);
    ambient_hint.add_css_class("metis-settings-hint");
    bar_body.append(&ambient_hint);

    // -- Bar fill (theme / solid / gradient) --
    let bf = bar.bar_fill.clone();
    let bf_mode = {
        let __dd_labels = [tr("Theme"), tr("Solid color"), tr("Custom gradient")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    bf_mode.set_selected(match bf.mode {
        metis_config::BarFillMode::Theme => 0,
        metis_config::BarFillMode::Solid => 1,
        metis_config::BarFillMode::Gradient => 2,
    });
    bar_body.append(&ui::row_with_icon(
        "preferences-color-symbolic",
        &tr("Bar background"),
        &bf_mode,
    ));

    let bf_solid_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let bf_solid = color_dialog_button();
    bf_solid.set_rgba(&hex_to_rgba(&bf.color));
    bf_solid_box.append(&ui::row_with_icon(
        "applications-graphics-symbolic",
        &tr("Background color"),
        &bf_solid,
    ));
    bar_body.append(&bf_solid_box);

    let bf_grad_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let bf_stop = |idx: usize, fallback: &str| {
        let b = color_dialog_button();
        let hex = bf.gradient.get(idx).map(String::as_str).unwrap_or(fallback);
        b.set_rgba(&hex_to_rgba(hex));
        b
    };
    let bf_g1 = bf_stop(0, "#1a1b26");
    let bf_g2 = bf_stop(1, "#2a2b3d");
    bf_grad_box.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Gradient start"),
        &bf_g1,
    ));
    bf_grad_box.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Gradient end"),
        &bf_g2,
    ));
    let bf_dir = {
        let __dd_labels = [
            tr("Auto (along bar)"),
            tr("Left → right"),
            tr("Right → left"),
            tr("Top → bottom"),
            tr("Bottom → top"),
        ];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    bf_dir.set_selected(match bf.gradient_direction {
        metis_config::BarGradientDirection::Auto => 0,
        metis_config::BarGradientDirection::ToRight => 1,
        metis_config::BarGradientDirection::ToLeft => 2,
        metis_config::BarGradientDirection::ToBottom => 3,
        metis_config::BarGradientDirection::ToTop => 4,
    });
    bf_grad_box.append(&ui::row_with_icon(
        "go-next-symbolic",
        &tr("Gradient direction"),
        &bf_dir,
    ));
    bar_body.append(&bf_grad_box);

    let bf_update_vis = {
        let bf_solid_box = bf_solid_box.clone();
        let bf_grad_box = bf_grad_box.clone();
        Rc::new(move |mode: metis_config::BarFillMode| {
            bf_solid_box.set_visible(mode == metis_config::BarFillMode::Solid);
            bf_grad_box.set_visible(mode == metis_config::BarFillMode::Gradient);
        })
    };
    bf_update_vis(bf.mode);

    // -- Auto-hide --
    let auto_hide = gtk::Switch::new();
    auto_hide.set_active(bar.auto_hide);
    auto_hide.set_halign(gtk::Align::End);
    auto_hide.set_valign(gtk::Align::Center);
    bar_body.append(&ui::row_with_icon(
        "window-minimize-symbolic",
        &tr("Auto-hide"),
        &auto_hide,
    ));

    let auto_hide_opts = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let auto_peek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 2.0, 8.0, 1.0);
    auto_peek.set_value(bar.auto_hide_peek_px as f64);
    auto_peek.set_size_request(200, -1);
    auto_peek.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&auto_peek);
    auto_hide_opts.append(&ui::row_with_icon(
        "view-reveal-symbolic",
        &tr("Peek thickness"),
        &auto_peek,
    ));
    bar_body.append(&auto_hide_opts);
    auto_hide_opts.set_sensitive(bar.auto_hide);

    let auto_hint = gtk::Label::new(Some(&tr(
        "Hides as soon as the pointer leaves, sliding to a thin peek. Move back \
         to the edge to slide it open again (same motion as titlebars). Popovers \
         and Control Center keep the bar visible while open.",
    )));
    auto_hint.set_xalign(0.0);
    auto_hint.set_wrap(true);
    auto_hint.add_css_class("metis-settings-hint");
    bar_body.append(&auto_hint);

    // -- Edge-bar border (mode / width / colors) --
    let bb = bar.bar_border.clone();

    let bb_mode = {
        let __dd_labels = [tr("Theme accent"), tr("Solid color"), tr("Custom gradient")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    bb_mode.set_selected(match bb.mode {
        metis_config::BorderMode::Accent => 0,
        metis_config::BorderMode::Solid => 1,
        metis_config::BorderMode::Gradient => 2,
    });
    bar_body.append(&ui::row_with_icon(
        "view-paged-symbolic",
        &tr("Bar border"),
        &bb_mode,
    ));

    let bb_width = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 8.0, 1.0);
    bb_width.set_value(bb.width_px as f64);
    bb_width.set_size_request(200, -1);
    bb_width.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&bb_width);
    bar_body.append(&ui::row_with_icon(
        "view-fullscreen-symbolic",
        &tr("Bar border thickness"),
        &bb_width,
    ));

    let bb_solid_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let bb_solid = color_dialog_button();
    bb_solid.set_rgba(&hex_to_rgba(&bb.color));
    bb_solid_box.append(&ui::row_with_icon(
        "applications-graphics-symbolic",
        &tr("Border color"),
        &bb_solid,
    ));
    bar_body.append(&bb_solid_box);

    let bb_grad_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let bb_stop = |idx: usize, fallback: &str| {
        let b = color_dialog_button();
        let hex = bb.gradient.get(idx).map(String::as_str).unwrap_or(fallback);
        b.set_rgba(&hex_to_rgba(hex));
        b
    };
    let bb_g1 = bb_stop(0, "#00F2FE");
    let bb_g2 = bb_stop(1, "#4FACFE");
    let bb_g3 = bb_stop(2, "#A24BFF");
    bb_grad_box.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Gradient start"),
        &bb_g1,
    ));
    bb_grad_box.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Gradient middle"),
        &bb_g2,
    ));
    bb_grad_box.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Gradient end"),
        &bb_g3,
    ));
    bar_body.append(&bb_grad_box);

    let bb_hint = gtk::Label::new(Some(
        "The border around the edge bar's pill. \"Theme accent\" tracks your accent \
         colors; or pick a solid color / custom gradient. Set thickness to 0 to \
         disable it. Changes apply within ~1s.",
    ));
    bb_hint.set_xalign(0.0);
    bb_hint.set_wrap(true);
    bb_hint.add_css_class("metis-settings-hint");
    bar_body.append(&bb_hint);

    let bb_update_vis = {
        let bb_solid_box = bb_solid_box.clone();
        let bb_grad_box = bb_grad_box.clone();
        Rc::new(move |mode: metis_config::BorderMode| {
            bb_solid_box.set_visible(mode == metis_config::BorderMode::Solid);
            bb_grad_box.set_visible(mode == metis_config::BorderMode::Gradient);
        })
    };
    bb_update_vis(bb.mode);

    content.append(&bar_card);

    // ---- Status icons (toggle + order) ------------------------------------
    let (status_card, status_body) =
        ui::section_with_icon(&tr("Status icons"), "view-list-symbolic");
    let status_hint = gtk::Label::new(Some(&tr(
        "Enable icons here, then drag them on the edge bar (past the drag \
         threshold) or use the arrows below to reorder — both update the bar \
         live and persist in bar.json. Icons also hide automatically when \
         unused (no VPN profiles, no Bluetooth adapter, weather unavailable, \
         and so on). Workspaces, running apps, and the clock stay on the bar.",
    )));
    status_hint.set_xalign(0.0);
    status_hint.set_wrap(true);
    status_hint.add_css_class("metis-settings-hint");
    status_body.append(&status_hint);

    let toggle_grid = gtk::FlowBox::new();
    toggle_grid.set_selection_mode(gtk::SelectionMode::None);
    toggle_grid.set_max_children_per_line(2);
    toggle_grid.set_column_spacing(16);
    toggle_grid.set_row_spacing(8);
    status_body.append(&toggle_grid);

    let order_label = gtk::Label::new(Some(&tr("Order on bar")));
    order_label.set_xalign(0.0);
    order_label.add_css_class("metis-settings-hint");
    status_body.append(&order_label);

    let order_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    order_list.add_css_class("metis-widget-list");
    status_body.append(&order_list);

    let (_, initial_status, _) = split_bar_widget_layout(&bar.widgets);
    let status_state = Rc::new(RefCell::new(initial_status));
    let seeding = Rc::new(Cell::new(true));
    let refresh_order: OptFn0Cell = Rc::new(RefCell::new(None));
    {
        let order_list = order_list.clone();
        let status_state = status_state.clone();
        let refresh_slot = refresh_order.clone();
        let refresh = Rc::new(move || {
            while let Some(child) = order_list.first_child() {
                order_list.remove(&child);
            }
            let status = status_state.borrow().clone();
            if status.is_empty() {
                let empty = gtk::Label::new(Some(&tr("No status icons enabled.")));
                empty.set_xalign(0.0);
                empty.add_css_class("metis-settings-hint");
                order_list.append(&empty);
                return;
            }
            for (idx, id) in status.iter().enumerate() {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.set_hexpand(true);
                row.add_css_class("metis-widget-list-row");
                if idx % 2 == 1 {
                    row.add_css_class("metis-widget-list-row-alt");
                }
                let name = gtk::Label::new(Some(&tr(id.label())));
                name.set_xalign(0.0);
                name.set_hexpand(true);
                name.add_css_class("metis-widget-list-title");
                row.append(&name);

                let up = gtk::Button::from_icon_name("go-up-symbolic");
                up.set_tooltip_text(Some(&tr("Move up")));
                up.set_sensitive(idx > 0);
                let down = gtk::Button::from_icon_name("go-down-symbolic");
                down.set_tooltip_text(Some(&tr("Move down")));
                down.set_sensitive(idx + 1 < status.len());

                let move_id = *id;
                {
                    let status_state = status_state.clone();
                    let refresh_slot = refresh_slot.clone();
                    up.connect_clicked(move |_| {
                        {
                            let mut status = status_state.borrow_mut();
                            if let Some(pos) = status.iter().position(|w| *w == move_id)
                                && pos > 0
                            {
                                status.swap(pos, pos - 1);
                            }
                            let next = status.clone();
                            drop(status);
                            update_bar(move |c| c.widgets = rebuild_bar_widgets(&next));
                        }
                        if let Some(refresh) = refresh_slot.borrow().as_ref() {
                            refresh();
                        }
                    });
                }
                {
                    let status_state = status_state.clone();
                    let refresh_slot = refresh_slot.clone();
                    down.connect_clicked(move |_| {
                        {
                            let mut status = status_state.borrow_mut();
                            if let Some(pos) = status.iter().position(|w| *w == move_id)
                                && pos + 1 < status.len()
                            {
                                status.swap(pos, pos + 1);
                            }
                            let next = status.clone();
                            drop(status);
                            update_bar(move |c| c.widgets = rebuild_bar_widgets(&next));
                        }
                        if let Some(refresh) = refresh_slot.borrow().as_ref() {
                            refresh();
                        }
                    });
                }
                row.append(&up);
                row.append(&down);
                order_list.append(&row);
            }
        });
        *refresh_order.borrow_mut() = Some(refresh.clone());
        refresh();
    }

    for &id in BarWidgetId::status_optional_catalogue() {
        let check = gtk::CheckButton::with_label(&tr(id.label()));
        check.set_active(status_state.borrow().contains(&id));
        let seeding = seeding.clone();
        let status_state = status_state.clone();
        let refresh_order = refresh_order.clone();
        check.connect_active_notify(move |btn| {
            if seeding.get() {
                return;
            }
            let on = btn.is_active();
            {
                let mut widgets = rebuild_bar_widgets(&status_state.borrow());
                if on {
                    enable_status_widget(&mut widgets, id);
                } else {
                    disable_status_widget(&mut widgets, id);
                }
                let (_, status, _) = split_bar_widget_layout(&widgets);
                *status_state.borrow_mut() = status.clone();
                update_bar(move |c| c.widgets = rebuild_bar_widgets(&status));
            }
            if let Some(refresh) = refresh_order.borrow().as_ref() {
                refresh();
            }
        });
        toggle_grid.append(&check);
    }
    seeding.set(false);
    content.append(&status_card);

    // ---- Wiring -----------------------------------------------------------
    position_dd.connect_selected_notify(move |dd| {
        let v = index_to_bar_position(dd.selected());
        update_bar(move |c| c.position = v);
    });
    displays_dd.connect_selected_notify(move |dd| {
        let v = index_to_bar_displays(dd.selected());
        update_bar(move |c| c.displays = v);
    });
    workspace_mode_dd.connect_selected_notify(move |dd| {
        let v = index_to_workspace_mode(dd.selected());
        update_bar(move |c| c.workspace_mode = v);
    });
    default_layout_dd.connect_selected_notify(move |dd| {
        let layout = index_to_default_layout(dd.selected());
        update_bar(move |c| c.default_layout = layout);
        // Apply live to every workspace so the dropdown is a real on/off switch,
        // not just a seed for future workspaces.
        runtime::apply_default_layout(match layout {
            metis_config::DefaultLayout::Free => metis_protocol::LayoutKind::Free,
            metis_config::DefaultLayout::Grid => metis_protocol::LayoutKind::Grid,
            metis_config::DefaultLayout::Scroll => metis_protocol::LayoutKind::Scroll,
        });
    });
    tray_mode_dd.connect_selected_notify(move |dd| {
        let v = index_to_tray_icon_mode(dd.selected());
        update_bar(move |c| c.tray_icon_mode = v);
    });
    edge_margin.connect_value_changed(move |s| {
        let v = s.value().round() as u32;
        update_bar(move |c| c.margin_top = v);
    });
    length.connect_value_changed(move |s| {
        let v = s.value().round() as u32;
        update_bar(move |c| c.length_percent = v.clamp(40, 100));
    });
    opacity.connect_value_changed(move |s| {
        let v = s.value() as f32;
        update_bar(move |c| c.opacity = v);
    });
    ui::defer_switch_active_notify(&blur, move |active| {
        update_bar(move |c| c.blur = active);
    });
    ui::defer_switch_active_notify(&ambient, move |active| {
        update_bar(move |c| c.ambient_tint = active);
    });
    blur_radius.connect_value_changed(move |s| {
        let v = s.value() as f32;
        update_bar(move |c| c.blur_radius = v);
    });
    {
        let bf_update_vis = bf_update_vis.clone();
        bf_mode.connect_selected_notify(move |dd| {
            let mode = match dd.selected() {
                1 => metis_config::BarFillMode::Solid,
                2 => metis_config::BarFillMode::Gradient,
                _ => metis_config::BarFillMode::Theme,
            };
            bf_update_vis(mode);
            update_bar(move |c| c.bar_fill.mode = mode);
        });
    }
    bf_solid.connect_rgba_notify(move |b| {
        let hex = rgba_to_hex(&b.rgba());
        update_bar(move |c| c.bar_fill.color = hex);
    });
    for (idx, btn) in [(0usize, &bf_g1), (1, &bf_g2)] {
        btn.connect_rgba_notify(move |b| {
            let hex = rgba_to_hex(&b.rgba());
            update_bar(move |c| set_stops(&mut c.bar_fill.gradient, idx, hex));
        });
    }
    bf_dir.connect_selected_notify(move |dd| {
        let dir = match dd.selected() {
            1 => metis_config::BarGradientDirection::ToRight,
            2 => metis_config::BarGradientDirection::ToLeft,
            3 => metis_config::BarGradientDirection::ToBottom,
            4 => metis_config::BarGradientDirection::ToTop,
            _ => metis_config::BarGradientDirection::Auto,
        };
        update_bar(move |c| c.bar_fill.gradient_direction = dir);
    });
    {
        let auto_hide_opts = auto_hide_opts.clone();
        ui::defer_switch_active_notify(&auto_hide, move |active| {
            auto_hide_opts.set_sensitive(active);
            update_bar(move |c| c.auto_hide = active);
        });
    }
    auto_peek.connect_value_changed(move |s| {
        let v = s.value().round() as u32;
        update_bar(move |c| c.auto_hide_peek_px = v.clamp(2, 8));
    });
    {
        let bb_update_vis = bb_update_vis.clone();
        bb_mode.connect_selected_notify(move |dd| {
            let mode = match dd.selected() {
                1 => metis_config::BorderMode::Solid,
                2 => metis_config::BorderMode::Gradient,
                _ => metis_config::BorderMode::Accent,
            };
            bb_update_vis(mode);
            update_bar(move |c| c.bar_border.mode = mode);
        });
    }
    bb_width.connect_value_changed(move |s| {
        let v = s.value() as f32;
        update_bar(move |c| c.bar_border.width_px = v);
    });
    bb_solid.connect_rgba_notify(move |b| {
        let hex = rgba_to_hex(&b.rgba());
        update_bar(move |c| c.bar_border.color = hex);
    });
    for (idx, btn) in [(0usize, &bb_g1), (1, &bb_g2), (2, &bb_g3)] {
        btn.connect_rgba_notify(move |b| {
            let hex = rgba_to_hex(&b.rgba());
            update_bar(move |c| set_stops(&mut c.bar_border.gradient, idx, hex));
        });
    }

    scroller.upcast()
}

fn bar_position_to_index(pos: metis_config::BarPosition) -> u32 {
    use metis_config::BarPosition as P;
    match pos {
        P::Top => 0,
        P::Bottom => 1,
        P::Left => 2,
        P::Right => 3,
    }
}

fn index_to_bar_position(idx: u32) -> metis_config::BarPosition {
    use metis_config::BarPosition as P;
    match idx {
        1 => P::Bottom,
        2 => P::Left,
        3 => P::Right,
        _ => P::Top,
    }
}

fn bar_displays_to_index(displays: metis_config::BarDisplays) -> u32 {
    match displays {
        metis_config::BarDisplays::All => 0,
        metis_config::BarDisplays::Primary => 1,
    }
}

fn index_to_bar_displays(idx: u32) -> metis_config::BarDisplays {
    match idx {
        1 => metis_config::BarDisplays::Primary,
        _ => metis_config::BarDisplays::All,
    }
}

fn workspace_mode_to_index(mode: metis_config::WorkspaceMode) -> u32 {
    match mode {
        metis_config::WorkspaceMode::Separate => 0,
        metis_config::WorkspaceMode::Linked => 1,
    }
}

fn index_to_workspace_mode(idx: u32) -> metis_config::WorkspaceMode {
    match idx {
        1 => metis_config::WorkspaceMode::Linked,
        _ => metis_config::WorkspaceMode::Separate,
    }
}

fn tray_icon_mode_to_index(mode: metis_config::TrayIconMode) -> u32 {
    match mode {
        metis_config::TrayIconMode::Collapsed => 0,
        metis_config::TrayIconMode::Pinned => 1,
    }
}

fn index_to_tray_icon_mode(idx: u32) -> metis_config::TrayIconMode {
    match idx {
        1 => metis_config::TrayIconMode::Pinned,
        _ => metis_config::TrayIconMode::Collapsed,
    }
}

fn default_layout_to_index(layout: metis_config::DefaultLayout) -> u32 {
    match layout {
        metis_config::DefaultLayout::Free => 0,
        metis_config::DefaultLayout::Grid => 1,
        metis_config::DefaultLayout::Scroll => 2,
    }
}

fn index_to_default_layout(idx: u32) -> metis_config::DefaultLayout {
    match idx {
        1 => metis_config::DefaultLayout::Grid,
        2 => metis_config::DefaultLayout::Scroll,
        _ => metis_config::DefaultLayout::Free,
    }
}
