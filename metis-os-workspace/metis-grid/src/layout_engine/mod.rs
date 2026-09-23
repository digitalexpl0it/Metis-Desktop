//! Grid reflow engine — algorithm reference: [react-grid-layout/core](https://github.com/react-grid-layout/react-grid-layout/tree/master/src/core).

mod collision;
mod r#move;
mod sort;

use crate::model::{GridLayout, ReflowError, TileRect};

pub use r#move::{layout_fits, move_element};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactType {
    Null,
    Vertical,
    Horizontal,
    Wrap,
}

#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    pub cols: u32,
    pub rows: u32,
    pub compact_type: CompactType,
    pub allow_overlap: bool,
    pub prevent_collision: bool,
}

impl EngineConfig {
    pub fn for_layout(layout: &GridLayout) -> Self {
        Self {
            cols: layout.columns,
            rows: layout.rows,
            compact_type: CompactType::Null,
            allow_overlap: false,
            prevent_collision: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LayoutItem {
    pub id: String,
    pub col: u32,
    pub row: u32,
    pub w: u32,
    pub h: u32,
    pub pinned: bool,
    pub moved: bool,
}

impl LayoutItem {
    pub fn right(&self) -> u32 {
        self.col + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.row + self.h
    }

    pub fn to_rect(&self) -> TileRect {
        TileRect::new(self.col, self.row, self.w, self.h)
    }
}

fn layout_to_items(layout: &GridLayout) -> Vec<LayoutItem> {
    layout
        .tiles
        .iter()
        .map(|t| LayoutItem {
            id: t.id.clone(),
            col: t.rect.col,
            row: t.rect.row,
            w: t.rect.w,
            h: t.rect.h,
            pinned: t.pinned,
            moved: false,
        })
        .collect()
}

fn apply_items_to_layout(layout: &mut GridLayout, items: &[LayoutItem]) {
    for item in items {
        if let Some(tile) = layout.tile_mut(&item.id) {
            tile.rect = item.to_rect();
        }
    }
}

/// Move or resize a tile with RGL push cascade; half-screen snaps use complement packing.
pub fn move_item(layout: &mut GridLayout, id: &str, target: TileRect) -> Result<(), ReflowError> {
    if !layout.tiles.iter().any(|t| t.id == id) {
        return Err(ReflowError::NotFound(id.to_string()));
    }
    if layout.tiles.iter().any(|t| t.id == id && t.pinned) {
        return Err(ReflowError::NoSpace {
            col: target.col,
            row: target.row,
        });
    }
    if target.right() > layout.columns || target.bottom() > layout.rows {
        return Err(ReflowError::NoSpace {
            col: target.col,
            row: target.row,
        });
    }

    if layout
        .tiles
        .iter()
        .any(|t| t.id != id && t.pinned && t.rect.intersects(&target))
    {
        return Err(ReflowError::NoSpace {
            col: target.col,
            row: target.row,
        });
    }

    let config = EngineConfig::for_layout(layout);

    if can_place(layout, id, &target) {
        if let Some(tile) = layout.tile_mut(id) {
            tile.rect = target;
        }
        return Ok(());
    }

    if let Some(complement) = complement_region(&target, layout.columns, layout.rows) {
        push_neighbors_into_region(layout, id, &target, complement)?;
        if let Some(tile) = layout.tile_mut(id) {
            tile.rect = target;
        }
        validate_layout(layout)?;
        return Ok(());
    }

    let mut items = layout_to_items(layout);
    if let Some(item) = items.iter_mut().find(|i| i.id == id) {
        item.col = target.col;
        item.row = target.row;
        item.w = target.w;
        item.h = target.h;
        item.moved = true;
    }

    if !move_element(
        &mut items,
        id,
        Some(target.col),
        Some(target.row),
        true,
        &config,
        0,
    ) || !layout_fits(&items, config.cols, config.rows)
    {
        push_neighbors_relocate(layout, id, &target)?;
    } else {
        apply_items_to_layout(layout, &items);
    }

    if let Some(tile) = layout.tile_mut(id) {
        tile.rect = target;
    }

    validate_layout(layout)
}

/// Non-mutating preview for live drag reflow.
pub fn preview_move(
    layout: &GridLayout,
    id: &str,
    target: TileRect,
) -> Result<GridLayout, ReflowError> {
    let mut preview = layout.clone();
    move_item(&mut preview, id, target)?;
    Ok(preview)
}

/// Resize a tile (Phase 5.5 constraints applied when min/max fields are set).
pub fn resize_item(layout: &mut GridLayout, id: &str, target: TileRect) -> Result<(), ReflowError> {
    move_item(layout, id, target)
}

pub fn validate_layout(layout: &GridLayout) -> Result<(), ReflowError> {
    for (i, a) in layout.tiles.iter().enumerate() {
        if a.rect.right() > layout.columns || a.rect.bottom() > layout.rows {
            return Err(ReflowError::NoSpace {
                col: a.rect.col,
                row: a.rect.row,
            });
        }
        for b in layout.tiles.iter().skip(i + 1) {
            if a.rect.intersects(&b.rect) {
                return Err(ReflowError::NoSpace {
                    col: a.rect.col,
                    row: a.rect.row,
                });
            }
        }
    }
    Ok(())
}

fn union_rect(a: &TileRect, b: &TileRect) -> TileRect {
    let col = a.col.min(b.col);
    let row = a.row.min(b.row);
    let right = a.right().max(b.right());
    let bottom = a.bottom().max(b.bottom());
    TileRect::new(
        col,
        row,
        right.saturating_sub(col),
        bottom.saturating_sub(row),
    )
}

pub fn repair_layout(layout: &mut GridLayout) {
    let max_attempts = layout.tiles.len().saturating_mul(layout.tiles.len()).max(1);
    for _ in 0..max_attempts {
        if validate_layout(layout).is_ok() {
            return;
        }

        let Some((move_id, avoid)) = first_overlap(layout) else {
            return;
        };

        let Some(tile) = layout.tiles.iter().find(|t| t.id == move_id) else {
            return;
        };
        if tile.pinned {
            return;
        }
        let rect = tile.rect;

        let Some(new_rect) = find_relocate_rect(layout, &move_id, rect, &avoid) else {
            return;
        };
        if let Some(t) = layout.tile_mut(&move_id) {
            t.rect = new_rect;
        }
    }
}

fn first_overlap(layout: &GridLayout) -> Option<(String, TileRect)> {
    for i in 0..layout.tiles.len() {
        for j in (i + 1)..layout.tiles.len() {
            let a = &layout.tiles[i];
            let b = &layout.tiles[j];
            if !a.rect.intersects(&b.rect) {
                continue;
            }
            let avoid = union_rect(&a.rect, &b.rect);
            if !a.pinned {
                return Some((a.id.clone(), avoid));
            }
            if !b.pinned {
                return Some((b.id.clone(), avoid));
            }
        }
    }
    None
}

fn layout_needs_reset(layout: &GridLayout) -> bool {
    if validate_layout(layout).is_err() {
        return true;
    }
    layout.tiles.iter().any(|t| {
        matches!(t.kind, crate::model::TileKind::Widget { .. }) && (t.rect.w < 2 || t.rect.h < 2)
    }) || !layout.app_tiling_region_viable()
}

fn reset_to_default_preserving_apps(layout: &mut GridLayout) {
    use crate::model::{GridLayout, GridTile, TileKind};

    let apps: Vec<GridTile> = layout
        .tiles
        .iter()
        .filter(|t| matches!(t.kind, TileKind::App { .. }))
        .cloned()
        .collect();
    let columns = layout.columns;
    let rows = layout.rows;
    *layout = GridLayout::default();
    layout.columns = columns;
    layout.rows = rows;

    for mut app in apps {
        let mut placed = layout.default_app_tile_rect();
        if !can_place(layout, &app.id, &placed)
            && let Some(found) =
                find_relocate_rect(layout, &app.id, app.rect, &TileRect::new(0, 0, 1, 1))
        {
            placed = found;
        }
        app.rect = placed;
        if !layout.tiles.iter().any(|t| t.id == app.id) {
            layout.tiles.push(app);
        }
    }
}

/// Repair overlaps; reset degenerate saved layouts. Strips legacy on-desktop
/// widget tiles — those modules live in the edge bar until desk widgets ship.
pub fn sanitize_layout(layout: &mut GridLayout) {
    layout
        .tiles
        .retain(|t| !matches!(t.kind, crate::model::TileKind::Widget { .. }));
    repair_layout(layout);
    if layout_needs_reset(layout) {
        reset_to_default_preserving_apps(layout);
        repair_layout(layout);
    }
}

fn can_place(layout: &GridLayout, ignore_id: &str, rect: &TileRect) -> bool {
    if rect.right() > layout.columns || rect.bottom() > layout.rows {
        return false;
    }
    layout
        .tiles
        .iter()
        .all(|t| t.id == ignore_id || !t.rect.intersects(rect))
}

fn complement_region(target: &TileRect, columns: u32, rows: u32) -> Option<TileRect> {
    let half_w = (columns / 2).max(1);
    let half_h = (rows / 2).max(1);

    if target.col == 0 && target.w >= half_w && target.h == rows {
        return Some(TileRect::new(
            half_w,
            0,
            columns.saturating_sub(half_w),
            rows,
        ));
    }
    if target.right() == columns && target.w >= half_w && target.h == rows {
        return Some(TileRect::new(0, 0, half_w, rows));
    }
    if target.row == 0 && target.h >= half_h && target.w == columns {
        return Some(TileRect::new(
            0,
            half_h,
            columns,
            rows.saturating_sub(half_h),
        ));
    }
    if target.bottom() == rows && target.h >= half_h && target.w == columns {
        return Some(TileRect::new(0, 0, columns, half_h));
    }
    None
}

fn region_contains(region: &TileRect, inner: &TileRect) -> bool {
    inner.col >= region.col
        && inner.row >= region.row
        && inner.right() <= region.right()
        && inner.bottom() <= region.bottom()
}

fn push_neighbors_into_region(
    layout: &mut GridLayout,
    id: &str,
    target: &TileRect,
    region: TileRect,
) -> Result<(), ReflowError> {
    let mut pending: Vec<String> = layout
        .tiles
        .iter()
        .filter(|t| t.id != id && !t.pinned && t.rect.intersects(target))
        .map(|t| t.id.clone())
        .collect();

    let max_passes = layout.tiles.len().saturating_mul(4).max(1);
    let mut pass = 0usize;
    while let Some(bid) = pending.pop() {
        pass += 1;
        if pass > max_passes {
            return Err(ReflowError::NoSpace {
                col: target.col,
                row: target.row,
            });
        }

        let tile_rect = layout
            .tiles
            .iter()
            .find(|t| t.id == bid)
            .map(|t| t.rect)
            .ok_or_else(|| ReflowError::NotFound(bid.clone()))?;

        if !tile_rect.intersects(target) {
            continue;
        }

        let Some(new_rect) = find_rect_in_region(layout, &bid, tile_rect, region, target) else {
            return Err(ReflowError::NoSpace {
                col: target.col,
                row: target.row,
            });
        };

        if let Some(t) = layout.tile_mut(&bid) {
            t.rect = new_rect;
        }

        for other in layout.tiles.iter() {
            if other.id == id || other.id == bid || other.pinned {
                continue;
            }
            if (other.rect.intersects(target) || other.rect.intersects(&new_rect))
                && !pending.iter().any(|p| p == &other.id)
            {
                pending.push(other.id.clone());
            }
        }
    }
    Ok(())
}

fn push_neighbors_relocate(
    layout: &mut GridLayout,
    id: &str,
    target: &TileRect,
) -> Result<(), ReflowError> {
    let mut pending: Vec<String> = layout
        .tiles
        .iter()
        .filter(|t| t.id != id && !t.pinned && t.rect.intersects(target))
        .map(|t| t.id.clone())
        .collect();

    let max_passes = layout.tiles.len().saturating_mul(4).max(1);
    let mut pass = 0usize;
    while let Some(bid) = pending.pop() {
        pass += 1;
        if pass > max_passes {
            return Err(ReflowError::NoSpace {
                col: target.col,
                row: target.row,
            });
        }

        let tile_rect = layout
            .tiles
            .iter()
            .find(|t| t.id == bid)
            .map(|t| t.rect)
            .ok_or_else(|| ReflowError::NotFound(bid.clone()))?;

        if !tile_rect.intersects(target) {
            continue;
        }

        let Some(new_rect) = find_relocate_rect(layout, &bid, tile_rect, target) else {
            return Err(ReflowError::NoSpace {
                col: target.col,
                row: target.row,
            });
        };

        if let Some(t) = layout.tile_mut(&bid) {
            t.rect = new_rect;
        }

        for other in layout.tiles.iter() {
            if other.id == id || other.id == bid || other.pinned {
                continue;
            }
            if (other.rect.intersects(target) || other.rect.intersects(&new_rect))
                && !pending.iter().any(|p| p == &other.id)
            {
                pending.push(other.id.clone());
            }
        }
    }
    Ok(())
}

fn find_rect_in_region(
    layout: &GridLayout,
    tile_id: &str,
    prefer: TileRect,
    region: TileRect,
    avoid: &TileRect,
) -> Option<TileRect> {
    for h in (1..=prefer.h.min(region.h)).rev() {
        for w in (1..=prefer.w.min(region.w)).rev() {
            let mut positions = Vec::new();
            for row in region.row..=region.bottom().saturating_sub(h) {
                for col in region.col..=region.right().saturating_sub(w) {
                    positions.push(TileRect::new(col, row, w, h));
                }
            }
            positions.sort_by_key(|r| {
                (
                    r.col.abs_diff(prefer.col) + r.row.abs_diff(prefer.row),
                    r.col,
                    r.row,
                )
            });
            for candidate in positions {
                if candidate.intersects(avoid) {
                    continue;
                }
                if !region_contains(&region, &candidate) {
                    continue;
                }
                if can_place(layout, tile_id, &candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn find_relocate_rect(
    layout: &GridLayout,
    tile_id: &str,
    rect: TileRect,
    avoid: &TileRect,
) -> Option<TileRect> {
    for h in (1..=rect.h).rev() {
        for w in (1..=rect.w).rev() {
            if let Some(found) = find_rect_with_size(layout, tile_id, w, h, avoid, rect) {
                return Some(found);
            }
        }
    }
    None
}

fn find_rect_with_size(
    layout: &GridLayout,
    tile_id: &str,
    w: u32,
    h: u32,
    avoid: &TileRect,
    prefer: TileRect,
) -> Option<TileRect> {
    if w == 0 || h == 0 || w > layout.columns || h > layout.rows {
        return None;
    }

    let mut positions = Vec::new();
    for row in 0..=layout.rows.saturating_sub(h) {
        for col in 0..=layout.columns.saturating_sub(w) {
            positions.push(TileRect::new(col, row, w, h));
        }
    }
    positions.sort_by_key(|r| {
        (
            r.col.abs_diff(prefer.col) + r.row.abs_diff(prefer.row),
            r.col,
            r.row,
        )
    });

    for candidate in positions {
        if candidate.intersects(avoid) {
            continue;
        }
        if can_place(layout, tile_id, &candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GridTile, TileKind};

    fn widget(id: &str, rect: TileRect) -> GridTile {
        GridTile {
            id: id.into(),
            rect,
            kind: TileKind::Widget { module: id.into() },
            glow: "cool".into(),
            pinned: false,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        }
    }

    fn app(id: &str, rect: TileRect, pinned: bool) -> GridTile {
        GridTile {
            id: id.into(),
            rect,
            kind: TileKind::App {
                window_id: Some(1),
                class: Some("foot".into()),
            },
            glow: "cool".into(),
            pinned,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        }
    }

    fn no_overlap(layout: &GridLayout) {
        for (i, a) in layout.tiles.iter().enumerate() {
            for b in layout.tiles.iter().skip(i + 1) {
                assert!(!a.rect.intersects(&b.rect), "{} overlaps {}", a.id, b.id);
            }
        }
    }

    #[test]
    fn sanitize_layout_resets_degenerate_saved_desk() {
        let raw = r#"{
            "columns": 12,
            "rows": 8,
            "tiles": [
                {"id": "weather", "rect": {"col": 0, "row": 4, "w": 3, "h": 4}, "kind": {"type": "widget", "module": "weather"}, "glow": "warm"},
                {"id": "rss", "rect": {"col": 2, "row": 0, "w": 10, "h": 4}, "kind": {"type": "widget", "module": "rss"}, "glow": "violet"},
                {"id": "settings", "rect": {"col": 1, "row": 1, "w": 1, "h": 3}, "kind": {"type": "widget", "module": "settings"}, "glow": "cool"},
                {"id": "app-1", "rect": {"col": 9, "row": 4, "w": 3, "h": 3}, "kind": {"type": "app", "window_id": 1, "class": "foot"}, "glow": "cool"}
            ]
        }"#;
        let mut layout: GridLayout = serde_json::from_str(raw).expect("parse desk.json fixture");
        sanitize_layout(&mut layout);
        validate_layout(&layout).expect("sanitized layout must be valid");
        assert!(
            layout.tiles.iter().any(|t| t.id == "app-1"),
            "app tile preserved after sanitize"
        );
        assert!(
            !layout
                .tiles
                .iter()
                .any(|t| matches!(t.kind, TileKind::Widget { .. })),
            "legacy widget tiles stripped"
        );
        no_overlap(&layout);
    }

    #[test]
    fn repair_layout_fixes_in_place_overlap() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("a", TileRect::new(0, 0, 4, 4)),
                widget("b", TileRect::new(2, 2, 4, 4)),
            ],
        };
        repair_layout(&mut layout);
        validate_layout(&layout).expect("repaired layout must be collision-free");
    }

    #[test]
    fn preview_move_does_not_mutate_committed_layout() {
        let layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("a", TileRect::new(0, 0, 3, 2)),
                widget("b", TileRect::new(3, 0, 3, 2)),
            ],
        };
        let original = layout.clone();
        let _ = preview_move(&layout, "a", TileRect::new(6, 0, 3, 2)).expect("preview");
        assert_eq!(layout, original);
    }

    #[test]
    fn push_cascade_moves_multiple_neighbors() {
        let mut layout = GridLayout {
            columns: 6,
            rows: 4,
            tiles: vec![
                widget("a", TileRect::new(0, 0, 2, 2)),
                widget("b", TileRect::new(2, 0, 2, 2)),
                widget("c", TileRect::new(4, 0, 2, 2)),
            ],
        };
        move_item(&mut layout, "a", TileRect::new(2, 0, 2, 2)).expect("move");
        no_overlap(&layout);
        let a = layout.tiles.iter().find(|t| t.id == "a").unwrap();
        assert_eq!(a.rect, TileRect::new(2, 0, 2, 2));
    }

    #[test]
    fn pinned_tile_blocks_push() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("weather", TileRect::new(0, 0, 3, 2)),
                app("foot", TileRect::new(3, 0, 4, 4), true),
            ],
        };
        let err = move_item(&mut layout, "weather", TileRect::new(3, 0, 4, 4));
        assert!(err.is_err());
        assert!(layout.tiles.iter().find(|t| t.id == "foot").unwrap().pinned);
    }

    #[test]
    fn drag_reroutes_around_pinned_app() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("weather", TileRect::new(0, 0, 3, 2)),
                app("foot", TileRect::new(8, 0, 4, 4), true),
                widget("rss", TileRect::new(0, 4, 12, 4)),
            ],
        };
        move_item(&mut layout, "weather", TileRect::new(4, 0, 3, 2)).expect("move around pinned");
        no_overlap(&layout);
        let foot = layout.tiles.iter().find(|t| t.id == "foot").unwrap();
        assert_eq!(foot.rect, TileRect::new(8, 0, 4, 4));
    }

    #[test]
    fn pinned_tile_immovable() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![app("foot", TileRect::new(0, 0, 4, 4), true)],
        };
        let err = move_item(&mut layout, "foot", TileRect::new(4, 0, 4, 4));
        assert!(err.is_err());
    }
}
