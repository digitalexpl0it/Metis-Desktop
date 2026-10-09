//! Gamescope profiles and Steam library path lists.

use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    GameScopeProfile, load_gaming_config, sanitize_gamescope_args, save_gaming_config,
};
use metis_i18n::tr;

use crate::ui;

use super::Sections;

pub(crate) fn refresh_gamescope_profiles_list(sections: &Rc<Sections>) {
    while let Some(child) = sections.gamescope_profiles_list.first_child() {
        sections.gamescope_profiles_list.remove(&child);
    }
    let cfg = load_gaming_config();
    if cfg.gamescope_profiles.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No per-app Gamescope profiles configured.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        sections.gamescope_profiles_list.append(&empty);
        return;
    }
    for profile in cfg.gamescope_profiles {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("metis-settings-row");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Label::new(Some(&tr(&format!("Steam app {}", profile.steam_app_id))));
        title.set_xalign(0.0);
        let args_label = if profile.args.is_empty() {
            tr("gamescope -- (no extra flags)")
        } else {
            tr(&format!("gamescope {} --", profile.args))
        };
        let detail = gtk::Label::new(Some(&args_label));
        detail.set_xalign(0.0);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.add_css_class("metis-settings-hint");
        text.append(&title);
        text.append(&detail);
        row.append(&text);

        let edit = gtk::Button::with_label(&tr("Edit"));
        edit.add_css_class("metis-settings-secondary");
        let profile_edit = profile.clone();
        let sections_edit = Rc::clone(sections);
        edit.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            show_gamescope_profile_dialog(
                &parent,
                Some(profile_edit.clone()),
                sections_edit.clone(),
            );
        });
        row.append(&edit);

        let remove = gtk::Button::with_label(&tr("Remove"));
        remove.add_css_class("metis-settings-secondary");
        let app_id = profile.steam_app_id;
        let sections_rm = Rc::clone(sections);
        remove.connect_clicked(move |_| {
            let mut cfg = load_gaming_config();
            cfg.gamescope_profiles.retain(|p| p.steam_app_id != app_id);
            if save_gaming_config(&cfg).is_ok() {
                crate::runtime::reload_gaming_async();
                refresh_gamescope_profiles_list(&sections_rm);
            }
        });
        row.append(&remove);
        sections.gamescope_profiles_list.append(&row);
    }
}

/// Add or replace a Gamescope profile. `existing` is `Some` when editing.
pub(crate) fn show_gamescope_profile_dialog(
    parent: &gtk::Window,
    existing: Option<GameScopeProfile>,
    sections: Rc<Sections>,
) {
    let editing = existing.is_some();
    let title = if editing {
        tr("Edit Gamescope profile")
    } else {
        tr("Add Gamescope profile")
    };
    let dialog = gtk::Window::builder()
        .title(&title)
        .modal(true)
        .transient_for(parent)
        .resizable(false)
        .default_width(440)
        .build();
    dialog.add_css_class("metis-settings-window");

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    root.append(&heading);

    let app_id_entry = gtk::Entry::new();
    app_id_entry.set_placeholder_text(Some("570"));
    app_id_entry.set_input_purpose(gtk::InputPurpose::Digits);
    if let Some(ref p) = existing {
        app_id_entry.set_text(&p.steam_app_id.to_string());
    }
    root.append(&ui::row(&tr("Steam app id"), &app_id_entry));

    let args_entry = gtk::Entry::new();
    args_entry.set_placeholder_text(Some("-W 1920 -H 1080 -f"));
    args_entry.set_hexpand(true);
    if let Some(ref p) = existing {
        args_entry.set_text(&p.args);
    }
    root.append(&ui::row(&tr("Gamescope flags"), &args_entry));

    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.add_css_class("metis-settings-hint");
    error.set_visible(false);
    root.append(&error);

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("metis-settings-secondary");
    let save = gtk::Button::with_label(&tr("Save"));
    save.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&save);
    root.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&root)));

    let original_id = existing.as_ref().map(|p| p.steam_app_id);

    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.close()
    });
    save.connect_clicked({
        let dialog = dialog.clone();
        let app_id_entry = app_id_entry.clone();
        let args_entry = args_entry.clone();
        let error = error.clone();
        let sections = sections.clone();
        move |_| {
            let id_text = app_id_entry.text();
            let Ok(app_id) = id_text.trim().parse::<u32>() else {
                error.set_text(&tr("Enter a valid Steam app id (positive number)."));
                error.set_visible(true);
                return;
            };
            if app_id == 0 {
                error.set_text(&tr("Steam app id must be greater than zero."));
                error.set_visible(true);
                return;
            }
            let Some(args) = sanitize_gamescope_args(args_entry.text().as_str()) else {
                error.set_text(&tr(
                    "Flags look unsafe — use plain gamescope options only (no quotes or shell).",
                ));
                error.set_visible(true);
                return;
            };

            let mut cfg = load_gaming_config();
            if let Some(old) = original_id {
                cfg.gamescope_profiles.retain(|p| p.steam_app_id != old);
            }
            cfg.gamescope_profiles.retain(|p| p.steam_app_id != app_id);
            cfg.gamescope_profiles.push(GameScopeProfile {
                steam_app_id: app_id,
                args,
            });
            match save_gaming_config(&cfg) {
                Ok(()) => {
                    crate::runtime::reload_gaming_async();
                    refresh_gamescope_profiles_list(&sections);
                    dialog.close();
                }
                Err(err) => {
                    error.set_text(&tr(&format!("Could not save: {err}")));
                    error.set_visible(true);
                }
            }
        }
    });

    dialog.present();
}

pub(crate) fn refresh_steam_paths_list(sections: &Rc<Sections>) {
    while let Some(child) = sections.steam_paths_list.first_child() {
        sections.steam_paths_list.remove(&child);
    }
    let cfg = load_gaming_config();
    if cfg.extra_steam_paths.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No extra library paths configured.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        sections.steam_paths_list.append(&empty);
        return;
    }
    for path in cfg.extra_steam_paths {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("metis-settings-row");
        let label = gtk::Label::new(Some(&path));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        row.append(&label);
        let remove = gtk::Button::with_label(&tr("Remove"));
        remove.add_css_class("metis-settings-secondary");
        let path_rm = path.clone();
        let sections_rm = Rc::clone(sections);
        remove.connect_clicked(move |_| {
            let mut cfg = load_gaming_config();
            cfg.extra_steam_paths.retain(|p| p != &path_rm);
            if save_gaming_config(&cfg).is_ok() {
                crate::runtime::reload_gaming_async();
                refresh_steam_paths_list(&sections_rm);
                sections_rm.status_text.set_text(&tr(
                    "Path removed — run Optimize Flatpak Steam to update overrides.",
                ));
            }
        });
        row.append(&remove);
        sections.steam_paths_list.append(&row);
    }
}
