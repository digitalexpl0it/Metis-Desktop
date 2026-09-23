//! Per-app Metis titlebar overrides — Auto / Metis titlebar / App titlebar.
//! Persists to `decorations.json` and reloads the compositor live.
//!
//! Search uses a virtualized `ListView` + `FilterListModel` so typing never
//! rebuilds or remeasures hundreds of DropDown rows (that is what stalled the
//! older ListBox filter path).

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use gio::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use metis_config::DecorationsOverride;

use crate::apps::{self, AppEntry};
use crate::{runtime, ui};
use metis_i18n::tr;

#[derive(Clone)]
struct AppMeta {
    entry: AppEntry,
    search_text: String,
    candidates: Vec<String>,
    mode: Option<DecorationsOverride>,
}

pub fn build() -> gtk::Widget {
    apps::watch_app_index();

    let (scroller, content) = ui::page_for("titlebars");
    // Header/search stay fixed; only the applications ListView scrolls — avoids
    // the double scrollbar from nesting page + list ScrolledWindows.
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    content.set_vexpand(true);
    content.set_margin_bottom(16);

    let hint = gtk::Label::new(Some(&tr(
        "Auto uses Metis defaults. Override only when an app shows a double \
         titlebar or is missing Metis window controls. Changes apply immediately.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search applications…")));
    search.set_hexpand(true);
    content.append(&search);

    {
        let search_key = search.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::BackSpace && search_key.text().is_empty() {
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        search.add_controller(key);
    }

    let (list_card, list_outer) =
        ui::section_with_icon(&tr("Applications"), "application-x-executable-symbolic");
    list_card.set_vexpand(true);
    list_outer.set_vexpand(true);

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let filter = gtk::CustomFilter::new({
        let query = query.clone();
        move |obj| {
            let Some(boxed) = obj.downcast_ref::<glib::BoxedAnyObject>() else {
                return false;
            };
            let meta: Ref<AppMeta> = boxed.borrow();
            let q = query.borrow();
            q.is_empty() || meta.search_text.contains(q.as_str())
        }
    });
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
    let selection = gtk::NoSelection::new(Some(filtered.clone()));

    let bind_block = Rc::new(RefCell::new(false));
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup({
        let bind_block = bind_block.clone();
        move |_, obj| {
            let list_item = obj
                .downcast_ref::<gtk::ListItem>()
                .expect("ListItemFactory setup argument");
            let row = build_row_shell();
            let dd = row_dropdown(&row);
            dd.connect_selected_notify({
                let list_item = list_item.clone();
                let bind_block = bind_block.clone();
                let badge = row_badge(&row);
                move |dd| {
                    if *bind_block.borrow() {
                        return;
                    }
                    let Some(boxed) = list_item.item().and_downcast::<glib::BoxedAnyObject>()
                    else {
                        return;
                    };
                    let mode = index_to_mode(dd.selected());
                    {
                        let mut meta: RefMut<AppMeta> = boxed.borrow_mut();
                        meta.mode = mode;
                        let mut cfg = metis_config::load_decorations_config();
                        cfg.set_for_candidates(&meta.candidates, mode);
                        if let Err(err) = metis_config::save_decorations_config(&cfg) {
                            tracing::warn!(%err, "failed to save decorations.json");
                            return;
                        }
                    }
                    badge.set_visible(mode.is_some());
                    runtime::reload_decorations_async();
                }
            });
            list_item.set_child(Some(&row));
        }
    });
    factory.connect_bind({
        let bind_block = bind_block.clone();
        move |_, obj| {
            let list_item = obj
                .downcast_ref::<gtk::ListItem>()
                .expect("ListItemFactory bind argument");
            let Some(boxed) = list_item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let Some(row) = list_item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let meta: Ref<AppMeta> = boxed.borrow();
            let (image, name, badge, dd) = row_parts(&row);

            if let Some(icon) = &meta.entry.icon {
                image.set_from_gicon(icon);
            } else {
                image.set_icon_name(Some("application-x-executable-symbolic"));
            }
            name.set_label(&meta.entry.name);
            badge.set_visible(meta.mode.is_some());

            *bind_block.borrow_mut() = true;
            dd.set_selected(mode_to_index(meta.mode));
            *bind_block.borrow_mut() = false;

            if list_item.position() % 2 == 0 {
                row.add_css_class("metis-settings-app-row-odd");
            } else {
                row.remove_css_class("metis-settings-app-row-odd");
            }
        }
    });

    let list_view = gtk::ListView::new(Some(selection), Some(factory));
    list_view.add_css_class("metis-settings-app-list");
    list_view.set_single_click_activate(false);
    // Viewport fills remaining page height so ListView virtualizes without a
    // second outer page scrollbar.
    let list_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .overlay_scrolling(false)
        .child(&list_view)
        .build();
    list_scroll.set_kinetic_scrolling(false);
    list_scroll.add_css_class("metis-settings-app-scroll");
    ui::wire_vertical_scroll(&list_scroll);
    list_outer.append(&list_scroll);

    let empty_label = gtk::Label::new(Some(&tr("No matching applications")));
    empty_label.add_css_class("metis-settings-hint");
    empty_label.set_xalign(0.0);
    empty_label.set_visible(false);
    list_outer.append(&empty_label);
    content.append(&list_card);

    let empty_label = Rc::new(empty_label);
    let filtered = Rc::new(filtered);
    let filter = Rc::new(filter);
    let store = Rc::new(store);
    let debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let r#gen: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    prune_redundant_overrides();
    refill_store(&store);
    update_empty_state(&filtered, &empty_label, &query.borrow());

    {
        let store = store.clone();
        let filter = filter.clone();
        let filtered = filtered.clone();
        let empty_label = empty_label.clone();
        let query = query.clone();
        let install_debounce: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        apps::register_refresh(Rc::new(move || {
            let mut slot = install_debounce.borrow_mut();
            if let Some(id) = slot.take() {
                id.remove();
            }
            let store = store.clone();
            let filter = filter.clone();
            let filtered = filtered.clone();
            let empty_label = empty_label.clone();
            let query = query.clone();
            let install_debounce = install_debounce.clone();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(750), move || {
                *install_debounce.borrow_mut() = None;
                refill_store(&store);
                filter.changed(gtk::FilterChange::Different);
                update_empty_state(&filtered, &empty_label, &query.borrow());
                glib::ControlFlow::Break
            });
            *slot = Some(id);
        }));
    }

    search.connect_search_changed({
        let filter = filter.clone();
        let filtered = filtered.clone();
        let empty_label = empty_label.clone();
        let query = query.clone();
        let debounce = debounce.clone();
        let r#gen = r#gen.clone();
        move |entry| {
            let new_q = entry.text().trim().to_ascii_lowercase();
            *r#gen.borrow_mut() += 1;
            let my_gen = *r#gen.borrow();

            let mut slot = debounce.borrow_mut();
            if let Some(id) = slot.take() {
                id.remove();
            }
            let filter = filter.clone();
            let filtered = filtered.clone();
            let empty_label = empty_label.clone();
            let query = query.clone();
            let debounce = debounce.clone();
            let r#gen = r#gen.clone();
            // Tiny debounce coalesces paste/bursts; filter itself is cheap.
            let id = glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
                *debounce.borrow_mut() = None;
                if *r#gen.borrow() != my_gen {
                    return glib::ControlFlow::Break;
                }
                *query.borrow_mut() = new_q.clone();
                filter.changed(gtk::FilterChange::Different);
                update_empty_state(&filtered, &empty_label, &new_q);
                glib::ControlFlow::Break
            });
            *slot = Some(id);
        }
    });

    scroller.upcast()
}

fn update_empty_state(filtered: &gtk::FilterListModel, empty_label: &gtk::Label, query: &str) {
    let none = filtered.n_items() == 0;
    empty_label.set_visible(none && !query.is_empty());
}

fn refill_store(store: &gio::ListStore) {
    store.remove_all();
    for meta in load_metas() {
        store.append(&glib::BoxedAnyObject::new(meta));
    }
}

fn load_metas() -> Vec<AppMeta> {
    let cfg = metis_config::load_decorations_config();
    let mut apps = apps::list_apps();
    let mut built: Vec<AppMeta> = apps
        .drain(..)
        .map(|entry| {
            let candidates = entry.decoration_candidates();
            let mode = cfg.mode_for_candidates(&candidates);
            let search_text = format!("{} {}", entry.name, entry.id).to_lowercase();
            AppMeta {
                entry,
                search_text,
                candidates,
                mode,
            }
        })
        .collect();
    built.sort_by(|a, b| match (a.mode.is_some(), b.mode.is_some()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a
            .entry
            .name
            .to_lowercase()
            .cmp(&b.entry.name.to_lowercase()),
    });
    built
}

/// Drop junk keys that never map to a real window `app_id` (numeric icons,
/// FreeDesktop theme tokens, Wine menu stubs). Do **not** strip real app ids —
/// that used to wipe intentional Text Editor / Settings / GitHub Desktop
/// overrides every time this page opened.
fn prune_redundant_overrides() {
    let mut cfg = metis_config::load_decorations_config();
    let before = cfg.overrides.len();
    cfg.overrides
        .retain(|key, _mode| !is_noise_override_key(key));
    if cfg.overrides.len() != before {
        if let Err(err) = metis_config::save_decorations_config(&cfg) {
            tracing::warn!(%err, "failed to prune decorations.json");
            return;
        }
        runtime::reload_decorations_async();
        tracing::info!(
            before,
            after = cfg.overrides.len(),
            "pruned decorations.json overrides"
        );
    }
}

fn is_noise_override_key(key: &str) -> bool {
    let k = key.trim().to_ascii_lowercase();
    if k.is_empty() || k.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if k.contains('_')
        && k.chars().any(|c| c.is_ascii_hexdigit())
        && k.contains('.')
        && k.split('_').next().is_some_and(|p| p.len() <= 6)
    {
        return true;
    }
    matches!(
        k.as_str(),
        "0" | "application"
            | "env"
            | "flatpak"
            | "desktop"
            | "package-x-generic"
            | "multimedia-volume-control"
            | "printer"
            | "preferences-desktop-locale"
            | "preferences-desktop-theme"
            | "settings"
            | "notepad"
            | "session-properties"
            | "powerstats"
            | "volman"
            | "rhythmbox3"
            | "texteditor"
    ) || k.starts_with("wine-programs-")
        || k.ends_with(".0")
        || k.ends_with(".chm")
}

fn build_row_shell() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_hexpand(true);
    row.add_css_class("metis-settings-row");

    let image = gtk::Image::new();
    image.set_pixel_size(28);
    row.append(&image);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(None);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&name);
    let badge = gtk::Label::new(Some(&tr("Customized")));
    badge.set_xalign(0.0);
    badge.add_css_class("metis-settings-app-badge");
    badge.set_visible(false);
    labels.append(&badge);
    row.append(&labels);

    let dd = {
        let __dd_labels = [tr("Auto"), tr("Metis titlebar"), tr("App titlebar")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    dd.set_valign(gtk::Align::Center);
    dd.set_halign(gtk::Align::End);
    row.append(&dd);
    row
}

fn row_parts(row: &gtk::Box) -> (gtk::Image, gtk::Label, gtk::Label, gtk::DropDown) {
    let image = row
        .first_child()
        .and_downcast::<gtk::Image>()
        .expect("titlebar row icon");
    let labels = image
        .next_sibling()
        .and_downcast::<gtk::Box>()
        .expect("titlebar row labels");
    let name = labels
        .first_child()
        .and_downcast::<gtk::Label>()
        .expect("titlebar row name");
    let badge = name
        .next_sibling()
        .and_downcast::<gtk::Label>()
        .expect("titlebar row badge");
    let dd = labels
        .next_sibling()
        .and_downcast::<gtk::DropDown>()
        .expect("titlebar row dropdown");
    (image, name, badge, dd)
}

fn row_dropdown(row: &gtk::Box) -> gtk::DropDown {
    row_parts(row).3
}

fn row_badge(row: &gtk::Box) -> gtk::Label {
    row_parts(row).2
}

fn mode_to_index(mode: Option<DecorationsOverride>) -> u32 {
    match mode {
        None => 0,
        Some(DecorationsOverride::Server) => 1,
        Some(DecorationsOverride::Client) => 2,
    }
}

fn index_to_mode(idx: u32) -> Option<DecorationsOverride> {
    match idx {
        1 => Some(DecorationsOverride::Server),
        2 => Some(DecorationsOverride::Client),
        _ => None,
    }
}
