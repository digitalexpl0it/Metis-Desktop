//! Proxy tab: system proxy editor (GNOME gsettings).

use std::rc::Rc;

use gtk::prelude::*;

use crate::net;
use crate::ui;
use metis_i18n::tr;

use super::{entry, hint, schedule_refresh};

pub(crate) fn proxy_editor<F: Fn() + 'static>(
    cfg: &net::ProxyConfig,
    refresh: &Rc<F>,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);

    if !cfg.available {
        card.append(&hint(&tr(
            "System proxy settings are unavailable (the GNOME proxy schema isn't installed).",
        )));
        return card.upcast();
    }

    let mode = {
        let __dd_labels = [tr("None"), tr("Manual"), tr("Automatic (PAC)")];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    let mode_idx = match cfg.mode.as_str() {
        "manual" => 1,
        "auto" => 2,
        _ => 0,
    };
    mode.set_selected(mode_idx);
    card.append(&ui::row(&tr("Proxy mode"), &mode));

    // Manual: per-protocol host:port + ignore list.
    let manual_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (http_row, http_host, http_port) = host_port_row("HTTP", &cfg.http_host, cfg.http_port);
    let (https_row, https_host, https_port) =
        host_port_row("HTTPS", &cfg.https_host, cfg.https_port);
    let (socks_row, socks_host, socks_port) =
        host_port_row("SOCKS", &cfg.socks_host, cfg.socks_port);
    let ignore = entry("localhost, 127.0.0.0/8, ::1", &cfg.ignore_hosts);
    manual_box.append(&http_row);
    manual_box.append(&https_row);
    manual_box.append(&socks_row);
    manual_box.append(&ui::row(&tr("Ignore hosts"), &ignore));
    manual_box.set_visible(mode_idx == 1);
    card.append(&manual_box);

    // Automatic: PAC URL.
    let auto_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let pac = entry("http://example.com/proxy.pac", &cfg.auto_url);
    auto_box.append(&ui::row(&tr("PAC URL"), &pac));
    auto_box.set_visible(mode_idx == 2);
    card.append(&auto_box);

    {
        let manual_box = manual_box.clone();
        let auto_box = auto_box.clone();
        mode.connect_selected_notify(move |dd| {
            manual_box.set_visible(dd.selected() == 1);
            auto_box.set_visible(dd.selected() == 2);
        });
    }

    card.append(&hint(&tr(
        "Applies to GLib/GTK apps via the system proxy resolver.",
    )));

    let apply = gtk::Button::with_label(&tr("Apply"));
    apply.add_css_class("suggested-action");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_halign(gtk::Align::End);
    actions.append(&apply);
    card.append(&actions);
    {
        let refresh = refresh.clone();
        let mode = mode.clone();
        apply.connect_clicked(move |_| {
            let mode_str = match mode.selected() {
                1 => "manual",
                2 => "auto",
                _ => "none",
            };
            let new = net::ProxyConfig {
                mode: mode_str.to_string(),
                auto_url: pac.text().to_string(),
                http_host: http_host.text().to_string(),
                http_port: parse_port(&http_port.text()),
                https_host: https_host.text().to_string(),
                https_port: parse_port(&https_port.text()),
                socks_host: socks_host.text().to_string(),
                socks_port: parse_port(&socks_port.text()),
                ignore_hosts: ignore.text().to_string(),
                available: true,
            };
            net::set_proxy(new);
            schedule_refresh(&refresh, 1200);
        });
    }

    card.upcast()
}

/// A "<proto>  [host............] [port]" row for the proxy editor.
fn host_port_row(label: &str, host: &str, port: u32) -> (gtk::Box, gtk::Entry, gtk::Entry) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_width_chars(6);
    row.append(&lbl);

    let host_e = gtk::Entry::builder()
        .placeholder_text(tr("proxy.example.com"))
        .hexpand(true)
        .build();
    host_e.set_text(host);
    row.append(&host_e);

    let port_e = gtk::Entry::builder()
        .placeholder_text(tr("8080"))
        .max_width_chars(6)
        .build();
    if port != 0 {
        port_e.set_text(&port.to_string());
    }
    row.append(&port_e);

    (row, host_e, port_e)
}

fn parse_port(s: &str) -> u32 {
    s.trim().parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_edge_cases() {
        assert_eq!(parse_port("8080"), 8080);
        assert_eq!(parse_port(" 443 "), 443);
        assert_eq!(parse_port(""), 0);
        assert_eq!(parse_port("abc"), 0);
        assert_eq!(parse_port("-1"), 0);
    }
}
