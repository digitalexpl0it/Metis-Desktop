use std::sync::mpsc::Receiver;

use gtk::glib;

use crate::state::{MetisInit, SystemEvent};
use crate::ui::{bar, onboarding, splash, theme};

/// Minimal GTK shell — edge bar only (no command overlay).
///
/// We deliberately do **not** use `GtkApplication`. Its `startup` handler (run
/// inside `g_application_register`) creates a *synchronous* `GDBusProxy` for the
/// desktop portal with a 25-second timeout. In a bare standalone Metis session
/// (no GNOME), that portal cold-starts slowly or hangs, so `app.run()` blocks
/// before `activate` ever fires — the bar is never built and only the wallpaper
/// shows. Driving GTK directly with `gtk::init()` + a plain `glib::MainLoop`
/// avoids `g_application_register` entirely, so there is no blocking portal proxy.
pub fn run(init: MetisInit) {
    tracing::info!(
        wayland_display = ?std::env::var("WAYLAND_DISPLAY").ok(),
        "initializing GTK"
    );
    let gtk_init_started = std::time::Instant::now();
    if let Err(err) = gtk::init() {
        tracing::error!(?err, "gtk::init() failed — cannot open display");
        return;
    }
    let gtk_init_ms = gtk_init_started.elapsed().as_millis();
    if gtk_init_ms > 2_000 {
        tracing::warn!(
            gtk_init_ms,
            "gtk::init() was slow — usually a portal/Settings D-Bus timeout"
        );
    }
    tracing::info!(
        have_display = gtk::gdk::Display::default().is_some(),
        gtk_init_ms,
        "gtk::init() ok — building shell"
    );

    // Must run after `gtk::init()` — gtk4 panics otherwise.
    let dir = if metis_i18n::is_rtl() {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    };
    gtk::Widget::set_default_direction(dir);

    theme::install_theme();
    splash::show();
    splash::on_complete(onboarding::show_if_needed);
    bar::init_and_show();
    attach_system_events(init.event_rx);
    // The bar maps and pollers attach shortly after; let the splash ramp to
    // completion and fade once the shell is up and running.
    glib::timeout_add_seconds_local_once(2, splash::finish);

    tracing::info!("starting GLib main loop");
    let main_loop = glib::MainLoop::new(None, false);
    main_loop.run();
    tracing::warn!("GLib main loop returned — shell exiting");
}

/// Isolated desktop-widgets process — no edge bar, NC, dashboard, or splash.
pub fn run_desktop_widgets(init: MetisInit) {
    tracing::info!(
        wayland_display = ?std::env::var("WAYLAND_DISPLAY").ok(),
        "initializing GTK (desktop-widgets mode)"
    );
    let gtk_init_started = std::time::Instant::now();
    if let Err(err) = gtk::init() {
        tracing::error!(?err, "gtk::init() failed — cannot open display");
        return;
    }
    let gtk_init_ms = gtk_init_started.elapsed().as_millis();
    if gtk_init_ms > 2_000 {
        tracing::warn!(
            gtk_init_ms,
            "gtk::init() (desktop-widgets) was slow — usually a portal/Settings D-Bus timeout"
        );
    }

    let dir = if metis_i18n::is_rtl() {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    };
    gtk::Widget::set_default_direction(dir);

    theme::install_theme();
    crate::ui::desktop_widgets::init_autonomous();
    // Keep the compositor event channel drained so the listener thread does not
    // fill unbounded buffers; widgets do not render bar chrome from these events.
    attach_system_events_widgets(init.event_rx);

    tracing::info!("starting GLib main loop (desktop-widgets)");
    let main_loop = glib::MainLoop::new(None, false);
    main_loop.run();
    tracing::warn!("GLib main loop returned — desktop widgets exiting");
}

fn attach_system_events(event_rx: Receiver<SystemEvent>) {
    glib::timeout_add_local(std::time::Duration::from_millis(32), move || {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                SystemEvent::Status(msg) => tracing::debug!(%msg, "status"),
                SystemEvent::CompositorConnected => {
                    crate::services::windows::reconcile_now();
                }
                SystemEvent::Compositor(evt) => {
                    if let metis_protocol::CompositorEvent::WorkspaceChanged {
                        output,
                        active,
                        ephemeral_remote,
                        ..
                    } = &evt
                    {
                        crate::services::set_active_workspace(output, *active);
                        crate::services::set_ephemeral_remote(output, *ephemeral_remote);
                        crate::ui::bar::refresh_workspaces();
                        // Pull a fresh window list so each output's dock reflects
                        // the now-visible workspace (and any cross-workspace move).
                        crate::services::windows::reconcile_now();
                    }
                    if let metis_protocol::CompositorEvent::EdgeBarVisible { output, visible } =
                        &evt
                    {
                        crate::ui::bar::set_edge_bar_visible(output, *visible);
                    }
                    crate::services::windows::apply_event(&evt);
                    // A freshly opened window arrives without its output/workspace
                    // (the event doesn't carry them); reconcile so dock filtering
                    // routes it to the right bar promptly.
                    if matches!(&evt, metis_protocol::CompositorEvent::WindowOpened { .. }) {
                        crate::services::windows::reconcile_now();
                    }
                    // Refresh Task View only on window/layout churn (not clipboard,
                    // hotplug, etc.) so shelf thumbs track focus/stacking changes.
                    if matches!(
                        &evt,
                        metis_protocol::CompositorEvent::WindowOpened { .. }
                            | metis_protocol::CompositorEvent::WindowClosed { .. }
                            | metis_protocol::CompositorEvent::WindowFocused { .. }
                            | metis_protocol::CompositorEvent::WindowMetadata { .. }
                            | metis_protocol::CompositorEvent::WindowList { .. }
                            | metis_protocol::CompositorEvent::LayoutApplied
                            | metis_protocol::CompositorEvent::WorkspaceChanged { .. }
                    ) {
                        crate::ui::task_view::on_windows_changed();
                    }
                    if let metis_protocol::CompositorEvent::ClipboardChanged {
                        mime,
                        preview_text,
                        image_path,
                    } = &evt
                    {
                        crate::services::apply_clipboard_event(
                            mime,
                            preview_text.clone(),
                            image_path.clone(),
                        );
                    }
                    if let metis_protocol::CompositorEvent::OutputHotplug {
                        connected,
                        name,
                        make,
                        model,
                    } = &evt
                    {
                        crate::ui::bar::notify_display_hotplug(*connected, name, make, model);
                    }
                }
                SystemEvent::BriefingReady(items) => {
                    tracing::info!(count = items.len(), "briefing ready");
                }
            }
        }
        glib::ControlFlow::Continue
    });

    // Safety-net reconcile: the event stream is authoritative, but a periodic
    // ListWindows resync guards against any dropped/missed window events.
    glib::timeout_add_seconds_local(5, || {
        crate::services::windows::reconcile_now();
        glib::ControlFlow::Continue
    });
}

fn attach_system_events_widgets(event_rx: Receiver<SystemEvent>) {
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        while let Ok(event) = event_rx.try_recv() {
            if let SystemEvent::Status(msg) = event {
                tracing::debug!(%msg, "status");
            }
        }
        glib::ControlFlow::Continue
    });
}
