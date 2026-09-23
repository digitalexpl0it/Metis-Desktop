//! Pull-down / pull-aside system dashboard (Phase 10).
//!
//! The control center is a separate layer-shell surface attached just inside the
//! edge-bar pill. The bar strip itself never resizes when the panel opens.

mod charts;
mod views;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::{BarPosition, DashboardWidgetId, load_bar_config, load_dashboard_config};
use metis_i18n::tr;

use crate::services::{
    DashboardSnapshot, GpuTempReading, ProcessClass, ProcessRow, format_bytes, format_rate,
    format_uptime, kill_process, kill_process_tree, short_kernel_version,
};
use crate::ui::bar::{BarShell, ensure_bar_strip_geometry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProcessClassFilter {
    #[default]
    All,
    UserApps,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProcessSortColumn {
    #[default]
    Cpu,
    Name,
    Pid,
    User,
    Kind,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SortDirection {
    #[default]
    Desc,
    Asc,
}

const SNAP_MS: u32 = 220;
const OPEN_THRESHOLD: f64 = 72.0;
const PULL_START_SLOP: f64 = 6.0;

thread_local! {
    static DASHBOARD: RefCell<Option<Rc<Dashboard>>> = const { RefCell::new(None) };
    static POLL_ATTACHED: Cell<bool> = const { Cell::new(false) };
    /// Open process-row context menu; list rebuild is paused while set.
    static PROCESS_CONTEXT_MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

struct Dashboard {
    shell: BarShell,
    /// Dedicated layer-shell window — never shares geometry with the edge bar.
    window: gtk::Window,
    root: gtk::Box,
    header: gtk::Box,
    tab_switcher: gtk::StackSwitcher,
    stack: gtk::Stack,
    overview: views::OverviewPage,
    processes: views::ProcessPage,
    cpu_hist: Rc<RefCell<Vec<f32>>>,
    cpu_core_hist: Rc<RefCell<Vec<Vec<f32>>>>,
    mem_hist: Rc<RefCell<Vec<f32>>>,
    swap_hist: Rc<RefCell<Vec<f32>>>,
    has_swap: Rc<Cell<bool>>,
    rx_hist: Rc<RefCell<Vec<f64>>>,
    tx_hist: Rc<RefCell<Vec<f64>>>,
    disk_read_hist: Rc<RefCell<Vec<f64>>>,
    disk_write_hist: Rc<RefCell<Vec<f64>>>,
    battery_hist: Rc<RefCell<Vec<f32>>>,
    gpu_gauges: RefCell<Vec<views::TempGaugeCard>>,
    open: Cell<bool>,
    pulling: Cell<bool>,
    animating: Cell<bool>,
    current_extent: Cell<i32>,
    max_extent: Cell<i32>,
    snapshot: RefCell<DashboardSnapshot>,
    text_filter: RefCell<String>,
    class_filter: RefCell<ProcessClassFilter>,
    sort_column: RefCell<ProcessSortColumn>,
    sort_direction: RefCell<SortDirection>,
    /// PIDs whose children are shown in the Processes tree.
    expanded_processes: RefCell<HashSet<u32>>,
    last_legend_cores: Cell<usize>,
    last_disk_sig: RefCell<String>,
    last_relayout_key: Cell<(i32, i32)>,
    last_process_sig: RefCell<u64>,
}

pub fn init() {
    if let Err(err) = metis_config::save_default_dashboard_config() {
        tracing::warn!(%err, "failed to write default dashboard.json");
    }
}

/// Press on the bar pill and drag toward the desktop to pull the dashboard open.
pub fn wire_bar_pull(pill: &gtk::Box, shell: &BarShell) {
    let pill_weak = pill.downgrade();
    let _shell_pull = shell.clone();

    let drag = gtk::GestureDrag::new();
    drag.set_button(0);
    drag.set_touch_only(false);

    drag.connect_drag_begin(move |gesture, start_x, start_y| {
        if !load_dashboard_config().enabled {
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        let Some(pill) = pill_weak.upgrade() else {
            return;
        };
        if press_on_bar_widget(&pill, start_x, start_y) {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });

    let shell_update = shell.clone();
    drag.connect_drag_update(move |gesture, offset_x, offset_y| {
        if !load_dashboard_config().enabled {
            return;
        }
        let position = load_bar_config().position;
        let delta = pull_delta(position, offset_x, offset_y);
        if delta < PULL_START_SLOP {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let dash = ensure_dashboard(&shell_update);
        if dash.open.get() && !dash.pulling.get() {
            return;
        }
        if !dash.pulling.get() {
            dropdown::request_close_all();
            dash.pulling.set(true);
            dash.max_extent.set(compute_max_extent(
                position,
                dash.shell.window.monitor().as_ref(),
            ));
        }
        let max = dash.max_extent.get().max(1);
        let extent = (delta as i32).clamp(0, max);
        dash.set_pull_preview(extent);
    });

    let shell_end = shell.clone();
    drag.connect_drag_end(move |_, offset_x, offset_y| {
        let position = load_bar_config().position;
        let delta = pull_delta(position, offset_x, offset_y);
        let Some(dash) = DASHBOARD.with(|d| d.borrow().clone()) else {
            return;
        };
        dash.pulling.set(false);
        if dash.open.get() {
            return;
        }
        let monitor = shell_end.window.monitor();
        let max = compute_max_extent(position, monitor.as_ref());
        dash.max_extent.set(max);
        if delta >= OPEN_THRESHOLD {
            dash.snap_open();
        } else {
            dash.snap_closed();
        }
    });

    pill.add_controller(drag);
}

pub fn on_theme_changed() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.header.queue_draw();
            dash.tab_switcher.queue_draw();
            dash.redraw_for_theme();
        }
    });
}

pub fn on_bar_config_changed() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.relayout_for_bar();
            if !dash.open.get() && !dash.pulling.get() {
                dash.set_closed_state();
            } else if dash.open.get() {
                dash.max_extent.set(compute_max_extent(
                    load_bar_config().position,
                    dash.shell.window.monitor().as_ref(),
                ));
                dash.apply_extent(dash.max_extent.get());
            }
        }
    });
}

pub fn on_dashboard_config_changed() {
    crate::ui::bar::sync_control_center_button();
    crate::ui::bar::refresh_workspaces();

    if !load_dashboard_config().enabled {
        request_close();
    }

    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.apply_widget_config();
            let position = load_bar_config().position;
            let max = compute_max_extent(position, dash.shell.window.monitor().as_ref());
            dash.max_extent.set(max);
            if dash.open.get() {
                dash.apply_extent(max);
            } else if !dash.pulling.get() {
                dash.set_closed_state();
            }
        }
    });
}

/// Drop a built Control Center so the next open uses freshly translated chrome.
pub fn reload_for_locale() {
    let was_open = DASHBOARD.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|dash| dash.open.get() || dash.current_extent.get() > 0)
    });
    teardown_dashboard();
    if was_open {
        // Reopen requires a bar shell; next toggle / pull rebuilds lazily.
        tracing::debug!("control center torn down for locale; reopen from the edge bar");
    }
}

pub fn attach_poll_channel(rx: Receiver<DashboardSnapshot>) {
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        while let Ok(snapshot) = rx.try_recv() {
            apply_snapshot(&snapshot);
        }
        glib::ControlFlow::Continue
    });
}

pub fn request_close() {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.snap_closed();
        }
    });
}

/// True while the control center panel is open (or mid-open animation).
pub fn is_open() -> bool {
    DASHBOARD.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|dash| dash.open.get() || dash.window.is_visible())
    })
}

/// Demote Exclusive keyboard while the screenshot Overlay owns input so Control
/// Center stays visible for capture without covering Esc / type-ahead.
pub fn set_below_screenshot(below: bool) {
    DASHBOARD.with(|d| {
        let borrow = d.borrow();
        let Some(dash) = borrow.as_ref() else {
            return;
        };
        if !dash.window.is_visible() && !dash.open.get() {
            return;
        }
        dash.window.set_keyboard_mode(if below {
            KeyboardMode::OnDemand
        } else {
            KeyboardMode::Exclusive
        });
    });
}

/// Open or close the control center with the same slide animation as a bar pull.
pub fn request_toggle(shell: &BarShell) {
    if !load_dashboard_config().enabled {
        return;
    }
    crate::ui::bar::notify_bar_interaction();
    let dash = ensure_dashboard(shell);
    if dash.open.get() || dash.current_extent.get() > OPEN_THRESHOLD as i32 / 2 {
        dash.snap_closed();
    } else {
        dash.snap_open();
    }
}

fn ensure_dashboard(shell: &BarShell) -> Rc<Dashboard> {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            if dash.shell.window != shell.window {
                teardown_dashboard();
            } else {
                return dash.clone();
            }
        }
        tracing::debug!("lazy-starting system dashboard");
        let dash = Rc::new(build_dashboard(shell));
        dash.set_closed_state();
        *d.borrow_mut() = Some(dash.clone());
        start_polling();
        dash
    })
}

fn start_polling() {
    if POLL_ATTACHED.get() {
        crate::services::set_polling_active(true);
        return;
    }
    POLL_ATTACHED.set(true);
    attach_poll_channel(crate::services::spawn_dashboard_pollers());
}

fn teardown_dashboard() {
    crate::services::set_polling_active(false);
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow_mut().take() {
            dash.set_closed_state();
            dash.window.set_visible(false);
            dash.window.destroy();
            ensure_bar_strip_geometry(&dash.shell);
        }
    });
    tracing::debug!("system dashboard torn down (idle)");
}

fn apply_snapshot(snapshot: &DashboardSnapshot) {
    DASHBOARD.with(|d| {
        if let Some(dash) = d.borrow().as_ref() {
            dash.update(snapshot);
        }
    });
}

fn press_on_bar_widget(pill: &gtk::Box, x: f64, y: f64) -> bool {
    let Some(target) = pill.pick(x, y, gtk::PickFlags::DEFAULT) else {
        return false;
    };
    let mut node = Some(target);
    while let Some(w) = node {
        if w.has_css_class("metis-bar-widget") {
            return true;
        }
        node = w.parent();
    }
    false
}

fn pull_delta(position: BarPosition, offset_x: f64, offset_y: f64) -> f64 {
    match position {
        BarPosition::Top => offset_y,
        BarPosition::Bottom => -offset_y,
        BarPosition::Left => offset_x,
        BarPosition::Right => -offset_x,
    }
}

fn close_delta(position: BarPosition, offset_x: f64, offset_y: f64) -> f64 {
    match position {
        BarPosition::Top => -offset_y,
        BarPosition::Bottom => offset_y,
        BarPosition::Left => -offset_x,
        BarPosition::Right => offset_x,
    }
}

fn build_dashboard(shell: &BarShell) -> Dashboard {
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
            if process_context_menu_open() {
                dismiss_process_context_menu();
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
        if !process_context_menu_open() {
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
        dismiss_process_context_menu();
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

    wire_process_sort(&dash.processes.headers, &dash.processes.list);
    dash.processes
        .monitor_btn
        .connect_clicked(|_| launch_process_monitor());

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

impl Dashboard {
    fn apply_widget_config(&self) {
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

    fn relayout_for_bar(&self) {
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

    fn set_closed_state(&self) {
        self.open.set(false);
        self.pulling.set(false);
        self.current_extent.set(0);
        self.root.set_opacity(0.0);
        self.root.set_sensitive(false);
        self.apply_extent(0);
    }

    fn set_pull_preview(&self, extent: i32) {
        let max = self.max_extent.get().max(1);
        let e = extent.clamp(0, max);
        self.current_extent.set(e);
        let fade = (e as f64 / 96.0).clamp(0.0, 1.0);
        self.root.set_opacity(fade);
        self.root.set_sensitive(false);
        self.apply_extent(e);
    }

    fn snap_open(&self) {
        if self.open.get() && self.animating.get() {
            return;
        }
        if self.open.get() && self.current_extent.get() >= self.max_extent.get() {
            return;
        }
        dropdown::request_close_all();
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

    fn snap_closed(&self) {
        if !self.open.get() && self.current_extent.get() == 0 {
            self.set_closed_state();
            teardown_dashboard();
            return;
        }
        self.open.set(false);
        self.animate_to(0);
    }

    fn redraw_for_theme(&self) {
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

    fn apply_extent(&self, extent: i32) {
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
    fn apply_panel_extent(&self, extent: i32) {
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
    fn host_content_size(&self, extent: i32) -> (i32, i32) {
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

    fn relayout_for_size(&self, width: i32, height: i32) {
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

    fn animate_to(&self, target: i32) {
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

    fn update(&self, snapshot: &DashboardSnapshot) {
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
            let sig = process_list_sig(&snapshot.processes);
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

    fn processes_tab_active(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("processes")
    }

    fn sync_cpu_legend(&self, core_count: usize) {
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

    fn sync_disk_tiles(&self, disks: &[crate::services::DiskMount]) {
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

    fn sync_gpu_gauges(&self, readings: &[GpuTempReading]) {
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

    fn refresh_sort_headers(&self) {
        let col = *self.sort_column.borrow();
        let dir = *self.sort_direction.borrow();
        let h = &self.processes.headers;
        set_sort_label(&h.name, &tr("Name"), col == ProcessSortColumn::Name, dir);
        set_sort_label(&h.pid, &tr("PID"), col == ProcessSortColumn::Pid, dir);
        set_sort_label(&h.user, &tr("User"), col == ProcessSortColumn::User, dir);
        set_sort_label(&h.kind, &tr("Type"), col == ProcessSortColumn::Kind, dir);
        set_sort_label(&h.cpu, &tr("CPU"), col == ProcessSortColumn::Cpu, dir);
        set_sort_label(
            &h.memory,
            &tr("Memory"),
            col == ProcessSortColumn::Memory,
            dir,
        );
    }

    fn rebuild_process_list(&self, list: &gtk::ListBox) {
        if process_context_menu_open() {
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
            if text_ok && matches_class_filter(class_filter, proc.class) {
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
                compare_process_rows(pa, pb, sort_col, sort_dir)
            });
        };
        sort_pids(&mut roots);
        for kids in children.values_mut() {
            sort_pids(kids);
        }

        let expanded = self.expanded_processes.borrow().clone();
        let mut flat: Vec<ProcessTreeEntry> = Vec::new();
        for root in roots {
            flatten_process_tree(root, 0, &children, &by_pid, &expanded, &mut flat);
        }

        let mut shown = 0usize;
        let truncated = flat.len() > 300;
        for entry in flat.into_iter().take(300) {
            list.append(&process_row(&entry));
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

struct ProcessTreeEntry {
    proc: ProcessRow,
    depth: usize,
    child_count: usize,
}

fn flatten_process_tree(
    pid: u32,
    depth: usize,
    children: &HashMap<u32, Vec<u32>>,
    by_pid: &HashMap<u32, &ProcessRow>,
    expanded: &HashSet<u32>,
    out: &mut Vec<ProcessTreeEntry>,
) {
    let Some(proc) = by_pid.get(&pid) else {
        return;
    };
    let kids = children.get(&pid).map(|v| v.as_slice()).unwrap_or(&[]);
    out.push(ProcessTreeEntry {
        proc: (*proc).clone(),
        depth,
        child_count: kids.len(),
    });
    if kids.is_empty() || !expanded.contains(&pid) {
        return;
    }
    for child in kids {
        flatten_process_tree(*child, depth + 1, children, by_pid, expanded, out);
    }
}

fn compare_process_rows(
    a: &ProcessRow,
    b: &ProcessRow,
    col: ProcessSortColumn,
    dir: SortDirection,
) -> std::cmp::Ordering {
    let ord = match col {
        ProcessSortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        ProcessSortColumn::Pid => a.pid.cmp(&b.pid),
        ProcessSortColumn::User => a.user.to_lowercase().cmp(&b.user.to_lowercase()),
        ProcessSortColumn::Kind => class_order(a.class).cmp(&class_order(b.class)),
        ProcessSortColumn::Cpu => a
            .cpu_percent
            .partial_cmp(&b.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal),
        ProcessSortColumn::Memory => a.memory_bytes.cmp(&b.memory_bytes),
    };
    match dir {
        SortDirection::Asc => ord,
        SortDirection::Desc => ord.reverse(),
    }
}

fn process_list_sig(procs: &[ProcessRow]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    procs.len().hash(&mut hasher);
    for proc in procs.iter().take(48) {
        proc.pid.hash(&mut hasher);
        (proc.cpu_percent as u32).hash(&mut hasher);
        proc.memory_bytes.hash(&mut hasher);
    }
    hasher.finish()
}

fn set_sort_label(btn: &gtk::Button, title: &str, active: bool, dir: SortDirection) {
    let suffix = if active {
        match dir {
            SortDirection::Asc => " ↑",
            SortDirection::Desc => " ↓",
        }
    } else {
        ""
    };
    btn.set_label(&format!("{title}{suffix}"));
    btn.set_sensitive(true);
    if active {
        btn.add_css_class("metis-dash-sort-active");
    } else {
        btn.remove_css_class("metis-dash-sort-active");
    }
}

fn default_sort_direction(col: ProcessSortColumn) -> SortDirection {
    match col {
        ProcessSortColumn::Name | ProcessSortColumn::User | ProcessSortColumn::Kind => {
            SortDirection::Asc
        }
        ProcessSortColumn::Pid | ProcessSortColumn::Cpu | ProcessSortColumn::Memory => {
            SortDirection::Desc
        }
    }
}

fn class_order(class: ProcessClass) -> u8 {
    match class {
        ProcessClass::Metis => 0,
        ProcessClass::UserApp => 1,
        ProcessClass::System => 2,
    }
}

fn wire_process_sort(headers: &views::ProcessHeader, list: &gtk::ListBox) {
    wire_sort_btn(&headers.name, ProcessSortColumn::Name, list);
    wire_sort_btn(&headers.pid, ProcessSortColumn::Pid, list);
    wire_sort_btn(&headers.user, ProcessSortColumn::User, list);
    wire_sort_btn(&headers.kind, ProcessSortColumn::Kind, list);
    wire_sort_btn(&headers.cpu, ProcessSortColumn::Cpu, list);
    wire_sort_btn(&headers.memory, ProcessSortColumn::Memory, list);
}

fn wire_sort_btn(btn: &gtk::Button, col: ProcessSortColumn, list: &gtk::ListBox) {
    let list = list.clone();
    btn.connect_clicked(move |_| {
        DASHBOARD.with(|d| {
            let holder = d.borrow();
            let Some(dash) = holder.as_ref() else {
                return;
            };
            {
                let mut column = dash.sort_column.borrow_mut();
                let mut dir = dash.sort_direction.borrow_mut();
                if *column == col {
                    *dir = match *dir {
                        SortDirection::Asc => SortDirection::Desc,
                        SortDirection::Desc => SortDirection::Asc,
                    };
                } else {
                    *column = col;
                    *dir = default_sort_direction(col);
                }
            }
            dash.rebuild_process_list(&list);
            dash.refresh_sort_headers();
        });
    });
}

fn matches_class_filter(filter: ProcessClassFilter, class: ProcessClass) -> bool {
    match filter {
        ProcessClassFilter::All => true,
        ProcessClassFilter::UserApps => {
            matches!(class, ProcessClass::UserApp | ProcessClass::Metis)
        }
        ProcessClassFilter::System => class == ProcessClass::System,
    }
}

fn process_row(entry: &ProcessTreeEntry) -> gtk::ListBoxRow {
    let proc = &entry.proc;
    let row = gtk::ListBoxRow::new();
    row.add_css_class("metis-dash-table-row");
    let grid = gtk::Grid::builder()
        .column_spacing(8)
        .margin_top(5)
        .margin_bottom(5)
        .build();
    grid.add_css_class("metis-dash-proc-cols");

    let name_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    name_box.set_hexpand(true);
    name_box.set_halign(gtk::Align::Fill);
    name_box.set_margin_start((entry.depth as i32) * 14);

    if entry.child_count > 0 {
        let expanded = DASHBOARD.with(|d| {
            d.borrow()
                .as_ref()
                .is_some_and(|dash| dash.expanded_processes.borrow().contains(&proc.pid))
        });
        let toggle = gtk::Button::from_icon_name(if expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
        toggle.add_css_class("flat");
        toggle.add_css_class("metis-dash-proc-expand");
        let expand_tooltip = if expanded {
            tr("Collapse child processes")
        } else {
            tr("Expand child processes")
        };
        toggle.set_tooltip_text(Some(&expand_tooltip));
        let pid_toggle = proc.pid;
        toggle.connect_clicked(move |_| {
            DASHBOARD.with(|d| {
                let holder = d.borrow();
                let Some(dash) = holder.as_ref() else {
                    return;
                };
                {
                    let mut expanded = dash.expanded_processes.borrow_mut();
                    if !expanded.remove(&pid_toggle) {
                        expanded.insert(pid_toggle);
                    }
                }
                dash.rebuild_process_list(&dash.processes.list);
            });
        });
        name_box.append(&toggle);
    } else {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(24, -1);
        name_box.append(&spacer);
    }

    let name = gtk::Label::new(Some(&proc.name));
    name.set_halign(gtk::Align::Start);
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    match proc.class {
        ProcessClass::Metis => name.add_css_class("metis-dash-process-metis"),
        ProcessClass::System => name.add_css_class("metis-dash-muted"),
        ProcessClass::UserApp => name.add_css_class("metis-dash-proc-name"),
    }
    name_box.append(&name);

    let pid = gtk::Label::new(Some(&proc.pid.to_string()));
    pid.add_css_class("metis-dash-muted");
    pid.set_halign(gtk::Align::Start);

    let user = gtk::Label::new(Some(&proc.user));
    user.add_css_class("metis-dash-muted");
    user.set_halign(gtk::Align::Start);
    user.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let kind = gtk::Label::new(Some(&class_label(proc.class)));
    kind.add_css_class("metis-dash-muted");
    kind.set_halign(gtk::Align::Start);

    let cpu = gtk::Label::new(Some(&format!("{:.1}%", proc.cpu_percent)));
    cpu.add_css_class("metis-dash-muted");
    cpu.set_halign(gtk::Align::End);
    cpu.set_xalign(1.0);

    let mem = gtk::Label::new(Some(&format_bytes(proc.memory_bytes)));
    mem.add_css_class("metis-dash-muted");
    mem.set_halign(gtk::Align::End);
    mem.set_xalign(1.0);

    pid.set_width_request(64);
    user.set_width_request(88);
    kind.set_width_request(64);
    cpu.set_width_request(64);
    mem.set_width_request(80);
    grid.attach(&name_box, 0, 0, 1, 1);
    grid.attach(&pid, 1, 0, 1, 1);
    grid.attach(&user, 2, 0, 1, 1);
    grid.attach(&kind, 3, 0, 1, 1);
    grid.attach(&cpu, 4, 0, 1, 1);
    grid.attach(&mem, 5, 0, 1, 1);

    let kill = gtk::Button::from_icon_name("process-stop-symbolic");
    kill.add_css_class("flat");
    kill.set_sensitive(proc.killable);
    let kill_tooltip = if proc.killable {
        tr("End task")
    } else {
        tr("Cannot end processes owned by another user")
    };
    kill.set_tooltip_text(Some(&kill_tooltip));
    let pid_val = proc.pid;
    kill.connect_clicked(move |btn| {
        let cfg = load_dashboard_config();
        if cfg.confirm_before_kill {
            confirm_kill_process(btn, pid_val, false, false);
        } else if let Err(err) = kill_process(pid_val, false) {
            tracing::warn!(%err, pid = pid_val, "end task failed");
        }
    });

    grid.attach(&kill, 6, 0, 1, 1);

    if proc.killable {
        attach_process_context_menu(&row, proc.pid, &proc.name, entry.child_count > 0);
    }

    row.set_child(Some(&grid));
    row
}

fn process_context_menu_open() -> bool {
    PROCESS_CONTEXT_MENU.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|p| p.is_visible() || p.parent().is_some())
    })
}

fn dismiss_process_context_menu() {
    // Take the popover out before popdown — the closed handler also touches
    // PROCESS_CONTEXT_MENU, so holding RefMut across popdown aborts the shell.
    let popover = PROCESS_CONTEXT_MENU.with(|slot| slot.borrow_mut().take());
    if let Some(popover) = popover
        && popover.parent().is_some()
    {
        popover.popdown();
        if popover.parent().is_some() {
            popover.unparent();
        }
    }
}

fn attach_process_context_menu(row: &gtk::ListBoxRow, pid: u32, name: &str, has_children: bool) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let row_widget: gtk::Widget = row.clone().upcast();
    let name = name.to_string();
    gesture.connect_pressed(move |_, _n, x, y| {
        show_process_context_menu(&row_widget, pid, &name, has_children, x, y);
    });
    row.add_controller(gesture);
}

fn show_process_context_menu(
    anchor: &gtk::Widget,
    pid: u32,
    name: &str,
    has_children: bool,
    x: f64,
    y: f64,
) {
    dismiss_process_context_menu();
    crate::ui::bar::close_bar_popovers();

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
    panel.add_css_class("metis-dash-context-menu");
    panel.set_margin_top(6);
    panel.set_margin_bottom(6);
    panel.set_margin_start(4);
    panel.set_margin_end(4);
    panel.set_width_request(220);

    let header = gtk::Label::new(Some(name));
    header.set_xalign(0.0);
    header.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.add_css_class("metis-dash-popover-title");
    header.set_margin_start(8);
    header.set_margin_end(8);
    header.set_margin_top(4);
    panel.append(&header);

    append_process_menu_item(&panel, &tr("End task"), {
        let anchor = anchor.clone();
        move || {
            dismiss_process_context_menu();
            confirm_kill_process(&anchor, pid, false, false);
        }
    });
    append_process_menu_item(&panel, &tr("Force quit"), {
        let anchor = anchor.clone();
        move || {
            dismiss_process_context_menu();
            confirm_kill_process(&anchor, pid, true, false);
        }
    });
    if has_children {
        append_process_menu_item(&panel, &tr("End process tree"), {
            let anchor = anchor.clone();
            move || {
                dismiss_process_context_menu();
                confirm_kill_process(&anchor, pid, false, true);
            }
        });
        append_process_menu_item(&panel, &tr("Force quit tree"), {
            let anchor = anchor.clone();
            move || {
                dismiss_process_context_menu();
                confirm_kill_process(&anchor, pid, true, true);
            }
        });
    }
    append_process_menu_item(&panel, &format!("{} ({pid})", tr("Copy PID")), move || {
        copy_text_to_clipboard(&pid.to_string());
        dismiss_process_context_menu();
    });

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-dash-popover");
    popover.set_parent(anchor);
    let rect = gdk::Rectangle::new(x.round() as i32, y.round() as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));

    PROCESS_CONTEXT_MENU.with(|slot| *slot.borrow_mut() = Some(popover.clone()));
    crate::ui::bar::register_bar_popover(&popover);

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        PROCESS_CONTEXT_MENU.with(|slot| {
            let clear = slot
                .borrow()
                .as_ref()
                .is_some_and(|p| p.downgrade() == weak);
            if clear {
                *slot.borrow_mut() = None;
            }
        });
        if let Some(p) = weak.upgrade()
            && p.parent().is_some()
        {
            p.unparent();
        }
    });

    let popover_show = popover.clone();
    glib::idle_add_local_once(move || {
        popover_show.popup();
    });
}

fn append_process_menu_item<F>(panel: &gtk::Box, label: &str, action: F)
where
    F: Fn() + 'static,
{
    let item = gtk::Button::builder()
        .label(label)
        .has_frame(false)
        .halign(gtk::Align::Fill)
        .build();
    item.add_css_class("metis-dash-menu-item");
    if let Some(child) = item.child()
        && let Ok(lbl) = child.downcast::<gtk::Label>()
    {
        lbl.set_halign(gtk::Align::Start);
        lbl.set_xalign(0.0);
    }
    item.connect_clicked(move |_| action());
    panel.append(&item);
}

fn confirm_kill_process(anchor: &impl IsA<gtk::Widget>, pid: u32, force: bool, tree: bool) {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.set_margin_top(10);
    panel.set_margin_bottom(10);
    panel.set_margin_start(12);
    panel.set_margin_end(12);
    panel.set_width_request(260);

    let title = match (force, tree) {
        (true, true) => tr("Force quit process tree?"),
        (true, false) => tr("Force quit?"),
        (false, true) => tr("End process tree?"),
        (false, false) => tr("End task?"),
    };
    let body = match (force, tree) {
        (true, true) => format!(
            "{} {pid} {}? {}",
            tr("Send SIGKILL to process"),
            tr("and its child processes"),
            tr("This cannot be undone.")
        ),
        (true, false) => format!(
            "{} {pid}? {}",
            tr("Send SIGKILL to process"),
            tr("This cannot be undone.")
        ),
        (false, true) => format!(
            "{} {pid} {}?",
            tr("Send SIGTERM to process"),
            tr("and its child processes")
        ),
        (false, false) => format!("{} {pid}?", tr("Send SIGTERM to process")),
    };

    let t = gtk::Label::new(Some(&title));
    t.set_xalign(0.0);
    t.add_css_class("metis-dash-popover-title");
    panel.append(&t);
    let b = gtk::Label::new(Some(&body));
    b.set_xalign(0.0);
    b.set_wrap(true);
    b.add_css_class("metis-dash-muted");
    panel.append(&b);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let ok_label = match (force, tree) {
        (true, true) => tr("Force quit tree"),
        (true, false) => tr("Force quit"),
        (false, true) => tr("End tree"),
        (false, false) => tr("End task"),
    };
    let ok = gtk::Button::with_label(&ok_label);
    if force {
        ok.add_css_class("destructive-action");
    }
    actions.append(&cancel);
    actions.append(&ok);
    panel.append(&actions);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-dash-popover");
    popover.set_parent(anchor);
    crate::ui::bar::register_bar_popover(&popover);

    let pop_cancel = popover.clone();
    cancel.connect_clicked(move |_| pop_cancel.popdown());
    let pop_ok = popover.clone();
    ok.connect_clicked(move |_| {
        let result = if tree {
            DASHBOARD.with(|d| {
                let holder = d.borrow();
                let procs = holder
                    .as_ref()
                    .map(|dash| dash.snapshot.borrow().processes.clone())
                    .unwrap_or_default();
                kill_process_tree(pid, &procs, force)
            })
        } else {
            kill_process(pid, force)
        };
        if let Err(err) = result {
            tracing::warn!(%err, pid, force, tree, "kill failed");
        }
        pop_ok.popdown();
    });
    let popover_show = popover.clone();
    glib::idle_add_local_once(move || {
        popover_show.popup();
    });
}

fn copy_text_to_clipboard(text: &str) {
    let display = gdk::Display::default();
    let Some(display) = display else {
        return;
    };
    let clipboard = display.clipboard();
    clipboard.set_text(text);
}

fn class_label(class: ProcessClass) -> String {
    match class {
        ProcessClass::Metis => tr("Metis"),
        ProcessClass::UserApp => tr("App"),
        ProcessClass::System => tr("System"),
    }
}

fn launch_process_monitor() {
    let dash = metis_config::load_dashboard_config();
    let candidates: Vec<String> = match dash
        .process_monitor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(chosen) => vec![chosen.to_string()],
        None => metis_config::KNOWN_PROCESS_MONITORS
            .iter()
            .map(|(bin, _)| (*bin).to_string())
            .collect(),
    };
    for bin in &candidates {
        if !metis_config::binary_in_path(bin) {
            continue;
        }
        let argv = if metis_config::process_monitor_needs_terminal(bin) {
            let Some(term) = metis_config::resolve_terminal() else {
                continue;
            };
            metis_config::argv_in_terminal(&term, bin)
        } else {
            vec![bin.clone()]
        };
        if let Err(err) = crate::compositor::launch_argv(argv) {
            tracing::warn!(%err, bin, "failed to launch process monitor");
            continue;
        }
        return;
    }
    let _ = std::process::Command::new("notify-send")
        .args([
            "-a",
            "Metis",
            "No process monitor found",
            "Install btop, htop, or a system monitor — or set one in Settings → Control Center.",
        ])
        .spawn();
}

fn set_temp_label(label: &gtk::Label, temp: Option<f32>) {
    match temp {
        Some(c) => label.set_text(&format!("{c:.0}°C")),
        None => label.set_text(&tr("N/A")),
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

fn compute_max_extent(position: BarPosition, monitor: Option<&gtk::gdk::Monitor>) -> i32 {
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

fn monitor_size(monitor: Option<&gtk::gdk::Monitor>) -> (i32, i32) {
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

mod dropdown {
    pub fn request_close_all() {
        crate::ui::bar::close_popovers();
    }
}
