mod dropdown;
pub(crate) use dropdown::{
    close_all as close_bar_popovers, is_open as dropdown_is_open, register as register_bar_popover,
};
pub(crate) mod widgets;

mod autohide;
mod geometry;
mod lifecycle;

// Re-export public / crate surfaces so callers keep `crate::ui::bar::…` paths.
#[allow(unused_imports)]
pub use autohide::{notify_bar_interaction, reveal_auto_hidden_bars};
#[allow(unused_imports)]
pub(crate) use geometry::popover_position;
#[allow(unused_imports)] // re-exports for crate-wide `ui::bar::…` callers
pub use geometry::{
    bar_position, dashboard_layer_inset, refresh_workspaces, set_edge_bar_visible,
    sync_control_center_button,
};
#[allow(unused_imports)]
pub use lifecycle::{
    apply_bar_config_now, broadcast_audio, close_popovers, emit_updates_notification,
    notify_display_hotplug, rebuild_from_config,
};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

use crate::config::{BarConfig, BarPosition, load_bar_config, save_default_bar_config};
use crate::services::{
    BarSnapshot, spawn_bar_pollers, spawn_notification_service, spawn_weather_service,
    workspace_snapshot,
};

thread_local! {
    // One bar per output (monitor); see `BarDisplays`. A single-monitor session
    // holds exactly one handle.
    static BARS: RefCell<Vec<BarHandle>> = const { RefCell::new(Vec::new()) };
    // Mirror of the active bar position, kept outside the `BARS` RefCell so
    // `popover_position()` can be read while `BARS` is mutably borrowed (e.g. from
    // within `rebuild_bars`, which builds widgets that query the popover side).
    static BAR_POSITION: Cell<BarPosition> = const { Cell::new(BarPosition::Top) };
    /// Screen-edge inset (px) where the control center attaches below the bar pill.
    static DASH_ATTACH_INSET: Cell<i32> = const { Cell::new(0) };
    // Set while a coalesced rebuild is queued, so a burst of config/monitor change
    // triggers collapses into a single rebuild pass.
    static REBUILD_SCHEDULED: Cell<bool> = const { Cell::new(false) };
    /// Last menu layout applied to bar widgets. `reload-bar` only diffs `bar.json`,
    /// so Settings → Metis Menu style/feature toggles (in `menu.json`) need this
    /// to force a widget remount when the launcher layout changes.
    static LAST_MENU_LAYOUT: Cell<Option<MenuLayoutSnap>> =
        const { Cell::new(None) };
}

/// Layout-relevant slice of `menu.json` (not launch counts / pins).
#[derive(Clone, Copy, PartialEq, Eq)]
struct MenuLayoutSnap {
    style: metis_config::MenuStyle,
    show_user_header: bool,
    show_rail: bool,
    show_pinned: bool,
}

fn menu_layout_snap() -> MenuLayoutSnap {
    let cfg = metis_config::load_menu_config();
    MenuLayoutSnap {
        style: cfg.style,
        show_user_header: cfg.show_user_header,
        show_rail: cfg.show_rail,
        show_pinned: cfg.show_pinned,
    }
}

fn remember_menu_layout() {
    LAST_MENU_LAYOUT.with(|cell| cell.set(Some(menu_layout_snap())));
}

/// True when `menu.json` layout fields changed since the last bar widget build.
fn take_menu_layout_changed() -> bool {
    let snap = menu_layout_snap();
    LAST_MENU_LAYOUT.with(|cell| {
        let prev = cell.get();
        cell.set(Some(snap));
        match prev {
            Some(old) => old != snap,
            // Unseeded (should be rare after init) — remount so a Settings
            // layout click is never a silent no-op.
            None => true,
        }
    })
}

/// GTK widgets the control center embeds into (same layer surface as the bar).
#[derive(Clone)]
pub struct BarShell {
    pub window: gtk::Window,
    pub outer: gtk::Box,
    pub column: gtk::Box,
    pub host: gtk::Box,
}

struct BarHandle {
    window: gtk::Window,
    outer: gtk::Box,
    column: gtk::Box,
    pill: gtk::Box,
    dash_host: gtk::Box,
    config: Rc<RefCell<BarConfig>>,
    widget_refs: widgets::WidgetRefs,
    /// Compositor output name this bar is bound to (e.g. `metis-0`), used so its
    /// workspace widget switches/reads that output's own workspaces.
    output: Option<String>,
    /// True while a client on this output is in true fullscreen (edge bar hidden).
    chrome_suppressed: Cell<bool>,
    /// Auto-hide: bar is slid off-edge (peek only).
    auto_hide_hidden: Cell<bool>,
    /// Pointer is over this bar's GTK layer surface (EventControllerMotion).
    pointer_over: Cell<bool>,
    /// Compositor reports the pointer is in the screen-edge strip (includes the
    /// margin gap outside the GTK surface). Cleared via `bar-edge-leave`.
    edge_strip_hover: Cell<bool>,
    /// Pending idle hide timeout.
    hide_timeout: RefCell<Option<glib::SourceId>>,
    /// Pointer left while a popover/NC/dashboard was open — hide once UI closes.
    hide_when_ui_closes: Cell<bool>,
    /// Per-bar CSS provider for pixel-accurate auto-hide transforms (GTK CSS
    /// does not reliably parse `calc()` inside `transform`).
    autohide_css: RefCell<Option<gtk::CssProvider>>,
}

pub fn init_and_show() {
    if let Err(err) = save_default_bar_config() {
        tracing::warn!(%err, "failed to write default bar.json");
    }
    if let Err(err) = crate::config::save_default_decorations_config() {
        tracing::warn!(%err, "failed to write default decorations.json");
    }

    let tray = crate::services::spawn_tray_service();
    crate::services::set_command_sender(tray.commands);
    lifecycle::attach_tray_channel(tray.events);

    let config = Rc::new(RefCell::new(load_bar_config()));
    let cfg = config.borrow().clone();

    // One bar per target output. `target_monitors` returns at least one entry
    // (`None` = let the compositor pick the output) so the single-monitor path is
    // unchanged.
    let monitors = geometry::target_monitors(&cfg);
    let handles: Vec<BarHandle> = monitors
        .iter()
        .map(|m| build_bar(config.clone(), m.as_ref()))
        .collect();
    let count = handles.len();
    BARS.with(|bars| *bars.borrow_mut() = handles);
    remember_menu_layout();

    // Defer pollers so GTK can finish the first layer-shell commit before subprocess I/O.
    glib::timeout_add_seconds_local(2, move || {
        lifecycle::attach_poll_channel(spawn_bar_pollers());
        lifecycle::attach_weather_channel(spawn_weather_service());
        lifecycle::attach_notification_channel(spawn_notification_service());
        crate::services::spawn_updates_service();
        crate::ui::dashboard::init();
        crate::ui::screenshot::init();
        crate::ui::notification_center::init();
        // Desktop widgets run in a separate `metis-shell --desktop-widgets`
        // process so a widget hang cannot freeze the edge bar.
        lifecycle::watch_bar_config();
        lifecycle::watch_dashboard_config();
        lifecycle::spawn_gaming_daemon();
        lifecycle::watch_theme_files();
        lifecycle::watch_compositor_dismiss();
        lifecycle::watch_monitors();
        crate::services::watch_app_index();
        glib::ControlFlow::Break
    });

    tracing::info!(bars = count, position = ?cfg.position, "Metis edge bar initialized");
}

/// Build a single bar window, optionally bound to `monitor` (None lets the
/// compositor choose the output). Returns the handle without registering it.
fn build_bar(config: Rc<RefCell<BarConfig>>, monitor: Option<&gtk::gdk::Monitor>) -> BarHandle {
    let cfg = config.borrow().clone();
    let (win_w, win_h) = geometry::layer_window_size(&cfg);

    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Metis Bar"))
        .default_width(win_w)
        .default_height(win_h)
        .build();

    // Establish the layer-shell role, anchors, exclusive zone, and output binding
    // *before* the window is realized or any child widgets are built. At startup
    // GTK defers realization so ordering is forgiving, but building and presenting
    // a fresh layer window at runtime (a displays-toggle / hotplug rebuild)
    // realizes immediately — if the role/output aren't set first, gtk4-layer-shell
    // can commit an invalid surface and the compositor drops the connection.
    geometry::apply_layer_geometry(&window, &cfg);
    // Bind to a specific output (multi-monitor); must be set before the surface is
    // mapped. Omitted (None) lets the compositor place it on the primary output.
    if let Some(monitor) = monitor {
        window.set_monitor(Some(monitor));
    }
    // The compositor output name this bar lives on (its workspace widget uses it to
    // drive that output's own workspaces).
    let output = monitor.and_then(geometry::monitor_output_name);

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outer.add_css_class("metis-bar-outer");

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("metis-bar-column");

    let pill = gtk::Box::new(geometry::orientation_for(&cfg), 4);
    pill.add_css_class("metis-bar-pill");

    geometry::configure_surface(&outer, &column, &pill, &cfg);

    let dash_host = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .overflow(gtk::Overflow::Hidden)
        .build();
    dash_host.add_css_class("metis-dashboard-host");
    dash_host.set_visible(false);
    mount_dash_host(&cfg, &outer, &column, &pill, &dash_host);

    let shell = BarShell {
        window: window.clone(),
        outer: outer.clone(),
        column: column.clone(),
        host: dash_host.clone(),
    };
    crate::ui::dashboard::wire_bar_pull(&pill, &shell);

    // Auto-hide: track pointer over the bar surface.
    {
        let motion = gtk::EventControllerMotion::new();
        let window_for_enter = window.clone();
        motion.connect_enter(move |_, _, _| {
            autohide::on_bar_pointer_enter(&window_for_enter);
        });
        let window_for_leave = window.clone();
        motion.connect_leave(move |_| {
            autohide::on_bar_pointer_leave(&window_for_leave);
        });
        window.add_controller(motion);
    }

    // Click on empty bar space
    // open popover. Bubble phase means child buttons that claim the press are
    // skipped, so this never fires when toggling/opening an icon.
    let dismiss = gtk::GestureClick::builder()
        .button(0)
        .propagation_phase(gtk::PropagationPhase::Bubble)
        .build();
    let pill_for_dismiss = pill.clone();
    dismiss.connect_pressed(move |_, _, x, y| {
        // Popover presses bubble up here because the popover is a widget-tree
        // child of its icon button. Ignore anything outside the pill's own strip
        // so interacting with the popover doesn't dismiss it.
        let w = pill_for_dismiss.width() as f64;
        let h = pill_for_dismiss.height() as f64;
        if x < 0.0 || y < 0.0 || x > w || y > h {
            return;
        }
        // If the press landed on (or inside) one of the bar's own icon buttons,
        // let that button's own click handler toggle its popover. Dismissing here
        // would race the toggle and re-open the popover on the second click.
        if let Some(target) = pill_for_dismiss.pick(x, y, gtk::PickFlags::DEFAULT) {
            let mut node = Some(target);
            while let Some(w) = node {
                if w.has_css_class("metis-bar-widget") {
                    return;
                }
                node = w.parent();
            }
        }
        dropdown::request_close_all();
        crate::ui::notification_center::dismiss();
    });
    pill.add_controller(dismiss);

    if matches!(cfg.position, BarPosition::Left | BarPosition::Right) {
        if matches!(cfg.position, BarPosition::Left) {
            outer.append(&column);
            outer.append(&dash_host);
        } else {
            outer.append(&dash_host);
            outer.append(&column);
        }
    } else {
        outer.append(&column);
    }
    window.set_child(Some(&outer));

    let widget_refs = widgets::build(&pill, config.clone(), output.clone(), shell.clone());
    widget_refs.apply_snapshot(&BarSnapshot {
        workspaces: workspace_snapshot(),
        ..Default::default()
    });
    lifecycle::rehydrate_widget_state(&widget_refs);

    // Defer map until layer-shell anchors/size are applied (avoids 0-height first commit).
    let show_window = window.clone();
    glib::idle_add_local_once(move || {
        show_window.set_visible(true);
        show_window.present();
    });

    BarHandle {
        window,
        outer,
        column,
        pill,
        dash_host,
        config,
        widget_refs,
        output,
        chrome_suppressed: Cell::new(false),
        auto_hide_hidden: Cell::new(false),
        pointer_over: Cell::new(false),
        edge_strip_hover: Cell::new(false),
        hide_timeout: RefCell::new(None),
        hide_when_ui_closes: Cell::new(false),
        autohide_css: RefCell::new(None),
    }
}

/// Place the pill and dashboard host in the bar tree for the current edge.
fn mount_dash_host(
    config: &BarConfig,
    _outer: &gtk::Box,
    column: &gtk::Box,
    pill: &gtk::Box,
    dash_host: &gtk::Box,
) {
    match config.position {
        BarPosition::Top => {
            column.append(pill);
            column.append(dash_host);
        }
        BarPosition::Bottom => {
            column.append(dash_host);
            column.append(pill);
        }
        BarPosition::Left | BarPosition::Right => {
            column.append(pill);
        }
    }
}

/// Re-parent pill + dashboard host after an edge/position change so the control
/// center always opens toward the desktop and the pill stays on the anchored edge.
fn remount_bar_chrome(handle: &BarHandle, cfg: &BarConfig) {
    if let Some(parent) = handle.pill.parent()
        && let Ok(box_) = parent.downcast::<gtk::Box>()
    {
        box_.remove(&handle.pill);
    }
    if let Some(parent) = handle.dash_host.parent()
        && let Ok(box_) = parent.downcast::<gtk::Box>()
    {
        box_.remove(&handle.dash_host);
    }
    while let Some(child) = handle.column.first_child() {
        handle.column.remove(&child);
    }
    while let Some(child) = handle.outer.first_child() {
        handle.outer.remove(&child);
    }

    // Collapse the CC host before remounting. Leftover size/expand from a prior
    // left/right layout (or an open Control Center) inside a fixed-height top/
    // bottom column steals strip pixels and visually squashes the pill.
    reset_dash_host_strip(&handle.dash_host);

    mount_dash_host(
        cfg,
        &handle.outer,
        &handle.column,
        &handle.pill,
        &handle.dash_host,
    );
    match cfg.position {
        BarPosition::Left => {
            handle.outer.append(&handle.column);
            handle.outer.append(&handle.dash_host);
        }
        BarPosition::Right => {
            handle.outer.append(&handle.dash_host);
            handle.outer.append(&handle.column);
        }
        BarPosition::Top | BarPosition::Bottom => {
            handle.outer.append(&handle.column);
        }
    }
}

/// Zero the in-bar Control Center host so it cannot compete with the pill for
/// the fixed strip allocation.
fn reset_dash_host_strip(host: &gtk::Box) {
    host.set_visible(false);
    host.set_hexpand(false);
    host.set_vexpand(false);
    host.set_halign(gtk::Align::Fill);
    host.set_valign(gtk::Align::Fill);
    host.set_size_request(0, 0);
}

/// Keep the edge-bar layer at its closed strip size. Control Center uses a
/// separate layer surface, so opening it must never grow/shrink this window.
pub(crate) fn ensure_bar_strip_geometry(shell: &BarShell) {
    let cfg = load_bar_config();
    let closed = geometry::bar_body_thickness(&cfg);
    let cross = geometry::bar_cross_thickness(&cfg);

    reset_dash_host_strip(&shell.host);

    match cfg.position {
        BarPosition::Top | BarPosition::Bottom => {
            shell.window.set_height_request(closed);
            shell.window.set_width_request(-1);
            shell.column.set_size_request(-1, closed);
            shell.outer.set_size_request(-1, closed);
            shell.host.set_size_request(-1, 0);
            let valign = geometry::edge_valign(cfg.position);
            shell.outer.set_valign(valign);
            shell.column.set_valign(valign);
            shell.outer.set_halign(gtk::Align::Fill);
            shell.column.set_halign(gtk::Align::Fill);
            shell.window.set_exclusive_zone(cross);
        }
        BarPosition::Left | BarPosition::Right => {
            shell.window.set_width_request(closed);
            shell.window.set_height_request(-1);
            shell.outer.set_size_request(closed, -1);
            shell.column.set_size_request(closed, -1);
            shell.host.set_size_request(0, -1);
            let halign = geometry::edge_halign(cfg.position);
            shell.outer.set_halign(halign);
            shell.column.set_halign(halign);
            shell.outer.set_valign(gtk::Align::Fill);
            shell.column.set_valign(gtk::Align::Fill);
            shell.window.set_exclusive_zone(0);
        }
    }
    shell.window.queue_resize();
}
