use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::ipc;
use crate::night_light::RenderTargetInfo;
use crate::render::CLEAR_COLOR;
use crate::state::MetisState;

/// Map the hovered resize edge to the matching host (winit) cursor shape. This
/// nested backend always draws the host cursor and ignores client cursor
/// surfaces, so directional resize feedback has to be applied here.
fn resize_cursor(
    edge: Option<crate::grabs::ResizeEdge>,
) -> smithay::reexports::winit::cursor::Cursor {
    use crate::grabs::ResizeEdge;
    use smithay::reexports::winit::cursor::CursorIcon;

    let icon = match edge {
        Some(e) if e == ResizeEdge::TOP_LEFT || e == ResizeEdge::BOTTOM_RIGHT => {
            CursorIcon::NeswResize
        }
        Some(e) if e == ResizeEdge::TOP_RIGHT || e == ResizeEdge::BOTTOM_LEFT => {
            CursorIcon::NwseResize
        }
        Some(e) if e.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) => CursorIcon::EwResize,
        Some(e) if e.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) => CursorIcon::NsResize,
        _ => CursorIcon::Default,
    };
    icon.into()
}

/// Number of virtual outputs to simulate in the nested dev session, from
/// `METIS_VIRTUAL_OUTPUTS` (default 1, clamped 1..=2). `2` tiles the winit window
/// into two side-by-side logical monitors so multi-output behavior (per-output
/// bars, placement, workspaces) can be exercised before a real DRM backend.
fn virtual_output_count() -> usize {
    std::env::var("METIS_VIRTUAL_OUTPUTS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 2)
}

/// Tile the winit framebuffer into `count` side-by-side logical outputs, each as
/// `(mode size, global position)`. The outputs tile the global coordinate space
/// contiguously, so rendering everything in global coords fills the window.
fn output_layout(
    window: Size<i32, Physical>,
    count: usize,
) -> Vec<(Size<i32, Physical>, Point<i32, Logical>)> {
    let w = window.w.max(1);
    let h = window.h.max(1);
    if count >= 2 {
        let left = w / 2;
        vec![
            (Size::from((left, h)), Point::from((0, 0))),
            (Size::from((w - left, h)), Point::from((left, 0))),
        ]
    } else {
        vec![(Size::from((w, h)), Point::from((0, 0)))]
    }
}

pub fn init_winit(
    event_loop: &mut EventLoop<'_, MetisState>,
    state: &mut MetisState,
) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, winit) = winit::init::<GlesRenderer>()?;
    let backend = Rc::new(RefCell::new(backend));

    let window_size = backend.borrow().window_size();
    let count = virtual_output_count();
    let layout = output_layout(window_size, count);
    if count > 1 {
        tracing::info!(count, "METIS_VIRTUAL_OUTPUTS: simulating multiple outputs");
    }

    // Client-visible logical outputs — one per virtual monitor, tiling the window.
    let mut logical_outputs: Vec<Output> = Vec::new();
    for (i, (size, pos)) in layout.iter().enumerate() {
        let output = Output::new(
            format!("metis-{i}"),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Metis".into(),
                model: "Compositor".into(),
                serial_number: i.to_string(),
            },
        );
        let global = output.create_global::<MetisState>(&state.display_handle);
        state.output_globals.insert(output.name(), global);
        let mode = Mode {
            size: *size,
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some(*pos));
        output.set_preferred(mode);
        state.space.map_output(&output, *pos);
        state.ensure_desk_for_output(&output);
        logical_outputs.push(output);
    }
    state.winit_outputs = logical_outputs.clone();

    // Dedicated full-window render output: NOT client-visible and NOT in the
    // Space. It only drives the damage tracker, render scale, wallpaper size, and
    // frame timing so the winit framebuffer is always fully covered no matter how
    // many logical outputs tile it. With one output it matches `metis-0` exactly.
    let render_output = Output::new(
        "metis-render".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Metis".into(),
            model: "Compositor".into(),
            serial_number: "render".into(),
        },
    );
    let render_mode = Mode {
        size: window_size,
        refresh: 60_000,
    };
    render_output.change_current_state(
        Some(render_mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    render_output.set_preferred(render_mode);

    if let Some(geo) = logical_outputs
        .first()
        .and_then(|o| state.space.output_geometry(o))
    {
        state.monitor.width = geo.size.w;
        state.monitor.height = geo.size.h;
    }
    // Compose the wallpaper per output: each logical monitor gets its own
    // cover-cropped image blitted into the shared framebuffer texture.
    let (wp_full, wp_regions) = state.wallpaper_layout();
    state.wallpaper.set_layout(wp_full, wp_regions);

    if state.wallpaper.enabled() {
        state.wallpaper.start_async_decode();
    }

    let cfg = state.output_runtime.cached().clone();
    crate::output_prefs::apply_outputs(state, &cfg);

    let mut damage_tracker = OutputDamageTracker::from_output(&render_output);

    let backend_winit = backend.clone();
    backend.borrow().window().set_title("Metis");
    state.set_redraw_trigger(Rc::new(move || {
        backend_winit.borrow().window().request_redraw();
    }));
    state.damaged = true;
    state.request_redraw();

    // Self-paced ~60fps heartbeat. The nested host does NOT throttle
    // RedrawRequested to vsync, so driving redraws from the Redraw handler
    // (re-request at end of frame) becomes an unbounded busy loop. Instead we
    // re-arm here only while damage is pending, capping us at ~60fps while
    // staying near-zero CPU when idle.
    event_loop.handle().insert_source(
        Timer::from_duration(Duration::from_millis(16)),
        move |_, _, state| {
            // Shared per-tick housekeeping (startup, IPC, wallpaper decode, live
            // blur/decoration config, scroll animation). Kept off the render path
            // so going idle can never starve shell/client spawn.
            state.tick_housekeeping();

            // Frame callbacks are delivered after each render (see Redraw), not
            // on a fixed clock — that keeps GTK's frame clock from spinning when
            // nothing changed. Every client commit schedules a redraw, so a
            // client waiting on its first frame callback is unblocked promptly.
            if state.damaged {
                state.request_redraw();
            }
            TimeoutAction::ToDuration(Duration::from_millis(16))
        },
    )?;

    let backend_winit = backend.clone();
    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            match event {
                WinitEvent::Resized { size, .. } => {
                    state.run_pending_startup();
                    // Re-tile the logical outputs across the new framebuffer size.
                    let layout = output_layout(size, logical_outputs.len().max(1));
                    for (out, (out_size, pos)) in logical_outputs.iter().zip(layout.iter()) {
                        if !state.output_globals.contains_key(&out.name()) {
                            continue;
                        }
                        out.change_current_state(
                            Some(Mode {
                                size: *out_size,
                                refresh: 60_000,
                            }),
                            Some(Transform::Flipped180),
                            None,
                            Some(*pos),
                        );
                        state.space.map_output(out, *pos);
                    }
                    render_output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: 60_000,
                        }),
                        Some(Transform::Flipped180),
                        None,
                        None,
                    );
                    if let Some(geo) = logical_outputs
                        .first()
                        .and_then(|o| state.space.output_geometry(o))
                    {
                        state.monitor.width = geo.size.w;
                        state.monitor.height = geo.size.h;
                    }
                    let (wp_full, wp_regions) = state.wallpaper_layout();
                    state.wallpaper.set_layout(wp_full, wp_regions);
                    state.emit_monitor_changed();
                    let ids: Vec<u32> = state.windows.ids();
                    if !ids.is_empty() {
                        for id in ids {
                            state.apply_window_rect(id);
                        }
                        state.sync_all_app_windows();
                        state.refresh_all_scroll_offsets();
                    }
                    state.arrange_layers();
                }
                WinitEvent::Input(event) => {
                    state.run_pending_startup();
                    state.process_input_event(event);
                }
                WinitEvent::Redraw => {
                    state.run_pending_startup();
                    ipc::drain_ipc(state);

                    // Damage-gated render keeps idle CPU near zero. Input handlers
                    // flag damage on pointer motion/clicks, so cursor and UI feedback
                    // still update during interaction; the heartbeat caps us at 60fps.
                    let render = state.damaged;

                    if state.image_capture.has_pending()
                        || state.has_pending_window_thumbs()
                        || state.has_pending_workspace_thumbs()
                    {
                        let mut backend = backend_winit.borrow_mut();
                        match backend.bind() {
                            Ok((renderer, _framebuffer)) => {
                                state.process_pending_captures(renderer);
                            }
                            Err(err) => tracing::warn!(?err, "winit capture bind failed"),
                        };
                    }

                    if render {
                        let mut backend = backend_winit.borrow_mut();
                        let size = backend.window_size();
                        let damage = Rectangle::from_size(size);

                        // Single cursor source: always show the host (winit) cursor and
                        // never render client cursor surfaces. In this nested compositor,
                        // rendering the client's own cursor produced a second cursor with a
                        // mismatched size over GTK surfaces; the host cursor stays uniform.
                        backend.window().set_cursor_visible(true);
                        let cursor = if state.metis_bar_ui_hit(
                            state
                                .seat
                                .get_pointer()
                                .map(|p| p.current_location())
                                .unwrap_or_default(),
                        ) {
                            resize_cursor(None)
                        } else {
                            resize_cursor(state.hover_cursor)
                        };
                        backend.window().set_cursor(cursor);

                        let output_scale =
                            Scale::from(render_output.current_scale().fractional_scale());
                        match backend.bind() {
                            Ok((renderer, mut framebuffer)) => {
                                // The nested backend renders the whole virtual desktop
                                // into a single framebuffer, so it builds elements in
                                // global coords (render origin (0, 0)).
                                let render_elements = state.build_render_elements(
                                    renderer,
                                    Point::<i32, Physical>::from((0, 0)),
                                    output_scale,
                                    RenderTargetInfo {
                                        size: Size::from((size.w, size.h)),
                                        output_name: None,
                                        skip_night_light: false,
                                    },
                                    &[],
                                    true,
                                );

                                // Winit re-binds the same framebuffer each frame, so it
                                // can't track buffer age; a fixed age of 0 makes the
                                // damage tracker treat each frame as a full redraw.
                                if let Err(err) = damage_tracker.render_output(
                                    renderer,
                                    &mut framebuffer,
                                    0,
                                    &render_elements,
                                    CLEAR_COLOR,
                                ) {
                                    tracing::warn!(?err, "render_output failed");
                                }
                                state.process_pending_captures(renderer);
                            }
                            Err(err) => {
                                tracing::warn!(?err, "winit GL bind failed — skipping frame");
                            }
                        }
                        if let Err(err) = backend.submit(Some(&[damage])) {
                            tracing::warn!(?err, "winit frame submit failed");
                        }
                    }

                    if render {
                        // Deliver frame callbacks for the frame we just presented so
                        // clients paint their next frame; then clear damage + flush.
                        let now = state.start_time.elapsed();
                        let frame_outputs: Vec<Output> = state.space.outputs().cloned().collect();
                        if let Some(primary) = frame_outputs.first() {
                            let primary = primary.clone();
                            state.space.elements().for_each(|window| {
                                window.send_frame(&primary, now, Some(Duration::ZERO), |_, _| {
                                    Some(primary.clone())
                                });
                            });
                        }
                        for out in &frame_outputs {
                            state.send_layer_frames(out, now);
                        }
                        state.damaged = false;
                        state.defer_client_flush = true;
                    }

                    state.space.refresh();
                    state.cleanup_destroyed_windows();
                    state.popups.cleanup();
                    for out in state.space.outputs() {
                        smithay::desktop::layer_map_for_output(out).cleanup();
                    }
                }
                WinitEvent::CloseRequested => {
                    tracing::info!("compositor winit window close requested — shutting down");
                    state.end_compositor_session();
                }
                _ => (),
            }
        })?;

    Ok(())
}
