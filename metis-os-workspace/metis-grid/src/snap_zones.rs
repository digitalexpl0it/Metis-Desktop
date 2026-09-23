use super::layout::{GridMetrics, PixelRect, pixel_to_grid_cell};
use super::model::{GridLayout, TileRect};

/// Distance from monitor edge to trigger a half/corner snap (macOS-style).
const EDGE_SNAP_PX: i32 = 48;

/// Pointer distance from the left / right / bottom usable-zone edges that
/// triggers a pixel-space snap. Being within this band of two edges at once
/// snaps to the matching quarter.
const PIXEL_SNAP_PX: i32 = 64;

/// The top edge (maximize) uses a much tighter band: the cursor must get right
/// up against the top so a normal drag toward the upper area doesn't prematurely
/// flip to a full-screen maximize.
const PIXEL_SNAP_TOP_PX: i32 = 16;

/// macOS / Windows-style edge snapping in *pixel* space, computed against the
/// `zone` the window may occupy (the usable area, already excluding the bar).
///
/// Given the pointer at (`x`, `y`), returns the *raw* target region (a fraction
/// of `zone`, with no inset) plus a short human label, or `None` when the
/// pointer isn't inside any snap band. The caller applies its own edge gaps and
/// window states (so the result matches the maximize look). Unlike
/// [`drop_target_for_tile`] this works directly in screen pixels, so callers
/// (the compositor's move grab) don't have to round-trip through grid cells and
/// it naturally respects the bar's exclusive zone.
pub fn pixel_snap_target(x: i32, y: i32, zone: PixelRect) -> Option<(PixelRect, &'static str)> {
    let right = zone.x + zone.width;
    let bottom = zone.y + zone.height;
    if zone.width <= 0 || zone.height <= 0 {
        return None;
    }
    if x < zone.x || y < zone.y || x > right || y > bottom {
        return None;
    }

    let near_left = x - zone.x < PIXEL_SNAP_PX;
    let near_right = right - x < PIXEL_SNAP_PX;
    let near_top = y - zone.y < PIXEL_SNAP_TOP_PX;
    let near_bottom = bottom - y < PIXEL_SNAP_PX;
    if !near_left && !near_right && !near_top && !near_bottom {
        return None;
    }

    let hw = zone.width / 2;
    let hh = zone.height / 2;
    let left = zone.x;
    let top = zone.y;

    // Corners (two edges at once) take priority over halves; the top edge maps to
    // a full-screen maximize, the other three edges to halves.
    let (rect, label) = match (near_top, near_bottom, near_left, near_right) {
        (true, _, true, _) => (
            PixelRect {
                x: left,
                y: top,
                width: hw,
                height: hh,
            },
            "Top-left",
        ),
        (true, _, _, true) => (
            PixelRect {
                x: left + hw,
                y: top,
                width: zone.width - hw,
                height: hh,
            },
            "Top-right",
        ),
        (_, true, true, _) => (
            PixelRect {
                x: left,
                y: top + hh,
                width: hw,
                height: zone.height - hh,
            },
            "Bottom-left",
        ),
        (_, true, _, true) => (
            PixelRect {
                x: left + hw,
                y: top + hh,
                width: zone.width - hw,
                height: zone.height - hh,
            },
            "Bottom-right",
        ),
        (true, _, _, _) => (
            PixelRect {
                x: left,
                y: top,
                width: zone.width,
                height: zone.height,
            },
            "Maximize",
        ),
        (_, true, _, _) => (
            PixelRect {
                x: left,
                y: top + hh,
                width: zone.width,
                height: zone.height - hh,
            },
            "Bottom half",
        ),
        (_, _, true, _) => (
            PixelRect {
                x: left,
                y: top,
                width: hw,
                height: zone.height,
            },
            "Left half",
        ),
        (_, _, _, true) => (
            PixelRect {
                x: left + hw,
                y: top,
                width: zone.width - hw,
                height: zone.height,
            },
            "Right half",
        ),
        _ => return None,
    };

    Some((rect, label))
}

/// Drop target while dragging `dragged_id` — edge snaps resize the tile; otherwise it keeps its size.
pub fn drop_target_for_tile(
    dragged_id: &str,
    x: i32,
    y: i32,
    layout: &GridLayout,
    metrics: &GridMetrics,
) -> TileRect {
    let Some(dragged) = layout.tiles.iter().find(|t| t.id == dragged_id) else {
        return snap_target_at_point(x, y, layout, metrics);
    };

    // Edge / corner snaps change the dragged tile to half- or quarter-screen presets.
    if let Some(snap) = edge_snap_zone(x, y, layout.columns, layout.rows, metrics) {
        return snap;
    }

    let (w, h) = (dragged.rect.w, dragged.rect.h);
    let (col, row) = pixel_to_grid_cell(x, y, metrics);
    let col = col
        .saturating_sub(w / 2)
        .min(layout.columns.saturating_sub(w));
    let row = row.saturating_sub(h / 2).min(layout.rows.saturating_sub(h));
    TileRect::new(col, row, w, h)
}

/// Pick a snap target for the pointer (used when the dragged tile id is unknown).
pub fn snap_target_at_point(
    x: i32,
    y: i32,
    layout: &GridLayout,
    metrics: &GridMetrics,
) -> TileRect {
    if let Some(edge) = edge_snap_zone(x, y, layout.columns, layout.rows, metrics) {
        return edge;
    }

    let (col, row) = pixel_to_grid_cell(x, y, metrics);
    TileRect::new(col, row, 1, 1)
}

fn edge_snap_zone(
    x: i32,
    y: i32,
    columns: u32,
    rows: u32,
    metrics: &GridMetrics,
) -> Option<TileRect> {
    let m = metrics.monitor;
    let rx = x - m.x;
    let ry = y - m.y;
    if rx < 0 || ry < 0 || rx > m.width || ry > m.height {
        return None;
    }

    let near_left = rx < EDGE_SNAP_PX;
    let near_right = rx > m.width - EDGE_SNAP_PX;
    let near_top = ry < EDGE_SNAP_PX;
    let near_bottom = ry > m.height - EDGE_SNAP_PX;

    if !near_left && !near_right && !near_top && !near_bottom {
        return None;
    }

    let half_w = (columns / 2).max(1);
    let half_h = (rows / 2).max(1);

    match (near_top, near_bottom, near_left, near_right) {
        (true, _, true, _) => Some(TileRect::new(0, 0, half_w, half_h)),
        (true, _, _, true) => Some(TileRect::new(
            half_w,
            0,
            columns.saturating_sub(half_w),
            half_h,
        )),
        (_, true, true, _) => Some(TileRect::new(
            0,
            half_h,
            half_w,
            rows.saturating_sub(half_h),
        )),
        (_, true, _, true) => Some(TileRect::new(
            half_w,
            half_h,
            columns.saturating_sub(half_w),
            rows.saturating_sub(half_h),
        )),
        (true, _, _, _) => Some(TileRect::new(0, 0, columns, half_h)),
        (_, true, _, _) => Some(TileRect::new(
            0,
            half_h,
            columns,
            rows.saturating_sub(half_h),
        )),
        (_, _, true, _) => Some(TileRect::new(0, 0, half_w, rows)),
        (_, _, _, true) => Some(TileRect::new(
            half_w,
            0,
            columns.saturating_sub(half_w),
            rows,
        )),
        _ => None,
    }
}

pub fn monitor_point_from_grid_local(
    grid_x: f64,
    grid_y: f64,
    monitor: super::layout::MonitorRect,
) -> (i32, i32) {
    (
        monitor.x + grid_x.round() as i32,
        monitor.y + grid_y.round() as i32,
    )
}

pub fn snap_label(zone: &TileRect, layout: &GridLayout) -> &'static str {
    if layout.tiles.iter().any(|t| t.rect == *zone) {
        return "Snap to tile";
    }
    if zone.col == 0 && zone.row == 0 && zone.w == layout.columns && zone.h == layout.rows {
        return "Full screen";
    }
    if zone.w == layout.columns && zone.h == layout.rows / 2 {
        return if zone.row == 0 {
            "Top half"
        } else {
            "Bottom half"
        };
    }
    if zone.h == layout.rows && zone.w == layout.columns / 2 {
        return if zone.col == 0 {
            "Left half"
        } else {
            "Right half"
        };
    }
    if zone.w == 1 && zone.h == 1 {
        return "Grid cell";
    }
    "Drop here"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MonitorRect;

    fn metrics() -> GridMetrics {
        GridMetrics {
            columns: 12,
            rows: 8,
            gutter: 8,
            monitor: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        }
    }

    #[test]
    fn edge_left_returns_left_half() {
        let layout = GridLayout::default();
        let zone = drop_target_for_tile("clock", 10, 540, &layout, &metrics());
        assert_eq!(zone, TileRect::new(0, 0, 6, 8));
    }

    #[test]
    fn edge_right_returns_right_half() {
        let layout = GridLayout::default();
        let zone = drop_target_for_tile("clock", 1910, 540, &layout, &metrics());
        assert_eq!(zone, TileRect::new(6, 0, 6, 8));
    }

    fn zone() -> PixelRect {
        // Usable area below a 40px bar.
        PixelRect {
            x: 0,
            y: 40,
            width: 1920,
            height: 1040,
        }
    }

    #[test]
    fn pixel_center_is_no_snap() {
        assert!(pixel_snap_target(960, 560, zone()).is_none());
    }

    #[test]
    fn pixel_left_edge_is_left_half() {
        let (rect, label) = pixel_snap_target(5, 560, zone()).unwrap();
        assert_eq!(label, "Left half");
        assert_eq!(
            rect,
            PixelRect {
                x: 0,
                y: 40,
                width: 960,
                height: 1040
            }
        );
    }

    #[test]
    fn pixel_top_edge_is_maximize() {
        let (rect, label) = pixel_snap_target(960, 44, zone()).unwrap();
        assert_eq!(label, "Maximize");
        assert_eq!(rect, zone());
    }

    #[test]
    fn pixel_top_left_corner_is_quarter() {
        let (rect, label) = pixel_snap_target(10, 50, zone()).unwrap();
        assert_eq!(label, "Top-left");
        assert_eq!(
            rect,
            PixelRect {
                x: 0,
                y: 40,
                width: 960,
                height: 520
            }
        );
    }

    #[test]
    fn pixel_outside_zone_is_none() {
        assert!(pixel_snap_target(-5, 560, zone()).is_none());
    }
}
