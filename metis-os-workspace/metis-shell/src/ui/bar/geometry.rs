//! Bar monitor targeting, layer-shell geometry, and pill layout.

use std::cell::Cell;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::config::{BarConfig, BarDisplays, BarPosition};

use super::{BAR_POSITION, BARS, DASH_ATTACH_INSET, dropdown};

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

pub(crate) fn bar_matches_output(bar_output: Option<&str>, event_output: &str) -> bool {
    match bar_output {
        Some(name) => name == event_output,
        None => true,
    }
}

/// The compositor output name (e.g. `metis-0`) backing a GDK monitor. Under the
/// nested Metis session GDK exposes the compositor's `wl_output` name via the
/// monitor connector.
pub(crate) fn monitor_output_name(monitor: &gtk::gdk::Monitor) -> Option<String> {
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
pub(crate) fn target_monitors(cfg: &BarConfig) -> Vec<Option<gtk::gdk::Monitor>> {
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
pub(crate) fn connected_monitors() -> Vec<gtk::gdk::Monitor> {
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

/// Layer-shell width/height request for the edge-bar window on `config.position`.
pub(crate) fn layer_window_size(config: &BarConfig) -> (i32, i32) {
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
pub(crate) fn bar_body_thickness(config: &BarConfig) -> i32 {
    config.height as i32 + metis_config::bar::bar_layer_shadow_pad(config)
}

/// Visible cross-axis size of the bar pill (height when horizontal, width when
/// vertical). Kept equal so left/right bars match the top/bottom strip thickness.
pub(crate) fn bar_cross_thickness(config: &BarConfig) -> i32 {
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

/// Configure the bar's surface widget tree (outer box, column, pill) for the
/// current position: orientation, expansion, alignment (so the pill sits flush
/// against the anchored edge), size requests, and the vertical-bar CSS classes.
/// Shared by the initial build and the live `rebuild_bar` path so switching
/// between horizontal and vertical layouts at runtime re-sizes correctly.
pub(crate) fn configure_surface(
    outer: &gtk::Box,
    column: &gtk::Box,
    pill: &gtk::Box,
    config: &BarConfig,
) {
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
pub(crate) fn edge_halign(position: BarPosition) -> gtk::Align {
    match position {
        BarPosition::Right => gtk::Align::End,
        BarPosition::Left => gtk::Align::Start,
        // Top/bottom bars fill the full monitor width.
        BarPosition::Top | BarPosition::Bottom => gtk::Align::Fill,
    }
}

/// Vertical alignment of the bar strip within its layer surface.
pub(crate) fn edge_valign(position: BarPosition) -> gtk::Align {
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

pub(crate) fn orientation_for(config: &BarConfig) -> gtk::Orientation {
    match config.position {
        BarPosition::Top | BarPosition::Bottom => gtk::Orientation::Horizontal,
        BarPosition::Left | BarPosition::Right => gtk::Orientation::Vertical,
    }
}

pub(crate) fn apply_layer_geometry(window: &gtk::Window, config: &BarConfig) {
    if !window.is_layer_window() {
        window.init_layer_shell();
    }
    window.set_layer(Layer::Top);
    window.set_namespace(Some("metis-bar"));
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
