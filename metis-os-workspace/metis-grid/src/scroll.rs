//! Horizontally scrolling workspace layout (niri / PaperWM style).
//!
//! App windows form a horizontal strip of [`ScrollColumn`]s; each column holds a
//! vertical stack of windows. The viewport ([`ScrollState::scroll_x`]) shifts the
//! strip so the focused column stays visible. This module is pure arrangement
//! logic — it stores window ids and computes pixel frames; the compositor maps
//! the actual surfaces and draws decorations from those frames.

use serde::{Deserialize, Serialize};

use crate::layout::PixelRect;

/// Column widths are a fraction of the usable viewport width so they survive
/// output-size / bar changes. Mouse-resizing sets an arbitrary fraction within
/// [`MIN_COL_FRAC`]..=[`FULL_COL_FRAC`]; the keyboard cycle snaps to half/full.
pub const MIN_COL_FRAC: f32 = 0.15;
pub const HALF_COL_FRAC: f32 = 0.5;
pub const FULL_COL_FRAC: f32 = 1.0;
/// Width a brand-new column opens at (half the viewport, niri/paneru style).
pub const DEFAULT_COL_FRAC: f32 = HALF_COL_FRAC;

/// A single column in the scroll strip: a vertical stack of windows (top to
/// bottom) plus the column's width preset and which window in the stack is focused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollColumn {
    pub windows: Vec<u32>,
    #[serde(default)]
    pub focus_row: usize,
    /// Column width as a fraction of the usable viewport width.
    #[serde(default = "default_col_frac")]
    pub width_frac: f32,
}

fn default_col_frac() -> f32 {
    DEFAULT_COL_FRAC
}

impl ScrollColumn {
    fn single(window: u32) -> Self {
        Self {
            windows: vec![window],
            focus_row: 0,
            width_frac: DEFAULT_COL_FRAC,
        }
    }

    fn clamp_focus(&mut self) {
        if self.windows.is_empty() {
            self.focus_row = 0;
        } else if self.focus_row >= self.windows.len() {
            self.focus_row = self.windows.len() - 1;
        }
    }
}

/// Rest threshold (px/s) below which kinetic coast stops.
pub const SCROLL_VELOCITY_REST_EPS: f32 = 12.0;
/// Cap on kinetic velocity so a single wild flick cannot runaway.
const SCROLL_VELOCITY_MAX: f32 = 14_000.0;
/// Scale from per-event pixel delta → velocity impulse (treat ~1/60s sample).
const SCROLL_IMPULSE_SCALE: f32 = 55.0;
/// Soft rubber-band overshoot limit while bouncing (px).
const SCROLL_MAX_OVERSHOOT_PX: f32 = 96.0;
/// Spring strength pulling an overshot viewport back inside bounds (1/s²).
const SCROLL_BOUNCE_SPRING: f32 = 22.0;

/// The scroll-strip arrangement for one workspace.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrollState {
    pub columns: Vec<ScrollColumn>,
    #[serde(default)]
    pub focus_col: usize,
    /// Current viewport offset (px) into the strip; animated toward
    /// [`ScrollState::scroll_x_target`].
    #[serde(default)]
    pub scroll_x: i32,
    /// Target viewport offset; updated by focus changes and clamped to the strip.
    #[serde(default)]
    pub scroll_x_target: i32,
    /// Kinetic coast velocity (px/s). Runtime-only — not persisted in session.
    #[serde(skip)]
    pub velocity_px_s: f32,
}

impl ScrollState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.iter().all(|c| c.windows.is_empty())
    }

    pub fn contains(&self, window: u32) -> bool {
        self.columns.iter().any(|c| c.windows.contains(&window))
    }

    /// The currently focused window, if any.
    pub fn focused_window(&self) -> Option<u32> {
        let col = self.columns.get(self.focus_col)?;
        col.windows.get(col.focus_row).copied()
    }

    fn clamp_focus(&mut self) {
        if self.columns.is_empty() {
            self.focus_col = 0;
            return;
        }
        if self.focus_col >= self.columns.len() {
            self.focus_col = self.columns.len() - 1;
        }
        if let Some(col) = self.columns.get_mut(self.focus_col) {
            col.clamp_focus();
        }
    }

    /// Append a window as a brand-new column immediately to the right of the
    /// focused column, and focus it. No-op if the window is already present.
    pub fn insert_window_after_focus(&mut self, window: u32) {
        if self.contains(window) {
            return;
        }
        if self.columns.is_empty() {
            self.columns.push(ScrollColumn::single(window));
            self.focus_col = 0;
        } else {
            let at = (self.focus_col + 1).min(self.columns.len());
            self.columns.insert(at, ScrollColumn::single(window));
            self.focus_col = at;
        }
    }

    /// Remove a window from the strip, dropping its column if it becomes empty.
    pub fn remove_window(&mut self, window: u32) {
        let mut removed = false;
        for col in &mut self.columns {
            if let Some(pos) = col.windows.iter().position(|&w| w == window) {
                col.windows.remove(pos);
                col.clamp_focus();
                removed = true;
                break;
            }
        }
        if removed {
            self.columns.retain(|c| !c.windows.is_empty());
            self.clamp_focus();
        }
    }

    pub fn focus_left(&mut self) {
        if self.focus_col > 0 {
            self.focus_col -= 1;
        }
    }

    pub fn focus_right(&mut self) {
        if self.focus_col + 1 < self.columns.len() {
            self.focus_col += 1;
        }
    }

    pub fn focus_up(&mut self) {
        if let Some(col) = self.columns.get_mut(self.focus_col)
            && col.focus_row > 0
        {
            col.focus_row -= 1;
        }
    }

    pub fn focus_down(&mut self) {
        if let Some(col) = self.columns.get_mut(self.focus_col)
            && col.focus_row + 1 < col.windows.len()
        {
            col.focus_row += 1;
        }
    }

    pub fn move_column_left(&mut self) {
        if self.focus_col > 0 {
            self.columns.swap(self.focus_col, self.focus_col - 1);
            self.focus_col -= 1;
        }
    }

    pub fn move_column_right(&mut self) {
        if self.focus_col + 1 < self.columns.len() {
            self.columns.swap(self.focus_col, self.focus_col + 1);
            self.focus_col += 1;
        }
    }

    pub fn move_window_up(&mut self) {
        if let Some(col) = self.columns.get_mut(self.focus_col)
            && col.focus_row > 0
        {
            col.windows.swap(col.focus_row, col.focus_row - 1);
            col.focus_row -= 1;
        }
    }

    pub fn move_window_down(&mut self) {
        if let Some(col) = self.columns.get_mut(self.focus_col)
            && col.focus_row + 1 < col.windows.len()
        {
            col.windows.swap(col.focus_row, col.focus_row + 1);
            col.focus_row += 1;
        }
    }

    /// Pull the focused window into the bottom of the previous column's stack.
    /// No-op when the focused column is the leftmost.
    pub fn consume_into_prev(&mut self) {
        if self.focus_col == 0 || self.focus_col >= self.columns.len() {
            return;
        }
        let Some(window) = self.focused_window() else {
            return;
        };
        let col = &mut self.columns[self.focus_col];
        col.windows.remove(col.focus_row);
        col.clamp_focus();
        let from = self.focus_col;
        let prev = from - 1;
        self.columns[prev].windows.push(window);
        self.columns[prev].focus_row = self.columns[prev].windows.len() - 1;
        if self.columns[from].windows.is_empty() {
            self.columns.remove(from);
        }
        self.focus_col = prev;
        self.clamp_focus();
    }

    /// Pop the focused window out of its stack into a new column to the right.
    /// No-op when the focused column already holds just that one window.
    pub fn expel_to_new_column(&mut self) {
        let Some(col) = self.columns.get_mut(self.focus_col) else {
            return;
        };
        if col.windows.len() <= 1 {
            return;
        }
        let window = col.windows.remove(col.focus_row);
        col.clamp_focus();
        let at = self.focus_col + 1;
        self.columns.insert(at, ScrollColumn::single(window));
        self.focus_col = at;
    }

    /// Keyboard width toggle: snap the focused column to full, or back to half
    /// once it's already (near) full — works from any mouse-set custom width.
    pub fn cycle_focus_width(&mut self) {
        if let Some(col) = self.columns.get_mut(self.focus_col) {
            col.width_frac = if col.width_frac >= FULL_COL_FRAC - 0.01 {
                HALF_COL_FRAC
            } else {
                FULL_COL_FRAC
            };
        }
    }

    /// Index of the column holding `window`, if any.
    pub fn column_index_of(&self, window: u32) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.windows.contains(&window))
    }

    /// Current pixel width of column `ci` for `zone`.
    pub fn column_width_px(&self, ci: usize, zone: PixelRect) -> i32 {
        self.columns
            .get(ci)
            .map(|c| col_width_px(c.width_frac, zone.width))
            .unwrap_or(0)
    }

    /// Set the pixel width of the column holding `window`, clamped to sane
    /// fractions of `zone`. Columns to its right reflow automatically in
    /// [`Self::layout`]. Returns true when a column was found and updated.
    pub fn set_column_width_px_for(&mut self, window: u32, width_px: i32, zone: PixelRect) -> bool {
        let Some(ci) = self.column_index_of(window) else {
            return false;
        };
        let denom = zone.width.max(1) as f32;
        self.columns[ci].width_frac = (width_px as f32 / denom).clamp(MIN_COL_FRAC, FULL_COL_FRAC);
        true
    }

    /// Focus the column/row that holds `window`, if present.
    pub fn focus_window(&mut self, window: u32) {
        for (ci, col) in self.columns.iter_mut().enumerate() {
            if let Some(ri) = col.windows.iter().position(|&w| w == window) {
                self.focus_col = ci;
                col.focus_row = ri;
                return;
            }
        }
    }

    /// Compute the full (titlebar-inclusive) pixel frame for every window, laid
    /// out left to right with the animated [`Self::scroll_x`] viewport offset.
    /// `zone` is the bar-excluded usable area in global logical coordinates.
    pub fn layout(&self, zone: PixelRect, gutter: i32) -> Vec<(u32, PixelRect)> {
        self.layout_at_scroll(zone, gutter, self.scroll_x)
    }

    /// Same as [`Self::layout`] but pinned to [`Self::scroll_x_target`]. The
    /// compositor maps client surfaces here and applies a render-time X nudge
    /// (`scroll_x_target - scroll_x`) while the viewport eases, avoiding
    /// per-frame Wayland configure storms.
    pub fn layout_placed(&self, zone: PixelRect, gutter: i32) -> Vec<(u32, PixelRect)> {
        self.layout_at_scroll(zone, gutter, self.scroll_x_target)
    }

    fn layout_at_scroll(
        &self,
        zone: PixelRect,
        gutter: i32,
        scroll_x: i32,
    ) -> Vec<(u32, PixelRect)> {
        let mut out = Vec::new();
        let mut cursor = zone.x - scroll_x;
        for col in &self.columns {
            let w = col_width_px(col.width_frac, zone.width);
            let n = col.windows.len();
            if n == 0 {
                cursor += w + gutter;
                continue;
            }
            let total_gut = gutter * (n as i32 - 1);
            let available = (zone.height - total_gut).max(n as i32);
            let base = available / n as i32;
            let rem = available % n as i32;
            let mut y = zone.y;
            for (i, &window) in col.windows.iter().enumerate() {
                let cell_h = base + if (i as i32) < rem { 1 } else { 0 };
                out.push((
                    window,
                    PixelRect {
                        x: cursor,
                        y,
                        width: w,
                        height: cell_h.max(1),
                    },
                ));
                y += cell_h + gutter;
            }
            cursor += w + gutter;
        }
        out
    }

    /// Total width of the horizontal strip in pixels (columns + gutters).
    pub fn strip_width(&self, zone: PixelRect, gutter: i32) -> i32 {
        if self.columns.is_empty() {
            return 0;
        }
        let mut w = 0;
        for (i, col) in self.columns.iter().enumerate() {
            if i > 0 {
                w += gutter;
            }
            w += col_width_px(col.width_frac, zone.width);
        }
        w
    }

    /// Maximum scroll offset that keeps the strip's right edge aligned with the
    /// viewport when the strip is wider than the zone.
    pub fn max_scroll_x(&self, zone: PixelRect, gutter: i32) -> i32 {
        (self.strip_width(zone, gutter) - zone.width).max(0)
    }

    /// Advance the animated viewport toward [`Self::scroll_x_target`]. Returns
    /// `true` when `scroll_x` moved this step.
    ///
    /// Focus-ease only — skip while [`Self::has_momentum`] is true so kinetic
    /// coast owns the viewport.
    pub fn advance_scroll_animation(&mut self, dt_secs: f32) -> bool {
        if self.has_momentum() || self.scroll_x == self.scroll_x_target {
            return false;
        }
        const SPEED_PX_PER_SEC: f32 = 3200.0;
        let step = (SPEED_PX_PER_SEC * dt_secs).round() as i32;
        let step = step.max(1);
        let delta = self.scroll_x_target - self.scroll_x;
        let before = self.scroll_x;
        if delta.abs() <= step {
            self.scroll_x = self.scroll_x_target;
        } else {
            self.scroll_x += step * delta.signum();
        }
        self.scroll_x != before
    }

    /// Snap the viewport to the target immediately (no animation).
    pub fn snap_scroll(&mut self) {
        self.scroll_x = self.scroll_x_target;
        self.clear_velocity();
    }

    /// True while kinetic coast (or overshoot spring) is still driving the viewport.
    pub fn has_momentum(&self) -> bool {
        self.velocity_px_s.abs() > SCROLL_VELOCITY_REST_EPS
    }

    /// Drop kinetic velocity (focus ease / keybind / animations-off path).
    pub fn clear_velocity(&mut self) {
        self.velocity_px_s = 0.0;
    }

    /// Apply a pointer-axis pixel delta to the strip and accumulate coast velocity.
    ///
    /// Keeps `scroll_x_target` glued to `scroll_x` so focus-ease does not fight
    /// the flick. When `coast` is false (animations off), clamps hard and clears
    /// velocity.
    pub fn apply_scroll_impulse(&mut self, dx_px: f32, zone: PixelRect, gutter: i32, coast: bool) {
        if !dx_px.is_finite() || dx_px.abs() < f32::EPSILON {
            return;
        }
        let max = self.max_scroll_x(zone, gutter);
        if !coast {
            self.clear_velocity();
            let next = (self.scroll_x as f32 + dx_px).round() as i32;
            self.scroll_x = next.clamp(0, max);
            self.scroll_x_target = self.scroll_x;
            return;
        }

        self.velocity_px_s = (self.velocity_px_s + dx_px * SCROLL_IMPULSE_SCALE)
            .clamp(-SCROLL_VELOCITY_MAX, SCROLL_VELOCITY_MAX);

        let next = self.scroll_x as f32 + dx_px;
        // Soft rubber during active flick; hard clamp only past overshoot.
        let lo = -SCROLL_MAX_OVERSHOOT_PX;
        let hi = max as f32 + SCROLL_MAX_OVERSHOOT_PX;
        self.scroll_x = next.clamp(lo, hi).round() as i32;
        self.scroll_x_target = self.scroll_x.clamp(0, max);
    }

    /// Integrate kinetic velocity with exponential friction (and optional bounce).
    ///
    /// Returns `true` when the viewport moved this step. When `bounce` is false,
    /// hard-clamps at strip ends and kills outward velocity.
    pub fn advance_scroll_momentum(
        &mut self,
        dt_secs: f32,
        friction: f32,
        bounce: bool,
        zone: PixelRect,
        gutter: i32,
    ) -> bool {
        let dt = dt_secs.clamp(0.0, 0.05);
        if dt <= 0.0 {
            return false;
        }
        let max = self.max_scroll_x(zone, gutter);
        let max_f = max as f32;
        let before = self.scroll_x;

        // Spring pull when overshot (even if velocity has nearly died).
        let x = self.scroll_x as f32;
        if bounce {
            if x < 0.0 {
                self.velocity_px_s += -x * SCROLL_BOUNCE_SPRING * dt;
            } else if x > max_f {
                self.velocity_px_s += (max_f - x) * SCROLL_BOUNCE_SPRING * dt;
            }
        }

        if !self.has_momentum() && self.scroll_x >= 0 && self.scroll_x <= max {
            // Snap target to visual when coast ends inside bounds.
            self.scroll_x_target = self.scroll_x;
            self.clear_velocity();
            return false;
        }

        let friction = friction.max(0.0);
        self.velocity_px_s *= (-friction * dt).exp();

        let mut next = x + self.velocity_px_s * dt;
        if bounce {
            let lo = -SCROLL_MAX_OVERSHOOT_PX;
            let hi = max_f + SCROLL_MAX_OVERSHOOT_PX;
            if next < lo {
                next = lo;
                self.velocity_px_s = self.velocity_px_s.max(0.0);
            } else if next > hi {
                next = hi;
                self.velocity_px_s = self.velocity_px_s.min(0.0);
            }
            // Settle when nearly inside and slow.
            if next >= 0.0 && next <= max_f && self.velocity_px_s.abs() <= SCROLL_VELOCITY_REST_EPS
            {
                next = next.clamp(0.0, max_f);
                self.clear_velocity();
            }
        } else {
            if next < 0.0 {
                next = 0.0;
                self.velocity_px_s = 0.0;
            } else if next > max_f {
                next = max_f;
                self.velocity_px_s = 0.0;
            } else if self.velocity_px_s.abs() <= SCROLL_VELOCITY_REST_EPS {
                self.clear_velocity();
            }
        }

        self.scroll_x = next.round() as i32;
        // Clients stay mapped at the clamped target; visual uses scroll_x (nudge).
        self.scroll_x_target = self.scroll_x.clamp(0, max);
        self.scroll_x != before || self.has_momentum()
    }

    /// Set the scroll target, clamped to the strip width for `zone`.
    pub fn set_scroll_target(&mut self, target: i32, zone: PixelRect, gutter: i32) {
        let max = self.max_scroll_x(zone, gutter);
        self.scroll_x_target = target.clamp(0, max);
        if self.scroll_x > max {
            self.scroll_x = max;
        }
        if self.scroll_x < 0 {
            self.scroll_x = 0;
        }
    }

    /// The `scroll_x` value that brings the focused column fully into view (left
    /// aligned when it's wider than the viewport). Callers store the result in
    /// [`ScrollState::scroll_x_target`] (and optionally animate `scroll_x`).
    pub fn desired_scroll_x(&self, zone: PixelRect, gutter: i32) -> i32 {
        if self.columns.is_empty() {
            return 0;
        }
        let mut start = 0;
        for col in self.columns.iter().take(self.focus_col) {
            start += col_width_px(col.width_frac, zone.width) + gutter;
        }
        let width = self
            .columns
            .get(self.focus_col)
            .map(|c| col_width_px(c.width_frac, zone.width))
            .unwrap_or(zone.width);

        let mut scroll = self.scroll_x;
        if width >= zone.width || start < scroll {
            scroll = start;
        } else if start + width > scroll + zone.width {
            scroll = start + width - zone.width;
        }
        scroll.clamp(0, self.max_scroll_x(zone, gutter))
    }
}

fn col_width_px(width_frac: f32, zone_width: i32) -> i32 {
    let frac = width_frac.clamp(MIN_COL_FRAC, FULL_COL_FRAC);
    ((zone_width as f32) * frac).round().max(1.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: PixelRect = PixelRect {
        x: 0,
        y: 0,
        width: 1200,
        height: 800,
    };

    #[test]
    fn insert_creates_columns_right_of_focus() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        s.insert_window_after_focus(3);
        assert_eq!(s.columns.len(), 3);
        assert_eq!(s.focused_window(), Some(3));
        // Inserting in the middle: focus column 0 then insert.
        s.focus_col = 0;
        s.insert_window_after_focus(4);
        assert_eq!(s.columns[1].windows, vec![4]);
        assert_eq!(s.focused_window(), Some(4));
    }

    #[test]
    fn insert_is_idempotent() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(1);
        assert_eq!(s.columns.len(), 1);
    }

    #[test]
    fn remove_drops_empty_column() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        s.remove_window(2);
        assert_eq!(s.columns.len(), 1);
        assert_eq!(s.focused_window(), Some(1));
    }

    #[test]
    fn consume_and_expel_roundtrip() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        // focus is col 1 (window 2); consume into prev stacks it under window 1.
        s.consume_into_prev();
        assert_eq!(s.columns.len(), 1);
        assert_eq!(s.columns[0].windows, vec![1, 2]);
        assert_eq!(s.focused_window(), Some(2));
        // expel pops window 2 back into its own column.
        s.expel_to_new_column();
        assert_eq!(s.columns.len(), 2);
        assert_eq!(s.columns[1].windows, vec![2]);
        assert_eq!(s.focused_window(), Some(2));
    }

    #[test]
    fn cycle_width_changes_focus_column() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        assert_eq!(s.columns[0].width_frac, HALF_COL_FRAC);
        s.cycle_focus_width();
        assert_eq!(s.columns[0].width_frac, FULL_COL_FRAC);
        s.cycle_focus_width();
        assert_eq!(s.columns[0].width_frac, HALF_COL_FRAC);
    }

    #[test]
    fn layout_places_columns_left_to_right() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        let frames = s.layout(ZONE, 10);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, 1);
        assert_eq!(frames[0].1.x, 0);
        // Half of 1200 = 600, + gutter 10.
        assert_eq!(frames[0].1.width, 600);
        assert_eq!(frames[1].1.x, 610);
    }

    #[test]
    fn layout_stacks_within_column() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.consume_into_prev(); // single column, can't consume — still col 0
        s.insert_window_after_focus(2);
        s.consume_into_prev(); // stack 2 under 1 in col 0
        let frames = s.layout(ZONE, 10);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1.y, 0);
        // (800 - 10) / 2 = 395, next y = 395 + 10 = 405.
        assert_eq!(frames[0].1.height, 395);
        assert_eq!(frames[1].1.y, 405);
    }

    #[test]
    fn desired_scroll_brings_focus_into_view() {
        let mut s = ScrollState::new();
        for id in 1..=4 {
            s.insert_window_after_focus(id);
            s.columns[s.focus_col].width_frac = HALF_COL_FRAC;
        }
        let scroll = s.desired_scroll_x(ZONE, 10);
        assert!(scroll > 0);
    }

    #[test]
    fn max_scroll_clamps_desired() {
        let mut s = ScrollState::new();
        for id in 1..=4 {
            s.insert_window_after_focus(id);
            s.columns[s.focus_col].width_frac = HALF_COL_FRAC;
        }
        let max = s.max_scroll_x(ZONE, 10);
        assert!(max > 0);
        let desired = s.desired_scroll_x(ZONE, 10);
        assert!(desired <= max);
    }

    #[test]
    fn mouse_resize_sets_column_width_and_reflows() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        // Grow column 0 (window 1) to ~800px of the 1200px zone.
        assert!(s.set_column_width_px_for(1, 800, ZONE));
        let frames = s.layout(ZONE, 10);
        assert_eq!(frames[0].1.width, 800);
        // The right neighbour slides over by the new width + gutter.
        assert_eq!(frames[1].1.x, 810);
    }

    #[test]
    fn mouse_resize_clamps_to_min_and_full() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        // Absurdly wide clamps to full viewport.
        s.set_column_width_px_for(1, 999_999, ZONE);
        assert_eq!(s.layout(ZONE, 10)[0].1.width, ZONE.width);
        // Absurdly narrow clamps to the minimum fraction.
        s.set_column_width_px_for(1, 1, ZONE);
        let expect_min = (ZONE.width as f32 * MIN_COL_FRAC).round() as i32;
        assert_eq!(s.layout(ZONE, 10)[0].1.width, expect_min);
    }

    #[test]
    fn vertical_stack_distributes_remainder() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        s.consume_into_prev();
        s.insert_window_after_focus(3);
        s.consume_into_prev();
        let frames = s.layout(ZONE, 10);
        assert_eq!(frames.len(), 3);
        let total_h: i32 = frames.iter().map(|(_, r)| r.height).sum::<i32>() + 10 * 2;
        assert_eq!(total_h, ZONE.height);
    }

    #[test]
    fn layout_placed_uses_target_offset() {
        let mut s = ScrollState::new();
        s.insert_window_after_focus(1);
        s.insert_window_after_focus(2);
        s.scroll_x = 0;
        s.scroll_x_target = 200;
        let visual = s.layout(ZONE, 10);
        let placed = s.layout_placed(ZONE, 10);
        assert_eq!(visual[0].1.x, 0);
        assert_eq!(placed[0].1.x, -200);
    }

    fn wide_strip() -> ScrollState {
        let mut s = ScrollState::new();
        for id in 1..=5 {
            s.insert_window_after_focus(id);
            s.columns[s.focus_col].width_frac = HALF_COL_FRAC;
        }
        s.scroll_x = 400;
        s.scroll_x_target = 400;
        s
    }

    #[test]
    fn impulse_moves_viewport_and_builds_velocity() {
        let mut s = wide_strip();
        let before = s.scroll_x;
        s.apply_scroll_impulse(40.0, ZONE, 10, true);
        assert!(s.scroll_x > before);
        assert!(s.has_momentum());
        assert_eq!(
            s.scroll_x_target,
            s.scroll_x.clamp(0, s.max_scroll_x(ZONE, 10))
        );
    }

    #[test]
    fn impulse_without_coast_clamps_and_clears_velocity() {
        let mut s = wide_strip();
        s.velocity_px_s = 500.0;
        s.apply_scroll_impulse(-10_000.0, ZONE, 10, false);
        assert_eq!(s.scroll_x, 0);
        assert_eq!(s.scroll_x_target, 0);
        assert!(!s.has_momentum());
    }

    #[test]
    fn momentum_friction_decays_to_rest() {
        let mut s = wide_strip();
        s.apply_scroll_impulse(80.0, ZONE, 10, true);
        assert!(s.has_momentum());
        for _ in 0..240 {
            s.advance_scroll_momentum(1.0 / 60.0, 4.5, false, ZONE, 10);
        }
        assert!(!s.has_momentum());
        assert!(s.scroll_x >= 0);
        assert!(s.scroll_x <= s.max_scroll_x(ZONE, 10));
    }

    #[test]
    fn momentum_hard_clamp_kills_outward_velocity() {
        let mut s = wide_strip();
        s.scroll_x = 0;
        s.scroll_x_target = 0;
        s.velocity_px_s = -3_000.0;
        s.advance_scroll_momentum(1.0 / 60.0, 4.5, false, ZONE, 10);
        assert_eq!(s.scroll_x, 0);
        assert!(!s.has_momentum());
    }

    #[test]
    fn momentum_bounce_returns_inside_bounds() {
        let mut s = wide_strip();
        s.scroll_x = -40;
        s.scroll_x_target = 0;
        s.velocity_px_s = -200.0;
        for _ in 0..180 {
            s.advance_scroll_momentum(1.0 / 60.0, 4.5, true, ZONE, 10);
        }
        assert!(s.scroll_x >= 0);
        assert!(s.scroll_x <= s.max_scroll_x(ZONE, 10));
        assert!(!s.has_momentum());
    }
}
