//! Settings → Input → Shortcuts — searchable read-only keybind guide.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use metis_config::{KeybindAction, KeybindGroup, load_keybinds_config, reserved_system_rows};
use metis_i18n::tr;

use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("shortcuts");

    let search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search shortcuts…"))
        .hexpand(true)
        .build();
    search.add_css_class("metis-settings-search");
    content.append(&search);

    let list_host = gtk::Box::new(gtk::Orientation::Vertical, 16);
    list_host.set_hexpand(true);
    content.append(&list_host);

    let hint = gtk::Label::new(Some(&tr(
        "These are the current desktop shortcuts. To change a binding, open Keyboard.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let open_kb = gtk::Button::with_label(&tr("Change shortcuts in Keyboard"));
    open_kb.add_css_class("suggested-action");
    open_kb.set_halign(gtk::Align::Start);
    open_kb.connect_clicked(|_| {
        crate::nav::request_page("keyboard");
    });
    content.append(&open_kb);

    let rebuild = Rc::new(RefCell::new(None::<Rc<dyn Fn()>>));
    {
        let list_host = list_host.clone();
        let search_for_rebuild = search.clone();
        let do_rebuild: Rc<dyn Fn()> = Rc::new(move || {
            while let Some(child) = list_host.first_child() {
                list_host.remove(&child);
            }
            let query = search_for_rebuild.text().to_ascii_lowercase();
            let cfg = load_keybinds_config();
            for &group in KeybindGroup::all() {
                if group == KeybindGroup::System {
                    continue;
                }
                let group_label = tr(group.label());
                let mut rows: Vec<(String, String)> = KeybindAction::all()
                    .iter()
                    .copied()
                    .filter(|a| a.group() == group)
                    .map(|a| {
                        let label = tr(a.label());
                        let chord = cfg.chord_for(a).display();
                        (label, chord)
                    })
                    .filter(|(label, chord)| {
                        query.is_empty()
                            || label.to_ascii_lowercase().contains(&query)
                            || chord.to_ascii_lowercase().contains(&query)
                            || group_label.to_ascii_lowercase().contains(&query)
                    })
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                let (card, body) = ui::section(&group_label);
                for (label, chord) in rows {
                    body.append(&shortcut_row(&label, &chord));
                }
                list_host.append(&card);
            }

            // Reserved system binds (VT, quit, hardware keys).
            let reserved: Vec<(String, String)> = reserved_system_rows()
                .into_iter()
                .map(|(label, chord)| (tr(&label), chord.display()))
                .filter(|(label, chord)| {
                    query.is_empty()
                        || label.to_ascii_lowercase().contains(&query)
                        || chord.to_ascii_lowercase().contains(&query)
                        || tr("System").to_ascii_lowercase().contains(&query)
                })
                .collect();
            if !reserved.is_empty() {
                let (card, body) = ui::section(&tr("System"));
                for (label, chord) in reserved {
                    body.append(&shortcut_row(&label, &chord));
                }
                list_host.append(&card);
            }

            if list_host.first_child().is_none() {
                let empty = gtk::Label::new(Some(&tr("No shortcuts match your search.")));
                empty.set_xalign(0.0);
                empty.add_css_class("metis-settings-hint");
                list_host.append(&empty);
            }
        });
        *rebuild.borrow_mut() = Some(do_rebuild.clone());
        do_rebuild();

        let rebuild = rebuild.clone();
        search.connect_search_changed(move |_| {
            if let Some(f) = rebuild.borrow().as_ref() {
                f();
            }
        });
    }

    // Refresh chords when the page is shown again (after Keyboard edits).
    {
        let rebuild = rebuild.clone();
        scroller.connect_map(move |_| {
            if let Some(f) = rebuild.borrow().as_ref() {
                f();
            }
        });
    }

    scroller.upcast()
}

fn shortcut_row(label: &str, chord: &str) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    row.set_hexpand(true);

    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_wrap(true);
    row.append(&name);

    let key = gtk::Label::new(Some(chord));
    key.set_xalign(1.0);
    key.add_css_class("metis-keybind-chord");
    row.append(&key);

    row.upcast()
}
