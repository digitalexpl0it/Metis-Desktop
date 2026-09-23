//! Weather widget — reuses the edge-bar weather snapshot.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::services::{LocationWeather, WeatherSnapshot, last_weather_snapshot, weather_refresh};
use crate::ui::icons;
use metis_config::DesktopWidgetInstance;

use super::font::apply_font;

thread_local! {
    /// Live desktop weather panels — updated when the bar weather channel fires.
    static PANELS: RefCell<Vec<WeatherPanel>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct WeatherPanel {
    icon: gtk::Image,
    temp: gtk::Label,
    place: gtk::Label,
    detail: gtk::Label,
}

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_valign(gtk::Align::Center);

    let icon = gtk::Image::new();
    icon.set_pixel_size(48);
    icon.set_halign(gtk::Align::Center);
    root.append(&icon);

    let temp = gtk::Label::new(None);
    temp.add_css_class("metis-dw-clock-time");
    temp.set_xalign(0.5);
    apply_font(&temp, &inst.font);
    root.append(&temp);

    let place = gtk::Label::new(None);
    place.add_css_class("metis-dw-clock-date");
    place.set_xalign(0.5);
    place.set_wrap(true);
    apply_font(&place, &inst.font);
    root.append(&place);

    let detail = gtk::Label::new(None);
    detail.add_css_class("metis-dw-hint");
    detail.set_xalign(0.5);
    detail.set_wrap(true);
    apply_font(&detail, &inst.font);
    root.append(&detail);

    let panel = WeatherPanel {
        icon: icon.clone(),
        temp: temp.clone(),
        place: place.clone(),
        detail: detail.clone(),
    };
    let snap = last_weather_snapshot();
    paint(&panel, snap.as_ref());
    if snap.is_none() {
        // Kick the worker if we came up before the first fetch landed.
        weather_refresh();
    }

    PANELS.with(|list| list.borrow_mut().push(panel.clone()));
    let source = Rc::new(Cell::new(Some(glib::timeout_add_seconds_local(30, {
        let panel = panel.clone();
        move || {
            paint(&panel, last_weather_snapshot().as_ref());
            glib::ControlFlow::Continue
        }
    }))));

    root.connect_destroy({
        let panel = panel.clone();
        move |_| {
            if let Some(id) = source.take() {
                id.remove();
            }
            PANELS.with(|list| {
                list.borrow_mut().retain(|p| p.temp != panel.temp);
            });
        }
    });

    root.upcast()
}

/// Called from the bar weather channel when a fresh snapshot arrives.
pub fn on_snapshot(snapshot: &WeatherSnapshot) {
    PANELS.with(|list| {
        for panel in list.borrow().iter() {
            paint(panel, Some(snapshot));
        }
    });
}

fn paint(panel: &WeatherPanel, snap: Option<&WeatherSnapshot>) {
    match snap {
        Some(snap) if !snap.locations.is_empty() => {
            apply_snapshot(&panel.icon, &panel.temp, &panel.place, &panel.detail, snap);
        }
        Some(snap) if snap.error.is_some() => {
            panel.icon.set_icon_name(Some("weather-overcast-symbolic"));
            panel.temp.set_text("—");
            panel.place.set_text(&metis_i18n::tr("Weather unavailable"));
            panel.detail.set_text(snap.error.as_deref().unwrap_or(""));
        }
        _ => {
            panel.icon.set_icon_name(Some("weather-overcast-symbolic"));
            panel.temp.set_text("—");
            panel
                .place
                .set_text(&metis_i18n::tr("Waiting for weather…"));
            panel.detail.set_text(&metis_i18n::tr(
                "Configure locations in Settings → Weather, or wait for auto-detect.",
            ));
        }
    }
}

fn apply_snapshot(
    icon: &gtk::Image,
    temp: &gtk::Label,
    place: &gtk::Label,
    detail: &gtk::Label,
    snap: &WeatherSnapshot,
) {
    let loc: &LocationWeather = &snap.locations[0];
    icons::set_icon(icon, weather_icon(loc.code, loc.is_day));
    icon.set_pixel_size(48);
    let unit = if snap.fahrenheit { 'F' } else { 'C' };
    temp.set_text(&format!("{:.0}°{unit}", loc.temp));
    place.set_text(&loc.name);
    detail.set_text(&format!(
        "{}  ·  {}",
        crate::services::weather::condition_label(loc.code),
        metis_i18n::tr("H:%1°%2 L:%3°%2")
            .replace("%1", &format!("{:.0}", loc.high))
            .replace("%3", &format!("{:.0}", loc.low))
            .replace("%2", &unit.to_string()),
    ));
}

fn weather_icon(code: i64, is_day: bool) -> &'static str {
    match code {
        0 => {
            if is_day {
                "weather-clear-symbolic"
            } else {
                "weather-clear-night-symbolic"
            }
        }
        1 | 2 => {
            if is_day {
                "weather-few-clouds-symbolic"
            } else {
                "weather-few-clouds-night-symbolic"
            }
        }
        3 => "weather-overcast-symbolic",
        45 | 48 => "weather-fog-symbolic",
        51..=57 => "weather-showers-scattered-symbolic",
        61..=67 | 80..=82 => "weather-showers-symbolic",
        71..=77 | 85 | 86 => "weather-snow-symbolic",
        95..=99 => "weather-storm-symbolic",
        _ => "weather-overcast-symbolic",
    }
}
