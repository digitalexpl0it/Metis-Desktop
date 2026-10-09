//! Edge-bar auto-hide reveal, hide scheduling, and visual transforms.

use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

use crate::config::{BarConfig, BarPosition};

use super::geometry::{bar_body_thickness, bar_cross_thickness};
use super::{BARS, BarHandle, dropdown};

pub(crate) fn apply_bar_visibility(handle: &BarHandle) {
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
pub(crate) fn on_bar_edge_hover() {
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
pub(crate) fn on_bar_edge_leave() {
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

pub(crate) fn on_bar_pointer_enter(window: &gtk::Window) {
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

pub(crate) fn on_bar_pointer_leave(window: &gtk::Window) {
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

pub(crate) fn schedule_auto_hide(handle: &BarHandle) {
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
pub(crate) fn rearm_auto_hide_after_ui_dismiss() {
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
pub(crate) fn force_auto_hide_all() {
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
    provider.load_from_string(&css);
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
