//! Wireless tab: Wi-Fi scan/connect, password prompt, known networks sheet.

use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::dialog;
use crate::net::{self, SavedConn};
use metis_i18n::tr;

use super::{clear, hint, schedule_refresh};

/// Fill the Wi-Fi scan list for the Wireless tab.
pub(crate) fn render_wifi_list<F: Fn() + 'static>(
    wifi_box: &gtk::Box,
    snap: &crate::net::NetSnapshot,
    refresh: &Rc<F>,
) {
    clear(wifi_box);
    if !snap.wifi_enabled {
        wifi_box.append(&hint(&tr("Wi-Fi is off.")));
    } else if snap.wifi.is_empty() {
        wifi_box.append(&hint(&tr("No networks found.")));
    } else {
        for (i, n) in snap.wifi.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("metis-settings-zebra-row");
            row.add_css_class("metis-settings-wifi-row");
            if i % 2 == 1 {
                row.add_css_class("metis-settings-zebra-row-alt");
                row.add_css_class("metis-settings-wifi-row-alt");
            }
            let lock = if n.secured { "🔒 " } else { "" };
            let label = gtk::Label::new(Some(&format!("{lock}{}  ·  {}%", n.ssid, n.signal)));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            row.append(&label);
            if n.active {
                let tag = gtk::Label::new(Some(&tr("Connected")));
                tag.add_css_class("metis-settings-hint");
                row.append(&tag);
            } else {
                let connect = gtk::Button::with_label(&tr("Connect"));
                {
                    let refresh = refresh.clone();
                    let net_box = wifi_box.clone();
                    let row_ref = row.clone();
                    let n = n.clone();
                    connect.connect_clicked(move |btn| {
                        if n.secured {
                            prompt_password(&net_box, &row_ref, &n.ssid, &refresh);
                            btn.set_sensitive(false);
                        } else {
                            net::connect_wifi(n.ssid.clone(), None);
                            schedule_refresh(&refresh, 3000);
                        }
                    });
                }
                row.append(&connect);
            }
            wifi_box.append(&row);
        }
    }
}

/// Update the Known Wi-Fi networks button from the snapshot.
pub(crate) fn update_known_button(
    known_btn: &gtk::Button,
    saved: &[crate::net::SavedConn],
    last_saved: &std::cell::RefCell<Vec<crate::net::SavedConn>>,
) {
    let n = saved.len();
    *last_saved.borrow_mut() = saved.to_vec();
    if n == 0 {
        known_btn.set_label(&tr("Known Wi-Fi networks…"));
        known_btn.set_sensitive(false);
    } else {
        known_btn.set_label(&tr(&format!("Known Wi-Fi networks ({n})…")));
        known_btn.set_sensitive(true);
    }
}

/// Top-slide sheet to forget saved Wi-Fi profiles or view connection info.
pub(crate) fn show_known_wifi_sheet<F: Fn() + 'static>(saved: Vec<SavedConn>, refresh: Rc<F>) {
    if dialog::is_open() {
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let intro = gtk::Label::new(Some(&tr(
        "Saved Wi-Fi profiles on this device. Forget removes the NetworkManager connection.",
    )));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("metis-settings-top-dialog-body");
    wrap.append(&intro);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    list.add_css_class("metis-settings-list");
    list.set_hexpand(true);

    let detail = gtk::Label::new(None);
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_selectable(true);
    detail.add_css_class("metis-settings-hint");
    detail.set_visible(false);
    detail.set_margin_top(4);

    let empty = hint(&tr("No saved Wi-Fi networks."));
    empty.set_visible(saved.is_empty());
    wrap.append(&empty);

    if saved.is_empty() {
        let _ = dialog::present(&tr("Known Wi-Fi networks"), &wrap, Rc::new(|| {}));
        return;
    }

    let (info_tx, info_rx) = mpsc::channel::<(String, String)>();
    {
        let detail = detail.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            match info_rx.try_recv() {
                Ok((title, body)) => {
                    detail.set_markup(&format!("<b>{title}</b>\n{body}"));
                    detail.set_visible(true);
                    glib::ControlFlow::Continue
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    for c in saved {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("metis-settings-row");
        row.set_hexpand(true);
        let label = gtk::Label::new(Some(&c.name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let info = gtk::Button::with_label(&tr("Info"));
        info.add_css_class("metis-settings-secondary");
        info.add_css_class("flat");
        {
            let name = c.name.clone();
            let uuid = c.uuid.clone();
            let ctype = c.ctype.clone();
            let info_tx = info_tx.clone();
            info.connect_clicked(move |_| {
                let name = name.clone();
                let uuid = uuid.clone();
                let ctype = ctype.clone();
                let info_tx = info_tx.clone();
                std::thread::spawn(move || {
                    let ipv4 = net::read_ipv4(&name);
                    let mut body = format!("{}: {uuid}\n{}: {ctype}", tr("UUID"), tr("Type"));
                    if !ipv4.method.is_empty() {
                        body.push_str(&format!("\n{}: {}", tr("IPv4 method"), ipv4.method));
                    }
                    if !ipv4.addresses.is_empty() {
                        body.push_str(&format!("\n{}: {}", tr("Address"), ipv4.addresses));
                    }
                    if !ipv4.gateway.is_empty() {
                        body.push_str(&format!("\n{}: {}", tr("Gateway"), ipv4.gateway));
                    }
                    if !ipv4.dns.is_empty() {
                        body.push_str(&format!("\n{}: {}", tr("DNS"), ipv4.dns));
                    }
                    let _ = info_tx.send((
                        glib::markup_escape_text(&name).to_string(),
                        glib::markup_escape_text(&body).to_string(),
                    ));
                });
            });
        }

        let forget = gtk::Button::with_label(&tr("Forget"));
        forget.add_css_class("destructive-action");
        {
            let refresh = refresh.clone();
            let uuid = c.uuid.clone();
            let row = row.clone();
            let list = list.clone();
            let empty = empty.clone();
            forget.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                net::forget(&uuid);
                list.remove(&row);
                if list.first_child().is_none() {
                    empty.set_visible(true);
                }
                schedule_refresh(&refresh, 1200);
            });
        }

        row.append(&label);
        row.append(&info);
        row.append(&forget);
        list.append(&row);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(160)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    wrap.append(&scroll);
    wrap.append(&detail);

    let _ = dialog::present(&tr("Known Wi-Fi networks"), &wrap, Rc::new(|| {}));
}

fn prompt_password<F: Fn() + 'static>(
    container: &gtk::Box,
    after: &gtk::Box,
    ssid: &str,
    refresh: &Rc<F>,
) {
    let prompt = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    prompt.add_css_class("metis-settings-row");
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .build();
    let join = gtk::Button::with_label(&tr("Join"));
    prompt.append(&entry);
    prompt.append(&join);

    // Insert right after the network row.
    container.insert_child_after(&prompt, Some(after));
    entry.grab_focus();

    {
        let refresh = refresh.clone();
        let ssid = ssid.to_string();
        let entry2 = entry.clone();
        join.connect_clicked(move |_| {
            net::connect_wifi(ssid.clone(), Some(entry2.text().to_string()));
            schedule_refresh(&refresh, 3000);
        });
    }
    {
        let refresh = refresh.clone();
        let ssid = ssid.to_string();
        entry.connect_activate(move |e| {
            net::connect_wifi(ssid.clone(), Some(e.text().to_string()));
            schedule_refresh(&refresh, 3000);
        });
    }
}
