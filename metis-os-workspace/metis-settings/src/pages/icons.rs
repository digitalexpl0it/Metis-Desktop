//! Settings → Desktop → Icons — Freedesktop icon themes, branding, pack install.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use metis_config::{
    CURSOR_SIZE_MAX, CURSOR_SIZE_MIN, DEFAULT_CURSOR_SIZE, IconsConfig, apply_session_cursor_theme,
    discover_cursor_themes, discover_icon_themes, install_icon_theme_archive, load_icons_config,
    remove_user_icon_theme, save_icons_config,
};
use metis_i18n::tr;

use crate::runtime;
use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("icons");

    let cfg = Rc::new(RefCell::new(load_icons_config()));
    let themes = Rc::new(RefCell::new(discover_icon_themes()));

    // ---- Theme picker -------------------------------------------------------
    let (theme_card, theme_body) =
        ui::section_with_icon(&tr("Icon theme"), "preferences-desktop-icons-symbolic");

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("metis-settings-list");
    list.add_css_class("metis-settings-zebra-list");
    list.set_hexpand(true);

    let preview = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    preview.set_halign(gtk::Align::Start);
    preview.add_css_class("metis-settings-icon-preview");
    preview.add_css_class("metis-settings-inset");
    for name in [
        "folder-symbolic",
        "network-wireless-symbolic",
        "audio-volume-high-symbolic",
        "applications-system-symbolic",
        "user-home-symbolic",
    ] {
        let img = gtk::Image::from_icon_name(name);
        img.set_pixel_size(28);
        preview.append(&img);
    }
    theme_body.append(&preview);

    let filter = gtk::SearchEntry::new();
    filter.set_placeholder_text(Some(&tr("Search themes…")));
    filter.set_hexpand(true);
    filter.add_css_class("metis-settings-inset");
    theme_body.append(&filter);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .max_content_height(320)
        .child(&list)
        .build();
    scroll.add_css_class("metis-settings-list-scroll");
    theme_body.append(&scroll);

    theme_body.append(&ui::hint(&tr(
        "Some packs omit status icons Metis uses; missing glyphs fall back to Adwaita. \
         Pointer themes are chosen in the Cursor theme section below.",
    )));

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("metis-settings-hint");
    theme_body.append(&status);

    refill_theme_list(&list, &themes.borrow(), &cfg.borrow(), "");

    {
        let list = list.clone();
        let themes = themes.clone();
        let cfg = cfg.clone();
        filter.connect_search_changed(move |entry| {
            let q = entry.text().to_lowercase();
            refill_theme_list(&list, &themes.borrow(), &cfg.borrow(), &q);
        });
    }

    {
        let themes = themes.clone();
        let cfg = cfg.clone();
        let status = status.clone();
        let preview = preview.clone();
        list.connect_row_activated(move |_, row| {
            let Some(id) = row_theme_id(row) else {
                return;
            };
            if !themes.borrow().iter().any(|t| t.id == id) {
                return;
            }
            let mut c = cfg.borrow().clone();
            c.theme = Some(id.clone());
            if let Err(err) = save_icons_config(&c) {
                status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = c;
            apply_icon_theme_locally(&id);
            metis_config::apply_session_icon_theme(&id);
            runtime::send("reload-icons");
            runtime::send_widgets("reload-icons");
            status.set_text(&format!("{}: {id}", tr("Active theme")));
            refresh_preview_icons(&preview);
        });
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.add_css_class("metis-settings-actions");
    let use_system = gtk::Button::with_label(&tr("Use system default"));
    let install_btn = gtk::Button::with_label(&tr("Install from archive…"));
    install_btn.add_css_class("suggested-action");
    let remove_btn = gtk::Button::with_label(&tr("Remove selected"));
    remove_btn.add_css_class("destructive-action");
    actions.append(&use_system);
    actions.append(&install_btn);
    actions.append(&remove_btn);
    theme_body.append(&actions);

    {
        let cfg = cfg.clone();
        let status = status.clone();
        let list = list.clone();
        let themes = themes.clone();
        use_system.connect_clicked(move |_| {
            let mut c = cfg.borrow().clone();
            c.theme = None;
            if let Err(err) = save_icons_config(&c) {
                status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = c;
            runtime::send("reload-icons");
            runtime::send_widgets("reload-icons");
            status.set_text(&tr("Using system icon theme"));
            refill_theme_list(&list, &themes.borrow(), &cfg.borrow(), "");
        });
    }

    {
        let themes = themes.clone();
        let cfg = cfg.clone();
        let list = list.clone();
        let status = status.clone();
        let scroller = scroller.clone();
        install_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Install icon theme"))
                .modal(true)
                .build();
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&tr("Icon theme archives")));
            filter.add_pattern("*.zip");
            filter.add_pattern("*.tar");
            filter.add_pattern("*.tar.gz");
            filter.add_pattern("*.tgz");
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let themes = themes.clone();
            let cfg = cfg.clone();
            let list = list.clone();
            let status = status.clone();
            let parent = scroller.root().and_downcast::<gtk::Window>();
            dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else {
                    return;
                };
                let Some(path) = file.path() else {
                    status.set_text(&tr("Could not read archive path"));
                    return;
                };
                match install_icon_theme_archive(&path) {
                    Ok(info) => {
                        status.set_text(&format!("{}: {}", tr("Installed"), info.name));
                        *themes.borrow_mut() = discover_icon_themes();
                        let mut c = cfg.borrow().clone();
                        c.theme = Some(info.id.clone());
                        let _ = save_icons_config(&c);
                        *cfg.borrow_mut() = c;
                        apply_icon_theme_locally(&info.id);
                        metis_config::apply_session_icon_theme(&info.id);
                        runtime::send("reload-icons");
                        runtime::send_widgets("reload-icons");
                        refill_theme_list(&list, &themes.borrow(), &cfg.borrow(), "");
                    }
                    Err(err) => status.set_text(&err),
                }
            });
        });
    }

    {
        let themes = themes.clone();
        let cfg = cfg.clone();
        let list = list.clone();
        let status = status.clone();
        remove_btn.connect_clicked(move |_| {
            let Some(row) = list.selected_row() else {
                status.set_text(&tr("Select a user-installed theme to remove"));
                return;
            };
            let Some(id) = row_theme_id(&row) else {
                return;
            };
            let user = themes
                .borrow()
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.user_installed)
                .unwrap_or(false);
            if !user {
                status.set_text(&tr("System themes cannot be removed"));
                return;
            }
            match remove_user_icon_theme(&id) {
                Ok(()) => {
                    status.set_text(&format!("{}: {id}", tr("Removed")));
                    *themes.borrow_mut() = discover_icon_themes();
                    *cfg.borrow_mut() = load_icons_config();
                    runtime::send("reload-icons");
                    runtime::send_widgets("reload-icons");
                    refill_theme_list(&list, &themes.borrow(), &cfg.borrow(), "");
                }
                Err(err) => status.set_text(&err),
            }
        });
    }

    content.append(&theme_card);

    // ---- Cursor theme -------------------------------------------------------
    let (cursor_card, cursor_body) =
        ui::section_with_icon(&tr("Cursor theme"), "input-mouse-symbolic");
    let cursor_themes = Rc::new(RefCell::new(discover_cursor_themes()));
    let cursor_list = gtk::ListBox::new();
    cursor_list.set_selection_mode(gtk::SelectionMode::Single);
    cursor_list.add_css_class("metis-settings-list");
    cursor_list.add_css_class("metis-settings-zebra-list");
    cursor_list.set_hexpand(true);

    let cursor_filter = gtk::SearchEntry::new();
    cursor_filter.set_placeholder_text(Some(&tr("Search cursor themes…")));
    cursor_filter.set_hexpand(true);
    cursor_filter.add_css_class("metis-settings-inset");
    cursor_body.append(&cursor_filter);

    let cursor_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(160)
        .max_content_height(260)
        .child(&cursor_list)
        .build();
    cursor_scroll.add_css_class("metis-settings-list-scroll");
    cursor_body.append(&cursor_scroll);

    let size_adj = gtk::Adjustment::new(
        f64::from(
            cfg.borrow()
                .cursor_size
                .unwrap_or(DEFAULT_CURSOR_SIZE)
                .clamp(CURSOR_SIZE_MIN, CURSOR_SIZE_MAX),
        ),
        f64::from(CURSOR_SIZE_MIN),
        f64::from(CURSOR_SIZE_MAX),
        1.0,
        4.0,
        0.0,
    );
    let size_spin = gtk::SpinButton::new(Some(&size_adj), 1.0, 0);
    size_spin.set_numeric(true);
    size_spin.set_snap_to_ticks(true);
    size_spin.set_width_chars(4);
    cursor_body.append(&ui::row(&tr("Pointer size"), &size_spin));

    cursor_body.append(&ui::hint(&tr(
        "Applies to the Metis pointer and apps that honour XCURSOR / gsettings. \
         Already-running apps may need a restart to pick up a new theme.",
    )));

    let cursor_status = gtk::Label::new(None);
    cursor_status.set_xalign(0.0);
    cursor_status.add_css_class("metis-settings-hint");
    cursor_body.append(&cursor_status);

    refill_cursor_list(&cursor_list, &cursor_themes.borrow(), &cfg.borrow(), "");

    {
        let cursor_list = cursor_list.clone();
        let cursor_themes = cursor_themes.clone();
        let cfg = cfg.clone();
        cursor_filter.connect_search_changed(move |entry| {
            let q = entry.text().to_lowercase();
            refill_cursor_list(&cursor_list, &cursor_themes.borrow(), &cfg.borrow(), &q);
        });
    }

    {
        let cursor_themes = cursor_themes.clone();
        let cfg = cfg.clone();
        let cursor_status = cursor_status.clone();
        let size_spin = size_spin.clone();
        cursor_list.connect_row_activated(move |_, row| {
            let Some(id) = row_theme_id(row) else {
                return;
            };
            if !cursor_themes.borrow().iter().any(|t| t.id == id) {
                return;
            }
            let size = size_spin
                .value()
                .round()
                .clamp(f64::from(CURSOR_SIZE_MIN), f64::from(CURSOR_SIZE_MAX))
                as u32;
            let mut c = cfg.borrow().clone();
            c.cursor_theme = Some(id.clone());
            c.cursor_size = Some(size);
            if let Err(err) = save_icons_config(&c) {
                cursor_status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = c;
            apply_session_cursor_theme(&id, size);
            runtime::send("reload-cursors");
            runtime::reload_cursors_async();
            cursor_status.set_text(&format!("{}: {id} ({size}px)", tr("Active cursor")));
        });
    }

    {
        let cfg = cfg.clone();
        let cursor_status = cursor_status.clone();
        let cursor_list = cursor_list.clone();
        let cursor_themes = cursor_themes.clone();
        size_spin.connect_value_changed(move |spin| {
            let size = spin
                .value()
                .round()
                .clamp(f64::from(CURSOR_SIZE_MIN), f64::from(CURSOR_SIZE_MAX))
                as u32;
            let mut c = cfg.borrow().clone();
            c.cursor_size = Some(size);
            let theme = c.cursor_theme.clone();
            if let Err(err) = save_icons_config(&c) {
                cursor_status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = c;
            if let Some(theme) = theme {
                apply_session_cursor_theme(&theme, size);
                runtime::send("reload-cursors");
                runtime::reload_cursors_async();
                cursor_status.set_text(&format!("{}: {theme} ({size}px)", tr("Active cursor")));
            }
            refill_cursor_list(&cursor_list, &cursor_themes.borrow(), &cfg.borrow(), "");
        });
    }

    let cursor_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    cursor_actions.set_halign(gtk::Align::End);
    cursor_actions.add_css_class("metis-settings-actions");
    let cursor_system = gtk::Button::with_label(&tr("Use system default"));
    cursor_actions.append(&cursor_system);
    cursor_body.append(&cursor_actions);

    {
        let cfg = cfg.clone();
        let cursor_status = cursor_status.clone();
        let cursor_list = cursor_list.clone();
        let cursor_themes = cursor_themes.clone();
        cursor_system.connect_clicked(move |_| {
            let mut c = cfg.borrow().clone();
            c.cursor_theme = None;
            c.cursor_size = None;
            if let Err(err) = save_icons_config(&c) {
                cursor_status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = c;
            runtime::send("reload-cursors");
            runtime::reload_cursors_async();
            cursor_status.set_text(&tr("Using system cursor theme"));
            refill_cursor_list(&cursor_list, &cursor_themes.borrow(), &cfg.borrow(), "");
        });
    }

    content.append(&cursor_card);

    // ---- Branding -----------------------------------------------------------
    let (brand_card, brand_body) =
        ui::section_with_icon(&tr("Metis brand icons"), "start-here-symbolic");
    let launcher_row = path_row(
        &tr("Edge bar launcher"),
        cfg.borrow().brand_launcher.clone(),
        cfg.clone(),
        |c, path| c.brand_launcher = path,
    );
    launcher_row.add_css_class("metis-settings-inset");
    brand_body.append(&launcher_row);
    let settings_row = path_row(
        &tr("Settings app icon"),
        cfg.borrow().brand_settings.clone(),
        cfg.clone(),
        |c, path| c.brand_settings = path,
    );
    settings_row.add_css_class("metis-settings-inset");
    brand_body.append(&settings_row);
    brand_body.append(&ui::hint(&tr(
        "PNG paths under your home directory. Leave empty to use the packaged Metis assets.",
    )));
    content.append(&brand_card);

    // ---- Per-app overrides --------------------------------------------------
    let (apps_card, apps_body) =
        ui::section_with_icon(&tr("Custom app icons"), "application-x-executable-symbolic");
    let overrides_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    overrides_box.add_css_class("metis-settings-inset");
    apps_body.append(&overrides_box);
    rebuild_override_rows(&overrides_box, cfg.clone());

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_row.add_css_class("metis-settings-inset");
    let id_entry = gtk::Entry::new();
    id_entry.set_placeholder_text(Some("firefox.desktop"));
    id_entry.set_hexpand(true);
    let icon_entry = gtk::Entry::new();
    icon_entry.set_placeholder_text(Some(&tr("Icon name or PNG path")));
    icon_entry.set_hexpand(true);
    let add_btn = gtk::Button::with_label(&tr("Add"));
    add_row.append(&id_entry);
    add_row.append(&icon_entry);
    add_row.append(&add_btn);
    apps_body.append(&add_row);
    apps_body.append(&ui::hint(&tr(
        "Overrides apply in the Metis Menu and taskbar before the .desktop Icon= value.",
    )));

    {
        let cfg = cfg.clone();
        let overrides_box = overrides_box.clone();
        let status = status.clone();
        add_btn.connect_clicked(move |_| {
            let id = id_entry.text().trim().to_string();
            let icon = icon_entry.text().trim().to_string();
            if id.is_empty() || icon.is_empty() {
                return;
            }
            let mut c = cfg.borrow().clone();
            c.app_icons.insert(id, icon);
            if let Err(err) = save_icons_config(&c) {
                status.set_text(&format!("{}: {err}", tr("Could not save")));
                return;
            }
            *cfg.borrow_mut() = load_icons_config();
            id_entry.set_text("");
            icon_entry.set_text("");
            rebuild_override_rows(&overrides_box, cfg.clone());
            runtime::send("reload-icons");
            runtime::send_widgets("reload-icons");
        });
    }

    content.append(&apps_card);

    if let Some(theme) = cfg.borrow().theme.as_deref() {
        status.set_text(&format!("{}: {theme}", tr("Active theme")));
        apply_icon_theme_locally(theme);
    }

    scroller.upcast()
}

fn apply_icon_theme_locally(theme: &str) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_icon_theme_name(Some(theme));
    }
}

fn refresh_preview_icons(preview: &gtk::Box) {
    let mut child = preview.first_child();
    while let Some(widget) = child {
        if let Ok(img) = widget.clone().downcast::<gtk::Image>()
            && let Some(name) = img.icon_name()
        {
            img.set_icon_name(Some(name.as_str()));
        }
        child = widget.next_sibling();
    }
}

fn refill_cursor_list(
    list: &gtk::ListBox,
    themes: &[metis_config::IconThemeInfo],
    cfg: &IconsConfig,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let active = cfg.cursor_theme.as_deref();
    let mut zebra = 0usize;
    for theme in themes {
        if !query.is_empty()
            && !theme.name.to_lowercase().contains(query)
            && !theme.id.to_lowercase().contains(query)
        {
            continue;
        }
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&format!("theme:{}", theme.id));
        row.add_css_class("metis-settings-zebra-row");
        if zebra % 2 == 1 {
            row.add_css_class("metis-settings-zebra-row-alt");
        }
        zebra += 1;
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        box_.set_margin_start(4);
        box_.set_margin_end(4);
        box_.set_margin_top(2);
        box_.set_margin_bottom(2);
        let icon = gtk::Image::from_icon_name("input-mouse-symbolic");
        icon.set_pixel_size(16);
        icon.add_css_class("metis-settings-row-icon");
        box_.append(&icon);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&theme.name));
        title.set_xalign(0.0);
        title.add_css_class("metis-settings-row-title");
        let sub = gtk::Label::new(Some(&if theme.user_installed {
            format!("{} · {}", theme.id, tr("User"))
        } else {
            format!("{} · {}", theme.id, tr("System"))
        }));
        sub.set_xalign(0.0);
        sub.add_css_class("metis-settings-hint");
        labels.append(&title);
        labels.append(&sub);
        box_.append(&labels);
        if active == Some(theme.id.as_str()) {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_halign(gtk::Align::End);
            check.set_hexpand(true);
            box_.append(&check);
            row.add_css_class("metis-settings-selected");
        }
        row.set_child(Some(&box_));
        list.append(&row);
        if active == Some(theme.id.as_str()) {
            list.select_row(Some(&row));
        }
    }
}

fn refill_theme_list(
    list: &gtk::ListBox,
    themes: &[metis_config::IconThemeInfo],
    cfg: &IconsConfig,
    query: &str,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let active = cfg.theme.as_deref();
    let mut zebra = 0usize;
    for theme in themes {
        if !query.is_empty()
            && !theme.name.to_lowercase().contains(query)
            && !theme.id.to_lowercase().contains(query)
        {
            continue;
        }
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&format!("theme:{}", theme.id));
        row.add_css_class("metis-settings-zebra-row");
        if zebra % 2 == 1 {
            row.add_css_class("metis-settings-zebra-row-alt");
        }
        zebra += 1;
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        box_.set_margin_start(4);
        box_.set_margin_end(4);
        box_.set_margin_top(2);
        box_.set_margin_bottom(2);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(&theme.name));
        title.set_xalign(0.0);
        title.add_css_class("metis-settings-row-title");
        let sub = gtk::Label::new(Some(&if theme.user_installed {
            format!("{} · {}", theme.id, tr("User"))
        } else {
            format!("{} · {}", theme.id, tr("System"))
        }));
        sub.set_xalign(0.0);
        sub.add_css_class("metis-settings-hint");
        labels.append(&title);
        labels.append(&sub);
        box_.append(&labels);
        if active == Some(theme.id.as_str()) {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_halign(gtk::Align::End);
            check.set_hexpand(true);
            box_.append(&check);
            row.add_css_class("metis-settings-selected");
        }
        row.set_child(Some(&box_));
        list.append(&row);
        if active == Some(theme.id.as_str()) {
            list.select_row(Some(&row));
        }
    }
}

fn row_theme_id(row: &gtk::ListBoxRow) -> Option<String> {
    let name = row.widget_name();
    name.strip_prefix("theme:").map(str::to_string)
}

fn path_row(
    label: &str,
    initial: Option<String>,
    cfg: Rc<RefCell<IconsConfig>>,
    set: impl Fn(&mut IconsConfig, Option<String>) + 'static,
) -> gtk::Widget {
    let set = Rc::new(set);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lab = gtk::Label::new(Some(label));
    lab.set_xalign(0.0);
    lab.set_hexpand(true);
    let entry = gtk::Entry::new();
    if let Some(path) = initial {
        entry.set_text(&path);
    }
    entry.set_hexpand(true);
    entry.set_width_chars(28);
    let browse = gtk::Button::with_label(&tr("Browse…"));
    let clear = gtk::Button::with_label(&tr("Clear"));
    row.append(&lab);
    row.append(&entry);
    row.append(&browse);
    row.append(&clear);

    {
        let cfg = cfg.clone();
        let entry = entry.clone();
        let set = set.clone();
        entry.connect_activate(move |e| {
            let text = e.text().trim().to_string();
            let mut c = cfg.borrow().clone();
            set(&mut c, if text.is_empty() { None } else { Some(text) });
            if save_icons_config(&c).is_ok() {
                *cfg.borrow_mut() = load_icons_config();
                runtime::send("reload-icons");
            }
        });
    }
    {
        let cfg = cfg.clone();
        let entry = entry.clone();
        let set = set.clone();
        browse.connect_clicked(move |btn| {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Choose PNG icon"))
                .modal(true)
                .build();
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("PNG"));
            filter.add_pattern("*.png");
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let parent = btn.root().and_downcast::<gtk::Window>();
            let cfg = cfg.clone();
            let entry = entry.clone();
            let set = set.clone();
            dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else {
                    return;
                };
                let Some(path) = file.path() else {
                    return;
                };
                let path_s = path.to_string_lossy().into_owned();
                entry.set_text(&path_s);
                let mut c = cfg.borrow().clone();
                set(&mut c, Some(path_s));
                if save_icons_config(&c).is_ok() {
                    *cfg.borrow_mut() = load_icons_config();
                    runtime::send("reload-icons");
                }
            });
        });
    }
    {
        let cfg = cfg.clone();
        clear.connect_clicked(move |_| {
            entry.set_text("");
            let mut c = cfg.borrow().clone();
            set(&mut c, None);
            if save_icons_config(&c).is_ok() {
                *cfg.borrow_mut() = load_icons_config();
                runtime::send("reload-icons");
            }
        });
    }

    row.upcast()
}

fn rebuild_override_rows(host: &gtk::Box, cfg: Rc<RefCell<IconsConfig>>) {
    while let Some(child) = host.first_child() {
        host.remove(&child);
    }
    let snapshot = cfg.borrow().app_icons.clone();
    for (id, icon) in snapshot {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let lab = gtk::Label::new(Some(&format!("{id} → {icon}")));
        lab.set_xalign(0.0);
        lab.set_hexpand(true);
        lab.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let rm = gtk::Button::with_label(&tr("Remove"));
        rm.add_css_class("destructive-action");
        let cfg_rm = cfg.clone();
        let host_rm = host.clone();
        let id_rm = id.clone();
        rm.connect_clicked(move |_| {
            let mut c = cfg_rm.borrow().clone();
            c.app_icons.remove(&id_rm);
            if save_icons_config(&c).is_ok() {
                *cfg_rm.borrow_mut() = load_icons_config();
                rebuild_override_rows(&host_rm, cfg_rm.clone());
                runtime::send("reload-icons");
            }
        });
        row.append(&lab);
        row.append(&rm);
        host.append(&row);
    }
}
