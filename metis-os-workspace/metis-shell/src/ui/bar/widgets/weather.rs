use std::cell::RefCell;

use gtk::prelude::*;

use crate::services::{LocationWeather, WeatherSnapshot};
use crate::ui::icons;

/// Bar widget: a condition icon + temperature button that opens a popover with
/// current conditions, a short hourly strip, and any extra saved locations.
pub struct WeatherWidget {
    root: gtk::Button,
    icon: gtk::Image,
    temp_label: gtk::Label,
    primary: gtk::Box,
    hourly: gtk::Box,
    others: gtk::Box,
    others_sep: gtk::Separator,
    status: gtk::Label,
    last_sig: RefCell<String>,
    compact: bool,
}

impl WeatherWidget {
    pub fn new(compact: bool) -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-weather");
        if compact {
            root.add_css_class("metis-bar-weather-compact");
        }

        let bar_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = icons::image("weather-overcast-symbolic");
        bar_box.append(&icon);
        let temp_label = gtk::Label::new(None);
        temp_label.add_css_class("metis-weather-bar-label");
        if !compact {
            bar_box.append(&temp_label);
        }
        root.set_child(Some(&bar_box));
        root.set_tooltip_text(Some(&metis_i18n::tr("Weather")));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(10);
        panel.set_width_request(300);

        let primary = gtk::Box::new(gtk::Orientation::Vertical, 4);
        primary.add_css_class("metis-weather-primary");
        panel.append(&primary);

        let hourly = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hourly.set_homogeneous(true);
        hourly.add_css_class("metis-weather-hourly");
        panel.append(&hourly);

        let others_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        others_sep.add_css_class("metis-weather-sep");
        others_sep.set_visible(false);
        panel.append(&others_sep);

        let others = gtk::Box::new(gtk::Orientation::Vertical, 2);
        others.add_css_class("metis-weather-others");
        panel.append(&others);

        let status = gtk::Label::new(None);
        status.add_css_class("metis-weather-status");
        status.set_halign(gtk::Align::Start);
        status.set_visible(false);
        panel.append(&status);

        let attrib = gtk::Label::new(Some("Open-Meteo"));
        attrib.add_css_class("metis-weather-attrib");
        attrib.set_halign(gtk::Align::End);
        panel.append(&attrib);

        super::super::dropdown::wire_toggle_prepare(&root, &panel, || {
            crate::services::weather_refresh();
        });

        Self {
            root,
            icon,
            temp_label,
            primary,
            hourly,
            others,
            others_sep,
            status,
            last_sig: RefCell::new(String::new()),
            compact,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, snapshot: &WeatherSnapshot) {
        let sig = signature(snapshot);
        if *self.last_sig.borrow() == sig {
            return;
        }
        *self.last_sig.borrow_mut() = sig;

        let unit = unit_letter(snapshot.fahrenheit);

        match snapshot.locations.first() {
            Some(primary) => {
                if !self.compact {
                    self.temp_label
                        .set_text(&format!("{:.0}°{}", primary.temp.round(), unit));
                }
                icons::set_icon(&self.icon, weather_icon(primary.code, primary.is_day));
                self.root.set_tooltip_text(Some(&format!(
                    "{} · {:.0}°{} {}",
                    primary.name,
                    primary.temp.round(),
                    unit,
                    crate::services::weather::condition_label(primary.code)
                )));
                self.status.set_visible(false);
            }
            None => {
                if !self.compact {
                    self.temp_label.set_text("");
                }
                icons::set_icon(&self.icon, "weather-severe-alert-symbolic");
                let msg = snapshot
                    .error
                    .clone()
                    .unwrap_or_else(|| metis_i18n::tr("Weather unavailable"));
                self.root.set_tooltip_text(Some(&msg));
                self.status.set_text(&msg);
                self.status.set_visible(true);
            }
        }

        self.rebuild_primary(snapshot.locations.first(), unit);
        self.rebuild_hourly(snapshot.locations.first(), unit);
        self.rebuild_others(snapshot, unit);
    }

    fn rebuild_primary(&self, primary: Option<&LocationWeather>, unit: char) {
        clear(&self.primary);
        let Some(p) = primary else {
            return;
        };

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let left = gtk::Box::new(gtk::Orientation::Vertical, 2);
        left.set_hexpand(true);
        left.set_halign(gtk::Align::Start);
        let name = gtk::Label::new(Some(&p.name));
        name.add_css_class("metis-weather-loc");
        name.set_halign(gtk::Align::Start);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(20);
        left.append(&name);
        let temp = gtk::Label::new(Some(&format!("{:.0}°", p.temp.round())));
        temp.add_css_class("metis-weather-temp");
        temp.set_halign(gtk::Align::Start);
        left.append(&temp);
        row.append(&left);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 2);
        right.set_halign(gtk::Align::End);
        right.set_valign(gtk::Align::Center);
        let cond = gtk::Label::new(Some(&crate::services::weather::condition_label(p.code)));
        cond.add_css_class("metis-weather-cond");
        cond.set_halign(gtk::Align::End);
        right.append(&cond);
        let hl = gtk::Label::new(Some(
            &metis_i18n::tr("H:%1°%2 L:%3°%2")
                .replace("%1", &format!("{:.0}", p.high.round()))
                .replace("%3", &format!("{:.0}", p.low.round()))
                .replace("%2", &unit.to_string()),
        ));
        hl.add_css_class("metis-weather-hl");
        hl.set_halign(gtk::Align::End);
        right.append(&hl);
        row.append(&right);

        self.primary.append(&row);
    }

    fn rebuild_hourly(&self, primary: Option<&LocationWeather>, _unit: char) {
        clear(&self.hourly);
        let Some(p) = primary else {
            self.hourly.set_visible(false);
            return;
        };
        if p.hourly.is_empty() {
            self.hourly.set_visible(false);
            return;
        }
        self.hourly.set_visible(true);
        for point in &p.hourly {
            let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
            col.add_css_class("metis-weather-hour");
            let label = gtk::Label::new(Some(&crate::services::weather::hour_label(point.hour24)));
            label.add_css_class("metis-weather-hour-label");
            col.append(&label);
            let icon = icons::image(weather_icon(point.code, point.is_day));
            col.append(&icon);
            let temp = gtk::Label::new(Some(&format!("{:.0}°", point.temp.round())));
            temp.add_css_class("metis-weather-hour-temp");
            col.append(&temp);
            self.hourly.append(&col);
        }
    }

    fn rebuild_others(&self, snapshot: &WeatherSnapshot, unit: char) {
        clear(&self.others);
        let extras = snapshot.locations.get(1..).unwrap_or(&[]);
        self.others_sep.set_visible(!extras.is_empty());
        for loc in extras {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.add_css_class("metis-weather-other");
            let name = gtk::Label::new(Some(&loc.name));
            name.set_halign(gtk::Align::Start);
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_max_width_chars(20);
            row.append(&name);
            let icon = icons::image(weather_icon(loc.code, loc.is_day));
            row.append(&icon);
            let temp = gtk::Label::new(Some(&format!("{:.0}°{unit}", loc.temp.round())));
            temp.add_css_class("metis-weather-other-temp");
            row.append(&temp);
            self.others.append(&row);
        }
    }
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn unit_letter(fahrenheit: bool) -> char {
    if fahrenheit { 'F' } else { 'C' }
}

/// Map an Open-Meteo WMO weather code to a freedesktop symbolic icon.
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
        61..=67 => "weather-showers-symbolic",
        71..=77 | 85 | 86 => "weather-snow-symbolic",
        80..=82 => "weather-showers-symbolic",
        95..=99 => "weather-storm-symbolic",
        _ => "weather-overcast-symbolic",
    }
}

fn signature(snapshot: &WeatherSnapshot) -> String {
    let mut s = format!(
        "f{}|e{}|",
        snapshot.fahrenheit as u8,
        snapshot.error.as_deref().unwrap_or("")
    );
    for loc in &snapshot.locations {
        s.push_str(&format!(
            "{}:{:.0}:{}:{:.0}:{:.0}:{};",
            loc.name,
            loc.temp.round(),
            loc.code,
            loc.high.round(),
            loc.low.round(),
            loc.hourly.len()
        ));
    }
    s
}
