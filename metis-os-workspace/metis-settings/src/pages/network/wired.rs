//! Wired tab: per-NIC Ethernet IPv4 DHCP/static + DNS editor.

use std::rc::Rc;

use gtk::prelude::*;

use crate::net;
use crate::ui;
use metis_i18n::tr;

use super::{entry, hint, schedule_refresh};

pub(crate) fn ethernet_editor<F: Fn() + 'static>(
    dev: &net::EthDev,
    refresh: &Rc<F>,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("metis-settings-inset");
    card.set_margin_top(4);

    let status = if dev.connected {
        tr("Connected")
    } else {
        tr("Disconnected")
    };
    let title = match &dev.connection {
        Some(conn) if !conn.is_empty() => format!("{}  ·  {status}  ·  {conn}", dev.device),
        _ => format!("{}  ·  {status}", dev.device),
    };
    let title = gtk::Label::new(Some(&title));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-value");
    card.append(&title);

    let Some(conn) = dev.connection.clone() else {
        card.append(&hint(&tr("No active profile for this device.")));
        return card.upcast();
    };

    let method = {
        let __dd_labels = [tr("Automatic (DHCP)"), tr("Manual (static)")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    let is_manual = dev.ipv4.method == "manual";
    method.set_selected(if is_manual { 1 } else { 0 });
    card.append(&ui::row(&tr("IPv4 method"), &method));

    // Address + gateway only apply to a static config.
    let manual_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let addr = entry("192.168.1.50/24", &dev.ipv4.addresses);
    let gw = entry("192.168.1.1", &dev.ipv4.gateway);
    manual_box.append(&ui::row(&tr("Address (CIDR)"), &addr));
    manual_box.append(&ui::row(&tr("Gateway"), &gw));
    manual_box.set_visible(is_manual);
    card.append(&manual_box);

    // DNS applies to both methods: on DHCP it overrides the provided servers.
    let dns = entry("1.1.1.1, 8.8.8.8", &dev.ipv4.dns);
    card.append(&ui::row(&tr("DNS (override)"), &dns));

    {
        let manual_box = manual_box.clone();
        method.connect_selected_notify(move |dd| {
            manual_box.set_visible(dd.selected() == 1);
        });
    }

    let apply = gtk::Button::with_label(&tr("Apply"));
    apply.add_css_class("suggested-action");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_halign(gtk::Align::End);
    actions.append(&apply);
    card.append(&actions);
    {
        let refresh = refresh.clone();
        let conn = conn.clone();
        let method = method.clone();
        apply.connect_clicked(move |_| {
            if method.selected() == 1 {
                net::set_ipv4_static(&conn, &addr.text(), &gw.text(), &dns.text());
            } else {
                net::set_ipv4_dhcp(&conn, &dns.text());
            }
            schedule_refresh(&refresh, 2500);
        });
    }

    card.upcast()
}
