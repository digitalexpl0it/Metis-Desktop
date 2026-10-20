//! Window motion effects (minimize genie, workspace slide, shared animation toggle).

use metis_grid::PixelRect;
use smithay::utils::{Logical, Point};

/// Whether compositor window animations are enabled (`bar.json`), suppressed in
/// VM compatibility graphics mode.
pub fn animations_enabled() -> bool {
    if metis_config::session_graphics_compatibility() {
        return false;
    }
    metis_config::load_bar_config().window_animations
}

pub const MINIMIZE_GENIE_SECS: f32 = 0.48;

/// Horizontal workspace slide duration (ease-out).
pub const WORKSPACE_SLIDE_SECS: f32 = 0.28;

/// In-flight minimize genie toward the edge bar.
pub struct MinimizeGenieFx {
    pub started: std::time::Instant,
    /// Visual bounds at animation start (client + SSD chrome when present).
    pub anchor: PixelRect,
    pub target: Point<i32, Logical>,
}

impl MinimizeGenieFx {
    pub fn progress(&self) -> f32 {
        (self.started.elapsed().as_secs_f32() / MINIMIZE_GENIE_SECS).clamp(0.0, 1.0)
    }

    pub fn finished(&self) -> bool {
        self.started.elapsed().as_secs_f32() >= MINIMIZE_GENIE_SECS
    }

    /// Genie-style shrink + travel toward the bar. Returns `(clip rect, alpha)`.
    pub fn frame(&self) -> (PixelRect, f32) {
        let t = ease_in_genie(self.progress());
        let alpha = (1.0 - t * t).clamp(0.0, 1.0);
        let squeeze = t * t;
        let w = lerp_i32(self.anchor.width.max(1), 6, squeeze).max(1);
        let h = lerp_i32(self.anchor.height.max(1), 4, t).max(1);
        let cx = lerp_i32(self.anchor.x + self.anchor.width / 2, self.target.x, t);
        let cy = lerp_i32(self.anchor.y + self.anchor.height / 2, self.target.y, t);
        (
            PixelRect {
                x: cx - w / 2,
                y: cy - h / 2,
                width: w,
                height: h,
            },
            alpha,
        )
    }
}

/// Dual-mapped horizontal workspace transition on one output.
///
/// `sign`: `+1` = next (content moves left; new enters from the right);
/// `-1` = previous (content moves right; new enters from the left).
pub struct WorkspaceSlideFx {
    pub started: std::time::Instant,
    pub sign: i32,
    pub width_px: i32,
    pub leaving: Vec<u32>,
    pub entering: Vec<u32>,
}

impl WorkspaceSlideFx {
    pub fn progress(&self) -> f32 {
        (self.started.elapsed().as_secs_f32() / WORKSPACE_SLIDE_SECS).clamp(0.0, 1.0)
    }

    pub fn finished(&self) -> bool {
        self.started.elapsed().as_secs_f32() >= WORKSPACE_SLIDE_SECS
    }

    pub fn eased(&self) -> f32 {
        ease_out_cubic(self.progress())
    }

    /// Render-time X offset (logical px) for a window in this slide, or `None`.
    pub fn dx_for(&self, id: u32) -> Option<i32> {
        let t = self.eased();
        let w = self.width_px.max(1);
        let sign = if self.sign >= 0 { 1 } else { -1 };
        if self.leaving.contains(&id) {
            Some((-sign as f32 * t * w as f32).round() as i32)
        } else if self.entering.contains(&id) {
            Some((sign as f32 * (1.0 - t) * w as f32).round() as i32)
        } else {
            None
        }
    }

    pub fn is_leaving(&self, id: u32) -> bool {
        self.leaving.contains(&id)
    }

    pub fn contains(&self, id: u32) -> bool {
        self.leaving.contains(&id) || self.entering.contains(&id)
    }
}

fn ease_in_genie(t: f32) -> f32 {
    // Fast start, strong pull into the bar at the end (genie “suck”).
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn lerp_i32(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genie_finishes_at_one() {
        let fx = MinimizeGenieFx {
            started: std::time::Instant::now()
                - std::time::Duration::from_secs_f32(MINIMIZE_GENIE_SECS + 0.01),
            anchor: PixelRect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            target: Point::from((400, 20)),
        };
        assert!(fx.finished());
        let (rect, alpha) = fx.frame();
        assert!(alpha <= 0.05);
        assert!(rect.width <= 8);
    }

    #[test]
    fn workspace_slide_next_offsets() {
        let fx = WorkspaceSlideFx {
            started: std::time::Instant::now(),
            sign: 1,
            width_px: 1000,
            leaving: vec![1],
            entering: vec![2],
        };
        // t≈0: leaving on-screen, entering off to the right.
        assert!(fx.dx_for(1).unwrap().abs() <= 2);
        assert!((fx.dx_for(2).unwrap() - 1000).abs() <= 2);
        assert!(fx.dx_for(3).is_none());
    }

    #[test]
    fn workspace_slide_finished_at_end() {
        let fx = WorkspaceSlideFx {
            started: std::time::Instant::now()
                - std::time::Duration::from_secs_f32(WORKSPACE_SLIDE_SECS + 0.01),
            sign: -1,
            width_px: 800,
            leaving: vec![1],
            entering: vec![2],
        };
        assert!(fx.finished());
        let t = fx.eased();
        assert!((t - 1.0).abs() < f32::EPSILON);
        // Prev: leaving exits right, entering arrives from left → both at rest.
        assert_eq!(fx.dx_for(1), Some(800));
        assert_eq!(fx.dx_for(2), Some(0));
    }
}
