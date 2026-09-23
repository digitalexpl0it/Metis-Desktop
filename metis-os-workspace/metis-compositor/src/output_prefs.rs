//! Apply `outputs.json` per-output preferences (scale + enable/disable).

use std::time::{Duration, Instant};

use metis_config::{OutputsConfig, load_outputs_config, output_prefs};
use smithay::output::Output;
use smithay::output::Scale;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::state::MetisState;

pub struct OutputRuntime {
    last_check: Instant,
    cached: OutputsConfig,
}

impl OutputRuntime {
    pub fn new() -> Self {
        Self {
            last_check: Instant::now(),
            cached: load_outputs_config(),
        }
    }

    pub fn reload_from_disk(&mut self) -> OutputsConfig {
        let cfg = metis_config::load_outputs_config_with_fallback(&self.cached);
        tracing::debug!("reloading outputs.json");
        self.cached = cfg.clone();
        cfg
    }

    pub fn cached(&self) -> &OutputsConfig {
        &self.cached
    }

    /// Throttled re-read of `outputs.json` (~1s), mirroring `input.json`.
    pub fn maybe_refresh(&mut self) -> Option<(OutputsConfig, OutputsConfig)> {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return None;
        }
        self.last_check = Instant::now();
        let before = self.cached.clone();
        let cfg = metis_config::load_outputs_config_with_fallback(&self.cached);
        if cfg == self.cached {
            return None;
        }
        tracing::info!("outputs.json changed — reapplying output preferences");
        self.cached = cfg.clone();
        Some((before, cfg))
    }
}

/// True when the only differences between two configs are night-light fields.
pub fn is_night_light_only_change(before: &OutputsConfig, after: &OutputsConfig) -> bool {
    let night_changed = before.night_light_enabled != after.night_light_enabled
        || before.night_light_temperature != after.night_light_temperature
        || before.night_light_schedule != after.night_light_schedule
        || before.night_light_schedule_12h != after.night_light_schedule_12h;
    if !night_changed {
        return false;
    }
    let mut normalized = after.clone();
    normalized.night_light_enabled = before.night_light_enabled;
    normalized.night_light_temperature = before.night_light_temperature;
    normalized.night_light_schedule = before.night_light_schedule.clone();
    normalized.night_light_schedule_12h = before.night_light_schedule_12h;
    normalized == *before
}

/// Apply night-light field changes already loaded into `output_runtime.cached`.
pub fn refresh_night_light(state: &mut MetisState, before: &OutputsConfig) {
    let cfg = state.output_runtime.cached().clone();
    sync_night_light_schedule_state(state, &cfg);
    let vis_before = metis_config::night_light_effective(before);
    let vis_after = metis_config::night_light_effective(&cfg);
    if vis_before != vis_after || before.night_light_temperature != cfg.night_light_temperature {
        state.night_light_commit.increment();
    }
    if !vis_after {
        state.night_light_schedule_effective = None;
    }
    state.schedule_redraw();
}

fn sync_night_light_schedule_state(state: &mut MetisState, cfg: &OutputsConfig) {
    if cfg.night_light_schedule.enabled {
        state.night_light_schedule_effective = Some(metis_config::night_light_effective(cfg));
    } else {
        state.night_light_schedule_effective = None;
    }
}

/// Apply saved preferences to every connected output. Returns true when any
/// output scale or enabled state changed.
pub fn apply_outputs(state: &mut MetisState, cfg: &OutputsConfig) -> bool {
    let outputs: Vec<Output> = state.connected_outputs();
    let mut changed = false;
    let mut enable_changed = false;
    for output in &outputs {
        let name = output.name();
        let prefs = output_prefs(cfg, &name);
        if prefs.enabled != state.is_output_enabled(&name)
            && state.set_output_enabled(&name, prefs.enabled)
        {
            enable_changed = true;
            changed = true;
        }
    }
    if enable_changed {
        state.retile_after_output_prefs();
    }
    if state.mirror_mode_active() {
        if crate::mirror::apply_mirror_layout(state, cfg) {
            changed = true;
        }
    } else if apply_output_layout(state, cfg) {
        changed = true;
    }
    if crate::output_modes::apply_output_modes(state, cfg) {
        changed = true;
    }
    if crate::output_vrr::apply_output_vrrs(state, cfg) {
        changed = true;
    }
    if crate::output_hdr::apply_output_hdrs(state, cfg) {
        changed = true;
    }
    crate::color_management::apply_color_profiles(state, cfg);
    // Upload each output's ICC vcgt calibration to its CRTC gamma ramp.
    crate::output_gamma::apply_output_gamma(state);
    if state.mirror_mode_active() {
        if crate::mirror::apply_mirror_scales(state, cfg) {
            changed = true;
        }
    } else {
        for output in state.connected_outputs() {
            if !state.is_output_enabled(&output.name()) {
                continue;
            }
            if apply_output_scale(&output, cfg) {
                changed = true;
            }
        }
    }
    if changed {
        post_output_geometry_change(state);
    }
    sync_night_light_schedule_state(state, cfg);
    state.night_light_commit.increment();
    state.schedule_redraw();
    changed
}

fn apply_output_scale(output: &Output, cfg: &OutputsConfig) -> bool {
    let prefs = output_prefs(cfg, &output.name());
    let current = output.current_scale().fractional_scale();
    let next = clamp_user_scale(prefs.scale);
    if (current - next).abs() <= 0.001 {
        return false;
    }
    output.change_current_state(None, None, Some(Scale::Fractional(next)), None);
    tracing::info!(name = %output.name(), scale = next, "applied output scale");
    true
}

fn post_output_geometry_change(state: &mut MetisState) {
    state.clear_mirror_batch_cache();
    state.decorations.invalidate_all();
    state.reflow_for_bar_geometry_change();
    let (full, regions) = state.wallpaper_layout();
    state.wallpaper.set_layout(full, regions);
    state.wallpaper.start_async_decode();
    // DRM modeset / layout shifts can leave client GL buffers blank while Metis
    // SSD chrome still paints. Re-send configures so GTK (and others) redraw.
    state.nudge_clients_after_output_change();
}

fn clamp_user_scale(raw: f64) -> f64 {
    raw.clamp(1.0, 4.0)
}

/// Reposition outputs from saved `layout_x` / `layout_y` in `outputs.json`.
/// With a single active display the desktop always stays at the origin — saved
/// layout offsets are only meaningful when two or more outputs are enabled.
pub fn apply_output_layout(state: &mut MetisState, cfg: &OutputsConfig) -> bool {
    let active: Vec<Output> = state
        .connected_outputs()
        .into_iter()
        .filter(|o| state.is_output_enabled(&o.name()))
        .collect();

    if active.len() < 2 {
        let mut changed = false;
        let origin = Point::from((0i32, 0i32));
        for output in active {
            let current = state.space.output_geometry(&output).map(|g| g.loc);
            if current == Some(origin) {
                continue;
            }
            output.change_current_state(None, None, None, Some(origin));
            state.space.map_output(&output, origin);
            tracing::info!(name = %output.name(), "reset single output to origin");
            changed = true;
        }
        return changed;
    }

    let mut changed = false;
    // Collect intended positions, then shift so the arrangement's top-left is
    // (0, 0). Stops a bad settings drag from parking the primary far off-origin.
    let mut planned: Vec<(Output, i32, i32)> = Vec::new();
    for output in &active {
        let prefs = output_prefs(cfg, &output.name());
        let Some(x) = prefs.layout_x else { continue };
        let Some(y) = prefs.layout_y else { continue };
        planned.push((output.clone(), x, y));
    }
    if planned.is_empty() {
        return false;
    }
    let min_x = planned.iter().map(|(_, x, _)| *x).min().unwrap_or(0);
    let min_y = planned.iter().map(|(_, _, y)| *y).min().unwrap_or(0);
    for (output, x, y) in planned {
        let pos = Point::from((x - min_x, y - min_y));
        let current = state.space.output_geometry(&output).map(|g| g.loc);
        if current == Some(pos) {
            continue;
        }
        output.change_current_state(None, None, None, Some(pos));
        state.space.map_output(&output, pos);
        tracing::info!(name = %output.name(), ?pos, "applied output layout position");
        changed = true;
    }
    changed
}

fn position_right_of_primary(state: &MetisState, cfg: &OutputsConfig) -> Point<i32, Logical> {
    let primary_name = cfg.primary_output.clone().or_else(|| {
        state
            .connected_outputs()
            .into_iter()
            .find(|o| state.is_output_enabled(&o.name()))
            .map(|o| o.name())
    });
    if let Some(name) = primary_name
        && let Some(output) = state
            .connected_outputs()
            .into_iter()
            .find(|o| o.name() == name)
        && let Some(geo) = state.space.output_geometry(&output)
    {
        return Point::from((geo.loc.x + geo.size.w, geo.loc.y));
    }
    let x: i32 = state
        .connected_outputs()
        .iter()
        .filter_map(|o| state.space.output_geometry(o))
        .map(|g| g.loc.x + g.size.w)
        .max()
        .unwrap_or(0);
    Point::from((x, 0))
}

pub fn output_position_for_connect(
    state: &MetisState,
    cfg: &OutputsConfig,
    name: &str,
) -> Point<i32, Logical> {
    let prefs = output_prefs(cfg, name);
    if let (Some(x), Some(y)) = (prefs.layout_x, prefs.layout_y) {
        Point::from((x, y))
    } else {
        position_right_of_primary(state, cfg)
    }
}

/// On HDMI/DP plug: enable as an extended display at the preferred mode, place
/// to the right of the primary when no saved layout exists, and write the
/// result to `outputs.json` so the next replug restores the last arrangement.
///
/// Respects an explicit `enabled: false` from a previous Settings disable.
/// Returns the (possibly updated) config to use for the rest of the connect path.
pub fn persist_hotplug_connect(
    state: &mut MetisState,
    name: &str,
    mode: &metis_protocol::OutputModeInfo,
) -> OutputsConfig {
    use metis_config::{DisplayLayoutMode, save_outputs_config};

    let mut cfg = state.output_runtime.cached().clone();
    let mut changed = false;
    let known = cfg.outputs.contains_key(name);

    let needs_layout = {
        let entry = cfg.outputs.entry(name.to_string()).or_default();
        // Brand-new connector → start enabled. Existing `enabled: false` is kept.
        if !known && !entry.enabled {
            entry.enabled = true;
            changed = true;
        }

        if entry.mode_width.is_none()
            || entry.mode_height.is_none()
            || entry.mode_refresh_millihz.is_none()
        {
            entry.mode_width = Some(mode.width);
            entry.mode_height = Some(mode.height);
            entry.mode_refresh_millihz = Some(mode.refresh_millihz);
            changed = true;
        }

        entry.layout_x.is_none() || entry.layout_y.is_none()
    };

    if needs_layout {
        // Anchor the primary at the origin so relative placement stays stable.
        let primary = cfg.primary_output.clone().or_else(|| {
            state
                .connected_outputs()
                .into_iter()
                .find(|o| o.name() != name && state.is_output_enabled(&o.name()))
                .map(|o| o.name())
        });
        if let Some(ref primary) = primary {
            let p = cfg.outputs.entry(primary.clone()).or_default();
            if p.layout_x.is_none() {
                p.layout_x = Some(0);
            }
            if p.layout_y.is_none() {
                p.layout_y = Some(0);
            }
        }

        let pos = position_right_of_primary(state, &cfg);
        {
            let entry = cfg.outputs.entry(name.to_string()).or_default();
            entry.layout_x = Some(pos.x);
            entry.layout_y = Some(pos.y);
        }
        changed = true;

        if cfg.display_mode != DisplayLayoutMode::Extend {
            cfg.display_mode = DisplayLayoutMode::Extend;
            changed = true;
        }
        tracing::info!(
            %name,
            x = pos.x,
            y = pos.y,
            mode = %format!("{}x{}@{}", mode.width, mode.height, mode.refresh_millihz),
            "hotplug: auto-enabled as extended display"
        );
    }

    if changed {
        if let Err(err) = save_outputs_config(&cfg) {
            tracing::warn!(%err, "failed to persist hotplug output defaults");
        } else {
            state.output_runtime.reload_from_disk();
            cfg = state.output_runtime.cached().clone();
        }
    }
    cfg
}

/// Snapshot the live layout (and current mode) of an output into `outputs.json`
/// before it disappears, so a later replug restores the last working arrangement
/// even if Settings never pressed Save after a compositor-side move.
pub fn persist_output_snapshot(state: &mut MetisState, output: &Output) {
    use metis_config::save_outputs_config;

    let name = output.name();
    let mut cfg = state.output_runtime.cached().clone();
    let entry = cfg.outputs.entry(name.clone()).or_default();
    let mut changed = false;

    if let Some(geo) = state.space.output_geometry(output)
        && (entry.layout_x != Some(geo.loc.x) || entry.layout_y != Some(geo.loc.y))
    {
        entry.layout_x = Some(geo.loc.x);
        entry.layout_y = Some(geo.loc.y);
        changed = true;
    }
    if let Some(mode) = output.current_mode() {
        let w = mode.size.w;
        let h = mode.size.h;
        let r = mode.refresh;
        if entry.mode_width != Some(w)
            || entry.mode_height != Some(h)
            || entry.mode_refresh_millihz != Some(r)
        {
            entry.mode_width = Some(w);
            entry.mode_height = Some(h);
            entry.mode_refresh_millihz = Some(r);
            changed = true;
        }
    }

    if !changed {
        return;
    }
    if let Err(err) = save_outputs_config(&cfg) {
        tracing::warn!(%err, output = %name, "failed to snapshot output layout on disconnect");
        return;
    }
    state.output_runtime.reload_from_disk();
    tracing::debug!(output = %name, "snapshotted output layout for next reconnect");
}

pub(crate) fn output_geometry(
    state: &MetisState,
    output: &Output,
) -> Option<Rectangle<i32, Logical>> {
    state.space.output_geometry(output).or_else(|| {
        output
            .current_mode()
            .map(|mode| Rectangle::new(Point::from((0, 0)), Size::from((mode.size.w, mode.size.h))))
    })
}

pub fn output_info_for(
    state: &MetisState,
    output: &Output,
    primary: Option<&str>,
    mirror_source: Option<&str>,
) -> metis_protocol::OutputInfo {
    let name = output.name();
    let geo = output_geometry(state, output);
    let rect = geo
        .map(|g| metis_protocol::MonitorRect {
            x: g.loc.x,
            y: g.loc.y,
            width: g.size.w,
            height: g.size.h,
        })
        .unwrap_or(metis_protocol::MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    let cfg = &state.output_runtime.cached;
    let prefs = output_prefs(cfg, &name);
    let phys = output.physical_properties();
    let active = state.is_output_enabled(&name);
    let mirror_active = state.mirror_mode_active();
    let is_mirror_source = mirror_active && mirror_source.is_some_and(|s| s == name);
    let is_mirrored = mirror_active && !is_mirror_source && active && prefs.enabled;
    let vrr_support = crate::output_vrr::query_vrr_support(state, &name);
    let vrr_active = crate::output_vrr::query_vrr_active(state, &name);
    let hdr_available = crate::output_hdr::query_hdr_available(state, &name);
    let hdr_active = crate::output_hdr::query_hdr_active(state, &name);
    metis_protocol::OutputInfo {
        name,
        primary: primary.is_some_and(|p| p == output.name()),
        rect,
        scale: output.current_scale().fractional_scale(),
        enabled: active && prefs.enabled,
        make: phys.make,
        model: phys.model,
        mirrored: is_mirrored,
        mirror_source: is_mirror_source,
        vrr_available: crate::output_vrr::vrr_available(vrr_support),
        vrr_active,
        hdr_available,
        hdr_active,
    }
}
