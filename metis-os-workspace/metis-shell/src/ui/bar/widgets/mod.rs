mod clipboard;
pub mod clock;
mod launcher;
mod menu;
mod notifications;
pub mod sys;
mod tasks;
mod tray;
mod updates;
mod volumes;
mod weather;
pub(crate) use menu::request_toggle as toggle_menu;
pub(crate) use menu::set_below_screenshot as set_menu_below_screenshot;
pub mod workspaces;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::config::{BarConfig, BarWidgetId};
use crate::services::BarSnapshot;
use crate::ui::bar::BarShell;

pub use crate::services::do_not_disturb;
use clipboard::ClipboardWidget;
use clock::ClockWidget;
use launcher::LauncherWidget;
use notifications::NotificationsWidget;
pub(crate) use notifications::build_action_row;
use sys::{BatteryWidget, BluetoothWidget, NetworkWidget, VolumeWidget, VpnWidget};
use tasks::TasksWidget;
use tray::TrayWidget;
use updates::UpdatesWidget;
use volumes::VolumesWidget;
use weather::WeatherWidget;
use workspaces::WorkspacesWidget;

pub struct WidgetRefs {
    workspaces: RefCell<Option<WorkspacesWidget>>,
    tasks: RefCell<Option<TasksWidget>>,
    clock: RefCell<Option<ClockWidget>>,
    battery: RefCell<Option<BatteryWidget>>,
    bluetooth: RefCell<Option<BluetoothWidget>>,
    network: RefCell<Option<NetworkWidget>>,
    vpn: RefCell<Option<VpnWidget>>,
    volume: RefCell<Option<VolumeWidget>>,
    notifications: RefCell<Option<NotificationsWidget>>,
    clipboard: RefCell<Option<ClipboardWidget>>,
    weather: RefCell<Option<WeatherWidget>>,
    tray: RefCell<Option<TrayWidget>>,
    removable_volumes: RefCell<Option<VolumesWidget>>,
    updates: RefCell<Option<UpdatesWidget>>,
}

impl WidgetRefs {
    pub fn apply_snapshot(&self, snapshot: &BarSnapshot) {
        if let Some(w) = self.workspaces.borrow().as_ref() {
            w.update(&snapshot.workspaces);
        }
        if let Some(w) = self.battery.borrow().as_ref() {
            w.update(snapshot.battery_percent, snapshot.battery_charging);
        }
        if let Some(w) = self.bluetooth.borrow().as_ref() {
            w.update(&snapshot.bluetooth);
        }
        if let Some(w) = self.network.borrow().as_ref() {
            w.update(&snapshot.ethernet, &snapshot.wifi, snapshot.wifi_enabled);
        }
        if let Some(w) = self.vpn.borrow().as_ref() {
            w.update(&snapshot.vpn, &snapshot.vpn_feedback);
        }
        if let Some(w) = self.volume.borrow().as_ref() {
            w.update(
                snapshot.volume_percent,
                snapshot.volume_muted,
                snapshot.mic_percent,
                snapshot.mic_muted,
            );
        }
        if let Some(w) = self.notifications.borrow().as_ref() {
            w.update(&snapshot.notifications);
        }
    }

    /// Repaint the taskbar from the latest window store snapshot. The tasks
    /// widget also self-refreshes via the window store's `register_refresh` hook;
    /// this fan-out is the explicit driver for callers that hold the snapshot.
    #[allow(dead_code)] // bar tasks fan-out helper
    pub fn apply_tasks(&self, snapshot: &crate::services::windows::WindowsSnapshot) {
        if let Some(w) = self.tasks.borrow().as_ref() {
            w.update(snapshot);
        }
    }

    /// Force the volume widget to show a user-driven audio change immediately,
    /// bypassing the poll round-trip — used to mirror an action from another bar.
    pub fn apply_volume_optimistic(
        &self,
        percent: u8,
        muted: bool,
        mic_percent: u8,
        mic_muted: bool,
    ) {
        if let Some(w) = self.volume.borrow().as_ref() {
            w.apply_optimistic(percent, muted, mic_percent, mic_muted);
        }
    }

    /// Weather arrives on its own (slow) channel, separate from the poll snapshot.
    pub fn apply_weather(&self, snapshot: &crate::services::WeatherSnapshot) {
        if let Some(w) = self.weather.borrow().as_ref() {
            w.update(snapshot);
        }
    }

    pub fn refresh_workspaces(&self) {
        if let Some(w) = self.workspaces.borrow().as_ref() {
            w.update(&crate::services::workspace_snapshot());
        }
    }

    /// Live-toggle the Control Center grid button without rebuilding workspace dots.
    pub fn sync_control_center_button(&self) {
        if let Some(w) = self.workspaces.borrow().as_ref() {
            w.sync_control_center_visible();
        }
    }

    /// Apply bar.json fields that do not require tearing down widgets.
    pub fn apply_bar_config(&self, cfg: &BarConfig) {
        if let Some(w) = self.tray.borrow().as_ref() {
            w.set_mode(cfg.tray_icon_mode);
        }
    }
}

pub fn build(
    root: &gtk::Box,
    config: Rc<RefCell<BarConfig>>,
    output: Option<String>,
    shell: BarShell,
) -> WidgetRefs {
    let refs = WidgetRefs {
        workspaces: RefCell::new(None),
        tasks: RefCell::new(None),
        clock: RefCell::new(None),
        battery: RefCell::new(None),
        bluetooth: RefCell::new(None),
        network: RefCell::new(None),
        vpn: RefCell::new(None),
        volume: RefCell::new(None),
        notifications: RefCell::new(None),
        clipboard: RefCell::new(None),
        weather: RefCell::new(None),
        tray: RefCell::new(None),
        removable_volumes: RefCell::new(None),
        updates: RefCell::new(None),
    };

    let cfg = config.borrow().clone();
    let compact = matches!(
        cfg.position,
        crate::config::BarPosition::Left | crate::config::BarPosition::Right
    );
    let bar_orientation = match cfg.position {
        crate::config::BarPosition::Top | crate::config::BarPosition::Bottom => {
            gtk::Orientation::Horizontal
        }
        crate::config::BarPosition::Left | crate::config::BarPosition::Right => {
            gtk::Orientation::Vertical
        }
    };

    // Pinned brand/launcher icon, always first so it sits at the far-leading edge
    // of the bar (left for a top bar, top for a vertical bar).
    let launcher = LauncherWidget::new();
    append_bar_widget(root, launcher.root(), bar_orientation);

    for widget in &cfg.widgets {
        match widget {
            BarWidgetId::Spacer => {
                let spacer = gtk::Box::new(bar_orientation, 0);
                spacer.set_hexpand(true);
                spacer.set_vexpand(true);
                spacer.add_css_class("metis-bar-spacer");
                root.append(&spacer);
            }
            BarWidgetId::Workspaces => {
                let w = WorkspacesWidget::new(cfg.position, output.clone(), shell.clone());
                append_bar_widget(root, w.root(), bar_orientation);
                w.update(&crate::services::workspace_snapshot_for(output.as_deref()));
                *refs.workspaces.borrow_mut() = Some(w);
            }
            BarWidgetId::Tasks => {
                let w = TasksWidget::new(compact, output.clone());
                append_bar_widget(root, w.root(), bar_orientation);
                // Claim flexible middle space so the dock grows with monitor
                // width/height and scrolls when icons overflow.
                if bar_orientation == gtk::Orientation::Horizontal {
                    w.root().set_hexpand(true);
                    w.root().set_halign(gtk::Align::Fill);
                    w.root().set_propagate_natural_width(false);
                } else {
                    w.root().set_vexpand(true);
                    w.root().set_valign(gtk::Align::Fill);
                    w.root().set_propagate_natural_height(false);
                }
                *refs.tasks.borrow_mut() = Some(w);
            }
            BarWidgetId::Tray => {
                let w = TrayWidget::new(cfg.tray_icon_mode);
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.tray.borrow_mut() = Some(w);
            }
            BarWidgetId::RemovableVolumes => {
                let w = VolumesWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.removable_volumes.borrow_mut() = Some(w);
            }
            BarWidgetId::Updates => {
                let w = UpdatesWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.updates.borrow_mut() = Some(w);
            }
            BarWidgetId::Clock => {
                let w = ClockWidget::new(&cfg.clock, compact);
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.clock.borrow_mut() = Some(w);
            }
            BarWidgetId::Battery => {
                let w = BatteryWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.battery.borrow_mut() = Some(w);
            }
            BarWidgetId::Network => {
                let w = NetworkWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.network.borrow_mut() = Some(w);
            }
            BarWidgetId::Vpn => {
                let w = VpnWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.vpn.borrow_mut() = Some(w);
            }
            BarWidgetId::Bluetooth => {
                let w = BluetoothWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.bluetooth.borrow_mut() = Some(w);
            }
            BarWidgetId::Volume => {
                let w = VolumeWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.volume.borrow_mut() = Some(w);
            }
            BarWidgetId::Notifications => {
                let w = NotificationsWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.notifications.borrow_mut() = Some(w);
            }
            BarWidgetId::Clipboard => {
                let w = ClipboardWidget::new();
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.clipboard.borrow_mut() = Some(w);
            }
            BarWidgetId::Weather => {
                let w = WeatherWidget::new(compact);
                append_bar_widget(root, w.root(), bar_orientation);
                *refs.weather.borrow_mut() = Some(w);
            }
        }
    }

    refs
}

fn append_bar_widget(
    root: &gtk::Box,
    widget: &impl IsA<gtk::Widget>,
    orientation: gtk::Orientation,
) {
    let w = widget.upcast_ref::<gtk::Widget>();
    // Fill the bar's cross axis so the hover gradient + underline reach the bar's
    // edge instead of floating around the icon. Only stretch the cross axis.
    if orientation == gtk::Orientation::Horizontal {
        w.set_valign(gtk::Align::Fill);
        w.set_vexpand(true);
        w.set_halign(gtk::Align::Fill);
        w.set_hexpand(false);
    } else {
        // Vertical bar: keep icons centered in the narrow strip; stretching
        // horizontally leaves a dead gap and breaks hover highlights.
        w.set_halign(gtk::Align::Center);
        w.set_hexpand(false);
        w.set_valign(gtk::Align::Center);
        w.set_vexpand(false);
    }
    root.append(w);
}
