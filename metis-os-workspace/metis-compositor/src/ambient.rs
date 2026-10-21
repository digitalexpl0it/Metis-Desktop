//! Wallpaper-sampled ambient tint for frosted shell chrome (Phase 21 §D).
//!
//! Samples retained CPU wallpaper pixels (no GPU readback), emits
//! [`metis_protocol::CompositorEvent::AmbientTint`] when the colour or
//! `bar.json` kill-switch changes. Replays the last sample when a new
//! event-bus subscriber connects (shell often starts after the first compose).

use std::time::{Duration, Instant};

use metis_protocol::CompositorEvent;

use crate::state::MetisState;

const SAMPLE_EDGE: u32 = 32;

/// Tracks last emit so we do not spam the event bus every frame.
pub struct AmbientTintRuntime {
    enabled: bool,
    last_check: Instant,
    pub(crate) last_gen: u64,
    pub(crate) last_rgb: Option<(u8, u8, u8)>,
    last_lum: f32,
    last_enabled_emit: bool,
    /// Replay when the shell (re)subscribes after wallpaper already composed.
    last_subscriber_count: usize,
}

impl Default for AmbientTintRuntime {
    fn default() -> Self {
        Self {
            enabled: metis_config::load_bar_config().ambient_tint,
            last_check: Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(Instant::now),
            last_gen: 0,
            last_rgb: None,
            last_lum: 0.0,
            last_enabled_emit: true,
            last_subscriber_count: 0,
        }
    }
}

impl MetisState {
    /// Throttled `bar.json` re-read + sample when wallpaper pixels change.
    /// Call from the render path after [`crate::wallpaper::Wallpaper::poll_decode`].
    pub fn maybe_refresh_ambient_tint(&mut self) {
        // Shell often connects after the first wallpaper compose — replay then.
        let subs = self.event_bus.subscriber_count();
        if subs > self.ambient.last_subscriber_count {
            self.ambient.last_subscriber_count = subs;
            self.replay_ambient_tint();
        } else if subs < self.ambient.last_subscriber_count {
            self.ambient.last_subscriber_count = subs;
        }

        let force_cfg = self.ambient.last_check.elapsed() >= Duration::from_secs(1);
        if force_cfg {
            self.ambient.last_check = Instant::now();
            let enabled = metis_config::load_bar_config().ambient_tint;
            if enabled != self.ambient.enabled {
                self.ambient.enabled = enabled;
                if !enabled {
                    self.emit_ambient_disabled();
                    return;
                }
                // Re-sample immediately when turned back on.
                self.ambient.last_gen = 0;
                self.ambient.last_rgb = None;
            }
        }

        if !self.ambient.enabled {
            return;
        }

        let pixels_gen = self.wallpaper.cpu_pixels_gen();
        if pixels_gen == self.ambient.last_gen && self.ambient.last_rgb.is_some() {
            return;
        }

        let Some((r, g, b, lum)) = sample_wallpaper_ambient(&self.wallpaper) else {
            // Decode pending / solid not ready — keep prior tint; do not clear.
            return;
        };

        if self.ambient.last_rgb == Some((r, g, b)) && self.ambient.last_enabled_emit {
            self.ambient.last_gen = pixels_gen;
            return;
        }

        self.emit_ambient_enabled(r, g, b, lum, pixels_gen);
    }

    fn replay_ambient_tint(&mut self) {
        if !self.ambient.enabled {
            self.emit_ambient_disabled();
            return;
        }
        if let Some((r, g, b)) = self.ambient.last_rgb {
            self.emit_ambient_enabled(r, g, b, self.ambient.last_lum, self.ambient.last_gen);
            return;
        }
        // No cached sample yet — try to build one now that a reader is listening.
        self.ambient.last_gen = 0;
    }

    fn emit_ambient_enabled(&mut self, r: u8, g: u8, b: u8, lum: f32, pixels_gen: u64) {
        self.ambient.last_gen = pixels_gen;
        self.ambient.last_rgb = Some((r, g, b));
        self.ambient.last_lum = lum;
        self.ambient.last_enabled_emit = true;
        tracing::info!(r, g, b, lum, pixels_gen, "ambient tint emit");
        self.event_bus.emit(&CompositorEvent::AmbientTint {
            r,
            g,
            b,
            luminance: lum,
            generation: pixels_gen,
            enabled: true,
        });
    }

    fn emit_ambient_disabled(&mut self) {
        self.ambient.last_enabled_emit = false;
        self.ambient.last_rgb = None;
        self.event_bus.emit(&CompositorEvent::AmbientTint {
            r: 0,
            g: 0,
            b: 0,
            luminance: 0.0,
            generation: self.wallpaper.cpu_pixels_gen(),
            enabled: false,
        });
    }
}

fn sample_wallpaper_ambient(wallpaper: &crate::wallpaper::Wallpaper) -> Option<(u8, u8, u8, f32)> {
    let pixels = wallpaper.cpu_pixels_ref()?;
    let full = wallpaper.full_size();
    let fw = full.w.max(1) as u32;
    let fh = full.h.max(1) as u32;
    let expected = (fw as usize).saturating_mul(fh as usize).saturating_mul(4);
    if pixels.len() != expected {
        return None;
    }
    let full_img = image::RgbaImage::from_raw(fw, fh, pixels.to_vec())?;
    let small = image::imageops::resize(
        &full_img,
        SAMPLE_EDGE,
        SAMPLE_EDGE,
        image::imageops::FilterType::Triangle,
    );

    let mut sum_r = 0u64;
    let mut sum_g = 0u64;
    let mut sum_b = 0u64;
    let mut n = 0u64;
    for p in small.pixels() {
        // Skip near-transparent samples (gradients may pad).
        if p.0[3] < 8 {
            continue;
        }
        sum_r += u64::from(p.0[0]);
        sum_g += u64::from(p.0[1]);
        sum_b += u64::from(p.0[2]);
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mut r = (sum_r / n) as u8;
    let mut g = (sum_g / n) as u8;
    let mut b = (sum_b / n) as u8;

    // Pull extreme averages toward midtones so light/dark themes stay readable.
    let lum = relative_luminance(r, g, b);
    if lum < 0.08 {
        r = r.saturating_add(24);
        g = g.saturating_add(24);
        b = b.saturating_add(24);
    } else if lum > 0.92 {
        r = r.saturating_sub(24);
        g = g.saturating_sub(24);
        b = b.saturating_sub(24);
    }
    let lum = relative_luminance(r, g, b);
    Some((r, g, b, lum))
}

fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    let lin = |c: u8| {
        let x = f32::from(c) / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}
