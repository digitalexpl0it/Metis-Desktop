//! Dashboard window construction and `Dashboard` methods.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::{BarPosition, DashboardWidgetId, load_bar_config, load_dashboard_config};
use metis_i18n::tr;

use crate::services::{
    DashboardSnapshot, GpuTempReading, ProcessRow, format_bytes, format_rate, format_uptime,
    short_kernel_version,
};
use crate::ui::bar::{BarShell, ensure_bar_strip_geometry};

use super::charts;
use super::lifecycle::{close_delta, request_close, teardown_dashboard};
use super::views;
use super::{
    DASHBOARD, Dashboard, OPEN_THRESHOLD, ProcessClassFilter, ProcessSortColumn, SNAP_MS,
    SortDirection,
};

pub(crate) fn build_dashboard(shell: &BarShell) -> Dashboard {
    let window = gtk::Window::builder()
        .title(tr("Metis Control Center"))
        .decorated(false)
        .build();
    window.add_css_class("metis-dashboard-window");
    window.set_can_focus(true);
    window.init_layer_shell();
    window.set_namespace(Some("metis-dashboard"));
    window.set_layer(Layer::Top);
    // Exclusive so SearchEntry / filters receive keys while the panel is open
    // (OnDemand never focuses the layer surface under Metis hit-testing).
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_exclusive_zone(-1);
    if let Some(monitor) = shell.window.monitor() {
        window.set_monitor(Some(&monitor));
    }
    window.set_visible(false);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    root.add_css_class("metis-dashboard-root");
    root.set_overflow(gtk::Overflow::Hidden);
    root.set_hexpand(true);
    root.set_vexpand(true);

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.add_css_class("metis-dashboard-header");

    let title = gtk::Label::new(Some(&tr("Control Center")));
    title.add_css_class("metis-dashboard-title");
    title.set_halign(gtk::Align::Start);
    title.set_hexpand(false);

    let stack = gtk::Stack::new();
    stack.add_css_class("metis-dashboard-stack");
    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    switcher.add_css_class("metis-dash-tabs");
    switcher.set_halign(gtk::Align::Start);
    switcher.set_hexpand(true);

    header.append(&title);
    header.append(&switcher);

    let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
    close_btn.add_css_class("metis-dashboard-close");
    close_btn.connect_clicked(|_| request_close());
    header.append(&close_btn);

    let cpu_hist = Rc::new(RefCell::new(Vec::new()));
    let cpu_core_hist = Rc::new(RefCell::new(Vec::new()));
    let mem_hist = Rc::new(RefCell::new(Vec::new()));
    let swap_hist = Rc::new(RefCell::new(Vec::new()));
    let has_swap = Rc::new(Cell::new(false));
    let rx_hist = Rc::new(RefCell::new(Vec::new()));
    let tx_hist = Rc::new(RefCell::new(Vec::new()));
    let disk_read_hist = Rc::new(RefCell::new(Vec::new()));
    let disk_write_hist = Rc::new(RefCell::new(Vec::new()));
    let battery_hist = Rc::new(RefCell::new(Vec::new()));

    let overview = views::build_overview();
    charts::wire_multi_core_chart(&overview.cpu_chart, cpu_core_hist.clone(), cpu_hist.clone());
    charts::wire_memory_chart(
        &overview.mem_chart,
        mem_hist.clone(),
        swap_hist.clone(),
        has_swap.clone(),
    );
    charts::wire_dual_rate_chart(&overview.net_chart, rx_hist.clone(), tx_hist.clone());
    charts::wire_dual_rate_chart(
        &overview.disk_io_chart,
        disk_read_hist.clone(),
        disk_write_hist.clone(),
    );
    charts::wire_percent_chart(&overview.battery_chart, battery_hist.clone(), true);

    let overview_scroll = gtk::ScrolledWindow::new();
    overview_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    overview_scroll.set_vexpand(true);
    overview_scroll.set_hexpand(true);
    overview_scroll.set_child(Some(&overview.widget));
    overview_scroll.add_css_class("metis-dashboard-scroll");

    let processes = views::build_processes();

    stack.add_titled(&overview_scroll, Some("overview"), &tr("Overview"));
    stack.add_titled(&processes.widget, Some("processes"), &tr("Processes"));
    stack.set_visible_child_name("overview");
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let dash = Dashboard {
        shell: shell.clone(),
        window: window.clone(),
        root,
        header,
        tab_switcher: switcher,
        stack,
        overview,
        processes,
        cpu_hist,
        cpu_core_hist,
        mem_hist,
        swap_hist,
        has_swap,
        rx_hist,
        tx_hist,
        disk_read_hist,
        disk_write_hist,
        battery_hist,
        gpu_gauges: RefCell::new(Vec::new()),
        open: Cell::new(false),
        pulling: Cell::new(false),
        animating: Cell::new(false),
        current_extent: Cell::new(0),
        max_extent: Cell::new(480),
        snapshot: RefCell::new(DashboardSnapshot::default()),
        text_filter: RefCell::new(String::new()),
        class_filter: RefCell::new(ProcessClassFilter::All),
        sort_column: RefCell::new(ProcessSortColumn::Cpu),
        sort_direction: RefCell::new(SortDirection::Desc),
        expanded_processes: RefCell::new(HashSet::new()),
        last_legend_cores: Cell::new(0),
        last_disk_sig: RefCell::new(String::new()),
        last_relayout_key: Cell::new((0, 0)),
        last_process_sig: RefCell::new(0),
    };

    dash.relayout_for_bar();
    dash.window.set_child(Some(&dash.root));
    ensure_bar_strip_geometry(shell);

    let key = gtk::EventControllerKey::new();
    // Capture before SearchEntry/dropdowns so Escape always dismisses the
    // Control Center (after closing an open process context menu).
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed(|_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            if super::processes::process_context_menu_open() {
                super::processes::dismiss_process_context_menu();
                return glib::Propagation::Stop;
            }
            request_close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    dash.root.add_controller(key);

    // Process context menus use autohide(false) (same grab issues as bar popovers).
    // Clicks inside the CC surface must dismiss them — compositor close-popovers
    // only fires for presses outside shell UI. Skip targets inside the popover so
    // menu actions still receive the click.
    let dismiss_ctx = gtk::GestureClick::builder()
        .button(gdk::BUTTON_PRIMARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    dismiss_ctx.connect_pressed(move |gesture, _, x, y| {
        if !super::processes::process_context_menu_open() {
            return;
        }
        let target = gesture
            .widget()
            .and_then(|host| host.pick(x, y, gtk::PickFlags::DEFAULT));
        if let Some(target) = target {
            let mut node = Some(target);
            while let Some(w) = node {
                if w.is::<gtk::Popover>() || w.has_css_class("metis-dash-context-menu") {
                    return;
                }
                node = w.parent();
            }
        }
        super::processes::dismiss_process_context_menu();
    });
    dash.root.add_controller(dismiss_ctx);

    let root_alloc = dash.root.clone();
    let slf_weak = dash.root.downgrade();
    root_alloc.connect_map(move |_| {
        let Some(root) = slf_weak.upgrade() else {
            return;
        };
        DASHBOARD.with(|d| {
            if let Some(dash) = d.borrow().as_ref()
                && dash.root == root
                && dash.current_extent.get() > 0
            {
                let (w, h) = dash.host_content_size(dash.current_extent.get());
                dash.relayout_for_size(w, h);
            }
        });
    });

    let filter_entry = dash.processes.search.clone();
    let list = dash.processes.list.clone();
    filter_entry.connect_search_changed(move |entry| {
        let text = entry.text().to_string();
        DASHBOARD.with(|d| {
            if let Some(dash) = d.borrow().as_ref() {
                *dash.text_filter.borrow_mut() = text;
                dash.rebuild_process_list(&list);
            }
        });
    });

    let filter_dd = dash.processes.filter.clone();
    let list = dash.processes.list.clone();
    filter_dd.connect_selected_notify(move |dd| {
        let filter = match dd.selected() {
            1 => ProcessClassFilter::UserApps,
            2 => ProcessClassFilter::System,
            _ => ProcessClassFilter::All,
        };
        DASHBOARD.with(|d| {
            if let Some(dash) = d.borrow().as_ref() {
                *dash.class_filter.borrow_mut() = filter;
                dash.rebuild_process_list(&list);
            }
        });
    });

    super::processes::wire_process_sort(&dash.processes.headers, &dash.processes.list);
    dash.processes
        .monitor_btn
        .connect_clicked(|_| super::processes::launch_process_monitor());

    {
        let stack = dash.stack.clone();
        let list = dash.processes.list.clone();
        stack.connect_visible_child_notify(move |s| {
            if s.visible_child_name().as_deref() != Some("processes") {
                return;
            }
            DASHBOARD.with(|d| {
                if let Some(dash) = d.borrow().as_ref() {
                    *dash.last_process_sig.borrow_mut() = 0;
                    dash.rebuild_process_list(&list);
                }
            });
        });
    }

    let header_drag = gtk::GestureDrag::new();
    header_drag.set_button(0);
    {
        let header = dash.header.clone();
        header_drag.connect_drag_begin(move |gesture, start_x, start_y| {
            // Tab switcher / close must keep the click — don't treat them as a
            // dismiss-drag on the header chrome.
            if let Some(target) = header.pick(start_x, start_y, gtk::PickFlags::DEFAULT) {
                let mut node = Some(target);
                while let Some(w) = node {
                    if w.has_css_class("metis-dash-tabs")
                        || w.has_css_class("metis-dashboard-close")
                        || w.type_().name() == "GtkStackSwitcher"
                        || w.is::<gtk::Button>()
                    {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                        return;
                    }
                    node = w.parent();
                }
            }
        });
    }
    header_drag.connect_drag_update(|gesture, offset_x, offset_y| {
        let position = load_bar_config().position;
        if close_delta(position, offset_x, offset_y) > OPEN_THRESHOLD {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            request_close();
        }
    });
    dash.header.add_controller(header_drag);

    dash.refresh_sort_headers();
    dash.apply_widget_config();
    dash
}

impl super::Dashboard {
    pub(crate) fn apply_widget_config(&self) {
        let cfg = load_dashboard_config();
        let enabled = |id: DashboardWidgetId| cfg.widgets.contains(&id);
        self.overview
            .cpu_card
            .set_visible(enabled(DashboardWidgetId::Cpu));
        self.overview
            .mem_card
            .set_visible(enabled(DashboardWidgetId::Memory));
        self.overview
            .disk_card
            .set_visible(enabled(DashboardWidgetId::Disk));
        self.overview
            .disk_io_card
            .set_visible(enabled(DashboardWidgetId::Disk));
        self.overview
            .net_card
            .set_visible(enabled(DashboardWidgetId::Network));
        let show_battery =
            enabled(DashboardWidgetId::Battery) && self.snapshot.borrow().battery_percent.is_some();
        self.overview.battery_card.set_visible(show_battery);
        self.overview
            .logs_card
            .set_visible(enabled(DashboardWidgetId::Logs));
        let show_processes = enabled(DashboardWidgetId::Processes);
        self.processes.widget.set_visible(show_processes);
        if !show_processes && self.stack.visible_child_name().as_deref() == Some("processes") {
            self.stack.set_visible_child_name("overview");
        }
    }

    pub(crate) fn relayout_for_bar(&self) {
        let position = load_bar_config().position;
        self.max_extent.set(compute_max_extent(
            position,
            self.shell.window.monitor().as_ref(),
        ));
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        for class in [
            "metis-dashboard-root-bottom",
            "metis-dashboard-root-left",
            "metis-dashboard-root-right",
        ] {
            self.root.remove_css_class(class);
        }
        match position {
            BarPosition::Bottom => {
                // Header sits against the pill (bottom of the panel).
                self.root.add_css_class("metis-dashboard-root-bottom");
                self.root.append(&self.stack);
                self.root.append(&self.header);
            }
            BarPosition::Left => {
                self.root.add_css_class("metis-dashboard-root-left");
                self.root.append(&self.header);
                self.root.append(&self.stack);
            }
            BarPosition::Right => {
                self.root.add_css_class("metis-dashboard-root-right");
                self.root.append(&self.header);
                self.root.append(&self.stack);
            }
            BarPosition::Top => {
                self.root.append(&self.header);
                self.root.append(&self.stack);
            }
        }
        self.apply_extent(self.current_extent.get());
    }

    pub(crate) fn set_closed_state(&self) {
        self.open.set(false);
        self.pulling.set(false);
        self.current_extent.set(0);
        self.root.set_opacity(0.0);
        self.root.set_sensitive(false);
        self.apply_extent(0);
    }

    pub(crate) fn set_pull_preview(&self, extent: i32) {
        let max = self.max_extent.get().max(1);
        let e = extent.clamp(0, max);
        self.current_extent.set(e);
        let fade = (e as f64 / 96.0).clamp(0.0, 1.0);
        self.root.set_opacity(fade);
        self.root.set_sensitive(false);
        self.apply_extent(e);
    }

    pub(crate) fn snap_open(&self) {
        if self.open.get() && self.animating.get() {
            return;
        }
        if self.open.get() && self.current_extent.get() >= self.max_extent.get() {
            return;
        }
        super::dropdown::request_close_all();
        self.pulling.set(false);
        self.max_extent.set(compute_max_extent(
            load_bar_config().position,
            self.shell.window.monitor().as_ref(),
        ));
        if self.current_extent.get() == 0 {
            self.root.set_opacity(0.0);
            self.root.set_sensitive(false);
        }
        self.open.set(false);
        self.animate_to(self.max_extent.get());
    }

    pub(crate) fn snap_closed(&self) {
        if !self.open.get() && self.current_extent.get() == 0 {
            self.set_closed_state();
            teardown_dashboard();
            return;
        }
        self.open.set(false);
        self.animate_to(0);
    }

    pub(crate) fn redraw_for_theme(&self) {
        self.overview.cpu_chart.queue_draw();
        self.overview.mem_chart.queue_draw();
        self.overview.net_chart.queue_draw();
        self.overview.disk_io_chart.queue_draw();
        self.overview.cpu_temp.gauge.queue_draw();
        for gauge in self.gpu_gauges.borrow().iter() {
            gauge.gauge.queue_draw();
        }
        let core_count = self.snapshot.borrow().cpu_per_core.len();
        self.last_legend_cores.set(0);
        self.sync_cpu_legend(core_count);
    }

    pub(crate) fn apply_extent(&self, extent: i32) {
        // Edge bar geometry stays fixed — only the CC layer surface changes.
        ensure_bar_strip_geometry(&self.shell);
        self.apply_panel_extent(extent);
        let (width, height) = self.host_content_size(extent);
        if width > 0 && height > 0 {
            self.relayout_for_size(width, height);
        } else if extent > 0 {
            let slf = DASHBOARD.with(|d| d.borrow().clone());
            glib::idle_add_local_once(move || {
                if let Some(dash) = slf
                    && dash.current_extent.get() > 0
                {
                    let (w, h) = dash.host_content_size(dash.current_extent.get());
                    dash.relayout_for_size(w, h);
                }
            });
        }
    }

    /// Size/place the dedicated control-center layer surface.
    pub(crate) fn apply_panel_extent(&self, extent: i32) {
        let e = extent.max(0);
        let cfg = load_bar_config();
        // Attach to the inner edge of the pill (margin + height), not the shadow pad.
        let attach = metis_config::bar::bar_pill_inset(&cfg);
        let side = metis_config::bar::bar_pill_side_inset(&cfg);

        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            self.window.set_anchor(edge, false);
            self.window.set_margin(edge, 0);
        }
        self.window.set_exclusive_zone(-1);

        if e == 0 {
            self.window.set_visible(false);
            return;
        }

        match cfg.position {
            BarPosition::Bottom => {
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_anchor(Edge::Left, true);
                self.window.set_anchor(Edge::Right, true);
                self.window.set_margin(Edge::Bottom, attach);
                self.window.set_margin(Edge::Left, side);
                self.window.set_margin(Edge::Right, side);
                self.window.set_height_request(e);
                self.window.set_default_size(-1, e);
            }
            BarPosition::Top => {
                self.window.set_anchor(Edge::Top, true);
                self.window.set_anchor(Edge::Left, true);
                self.window.set_anchor(Edge::Right, true);
                self.window.set_margin(Edge::Top, attach);
                self.window.set_margin(Edge::Left, side);
                self.window.set_margin(Edge::Right, side);
                self.window.set_height_request(e);
                self.window.set_default_size(-1, e);
            }
            BarPosition::Left => {
                self.window.set_anchor(Edge::Left, true);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_margin(Edge::Left, attach);
                self.window.set_margin(Edge::Top, side);
                self.window.set_margin(Edge::Bottom, side);
                self.window.set_width_request(e);
                self.window.set_default_size(e, -1);
            }
            BarPosition::Right => {
                self.window.set_anchor(Edge::Right, true);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_margin(Edge::Right, attach);
                self.window.set_margin(Edge::Top, side);
                self.window.set_margin(Edge::Bottom, side);
                self.window.set_width_request(e);
                self.window.set_default_size(e, -1);
            }
        }

        if let Some(monitor) = self.shell.window.monitor() {
            self.window.set_monitor(Some(&monitor));
        }
        self.window.set_visible(true);
        self.window.present();
        self.window.queue_resize();
    }

    /// Panel content box size for the current bar edge.
    pub(crate) fn host_content_size(&self, extent: i32) -> (i32, i32) {
        let position = load_bar_config().position;
        let (mon_w, mon_h) = monitor_size(self.shell.window.monitor().as_ref());
        let side = metis_config::bar::bar_pill_side_inset(&load_bar_config());
        match position {
            BarPosition::Left | BarPosition::Right => {
                let h = (mon_h - 2 * side).max(1);
                (extent.max(1), h)
            }
            BarPosition::Top | BarPosition::Bottom => {
                let w = (mon_w - 2 * side).max(1);
                (w, extent.max(1))
            }
        }
    }

    pub(crate) fn relayout_for_size(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }

        let (_, nat_h, _, _) = self.header.measure(gtk::Orientation::Vertical, -1);
        let header_h = nat_h;
        let content_h = (height - header_h).max(120);

        let session_w = ((width as f64 * 0.32).round() as i32).clamp(220, 320);
        self.overview.session_card.set_size_request(session_w, -1);

        let cpu_h = ((content_h as f64 * 0.36).round() as i32).clamp(96, 200);
        let row_h = ((content_h as f64 * 0.22).round() as i32).clamp(72, 120);
        self.overview.cpu_chart.set_content_height(cpu_h);
        self.overview.mem_chart.set_content_height(cpu_h);
        self.overview.net_chart.set_content_height(row_h);
        self.overview.disk_io_chart.set_content_height(row_h);
    }

    pub(crate) fn animate_to(&self, target: i32) {
        self.animating.set(true);
        let start = self.current_extent.get();
        let delta = target - start;
        let start_at = glib::monotonic_time();
        let duration_us = (SNAP_MS as i64) * 1000;
        let slf = DASHBOARD.with(|d| d.borrow().clone()).unwrap();

        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let elapsed = glib::monotonic_time() - start_at;
            let t = (elapsed as f64 / duration_us as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            let e = start + (delta as f64 * eased) as i32;
            slf.current_extent.set(e);
            if e > 0 {
                let max_extent = slf.max_extent.get().max(1) as f64;
                let fade = (e as f64 / max_extent).clamp(0.0, 1.0);
                slf.root.set_opacity(if t >= 1.0 && target > 0 {
                    1.0
                } else {
                    fade.max(0.05)
                });
            }
            slf.apply_extent(e);
            if t >= 1.0 {
                slf.animating.set(false);
                if target == 0 {
                    slf.set_closed_state();
                    teardown_dashboard();
                } else {
                    slf.open.set(true);
                    slf.root.set_sensitive(true);
                    slf.root.set_opacity(1.0);
                    let _ = slf.window.grab_focus();
                }
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    pub(crate) fn update(&self, snapshot: &DashboardSnapshot) {
        *self.snapshot.borrow_mut() = snapshot.clone();
        *self.cpu_hist.borrow_mut() = snapshot.cpu_history.clone();
        *self.cpu_core_hist.borrow_mut() = snapshot.cpu_core_histories.clone();
        *self.mem_hist.borrow_mut() = snapshot.mem_percent_history.clone();
        *self.swap_hist.borrow_mut() = snapshot.swap_percent_history.clone();
        self.has_swap.set(snapshot.swap_total_bytes > 0);
        *self.rx_hist.borrow_mut() = snapshot.net_rx_history.clone();
        *self.tx_hist.borrow_mut() = snapshot.net_tx_history.clone();
        *self.disk_read_hist.borrow_mut() = snapshot.disk_read_history.clone();
        *self.disk_write_hist.borrow_mut() = snapshot.disk_write_history.clone();
        *self.battery_hist.borrow_mut() = snapshot.battery_history.clone();

        self.overview.cpu_value.set_text(&format!(
            "{:.0}% {} · {} {}",
            snapshot.cpu_percent,
            tr("total"),
            snapshot.cpu_per_core.len().max(1),
            tr("cores")
        ));
        self.overview.cpu_chart.queue_draw();
        self.sync_cpu_legend(snapshot.cpu_per_core.len());

        let mem_pct = pct(snapshot.memory_used_bytes, snapshot.memory_total_bytes);
        self.overview.mem_value.set_text(&format!(
            "{} / {} ({:.0}%)",
            format_bytes(snapshot.memory_used_bytes),
            format_bytes(snapshot.memory_total_bytes),
            mem_pct
        ));
        self.overview.mem_chart.queue_draw();
        self.overview
            .mem_legend
            .set_visible(snapshot.swap_total_bytes > 0);

        match snapshot.battery_percent {
            Some(pct) => {
                let charge = if snapshot.battery_charging {
                    tr("Charging")
                } else {
                    tr("On battery")
                };
                self.overview
                    .battery_value
                    .set_text(&format!("{pct:.0}% · {charge}"));
                self.overview.battery_chart.queue_draw();
            }
            None => {
                self.overview.battery_value.set_text("—");
            }
        }

        if snapshot.logs_available {
            let text = if snapshot.log_lines.is_empty() {
                tr("No recent journal entries.")
            } else {
                snapshot.log_lines.join("\n")
            };
            self.overview.logs_buffer.set_text(&text);
        } else {
            self.overview
                .logs_buffer
                .set_text(&tr("Journal unavailable (install systemd journal tools)."));
        }

        // Re-evaluate battery visibility when supply appears/disappears.
        self.apply_widget_config();

        self.overview.load_label.set_text(&format!(
            "{:.2}  {:.2}  {:.2}",
            snapshot.load_avg[0], snapshot.load_avg[1], snapshot.load_avg[2]
        ));
        self.overview
            .uptime_label
            .set_text(&format_uptime(snapshot.uptime_secs));

        self.overview.eth_down.set_text(&format!(
            "{} {}",
            tr("Ethernet ↓"),
            format_rate(snapshot.ethernet_rx_bps)
        ));
        self.overview.eth_up.set_text(&format!(
            "{} {}",
            tr("Ethernet ↑"),
            format_rate(snapshot.ethernet_tx_bps)
        ));
        self.overview.wifi_down.set_text(&format!(
            "{} {}",
            tr("Wi‑Fi ↓"),
            format_rate(snapshot.wifi_rx_bps)
        ));
        self.overview.wifi_up.set_text(&format!(
            "{} {}",
            tr("Wi‑Fi ↑"),
            format_rate(snapshot.wifi_tx_bps)
        ));
        self.overview.net_chart.queue_draw();

        let fw = &snapshot.firewall;
        if fw.active {
            self.overview.firewall_status.set_text(&format!(
                "{} · {}",
                tr("Firewall active"),
                fw.backend
            ));
        } else {
            self.overview.firewall_status.set_text(&format!(
                "{} · {}",
                tr("Firewall inactive"),
                fw.backend
            ));
        }

        self.overview.disk_io_value.set_text(&format!(
            "↓ {}  ↑ {}",
            format_rate(snapshot.disk_read_bps),
            format_rate(snapshot.disk_write_bps)
        ));
        self.overview.disk_io_chart.queue_draw();
        self.sync_disk_tiles(&snapshot.disks);

        let hw = &snapshot.hardware;
        self.overview.hostname.set_text(&hw.hostname);
        self.overview.cpu_model.set_text(&hw.cpu_model);
        self.overview.cpu_cores.set_text(&hw.cpu_cores.to_string());
        self.overview.system_memory.set_text(&format!(
            "{} {}",
            format_bytes(snapshot.memory_total_bytes),
            tr("total")
        ));
        let kernel_short = short_kernel_version(&hw.kernel);
        self.overview.kernel.set_text(&kernel_short);
        self.overview.kernel.set_tooltip_text(Some(&hw.kernel));
        self.overview
            .cpu_model
            .set_tooltip_text(Some(&hw.cpu_model));

        set_temp_label(&self.overview.cpu_temp.value, snapshot.cpu_temp_celsius);
        *self.overview.cpu_temp.temp.borrow_mut() = snapshot.cpu_temp_celsius;
        self.overview.cpu_temp.gauge.queue_draw();
        self.sync_gpu_gauges(&snapshot.gpu_temps);

        if self.processes_tab_active() {
            let sig = super::processes::process_list_sig(&snapshot.processes);
            if sig != *self.last_process_sig.borrow() {
                *self.last_process_sig.borrow_mut() = sig;
                self.rebuild_process_list(&self.processes.list);
            }
        }

        if self.open.get() {
            let (w, h) = self.host_content_size(self.current_extent.get());
            let key = (w, h);
            if key != self.last_relayout_key.get() {
                self.last_relayout_key.set(key);
                self.relayout_for_size(key.0, key.1);
            }
        }
    }

    pub(crate) fn processes_tab_active(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("processes")
    }

    pub(crate) fn sync_cpu_legend(&self, core_count: usize) {
        if core_count == self.last_legend_cores.get() {
            return;
        }
        self.last_legend_cores.set(core_count);
        while let Some(child) = self.overview.cpu_legend.first_child() {
            self.overview.cpu_legend.remove(&child);
        }
        let legend_cap = core_count.min(16);
        for i in 0..legend_cap {
            let label = if core_count <= 16 {
                format!("C{i}")
            } else if i == 15 {
                format!("+{}", core_count - 15)
            } else {
                format!("C{i}")
            };
            self.overview
                .cpu_legend
                .append(&views::legend_chip(i, &label));
        }
        if core_count > 0 {
            self.overview
                .cpu_legend
                .append(&views::aggregate_legend_chip(&tr("Σ total")));
        }
    }

    pub(crate) fn sync_disk_tiles(&self, disks: &[crate::services::DiskMount]) {
        let sig: String = disks
            .iter()
            .map(|d| format!("{}:{}:{}", d.mount_point, d.used_bytes, d.total_bytes))
            .collect::<Vec<_>>()
            .join("|");
        if sig == *self.last_disk_sig.borrow() {
            return;
        }
        *self.last_disk_sig.borrow_mut() = sig;
        while let Some(child) = self.overview.disk_box.first_child() {
            self.overview.disk_box.remove(&child);
        }
        for disk in disks {
            let disk_pct = pct(disk.used_bytes, disk.total_bytes);
            let tile = views::disk_mount_card(
                &disk.mount_point,
                disk_pct,
                &format_bytes(disk.used_bytes),
                &format_bytes(disk.total_bytes),
            );
            self.overview.disk_box.append(&tile);
        }
    }

    pub(crate) fn sync_gpu_gauges(&self, readings: &[GpuTempReading]) {
        let desired = readings.len();
        let mut slots = self.gpu_gauges.borrow_mut();

        while slots.len() > desired {
            let slot = slots.pop().expect("slot count");
            self.overview.temp_gauges.remove(&slot.card);
        }

        while slots.len() < desired {
            let (card, gauge_card) = views::build_temp_gauge_card(
                &tr("GPU"),
                &[
                    "video-display-symbolic",
                    "display-brightness-symbolic",
                    "computer-symbolic",
                ],
            );
            self.overview.temp_gauges.append(&card);
            slots.push(gauge_card);
        }

        for (slot, reading) in slots.iter_mut().zip(readings.iter()) {
            slot.title.set_text(&reading.label);
            let value = match reading.util_percent {
                Some(util) => format!("{:.0}°C · {:.0}%", reading.temp_celsius, util),
                None => format!("{:.0}°C", reading.temp_celsius),
            };
            slot.value.set_text(&value);
            *slot.temp.borrow_mut() = Some(reading.temp_celsius);
            slot.gauge.queue_draw();
        }
    }

    pub(crate) fn refresh_sort_headers(&self) {
        let col = *self.sort_column.borrow();
        let dir = *self.sort_direction.borrow();
        let h = &self.processes.headers;
        super::processes::set_sort_label(&h.name, &tr("Name"), col == ProcessSortColumn::Name, dir);
        super::processes::set_sort_label(&h.pid, &tr("PID"), col == ProcessSortColumn::Pid, dir);
        super::processes::set_sort_label(&h.user, &tr("User"), col == ProcessSortColumn::User, dir);
        super::processes::set_sort_label(&h.kind, &tr("Type"), col == ProcessSortColumn::Kind, dir);
        super::processes::set_sort_label(&h.cpu, &tr("CPU"), col == ProcessSortColumn::Cpu, dir);
        super::processes::set_sort_label(
            &h.memory,
            &tr("Memory"),
            col == ProcessSortColumn::Memory,
            dir,
        );
    }

    pub(crate) fn rebuild_process_list(&self, list: &gtk::ListBox) {
        if super::processes::process_context_menu_open() {
            return;
        }
        self.refresh_sort_headers();
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let text_filter = self.text_filter.borrow().to_lowercase();
        let class_filter = *self.class_filter.borrow();
        let sort_col = *self.sort_column.borrow();
        let sort_dir = *self.sort_direction.borrow();

        let snapshot = self.snapshot.borrow();
        let all = &snapshot.processes;
        let by_pid: HashMap<u32, &ProcessRow> = all.iter().map(|p| (p.pid, p)).collect();

        let mut matched: HashSet<u32> = HashSet::new();
        for proc in all {
            let text_ok = text_filter.is_empty()
                || proc.name.to_lowercase().contains(&text_filter)
                || proc.user.to_lowercase().contains(&text_filter)
                || proc.pid.to_string().contains(&text_filter);
            if text_ok && super::processes::matches_class_filter(class_filter, proc.class) {
                matched.insert(proc.pid);
            }
        }

        // Keep ancestors of matches so filtered children stay under their tree.
        let mut visible: HashSet<u32> = matched.clone();
        for &pid in &matched {
            let mut walk = by_pid.get(&pid).and_then(|p| p.parent_pid);
            while let Some(ppid) = walk {
                if !visible.insert(ppid) {
                    break;
                }
                walk = by_pid.get(&ppid).and_then(|p| p.parent_pid);
            }
        }

        // Auto-expand ancestors when searching so matches are reachable.
        if !text_filter.is_empty() {
            let mut expand = self.expanded_processes.borrow_mut();
            for &pid in &matched {
                let mut walk = by_pid.get(&pid).and_then(|p| p.parent_pid);
                while let Some(ppid) = walk {
                    expand.insert(ppid);
                    walk = by_pid.get(&ppid).and_then(|p| p.parent_pid);
                }
            }
        }

        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut roots: Vec<u32> = Vec::new();
        for proc in all.iter().filter(|p| visible.contains(&p.pid)) {
            let parent_in_view = proc.parent_pid.is_some_and(|ppid| visible.contains(&ppid));
            if parent_in_view {
                if let Some(ppid) = proc.parent_pid {
                    children.entry(ppid).or_default().push(proc.pid);
                }
            } else {
                roots.push(proc.pid);
            }
        }

        let sort_pids = |pids: &mut [u32]| {
            pids.sort_by(|a, b| {
                let Some(pa) = by_pid.get(a) else {
                    return std::cmp::Ordering::Equal;
                };
                let Some(pb) = by_pid.get(b) else {
                    return std::cmp::Ordering::Equal;
                };
                super::processes::compare_process_rows(pa, pb, sort_col, sort_dir)
            });
        };
        sort_pids(&mut roots);
        for kids in children.values_mut() {
            sort_pids(kids);
        }

        let expanded = self.expanded_processes.borrow().clone();
        let mut flat: Vec<super::processes::ProcessTreeEntry> = Vec::new();
        for root in roots {
            super::processes::flatten_process_tree(
                root, 0, &children, &by_pid, &expanded, &mut flat,
            );
        }

        let mut shown = 0usize;
        let truncated = flat.len() > 300;
        for entry in flat.into_iter().take(300) {
            list.append(&super::processes::process_row(&entry));
            shown += 1;
        }
        if shown == 0 {
            let empty = gtk::Label::new(Some(&tr("No matching processes")));
            empty.add_css_class("metis-dash-muted");
            empty.set_margin_top(12);
            empty.set_margin_start(16);
            empty.set_halign(gtk::Align::Start);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&empty));
            list.append(&row);
        } else if truncated {
            let more = gtk::Label::new(Some(&tr(
                "Showing first 300 visible rows — refine the filter",
            )));
            more.add_css_class("metis-dash-muted");
            more.set_margin_top(8);
            more.set_margin_start(16);
            more.set_halign(gtk::Align::Start);
            let row = gtk::ListBoxRow::new();
            row.set_sensitive(false);
            row.set_child(Some(&more));
            list.append(&row);
        }
    }
}

pub(crate) fn set_temp_label(label: &gtk::Label, temp: Option<f32>) {
    match temp {
        Some(c) => label.set_text(&format!("{c:.0}°C")),
        None => label.set_text(&tr("N/A")),
    }
}

pub(crate) fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

pub(crate) fn compute_max_extent(
    position: BarPosition,
    monitor: Option<&gtk::gdk::Monitor>,
) -> i32 {
    let (mon_w, mon_h) = monitor_size(monitor);
    let cfg = load_bar_config();
    let dash_cfg = load_dashboard_config();
    let pct = (dash_cfg.max_height_percent.clamp(20, 100) as f64) / 100.0;
    let cross = cfg.height as i32;
    let edge = cfg.margin_top as i32;
    match position {
        BarPosition::Top | BarPosition::Bottom => {
            let available = (mon_h - edge - cross).max(0);
            ((available as f64) * pct).round() as i32
        }
        BarPosition::Left | BarPosition::Right => {
            let available = (mon_w - edge - cross).max(0);
            ((available as f64) * pct).round() as i32
        }
    }
    .max(320)
}

pub(crate) fn monitor_size(monitor: Option<&gtk::gdk::Monitor>) -> (i32, i32) {
    if let Some(monitor) = monitor {
        let g = monitor.geometry();
        if g.width() > 0 && g.height() > 0 {
            return (g.width(), g.height());
        }
    }
    if let Some(display) = gtk::gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gtk::gdk::Monitor>()
    {
        let g = monitor.geometry();
        if g.width() > 0 && g.height() > 0 {
            return (g.width(), g.height());
        }
    }
    (1280, 720)
}
