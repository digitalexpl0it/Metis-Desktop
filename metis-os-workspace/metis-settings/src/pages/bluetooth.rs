//! Bluetooth: adapter power, scan, pair/connect, and device list.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::bluetooth::{self, BluetoothSnapshot, DeviceState};
use crate::ui;
use metis_i18n::tr;

struct Sections {
    powered: gtk::Switch,
    power_handler: RefCell<Option<glib::SignalHandlerId>>,
    scan_btn: gtk::Button,
    status: gtk::Label,
    list: gtk::Box,
    /// Mirrors the adapter's `Discovering` flag so the scan button can toggle
    /// between starting and stopping a scan without re-reading a snapshot.
    scanning: std::cell::Cell<bool>,
    /// While a power toggle is in flight, don't snap the switch back to the
    /// stale snapshot from a background refresh.
    power_pending: std::cell::Cell<bool>,
    last_devices: RefCell<Vec<crate::bluetooth::BtDevice>>,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("bluetooth");

    let (adapter_card, adapter_body) = ui::section(&tr("Adapter"));
    let (power_row, powered) = ui::switch_row(&tr("Bluetooth"));
    adapter_body.append(&power_row);
    let scan_btn = gtk::Button::with_label(&tr("Scan for devices"));
    adapter_body.append(&scan_btn);
    let status = gtk::Label::new(Some(&tr("Checking adapter…")));
    status.set_xalign(0.0);
    status.add_css_class("metis-settings-hint");
    adapter_body.append(&status);
    content.append(&adapter_card);

    let (dev_card, dev_body) = ui::section(&tr("Devices"));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("metis-settings-list");
    list.add_css_class("metis-settings-zebra-list");
    dev_body.append(&list);
    content.append(&dev_card);

    let (tx, rx) = mpsc::channel::<BluetoothSnapshot>();
    let refresh = {
        let tx = tx.clone();
        Rc::new(move || {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(bluetooth::load_snapshot());
            });
        })
    };

    let sections = Rc::new(Sections {
        powered: powered.clone(),
        power_handler: RefCell::new(None),
        scan_btn: scan_btn.clone(),
        status,
        list,
        scanning: std::cell::Cell::new(false),
        power_pending: std::cell::Cell::new(false),
        last_devices: RefCell::new(Vec::new()),
    });

    let power_handler = powered.connect_active_notify({
        let refresh = refresh.clone();
        let sections = sections.clone();
        move |s| {
            sections.power_pending.set(true);
            let on = s.is_active();
            let refresh = refresh.clone();
            glib::idle_add_local_once(move || {
                bluetooth::set_powered_async(on);
                schedule_refresh(&refresh, 800);
            });
        }
    });
    *sections.power_handler.borrow_mut() = Some(power_handler);

    {
        let sections_poll = sections.clone();
        let refresh_poll = refresh.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if let Ok(snap) = rx.try_recv() {
                render(&sections_poll, &snap, &refresh_poll);
            }
            glib::ControlFlow::Continue
        });
        scan_btn.connect_clicked({
            let refresh = refresh.clone();
            let sections = sections.clone();
            move |_| {
                if sections.scanning.get() {
                    sections.scanning.set(false);
                    sections.scan_btn.set_label(&tr("Scan for devices"));
                    std::thread::spawn(bluetooth::stop_scan);
                    schedule_refresh(&refresh, 600);
                } else {
                    sections.scanning.set(true);
                    sections.scan_btn.set_label(&tr("Stop scanning"));
                    std::thread::spawn(bluetooth::start_scan);
                    schedule_refresh(&refresh, 1500);
                    schedule_auto_stop_scan(&sections, &refresh, 30_000);
                }
            }
        });
    }

    refresh();
    scroller.upcast()
}

fn render(sections: &Sections, snap: &BluetoothSnapshot, refresh: &Rc<impl Fn() + 'static>) {
    if !snap.adapter_present {
        sections.powered.set_sensitive(false);
        sections.scan_btn.set_sensitive(false);
        sections.power_pending.set(false);
        sections.status.set_text(&tr("No Bluetooth adapter found."));
        clear_list(&sections.list);
        sections.last_devices.borrow_mut().clear();
        return;
    }
    sections.powered.set_sensitive(true);
    sections.scan_btn.set_sensitive(snap.powered);
    sync_power_switch(sections, snap.powered);
    sections.scanning.set(snap.discovering && snap.powered);
    sections.scan_btn.set_label(if sections.scanning.get() {
        "Stop scanning"
    } else {
        "Scan for devices"
    });
    let status_text = if snap.discovering {
        "Scanning for nearby devices…".to_string()
    } else if snap.powered {
        format!("Adapter: {}", snap.adapter_name)
    } else {
        "Bluetooth is off".to_string()
    };
    sections.status.set_text(&status_text);

    if *sections.last_devices.borrow() == snap.devices {
        return;
    }
    *sections.last_devices.borrow_mut() = snap.devices.clone();
    clear_list(&sections.list);
    if snap.devices.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No devices found. Tap Scan to search.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        sections.list.append(&empty);
        return;
    }
    for (i, dev) in snap.devices.iter().enumerate() {
        let row = device_row(dev, refresh, i % 2 == 1);
        sections.list.append(&row);
    }
}

fn sync_power_switch(sections: &Sections, adapter_powered: bool) {
    let switch_on = sections.powered.is_active();
    if sections.power_pending.get() {
        if adapter_powered == switch_on {
            sections.power_pending.set(false);
        }
        return;
    }
    if adapter_powered != switch_on
        && let Some(handler) = sections.power_handler.borrow().as_ref()
    {
        sections.powered.block_signal(handler);
        sections.powered.set_active(adapter_powered);
        sections.powered.unblock_signal(handler);
    }
}

fn device_row(
    dev: &crate::bluetooth::BtDevice,
    refresh: &Rc<impl Fn() + 'static>,
    alt: bool,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-settings-zebra-row");
    if alt {
        row.add_css_class("metis-settings-zebra-row-alt");
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&dev.name));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-row-title");
    let sub = gtk::Label::new(Some(&format!(
        "{} · {}",
        dev.address,
        state_label(&dev.state)
    )));
    sub.set_xalign(0.0);
    sub.add_css_class("metis-settings-hint");
    text.append(&title);
    text.append(&sub);
    row.append(&text);

    if dev.state != DeviceState::Connected {
        let connect = gtk::Button::with_label(&tr("Connect"));
        let addr = dev.address.clone();
        let refresh = refresh.clone();
        connect.connect_clicked(move |_| {
            let addr = addr.clone();
            std::thread::spawn(move || bluetooth::pair_and_connect(&addr));
            schedule_refresh(&refresh, 2000);
        });
        row.append(&connect);
    } else {
        let disconnect = gtk::Button::with_label(&tr("Disconnect"));
        let addr = dev.address.clone();
        let refresh = refresh.clone();
        disconnect.connect_clicked(move |_| {
            let addr = addr.clone();
            std::thread::spawn(move || bluetooth::disconnect(&addr));
            schedule_refresh(&refresh, 1000);
        });
        row.append(&disconnect);
    }

    let remove = gtk::Button::with_label(&tr("Remove"));
    remove.add_css_class("destructive-action");
    let addr = dev.address.clone();
    let refresh = refresh.clone();
    remove.connect_clicked(move |_| {
        let addr = addr.clone();
        std::thread::spawn(move || bluetooth::remove_device(&addr));
        schedule_refresh(&refresh, 1000);
    });
    row.append(&remove);
    row
}

fn state_label(s: &DeviceState) -> String {
    match s {
        DeviceState::Connected => tr("Connected"),
        DeviceState::Paired => tr("Paired"),
        DeviceState::Available => tr("Available"),
    }
}

fn clear_list(list: &gtk::Box) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn schedule_refresh(refresh: &Rc<impl Fn() + 'static>, delay_ms: u64) {
    let refresh = refresh.clone();
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || refresh());
}

fn schedule_auto_stop_scan(
    sections: &Rc<Sections>,
    refresh: &Rc<impl Fn() + 'static>,
    delay_ms: u64,
) {
    let sections = sections.clone();
    let refresh = refresh.clone();
    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
        if sections.scanning.get() {
            std::thread::spawn(bluetooth::stop_scan);
            sections.scanning.set(false);
            sections.scan_btn.set_label(&tr("Scan for devices"));
            refresh();
        }
    });
}
