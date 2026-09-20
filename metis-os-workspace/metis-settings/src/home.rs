//! Settings Home / overview — category tiles and search results.

use std::rc::Rc;

use gtk::prelude::*;
use metis_i18n::tr;

use crate::nav::{self, Category, CATEGORIES};

/// Build the Home overview. `on_category` opens a category sheet; `on_page` deep-links.
pub fn build(
    on_category: Rc<dyn Fn(&str)>,
    on_page: Rc<dyn Fn(&str)>,
) -> (gtk::Box, gtk::Entry, Rc<dyn Fn(&str)>) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("metis-settings-home");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .overlay_scrolling(false)
        .build();
    scroller.add_css_class("metis-settings-scroller");
    scroller.add_css_class("metis-settings-home-scroll");
    scroller.set_kinetic_scrolling(false);
    crate::ui::wire_vertical_scroll(&scroller);

    let inner = gtk::Box::new(gtk::Orientation::Vertical, 20);
    inner.set_margin_top(28);
    inner.set_margin_bottom(36);
    inner.set_margin_start(36);
    inner.set_margin_end(36);
    inner.set_halign(gtk::Align::Fill);
    inner.add_css_class("metis-settings-home-inner");

    let heading = gtk::Label::new(Some(&tr("Settings")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-home-title");
    inner.append(&heading);

    let sub = gtk::Label::new(Some(&tr("Choose a category, or search for a setting.")));
    sub.set_xalign(0.0);
    sub.add_css_class("metis-settings-home-subtitle");
    sub.set_wrap(true);
    inner.append(&sub);

    let search = gtk::Entry::builder()
        .placeholder_text(tr("Search settings"))
        .hexpand(true)
        .build();
    search.add_css_class("metis-settings-search");
    search.add_css_class("metis-settings-home-search");
    search.set_margin_top(4);
    search.set_margin_bottom(8);
    {
        let search_key = search.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::BackSpace && search_key.text().is_empty() {
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        search.add_controller(key);
    }
    inner.append(&search);

    // Fixed 2-column grid — store buttons so search can reflow into packed slots
    // (hiding in-place leaves holes and looks staggered).
    let tiles = gtk::Grid::new();
    tiles.add_css_class("metis-settings-home-tiles");
    tiles.set_column_spacing(14);
    tiles.set_row_spacing(14);
    tiles.set_column_homogeneous(true);
    tiles.set_hexpand(true);
    tiles.set_halign(gtk::Align::Fill);

    let tile_buttons: Vec<gtk::Button> = CATEGORIES
        .iter()
        .map(|cat| {
            let tile = category_tile(cat, on_category.clone());
            tile.set_hexpand(true);
            tile.set_halign(gtk::Align::Fill);
            tile
        })
        .collect();
    for (i, tile) in tile_buttons.iter().enumerate() {
        tiles.attach(tile, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    inner.append(&tiles);

    let results_label = gtk::Label::new(Some(&tr("Matching settings")));
    results_label.set_xalign(0.0);
    results_label.add_css_class("metis-settings-home-section");
    results_label.set_visible(false);
    inner.append(&results_label);

    let results = gtk::Box::new(gtk::Orientation::Vertical, 6);
    results.add_css_class("metis-settings-home-results");
    results.set_visible(false);
    inner.append(&results);

    scroller.set_child(Some(&inner));
    root.append(&scroller);

    let apply_filter: Rc<dyn Fn(&str)> = {
        let tiles = tiles.clone();
        let tile_buttons = tile_buttons.clone();
        let results = results.clone();
        let results_label = results_label.clone();
        let on_page = on_page.clone();
        Rc::new(move |query: &str| {
            let q = query.trim().to_ascii_lowercase();
            let searching = !q.is_empty();

            // Detach all tiles, then re-attach visible ones packed left-to-right.
            for tile in &tile_buttons {
                tiles.remove(tile);
            }
            let mut slot = 0usize;
            for (i, cat) in CATEGORIES.iter().enumerate() {
                let show = !searching || nav::category_matches(cat, &q);
                tile_buttons[i].set_visible(show);
                if show {
                    tiles.attach(&tile_buttons[i], (slot % 2) as i32, (slot / 2) as i32, 1, 1);
                    slot += 1;
                }
            }
            tiles.set_visible(slot > 0);

            while let Some(child) = results.first_child() {
                results.remove(&child);
            }

            if searching {
                let matches = nav::matching_page_ids(&q);
                results_label.set_visible(!matches.is_empty());
                results.set_visible(!matches.is_empty());
                for page_id in matches {
                    let Some(meta) = nav::meta_for(page_id) else {
                        continue;
                    };
                    let row = result_row(meta, on_page.clone());
                    results.append(&row);
                }
            } else {
                results_label.set_visible(false);
                results.set_visible(false);
            }
        })
    };

    {
        let apply = apply_filter.clone();
        search.connect_changed(move |entry| {
            apply(&entry.text());
        });
    }

    (root, search, apply_filter)
}

fn category_tile(cat: &Category, on_category: Rc<dyn Fn(&str)>) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-settings-home-tile");
    btn.set_hexpand(true);
    btn.set_halign(gtk::Align::Fill);
    // Avoid focus/active chrome flashing over the tile before the stack slides.
    btn.set_focus_on_click(false);
    btn.set_can_focus(false);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.set_margin_top(16);
    row.set_margin_bottom(16);
    row.set_margin_start(16);
    row.set_margin_end(16);

    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("metis-settings-home-tile-badge");
    badge.add_css_class(cat.hue.css_class());
    badge.set_valign(gtk::Align::Start);
    let img = gtk::Image::from_icon_name(cat.icon);
    img.set_pixel_size(22);
    img.add_css_class("metis-settings-home-tile-icon");
    badge.append(&img);
    row.append(&badge);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&tr(cat.title)));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-home-tile-title");
    let blurb = gtk::Label::new(Some(&tr(cat.blurb)));
    blurb.set_xalign(0.0);
    blurb.set_wrap(true);
    blurb.add_css_class("metis-settings-home-tile-blurb");
    text.append(&title);
    text.append(&blurb);
    row.append(&text);

    btn.set_child(Some(&row));
    let id = cat.id;
    btn.connect_clicked(move |_| {
        // Defer so GTK finishes the press/release paint before we leave Home.
        let on_category = on_category.clone();
        glib::idle_add_local_once(move || on_category(id));
    });
    btn
}

fn result_row(meta: &nav::NavItem, on_page: Rc<dyn Fn(&str)>) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-settings-home-result");
    btn.set_halign(gtk::Align::Fill);
    btn.set_hexpand(true);
    btn.set_focus_on_click(false);
    btn.set_can_focus(false);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(10);
    row.set_margin_bottom(10);
    row.set_margin_start(12);
    row.set_margin_end(12);

    if let Some(icon) = meta.icon {
        let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        badge.add_css_class("metis-settings-nav-icon-wrap");
        if let Some(hue) = meta.hue {
            badge.add_css_class(hue.css_class());
        }
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(16);
        badge.append(&img);
        row.append(&badge);
    }

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let title = gtk::Label::new(Some(&tr(meta.title)));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-home-result-title");
    labels.append(&title);
    if let Some(sub) = meta.subtitle {
        let s = gtk::Label::new(Some(&tr(sub)));
        s.set_xalign(0.0);
        s.set_wrap(true);
        s.add_css_class("metis-settings-home-result-sub");
        labels.append(&s);
    }
    row.append(&labels);
    btn.set_child(Some(&row));

    let id = meta.page_id.unwrap_or("");
    btn.connect_clicked(move |_| {
        if !id.is_empty() {
            on_page(id);
        }
    });
    btn
}
