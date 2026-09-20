//! Weather: temperature unit, auto-detect toggles, and a pinned-locations list
//! (with Open-Meteo geocoding search). Writes `weather.json` and nudges the bar
//! to refetch via the `reload-weather` runtime command.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use metis_config::{TempUnit, WeatherConfig, WeatherLocation};

use crate::{runtime, ui};
use metis_i18n::tr;

#[derive(Debug, Clone)]
struct GeoResult {
    name: String,
    detail: String,
    lat: f64,
    lon: f64,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("weather");
    let cfg = Rc::new(RefCell::new(metis_config::load_weather_config()));

    // ---- Units ------------------------------------------------------------
    let (unit_card, unit_body) = ui::section(&tr("Units"));
    let unit_dd = {
        let __dd_labels = [tr("Automatic"), tr("Celsius"), tr("Fahrenheit")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    unit_dd.set_selected(match cfg.borrow().unit {
        TempUnit::Auto => 0,
        TempUnit::Celsius => 1,
        TempUnit::Fahrenheit => 2,
    });
    unit_body.append(&ui::row(&tr("Temperature unit"), &unit_dd));
    content.append(&unit_card);
    {
        let cfg = cfg.clone();
        unit_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().unit = match dd.selected() {
                1 => TempUnit::Celsius,
                2 => TempUnit::Fahrenheit,
                _ => TempUnit::Auto,
            };
            save(&cfg.borrow());
        });
    }

    // ---- Auto-detect ------------------------------------------------------
    let (auto_card, auto_body) = ui::section(&tr("Auto-detect"));
    let auto_sw = gtk::Switch::new();
    auto_sw.set_active(cfg.borrow().auto_detect);
    auto_sw.set_halign(gtk::Align::End);
    auto_body.append(&ui::row(&tr("Detect my location"), &auto_sw));

    let ip_sw = gtk::Switch::new();
    ip_sw.set_active(cfg.borrow().ip_geolocation);
    ip_sw.set_halign(gtk::Align::End);
    ip_sw.set_sensitive(cfg.borrow().auto_detect);
    auto_body.append(&ui::row(&tr("Use IP geolocation (more precise)"), &ip_sw));

    let auto_hint = gtk::Label::new(Some(&tr(
        "Auto-detect is used only when no locations are pinned below.",
    )));
    auto_hint.set_xalign(0.0);
    auto_hint.add_css_class("metis-settings-hint");
    auto_body.append(&auto_hint);
    content.append(&auto_card);
    {
        let cfg = cfg.clone();
        let ip_sw = ip_sw.clone();
        auto_sw.connect_active_notify(move |s| {
            cfg.borrow_mut().auto_detect = s.is_active();
            ip_sw.set_sensitive(s.is_active());
            save(&cfg.borrow());
        });
    }
    {
        let cfg = cfg.clone();
        ip_sw.connect_active_notify(move |s| {
            cfg.borrow_mut().ip_geolocation = s.is_active();
            save(&cfg.borrow());
        });
    }

    // ---- Locations --------------------------------------------------------
    let (loc_card, loc_body) = ui::section(&tr("Locations"));

    let saved_list = gtk::ListBox::new();
    saved_list.set_selection_mode(gtk::SelectionMode::None);
    saved_list.add_css_class("metis-settings-list");
    loc_body.append(&saved_list);
    rebuild_saved(&saved_list, &cfg);

    let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    search_row.add_css_class("metis-settings-inset");
    let search_entry = gtk::Entry::builder()
        .placeholder_text(tr("Search for a city…"))
        .hexpand(true)
        .build();
    ui::swallow_empty_backspace(&search_entry);
    let search_btn = gtk::Button::with_label(&tr("Search"));
    search_row.append(&search_entry);
    search_row.append(&search_btn);
    loc_body.append(&search_row);

    let results_list = gtk::ListBox::new();
    results_list.set_selection_mode(gtk::SelectionMode::None);
    results_list.add_css_class("metis-settings-list");
    loc_body.append(&results_list);
    content.append(&loc_card);

    // Geocoding runs on a worker thread; results come back over an mpsc channel
    // drained on the GTK main thread.
    let (tx, rx) = mpsc::channel::<Vec<GeoResult>>();
    {
        let results_list = results_list.clone();
        let saved_list = saved_list.clone();
        let cfg = cfg.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            if let Ok(results) = rx.try_recv() {
                populate_results(&results_list, &results, &saved_list, &cfg);
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let tx = tx.clone();
        let entry = search_entry.clone();
        let results_list = results_list.clone();
        let run_search = move || {
            let query = entry.text().trim().to_string();
            if query.is_empty() {
                return;
            }
            clear_list(&results_list);
            let placeholder = gtk::Label::new(Some(&tr("Searching…")));
            placeholder.set_xalign(0.0);
            results_list.append(&placeholder);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let results = geocode_search(&query);
                let _ = tx.send(results);
            });
        };
        let run2 = run_search.clone();
        search_btn.connect_clicked(move |_| run_search());
        search_entry.connect_activate(move |_| run2());
    }

    scroller.upcast()
}

fn populate_results(
    results_list: &gtk::ListBox,
    results: &[GeoResult],
    saved_list: &gtk::ListBox,
    cfg: &Rc<RefCell<WeatherConfig>>,
) {
    clear_list(results_list);
    if results.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No matches found.")));
        empty.set_xalign(0.0);
        results_list.append(&empty);
        return;
    }
    for r in results {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&format!("{}, {}", r.name, r.detail)));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let add = gtk::Button::with_label(&tr("Add"));
        row.append(&label);
        row.append(&add);
        {
            let cfg = cfg.clone();
            let saved_list = saved_list.clone();
            let results_list = results_list.clone();
            let r = r.clone();
            add.connect_clicked(move |_| {
                cfg.borrow_mut().locations.push(WeatherLocation {
                    name: r.name.clone(),
                    latitude: r.lat,
                    longitude: r.lon,
                });
                save(&cfg.borrow());
                rebuild_saved(&saved_list, &cfg);
                clear_list(&results_list);
            });
        }
        results_list.append(&row);
    }
}

fn rebuild_saved(list: &gtk::ListBox, cfg: &Rc<RefCell<WeatherConfig>>) {
    clear_list(list);
    let count = cfg.borrow().locations.len();
    if count == 0 {
        let empty = gtk::Label::new(Some(&tr("No pinned locations (using auto-detect).")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        list.append(&empty);
        return;
    }
    for idx in 0..count {
        let name = cfg.borrow().locations[idx].name.clone();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        if idx == 0 {
            label.set_text(&format!("{name}  (primary)"));
        }
        let up = gtk::Button::from_icon_name("go-up-symbolic");
        let down = gtk::Button::from_icon_name("go-down-symbolic");
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        up.set_sensitive(idx > 0);
        down.set_sensitive(idx + 1 < count);
        row.append(&label);
        row.append(&up);
        row.append(&down);
        row.append(&remove);

        {
            let cfg = cfg.clone();
            let list = list.clone();
            up.connect_clicked(move |_| {
                cfg.borrow_mut().locations.swap(idx, idx - 1);
                save(&cfg.borrow());
                rebuild_saved(&list, &cfg);
            });
        }
        {
            let cfg = cfg.clone();
            let list = list.clone();
            down.connect_clicked(move |_| {
                cfg.borrow_mut().locations.swap(idx, idx + 1);
                save(&cfg.borrow());
                rebuild_saved(&list, &cfg);
            });
        }
        {
            let cfg = cfg.clone();
            let list = list.clone();
            remove.connect_clicked(move |_| {
                cfg.borrow_mut().locations.remove(idx);
                save(&cfg.borrow());
                rebuild_saved(&list, &cfg);
            });
        }
        list.append(&row);
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn save(cfg: &WeatherConfig) {
    if let Err(err) = metis_config::save_weather_config(cfg) {
        tracing::warn!(%err, "failed to save weather.json");
    }
    runtime::send("reload-weather");
}

/// Blocking Open-Meteo geocoding lookup (runs on a worker thread).
fn geocode_search(query: &str) -> Vec<GeoResult> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=en&format=json",
        urlencode(query)
    );
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "geocode: client build failed");
            return Vec::new();
        }
    };
    let json: serde_json::Value = match client.get(&url).send().and_then(|r| r.json()) {
        Ok(j) => j,
        Err(err) => {
            tracing::warn!(%err, "geocode: request failed");
            return Vec::new();
        }
    };
    let Some(results) = json.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let name = r.get("name")?.as_str()?.to_string();
            let lat = r.get("latitude")?.as_f64()?;
            let lon = r.get("longitude")?.as_f64()?;
            let admin = r.get("admin1").and_then(|v| v.as_str()).unwrap_or("");
            let country = r.get("country").and_then(|v| v.as_str()).unwrap_or("");
            let detail = [admin, country]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            Some(GeoResult {
                name,
                detail,
                lat,
                lon,
            })
        })
        .collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
