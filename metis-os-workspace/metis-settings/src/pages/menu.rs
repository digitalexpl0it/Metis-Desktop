//! Metis Menu settings: layout style, feature toggles, quick launchers
//! (terminal / file manager), and panel opacity. Layout changes persist to
//! `menu.json` and nudge `reload-bar` so the shell rebuilds the menu live.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{MenuConfig, MenuStyle};

use crate::{runtime, ui};
use metis_i18n::tr;

/// Two rows of four thumbnails — matches the old scroller viewport without
/// overflowing the layout card.
const LAYOUT_PAGE_SIZE: usize = 8;

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

    let (pin_row, pin_sw) = ui::switch_row(&tr("Show pinned apps"));
    pin_sw.set_active(cfg.show_pinned && !cfg.style.hides_pinned());
    pin_sw.set_sensitive(!cfg.style.hides_pinned());
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
                if c.style.hides_pinned() {
                    return;
                }
                if c.show_pinned == active {
                    return;
                }
                c.show_pinned = active;
                persist_and_reload(&c);
            },
        );
    }

    let mut first_btn: Option<gtk::ToggleButton> = None;
    let mut buttons: Vec<gtk::ToggleButton> = Vec::with_capacity(MenuStyle::ALL.len());
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
            let pin_sw = pin_sw.clone();
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
                if style.prefers_user_header() && !c.show_user_header {
                    c.show_user_header = true;
                    suppress.set(true);
                    user_sw.set_active(true);
                    suppress.set(false);
                }
                if style.hides_pinned() {
                    c.show_pinned = false;
                    suppress.set(true);
                    pin_sw.set_active(false);
                    pin_sw.set_sensitive(false);
                    suppress.set(false);
                } else {
                    pin_sw.set_sensitive(true);
                    if !c.show_pinned {
                        c.show_pinned = true;
                        suppress.set(true);
                        pin_sw.set_active(true);
                        suppress.set(false);
                    }
                }
                persist_and_reload(&c);
            });
        }
        buttons.push(btn);
    }

    let selected_idx = MenuStyle::ALL
        .iter()
        .position(|s| *s == cfg.style)
        .unwrap_or(0);
    let page = Rc::new(Cell::new(selected_idx / LAYOUT_PAGE_SIZE));

    let pager = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    pager.add_css_class("metis-menu-layout-pager");
    pager.set_halign(gtk::Align::Center);

    let prev = gtk::Button::from_icon_name("go-previous-symbolic");
    prev.add_css_class("flat");
    prev.set_tooltip_text(Some(&tr("Previous page")));

    let status = gtk::Label::new(None);
    status.add_css_class("metis-settings-hint");
    status.set_width_chars(14);
    status.set_halign(gtk::Align::Center);

    let next = gtk::Button::from_icon_name("go-next-symbolic");
    next.add_css_class("flat");
    next.set_tooltip_text(Some(&tr("Next page")));

    pager.append(&prev);
    pager.append(&status);
    pager.append(&next);

    let show_page = {
        let chooser = chooser.clone();
        let buttons = buttons.clone();
        let page = page.clone();
        let prev = prev.clone();
        let next = next.clone();
        let status = status.clone();
        let pager = pager.clone();
        Rc::new(move || {
            apply_layout_page(
                &chooser,
                &buttons,
                page.get(),
                &prev,
                &next,
                &status,
                &pager,
            );
        })
    };
    show_page();

    {
        let page = page.clone();
        let show_page = show_page.clone();
        prev.connect_clicked(move |_| {
            let p = page.get();
            if p > 0 {
                page.set(p - 1);
                show_page();
            }
        });
    }
    {
        let page = page.clone();
        let show_page = show_page.clone();
        next.connect_clicked(move |_| {
            let p = page.get();
            let pages = layout_page_count(MenuStyle::ALL.len());
            if p + 1 < pages {
                page.set(p + 1);
                show_page();
            }
        });
    }

    layout_body.append(&chooser);
    layout_body.append(&pager);

    let layout_hint = gtk::Label::new(Some(&tr(
        "Pick a layout for the Metis Menu. Some layouts hide the pinned column \
         (Whisker, Bracket, Ledger, Mosaic, Ladder, Plaza, Crest, Chip). Changes \
         apply after the edge bar reloads (~1s).",
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
    feat_body.append(&pin_row);

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

fn layout_page_count(total: usize) -> usize {
    total.div_ceil(LAYOUT_PAGE_SIZE).max(1)
}

fn apply_layout_page(
    chooser: &gtk::FlowBox,
    buttons: &[gtk::ToggleButton],
    page: usize,
    prev: &gtk::Button,
    next: &gtk::Button,
    status: &gtk::Label,
    pager: &gtk::Box,
) {
    let total = buttons.len();
    let pages = layout_page_count(total);
    let page = page.min(pages.saturating_sub(1));
    let start = page * LAYOUT_PAGE_SIZE;
    let end = (start + LAYOUT_PAGE_SIZE).min(total);

    while let Some(child) = chooser.first_child() {
        chooser.remove(&child);
    }
    for btn in &buttons[start..end] {
        chooser.append(btn);
    }

    pager.set_visible(pages > 1);
    prev.set_sensitive(page > 0);
    next.set_sensitive(page + 1 < pages);
    if total == 0 {
        status.set_text("");
    } else {
        status.set_text(&format!("{}–{} of {total}", start + 1, end));
    }
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
        MenuStyle::Mint | MenuStyle::Ramp => {
            preview.append(&thumb_search_bar());
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_rail());
            body.append(&thumb_list(false));
            body.append(&thumb_pins());
            preview.append(&body);
        }
        MenuStyle::Bracket | MenuStyle::Ladder => {
            preview.append(&thumb_search_bar());
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            body.set_hexpand(true);
            body.set_vexpand(true);
            body.append(&thumb_cats());
            body.append(&thumb_list(false));
            preview.append(&body);
        }
        MenuStyle::Ledger => {
            preview.append(&thumb_search_bar());
            preview.append(&thumb_list(false));
        }
        MenuStyle::Mosaic | MenuStyle::Chip => {
            preview.append(&thumb_search_bar());
            preview.append(&thumb_grid(style == MenuStyle::Chip));
        }
        MenuStyle::Plaza => {
            preview.append(&thumb_search_bar());
            preview.append(&thumb_grid(false));
            preview.append(&thumb_footer());
        }
        MenuStyle::Crest => {
            preview.append(&thumb_crest());
            preview.append(&thumb_grid(false));
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

fn thumb_cats() -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.add_css_class("metis-menu-thumb-cats");
    col.set_size_request(28, -1);
    col.set_vexpand(true);
    for _ in 0..4 {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("metis-menu-thumb-row");
        row.set_size_request(-1, 8);
        col.append(&row);
    }
    col
}

fn thumb_grid(dense: bool) -> gtk::Box {
    let grid = gtk::Box::new(gtk::Orientation::Vertical, 2);
    grid.add_css_class("metis-menu-thumb-grid");
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    let cols = if dense { 4 } else { 3 };
    let size = if dense { 10 } else { 14 };
    for _ in 0..2 {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.set_halign(gtk::Align::Center);
        for _ in 0..cols {
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            cell.add_css_class("metis-menu-thumb-tile");
            cell.set_size_request(size, size);
            row.append(&cell);
        }
        grid.append(&row);
    }
    grid
}

fn thumb_footer() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("metis-menu-thumb-footer");
    row.set_hexpand(true);
    let avatar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    avatar.add_css_class("metis-menu-thumb-avatar");
    avatar.set_size_request(10, 10);
    row.append(&avatar);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    line.add_css_class("metis-menu-thumb-line");
    line.set_hexpand(true);
    line.set_size_request(-1, 6);
    row.append(&line);
    for _ in 0..2 {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("metis-menu-thumb-dot");
        dot.set_size_request(8, 8);
        row.append(&dot);
    }
    row
}

fn thumb_crest() -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 3);
    col.set_halign(gtk::Align::Center);
    col.add_css_class("metis-menu-thumb-crest");
    let avatar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    avatar.add_css_class("metis-menu-thumb-avatar");
    avatar.set_size_request(20, 20);
    avatar.set_halign(gtk::Align::Center);
    col.append(&avatar);
    let name = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    name.add_css_class("metis-menu-thumb-line");
    name.set_size_request(48, 6);
    name.set_halign(gtk::Align::Center);
    col.append(&name);
    col
}
