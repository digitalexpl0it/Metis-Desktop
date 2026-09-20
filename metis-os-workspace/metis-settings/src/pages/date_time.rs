//! Settings → System → Date & Time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::FirstDayOfWeek;
use metis_i18n::tr;
use metis_remote::DateTimeStatus;

use crate::{bg, dialog, runtime, ui};

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("date_time");
    let suppress = Rc::new(Cell::new(false));
    let status = Rc::new(RefCell::new(metis_remote::datetime_status().ok()));
    let dt_cfg = metis_config::load_datetime_config();

    let (card, body) = ui::section_with_icon(&tr("Date & Time"), "preferences-system-time-symbolic");

    // Automatic Date & Time (NTP)
    let ntp_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    ntp_box.set_hexpand(true);
    let ntp_title = gtk::Label::new(Some(&tr("Automatic Date & Time")));
    ntp_title.set_xalign(0.0);
    let ntp_sub = gtk::Label::new(Some(&tr("Requires internet access")));
    ntp_sub.set_xalign(0.0);
    ntp_sub.add_css_class("metis-settings-hint");
    ntp_box.append(&ntp_title);
    ntp_box.append(&ntp_sub);
    let ntp_sw = gtk::Switch::new();
    ntp_sw.set_valign(gtk::Align::Center);
    ntp_sw.set_vexpand(false);
    ntp_sw.set_active(status.borrow().as_ref().map(|s| s.ntp).unwrap_or(false));
    let ntp_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    ntp_row.add_css_class("metis-settings-row");
    ntp_row.append(&ntp_box);
    ntp_row.append(&ntp_sw);
    body.append(&ntp_row);

    // Date & Time value row
    let time_value = gtk::Label::new(Some(
        status
            .borrow()
            .as_ref()
            .map(|s| s.local_time.as_str())
            .unwrap_or("—"),
    ));
    time_value.set_xalign(1.0);
    time_value.add_css_class("metis-settings-value");
    let time_chevron = gtk::Image::from_icon_name("go-next-symbolic");
    let time_trail = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    time_trail.append(&time_value);
    time_trail.append(&time_chevron);
    let time_btn = gtk::Button::new();
    time_btn.add_css_class("flat");
    time_btn.add_css_class("metis-settings-nav-row");
    let time_inner = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    time_inner.add_css_class("metis-settings-row");
    let time_lbl = gtk::Label::new(Some(&tr("Date & Time")));
    time_lbl.set_xalign(0.0);
    time_lbl.set_hexpand(true);
    time_inner.append(&time_lbl);
    time_inner.append(&time_trail);
    time_btn.set_child(Some(&time_inner));
    time_btn.set_sensitive(!ntp_sw.is_active());
    body.append(&time_btn);

    // Automatic Time Zone
    let atz_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    atz_box.set_hexpand(true);
    let atz_title = gtk::Label::new(Some(&tr("Automatic Time Zone")));
    atz_title.set_xalign(0.0);
    let atz_sub = gtk::Label::new(Some(&tr(
        "Requires location services enabled and internet access",
    )));
    atz_sub.set_xalign(0.0);
    atz_sub.add_css_class("metis-settings-hint");
    atz_box.append(&atz_title);
    atz_box.append(&atz_sub);
    let atz_sw = gtk::Switch::new();
    atz_sw.set_valign(gtk::Align::Center);
    atz_sw.set_vexpand(false);
    atz_sw.set_active(dt_cfg.auto_timezone);
    let atz_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    atz_row.add_css_class("metis-settings-row");
    atz_row.append(&atz_box);
    atz_row.append(&atz_sw);
    body.append(&atz_row);

    // Time Zone row
    let tz_value = gtk::Label::new(Some(
        status
            .borrow()
            .as_ref()
            .map(|s| s.timezone.as_str())
            .unwrap_or("—"),
    ));
    tz_value.set_xalign(1.0);
    tz_value.add_css_class("metis-settings-value");
    let tz_chevron = gtk::Image::from_icon_name("go-next-symbolic");
    let tz_trail = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    tz_trail.append(&tz_value);
    tz_trail.append(&tz_chevron);
    let tz_btn = gtk::Button::new();
    tz_btn.add_css_class("flat");
    let tz_inner = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    tz_inner.add_css_class("metis-settings-row");
    let tz_lbl = gtk::Label::new(Some(&tr("Time Zone")));
    tz_lbl.set_xalign(0.0);
    tz_lbl.set_hexpand(true);
    tz_inner.append(&tz_lbl);
    tz_inner.append(&tz_trail);
    tz_btn.set_child(Some(&tz_inner));
    tz_btn.set_sensitive(!atz_sw.is_active());
    body.append(&tz_btn);

    // Time Format
    let format_labels = [tr("12-hour AM / PM"), tr("24-hour")];
    let format_refs: Vec<&str> = format_labels.iter().map(|s| s.as_str()).collect();
    let format_dd = gtk::DropDown::from_strings(&format_refs);
    let bar = metis_config::load_bar_config();
    let is_24 = !bar.clock.time_format.contains("%p")
        && (bar.clock.time_format.contains("%H") || bar.clock.time_format.contains("%k"));
    format_dd.set_selected(if is_24 { 1 } else { 0 });
    body.append(&ui::row(&tr("Time Format"), &format_dd));

    // First day of week
    let day_choices = [
        FirstDayOfWeek::LocaleDefault,
        FirstDayOfWeek::Sunday,
        FirstDayOfWeek::Monday,
    ];
    let day_labels: Vec<String> = day_choices.iter().map(|d| tr(d.title())).collect();
    let day_refs: Vec<&str> = day_labels.iter().map(|s| s.as_str()).collect();
    let day_dd = gtk::DropDown::from_strings(&day_refs);
    let day_idx = day_choices
        .iter()
        .position(|d| *d == dt_cfg.first_day_of_week)
        .unwrap_or(0);
    day_dd.set_selected(day_idx as u32);
    body.append(&ui::row(&tr("First Day of the Week"), &day_dd));

    content.append(&card);

    // Wire NTP
    {
        let suppress = suppress.clone();
        let time_btn = time_btn.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        let status = status.clone();
        ui::defer_switch_active_notify_when(
            &ntp_sw,
            {
                let suppress = suppress.clone();
                move || !suppress.get()
            },
            move |active| {
                let time_btn = time_btn.clone();
                let time_value = time_value.clone();
                let tz_value = tz_value.clone();
                let status = status.clone();
                time_btn.set_sensitive(!active);
                bg::run_bg(
            move || {
                let result = metis_remote::set_ntp(active);
                result
            },
            move |result| {
        if let Err(err) = result {
                                    tracing::warn!(%err, "set-ntp failed");
                                }
                                refresh_status(&status, &time_value, &tz_value);
            },
        );
            },
        );
    }

    // Wire auto timezone
    {
        let suppress = suppress.clone();
        let tz_btn = tz_btn.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        let status = status.clone();
        ui::defer_switch_active_notify_when(
            &atz_sw,
            {
                let suppress = suppress.clone();
                move || !suppress.get()
            },
            move |active| {
                tz_btn.set_sensitive(!active);
                let mut cfg = metis_config::load_datetime_config();
                cfg.auto_timezone = active;
                let _ = metis_config::save_datetime_config(&cfg);
                if active {
                    let time_value = time_value.clone();
                    let tz_value = tz_value.clone();
                    let status = status.clone();
                    bg::run_bg(
                    move || {
                        if let Some(tz) = resolve_timezone_from_ip() {
                            let _ = metis_remote::set_timezone(&tz);
                        }
                    },
                    move |_| {
                        refresh_status(&status, &time_value, &tz_value);
                    },
                );
                }
            },
        );
    }

    // Manual date/time sheet
    {
        let status = status.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        time_btn.connect_clicked(move |_| {
            open_set_time_sheet(status.clone(), time_value.clone(), tz_value.clone());
        });
    }

    // Timezone picker
    {
        let status = status.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        tz_btn.connect_clicked(move |_| {
            open_timezone_sheet(status.clone(), time_value.clone(), tz_value.clone());
        });
    }

    // Time format
    {
        let suppress = suppress.clone();
        let _labels = format_labels;
        format_dd.connect_selected_notify(move |dd| {
            if suppress.get() {
                return;
            }
            let mut cfg = metis_config::load_bar_config();
            cfg.clock.time_format = if dd.selected() == 1 {
                "%H:%M".into()
            } else {
                "%-I:%M %p".into()
            };
            if let Err(err) = metis_config::save_bar_config(&cfg) {
                tracing::warn!(%err, "failed to save bar.json time format");
                return;
            }
            runtime::send("reload-bar");
        });
    }

    // First day of week
    {
        let suppress = suppress.clone();
        let _day_labels = day_labels;
        day_dd.connect_selected_notify(move |dd| {
            if suppress.get() {
                return;
            }
            let idx = dd.selected() as usize;
            let mut cfg = metis_config::load_datetime_config();
            cfg.first_day_of_week = day_choices.get(idx).copied().unwrap_or_default();
            if let Err(err) = metis_config::save_datetime_config(&cfg) {
                tracing::warn!(%err, "failed to save datetime.json");
            }
        });
    }

    // Periodic refresh while visible
    {
        let status = status.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        let ntp_sw = ntp_sw.clone();
        let suppress = suppress.clone();
        let weak = scroller.downgrade();
        glib::timeout_add_local(std::time::Duration::from_secs(5), move || {
            let Some(scroller) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !scroller.is_visible() {
                return glib::ControlFlow::Continue;
            }
            refresh_status(&status, &time_value, &tz_value);
            if let Some(s) = status.borrow().as_ref() {
                suppress.set(true);
                ntp_sw.set_active(s.ntp);
                suppress.set(false);
            }
            glib::ControlFlow::Continue
        });
    }

    scroller.upcast()
}

fn refresh_status(
    status: &Rc<RefCell<Option<DateTimeStatus>>>,
    time_value: &gtk::Label,
    tz_value: &gtk::Label,
) {
    if let Ok(snap) = metis_remote::datetime_status() {
        time_value.set_label(&snap.local_time);
        tz_value.set_label(&snap.timezone);
        *status.borrow_mut() = Some(snap);
    }
}

fn open_set_time_sheet(
    status: Rc<RefCell<Option<DateTimeStatus>>>,
    time_value: gtk::Label,
    tz_value: gtk::Label,
) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let entry = gtk::Entry::builder()
        .placeholder_text("2026-09-19 20:37:00")
        .hexpand(true)
        .build();
    ui::swallow_empty_backspace(&entry);
    if let Some(s) = status.borrow().as_ref() {
        // Best-effort seed from `date`.
        let _ = s;
    }
    body.append(&ui::hint(&tr(
        "Enter local time as YYYY-MM-DD HH:MM:SS. Network time must be off.",
    )));
    body.append(&entry);
    let apply = gtk::Button::with_label(&tr("Set"));
    apply.add_css_class("suggested-action");
    apply.set_halign(gtk::Align::End);
    body.append(&apply);
    apply.connect_clicked(move |_| {
        let spec = entry.text().to_string();
        let status = status.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        bg::run_bg(
            move || {
                let result = metis_remote::set_time(&spec);
                result
            },
            move |result| {
        if let Err(err) = result {
                            tracing::warn!(%err, "set-time failed");
                        } else {
                            dialog::dismiss(false);
                            refresh_status(&status, &time_value, &tz_value);
                        }
            },
        );
    });
    let _ = dialog::present(&tr("Date & Time"), &body, Rc::new(|| {}));
}

fn open_timezone_sheet(
    status: Rc<RefCell<Option<DateTimeStatus>>>,
    time_value: gtk::Label,
    tz_value: gtk::Label,
) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search time zones…"))
        .hexpand(true)
        .build();
    body.append(&search);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("metis-settings-list");
    let zones = Rc::new(load_zone_list());
    populate_zones(&list, &zones, "");
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(280)
        .vexpand(true)
        .child(&list)
        .build();
    body.append(&scroll);

    {
        let list = list.clone();
        let zones = zones.clone();
        search.connect_search_changed(move |s| {
            populate_zones(&list, &zones, &s.text());
        });
    }

    {
        let status = status.clone();
        let time_value = time_value.clone();
        let tz_value = tz_value.clone();
        list.connect_row_activated(move |_, row| {
            let Some(child) = row.child() else { return };
            let Some(lbl) = child.downcast_ref::<gtk::Label>() else {
                return;
            };
            let tz = lbl.text().to_string();
            let status = status.clone();
            let time_value = time_value.clone();
            let tz_value = tz_value.clone();
            bg::run_bg(
            move || {
                let result = metis_remote::set_timezone(&tz);
                result
            },
            move |result| {
        if let Err(err) = result {
                                tracing::warn!(%err, "set-timezone failed");
                            } else {
                                dialog::dismiss(false);
                                refresh_status(&status, &time_value, &tz_value);
                            }
            },
        );
        });
    }

    let _ = dialog::present(&tr("Time Zone"), &body, Rc::new(|| {}));
}

fn populate_zones(list: &gtk::ListBox, zones: &[String], query: &str) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let q = query.trim().to_ascii_lowercase();
    for z in zones {
        if !q.is_empty() && !z.to_ascii_lowercase().contains(&q) {
            continue;
        }
        let lbl = gtk::Label::new(Some(z));
        lbl.set_xalign(0.0);
        lbl.set_margin_start(8);
        lbl.set_margin_end(8);
        lbl.set_margin_top(6);
        lbl.set_margin_bottom(6);
        list.append(&lbl);
    }
}

fn load_zone_list() -> Vec<String> {
    let mut out = Vec::new();
    for path in [
        "/usr/share/zoneinfo/zone1970.tab",
        "/usr/share/zoneinfo/zone.tab",
    ] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            // zone.tab: code coords TZ comments
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 3 {
                let tz = if path.ends_with("zone1970.tab") {
                    cols.get(2).copied()
                } else {
                    cols.get(2).copied()
                };
                if let Some(tz) = tz {
                    if tz.contains('/') {
                        out.push(tz.to_string());
                    }
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out.sort();
    out.dedup();
    if out.is_empty() {
        out.extend(
            [
                "UTC",
                "America/Los_Angeles",
                "America/New_York",
                "Europe/London",
                "Europe/Berlin",
                "Asia/Tokyo",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    out
}

/// Best-effort IP → IANA timezone (same family of lookup as weather).
fn resolve_timezone_from_ip() -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    // ipapi.co returns timezone in JSON.
    let resp: serde_json::Value = client
        .get("https://ipapi.co/json/")
        .send()
        .ok()?
        .json()
        .ok()?;
    resp.get("timezone")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}
