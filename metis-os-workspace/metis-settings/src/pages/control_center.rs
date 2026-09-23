//! Control Center: enable the pull-down system monitor, panel height, refresh
//! interval, process-kill confirmation, process monitor app, and which overview
//! widgets are active. Persists to `dashboard.json`; the shell picks up changes
//! via its file watcher.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    DashboardConfig, DashboardWidgetId, KNOWN_PROCESS_MONITORS, load_dashboard_config,
    save_dashboard_config,
};

use crate::ui;
use metis_i18n::tr;

struct WidgetToggles {
    cpu: gtk::CheckButton,
    memory: gtk::CheckButton,
    disk: gtk::CheckButton,
    network: gtk::CheckButton,
    battery: gtk::CheckButton,
    logs: gtk::CheckButton,
    processes: gtk::CheckButton,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("control_center");
    let cfg = load_dashboard_config();

    let (panel_card, panel_body) =
        ui::section_with_icon(&tr("Control Center"), "utilities-system-monitor-symbolic");

    let enabled = gtk::Switch::new();
    enabled.set_active(cfg.enabled);
    enabled.set_halign(gtk::Align::End);
    panel_body.append(&ui::row_with_icon(
        "view-grid-symbolic",
        &tr("Show Control Center"),
        &enabled,
    ));

    let max_height = gtk::Scale::with_range(gtk::Orientation::Horizontal, 20.0, 100.0, 1.0);
    max_height.set_value(cfg.max_height_percent as f64);
    max_height.set_size_request(200, -1);
    max_height.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&max_height);
    panel_body.append(&ui::row_with_icon(
        "view-fullscreen-symbolic",
        &tr("Maximum panel height (%)"),
        &max_height,
    ));

    let refresh = gtk::Scale::with_range(gtk::Orientation::Horizontal, 500.0, 5000.0, 100.0);
    refresh.set_value(cfg.refresh_interval_ms as f64);
    refresh.set_size_request(200, -1);
    refresh.set_draw_value(true);
    ui::forward_wheel_to_page_scroller(&refresh);
    panel_body.append(&ui::row_with_icon(
        "view-refresh-symbolic",
        &tr("Refresh interval (ms)"),
        &refresh,
    ));

    let confirm_kill = gtk::Switch::new();
    confirm_kill.set_active(cfg.confirm_before_kill);
    confirm_kill.set_halign(gtk::Align::End);
    panel_body.append(&ui::row_with_icon(
        "process-stop-symbolic",
        &tr("Confirm before ending tasks"),
        &confirm_kill,
    ));

    panel_body.append(&ui::launcher_picker(
        "utilities-system-monitor-symbolic",
        &tr("Process monitor"),
        KNOWN_PROCESS_MONITORS,
        cfg.process_monitor.clone(),
        |val| {
            persist(|cfg| cfg.process_monitor = val);
        },
    ));

    let panel_hint = gtk::Label::new(Some(&tr(
        "Open Control Center from the grid icon beside workspace dots, or pull down \
         on the edge bar. Process monitor Auto-detect prefers btop/htop (in your \
         terminal) then GUI monitors. Changes apply within about a second.",
    )));
    panel_hint.set_xalign(0.0);
    panel_hint.set_wrap(true);
    panel_hint.add_css_class("metis-settings-hint");
    panel_body.append(&panel_hint);
    content.append(&panel_card);

    let (widgets_card, widgets_body) =
        ui::section_with_icon(&tr("Overview widgets"), "view-app-grid-symbolic");

    let toggles = Rc::new(WidgetToggles {
        cpu: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Cpu.label())),
        memory: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Memory.label())),
        disk: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Disk.label())),
        network: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Network.label())),
        battery: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Battery.label())),
        logs: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Logs.label())),
        processes: gtk::CheckButton::with_label(&tr(DashboardWidgetId::Processes.label())),
    });
    apply_widget_toggles(&toggles, &cfg.widgets);

    let widget_grid = gtk::FlowBox::new();
    widget_grid.set_selection_mode(gtk::SelectionMode::None);
    widget_grid.set_max_children_per_line(2);
    widget_grid.set_column_spacing(16);
    widget_grid.set_row_spacing(8);
    for toggle in [
        &toggles.cpu,
        &toggles.memory,
        &toggles.disk,
        &toggles.network,
        &toggles.battery,
        &toggles.logs,
        &toggles.processes,
    ] {
        widget_grid.append(toggle);
    }
    widgets_body.append(&widget_grid);

    let widgets_hint = gtk::Label::new(Some(&tr(
        "Overview cards can be toggled independently. Battery hides on desktops \
         without a battery. Logs shows a short journalctl tail. Disabling Processes \
         hides the Processes tab.",
    )));
    widgets_hint.set_xalign(0.0);
    widgets_hint.set_wrap(true);
    widgets_hint.add_css_class("metis-settings-hint");
    widgets_body.append(&widgets_hint);
    content.append(&widgets_card);

    let seeding = Rc::new(RefCell::new(true));

    {
        let seeding = seeding.clone();
        enabled.connect_active_notify(move |sw| {
            if *seeding.borrow() {
                return;
            }
            persist(|cfg| cfg.enabled = sw.is_active());
        });
    }
    {
        let seeding = seeding.clone();
        max_height.connect_value_changed(move |scale| {
            if *seeding.borrow() {
                return;
            }
            persist(|cfg| cfg.max_height_percent = scale.value().round() as u8);
        });
    }
    {
        let seeding = seeding.clone();
        refresh.connect_value_changed(move |scale| {
            if *seeding.borrow() {
                return;
            }
            persist(|cfg| cfg.refresh_interval_ms = scale.value().round() as u32);
        });
    }
    {
        let seeding = seeding.clone();
        confirm_kill.connect_active_notify(move |sw| {
            if *seeding.borrow() {
                return;
            }
            persist(|cfg| cfg.confirm_before_kill = sw.is_active());
        });
    }
    {
        let seeding = seeding.clone();
        let toggles = toggles.clone();
        let wire = |toggle: &gtk::CheckButton| {
            let seeding = seeding.clone();
            let toggles = toggles.clone();
            toggle.connect_active_notify(move |_| {
                if *seeding.borrow() {
                    return;
                }
                persist(|cfg| cfg.widgets = collect_widgets(&toggles));
            });
        };
        wire(&toggles.cpu);
        wire(&toggles.memory);
        wire(&toggles.disk);
        wire(&toggles.network);
        wire(&toggles.battery);
        wire(&toggles.logs);
        wire(&toggles.processes);
    }

    seeding.replace(false);

    scroller.upcast()
}

fn apply_widget_toggles(toggles: &WidgetToggles, enabled: &[DashboardWidgetId]) {
    toggles
        .cpu
        .set_active(enabled.contains(&DashboardWidgetId::Cpu));
    toggles
        .memory
        .set_active(enabled.contains(&DashboardWidgetId::Memory));
    toggles
        .disk
        .set_active(enabled.contains(&DashboardWidgetId::Disk));
    toggles
        .network
        .set_active(enabled.contains(&DashboardWidgetId::Network));
    toggles
        .battery
        .set_active(enabled.contains(&DashboardWidgetId::Battery));
    toggles
        .logs
        .set_active(enabled.contains(&DashboardWidgetId::Logs));
    toggles
        .processes
        .set_active(enabled.contains(&DashboardWidgetId::Processes));
}

fn collect_widgets(toggles: &WidgetToggles) -> Vec<DashboardWidgetId> {
    let mut widgets = Vec::new();
    for id in DashboardWidgetId::default_order() {
        let active = match id {
            DashboardWidgetId::Cpu => toggles.cpu.is_active(),
            DashboardWidgetId::Memory => toggles.memory.is_active(),
            DashboardWidgetId::Disk => toggles.disk.is_active(),
            DashboardWidgetId::Network => toggles.network.is_active(),
            DashboardWidgetId::Battery => toggles.battery.is_active(),
            DashboardWidgetId::Logs => toggles.logs.is_active(),
            DashboardWidgetId::Processes => toggles.processes.is_active(),
        };
        if active {
            widgets.push(*id);
        }
    }
    if widgets.is_empty() {
        DashboardWidgetId::default_order().to_vec()
    } else {
        widgets
    }
}

fn persist(mutate: impl FnOnce(&mut DashboardConfig)) {
    let mut cfg = load_dashboard_config();
    mutate(&mut cfg);
    if let Err(err) = save_dashboard_config(&cfg) {
        tracing::warn!(%err, "failed to save dashboard.json");
    }
    crate::runtime::send("reload-dashboard");
}
