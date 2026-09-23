//! Free-floating desktop widgets over the wallpaper (Phase 14).
//!
//! One transparent layer-shell surface per output hosts many widget cards on a
//! `GtkFixed` canvas. Master switch defaults off; edit mode enables move/resize
//! for unlocked instances. Empty chrome may still receive pointer hits in v1
//! (imperfect click-through — documented in TODO).

mod content;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::{
    DesktopWidgetInstance, DesktopWidgetsConfig, desktop_widgets_config_path,
    load_desktop_widgets_config, save_desktop_widgets_config,
};

const MIN_W: i32 = 160;
const MIN_H: i32 = 120;
const RESIZE_HANDLE: i32 = 20;
const SAVE_DEBOUNCE_MS: u64 = 350;
/// Ignore config reloads while dragging and shortly after we write geometry.
const RELOAD_SUPPRESS_AFTER_SAVE: Duration = Duration::from_millis(900);
/// Block disk reloads from the moment geometry changes (not only after save).
const RELOAD_SUPPRESS_ON_EDIT: Duration = Duration::from_millis(1200);
/// Coalesce rapid Settings/file-monitor reloads (opacity slider, etc.).
const RELOAD_DEBOUNCE_MS: u64 = 180;

struct HostSurface {
    window: gtk::Window,
    canvas: gtk::Fixed,
    /// Compositor / GDK connector name (`DP-1`, `metis-0`, …).
    output: String,
    /// True when this host is bound to the session's first monitor (primary).
    is_primary: bool,
}

thread_local! {
    static HOSTS: RefCell<Vec<HostSurface>> = const { RefCell::new(Vec::new()) };
    static CFG: RefCell<DesktopWidgetsConfig> = RefCell::new(DesktopWidgetsConfig::default());
    static INITED: Cell<bool> = const { Cell::new(false) };
    static FILE_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static AUX_MONITORS: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
    static SAVE_PENDING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    /// Active move/resize gestures — file monitor / Settings reload must not
    /// tear down the surface mid-drag (that caused stutter + resize rubberbanding).
    static INTERACTION: Cell<u32> = const { Cell::new(0) };
    static SUPPRESS_RELOAD_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Per-card CssProviders (keyed by sanitized instance id). Kept alive so
    /// USER-priority chrome CSS outlives the load call.
    static CARD_CHROME: RefCell<std::collections::HashMap<String, gtk::CssProvider>> =
        RefCell::new(std::collections::HashMap::new());
    /// Coalesce file-monitor / IPC reload storms (slider drags write often).
    static RELOAD_DEBOUNCE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    /// Language Apply must rebuild even when desktop-widgets.json is unchanged.
    static LOCALE_REBUILD_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Start the desktop-widgets host (idempotent). Called once from bar startup.
pub fn init() {
    if INITED.replace(true) {
        return;
    }
    scan_widget_packs_at_startup();
    reload_from_disk();
    watch_config_file();
    watch_monitors();
}

/// Full autonomous host for `metis-shell --desktop-widgets`: config/theme/menu/
/// locale watches, weather fan-out, and a dedicated runtime command file.
pub fn init_autonomous() {
    init();
    watch_menu_pins();
    watch_theme_and_config();
    watch_locale_file();
    watch_widgets_commands();
    attach_weather();
    crate::services::watch_app_index();
}

/// Fail-closed pack scan (Phase 18 C): log skips and toast once if any pack dropped.
fn scan_widget_packs_at_startup() {
    let result = metis_config::discover_widget_extensions_detailed();
    if result.skipped.is_empty() {
        return;
    }
    let n = result.skipped.len();
    tracing::warn!(
        skipped = n,
        packs = result.packs.len(),
        "invalid widget extension packs skipped at startup"
    );
    let message = format!(
        "{} ({n}) — {}",
        metis_i18n::tr("Skipped invalid widget pack(s)"),
        metis_i18n::tr("see logs")
    );
    crate::ui::toast::show(&crate::services::BarNotification::internal(
        crate::services::NotificationKind::Information,
        metis_i18n::tr("Desktop widgets"),
        message,
    ));
}

/// Re-read `desktop-widgets.json` and rebuild host surfaces.
pub fn reload() {
    schedule_reload();
}

/// Language Apply — config identity is unchanged, so [`reload`] / `reload_from_disk`
/// would short-circuit and leave English chrome. Force a full host rebuild.
pub fn reload_for_locale() {
    if !INITED.get() {
        return;
    }
    LOCALE_REBUILD_PENDING.set(true);
    if interaction_blocks_reload() || SAVE_PENDING.with(|c| c.borrow().is_some()) {
        schedule_reload();
        return;
    }
    // Skip debounce: Settings Apply should refresh cards immediately.
    reload_from_disk();
}

/// Theme tokens changed — rebuild cards so empty chrome colours pick up the new surface/text.
pub fn on_theme_changed() {
    if !INITED.get() {
        return;
    }
    content::refresh_opaque_dialog_css();
    if interaction_blocks_reload() {
        return;
    }
    // Theme tint changes only affect resolved chrome colours.
    let cfg = current_cfg();
    if hosts_are_live() && cfg.enabled {
        apply_all_card_chrome(&cfg);
    } else {
        rebuild_hosts();
    }
}

/// Start-menu pin list changed — refresh Apps widgets that follow `menu.json`.
pub fn on_menu_pins_changed() {
    if !INITED.get() {
        return;
    }
    let cfg = current_cfg();
    if !cfg.enabled {
        return;
    }
    let follows_menu = cfg
        .instances
        .iter()
        .any(|i| i.kind == metis_config::DesktopWidgetKind::Apps && i.pins.is_empty());
    if !follows_menu {
        return;
    }
    if interaction_blocks_reload() {
        schedule_reload();
        return;
    }
    rebuild_hosts();
}

/// Fresh weather snapshot from the bar service — update live desktop weather cards.
pub fn on_weather_snapshot(snapshot: &crate::services::WeatherSnapshot) {
    content::weather::on_snapshot(snapshot);
}

fn schedule_reload() {
    if !INITED.get() {
        return;
    }
    if let Some(id) = RELOAD_DEBOUNCE.with(|c| c.borrow_mut().take()) {
        id.remove();
    }

    // If a drag/save suppress window is active, wait until it ends — do not spin
    // every debounce tick (that caused post-drop rebuild storms).
    let delay = if let Some(until) = SUPPRESS_RELOAD_UNTIL.get() {
        let now = Instant::now();
        if now < until {
            (until - now) + Duration::from_millis(50)
        } else {
            Duration::from_millis(RELOAD_DEBOUNCE_MS)
        }
    } else if INTERACTION.get() > 0 {
        Duration::from_millis(RELOAD_DEBOUNCE_MS.max(250))
    } else {
        Duration::from_millis(RELOAD_DEBOUNCE_MS)
    };

    let id = glib::timeout_add_local(delay, || {
        RELOAD_DEBOUNCE.with(|c| *c.borrow_mut() = None);
        if interaction_blocks_reload() || SAVE_PENDING.with(|c| c.borrow().is_some()) {
            tracing::debug!("desktop widgets reload deferred (edit / pending save)");
            schedule_reload();
            return glib::ControlFlow::Break;
        }
        reload_from_disk();
        glib::ControlFlow::Break
    });
    RELOAD_DEBOUNCE.with(|c| *c.borrow_mut() = Some(id));
}

fn interaction_blocks_reload() -> bool {
    if INTERACTION.get() > 0 {
        return true;
    }
    if let Some(until) = SUPPRESS_RELOAD_UNTIL.get() {
        if Instant::now() < until {
            return true;
        }
        SUPPRESS_RELOAD_UNTIL.set(None);
    }
    false
}

fn suppress_reloads_for(duration: Duration) {
    let until = Instant::now() + duration;
    match SUPPRESS_RELOAD_UNTIL.get() {
        Some(prev) if prev > until => {}
        _ => SUPPRESS_RELOAD_UNTIL.set(Some(until)),
    }
}

fn begin_interaction() {
    INTERACTION.set(INTERACTION.get().saturating_add(1));
    suppress_reloads_for(RELOAD_SUPPRESS_ON_EDIT);
}

fn end_interaction() {
    INTERACTION.set(INTERACTION.get().saturating_sub(1));
    // Keep suppress briefly after release so the debounced save + file monitor
    // cannot rebuild hosts from stale disk geometry.
    suppress_reloads_for(RELOAD_SUPPRESS_ON_EDIT);
}

fn hosts_are_live() -> bool {
    HOSTS.with(|h| !h.borrow().is_empty())
}

fn reload_from_disk() {
    // Never clobber in-flight geometry edits with a stale file read.
    if SAVE_PENDING.with(|c| c.borrow().is_some()) {
        schedule_reload();
        return;
    }

    let cfg = load_desktop_widgets_config();
    let prev = current_cfg();
    let force_locale = LOCALE_REBUILD_PENDING.replace(false);

    // Locale Apply: same JSON identity, but construction-time `tr()` labels must
    // be rebuilt. Do not take the geometry/chrome short-circuits below.
    if force_locale {
        CFG.with(|cell| *cell.borrow_mut() = cfg);
        rebuild_hosts();
        if let Some(snapshot) = crate::services::last_weather_snapshot() {
            content::weather::on_snapshot(&snapshot);
        }
        return;
    }

    // Same widgets, only positions/sizes differ → move in place (no flicker).
    if hosts_are_live() && cfg.enabled && same_widget_identity(&prev, &cfg) {
        CFG.with(|cell| *cell.borrow_mut() = cfg.clone());
        if !same_widget_geometry(&prev, &cfg) {
            apply_geometry_in_place(&cfg);
        }
        if prev.chrome != cfg.chrome
            || prev
                .instances
                .iter()
                .zip(&cfg.instances)
                .any(|(a, b)| a.chrome != b.chrome)
        {
            apply_all_card_chrome(&cfg);
        }
        return;
    }

    let layout_same = hosts_are_live() && cfg.enabled && same_widget_layout(&prev, &cfg);
    CFG.with(|cell| *cell.borrow_mut() = cfg.clone());

    if layout_same {
        if prev.chrome != cfg.chrome
            || prev
                .instances
                .iter()
                .zip(&cfg.instances)
                .any(|(a, b)| a.chrome != b.chrome)
        {
            apply_all_card_chrome(&cfg);
        }
        return;
    }

    rebuild_hosts();
}

/// Layout / content identity (everything except chrome). When this matches, only
/// CSS needs updating.
fn same_widget_layout(a: &DesktopWidgetsConfig, b: &DesktopWidgetsConfig) -> bool {
    same_widget_identity(a, b) && same_widget_geometry(a, b)
}

fn same_widget_identity(a: &DesktopWidgetsConfig, b: &DesktopWidgetsConfig) -> bool {
    a.enabled == b.enabled
        && a.edit_mode == b.edit_mode
        && a.instances.len() == b.instances.len()
        && a.instances.iter().zip(&b.instances).all(|(x, y)| {
            x.id == y.id
                && x.kind == y.kind
                && x.output == y.output
                && x.locked == y.locked
                && x.path == y.path
                && x.pins == y.pins
                && x.view == y.view
                && x.show_title == y.show_title
                && x.font == y.font
                && x.text_color == y.text_color
                && x.accent_color == y.accent_color
                && x.viz_style == y.viz_style
                && x.bar_shape == y.bar_shape
                && x.color_mode == y.color_mode
                && x.solid_color == y.solid_color
                && x.gradient_start == y.gradient_start
                && x.gradient_end == y.gradient_end
                && x.bar_count == y.bar_count
                && x.bar_gradient == y.bar_gradient
                && x.show_peaks == y.show_peaks
                && x.peak_color == y.peak_color
                && x.show_reflection == y.show_reflection
        })
}

fn same_widget_geometry(a: &DesktopWidgetsConfig, b: &DesktopWidgetsConfig) -> bool {
    a.instances
        .iter()
        .zip(&b.instances)
        .all(|(x, y)| x.x == y.x && x.y == y.y && x.w == y.w && x.h == y.h)
}

fn apply_geometry_in_place(cfg: &DesktopWidgetsConfig) {
    HOSTS.with(|hosts| {
        for host in hosts.borrow().iter() {
            let mut child = host.canvas.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                let name = widget.widget_name();
                if name.is_empty() {
                    continue;
                }
                let Some(inst) = cfg.instances.iter().find(|i| i.id == name.as_str()) else {
                    continue;
                };
                if !instance_belongs(inst, host) {
                    continue;
                }
                host.canvas.move_(&widget, inst.x as f64, inst.y as f64);
                widget.set_size_request(inst.w as i32, inst.h as i32);
            }
        }
    });
}

fn apply_all_card_chrome(cfg: &DesktopWidgetsConfig) {
    for inst in &cfg.instances {
        let style_class = format!("metis-dw-c-{}", sanitize_css_id(&inst.id));
        apply_card_chrome(
            &style_class,
            &cfg.chrome.resolve(&inst.chrome),
            &inst.text_color,
            &inst.accent_color,
        );
    }
}

fn current_cfg() -> DesktopWidgetsConfig {
    CFG.with(|cell| cell.borrow().clone())
}

fn rebuild_hosts() {
    let cfg = current_cfg();
    tear_down_hosts();

    if !cfg.enabled {
        tracing::debug!("desktop widgets disabled — no host surfaces");
        return;
    }

    let monitors = connected_monitors();
    if monitors.is_empty() {
        tracing::warn!("desktop widgets enabled but no GDK monitors");
        return;
    }

    let mut hosts = Vec::with_capacity(monitors.len());
    for (idx, monitor) in monitors.iter().enumerate() {
        let output = monitor_output_name(monitor).unwrap_or_default();
        let is_primary = idx == 0;
        let host = build_host(monitor, output, is_primary);
        populate_host(&host, &cfg);
        hosts.push(host);
    }
    HOSTS.with(|cell| *cell.borrow_mut() = hosts);
    tracing::info!(
        outputs = monitors.len(),
        instances = cfg.instances.len(),
        edit = cfg.edit_mode,
        "desktop widgets host ready"
    );
}

fn tear_down_hosts() {
    let old = HOSTS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    for host in old {
        host.window.destroy();
    }
}

fn build_host(monitor: &gdk::Monitor, output: String, is_primary: bool) -> HostSurface {
    let geo = monitor.geometry();
    let window = gtk::Window::builder()
        .title("Metis Desktop Widgets")
        .default_width(geo.width().max(1))
        .default_height(geo.height().max(1))
        .build();
    window.add_css_class("metis-desktop-widgets-window");
    window.init_layer_shell();
    // Bottom sits above the wallpaper (Background) and below normal windows / Top bar.
    window.set_layer(Layer::Bottom);
    window.set_namespace(Some("metis-desktop-widgets"));
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_exclusive_zone(0);
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, true);
        window.set_margin(edge, 0);
    }
    window.set_monitor(Some(monitor));

    let canvas = gtk::Fixed::new();
    canvas.add_css_class("metis-desktop-widgets-canvas");
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    window.set_child(Some(&canvas));
    window.present();

    HostSurface {
        window,
        canvas,
        output,
        is_primary,
    }
}

fn populate_host(host: &HostSurface, cfg: &DesktopWidgetsConfig) {
    while let Some(child) = host.canvas.first_child() {
        host.canvas.remove(&child);
    }

    for inst in &cfg.instances {
        if !instance_belongs(inst, host) {
            continue;
        }
        let card = build_card(inst, &cfg.chrome, cfg.edit_mode);
        host.canvas.put(&card, inst.x as f64, inst.y as f64);
    }
}

fn instance_belongs(inst: &DesktopWidgetInstance, host: &HostSurface) -> bool {
    if inst.output.is_empty() {
        return host.is_primary;
    }
    inst.output == host.output
}

fn build_card(
    inst: &DesktopWidgetInstance,
    global_chrome: &metis_config::DesktopWidgetChrome,
    edit_mode: bool,
) -> gtk::Widget {
    let can_edit = edit_mode && !inst.locked;

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("metis-dw-card");
    // Stable id for in-place geometry updates after config reload.
    outer.set_widget_name(&inst.id);
    let style_class = format!("metis-dw-c-{}", sanitize_css_id(&inst.id));
    outer.add_css_class(&style_class);
    if can_edit {
        outer.add_css_class("metis-dw-edit");
    }
    if inst.locked {
        outer.add_css_class("metis-dw-locked");
    }
    outer.set_size_request(inst.w as i32, inst.h as i32);
    outer.set_overflow(gtk::Overflow::Hidden);

    apply_card_chrome(
        &style_class,
        &global_chrome.resolve(&inst.chrome),
        &inst.text_color,
        &inst.accent_color,
    );

    // Title bar: full chrome when shown; thin drag strip in edit mode when hidden.
    let show_header = inst.show_title || can_edit;
    let mut move_target: Option<gtk::Box> = None;
    if show_header {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("metis-dw-header");
        if inst.show_title {
            header.set_height_request(28);
            let title = gtk::Label::new(Some(&metis_i18n::tr(&inst.display_title())));
            title.add_css_class("metis-dw-title");
            title.set_xalign(0.0);
            title.set_hexpand(true);
            title.set_can_target(false);
            header.append(&title);
            if inst.locked {
                let lock = gtk::Label::new(Some(&metis_i18n::tr("Locked")));
                lock.add_css_class("metis-dw-badge");
                lock.set_can_target(false);
                header.append(&lock);
            } else if can_edit {
                let badge = gtk::Label::new(Some(&metis_i18n::tr("Drag title · resize ↘")));
                badge.add_css_class("metis-dw-badge");
                badge.set_can_target(false);
                header.append(&badge);
            }
        } else {
            // Title off but edit mode: keep a grab strip so the card stays movable.
            header.set_height_request(18);
            header.add_css_class("metis-dw-header-slim");
            let badge = gtk::Label::new(Some(&metis_i18n::tr("⋮⋮ drag")));
            badge.add_css_class("metis-dw-badge");
            badge.set_hexpand(true);
            badge.set_xalign(0.5);
            badge.set_can_target(false);
            header.append(&badge);
        }
        outer.append(&header);
        if can_edit {
            move_target = Some(header);
        }
    }

    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.add_css_class("metis-dw-body");
    body.set_hexpand(true);
    body.set_vexpand(true);
    body.append(&content::build(inst));
    outer.append(&body);

    if can_edit {
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        footer.set_halign(gtk::Align::End);
        // Plain box, not a Button — Button + GestureDrag fights and rubberbands.
        let handle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        handle.add_css_class("metis-dw-resize");
        handle.set_size_request(RESIZE_HANDLE, RESIZE_HANDLE);
        handle.set_tooltip_text(Some(&metis_i18n::tr("Drag to resize")));
        let grip = gtk::Label::new(Some("↘"));
        grip.set_can_target(false);
        handle.append(&grip);
        footer.append(&handle);
        outer.append(&footer);

        if let Some(header) = move_target {
            wire_move(&header, &outer, &inst.id);
        }
        wire_resize(&handle, &outer, &inst.id);
    }

    outer.upcast()
}

/// Pointer position in the native surface's coordinate space.
///
/// `GestureDrag::offset()` is widget-local. Moving/resizing that widget under the
/// cursor collapses the offset toward zero (rubberband / jitter). Surface coords
/// stay stable for the lifetime of the gesture.
fn gesture_surface_pos(gesture: &gtk::GestureDrag) -> Option<(f64, f64)> {
    gesture.current_event()?.position()
}

fn wire_move(header: &gtk::Box, card: &gtk::Box, id: &str) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    drag.set_exclusive(true);

    let id = id.to_string();
    let card_origin = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let ptr_origin = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let last_pos = Rc::new(Cell::new((0.0_f64, 0.0_f64)));

    {
        let card = card.clone();
        let card_origin = card_origin.clone();
        let ptr_origin = ptr_origin.clone();
        let last_pos = last_pos.clone();
        drag.connect_drag_begin(move |gesture, _x, _y| {
            begin_interaction();
            if let Some(fixed) = card.parent().and_then(|p| p.downcast::<gtk::Fixed>().ok()) {
                let origin = fixed.child_position(&card);
                card_origin.set(origin);
                last_pos.set(origin);
            }
            if let Some(pos) = gesture_surface_pos(gesture) {
                ptr_origin.set(pos);
            }
        });
    }
    {
        let card = card.clone();
        let card_origin = card_origin.clone();
        let ptr_origin = ptr_origin.clone();
        let last_pos = last_pos.clone();
        drag.connect_drag_update(move |gesture, _ox, _oy| {
            let Some((px, py)) = gesture_surface_pos(gesture) else {
                return;
            };
            let (sx, sy) = ptr_origin.get();
            let (ox, oy) = card_origin.get();
            let nx = (ox + (px - sx)).max(0.0);
            let ny = (oy + (py - sy)).max(0.0);
            last_pos.set((nx, ny));
            // Surface-absolute Fixed moves — no CSS transform, so release cannot
            // "snap back" from a translate/layout mismatch.
            if let Some(fixed) = card.parent().and_then(|p| p.downcast::<gtk::Fixed>().ok()) {
                fixed.move_(&card, nx, ny);
            }
        });
    }
    {
        let card = card.clone();
        let last_pos = last_pos.clone();
        let card_origin = card_origin.clone();
        let ptr_origin = ptr_origin.clone();
        drag.connect_drag_end(move |gesture, _, _| {
            let (mut nx, mut ny) = last_pos.get();
            if let Some((px, py)) = gesture_surface_pos(gesture) {
                let (sx, sy) = ptr_origin.get();
                let (ox, oy) = card_origin.get();
                nx = (ox + (px - sx)).max(0.0);
                ny = (oy + (py - sy)).max(0.0);
            }
            if let Some(fixed) = card.parent().and_then(|p| p.downcast::<gtk::Fixed>().ok()) {
                fixed.move_(&card, nx, ny);
            }
            let (ox, oy) = card_origin.get();
            if (nx - ox).abs() >= 1.0 || (ny - oy).abs() >= 1.0 {
                update_instance_geometry(&id, |inst| {
                    inst.x = nx.round() as i32;
                    inst.y = ny.round() as i32;
                });
            }
            end_interaction();
        });
    }
    {
        drag.connect_cancel(move |_, _| {
            end_interaction();
        });
    }

    header.add_controller(drag);
}

fn wire_resize(handle: &gtk::Box, card: &gtk::Box, id: &str) {
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    drag.set_exclusive(true);

    let id = id.to_string();
    let start_size = Rc::new(Cell::new((0_i32, 0_i32)));
    let ptr_origin = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let pending = Rc::new(Cell::new((0_i32, 0_i32)));

    {
        let start_size = start_size.clone();
        let ptr_origin = ptr_origin.clone();
        let pending = pending.clone();
        let card = card.clone();
        drag.connect_drag_begin(move |gesture, _, _| {
            begin_interaction();
            let w = card.width().max(card.width_request()).max(MIN_W);
            let h = card.height().max(card.height_request()).max(MIN_H);
            start_size.set((w, h));
            pending.set((w, h));
            if let Some(pos) = gesture_surface_pos(gesture) {
                ptr_origin.set(pos);
            }
        });
    }
    {
        let start_size = start_size.clone();
        let ptr_origin = ptr_origin.clone();
        let pending = pending.clone();
        let card = card.clone();
        drag.connect_drag_update(move |gesture, _, _| {
            let Some((px, py)) = gesture_surface_pos(gesture) else {
                return;
            };
            let (sx, sy) = ptr_origin.get();
            let (sw, sh) = start_size.get();
            let nw = ((sw as f64 + (px - sx)).round() as i32).max(MIN_W);
            let nh = ((sh as f64 + (py - sy)).round() as i32).max(MIN_H);
            pending.set((nw, nh));
            // Apply immediately from surface deltas (not widget-local offset) so
            // growing the card under the grip cannot collapse the gesture.
            card.set_size_request(nw, nh);
        });
    }
    {
        let card = card.clone();
        let pending = pending.clone();
        drag.connect_drag_end(move |_, _, _| {
            let (w, h) = pending.get();
            let w = w.max(MIN_W) as u32;
            let h = h.max(MIN_H) as u32;
            card.set_size_request(w as i32, h as i32);
            update_instance_geometry(&id, |inst| {
                inst.w = w.clamp(160, 2400);
                inst.h = h.clamp(120, 1800);
            });
            end_interaction();
        });
    }
    {
        drag.connect_cancel(move |_, _| {
            end_interaction();
        });
    }

    handle.add_controller(drag);
}

fn update_instance_geometry(id: &str, f: impl FnOnce(&mut DesktopWidgetInstance)) {
    let mut cfg = current_cfg();
    let Some(inst) = cfg.instances.iter_mut().find(|i| i.id == id) else {
        return;
    };
    f(inst);
    CFG.with(|cell| *cell.borrow_mut() = cfg.clone());
    // Suppress before the debounced write — otherwise the file monitor (or a
    // deferred reload) can rebuild from stale disk coords mid-save window.
    suppress_reloads_for(RELOAD_SUPPRESS_ON_EDIT);
    schedule_save(cfg);
}

fn schedule_save(cfg: DesktopWidgetsConfig) {
    if let Some(id) = SAVE_PENDING.with(|cell| cell.borrow_mut().take()) {
        id.remove();
    }
    let id = glib::timeout_add_local(Duration::from_millis(SAVE_DEBOUNCE_MS), move || {
        SAVE_PENDING.with(|cell| *cell.borrow_mut() = None);
        suppress_reloads_for(RELOAD_SUPPRESS_AFTER_SAVE);
        if let Err(err) = save_desktop_widgets_config(&cfg) {
            tracing::warn!(%err, "failed to persist desktop widget geometry");
        }
        glib::ControlFlow::Break
    });
    SAVE_PENDING.with(|cell| *cell.borrow_mut() = Some(id));
}

fn watch_config_file() {
    let path = desktop_widgets_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(
            path = %path.display(),
            "desktop-widgets.json file monitor unavailable"
        );
        return;
    };
    monitor.connect_changed(move |_, _, _, _| {
        // Atomic save is write(tmp)+rename → DELETE/CREATE flood. One debounced
        // reload is enough; chrome-only edits skip host teardown.
        schedule_reload();
    });
    FILE_MONITOR.with(|cell| *cell.borrow_mut() = Some(monitor));
}

fn retain_monitor(monitor: gio::FileMonitor) {
    AUX_MONITORS.with(|cell| cell.borrow_mut().push(monitor));
}

fn watch_menu_pins() {
    let path = metis_config::menu_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "menu.json file monitor unavailable");
        return;
    };
    monitor.connect_changed(move |_, _, _, _| {
        glib::timeout_add_local_once(Duration::from_millis(200), on_menu_pins_changed);
    });
    retain_monitor(monitor);
}

fn watch_theme_and_config() {
    let themes = crate::config::config_dir().join("themes");
    if let Err(err) = crate::config::ensure_config_dirs() {
        tracing::warn!(%err, "failed to ensure themes dir");
    }
    let themes_file = gio::File::for_path(&themes);
    match themes_file.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>) {
        Ok(monitor) => {
            monitor.connect_changed(move |_, _, _, _| {
                glib::timeout_add_local_once(Duration::from_millis(250), || {
                    let _ = crate::ui::theme::init_theme();
                });
            });
            retain_monitor(monitor);
        }
        _ => {
            tracing::warn!(path = %themes.display(), "themes dir monitor unavailable");
        }
    }

    let cfg_path = metis_config::app_config_path();
    let cfg_file = gio::File::for_path(&cfg_path);
    if let Ok(monitor) =
        cfg_file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    {
        monitor.connect_changed(move |_, _, _, _| {
            glib::timeout_add_local_once(Duration::from_millis(250), || {
                let _ = crate::ui::theme::init_theme();
            });
        });
        retain_monitor(monitor);
    }
}

fn watch_locale_file() {
    let path = metis_config::locale_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = gio::File::for_path(&path);
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    else {
        tracing::warn!(path = %path.display(), "locale.json file monitor unavailable");
        return;
    };
    monitor.connect_changed(move |_, _, _, _| {
        glib::timeout_add_local_once(Duration::from_millis(200), apply_locale_reload);
    });
    retain_monitor(monitor);
}

fn apply_locale_reload() {
    metis_i18n::reload();
    let dir = if metis_i18n::is_rtl() {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    };
    gtk::Widget::set_default_direction(dir);
    reload_for_locale();
}

fn watch_widgets_commands() {
    let path = metis_protocol::runtime_command_path_widgets();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let cmd = raw.trim();
            if !cmd.is_empty() {
                match metis_protocol::parse_runtime_command(
                    cmd,
                    metis_protocol::WIDGETS_RUNTIME_VERBS,
                ) {
                    Ok(parsed) => {
                        if !metis_protocol::try_admit_runtime_command_widgets_dispatch() {
                            tracing::warn!("widgets runtime command dispatch rate limited");
                        } else {
                            match parsed.verb {
                                "reload-desktop-widgets" => reload(),
                                "reload-theme" => {
                                    let _ = crate::ui::theme::init_theme();
                                }
                                "reload-locale" => apply_locale_reload(),
                                "reload-weather" => crate::services::weather::weather_refresh(),
                                other => {
                                    tracing::debug!(%other, "unhandled widgets runtime command")
                                }
                            }
                        }
                    }
                    Err(err) => tracing::warn!(%err, "rejected widgets runtime command"),
                }
                let _ = std::fs::remove_file(&path);
            }
        }
        glib::ControlFlow::Continue
    });
}

fn attach_weather() {
    let rx = crate::services::weather::spawn_weather_service();
    glib::timeout_add_local(Duration::from_millis(1000), move || {
        while let Ok(snapshot) = rx.try_recv() {
            crate::services::weather::remember_snapshot(&snapshot);
            on_weather_snapshot(&snapshot);
        }
        glib::ControlFlow::Continue
    });
}

fn watch_monitors() {
    use gtk::gio::prelude::ListModelExt;
    let Some(display) = gdk::Display::default() else {
        return;
    };
    display.monitors().connect_items_changed(move |_, _, _, _| {
        glib::timeout_add_local_once(Duration::from_millis(200), || {
            if interaction_blocks_reload() {
                return;
            }
            rebuild_hosts();
        });
    });
}

fn connected_monitors() -> Vec<gdk::Monitor> {
    use gtk::gio::prelude::ListModelExt;
    let Some(display) = gdk::Display::default() else {
        return Vec::new();
    };
    let list = display.monitors();
    let mut out = Vec::new();
    for i in 0..list.n_items() {
        if let Some(monitor) = list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()) {
            out.push(monitor);
        }
    }
    out
}

fn monitor_output_name(monitor: &gdk::Monitor) -> Option<String> {
    monitor
        .connector()
        .map(|c| c.to_string())
        .filter(|c| !c.is_empty())
}

fn sanitize_css_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn apply_card_chrome(
    style_class: &str,
    chrome: &metis_config::ResolvedDesktopWidgetChrome,
    text_color: &str,
    accent_color: &str,
) {
    let tokens = crate::ui::theme::active_tokens();
    let fill_rgb = if chrome.background_color.is_empty() {
        if tokens.mode.eq_ignore_ascii_case("light") {
            tokens.surface_raised_rgb()
        } else {
            tokens.surface_rgb()
        }
    } else {
        hex_to_rgb_triplet(&chrome.background_color).unwrap_or_else(|| tokens.surface_rgb())
    };
    let alpha = chrome.background_opacity.clamp(0.0, 1.0);
    let border_css = if chrome.border_width <= 0.0 {
        "border: none;".to_string()
    } else {
        let border_rgb = if chrome.border_color.is_empty() {
            tokens.text_rgb()
        } else {
            hex_to_rgb_triplet(&chrome.border_color).unwrap_or_else(|| tokens.text_rgb())
        };
        let border_alpha = if chrome.border_color.is_empty() {
            0.12
        } else {
            1.0
        };
        format!(
            "border: {:.2}px solid rgba({border_rgb}, {border_alpha:.3});",
            chrome.border_width
        )
    };

    let mut extra = String::new();
    if let Some(text_rgb) = hex_to_rgb_triplet(text_color) {
        extra.push_str(&format!(
            ".metis-dw-card.{style_class} label {{ color: rgb({text_rgb}); }}\
             .metis-dw-card.{style_class} .metis-dw-hint {{ color: rgba({text_rgb}, 0.72); }}\
             .metis-dw-card.{style_class} .metis-dw-title {{ color: rgb({text_rgb}); }}\
             .metis-dw-card.{style_class} image {{ color: rgb({text_rgb}); }}"
        ));
    }
    if let Some(accent_rgb) = hex_to_rgb_triplet(accent_color) {
        extra.push_str(&format!(
            "progressbar.metis-dw-progress.{style_class} progress,\
             .metis-dw-card.{style_class} progressbar.metis-dw-progress progress {{\
                 background-color: rgb({accent_rgb}); background-image: none; }}"
        ));
    }

    let css = format!(
        ".metis-dw-card.{style_class} {{ background-color: rgba({fill_rgb}, {alpha:.3}); {border_css} }}{extra}"
    );

    CARD_CHROME.with(|map| {
        let mut map = map.borrow_mut();
        let is_new = !map.contains_key(style_class);
        let provider = map
            .entry(style_class.to_string())
            .or_insert_with(gtk::CssProvider::new);
        provider.load_from_string(&css);
        if is_new && let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
}

fn hex_to_rgb_triplet(hex: &str) -> Option<String> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(format!("{r}, {g}, {b}"))
}
