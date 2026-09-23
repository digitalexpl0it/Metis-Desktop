//! Equalizer desktop widget — system-audio spectrum / wave visualizer.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;
use std::time::Duration;

use gtk::cairo::{self, Context};
use gtk::prelude::*;
use metis_config::{
    DesktopWidgetInstance, EqualizerBarShape, EqualizerColorMode, EqualizerVizStyle,
};

use crate::services::{audio_viz_frame, ensure_audio_viz, release_audio_viz};
use crate::ui::theme::active_tokens;

#[derive(Clone)]
struct EqDrawOpts {
    style: EqualizerVizStyle,
    bar_shape: EqualizerBarShape,
    color_mode: EqualizerColorMode,
    solid: String,
    gradient_start: String,
    gradient_end: String,
    bar_gradient: bool,
    show_peaks: bool,
    peak_color: String,
    reflection: bool,
}

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    ensure_audio_viz();

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_content_width(inst.w as i32);
    area.set_content_height(inst.h.saturating_sub(36) as i32);
    area.add_css_class("metis-dw-equalizer");

    let opts = EqDrawOpts {
        style: inst.viz_style,
        bar_shape: inst.bar_shape,
        color_mode: inst.color_mode,
        solid: inst.solid_color.clone(),
        gradient_start: inst.gradient_start.clone(),
        gradient_end: inst.gradient_end.clone(),
        bar_gradient: inst.bar_gradient,
        show_peaks: inst.show_peaks,
        peak_color: inst.peak_color.clone(),
        reflection: inst.show_reflection,
    };
    let bar_count = inst.bar_count.clamp(16, 96);

    let latest = Rc::new(RefCell::new(audio_viz_frame(bar_count)));

    {
        let latest = latest.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let frame = latest.borrow().clone();
            draw_viz(cr, w as f64, h as f64, &frame.bands, &frame.peaks, &opts);
        });
    }

    let area_weak = area.downgrade();
    let tick = glib::timeout_add_local(Duration::from_millis(16), {
        let latest = latest.clone();
        let mut silent_frames = 0u32;
        move || {
            let Some(area) = area_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let frame = audio_viz_frame(bar_count);
            let energy: f32 = frame.bands.iter().chain(frame.peaks.iter()).copied().sum();
            if energy < 0.02 {
                silent_frames = silent_frames.saturating_add(1);
                // Silence (incl. decayed peaks) already drawn: stop damaging
                // the surface until audio returns.
                if silent_frames > 30 {
                    return glib::ControlFlow::Continue;
                }
            } else {
                silent_frames = 0;
            }
            *latest.borrow_mut() = frame;
            area.queue_draw();
            glib::ControlFlow::Continue
        }
    });
    let tick_id = Rc::new(Cell::new(Some(tick)));
    area.connect_destroy({
        let tick_id = tick_id.clone();
        move |_| {
            if let Some(id) = tick_id.take() {
                id.remove();
            }
            release_audio_viz();
        }
    });

    area.upcast()
}

fn draw_viz(cr: &Context, w: f64, h: f64, bands: &[f32], peaks: &[f32], opts: &EqDrawOpts) {
    cr.set_operator(cairo::Operator::Over);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    cr.paint().ok();

    if w < 4.0 || h < 4.0 || bands.is_empty() {
        return;
    }

    match opts.style {
        EqualizerVizStyle::SpectrumLines => {
            draw_spectrum_lines(cr, w, h, bands, opts);
        }
        EqualizerVizStyle::Bars => {
            draw_bars(cr, w, h, bands, peaks, opts);
        }
        EqualizerVizStyle::NeonWave => {
            draw_neon_wave(cr, w, h, bands, opts);
        }
        EqualizerVizStyle::Radial => {
            draw_radial(cr, w, h, bands, opts);
        }
    }
}

fn color_at(t: f64, opts: &EqDrawOpts) -> (f64, f64, f64) {
    let t = t.clamp(0.0, 1.0);
    match opts.color_mode {
        EqualizerColorMode::Solid => parse_hex_rgb(&opts.solid).unwrap_or((0.0, 0.9, 1.0)),
        EqualizerColorMode::Theme => {
            let tokens = active_tokens();
            let a = parse_hex_rgb(tokens.accent_primary()).unwrap_or((0.0, 0.85, 1.0));
            let b = parse_hex_rgb(tokens.accent_secondary()).unwrap_or((0.7, 0.3, 1.0));
            (
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                a.2 + (b.2 - a.2) * t,
            )
        }
        EqualizerColorMode::Multi => {
            let a = parse_hex_rgb(&opts.gradient_start).unwrap_or((1.0, 0.88, 0.2));
            let b = parse_hex_rgb(&opts.gradient_end).unwrap_or((0.15, 0.85, 1.0));
            (
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                a.2 + (b.2 - a.2) * t,
            )
        }
    }
}

fn shade(rgb: (f64, f64, f64), t: f64, gradient: bool) -> (f64, f64, f64) {
    if !gradient {
        return rgb;
    }
    let t = t.clamp(0.0, 1.0);
    (
        (rgb.0 * (0.55 + 0.45 * t)).clamp(0.0, 1.0),
        (rgb.1 * (0.55 + 0.45 * t)).clamp(0.0, 1.0),
        (rgb.2 * (0.55 + 0.45 * t)).clamp(0.0, 1.0),
    )
}

fn parse_hex_rgb(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()? as f64 / 255.0;
    let g = u8::from_str_radix(&h[2..4], 16).ok()? as f64 / 255.0;
    let b = u8::from_str_radix(&h[4..6], 16).ok()? as f64 / 255.0;
    Some((r, g, b))
}

fn draw_spectrum_lines(cr: &Context, w: f64, h: f64, bands: &[f32], opts: &EqDrawOpts) {
    let mid = h * 0.5;
    let n = bands.len().max(1);
    let step = w / n as f64;
    cr.set_line_width((1.2_f64).max(step * 0.35));
    for (i, &level) in bands.iter().enumerate() {
        let x = (i as f64 + 0.5) * step;
        let amp = (level as f64).clamp(0.0, 1.0) * mid * 0.95;
        let (r, g, b) = color_at(i as f64 / n as f64, opts);
        cr.set_source_rgba(r, g, b, 0.92);
        cr.move_to(x, mid - amp);
        cr.line_to(x, mid + amp);
        let _ = cr.stroke();
        cr.set_source_rgba(r, g, b, 0.25);
        cr.set_line_width((2.5_f64).max(step * 0.55));
        cr.move_to(x, mid - amp * 1.05);
        cr.line_to(x, mid + amp * 1.05);
        let _ = cr.stroke();
        cr.set_line_width((1.2_f64).max(step * 0.35));
    }
}

fn draw_bars(cr: &Context, w: f64, h: f64, bands: &[f32], peaks: &[f32], opts: &EqDrawOpts) {
    let n = bands.len().max(1);
    let gap = 2.0;
    let bar_w = ((w - gap * (n as f64 + 1.0)) / n as f64).max(2.0);
    let main_h = if opts.reflection { h * 0.62 } else { h * 0.92 };
    let base_y = if opts.reflection { h * 0.58 } else { h * 0.96 };
    let peak_rgb = parse_hex_rgb(&opts.peak_color).unwrap_or((1.0, 0.35, 0.85));

    for (i, &level) in bands.iter().enumerate() {
        let x = gap + i as f64 * (bar_w + gap);
        let rgb = color_at(i as f64 / n as f64, opts);
        let amp = (level as f64).clamp(0.0, 1.0) * main_h;

        match opts.bar_shape {
            EqualizerBarShape::Segmented => {
                draw_segmented_column(cr, x, base_y, bar_w, amp, rgb, opts.bar_gradient, false);
                if opts.reflection {
                    draw_segmented_column(cr, x, base_y + 4.0, bar_w, amp * 0.45, rgb, false, true);
                }
            }
            EqualizerBarShape::Solid => {
                draw_solid_column(cr, x, base_y, bar_w, amp, rgb, opts.bar_gradient, false);
                if opts.reflection {
                    draw_solid_column(cr, x, base_y + 4.0, bar_w, amp * 0.45, rgb, false, true);
                }
            }
            EqualizerBarShape::Dots | EqualizerBarShape::DenseDots => {
                let dense = opts.bar_shape == EqualizerBarShape::DenseDots;
                draw_dot_column(
                    cr,
                    x,
                    base_y,
                    bar_w,
                    amp,
                    rgb,
                    opts.bar_gradient,
                    dense,
                    false,
                );
                if opts.reflection {
                    draw_dot_column(
                        cr,
                        x,
                        base_y + 4.0,
                        bar_w,
                        amp * 0.45,
                        rgb,
                        false,
                        dense,
                        true,
                    );
                }
            }
        }

        if opts.show_peaks {
            let peak = peaks.get(i).copied().unwrap_or(level) as f64;
            let peak_amp = peak.clamp(0.0, 1.0) * main_h;
            if peak_amp > 2.0 {
                match opts.bar_shape {
                    EqualizerBarShape::Solid => {
                        // Soft circular tip glow (screenshot middle-left).
                        let cx = x + bar_w * 0.5;
                        let cy = base_y - peak_amp;
                        let rad = (bar_w * 0.55).clamp(2.5, 7.0);
                        cr.arc(cx, cy, rad * 1.6, 0.0, TAU);
                        cr.set_source_rgba(peak_rgb.0, peak_rgb.1, peak_rgb.2, 0.28);
                        let _ = cr.fill();
                        cr.arc(cx, cy, rad, 0.0, TAU);
                        cr.set_source_rgba(peak_rgb.0, peak_rgb.1, peak_rgb.2, 0.95);
                        let _ = cr.fill();
                    }
                    EqualizerBarShape::Dots | EqualizerBarShape::DenseDots => {
                        let cx = x + bar_w * 0.5;
                        let cy = base_y - peak_amp;
                        let rad = if opts.bar_shape == EqualizerBarShape::DenseDots {
                            (bar_w * 0.28).clamp(1.2, 3.5)
                        } else {
                            (bar_w * 0.38).clamp(1.8, 5.0)
                        };
                        cr.arc(cx, cy, rad, 0.0, TAU);
                        cr.set_source_rgba(peak_rgb.0, peak_rgb.1, peak_rgb.2, 0.95);
                        let _ = cr.fill();
                    }
                    EqualizerBarShape::Segmented => {
                        let py = base_y - peak_amp - 3.0;
                        rounded_rect(cr, x, py, bar_w, 2.8, 1.0);
                        cr.set_source_rgba(peak_rgb.0, peak_rgb.1, peak_rgb.2, 0.9);
                        let _ = cr.fill();
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_segmented_column(
    cr: &Context,
    x: f64,
    base_y: f64,
    bar_w: f64,
    amp: f64,
    rgb: (f64, f64, f64),
    gradient: bool,
    downward: bool,
) {
    let seg_h = 4.0;
    let seg_gap = 1.5;
    let segs = ((amp / (seg_h + seg_gap)).floor() as i32).max(0);
    for s in 0..segs {
        let y = if downward {
            base_y + s as f64 * (seg_h + seg_gap)
        } else {
            base_y - (s as f64 + 1.0) * (seg_h + seg_gap)
        };
        let t = s as f64 / segs.max(1) as f64;
        let (rr, gg, bb) = shade(rgb, t, gradient);
        let alpha = if downward { 0.22 * (1.0 - t) } else { 0.95 };
        rounded_rect(cr, x, y, bar_w, seg_h, 1.2);
        cr.set_source_rgba(rr, gg, bb, alpha);
        let _ = cr.fill();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_solid_column(
    cr: &Context,
    x: f64,
    base_y: f64,
    bar_w: f64,
    amp: f64,
    rgb: (f64, f64, f64),
    gradient: bool,
    downward: bool,
) {
    if amp < 1.0 {
        return;
    }
    let (y, h) = if downward {
        (base_y, amp)
    } else {
        (base_y - amp, amp)
    };
    let radius = (bar_w * 0.35).clamp(1.5, 6.0);
    if gradient && !downward {
        // Vertical gradient via stacked thin strips for a smooth lift.
        let steps = ((amp / 2.0).ceil() as i32).max(1);
        let step_h = amp / steps as f64;
        for s in 0..steps {
            let t = s as f64 / steps.max(1) as f64;
            let (rr, gg, bb) = shade(rgb, t, true);
            let sy = y + (steps - 1 - s) as f64 * step_h;
            rounded_rect(cr, x, sy, bar_w, step_h + 0.5, radius.min(step_h));
            cr.set_source_rgba(rr, gg, bb, 0.95);
            let _ = cr.fill();
        }
    } else {
        rounded_rect(cr, x, y, bar_w, h, radius);
        let alpha = if downward { 0.22 } else { 0.95 };
        cr.set_source_rgba(rgb.0, rgb.1, rgb.2, alpha);
        let _ = cr.fill();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_dot_column(
    cr: &Context,
    x: f64,
    base_y: f64,
    bar_w: f64,
    amp: f64,
    rgb: (f64, f64, f64),
    gradient: bool,
    dense: bool,
    downward: bool,
) {
    let (rad, gap) = if dense {
        ((bar_w * 0.28).clamp(1.2, 3.2), 1.4_f64)
    } else {
        ((bar_w * 0.38).clamp(1.8, 5.0), 2.2_f64)
    };
    let pitch = rad * 2.0 + gap;
    let dots = ((amp / pitch).floor() as i32).max(0);
    let cx = x + bar_w * 0.5;
    for s in 0..dots {
        let cy = if downward {
            base_y + rad + s as f64 * pitch
        } else {
            base_y - rad - s as f64 * pitch
        };
        let t = s as f64 / dots.max(1) as f64;
        let (rr, gg, bb) = shade(rgb, t, gradient);
        let alpha = if downward { 0.2 * (1.0 - t) } else { 0.95 };
        cr.arc(cx, cy, rad, 0.0, TAU);
        cr.set_source_rgba(rr, gg, bb, alpha);
        let _ = cr.fill();
    }
}

fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

fn draw_neon_wave(cr: &Context, w: f64, h: f64, bands: &[f32], opts: &EqDrawOpts) {
    let mid = h * 0.5;
    let n = bands.len().max(2);
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(n);
    for (i, &level) in bands.iter().enumerate() {
        let x = (i as f64 / (n - 1) as f64) * w;
        let y = mid - (level as f64).clamp(0.0, 1.0) * mid * 0.9;
        pts.push((x, y));
    }

    for (width, alpha) in [(10.0, 0.12), (5.0, 0.28), (2.2, 0.85)] {
        stroke_smooth_path(cr, &pts, mid, width, alpha, opts, opts.reflection);
    }
    stroke_smooth_path(cr, &pts, mid, 1.1, 1.0, opts, false);
}

fn stroke_smooth_path(
    cr: &Context,
    pts: &[(f64, f64)],
    mid: f64,
    width: f64,
    alpha: f64,
    opts: &EqDrawOpts,
    mirror: bool,
) {
    if pts.len() < 2 {
        return;
    }
    cr.set_line_width(width);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);

    for i in 0..pts.len() - 1 {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[i + 1];
        let t = i as f64 / (pts.len() - 1) as f64;
        let (r, g, b) = color_at(t, opts);
        cr.set_source_rgba(r, g, b, alpha);
        cr.move_to(x0, y0);
        let mx = (x0 + x1) * 0.5;
        let my = (y0 + y1) * 0.5;
        cr.curve_to(x0 + (mx - x0) * 0.5, y0, mx, my, x1, y1);
        let _ = cr.stroke();

        if mirror {
            cr.set_source_rgba(r, g, b, alpha * 0.55);
            let y0m = mid + (mid - y0);
            let y1m = mid + (mid - y1);
            let mym = mid + (mid - my);
            cr.move_to(x0, y0m);
            cr.curve_to(x0 + (mx - x0) * 0.5, y0m, mx, mym, x1, y1m);
            let _ = cr.stroke();
        }
    }
}

fn draw_radial(cr: &Context, w: f64, h: f64, bands: &[f32], opts: &EqDrawOpts) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let max_r = cx.min(cy) * 0.92;
    let inner = max_r * 0.22;
    let n = bands.len().max(1);
    cr.set_line_cap(cairo::LineCap::Round);

    for (i, &level) in bands.iter().enumerate() {
        let t = i as f64 / n as f64;
        let angle = t * TAU - std::f64::consts::FRAC_PI_2;
        let amp = (level as f64).clamp(0.0, 1.0);
        let outer = inner + amp * (max_r - inner);
        let (r, g, b) = color_at(t, opts);
        let (x0, y0) = (cx + angle.cos() * inner, cy + angle.sin() * inner);
        let (x1, y1) = (cx + angle.cos() * outer, cy + angle.sin() * outer);

        cr.set_line_width((2.0_f64).max(max_r * 0.035));
        cr.set_source_rgba(r, g, b, 0.22);
        cr.move_to(x0, y0);
        cr.line_to(x1, y1);
        let _ = cr.stroke();

        cr.set_line_width((1.1_f64).max(max_r * 0.018));
        cr.set_source_rgba(r, g, b, 0.95);
        cr.move_to(x0, y0);
        cr.line_to(x1, y1);
        let _ = cr.stroke();
    }

    // Soft centre ring
    cr.arc(cx, cy, inner * 0.85, 0.0, TAU);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
    let _ = cr.fill();
}
