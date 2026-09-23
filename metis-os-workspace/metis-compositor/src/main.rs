#![allow(irrefutable_let_patterns)]

mod battery_dim;
mod blur;
mod capture_overlay;
mod clipboard;
mod color_lut;
mod color_management;
mod cross_gpu;
mod cursor;
mod decoration;
mod decoration_overrides;
mod decoration_policy;
mod desk_input;
mod device_input;
mod events;
mod focus;
mod grabs;
mod handlers;
mod hdr_encode;
mod hdr_surface;
mod idle;
mod image_capture;
mod input;
mod ipc;
mod ipc_dispatch;
mod keybinds;
mod lock;
mod lock_auth_cues;
mod mirror;
mod night_light;
mod output_colour;
mod output_gamma;
mod output_hdr;
mod output_modes;
mod output_prefs;
mod output_vrr;
mod remote_input;
mod render;
mod screenshot_overlay;
mod session_lock;
mod state;
mod text_layout;
mod udev;
mod wallpaper;
mod window_fx;
mod window_state;
mod window_thumb;
mod windows;
mod winit;
mod workspace_thumb;
mod xwayland;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::state::MetisState;

/// Drops one specific benign Smithay log line.
///
/// GTK creates short-lived sub-popups (text-entry selection menus, etc.) and
/// tears them down before their xdg-popup grab is processed, which makes
/// Smithay's pre-commit hook log `surface missing from known popups` at ERROR.
/// The popover still works; this filter swallows only that exact message while
/// keeping every other xdg-shell diagnostic intact.
struct DropKnownPopupNoise;

impl<S> tracing_subscriber::layer::Filter<S> for DropKnownPopupNoise {
    fn enabled(
        &self,
        _meta: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        true
    }

    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        if event.metadata().target() != "smithay::wayland::shell::xdg" {
            return true;
        }
        let mut probe = PopupNoiseProbe::default();
        event.record(&mut probe);
        !probe.matched
    }
}

#[derive(Default)]
struct PopupNoiseProbe {
    matched: bool,
}

impl tracing::field::Visit for PopupNoiseProbe {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message"
            && format!("{value:?}").contains("surface missing from known popups")
        {
            self.matched = true;
        }
    }
}

/// Which rendering/session backend Metis runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// Nested winit window inside an existing Wayland/X11 session (dev path).
    Winit,
    /// Standalone DRM/KMS + libseat + libinput session on a bare TTY/GPU.
    Drm,
}

/// Pick the backend. `METIS_BACKEND=winit|drm` forces a choice; otherwise we
/// autodetect: a parent Wayland/X11 session (`WAYLAND_DISPLAY`/`DISPLAY` set)
/// means we are nested (winit), and a bare TTY means the standalone DRM session.
fn select_backend() -> Backend {
    match std::env::var("METIS_BACKEND").ok().as_deref() {
        Some("winit") => return Backend::Winit,
        Some("drm") | Some("udev") | Some("tty") => {
            let nested = std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var_os("DISPLAY").is_some();
            if nested {
                tracing::warn!(
                    "METIS_BACKEND=drm ignored while nested in a host session — using winit"
                );
                return Backend::Winit;
            }
            return Backend::Drm;
        }
        Some(other) if !other.is_empty() => {
            tracing::warn!(%other, "unknown METIS_BACKEND, autodetecting");
        }
        _ => {}
    }
    let nested =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if nested { Backend::Winit } else { Backend::Drm }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .with_filter(DropKnownPopupNoise),
        )
        .init();

    metis_i18n::init();

    let backend = select_backend();
    tracing::info!(?backend, "selected compositor backend");

    let mut event_loop: EventLoop<'static, MetisState> = EventLoop::try_new()?;
    let display: Display<MetisState> = Display::new()?;
    let mut state = MetisState::new(&mut event_loop, display);

    match backend {
        Backend::Winit => {
            winit::init_winit(&mut event_loop, &mut state)?;
            // Nested winit dev session — shell skips dbus notification takeover and
            // other host-session side effects that fight GNOME during startup.
            unsafe {
                std::env::set_var("METIS_NESTED", "1");
            }
        }
        Backend::Drm => {
            udev::init_udev(&mut event_loop, &mut state)?;
        }
    }

    ipc::init_ipc(&mut state)?;
    state.start_xwayland(event_loop.handle());

    // Arm the idle blank countdown up front so an untouched session still blanks
    // after the configured timeout (input activity re-arms it thereafter).
    state.idle_reschedule();

    // Register the PAM auth result channel so lock-screen authentication (run on
    // a worker thread) can hand its result back to the event loop.
    state.lock_register_auth_channel();

    // Capture the host's WAYLAND_DISPLAY before we overwrite it with our own
    // socket, so the activation-env import (below) can be undone on exit. Only
    // meaningful in the nested winit session.
    let host_wayland_display = std::env::var_os("WAYLAND_DISPLAY");

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
    }

    if backend == Backend::Drm {
        // Warm up the portal stack (metis Settings backend + xdp) in the
        // background so the first GtkApplication launch doesn't cold-start it.
        // Must not block: the compositor needs to start rendering immediately.
        start_portal_stack(state.socket_name.to_string_lossy().into_owned());
        // First-party PolicyKit agent for pkexec prompts (Users, firewall, …).
        ensure_polkit_agent();
        start_polkit_agent_watchdog();
        update_standalone_activation_env(&state.socket_name.to_string_lossy());
    }

    // Opt-in dev convenience (set by `run-metis.sh --import-env`): point the user
    // D-Bus session bus and `systemd --user` activation environment at this
    // nested compositor, so D-Bus-activated and single-instance apps open inside
    // Metis instead of the host session. It is opt-in because, in a nested dev
    // session, this temporarily redirects activation for the whole logged-in
    // user; we restore the host value when the event loop exits. The standalone
    // DRM session owns the seat outright, so it never touches the host env here.
    let import_activation_env =
        backend == Backend::Winit && std::env::var_os("METIS_IMPORT_ACTIVATION_ENV").is_some();
    if import_activation_env {
        update_activation_environment(&state.socket_name.to_string_lossy());
        // Nested session owns activation — run our agent rather than fighting
        // the host DE's agent on a shared bus without import-env.
        ensure_polkit_agent();
        start_polkit_agent_watchdog();
        tracing::info!(
            wayland_display = ?state.socket_name,
            "imported nested WAYLAND_DISPLAY into D-Bus/systemd activation environment"
        );
    }

    let shell = if std::env::var("METIS_NO_SHELL").is_ok() {
        None
    } else {
        Some(std::env::var("METIS_SHELL_BIN").unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .and_then(|p| {
                    p.parent()
                        .map(|d| d.join("metis-shell").display().to_string())
                })
                .unwrap_or_else(|| "metis-shell".into())
        }))
    };

    let client = parse_client_command();
    state.queue_startup(shell, client);

    tracing::info!(
        socket = ?state.socket_name,
        keybind_mod = crate::keybinds::keybind_mod_label(&state.keybinds),
        "Metis compositor running — apps, layer-shell overlays, and notifications supported"
    );

    // Block until calloop sources fire (input, timers, Wayland). A short fixed
    // timeout here spun at ~1 kHz idle and burned a full CPU core for no work.
    event_loop.run(None, &mut state, |state| {
        state.flush_clients_if_pending();
    })?;

    // Standalone DRM session: release the GPU deterministically before the rest
    // of teardown so DRM master / KMS state is handed back cleanly (otherwise the
    // next session can fail to become master and the TTY is left on a black
    // framebuffer). Dropping `UdevState` drops the DrmDevice + libseat session.
    if state.is_drm_backend() {
        tracing::info!("releasing DRM devices and seat session");
        state.udev = None;
    }

    // Hand the user's D-Bus/systemd activation environment back to the host
    // compositor so later app launches don't keep targeting our dead socket.
    if import_activation_env {
        match host_wayland_display.as_deref().map(|v| v.to_string_lossy()) {
            Some(host) => {
                update_activation_environment(&host);
                tracing::info!(
                    wayland_display = %host,
                    "restored host WAYLAND_DISPLAY in activation environment"
                );
            }
            None => tracing::warn!(
                "no prior WAYLAND_DISPLAY to restore — activation env still points at the closed Metis socket"
            ),
        }
    }

    tracing::info!("Metis compositor event loop exited");
    Ok(())
}

/// Push session variables into the user D-Bus / systemd activation environment.
/// Failures are non-fatal (logged).
fn push_activation_environment(vars: &[&str]) {
    let mut cmd = std::process::Command::new("dbus-update-activation-environment");
    cmd.arg("--systemd");
    for var in vars {
        cmd.arg(var);
    }
    match cmd.status() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(
            code = ?status.code(),
            "dbus-update-activation-environment exited non-zero"
        ),
        Err(err) => tracing::warn!(
            %err,
            "could not run dbus-update-activation-environment (is dbus installed?)"
        ),
    }
}

/// Nested dev convenience: point activation at our Wayland socket.
fn update_activation_environment(display: &str) {
    push_activation_environment(&[&format!("WAYLAND_DISPLAY={display}")]);
}

/// Standalone DRM session: publish Wayland + GTK hardening vars so D-Bus-activated
/// apps land in this session. Portal Settings are served by `metis-portal`.
fn update_standalone_activation_env(display: &str) {
    unsafe {
        std::env::set_var("GTK_A11Y", "none");
        std::env::set_var("NO_AT_BRIDGE", "1");
    }
    let gsk_renderer = std::env::var("GSK_RENDERER").unwrap_or_else(|_| "cairo".into());
    let profile = metis_config::load_graphics_profile();
    let compat = metis_config::effective_graphics_compatibility(profile);
    let profile_label = metis_config::effective_graphics_profile_label(profile);
    let theme_mode = metis_config::load_theme_preference().unwrap_or(metis_config::ThemeMode::Dark);
    let mut vars = vec![
        format!("WAYLAND_DISPLAY={display}"),
        "GTK_A11Y=none".into(),
        "NO_AT_BRIDGE=1".into(),
        "GDK_BACKEND=wayland".into(),
        format!("METIS_SHELL_GSK_RENDERER={gsk_renderer}"),
        format!("METIS_GRAPHICS_PROFILE={profile_label}"),
        "XDG_CURRENT_DESKTOP=Metis:GNOME".into(),
        "XDG_SESSION_DESKTOP=metis".into(),
    ];
    // Compatibility: push Cairo for D-Bus-activated GTK apps. Normal mode leaves
    // GSK unset so apps can use GL.
    if compat {
        vars.push("GSK_RENDERER=cairo".into());
    }
    if let Some(gtk_theme) = metis_config::appearance_gtk_theme_env(theme_mode) {
        vars.push(format!("GTK_THEME={gtk_theme}"));
    }
    if let Ok(session_id) = std::env::var("XDG_SESSION_ID")
        && !session_id.is_empty()
    {
        vars.push(format!("XDG_SESSION_ID={session_id}"));
    }
    let refs: Vec<&str> = vars.iter().map(String::as_str).collect();
    push_activation_environment(&refs);
}

/// Start the Metis Settings portal backend, then the xdp front-end.
///
/// This runs on a detached thread: the daemon spawns are quick, but waiting for
/// each D-Bus name to appear can take seconds, and the compositor must never
/// block its event loop on that (doing so leaves the screen black until the
/// portal stack settles). Apps that need portals launch later anyway, by which
/// point this background warm-up has finished.
fn start_portal_stack(wayland_display: String) {
    std::thread::Builder::new()
        .name("metis-portal-warmup".into())
        .spawn(move || {
            spawn_metis_portal(&wayland_display);
            if !wait_for_session_bus_name(
                "org.freedesktop.impl.portal.desktop.metis",
                std::time::Duration::from_secs(5),
            ) {
                tracing::warn!(
                    "metis-portal did not claim its D-Bus name — xdg-desktop-portal Settings may be unavailable"
                );
            }
            prewarm_desktop_portal();
            prewarm_portal_gtk();
            if wait_for_session_bus_name(
                "org.freedesktop.portal.Desktop",
                std::time::Duration::from_secs(12),
            ) {
                tracing::info!("xdg-desktop-portal is ready on the session bus");
            } else {
                tracing::warn!(
                    "xdg-desktop-portal did not become ready — GTK/Chromium apps may block ~25s on first launch"
                );
            }
            start_portal_watchdog(wayland_display);
        })
        .expect("spawn portal warm-up thread");
}

fn start_portal_watchdog(wayland_display: String) {
    std::thread::Builder::new()
        .name("metis-portal-watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(8));
                if std::env::var_os("METIS_NO_PORTAL").is_some() {
                    continue;
                }
                let portal_ok =
                    session_bus_name_active("org.freedesktop.impl.portal.desktop.metis");
                let screencast_ok = session_bus_name_active("org.gnome.Mutter.ScreenCast");
                if portal_ok && screencast_ok {
                    continue;
                }
                tracing::warn!(
                    portal_ok,
                    screencast_ok,
                    "metis-portal D-Bus services missing — respawning"
                );
                spawn_metis_portal(&wayland_display);
            }
        })
        .expect("spawn portal watchdog thread");
}

fn spawn_metis_portal(wayland_display: &str) {
    if std::env::var_os("METIS_NO_PORTAL").is_some() {
        return;
    }
    let portal_bin = std::env::var("METIS_PORTAL_BIN").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent()
                    .map(|d| d.join("metis-portal").display().to_string())
            })
            .unwrap_or_else(|| "metis-portal".into())
    });
    let log_path = metis_protocol::runtime_dir().join("portal.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);
    let mut cmd = std::process::Command::new(&portal_bin);
    cmd.env("WAYLAND_DISPLAY", wayland_display);
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        cmd.env("XDG_RUNTIME_DIR", runtime);
    }
    if let Ok(bus) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        cmd.env("DBUS_SESSION_BUS_ADDRESS", bus);
    }
    if std::env::var_os("RUST_LOG").is_none() {
        cmd.env("RUST_LOG", "info");
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    match log_file {
        Ok(file) => {
            cmd.stderr(std::process::Stdio::from(file));
        }
        Err(err) => {
            tracing::warn!(%err, path = %log_path.display(), "portal log file unavailable");
            cmd.stderr(std::process::Stdio::null());
        }
    }
    match cmd.spawn() {
        Ok(mut child) => {
            tracing::info!(
                pid = child.id(),
                wayland_display,
                log = %log_path.display(),
                "started metis-portal backend (Settings, Screenshot, ScreenCast)"
            );
            std::thread::Builder::new()
                .name("metis-portal-reaper".into())
                .spawn(move || match child.wait() {
                    Ok(status) => tracing::warn!(?status, "metis-portal exited"),
                    Err(err) => tracing::warn!(%err, "metis-portal wait failed"),
                })
                .ok();
        }
        Err(err) => tracing::warn!(
            %err,
            portal = %portal_bin,
            "metis-portal unavailable — install metis-portal or set METIS_NO_PORTAL=1"
        ),
    }
}

/// Resolve a portal helper installed under `/usr/libexec` (not always on PATH).
fn resolve_libexec_binary(name: &str) -> Option<std::path::PathBuf> {
    let candidates = [
        std::path::PathBuf::from(format!("/usr/libexec/{name}")),
        std::path::PathBuf::from(name),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn session_bus_name_active(name: &str) -> bool {
    std::process::Command::new("busctl")
        .args(["--user", "status", name])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn wait_for_session_bus_name(name: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if session_bus_name_active(name) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// `XDG_CURRENT_DESKTOP` for the portal daemons, with any `GNOME`/other suffix
/// stripped down to just `Metis`.
///
/// The session keeps `XDG_CURRENT_DESKTOP=Metis:GNOME` so Chromium/Electron apps
/// auto-select the gnome-libsecret keyring backend. But xdg-desktop-portal uses
/// the deprecated `UseIn` key to pick a backend for any interface its configured
/// backends (gtk/metis) don't implement — Screenshot, ScreenCast, Wallpaper,
/// Background, etc. With `GNOME` present it selects the GNOME portal backend and
/// then blocks ~25s *per interface* trying to D-Bus-activate it (gnome-shell is
/// not running), so xdp never claims `org.freedesktop.portal.Desktop` and every
/// GTK/Chromium launch stalls. Stripping the suffix makes those interfaces
/// resolve to "no backend" instead of hanging, while config-based selection
/// (gtk/metis/gnome-keyring) is unaffected.
fn portal_current_desktop() -> String {
    let current = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    match current.split_once(':') {
        Some((first, _)) if !first.is_empty() => first.to_string(),
        _ if current.is_empty() => "Metis".to_string(),
        _ => current,
    }
}

fn spawn_portal_daemon(name: &str) -> bool {
    let Some(bin) = resolve_libexec_binary(name) else {
        tracing::warn!(daemon = name, "portal binary not found under /usr/libexec");
        return false;
    };
    if session_bus_name_active(match name {
        "xdg-desktop-portal" => "org.freedesktop.portal.Desktop",
        "xdg-desktop-portal-gtk" => "org.freedesktop.impl.portal.desktop.gtk",
        _ => return false,
    }) {
        return true;
    }
    let theme_mode = metis_config::load_theme_preference().unwrap_or(metis_config::ThemeMode::Dark);
    let mut cmd = std::process::Command::new(&bin);
    cmd.env("XDG_CURRENT_DESKTOP", portal_current_desktop())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // FileChooser / OpenURI live in portal-gtk; without GTK_THEME it often
    // paints light Adwaita even when Metis is dark.
    if name == "xdg-desktop-portal-gtk" {
        match metis_config::appearance_gtk_theme_env(theme_mode) {
            Some(gtk_theme) => {
                cmd.env("GTK_THEME", gtk_theme);
            }
            None => {
                cmd.env_remove("GTK_THEME");
            }
        }
    }
    match cmd.spawn() {
        Ok(_) => {
            tracing::info!(daemon = name, path = %bin.display(), "started portal daemon");
            true
        }
        Err(err) => {
            tracing::warn!(%err, daemon = name, path = %bin.display(), "failed to start portal daemon");
            false
        }
    }
}

/// Best-effort: start xdg-desktop-portal once our Wayland socket exists so the
/// first GtkApplication launch does not cold-start the whole portal stack.
fn prewarm_desktop_portal() {
    if std::env::var_os("METIS_NO_PORTAL_PREWARM").is_some() {
        return;
    }
    let _ = spawn_portal_daemon("xdg-desktop-portal");
}

/// GTK apps block on the FileChooser/OpenURI portal during startup unless the
/// gtk backend is already running. Pre-start it alongside the main portal.
fn prewarm_portal_gtk() {
    if std::env::var_os("METIS_NO_PORTAL_PREWARM").is_some() {
        return;
    }
    let _ = spawn_portal_daemon("xdg-desktop-portal-gtk");
}

/// Start the Metis PolicyKit authentication agent when missing.
///
/// `pkexec` from Settings needs an agent to show the password dialog. Metis
/// ships `metis-polkit-agent` so we do not depend on GNOME/KDE agents.
fn ensure_polkit_agent() {
    if std::env::var_os("METIS_NO_POLKIT_AGENT").is_some() {
        return;
    }
    if metis_polkit_agent_running() {
        return;
    }
    // Own the session: stop third-party agents so RegisterAuthenticationAgent
    // succeeds for metis-polkit-agent.
    stop_third_party_polkit_agents();
    let Some(bin) = resolve_metis_polkit_agent() else {
        tracing::warn!(
            "metis-polkit-agent not found — pkexec prompts will fail until it is installed"
        );
        return;
    };
    let mut cmd = std::process::Command::new(&bin);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env("GTK_A11Y", "none")
        .env("NO_AT_BRIDGE", "1")
        .env("GSK_RENDERER", "cairo");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(_) => tracing::info!(path = %bin.display(), "started metis-polkit-agent"),
        Err(err) => {
            tracing::warn!(%err, path = %bin.display(), "failed to start metis-polkit-agent")
        }
    }
}

fn stop_third_party_polkit_agents() {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        let exe = exe.to_string_lossy();
        if exe.contains("polkit-gnome-authentication-agent")
            || exe.contains("polkit-kde-authentication-agent")
            || exe.contains("polkit-mate-authentication-agent")
            || exe.contains("lxqt-policykit-agent")
        {
            tracing::info!(%pid, path = %exe, "stopping third-party PolicyKit agent");
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn start_polkit_agent_watchdog() {
    if std::env::var_os("METIS_NO_POLKIT_AGENT").is_some() {
        return;
    }
    std::thread::Builder::new()
        .name("metis-polkit-watchdog".into())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if std::env::var_os("METIS_NO_POLKIT_AGENT").is_some() {
                    continue;
                }
                if metis_polkit_agent_running() {
                    continue;
                }
                tracing::warn!("metis-polkit-agent missing — respawning");
                ensure_polkit_agent();
            }
        })
        .expect("spawn polkit agent watchdog");
}

fn metis_polkit_agent_running() -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        if exe
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "metis-polkit-agent")
        {
            return true;
        }
    }
    false
}

fn resolve_metis_polkit_agent() -> Option<std::path::PathBuf> {
    if let Ok(override_bin) = std::env::var("METIS_POLKIT_AGENT_BIN") {
        let p = std::path::PathBuf::from(override_bin);
        if p.is_file() {
            return Some(p);
        }
    }
    const INSTALLED: &[&str] = &[
        "/usr/libexec/metis-polkit-agent",
        "/usr/bin/metis-polkit-agent",
        "/usr/local/bin/metis-polkit-agent",
    ];
    for path in INSTALLED {
        let p = std::path::Path::new(path);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    std::env::current_exe().ok().and_then(|exe| {
        let sibling = exe.parent()?.join("metis-polkit-agent");
        sibling.is_file().then_some(sibling)
    })
}

fn parse_client_command() -> Option<String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("-c" | "--command"), Some(command)) => Some(command),
        _ => None,
    }
}
