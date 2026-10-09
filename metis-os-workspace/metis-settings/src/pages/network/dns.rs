//! DNS tab: Wi-Fi DNS override editor.

use std::rc::Rc;

use gtk::prelude::*;

use crate::net;
use crate::ui;
use metis_i18n::tr;

use super::{entry, hint, schedule_refresh};

/// A standalone DNS-override editor for a connection (DNS tab):
/// a comma-separated DNS list applied with `ignore-auto-dns` so it overrides DHCP.
pub(crate) fn dns_override_editor<F: Fn() + 'static>(
    conn: &str,
    ipv4: &net::Ipv4,
    refresh: &Rc<F>,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("metis-settings-inset");

    let title = gtk::Label::new(Some(&tr(&format!("Connected: {conn}"))));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-value");
    card.append(&title);

    let dns = entry("1.1.1.1, 8.8.8.8", &ipv4.dns);
    card.append(&ui::row(&tr("DNS servers"), &dns));
    card.append(&hint(&tr(
        "Comma-separated. Leave empty to use the DHCP-provided DNS.",
    )));

    let apply = gtk::Button::with_label(&tr("Apply DNS"));
    apply.add_css_class("suggested-action");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_halign(gtk::Align::End);
    actions.append(&apply);
    card.append(&actions);
    {
        let refresh = refresh.clone();
        let conn = conn.to_string();
        apply.connect_clicked(move |_| {
            net::set_dns_override(&conn, &dns.text());
            schedule_refresh(&refresh, 2500);
        });
    }

    card.upcast()
}
