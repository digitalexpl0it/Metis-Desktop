//! Clock widget — large time + date using the edge-bar clock formats.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{DesktopWidgetInstance, load_bar_config};

use super::font::apply_font;

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_valign(gtk::Align::Center);
    root.set_halign(gtk::Align::Center);
    root.set_hexpand(true);
    root.set_vexpand(true);

    let time = gtk::Label::new(None);
    time.add_css_class("metis-dw-clock-time");
    time.set_xalign(0.5);
    apply_font(&time, &inst.font);
    root.append(&time);

    let date = gtk::Label::new(None);
    date.add_css_class("metis-dw-clock-date");
    date.set_xalign(0.5);
    apply_font(&date, &inst.font);
    root.append(&date);

    let tick = {
        let time = time.clone();
        let date = date.clone();
        move || {
            let cfg = load_bar_config();
            let now = chrono::Local::now();
            let tf = if cfg.clock.time_format.trim().is_empty() {
                "%I:%M %p"
            } else {
                cfg.clock.time_format.as_str()
            };
            let df = if cfg.clock.date_format.trim().is_empty() {
                "%A, %B %e"
            } else {
                cfg.clock.date_format.as_str()
            };
            // Localized weekday/month names when formats_from_locale is on.
            time.set_text(&metis_i18n::format_pattern(&now, tf));
            date.set_text(&metis_i18n::format_pattern(&now, df));
            glib::ControlFlow::Continue
        }
    };
    tick();
    let source = Rc::new(Cell::new(Some(glib::timeout_add_seconds_local(1, tick))));
    root.connect_destroy(move |_| {
        if let Some(id) = source.take() {
            id.remove();
        }
    });

    root.upcast()
}
