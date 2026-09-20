//! Network: a pill-tabbed page splitting Wireless (Wi-Fi scan/connect + known
//! networks sheet), DNS (Wi-Fi DNS override), Wired (per-NIC IPv4 DHCP/static +
//! DNS), VPN (NetworkManager OpenVPN / WireGuard), and Proxy (system proxy via
//! GNOME gsettings). All `nmcli`/`gsettings` work runs off the GTK main thread;
//! results arrive over an mpsc channel drained on a timeout.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::dialog;
use crate::gtk_cb::{OptFnStrRef, TabBarHandler};
use crate::net::{
    self, ActiveConn, EthDev, NetSnapshot, OpenVpnCreate, ProxyConfig, SavedConn, VpnConn, VpnKind,
    WireGuardCreate, WireGuardProfile,
};
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
            show_known_wifi_sheet(saved, refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        import_btn.connect_clicked(move |btn| {
            let parent = btn.root().and_downcast::<gtk::Window>();
            pick_vpn_import(parent.as_ref(), status.clone(), refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        add_ovpn_btn.connect_clicked(move |_| {
            show_openvpn_sheet(status.clone(), refresh.clone());
        });
    }
    {
        let refresh = refresh.clone();
        let status = vpn_status.clone();
        add_wg_btn.connect_clicked(move |_| {
            show_wireguard_sheet(status.clone(), refresh.clone());
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

fn schedule_refresh(refresh: &Rc<impl Fn() + 'static>, delay_ms: u32) {
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
    clear(&sections.wifi);
    if !snap.wifi_enabled {
        sections.wifi.append(&hint(&tr("Wi-Fi is off.")));
    } else if snap.wifi.is_empty() {
        sections.wifi.append(&hint(&tr("No networks found.")));
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
                    let net_box = sections.wifi.clone();
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
            sections.wifi.append(&row);
        }
    }

    // ---- Known networks (button + top-slide sheet; list lives off Wireless) ----
    {
        let n = snap.saved.len();
        *sections.last_saved.borrow_mut() = snap.saved.clone();
        if n == 0 {
            sections.known_btn.set_label(&tr("Known Wi-Fi networks…"));
            sections.known_btn.set_sensitive(false);
        } else {
            sections
                .known_btn
                .set_label(&tr(&format!("Known Wi-Fi networks ({n})…")));
            sections.known_btn.set_sensitive(true);
        }
    }

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
                    .append(&dns_override_editor(&conn.name, &conn.ipv4, refresh)),
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
                    sections.eth.append(&ethernet_editor(dev, refresh));
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
                for vpn in &snap.vpn {
                    sections
                        .vpn
                        .append(&vpn_row(vpn, refresh, &sections.vpn_status));
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
            sections.proxy.append(&proxy_editor(&snap.proxy, refresh));
            *sections.last_proxy.borrow_mut() = Some(snap.proxy.clone());
        }
    }
}

fn vpn_row<F: Fn() + 'static>(vpn: &VpnConn, refresh: &Rc<F>, status: &gtk::Label) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_valign(gtk::Align::Center);
    if vpn.active {
        row.add_css_class("metis-settings-row-active");
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(&vpn.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(28);
    text.append(&name);
    let mut meta = vpn.kind.label().to_string();
    if vpn.active {
        meta.push_str(" · ");
        meta.push_str(&tr("Connected"));
    }
    let kind = gtk::Label::new(Some(&meta));
    kind.set_xalign(0.0);
    kind.add_css_class("metis-settings-hint");
    text.append(&kind);

    // Autoconnect under the title keeps the action strip compact so a wide
    // VPN row cannot lock Settings' minimum window width.
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    auto_row.set_halign(gtk::Align::Start);
    auto_row.set_margin_top(2);
    let auto_lbl = gtk::Label::new(Some(&tr("Auto-connect")));
    auto_lbl.add_css_class("metis-settings-hint");
    auto_lbl.set_valign(gtk::Align::Center);
    auto_lbl.set_tooltip_text(Some(&tr(
        "Connect this VPN after login when the network is ready. Only one profile can auto-connect."
        )));
    auto_row.append(&auto_lbl);
    let auto = gtk::Switch::new();
    auto.set_active(vpn.autoconnect);
    auto.set_valign(gtk::Align::Center);
    auto.set_tooltip_text(Some(&tr(
        "Connect this VPN after login when the network is ready. Only one profile can auto-connect."
        )));
    {
        let refresh = refresh.clone();
        let status = status.clone();
        let uuid = vpn.uuid.clone();
        let was_active = vpn.active;
        auto.connect_state_set(move |sw, on| {
            let uuid_for_set = uuid.clone();
            let uuid = uuid.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let sw = sw.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_set_autoconnect(&uuid_for_set, on));
            });
            glib::timeout_add_local(Duration::from_millis(50), move || {
                match rx.try_recv() {
                    Ok(Ok(())) => {
                        let msg = if on {
                            tr("Autoconnect enabled (only this profile).")
                        } else {
                            tr("Autoconnect disabled.")
                        };
                        set_vpn_status(&status, &msg, false);
                        if on && !was_active {
                            // Connect now so the setting takes effect without
                            // waiting for the next login.
                            let uuid = uuid.clone();
                            let status = status.clone();
                            let refresh = refresh.clone();
                            set_vpn_status(&status, &tr("Connecting…"), false);
                            let (tx2, rx2) = mpsc::channel::<Result<(), String>>();
                            std::thread::spawn(move || {
                                let _ = tx2.send(net::vpn_up(&uuid));
                            });
                            glib::timeout_add_local(Duration::from_millis(50), move || {
                                match rx2.try_recv() {
                                    Ok(Ok(())) => {
                                        set_vpn_status(&status, &tr("Connected."), false);
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                    Ok(Err(e)) => {
                                        set_vpn_status(&status, &e, true);
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                }
                            });
                        } else {
                            refresh();
                        }
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        sw.set_active(!on);
                        set_vpn_status(&status, &e, true);
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        sw.set_active(!on);
                        glib::ControlFlow::Break
                    }
                }
            });
            glib::Propagation::Proceed
        });
    }
    auto_row.append(&auto);
    text.append(&auto_row);
    row.append(&text);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_valign(gtk::Align::Center);
    actions.set_halign(gtk::Align::End);
    actions.set_hexpand(false);

    if vpn.kind == VpnKind::WireGuard {
        let edit = gtk::Button::with_label(&tr("Edit"));
        edit.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            edit.connect_clicked(move |btn| {
                let parent = btn.root().and_downcast::<gtk::Window>();
                show_wireguard_edit_dialog(parent.as_ref(), &uuid, status.clone(), refresh.clone());
            });
        }
        actions.append(&edit);
    }

    if vpn.active {
        let disconnect = gtk::Button::with_label(&tr("Disconnect"));
        disconnect.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            disconnect.connect_clicked(move |btn| {
                run_vpn_toggle(false, &uuid, btn, &status, &refresh);
            });
        }
        actions.append(&disconnect);
    } else {
        let connect = gtk::Button::with_label(&tr("Connect"));
        connect.add_css_class("suggested-action");
        connect.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            connect.connect_clicked(move |btn| {
                run_vpn_toggle(true, &uuid, btn, &status, &refresh);
            });
        }
        actions.append(&connect);
    }

    let delete = gtk::Button::with_label(&tr("Delete"));
    delete.add_css_class("destructive-action");
    delete.set_valign(gtk::Align::Center);
    {
        let refresh = refresh.clone();
        let uuid = vpn.uuid.clone();
        delete.connect_clicked(move |_| {
            net::vpn_delete(&uuid);
            schedule_refresh(&refresh, 1200);
        });
    }
    actions.append(&delete);
    row.append(&actions);
    row.upcast()
}

fn run_vpn_toggle<F: Fn() + 'static>(
    connect: bool,
    uuid: &str,
    btn: &gtk::Button,
    status: &gtk::Label,
    refresh: &Rc<F>,
) {
    let busy = if connect {
        tr("Connecting…")
    } else {
        tr("Disconnecting…")
    };
    let done_label = if connect {
        tr("Connect")
    } else {
        tr("Disconnect")
    };
    btn.set_sensitive(false);
    btn.set_label(&busy);
    set_vpn_status(status, &busy, false);

    let uuid = uuid.to_string();
    let uuid_for_prompt = uuid.clone();
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        let result = if connect {
            net::vpn_up(&uuid)
        } else {
            net::vpn_down(&uuid)
        };
        let _ = tx.send(result);
    });

    let btn = btn.clone();
    let status = status.clone();
    let refresh = refresh.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
        Ok(Ok(())) => {
            let msg = if connect {
                tr("Connected.")
            } else {
                tr("Disconnected.")
            };
            set_vpn_status(&status, &msg, false);
            refresh();
            glib::ControlFlow::Break
        }
        Ok(Err(e)) => {
            btn.set_sensitive(true);
            btn.set_label(&done_label);
            if connect && net::vpn_secret_required(&e) {
                set_vpn_status(
                    &status,
                    &tr("Password required — enter your VPN password."),
                    false,
                );
                let parent = btn.root().and_downcast::<gtk::Window>();
                show_vpn_password_dialog(
                    parent.as_ref(),
                    &uuid_for_prompt,
                    status.clone(),
                    refresh.clone(),
                    btn.clone(),
                );
            } else {
                set_vpn_status(&status, &e, true);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            btn.set_sensitive(true);
            btn.set_label(&done_label);
            glib::ControlFlow::Break
        }
    });
}

fn show_vpn_password_dialog<F: Fn() + 'static>(
    parent: Option<&gtk::Window>,
    uuid: &str,
    status: gtk::Label,
    refresh: Rc<F>,
    connect_btn: gtk::Button,
) {
    let Some(parent) = parent else {
        set_vpn_status(
            &status,
            &tr("VPN password required, but no window is available to prompt."),
            true,
        );
        return;
    };

    let dialog = gtk::Window::builder()
        .title(tr("VPN password"))
        .modal(true)
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .default_width(420)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let heading = gtk::Label::new(Some(&tr("VPN password required")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    outer.append(&heading);

    let hint = gtk::Label::new(Some(&tr(
        "This profile needs a password that NetworkManager does not have yet \
         (common after moving from another desktop). Enter it to connect.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    outer.append(&hint);

    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .placeholder_text(tr("Password"))
        .build();
    outer.append(&entry);

    let remember = gtk::CheckButton::with_label(&tr("Remember password on this profile"));
    remember.set_active(true);
    outer.append(&remember);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    outer.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    btn_row.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let connect = gtk::Button::with_label(&tr("Connect"));
    connect.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&connect);
    outer.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let do_connect = {
        let dialog = dialog.clone();
        let entry = entry.clone();
        let remember = remember.clone();
        let err = err.clone();
        let status = status.clone();
        let refresh = refresh.clone();
        let connect_btn = connect_btn.clone();
        let connect = connect.clone();
        let uuid = uuid.to_string();
        Rc::new(move || {
            let password = entry.text().to_string();
            if password.trim().is_empty() {
                err.set_text(&tr("Enter the VPN password."));
                err.set_visible(true);
                return;
            }
            err.set_visible(false);
            connect.set_sensitive(false);
            connect.set_label(&tr("Connecting…"));
            connect_btn.set_sensitive(false);
            connect_btn.set_label(&tr("Connecting…"));
            set_vpn_status(&status, &tr("Connecting…"), false);

            let remember_on = remember.is_active();
            let uuid = uuid.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_up_with_password(&uuid, &password, remember_on));
            });

            let dialog = dialog.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let connect = connect.clone();
            let connect_btn = connect_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("Connected."), false);
                    refresh();
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    connect.set_sensitive(true);
                    connect.set_label(&tr("Connect"));
                    connect_btn.set_sensitive(true);
                    connect_btn.set_label(&tr("Connect"));
                    set_vpn_status(&status, &e, true);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    connect.set_sensitive(true);
                    connect.set_label(&tr("Connect"));
                    connect_btn.set_sensitive(true);
                    connect_btn.set_label(&tr("Connect"));
                    glib::ControlFlow::Break
                }
            });
        })
    };

    {
        let do_connect = do_connect.clone();
        connect.connect_clicked(move |_| do_connect());
    }
    {
        let do_connect = do_connect.clone();
        entry.connect_activate(move |_| do_connect());
    }

    dialog.present();
    let focus = entry.clone();
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

fn set_vpn_status(label: &gtk::Label, msg: &str, error: bool) {
    label.set_text(msg);
    label.set_visible(!msg.is_empty());
    if error {
        label.add_css_class("metis-settings-error");
        label.remove_css_class("metis-settings-hint");
    } else {
        label.remove_css_class("metis-settings-error");
        label.add_css_class("metis-settings-hint");
    }
}

fn pick_vpn_import(
    parent: Option<&gtk::Window>,
    status: gtk::Label,
    refresh: Rc<impl Fn() + 'static>,
) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title(&tr("Import VPN profile"));
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("VPN configs (*.ovpn, *.conf)")));
    filter.add_pattern("*.ovpn");
    filter.add_pattern("*.OVPN");
    filter.add_pattern("*.conf");
    filter.add_pattern("*.CONF");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    dialog.open(parent, gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let path_s = path.to_string_lossy().to_string();
        set_vpn_status(&status, &tr("Importing…"), false);
        // Brief main-thread wait — import is a single nmcli call.
        match import_vpn_file(&path_s) {
            Ok(name) => {
                set_vpn_status(&status, &format!("Imported “{name}”."), false);
                refresh();
            }
            Err(err) => set_vpn_status(&status, &err, true),
        }
    });
}

fn import_vpn_file(path: &str) -> Result<String, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ovpn" => net::vpn_import_openvpn(path),
        "conf" => net::vpn_import_wireguard(path),
        _ => {
            // Try OpenVPN first, then WireGuard.
            net::vpn_import_openvpn(path).or_else(|_| net::vpn_import_wireguard(path))
        }
    }
}

fn show_openvpn_sheet(status: gtk::Label, refresh: Rc<impl Fn() + 'static>) {
    if dialog::is_open() {
        return;
    }
    if !net::openvpn_plugin_present() {
        set_vpn_status(
            &status,
            &tr("OpenVPN plugin missing. Install with: sudo apt install network-manager-openvpn"),
            true,
        );
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let hint = gtk::Label::new(Some(&tr(
        "Password authentication only. For provider configs with certificates, use Import… on an .ovpn file.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-top-dialog-body");
    wrap.append(&hint);

    let name = wg_entry("Work VPN", "");
    let gateway = wg_entry("vpn.example.com", "");
    let username = wg_entry("username", "");
    let password = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .placeholder_text(tr("Password (optional)"))
        .build();
    let ca = wg_entry("/path/to/ca.crt", "");

    wrap.append(&wg_field(&tr("Name"), &name));
    wrap.append(&wg_field(&tr("Gateway"), &gateway));
    wrap.append(&wg_field(&tr("Username"), &username));

    let pw_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let pw_lbl = gtk::Label::new(Some(&tr("Password (optional)")));
    pw_lbl.set_xalign(0.0);
    pw_lbl.add_css_class("metis-settings-hint");
    pw_box.append(&pw_lbl);
    pw_box.append(&password);
    wrap.append(&pw_box);

    let ca_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    ca.set_hexpand(true);
    let browse = gtk::Button::with_label(&tr("Browse…"));
    ca_row.append(&ca);
    ca_row.append(&browse);
    let ca_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let ca_lbl = gtk::Label::new(Some(&tr("CA certificate (optional)")));
    ca_lbl.set_xalign(0.0);
    ca_lbl.add_css_class("metis-settings-hint");
    ca_box.append(&ca_lbl);
    ca_box.append(&ca_row);
    wrap.append(&ca_box);

    let remember = gtk::CheckButton::with_label(&tr("Remember password on this profile"));
    remember.set_active(true);
    wrap.append(&remember);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    // Header Cancel dismisses — only Create in the body.
    let create = gtk::Button::with_label(&tr("Create"));
    create.add_css_class("suggested-action");
    btn_row.append(&create);
    wrap.append(&btn_row);

    {
        let ca = ca.clone();
        browse.connect_clicked(move |btn| {
            let picker = gtk::FileDialog::new();
            picker.set_title(&tr("Choose CA certificate"));
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&tr("Certificates (*.crt, *.pem)")));
            filter.add_pattern("*.crt");
            filter.add_pattern("*.pem");
            filter.add_pattern("*.CER");
            filter.add_pattern("*.CRT");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            picker.set_filters(Some(&filters));
            let parent = btn.root().and_downcast::<gtk::Window>();
            let ca = ca.clone();
            picker.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                if let Some(path) = file.path() {
                    ca.set_text(&path.to_string_lossy());
                }
            });
        });
    }
    {
        let status = status.clone();
        let create_btn = create.clone();
        let name = name.clone();
        let gateway = gateway.clone();
        let username = username.clone();
        let password = password.clone();
        let ca = ca.clone();
        let remember = remember.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        create.connect_clicked(move |_| {
            create_btn.set_sensitive(false);
            create_btn.set_label(&tr("Creating…"));
            err.set_visible(false);

            let cfg = OpenVpnCreate {
                name: name.text().to_string(),
                gateway: gateway.text().to_string(),
                username: username.text().to_string(),
                password: password.text().to_string(),
                ca_path: ca.text().to_string(),
                remember_password: remember.is_active(),
            };
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_create_openvpn(cfg));
            });

            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let create_btn = create_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("OpenVPN profile created."), false);
                    refresh();
                    dialog::dismiss_silent();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Continue
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    if !dialog::present(&tr("Add OpenVPN"), &wrap, Rc::new(|| {})) {
        set_vpn_status(&status, &tr("Could not open OpenVPN sheet."), true);
        return;
    }
    let focus = name.clone();
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

fn show_wireguard_sheet(status: gtk::Label, refresh: Rc<impl Fn() + 'static>) {
    if dialog::is_open() {
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let name = wg_entry("Home VPN", "");
    let private_key = wg_entry("Interface private key", "");
    let address = wg_entry("10.0.0.2/32", "");
    let peer_pub = wg_entry("Peer public key", "");
    let endpoint = wg_entry("vpn.example.com:51820", "");
    let allowed = wg_entry("0.0.0.0/0, ::/0", "0.0.0.0/0, ::/0");
    let dns = wg_entry("1.1.1.1", "");

    wrap.append(&wg_field(&tr("Name"), &name));
    wrap.append(&wg_field(&tr("Private key"), &private_key));
    wrap.append(&wg_field(&tr("Address (CIDR)"), &address));
    wrap.append(&wg_field(&tr("Peer public key"), &peer_pub));
    wrap.append(&wg_field(&tr("Endpoint"), &endpoint));
    wrap.append(&wg_field(&tr("Allowed IPs"), &allowed));
    wrap.append(&wg_field(&tr("DNS (optional)"), &dns));

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    let create = gtk::Button::with_label(&tr("Create"));
    create.add_css_class("suggested-action");
    btn_row.append(&create);
    wrap.append(&btn_row);

    {
        let status = status.clone();
        let create_btn = create.clone();
        let name = name.clone();
        let private_key = private_key.clone();
        let address = address.clone();
        let peer_pub = peer_pub.clone();
        let endpoint = endpoint.clone();
        let allowed = allowed.clone();
        let dns = dns.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        create.connect_clicked(move |_| {
            create_btn.set_sensitive(false);
            create_btn.set_label(&tr("Creating…"));
            err.set_visible(false);

            let cfg = WireGuardCreate {
                name: name.text().to_string(),
                private_key: private_key.text().to_string(),
                address: address.text().to_string(),
                peer_public_key: peer_pub.text().to_string(),
                endpoint: endpoint.text().to_string(),
                allowed_ips: allowed.text().to_string(),
                dns: dns.text().to_string(),
            };

            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_create_wireguard(cfg));
            });

            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let create_btn = create_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("WireGuard connection created."), false);
                    refresh();
                    dialog::dismiss_silent();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Continue
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    if !dialog::present(&tr("Add WireGuard"), &wrap, Rc::new(|| {})) {
        set_vpn_status(&status, &tr("Could not open WireGuard sheet."), true);
        return;
    }
    let focus_entry = name.clone();
    glib::idle_add_local_once(move || {
        focus_entry.grab_focus();
    });
}

fn show_wireguard_edit_dialog(
    parent: Option<&gtk::Window>,
    uuid: &str,
    status: gtk::Label,
    refresh: Rc<impl Fn() + 'static>,
) {
    let Some(parent) = parent else {
        set_vpn_status(
            &status,
            &tr("Could not open edit dialog (no parent window)."),
            true,
        );
        return;
    };
    let Some(profile) = net::vpn_get_wireguard(uuid) else {
        set_vpn_status(
            &status,
            &tr("Could not load WireGuard profile details."),
            true,
        );
        return;
    };

    let dialog = gtk::Window::builder()
        .title(tr("Edit WireGuard"))
        .modal(true)
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .default_width(480)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let heading = gtk::Label::new(Some(&tr("Edit WireGuard connection")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    outer.append(&heading);

    let name = wg_entry("Home VPN", &profile.name);
    let address = wg_entry("10.0.0.2/32", &profile.address);
    let peer_pub = wg_entry("Peer public key", &profile.peer_public_key);
    let endpoint = wg_entry("vpn.example.com:51820", &profile.endpoint);
    let allowed = wg_entry(
        "0.0.0.0/0, ::/0",
        if profile.allowed_ips.is_empty() {
            "0.0.0.0/0, ::/0"
        } else {
            &profile.allowed_ips
        },
    );
    let dns = wg_entry("1.1.1.1", &profile.dns);

    outer.append(&wg_field(&tr("Name"), &name));
    outer.append(&wg_field(&tr("Address (CIDR)"), &address));
    outer.append(&wg_field(&tr("Peer public key"), &peer_pub));
    outer.append(&wg_field(&tr("Endpoint"), &endpoint));
    outer.append(&wg_field(&tr("Allowed IPs"), &allowed));
    outer.append(&wg_field(&tr("DNS (optional)"), &dns));

    let hint = gtk::Label::new(Some(&tr(
        "Private key is unchanged. Disconnect and reconnect for peer/address edits to take effect.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    outer.append(&hint);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    outer.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    btn_row.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let save = gtk::Button::with_label(&tr("Save"));
    save.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&save);
    outer.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let status = status.clone();
        let save_btn = save.clone();
        let uuid = uuid.to_string();
        let name = name.clone();
        let address = address.clone();
        let peer_pub = peer_pub.clone();
        let endpoint = endpoint.clone();
        let allowed = allowed.clone();
        let dns = dns.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        save.connect_clicked(move |_| {
            save_btn.set_sensitive(false);
            save_btn.set_label(&tr("Saving…"));
            err.set_visible(false);

            let cfg = WireGuardProfile {
                name: name.text().to_string(),
                address: address.text().to_string(),
                peer_public_key: peer_pub.text().to_string(),
                endpoint: endpoint.text().to_string(),
                allowed_ips: allowed.text().to_string(),
                dns: dns.text().to_string(),
            };
            let uuid = uuid.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_update_wireguard(&uuid, cfg));
            });

            let dialog = dialog.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let save_btn = save_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("WireGuard profile updated."), false);
                    refresh();
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    save_btn.set_sensitive(true);
                    save_btn.set_label(&tr("Save"));
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    save_btn.set_sensitive(true);
                    save_btn.set_label(&tr("Save"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    dialog.present();
    let focus_entry = name.clone();
    glib::idle_add_local_once(move || {
        focus_entry.grab_focus();
    });
}

fn wg_entry(placeholder: &str, value: &str) -> gtk::Entry {
    let e = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    e.set_text(value);
    // Held Backspace on an empty field must not bubble to the sidebar search.
    ui::swallow_empty_backspace(&e);
    e
}

fn wg_field(label: &str, entry: &gtk::Entry) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.add_css_class("metis-settings-hint");
    box_.append(&lbl);
    box_.append(entry);
    box_
}

/// Top-slide sheet to forget saved Wi-Fi profiles or view connection info.
fn show_known_wifi_sheet<F: Fn() + 'static>(saved: Vec<SavedConn>, refresh: Rc<F>) {
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

/// A standalone DNS-override editor for a connection (DNS tab):
/// a comma-separated DNS list applied with `ignore-auto-dns` so it overrides DHCP.
fn dns_override_editor<F: Fn() + 'static>(
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

fn ethernet_editor<F: Fn() + 'static>(dev: &net::EthDev, refresh: &Rc<F>) -> gtk::Widget {
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

fn proxy_editor<F: Fn() + 'static>(cfg: &net::ProxyConfig, refresh: &Rc<F>) -> gtk::Widget {
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

fn entry(placeholder: &str, value: &str) -> gtk::Entry {
    let e = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    e.set_text(value);
    e
}

fn hint(text: &str) -> gtk::Label {
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

fn clear(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}
