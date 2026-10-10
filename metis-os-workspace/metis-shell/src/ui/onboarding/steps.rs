//! Onboarding wizard step bodies.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use metis_config::{BarDisplays, ThemeMode, WeatherLocation};

use super::helpers::{
    BODY_HEIGHT, BODY_INNER_WIDTH, GeoResult, WALL_H, WALL_W, apply_theme, apply_wallpaper,
    bar_position_index, clear_list_box, current_wallpaper_path, geocode_search,
    index_to_bar_position, keybind_row, labeled_row, load_logo, save_weather, step_shell,
    theme_preview_button, update_bar, wrapping_check,
};
use super::optional::build_optional_software;

pub(crate) fn widget_for_step(step: usize) -> gtk::Widget {
    match step {
        0 => build_language(),
        1 => build_welcome(),
        2 => build_theme(),
        3 => build_wallpaper(),
        4 => build_clock(),
        5 => build_edge_bar(),
        6 => build_network(),
        7 => build_weather(),
        8 => build_desktop_widgets(),
        9 => build_gaming(),
        10 => build_optional_software(),
        11 => build_finish(),
        _ => gtk::Label::new(Some("")).upcast(),
    }
}

fn build_language() -> gtk::Widget {
    use metis_config::save_locale_config;
    use metis_i18n::tr;

    let col = step_shell();
    let intro = gtk::Label::new(Some(&tr(
        "Choose the language used by Metis. You can change this later in Settings.",
    )));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("metis-onboarding-copy");
    col.append(&intro);

    let choices = metis_i18n::known_language_choices();
    let labels: Vec<String> = choices
        .iter()
        .map(|(tag, name)| {
            if tag.is_empty() {
                tr(name)
            } else {
                name.clone()
            }
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    let cfg = metis_config::load_locale_config();
    let selected = match cfg.locale.as_deref() {
        None | Some("") => 0usize,
        Some(current) => choices
            .iter()
            .position(|(tag, _)| {
                !tag.is_empty()
                    && (tag == current
                        || current.starts_with(&format!("{tag}_"))
                        || tag.as_str() == current.split(['_', '-']).next().unwrap_or(""))
            })
            .unwrap_or(0),
    };
    dd.set_selected(selected as u32);
    dd.set_margin_top(12);
    col.append(&dd);

    {
        let choices = choices.clone();
        let labels_keep = labels;
        dd.connect_selected_notify(move |dd| {
            let _ = &labels_keep;
            let idx = dd.selected() as usize;
            let locale = choices.get(idx).and_then(|(tag, _)| {
                if tag.is_empty() {
                    None
                } else {
                    Some(tag.clone())
                }
            });
            let mut cfg = metis_config::load_locale_config();
            if cfg.locale == locale {
                return;
            }
            cfg.locale = locale;
            let _ = save_locale_config(&cfg);
            metis_i18n::reload();
            // Update chrome only — do not rebuild this step (would re-enter notify).
            super::refresh_chrome_after_locale_change();
        });
    }

    col.upcast()
}

fn build_welcome() -> gtk::Widget {
    let col = step_shell();
    col.set_halign(gtk::Align::Center);

    let logo = gtk::Image::new();
    logo.set_pixel_size(160);
    if let Some(texture) = load_logo() {
        logo.set_paintable(Some(&texture));
    }
    logo.set_halign(gtk::Align::Center);
    col.append(&logo);

    let text = gtk::Label::new(Some(&metis_i18n::tr(
        "Metis is a fast, modern desktop built on Wayland.\n\
         This quick setup will personalize your workspace — you can change\n\
         everything later in Settings.",
    )));
    text.add_css_class("metis-onboarding-subtitle");
    text.set_halign(gtk::Align::Center);
    text.set_justify(gtk::Justification::Center);
    col.append(&text);

    col.upcast()
}

fn build_theme() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Pick light or dark — your desktop updates live behind this card.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_width_request(BODY_INNER_WIDTH);
    hint.set_max_width_chars(42);
    col.append(&hint);

    let wp = current_wallpaper_path();
    let chooser = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    chooser.set_halign(gtk::Align::Center);
    chooser.set_hexpand(false);
    chooser.set_margin_top(8);

    let light_label = metis_i18n::tr("Light");
    let dark_label = metis_i18n::tr("Dark");
    let light_btn = theme_preview_button(&light_label, false, wp.as_deref());
    let dark_btn = theme_preview_button(&dark_label, true, wp.as_deref());
    dark_btn.set_group(Some(&light_btn));

    let mode = crate::config::load_theme_preference().unwrap_or(ThemeMode::Light);
    match mode {
        ThemeMode::Light => light_btn.set_active(true),
        _ => dark_btn.set_active(true),
    }

    chooser.append(&light_btn);
    chooser.append(&dark_btn);
    col.append(&chooser);

    light_btn.connect_toggled(move |b| {
        if b.is_active() {
            apply_theme(ThemeMode::Light);
        }
    });
    dark_btn.connect_toggled(move |b| {
        if b.is_active() {
            apply_theme(ThemeMode::Dark);
        }
    });

    col.upcast()
}

fn build_wallpaper() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Choose a bundled background — applied instantly.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    col.append(&hint);

    let wallpapers = metis_config::list_bundled_wallpapers();
    if wallpapers.is_empty() {
        let empty = gtk::Label::new(Some(&metis_i18n::tr(
            "No bundled wallpapers found. Reinstall Metis (wallpapers ship under \
             /usr/share/metis/wallpapers), or add images in Settings → Appearance.",
        )));
        empty.add_css_class("metis-onboarding-hint");
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        empty.set_margin_top(12);
        col.append(&empty);
        return col.upcast();
    }

    let grid = gtk::Grid::new();
    grid.set_column_spacing(10);
    grid.set_row_spacing(10);
    grid.set_halign(gtk::Align::Center);
    grid.set_hexpand(false);
    grid.set_width_request(WALL_W * 2 + 10);
    grid.add_css_class("metis-onboarding-wall-grid");

    let current = current_wallpaper_path();

    for (i, path) in wallpapers.into_iter().enumerate() {
        let btn = gtk::Button::new();
        btn.add_css_class("flat");
        btn.add_css_class("metis-onboarding-wall-pick");
        if current.as_ref() == Some(&path) {
            btn.add_css_class("selected");
        }

        let img = gtk::Image::new();
        img.add_css_class("metis-onboarding-wall-img");
        img.set_halign(gtk::Align::Center);
        img.set_valign(gtk::Align::Center);
        if let Ok(texture) = gdk::Texture::from_filename(&path) {
            img.set_paintable(Some(&texture));
        }
        img.set_pixel_size(WALL_H);
        btn.set_child(Some(&img));
        btn.set_size_request(WALL_W, WALL_H);

        let path_str = path.to_string_lossy().into_owned();
        let grid_ref = grid.clone();
        btn.connect_clicked(move |b| {
            apply_wallpaper(&path_str);
            let mut child = grid_ref.first_child();
            while let Some(c) = child {
                let next = c.next_sibling();
                if let Ok(pick) = c.downcast::<gtk::Button>() {
                    pick.remove_css_class("selected");
                }
                child = next;
            }
            b.add_css_class("selected");
        });

        let col_idx = (i % 2) as i32;
        let row_idx = (i / 2) as i32;
        grid.attach(&btn, col_idx, row_idx, 1, 1);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(BODY_HEIGHT - 40)
        .max_content_height(BODY_HEIGHT - 40)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&grid)
        .build();
    scroll.set_propagate_natural_height(false);
    scroll.set_width_request(BODY_INNER_WIDTH);
    scroll.set_hexpand(false);
    scroll.set_vexpand(false);
    scroll.set_size_request(BODY_INNER_WIDTH, BODY_HEIGHT - 40);
    scroll.set_overflow(gtk::Overflow::Hidden);
    col.append(&scroll);

    col.upcast()
}

fn build_clock() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "How should the edge-bar clock display time?",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    col.append(&hint);

    let bar = crate::config::load_bar_config();
    let is_24h = bar.clock.time_format == "%H:%M";

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    row.set_halign(gtk::Align::Center);
    row.set_margin_top(8);

    let btn_12 = gtk::ToggleButton::with_label(&metis_i18n::tr("12-hour (3:45 PM)"));
    let btn_24 = gtk::ToggleButton::with_label(&metis_i18n::tr("24-hour (15:45)"));
    btn_24.set_group(Some(&btn_12));

    if is_24h {
        btn_24.set_active(true);
    } else {
        btn_12.set_active(true);
    }

    row.append(&btn_12);
    row.append(&btn_24);
    col.append(&row);

    btn_12.connect_toggled(move |b| {
        if b.is_active() {
            update_bar(|c| c.clock.time_format = "%I:%M %p".into());
        }
    });
    btn_24.connect_toggled(move |b| {
        if b.is_active() {
            update_bar(|c| c.clock.time_format = "%H:%M".into());
        }
    });

    col.upcast()
}

fn build_edge_bar() -> gtk::Widget {
    let col = step_shell();
    let bar = crate::config::load_bar_config();

    let position_labels = [
        metis_i18n::tr("Top"),
        metis_i18n::tr("Bottom"),
        metis_i18n::tr("Left"),
        metis_i18n::tr("Right"),
    ];
    let position_refs: Vec<&str> = position_labels.iter().map(|s| s.as_str()).collect();
    let position_dd = gtk::DropDown::from_strings(&position_refs);
    position_dd.set_selected(bar_position_index(bar.position));
    col.append(&labeled_row(&metis_i18n::tr("Position"), &position_dd));

    let display_labels = [
        metis_i18n::tr("All displays"),
        metis_i18n::tr("Primary display only"),
    ];
    let display_refs: Vec<&str> = display_labels.iter().map(|s| s.as_str()).collect();
    let displays_dd = gtk::DropDown::from_strings(&display_refs);
    displays_dd.set_selected(match bar.displays {
        BarDisplays::Primary => 1,
        _ => 0,
    });
    col.append(&labeled_row(&metis_i18n::tr("Show bar on"), &displays_dd));

    let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 1.0, 0.01);
    opacity.set_value(bar.opacity as f64);
    opacity.set_size_request(220, -1);
    opacity.set_draw_value(true);
    col.append(&labeled_row(&metis_i18n::tr("Opacity"), &opacity));

    let blur = gtk::Switch::new();
    blur.set_active(bar.blur);
    blur.set_halign(gtk::Align::End);
    col.append(&labeled_row(&metis_i18n::tr("Backdrop blur"), &blur));

    position_dd.connect_selected_notify(move |dd| {
        let pos = index_to_bar_position(dd.selected());
        update_bar(|c| c.position = pos);
    });
    displays_dd.connect_selected_notify(move |dd| {
        let displays = if dd.selected() == 1 {
            BarDisplays::Primary
        } else {
            BarDisplays::All
        };
        update_bar(|c| c.displays = displays);
    });
    opacity.connect_value_changed(move |s| {
        let v = s.value() as f32;
        update_bar(|c| c.opacity = v);
    });
    blur.connect_active_notify(move |s| {
        update_bar(|c| c.blur = s.is_active());
    });

    col.upcast()
}

fn build_network() -> gtk::Widget {
    use crate::services::WifiNetwork;

    // Fixed-height overlay host so revealing the password sheet does not grow
    // the onboarding card (same BODY_HEIGHT budget as other steps).
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(BODY_INNER_WIDTH, BODY_HEIGHT);
    overlay.set_hexpand(false);
    overlay.set_vexpand(false);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.set_size_request(BODY_INNER_WIDTH, BODY_HEIGHT);
    col.set_hexpand(false);
    col.set_vexpand(false);
    col.set_overflow(gtk::Overflow::Hidden);

    let status = gtk::Label::new(None);
    status.add_css_class("metis-onboarding-subtitle");
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    status.set_max_width_chars(42);
    status.set_width_request(BODY_INNER_WIDTH);
    col.append(&status);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("metis-onboarding-wifi-list");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();
    scroll.set_size_request(BODY_INNER_WIDTH, 180);
    scroll.set_child(Some(&list));
    col.append(&scroll);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let refresh = gtk::Button::with_label(&metis_i18n::tr("Refresh"));
    refresh.add_css_class("flat");
    let skip_hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Already online? Continue — you can change this anytime in Settings.",
    )));
    skip_hint.add_css_class("metis-onboarding-hint");
    skip_hint.set_xalign(0.0);
    skip_hint.set_wrap(true);
    skip_hint.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    skip_hint.set_hexpand(true);
    skip_hint.set_max_width_chars(36);
    actions.append(&refresh);
    actions.append(&skip_hint);
    col.append(&actions);

    overlay.set_child(Some(&col));

    // Top-slide password sheet (overlays the list — does not resize the card).
    let sheet = gtk::Box::new(gtk::Orientation::Vertical, 8);
    sheet.add_css_class("metis-onboarding-wifi-sheet");
    sheet.set_margin_start(4);
    sheet.set_margin_end(4);
    sheet.set_margin_top(4);
    let sheet_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let connect_title = gtk::Label::new(None);
    connect_title.set_xalign(0.0);
    connect_title.set_hexpand(true);
    connect_title.add_css_class("metis-onboarding-wifi-sheet-title");
    let cancel_sheet = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
    cancel_sheet.add_css_class("flat");
    sheet_header.append(&connect_title);
    sheet_header.append(&cancel_sheet);
    sheet.append(&sheet_header);
    let password = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .placeholder_text(metis_i18n::tr("Password"))
        .hexpand(true)
        .build();
    sheet.append(&password);
    let connect_btn = gtk::Button::with_label(&metis_i18n::tr("Connect"));
    connect_btn.add_css_class("suggested-action");
    connect_btn.set_halign(gtk::Align::End);
    sheet.append(&connect_btn);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(220)
        .reveal_child(false)
        .child(&sheet)
        .build();
    revealer.set_halign(gtk::Align::Fill);
    revealer.set_valign(gtk::Align::Start);
    revealer.set_hexpand(true);
    overlay.add_overlay(&revealer);

    let selected = Rc::new(RefCell::new(Option::<String>::None));
    let nets = Rc::new(RefCell::new(Vec::<WifiNetwork>::new()));

    let hide_sheet = {
        let revealer = revealer.clone();
        let password = password.clone();
        let selected = selected.clone();
        Rc::new(move || {
            revealer.set_reveal_child(false);
            password.set_text("");
            *selected.borrow_mut() = None;
        })
    };

    let show_sheet = {
        let revealer = revealer.clone();
        let connect_title = connect_title.clone();
        let password = password.clone();
        let selected = selected.clone();
        Rc::new(move |ssid: String| {
            *selected.borrow_mut() = Some(ssid.clone());
            connect_title.set_text(&metis_i18n::tr("Password for %1").replace("%1", &ssid));
            password.set_text("");
            revealer.set_reveal_child(true);
            let password = password.clone();
            glib::timeout_add_local_once(Duration::from_millis(240), move || {
                password.grab_focus();
            });
        })
    };

    {
        let hide_sheet = hide_sheet.clone();
        cancel_sheet.connect_clicked(move |_| hide_sheet());
    }

    let rebuild = {
        let list = list.clone();
        let status = status.clone();
        let nets = nets.clone();
        let show_sheet = show_sheet.clone();
        let hide_sheet = hide_sheet.clone();
        Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let (wifi_present, wifi_on, eth, networks) = crate::services::network_snapshot_for_ui();
            *nets.borrow_mut() = networks.clone();

            let mut lines = Vec::new();
            if eth.present {
                if eth.connected {
                    lines.push(metis_i18n::tr("Wired: connected (%1)").replace("%1", &eth.label));
                } else {
                    lines.push(metis_i18n::tr("Wired: available"));
                }
            }
            if !wifi_present {
                if !eth.present {
                    lines.push(metis_i18n::tr("No Wi-Fi or Ethernet adapter found"));
                }
            } else if !wifi_on {
                lines.push(metis_i18n::tr("Wi-Fi radio is off"));
            } else if networks.is_empty() {
                lines.push(metis_i18n::tr("Scanning for Wi-Fi networks…"));
            } else if let Some(active) = networks.iter().find(|n| n.active) {
                lines.push(metis_i18n::tr("Wi-Fi: connected to %1").replace("%1", &active.ssid));
            } else {
                lines.push(metis_i18n::tr("Select a network to connect"));
            }
            status.set_text(&lines.join("\n"));

            if !wifi_present || !wifi_on {
                return;
            }
            for (i, net) in networks.iter().take(12).enumerate() {
                let row = gtk::Button::builder().has_frame(false).build();
                row.add_css_class("metis-onboarding-wifi-row");
                if i % 2 == 1 {
                    row.add_css_class("metis-onboarding-wifi-row-alt");
                }
                let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let name = gtk::Label::new(Some(&net.ssid));
                name.set_xalign(0.0);
                name.set_hexpand(true);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                h.append(&name);
                if net.secured {
                    let lock = gtk::Label::new(Some(&metis_i18n::tr("secured")));
                    lock.add_css_class("metis-onboarding-hint");
                    h.append(&lock);
                }
                if net.active {
                    let check = gtk::Label::new(Some(&metis_i18n::tr("connected")));
                    check.add_css_class("metis-onboarding-optional-title");
                    h.append(&check);
                }
                row.set_child(Some(&h));
                let ssid = net.ssid.clone();
                let secured = net.secured;
                let active = net.active;
                let show_sheet = show_sheet.clone();
                let hide_sheet = hide_sheet.clone();
                row.connect_clicked(move |_| {
                    if active {
                        return;
                    }
                    if secured {
                        show_sheet(ssid.clone());
                    } else {
                        hide_sheet();
                        crate::services::wifi_connect(ssid.clone(), None);
                    }
                });
                list.append(&row);
            }
        })
    };

    {
        let rebuild = rebuild.clone();
        let hide_sheet = hide_sheet.clone();
        refresh.connect_clicked(move |_| {
            hide_sheet();
            crate::services::wifi_ensure_radio_on();
            crate::services::wifi_scan();
            rebuild();
        });
    }
    {
        let selected = selected.clone();
        let password = password.clone();
        let hide_sheet = hide_sheet.clone();
        let rebuild = rebuild.clone();
        let submit = {
            let selected = selected.clone();
            let password = password.clone();
            let hide_sheet = hide_sheet.clone();
            let rebuild = rebuild.clone();
            Rc::new(move || {
                let Some(ssid) = selected.borrow().clone() else {
                    return;
                };
                let pw = password.text().to_string();
                if pw.is_empty() {
                    return;
                }
                crate::services::wifi_connect(ssid, Some(pw));
                hide_sheet();
                rebuild();
            })
        };
        let submit_btn = submit.clone();
        connect_btn.connect_clicked(move |_| submit_btn());
        password.connect_activate(move |_| submit());
    }

    crate::services::wifi_ensure_radio_on();
    crate::services::wifi_scan();
    rebuild();

    let rebuild_later = rebuild.clone();
    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
        rebuild_later();
    });

    overlay.upcast()
}

fn build_desktop_widgets() -> gtk::Widget {
    use metis_config::{
        DesktopWidgetInstance, DesktopWidgetKind, load_desktop_widgets_config,
        save_desktop_widgets_config,
    };

    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Optional panels on your wallpaper — Folders, Clock, Weather, and more. \
         Off by default so the desktop stays clean.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    hint.set_width_request(BODY_INNER_WIDTH);
    hint.set_max_width_chars(42);
    col.append(&hint);

    let enabled = gtk::Switch::new();
    let cfg = load_desktop_widgets_config();
    enabled.set_active(cfg.enabled);
    enabled.set_halign(gtk::Align::End);
    col.append(&labeled_row(
        &metis_i18n::tr("Show desktop widgets"),
        &enabled,
    ));

    let detail = gtk::Label::new(Some(&metis_i18n::tr(
        "Turning this on places a Clock and Folders widget you can move later \
         (Settings → Desktop widgets → Edit mode).",
    )));
    detail.add_css_class("metis-onboarding-hint");
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    detail.set_width_request(BODY_INNER_WIDTH);
    detail.set_max_width_chars(42);
    col.append(&detail);

    enabled.connect_active_notify(move |sw| {
        let on = sw.is_active();
        let mut cfg = load_desktop_widgets_config();
        cfg.enabled = on;
        if on && cfg.instances.is_empty() {
            let mut clock = DesktopWidgetInstance::new(DesktopWidgetKind::Clock);
            clock.x = 48;
            clock.y = 48;
            let mut folders = DesktopWidgetInstance::new(DesktopWidgetKind::Folders);
            folders.x = 48;
            folders.y = 220;
            cfg.instances = vec![clock, folders];
        }
        if let Err(err) = save_desktop_widgets_config(&cfg) {
            tracing::warn!(%err, "failed to save desktop-widgets.json");
        }
    });

    col.upcast()
}

fn build_weather() -> gtk::Widget {
    let col = step_shell();
    let cfg = Rc::new(RefCell::new(crate::config::load_weather_config()));

    let auto_sw = gtk::Switch::new();
    auto_sw.set_active(cfg.borrow().auto_detect);
    auto_sw.set_halign(gtk::Align::End);
    col.append(&labeled_row(
        &metis_i18n::tr("Detect my location"),
        &auto_sw,
    ));

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Or search for a city to pin a location (overrides auto-detect).",
    )));
    hint.add_css_class("metis-onboarding-hint");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    col.append(&hint);

    let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::builder()
        .placeholder_text(metis_i18n::tr("Search for a city…"))
        .hexpand(true)
        .build();
    let search_btn = gtk::Button::with_label(&metis_i18n::tr("Search"));
    search_row.append(&entry);
    search_row.append(&search_btn);
    col.append(&search_row);

    let results = gtk::ListBox::new();
    results.set_selection_mode(gtk::SelectionMode::None);
    results.set_margin_top(4);
    let results_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(72)
        .max_content_height(72)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&results)
        .build();
    col.append(&results_scroll);

    let (tx, rx) = mpsc::channel::<Vec<GeoResult>>();
    {
        let results = results.clone();
        let cfg = cfg.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            if let Ok(items) = rx.try_recv() {
                clear_list_box(&results);
                if items.is_empty() {
                    let row = gtk::ListBoxRow::new();
                    let lbl = gtk::Label::new(Some(&metis_i18n::tr(
                        "No results — try another spelling.",
                    )));
                    lbl.add_css_class("metis-onboarding-hint");
                    lbl.set_xalign(0.0);
                    row.set_child(Some(&lbl));
                    results.append(&row);
                } else {
                    for item in items {
                        let row = gtk::ListBoxRow::new();
                        let btn = gtk::Button::new();
                        btn.add_css_class("flat");
                        let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
                        let name = gtk::Label::new(Some(&item.name));
                        name.set_xalign(0.0);
                        name.set_halign(gtk::Align::Start);
                        v.append(&name);
                        if !item.detail.is_empty() {
                            let sub = gtk::Label::new(Some(&item.detail));
                            sub.add_css_class("metis-onboarding-hint");
                            sub.set_xalign(0.0);
                            v.append(&sub);
                        }
                        btn.set_child(Some(&v));
                        let cfg = cfg.clone();
                        btn.connect_clicked(move |_| {
                            let mut c = cfg.borrow_mut();
                            c.auto_detect = false;
                            c.locations = vec![WeatherLocation {
                                name: item.name.clone(),
                                latitude: item.lat,
                                longitude: item.lon,
                            }];
                            save_weather(&c);
                        });
                        row.set_child(Some(&btn));
                        results.append(&row);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let cfg = cfg.clone();
        auto_sw.connect_active_notify(move |s| {
            cfg.borrow_mut().auto_detect = s.is_active();
            if s.is_active() {
                cfg.borrow_mut().locations.clear();
            }
            save_weather(&cfg.borrow());
        });
    }

    search_btn.connect_clicked(move |_| {
        let query = entry.text().to_string();
        if query.trim().is_empty() {
            return;
        }
        let tx = tx.clone();
        std::thread::spawn(move || {
            let results = geocode_search(&query);
            let _ = tx.send(results);
        });
    });

    col.upcast()
}

fn build_gaming() -> gtk::Widget {
    let col = step_shell();

    let hybrid = metis_config::detect_hybrid_gpu(None).is_some();
    let steam = match metis_gaming::detect_steam() {
        metis_gaming::SteamInstall::Native => metis_i18n::tr("native Steam"),
        metis_gaming::SteamInstall::Flatpak => metis_i18n::tr("Flatpak Steam"),
        metis_gaming::SteamInstall::None => metis_i18n::tr("not installed"),
    };
    let hybrid_status = if hybrid {
        metis_i18n::tr("detected")
    } else {
        metis_i18n::tr("not detected")
    };
    let gamemode_status = if metis_gaming::gamemode_installed() {
        metis_i18n::tr("installed")
    } else {
        metis_i18n::tr("optional — install gamemode")
    };
    let vulkan_status = if metis_gaming::i386_vulkan_likely_missing()
        || metis_gaming::mesa_vulkan_amd64_missing()
    {
        metis_i18n::tr("needs Fix in Settings → Gaming")
    } else {
        metis_i18n::tr("looks OK")
    };
    let nvidia_status = if metis_gaming::nvidia_gpu_present() {
        if metis_gaming::detect::nvidia_driver_loaded() {
            metis_i18n::tr("driver loaded")
        } else {
            metis_i18n::tr("driver missing — use Settings → Gaming (consent install)")
        }
    } else {
        metis_i18n::tr("not detected")
    };
    let summary = gtk::Label::new(Some(&format!(
        "{}\n{}\n{}\n{}\n{}",
        metis_i18n::tr("Hybrid GPU: %1").replace("%1", &hybrid_status),
        metis_i18n::tr("Steam: %1").replace("%1", &steam),
        metis_i18n::tr("GameMode: %1").replace("%1", &gamemode_status),
        metis_i18n::tr("Vulkan: %1").replace("%1", &vulkan_status),
        metis_i18n::tr("NVIDIA: %1").replace("%1", &nvidia_status),
    )));
    summary.add_css_class("metis-onboarding-subtitle");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    summary.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    summary.set_width_request(BODY_INNER_WIDTH);
    summary.set_max_width_chars(42);
    col.append(&summary);

    let auto_gpu = wrapping_check(
        &metis_i18n::tr("Enable automatic GPU switching for games"),
        super::gaming_auto_gpu(),
    );
    auto_gpu.connect_active_notify(|s| super::set_gaming_auto_gpu(s.is_active()));
    col.append(&auto_gpu);

    let optimize = wrapping_check(
        &metis_i18n::tr("Optimize Flatpak Steam / Lutris / Heroic"),
        super::gaming_optimize(),
    );
    optimize.set_tooltip_text(Some(&metis_i18n::tr(
        "Applies Flatpak overrides: --device=all, network, and Wayland sockets.",
    )));
    optimize.connect_active_notify(|s| super::set_gaming_optimize(s.is_active()));
    col.append(&optimize);

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Games use the discrete GPU automatically — leave Steam Launch Options empty. \
         Install drivers and packages from Settings → Gaming (never silently).",
    )));
    hint.add_css_class("metis-onboarding-hint");
    hint.set_xalign(0.0);
    hint.set_margin_top(8);
    hint.set_wrap(true);
    hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    hint.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    hint.set_width_request(BODY_INNER_WIDTH);
    hint.set_max_width_chars(42);
    col.append(&hint);

    col.upcast()
}

/// CheckButton whose label wraps inside the onboarding card width.
fn build_finish() -> gtk::Widget {
    let col = step_shell();

    let summary = gtk::Label::new(Some(&metis_i18n::tr(
        "Your desktop is ready. Here are a few shortcuts to get started:",
    )));
    summary.add_css_class("metis-onboarding-subtitle");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    col.append(&summary);

    let cfg = metis_config::load_keybinds_config();
    let mod_label = cfg.mod_key.as_str();
    let close = cfg
        .chord_for(metis_config::KeybindAction::CloseWindow)
        .display();
    let layout_free = cfg
        .chord_for(metis_config::KeybindAction::LayoutFree)
        .display();
    let ws1 = cfg
        .chord_for(metis_config::KeybindAction::Workspace1)
        .display();
    let keybinds = [
        (
            metis_i18n::tr("Click the brand icon"),
            metis_i18n::tr("Open the app launcher"),
        ),
        (
            layout_free,
            metis_i18n::tr("Disable tiling / return to free desktop"),
        ),
        (close, metis_i18n::tr("Close the focused window")),
        (
            format!("{mod_label} + 1 … 9"),
            metis_i18n::tr("Switch workspace"),
        ),
    ];
    // Keep a note that defaults use the configured Metis modifier.
    let _ = ws1;
    for (key, desc) in &keybinds {
        col.append(&keybind_row(key, desc));
    }

    let display_hint = gtk::Label::new(Some(&metis_i18n::tr(
        "For monitor arrangement, resolution, and refresh rate, open\n\
         Settings → Display.",
    )));
    display_hint.add_css_class("metis-onboarding-hint");
    display_hint.set_xalign(0.0);
    display_hint.set_margin_top(8);
    display_hint.set_wrap(true);
    col.append(&display_hint);

    col.upcast()
}
