use gtk::prelude::*;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::gtk_cb::OptFn0Cell;
use crate::ui::icons::{self, names};

use crate::services::{
    BluetoothDevice, BluetoothStatus, EthernetStatus, VpnFeedback, VpnStatus, WifiNetwork,
};

pub struct BatteryWidget {
    root: gtk::Button,
    icon: gtk::Image,
    label: gtk::Label,
    last_percent: std::cell::Cell<Option<u8>>,
    last_charging: std::cell::Cell<bool>,
}

impl BatteryWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-battery");
        root.add_css_class("metis-bar-sys-icon");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let icon = icons::image(names::battery(100, false));
        let label = gtk::Label::new(None);
        label.add_css_class("metis-bar-battery-label");
        row.append(&icon);
        row.append(&label);
        root.set_child(Some(&row));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(8);
        let status = gtk::Label::new(Some(&metis_i18n::tr("Battery")));
        status.set_xalign(0.0);
        status.add_css_class("metis-bar-section-title");
        panel.append(&status);
        let settings_btn = gtk::Button::with_label(&metis_i18n::tr("Power settings…"));
        settings_btn.connect_clicked(|_| {
            if let Err(err) = crate::compositor::launch_program("metis-settings --page power") {
                tracing::warn!(%err, "failed to open power settings");
            }
            super::super::dropdown::request_close_all();
        });
        panel.append(&settings_btn);
        super::super::dropdown::wire_toggle_prepare(&root, &panel, || {});

        Self {
            root,
            icon,
            label,
            last_percent: std::cell::Cell::new(None),
            last_charging: std::cell::Cell::new(false),
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, percent: Option<u8>, charging: bool) {
        if self.last_percent.get() == percent && self.last_charging.get() == charging {
            return;
        }
        self.last_percent.set(percent);
        self.last_charging.set(charging);
        if let Some(pct) = percent {
            self.label.set_text(&format!("{pct}%"));
            self.label.set_visible(true);
            self.root.set_tooltip_text(Some(
                &metis_i18n::tr("Battery %1%").replace("%1", &pct.to_string()),
            ));
        } else {
            self.label.set_visible(false);
            self.root
                .set_tooltip_text(Some(&metis_i18n::tr("AC power")));
        }
        icons::set_icon(&self.icon, names::battery(percent.unwrap_or(100), charging));
    }
}

pub struct BluetoothWidget {
    root: gtk::Button,
    icon: gtk::Image,
    power_switch: gtk::Switch,
    updating_switch: Rc<Cell<bool>>,
    suppress_until: Rc<Cell<Instant>>,
    /// Status line shown when no devices are connected ("On" / "Off").
    status_label: gtk::Label,
    /// Container the connected-device rows are rebuilt into on each update.
    device_list: gtk::Box,
    last_status: RefCell<Option<BluetoothStatus>>,
}

impl BluetoothWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-bluetooth");
        root.add_css_class("metis-bar-sys-icon");
        root.set_visible(false);

        let icon = icons::image(names::bluetooth(false, false));
        root.set_child(Some(&icon));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(8);
        panel.set_width_request(260);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::builder()
            .label(metis_i18n::tr("Bluetooth"))
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        title.add_css_class("metis-bar-section-title");
        header.append(&title);

        let updating_switch = Rc::new(Cell::new(false));
        let suppress_until = Rc::new(Cell::new(Instant::now()));
        let power_switch = gtk::Switch::new();
        power_switch.set_valign(gtk::Align::Center);
        power_switch.add_css_class("metis-net-switch");
        {
            let updating_switch = updating_switch.clone();
            let suppress_until = suppress_until.clone();
            let icon = icon.clone();
            let root = root.clone();
            power_switch.connect_state_set(move |_, state| {
                if !updating_switch.get() {
                    bump_suppress(&suppress_until);
                    crate::services::bluetooth_set_powered(state);
                    icons::set_icon(&icon, names::bluetooth(state, false));
                    root.set_tooltip_text(Some(&if state {
                        metis_i18n::tr("Bluetooth on")
                    } else {
                        metis_i18n::tr("Bluetooth off")
                    }));
                }
                glib::Propagation::Proceed
            });
        }
        header.append(&power_switch);
        panel.append(&header);

        let status_label = gtk::Label::new(Some(&metis_i18n::tr("Off")));
        status_label.set_xalign(0.0);
        status_label.add_css_class("metis-net-status");
        panel.append(&status_label);

        let device_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        device_list.add_css_class("metis-bt-device-list");
        panel.append(&device_list);

        let settings_btn = gtk::Button::with_label(&metis_i18n::tr("Bluetooth settings…"));
        settings_btn.connect_clicked(|_| {
            if let Err(err) = crate::compositor::launch_program("metis-settings --page bluetooth") {
                tracing::warn!(%err, "failed to open bluetooth settings");
            }
            super::super::dropdown::request_close_all();
        });
        panel.append(&settings_btn);
        super::super::dropdown::wire_toggle_prepare(&root, &panel, || {});

        Self {
            root,
            icon,
            power_switch,
            updating_switch,
            suppress_until,
            status_label,
            device_list,
            last_status: RefCell::new(None),
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, status: &BluetoothStatus) {
        self.root.set_visible(status.adapter_present);
        if !status.adapter_present {
            return;
        }

        let suppressed = Instant::now() < self.suppress_until.get();
        if !suppressed {
            self.updating_switch.set(true);
            self.power_switch.set_active(status.powered);
            self.updating_switch.set(false);
        }

        if self.last_status.borrow().as_ref() == Some(status) {
            return;
        }
        self.last_status.replace(Some(status.clone()));

        if !suppressed {
            icons::set_icon(
                &self.icon,
                names::bluetooth(status.powered, status.connected),
            );

            let tip = if !status.powered {
                metis_i18n::tr("Bluetooth off")
            } else if let Some(name) = &status.device_name {
                metis_i18n::tr("Connected to %1").replace("%1", name)
            } else {
                metis_i18n::tr("Bluetooth on")
            };
            self.root.set_tooltip_text(Some(&tip));
        }

        self.rebuild_devices(status);
    }

    fn rebuild_devices(&self, status: &BluetoothStatus) {
        while let Some(child) = self.device_list.first_child() {
            self.device_list.remove(&child);
        }

        if !status.powered {
            self.status_label.set_text(&metis_i18n::tr("Off"));
            self.status_label.set_visible(true);
            self.device_list.set_visible(false);
            return;
        }
        if status.devices.is_empty() {
            self.status_label
                .set_text(&metis_i18n::tr("On — no devices connected"));
            self.status_label.set_visible(true);
            self.device_list.set_visible(false);
            return;
        }

        self.status_label.set_visible(false);
        self.device_list.set_visible(true);
        for dev in &status.devices {
            self.device_list.append(&build_bt_device_row(dev));
        }
    }
}

fn build_bt_device_row(dev: &BluetoothDevice) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("metis-bt-device-row");

    let name = gtk::Label::new(Some(&dev.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&name);

    if let Some(pct) = dev.battery_percent {
        let charging = dev.battery_charging == Some(true);
        // A charging device isn't "low" even at a low percentage.
        let low = pct <= 20 && !charging;
        let batt_icon = icons::image(names::battery(pct, charging));
        batt_icon.add_css_class("metis-bt-battery-icon");
        if low {
            batt_icon.add_css_class("metis-bt-battery-low");
        }
        row.append(&batt_icon);
        let text = if charging {
            metis_i18n::tr("%1% (charging)").replace("%1", &pct.to_string())
        } else {
            format!("{pct}%")
        };
        let batt = gtk::Label::new(Some(&text));
        batt.add_css_class("metis-bt-battery-label");
        if low {
            batt.add_css_class("metis-bt-battery-low");
        }
        row.append(&batt);
    }
    row
}

/// Shared state + interactive widgets for the network popover so row/button
/// closures can re-render the Wi-Fi list and drive the connect flow.
struct NetInner {
    list: gtk::Box,
    status_label: gtk::Label,
    connect_box: gtk::Box,
    connect_title: gtk::Label,
    password_entry: gtk::Entry,
    selected_ssid: RefCell<Option<String>>,
    /// SSID we just asked to connect to, with the time of the request (for the
    /// in-row spinner). Cleared once that network reports active or it times out.
    pending: RefCell<Option<(String, Instant)>>,
    last_sig: RefCell<String>,
    wifi: RefCell<Vec<WifiNetwork>>,
    wifi_enabled: Cell<bool>,
}

impl NetInner {
    fn rebuild_list(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if !self.wifi_enabled.get() {
            self.status_label.set_text(&metis_i18n::tr("Wi-Fi is off"));
            self.status_label.set_visible(true);
            return;
        }
        let wifi = self.wifi.borrow();
        if wifi.is_empty() {
            self.status_label
                .set_text(&metis_i18n::tr("No networks found"));
            self.status_label.set_visible(true);
            return;
        }
        self.status_label.set_visible(false);
        let pending = self.pending.borrow().as_ref().map(|(s, _)| s.clone());
        for net in wifi.iter() {
            let row = self.build_row(net, pending.as_deref());
            self.list.append(&row);
        }
    }

    fn build_row(self: &Rc<Self>, net: &WifiNetwork, pending: Option<&str>) -> gtk::Button {
        let row = gtk::Button::builder().has_frame(false).build();
        row.add_css_class("metis-net-row");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        let signal = icons::image(wifi_signal_icon(net.signal));
        hbox.append(&signal);

        let ssid = gtk::Label::new(Some(&net.ssid));
        ssid.set_halign(gtk::Align::Start);
        ssid.set_hexpand(true);
        ssid.set_ellipsize(gtk::pango::EllipsizeMode::End);
        ssid.set_max_width_chars(22);
        hbox.append(&ssid);

        if net.secured {
            let lock = icons::image("network-wireless-encrypted-symbolic");
            lock.add_css_class("metis-net-lock");
            hbox.append(&lock);
        }

        if pending == Some(net.ssid.as_str()) && !net.active {
            let spinner = gtk::Spinner::new();
            spinner.start();
            hbox.append(&spinner);
        } else if net.active {
            let check = icons::image("object-select-symbolic");
            check.add_css_class("metis-net-active");
            hbox.append(&check);
        }

        row.set_child(Some(&hbox));

        let inner = self.clone();
        let net = net.clone();
        row.connect_clicked(move |_| inner.on_row_clicked(&net));
        row
    }

    fn on_row_clicked(self: &Rc<Self>, net: &WifiNetwork) {
        if net.active {
            return;
        }
        if net.secured {
            self.selected_ssid.replace(Some(net.ssid.clone()));
            self.connect_title
                .set_text(&metis_i18n::tr("Connect to %1").replace("%1", &net.ssid));
            self.password_entry.set_text("");
            self.connect_box.set_visible(true);
            self.password_entry.grab_focus();
        } else {
            crate::services::wifi_connect(net.ssid.clone(), None);
            self.pending
                .replace(Some((net.ssid.clone(), Instant::now())));
            self.connect_box.set_visible(false);
            self.rebuild_list();
        }
    }

    fn submit_connect(self: &Rc<Self>) {
        let Some(ssid) = self.selected_ssid.borrow().clone() else {
            return;
        };
        let password = self.password_entry.text().to_string();
        crate::services::wifi_connect(ssid.clone(), Some(password));
        self.pending.replace(Some((ssid, Instant::now())));
        self.connect_box.set_visible(false);
        self.password_entry.set_text("");
        self.rebuild_list();
    }
}

pub struct NetworkWidget {
    root: gtk::Button,
    icon: gtk::Image,
    eth_row: gtk::Box,
    eth_icon: gtk::Image,
    eth_label: gtk::Label,
    wifi_switch: gtk::Switch,
    wifi_refresh: gtk::Button,
    wifi_scroll: gtk::ScrolledWindow,
    /// Mirrored from the poller — drives Wi-Fi chrome visibility.
    wifi_present: Rc<Cell<bool>>,
    updating_switch: Rc<Cell<bool>>,
    /// Same pattern as Bluetooth: ignore poller echo after the user toggles.
    suppress_until: Rc<Cell<Instant>>,
    /// Require two consecutive "off" poll readings before reflecting off in UI,
    /// so a single flaky nmcli read during HDMI modeset cannot flip the switch.
    wifi_off_streak: Cell<u32>,
    inner: Rc<NetInner>,
}

impl NetworkWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-network");
        root.add_css_class("metis-bar-sys-icon");

        let icon = icons::image(names::network(true));
        root.set_child(Some(&icon));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(10);
        panel.set_width_request(300);

        // ---- Header: title, Wi-Fi radio toggle, refresh ----
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::builder()
            .label(metis_i18n::tr("Network"))
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        title.add_css_class("metis-bar-section-title");
        header.append(&title);

        let updating_switch = Rc::new(Cell::new(false));
        let suppress_until = Rc::new(Cell::new(Instant::now()));
        let wifi_switch = gtk::Switch::new();
        wifi_switch.set_valign(gtk::Align::Center);
        wifi_switch.add_css_class("metis-net-switch");
        {
            let updating_switch = updating_switch.clone();
            let suppress_until = suppress_until.clone();
            // Only a real press may turn Wi-Fi off — HDMI modeset synthesizes
            // state-set/active notifies without a click.
            let arm = gtk::GestureClick::new();
            arm.set_button(1);
            arm.set_propagation_phase(gtk::PropagationPhase::Capture);
            arm.connect_pressed(move |_, _, _, _| {
                crate::services::arm_user_wifi_radio_toggle();
            });
            wifi_switch.add_controller(arm);

            wifi_switch.connect_state_set(move |_, state| {
                if updating_switch.get() {
                    return glib::Propagation::Proceed;
                }
                if !state && crate::services::wifi_hotplug_suppressed() {
                    return glib::Propagation::Stop;
                }
                if !crate::services::wifi_set_radio(state) {
                    // Unarmed off — refuse the visual toggle too.
                    return glib::Propagation::Stop;
                }
                bump_suppress(&suppress_until);
                glib::Propagation::Proceed
            });
        }
        header.append(&wifi_switch);

        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_valign(gtk::Align::Center);
        refresh.add_css_class("metis-net-refresh");
        refresh.connect_clicked(|_| crate::services::wifi_scan());
        header.append(&refresh);
        panel.append(&header);

        // ---- Ethernet status row (read-only) ----
        let eth_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        eth_row.add_css_class("metis-net-eth-row");
        let eth_icon = icons::image("network-wired-symbolic");
        eth_row.append(&eth_icon);
        let eth_label = gtk::Label::new(Some(&metis_i18n::tr("Ethernet")));
        eth_label.set_halign(gtk::Align::Start);
        eth_label.set_hexpand(true);
        eth_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        eth_row.append(&eth_label);
        eth_row.set_visible(false);
        panel.append(&eth_row);

        // ---- Wi-Fi list (scrollable) ----
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(240)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        scroll.add_css_class("metis-net-scroll");
        panel.append(&scroll);
        let wifi_present = Rc::new(Cell::new(true));

        let status_label = gtk::Label::new(Some(&metis_i18n::tr("Scanning…")));
        status_label.add_css_class("metis-net-status");
        status_label.set_halign(gtk::Align::Start);
        panel.append(&status_label);

        // ---- Inline connect area (shared password entry for secured nets) ----
        // A plain, visibility-toggled Box (not a Revealer): the entry stays
        // realized so a synchronous grab_focus() lands and the OnDemand layer
        // surface actually receives keyboard input — same proven pattern as the
        // clock world-clock picker. A Revealer's child isn't focusable mid-
        // animation, so focus (and typing) silently failed.
        let connect_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        connect_box.add_css_class("metis-net-connect");
        connect_box.set_visible(false);
        let connect_title = gtk::Label::new(Some(""));
        connect_title.set_halign(gtk::Align::Start);
        connect_title.add_css_class("metis-net-connect-title");
        connect_box.append(&connect_title);
        let password_entry = gtk::Entry::builder()
            .visibility(false)
            .placeholder_text(metis_i18n::tr("Password"))
            .build();
        password_entry.add_css_class("metis-net-password");
        connect_box.append(&password_entry);
        let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_row.set_halign(gtk::Align::End);
        let cancel_btn = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
        cancel_btn.add_css_class("metis-net-cancel");
        let connect_btn = gtk::Button::with_label(&metis_i18n::tr("Connect"));
        connect_btn.add_css_class("metis-net-connect-btn");
        btn_row.append(&cancel_btn);
        btn_row.append(&connect_btn);
        connect_box.append(&btn_row);
        panel.append(&connect_box);

        // ---- Footer: open the full Network settings page ----
        let settings_btn = gtk::Button::with_label(&metis_i18n::tr("Network Settings…"));
        settings_btn.add_css_class("metis-net-settings-btn");
        settings_btn.set_halign(gtk::Align::Start);
        settings_btn.connect_clicked(|_| {
            if let Err(err) = crate::compositor::launch_program("metis-settings --page network") {
                tracing::warn!(%err, "failed to launch network settings");
            }
            super::super::dropdown::request_close_all();
        });
        panel.append(&settings_btn);

        let inner = Rc::new(NetInner {
            list,
            status_label,
            connect_box: connect_box.clone(),
            connect_title,
            password_entry: password_entry.clone(),
            selected_ssid: RefCell::new(None),
            pending: RefCell::new(None),
            last_sig: RefCell::new(String::new()),
            wifi: RefCell::new(Vec::new()),
            wifi_enabled: Cell::new(true),
        });

        {
            let inner = inner.clone();
            connect_btn.connect_clicked(move |_| inner.submit_connect());
        }
        {
            let inner = inner.clone();
            password_entry.connect_activate(move |_| inner.submit_connect());
        }
        {
            let inner = inner.clone();
            cancel_btn.connect_clicked(move |_| {
                inner.selected_ssid.replace(None);
                inner.connect_box.set_visible(false);
            });
        }

        // Trigger a scan whenever the popover opens (Wi-Fi hardware only).
        {
            let wifi_present = wifi_present.clone();
            super::super::dropdown::wire_toggle_prepare(&root, &panel, move || {
                if wifi_present.get() {
                    crate::services::wifi_scan();
                }
            });
        }

        Self {
            root,
            icon,
            eth_row,
            eth_icon,
            eth_label,
            wifi_switch,
            wifi_refresh: refresh,
            wifi_scroll: scroll,
            wifi_present,
            updating_switch,
            suppress_until,
            wifi_off_streak: Cell::new(0),
            inner,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(
        &self,
        eth: &EthernetStatus,
        wifi: &[WifiNetwork],
        wifi_present: bool,
        wifi_enabled: bool,
    ) {
        self.wifi_present.set(wifi_present);

        let hotplug = crate::services::wifi_hotplug_suppressed();
        let mut wifi_enabled = wifi_enabled;
        if wifi_present {
            if wifi_enabled {
                self.wifi_off_streak.set(0);
            } else {
                let streak = self.wifi_off_streak.get().saturating_add(1);
                self.wifi_off_streak.set(streak);
                // Ignore transient "off" during/after HDMI modeset. Need a long
                // consistent streak before the switch is allowed to show off.
                if hotplug || streak < 6 {
                    wifi_enabled = true;
                }
            }
        } else {
            wifi_enabled = false;
            self.wifi_off_streak.set(0);
        }

        icons::set_icon(&self.icon, bar_icon(eth, wifi, wifi_present, wifi_enabled));
        self.root.set_tooltip_text(Some(&network_tooltip(
            eth,
            wifi,
            wifi_present,
            wifi_enabled,
        )));

        self.eth_row.set_visible(eth.present);
        if eth.present {
            icons::set_icon(
                &self.eth_icon,
                if eth.connected {
                    "network-wired-symbolic"
                } else {
                    "network-wired-disconnected-symbolic"
                },
            );
            self.eth_label.set_text(&eth.label);
        }

        // Hide Wi-Fi chrome when no adapter (wired-only machines).
        self.wifi_switch.set_visible(wifi_present);
        self.wifi_refresh.set_visible(wifi_present);
        self.wifi_scroll.set_visible(wifi_present);
        self.inner.status_label.set_visible(wifi_present);
        if !wifi_present {
            self.inner.connect_box.set_visible(false);
        }

        if wifi_present {
            let suppressed = Instant::now() < self.suppress_until.get() || hotplug;
            if !suppressed {
                self.updating_switch.set(true);
                // Poll may turn the switch ON to match NM — never OFF (only the user can).
                if wifi_enabled && !self.wifi_switch.is_active() {
                    self.wifi_switch.set_active(true);
                }
                let updating = self.updating_switch.clone();
                glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    updating.set(false);
                });
            }
        }

        // Clear a stale "connecting" spinner once the target is active (or it
        // has been pending too long).
        {
            let mut pending = self.inner.pending.borrow_mut();
            if let Some((ssid, started)) = pending.clone() {
                let connected_now = wifi.iter().any(|n| n.ssid == ssid && n.active);
                if connected_now || started.elapsed() > Duration::from_secs(30) {
                    *pending = None;
                }
            }
        }

        *self.inner.wifi.borrow_mut() = wifi.to_vec();
        self.inner.wifi_enabled.set(wifi_enabled);

        let sig = network_signature(
            wifi,
            wifi_present,
            wifi_enabled,
            &self.inner.pending.borrow(),
        );
        if *self.inner.last_sig.borrow() != sig {
            *self.inner.last_sig.borrow_mut() = sig;
            if wifi_present {
                self.inner.rebuild_list();
            } else {
                while let Some(child) = self.inner.list.first_child() {
                    self.inner.list.remove(&child);
                }
            }
        }
    }
}

/// Dedicated VPN / WireGuard indicator with its own connect/disconnect popover.
pub struct VpnWidget {
    root: gtk::Button,
    icon: gtk::Image,
    spinner: gtk::Spinner,
    check: gtk::Image,
    list: gtk::Box,
    empty_label: gtk::Label,
    status_label: gtk::Label,
    password_box: gtk::Box,
    password_title: gtk::Label,
    password_entry: gtk::PasswordEntry,
    #[allow(dead_code)] // kept for lifetime — GTK widget in VPN password row
    remember: gtk::CheckButton,
    password_target: Rc<RefCell<Option<String>>>,
    last_sig: RefCell<String>,
}

impl VpnWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-vpn");
        root.add_css_class("metis-bar-sys-icon");

        let face = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        face.set_halign(gtk::Align::Center);
        face.set_valign(gtk::Align::Center);

        let icon = icons::image("network-vpn-symbolic");
        face.append(&icon);

        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        face.append(&spinner);

        let check = icons::image("object-select-symbolic");
        check.add_css_class("metis-bar-vpn-active");
        check.set_visible(false);
        face.append(&check);

        root.set_child(Some(&face));
        root.set_tooltip_text(Some(&metis_i18n::tr("VPN")));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(10);
        panel.set_width_request(280);

        let title = gtk::Label::builder()
            .label(metis_i18n::tr("VPN"))
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("metis-bar-section-title");
        panel.append(&title);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        panel.append(&list);

        let empty_label = gtk::Label::new(Some(&metis_i18n::tr(
            "No VPN profiles yet. Import OpenVPN or WireGuard in Settings.",
        )));
        empty_label.set_wrap(true);
        empty_label.set_xalign(0.0);
        empty_label.add_css_class("metis-net-status");
        panel.append(&empty_label);

        let status_label = gtk::Label::new(None);
        status_label.set_wrap(true);
        status_label.set_xalign(0.0);
        status_label.add_css_class("metis-net-status");
        status_label.set_visible(false);
        panel.append(&status_label);

        let password_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        password_box.add_css_class("metis-net-connect");
        password_box.set_visible(false);
        let password_title = gtk::Label::new(Some(&metis_i18n::tr("VPN password")));
        password_title.set_halign(gtk::Align::Start);
        password_title.add_css_class("metis-net-connect-title");
        password_box.append(&password_title);
        let password_entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .hexpand(true)
            .placeholder_text(metis_i18n::tr("Password"))
            .build();
        password_entry.add_css_class("metis-net-password");
        password_box.append(&password_entry);
        let remember = gtk::CheckButton::with_label(&metis_i18n::tr("Remember on this profile"));
        remember.set_active(true);
        password_box.append(&remember);
        let pw_btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pw_btn_row.set_halign(gtk::Align::End);
        let pw_cancel = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
        pw_cancel.add_css_class("metis-net-cancel");
        let pw_connect = gtk::Button::with_label(&metis_i18n::tr("Connect"));
        pw_connect.add_css_class("metis-net-connect-btn");
        pw_btn_row.append(&pw_cancel);
        pw_btn_row.append(&pw_connect);
        password_box.append(&pw_btn_row);
        panel.append(&password_box);

        let settings_btn = gtk::Button::with_label(&metis_i18n::tr("VPN Settings…"));
        settings_btn.add_css_class("metis-net-settings-btn");
        settings_btn.set_halign(gtk::Align::Start);
        settings_btn.connect_clicked(|_| {
            if let Err(err) = crate::compositor::launch_program("metis-settings --page network/vpn")
            {
                tracing::warn!(%err, "failed to launch VPN settings");
            }
            super::super::dropdown::request_close_all();
        });
        panel.append(&settings_btn);

        super::super::dropdown::wire_toggle_prepare(&root, &panel, || {});

        let password_target = Rc::new(RefCell::new(None::<String>));

        {
            let password_box = password_box.clone();
            let password_entry = password_entry.clone();
            let password_target = password_target.clone();
            pw_cancel.connect_clicked(move |_| {
                *password_target.borrow_mut() = None;
                password_entry.set_text("");
                password_box.set_visible(false);
                crate::services::vpn_clear_password_prompt();
            });
        }
        let submit = {
            let password_box = password_box.clone();
            let password_entry = password_entry.clone();
            let remember = remember.clone();
            let password_target = password_target.clone();
            Rc::new(move || {
                let Some(target) = password_target.borrow().clone() else {
                    return;
                };
                let password = password_entry.text().to_string();
                if password.trim().is_empty() {
                    return;
                }
                let remember_on = remember.is_active();
                crate::services::vpn_up_with_password(target, password, remember_on);
                password_entry.set_text("");
                password_box.set_visible(false);
                *password_target.borrow_mut() = None;
            })
        };
        {
            let submit = submit.clone();
            pw_connect.connect_clicked(move |_| submit());
        }
        {
            let submit = submit.clone();
            password_entry.connect_activate(move |_| submit());
        }

        Self {
            root,
            icon,
            spinner,
            check,
            list,
            empty_label,
            status_label,
            password_box,
            password_title,
            password_entry,
            remember,
            password_target,
            last_sig: RefCell::new(String::new()),
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, vpn: &[VpnStatus], feedback: &VpnFeedback) {
        let sig = format!(
            "{}|{}|{}|{}|{}",
            vpn_signature(vpn),
            feedback.pending_target.as_deref().unwrap_or(""),
            feedback.connecting as u8,
            feedback.last_error.as_deref().unwrap_or(""),
            feedback.needs_password.as_deref().unwrap_or("")
        );
        if *self.last_sig.borrow() == sig {
            return;
        }
        *self.last_sig.borrow_mut() = sig;

        let active: Vec<&str> = vpn
            .iter()
            .filter(|v| v.active)
            .map(|v| v.name.as_str())
            .collect();
        let pending = feedback.pending_target.is_some();
        let tooltip = if feedback.needs_password.is_some() {
            metis_i18n::tr("VPN password required")
        } else if let Some(err) = feedback.last_error.as_deref() {
            err.to_string()
        } else if pending {
            if feedback.connecting {
                metis_i18n::tr("Connecting VPN…")
            } else {
                metis_i18n::tr("Disconnecting VPN…")
            }
        } else if active.is_empty() {
            if vpn.is_empty() {
                metis_i18n::tr("VPN")
            } else {
                metis_i18n::tr("VPN disconnected")
            }
        } else {
            metis_i18n::tr("VPN: %1").replace("%1", &active.join(", "))
        };
        self.root.set_tooltip_text(Some(&tooltip));

        if pending {
            self.spinner.set_visible(true);
            self.spinner.start();
            self.check.set_visible(false);
            self.icon.remove_css_class("metis-bar-vpn-active");
        } else {
            self.spinner.stop();
            self.spinner.set_visible(false);
            let connected = !active.is_empty();
            self.check.set_visible(connected);
            if connected {
                self.icon.add_css_class("metis-bar-vpn-active");
            } else {
                self.icon.remove_css_class("metis-bar-vpn-active");
            }
        }
        icons::set_icon(&self.icon, "network-vpn-symbolic");

        if let Some(target) = feedback.needs_password.as_deref() {
            let label = vpn
                .iter()
                .find(|v| v.uuid == target || v.name == target)
                .map(|v| v.name.as_str())
                .unwrap_or(target);
            self.password_title
                .set_text(&metis_i18n::tr("Password for %1").replace("%1", label));
            *self.password_target.borrow_mut() = Some(target.to_string());
            self.password_box.set_visible(true);
            self.status_label
                .set_text(&metis_i18n::tr("Enter the VPN password to connect."));
            self.status_label.set_visible(true);
            let entry = self.password_entry.clone();
            glib::idle_add_local_once(move || {
                entry.grab_focus();
            });
        } else if !pending {
            // Keep an in-progress typed password if user is mid-entry and we
            // only got a transient poll; clear when no longer needed.
            if self.password_target.borrow().is_some() {
                *self.password_target.borrow_mut() = None;
                self.password_entry.set_text("");
                self.password_box.set_visible(false);
            }
        }

        if feedback.needs_password.is_none() {
            if let Some(err) = feedback.last_error.as_deref() {
                self.status_label.set_text(err);
                self.status_label.set_visible(true);
            } else if pending {
                self.status_label.set_text(&if feedback.connecting {
                    metis_i18n::tr("Connecting…")
                } else {
                    metis_i18n::tr("Disconnecting…")
                });
                self.status_label.set_visible(true);
            } else if !self.password_box.is_visible() {
                self.status_label.set_visible(false);
            }
        }

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.empty_label.set_visible(vpn.is_empty());
        for profile in vpn {
            self.list.append(&build_vpn_row(profile));
        }
    }
}

fn build_vpn_row(profile: &VpnStatus) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("metis-net-vpn-row");

    let icon = icons::image("network-vpn-symbolic");
    row.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&profile.name));
    name.set_halign(gtk::Align::Start);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(18);
    text.append(&name);
    let kind = gtk::Label::new(Some(&profile.kind));
    kind.set_halign(gtk::Align::Start);
    kind.add_css_class("metis-net-status");
    text.append(&kind);
    row.append(&text);

    if profile.pending {
        let spinner = gtk::Spinner::new();
        spinner.start();
        row.append(&spinner);
        let label = gtk::Label::new(Some(&if profile.active {
            metis_i18n::tr("Disconnecting…")
        } else {
            metis_i18n::tr("Connecting…")
        }));
        label.add_css_class("metis-net-status");
        row.append(&label);
        return row;
    }

    if profile.active {
        let check = icons::image("object-select-symbolic");
        check.add_css_class("metis-net-active");
        row.append(&check);
    }

    let action = if profile.active {
        gtk::Button::with_label(&metis_i18n::tr("Disconnect"))
    } else {
        gtk::Button::with_label(&metis_i18n::tr("Connect"))
    };
    action.add_css_class("metis-net-vpn-btn");
    action.set_valign(gtk::Align::Center);
    let target = if profile.uuid.is_empty() {
        profile.name.clone()
    } else {
        profile.uuid.clone()
    };
    let active = profile.active;
    action.connect_clicked(move |_| {
        if active {
            crate::services::vpn_down(target.clone());
        } else {
            crate::services::vpn_up(target.clone());
        }
    });
    row.append(&action);
    row
}

fn vpn_signature(vpn: &[VpnStatus]) -> String {
    let mut s = String::new();
    for v in vpn {
        s.push_str(&format!(
            "{}:{}:{}:{}:{};",
            v.uuid, v.name, v.kind, v.active as u8, v.pending as u8
        ));
    }
    s
}

fn wifi_signal_icon(signal: u8) -> &'static str {
    match signal {
        80..=u8::MAX => "network-wireless-signal-excellent-symbolic",
        55..=79 => "network-wireless-signal-good-symbolic",
        30..=54 => "network-wireless-signal-ok-symbolic",
        10..=29 => "network-wireless-signal-weak-symbolic",
        _ => "network-wireless-signal-none-symbolic",
    }
}

fn bar_icon(
    eth: &EthernetStatus,
    wifi: &[WifiNetwork],
    wifi_present: bool,
    wifi_enabled: bool,
) -> &'static str {
    if wifi_present && let Some(active) = wifi.iter().find(|n| n.active) {
        return wifi_signal_icon(active.signal);
    }
    if eth.connected {
        return "network-wired-symbolic";
    }
    if eth.present && !wifi_present {
        return "network-wired-disconnected-symbolic";
    }
    if !wifi_present {
        // No Wi-Fi and no ethernet — still a Network affordance for Settings.
        return "network-wired-disconnected-symbolic";
    }
    if !wifi_enabled {
        return "network-wireless-disabled-symbolic";
    }
    "network-wireless-offline-symbolic"
}

fn network_tooltip(
    eth: &EthernetStatus,
    wifi: &[WifiNetwork],
    wifi_present: bool,
    wifi_enabled: bool,
) -> String {
    if wifi_present && let Some(active) = wifi.iter().find(|n| n.active) {
        return active.ssid.clone();
    }
    if eth.connected {
        return eth.label.clone();
    }
    if eth.present && !wifi_present {
        return eth.label.clone();
    }
    if !wifi_present {
        return metis_i18n::tr("Offline");
    }
    if !wifi_enabled {
        return metis_i18n::tr("Wi-Fi off");
    }
    metis_i18n::tr("Offline")
}

fn network_signature(
    wifi: &[WifiNetwork],
    wifi_present: bool,
    enabled: bool,
    pending: &Option<(String, Instant)>,
) -> String {
    let mut s = format!("p{}|e{}|", wifi_present as u8, enabled as u8);
    for n in wifi {
        // Bucket the signal so minor RSSI jitter doesn't trigger a rebuild.
        s.push_str(&format!(
            "{}:{}:{}:{};",
            n.ssid,
            n.active as u8,
            n.secured as u8,
            n.signal / 25
        ));
    }
    if let Some((ssid, _)) = pending {
        s.push_str("p:");
        s.push_str(ssid);
    }
    s
}

/// One labelled row: a mute icon-button on the left, a slider filling the rest.
struct AudioRow {
    scale: gtk::Scale,
    mute_icon: gtk::Image,
    percent: Rc<Cell<u8>>,
    muted: Rc<Cell<bool>>,
}

pub struct VolumeWidget {
    root: gtk::Button,
    icon: gtk::Image,
    output: AudioRow,
    input: AudioRow,
    updating: Rc<Cell<bool>>,
    suppress_until: Rc<Cell<Instant>>,
    last_out: Cell<(u8, bool)>,
    last_in: Cell<(u8, bool)>,
}

/// Hold off poller-driven updates briefly after a user action so optimistic UI
/// state isn't reverted by the lagging pactl read-back (fixes the mute flicker).
fn bump_suppress(cell: &Rc<Cell<Instant>>) {
    cell.set(Instant::now() + Duration::from_millis(700));
}

impl VolumeWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-volume");
        root.add_css_class("metis-bar-sys-icon");

        let icon = icons::image(names::volume(50, false));
        root.set_child(Some(&icon));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(12);
        panel.set_width_request(260);

        let title = gtk::Label::builder()
            .label(metis_i18n::tr("Audio"))
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("metis-bar-section-title");
        panel.append(&title);

        let updating = Rc::new(Cell::new(false));
        let suppress_until = Rc::new(Cell::new(Instant::now()));

        // Late-bound so the row handlers can broadcast the *combined* audio state
        // (output + input) to every bar after a user action — the real closure is
        // installed below, once both rows (and their cells) exist.
        let on_change_slot: OptFn0Cell = Rc::new(RefCell::new(None));
        let on_change: Rc<dyn Fn()> = {
            let slot = on_change_slot.clone();
            Rc::new(move || {
                if let Some(cb) = slot.borrow().as_ref().cloned() {
                    cb();
                }
            })
        };

        let output = build_audio_row(
            &panel,
            AudioKind::Output,
            &updating,
            &suppress_until,
            on_change.clone(),
        );
        let input = build_audio_row(
            &panel,
            AudioKind::Input,
            &updating,
            &suppress_until,
            on_change.clone(),
        );

        {
            let out_p = output.percent.clone();
            let out_m = output.muted.clone();
            let in_p = input.percent.clone();
            let in_m = input.muted.clone();
            *on_change_slot.borrow_mut() = Some(Rc::new(move || {
                crate::ui::bar::broadcast_audio(out_p.get(), out_m.get(), in_p.get(), in_m.get());
            }));
        }

        super::super::dropdown::wire_toggle(&root, &panel, "volume");

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let suppress_until = suppress_until.clone();
            scroll.connect_scroll(move |_, _, dy| {
                let delta = if dy < 0.0 { 5i8 } else { -5i8 };
                bump_suppress(&suppress_until);
                crate::services::set_volume_relative(delta);
                glib::Propagation::Stop
            });
        }
        root.add_controller(scroll);

        Self {
            root,
            icon,
            output,
            input,
            updating,
            suppress_until,
            last_out: Cell::new((255, false)),
            last_in: Cell::new((255, false)),
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    pub fn update(&self, percent: u8, muted: bool, mic_percent: u8, mic_muted: bool) {
        // Don't let the poller stomp optimistic UI right after a user action.
        if Instant::now() < self.suppress_until.get() {
            return;
        }

        if self.last_out.get() != (percent, muted) {
            self.last_out.set((percent, muted));
            self.output.percent.set(percent);
            self.output.muted.set(muted);
            self.root.set_tooltip_text(Some(
                &metis_i18n::tr("Volume %1%").replace("%1", &percent.to_string()),
            ));
            self.updating.set(true);
            self.output
                .scale
                .set_value(f64::from(if muted { 0 } else { percent }));
            self.updating.set(false);
            icons::set_icon(&self.icon, names::volume(percent, muted));
            icons::set_icon(&self.output.mute_icon, names::volume(percent, muted));
        }

        if self.last_in.get() != (mic_percent, mic_muted) {
            self.last_in.set((mic_percent, mic_muted));
            self.input.percent.set(mic_percent);
            self.input.muted.set(mic_muted);
            self.updating.set(true);
            self.input
                .scale
                .set_value(f64::from(if mic_muted { 0 } else { mic_percent }));
            self.updating.set(false);
            icons::set_icon(&self.input.mute_icon, names::mic(mic_percent, mic_muted));
        }
    }

    /// Force the displayed audio state immediately (mirrors a user action made on
    /// another bar). Bumps suppression so the lagging pactl read-back doesn't undo
    /// it, and writes through `updating` so setting the slider doesn't re-fire the
    /// value-changed handler.
    pub fn apply_optimistic(&self, percent: u8, muted: bool, mic_percent: u8, mic_muted: bool) {
        bump_suppress(&self.suppress_until);

        self.last_out.set((percent, muted));
        self.output.percent.set(percent);
        self.output.muted.set(muted);
        self.root.set_tooltip_text(Some(
            &metis_i18n::tr("Volume %1%").replace("%1", &percent.to_string()),
        ));
        self.updating.set(true);
        self.output
            .scale
            .set_value(f64::from(if muted { 0 } else { percent }));
        self.updating.set(false);
        icons::set_icon(&self.icon, names::volume(percent, muted));
        icons::set_icon(&self.output.mute_icon, names::volume(percent, muted));

        self.last_in.set((mic_percent, mic_muted));
        self.input.percent.set(mic_percent);
        self.input.muted.set(mic_muted);
        self.updating.set(true);
        self.input
            .scale
            .set_value(f64::from(if mic_muted { 0 } else { mic_percent }));
        self.updating.set(false);
        icons::set_icon(&self.input.mute_icon, names::mic(mic_percent, mic_muted));
    }
}

#[derive(Clone, Copy)]
enum AudioKind {
    Output,
    Input,
}

fn build_audio_row(
    panel: &gtk::Box,
    kind: AudioKind,
    updating: &Rc<Cell<bool>>,
    suppress_until: &Rc<Cell<Instant>>,
    on_change: Rc<dyn Fn()>,
) -> AudioRow {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();

    let percent = Rc::new(Cell::new(0u8));
    let muted = Rc::new(Cell::new(false));

    let initial_icon = match kind {
        AudioKind::Output => names::volume(50, false),
        AudioKind::Input => names::mic(50, false),
    };
    let mute_icon = icons::image(initial_icon);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("metis-bar-volume-scale");

    let set_mute_icon = {
        let mute_icon = mute_icon.clone();
        move |pct: u8, muted: bool| {
            let name = match kind {
                AudioKind::Output => names::volume(pct, muted),
                AudioKind::Input => names::mic(pct, muted),
            };
            icons::set_icon(&mute_icon, name);
        }
    };

    let mute_btn = gtk::Button::builder().build();
    mute_btn.add_css_class("metis-bar-audio-mute");
    mute_btn.set_child(Some(&mute_icon));
    mute_btn.set_valign(gtk::Align::Center);
    {
        let muted = muted.clone();
        let percent = percent.clone();
        let suppress_until = suppress_until.clone();
        let updating = updating.clone();
        let scale = scale.clone();
        let set_mute_icon = set_mute_icon.clone();
        let on_change = on_change.clone();
        mute_btn.connect_clicked(move |_| {
            let new_muted = !muted.get();
            muted.set(new_muted);
            bump_suppress(&suppress_until);
            set_mute_icon(percent.get(), new_muted);
            // Reflect mute on the slider immediately (poller is suppressed now).
            updating.set(true);
            scale.set_value(f64::from(if new_muted { 0 } else { percent.get() }));
            updating.set(false);
            match kind {
                AudioKind::Output => crate::services::set_mute(new_muted),
                AudioKind::Input => crate::services::set_mic_mute(new_muted),
            }
            on_change();
        });
    }
    row.append(&mute_btn);

    {
        let updating = updating.clone();
        let suppress_until = suppress_until.clone();
        let percent = percent.clone();
        let muted = muted.clone();
        let set_mute_icon = set_mute_icon.clone();
        let on_change = on_change.clone();
        scale.connect_value_changed(move |scale| {
            if updating.get() {
                return;
            }
            let pct = scale.value().round() as u8;
            percent.set(pct);
            bump_suppress(&suppress_until);
            // Dragging the slider implies the user wants sound: unmute.
            if muted.get() {
                muted.set(false);
                set_mute_icon(pct, false);
                match kind {
                    AudioKind::Output => crate::services::set_mute(false),
                    AudioKind::Input => crate::services::set_mic_mute(false),
                }
            } else {
                set_mute_icon(pct, false);
            }
            match kind {
                AudioKind::Output => crate::services::set_volume_absolute(pct),
                AudioKind::Input => crate::services::set_mic_volume_absolute(pct),
            }
            on_change();
        });
    }
    row.append(&scale);

    panel.append(&row);

    AudioRow {
        scale,
        mute_icon,
        percent,
        muted,
    }
}
