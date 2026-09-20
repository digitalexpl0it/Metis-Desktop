//! Metis Menu settings: layout style, feature toggles, quick launchers
//! (terminal / file manager), and panel opacity. Layout changes persist to
//! `menu.json` and nudge `reload-bar` so the shell rebuilds the menu live.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{MenuConfig, MenuStyle};

use crate::{runtime, ui};
use metis_i18n::tr;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("menu");
    let cfg = metis_config::load_menu_config();
    let suppress = Rc::new(Cell::new(false));

    // ---- Layout -----------------------------------------------------------
    let (layout_card, layout_body) = ui::section_with_icon(&tr("Layout"), "view-grid-symbolic");

    let chooser = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .row_spacing(12)
        .column_spacing(12)
        .halign(gtk::Align::Fill)
        .build();
    chooser.add_css_class("metis-menu-style-chooser");

    // ---- Features (built before layout so ArcMenu can sync the user switch) -
    let (feat_card, feat_body) =
        ui::section_with_icon(&tr("Features"), "preferences-system-symbolic");

    let (user_row, user_sw) = ui::switch_row(&tr("Show user avatar & name"));
    user_sw.set_active(cfg.show_user_header);
    feat_body.append(&user_row);
    {
        let suppress = suppress.clone();
        ui::defer_switch_active_notify_when(
            &user_sw,
            {
                let suppress = suppress.clone();
                move || !suppress.get()
            },
            |active| {
                let mut c = metis_config::load_menu_config();
                if c.show_user_header == active {
                    return;
                }
                c.show_user_header = active;
                persist_and_reload(&c);
            },
        );
    }

    let mut first_btn: Option<gtk::ToggleButton> = None;
    for style in MenuStyle::ALL {
        let btn = layout_thumb_button(*style);
        if let Some(ref group) = first_btn {
            btn.set_group(Some(group));
        } else {
            first_btn = Some(btn.clone());
        }
        if *style == cfg.style {
            btn.set_active(true);
        }
        {
            let suppress = suppress.clone();
            let user_sw = user_sw.clone();
            let style = *style;
            btn.connect_toggled(move |b| {
                if suppress.get() || !b.is_active() {
                    return;
                }
                let mut c = metis_config::load_menu_config();
                if c.style == style {
                    return;
                }
                c.style = style;
                // ArcMenu is the user-header layout — turn the header on when
                // picking it so the style matches expectations out of the box.
                if style == MenuStyle::ArcMenu && !c.show_user_header {
                    c.show_user_header = true;
                    suppress.set(true);
                    user_sw.set_active(true);
                    suppress.set(false);
                }
                persist_and_reload(&c);
            });
        }
        chooser.append(&btn);
    }
    layout_body.append(&chooser);

    let layout_hint = gtk::Label::new(Some(&tr(
        "Metis is today’s classic menu. Other layouts rearrange search, the rail, \
         and pinned apps. Changes apply after the edge bar reloads (~1s).",
    )));
    layout_hint.set_xalign(0.0);
    layout_hint.set_wrap(true);
    layout_hint.add_css_class("metis-settings-hint");
    layout_body.append(&layout_hint);
    content.append(&layout_card);

    let (rail_row, rail_sw) = ui::switch_row(&tr("Show places & power rail"));
    rail_sw.set_active(cfg.show_rail);
    feat_body.append(&rail_row);
    {
        let suppress = suppress.clone();
        ui::defer_switch_active_notify_when(
            &rail_sw,
            {
                let suppress = suppress.clone();
                move || !suppress.get()
            },
            |active| {
                let mut c = metis_config::load_menu_config();
                if c.show_rail == active {
                    return;
                }
                c.show_rail = active;
                persist_and_reload(&c);
            },
        );
    }

    let (pin_row, pin_sw) = ui::switch_row(&tr("Show pinned apps"));
    pin_sw.set_active(cfg.show_pinned);
    feat_body.append(&pin_row);
    {
        let suppress = suppress.clone();
        ui::defer_switch_active_notify_when(
            &pin_sw,
            {
                let suppress = suppress.clone();
                move || !suppress.get()
            },
            |active| {
                let mut c = metis_config::load_menu_config();
                if c.show_pinned == active {
                    return;
                }
                c.show_pinned = active;
                persist_and_reload(&c);
            },
        );
    }

    let feat_hint = gtk::Label::new(Some(&tr(
        "Avatar uses ~/.face, ~/.face.icon, or the GNOME/KDE AccountsService \
         picture when present (or a path in menu.json). Display name falls back \
         to your account full name, then $USER.",
    )));
    feat_hint.set_xalign(0.0);
    feat_hint.set_wrap(true);
    feat_hint.add_css_class("metis-settings-hint");
    feat_body.append(&feat_hint);
    content.append(&feat_card);

    // ---- Quick launchers --------------------------------------------------
    let (launch_card, launch_body) =
        ui::section_with_icon(&tr("Quick launchers"), "applications-utilities-symbolic");

    launch_body.append(&ui::launcher_picker(
        "utilities-terminal-symbolic",
        &tr("Terminal"),
        metis_config::KNOWN_TERMINALS,
        cfg.terminal.clone(),
        |val| {
            let mut c = metis_config::load_menu_config();
            c.terminal = val;
            persist(&c);
        },
    ));

    launch_body.append(&ui::launcher_picker(
        "system-file-manager-symbolic",
        &tr("File manager"),
        metis_config::KNOWN_FILE_MANAGERS,
        cfg.file_manager.clone(),
        |val| {
            let mut c = metis_config::load_menu_config();
            c.file_manager = val;
            persist(&c);
        },
    ));

    let launch_hint = gtk::Label::new(Some(&tr(
        "Auto-detect uses the first installed option. Choose Custom to point at any \
         executable on your system.",
    )));
    launch_hint.set_xalign(0.0);
    launch_hint.set_wrap(true);
    launch_hint.add_css_class("metis-settings-hint");
    launch_body.append(&launch_hint);
    content.append(&launch_card);

    // ---- Appearance -------------------------------------------------------
    let (look_card, look_body) = ui::section_with_icon(&tr("Appearance"), "view-app-grid-symbolic");

    let menu_opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 1.0, 0.01);
    menu_opacity.set_value(metis_config::load_bar_config().menu_opacity as f64);
    menu_opacity.set_size_request(200, -1);
    menu_opacity.set_draw_value(true);
    look_body.append(&ui::row_with_icon(
        "display-brightness-symbolic",
        &tr("Panel opacity"),
        &menu_opacity,
    ));
    menu_opacity.connect_value_changed(|s| set_menu_opacity(s.value() as f32));

    let look_hint = gtk::Label::new(Some(&tr(
        "Opacity of the Metis menu panel and its translucent surfaces. Applies within ~1s.",
    )));
    look_hint.set_xalign(0.0);
    look_hint.set_wrap(true);
    look_hint.add_css_class("metis-settings-hint");
    look_body.append(&look_hint);
    content.append(&look_card);

    let _ = suppress;
    scroller.upcast()
}

fn persist(cfg: &MenuConfig) {
    if let Err(err) = metis_config::save_menu_config(cfg) {
        tracing::warn!(%err, "failed to save menu.json");
    }
}

fn persist_and_reload(cfg: &MenuConfig) {
    persist(cfg);
    runtime::send("reload-bar");
}

fn set_menu_opacity(value: f32) {
    let mut cfg = metis_config::load_bar_config();
    let clamped = value.clamp(0.3, 1.0);
    if (cfg.menu_opacity - clamped).abs() < f32::EPSILON {
        return;
    }
    cfg.menu_opacity = clamped;
    if let Err(err) = metis_config::save_bar_config(&cfg) {
        tracing::warn!(%err, "failed to save bar.json menu opacity");
        return;
    }
    runtime::send("reload-bar");
}

/// Compact wireframe thumbnail for a menu layout preset.
fn layout_thumb_button(style: MenuStyle) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("metis-style-button");
    btn.add_css_class("metis-menu-layout-button");
    btn.set_tooltip_text(Some(style.description()));

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_halign(gtk::Align::Center);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 3);
    preview.add_css_class("metis-menu-layout-preview");
    preview.set_size_request(132, 88);

    match style {
        MenuStyle::Default => {
            // rail | list+search | pinned  (search at bottom of list)
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_rail());
            body.append(&thumb_list(true));
            body.append(&thumb_pins());
            preview.append(&body);
        }
        MenuStyle::Whisker => {
            preview.append(&thumb_search_bar());
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_rail());
            body.append(&thumb_list(false));
            preview.append(&body);
        }
        MenuStyle::ArcMenu => {
            preview.append(&thumb_user_bar());
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_rail());
            body.append(&thumb_list(true));
            body.append(&thumb_pins());
            preview.append(&body);
        }
        MenuStyle::Mint => {
            preview.append(&thumb_search_bar());
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_rail());
            body.append(&thumb_list(false));
            body.append(&thumb_pins());
            preview.append(&body);
        }
    }

    vbox.append(&preview);

    let caption = gtk::Label::new(Some(&tr(style.title())));
    caption.add_css_class("metis-style-caption");
    vbox.append(&caption);

    btn.set_child(Some(&vbox));
    btn
}

fn thumb_rail() -> gtk::Box {
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rail.add_css_class("metis-menu-thumb-rail");
    rail.set_size_request(14, -1);
    rail.set_vexpand(true);
    for _ in 0..4 {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("metis-menu-thumb-dot");
        dot.set_size_request(8, 8);
        rail.append(&dot);
    }
    rail
}

fn thumb_list(search_bottom: bool) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.add_css_class("metis-menu-thumb-list");
    col.set_hexpand(true);
    col.set_vexpand(true);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    title.add_css_class("metis-menu-thumb-line");
    title.set_size_request(-1, 6);
    col.append(&title);
    for _ in 0..3 {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("metis-menu-thumb-row");
        row.set_size_request(-1, 8);
        row.set_hexpand(true);
        col.append(&row);
    }
    if search_bottom {
        let search = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search.add_css_class("metis-menu-thumb-search");
        search.set_size_request(-1, 10);
        search.set_hexpand(true);
        col.append(&search);
    }
    col
}

fn thumb_pins() -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.add_css_class("metis-menu-thumb-pins");
    col.set_size_request(36, -1);
    col.set_vexpand(true);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    title.add_css_class("metis-menu-thumb-line");
    title.set_size_request(-1, 6);
    col.append(&title);
    let grid = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for _ in 0..2 {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        for _ in 0..2 {
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            cell.add_css_class("metis-menu-thumb-tile");
            cell.set_size_request(14, 14);
            row.append(&cell);
        }
        grid.append(&row);
    }
    col.append(&grid);
    col
}

fn thumb_search_bar() -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("metis-menu-thumb-search");
    bar.set_size_request(-1, 10);
    bar.set_hexpand(true);
    bar
}

fn thumb_user_bar() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("metis-menu-thumb-user");
    row.set_hexpand(true);
    let avatar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    avatar.add_css_class("metis-menu-thumb-avatar");
    avatar.set_size_request(12, 12);
    row.append(&avatar);
    let name = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    name.add_css_class("metis-menu-thumb-line");
    name.set_hexpand(true);
    name.set_size_request(-1, 8);
    row.append(&name);
    row
}
