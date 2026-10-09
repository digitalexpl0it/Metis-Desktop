//! Network: a pill-tabbed page splitting Wireless (Wi-Fi scan/connect + known
//! networks sheet), DNS (Wi-Fi DNS override), Wired (per-NIC IPv4 DHCP/static +
//! DNS), VPN (NetworkManager OpenVPN / WireGuard), and Proxy (system proxy via
//! GNOME gsettings). All `nmcli`/`gsettings` work runs off the GTK main thread;
//! results arrive over an mpsc channel drained on a timeout.

mod dns;
mod proxy;
mod vpn;
mod wired;
mod wireless;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::gtk_cb::{OptFnStrRef, TabBarHandler};
use crate::net::{self, ActiveConn, EthDev, NetSnapshot, ProxyConfig, SavedConn, VpnConn};
use crate::ui;
use metis_i18n::tr;

thread_local! {
    /// Live Network tab switch (e.g. reuse `--page network/vpn` on an open window).
    static TAB_REQUEST: OptFnStrRef = const { RefCell::new(None) };
}

/// Switch the Network pill tab after the page is already built.
pub fn request_tab(tab: &str) {
    TAB_REQUEST.with(|slot| {
        if let Some(handler) = slot.borrow().as_ref() {
            handler(tab);
        }
    });
}

fn set_tab_request_handler(handler: Rc<dyn Fn(&str)>) {
    TAB_REQUEST.with(|slot| {
        *slot.borrow_mut() = Some(handler);
    });
}

struct Sections {
    radio: gtk::Switch,
    /// True while `render` syncs the radio switch from nmcli — must not call
    /// `set_radio` (a flaky read during DRM modeset would permanently kill Wi‑Fi).
    syncing_radio: Cell<bool>,
    wifi: gtk::Box,
    known_btn: gtk::Button,
    last_saved: RefCell<Vec<SavedConn>>,
    wifi_dns: gtk::Box,
    eth: gtk::Box,
    vpn: gtk::Box,
    vpn_status: gtk::Label,
    proxy: gtk::Box,
    /// Last values used to build DropDown/entry editors. Rebuilding those while a
    /// popover is open dismisses it (and wipes in-progress edits).
    last_proxy: RefCell<Option<ProxyConfig>>,
    last_active_wifi: RefCell<Option<Option<ActiveConn>>>,
    last_eth: RefCell<Option<Vec<EthDev>>>,
    last_vpn: RefCell<Option<Vec<VpnConn>>>,
}

/// Build the Network page. `initial_tab` selects Wireless / DNS / Wired / VPN /
/// Proxy (`Some("vpn")` from `--page network/vpn`).
pub fn build(initial_tab: Option<&str>) -> gtk::Widget {
    let (scroller, content) = ui::page_for("network");

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);

    let tabs = [
        ("wireless", "Wireless"),
        ("dns", "DNS"),
        ("wired", "Wired"),
        ("vpn", "VPN"),
        ("proxy", "Proxy"),
    ];
    let initial = resolve_initial_tab(&tabs, initial_tab.unwrap_or("wireless"));

    // Pill buttons can be marked active before stack children exist; the
    // visible child is applied after all `add_named` calls below.
    let (tab_bar, select_tab) = pill_tabs(&stack, &tabs, initial);
    set_tab_request_handler(select_tab);
    content.append(&tab_bar);
    content.append(&stack);

    // ---- Wireless page ----
    let wireless = page_box();
    let (wifi_card, wifi_body) = ui::section(&tr("Wi-Fi"));
    let radio = gtk::Switch::new();
    radio.set_halign(gtk::Align::End);
    radio.set_valign(gtk::Align::Center);
    let radio_row = ui::row(&tr("Wi-Fi radio"), &radio);
    let rescan = gtk::Button::with_label(&tr("Rescan"));
    radio_row.append(&rescan);
    wifi_body.append(&radio_row);
    let wifi_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    wifi_list.add_css_class("metis-settings-list");
    wifi_list.add_css_class("metis-settings-zebra-list");
    wifi_list.add_css_class("metis-settings-wifi-list");
    wifi_body.append(&wifi_list);

    let known_btn = gtk::Button::with_label(&tr("Known Wi-Fi networks…"));
    known_btn.add_css_class("metis-settings-secondary");
    known_btn.set_halign(gtk::Align::Start);
    known_btn.set_sensitive(false);
    known_btn.set_tooltip_text(Some(&tr(
        "Forget saved networks or view connection details",
    )));
    wifi_body.append(&known_btn);
    wireless.append(&wifi_card);
    stack.add_named(&wireless, Some("wireless"));

    // ---- DNS page (Wi-Fi override; Ethernet DNS lives under Wired) ----
    let dns_page = page_box();
    let (wdns_card, wdns_body) = ui::section(&tr("DNS"));
    dns_page.append(&wdns_card);
    dns_page.append(&hint(&tr(
        "Override DNS for the active Wi-Fi connection. Ethernet DNS is configured under Wired.",
    )));
    stack.add_named(&dns_page, Some("dns"));

    // ---- Wired page ----
    let wired = page_box();
    let (eth_card, eth_body) = ui::section(&tr("Ethernet"));
    wired.append(&eth_card);
    stack.add_named(&wired, Some("wired"));

    // ---- VPN page ----
    let vpn_page = page_box();
    let (vpn_card, vpn_body) = ui::section(&tr("VPN connections"));
    if !net::openvpn_plugin_present() {
        let plugin_hint = gtk::Label::new(Some(&tr(
            "OpenVPN import needs the NetworkManager plugin. Install with:\n\
             sudo apt install network-manager-openvpn",
        )));
        plugin_hint.set_xalign(0.0);
        plugin_hint.set_wrap(true);
        plugin_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        plugin_hint.set_hexpand(true);
        plugin_hint.set_width_chars(28);
        plugin_hint.set_max_width_chars(72);
        plugin_hint.add_css_class("metis-settings-error");
        plugin_hint.set_margin_bottom(8);
        vpn_body.append(&plugin_hint);
    }
    let vpn_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    vpn_actions.add_css_class("metis-settings-actions");
    vpn_actions.set_halign(gtk::Align::Start);
    let import_btn = gtk::Button::with_label(&tr("Import…"));
    import_btn.add_css_class("suggested-action");
    let add_ovpn_btn = gtk::Button::with_label(&tr("Add OpenVPN…"));
    let add_wg_btn = gtk::Button::with_label(&tr("Add WireGuard…"));
    vpn_actions.append(&import_btn);
    vpn_actions.append(&add_ovpn_btn);
    vpn_actions.append(&add_wg_btn);
    vpn_body.append(&vpn_actions);
    let vpn_status = gtk::Label::new(None);
    vpn_status.set_xalign(0.0);
    vpn_status.set_wrap(true);
    vpn_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    vpn_status.set_hexpand(true);
    vpn_status.set_width_chars(28);
    vpn_status.set_max_width_chars(72);
    vpn_status.add_css_class("metis-settings-hint");
    vpn_status.set_visible(false);
    vpn_body.append(&vpn_status);
    let vpn_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vpn_list.add_css_class("metis-settings-list");
    vpn_body.append(&vpn_list);
    vpn_page.append(&vpn_card);
    vpn_page.append(&hint(&tr(
        "Import .ovpn / WireGuard .conf files, or add OpenVPN / WireGuard manually. Saved NetworkManager VPN profiles appear here automatically."
        )));
    stack.add_named(&vpn_page, Some("vpn"));

    // ---- Proxy page ----
    let proxy_page = page_box();
    let (proxy_card, proxy_body) = ui::section(&tr("System proxy"));
    proxy_page.append(&proxy_card);
    stack.add_named(&proxy_page, Some("proxy"));
    stack.set_visible_child_name(initial);

    let sections = Rc::new(Sections {
        radio: radio.clone(),
        syncing_radio: Cell::new(false),
        wifi: wifi_list,
        known_btn: known_btn.clone(),
        last_saved: RefCell::new(Vec::new()),
        wifi_dns: wdns_body,
        eth: eth_body,
        vpn: vpn_list,
        vpn_status: vpn_status.clone(),
        proxy: proxy_body,
        last_proxy: RefCell::new(None),
        last_active_wifi: RefCell::new(None),
        last_eth: RefCell::new(None),
        last_vpn: RefCell::new(None),
    });

    // Snapshot delivery: worker thread -> mpsc -> glib poll -> render.
    let (tx, rx) = mpsc::channel::<NetSnapshot>();
    let refresh = {
        let tx = tx.clone();
        Rc::new(move || {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(net::load_snapshot());
            });
        })
    };

    {
        let sections = sections.clone();
        let refresh = refresh.clone();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            if let Ok(snap) = rx.try_recv() {
                render(&sections, &snap, &refresh);
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let refresh = refresh.clone();
        let sections = sections.clone();
        // HDMI modeset can synthesize active-notify without a click. Only a
        // real press may turn the radio off.
        let arm = gtk::GestureClick::new();
        arm.set_button(1);
        arm.set_propagation_phase(gtk::PropagationPhase::Capture);
        arm.connect_pressed(move |_, _, _, _| {
            net::arm_user_wifi_radio_toggle();
        });
        radio.add_controller(arm);

        radio.connect_active_notify(move |s| {
            if sections.syncing_radio.get() {
                return;
            }
            if !net::set_radio(s.is_active()) {
                // Unarmed off refused — keep UI matching the still-enabled radio.
                sections.syncing_radio.set(true);
                s.set_active(true);
                sections.syncing_radio.set(false);
                return;
            }
            schedule_refresh(&refresh, 1500);
        });
    }
    {
        let refresh = refresh.clone();
        rescan.connect_clicked(move |_| {
            net::set_radio(true);
            schedule_refresh(&refresh, 2500);
        });
    }
    {
        let sections = sections.clone();
        let refresh = refresh.clone();
        known_btn.connect_clicked(move |_| {
            let saved = sections.last_saved.borrow().clone();
            wireless::show_known_wifi_sheet(saved, refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        import_btn.connect_clicked(move |btn| {
            let parent = btn.root().and_downcast::<gtk::Window>();
            vpn::pick_vpn_import(parent.as_ref(), status.clone(), refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        add_ovpn_btn.connect_clicked(move |_| {
            vpn::show_openvpn_sheet(status.clone(), refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        add_wg_btn.connect_clicked(move |_| {
            vpn::show_wireguard_sheet(status.clone(), refresh.clone());
        });
    }

    // Initial load + keep in sync when the edge-bar (or nmcli) toggles VPN.
    // Skip the timer while Proxy is visible: that tab hosts a DropDown, and the
    // periodic rebuild was dismissing its popover as soon as it opened.
    refresh();
    {
        let refresh = refresh.clone();
        let stack = stack.clone();
        glib::timeout_add_local(Duration::from_secs(2), move || {
            if stack.visible_child_name().as_deref() != Some("proxy") {
                refresh();
            }
            glib::ControlFlow::Continue
        });
    }

    scroller.upcast()
}

/// A vertical content box for a stack page (matches the page's own spacing).
fn page_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.set_margin_top(8);
    b
}

fn resolve_initial_tab<'a>(tabs: &[(&'a str, &str)], requested: &'a str) -> &'a str {
    if tabs.iter().any(|(name, _)| *name == requested) {
        requested
    } else {
        tabs.first().map(|(n, _)| *n).unwrap_or("wireless")
    }
}

/// A segmented pill-tab bar that switches `stack` between named children.
/// Caller must call `stack.set_visible_child_name(initial)` after children exist.
/// Returns the bar and a live `select(tab_name)` callback for external navigation.
fn pill_tabs(stack: &gtk::Stack, tabs: &[(&str, &str)], initial: &str) -> TabBarHandler {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("metis-settings-tabs");
    bar.set_halign(gtk::Align::Center);

    let buttons: Rc<RefCell<Vec<(String, gtk::ToggleButton)>>> = Rc::new(RefCell::new(Vec::new()));
    let mut group: Option<gtk::ToggleButton> = None;
    for (name, label) in tabs {
        let btn = gtk::ToggleButton::with_label(&tr(label));
        btn.add_css_class("metis-settings-tab");
        match &group {
            Some(g) => btn.set_group(Some(g)),
            None => group = Some(btn.clone()),
        }
        if *name == initial {
            btn.set_active(true);
        }
        let stack = stack.clone();
        let tab_name = (*name).to_string();
        btn.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name(&tab_name);
            }
        });
        buttons
            .borrow_mut()
            .push(((*name).to_string(), btn.clone()));
        bar.append(&btn);
    }

    let select: Rc<dyn Fn(&str)> = {
        let buttons = buttons.clone();
        let stack = stack.clone();
        Rc::new(move |name: &str| {
            if let Some((_, btn)) = buttons.borrow().iter().find(|(n, _)| n == name) {
                btn.set_active(true);
            } else {
                stack.set_visible_child_name(name);
            }
        })
    };

    (bar, select)
}

pub(crate) fn schedule_refresh(refresh: &Rc<impl Fn() + 'static>, delay_ms: u32) {
    let refresh = refresh.clone();
    glib::timeout_add_local_once(Duration::from_millis(delay_ms as u64), move || refresh());
}

fn render<F: Fn() + 'static>(sections: &Rc<Sections>, snap: &NetSnapshot, refresh: &Rc<F>) {
    // Sync UI from nmcli without writing the radio back. Never flip the switch
    // OFF from a poll — HDMI modeset used to make nmcli report disabled and the
    // notify handler then ran `nmcli radio wifi off`.
    sections.syncing_radio.set(true);
    if snap.wifi_enabled && !sections.radio.is_active() {
        sections.radio.set_active(true);
    }
    sections.syncing_radio.set(false);

    // ---- Wi-Fi list ----
    wireless::render_wifi_list(&sections.wifi, snap, refresh);

    // ---- Known networks (button + top-slide sheet; list lives off Wireless) ----
    wireless::update_known_button(&sections.known_btn, &snap.saved, &sections.last_saved);

    // ---- Wi-Fi DNS override (DNS tab) ----
    {
        let same = sections
            .last_active_wifi
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev == &snap.active_wifi);
        if !same {
            clear(&sections.wifi_dns);
            match &snap.active_wifi {
                Some(conn) => sections
                    .wifi_dns
                    .append(&dns::dns_override_editor(&conn.name, &conn.ipv4, refresh)),
                None => sections.wifi_dns.append(&hint(&tr(
                    "Connect to a Wi-Fi network to override its DNS.",
                ))),
            }
            *sections.last_active_wifi.borrow_mut() = Some(snap.active_wifi.clone());
        }
    }

    // ---- Ethernet ----
    {
        let same = sections
            .last_eth
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev == &snap.eth);
        if !same {
            clear(&sections.eth);
            if snap.eth.is_empty() {
                sections.eth.append(&hint(&tr("No Ethernet devices.")));
            } else {
                for dev in &snap.eth {
                    sections.eth.append(&wired::ethernet_editor(dev, refresh));
                }
            }
            *sections.last_eth.borrow_mut() = Some(snap.eth.clone());
        }
    }

    // ---- VPN ----
    {
        let same = sections
            .last_vpn
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev == &snap.vpn);
        if !same {
            clear(&sections.vpn);
            if snap.vpn.is_empty() {
                sections.vpn.append(&hint(&tr(
                    "No VPN profiles yet. Import an .ovpn or WireGuard .conf, or add OpenVPN / WireGuard manually."
                    )));
            } else {
                for vpn_conn in &snap.vpn {
                    sections
                        .vpn
                        .append(&vpn::vpn_row(vpn_conn, refresh, &sections.vpn_status));
                }
            }
            *sections.last_vpn.borrow_mut() = Some(snap.vpn.clone());
        }
    }

    // ---- Proxy ----
    {
        let same = sections
            .last_proxy
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev == &snap.proxy);
        if !same {
            clear(&sections.proxy);
            sections
                .proxy
                .append(&proxy::proxy_editor(&snap.proxy, refresh));
            *sections.last_proxy.borrow_mut() = Some(snap.proxy.clone());
        }
    }
}

pub(crate) fn entry(placeholder: &str, value: &str) -> gtk::Entry {
    let e = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    e.set_text(value);
    e
}

pub(crate) fn hint(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_hexpand(true);
    // Without this, wrapped labels still report the full unwrapped string as
    // their natural/min width and lock the Settings window from shrinking.
    l.set_width_chars(28);
    l.set_max_width_chars(72);
    l.add_css_class("metis-settings-hint");
    l
}

pub(crate) fn clear(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}
