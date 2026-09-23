use std::path::Path;

use crate::layout_engine;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRect {
    pub col: u32,
    pub row: u32,
    pub w: u32,
    pub h: u32,
}

impl TileRect {
    pub fn new(col: u32, row: u32, w: u32, h: u32) -> Self {
        Self { col, row, w, h }
    }

    pub fn right(&self) -> u32 {
        self.col + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.row + self.h
    }

    pub fn intersects(&self, other: &TileRect) -> bool {
        self.col < other.right()
            && self.right() > other.col
            && self.row < other.bottom()
            && self.bottom() > other.row
    }
}

/// Which arrangement model a workspace uses for its app windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    /// Regular floating desktop — windows open centered and stay free until the
    /// user toggles grid or scroll mode.
    #[default]
    Free,
    /// Auto-tiling grid below desk widgets.
    Grid,
    /// A horizontally scrolling strip of columns (niri / PaperWM style).
    Scroll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TileKind {
    Widget {
        module: String,
    },
    App {
        window_id: Option<u32>,
        class: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridTile {
    pub id: String,
    pub rect: TileRect,
    pub kind: TileKind,
    #[serde(default)]
    pub glow: String,
    /// Pinned tiles stay fixed during reflow (RGL `static`).
    #[serde(default, alias = "static")]
    pub pinned: bool,
    #[serde(default)]
    pub min_w: Option<u32>,
    #[serde(default)]
    pub max_w: Option<u32>,
    #[serde(default)]
    pub min_h: Option<u32>,
    #[serde(default)]
    pub max_h: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridLayout {
    pub columns: u32,
    pub rows: u32,
    pub tiles: Vec<GridTile>,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            columns: 12,
            rows: 8,
            tiles: default_desk_tiles(),
        }
    }
}

fn default_desk_tiles() -> Vec<GridTile> {
    // Desk widgets (clock, weather, RSS, etc.) live in the edge bar for now.
    // On-desktop widget tiles will be added here when that UI ships.
    Vec::new()
}

#[derive(Debug, Error)]
pub enum ReflowError {
    #[error("tile {0} not found")]
    NotFound(String),
    #[error("no space for tile at col={col} row={row}")]
    NoSpace { col: u32, row: u32 },
}

impl GridLayout {
    pub fn load_from_path(path: &Path) -> Self {
        let layout = if path.exists() {
            if let Ok(text) = std::fs::read_to_string(path) {
                serde_json::from_str(&text).unwrap_or_default()
            } else {
                Self::default()
            }
        } else {
            Self::default()
        };
        let mut layout = layout;
        let before = layout.clone();
        layout_engine::sanitize_layout(&mut layout);
        if layout != before {
            let _ = layout.save_to_path(path);
        }
        layout
    }

    /// Full grid band for app auto-tiling (the compositor grid already sits below
    /// the edge bar). Legacy widget tiles in `desk.json` do not shrink this region.
    pub fn app_tiling_region(&self) -> TileRect {
        TileRect::new(0, 0, self.columns, self.rows)
    }

    /// Whether the computed app band has enough vertical space to tile windows.
    pub fn app_tiling_region_viable(&self) -> bool {
        self.rows >= 3
    }

    /// Default slot for a newly opened app window (bottom of the app tiling band).
    pub fn default_app_tile_rect(&self) -> TileRect {
        let region = self.app_tiling_region();
        let h = region.h.clamp(2, 4);
        let w = region.w.clamp(4, 6);
        TileRect::new(region.col, region.row + region.h.saturating_sub(h), w, h)
    }

    pub fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn tile_mut(&mut self, id: &str) -> Option<&mut GridTile> {
        self.tiles.iter_mut().find(|t| t.id == id)
    }

    pub fn resize_and_move(&mut self, id: &str, target: TileRect) -> Result<(), ReflowError> {
        layout_engine::move_item(self, id, clamp_target(self, id, target))
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<(), ReflowError> {
        let tile = self
            .tiles
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| ReflowError::NotFound(id.to_string()))?;
        tile.pinned = pinned;
        Ok(())
    }

    pub fn shift_column_boundary(&mut self, boundary: u32, delta: i32) -> Result<(), ReflowError> {
        if delta == 0 || boundary == 0 || boundary >= self.columns {
            return Ok(());
        }
        let delta = delta.clamp(
            -(boundary as i32 - 1),
            self.columns as i32 - boundary as i32,
        );
        if delta == 0 {
            return Ok(());
        }

        let left_ids: Vec<String> = self
            .tiles
            .iter()
            .filter(|t| !t.pinned && t.rect.right() == boundary)
            .map(|t| t.id.clone())
            .collect();
        let right_ids: Vec<String> = self
            .tiles
            .iter()
            .filter(|t| !t.pinned && t.rect.col == boundary)
            .map(|t| t.id.clone())
            .collect();

        for id in left_ids {
            if let Some(tile) = self.tile_mut(&id) {
                tile.rect.w = (tile.rect.w as i32 + delta).max(1) as u32;
            }
        }
        for id in right_ids {
            if let Some(tile) = self.tile_mut(&id) {
                tile.rect.col = (tile.rect.col as i32 + delta).max(0) as u32;
                tile.rect.w = (tile.rect.w as i32 - delta).max(1) as u32;
            }
        }

        for tile in &self.tiles {
            if tile.rect.right() > self.columns {
                return Err(ReflowError::NoSpace {
                    col: tile.rect.col,
                    row: tile.rect.row,
                });
            }
        }
        Ok(())
    }

    pub fn shift_row_boundary(&mut self, boundary: u32, delta: i32) -> Result<(), ReflowError> {
        if delta == 0 || boundary == 0 || boundary >= self.rows {
            return Ok(());
        }
        let delta = delta.clamp(-(boundary as i32 - 1), self.rows as i32 - boundary as i32);
        if delta == 0 {
            return Ok(());
        }

        let top_ids: Vec<String> = self
            .tiles
            .iter()
            .filter(|t| !t.pinned && t.rect.bottom() == boundary)
            .map(|t| t.id.clone())
            .collect();
        let bottom_ids: Vec<String> = self
            .tiles
            .iter()
            .filter(|t| !t.pinned && t.rect.row == boundary)
            .map(|t| t.id.clone())
            .collect();

        for id in top_ids {
            if let Some(tile) = self.tile_mut(&id) {
                tile.rect.h = (tile.rect.h as i32 + delta).max(1) as u32;
            }
        }
        for id in bottom_ids {
            if let Some(tile) = self.tile_mut(&id) {
                tile.rect.row = (tile.rect.row as i32 + delta).max(0) as u32;
                tile.rect.h = (tile.rect.h as i32 - delta).max(1) as u32;
            }
        }

        for tile in &self.tiles {
            if tile.rect.bottom() > self.rows {
                return Err(ReflowError::NoSpace {
                    col: tile.rect.col,
                    row: tile.rect.row,
                });
            }
        }
        Ok(())
    }
}

fn clamp_target(layout: &GridLayout, id: &str, mut target: TileRect) -> TileRect {
    let Some(tile) = layout.tiles.iter().find(|t| t.id == id) else {
        return target;
    };
    let min_w = tile.min_w.unwrap_or(1);
    let max_w = tile.max_w.unwrap_or(layout.columns);
    let min_h = tile.min_h.unwrap_or(1);
    let max_h = tile.max_h.unwrap_or(layout.rows);
    target.w = target.w.clamp(min_w, max_w);
    target.h = target.h.clamp(min_h, max_h);
    target.col = target.col.min(layout.columns.saturating_sub(target.w));
    target.row = target.row.min(layout.rows.saturating_sub(target.h));
    target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TileKind, TilePreset, apply_preset};

    fn widget(id: &str, rect: TileRect, glow: &str) -> GridTile {
        GridTile {
            id: id.into(),
            rect,
            kind: TileKind::Widget { module: id.into() },
            glow: glow.into(),
            pinned: false,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        }
    }

    fn app(id: &str, rect: TileRect) -> GridTile {
        GridTile {
            id: id.into(),
            rect,
            kind: TileKind::App {
                window_id: Some(1),
                class: Some("foot".into()),
            },
            glow: "cool".into(),
            pinned: false,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        }
    }

    #[test]
    fn move_tile_to_free_cell() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![app("app-1", TileRect::new(0, 4, 3, 4))],
        };
        layout
            .resize_and_move("app-1", TileRect::new(9, 4, 3, 4))
            .expect("move");
        let tile = layout
            .tiles
            .iter()
            .find(|t| t.id == "app-1")
            .expect("app-1");
        assert_eq!(tile.rect.col, 9);
        assert_eq!(tile.rect.row, 4);
    }

    fn layouts_do_not_overlap(layout: &GridLayout) {
        for (i, a) in layout.tiles.iter().enumerate() {
            for b in layout.tiles.iter().skip(i + 1) {
                assert!(
                    !a.rect.intersects(&b.rect),
                    "{} ({:?}) overlaps {} ({:?})",
                    a.id,
                    a.rect,
                    b.id,
                    b.rect
                );
            }
        }
    }

    #[test]
    fn half_bottom_reflows_neighbors_like_user_desk() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("weather", TileRect::new(3, 0, 3, 2), "warm"),
                widget("rss", TileRect::new(0, 4, 12, 4), "violet"),
                widget("settings", TileRect::new(0, 2, 6, 2), "cool"),
            ],
        };

        apply_preset(&mut layout, "settings", TilePreset::HalfBottom).expect("reflow");

        let settings = layout
            .tiles
            .iter()
            .find(|t| t.id == "settings")
            .expect("settings");
        assert_eq!(settings.rect, TileRect::new(0, 4, 12, 4));
        layouts_do_not_overlap(&layout);
    }

    #[test]
    fn half_left_packs_neighbors_into_right_half() {
        let mut layout = GridLayout {
            columns: 12,
            rows: 8,
            tiles: vec![
                widget("settings", TileRect::new(0, 0, 4, 4), "cool"),
                widget("weather", TileRect::new(4, 0, 4, 2), "warm"),
                app("foot", TileRect::new(8, 0, 4, 4)),
                widget("rss", TileRect::new(0, 4, 12, 4), "violet"),
            ],
        };

        let target = TileRect::new(0, 0, 6, 8);
        layout
            .resize_and_move("settings", target)
            .expect("half-left reflow");

        let settings = layout.tiles.iter().find(|t| t.id == "settings").unwrap();
        assert_eq!(settings.rect, target);

        for tile in &layout.tiles {
            if tile.id == "settings" {
                continue;
            }
            assert!(
                tile.rect.col >= 6,
                "{} should pack into right half, got {:?}",
                tile.id,
                tile.rect
            );
        }
        layouts_do_not_overlap(&layout);
    }
}
