mod dropdown;
pub(crate) use dropdown::{close_all as close_bar_popovers, register as register_bar_popover};
pub(crate) mod widgets;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::config::{
    load_bar_config, save_default_bar_config, BarConfig, BarDisplays, BarPosition,
};
use crate::services::{
    apply_event, last_weather_snapshot, refresh_taskbars, spawn_bar_pollers,
    spawn_notification_service, spawn_weather_service, weather_refresh, workspace_snapshot,
    BarSnapshot, WeatherSnapshot,
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

    let tray = crate::services::spawn_tray_service();
    crate::services::set_command_sender(tray.commands);
    attach_tray_channel(tray.events);

    let config = Rc::new(RefCell::new(load_bar_config()));
    let cfg = config.borrow().clone();

    // One bar per target output. `target_monitors` returns at least one entry
    // (`None` = let the compositor pick the output) so the single-monitor path is
    // unchanged.
    let monitors = target_monitors(&cfg);
    let handles: Vec<BarHandle> = monitors
        .iter()
        .map(|m| build_bar(config.clone(), m.as_ref()))
        .collect();
    let count = handles.len();
    BARS.with(|bars| *bars.borrow_mut() = handles);
    remember_menu_layout();

    // Defer pollers so GTK can finish the first layer-shell commit before subprocess I/O.
    glib::timeout_add_seconds_local(2, move || {
        attach_poll_channel(spawn_bar_pollers());
        attach_weather_channel(spawn_weather_service());
        attach_notification_channel(spawn_notification_service());
        crate::ui::dashboard::init();
        crate::ui::screenshot::init();
        crate::ui::notification_center::init();
        // Desktop widgets run in a separate `metis-shell --desktop-widgets`
        // process so a widget hang cannot freeze the edge bar.
        watch_bar_config();
        watch_dashboard_config();
        spawn_gaming_daemon();
        watch_theme_files();
        watch_compositor_dismiss();
        watch_monitors();
        crate::services::watch_app_index();
        glib::ControlFlow::Break
    });

    tracing::info!(bars = count, position = ?cfg.position, "Metis edge bar initialized");
}

/// Build a single bar window, optionally bound to `monitor` (None lets the
/// compositor choose the output). Returns the handle without registering it.
fn build_bar(config: Rc<RefCell<BarConfig>>, monitor: Option<&gtk::gdk::Monitor>) -> BarHandle {
    let cfg = config.borrow().clone();
    let (win_w, win_h) = layer_window_size(&cfg);

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
    apply_layer_geometry(&window, &cfg);
    // Bind to a specific output (multi-monitor); must be set before the surface is
    // mapped. Omitted (None) lets the compositor place it on the primary output.
    if let Some(monitor) = monitor {
        window.set_monitor(monitor);
    }
    // The compositor output name this bar lives on (its workspace widget uses it to
    // drive that output's own workspaces).
    let output = monitor.and_then(monitor_output_name);

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outer.add_css_class("metis-bar-outer");

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("metis-bar-column");

    let pill = gtk::Box::new(orientation_for(&cfg), 4);
    pill.add_css_class("metis-bar-pill");

    configure_surface(&outer, &column, &pill, &cfg);

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
            on_bar_pointer_enter(&window_for_enter);
        });
        let window_for_leave = window.clone();
        motion.connect_leave(move |_| {
            on_bar_pointer_leave(&window_for_leave);
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
    rehydrate_widget_state(&widget_refs);

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
    if let Some(parent) = handle.pill.parent() {
        if let Ok(box_) = parent.downcast::<gtk::Box>() {
            box_.remove(&handle.pill);
        }
    }
    if let Some(parent) = handle.dash_host.parent() {
        if let Ok(box_) = parent.downcast::<gtk::Box>() {
            box_.remove(&handle.dash_host);
        }
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
    let closed = bar_body_thickness(&cfg);
    let cross = bar_cross_thickness(&cfg);

    reset_dash_host_strip(&shell.host);

    match cfg.position {
        BarPosition::Top | BarPosition::Bottom => {
            shell.window.set_height_request(closed);
            shell.window.set_width_request(-1);
            shell.column.set_size_request(-1, closed);
            shell.outer.set_size_request(-1, closed);
            shell.host.set_size_request(-1, 0);
            let valign = edge_valign(cfg.position);
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
            let halign = edge_halign(cfg.position);
            shell.outer.set_halign(halign);
            shell.column.set_halign(halign);
            shell.outer.set_valign(gtk::Align::Fill);
            shell.column.set_valign(gtk::Align::Fill);
            shell.window.set_exclusive_zone(0);
        }
    }
    shell.window.queue_resize();
}

/// Hide or restore the edge bar on the output that has a true-fullscreen client.
pub fn set_edge_bar_visible(output: &str, visible: bool) {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if !bar_matches_output(handle.output.as_deref(), output) {
                continue;
            }
            handle.chrome_suppressed.set(!visible);
            if visible {
                handle.window.set_visible(true);
                apply_layer_geometry(&handle.window, &handle.config.borrow());
            } else {
                dropdown::close_all();
                handle.window.set_exclusive_zone(0);
                handle.window.set_visible(false);
            }
        }
    });
}

fn bar_matches_output(bar_output: Option<&str>, event_output: &str) -> bool {
    match bar_output {
        Some(name) => name == event_output,
        None => true,
    }
}

/// The compositor output name (e.g. `metis-0`) backing a GDK monitor. Under the
/// nested Metis session GDK exposes the compositor's `wl_output` name via the
/// monitor connector.
fn monitor_output_name(monitor: &gtk::gdk::Monitor) -> Option<String> {
    monitor
        .connector()
        .map(|c| c.to_string())
        .filter(|c| !c.is_empty())
}

/// Repaint every bar's workspace dots from the current per-output active
/// workspace (called after an optimistic switch or a `WorkspaceChanged` event).
pub fn refresh_workspaces() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            handle.widget_refs.refresh_workspaces();
        }
    });
}

/// Show or hide the Control Center grid button on every bar (live reload from
/// `dashboard.json`).
pub fn sync_control_center_button() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            handle.widget_refs.sync_control_center_button();
        }
    });
}

/// The outputs the bar should appear on, as GDK monitors. Returns at least one
/// entry; `None` means "no specific monitor" (compositor picks the primary).
/// `BarDisplays::Primary` yields a single bar on the first monitor.
fn target_monitors(cfg: &BarConfig) -> Vec<Option<gtk::gdk::Monitor>> {
    let monitors = connected_monitors();
    match cfg.displays {
        BarDisplays::Primary => vec![monitors.into_iter().next()],
        BarDisplays::All => {
            if monitors.is_empty() {
                vec![None]
            } else {
                monitors.into_iter().map(Some).collect()
            }
        }
    }
}

/// Snapshot of the currently connected GDK monitors (first entry is treated as
/// the primary output).
fn connected_monitors() -> Vec<gtk::gdk::Monitor> {
    use gtk::gio::prelude::ListModelExt;
    let Some(display) = gtk::gdk::Display::default() else {
        return Vec::new();
    };
    let list = display.monitors();
    let mut out = Vec::new();
    for i in 0..list.n_items() {
        if let Some(monitor) = list
            .item(i)
            .and_then(|o| o.downcast::<gtk::gdk::Monitor>().ok())
        {
            out.push(monitor);
        }
    }
    out
}

/// Rebuild every bar when monitors are added/removed so the per-output bars track
/// the current display layout.
fn watch_monitors() {
    use gtk::gio::prelude::ListModelExt;
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    display
        .monitors()
        .connect_items_changed(move |_, _, _, _| rebuild_from_config());
}

fn layer_window_size(config: &BarConfig) -> (i32, i32) {
    let thickness = bar_body_thickness(config);
    match config.position {
        BarPosition::Top | BarPosition::Bottom => (-1, thickness),
        BarPosition::Left | BarPosition::Right => (thickness, -1),
    }
}

/// Empty padding kept inside the layer surface around the visible pill so the
/// pill's drop shadow renders fully (and follows its rounded corners) instead of
/// being clipped square at the surface's rectangular edge. Shared with the
/// compositor (via `metis-config`) so backdrop blur can exclude this margin.
/// At distance 0 this is 0 so the surface height equals the pill (true flush).
fn bar_body_thickness(config: &BarConfig) -> i32 {
    config.height as i32 + metis_config::bar::bar_layer_shadow_pad(config)
}

/// Visible cross-axis size of the bar pill (height when horizontal, width when
/// vertical). Kept equal so left/right bars match the top/bottom strip thickness.
fn bar_cross_thickness(config: &BarConfig) -> i32 {
    config.height as i32
}

/// The side bar popovers/menus should open toward, derived from the bar's anchored
/// edge: a top bar opens downward, a bottom bar upward, a left bar to the right,
/// a right bar to the left. Falls back to `Bottom` before the bar is initialized.
pub(crate) fn popover_position() -> gtk::PositionType {
    match BAR_POSITION.with(Cell::get) {
        BarPosition::Bottom => gtk::PositionType::Top,
        BarPosition::Left => gtk::PositionType::Right,
        BarPosition::Right => gtk::PositionType::Left,
        BarPosition::Top => gtk::PositionType::Bottom,
    }
}

/// Layer-shell surfaces do not receive outside-click events; the compositor
/// tells us to pop down when the pointer hits bare desktop.
fn watch_compositor_dismiss() {
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let path = metis_protocol::runtime_command_path();
        if let Ok(cmd) = std::fs::read_to_string(&path) {
            let parsed = match metis_protocol::parse_runtime_command(
                cmd.trim(),
                metis_protocol::BAR_RUNTIME_VERBS,
            ) {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!(%err, "rejected bar runtime command");
                    let _ = std::fs::remove_file(&path);
                    return glib::ControlFlow::Continue;
                }
            };
            if !metis_protocol::try_admit_runtime_command_dispatch() {
                tracing::warn!("bar runtime command dispatch rate limited");
                let _ = std::fs::remove_file(&path);
                return glib::ControlFlow::Continue;
            }
            let verb = parsed.verb;
            let arg = parsed.arg;
            match verb {
                "close-popovers" => {
                    // Sync close so auto-hide can re-arm on the same turn (idle
                    // popdown left `dropdown::is_open()` true and skipped hide).
                    // Task View stays open: it is sticky until Esc / activate /
                    // desktop click / backdrop click (not outside-click IPC).
                    dropdown::close_all();
                    crate::ui::dashboard::request_close();
                    crate::ui::notification_center::dismiss();
                    rearm_auto_hide_after_ui_dismiss();
                }
                "toggle-menu" => widgets::toggle_menu(),
                // Multimedia / hardware keys forwarded by the compositor
                // (`hw <action>`): volume, brightness, media transport, and the
                // display mirror/extend confirmation overlay.
                "hw" => {
                    let arg = arg.trim();
                    if let Some(mode) = arg.strip_prefix("osd-display") {
                        let mode = mode.trim();
                        let label = if mode == "mirror" {
                            "Mirror displays"
                        } else {
                            "Extend displays"
                        };
                        crate::ui::osd::show("video-display-symbolic", label, None, false);
                    } else if !crate::services::hardware::dispatch(arg) {
                        tracing::debug!(%arg, "unknown hardware key action");
                    }
                }
                "dismiss-screenshot" => crate::ui::screenshot::dismiss(),
                "window-switcher-next" | "task-view-next" => crate::ui::task_view::cycle_next(),
                "window-switcher-prev" | "task-view-prev" => crate::ui::task_view::cycle_prev(),
                "window-switcher-activate" | "task-view-activate" => {
                    crate::ui::task_view::activate_selected()
                }
                "dismiss-window-switcher" | "dismiss-task-view" | "dismiss-workspace-overview" => {
                    crate::ui::task_view::dismiss()
                }
                // Legacy verb: same as Super+Tab → open or cycle next.
                "workspace-overview" => crate::ui::task_view::cycle_next(),
                "reload-bar" => rebuild_from_config(),
                "reveal-edge-bar" | "bar-edge-hover" => on_bar_edge_hover(),
                "bar-edge-leave" => on_bar_edge_leave(),
                "reload-dashboard" => crate::ui::dashboard::on_dashboard_config_changed(),
                // Desktop widgets are isolated; Settings writes to command-widgets.
                "reload-desktop-widgets" => {
                    tracing::debug!("reload-desktop-widgets ignored in bar process");
                }
                "screenshot" => {
                    // Forms: `screenshot [CONNECTOR]`,
                    // `screenshot instant-full|window|record [CONNECTOR]`.
                    let mut parts = arg.split_whitespace();
                    let first = parts.next().unwrap_or("");
                    let (mode, connector) = match first {
                        "" => (crate::ui::screenshot::LaunchMode::Interactive, None),
                        "instant-full" => (
                            crate::ui::screenshot::LaunchMode::InstantFull,
                            parts.next().map(str::to_string),
                        ),
                        "window" => (
                            crate::ui::screenshot::LaunchMode::Window,
                            parts.next().map(str::to_string),
                        ),
                        "record" => (
                            crate::ui::screenshot::LaunchMode::Record,
                            parts.next().map(str::to_string),
                        ),
                        connector => (
                            crate::ui::screenshot::LaunchMode::Interactive,
                            Some(connector.to_string()),
                        ),
                    };
                    crate::ui::screenshot::show(mode, connector);
                }
                "reload-theme" => {
                    let _ = crate::ui::theme::init_theme();
                }
                "reload-graphics-profile" => {
                    // Compositor re-reads AppConfig on client spawn / animation
                    // checks; acknowledge so Settings does not leave a stale file.
                    tracing::debug!("graphics profile reload acknowledged");
                }
                "reload-weather" => {
                    if !crate::ui::onboarding::is_active() {
                        crate::services::weather::weather_refresh();
                    }
                }
                "reload-calendars" => crate::services::reload_calendars(),
                "reload-gaming" => {
                    let _ = metis_config::load_gaming_config();
                    let _ = crate::compositor::reload_gaming_config();
                }
                "reload-locale" => {
                    // Must recreate widgets — gettext reload alone leaves construction-time
                    // labels/tooltips in the old language. Do not go through
                    // `rebuild_from_config()`: that short-circuits to live geometry when
                    // bar.json is unchanged.
                    metis_i18n::reload();
                    let dir = if metis_i18n::is_rtl() {
                        gtk::TextDirection::Rtl
                    } else {
                        gtk::TextDirection::Ltr
                    };
                    gtk::Widget::set_default_direction(dir);
                    rebuild_for_locale();
                }
                "optimize-gaming" => {
                    // Require explicit `yes` (Settings confirm / metis-cmd --yes).
                    if arg.trim() == "yes"
                        || std::env::var_os("METIS_GAMING_OPTIMIZE_YES").is_some()
                    {
                        std::thread::spawn(|| {
                            let _ = metis_gaming::optimize_flatpak_gaming();
                            let _ = metis_gaming::ensure_steam_launcher();
                        });
                    } else {
                        tracing::warn!(
                            "optimize-gaming ignored without confirmation — use Settings → Gaming \
                             or: metis-cmd optimize-gaming --yes"
                        );
                    }
                }
                "show-onboarding" => crate::ui::onboarding::show(),
                "settings" => {
                    let program = if arg.trim().is_empty() {
                        "metis-settings".to_string()
                    } else {
                        format!("metis-settings --page {}", arg.trim())
                    };
                    if let Err(err) = crate::compositor::launch_program(&program) {
                        tracing::warn!(%err, "failed to launch metis-settings");
                    }
                }
                _ => {}
            }
            let _ = std::fs::remove_file(&path);
        }
        glib::ControlFlow::Continue
    });
}

/// Configure the bar's surface widget tree (outer box, column, pill) for the
/// current position: orientation, expansion, alignment (so the pill sits flush
/// against the anchored edge), size requests, and the vertical-bar CSS classes.
/// Shared by the initial build and the live `rebuild_bar` path so switching
/// between horizontal and vertical layouts at runtime re-sizes correctly.
fn configure_surface(outer: &gtk::Box, column: &gtk::Box, pill: &gtk::Box, config: &BarConfig) {
    // Publish the position for `popover_position()` before any widgets (which read
    // it) are built; reads must not borrow `BAR`, which is held during rebuilds.
    BAR_POSITION.with(|p| p.set(config.position));
    let is_vertical = matches!(config.position, BarPosition::Left | BarPosition::Right);
    let thickness = bar_body_thickness(config);

    outer.set_orientation(gtk::Orientation::Horizontal);
    // Stretch along the bar's long axis only (width for top/bottom, height for
    // left/right). Expanding on both axes makes a horizontal pill collapse to its
    // natural content width.
    outer.set_hexpand(!is_vertical);
    outer.set_vexpand(is_vertical);
    outer.set_halign(edge_halign(config.position));
    outer.set_valign(edge_valign(config.position));
    outer.remove_css_class("metis-bar-outer-vertical");
    if is_vertical {
        outer.add_css_class("metis-bar-outer-vertical");
        outer.set_size_request(thickness, -1);
    } else {
        outer.set_size_request(-1, thickness);
    }

    column.set_orientation(gtk::Orientation::Vertical);
    column.set_hexpand(!is_vertical);
    column.set_vexpand(is_vertical);
    column.set_halign(edge_halign(config.position));
    column.set_valign(edge_valign(config.position));
    if is_vertical {
        column.set_size_request(thickness, -1);
    } else {
        column.set_size_request(-1, thickness);
    }

    pill.set_orientation(orientation_for(config));
    pill.remove_css_class("metis-bar-pill-vertical");
    pill.remove_css_class("metis-bar-pill-vertical-right");
    if is_vertical {
        pill.set_size_request(bar_cross_thickness(config), -1);
        pill.add_css_class("metis-bar-pill-vertical");
        if matches!(config.position, BarPosition::Right) {
            pill.add_css_class("metis-bar-pill-vertical-right");
        }
    } else {
        pill.set_size_request(-1, config.height as i32);
    }
    apply_pill_layout(pill, config);
}

/// Horizontal alignment of the bar strip within its layer surface (flush to the
/// anchored screen edge; shadow pad sits on the inner side).
fn edge_halign(position: BarPosition) -> gtk::Align {
    match position {
        BarPosition::Right => gtk::Align::End,
        BarPosition::Left => gtk::Align::Start,
        // Top/bottom bars fill the full monitor width.
        BarPosition::Top | BarPosition::Bottom => gtk::Align::Fill,
    }
}

/// Vertical alignment of the bar strip within its layer surface.
fn edge_valign(position: BarPosition) -> gtk::Align {
    match position {
        BarPosition::Bottom => gtk::Align::End,
        BarPosition::Top => gtk::Align::Start,
        // Left/right bars fill the full monitor height.
        BarPosition::Left | BarPosition::Right => gtk::Align::Fill,
    }
}

fn apply_pill_layout(pill: &gtk::Box, config: &BarConfig) {
    pill.remove_css_class("metis-bar-full");
    pill.remove_css_class("metis-bar-floating");
    pill.remove_css_class("metis-bar-edge-bottom");
    pill.remove_css_class("metis-bar-edge-top");
    pill.remove_css_class("metis-bar-edge-left");
    pill.remove_css_class("metis-bar-edge-right");
    // Legacy flush classes (squared ends) — clear if a live theme reload left them.
    pill.remove_css_class("metis-bar-flush-bottom");
    pill.remove_css_class("metis-bar-flush-top");
    pill.remove_css_class("metis-bar-flush-left");
    pill.remove_css_class("metis-bar-flush-right");

    let vertical = matches!(config.position, BarPosition::Left | BarPosition::Right);
    // Stadium ends stay rounded at every distance. Side inset keeps the drop
    // shadow from clipping; cross-axis shadow pad sits on the *inner* side so
    // layer-shell `margin_top` is the true edge distance (1 ≈ 1px, not ~4–16).
    let side_pad = metis_config::bar::bar_pill_side_inset(config);
    let inner_pad = metis_config::bar::bar_layer_shadow_pad(config);
    match config.position {
        BarPosition::Bottom => {
            pill.set_margin_start(side_pad);
            pill.set_margin_end(side_pad);
            pill.set_margin_top(inner_pad);
            pill.set_margin_bottom(0);
        }
        BarPosition::Top => {
            pill.set_margin_start(side_pad);
            pill.set_margin_end(side_pad);
            pill.set_margin_top(0);
            pill.set_margin_bottom(inner_pad);
        }
        BarPosition::Left => {
            pill.set_margin_top(side_pad);
            pill.set_margin_bottom(side_pad);
            pill.set_margin_start(0);
            pill.set_margin_end(inner_pad);
        }
        BarPosition::Right => {
            pill.set_margin_top(side_pad);
            pill.set_margin_bottom(side_pad);
            pill.set_margin_start(inner_pad);
            pill.set_margin_end(0);
        }
    }

    let edge_class = match config.position {
        BarPosition::Bottom => "metis-bar-edge-bottom",
        BarPosition::Top => "metis-bar-edge-top",
        BarPosition::Left => "metis-bar-edge-left",
        BarPosition::Right => "metis-bar-edge-right",
    };

    if uses_strip_layout(config) {
        pill.add_css_class("metis-bar-full");
        pill.add_css_class(edge_class);
        if vertical {
            // Keep the pill at `height` px wide; the layer surface is wider only
            // for the inner-edge shadow pad — do not stretch the pill into it.
            pill.set_hexpand(false);
            pill.set_vexpand(true);
            pill.set_halign(edge_halign(config.position));
            pill.set_valign(gtk::Align::Fill);
        } else {
            pill.set_hexpand(true);
            pill.set_vexpand(false);
            pill.set_halign(gtk::Align::Fill);
            // Pin to the anchored screen edge. Inner shadow pad is widget margin
            // on the opposite side, so Fill cannot recenter the pill into the gap.
            pill.set_valign(if matches!(config.position, BarPosition::Bottom) {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });
        }
    } else {
        pill.add_css_class("metis-bar-floating");
        pill.add_css_class(edge_class);
        pill.set_hexpand(false);
        pill.set_vexpand(false);
        pill.set_halign(if matches!(config.position, BarPosition::Right) {
            gtk::Align::End
        } else if matches!(config.position, BarPosition::Left) {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
        pill.set_valign(if matches!(config.position, BarPosition::Bottom) {
            gtk::Align::End
        } else if matches!(config.position, BarPosition::Top) {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
    }
}

/// Full-edge strip or centered shortened strip (not content-hug floating).
fn uses_strip_layout(config: &BarConfig) -> bool {
    config.length_percent < 100 || config.full_width
}

/// Live attach inset for the pull-down control center (from the anchored screen edge).
pub fn dashboard_layer_inset() -> i32 {
    DASH_ATTACH_INSET.with(Cell::get)
}

/// Live edge-bar position (updated whenever bar geometry is applied).
#[allow(dead_code)] // bar layout query helper
pub fn bar_position() -> BarPosition {
    BAR_POSITION.with(Cell::get)
}

fn orientation_for(config: &BarConfig) -> gtk::Orientation {
    match config.position {
        BarPosition::Top | BarPosition::Bottom => gtk::Orientation::Horizontal,
        BarPosition::Left | BarPosition::Right => gtk::Orientation::Vertical,
    }
}

fn apply_layer_geometry(window: &gtk::Window, config: &BarConfig) {
    if !window.is_layer_window() {
        window.init_layer_shell();
    }
    window.set_layer(Layer::Top);
    window.set_namespace("metis-bar");
    window.add_css_class("metis-bar-window");
    // OnDemand (not None) so popovers spawned from the bar can receive keyboard
    // focus via their xdg_popup grab (text entries in the clock/calendar popover).
    window.set_keyboard_mode(KeyboardMode::OnDemand);

    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, false);
        window.set_margin(edge, 0);
    }

    let thickness = bar_body_thickness(config);
    // Reserve only the *visible* bar (margin + body), not the extra shadow padding
    // baked into the surface thickness. This lets windows tuck right up under the
    // bar's bottom edge (the shadow pad region is transparent) instead of leaving a
    // chunk of dead space below the bar.
    let visible_thickness = bar_cross_thickness(config);
    DASH_ATTACH_INSET.set(config.margin_top as i32 + visible_thickness);
    // Exclusive zone is the *visible body only*. Layer-shell margins are added by
    // the compositor (amount + margin), so including margin here double-counted
    // and left maximize/NC a few pixels off the pill.
    //
    // Top/bottom always reserve. Side bars overlay (exclusive 0). Auto-hide is
    // visual-only and must not toggle exclusive — that reflow path crashed sessions.
    let exclusive = match config.position {
        BarPosition::Top | BarPosition::Bottom => visible_thickness,
        BarPosition::Left | BarPosition::Right => 0,
    };
    window.set_exclusive_zone(exclusive);

    // Full-width stadium: side gap comes from the pill's side inset (shadow room),
    // not a second layer-shell margin — stacking both made distance-0 bars look
    // like they had ~20px ears after rounding was restored.
    //
    // Shortened bars (`length_percent` < 100) use equal along-edge margins so the
    // strip stays centered on the edge.
    let along_edge = along_edge_margin(window, config);
    let from_edge = config.margin_top as i32;

    match config.position {
        BarPosition::Top => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, from_edge);
            window.set_margin(Edge::Left, along_edge);
            window.set_margin(Edge::Right, along_edge);
            window.set_height_request(thickness);
            window.set_width_request(-1);
        }
        BarPosition::Bottom => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Bottom, from_edge);
            window.set_margin(Edge::Left, along_edge);
            window.set_margin(Edge::Right, along_edge);
            window.set_height_request(thickness);
            window.set_width_request(-1);
        }
        BarPosition::Left => {
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Left, from_edge);
            window.set_margin(Edge::Top, along_edge);
            window.set_margin(Edge::Bottom, along_edge);
            window.set_width_request(thickness);
            window.set_height_request(-1);
        }
        BarPosition::Right => {
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Right, from_edge);
            window.set_margin(Edge::Top, along_edge);
            window.set_margin(Edge::Bottom, along_edge);
            window.set_width_request(thickness);
            window.set_height_request(-1);
        }
    }

    // The configurable opacity dims only the bar's background surface (applied via
    // a CSS provider in the theme loader), so icons/text stay fully opaque. The
    // window itself must remain at full opacity.
    window.set_opacity(1.0);
    crate::ui::theme::apply_bar_appearance(
        config.opacity,
        &config.bar_border,
        &config.bar_fill,
        config.position,
    );
    crate::ui::theme::apply_menu_opacity(config.menu_opacity);

    // Anchor/margin changes do not always trigger a GTK relayout on their own;
    // queue a resize so gtk-layer-shell commits the new surface dimensions.
    window.queue_resize();
}

/// Equal left/right (or top/bottom) margin that centers a shortened strip.
fn along_edge_margin(window: &gtk::Window, config: &BarConfig) -> i32 {
    let pct = config.length_percent.clamp(40, 100);
    if pct >= 100 {
        return if config.full_width {
            0
        } else {
            config.margin_h as i32
        };
    }
    let edge = monitor_edge_length(window, config.position).max(1);
    let unused = edge.saturating_mul(100 - pct) / 100;
    (unused / 2) as i32
}

fn monitor_edge_length(window: &gtk::Window, position: BarPosition) -> u32 {
    let display = gtk::prelude::WidgetExt::display(window);
    let geo = window
        .surface()
        .and_then(|surface| display.monitor_at_surface(&surface))
        .map(|m| m.geometry())
        .or_else(|| {
            display
                .monitors()
                .item(0)
                .and_downcast::<gtk::gdk::Monitor>()
                .map(|m| m.geometry())
        });
    let Some(geo) = geo else {
        return match position {
            BarPosition::Top | BarPosition::Bottom => 1920,
            BarPosition::Left | BarPosition::Right => 1080,
        };
    };
    match position {
        BarPosition::Top | BarPosition::Bottom => geo.width().max(0) as u32,
        BarPosition::Left | BarPosition::Right => geo.height().max(0) as u32,
    }
}

fn apply_bar_visibility(handle: &BarHandle) {
    if handle.chrome_suppressed.get() {
        cancel_hide_timeout(handle);
        handle.auto_hide_hidden.set(false);
        clear_autohide_transform(&handle.outer);
        let _ = metis_protocol::set_bar_auto_hidden_flag(false);
        handle.window.set_exclusive_zone(0);
        handle.window.set_visible(false);
        return;
    }
    handle.window.set_visible(true);
    let cfg = handle.config.borrow().clone();
    if !cfg.auto_hide {
        cancel_hide_timeout(handle);
        handle.auto_hide_hidden.set(false);
        clear_autohide_transform(&handle.outer);
        let _ = metis_protocol::set_bar_auto_hidden_flag(false);
        restore_exclusive_zone(&handle.window, &cfg);
        return;
    }
    apply_auto_hide_visual(handle, &cfg);
    if !handle.auto_hide_hidden.get() && !auto_hide_pointer_blocks(handle) {
        schedule_auto_hide(handle);
    }
}

fn auto_hide_pointer_blocks(handle: &BarHandle) -> bool {
    handle.pointer_over.get() || handle.edge_strip_hover.get()
}

/// Compositor: pointer is in the bar's screen-edge strip (including the
/// margin gap outside the GTK surface). Cancel pending hide and reveal if needed.
fn on_bar_edge_hover() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if !handle.config.borrow().auto_hide {
                continue;
            }
            cancel_hide_timeout(handle);
            handle.edge_strip_hover.set(true);
            if handle.auto_hide_hidden.get() {
                handle.auto_hide_hidden.set(false);
                let cfg = handle.config.borrow().clone();
                apply_auto_hide_visual(handle, &cfg);
            }
        }
    });
}

/// Compositor: pointer left the bar strip — allow auto-hide again.
fn on_bar_edge_leave() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if !handle.config.borrow().auto_hide {
                handle.edge_strip_hover.set(false);
                continue;
            }
            handle.edge_strip_hover.set(false);
            if !handle.pointer_over.get() {
                schedule_auto_hide(handle);
            }
        }
    });
}

/// Force every edge bar visible (popover / menu / Control Center / NC open).
pub fn notify_bar_interaction() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            cancel_hide_timeout(handle);
            if handle.auto_hide_hidden.get() {
                handle.auto_hide_hidden.set(false);
                let cfg = handle.config.borrow().clone();
                apply_auto_hide_visual(handle, &cfg);
            }
            if handle.config.borrow().auto_hide && !auto_hide_pointer_blocks(handle) {
                schedule_auto_hide(handle);
            }
        }
    });
}

/// Compositor hot-edge reveal (same as edge hover when already hidden).
#[allow(dead_code)]
pub fn reveal_auto_hidden_bars() {
    on_bar_edge_hover();
}

fn on_bar_pointer_enter(window: &gtk::Window) {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if handle.window != *window {
                continue;
            }
            handle.pointer_over.set(true);
            cancel_hide_timeout(handle);
            if handle.auto_hide_hidden.get() {
                handle.auto_hide_hidden.set(false);
                let cfg = handle.config.borrow().clone();
                apply_auto_hide_visual(handle, &cfg);
            }
            break;
        }
    });
}

fn on_bar_pointer_leave(window: &gtk::Window) {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if handle.window != *window {
                continue;
            }
            handle.pointer_over.set(false);
            // Stay visible while the compositor still sees the pointer in the
            // edge strip (margin gap). Hide only when both are clear.
            if handle.config.borrow().auto_hide && !handle.edge_strip_hover.get() {
                schedule_auto_hide(handle);
            }
            break;
        }
    });
}

fn schedule_auto_hide(handle: &BarHandle) {
    cancel_hide_timeout(handle);
    if !handle.config.borrow().auto_hide {
        handle.hide_when_ui_closes.set(false);
        return;
    }
    if auto_hide_pointer_blocks(handle) {
        handle.hide_when_ui_closes.set(false);
        return;
    }
    // Popover / NC / dashboard open: remember to hide when they dismiss.
    if bar_ui_blocks_hide() {
        handle.hide_when_ui_closes.set(true);
        return;
    }
    handle.hide_when_ui_closes.set(false);
    // Short grace so a leave into the margin gap can be cancelled by
    // `bar-edge-hover` before the slide starts.
    const HIDE_GRACE_MS: u64 = 120;
    let window = handle.window.clone();
    let id = glib::timeout_add_local(std::time::Duration::from_millis(HIDE_GRACE_MS), move || {
        BARS.with(|bars| {
            for handle in bars.borrow().iter() {
                if handle.window != window {
                    continue;
                }
                *handle.hide_timeout.borrow_mut() = None;
                if auto_hide_pointer_blocks(handle) || bar_ui_blocks_hide() {
                    if bar_ui_blocks_hide() && !auto_hide_pointer_blocks(handle) {
                        handle.hide_when_ui_closes.set(true);
                    }
                    break;
                }
                if !handle.config.borrow().auto_hide {
                    break;
                }
                handle.auto_hide_hidden.set(true);
                let cfg = handle.config.borrow().clone();
                apply_auto_hide_visual(handle, &cfg);
                break;
            }
        });
        glib::ControlFlow::Break
    });
    *handle.hide_timeout.borrow_mut() = Some(id);
}

/// After popovers / NC / dashboard close, slide the bar away if the pointer is
/// no longer over it (outside click while a panel was open).
fn rearm_auto_hide_after_ui_dismiss() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if !handle.config.borrow().auto_hide || handle.chrome_suppressed.get() {
                handle.hide_when_ui_closes.set(false);
                continue;
            }
            handle.hide_when_ui_closes.set(false);
            // Outside click left the bar UI; drop stale strip-hover. If the
            // pointer is still in the edge band, the next compositor pulse
            // restores it and cancels hide within the grace window.
            handle.edge_strip_hover.set(false);
            if !handle.pointer_over.get() && !bar_ui_blocks_hide() {
                schedule_auto_hide(handle);
            }
        }
    });
}

fn cancel_hide_timeout(handle: &BarHandle) {
    if let Some(id) = handle.hide_timeout.borrow_mut().take() {
        id.remove();
    }
}

fn bar_ui_blocks_hide() -> bool {
    dropdown::is_open()
        || crate::ui::dashboard::is_open()
        || crate::ui::notification_center::is_open()
}

/// Slide every bar to its peek strip (used when Auto-hide is toggled on).
fn force_auto_hide_all() {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            if !handle.config.borrow().auto_hide || handle.chrome_suppressed.get() {
                continue;
            }
            if bar_ui_blocks_hide() {
                continue;
            }
            cancel_hide_timeout(handle);
            handle.pointer_over.set(false);
            handle.edge_strip_hover.set(false);
            handle.auto_hide_hidden.set(true);
            let cfg = handle.config.borrow().clone();
            apply_auto_hide_visual(handle, &cfg);
        }
    });
}

fn apply_auto_hide_visual(handle: &BarHandle, cfg: &BarConfig) {
    // Drop any previous per-bar transform provider.
    if let Some(prev) = handle.autohide_css.borrow_mut().take() {
        gtk::style_context_remove_provider_for_display(&handle.outer.display(), &prev);
    }
    clear_autohide_transform(&handle.outer);

    if !handle.auto_hide_hidden.get() {
        let _ = metis_protocol::set_bar_auto_hidden_flag(false);
        return;
    }

    // GTK's CSS engine often rejects `calc()` inside `transform`, so compute a
    // pixel offset in code and inject it via CssProvider.
    let peek = cfg.auto_hide_peek_px.clamp(2, 8) as i32;
    let slide = (auto_hide_slide_extent(handle, cfg) - peek).max(1);
    let transform = match cfg.position {
        BarPosition::Top => format!("translateY(-{slide}px)"),
        BarPosition::Bottom => format!("translateY({slide}px)"),
        BarPosition::Left => format!("translateX(-{slide}px)"),
        BarPosition::Right => format!("translateX({slide}px)"),
    };
    let css = format!(".metis-bar-outer.metis-bar-autohide-hidden {{ transform: {transform}; }}");
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(
        &handle.outer.display(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    *handle.autohide_css.borrow_mut() = Some(provider);

    handle.outer.add_css_class("metis-bar-autohide-hidden");
    let _ = metis_protocol::set_bar_auto_hidden_flag(true);
}

/// Real on-screen cross-axis extent of the bar, used as the auto-hide slide
/// distance. The layer surface is only *requested* at `bar_body_thickness()`;
/// layer-shell sizes it from the content's natural size, so a taller GTK/theme
/// minimum (Ubuntu 26.04 ships a newer GTK than 24.04) grows the surface. Sliding
/// by the configured thickness then leaves that overflow parked on screen as a
/// fat peek, so take the largest of config, measurement, and live allocation.
fn auto_hide_slide_extent(handle: &BarHandle, cfg: &BarConfig) -> i32 {
    let vertical_bar = matches!(cfg.position, BarPosition::Left | BarPosition::Right);
    let axis = if vertical_bar {
        gtk::Orientation::Horizontal
    } else {
        gtk::Orientation::Vertical
    };
    let (min_extent, nat_extent, _, _) = handle.outer.measure(axis, -1);
    let allocated = if vertical_bar {
        handle.window.width().max(handle.outer.width())
    } else {
        handle.window.height().max(handle.outer.height())
    };
    let peek = cfg.auto_hide_peek_px.clamp(2, 8) as i32;
    bar_body_thickness(cfg)
        .max(min_extent)
        .max(nat_extent)
        .max(allocated)
        .max(peek + 1)
}

fn restore_exclusive_zone(window: &gtk::Window, config: &BarConfig) {
    let visible_thickness = bar_cross_thickness(config);
    let exclusive = match config.position {
        BarPosition::Top | BarPosition::Bottom => visible_thickness,
        BarPosition::Left | BarPosition::Right => 0,
    };
    window.set_exclusive_zone(exclusive);
}

fn clear_autohide_transform(outer: &gtk::Box) {
    outer.remove_css_class("metis-bar-autohide-hidden");
    outer.remove_css_class("metis-bar-autohide-peeking");
    for i in 1..=8 {
        outer.remove_css_class(&format!("metis-bar-autohide-peek-{i}"));
    }
    outer.remove_css_class("metis-bar-edge-top");
    outer.remove_css_class("metis-bar-edge-bottom");
    outer.remove_css_class("metis-bar-edge-left");
    outer.remove_css_class("metis-bar-edge-right");
}

/// Re-apply geometry/widgets to every existing bar in place (keeps the layer
/// surfaces — used for live theme/opacity/position edits that don't change the
/// set of outputs).
fn rebuild_bars_in_place(config: Rc<RefCell<BarConfig>>) {
    dropdown::close_all();
    // Closing CC before remount avoids a visible dash_host fighting the pill
    // for the fixed strip height/width after an edge change.
    crate::ui::dashboard::request_close();
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        let cfg = config.borrow();
        let (win_w, win_h) = layer_window_size(&cfg);
        for handle in bars.iter_mut() {
            configure_surface(&handle.outer, &handle.column, &handle.pill, &cfg);
            remount_bar_chrome(handle, &cfg);
            apply_layer_geometry(&handle.window, &cfg);
            let shell = BarShell {
                window: handle.window.clone(),
                outer: handle.outer.clone(),
                column: handle.column.clone(),
                host: handle.dash_host.clone(),
            };
            ensure_bar_strip_geometry(&shell);
            apply_bar_visibility(handle);
            handle.window.set_default_size(win_w, win_h);
            handle.outer.queue_resize();
            handle.column.queue_resize();
            handle.pill.queue_resize();
            while let Some(child) = handle.pill.first_child() {
                handle.pill.remove(&child);
            }
            handle.widget_refs =
                widgets::build(&handle.pill, config.clone(), handle.output.clone(), shell);
            rehydrate_widget_state(&handle.widget_refs);
        }
    });
    remember_menu_layout();
}

/// Re-apply cached service state after tearing down and rebuilding bar widgets.
fn rehydrate_widget_state(refs: &widgets::WidgetRefs) {
    if let Some(snapshot) = last_weather_snapshot() {
        refs.apply_weather(&snapshot);
    } else if !crate::ui::onboarding::is_active() {
        weather_refresh();
    }
    crate::services::sync_tray();
}

/// Whether a bar.json change requires destroying and recreating bar widgets.
fn needs_widget_rebuild(old: &BarConfig, new: &BarConfig) -> bool {
    old.widgets != new.widgets
        || old.position != new.position
        || old.displays != new.displays
        || old.clock != new.clock
        || old.workspace_count != new.workspace_count
}

/// Live-update geometry, CSS, and widget settings without closing popovers or
/// recreating widgets (opacity, tray mode, taskbar pins, margins, etc.).
fn apply_bars_live(config: Rc<RefCell<BarConfig>>) {
    let cfg = config.borrow().clone();
    BAR_POSITION.with(|p| p.set(cfg.position));
    let (win_w, win_h) = layer_window_size(&cfg);
    BARS.with(|bars| {
        for handle in bars.borrow_mut().iter_mut() {
            configure_surface(&handle.outer, &handle.column, &handle.pill, &cfg);
            apply_layer_geometry(&handle.window, &cfg);
            let shell = BarShell {
                window: handle.window.clone(),
                outer: handle.outer.clone(),
                column: handle.column.clone(),
                host: handle.dash_host.clone(),
            };
            ensure_bar_strip_geometry(&shell);
            apply_bar_visibility(handle);
            handle.window.set_default_size(win_w, win_h);
            handle.outer.queue_resize();
            handle.column.queue_resize();
            handle.pill.queue_resize();
            handle.widget_refs.apply_bar_config(&cfg);
            rehydrate_widget_state(&handle.widget_refs);
        }
    });
    refresh_taskbars();
}

/// Force-recreate edge-bar chrome (and dependent overlays) after a language change.
fn rebuild_for_locale() {
    if crate::ui::onboarding::is_active() {
        tracing::debug!("locale bar rebuild deferred — onboarding active");
        return;
    }
    let config = BARS.with(|bars| bars.borrow().first().map(|h| h.config.clone()));
    let Some(config) = config else {
        return;
    };
    rebuild_bars_in_place(config);
    crate::ui::theme::reload_stylesheet();
    crate::ui::notification_center::reload_for_locale();
    crate::ui::dashboard::reload_for_locale();
    // Desktop widgets process watches locale.json / its own command file.
    // Re-apply weather with freshly translated condition/hour strings.
    if let Some(snapshot) = last_weather_snapshot() {
        BARS.with(|bars| {
            for handle in bars.borrow().iter() {
                handle.widget_refs.apply_weather(&snapshot);
            }
        });
    }
}

/// Tear down all bars and rebuild from scratch for the current monitor set —
/// used when the number of target outputs changes (monitor hotplug or toggling
/// the `displays` option).
fn rebuild_all_bars(config: Rc<RefCell<BarConfig>>) {
    dropdown::close_all();
    let cfg = config.borrow().clone();
    // Build the new bars *before* destroying the old ones to avoid a one-frame
    // flash with no bar on screen. (The shell runs its own GLib main loop, so an
    // empty window set never quits it.)
    let new_handles: Vec<BarHandle> = target_monitors(&cfg)
        .iter()
        .map(|m| build_bar(config.clone(), m.as_ref()))
        .collect();
    let old = BARS.with(|bars| std::mem::replace(&mut *bars.borrow_mut(), new_handles));
    for handle in old {
        handle.window.destroy();
    }
    remember_menu_layout();
}

fn watch_bar_config() {
    let path = crate::config::bar_config_path();
    if !path.exists() {
        if let Err(err) = crate::config::save_default_bar_config() {
            tracing::warn!(%err, "failed to create default bar.json");
        }
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "bar.json file monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            // Re-reads bar.json and rebuilds in place, or recreates the surfaces if
            // the `displays` option changed the number of bars.
            rebuild_from_config();
        });
    });
}

fn watch_dashboard_config() {
    let path = crate::config::dashboard_config_path();
    if !path.exists() {
        if let Err(err) = crate::config::save_default_dashboard_config() {
            tracing::warn!(%err, "failed to create default dashboard.json");
        }
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "dashboard.json file monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            crate::ui::dashboard::on_dashboard_config_changed();
        });
    });
}

fn spawn_gaming_daemon() {
    if std::env::var_os("METIS_NO_GAMINGD").is_some() {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("metis-gamingd")))
        .filter(|p| p.is_file());
    let exe = exe.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("metis-gamingd"))
                .find(|p| p.is_file())
        })
    });
    let Some(exe) = exe else {
        tracing::debug!("metis-gamingd not found beside shell or on PATH");
        return;
    };
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => tracing::info!(path = %exe.display(), "spawned metis-gamingd"),
        Err(err) => tracing::warn!(%err, "failed to spawn metis-gamingd"),
    }
}

/// Live-reload the active theme when any `themes/*.json` changes. Mirrors
/// `watch_bar_config`: the GFileMonitor stays alive via the main-context source,
/// and the debounced callback re-runs `init_theme()` (which re-reads the active
/// mode + on-disk token file and re-applies the CssProvider).
fn watch_theme_files() {
    let dir = crate::config::config_dir().join("themes");
    if let Err(err) = crate::config::ensure_config_dirs() {
        tracing::warn!(%err, "failed to ensure themes dir");
    }
    let file = gio::File::for_path(&dir);
    let Ok(monitor) =
        file.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %dir.display(), "themes dir monitor unavailable");
        return;
    };

    monitor.connect_changed(move |_, _, _event, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), || {
            let _ = crate::ui::theme::init_theme();
        });
    });
}

fn attach_weather_channel(rx: Receiver<WeatherSnapshot>) {
    glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
        while let Ok(snapshot) = rx.try_recv() {
            tracing::debug!(
                locations = snapshot.locations.len(),
                error = ?snapshot.error,
                "weather: UI received snapshot"
            );
            // Cache on the GTK thread too (worker already stores process-wide).
            crate::services::weather::remember_snapshot(&snapshot);
            BARS.with(|bars| {
                for handle in bars.borrow().iter() {
                    handle.widget_refs.apply_weather(&snapshot);
                }
            });
        }
        glib::ControlFlow::Continue
    });
}

fn attach_notification_channel(channels: crate::services::NotifyChannels) {
    let crate::services::NotifyChannels { incoming, actions } = channels;
    // Register the outgoing sender so popover/toast buttons can round-trip
    // ActionInvoked / NotificationClosed back to the originating apps.
    crate::services::set_action_sender(actions);

    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        while let Ok(event) = incoming.try_recv() {
            match event {
                crate::services::NotifyIncoming::Show(note) => {
                    let dnd = widgets::do_not_disturb();
                    if !dnd {
                        if !note.suppress_sound {
                            crate::services::play_notification_sound(&note);
                        }
                        crate::ui::toast::show(&note);
                    }
                    crate::services::push_notification(note);
                }
                crate::services::NotifyIncoming::Closed { id } => {
                    crate::services::dismiss_notification_by_dbus_id(id);
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn attach_tray_channel(events: std::sync::mpsc::Receiver<crate::services::TrayEvent>) {
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(event) = events.try_recv() {
            apply_event(event);
        }
        glib::ControlFlow::Continue
    });
}

fn attach_poll_channel(rx: Receiver<BarSnapshot>) {
    let mut last = BarSnapshot::default();
    glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
        while let Ok(snapshot) = rx.try_recv() {
            if snapshot == last {
                continue;
            }
            last = snapshot.clone();
            check_bluetooth_battery_alerts(&snapshot.bluetooth);
            BARS.with(|bars| {
                for handle in bars.borrow().iter() {
                    handle.widget_refs.apply_snapshot(&snapshot);
                }
            });
        }
        glib::ControlFlow::Continue
    });
}

/// Charge level (inclusive) at or below which a connected Bluetooth device is
/// considered "low" and worth a charge reminder.
const BT_BATTERY_LOW: u8 = 20;
/// Charge level a device must climb back above before it can alert again, so a
/// reading hovering around the threshold can't spam repeated notifications.
const BT_BATTERY_CLEAR: u8 = 25;

thread_local! {
    /// Per-device (by MAC) latch: `true` once we've fired a low-battery alert,
    /// cleared when the device recharges past `BT_BATTERY_CLEAR` or disconnects.
    static BT_LOW_ALERTED: std::cell::RefCell<std::collections::HashMap<String, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Fire a one-shot charge reminder when a connected Bluetooth device's battery
/// drops to a low level. Uses hysteresis + a per-device latch so each low
/// episode notifies exactly once, and prunes state for disconnected devices.
fn check_bluetooth_battery_alerts(status: &crate::services::BluetoothStatus) {
    use crate::services::{BarNotification, NotificationKind};

    BT_LOW_ALERTED.with(|cell| {
        let mut latched = cell.borrow_mut();
        latched.retain(|addr, _| status.devices.iter().any(|d| &d.address == addr));

        for dev in &status.devices {
            let Some(pct) = dev.battery_percent else {
                continue;
            };
            // Don't nag to charge a device that's already charging.
            if dev.battery_charging == Some(true) {
                latched.insert(dev.address.clone(), false);
                continue;
            }
            let already = latched.get(&dev.address).copied().unwrap_or(false);
            if pct <= BT_BATTERY_LOW && !already {
                latched.insert(dev.address.clone(), true);
                let mut note = BarNotification::internal(
                    NotificationKind::Error,
                    format!("{} battery low", dev.name),
                    format!("{pct}% remaining — charge it soon."),
                );
                note.sound_name = Some("battery-low".to_string());
                emit_internal_notification(note);
            } else if pct >= BT_BATTERY_CLEAR && already {
                latched.insert(dev.address.clone(), false);
            }
        }
    });
}

/// Deliver a Metis-originated notification through the same path as incoming
/// D-Bus ones: play a sound and show a toast (unless Do Not Disturb is on), then
/// store it in the in-bar notification list.
fn emit_internal_notification(note: crate::services::BarNotification) {
    if !widgets::do_not_disturb() {
        if !note.suppress_sound {
            crate::services::play_notification_sound(&note);
        }
        crate::ui::toast::show(&note);
    }
    crate::services::push_notification(note);
}

/// Toast + notification-center card when a monitor is plugged or unplugged.
pub fn notify_display_hotplug(connected: bool, name: &str, make: &str, model: &str) {
    use crate::services::{BarNotification, NotificationKind};

    // DRM modeset around hotplug can make NetworkManager drop the association
    // briefly — never write `nmcli radio wifi off` from that flake.
    crate::services::suppress_wifi_radio_writes(20);

    let pretty = {
        let branded = format!("{} {}", make.trim(), model.trim())
            .trim()
            .to_string();
        if branded.is_empty()
            || branded.eq_ignore_ascii_case("unknown unknown")
            || branded.eq_ignore_ascii_case("unknown")
        {
            name.to_string()
        } else {
            branded
        }
    };

    let (title, message, osd_label) = if connected {
        (
            metis_i18n::tr("Display connected"),
            metis_i18n::tr("%1 is ready to use.").replace("%1", &pretty),
            metis_i18n::tr("Display connected"),
        )
    } else {
        (
            metis_i18n::tr("Display disconnected"),
            metis_i18n::tr("%1 was unplugged.").replace("%1", &pretty),
            metis_i18n::tr("Display disconnected"),
        )
    };

    crate::ui::osd::show("video-display-symbolic", &osd_label, None, false);

    let mut note = BarNotification::internal(NotificationKind::Notification, title, message);
    note.sound_name = Some(if connected {
        "device-added".to_string()
    } else {
        "device-removed".to_string()
    });
    emit_internal_notification(note);
}

/// Mirror a user audio change (volume/mic/mute) onto every bar immediately, so a
/// multi-monitor session doesn't wait for the pactl poll round-trip to update the
/// other displays' volume icons/sliders.
pub fn broadcast_audio(percent: u8, muted: bool, mic_percent: u8, mic_muted: bool) {
    BARS.with(|bars| {
        for handle in bars.borrow().iter() {
            handle
                .widget_refs
                .apply_volume_optimistic(percent, muted, mic_percent, mic_muted);
        }
    });
}

/// Close all bar dropdown popovers (e.g. before a bar surface rebuild).
pub fn close_popovers() {
    dropdown::request_close_all();
    crate::ui::notification_center::dismiss();
}

pub fn rebuild_from_config() {
    if crate::ui::onboarding::is_active() {
        tracing::debug!("bar rebuild deferred — onboarding active");
        return;
    }
    // Coalesce bursts: one settings change writes bar.json (which can emit several
    // file-change events) *and* sends a `reload-bar` runtime command, so multiple
    // rebuild triggers land within a few hundred ms. Collapsing them into a single
    // deferred rebuild avoids overlapping teardown/rebuild passes racing each
    // other (and the deferred surface present) while bars are being recreated.
    if REBUILD_SCHEDULED.with(|f| f.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(std::time::Duration::from_millis(80), || {
        REBUILD_SCHEDULED.with(|f| f.set(false));
        if crate::ui::onboarding::is_active() {
            tracing::debug!("bar rebuild deferred — onboarding active");
            return;
        }
        rebuild_from_config_now();
    });
}

/// Apply the current `bar.json` immediately. Called after onboarding dismisses
/// its overlay window so bar layer surfaces are not rebuilt concurrently.
pub fn apply_bar_config_now() {
    let (config, cur_count) = BARS.with(|bars| {
        let bars = bars.borrow();
        (bars.first().map(|h| h.config.clone()), bars.len())
    });
    let Some(config) = config else {
        return;
    };
    let old = config.borrow().clone();
    let new = load_bar_config();
    apply_config_diff(config, cur_count, &old, &new);
}

fn apply_config_diff(
    config: Rc<RefCell<BarConfig>>,
    cur_count: usize,
    old: &BarConfig,
    new: &BarConfig,
) {
    let enabling_auto_hide = new.auto_hide && !old.auto_hide;
    let menu_layout_changed = take_menu_layout_changed();
    *config.borrow_mut() = new.clone();
    let target_count = target_monitors(new).len();
    if target_count != cur_count {
        rebuild_all_bars(config.clone());
    } else if menu_layout_changed || needs_widget_rebuild(old, new) {
        rebuild_bars_in_place(config.clone());
    } else {
        apply_bars_live(config.clone());
    }
    crate::ui::theme::reload_stylesheet();
    // Toggle ON from Settings: hide immediately so the user sees the effect
    // without waiting for a pointer leave (and despite a stale pointer_over).
    if enabling_auto_hide {
        force_auto_hide_all();
    }
}

fn rebuild_from_config_now() {
    let (config, cur_count) = BARS.with(|bars| {
        let bars = bars.borrow();
        (bars.first().map(|h| h.config.clone()), bars.len())
    });
    let Some(config) = config else {
        return;
    };
    let old = config.borrow().clone();
    let new = load_bar_config();
    apply_config_diff(config, cur_count, &old, &new);
    crate::ui::dashboard::on_bar_config_changed();
    crate::ui::notification_center::on_bar_config_changed();
}
