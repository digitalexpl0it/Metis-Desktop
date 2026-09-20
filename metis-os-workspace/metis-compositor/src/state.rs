use std::ffi::OsString;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use metis_grid::{
    app_tile_body_rect, cell_to_pixels, GridLayout, GridMetrics, MonitorRect, PixelRect, TileKind,
    TileModeState,
};
use metis_protocol::CompositorCommand;
use smithay::{
    desktop::{layer_map_for_output, PopupManager, Space, Window},
    input::{Seat, SeatState},
    reexports::{
        calloop::{
            generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        output::OutputManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        text_input::TextInputManagerState,
    },
    xwayland::X11Surface,
};

use crate::events::accept_event_subscribers;
use crate::events::EventBus;
use crate::focus::KeyboardFocusTarget;
use crate::windows::WindowRegistry;

/// Queued compositor launch waiting on the gaming XWayland bucket (Phase 18 D).
#[derive(Debug, Clone)]
pub(crate) struct PendingGamingLaunch {
    pub argv: Vec<String>,
    pub extra_env: Vec<(String, String)>,
}

/// Legacy default for bar-adjacent padding; live maximize/snap gaps come from
/// [`MetisState::configured_window_gap`] / `bar.json` `window_gap_px`.
#[allow(dead_code)]
pub const BAR_GAP_PX: i32 = 2;

/// Default Hyprland-style gap around maximized windows when `bar.json` has not
/// set `window_gap_px` yet. Live value comes from config (0..=10).
pub const WINDOW_GAP_PX: i32 = 8;

/// Per-edge gaps for placing/snapping windows inside the usable zone. All edges
/// use the configured `window_gap_px` (see [`MetisState::zone_edge_gaps`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ZoneGaps {
    pub(crate) top: i32,
    pub(crate) bottom: i32,
    pub(crate) left: i32,
    pub(crate) right: i32,
}

/// Minimum slice of a window that must remain on-screen. Used both to clamp
/// dragging (a window may slide off the left/right/bottom edges, but this much
/// stays reachable) and to decide when an off-screen window needs rescuing.
pub const MIN_VISIBLE_PX: i32 = 64;

/// How far *outside* a window edge the invisible resize grab band reaches (into
/// the gap/border around the window). Corners are where two bands overlap.
pub const RESIZE_MARGIN_PX: i32 = 12;
/// How far *inside* the client edge the resize band reaches. Keep this thin so
/// edge-hugging scrollbars (Chromium, etc.) stay clickable; most of the grab
/// affordance lives in [`RESIZE_MARGIN_PX`] outside the frame.
pub const RESIZE_INNER_PX: i32 = 3;

/// Apps that open as a centered floating window by default (rather than being
/// snapped into the tiling grid).
const CENTERED_FLOAT_APP_IDS: &[&str] = &["com.metis.Settings"];

/// Window titles that default to a centered floating window. Title fallback for
/// when GTK sets the Wayland app_id late (or not at all).
const CENTERED_FLOAT_TITLES: &[&str] = &["Metis Settings"];

/// Default size for a centered floating app when nothing is saved yet.
const DEFAULT_FLOAT_W: i32 = 900;
const DEFAULT_FLOAT_H: i32 = 660;
/// Splash / dialog floors: ignore saved geometry shorter than this so LibreOffice
/// `soffice` splash (~580×180) does not reopen Calc/Writer as a tiny strip.
const MIN_USABLE_SAVED_W: i32 = 480;
const MIN_USABLE_SAVED_H: i32 = 320;

fn saved_size_is_usable(width: i32, height: i32) -> bool {
    width >= MIN_USABLE_SAVED_W && height >= MIN_USABLE_SAVED_H
}

fn title_looks_like_splash(title: &str) -> bool {
    let t = title.trim().to_ascii_lowercase();
    t.contains("splash") || t.starts_with("frmce")
}

/// Near-monitor size that should become true fullscreen (flush to output origin).
pub(crate) fn x11_borderless_fullscreen_intent(
    output: Rectangle<i32, Logical>,
    w: i32,
    h: i32,
    undecorated: bool,
) -> bool {
    if w <= 0 || h <= 0 || output.size.w <= 0 || output.size.h <= 0 {
        return false;
    }
    if w >= output.size.w && h >= output.size.h {
        return true;
    }
    let ow = output.size.w as f32;
    let oh = output.size.h as f32;
    let fw = w as f32 / ow;
    let fh = h as f32 / oh;
    if undecorated {
        // Borderless games: ~85%+ of the panel, or one axis fills with the other
        // still substantial (letterboxed / ultrawide).
        (fw >= 0.85 && fh >= 0.85) || (fw >= 0.95 && fh >= 0.50) || (fh >= 0.95 && fw >= 0.50)
    } else {
        fw >= 0.97 && fh >= 0.97
    }
}

/// Large enough undecorated surface that a stale top-left would clip off-screen
/// when the client grows — re-center on the output (keep client size).
pub(crate) fn x11_large_undecorated_float(output: Rectangle<i32, Logical>, w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 || output.size.w <= 0 || output.size.h <= 0 {
        return false;
    }
    let fw = w as f32 / output.size.w as f32;
    let fh = h as f32 / output.size.h as f32;
    fw >= 0.45 && fh >= 0.45
}

/// Steam / Lutris / Heroic splash & main windows — must NOT get game borderless
/// auto-fullscreen or resize-loop re-centering (they animate size while loading).
pub(crate) fn x11_is_game_launcher(app_id: Option<&str>) -> bool {
    let Some(raw) = app_id.filter(|s| !s.is_empty()) else {
        return false;
    };
    let id = raw.to_ascii_lowercase();
    // Actual games — never treat as the store/launcher.
    if id.starts_with("steam_app_")
        || id.contains(".exe")
        || id.contains("proton")
        || id == "hytaleclient"
    {
        return false;
    }
    id == "steam"
        || id.starts_with("steam.")
        || id.contains("steamwebhelper")
        || id.contains("gamepadui")
        || id.contains("lutris")
        || id.contains("heroic")
        || id.contains("bottles")
        || id.contains("com.valvesoftware.steam")
}

#[cfg(test)]
mod borderless_intent_tests {
    use super::*;

    fn output(w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((0, 0)), Size::from((w, h)))
    }

    #[test]
    fn exact_monitor_size_matches() {
        assert!(x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1920,
            1080,
            false
        ));
    }

    #[test]
    fn undecorated_near_full_matches() {
        assert!(x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1728,
            972,
            true
        ));
    }

    #[test]
    fn undecorated_half_is_large_float() {
        assert!(!x11_borderless_fullscreen_intent(
            output(1920, 1080),
            1280,
            720,
            true
        ));
        assert!(x11_large_undecorated_float(output(1920, 1080), 1280, 720));
    }

    #[test]
    fn small_dialog_does_not_match() {
        assert!(!x11_borderless_fullscreen_intent(
            output(1920, 1080),
            640,
            480,
            true
        ));
        assert!(!x11_large_undecorated_float(output(1920, 1080), 640, 480));
    }

    #[test]
    fn steam_is_launcher_but_steam_app_is_not() {
        assert!(x11_is_game_launcher(Some("steam")));
        assert!(x11_is_game_launcher(Some("Steam")));
        assert!(!x11_is_game_launcher(Some("steam_app_12345")));
        assert!(!x11_is_game_launcher(Some("hl2.exe")));
    }
}

/// Per-output desktop state. Each output (monitor) owns an independent set of
/// virtual workspaces: its visible grid (`layout`), which workspace is showing
/// (`active_workspace`), and the hidden workspaces' app tiles (`stashed_app_tiles`).
/// Desk widget tiles (clock/weather/…) only exist on the primary output's desk.
pub struct OutputDesk {
    pub layout: GridLayout,
    /// Currently visible virtual workspace on this output (1-based).
    pub active_workspace: u32,
    /// App tiles for this output's hidden workspaces, keyed by workspace id.
    pub stashed_app_tiles: std::collections::HashMap<u32, Vec<metis_grid::GridTile>>,
    /// Per-workspace layout mode (grid vs. scroll). Absent entries fall back to the
    /// configured default; the grid tiles above remain the membership source of
    /// truth in either mode.
    pub layout_kind: std::collections::HashMap<u32, metis_grid::LayoutKind>,
    /// Per-workspace scrolling-strip arrangement, used when that workspace's
    /// `layout_kind` is `Scroll`.
    pub scroll: std::collections::HashMap<u32, metis_grid::ScrollState>,
}

pub struct MetisState {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    /// Event-loop handle for scheduling timers (idle blank, etc.). `'static` —
    /// the loop outlives the state.
    pub loop_handle: LoopHandle<'static, MetisState>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub _output_manager_state: OutputManagerState,
    pub seat_state: SeatState<MetisState>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub popups: PopupManager,

    pub seat: Seat<MetisState>,

    /// XWayland shell protocol state (xwayland-surface association). Always
    /// present; the X11 window manager (`xwm`) only exists once XWayland is up.
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    /// Live X11 window manager, populated when the XWayland server signals ready.
    pub xwm: Option<smithay::xwayland::X11Wm>,
    /// Optional second X11Wm for the gaming bucket (`xwayland_mode: isolated`).
    pub xwm_gaming: Option<smithay::xwayland::X11Wm>,
    /// X11 display number (e.g. `0` → `:0`) for the running XWayland server, used
    /// to set `DISPLAY` on X11 child processes.
    pub xdisplay: Option<u32>,
    /// DISPLAY number for the gaming XWayland bucket (isolated mode).
    pub xdisplay_gaming: Option<u32>,
    /// XwmId of the gaming bucket (for `xwm_state` routing).
    pub xwm_gaming_id: Option<smithay::xwayland::xwm::XwmId>,
    /// Gaming XWayland spawn in flight (Phase 18 D lazy bucket).
    pub(crate) xwayland_gaming_spawn_pending: bool,
    /// Launches waiting for the gaming XWayland to become ready.
    pub(crate) pending_gaming_launches: Vec<PendingGamingLaunch>,
    /// Pre-fullscreen geometry for mapped X11 windows (keyed by X11 window id).
    pub(crate) x11_fullscreen_restore: std::collections::HashMap<u32, Rectangle<i32, Logical>>,
    /// Per-output set of window ids currently in true-fullscreen (Wayland + X11);
    /// the edge bar hides while any client is fullscreen on that output. Tracking
    /// the concrete window ids (rather than a bare refcount) keeps the bar's
    /// visibility a pure function of live state: a window's fullscreen mark is
    /// removed unconditionally on teardown (see `drop_window_fullscreen`), so a
    /// missed decrement can never strand the bar hidden — which previously forced
    /// a shell restart after a game/launcher exited while fullscreen.
    pub(crate) output_fullscreen_windows:
        std::collections::HashMap<String, std::collections::HashSet<u32>>,
    /// Window ids for which we've already logged a "fullscreen not flush at
    /// output origin" diagnostic, so the render loop emits it at most once per
    /// fullscreen session instead of every frame. Cleared on teardown /
    /// un-fullscreen via `drop_window_fullscreen`.
    pub(crate) fs_offset_warned: std::collections::HashSet<u32>,

    pub windows: WindowRegistry,
    /// Windows the user has manually dragged out of the grid by their titlebar.
    /// They keep their free position (no grid snap-back) until closed.
    pub floating: std::collections::HashSet<u32>,
    /// Windows whose top edge meets the edge bar (maximized, or snapped left /
    /// right / top-corner), plus grid-tiled SSD windows: their titlebar auto-hides
    /// and re-appears as a translucent overlay on the client's top strip.
    pub auto_hide_titlebar: std::collections::HashSet<u32>,
    /// The auto-hide window whose titlebar is currently revealed (pointer in its
    /// top strip), or `None`. Drives both rendering and decoration clicks.
    pub revealed_titlebar: Option<u32>,
    /// Window id whose auto-hide titlebar overlay is sliding in/out (0..1 progress).
    pub titlebar_reveal_window: Option<u32>,
    /// Slide progress for [`Self::titlebar_reveal_window`]: 0 = hidden above the
    /// client, 1 = fully shown over its top strip.
    pub titlebar_reveal_progress: f32,
    last_titlebar_reveal_tick: Option<std::time::Instant>,
    /// Maximize ripple/wobble FX start times keyed by window id.
    maximize_fx_started: std::collections::HashMap<u32, std::time::Instant>,
    /// Last titlebar primary-button press for double-click maximize toggle.
    titlebar_last_click: Option<(u32, std::time::Instant)>,
    /// Maximized titlebar press waiting for drag threshold (no grab until then).
    titlebar_press_pending: Option<(u32, Point<f64, Logical>, smithay::utils::Serial)>,
    /// Minimize genie animations keyed by window id (window still mapped until done).
    minimize_genie_fx: std::collections::HashMap<u32, crate::window_fx::MinimizeGenieFx>,
    /// XWayland windows that have been unmapped by their client, pending a debounce
    /// before we treat it as a real withdraw ("close to tray"). Electron apps
    /// (Claude Desktop) unmap/remap constantly during normal operation, so reacting
    /// to every unmap would thrash the dock and flicker the window; we only tear the
    /// dock entry down if the window stays unmapped past the grace period. Keyed by
    /// window id → the instant the unmap was observed.
    x11_pending_withdraw: std::collections::HashMap<u32, std::time::Instant>,
    /// Persisted per-app floating geometry, so apps reopen where they were left.
    pub window_state: crate::window_state::WindowStateStore,

    /// Per-output desktops, keyed by output name. Created lazily as outputs map;
    /// the first (primary) output's desk is seeded from `desk.json` (widgets),
    /// secondary outputs get an app-only grid. See `OutputDesk`.
    pub desks: std::collections::HashMap<String, OutputDesk>,
    /// Baseline grid (columns/rows + widget tiles) loaded from `desk.json`, used to
    /// seed the primary output's desk and to size secondary (app-only) desks.
    pub default_layout: GridLayout,
    pub gutter_px: u32,
    pub tile_modes: TileModeState,
    pub monitor: MonitorRect,
    pub ipc_listener: Option<std::os::unix::net::UnixListener>,
    pub events_listener: Option<std::os::unix::net::UnixListener>,
    pub event_bus: EventBus,
    /// Sliding-window budget for command IPC (Phase 18 B).
    pub ipc_rate_limit: metis_protocol::SlidingWindow,
    /// Sliding-window budget for event-socket subscribe accepts.
    pub event_subscribe_rate_limit: metis_protocol::SlidingWindow,
    /// Skip clipboard history capture while the shell is setting the selection.
    pub clipboard_capture_suppressed: u32,
    /// Mimes from the latest client `SetSelection`, read on the next dispatch tick.
    pub(crate) pending_clipboard_mimes: Option<Vec<String>>,
    pub(crate) pending_clipboard_reads: Vec<crate::clipboard::PendingClipboardRead>,

    /// Spawn shell/client after the compositor is accepting connections.
    pub startup_shell: Option<String>,
    pub startup_client: Option<String>,
    pub startup_frames: u32,
    pub shell_spawned: bool,
    pub client_spawned: bool,
    /// One-shot: startup.json apps have been queued (or skipped) this session.
    pub startup_apps_spawned: bool,
    /// Deferred argv launches from `startup.json` (deadline, argv).
    pub startup_apps_queue: Vec<(std::time::Instant, Vec<String>)>,
    /// Isolated desktop-widgets process (`metis-shell --desktop-widgets`).
    pub widgets_cmd: Option<String>,
    pub widgets_pid: Option<u32>,
    pub widgets_last_spawn: Option<std::time::Instant>,
    /// Shared secret for the widgets process IPC scope (Phase 15 §D).
    pub widgets_ipc_token: Option<String>,
    pub child_processes: Vec<std::process::Child>,

    pub cursor_status: smithay::input::pointer::CursorImageStatus,
    /// Resize edge currently under the pointer (drives the host cursor shape).
    /// `None` when the pointer isn't hovering a window's resize band.
    pub hover_cursor: Option<crate::grabs::ResizeEdge>,
    /// Last app window the user brought forward (taskbar, Alt+Tab, etc.). Kept
    /// when keyboard focus moves to the edge bar so bulk layout sync does not
    /// re-raise a maximized window over the app the user just picked.
    last_focused_window: Option<u32>,
    /// Window ids waiting for a GL thumbnail render (`window_thumb`).
    pub(crate) pending_window_thumbs: std::collections::VecDeque<u32>,
    /// `(output, workspace)` pairs waiting for Task View shelf thumbnails.
    pub(crate) pending_workspace_thumbs: std::collections::VecDeque<(String, u32)>,
    /// Screenshot / screencast overlay windows elevated above ordinary clients.
    pub(crate) capture_overlay: crate::capture_overlay::CaptureOverlaySession,
    pub(crate) screenshot_overlay: crate::screenshot_overlay::ScreenshotOverlaySession,
    /// Active snap-zone preview while a window is being dragged by its titlebar:
    /// the target rect (already inset) plus a short label. `None` when no drag is
    /// in progress or the pointer isn't over a snap band. Drives both the live
    /// overlay and where the window lands on drop.
    pub snap_preview: Option<(PixelRect, &'static str)>,

    pub wallpaper: crate::wallpaper::Wallpaper,
    pub blur: crate::blur::BlurRuntime,
    pub hdr_encode: crate::hdr_encode::HdrEncodeRuntime,
    /// Wave 3c: any mapped client advertised PQ/HLG via colour management.
    pub(crate) hdr_client_content_visible: bool,
    pub color_lut: crate::color_lut::ColorLutRuntime,
    pub decorations: crate::decoration::DecorationRuntime,
    pub decoration_overrides: crate::decoration_overrides::DecorationsRuntime,
    pub input_runtime: crate::device_input::InputRuntime,
    pub keybinds: crate::keybinds::KeybindRuntime,
    /// Armed by a standalone Super press and cancelled by any chord key.
    /// Releasing while armed toggles the Metis application menu.
    pub(crate) super_tap_armed: bool,
    pub output_runtime: crate::output_prefs::OutputRuntime,

    redraw_trigger: Option<Rc<dyn Fn()>>,
    /// When true, the next winit Redraw performs GL compositing + layer frame delivery.
    pub damaged: bool,
    /// Defer `flush_clients` until after the winit redraw handler returns (avoids reentrancy).
    pub defer_client_flush: bool,
    /// One post-configure arrange after the bar commits its first real buffer.
    last_pointer_forward: Option<(std::time::Instant, Point<f64, Logical>)>,
    /// Last known edge-bar position; used to reflow windows immediately when the
    /// bar layer commits after a settings change (not only on the blur poll).
    pub(crate) last_bar_position: metis_config::BarPosition,
    /// Per-output: edge bar is visually auto-hidden (peek). Placement does not
    /// depend on this when `auto_hide` is enabled — the bar overlays windows.
    /// Still drives input peek-strip hit testing.
    pub(crate) bar_auto_hidden: std::collections::HashMap<String, bool>,
    /// Last seen `bar.json` `auto_hide` flag — reflow once when the setting flips.
    last_bar_auto_hide_enabled: bool,
    /// Debounce for `reveal-edge-bar` runtime commands (pointer hot-edge).
    last_bar_reveal_cmd: Option<std::time::Instant>,
    /// True while the pointer last sampled inside the bar strip (edge-trigger).
    bar_edge_pointer_in: bool,
    /// After `close-popovers`, ignore bar-edge IPC briefly so hover/leave cannot
    /// overwrite the dismiss command before the shell polls the command file.
    suppress_bar_edge_cmd_until: Option<std::time::Instant>,
    /// Last applied maximize/snap gap from `bar.json` (`window_gap_px`).
    last_window_gap_px: i32,
    /// Throttle for re-reading `window_gap_px` (~1s, same cadence as blur).
    last_window_gap_check: std::time::Instant,
    /// Last scroll-animation tick (16ms heartbeat).
    last_scroll_tick: Option<std::time::Instant>,
    /// Debounce grid/scroll toggle (`Mod+\`) so key-repeat cannot flip modes
    /// dozens of times per second and stall the compositor.
    last_layout_toggle: Option<std::time::Instant>,
    /// Debounce maximize toggle (`Mod+F`) so key-repeat cannot spam configure /
    /// wobble restarts and leave the window fighting the pointer (or stall the
    /// session).
    last_maximize_toggle: Option<std::time::Instant>,
    /// Resolved once at startup and reused for every spawned client — avoids
    /// blocking the event loop on `gsettings`/D-Bus during shell launch.
    client_cursor_theme: String,
    client_cursor_size: String,
    /// GPU steering env for spawned clients (DRM backend only; `None` under the
    /// nested winit session where the host compositor owns device selection).
    pub(crate) client_gpu: Option<ClientGpuHint>,
    /// PRIME render-offload steering for the discrete high-power GPU on a hybrid
    /// (Optimus) system. `Some` when a dGPU distinct from the display GPU exists;
    /// game/Steam launches are steered onto it instead of the weak iGPU.
    pub(crate) dgpu_offload: Option<DgpuOffload>,

    /// Monotonic clock for frame timing / cursor animation (shared by backends).
    pub clock: smithay::utils::Clock<smithay::utils::Monotonic>,
    /// Persistent identity + commit counter for the snap-zone overlay element so
    /// the damage tracker treats it as one stable element across frames.
    pub(crate) snap_overlay_id: smithay::backend::renderer::element::Id,
    pub(crate) snap_overlay_commit: smithay::backend::renderer::utils::CommitCounter,
    /// Solid desktop fill when wallpaper texture is not ready yet (splash / boot).
    pub(crate) desktop_underlay_id: smithay::backend::renderer::element::Id,
    pub(crate) desktop_underlay_commit: smithay::backend::renderer::utils::CommitCounter,
    pub(crate) night_light_id: smithay::backend::renderer::element::Id,
    pub(crate) night_light_commit: smithay::backend::renderer::utils::CommitCounter,
    /// Dim-on-battery overlay (`power.json` → `dim_on_battery`).
    pub(crate) battery_dim: crate::battery_dim::BatteryDimRuntime,
    /// Last computed night-light effective state when schedule gating is on.
    pub(crate) night_light_schedule_effective: Option<bool>,
    /// Deferred `outputs.json` apply so IPC replies return before layout/mirror work.
    pending_apply_outputs: bool,
    /// Coalesce rapid `ReloadOutputs` IPC (e.g. live night-light slider) into one reload.
    outputs_reload_due: Option<std::time::Instant>,
    pub(crate) last_snap_rect: Option<PixelRect>,
    /// DRM/udev backend state (session, GPUs, per-connector surfaces). `None` in
    /// the nested winit session.
    pub udev: Option<crate::udev::UdevState>,
    /// Client-visible logical outputs in the nested winit session (empty on DRM).
    pub winit_outputs: Vec<smithay::output::Output>,
    /// wl_output globals for winit logical outputs (DRM stores these per-surface).
    pub output_globals:
        std::collections::HashMap<String, smithay::reexports::wayland_server::backend::GlobalId>,
    /// Screen capture protocol state (ext-image-copy-capture).
    pub image_capture: crate::image_capture::ImageCaptureRuntime,
    pub(crate) color_mgmt: crate::color_management::ColorManagementRuntime,
    /// Idle detection + screen-blank (DPMS) + inhibitor bookkeeping.
    pub(crate) idle: crate::idle::IdleManager,
    /// Compositor-rendered session lock (background/blur/dim + PAM auth).
    pub(crate) lock: crate::lock::LockState,
    /// `ext-session-lock-v1` global state (third-party lockers).
    pub(crate) session_lock_state: SessionLockManagerState,
    /// Active protocol locker (if any). Mutually exclusive with [`Self::lock`].
    pub(crate) protocol_lock: crate::session_lock::ProtocolLock,
    /// Gaming window rules (float / auto-fullscreen by app-id/class/title) so
    /// games and launchers escape the tiling grid. Loaded once at startup.
    pub(crate) game_rules: metis_config::GameRulesConfig,
    /// Phase 11 gaming preferences (`gaming.json`).
    pub(crate) gaming_config: metis_config::GamingConfig,
    /// Windows a game rule asked to fullscreen, awaiting readiness (fullscreen is
    /// applied once the client has committed a buffer and been placed).
    pub(crate) pending_game_fullscreen: std::collections::HashSet<u32>,
    /// `linux-drm-syncobj-v1` explicit-sync state. `Some` only when the primary
    /// GPU supports syncobj eventfd. Explicit sync removes implicit-sync stutter
    /// on NVIDIA + DXVK/VKD3D and modern XWayland — critical for Proton.
    pub(crate) drm_syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// `zwp_idle_inhibit_manager_v1` global (native apps keep the screen awake).
    pub idle_inhibit_state: IdleInhibitManagerState,
    /// `ext_idle_notify_v1` global (idle notifications for swayidle-style clients).
    pub idle_notifier_state: IdleNotifierState<MetisState>,
    /// Where a locked-pointer client last drew its own cursor. Used only to
    /// restore the system cursor when the lock is destroyed — never to remap
    /// clicks while locked (Proton streams hints continuously during mouse-look).
    pub(crate) cursor_position_hint: Option<(WlSurface, Point<f64, Logical>)>,
    /// Per-surface pointer-constraint lifecycle (NeverActivated / Active).
    pub(crate) pointer_constraint_phases: std::collections::HashMap<
        smithay::reexports::wayland_server::backend::ObjectId,
        PointerConstraintPhase,
    >,
    /// Last surface that received pointer motion (for enter detection / tracing).
    pub(crate) last_pointer_motion_surface:
        Option<smithay::reexports::wayland_server::backend::ObjectId>,
}

/// Lifecycle of a `zwp_pointer_constraints_v1` lock on a surface.
/// Matches Mutter/KWin: inactive constraints may be activated again while the
/// pointer remains over the surface. Pause menus destroy the constraint object
/// (see `remove_constraint`); we do not latch a permanent "deactivated" phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointerConstraintPhase {
    /// Created but not yet activated (waiting for pointer to enter the region).
    NeverActivated,
    /// Constraint is active (locked / confined).
    Active,
}

/// Cursor theme/size for nested clients. Never calls D-Bus — a synchronous
/// `gsettings` in `spawn_client` blocked the compositor event loop during shell
/// startup (especially after `--import-env`), which GNOME reported as
/// "Unknown is not responding".
fn resolve_client_cursor_env() -> (String, String) {
    fn cursor_icon_dirs() -> Vec<std::path::PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(std::path::PathBuf::from(format!("{home}/.icons")));
            dirs.push(std::path::PathBuf::from(format!(
                "{home}/.local/share/icons"
            )));
        }
        dirs.push(std::path::PathBuf::from("/usr/share/icons"));
        dirs.push(std::path::PathBuf::from("/usr/local/share/icons"));
        dirs
    }
    fn resolve_cursor_theme(name: &str) -> Option<String> {
        fn inner(name: &str, dirs: &[std::path::PathBuf], depth: u8) -> Option<String> {
            if name.is_empty() || depth > 8 {
                return None;
            }
            if dirs.iter().any(|d| d.join(name).join("cursors").is_dir()) {
                return Some(name.to_string());
            }
            for d in dirs {
                let Ok(text) = std::fs::read_to_string(d.join(name).join("index.theme")) else {
                    continue;
                };
                for line in text.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Inherits") {
                        let rest = rest.trim_start_matches([' ', '=']).trim();
                        for parent in rest.split(',') {
                            if let Some(found) = inner(parent.trim(), dirs, depth + 1) {
                                return Some(found);
                            }
                        }
                    }
                }
            }
            None
        }
        inner(name, &cursor_icon_dirs(), 0)
    }
    fn gtk_settings_value(home: &str, key: &str) -> Option<String> {
        for rel in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            let path = std::path::Path::new(home).join(".config").join(rel);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') || !line.contains('=') {
                    continue;
                }
                let (k, v) = line.split_once('=')?;
                if k.trim() == key {
                    let v = v.trim().trim_matches('"');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
        None
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let theme_pref = std::env::var("XCURSOR_THEME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| gtk_settings_value(&home, "gtk-cursor-theme-name"))
        .unwrap_or_else(|| "default".into());
    let cursor_theme = resolve_cursor_theme(&theme_pref)
        .or_else(|| resolve_cursor_theme("default"))
        .or_else(|| resolve_cursor_theme("Yaru"))
        .or_else(|| resolve_cursor_theme("Adwaita"))
        .unwrap_or_else(|| "Adwaita".into());
    let cursor_size = std::env::var("XCURSOR_SIZE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| gtk_settings_value(&home, "gtk-cursor-theme-size"))
        .unwrap_or_else(|| "24".into());
    (cursor_theme, cursor_size)
}

/// GPU steering hint for spawned clients, derived from the render node the
/// compositor actually renders on (DRM/udev backend only). Exported so that
/// clients which do not negotiate a device over Wayland dmabuf feedback
/// (XWayland, Proton/Vulkan, native GL apps) default to the *same* GPU the
/// compositor uses — avoiding the "game picks the wrong card / black screen"
/// class of bugs on hybrid systems.
#[derive(Clone, Debug)]
pub(crate) struct ClientGpuHint {
    /// Mesa GL device selector, e.g. `pci-0000_03_00_0` (`DRI_PRIME`).
    dri_prime: Option<String>,
    /// Mesa Vulkan default-device selector, e.g. `1002:73df`
    /// (`MESA_VK_DEVICE_SELECT`).
    vk_select: Option<String>,
}

impl ClientGpuHint {
    /// Resolve the PCI identity of a DRM render node from sysfs. Returns `None`
    /// when the node has no PCI parent (e.g. virtual/vgem devices) or sysfs is
    /// unreadable, in which case no GPU env is exported.
    pub(crate) fn from_render_node(node: &smithay::backend::drm::DrmNode) -> Option<Self> {
        let base = format!("/sys/dev/char/{}:{}/device", node.major(), node.minor());
        // The PCI address is the basename of the resolved `device` symlink,
        // e.g. `/sys/.../0000:03:00.0` -> `0000:03:00.0`.
        let dri_prime = std::fs::canonicalize(&base).ok().and_then(|p| {
            p.file_name()
                .map(|f| format!("pci-{}", f.to_string_lossy().replace([':', '.'], "_")))
        });
        let read_hex = |name: &str| -> Option<String> {
            std::fs::read_to_string(format!("{base}/{name}"))
                .ok()
                .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
                .filter(|s| !s.is_empty())
        };
        let vk_select = match (read_hex("vendor"), read_hex("device")) {
            (Some(v), Some(d)) => Some(format!("{v}:{d}")),
            _ => None,
        };
        if dri_prime.is_none() && vk_select.is_none() {
            return None;
        }
        Some(Self {
            dri_prime,
            vk_select,
        })
    }

    /// Apply the hint to a spawned command, only for keys the surrounding
    /// environment has not already set (so Steam launch options such as
    /// `DRI_PRIME=1`, `prime-run`, or NVIDIA offload vars still win per game).
    fn apply(&self, cmd: &mut std::process::Command) {
        if let Some(tag) = &self.dri_prime {
            if std::env::var_os("DRI_PRIME").is_none() {
                cmd.env("DRI_PRIME", tag);
            }
        }
        if let Some(sel) = &self.vk_select {
            if std::env::var_os("MESA_VK_DEVICE_SELECT").is_none() {
                cmd.env("MESA_VK_DEVICE_SELECT", sel);
            }
        }
    }
}

/// PRIME render-offload steering for the discrete high-power GPU on a hybrid
/// (Optimus / muxless) laptop.
///
/// The compositor renders and scans out on the iGPU that owns the panel (see
/// `pick_primary_gpu`). `ClientGpuHint` then pins *every* spawned client to that
/// same iGPU — which is right for lightweight desktop apps but catastrophic for
/// games and Steam Big Picture: they get forced onto the weak integrated GPU
/// while the dGPU sits idle (the "Big Picture loads very slowly" report). This
/// steers game/launcher processes onto the dGPU instead; their buffers are
/// imported cross-GPU by the compositor for scanout (standard PRIME offload,
/// exactly what GNOME/KDE do for Wayland clients).
#[derive(Clone, Debug)]
pub(crate) enum DgpuOffload {
    /// Proprietary NVIDIA: offload via the NVIDIA GLX/Vulkan stack.
    Nvidia,
    /// A Mesa-driven dGPU (AMD / Intel Arc / Nouveau): select its render node.
    Mesa {
        dri_prime: String,
        vk_select: Option<String>,
    },
}

impl DgpuOffload {
    /// Detect a discrete GPU distinct from the compositor's display GPU by
    /// scanning `/sys/class/drm/card*`. Returns `None` on single-GPU systems.
    pub(crate) fn detect(display_node: &smithay::backend::drm::DrmNode) -> Option<Self> {
        let pci_addr_of_node = |node: &smithay::backend::drm::DrmNode| -> Option<String> {
            let base = format!("/sys/dev/char/{}:{}/device", node.major(), node.minor());
            std::fs::canonicalize(&base)
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        };
        let display_pci = pci_addr_of_node(display_node);

        for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Only `cardN` nodes; skip connectors like `card2-eDP-1`.
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = entry.path().join("device");
            // A real GPU exposes `boot_vga`; the discrete one has `boot_vga=0`.
            if !dev.join("boot_vga").exists() {
                continue;
            }
            let pci = std::fs::canonicalize(&dev)
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            // Skip the display GPU itself — we only want the *other* card.
            if pci.is_none() || pci == display_pci {
                continue;
            }
            let read_hex = |field: &str| -> Option<String> {
                std::fs::read_to_string(dev.join(field))
                    .ok()
                    .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
                    .filter(|s| !s.is_empty())
            };
            let vendor = read_hex("vendor");
            let device = read_hex("device");
            match vendor.as_deref() {
                // NVIDIA — but only if the proprietary stack is actually present;
                // with nouveau there is no `__NV_PRIME_RENDER_OFFLOAD`, so fall
                // through to the Mesa path instead.
                Some("10de") if std::path::Path::new("/proc/driver/nvidia").exists() => {
                    return Some(DgpuOffload::Nvidia);
                }
                Some(_) => {
                    let dri_prime = pci.map(|p| format!("pci-{}", p.replace([':', '.'], "_")))?;
                    let vk_select = match (vendor, device) {
                        (Some(v), Some(d)) => Some(format!("{v}:{d}")),
                        _ => None,
                    };
                    return Some(DgpuOffload::Mesa {
                        dri_prime,
                        vk_select,
                    });
                }
                None => continue,
            }
        }
        None
    }

    /// Apply the offload env to a spawned command, deferring to any value the
    /// surrounding environment (or a Steam per-game launch option) already set.
    fn apply(&self, cmd: &mut std::process::Command) {
        let set_if_unset = |cmd: &mut std::process::Command, key: &str, val: &str| {
            if std::env::var_os(key).is_none() {
                cmd.env(key, val);
            }
        };
        match self {
            DgpuOffload::Nvidia => {
                // Proprietary NVIDIA PRIME render offload: route GLX + Vulkan
                // (DXVK/VKD3D/Proton) to the NVIDIA GPU. Deliberately do NOT set
                // `DRI_PRIME`/`MESA_VK_DEVICE_SELECT` — those are Mesa-only and
                // would pin Vulkan back onto the iGPU, fighting the offload.
                set_if_unset(cmd, "__NV_PRIME_RENDER_OFFLOAD", "1");
                set_if_unset(cmd, "__GLX_VENDOR_LIBRARY_NAME", "nvidia");
                set_if_unset(cmd, "__VK_LAYER_NV_optimus", "NVIDIA_only");
            }
            DgpuOffload::Mesa {
                dri_prime,
                vk_select,
            } => {
                set_if_unset(cmd, "DRI_PRIME", dri_prime);
                if let Some(sel) = vk_select {
                    set_if_unset(cmd, "MESA_VK_DEVICE_SELECT", sel);
                }
            }
        }
    }
}

/// Environment shared by every client the compositor spawns (shell, settings,
/// menu launches). GTK hardening avoids portal/a11y stalls in a bare session.
#[allow(clippy::too_many_arguments)]
fn apply_spawned_client_env(
    cmd: &mut std::process::Command,
    program: &str,
    socket: &std::ffi::OsStr,
    xdisplay: Option<u32>,
    xdisplay_gaming: Option<u32>,
    client_gpu: Option<&ClientGpuHint>,
    dgpu_offload: Option<&DgpuOffload>,
    prefer_dgpu: bool,
    use_gaming_xwayland: bool,
) {
    cmd.env("WAYLAND_DISPLAY", socket);
    cmd.env("METIS_SESSION", "1");
    // Phase 18 D: gaming X11 class is independent of GPU offload / battery.
    // Browsers may still get dGPU without moving onto the gaming X server.
    let display = if use_gaming_xwayland {
        xdisplay_gaming.or(xdisplay)
    } else {
        xdisplay
    };
    match display {
        Some(n) => {
            tracing::debug!(
                program,
                display = n,
                gaming_x11 = use_gaming_xwayland,
                "spawn DISPLAY"
            );
            cmd.env("DISPLAY", format!(":{n}"));
        }
        None => {
            cmd.env_remove("DISPLAY");
        }
    }
    cmd.env("GDK_BACKEND", "wayland");
    let profile = metis_config::load_graphics_profile();
    let compat = metis_config::effective_graphics_compatibility(profile);
    cmd.env(
        "METIS_GRAPHICS_PROFILE",
        metis_config::effective_graphics_profile_label(profile),
    );
    // Drive GTK4 / libadwaita clients to Metis light/dark even when the Settings
    // portal is slow or unavailable (Nautilus, etc.).
    let theme_mode = metis_config::load_theme_preference().unwrap_or(metis_config::ThemeMode::Dark);
    match metis_config::appearance_gtk_theme_env(theme_mode) {
        Some(gtk_theme) => {
            cmd.env("GTK_THEME", gtk_theme);
        }
        None => {
            cmd.env_remove("GTK_THEME");
        }
    }
    // Shell always prefers Cairo for layer-shell stability unless overridden.
    // Compatibility mode also forces Cairo for every other GTK client (VMs).
    // Settings: GTK 4.22+ defaults to GskVulkanRenderer, which hitch-scrolls on
    // hybrid NVIDIA under Metis (pause then catch-up). Pin GL unless overridden;
    // do not inherit the session's shell Cairo `GSK_RENDERER`.
    if program.contains("metis-shell") {
        let renderer = std::env::var("METIS_SHELL_GSK_RENDERER")
            .or_else(|_| std::env::var("GSK_RENDERER"))
            .unwrap_or_else(|_| "cairo".into());
        cmd.env("GSK_RENDERER", renderer);
    } else if program.contains("metis-settings") {
        if compat {
            cmd.env("GSK_RENDERER", "cairo");
        } else if let Ok(renderer) = std::env::var("METIS_SETTINGS_GSK_RENDERER") {
            cmd.env("GSK_RENDERER", renderer);
        } else {
            // Cairo: GL/Vulkan hitch-scroll for seconds on tall Settings pages
            // under hybrid NVIDIA. Users can opt back into gl via the env var.
            cmd.env("GSK_RENDERER", "cairo");
        }
    } else if compat {
        cmd.env("GSK_RENDERER", "cairo");
    } else {
        cmd.env_remove("GSK_RENDERER");
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        cmd.env("XDG_RUNTIME_DIR", runtime);
    }
    cmd.env("GTK_A11Y", "none");
    cmd.env("NO_AT_BRIDGE", "1");
    // Prefer native Wayland for Electron/Chromium apps. Metis is a Wayland
    // compositor and its XWayland path is a fallback; Electron's XWayland
    // map/unmap lifecycle is unstable here (Claude Desktop juggles windows on
    // launch and cleanly quits via `window-all-closed` — it "opens then closes").
    // `ELECTRON_OZONE_PLATFORM_HINT=auto` is the standard opt-in most Electron
    // apps honor; Claude Desktop's launcher force-passes `--ozone-platform=x11`
    // and only switches to Wayland via `CLAUDE_USE_WAYLAND=1`. Both defer to a
    // value the surrounding session already set, so a user can force XWayland.
    if std::env::var_os("ELECTRON_OZONE_PLATFORM_HINT").is_none() {
        cmd.env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
    }
    if std::env::var_os("CLAUDE_USE_WAYLAND").is_none() {
        cmd.env("CLAUDE_USE_WAYLAND", "1");
    }
    // Firefox: prefer native Wayland (WebGL / canvas compose on Metis, not XWayland).
    if std::env::var_os("MOZ_ENABLE_WAYLAND").is_none() {
        cmd.env("MOZ_ENABLE_WAYLAND", "1");
    }
    // Nested dev sessions run inside GNOME/KDE — disable GTK's portal proxy so
    // startup does not block on the host portal stack.
    if std::env::var_os("METIS_NESTED").is_some() {
        let gdk_debug = std::env::var("GDK_DEBUG").unwrap_or_default();
        if gdk_debug.is_empty() {
            cmd.env("GDK_DEBUG", "no-portals");
        } else if !gdk_debug
            .split(',')
            .any(|p| p == "no-portals" || p == "portals")
        {
            cmd.env("GDK_DEBUG", format!("{gdk_debug},no-portals"));
        } else {
            cmd.env("GDK_DEBUG", gdk_debug);
        }
    }
    // GPU steering (DRM backend only), unless the user opted out with
    // METIS_NO_CLIENT_GPU. Per-game overrides still win since every `apply` only
    // sets keys that are not already present.
    //   * Games / launchers (`prefer_dgpu`) are pushed onto the discrete GPU via
    //     PRIME render offload when one exists — otherwise the display-GPU hint
    //     is a harmless fallback (single-GPU systems).
    //   * Everything else stays on the compositor's (display) GPU, which is the
    //     power-efficient iGPU on a hybrid laptop.
    if std::env::var_os("METIS_NO_CLIENT_GPU").is_none() {
        match (prefer_dgpu, dgpu_offload) {
            (true, Some(dgpu)) => dgpu.apply(cmd),
            _ => {
                if let Some(hint) = client_gpu {
                    hint.apply(cmd);
                }
            }
        }
    }
}

pub use crate::ipc_dispatch::IpcCaps;

impl MetisState {
    pub fn new(
        event_loop: &mut EventLoop<'static, MetisState>,
        display: Display<MetisState>,
    ) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<MetisState>(&dh);
        let xdg_shell_state = XdgShellState::new::<MetisState>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<MetisState>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<MetisState>(&dh);
        let shm_state = ShmState::new::<MetisState>(&dh, vec![]);
        let popups = PopupManager::default();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<MetisState>(&dh);
        let data_device_state = DataDeviceState::new::<MetisState>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<MetisState>(&dh);
        let xwayland_shell_state =
            smithay::wayland::xwayland_shell::XWaylandShellState::new::<MetisState>(&dh);
        TextInputManagerState::new::<MetisState>(&dh);

        let mut seat_state = SeatState::<MetisState>::new();
        let mut seat = seat_state.new_wl_seat(&dh, "metis");
        let input_cfg = crate::device_input::InputRuntime::initial_keyboard_config();
        let kb = &input_cfg.keyboard;
        seat.add_keyboard(
            smithay::input::keyboard::XkbConfig {
                rules: "",
                model: "",
                layout: &kb.layout,
                variant: &kb.variant,
                options: kb.merged_xkb_options(),
            },
            kb.repeat_delay_ms,
            kb.repeat_rate_hz,
        )
        .expect("seat keyboard init failed (xkb)");
        seat.add_pointer();

        let space = Space::<Window>::default();
        let socket_name = Self::init_wayland_listener(display, event_loop);
        let loop_signal = event_loop.get_signal();
        let loop_handle = event_loop.handle();

        // Idle blank + inhibit: `zwp_idle_inhibit` (native apps) and
        // `ext_idle_notify` (swayidle-style) globals, plus the blank timer state
        // seeded from the saved power preference.
        let idle_inhibit_state = IdleInhibitManagerState::new::<MetisState>(&dh);
        let idle_notifier_state = IdleNotifierState::<MetisState>::new(&dh, loop_handle.clone());

        // Game input: `zwp_relative_pointer_v1` (raw mouse deltas) and
        // `zwp_pointer_constraints_v1` (pointer lock / confinement). Together they
        // let titles capture the mouse for camera "look" — without them apps like
        // Hytale receive clicks but never any look motion. The globals need no
        // stored handle; per-surface constraint data lives on the seat and the
        // dispatch glue comes from `delegate_dispatch2!`.
        smithay::wayland::relative_pointer::RelativePointerManagerState::new::<MetisState>(&dh);
        smithay::wayland::pointer_constraints::PointerConstraintsState::new::<MetisState>(&dh);
        let session_lock_state = SessionLockManagerState::new::<MetisState, _>(&dh, |_| true);
        let power_cfg = metis_config::load_power_config();
        let idle = crate::idle::IdleManager::new(power_cfg.blank_after_minutes);

        let desk_path = desk_config_path();
        let grid_layout = GridLayout::load_from_path(&desk_path);
        let mut grid_layout = grid_layout;
        metis_grid::sanitize_layout(&mut grid_layout);
        let (client_cursor_theme, client_cursor_size) = resolve_client_cursor_env();
        tracing::info!(
            theme = %client_cursor_theme,
            size = %client_cursor_size,
            "client cursor theme"
        );

        Self {
            start_time,
            socket_name,
            display_handle: dh.clone(),
            space,
            loop_signal,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            shm_state,
            _output_manager_state: output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            popups,
            seat,
            xwayland_shell_state,
            xwm: None,
            xwm_gaming: None,
            xdisplay: None,
            xdisplay_gaming: None,
            xwm_gaming_id: None,
            xwayland_gaming_spawn_pending: false,
            pending_gaming_launches: Vec::new(),
            x11_fullscreen_restore: std::collections::HashMap::new(),
            output_fullscreen_windows: std::collections::HashMap::new(),
            fs_offset_warned: std::collections::HashSet::new(),
            windows: WindowRegistry::new(),
            floating: std::collections::HashSet::new(),
            auto_hide_titlebar: std::collections::HashSet::new(),
            revealed_titlebar: None,
            titlebar_reveal_window: None,
            titlebar_reveal_progress: 0.0,
            last_titlebar_reveal_tick: None,
            maximize_fx_started: std::collections::HashMap::new(),
            titlebar_last_click: None,
            titlebar_press_pending: None,
            minimize_genie_fx: std::collections::HashMap::new(),
            x11_pending_withdraw: std::collections::HashMap::new(),
            window_state: crate::window_state::WindowStateStore::load(),
            desks: std::collections::HashMap::new(),
            default_layout: grid_layout,
            gutter_px: 14,
            tile_modes: TileModeState::default(),
            monitor: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            ipc_listener: None,
            events_listener: None,
            event_bus: EventBus::default(),
            ipc_rate_limit: metis_protocol::SlidingWindow::ipc_requests(),
            event_subscribe_rate_limit: metis_protocol::SlidingWindow::event_subscribes(),
            clipboard_capture_suppressed: 0,
            pending_clipboard_mimes: None,
            pending_clipboard_reads: Vec::new(),
            startup_shell: None,
            startup_client: None,
            startup_frames: 0,
            shell_spawned: false,
            client_spawned: false,
            startup_apps_spawned: false,
            startup_apps_queue: Vec::new(),
            widgets_cmd: None,
            widgets_pid: None,
            widgets_last_spawn: None,
            widgets_ipc_token: None,
            child_processes: Vec::new(),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            hover_cursor: None,
            last_focused_window: None,
            pending_window_thumbs: std::collections::VecDeque::new(),
            pending_workspace_thumbs: std::collections::VecDeque::new(),
            capture_overlay: crate::capture_overlay::CaptureOverlaySession::default(),
            screenshot_overlay: crate::screenshot_overlay::ScreenshotOverlaySession::default(),
            snap_preview: None,
            wallpaper: crate::wallpaper::Wallpaper::new(),
            blur: crate::blur::BlurRuntime::default(),
            hdr_encode: crate::hdr_encode::HdrEncodeRuntime::default(),
            hdr_client_content_visible: false,
            color_lut: crate::color_lut::ColorLutRuntime::default(),
            decorations: crate::decoration::DecorationRuntime::default(),
            decoration_overrides: crate::decoration_overrides::DecorationsRuntime::load(),
            input_runtime: crate::device_input::InputRuntime::new(),
            keybinds: crate::keybinds::KeybindRuntime::load(),
            super_tap_armed: false,
            output_runtime: crate::output_prefs::OutputRuntime::new(),
            redraw_trigger: None,
            damaged: true,
            defer_client_flush: false,
            last_pointer_forward: None,
            last_bar_position: metis_config::load_bar_config().position,
            bar_auto_hidden: std::collections::HashMap::new(),
            last_bar_auto_hide_enabled: metis_config::load_bar_config().auto_hide,
            last_bar_reveal_cmd: None,
            bar_edge_pointer_in: false,
            suppress_bar_edge_cmd_until: None,
            last_window_gap_px: metis_config::bar::window_gap_px(&metis_config::load_bar_config()),
            last_window_gap_check: std::time::Instant::now(),
            last_scroll_tick: None,
            last_layout_toggle: None,
            last_maximize_toggle: None,
            client_cursor_theme,
            client_cursor_size,
            client_gpu: None,
            dgpu_offload: None,
            clock: smithay::utils::Clock::new(),
            snap_overlay_id: smithay::backend::renderer::element::Id::new(),
            snap_overlay_commit: smithay::backend::renderer::utils::CommitCounter::default(),
            desktop_underlay_id: smithay::backend::renderer::element::Id::new(),
            desktop_underlay_commit: smithay::backend::renderer::utils::CommitCounter::default(),
            night_light_id: smithay::backend::renderer::element::Id::new(),
            night_light_commit: smithay::backend::renderer::utils::CommitCounter::default(),
            battery_dim: crate::battery_dim::BatteryDimRuntime::new(power_cfg.dim_on_battery),
            night_light_schedule_effective: None,
            pending_apply_outputs: false,
            outputs_reload_due: None,
            last_snap_rect: None,
            udev: None,
            winit_outputs: Vec::new(),
            output_globals: std::collections::HashMap::new(),
            image_capture: crate::image_capture::ImageCaptureRuntime::new(&dh),
            color_mgmt: crate::color_management::ColorManagementRuntime::new(&dh),
            idle,
            lock: crate::lock::LockState::new(),
            session_lock_state,
            protocol_lock: crate::session_lock::ProtocolLock::Unlocked,
            game_rules: metis_config::load_game_rules_config(),
            gaming_config: metis_config::load_gaming_config(),
            pending_game_fullscreen: std::collections::HashSet::new(),
            drm_syncobj_state: None,
            idle_inhibit_state,
            idle_notifier_state,
            cursor_position_hint: None,
            pointer_constraint_phases: std::collections::HashMap::new(),
            last_pointer_motion_surface: None,
        }
    }

    pub(crate) fn process_pending_captures(
        &mut self,
        renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    ) {
        // Never satisfy a screen-capture request while locked — the framebuffer
        // shows the lock UI, but refusing outright avoids leaking even that.
        if self.session_is_locked() {
            self.pending_window_thumbs.clear();
            self.pending_workspace_thumbs.clear();
            return;
        }
        if self.image_capture.has_pending() {
            let start = self.start_time;
            crate::image_capture::finish_pending_captures(self, renderer, start);
        }
        crate::window_thumb::process_pending_window_thumbs(self, renderer);
        crate::workspace_thumb::process_pending_workspace_thumbs(self, renderer);
    }

    /// Per-tick housekeeping shared by both backends: drive the startup state
    /// machine, service shell IPC, advance the debounced wallpaper decode, pick
    /// up live blur / decoration config changes, and tick scroll animations.
    pub(crate) fn xcursor_config(&self) -> (&str, u32) {
        let size = self.client_cursor_size.parse().unwrap_or(24).clamp(16, 96);
        (&self.client_cursor_theme, size)
    }

    /// Returns nothing; callers redraw when `self.damaged` is set. Kept off the
    /// render path so going idle can never starve shell/client spawn.
    pub fn tick_housekeeping(&mut self) {
        self.run_pending_startup();
        crate::ipc::drain_ipc(self);
        self.tick_portal_elevate();
        self.protocol_lock_reap_dead();
        // Must run every tick: unreaped clients become zombies. Chromium/Electron
        // ProcessSingleton treats a zombie PID as still alive (`kill(pid,0)` ok)
        // and can block the next launch for minutes notifying that primary
        // (seen with GitHub Desktop under Metis).
        self.reap_exited_children();

        if self.wallpaper.tick_decode() {
            self.damaged = true;
        }

        let (blur_changed, bar_position_changed) = self.blur.maybe_refresh();
        if blur_changed {
            self.damaged = true;
        }
        if bar_position_changed {
            self.last_bar_position = self.blur.position;
            self.reflow_for_bar_geometry_change();
        }
        self.maybe_refresh_window_gap();
        self.maybe_sync_bar_auto_hidden();

        let deco = self.decorations.maybe_refresh();
        if deco.damage {
            self.damaged = true;
        }
        if deco.relayout {
            let ids: Vec<u32> = self.windows.ids();
            for id in ids {
                self.apply_window_rect(id);
            }
            self.sync_all_app_windows();
            self.refresh_all_scroll_offsets();
            self.damaged = true;
        }

        if let Some(cfg) = self.input_runtime.maybe_refresh() {
            crate::device_input::apply_keyboard(self, &cfg);
        }
        self.keybinds.maybe_refresh();
        if self.decoration_overrides.maybe_refresh() {
            self.refresh_all_window_decoration_modes();
        }

        if let Some((before, cfg)) = self.output_runtime.maybe_refresh() {
            if before.primary_output != cfg.primary_output {
                self.emit_monitor_changed();
            }
            if crate::output_prefs::is_night_light_only_change(&before, &cfg) {
                crate::output_prefs::refresh_night_light(self, &before);
            } else {
                crate::output_prefs::apply_outputs(self, &cfg);
            }
        }

        crate::night_light::maybe_tick_schedule(self);

        // Slow AC/battery sample for dim-on-battery (sysfs; cheap).
        if self.battery_dim.poll_battery() {
            self.damaged = true;
        }

        self.tick_outputs_reload();

        if self.pending_apply_outputs {
            self.pending_apply_outputs = false;
            let cfg = self.output_runtime.cached().clone();
            crate::output_prefs::apply_outputs(self, &cfg);
        }

        if self.tick_scroll_animations() {
            self.damaged = true;
        }

        if self.tick_titlebar_reveal_animation() {
            self.damaged = true;
        }

        if self.tick_maximize_fx() {
            self.damaged = true;
        }

        if self.tick_minimize_genie_fx() {
            self.damaged = true;
        }

        if self.tick_x11_withdraws() {
            self.damaged = true;
        }
    }

    /// True while the startup splash layer is on-screen (backdrop blur is deferred
    /// until it dismisses — the first blur pass is expensive).
    pub fn splash_overlay_visible(&self) -> bool {
        use smithay::desktop::layer_map_for_output;
        for out in self.space.outputs() {
            let map = layer_map_for_output(out);
            for layer in map.layers() {
                if layer.namespace() != "metis-splash" {
                    continue;
                }
                match map.layer_geometry(layer) {
                    Some(g) if g.loc.y >= 0 && g.loc.y < 16_000 => return true,
                    None => return true,
                    _ => {}
                }
            }
        }
        false
    }

    pub fn set_redraw_trigger(&mut self, trigger: Rc<dyn Fn()>) {
        self.redraw_trigger = Some(trigger);
    }

    pub fn request_redraw(&mut self) {
        if let Some(trigger) = &self.redraw_trigger {
            trigger();
        }
    }

    /// Mark the output dirty. Actual redraws are paced by the 16ms heartbeat
    /// timer in the winit backend (the nested host does not vsync-throttle us),
    /// so we only flag damage here and let the next tick coalesce it. This caps
    /// the render rate at ~60fps even under a flood of client commits.
    pub fn schedule_redraw(&mut self) {
        self.damaged = true;
        // DRM backend: arm every enabled scan-out surface for repaint *now*, at
        // damage time, rather than waiting for the 16 ms housekeeping tick to
        // propagate `damaged` → `pending`. That tick capped repaints at ~60 Hz;
        // by arming `pending` here, the surface's *next vblank* repaints it
        // immediately (see `on_drm_vblank`), so a continuously-committing client
        // (a game) runs the render loop at the panel's full refresh — 120/144/240
        // Hz — instead of 60. It is self-limiting when idle: a frame with no
        // damage produces an empty result, is not queued, and no further vblank
        // arrives, so we fall back to the tick with zero busy-looping.
        if let Some(udev) = self.udev.as_mut() {
            for surface in udev.surfaces_mut() {
                if !surface.user_disabled {
                    surface.pending = true;
                }
            }
        }
    }

    /// Arm repaint for a single output only. Used on client commits so a game on
    /// one monitor does not force a full composite on every other display.
    pub fn schedule_redraw_for_output(&mut self, output: &smithay::output::Output) {
        self.damaged = true;
        let name = output.name();
        if let Some(udev) = self.udev.as_mut() {
            for surface in udev.surfaces_mut() {
                if !surface.user_disabled && surface.output.name() == name {
                    surface.pending = true;
                }
            }
        }
    }

    /// Arm repaint for the output a window sits on; falls back to all outputs.
    pub fn schedule_redraw_for_window(&mut self, id: u32) {
        if let Some(output) = self.output_for_window(id) {
            self.schedule_redraw_for_output(&output);
        } else {
            self.schedule_redraw();
        }
    }

    /// True when an output has at least one client in true fullscreen (bar hidden).
    pub(crate) fn output_has_fullscreen(&self, output_name: Option<&str>) -> bool {
        let Some(name) = output_name else {
            return false;
        };
        self.output_fullscreen_windows
            .get(name)
            .is_some_and(|s| !s.is_empty())
    }

    /// True when a fullscreen client on `output` was promoted to primary-plane scanout.
    pub(crate) fn output_scanout_promoted(
        &self,
        output: &smithay::output::Output,
        states: &smithay::backend::renderer::element::RenderElementStates,
    ) -> bool {
        use std::cell::Cell;

        use smithay::backend::renderer::element::default_primary_scanout_output_compare;
        use smithay::desktop::utils::{
            surface_primary_scanout_output, update_surface_primary_scanout_output,
        };

        let name = output.name();
        if !self.output_has_fullscreen(Some(name.as_ref())) {
            return false;
        }
        let cfg = metis_config::load_outputs_config();
        if crate::night_light::night_light_active(&cfg, Some(name.as_ref())) {
            return false;
        }
        let promoted = Cell::new(false);
        for window in self.space.elements() {
            if !self.space.outputs_for_element(window).contains(output) {
                continue;
            }
            window.with_surfaces(|surface, surface_data| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    surface_data,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
                if let Some(scanout) = surface_primary_scanout_output(surface, surface_data) {
                    if scanout == *output {
                        promoted.set(true);
                    }
                }
            });
        }
        promoted.get()
    }

    /// While a locked-pointer constraint is active the game draws its own cursor
    /// (or none) and the compositor cursor only blocks primary-plane scanout.
    pub(crate) fn active_pointer_lock_suppresses_cursor(&self) -> bool {
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let Some(focus) = pointer.current_focus() else {
            return false;
        };
        self.pointer_locked_on_surface(&focus, &pointer)
    }

    pub fn flush_clients_if_pending(&mut self) {
        if self.defer_client_flush {
            self.defer_client_flush = false;
            let _ = self.display_handle.flush_clients();
        }
    }

    /// Throttle pointer motion forwarded to clients — GTK hover repaints were saturating the loop.
    pub fn should_forward_pointer_motion(&mut self, location: Point<f64, Logical>) -> bool {
        // Never throttle while a compositor grab (move/resize/scroll-resize) owns the
        // pointer — dropped motion events leave the grab stuck at its start size.
        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return true;
        }
        if self.metis_bar_ui_hit(location) {
            return true;
        }
        // Web browsers (windowed WebGL / canvas games): full-rate absolute motion.
        // The GTK throttle made in-tab games feel sluggish without fullscreen.
        if self.pointer_over_web_browser(location) {
            self.last_pointer_forward = Some((std::time::Instant::now(), location));
            return true;
        }
        const MIN_MS: u128 = 48;
        const MIN_DIST_SQ: f64 = 9.0;
        let now = std::time::Instant::now();
        if let Some((t, prev)) = self.last_pointer_forward {
            let dx = location.x - prev.x;
            let dy = location.y - prev.y;
            if now.duration_since(t).as_millis() < MIN_MS && (dx * dx + dy * dy) < MIN_DIST_SQ {
                return false;
            }
        }
        self.last_pointer_forward = Some((now, location));
        true
    }

    /// True when the topmost window under the pointer is a web browser.
    fn pointer_over_web_browser(&self, location: Point<f64, Logical>) -> bool {
        let Some((window, _)) = self.topmost_window_at_pointer(location) else {
            return false;
        };
        let Some(id) = self.windows.id_for_window(&window) else {
            return false;
        };
        self.windows
            .get(id)
            .and_then(|r| r.app_id.as_deref())
            .is_some_and(crate::decoration_policy::id_looks_web_browser)
    }

    /// Active `zwp_locked_pointer_v1` on this surface (mouse-look / raw input).
    pub(crate) fn pointer_locked_on_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) -> bool {
        use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
        with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|c| c.is_active() && matches!(&*c, PointerConstraint::Locked(_)))
        })
    }

    pub fn window_id_for_toplevel(
        &self,
        surface: &smithay::wayland::shell::xdg::ToplevelSurface,
    ) -> Option<u32> {
        self.windows.id_for_surface(surface.wl_surface())
    }

    /// Push a target geometry to a window's client. Native Wayland toplevels get a
    /// pending `size` + `configure`; XWayland surfaces get an absolute `configure`
    /// (the X server tracks position, so it needs the location too). This is the
    /// single seam every non-tiling relayout path uses so X11 and Wayland windows
    /// share `apply_window_rect` and friends.
    pub(crate) fn send_window_configure(
        &self,
        record: &crate::windows::WindowRecord,
        loc: Point<i32, Logical>,
        size: Size<i32, Logical>,
    ) {
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.size = Some(size);
            });
            toplevel.send_pending_configure();
        } else if let Some(x11) = record.x11() {
            let _ = x11.configure(Rectangle::new(loc, size));
        }
    }

    /// The xdg-decoration mode a window negotiated, or `None` for XWayland (which
    /// has no client-side decoration protocol — Metis always owns its chrome).
    pub(crate) fn window_decoration_mode(
        &self,
        record: &crate::windows::WindowRecord,
    ) -> Option<smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode>{
        record.wl_toplevel().and_then(read_toplevel_decoration_mode)
    }

    /// Title + app_id for a window. XWayland windows have no Wayland `app_id`; we
    /// use their X11 `class` (WM_CLASS) so decoration heuristics and the dock can
    /// still identify them (Chrome, Steam, JetBrains IDEs, …).
    pub(crate) fn read_window_metadata(
        &self,
        record: &crate::windows::WindowRecord,
    ) -> (String, Option<String>) {
        if let Some(toplevel) = record.wl_toplevel() {
            return read_toplevel_metadata(toplevel);
        }
        if let Some(x11) = record.x11() {
            let title = {
                let t = x11.title();
                if t.trim().is_empty() {
                    "Application".to_string()
                } else {
                    t
                }
            };
            let app_id = {
                let class = x11.class();
                if class.trim().is_empty() {
                    None
                } else {
                    Some(class)
                }
            };
            return (title, app_id);
        }
        ("Application".into(), None)
    }

    /// True when Metis should draw server-side titlebar/border chrome for this window.
    pub(crate) fn window_uses_ssd(&self, id: u32) -> bool {
        self.windows.uses_ssd(id)
    }

    /// True when this SSD window should auto-hide its titlebar (maximize / snap /
    /// grid). All Metis-decorated windows use the slide-down hover overlay.
    pub(crate) fn should_auto_hide_titlebar(&self, id: u32) -> bool {
        self.window_uses_ssd(id)
    }

    /// True when Metis should render or hit-test server-side window chrome.
    pub(crate) fn should_draw_metis_ssd(&self, id: u32) -> bool {
        if !self.window_uses_ssd(id) {
            return false;
        }
        let Some(record) = self.windows.get(id) else {
            return false;
        };
        // A fullscreen window covers the whole output — it never wears chrome.
        // This also breaks a feedback loop: fullscreen grants the client
        // server-side decorations (so CSD toolkits like libdecor drop their own
        // frame), which would otherwise flip `uses_ssd` true and have Metis paint
        // a titlebar over the fullscreen surface.
        if record.fullscreen {
            return false;
        }
        let negotiated_mode = self.window_decoration_mode(record);
        !crate::decoration_policy::defer_ssd_paint(
            record.app_id.as_deref(),
            negotiated_mode,
            record.decoration_bound,
        )
    }

    /// Client surface rect within a tile footprint — body inset for SSD, full tile for CSD.
    pub(crate) fn tile_client_rect(&self, id: u32, full: PixelRect) -> PixelRect {
        if !self.should_draw_metis_ssd(id) {
            return full;
        }
        if self.auto_hide_titlebar.contains(&id) {
            metis_grid::app_tile_auto_hide_body_rect(full)
        } else {
            app_tile_body_rect(full)
        }
    }

    /// SSD client placement for a tile footprint: auto-hide windows fill the
    /// footprint; others keep a persistent titlebar inset.
    fn ssd_client_rect(&self, id: u32, full: PixelRect) -> PixelRect {
        if self.auto_hide_titlebar.contains(&id) {
            metis_grid::app_tile_auto_hide_body_rect(full)
        } else {
            app_tile_body_rect(full)
        }
    }

    /// Whether a maximized SSD window should use the hover overlay (compact or full).
    fn maximized_uses_auto_hide_titlebar(&self, id: u32) -> bool {
        self.should_auto_hide_titlebar(id)
    }

    /// The client process name backing a toplevel, resolved via the connection's
    /// pid. Used to disambiguate Electron shells that all report the generic
    /// `chromium` `app_id`. Reads `/proc/<pid>/comm`, which is world-readable even
    /// for sandboxed / non-dumpable Electron processes — unlike the `exe` symlink,
    /// which returns EACCES for them. Falls back to `exe` when `comm` is empty.
    fn client_executable_for_window(&self, id: u32) -> Option<String> {
        use smithay::reexports::wayland_server::Resource;
        let record = self.windows.get(id)?;
        // Only native Wayland clients expose a connection pid this way; XWayland
        // windows always default to Metis SSD, so they never reach this path.
        let client = record.wl_toplevel()?.wl_surface().client()?;
        let pid = client.get_credentials(&self.display_handle).ok()?.pid;
        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
            let name = comm.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        Some(path.file_name()?.to_string_lossy().into_owned())
    }

    /// True when a chromium-class window is actually a frameless Electron shell
    /// (e.g. Claude Desktop) that draws no chrome, so Metis should decorate it.
    /// Real Chromium-family browsers keep native CSD.
    pub(crate) fn chromium_window_needs_ssd(&self, id: u32, app_id: Option<&str>) -> bool {
        let Some(app_id) = app_id else {
            return false;
        };
        if !crate::decoration_policy::id_looks_chromium_family(app_id) {
            return false;
        }
        let Some(exe) = self.client_executable_for_window(id) else {
            return false;
        };
        crate::decoration_policy::chromium_class_needs_ssd(app_id, &exe)
    }

    /// Reconcile `uses_ssd` with xdg-decoration negotiation and app-id heuristics.
    pub(crate) fn refresh_window_decoration_mode(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let was_draw = self.should_draw_metis_ssd(id);
        let negotiated_mode = self.window_decoration_mode(&record);
        // User overrides apply to both Wayland and XWayland. (Fullscreen still
        // preserves the pre-fullscreen decision and ignores overrides.)
        let user_override = if record.fullscreen {
            None
        } else {
            self.decoration_overrides
                .user_override(record.app_id.as_deref())
        };
        // XWayland windows have no xdg-decoration protocol, but self-decorated X11
        // clients advertise "no server decorations" via `_MOTIF_WM_HINTS`
        // (`is_decorated()` == true). Steam, many game launchers, and Chromium's
        // X11 frame draw their own chrome — stacking a Metis titlebar on top gives
        // the tell-tale double titlebar. Honor the hint when Auto; known CSD
        // classes and user overrides still win (Thunar prefs dialog is X11 and
        // Motif-advertises SSD while drawing its own chrome).
        let mut uses_ssd = if record.fullscreen {
            // While fullscreen we grant the client server-side decorations to
            // strip its own CSD frame. That committed ServerSide mode must NOT
            // feed back into the client's windowed SSD decision here, or exiting
            // fullscreen would leave a Metis titlebar on a client that draws its
            // own chrome. Preserve the pre-fullscreen (windowed) decision.
            record.uses_ssd
        } else if let Some(force_ssd) = user_override {
            force_ssd
        } else if record.is_x11 {
            let app_id = record.app_id.as_deref();
            if app_id.is_some_and(crate::decoration_policy::id_looks_ssd) {
                true
            } else if app_id.is_some_and(crate::decoration_policy::id_looks_csd) {
                false
            } else if app_id.is_some_and(crate::decoration_policy::id_looks_wine) {
                // Win32-on-Wine: Motif "no decorations" usually means "no Linux
                // frame" rather than "I draw GTK chrome" — default to Metis SSD.
                true
            } else {
                record.x11().map(|x11| !x11.is_decorated()).unwrap_or(true)
            }
        } else {
            crate::decoration_policy::resolve_uses_ssd(
                record.app_id.as_deref(),
                negotiated_mode,
                record.decoration_bound,
                None,
            )
        };
        // User Force CSD must not be undone by the frameless-Electron heuristic.
        if user_override.is_none()
            && !record.fullscreen
            && !uses_ssd
            && self.chromium_window_needs_ssd(id, record.app_id.as_deref())
        {
            uses_ssd = true;
        }
        let mode_changed = uses_ssd != record.uses_ssd;
        if mode_changed {
            tracing::info!(
                id,
                uses_ssd,
                app_id = ?record.app_id,
                ?negotiated_mode,
                decoration_negotiated = record.decoration_negotiated,
                "window decoration policy updated"
            );
            if !uses_ssd {
                self.clear_auto_hide(id);
            }
        }
        self.windows.set_uses_ssd(id, uses_ssd);
        self.sync_auto_hide_titlebar(id);
        // Push once we can classify the client — including CSD from app_id,
        // negotiation, or an early xdg-decoration bind (GTK/Chromium).
        let app_id_known = record.app_id.as_ref().is_some_and(|id| !id.is_empty());
        if let Some(toplevel) = record.wl_toplevel() {
            if app_id_known || record.decoration_negotiated || !uses_ssd || record.fullscreen {
                self.push_preferred_decoration_mode(toplevel, uses_ssd, record.fullscreen);
            }
        }
        let now_draw = self.should_draw_metis_ssd(id);
        if mode_changed || was_draw != now_draw {
            self.apply_window_rect(id);
            self.schedule_redraw();
        }
    }

    /// Re-apply decoration policy to every tracked window (after overrides reload).
    pub(crate) fn refresh_all_window_decoration_modes(&mut self) {
        let ids = self.windows.ids();
        for id in ids {
            self.refresh_window_decoration_mode(id);
        }
    }

    fn push_preferred_decoration_mode(
        &self,
        toplevel: &smithay::wayland::shell::xdg::ToplevelSurface,
        uses_ssd: bool,
        fullscreen: bool,
    ) {
        // A fullscreen surface has no chrome: force server-side so CSD toolkits
        // (libdecor / GLFW games such as Hytale) drop their own titlebar+shadow
        // frame — the frame is what reports the negative window-geometry inset
        // that shifts the surface off the output origin on the first fullscreen.
        let mode = if fullscreen {
            crate::decoration_policy::grant_decoration_mode(true)
        } else {
            crate::decoration_policy::grant_decoration_mode(uses_ssd)
        };
        let mut changed = false;
        toplevel.with_pending_state(|state| {
            if state.decoration_mode != Some(mode) {
                changed = true;
            }
            state.decoration_mode = Some(mode);
        });
        // The first configure is sent from `apply_window_rect` / `ensure_initial_configure`
        // so size and decoration_mode ship together. Later mode changes need a flush.
        if changed && toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    pub fn tile_id_for_window(&self, window_id: u32) -> Option<String> {
        self.find_app_tile(window_id).map(|(_, t)| t.id)
    }

    /// App windows slotted in the desk grid — not free-floating or fullscreen.
    pub fn is_window_grid_managed(&self, id: u32) -> bool {
        if self.floating.contains(&id) {
            return false;
        }
        if self.tile_id_for_window(id).is_none() {
            return false;
        }
        let Some(record) = self.windows.get(id) else {
            return false;
        };
        !record.fullscreen && !record.maximized && !self.windows.is_minimized(id)
    }

    /// Snap a grid-managed window back to its tile body if the client moved or resized it.
    pub fn enforce_grid_window_geometry(&mut self, id: u32) {
        if !self.is_window_grid_managed(id) {
            return;
        }
        let Some(expected) = self
            .rect_for_window_tile(id)
            .map(|full| self.tile_client_rect(id, full))
        else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let drifted = self
            .space
            .element_location(&record.window)
            .is_none_or(|loc| loc.x != expected.x || loc.y != expected.y)
            || record.window.geometry().size
                != smithay::utils::Size::from((expected.width, expected.height))
            || record.target_rect != expected;
        if drifted {
            self.apply_window_rect(id);
        }
    }

    /// Compute the snap-zone target for a pointer at global-logical (`x`, `y`),
    /// in pixel space against the usable area (so the top edge maximizes below
    /// the bar). Returns the final *client* rect (gaps already applied to match
    /// the maximize look) + label, or `None` when the pointer isn't near an edge.
    pub fn snap_target_at(&self, x: i32, y: i32) -> Option<(PixelRect, &'static str)> {
        // Snap against the output the pointer is over, so dragging a window to a
        // secondary monitor's edge tiles it on *that* monitor.
        let place = match self.output_at(Point::from((x, y))) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let pos = metis_config::load_bar_config().position;

        // Snap geometry is computed in `place` (beside the bar). When the pointer
        // hugs the physical monitor edge over an overlay bar, project it onto the
        // nearest `place` edge so left/right/bottom snaps still fire.
        let mut sx = x;
        let mut sy = y;
        match pos {
            metis_config::BarPosition::Left if x < place.x => sx = place.x,
            metis_config::BarPosition::Right if x > place.x + place.width => {
                sx = place.x + place.width;
            }
            metis_config::BarPosition::Bottom if y > place.y + place.height => {
                sy = place.y + place.height;
            }
            _ => {}
        }

        let (raw, label) = metis_grid::pixel_snap_target(sx, sy, place)?;
        let gaps = self.zone_edge_gaps();
        Some((snap_client_rect(raw, place, gaps), label))
    }

    /// Drop a window into a snap zone. The "Maximize" zone routes through the real
    /// `set_maximized` so it's pixel-identical to the titlebar maximize button.
    /// Half / quarter zones float the window and mark it *tiled* (all four edges)
    /// so GTK squares its corners and drops its drop-shadow, filling the snapped
    /// rect exactly — otherwise the leftover CSD shadow makes the padding look
    /// uneven from edge to edge.
    pub fn apply_snap(&mut self, id: u32, rect: PixelRect, label: &str) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        if label == "Maximize" {
            self.set_maximized(id, true);
            return;
        }

        // Re-home desk membership to the monitor this snap targets *before*
        // applying geometry. Doing this after the snap (via `maybe_adopt`) used
        // to run `clamp_floating_rect`, adding a spurious titlebar inset on
        // auto-hide edge snaps dragged across outputs.
        let snap_center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        if let Some(output) = self.output_at(snap_center) {
            let key = output.name();
            if key != self.desk_key_for_window(id) {
                self.move_window_to_output_inner(id, &key, false);
            }
        }

        self.capture_pre_snap_geometry(id);

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.space.raise_element(&record.window, true);
        self.floating.insert(id);
        self.windows.set_maximized(id, false);

        // Snaps whose top edge meets the bar auto-hide the titlebar as a hover
        // overlay on the client's top strip.
        let top_touching = matches!(label, "Left half" | "Right half" | "Top-left" | "Top-right");
        let uses_ssd = self.window_uses_ssd(id);
        let body = if uses_ssd {
            if top_touching && self.should_auto_hide_titlebar(id) {
                metis_grid::app_tile_auto_hide_body_rect(rect)
            } else {
                app_tile_body_rect(rect)
            }
        } else {
            rect
        };
        let size = Size::from((body.width.max(1), body.height.max(1)));
        let loc = Point::from((body.x, body.y));
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::TiledLeft);
                state.states.set(xdg_toplevel::State::TiledRight);
                state.states.set(xdg_toplevel::State::TiledTop);
                state.states.set(xdg_toplevel::State::TiledBottom);
                state.size = Some(size);
            });
        }
        self.space.map_element(record.window.clone(), loc, true);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, body);
        if uses_ssd && top_touching && self.should_auto_hide_titlebar(id) {
            self.auto_hide_titlebar.insert(id);
        } else {
            self.clear_auto_hide(id);
        }
        self.reclamp_auto_hide(id);
        self.windows.set_snapped(id, true);
        self.save_window_geometry(id);
        tracing::info!(id, ?rect, label, "snap: window snapped to zone");
    }

    /// Clear the tiled states a snap applied, so a window pulled off a snapped
    /// position regains its normal floating chrome (GTK rounded corners + drop
    /// shadow). `send_pending_configure` is a no-op when nothing actually changed.
    fn clear_tiled_states(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(State::TiledLeft);
                state.states.unset(State::TiledRight);
                state.states.unset(State::TiledTop);
                state.states.unset(State::TiledBottom);
            });
            toplevel.send_pending_configure();
        }
    }

    /// Drop a window's auto-hide-titlebar state (e.g. on unmaximize, unsnap,
    /// minimize, fullscreen, or close), clearing the reveal if it was showing.
    pub fn clear_auto_hide(&mut self, id: u32) {
        self.auto_hide_titlebar.remove(&id);
        if self.revealed_titlebar == Some(id) {
            self.revealed_titlebar = None;
        }
        if self.titlebar_reveal_window == Some(id) {
            self.titlebar_reveal_window = None;
            self.titlebar_reveal_progress = 0.0;
        }
    }

    /// Tabbed browsers and other CSD clients keep native chrome; SSD windows use
    /// the hover overlay when grid-tiled, snapped, or maximized.
    fn sync_auto_hide_titlebar(&mut self, id: u32) {
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let ssd = self.should_auto_hide_titlebar(id) && self.should_draw_metis_ssd(id);
        if !ssd {
            return;
        }
        let grid_tiled = self.tile_id_for_window(id).is_some() && !self.floating.contains(&id);
        let maximized_or_snapped = record.maximized || record.snapped;
        let tabbed_floating = ssd
            && self.window_uses_compact_overlay(id)
            && self.floating.contains(&id)
            && !maximized_or_snapped;
        let snap_auto_hide =
            ssd && record.snapped && !record.maximized && self.should_auto_hide_titlebar(id);
        let maximized_auto_hide =
            ssd && record.maximized && self.maximized_uses_auto_hide_titlebar(id);
        let should_overlay = grid_tiled || maximized_auto_hide || snap_auto_hide || tabbed_floating;
        if should_overlay {
            let was_auto_hide = self.auto_hide_titlebar.contains(&id);
            self.auto_hide_titlebar.insert(id);
            if !was_auto_hide && record.maximized && ssd {
                self.reapply_maximized_geometry(id);
            }
        } else if self.auto_hide_titlebar.contains(&id) {
            self.auto_hide_titlebar.remove(&id);
        }
    }

    /// Live client-surface (body) geometry for a mapped window.
    pub(crate) fn window_body_rect(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        let loc = self.space.element_location(&record.window)?;
        let size = record.window.geometry().size;
        Some(PixelRect {
            x: loc.x,
            y: loc.y,
            width: size.w.max(1),
            height: size.h.max(1),
        })
    }

    /// Remember a floating window's size/position before the first snap in a chain.
    fn capture_pre_snap_geometry(&mut self, id: u32) {
        if self.windows.is_snapped(id) {
            return;
        }
        let Some(body) = self.window_body_rect(id) else {
            return;
        };
        self.windows.set_restore_rect(id, body);
    }

    /// Pull a snapped/maximized window back to its pre-snap floating size when the
    /// user starts dragging it by the titlebar. Keeps the grab point under the
    /// pointer so the window doesn't jump.
    fn restore_floating_from_snap(
        &mut self,
        id: u32,
        pointer: Point<f64, Logical>,
    ) -> Point<i32, Logical> {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return Point::default();
        };

        let current = self.window_body_rect(id).unwrap_or(record.target_rect);
        let restore = self.windows.take_restore_rect(id).unwrap_or(current);

        self.clear_auto_hide(id);
        self.windows.set_maximized(id, false);
        self.windows.set_snapped(id, false);

        let rel_x = if current.width > 0 {
            (pointer.x - current.x as f64) / current.width as f64
        } else {
            0.5
        };
        let rel_y = if current.height > 0 {
            (pointer.y - current.y as f64) / current.height as f64
        } else {
            0.0
        };
        let rel_x = rel_x.clamp(0.0, 1.0);
        let rel_y = rel_y.clamp(0.0, 1.0);

        let mut body = PixelRect {
            x: (pointer.x - rel_x * restore.width as f64).round() as i32,
            y: (pointer.y - rel_y * restore.height as f64).round() as i32,
            width: restore.width.max(1),
            height: restore.height.max(1),
        };
        body = self.clamp_floating_rect_for(id, body);

        let loc = Point::from((body.x, body.y));
        let size = Size::from((body.width, body.height));
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.unset(xdg_toplevel::State::TiledLeft);
                state.states.unset(xdg_toplevel::State::TiledRight);
                state.states.unset(xdg_toplevel::State::TiledTop);
                state.states.unset(xdg_toplevel::State::TiledBottom);
                state.size = Some(size);
                state.fullscreen_output = None;
            });
        }
        self.space.map_element(record.window.clone(), loc, true);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, body);
        self.schedule_redraw();
        loc
    }

    pub fn queue_startup(&mut self, shell: Option<String>, client: Option<String>) {
        self.startup_shell = shell.clone();
        self.startup_client = client;
        // Same binary, second process: widgets are isolated from the edge bar.
        self.widgets_cmd = shell.map(|s| {
            if s.contains("--desktop-widgets") {
                s
            } else {
                format!("{s} --desktop-widgets")
            }
        });
    }

    pub fn run_pending_startup(&mut self) {
        let elapsed = self.start_time.elapsed();

        if !self.shell_spawned && elapsed > Duration::from_millis(250) {
            if std::env::var("METIS_NO_SHELL").is_err() {
                if let Some(shell) = self.startup_shell.take() {
                    self.spawn_client(&shell);
                }
                // Spawn widgets shortly after the bar so layer-shell surfaces do
                // not contend on the first commit storm.
                if let Some(widgets) = self.widgets_cmd.clone() {
                    self.spawn_widgets_process(&widgets);
                }
            } else {
                self.startup_shell = None;
                self.widgets_cmd = None;
            }
            self.shell_spawned = true;
        }

        if self.shell_spawned && !self.client_spawned && elapsed > Duration::from_millis(750) {
            if let Some(client) = self.startup_client.take() {
                self.spawn_client(&client);
                // Only poll grid placement when an explicit `-c` client was requested.
                self.startup_frames = 120;
            }
            self.client_spawned = true;
        }

        // Phase 3: user startup applications (~2s after compositor start).
        if !self.startup_apps_spawned && elapsed > Duration::from_secs(2) {
            self.arm_startup_applications();
            self.startup_apps_spawned = true;
        }
        self.drain_startup_apps_queue();

        self.maybe_respawn_widgets();

        if self.startup_frames > 0 {
            self.startup_frames -= 1;
            self.sync_all_app_windows();
        }
    }

    /// Queue desktop apps from `startup.json` (one-shot per session).
    fn arm_startup_applications(&mut self) {
        if self.session_is_locked() {
            tracing::info!("startup apps skipped — session locked");
            return;
        }
        let app_cfg = metis_config::load_app_config();
        if !app_cfg.onboarding_complete {
            tracing::info!("startup apps skipped — onboarding incomplete");
            return;
        }
        let cfg = metis_config::load_startup_config();
        if !cfg.enabled {
            tracing::debug!("startup apps disabled in startup.json");
            return;
        }
        if cfg.entries.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        let mut stagger_i = 0u32;
        for entry in &cfg.entries {
            if !entry.enabled {
                continue;
            }
            match metis_config::resolve_desktop_launch_argv(&entry.id) {
                Some(argv) => {
                    let delay = Duration::from_secs(u64::from(entry.delay_seconds))
                        + Duration::from_millis(u64::from(stagger_i) * 120);
                    self.startup_apps_queue.push((now + delay, argv));
                    stagger_i = stagger_i.saturating_add(1);
                }
                None => {
                    tracing::warn!(
                        id = %entry.id,
                        "startup app missing or Exec unresolved — skipped"
                    );
                }
            }
        }
        if !self.startup_apps_queue.is_empty() {
            tracing::info!(
                count = self.startup_apps_queue.len(),
                "queued session startup applications"
            );
        }
    }

    /// Spawn due startup apps (at most two per tick; never via shell).
    fn drain_startup_apps_queue(&mut self) {
        if self.startup_apps_queue.is_empty() {
            return;
        }
        if self.session_is_locked() {
            return;
        }
        let now = std::time::Instant::now();
        let mut due = Vec::new();
        let mut rest = Vec::new();
        for (when, argv) in self.startup_apps_queue.drain(..) {
            if when <= now && due.len() < 2 {
                due.push(argv);
            } else {
                rest.push((when, argv));
            }
        }
        self.startup_apps_queue = rest;
        for argv in due {
            tracing::info!(program = %argv.first().map(String::as_str).unwrap_or("?"), "startup spawn");
            self.spawn_client_argv(&argv);
        }
    }

    fn spawn_widgets_process(&mut self, program: &str) {
        let before = self.child_processes.len();
        let token = Self::new_widgets_ipc_token();
        self.widgets_ipc_token = Some(token.clone());
        let argv = metis_protocol::split_command_line(program);
        self.spawn_client_argv_with_env(
            &argv,
            &[
                ("METIS_IPC_TOKEN", token.as_str()),
                ("METIS_IPC_SCOPE", "widgets"),
            ],
        );
        self.widgets_last_spawn = Some(std::time::Instant::now());
        if self.child_processes.len() > before {
            if let Some(child) = self.child_processes.last() {
                self.widgets_pid = Some(child.id());
            }
        } else {
            self.widgets_pid = None;
            self.widgets_ipc_token = None;
        }
    }

    /// Collect exit status for finished children so they leave the process table.
    fn reap_exited_children(&mut self) {
        let mut i = 0;
        while i < self.child_processes.len() {
            match self.child_processes[i].try_wait() {
                Ok(None) => i += 1,
                Ok(Some(status)) => {
                    // `try_wait` already reaped; drop the handle (clippy wants `.wait()`
                    // but a second wait after a successful try_wait is redundant).
                    #[allow(clippy::zombie_processes)]
                    let child = self.child_processes.remove(i);
                    let pid = child.id();
                    if self.widgets_pid == Some(pid) {
                        self.widgets_pid = None;
                        self.widgets_ipc_token = None;
                    }
                    tracing::debug!(pid, ?status, "reaped exited client process");
                }
                Err(err) => {
                    #[allow(clippy::zombie_processes)]
                    let child = self.child_processes.remove(i);
                    let pid = child.id();
                    if self.widgets_pid == Some(pid) {
                        self.widgets_pid = None;
                        self.widgets_ipc_token = None;
                    }
                    tracing::debug!(pid, %err, "dropped client process handle");
                }
            }
        }
    }

    /// Rate-limited respawn if the widgets process exits (crash / OOM).
    fn maybe_respawn_widgets(&mut self) {
        let Some(cmd) = self.widgets_cmd.clone() else {
            return;
        };

        self.reap_exited_children();

        if self.widgets_pid.is_some() {
            return; // still running
        }

        if self.widgets_last_spawn.is_some() {
            tracing::warn!("desktop-widgets process exited — respawning");
        }

        let min_gap = Duration::from_secs(3);
        if self
            .widgets_last_spawn
            .is_some_and(|t| t.elapsed() < min_gap)
        {
            return;
        }
        self.spawn_widgets_process(&cmd);
    }

    pub fn spawn_client(&mut self, program: &str) {
        let argv = metis_protocol::split_command_line(program);
        self.spawn_client_argv(&argv);
    }

    fn new_widgets_ipc_token() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        format!("{nanos:032x}{pid:08x}")
    }

    /// Spawn a client from argv — never via `sh -c`.
    pub fn spawn_client_argv(&mut self, argv: &[String]) {
        self.spawn_client_argv_with_env(argv, &[]);
    }

    pub fn spawn_client_argv_with_env(&mut self, argv: &[String], extra_env: &[(&str, &str)]) {
        if argv.is_empty() {
            tracing::warn!("spawn_client_argv: empty argv");
            return;
        }
        // Metis binaries (shell, settings) live alongside the compositor in the
        // cargo target dir, which is usually not on PATH. Resolve a bare program
        // name to its sibling-of-current-exe absolute path so `Launch` works.
        let mut argv = argv.to_vec();
        if !argv[0].contains('/') {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let candidate = dir.join(&argv[0]);
                    if candidate.is_file() {
                        argv[0] = candidate.display().to_string();
                    }
                }
            }
        }

        // Metis-owned Steam tweaks (MangoHud / Gamescope) — never Steam VDF writes.
        let tweaks = metis_config::apply_steam_launch_tweaks(&argv, &self.gaming_config);
        let argv = tweaks.argv;
        let tweak_env = tweaks.env;
        let program_joined = argv.join(" ");
        let app_cfg = metis_config::load_app_config();
        let use_gaming_xwayland = app_cfg.xwayland_mode == metis_config::XwaylandMode::Isolated
            && metis_config::command_uses_gaming_xwayland(
                &program_joined,
                &app_cfg.xwayland_policy,
            );

        if use_gaming_xwayland && self.xdisplay_gaming.is_none() {
            tracing::info!(
                program = %program_joined,
                "spawn: queueing until gaming XWayland is ready"
            );
            let mut queued_env: Vec<(String, String)> = extra_env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            for (k, v) in tweak_env {
                queued_env.push((k, v));
            }
            self.pending_gaming_launches.push(PendingGamingLaunch {
                argv: argv.clone(),
                extra_env: queued_env,
            });
            self.ensure_gaming_xwayland();
            return;
        }

        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        for (k, v) in &tweak_env {
            cmd.env(k, v);
        }

        // Games/launchers run on the discrete GPU; desktop apps on the iGPU.
        // `gaming.json` + METIS_GAME_GPU override the heuristic.
        let prefer_dgpu =
            metis_config::prefer_dgpu_for_launch(&program_joined, &self.gaming_config);
        if prefer_dgpu && self.dgpu_offload.is_some() {
            tracing::info!(
                program = %program_joined,
                "spawn: steering launch onto discrete GPU (PRIME offload)"
            );
        }
        if use_gaming_xwayland {
            tracing::info!(
                program = %program_joined,
                display = ?self.xdisplay_gaming,
                "spawn: gaming XWayland class"
            );
        }
        apply_spawned_client_env(
            &mut cmd,
            &program_joined,
            &self.socket_name,
            self.xdisplay,
            self.xdisplay_gaming,
            self.client_gpu.as_ref(),
            self.dgpu_offload.as_ref(),
            prefer_dgpu,
            use_gaming_xwayland,
        );
        cmd.env("XCURSOR_THEME", &self.client_cursor_theme);
        cmd.env("XCURSOR_SIZE", &self.client_cursor_size);

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                tracing::info!(
                    program = %program_joined,
                    pid,
                    wayland_display = ?self.socket_name,
                    "spawned client"
                );
                if metis_config::command_prefers_dgpu(&program_joined) && prefer_dgpu {
                    self.event_bus
                        .emit(&metis_protocol::CompositorEvent::GameSession {
                            active: true,
                            label: Some(program_joined.clone()),
                            pid: Some(pid),
                        });
                }
                self.child_processes.push(child);
            }
            Err(err) => {
                tracing::warn!(program = %program_joined, %err, "failed to spawn client")
            }
        }
    }

    pub(crate) fn flush_pending_gaming_launches(&mut self) {
        let pending = std::mem::take(&mut self.pending_gaming_launches);
        if pending.is_empty() {
            return;
        }
        tracing::info!(
            count = pending.len(),
            "flushing launches queued for gaming XWayland"
        );
        for launch in pending {
            let extra: Vec<(&str, &str)> = launch
                .extra_env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            self.spawn_client_argv_with_env(&launch.argv, &extra);
        }
    }

    pub(crate) fn drop_pending_gaming_launches(&mut self, reason: &str) {
        let n = self.pending_gaming_launches.len();
        if n == 0 {
            return;
        }
        tracing::warn!(count = n, %reason, "dropping launches queued for gaming XWayland");
        self.pending_gaming_launches.clear();
    }

    pub fn kill_spawned_clients(&mut self) {
        for mut child in self.child_processes.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Tear down spawned clients and stop the event loop. On the DRM backend,
    /// switch back to the login greeter's VT so the display manager can repaint
    /// instead of leaving a black framebuffer.
    pub(crate) fn end_compositor_session(&mut self) {
        tracing::info!("shutting down compositor session");
        self.kill_spawned_clients();
        if self.is_drm_backend() {
            self.drm_change_vt(1);
        }
        self.loop_signal.stop();
    }

    fn init_wayland_listener(
        display: Display<MetisState>,
        event_loop: &mut EventLoop<'_, MetisState>,
    ) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto()
            .expect("wayland listening socket (WAYLAND_DISPLAY) setup failed");
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                {
                    tracing::warn!(?err, "failed to insert wayland client");
                }
            })
            .expect("Failed to init the wayland event source.");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    state.run_pending_startup();
                    if let Err(err) = unsafe { display.get_mut().dispatch_clients(state) } {
                        tracing::error!(?err, "wayland dispatch failed");
                    }
                    state.flush_pending_clipboard_capture();
                    // Configure events are queued during dispatch; clients block until flushed.
                    let _ = state.display_handle.flush_clients();
                    if let Some(ref listener) = state.events_listener {
                        accept_event_subscribers(
                            listener,
                            &state.event_bus,
                            &mut state.event_subscribe_rate_limit,
                        );
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init the wayland display event source.");

        socket_name
    }

    pub fn send_layer_frames(&self, output: &smithay::output::Output, time: Duration) {
        let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
        let throttle = Duration::from_millis(16);
        for layer in layers {
            layer.send_frame(output, time, Some(throttle), |_, _| Some(output.clone()));
        }
        self.send_protocol_lock_frames(output, time);
    }

    pub fn arrange_layers(&self) {
        for output in self.space.outputs() {
            layer_map_for_output(output).arrange();
        }
    }

    // --- Output registry helpers ------------------------------------------------
    //
    // The single source of truth for output geometry is the smithay `Space`. These
    // helpers centralize "which output" decisions so the per-output refactor (Phase
    // 3) only has to change the chokepoints below rather than ~15 scattered
    // `space.outputs().next()` call sites. With one output they are equivalent to
    // the old primary-only behavior.

    /// The primary (configured or first-registered) enabled output.
    pub fn primary_output(&self) -> Option<smithay::output::Output> {
        let cfg = self.output_runtime.cached();
        if let Some(ref name) = cfg.primary_output {
            if self.is_output_enabled(name) {
                if let Some(o) = self.output_by_name(name) {
                    return Some(o);
                }
            }
        }
        self.space
            .outputs()
            .find(|o| o.name() != "metis-render")
            .cloned()
            .or_else(|| self.space.outputs().next().cloned())
    }

    /// Every connected client-visible output, including user-disabled ones.
    pub fn connected_outputs(&self) -> Vec<smithay::output::Output> {
        if let Some(udev) = &self.udev {
            udev.surfaces().map(|s| s.output.clone()).collect()
        } else {
            self.winit_outputs.clone()
        }
    }

    /// Whether `name` is currently mapped into the desktop and visible to clients.
    pub fn is_output_enabled(&self, name: &str) -> bool {
        self.space
            .outputs()
            .any(|o| o.name() == name && o.name() != "metis-render")
    }

    pub fn enabled_output_count(&self) -> usize {
        self.space
            .outputs()
            .filter(|o| o.name() != "metis-render")
            .count()
    }

    /// Move every window on `output_key` to another enabled output before disable.
    pub fn evacuate_output(&mut self, output_key: &str, fallback_key: &str) {
        if output_key.is_empty() || fallback_key.is_empty() || output_key == fallback_key {
            return;
        }
        let ids: Vec<u32> = self
            .windows
            .ids()
            .into_iter()
            .filter(|id| self.desk_key_for_window(*id) == output_key)
            .collect();
        for id in ids {
            self.move_window_to_output(id, fallback_key);
        }
    }

    pub(crate) fn fallback_output_key_excluding(&self, skip: &str) -> Option<String> {
        self.space
            .outputs()
            .find(|o| o.name() != skip && o.name() != "metis-render")
            .map(|o| o.name())
    }

    pub(crate) fn winit_disable_output(&mut self, name: &str) -> bool {
        if self.udev.is_some() {
            return false;
        }
        let Some(output) = self
            .winit_outputs
            .iter()
            .find(|o| o.name() == name)
            .cloned()
        else {
            return false;
        };
        if !self.is_output_enabled(name) {
            return false;
        }
        if let Some(global) = self.output_globals.remove(name) {
            self.display_handle.remove_global::<MetisState>(global);
        }
        self.space.unmap_output(&output);
        tracing::info!(output = %name, "output disabled by user");
        true
    }

    pub(crate) fn winit_enable_output(&mut self, name: &str) -> bool {
        if self.udev.is_some() {
            return false;
        }
        let Some(output) = self
            .winit_outputs
            .iter()
            .find(|o| o.name() == name)
            .cloned()
        else {
            return false;
        };
        if self.is_output_enabled(name) {
            return false;
        }
        let global = output.create_global::<MetisState>(&self.display_handle);
        self.output_globals.insert(name.to_string(), global);
        tracing::info!(output = %name, "output re-enabled by user");
        true
    }

    pub(crate) fn repack_winit_outputs(&mut self) {
        if self.udev.is_some() {
            return;
        }
        let mut enabled: Vec<smithay::output::Output> = self
            .winit_outputs
            .iter()
            .filter(|o| self.output_globals.contains_key(&o.name()))
            .cloned()
            .collect();
        enabled.sort_by_key(|o| o.name());
        let cfg = self.output_runtime.cached().clone();
        let mut auto_x = 0_i32;
        for output in enabled {
            let width = output
                .current_mode()
                .map(|m| m.size.w)
                .unwrap_or(self.monitor.width.max(1));
            let prefs = metis_config::output_prefs(&cfg, &output.name());
            let pos = if let (Some(x), Some(y)) = (prefs.layout_x, prefs.layout_y) {
                smithay::utils::Point::from((x, y))
            } else {
                let pos = smithay::utils::Point::from((auto_x, 0));
                auto_x += width;
                pos
            };
            output.change_current_state(None, None, None, Some(pos));
            self.space.map_output(&output, pos);
        }
        if let Some(geo) = self
            .space
            .outputs()
            .next()
            .and_then(|o| self.space.output_geometry(o))
        {
            self.monitor.width = geo.size.w;
            self.monitor.height = geo.size.h;
        }
    }

    pub(crate) fn retile_after_output_prefs(&mut self) {
        if self.udev.is_some() {
            self.retile_outputs();
        } else {
            self.repack_winit_outputs();
            let (wp_full, wp_regions) = self.wallpaper_layout();
            self.wallpaper.set_layout(wp_full, wp_regions);
            self.wallpaper.start_async_decode();
            self.reflow_for_bar_geometry_change();
            self.emit_monitor_changed();
            self.damaged = true;
            self.schedule_redraw();
        }
    }

    pub(crate) fn set_output_enabled(&mut self, name: &str, enabled: bool) -> bool {
        let currently = self.is_output_enabled(name);
        if enabled == currently {
            return false;
        }
        if !enabled {
            if self.enabled_output_count() <= 1 {
                tracing::warn!(output = %name, "refusing to disable last enabled output");
                return false;
            }
            let Some(fallback) = self.fallback_output_key_excluding(name) else {
                return false;
            };
            let fallback = fallback.clone();
            self.evacuate_output(name, &fallback);
            let ok = if self.udev.is_some() {
                self.udev_disable_output(name)
            } else {
                self.winit_disable_output(name)
            };
            if !ok {
                return false;
            }
        } else {
            let ok = if self.udev.is_some() {
                self.udev_enable_output(name)
            } else {
                self.winit_enable_output(name)
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// Global logical geometry of `output` as a `MonitorRect`.
    pub fn output_rect(&self, output: &smithay::output::Output) -> Option<MonitorRect> {
        self.space.output_geometry(output).map(|g| MonitorRect {
            x: g.loc.x,
            y: g.loc.y,
            width: g.size.w,
            height: g.size.h,
        })
    }

    /// Bounding rectangle of every output — the whole virtual desktop — in global
    /// logical coords. Falls back to the cached monitor before any output maps.
    /// Used for absolute-pointer mapping and cross-output window dragging.
    /// Clamp a pointer position to the union of output geometries so relative
    /// (libinput) motion can never leave the visible desktop.
    pub fn clamp_to_desktop(&self, p: Point<f64, Logical>) -> Point<f64, Logical> {
        let b = self.desktop_bounds();
        let max_x = (b.loc.x + b.size.w - 1).max(b.loc.x) as f64;
        let max_y = (b.loc.y + b.size.h - 1).max(b.loc.y) as f64;
        Point::from((
            p.x.clamp(b.loc.x as f64, max_x),
            p.y.clamp(b.loc.y as f64, max_y),
        ))
    }

    pub fn desktop_bounds(&self) -> smithay::utils::Rectangle<i32, Logical> {
        if self.mirror_mode_active() {
            if let Some(source) = self.resolve_mirror_source() {
                if let Some(g) = self.space.output_geometry(&source) {
                    return g;
                }
            }
        }
        let mut bounds: Option<smithay::utils::Rectangle<i32, Logical>> = None;
        for o in self.space.outputs() {
            if let Some(g) = self.space.output_geometry(o) {
                bounds = Some(match bounds {
                    Some(b) => b.merge(g),
                    None => g,
                });
            }
        }
        bounds.unwrap_or_else(|| {
            smithay::utils::Rectangle::new(
                Point::from((self.monitor.x, self.monitor.y)),
                Size::from((self.monitor.width, self.monitor.height)),
            )
        })
    }

    /// The output whose logical geometry contains `point` (global logical
    /// coords), falling back to the primary output when the point is off every
    /// output. Used to route placement, snapping, and maximize to the monitor a
    /// window or the cursor is actually on.
    pub fn output_at(&self, point: Point<i32, Logical>) -> Option<smithay::output::Output> {
        if self.mirror_mode_active() {
            if let Some(source) = self.resolve_mirror_source() {
                if self
                    .space
                    .output_geometry(&source)
                    .is_some_and(|g| g.contains(point))
                {
                    return Some(source);
                }
            }
        }
        self.space
            .outputs()
            .find(|o| {
                self.space
                    .output_geometry(o)
                    .is_some_and(|g| g.contains(point))
            })
            .cloned()
            .or_else(|| self.primary_output())
    }

    /// The output currently under the pointer, falling back to primary.
    pub fn output_under_pointer(&self) -> Option<smithay::output::Output> {
        match self.seat.get_pointer() {
            Some(p) => {
                let loc = p.current_location();
                self.output_at(Point::from((loc.x.round() as i32, loc.y.round() as i32)))
            }
            None => self.primary_output(),
        }
    }

    /// The output a window `id` sits on, decided by its center point (live
    /// geometry preferred, else its target rect), falling back to primary.
    pub fn output_for_window(&self, id: u32) -> Option<smithay::output::Output> {
        let rect = self
            .window_body_rect(id)
            .or_else(|| self.windows.target_rect(id))?;
        let center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        self.output_at(center)
    }

    /// True when `output` carries a Metis edge bar layer surface. Side bars
    /// (left/right) don't set a layer-shell exclusive zone, so window placement
    /// reserves their strip manually — but only on the outputs that actually show
    /// a bar (e.g. not on secondaries in "primary display only"). Top/bottom bars
    /// reserve via exclusive zone.
    pub(crate) fn output_has_bar(&self, output: &smithay::output::Output) -> bool {
        layer_map_for_output(output)
            .layers()
            .any(|l| l.namespace() == "metis-bar")
    }

    /// Re-apply window geometry after the edge bar moves between reserved
    /// (top) and overlay (bottom/left/right) modes.
    pub fn reflow_for_bar_geometry_change(&mut self) {
        let ids: Vec<u32> = self.windows.ids();
        for id in ids {
            if self.windows.is_minimized(id) {
                continue;
            }
            if self.windows.get(id).is_some_and(|r| r.maximized) {
                self.reapply_maximized_geometry(id);
            } else if self.windows.is_snapped(id) {
                self.reflow_snapped_window(id);
            } else {
                self.apply_window_rect(id);
            }
        }
        self.sync_all_app_windows();
        self.refresh_all_scroll_offsets();
        self.arrange_layers();
        self.restore_focus_stacking();
        self.schedule_redraw();
    }

    /// Force every mapped toplevel to receive a configure (same size) so clients
    /// redraw after a DRM modeset or output layout change. Without this, GTK
    /// windows often keep a fully transparent buffer while SSD borders still draw.
    pub(crate) fn nudge_clients_after_output_change(&mut self) {
        let ids: Vec<u32> = self.windows.ids();
        for id in ids {
            if self.windows.is_minimized(id) {
                continue;
            }
            let Some(record) = self.windows.get(id).cloned() else {
                continue;
            };
            if !self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                continue;
            }
            let Some(loc) = self.space.element_location(&record.window) else {
                continue;
            };
            let size = record.window.geometry().size;
            self.send_window_configure(&record, loc, size);
        }
        self.schedule_redraw();
    }

    /// Re-clamp a snapped (non-maximized) window into the new placement zone after
    /// the edge bar moves.
    fn reflow_snapped_window(&mut self, id: u32) {
        let Some(mut rect) = self.windows.target_rect(id) else {
            return;
        };
        rect = self.clamp_rect_on_screen(rect);
        self.windows.set_target_rect(id, rect);
        self.apply_window_rect(id);
        self.reclamp_auto_hide(id);
    }

    /// Called when the bar layer commits; reflows windows if position changed.
    /// Also keeps the auto-hide cache warm (CSS hide does not always re-commit).
    pub(crate) fn on_bar_layer_committed(&mut self, output: &smithay::output::Output) {
        let cfg = metis_config::load_bar_config();
        let pos = cfg.position;
        if pos != self.last_bar_position {
            tracing::info!(?pos, "edge bar position changed — reflowing windows");
            self.last_bar_position = pos;
            self.blur.position = pos;
            self.reflow_for_bar_geometry_change();
        }
        self.sync_bar_auto_hidden_for(output);
    }

    /// Poll the shell auto-hide flag for input/hit-testing. Maximize/snap layout
    /// does not reflow on peek/reveal — the bar overlays. Reflow only when the
    /// `auto_hide` setting itself is toggled.
    fn maybe_sync_bar_auto_hidden(&mut self) {
        let cfg = metis_config::load_bar_config();
        if cfg.auto_hide != self.last_bar_auto_hide_enabled {
            self.last_bar_auto_hide_enabled = cfg.auto_hide;
            if !cfg.auto_hide {
                self.bar_auto_hidden.clear();
            }
            tracing::info!(
                auto_hide = cfg.auto_hide,
                "edge bar auto_hide setting changed — reflowing windows"
            );
            self.reflow_for_bar_geometry_change();
        }
        if !cfg.auto_hide {
            return;
        }
        let hidden = metis_protocol::bar_auto_hidden_flag();
        for output in self.space.outputs() {
            if self.output_has_bar(output) {
                self.bar_auto_hidden.insert(output.name(), hidden);
            }
        }
    }

    fn sync_bar_auto_hidden_for(&mut self, output: &smithay::output::Output) {
        let hidden = metis_protocol::bar_auto_hidden_flag();
        self.bar_auto_hidden.insert(output.name(), hidden);
    }

    /// Pointer is in the edge-bar reveal/hover strip. While peeked, only the
    /// thin peek band reveals the bar so maximized top titlebars stay usable.
    /// While shown, the full strip keeps hover so the bar does not hide mid-use.
    pub(crate) fn maybe_reveal_auto_hidden_bar(&mut self, location: Point<f64, Logical>) {
        use crate::desk_input::point_in_rect;

        // Freeze reveal/hide while the screenshot picker owns the pointer so the
        // capture matches what was on screen when the overlay opened.
        if self.screenshot_overlay_active() {
            return;
        }
        let cfg = metis_config::load_bar_config();
        if !cfg.auto_hide {
            return;
        }
        let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .is_some_and(|geo| geo.contains(location.to_i32_round()))
        }) else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let (x, y) = (location.x as i32, location.y as i32);
        let hidden = self.bar_is_auto_hidden(output);
        let strip = if hidden {
            Self::bar_peek_strip_rect(&output_geo)
        } else {
            Self::bar_config_strip_rect(&output_geo)
        };
        if !point_in_rect(x, y, strip) {
            return;
        }
        self.bar_edge_pointer_in = true;
        if self.bar_edge_cmd_suppressed() {
            return;
        }
        // Pulse while the pointer rests in the strip so a GTK leave into the
        // margin gap cannot complete a hide (shell hide grace is ~180ms).
        const PULSE_MS: u128 = 100;
        let now = std::time::Instant::now();
        if self
            .last_bar_reveal_cmd
            .is_some_and(|t| now.duration_since(t).as_millis() < PULSE_MS)
        {
            return;
        }
        self.last_bar_reveal_cmd = Some(now);
        let cmd = if hidden {
            "reveal-edge-bar"
        } else {
            "bar-edge-hover"
        };
        if let Err(err) = metis_protocol::write_runtime_command(cmd) {
            tracing::debug!(%err, %cmd, "failed to write bar edge command");
        }
    }

    /// Clear strip-hover latch when the pointer leaves the bar band, and tell
    /// the shell so auto-hide can run (hover alone used to leave pointer_over
    /// stuck true forever).
    pub(crate) fn maybe_clear_bar_edge_hover(&mut self, location: Point<f64, Logical>) {
        use crate::desk_input::point_in_rect;

        if self.screenshot_overlay_active() {
            return;
        }
        if !self.bar_edge_pointer_in {
            return;
        }
        if !metis_config::load_bar_config().auto_hide {
            self.bar_edge_pointer_in = false;
            return;
        }
        let Some(output) = self.space.outputs().find(|o| {
            self.space
                .output_geometry(o)
                .is_some_and(|geo| geo.contains(location.to_i32_round()))
        }) else {
            self.bar_edge_pointer_in = false;
            if !self.bar_edge_cmd_suppressed() {
                let _ = metis_protocol::write_runtime_command("bar-edge-leave");
            }
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            self.bar_edge_pointer_in = false;
            if !self.bar_edge_cmd_suppressed() {
                let _ = metis_protocol::write_runtime_command("bar-edge-leave");
            }
            return;
        };
        let (x, y) = (location.x as i32, location.y as i32);
        let hover_strip = if self.bar_is_auto_hidden(output) {
            Self::bar_peek_strip_rect(&output_geo)
        } else {
            Self::bar_config_strip_rect(&output_geo)
        };
        if !point_in_rect(x, y, hover_strip) {
            self.bar_edge_pointer_in = false;
            if self.bar_edge_cmd_suppressed() {
                return;
            }
            if let Err(err) = metis_protocol::write_runtime_command("bar-edge-leave") {
                tracing::debug!(%err, "failed to write bar-edge-leave");
            }
        }
    }

    fn bar_edge_cmd_suppressed(&self) -> bool {
        self.suppress_bar_edge_cmd_until
            .is_some_and(|until| std::time::Instant::now() < until)
    }

    /// Dismiss bar popovers / NC and briefly pause edge-hover IPC so the shell
    /// actually receives `close-popovers` (same command file as edge hover).
    pub(crate) fn request_close_bar_popovers(&mut self) {
        self.suppress_bar_edge_cmd_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(350));
        if let Err(err) = metis_protocol::write_runtime_command("close-popovers") {
            tracing::debug!(%err, "failed to write close-popovers");
        }
    }

    /// True when the shell reports the edge bar is auto-hidden (flag file).
    pub(crate) fn bar_is_auto_hidden(&self, output: &smithay::output::Output) -> bool {
        self.bar_auto_hidden
            .get(&output.name())
            .copied()
            .unwrap_or_else(metis_protocol::bar_auto_hidden_flag)
    }

    // --- Per-output desk helpers -----------------------------------------------

    /// The output with the given name, if mapped.
    pub fn output_by_name(&self, name: &str) -> Option<smithay::output::Output> {
        self.space.outputs().find(|o| o.name() == name).cloned()
    }

    /// Fractional scale of the output containing `window`, or `fallback` when the
    /// window is not on a client-visible output (e.g. during unmap transitions).
    pub(crate) fn window_output_scale(
        &self,
        window: &smithay::desktop::Window,
        fallback: smithay::utils::Scale<f64>,
    ) -> smithay::utils::Scale<f64> {
        let Some(loc) = self.space.element_location(window) else {
            return fallback;
        };
        let geo = window.geometry();
        let center = loc + (geo.size.to_f64() / 2.0).to_i32_round();
        for output in self.space.outputs() {
            if output.name() == "metis-render" {
                continue;
            }
            if let Some(out_geo) = self.space.output_geometry(output) {
                if out_geo.contains(center) {
                    return smithay::utils::Scale::from(output.current_scale().fractional_scale());
                }
            }
        }
        fallback
    }

    /// Desk key (output name) of the primary output, falling back to any existing
    /// desk, then the empty string before any output/desk exists.
    pub fn primary_key(&self) -> String {
        self.primary_output()
            .map(|o| o.name())
            .or_else(|| self.desks.keys().next().cloned())
            .unwrap_or_default()
    }

    /// Desk for an output key, if it exists.
    pub fn desk(&self, key: &str) -> Option<&OutputDesk> {
        self.desks.get(key)
    }

    /// Desk for an output key, creating it on demand. The first desk created is
    /// the primary (seeded with widgets from `desk.json`); later ones are app-only.
    pub fn desk_mut_or_default(&mut self, key: &str) -> &mut OutputDesk {
        if !self.desks.contains_key(key) {
            let is_primary = self.desks.is_empty();
            let mut layout = self.default_layout.clone();
            if !is_primary {
                layout
                    .tiles
                    .retain(|t| matches!(t.kind, TileKind::App { .. }));
            }
            self.desks.insert(
                key.to_string(),
                OutputDesk {
                    layout,
                    active_workspace: 1,
                    stashed_app_tiles: std::collections::HashMap::new(),
                    layout_kind: std::collections::HashMap::new(),
                    scroll: std::collections::HashMap::new(),
                },
            );
        }
        self.desks
            .get_mut(key)
            .expect("desk entry exists after insert-or-present check")
    }

    /// Ensure a desk exists for `output` (called when an output is mapped).
    pub fn ensure_desk_for_output(&mut self, output: &smithay::output::Output) {
        let key = output.name();
        let _ = self.desk_mut_or_default(&key);
    }

    /// Desk key (output name) a window belongs to. Prefers its assigned `output`,
    /// then the output under its geometry, then the primary.
    pub fn desk_key_for_window(&self, id: u32) -> String {
        if let Some(name) = self.windows.output_name(id) {
            if !name.is_empty() && self.desks.contains_key(&name) {
                return name;
            }
        }
        self.output_for_window(id)
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key())
    }

    /// Active workspace on the given output key (defaults to 1).
    pub fn active_workspace_for(&self, key: &str) -> u32 {
        self.desk(key).map(|d| d.active_workspace).unwrap_or(1)
    }

    // --- Scrolling layout -----------------------------------------------------

    /// Layout mode new workspaces start in (from `bar.json`).
    pub fn default_layout_kind(&self) -> metis_grid::LayoutKind {
        match metis_config::load_bar_config().default_layout {
            metis_config::DefaultLayout::Free => metis_grid::LayoutKind::Free,
            metis_config::DefaultLayout::Grid => metis_grid::LayoutKind::Grid,
            metis_config::DefaultLayout::Scroll => metis_grid::LayoutKind::Scroll,
        }
    }

    /// Layout mode of a specific workspace on an output (falls back to the default).
    pub fn layout_kind_for(&self, key: &str, ws: u32) -> metis_grid::LayoutKind {
        self.desk(key)
            .and_then(|d| d.layout_kind.get(&ws).copied())
            .unwrap_or_else(|| self.default_layout_kind())
    }

    /// Layout mode of the output's currently-active workspace.
    pub fn active_layout_kind(&self, key: &str) -> metis_grid::LayoutKind {
        self.layout_kind_for(key, self.active_workspace_for(key))
    }

    /// The bar-excluded usable zone for an output key, used as the scroll viewport.
    fn scroll_zone_for(&self, key: &str) -> PixelRect {
        match self.output_by_name(key) {
            Some(o) => self.window_placement_zone_for(&o),
            None => self.window_placement_zone(),
        }
    }

    /// Full (titlebar-inclusive) frames for the active scroll workspace on `key`,
    /// using the animated viewport offset (visual position during easing).
    pub(crate) fn scroll_frames_for(&self, key: &str) -> Vec<(u32, PixelRect)> {
        self.scroll_frames_at(key, false)
    }

    /// Full frames at the scroll target offset — where client surfaces are mapped.
    fn scroll_frames_placed_for(&self, key: &str) -> Vec<(u32, PixelRect)> {
        self.scroll_frames_at(key, true)
    }

    fn scroll_frames_at(&self, key: &str, placed: bool) -> Vec<(u32, PixelRect)> {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) != metis_grid::LayoutKind::Scroll {
            return Vec::new();
        }
        let Some(scroll) = self.desk(key).and_then(|d| d.scroll.get(&ws)) else {
            return Vec::new();
        };
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        if placed {
            scroll.layout_placed(zone, gutter)
        } else {
            scroll.layout(zone, gutter)
        }
    }

    /// Full frame for a single window when its workspace is the active scroll
    /// workspace on its output; `None` otherwise (for decorations / hit-testing).
    pub(crate) fn scroll_frame_for_window(&self, id: u32) -> Option<PixelRect> {
        let key = self.desk_key_for_window(id);
        self.scroll_frames_for(&key)
            .into_iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, rect)| rect)
    }

    /// Mapped (target-offset) frame for scroll-managed window placement.
    fn scroll_frame_placed_for_window(&self, id: u32) -> Option<PixelRect> {
        let key = self.desk_key_for_window(id);
        self.scroll_frames_placed_for(&key)
            .into_iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, rect)| rect)
    }

    /// Render-time X offset for a scroll-managed window while the viewport eases.
    pub(crate) fn scroll_render_nudge(&self, id: u32) -> i32 {
        if !self.is_active_scroll_window(id) {
            return 0;
        }
        let key = self.desk_key_for_window(id);
        let ws = self.active_workspace_for(&key);
        let Some(scroll) = self.desk(&key).and_then(|d| d.scroll.get(&ws)) else {
            return 0;
        };
        scroll.scroll_x_target - scroll.scroll_x
    }

    /// Mutable scroll state for an output's workspace, creating it on demand.
    fn scroll_state_mut(&mut self, key: &str, ws: u32) -> &mut metis_grid::ScrollState {
        self.desk_mut_or_default(key).scroll.entry(ws).or_default()
    }

    /// Recompute the scroll offset for an output's active workspace so the focused
    /// column is visible. When `animate` is true the viewport eases toward the
    /// target via render-time translation (no per-frame client reconfigure).
    fn refresh_scroll_offset(&mut self, key: &str, animate: bool) {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) != metis_grid::LayoutKind::Scroll {
            return;
        }
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        if let Some(scroll) = self.desk_mut_or_default(key).scroll.get_mut(&ws) {
            let target = scroll.desired_scroll_x(zone, gutter);
            scroll.set_scroll_target(target, zone, gutter);
            if !animate {
                scroll.snap_scroll();
            }
        }
    }

    /// Update the scroll viewport and remap windows to the target strip layout.
    fn apply_scroll_viewport(&mut self, key: &str, animate: bool) {
        self.refresh_scroll_offset(key, animate);
        self.reposition_scroll_windows();
        if animate {
            self.damaged = true;
            self.request_redraw();
        }
    }

    /// Re-snap every active scroll workspace after the usable zone changes.
    pub(crate) fn refresh_all_scroll_offsets(&mut self) {
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll {
                self.refresh_scroll_offset(&key, false);
            }
        }
    }

    /// Advance scroll-strip animations on every output; returns true while any strip
    /// is still easing toward its target.
    pub fn tick_scroll_animations(&mut self) -> bool {
        let now = std::time::Instant::now();
        let dt = self
            .last_scroll_tick
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016);
        self.last_scroll_tick = Some(now);

        let keys: Vec<String> = self.desks.keys().cloned().collect();
        let mut moved = false;
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
                continue;
            }
            if let Some(scroll) = self.desk_mut_or_default(&key).scroll.get_mut(&ws) {
                if scroll.scroll_x != scroll.scroll_x_target {
                    moved |= scroll.advance_scroll_animation(dt);
                }
            }
        }
        if moved {
            self.request_redraw();
        }
        moved
    }

    /// Advance auto-hide titlebar slide animations. Returns true while a reveal or
    /// hide is still in progress.
    pub fn tick_titlebar_reveal_animation(&mut self) -> bool {
        const DURATION_SECS: f32 = 0.2;

        let now = std::time::Instant::now();
        let dt = self
            .last_titlebar_reveal_tick
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.016);
        self.last_titlebar_reveal_tick = Some(now);

        let target = if self.revealed_titlebar.is_some() {
            1.0
        } else {
            0.0
        };

        if let Some(id) = self.revealed_titlebar {
            self.titlebar_reveal_window = Some(id);
        }

        if self.titlebar_reveal_window.is_none() {
            return false;
        }

        let before = self.titlebar_reveal_progress;
        if (before - target).abs() < 0.001 {
            self.titlebar_reveal_progress = target;
            if target <= 0.0 {
                self.titlebar_reveal_window = None;
            }
            return false;
        }

        let step = dt / DURATION_SECS;
        self.titlebar_reveal_progress = if !crate::window_fx::animations_enabled() {
            target
        } else if target > before {
            (before + step).min(target)
        } else {
            (before - step).max(target)
        };

        if self.titlebar_reveal_progress <= 0.0 {
            self.titlebar_reveal_window = None;
        }

        if (self.titlebar_reveal_progress - before).abs() > f32::EPSILON {
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Per-edge outward ripple (top, right, bottom, left) in logical px during the
    /// post-maximize wobble. Zero when the effect has finished.
    pub(crate) fn maximize_wobble_offset(&self, id: u32) -> (i32, i32) {
        const DURATION_SECS: f32 = 0.55;
        const AMP_PX: f32 = 14.0;
        const FREQ_HZ: f32 = 5.5;

        let Some(start) = self.maximize_fx_started.get(&id) else {
            return (0, 0);
        };
        let elapsed = start.elapsed().as_secs_f32();
        if elapsed >= DURATION_SECS {
            return (0, 0);
        }
        let decay = (1.0 - elapsed / DURATION_SECS).powi(2);
        let amp = AMP_PX * decay;
        let phase = elapsed * FREQ_HZ * std::f32::consts::TAU;
        (
            (amp * phase.sin()).round() as i32,
            (amp * (phase * 1.37).sin()).round() as i32,
        )
    }

    fn window_base_location(&self, id: u32) -> Option<Point<i32, Logical>> {
        let record = self.windows.get(id)?;
        if record.maximized {
            return self
                .maximized_client_geometry(id)
                .map(|(client, _)| Point::from((client.x, client.y)));
        }
        Some(Point::from((record.target_rect.x, record.target_rect.y)))
    }

    fn apply_maximize_wobble(&mut self, id: u32) {
        let (dx, dy) = self.maximize_wobble_offset(id);
        if dx == 0 && dy == 0 && !self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(base) = self.window_base_location(id) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id))
        {
            return;
        }
        self.space
            .relocate_element(&record.window, Point::from((base.x + dx, base.y + dy)));
        self.schedule_redraw();
    }

    fn snap_maximize_wobble(&mut self, id: u32) {
        let Some(base) = self.window_base_location(id) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id))
        {
            self.space.relocate_element(&record.window, base);
            self.schedule_redraw();
        }
    }

    /// End post-maximize wobble for `id` and snap the map origin back to base.
    fn clear_maximize_fx(&mut self, id: u32) {
        if self.maximize_fx_started.remove(&id).is_some() {
            self.snap_maximize_wobble(id);
        }
    }

    fn start_maximize_fx(&mut self, id: u32) {
        if !crate::window_fx::animations_enabled() {
            return;
        }
        if self.window_uses_compact_overlay(id) {
            return;
        }
        // Do not restart an in-flight wobble — key-repeat / rapid toggles used to
        // reset the timer forever so relocate fought drags and stalled the loop.
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        self.maximize_fx_started
            .insert(id, std::time::Instant::now());
        self.apply_maximize_wobble(id);
    }

    /// Advance post-maximize wobble animations. Returns true while any are active.
    pub fn tick_maximize_fx(&mut self) -> bool {
        const DURATION_SECS: f32 = 0.55;
        // Do **not** cancel on any pointer grab — a titlebar maximize click can
        // leave a short-lived button grab that would kill the wobble on the
        // first tick. Move/resize paths call [`Self::clear_maximize_fx`] when
        // they take ownership of the map origin.
        let active_ids: Vec<u32> = self.maximize_fx_started.keys().copied().collect();
        for id in active_ids {
            self.apply_maximize_wobble(id);
        }
        let ended: Vec<u32> = self
            .maximize_fx_started
            .iter()
            .filter(|(_, t)| t.elapsed().as_secs_f32() >= DURATION_SECS)
            .map(|(id, _)| *id)
            .collect();
        for id in ended {
            self.maximize_fx_started.remove(&id);
            self.snap_maximize_wobble(id);
        }
        !self.maximize_fx_started.is_empty()
    }

    pub(crate) fn window_uses_compact_overlay(&self, id: u32) -> bool {
        self.windows
            .get(id)
            .and_then(|r| r.app_id.as_deref())
            .is_some_and(crate::decoration_policy::id_uses_compact_overlay)
    }

    fn minimize_visual_bounds(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        let loc = self.space.element_location(&record.window)?;
        if let Some(frame) = self.ssd_frame_for_mapped_window(id, &record.window) {
            return Some(frame);
        }
        let geo = record.window.geometry();
        Some(PixelRect {
            x: loc.x,
            y: loc.y,
            width: geo.size.w.max(1),
            height: geo.size.h.max(1),
        })
    }

    fn genie_minimize_target(&self, anchor: &PixelRect, id: u32) -> Option<Point<i32, Logical>> {
        let output = self
            .output_for_window(id)
            .or_else(|| self.primary_output())?;
        if !self.output_has_bar(&output) {
            return None;
        }
        let cfg = metis_config::load_bar_config();
        let margin = cfg.margin_top as i32;
        let half = cfg.height as i32 / 2;
        let zone = self.placement_zone_for(&output);
        let usable = self.usable_zone_for(&output).unwrap_or(zone);
        let cx = (anchor.x + anchor.width / 2).clamp(zone.x + 40, zone.x + zone.width - 40);
        let cy = anchor.y + anchor.height / 2;
        Some(match cfg.position {
            metis_config::BarPosition::Top => Point::from((cx, usable.y - margin - half)),
            metis_config::BarPosition::Bottom => {
                Point::from((cx, zone.y + zone.height - margin - half))
            }
            metis_config::BarPosition::Left => Point::from((zone.x + margin + half, cy)),
            metis_config::BarPosition::Right => {
                Point::from((zone.x + zone.width - margin - half, cy))
            }
        })
    }

    fn begin_minimize_genie(&mut self, id: u32) -> bool {
        if self.windows.is_minimized(id) {
            return false;
        }
        let anchor = if self.windows.get(id).is_some_and(|r| r.maximized) {
            self.maximized_client_geometry(id).map(|(c, _)| c)
        } else {
            self.minimize_visual_bounds(id)
        };
        let Some(anchor) = anchor else {
            return false;
        };
        let Some(target) = self.genie_minimize_target(&anchor, id) else {
            return false;
        };
        self.minimize_genie_fx.insert(
            id,
            crate::window_fx::MinimizeGenieFx {
                started: std::time::Instant::now(),
                anchor,
                target,
            },
        );
        self.schedule_redraw();
        true
    }

    fn tick_minimize_genie_fx(&mut self) -> bool {
        let finished: Vec<u32> = self
            .minimize_genie_fx
            .iter()
            .filter(|(_, fx)| fx.finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            self.minimize_genie_fx.remove(&id);
            self.minimize_window_now(id);
        }
        !self.minimize_genie_fx.is_empty()
    }

    /// Render clip + alpha for an in-flight minimize genie, if any.
    pub(crate) fn minimize_genie_render(&self, id: u32) -> Option<(PixelRect, f32)> {
        self.minimize_genie_fx.get(&id).map(|fx| fx.frame())
    }

    pub(crate) fn is_minimize_genie_active(&self, id: u32) -> bool {
        self.minimize_genie_fx.contains_key(&id)
    }

    /// Window ids slotted on a specific (output, workspace), in tile order.
    fn app_ids_for_workspace(&self, key: &str, ws: u32) -> Vec<u32> {
        let active = self.active_workspace_for(key);
        let collect_ids = |tiles: &[metis_grid::GridTile]| -> Vec<u32> {
            tiles
                .iter()
                .filter_map(|t| match &t.kind {
                    TileKind::App {
                        window_id: Some(wid),
                        ..
                    } => Some(*wid),
                    _ => None,
                })
                .collect()
        };
        self.desk(key)
            .map(|d| {
                if ws == active {
                    collect_ids(&d.layout.tiles)
                } else {
                    d.stashed_app_tiles
                        .get(&ws)
                        .map(|t| collect_ids(t))
                        .unwrap_or_default()
                }
            })
            .unwrap_or_default()
    }

    /// App windows positioned by the scroll strip (excludes free-floating clients).
    fn scroll_managed_app_ids(&self, key: &str, ws: u32) -> Vec<u32> {
        self.app_ids_for_workspace(key, ws)
            .into_iter()
            .filter(|id| !self.floating.contains(id))
            .collect()
    }

    /// True when the scroll strip lists exactly the workspace's app windows.
    fn scroll_strip_matches(app_ids: &[u32], scroll: &metis_grid::ScrollState) -> bool {
        use std::collections::HashSet;
        let strip: HashSet<u32> = scroll
            .columns
            .iter()
            .flat_map(|c| c.windows.iter().copied())
            .collect();
        let tiles: HashSet<u32> = app_ids.iter().copied().collect();
        strip == tiles
    }

    /// Window ids on a specific (output, workspace).
    pub(crate) fn window_ids_on_workspace(&self, key: &str, ws: u32) -> Vec<u32> {
        self.windows
            .ids()
            .into_iter()
            .filter(|id| {
                self.desk_key_for_window(*id) == key
                    && self.windows.workspace(*id).unwrap_or(1) == ws
            })
            .collect()
    }

    /// Bottom-to-top window order for workspace mini-desktop thumbs.
    ///
    /// Mapped windows follow `space.elements()` so raising/focusing an already-
    /// open app updates which window appears on top in the shelf preview.
    /// Unmapped windows (inactive workspaces) are appended afterward.
    pub(crate) fn workspace_thumb_stack_order(&self, key: &str, ws: u32) -> Vec<u32> {
        let mut ordered = Vec::new();
        for window in self.space.elements() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.desk_key_for_window(id) != key {
                continue;
            }
            if self.windows.workspace(id).unwrap_or(1) != ws {
                continue;
            }
            ordered.push(id);
        }
        for id in self.window_ids_on_workspace(key, ws) {
            if !ordered.contains(&id) {
                ordered.push(id);
            }
        }
        ordered
    }

    /// True when a window belongs to the active workspace on its output and may
    /// be mapped (minimized windows are handled separately).
    fn window_visible_on_desktop(&self, id: u32) -> bool {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        ws == self.active_workspace_for(&key)
    }

    /// Best-effort client body rect for a mapped or placed window.
    pub(crate) fn current_window_body_rect(&self, id: u32) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        if let Some(loc) = self.space.element_location(&record.window) {
            let size = record.window.geometry().size;
            if size.w > 0 && size.h > 0 {
                return Some(PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: size.w,
                    height: size.h,
                });
            }
        }
        self.windows.target_rect(id).or_else(|| {
            self.rect_for_window_tile(id)
                .map(|full| self.tile_client_rect(id, full))
        })
    }

    /// Drop grid/scroll management for a workspace — windows keep their on-screen
    /// geometry and float freely.
    fn release_workspace_to_free(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            if let Some(body) = self.current_window_body_rect(id) {
                self.windows.set_target_rect(id, body);
            }
            self.floating.insert(id);
            self.sync_auto_hide_titlebar(id);
            self.save_window_geometry(id);
        }
        let active = self.active_workspace_for(key);
        let desk = self.desk_mut_or_default(key);
        if ws == active {
            desk.layout
                .tiles
                .retain(|t| !matches!(t.kind, TileKind::App { .. }));
        } else {
            desk.stashed_app_tiles.remove(&ws);
        }
        desk.scroll.remove(&ws);
    }

    /// Pull every window on a workspace into the grid and reserve tiles.
    fn adopt_workspace_to_grid(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            self.floating.remove(&id);
            self.ensure_app_tile_for_window(id);
        }
    }

    /// Pull every window on a workspace into the scroll strip.
    fn adopt_workspace_to_scroll(&mut self, key: &str, ws: u32) {
        for id in self.window_ids_on_workspace(key, ws) {
            self.floating.remove(&id);
            self.ensure_app_tile_for_window(id);
        }
        self.seed_scroll_state(key, ws);
    }

    /// Build or refresh the scroll strip for a workspace from its app tiles.
    fn seed_scroll_state(&mut self, key: &str, ws: u32) {
        let app_ids = self.scroll_managed_app_ids(key, ws);
        let focused = self.focused_window_id();
        let zone = self.scroll_zone_for(key);
        let gutter = self.gutter_px as i32;
        let desk = self.desk_mut_or_default(key);
        let needs_rebuild = desk.scroll.get(&ws).is_none_or(|s| {
            (s.columns.is_empty() && !app_ids.is_empty())
                || !Self::scroll_strip_matches(&app_ids, s)
        });
        if needs_rebuild {
            let mut scroll = metis_grid::ScrollState::new();
            for wid in &app_ids {
                scroll.insert_window_after_focus(*wid);
            }
            if let Some(f) = focused {
                scroll.focus_window(f);
            }
            desk.scroll.insert(ws, scroll);
        }
        if let Some(scroll) = desk.scroll.get_mut(&ws) {
            let target = scroll.desired_scroll_x(zone, gutter);
            scroll.set_scroll_target(target, zone, gutter);
            scroll.snap_scroll();
        }
    }

    /// Re-position only the windows on active scroll workspaces (used during
    /// viewport animation so we don't reconfigure every client every frame).
    pub(crate) fn reposition_scroll_windows(&mut self) {
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        for key in keys {
            let ws = self.active_workspace_for(&key);
            if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
                continue;
            }
            for id in self.scroll_managed_app_ids(&key, ws) {
                self.apply_window_rect(id);
            }
        }
    }

    /// True when `id` belongs to the active scroll workspace on its output.
    fn is_active_scroll_window(&self, id: u32) -> bool {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        ws == self.active_workspace_for(&key)
            && self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll
    }

    /// Physical-space clip rect for a scroll-managed window so its column can
    /// scroll off its own display's edge (carousel) without bleeding onto an
    /// adjacent output. Returns `None` for windows that aren't scroll-managed —
    /// those must not be clipped (e.g. a floating window dragged across outputs).
    pub(crate) fn scroll_window_clip(
        &self,
        id: u32,
        scale: impl Into<smithay::utils::Scale<f64>>,
    ) -> Option<smithay::utils::Rectangle<i32, smithay::utils::Physical>> {
        if !self.is_active_scroll_window(id) {
            return None;
        }
        let key = self.desk_key_for_window(id);
        let output = self.output_by_name(&key)?;
        let geo = self.space.output_geometry(&output)?;
        Some(geo.to_physical_precise_round(scale))
    }

    /// Resolve a border drag on scroll window `id` to the column it should resize.
    /// The right edge grows this window's column; the left edge grows the previous
    /// column (the shared border). Returns a representative window of the target
    /// column plus that column's current pixel width, or `None` when the drag isn't
    /// a horizontal resize of a scroll column (e.g. left edge of the first column).
    pub(crate) fn scroll_resize_target(
        &self,
        id: u32,
        edges: crate::grabs::ResizeEdge,
    ) -> Option<(u32, i32)> {
        use crate::grabs::ResizeEdge;
        if !self.is_active_scroll_window(id) {
            return None;
        }
        let key = self.desk_key_for_window(id);
        let ws = self.active_workspace_for(&key);
        let scroll = self.desk(&key)?.scroll.get(&ws)?;
        let ci = scroll.column_index_of(id)?;
        let target_ci = if edges.contains(ResizeEdge::RIGHT) {
            ci
        } else if edges.contains(ResizeEdge::LEFT) {
            ci.checked_sub(1)?
        } else {
            return None;
        };
        let zone = self.scroll_zone_for(&key);
        let target_window = *scroll.columns.get(target_ci)?.windows.first()?;
        Some((target_window, scroll.column_width_px(target_ci, zone)))
    }

    /// Set the pixel width of the scroll column holding `target_window` and reflow
    /// the strip so the columns to its right slide over to make room. Driven live
    /// from [`crate::grabs::ScrollResizeGrab`] during a mouse resize.
    pub(crate) fn scroll_set_column_width_px(&mut self, target_window: u32, width_px: i32) {
        let key = self.desk_key_for_window(target_window);
        let ws = self.active_workspace_for(&key);
        let zone = self.scroll_zone_for(&key);
        if let Some(scroll) = self.desk_mut_or_default(&key).scroll.get_mut(&ws) {
            if !scroll.set_column_width_px_for(target_window, width_px, zone) {
                return;
            }
        }
        self.refresh_scroll_offset(&key, false);
        self.reposition_scroll_windows();
        self.damaged = true;
        self.request_redraw();
    }

    /// Drop a window from every output's scroll state (used on destroy / move).
    fn remove_from_scroll_everywhere(&mut self, id: u32) {
        for desk in self.desks.values_mut() {
            for scroll in desk.scroll.values_mut() {
                scroll.remove_window(id);
            }
        }
    }

    /// Set the layout mode of a specific (output, workspace) without repositioning.
    /// Entering scroll seeds the strip from that workspace's app tiles (visible or
    /// stashed); leaving scroll drops the strip and de-overlaps the visible grid.
    fn set_layout_kind_on(&mut self, key: &str, ws: u32, kind: metis_grid::LayoutKind) {
        let active = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) == kind {
            // Pin an explicit entry so a later default change can't silently flip it.
            self.desk_mut_or_default(key).layout_kind.insert(ws, kind);
            // Still sync backing state. When bar.json already matches the target
            // (e.g. SetDefaultLayout after saving the dropdown), the early return
            // used to skip seeding scroll strips entirely.
            match kind {
                metis_grid::LayoutKind::Scroll => self.seed_scroll_state(key, ws),
                metis_grid::LayoutKind::Grid => {
                    let desk = self.desk_mut_or_default(key);
                    desk.scroll.remove(&ws);
                    if ws == active {
                        metis_grid::sanitize_layout(&mut desk.layout);
                    }
                }
                metis_grid::LayoutKind::Free => {
                    self.desk_mut_or_default(key).scroll.remove(&ws);
                }
            }
            return;
        }

        match kind {
            metis_grid::LayoutKind::Scroll => {
                self.seed_scroll_state(key, ws);
                self.desk_mut_or_default(key).layout_kind.insert(ws, kind);
            }
            metis_grid::LayoutKind::Grid => {
                let desk = self.desk_mut_or_default(key);
                desk.layout_kind.insert(ws, kind);
                desk.scroll.remove(&ws);
                if ws == active {
                    metis_grid::sanitize_layout(&mut desk.layout);
                }
            }
            metis_grid::LayoutKind::Free => {
                let desk = self.desk_mut_or_default(key);
                desk.layout_kind.insert(ws, kind);
                desk.scroll.remove(&ws);
            }
        }
    }

    /// Set the layout mode of an output's active workspace and apply it live.
    pub fn set_layout_kind(&mut self, key: &str, kind: metis_grid::LayoutKind) {
        let ws = self.active_workspace_for(key);
        if self.layout_kind_for(key, ws) == kind {
            return;
        }
        let focused = self.focused_window_id();
        self.set_layout_kind_on(key, ws, kind);
        match kind {
            metis_grid::LayoutKind::Grid => {
                self.adopt_workspace_to_grid(key, ws);
                self.auto_reflow_grid_apps(key, focused, false);
            }
            metis_grid::LayoutKind::Scroll => {
                self.adopt_workspace_to_scroll(key, ws);
                self.refresh_scroll_offset(key, false);
                self.reposition_scroll_windows();
            }
            metis_grid::LayoutKind::Free => {
                self.release_workspace_to_free(key, ws);
                self.reposition_all_windows();
                self.persist_layout();
            }
        }
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
    }

    /// Apply a layout mode to every workspace on every output at once, so the
    /// settings "New workspace layout" default behaves as a live global on/off.
    pub fn set_layout_kind_all(&mut self, kind: metis_grid::LayoutKind) {
        let count = self.workspace_count();
        let keys: Vec<String> = self.desks.keys().cloned().collect();
        let focused = self.focused_window_id();
        for key in &keys {
            for ws in 1..=count {
                self.set_layout_kind_on(key, ws, kind);
            }
            match kind {
                metis_grid::LayoutKind::Free => {
                    for ws in 1..=count {
                        self.release_workspace_to_free(key, ws);
                    }
                }
                metis_grid::LayoutKind::Grid => {
                    for ws in 1..=count {
                        self.adopt_workspace_to_grid(key, ws);
                    }
                    self.auto_reflow_grid_apps(key, focused, false);
                }
                metis_grid::LayoutKind::Scroll => {
                    for ws in 1..=count {
                        self.adopt_workspace_to_scroll(key, ws);
                    }
                    self.refresh_scroll_offset(key, false);
                }
            }
        }
        self.reposition_all_windows();
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
    }

    /// Turn on grid tiling for the active workspace under `key`.
    pub fn enable_grid_tiling(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);
        if self.active_layout_kind(key) == metis_grid::LayoutKind::Grid {
            return;
        }
        tracing::info!(output = key, "enable_grid_tiling");
        self.set_layout_kind(key, metis_grid::LayoutKind::Grid);
    }

    /// Return the active workspace to a normal floating desktop (grid/scroll off).
    pub fn disable_grid_tiling(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);
        if self.active_layout_kind(key) == metis_grid::LayoutKind::Free {
            return;
        }
        tracing::info!(output = key, "disable_grid_tiling");
        self.set_layout_kind(key, metis_grid::LayoutKind::Free);
    }

    /// Cycle the active workspace: free desktop → grid tiling → scrolling.
    pub fn toggle_layout_kind(&mut self, key: &str) {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
        let now = std::time::Instant::now();
        if self
            .last_layout_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return;
        }
        self.last_layout_toggle = Some(now);

        let next = match self.active_layout_kind(key) {
            metis_grid::LayoutKind::Free => metis_grid::LayoutKind::Grid,
            metis_grid::LayoutKind::Grid => metis_grid::LayoutKind::Scroll,
            metis_grid::LayoutKind::Scroll => metis_grid::LayoutKind::Free,
        };
        tracing::info!(output = key, ?next, "toggle_layout_kind");
        self.set_layout_kind(key, next);
    }

    /// Give a window keyboard focus and raise it (mirrors `activate_window`'s tail).
    pub fn focus_window_id(&mut self, id: u32) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.note_window_focus(id);
        self.space.raise_element(&record.window, true);
        if self.focused_window_id() == Some(id) {
            return;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
            // Keyboard-focus diagnostics: the reported "mouse works but Esc/keys
            // don't reach the game" is a keyboard-focus problem, so log exactly
            // which window (and whether its XWayland surface is associated yet —
            // an X11 game that recreates its window on a video-mode change can be
            // focused before its `wl_surface` is linked, which drops key delivery).
            use smithay::wayland::seat::WaylandFocus;
            tracing::info!(
                id,
                app_id = ?record.app_id,
                is_x11 = record.is_x11,
                has_wl_surface = record.window.wl_surface().is_some(),
                "focus: keyboard focus set"
            );
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
        }
    }

    /// Apply a scroll action to the output under the pointer's active scroll
    /// workspace, then reposition and refocus. No-op unless that workspace is in
    /// scroll mode.
    fn with_active_scroll<F: FnOnce(&mut metis_grid::ScrollState)>(&mut self, f: F) -> bool {
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        let ws = self.active_workspace_for(&key);
        if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
            return false;
        }
        {
            let scroll = self.scroll_state_mut(&key, ws);
            f(scroll);
        }
        self.apply_scroll_viewport(&key, true);
        let focused = self
            .desk(&key)
            .and_then(|d| d.scroll.get(&ws))
            .and_then(|s| s.focused_window());
        if let Some(f) = focused {
            self.focus_window_id(f);
        }
        self.damaged = true;
        self.request_redraw();
        true
    }

    /// Keep the scroll strip's focused column aligned with keyboard focus.
    pub fn sync_scroll_focus_for_window(&mut self, id: u32) {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if ws != self.active_workspace_for(&key) {
            return;
        }
        if self.layout_kind_for(&key, ws) != metis_grid::LayoutKind::Scroll {
            return;
        }
        let changed = self
            .desk_mut_or_default(&key)
            .scroll
            .get_mut(&ws)
            .map(|scroll| {
                let before = scroll.focused_window();
                scroll.focus_window(id);
                before != scroll.focused_window()
            })
            .unwrap_or(false);
        if changed {
            self.apply_scroll_viewport(&key, true);
        }
    }

    pub fn scroll_focus_left(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_left())
    }
    pub fn scroll_focus_right(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_right())
    }
    pub fn scroll_focus_up(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_up())
    }
    pub fn scroll_focus_down(&mut self) -> bool {
        self.with_active_scroll(|s| s.focus_down())
    }
    pub fn scroll_move_left(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_column_left())
    }
    pub fn scroll_move_right(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_column_right())
    }
    pub fn scroll_move_up(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_window_up())
    }
    pub fn scroll_move_down(&mut self) -> bool {
        self.with_active_scroll(|s| s.move_window_down())
    }
    pub fn scroll_consume(&mut self) -> bool {
        self.with_active_scroll(|s| s.consume_into_prev())
    }
    pub fn scroll_expel(&mut self) -> bool {
        self.with_active_scroll(|s| s.expel_to_new_column())
    }
    pub fn scroll_cycle_width(&mut self) -> bool {
        self.with_active_scroll(|s| s.cycle_focus_width())
    }

    /// Grid metrics (columns/rows/gutter + monitor rect) for a specific output.
    pub fn grid_metrics_for(&self, output: &smithay::output::Output) -> GridMetrics {
        let key = output.name();
        let (columns, rows) = self
            .desk(&key)
            .map(|d| (d.layout.columns, d.layout.rows))
            .unwrap_or((self.default_layout.columns, self.default_layout.rows));
        let zone = self.grid_placement_zone_for(output);
        GridMetrics {
            columns,
            rows,
            gutter: self.gutter_px,
            monitor: MonitorRect {
                x: zone.x,
                y: zone.y,
                width: zone.width,
                height: zone.height,
            },
        }
    }

    /// Usable desktop band for grid tiling on `output` (below/ beside the edge bar).
    fn grid_placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        let mut zone = self.window_placement_zone_for(output);
        if !self.output_has_bar(output) {
            return zone;
        }
        let Some(output_geo) = self.output_rect(output) else {
            return zone;
        };
        let reserve = self.bar_reserved_px_for(output);
        let gaps = self.zone_edge_gaps();
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                let min_y = output_geo.y + reserve + gaps.top;
                if zone.y < min_y {
                    let delta = min_y - zone.y;
                    zone.y = min_y;
                    zone.height = (zone.height - delta).max(1);
                }
            }
            metis_config::BarPosition::Bottom => {
                zone.height = (zone.height - gaps.bottom).max(1);
            }
            metis_config::BarPosition::Left => {
                let min_x = output_geo.x + reserve + gaps.left;
                if zone.x < min_x {
                    let delta = min_x - zone.x;
                    zone.x = min_x;
                    zone.width = (zone.width - delta).max(1);
                }
            }
            metis_config::BarPosition::Right => {
                zone.width = (zone.width - gaps.right).max(1);
            }
        }
        zone
    }

    /// Hide persistent titlebars for grid-tiled windows; reveal on hover.
    fn sync_grid_titlebar_chrome(&mut self, output_key: &str) {
        let ws = self.active_workspace_for(output_key);
        if self.layout_kind_for(output_key, ws) != metis_grid::LayoutKind::Grid {
            return;
        }
        for id in self.window_ids_on_workspace(output_key, ws) {
            if self.tile_id_for_window(id).is_some()
                && !self.floating.contains(&id)
                && self.should_auto_hide_titlebar(id)
            {
                self.auto_hide_titlebar.insert(id);
            } else {
                self.sync_auto_hide_titlebar(id);
            }
        }
    }

    /// Grid metrics for the primary output (back-compat for output-agnostic call sites).
    pub fn grid_metrics(&self) -> GridMetrics {
        match self.primary_output() {
            Some(o) => self.grid_metrics_for(&o),
            None => GridMetrics {
                columns: self.default_layout.columns,
                rows: self.default_layout.rows,
                gutter: self.gutter_px,
                monitor: self.monitor,
            },
        }
    }

    /// Find the app tile for `window_id` across all outputs' visible layouts,
    /// returning its output key and a clone of the tile.
    pub fn find_app_tile(&self, window_id: u32) -> Option<(String, metis_grid::GridTile)> {
        for (key, desk) in &self.desks {
            for tile in &desk.layout.tiles {
                if let TileKind::App {
                    window_id: Some(wid),
                    ..
                } = &tile.kind
                {
                    if *wid == window_id {
                        return Some((key.clone(), tile.clone()));
                    }
                }
            }
        }
        None
    }

    /// Output key whose visible layout currently contains `tile_id`.
    pub fn desk_key_for_tile(&self, tile_id: &str) -> Option<String> {
        self.desks.iter().find_map(|(key, desk)| {
            desk.layout
                .tiles
                .iter()
                .any(|t| t.id == tile_id)
                .then(|| key.clone())
        })
    }

    /// Drop app tiles whose window no longer exists and dedupe multiple tiles for
    /// the same live window (stale `desk.json` entries otherwise block reflow).
    fn prune_stale_app_tiles(&mut self, output_key: &str) {
        use std::collections::{HashMap, HashSet};

        let live: HashSet<u32> = self.windows.ids().into_iter().collect();
        let desk = self.desk_mut_or_default(output_key);

        let prune_list = |tiles: &mut Vec<metis_grid::GridTile>| {
            tiles.retain(|t| match &t.kind {
                TileKind::App {
                    window_id: Some(wid),
                    ..
                } => live.contains(wid),
                TileKind::App {
                    window_id: None, ..
                } => false,
                _ => true,
            });
            let mut keep: HashMap<u32, String> = HashMap::new();
            for t in tiles.iter() {
                let TileKind::App {
                    window_id: Some(wid),
                    ..
                } = &t.kind
                else {
                    continue;
                };
                let canonical = format!("app-{wid}");
                keep.entry(*wid).or_insert_with(|| t.id.clone());
                if t.id == canonical {
                    keep.insert(*wid, canonical);
                }
            }
            tiles.retain(|t| match &t.kind {
                TileKind::App {
                    window_id: Some(wid),
                    ..
                } => keep.get(wid) == Some(&t.id),
                _ => true,
            });
        };

        prune_list(&mut desk.layout.tiles);
        for tiles in desk.stashed_app_tiles.values_mut() {
            prune_list(tiles);
        }
    }

    /// Drop a window's app tile from every desk (visible and stashed).
    pub(crate) fn remove_app_tile_everywhere(&mut self, window_id: u32) {
        let matches_window = |t: &metis_grid::GridTile| matches!(&t.kind, TileKind::App { window_id: Some(wid), .. } if *wid == window_id);
        for desk in self.desks.values_mut() {
            desk.layout.tiles.retain(|t| !matches_window(t));
            for tiles in desk.stashed_app_tiles.values_mut() {
                tiles.retain(|t| !matches_window(t));
            }
            for scroll in desk.scroll.values_mut() {
                scroll.remove_window(window_id);
            }
        }
    }

    pub fn focused_window_id(&self) -> Option<u32> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        match focus {
            KeyboardFocusTarget::Window(window) => self.windows.id_for_window(&window),
            _ => None,
        }
    }

    pub(crate) fn note_window_focus(&mut self, id: u32) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        self.last_focused_window = Some(id);
        // Keep Alt+Tab thumbnails fresh whenever a window is brought forward.
        self.queue_window_thumb(id);
    }

    /// True for a *running game* — as opposed to a launcher/store (Steam, Lutris,
    /// Heroic). Covers Proton/Wine game windows (`steam_app_*`, `*.exe`), the
    /// Hytale game client, and any true-fullscreen window (native games).
    ///
    /// Used for focus-stealing prevention: while a game holds focus, a background
    /// app (notably Steam, which fires `_NET_ACTIVE_WINDOW` at itself for tray
    /// updates/notifications) must not yank the game to the background — that both
    /// pops the launcher over the game and drops the game's keyboard focus + pointer
    /// lock, so Esc / movement keys stop reaching it.
    pub(crate) fn window_is_running_game(&self, id: u32) -> bool {
        let Some(record) = self.windows.get(id) else {
            return false;
        };
        if record.fullscreen {
            return true;
        }
        let app = record
            .app_id
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        app.starts_with("steam_app_")
            || app.contains(".exe")
            || app.contains("proton")
            || app == "hytaleclient"
    }

    /// Window the user last brought forward (taskbar, Alt+Tab path, etc.), falling
    /// back to live keyboard focus. Taskbar picks beat transient bar-layer focus.
    fn preferred_stacking_window(&self) -> Option<u32> {
        self.last_focused_window.or(self.focused_window_id())
    }

    /// Space-relative origin (top-left) of the wl_surface that owns `surface`, if
    /// its window is mapped. Matches `window_surface_for`: mapped location minus
    /// the client's geometry offset (CSD / X11 insets). Used to translate a
    /// locked-pointer cursor hint (surface-local) into global desktop coordinates
    /// when restoring the cursor on unlock.
    pub(crate) fn surface_space_origin(&self, surface: &WlSurface) -> Option<Point<f64, Logical>> {
        use smithay::wayland::seat::WaylandFocus;
        let window = self
            .windows
            .id_for_surface(surface)
            .and_then(|id| self.windows.get(id))
            .map(|record| record.window.clone())
            .or_else(|| {
                self.space
                    .elements()
                    .find(|window| window.wl_surface().as_deref() == Some(surface))
                    .cloned()
            })?;
        let map_loc = self.space.element_location(&window)?;
        let geo = window.geometry();
        Some(map_loc.to_f64() - geo.loc.to_f64())
    }

    /// Resolve window id + app_id for pointer-constraint diagnostics.
    pub(crate) fn pointer_trace_for_surface(
        &self,
        surface: &WlSurface,
    ) -> (Option<u32>, Option<String>) {
        let id = self.windows.id_for_surface(surface);
        let app_id = id.and_then(|i| self.windows.get(i).and_then(|r| r.app_id.clone()));
        (id, app_id)
    }

    /// Current constraint phase + whether the lock is active / locked for tracing.
    pub(crate) fn pointer_constraint_snapshot(
        &self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) -> (Option<PointerConstraintPhase>, bool, bool) {
        use smithay::reexports::wayland_server::Resource;
        use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};

        let phase = self.pointer_constraint_phases.get(&surface.id()).copied();
        let mut is_active = false;
        let mut is_locked = false;
        with_pointer_constraint(surface, pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            is_active = constraint.is_active();
            if is_active {
                is_locked = matches!(&*constraint, PointerConstraint::Locked(_));
            }
        });
        (phase, is_active, is_locked)
    }

    /// Emit a game pointer-lock trace line. Pass `state` when already inside a
    /// `with_pointer_constraint` callback — never call `pointer_constraint_snapshot`
    /// there: nested `with_pointer_constraint` calls deadlock the compositor
    /// (documented in smithay's pointer_constraints commit hook).
    pub(crate) fn trace_game_pointer_at(
        &self,
        surface: &WlSurface,
        event: &str,
        pointer_loc: Option<Point<f64, Logical>>,
        phase: Option<PointerConstraintPhase>,
        is_active: bool,
        is_locked: bool,
    ) {
        use smithay::reexports::wayland_server::Resource;
        let (window_id, app_id) = self.pointer_trace_for_surface(surface);
        let Some(app) = app_id.as_deref() else {
            return;
        };
        let is_game = app.starts_with("steam_app_")
            || app.contains(".exe")
            || app.contains("proton")
            || app.eq_ignore_ascii_case("hytaleclient");
        if !is_game && app != "steam" {
            return;
        }
        tracing::info!(
            event,
            window_id,
            app_id = app,
            ?phase,
            is_active,
            is_locked,
            ?pointer_loc,
            surface_id = ?surface.id(),
            "game-pointer"
        );
    }

    /// Safe to call only when NOT already inside `with_pointer_constraint`.
    pub(crate) fn trace_game_pointer(
        &self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
        event: &str,
        pointer_loc: Option<Point<f64, Logical>>,
    ) {
        let (phase, is_active, is_locked) = self.pointer_constraint_snapshot(surface, pointer);
        self.trace_game_pointer_at(surface, event, pointer_loc, phase, is_active, is_locked);
    }

    /// Sync constraint phase from the live protocol state (Mutter/KWin-style).
    /// Inactive constraints stay `NeverActivated` so they can be re-armed while
    /// the pointer remains over the surface. Do not latch a permanent pause-menu
    /// phase from compositor cursor visibility — windowed games show the theme
    /// cursor whenever unlocked, and that wrongly blocked mouse-look forever.
    pub(crate) fn sync_pointer_constraint_phase(
        &mut self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
        use smithay::reexports::wayland_server::Resource;
        use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};

        let surface_id = surface.id();
        let mut trace: Option<(&str, PointerConstraintPhase, bool, bool)> = None;
        with_pointer_constraint(surface, pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            let is_active = constraint.is_active();
            let is_locked = is_active && matches!(&*constraint, PointerConstraint::Locked(_));
            if is_active {
                let prev = self.pointer_constraint_phases.get(&surface_id).copied();
                self.pointer_constraint_phases
                    .insert(surface_id.clone(), PointerConstraintPhase::Active);
                if prev != Some(PointerConstraintPhase::Active) {
                    self.cursor_position_hint = None;
                    trace = Some((
                        "constraint became active",
                        PointerConstraintPhase::Active,
                        true,
                        is_locked,
                    ));
                }
            } else if self.pointer_constraint_phases.get(&surface_id)
                == Some(&PointerConstraintPhase::Active)
            {
                self.pointer_constraint_phases
                    .insert(surface_id.clone(), PointerConstraintPhase::NeverActivated);
                trace = Some((
                    "constraint became inactive (eligible to re-arm)",
                    PointerConstraintPhase::NeverActivated,
                    false,
                    false,
                ));
            }
        });
        if let Some((event, phase, is_active, is_locked)) = trace {
            self.trace_game_pointer_at(surface, event, None, Some(phase), is_active, is_locked);
        }
    }

    /// Activate an inactive pointer constraint while the pointer is over its
    /// surface/region (same model as Mutter/KWin). Re-arm on motion, not only
    /// on surface entry — games often drop the lock briefly without the pointer
    /// leaving the window; refusing to re-arm left absolute desktop motion in
    /// control and made attack clicks warp the camera.
    pub(crate) fn maybe_arm_pointer_constraint(
        &mut self,
        surface: &WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
        location: Point<f64, Logical>,
        surface_loc: Point<f64, Logical>,
    ) {
        use smithay::reexports::wayland_server::Resource;
        use smithay::wayland::pointer_constraints::with_pointer_constraint;

        self.sync_pointer_constraint_phase(surface, pointer);

        let surface_id = surface.id();
        self.last_pointer_motion_surface = Some(surface_id.clone());

        let mut armed = false;
        with_pointer_constraint(surface, pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            if constraint.is_active() {
                return;
            }
            let point = (location - surface_loc).to_i32_round();
            if constraint
                .region()
                .is_none_or(|region| region.contains(point))
            {
                constraint.activate();
                self.pointer_constraint_phases
                    .insert(surface_id.clone(), PointerConstraintPhase::Active);
                armed = true;
            }
        });
        if armed {
            self.cursor_position_hint = None;
            self.trace_game_pointer_at(
                surface,
                "constraint re-armed while pointer over surface",
                Some(location),
                Some(PointerConstraintPhase::Active),
                true,
                true,
            );
        }
    }

    /// True while at least one override-redirect X11 surface (menu, tooltip,
    /// combo dropdown) is mapped. These are intentionally kept out of the window
    /// registry, so callers that reorder registered windows must check this to
    /// avoid restacking a toplevel above its own transient popup.
    pub(crate) fn has_mapped_override_redirect_popup(&self) -> bool {
        self.space.elements().any(|window| {
            window
                .x11_surface()
                .map(|surface| surface.is_override_redirect())
                .unwrap_or(false)
        })
    }

    fn raise_stacking_window(&mut self, id: u32, activate: bool) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.space.raise_element(&record.window, activate);
    }

    pub fn close_window(&mut self, id: u32) {
        self.maximize_fx_started.remove(&id);
        self.minimize_genie_fx.remove(&id);
        if let Some(record) = self.windows.get(id).cloned() {
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.send_close();
            } else if let Some(x11) = record.x11() {
                let _ = x11.close();
            }
        }
    }

    pub fn set_fullscreen(
        &mut self,
        id: u32,
        enabled: bool,
        requested_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        use smithay::reexports::wayland_server::Resource;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if self.windows.is_minimized(id) {
            self.unminimize_window(id);
        }

        // XWayland fullscreen goes through the dedicated X11 path (which sets the
        // X11 fullscreen property + reconfigures the surface); the Wayland
        // `xdg_toplevel`-state body below does not apply to it.
        if let Some(x11) = record.x11().cloned() {
            let output_for_event = requested_output
                .as_ref()
                .and_then(|wl| self.output_for_wl_output(wl))
                .or_else(|| self.output_for_window(id))
                .or_else(|| self.primary_output());
            if enabled {
                self.apply_x11_fullscreen(x11);
            } else {
                self.apply_x11_unfullscreen(x11);
            }
            self.windows.set_fullscreen(id, enabled);
            if let Some(output) = output_for_event {
                self.event_bus
                    .emit(&metis_protocol::CompositorEvent::WindowFullscreen {
                        id,
                        fullscreen: enabled,
                        output: output.name(),
                    });
            }
            self.schedule_redraw();
            return;
        }

        let output_for_event = if enabled {
            requested_output
                .as_ref()
                .and_then(|wl| self.output_for_wl_output(wl))
                .or_else(|| self.output_for_window(id))
                .or_else(|| self.primary_output())
        } else if record.fullscreen {
            self.output_for_window(id).or_else(|| self.primary_output())
        } else {
            None
        };

        let output_name_for_event = output_for_event.as_ref().map(smithay::output::Output::name);

        if enabled {
            let output = output_for_event.clone().or_else(|| self.primary_output());
            let Some(output) = output else {
                return;
            };
            let Some(geo) = self.space.output_geometry(&output) else {
                tracing::warn!(
                    output = %output.name(),
                    "fullscreen: output has no geometry"
                );
                return;
            };
            let wl_surface = record.wl_toplevel().map(|t| t.wl_surface().clone());
            let wl_output = wl_surface.as_ref().and_then(|wl_surface| {
                self.display_handle
                    .get_client(wl_surface.id())
                    .ok()
                    .and_then(|client| output.client_outputs(&client).next())
            });

            let current = self.window_body_rect(id).unwrap_or(record.target_rect);
            self.windows
                .set_pre_fullscreen_maximized(id, record.maximized);
            // Keep the pre-maximize floating geometry when entering fullscreen
            // from a maximized window — we re-maximize on exit instead.
            if !record.maximized {
                self.windows.set_restore_rect(id, current);
            }
            self.floating.insert(id);

            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.states.set(xdg_toplevel::State::Fullscreen);
                    state.size = Some(geo.size);
                    state.fullscreen_output = wl_output;
                    // Fullscreen has no chrome — grant server-side so a CSD
                    // toolkit (libdecor / GLFW games) drops its own frame on the
                    // very first fullscreen configure instead of leaving a stale
                    // titlebar+shadow that reports a negative window-geometry
                    // inset and shifts the surface off the output origin.
                    state.decoration_mode =
                        Some(crate::decoration_policy::grant_decoration_mode(true));
                });
            }
            self.space.map_element(record.window.clone(), geo.loc, true);
            self.windows.set_fullscreen(id, true);
            self.windows.set_maximized(id, false);
            self.clear_auto_hide(id);
            self.note_output_fullscreen(&output, id, true);
            self.focus_window_id(id);
        } else {
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.fullscreen_output = None;
                    // Restore the windowed decoration mode negotiated for this
                    // client so a CSD app gets its own frame back on exit.
                    state.decoration_mode = Some(crate::decoration_policy::grant_decoration_mode(
                        record.uses_ssd,
                    ));
                });
            }
            self.windows.set_fullscreen(id, false);
            let was_maximized = self.windows.take_pre_fullscreen_maximized(id);
            // Clear from every output's set: the window may have been fullscreen
            // on a different output than `output_for_event` resolves to now.
            self.drop_window_fullscreen(id);
            if was_maximized {
                let _ = self.windows.take_restore_rect(id);
                self.set_maximized(id, true);
                self.reclamp_maximized_geometry(id);
            } else if self.tile_id_for_window(id).is_some() {
                self.floating.remove(&id);
                let _ = self.windows.take_restore_rect(id);
                self.apply_window_rect(id);
            } else {
                self.restore_floating_from_transient(id);
                self.apply_window_rect(id);
            }
        }

        if let Some(output_name) = output_name_for_event {
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowFullscreen {
                    id,
                    fullscreen: enabled,
                    output: output_name,
                });
        }

        if let Some(record) = self.windows.get(id) {
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.send_pending_configure();
            }
        }
        self.schedule_redraw();
    }

    /// Record (or clear) a window's true-fullscreen state on an output and drive
    /// the edge bar's visibility. The bar hides while the output's fullscreen set
    /// is non-empty and reappears the instant it empties. Keyed by window id so
    /// entering twice is idempotent and a stray leave for an unknown id is a
    /// no-op — the counter model this replaced could drift out of sync and leave
    /// the bar hidden forever after a game exited while fullscreen.
    pub(crate) fn note_output_fullscreen(
        &mut self,
        output: &smithay::output::Output,
        id: u32,
        entering: bool,
    ) {
        use metis_protocol::CompositorEvent;

        let name = output.name();
        if entering {
            let set = self
                .output_fullscreen_windows
                .entry(name.clone())
                .or_default();
            let was_empty = set.is_empty();
            set.insert(id);
            if was_empty {
                self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                    output: name,
                    visible: false,
                });
            }
        } else {
            let Some(set) = self.output_fullscreen_windows.get_mut(&name) else {
                return;
            };
            if set.remove(&id) && set.is_empty() {
                self.output_fullscreen_windows.remove(&name);
                self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                    output: name,
                    visible: true,
                });
            }
        }
    }

    /// Unconditionally drop a window from every output's fullscreen set (used on
    /// teardown). Unlike `note_output_fullscreen(.., false)`, this does not need
    /// the caller to know which output the window was fullscreen on, nor does it
    /// depend on the window's registry `fullscreen` flag still being set — so a
    /// window that is destroyed or withdrawn while fullscreen always releases its
    /// hold on the bar. Re-shows the edge bar for any output whose set empties.
    pub(crate) fn drop_window_fullscreen(&mut self, id: u32) {
        use metis_protocol::CompositorEvent;

        self.fs_offset_warned.remove(&id);
        let mut emptied = Vec::new();
        for (name, set) in self.output_fullscreen_windows.iter_mut() {
            if set.remove(&id) && set.is_empty() {
                emptied.push(name.clone());
            }
        }
        for name in emptied {
            self.output_fullscreen_windows.remove(&name);
            self.event_bus.emit(&CompositorEvent::EdgeBarVisible {
                output: name,
                visible: true,
            });
        }
    }

    fn schedule_outputs_reload(&mut self) {
        use std::time::{Duration, Instant};
        let due = Instant::now() + Duration::from_millis(300);
        self.outputs_reload_due = Some(
            self.outputs_reload_due
                .map(|existing| existing.max(due))
                .unwrap_or(due),
        );
    }

    fn tick_outputs_reload(&mut self) {
        let Some(due) = self.outputs_reload_due else {
            return;
        };
        if std::time::Instant::now() < due {
            return;
        }
        self.outputs_reload_due = None;
        let before = self.output_runtime.cached().clone();
        let cfg = self.output_runtime.reload_from_disk();
        // A `ReloadOutputs` where nothing on disk actually changed must not
        // re-run the (expensive) output apply — that re-decodes the wallpaper,
        // invalidates decorations, and reflows. A misbehaving client that spams
        // the IPC otherwise pins the compositor busy several times a second.
        if cfg == before {
            return;
        }
        tracing::info!("outputs.json changed via ReloadOutputs — reapplying");
        if before.primary_output != cfg.primary_output {
            self.emit_monitor_changed();
        }
        if crate::output_prefs::is_night_light_only_change(&before, &cfg) {
            crate::output_prefs::refresh_night_light(self, &before);
        } else {
            self.pending_apply_outputs = true;
        }
    }

    /// Toggle multi-monitor mode between extended desktop and duplicate (mirror),
    /// bound to the `XF86Display` key. Persists the choice to `outputs.json`,
    /// refreshes the runtime cache, and reapplies on the next tick. Notifies the
    /// shell so it can flash an on-screen confirmation. No-op unless the DRM
    /// backend is active (the nested winit session has a single output).
    pub(crate) fn toggle_display_mirror(&mut self) {
        if !self.is_drm_backend() {
            return;
        }
        let mut cfg = metis_config::load_outputs_config_with_fallback(self.output_runtime.cached());
        let next = match cfg.display_mode {
            metis_config::DisplayLayoutMode::Extend => metis_config::DisplayLayoutMode::Mirror,
            metis_config::DisplayLayoutMode::Mirror => metis_config::DisplayLayoutMode::Extend,
        };
        cfg.display_mode = next;
        if let Err(err) = metis_config::save_outputs_config(&cfg) {
            tracing::warn!(%err, "failed to persist display mode toggle");
            return;
        }
        let _ = self.output_runtime.reload_from_disk();
        self.pending_apply_outputs = true;
        let osd = match next {
            metis_config::DisplayLayoutMode::Mirror => "hw osd-display mirror",
            metis_config::DisplayLayoutMode::Extend => "hw osd-display extend",
        };
        let _ = metis_protocol::write_runtime_command(osd);
        tracing::info!(?next, "display mode toggled via XF86Display");
    }

    fn output_for_wl_output(
        &self,
        wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    ) -> Option<smithay::output::Output> {
        use smithay::reexports::wayland_server::Resource;
        let client = wl_output.client()?;
        self.space
            .outputs()
            .find(|output| {
                output
                    .client_outputs(&client)
                    .any(|co| co.id() == wl_output.id())
            })
            .cloned()
    }

    pub fn set_maximized(&mut self, id: u32, enabled: bool) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if self.windows.is_minimized(id) {
            self.unminimize_window(id);
        }

        if enabled {
            if record.maximized && !record.fullscreen {
                // Already maximized: leave an in-flight wobble alone. Checking
                // map location would see the wobble offset as "wrong" and clear
                // the FX on every redundant set_maximized(true).
                if self.maximize_fx_started.contains_key(&id) {
                    return;
                }
                if let Some((client, client_size)) = self.maximized_client_geometry(id) {
                    let loc = Point::from((client.x, client.y));
                    let at_loc = self.space.element_location(&record.window) == Some(loc);
                    let size_ok = record.window.geometry().size == client_size;
                    let rect_ok = self.windows.target_rect(id) == Some(client);
                    if at_loc && size_ok && rect_ok {
                        return;
                    }
                }
            }

            // Finish any in-flight wobble before remapping so restore/max geometry
            // is not applied on top of a stale offset (and so a fresh FX can start).
            self.clear_maximize_fx(id);

            // Mark maximized before any nested layout/configure work so bulk
            // `apply_window_rect` passes cannot reposition this window back into
            // its grid tile mid-transition.
            self.windows.set_maximized(id, true);

            let current = self.window_body_rect(id).unwrap_or(record.target_rect);
            self.windows.set_restore_rect(id, current);

            if self.maximized_uses_auto_hide_titlebar(id) {
                self.auto_hide_titlebar.insert(id);
            } else {
                self.clear_auto_hide(id);
            }

            let Some((client, client_size)) = self.maximized_client_geometry(id) else {
                return;
            };

            let loc = Point::from((client.x, client.y));
            if let Some(toplevel) = record.wl_toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Fullscreen);
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some(client_size);
                    state.fullscreen_output = None;
                });
            } else if let Some(x11) = record.x11() {
                let _ = x11.set_maximized(true);
            }
            self.space.map_element(record.window.clone(), loc, true);
            self.send_window_configure(&record, loc, client_size);
            self.windows.set_rect(id, client);
            self.reclamp_auto_hide(id);
            self.windows.set_snapped(id, true);
            self.sync_auto_hide_titlebar(id);
            self.start_maximize_fx(id);
        } else {
            self.demote_maximized(id);
            // Match the maximize path: remapping with `activate: true` forces the
            // freshly restored window above any neighbor that kept a stale
            // full-screen stack slot after `relocate_element`-only demotion.
            if let Some(record) = self.windows.get(id).cloned() {
                if let Some(loc) = self.space.element_location(&record.window) {
                    self.space.map_element(record.window.clone(), loc, true);
                }
            }
        }

        // Wayland maximize/demote set pending states above; flush them (X11 was
        // already reconfigured via `send_window_configure` / `demote_maximized`).
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.send_pending_configure();
        }
        self.focus_window_id(id);
        self.restore_focus_stacking();
        self.schedule_redraw();
    }

    /// Client body rect + configure for a window already marked maximized. Does
    /// not change focus or re-raise unless the caller does so afterward.
    fn reapply_maximized_geometry(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.maximized {
            return;
        }
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        if self.maximized_uses_auto_hide_titlebar(id) {
            self.auto_hide_titlebar.insert(id);
        } else {
            self.clear_auto_hide(id);
        }
        let Some((client, client_size)) = self.maximized_client_geometry(id) else {
            return;
        };
        let loc = Point::from((client.x, client.y));
        let current_loc = self.space.element_location(&record.window);
        let current_size = record.window.geometry().size;
        if current_loc == Some(loc)
            && current_size == client_size
            && self.windows.target_rect(id) == Some(client)
        {
            return;
        }
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(client_size);
                state.fullscreen_output = None;
            });
        }
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        if mapped {
            self.space.relocate_element(&record.window, loc);
        } else {
            self.space.map_element(record.window.clone(), loc, false);
        }
        self.windows.set_rect(id, client);
        self.reclamp_auto_hide(id);
        self.send_window_configure(&record, loc, client_size);
    }

    /// Usable-zone footprint for a maximized window on its current output.
    fn maximized_client_geometry(&self, id: u32) -> Option<(PixelRect, Size<i32, Logical>)> {
        let zone = match self.output_for_window(id) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let gaps = self.zone_edge_gaps();
        let full = PixelRect {
            x: zone.x + gaps.left,
            y: zone.y + gaps.top,
            width: (zone.width - gaps.left - gaps.right).max(1),
            height: (zone.height - gaps.top - gaps.bottom).max(1),
        };
        let client = if self.window_uses_ssd(id) {
            self.ssd_client_rect(id, full)
        } else {
            full
        };
        let client_size = Size::from((client.width.max(1), client.height.max(1)));
        Some((client, client_size))
    }

    /// Drop a window out of maximized mode without stealing focus (internal).
    fn demote_maximized(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.maximized {
            return;
        }
        // Snap out of any in-flight wobble before restoring geometry.
        self.clear_maximize_fx(id);
        // Clear before `apply_window_rect` — while `maximized` is still true that
        // path returns immediately and the window stays at its maximized map origin
        // (often tucked under the edge bar once chrome is restored).
        self.windows.set_maximized(id, false);
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            });
        } else if let Some(x11) = record.x11() {
            let _ = x11.set_maximized(false);
        }
        self.clear_tiled_states(id);
        self.clear_auto_hide(id);
        self.windows.set_snapped(id, false);
        if self.tile_id_for_window(id).is_some() {
            // Grid apps return to their tile instead of staying ad-hoc floating.
            self.floating.remove(&id);
        } else {
            self.floating.insert(id);
            self.restore_floating_from_transient(id);
        }
        self.sync_auto_hide_titlebar(id);
        self.apply_window_rect(id);
        self.start_maximize_fx(id);
    }

    /// Toggle maximize for the focused window, debounced against key-repeat.
    /// Returns `true` when the keybind should be consumed.
    pub fn toggle_maximized_debounced(&mut self, id: u32) -> bool {
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
        let now = std::time::Instant::now();
        if self
            .last_maximize_toggle
            .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
        {
            return true;
        }
        self.last_maximize_toggle = Some(now);
        let maxed = self.windows.get(id).map(|w| w.maximized).unwrap_or(false);
        self.set_maximized(id, !maxed);
        true
    }

    pub fn minimize_window(&mut self, id: u32) {
        if self.minimize_genie_fx.contains_key(&id) {
            return;
        }
        if crate::window_fx::animations_enabled() && self.begin_minimize_genie(id) {
            return;
        }
        self.minimize_window_now(id);
    }

    fn minimize_window_now(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.size = None;
                state.fullscreen_output = None;
            });
            toplevel.send_pending_configure();
        }

        self.space.unmap_elem(&record.window);
        self.windows.set_minimized(id, true);
        self.windows.set_maximized(id, false);
        self.windows.set_fullscreen(id, false);
        self.clear_auto_hide(id);

        if self.focused_window_id() == Some(id) {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
            } else {
                tracing::warn!("minimize_window: seat has no keyboard");
            }
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowMinimized {
                id,
                minimized: true,
            });
    }

    fn unminimize_window(&mut self, id: u32) {
        self.windows.set_minimized(id, false);
        self.apply_window_rect(id);
        if self.preferred_stacking_window() == Some(id) {
            self.raise_stacking_window(id, true);
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowMinimized {
                id,
                minimized: false,
            });
    }

    /// Minimize a window by id, routing grid tiles through `set_tile_mode` (so the
    /// tile's mode stays consistent) and floating windows directly. Mirrors the
    /// decoration minimize button.
    pub fn minimize_by_id(&mut self, id: u32) {
        if let Some(tile_id) = self.tile_id_for_window(id) {
            self.set_tile_mode(&tile_id, metis_protocol::TileMode::Minimized);
        } else {
            self.minimize_window(id);
        }
    }

    /// Restore a window by id (grid tiles back to Grid mode, floating windows via
    /// `unminimize_window`).
    pub fn restore_by_id(&mut self, id: u32) {
        if !self.windows.is_minimized(id) {
            return;
        }
        if let Some(tile_id) = self.tile_id_for_window(id) {
            // Grid restore clears minimized inside `set_tile_mode`; still force
            // unminimize if the tile path left the flag set (e.g. mode already Grid).
            self.set_tile_mode(&tile_id, metis_protocol::TileMode::Grid);
            if self.windows.is_minimized(id) {
                self.unminimize_window(id);
            }
        } else {
            self.unminimize_window(id);
        }
    }

    /// Bring a window to the foreground: restore if minimized, raise, and focus.
    pub fn activate_window_by_id(&mut self, id: u32) {
        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            return;
        }
        // Ignore Exclusive shell layers (Alt+Tab overlay) so activation is not
        // treated as a no-op while the switcher still owns the seat briefly.
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if ws != self.active_workspace_for(&key) {
            self.switch_workspace(&key, ws);
        }
        self.restore_by_id(id);
        self.note_window_focus(id);
        self.ensure_app_tile_for_window(id);
        self.remap_window_for_desktop(id);
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.raise_stacking_window(id, true);
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
        } else {
            tracing::warn!("activate_window_by_id: seat has no keyboard");
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
        // Do not call `restore_focus_stacking` here — it can re-raise a different
        // preferred window (e.g. after Exclusive layer teardown) and undo the
        // user's Alt+Tab selection.
        self.schedule_redraw();
    }

    /// Map, unmap, or refresh a window for the active workspace on its output.
    /// Grid tiles, floating geometry, maximize, and fullscreen each have their
    /// own placement path; hidden workspaces always unmap.
    fn remap_window_for_desktop(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if self.windows.is_minimized(id) {
            return;
        }
        if !self.window_visible_on_desktop(id) {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        }
        if record.maximized {
            self.reapply_maximized_geometry(id);
            return;
        }
        if record.fullscreen {
            self.reapply_fullscreen_geometry(id);
            return;
        }
        self.apply_window_rect(id);
    }

    fn reapply_fullscreen_geometry(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        use smithay::reexports::wayland_server::Resource;

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if !record.fullscreen {
            return;
        }
        let Some(output) = self.output_for_window(id).or_else(|| self.primary_output()) else {
            return;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return;
        };
        // XWayland fullscreen geometry is owned by the X11 configure path.
        let Some(toplevel) = record.wl_toplevel() else {
            return;
        };
        let wl_surface = toplevel.wl_surface().clone();
        let wl_output = self
            .display_handle
            .get_client(wl_surface.id())
            .ok()
            .and_then(|client| output.client_outputs(&client).next());
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geo.size);
            state.fullscreen_output = wl_output;
        });
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        if mapped {
            self.space.relocate_element(&record.window, geo.loc);
        } else {
            self.space
                .map_element(record.window.clone(), geo.loc, false);
        }
        toplevel.send_pending_configure();
        self.schedule_redraw();
    }

    pub fn apply_window_rect(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        if let Some(toplevel) = record.window.toplevel() {
            if crate::grabs::resize_grab::surface_is_interactively_resizing(toplevel.wl_surface()) {
                return;
            }
        }
        // Never (re)map a minimized window. Restoring goes through
        // `unminimize_window`, which clears the flag *before* calling this. Without
        // this guard a bulk `reposition_all_windows` (triggered when restoring a
        // single grid tile) would re-map and un-minimize *every* minimized window.
        if self.windows.is_minimized(id) {
            return;
        }
        if !self.window_visible_on_desktop(id) {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        }
        // Maximized / fullscreen geometry is owned by `set_maximized` /
        // `set_fullscreen` / `reapply_*`. Bulk tile passes must not snap these
        // back to their grid slot.
        if record.maximized || record.fullscreen {
            return;
        }
        // Floating windows keep their free geometry (only recovered if they'd
        // land off every active output); grid windows snap to their tile.
        let rect = if self.floating.contains(&id) {
            // Auto-hide (snapped/maximized) windows map flush under the bar; only
            // ordinary floating windows reserve the titlebar strip above the body.
            // Borderless / near-fullscreen game floats stay at output origin — the
            // usable-zone clamp would inset them under the bar and clip the edges.
            let auto_hide = self.auto_hide_titlebar.contains(&id);
            self.windows.target_rect(id).map(|r| {
                let r = self.recover_offscreen_rect(r);
                if auto_hide {
                    r
                } else if self.rect_is_output_covering(r) {
                    if let Some(snap) = self.borderless_output_rect_for(id, r.width, r.height, true)
                    {
                        snap
                    } else {
                        r
                    }
                } else if self.x11_keep_on_full_output(id, r) {
                    // Large undecorated X11 games: do not inset under the edge bar.
                    r
                } else if self.should_draw_metis_ssd(id) {
                    self.clamp_body_below_bar(r)
                } else {
                    self.clamp_floating_rect_for(id, r)
                }
            })
        } else {
            self.rect_for_window_tile(id).and_then(|full| {
                let body = self.tile_client_rect(id, full);
                if self.is_active_scroll_window(id) {
                    let key = self.desk_key_for_window(id);
                    let zone = self.scroll_zone_for(&key);
                    if !body.intersects(&zone) {
                        return None;
                    }
                }
                Some(body)
            })
        };
        let Some(rect) = rect else {
            if self
                .space
                .elements()
                .any(|w| self.windows.id_for_window(w) == Some(id))
            {
                self.space.unmap_elem(&record.window);
                self.schedule_redraw();
            }
            return;
        };
        let mapped = self
            .space
            .elements()
            .any(|w| self.windows.id_for_window(w) == Some(id));
        let loc = Point::from((rect.x, rect.y));
        let width = rect.width.max(1);
        let height = rect.height.max(1);
        let size = Size::from((width, height));
        if mapped {
            let prev_loc = self.space.element_location(&record.window);
            let unchanged = prev_loc == Some(loc)
                && record.target_rect == rect
                && (record.window.geometry().size == size
                    || self.floating.contains(&id)
                    || self.is_active_scroll_window(id));
            if unchanged {
                return;
            }
            // `map_element` always inserts at the top of the stack, even with
            // `activate: false`, so routine layout sync must relocate in place
            // instead of unmap/remap — otherwise a grid reflow raises every
            // repositioned window above a maximized or focused one.
            if prev_loc != Some(loc) {
                self.space.relocate_element(&record.window, loc);
            }
            self.send_window_configure(&record, loc, size);
            self.windows.set_target_rect(id, rect);
            self.sync_auto_hide_titlebar(id);
            self.reclamp_auto_hide(id);
            self.schedule_redraw();
            return;
        }
        // First map — insert without stealing keyboard activation.
        self.space.map_element(record.window.clone(), loc, false);
        self.send_window_configure(&record, loc, size);
        self.windows.set_target_rect(id, rect);
        // An auto-hide (maximized / edge-snapped) window may refuse to shrink to
        // its footprint; re-anchor it so the screen-edge gap survives.
        self.sync_auto_hide_titlebar(id);
        self.reclamp_auto_hide(id);
    }

    /// Keep an auto-hide (maximized / edge-snapped) window pinned to its snapped
    /// edge so the screen-edge gap survives even when the client refuses to
    /// shrink to its footprint (e.g. an app whose minimum width is wider than the
    /// snap zone on a small display). The footprint (`target_rect`) encodes the
    /// desired gaps; if the committed size is larger we re-anchor the window to
    /// the edge the footprint hugs so the overflow spills toward screen center
    /// instead of off the screen edge.
    pub fn reclamp_auto_hide(&mut self, id: u32) {
        if !self.auto_hide_titlebar.contains(&id) {
            return;
        }
        // A fullscreen window is mapped flush at its output origin by the
        // fullscreen path and its geometry is owned there — never by the
        // auto-hide (maximized / edge-snapped) footprint. Without this guard a
        // window that entered fullscreen while maximized (e.g. a game maximized
        // at launch, then F11'd) would be re-anchored to the gap-inset maximized
        // footprint on its next commit, shifting the fullscreen surface off the
        // origin and exposing the wallpaper along the top/left edge. Mirrors the
        // `!fullscreen` guard in `reclamp_maximized_geometry`.
        if self.windows.get(id).is_some_and(|r| r.fullscreen) {
            return;
        }
        // CSD overlay windows only use auto_hide for hover chrome — not geometry.
        if !self.should_draw_metis_ssd(id) {
            return;
        }
        // Post-maximize wobble temporarily offsets the map origin; reclamp would
        // snap it back every client commit and kill the animation.
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let Some(foot) = self.windows.target_rect(id) else {
            return;
        };
        let Some(loc) = self.space.element_location(&record.window) else {
            return;
        };
        let size = record.window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return;
        }
        // Anchor against the output the window sits on, not always the primary —
        // otherwise an auto-hide (maximized / edge-snapped) window on a secondary
        // monitor gets dragged back toward the primary output's zone.
        let zone = match self.output_for_window(id) {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.window_placement_zone(),
        };
        let gaps = self.zone_edge_gaps();
        let pos = metis_config::load_bar_config().position;
        let y_anchor = match pos {
            metis_config::BarPosition::Bottom => Some(BarEdgeAnchor::Max),
            metis_config::BarPosition::Top => Some(BarEdgeAnchor::Min),
            _ => None,
        };
        let x_anchor = match pos {
            metis_config::BarPosition::Left => Some(BarEdgeAnchor::Min),
            metis_config::BarPosition::Right => Some(BarEdgeAnchor::Max),
            _ => None,
        };
        let new_x = anchor_axis(
            foot.x, foot.width, zone.x, zone.width, size.w, gaps.left, gaps.right, x_anchor,
        );
        let new_y = anchor_axis(
            foot.y,
            foot.height,
            zone.y,
            zone.height,
            size.h,
            gaps.top,
            gaps.bottom,
            y_anchor,
        );
        if new_x != loc.x || new_y != loc.y {
            self.space
                .relocate_element(&record.window, Point::from((new_x, new_y)));
            self.schedule_redraw();
        }
    }

    /// Re-anchor a maximized window when a client (especially CSD Chromium/Cursor)
    /// commits the wrong origin *or* size. `apply_window_rect` skips maximized
    /// windows, so this runs on commit instead.
    ///
    /// Critical for bottom edge bars: clients often keep a full-output height
    /// while accepting the correct `y`, which paints under the bar. Detect that
    /// via `geometry().size` (not `bbox()` — shadows/subsurfaces routinely extend
    /// past the reserved bottom and would otherwise reconfigure forever, freezing
    /// the session while Settings or other maximized windows are open).
    pub fn reclamp_maximized_geometry(&mut self, id: u32) {
        if !self
            .windows
            .get(id)
            .is_some_and(|r| r.maximized && !r.fullscreen && !self.windows.is_minimized(id))
        {
            return;
        }
        if self.maximize_fx_started.contains_key(&id) {
            return;
        }
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let Some((expected, expected_size)) = self.maximized_client_geometry(id) else {
            return;
        };
        let Some(loc) = self.space.element_location(&record.window) else {
            return;
        };
        let expected_loc = Point::from((expected.x, expected.y));
        let current_size = record.window.geometry().size;
        // Geometry bottom (not bbox): bbox includes CSD shadows that intentionally
        // spill a few pixels past the client edge.
        let mapped_bottom = loc.y + current_size.h;
        let expected_bottom = expected.y + expected.height;
        let already_ok = loc == expected_loc
            && current_size == expected_size
            && self.windows.target_rect(id) == Some(expected);
        if already_ok {
            return;
        }
        if let Some(toplevel) = record.wl_toplevel() {
            toplevel.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(expected_size);
                state.fullscreen_output = None;
            });
        } else if let Some(x11) = record.x11() {
            let _ = x11.set_maximized(true);
        }
        self.space.relocate_element(&record.window, expected_loc);
        self.windows.set_rect(id, expected);
        self.send_window_configure(&record, expected_loc, expected_size);
        self.schedule_redraw();
        tracing::debug!(
            id,
            ?loc,
            ?expected_loc,
            current_w = current_size.w,
            current_h = current_size.h,
            expected_w = expected_size.w,
            expected_h = expected_size.h,
            mapped_bottom,
            expected_bottom,
            "reclamped maximized geometry after client commit"
        );
    }

    /// Collect decoration specs (frame, title, focus) for every mapped, ready
    /// window that should be decorated. The frame is derived from the window's
    /// *actual* mapped geometry (so chrome tracks tiled, floating, and maximized
    /// windows alike). Fullscreen and minimized windows are skipped.
    pub fn decoration_specs(&self) -> Vec<crate::decoration::WindowDeco> {
        let focused = self.focused_window_id();
        let mut specs = Vec::new();
        for id in self.windows.ids() {
            let Some(record) = self.windows.get(id) else {
                continue;
            };
            if record.fullscreen || self.windows.is_minimized(id) {
                continue;
            }
            if self.is_minimize_genie_active(id) {
                continue;
            }
            let draws_ssd = self.should_draw_metis_ssd(id);
            if !draws_ssd {
                continue;
            }
            // Gate on the window actually being mapped in the space with real
            // geometry rather than the `ready` flag: floating windows can be mapped
            // by `reposition_all_windows` without ever flipping `ready` (the
            // commit-time activation's buffer check is unreliable — see the note in
            // `handlers::compositor::commit`). A window that's in the space with a
            // positive-size buffer is renderable, so it gets chrome.
            //
            // Every non-fullscreen window — tiled, floating, maximized, or snapped —
            // is mapped at its inner *body* rect (placement insets the client by the
            // titlebar + border), so Metis draws the same server-side chrome around
            // all of them. The decoration frame is the body grown by the titlebar
            // (top) and border (sides/bottom).
            let size = record.window.geometry().size;
            if size.w <= 0 || size.h <= 0 {
                continue;
            }
            let Some(loc) = self.space.element_location(&record.window) else {
                continue;
            };
            let auto_hide = self.auto_hide_titlebar.contains(&id);
            let show_overlay_titlebar = auto_hide
                && self.titlebar_reveal_window == Some(id)
                && self.titlebar_reveal_progress > 0.0;
            if auto_hide && !show_overlay_titlebar {
                continue;
            }
            let overlay_compact = auto_hide && self.window_uses_compact_overlay(id);
            let (frame, overlay) = if auto_hide {
                (
                    PixelRect {
                        x: loc.x,
                        y: loc.y,
                        width: size.w,
                        height: size.h,
                    },
                    true,
                )
            } else if draws_ssd {
                if let Some(frame) = self.ssd_frame_for_mapped_window(id, &record.window) {
                    (frame, false)
                } else {
                    continue;
                }
            } else {
                continue;
            };
            specs.push(crate::decoration::WindowDeco {
                id,
                frame,
                title: if overlay_compact {
                    String::new()
                } else {
                    self.titlebar_title(id, record.app_id.as_deref(), &record.title)
                },
                focused: focused == Some(id) || self.revealed_titlebar == Some(id),
                overlay,
                overlay_reveal: if overlay {
                    self.titlebar_reveal_progress
                } else {
                    1.0
                },
                overlay_compact,
            });
        }
        specs
    }

    /// Title to draw in a window's titlebar. When more than one window of the same
    /// app is open, a 1-based ordinal (by ascending window id) is appended — e.g.
    /// "Alacritty (2)" — matching the number the dock's window picker shows, so the
    /// two can be visually correlated.
    fn titlebar_title(&self, id: u32, app_id: Option<&str>, title: &str) -> String {
        let Some(app_id) = app_id else {
            return title.to_string();
        };
        let mut same: Vec<u32> = self
            .windows
            .ids()
            .into_iter()
            .filter(|&oid| {
                self.windows
                    .get(oid)
                    .is_some_and(|r| r.app_id.as_deref() == Some(app_id))
            })
            .collect();
        if same.len() <= 1 {
            return title.to_string();
        }
        same.sort_unstable();
        match same.iter().position(|&x| x == id) {
            Some(p) => format!("{} ({})", title, p + 1),
            None => title.to_string(),
        }
    }

    /// Gaps for maximize/snap/clamp. All edges use the configured `window_gap_px`
    /// (0 = flush). Bar reserve is applied separately via `bar_reserved_px_for`.
    pub(crate) fn zone_edge_gaps(&self) -> ZoneGaps {
        let g = self.configured_window_gap();
        ZoneGaps {
            top: g,
            bottom: g,
            left: g,
            right: g,
        }
    }

    /// Live maximize/snap padding from `bar.json` (`window_gap_px`, 0..=10).
    pub(crate) fn configured_window_gap(&self) -> i32 {
        metis_config::bar::window_gap_px(&metis_config::load_bar_config())
    }

    /// Pick up Settings changes to `window_gap_px` (~1s) and reflow maximized/
    /// snapped windows so the new padding applies without a restart.
    fn maybe_refresh_window_gap(&mut self) {
        if self.last_window_gap_check.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.last_window_gap_check = std::time::Instant::now();
        let gap = self.configured_window_gap();
        if gap == self.last_window_gap_px {
            return;
        }
        tracing::info!(gap, "window gap changed — reflowing windows");
        self.last_window_gap_px = gap;
        self.reflow_for_bar_geometry_change();
    }

    /// Full edge-bar strip from `bar.json` (margin + visible body). Used when
    /// auto-hide is off. Prefer [`bar_reserved_px_for`](Self::bar_reserved_px_for).
    fn bar_reserved_px_full() -> i32 {
        let cfg = metis_config::load_bar_config();
        cfg.margin_top as i32 + cfg.height as i32
    }

    /// Pixels reserved for the edge bar on `output`. With auto-hide enabled the
    /// bar overlays windows (always `0`) so peek/reveal never resizes maximize /
    /// snap geometry. With auto-hide off, reserves the full strip.
    fn bar_reserved_px_for(&self, _output: &smithay::output::Output) -> i32 {
        if metis_config::load_bar_config().auto_hide {
            0
        } else {
            Self::bar_reserved_px_full()
        }
    }

    /// Region for maximize, snap, and clamp. Subtracts the configured bar strip
    /// when auto-hide is off. With auto-hide the bar overlays, so the zone is the
    /// full output (windows still get `window_gap_px` via [`zone_edge_gaps`]).
    pub(crate) fn window_placement_zone(&self) -> PixelRect {
        match self.primary_output() {
            Some(output) => self.window_placement_zone_for(&output),
            None => self.placement_zone(),
        }
    }

    /// Like [`window_placement_zone`](Self::window_placement_zone) but for a
    /// specific `output`, so snap/maximize/placement target the monitor a window
    /// (or the cursor) is on.
    pub(crate) fn window_placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        let mut zone = match self.output_rect(output) {
            Some(m) => PixelRect {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            },
            None => self.placement_zone_for(output),
        };
        if !self.output_has_bar(output) {
            return self.usable_zone_for(output).unwrap_or(zone);
        }
        let reserve = self.bar_reserved_px_for(output);
        if reserve <= 0 {
            return zone;
        }
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                zone.y += reserve;
                zone.height = (zone.height - reserve).max(1);
            }
            metis_config::BarPosition::Bottom => {
                zone.height = (zone.height - reserve).max(1);
            }
            metis_config::BarPosition::Left => {
                zone.x += reserve;
                zone.width = (zone.width - reserve).max(1);
            }
            metis_config::BarPosition::Right => {
                zone.width = (zone.width - reserve).max(1);
            }
        }
        zone
    }

    /// The output area not covered by exclusive layer-shell zones, in global logical coordinates.
    ///
    /// `BAR_GAP_PX` is the thin padding kept between the edge bar and any window so
    /// the bar's drop shadow has breathing room and nothing visually touches it.
    pub fn usable_zone(&self) -> Option<PixelRect> {
        let output = self.primary_output()?;
        self.usable_zone_for(&output)
    }

    /// The usable area of a specific `output` (its geometry minus that output's
    /// exclusive layer-shell zones), in global logical coordinates.
    pub fn usable_zone_for(&self, output: &smithay::output::Output) -> Option<PixelRect> {
        let zone = layer_map_for_output(output).non_exclusive_zone();
        let origin = self.space.output_geometry(output)?.loc;
        Some(PixelRect {
            x: zone.loc.x + origin.x,
            y: zone.loc.y + origin.y,
            width: zone.size.w,
            height: zone.size.h,
        })
    }

    /// The usable area, falling back to the full output if the bar zone isn't
    /// known yet, and finally to the configured monitor size. Always returns the
    /// main (first) output, so off-screen windows are recovered onto it.
    fn placement_zone(&self) -> PixelRect {
        match self.primary_output() {
            Some(output) => self.placement_zone_for(&output),
            None => {
                let monitor = self.monitor;
                PixelRect {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                }
            }
        }
    }

    /// Usable area of `output`, falling back to its full geometry if the bar
    /// zone isn't known yet, and finally to the configured monitor size.
    pub(crate) fn placement_zone_for(&self, output: &smithay::output::Output) -> PixelRect {
        // Maximize/snap prefer `window_placement_zone_for`. With auto-hide the
        // shell may still advertise a top/bottom exclusive zone — ignore it so
        // floating clamps match overlay maximize.
        let auto_hide = metis_config::load_bar_config().auto_hide;
        let mut zone = if let Some(zone) = self.usable_zone_for(output) {
            if self.output_has_bar(output) && auto_hide {
                match self.output_rect(output) {
                    Some(m) => PixelRect {
                        x: m.x,
                        y: m.y,
                        width: m.width,
                        height: m.height,
                    },
                    None => zone,
                }
            } else {
                zone
            }
        } else {
            let mut zone = {
                let monitor = self.output_rect(output).unwrap_or(self.monitor);
                PixelRect {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                }
            };
            // Layer-shell exclusive zone may not be committed yet at startup.
            if self.output_has_bar(output) {
                let reserve = self.bar_reserved_px_for(output);
                match metis_config::load_bar_config().position {
                    metis_config::BarPosition::Top => {
                        zone.y += reserve;
                        zone.height = (zone.height - reserve).max(1);
                    }
                    metis_config::BarPosition::Bottom => {
                        zone.height = (zone.height - reserve).max(1);
                    }
                    metis_config::BarPosition::Left => {
                        zone.x += reserve;
                        zone.width = (zone.width - reserve).max(1);
                    }
                    metis_config::BarPosition::Right => {
                        zone.width = (zone.width - reserve).max(1);
                    }
                }
            }
            zone
        };
        self.enforce_bar_reserve_on_zone(output, &mut zone);
        zone
    }

    /// Keep the bar strip reserved when auto-hide is off. With auto-hide the
    /// reserve is 0 (bar overlays; windows keep `window_gap_px` only).
    fn enforce_bar_reserve_on_zone(&self, output: &smithay::output::Output, zone: &mut PixelRect) {
        if !self.output_has_bar(output) {
            return;
        }
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let reserve = self.bar_reserved_px_for(output);
        if reserve <= 0 {
            return;
        }
        match metis_config::load_bar_config().position {
            metis_config::BarPosition::Top => {
                let min_y = output_geo.loc.y + reserve;
                if zone.y < min_y {
                    let delta = min_y - zone.y;
                    zone.y = min_y;
                    zone.height = (zone.height - delta).max(1);
                }
            }
            metis_config::BarPosition::Bottom => {
                let max_bottom = output_geo.loc.y + output_geo.size.h - reserve;
                let zone_bottom = zone.y + zone.height;
                if zone_bottom > max_bottom {
                    zone.height = (max_bottom - zone.y).max(1);
                }
            }
            metis_config::BarPosition::Left => {
                let min_x = output_geo.loc.x + reserve;
                if zone.x < min_x {
                    let delta = min_x - zone.x;
                    zone.x = min_x;
                    zone.width = (zone.width - delta).max(1);
                }
            }
            metis_config::BarPosition::Right => {
                let max_right = output_geo.loc.x + output_geo.size.w - reserve;
                let zone_right = zone.x + zone.width;
                if zone_right > max_right {
                    zone.width = (max_right - zone.x).max(1);
                }
            }
        }
    }

    /// Output a window was opened on (assigned at registration from the pointer).
    pub(crate) fn launch_output_for(&self, id: u32) -> Option<smithay::output::Output> {
        self.windows
            .output_name(id)
            .and_then(|name| self.output_by_name(&name))
            .or_else(|| self.output_under_pointer())
            .or_else(|| self.primary_output())
    }

    /// Center a client rect for a window, accounting for SSD chrome insets.
    pub(crate) fn centered_body_for_window(&self, id: u32, body_w: i32, body_h: i32) -> PixelRect {
        if !self.should_draw_metis_ssd(id) {
            let rect = match self.launch_output_for(id) {
                Some(output) => self.centered_rect_in(&output, body_w, body_h),
                None => self.centered_rect(body_w, body_h),
            };
            return self.clamp_floating_rect_for(id, rect);
        }
        if self.window_uses_compact_overlay(id) {
            let rect = match self.launch_output_for(id) {
                Some(output) => self.centered_rect_in(&output, body_w, body_h),
                None => self.centered_rect(body_w, body_h),
            };
            return self.clamp_floating_rect(rect);
        }
        let border = metis_grid::app_tile_border_px() as i32;
        let header = metis_grid::APP_TILE_HEADER_PX;
        let footprint_w = body_w + border * 2;
        let footprint_h = body_h + header + border;
        let footprint = match self.launch_output_for(id) {
            Some(output) => self.centered_rect_in(&output, footprint_w, footprint_h),
            None => self.centered_rect(footprint_w, footprint_h),
        };
        self.clamp_body_below_bar(app_tile_body_rect(footprint))
    }

    /// Restore a saved client rect, keeping position and size when possible.
    fn restore_body_for_window(&self, id: u32, saved: PixelRect) -> PixelRect {
        let rect = self.recover_offscreen_rect(saved);
        if self.should_draw_metis_ssd(id) {
            self.clamp_floating_rect(rect)
        } else {
            self.clamp_floating_rect_for(id, rect)
        }
    }

    /// A rect of `width`x`height` centered in the primary output's usable area.
    fn centered_rect(&self, width: i32, height: i32) -> PixelRect {
        self.centered_rect_in_zone(self.placement_zone(), width, height)
    }

    /// A rect of `width`x`height` centered in `output`'s usable area.
    fn centered_rect_in(
        &self,
        output: &smithay::output::Output,
        width: i32,
        height: i32,
    ) -> PixelRect {
        self.centered_rect_in_zone(self.placement_zone_for(output), width, height)
    }

    /// A rect of `width`x`height` centered in `zone` (clamped to fit).
    fn centered_rect_in_zone(&self, zone: PixelRect, width: i32, height: i32) -> PixelRect {
        let w = width.min((zone.width - WINDOW_GAP_PX * 2).max(1)).max(1);
        let h = height.min((zone.height - WINDOW_GAP_PX * 2).max(1)).max(1);
        PixelRect {
            x: zone.x + (zone.width - w) / 2,
            y: zone.y + (zone.height - h).max(0) / 2,
            width: w,
            height: h,
        }
    }

    /// True when `rect` is visible on at least one active output — i.e. it
    /// overlaps some monitor by a grabbable amount. A window on a secondary
    /// monitor counts as on-screen; only a window that lies off *every* output is
    /// considered lost. The minimum overlap ensures the titlebar stays reachable.
    fn rect_visible_on_any_output(&self, rect: PixelRect) -> bool {
        // Require a chunk at least this big (incl. the titlebar) on some output.
        const MIN_VISIBLE: i32 = MIN_VISIBLE_PX;
        for output in self.space.outputs() {
            let Some(g) = self.space.output_geometry(output) else {
                continue;
            };
            let left = rect.x.max(g.loc.x);
            let right = (rect.x + rect.width).min(g.loc.x + g.size.w);
            let top = rect.y.max(g.loc.y);
            let bottom = (rect.y + rect.height).min(g.loc.y + g.size.h);
            let overlap_w = (right - left).min(rect.width);
            let overlap_h = (bottom - top).min(rect.height);
            if overlap_w >= MIN_VISIBLE.min(rect.width) && overlap_h >= MIN_VISIBLE.min(rect.height)
            {
                return true;
            }
        }
        false
    }

    /// Keep a window reachable: if `rect` lies off every active output (e.g. it
    /// was saved on a monitor that's no longer connected), pull it back onto the
    /// primary output. Windows already visible on *some* monitor — including a
    /// secondary one in a multi-monitor setup — are left exactly where they are.
    pub fn recover_offscreen_rect(&self, rect: PixelRect) -> PixelRect {
        if self.rect_visible_on_any_output(rect) {
            return rect;
        }
        self.clamp_rect_on_screen(rect)
    }

    /// Force `rect` to be fully visible on the main output: cap its size to the
    /// usable area and shift its origin so the whole window is on-screen (under
    /// the bar). Used to recover a window that's off every active output.
    pub fn clamp_rect_on_screen(&self, rect: PixelRect) -> PixelRect {
        let zone = self.window_placement_zone();
        let gaps = self.zone_edge_gaps();
        let width = rect.width.clamp(1, zone.width.max(1));
        let height = rect.height.clamp(1, zone.height.max(1));
        let min_x = zone.x + gaps.left;
        let min_y = zone.y + gaps.top;
        let max_x = (zone.x + zone.width - width - gaps.right).max(min_x);
        let max_y = (zone.y + zone.height - height - gaps.bottom).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Keep a floating window's body clear of the top edge bar (exclusive zone).
    /// Bottom/left/right bars overlay the desktop — floating windows may pass under them.
    pub fn clamp_body_below_bar(&self, mut rect: PixelRect) -> PixelRect {
        let pos = metis_config::load_bar_config().position;
        if !matches!(pos, metis_config::BarPosition::Top) {
            return rect;
        }
        let gaps = self.zone_edge_gaps();
        let header = metis_grid::APP_TILE_HEADER_PX;
        let zone = self.placement_zone();
        let min_y = zone.y + gaps.top + header;
        if rect.y < min_y {
            rect.y = min_y;
        }
        rect
    }

    /// Keep a restored floating CSD window below the visible top bar.
    ///
    /// Layer-shell exclusive zones can lag by a commit when the bar reappears
    /// after fullscreen, so use configured bar geometry directly.
    fn clamp_below_top_bar_edge_for(&self, id: u32, mut rect: PixelRect) -> PixelRect {
        if !matches!(
            metis_config::load_bar_config().position,
            metis_config::BarPosition::Top
        ) {
            return rect;
        }
        let Some(output) = self.output_for_window(id).or_else(|| self.primary_output()) else {
            return rect;
        };
        if !self.output_has_bar(&output) {
            return rect;
        }
        let origin_y = self
            .space
            .output_geometry(&output)
            .map(|g| g.loc.y)
            .unwrap_or(0);
        let min_y = origin_y + self.bar_reserved_px_for(&output) + self.configured_window_gap();
        if rect.y < min_y {
            rect.y = min_y;
        }
        rect
    }

    fn clamp_restored_floating_rect(&self, id: u32, rect: PixelRect) -> PixelRect {
        let rect = self.recover_offscreen_rect(rect);
        if self.should_draw_metis_ssd(id) {
            if self.window_uses_compact_overlay(id) {
                self.clamp_floating_rect(rect)
            } else {
                self.clamp_body_below_bar(rect)
            }
        } else {
            let rect = self.clamp_floating_rect_for(id, rect);
            self.clamp_below_top_bar_edge_for(id, rect)
        }
    }

    /// Restore geometry saved before maximize/fullscreen and clamp below the bar.
    fn restore_floating_from_transient(&mut self, id: u32) {
        let Some(restore) = self.windows.take_restore_rect(id) else {
            return;
        };
        let restore = self.clamp_restored_floating_rect(id, restore);
        self.windows.set_target_rect(id, restore);
    }

    /// Keep a floating window on-screen. Overlay edge bars do not inset the bounds
    /// (windows may slide underneath); only the top bar reserves space for SSD windows.
    pub(crate) fn clamp_floating_rect_for(&self, id: u32, rect: PixelRect) -> PixelRect {
        if self.should_draw_metis_ssd(id) {
            self.clamp_floating_rect(rect)
        } else {
            self.clamp_floating_rect_no_header(rect)
        }
    }

    /// Like [`Self::clamp_floating_rect`] but without reserving space for Metis SSD chrome.
    fn clamp_floating_rect_no_header(&self, rect: PixelRect) -> PixelRect {
        let center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        let zone = match self.output_at(center) {
            Some(output) => self.placement_zone_for(&output),
            None => self.placement_zone(),
        };
        let g = WINDOW_GAP_PX;
        let width = rect.width.clamp(1, (zone.width - g * 2).max(1));
        let height = rect.height.clamp(1, (zone.height - g * 2).max(1));
        let min_x = zone.x + g;
        let min_y = zone.y + g;
        let max_x = (zone.x + zone.width - width - g).max(min_x);
        let max_y = (zone.y + zone.height - height - g).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Keep a floating window on-screen. Overlay edge bars do not inset the bounds
    /// (windows may slide underneath); only the top bar reserves space.
    fn clamp_floating_rect(&self, rect: PixelRect) -> PixelRect {
        // Clamp within the output the window mostly sits on (by its center), so a
        // floating window on a secondary monitor isn't yanked back to primary.
        let center = Point::from((rect.x + rect.width / 2, rect.y + rect.height / 2));
        let zone = match self.output_at(center) {
            Some(output) => self.placement_zone_for(&output),
            None => self.placement_zone(),
        };
        let g = WINDOW_GAP_PX;
        let pos = metis_config::load_bar_config().position;
        let gaps = self.zone_edge_gaps();
        let width = rect.width.clamp(1, (zone.width - g * 2).max(1));
        let height = rect.height.clamp(1, (zone.height - g * 2).max(1));
        let min_x = zone.x + g;
        let min_y = match pos {
            metis_config::BarPosition::Top => zone.y + gaps.top + metis_grid::APP_TILE_HEADER_PX,
            _ => zone.y + g,
        };
        let max_x = (zone.x + zone.width - width - g).max(min_x);
        let max_y = (zone.y + zone.height - height - g).max(min_y);
        PixelRect {
            x: rect.x.clamp(min_x, max_x),
            y: rect.y.clamp(min_y, max_y),
            width,
            height,
        }
    }

    /// Auto-place a window if it hasn't been finally positioned yet. Safe to call
    /// again whenever the app_id becomes known (GTK often sets it just *after* its
    /// first buffer commit, so the initial activation may not see it). No-ops once
    /// placement is locked in (`placement_chosen`) — i.e. positioned with a known
    /// app_id, or moved/resized by the user.
    pub(crate) fn maybe_autoplace_window(&mut self, id: u32) {
        // `placement_chosen` is the authoritative "we're done positioning" flag.
        // A free window may already be in `floating` with a provisional centered
        // rect (placed before its app_id was known) — that must still be allowed to
        // re-run here so the saved geometry can be restored once app_id arrives.
        if self.windows.placement_chosen(id) {
            return;
        }
        let app_id = self.windows.get(id).and_then(|r| r.app_id.clone());
        if self.place_new_window(id, app_id.as_deref()) && self.windows.is_ready(id) {
            self.apply_window_rect(id);
        }
    }

    /// Decide where a freshly-mapped window should appear (once per window).
    /// Grid workspaces tile; free and scroll workspaces center floating windows
    /// (saved size when the app was opened before, default size on first launch).
    /// Returns true when the window was placed as floating.
    fn place_new_window(&mut self, id: u32, app_id: Option<&str>) -> bool {
        if self.windows.placement_chosen(id) {
            return self.floating.contains(&id);
        }

        let title = self.windows.get(id).map(|r| r.title.clone());
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        let kind = self.layout_kind_for(&key, ws);

        tracing::info!(
            id,
            ?app_id,
            ?title,
            ?kind,
            "place_new_window: deciding placement"
        );

        // Settings and similar always open centered floating regardless of layout.
        let by_app_id = app_id.is_some_and(|a| CENTERED_FLOAT_APP_IDS.contains(&a));
        let by_title = title
            .as_deref()
            .is_some_and(|t| CENTERED_FLOAT_TITLES.contains(&t));
        if by_app_id || by_title {
            let rect = self.centered_body_for_window(id, DEFAULT_FLOAT_W, DEFAULT_FLOAT_H);
            self.floating.insert(id);
            self.windows.set_target_rect(id, rect);
            self.windows.set_placement_chosen(id, true);
            tracing::info!(id, ?rect, "place_new_window: centered default-float app");
            return true;
        }

        // Gaming rules: games and launchers must escape the tiling grid (a tile
        // clamps their size and fights the reflow engine). Float — and, when the
        // rule opts in, queue a true-fullscreen once the client is ready — for
        // matching windows on ANY layout (Grid included). Saved geometry is
        // restored when known so a game reopens where the user last left it.
        let rule = self.game_rules.evaluate(app_id, title.as_deref());
        if rule.float || rule.fullscreen {
            self.floating.insert(id);
            let rect = app_id
                .and_then(|a| self.window_state.get(a))
                .map(|saved| saved.to_rect())
                .filter(|r| saved_size_is_usable(r.width, r.height))
                .map(|saved| self.restore_body_for_window(id, saved))
                .unwrap_or_else(|| {
                    self.centered_body_for_window(id, DEFAULT_FLOAT_W, DEFAULT_FLOAT_H)
                });
            self.windows.set_target_rect(id, rect);
            self.windows.set_placement_chosen(id, true);
            if rule.fullscreen {
                self.pending_game_fullscreen.insert(id);
            }
            tracing::info!(
                id,
                ?rect,
                fullscreen = rule.fullscreen,
                "place_new_window: game rule float"
            );
            return true;
        }

        // Grid layout: tile via the desk — never auto-center from saved geometry.
        if kind == metis_grid::LayoutKind::Grid {
            self.windows.set_placement_chosen(id, true);
            return false;
        }

        // Free desktop: restore saved geometry when known, else centered default.
        if kind == metis_grid::LayoutKind::Free {
            // Free windows must be floating to map at all (apply_window_rect unmaps
            // non-floating free windows), so claim it up front on every path.
            self.floating.insert(id);
            if let Some(app_id) = app_id {
                if let Some(saved) = self.window_state.get(app_id) {
                    let saved_rect = saved.to_rect();
                    if saved_size_is_usable(saved_rect.width, saved_rect.height) {
                        let rect = self.restore_body_for_window(id, saved_rect);
                        self.windows.set_target_rect(id, rect);
                        self.windows.set_placement_chosen(id, true);
                        tracing::info!(id, ?rect, "place_new_window: restored saved geometry");
                        return true;
                    }
                    // Drop splash-sized / unusable saves so the next open uses default.
                    self.window_state.remove(app_id);
                }
                // app_id known but nothing saved: first launch, center and lock.
                let rect = self.centered_body_for_window(id, DEFAULT_FLOAT_W, DEFAULT_FLOAT_H);
                self.windows.set_target_rect(id, rect);
                self.windows.set_placement_chosen(id, true);
                tracing::info!(
                    id,
                    "place_new_window: free desktop centered on launch output"
                );
                return true;
            }
            // app_id not set yet (GTK usually assigns it just after the first
            // commit). Give the window a provisional centered rect so it maps, but
            // do NOT lock placement — a later pass, once the app_id is known, must
            // still be able to restore the saved geometry instead of leaving the
            // window stuck centered at the default size.
            if self.windows.target_rect(id).is_none() {
                let rect = self.centered_body_for_window(id, DEFAULT_FLOAT_W, DEFAULT_FLOAT_H);
                self.windows.set_target_rect(id, rect);
            }
            tracing::info!(
                id,
                "place_new_window: free desktop provisional center (awaiting app_id)"
            );
            return true;
        }

        // Scroll layout: the window belongs to the strip, not a free float. Like
        // the grid branch, just mark placement decided and let the scroll strip own
        // it — `ensure_app_tile_for_window` adds it to the strip (seed_scroll_state)
        // and `apply_window_rect` positions it from its scroll frame
        // (`rect_for_window_tile` → `scroll_frame_for_window`). Floating it here
        // would exclude it from `scroll_managed_app_ids` (which filters out floating
        // windows), leaving a centered window stranded on top of the strip. Column
        // widths are presets (⅓/½/⅔/full), so saved pixel geometry doesn't apply.
        self.windows.set_placement_chosen(id, true);
        tracing::info!(id, ?kind, "place_new_window: scroll strip-managed");
        false
    }

    /// Persist a floating window's current on-screen geometry under its app_id,
    /// so it reopens in the same place next time. No-op for grid-tiled windows
    /// (their position is derived from the grid) or windows without an app_id.
    pub(crate) fn save_window_geometry(&mut self, id: u32) {
        if !self.floating.contains(&id) {
            return;
        }
        let Some(record) = self.windows.get(id) else {
            return;
        };
        let Some(app_id) = record.app_id.clone() else {
            return;
        };
        // Splash / boot screens share the main app's WM_CLASS — never let their
        // tiny (or temporarily enlarged) footprint overwrite the real save.
        if title_looks_like_splash(&record.title) {
            return;
        }
        if record.is_x11 {
            if let Some(x11) = record.x11() {
                use smithay::xwayland::xwm::WmWindowType;
                if matches!(x11.window_type(), Some(WmWindowType::Splash)) {
                    return;
                }
            }
        }
        // Prefer the live mapped geometry (captures user resizes); for a maximized
        // or snapped window save its pre-snap rect so it reopens at a sane float size.
        let rect = if record.maximized || record.snapped {
            record.restore_rect.unwrap_or(record.target_rect)
        } else if let Some(loc) = self.space.element_location(&record.window) {
            let size = record.window.geometry().size;
            PixelRect {
                x: loc.x,
                y: loc.y,
                width: size.w.max(1),
                height: size.h.max(1),
            }
        } else {
            record.target_rect
        };
        // Never persist a degenerate or splash-sized footprint (LibreOffice
        // `soffice` splash was ~580×180 and reopened Calc as a strip).
        if !saved_size_is_usable(rect.width, rect.height) {
            return;
        }
        self.window_state
            .set(&app_id, crate::window_state::SavedGeometry::from_rect(rect));
    }

    /// Mapped client body in logical coords (element location + buffer size).
    pub(crate) fn window_client_body_rect(
        &self,
        id: u32,
        window: &smithay::desktop::Window,
    ) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        if self.windows.is_minimized(id) {
            return None;
        }
        if record.fullscreen {
            let geo = self.space.element_geometry(window)?;
            return Some(PixelRect {
                x: geo.loc.x,
                y: geo.loc.y,
                width: geo.size.w,
                height: geo.size.h,
            });
        }
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return None;
        }
        Some(PixelRect {
            x: loc.x,
            y: loc.y,
            width: size.w,
            height: size.h,
        })
    }

    /// True when a window stacked above `below_id` has client pixels at `(x, y)`.
    pub(crate) fn higher_window_client_occludes(&self, x: i32, y: i32, below_id: u32) -> bool {
        use crate::desk_input::point_in_rect;

        for window in self.space.elements().rev() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if id == below_id {
                break;
            }
            if let Some(body) = self.window_client_body_rect(id, window) {
                if point_in_rect(x, y, body) {
                    return true;
                }
            }
        }
        false
    }

    /// Handle a pointer press that may land on a server-side decoration (titlebar,
    /// control buttons, or border). Returns true when the press was consumed by the
    /// decoration (so the caller must not forward it to a client surface).
    /// Give keyboard focus to a window because its server-side chrome was
    /// clicked, and report it to the shell. No-op when already focused.
    fn focus_window_chrome(&mut self, id: u32, serial: smithay::utils::Serial) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        // Always raise: clicking any chrome must bring the window to the front,
        // even when it already holds keyboard focus (it can still be stacked
        // behind another window after a raise of its neighbor).
        self.note_window_focus(id);
        self.space.raise_element(&record.window, true);
        self.schedule_redraw();
        if self.focused_window_id() == Some(id) {
            return;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
        }
        self.event_bus
            .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
    }

    pub fn handle_decoration_press(
        &mut self,
        loc: Point<f64, Logical>,
        serial: smithay::utils::Serial,
        button: u32,
    ) -> bool {
        use crate::decoration::{control_hitboxes, DecoControl};
        use crate::desk_input::point_in_rect;

        // A live popup/move/resize grab owns the pointer — let it run.
        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return false;
        }
        if self.metis_bar_ui_hit(loc) {
            return false;
        }

        let (x, y) = (loc.x as i32, loc.y as i32);
        // Hit-test chrome in stacking order, topmost first, so a covered window's
        // titlebar/border can never catch a press that lands within the frame of a
        // window stacked in front of it (the front window owns that point). Revealed
        // overlay titlebars (auto-hide) always win — they float above all clients.
        let mut z: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for (i, window) in self.space.elements().enumerate() {
            if let Some(id) = self.windows.id_for_window(window) {
                z.insert(id, i);
            }
        }
        let specs = self.decoration_specs();
        let mut ordered: Vec<&crate::decoration::WindowDeco> = specs.iter().collect();
        ordered.sort_by_key(|s| {
            // overlay first (false < true); then highest stacking index (topmost).
            (
                !s.overlay,
                std::cmp::Reverse(z.get(&s.id).copied().unwrap_or(0)),
            )
        });
        for spec in ordered {
            let frame = spec.frame;
            if spec.overlay {
                let chrome = crate::decoration::overlay_chrome_rect(
                    spec.frame,
                    spec.overlay_reveal,
                    spec.overlay_compact,
                );
                // Overlay chrome can sit above the client rect while sliding in.
                if !point_in_rect(x, y, chrome) {
                    continue;
                }
            } else {
                if !point_in_rect(x, y, frame) {
                    continue;
                }
                if point_in_rect(x, y, metis_grid::app_tile_body_rect(frame)) {
                    // Inside the client body → not a decoration hit; let it pass through.
                    return false;
                }
                if self.higher_window_client_occludes(x, y, spec.id) {
                    continue;
                }
            }
            // Clicking any of a window's chrome focuses it, so the taskbar
            // highlight tracks focus immediately instead of waiting for the
            // periodic reconcile (decoration presses otherwise bypass the
            // keyboard-focus path entirely).
            self.focus_window_chrome(spec.id, serial);
            // Prefer edge resize over titlebar drag when the click lands on a
            // border strip (corners overlap both regions). Skipped for overlay
            // reveals — only the titlebar strip is interactive there.
            if !spec.overlay {
                let edges = self.resize_edges_for_point(loc, spec.frame, true);
                if !edges.is_empty() {
                    return self.start_edge_resize(spec.id, edges, loc, serial, button);
                }
            }
            let hit_frame = if spec.overlay {
                let chrome = crate::decoration::overlay_chrome_rect(
                    spec.frame,
                    spec.overlay_reveal,
                    spec.overlay_compact,
                );
                PixelRect {
                    x: spec.frame.x,
                    y: chrome.y,
                    width: spec.frame.width,
                    height: spec.frame.height,
                }
            } else {
                frame
            };
            for (control, rect) in control_hitboxes(hit_frame, spec.overlay_compact) {
                if !point_in_rect(x, y, rect) {
                    continue;
                }
                match control {
                    DecoControl::Close => self.close_window(spec.id),
                    DecoControl::Minimize => {
                        if self.windows.get(spec.id).is_some_and(|r| r.maximized) {
                            self.set_maximized(spec.id, false);
                        }
                        if let Some(tile_id) = self.tile_id_for_window(spec.id) {
                            self.set_tile_mode(&tile_id, metis_protocol::TileMode::Minimized);
                        } else {
                            self.minimize_window(spec.id);
                        }
                    }
                    DecoControl::Maximize => {
                        self.titlebar_press_pending = None;
                        let maxed = self
                            .windows
                            .get(spec.id)
                            .map(|r| r.maximized)
                            .unwrap_or(false);
                        self.set_maximized(spec.id, !maxed);
                    }
                    DecoControl::Titlebar => {
                        if self.titlebar_double_click_toggle(spec.id) {
                            self.titlebar_press_pending = None;
                            return true;
                        }
                        if self.windows.get(spec.id).is_some_and(|r| r.maximized) {
                            self.titlebar_press_pending = Some((spec.id, loc, serial));
                            return true;
                        }
                        self.start_titlebar_move(spec.id, loc, serial);
                    }
                }
                return true;
            }
        }
        false
    }

    /// Which resize edge(s) the pointer is over within `frame`'s border strips and
    /// outer grab halo. Returns empty when the pointer is in the interior body.
    /// When `strip_top_center_for_titlebar` is false (native CSD clients), the full
    /// top edge remains resizable.
    fn resize_edges_for_point(
        &self,
        loc: Point<f64, Logical>,
        frame: PixelRect,
        strip_top_center_for_titlebar: bool,
    ) -> crate::grabs::ResizeEdge {
        use crate::desk_input::point_in_rect;
        use crate::grabs::ResizeEdge;

        let (x, y) = (loc.x as i32, loc.y as i32);
        let outer = RESIZE_MARGIN_PX;
        let inner = RESIZE_INNER_PX;
        let corner = outer + metis_grid::app_tile_border_px().max(1);

        // Asymmetric band: mostly outside the frame (easy to grab) and only a
        // few px inside so right/bottom scrollbars are not swallowed.
        let y_lo = frame.y - outer;
        let y_hi = frame.y + frame.height + outer;
        let x_lo = frame.x - outer;
        let x_hi = frame.x + frame.width + outer;
        let on_left = x >= frame.x - outer && x < frame.x + inner && y >= y_lo && y < y_hi;
        let on_right = x >= frame.x + frame.width - inner
            && x < frame.x + frame.width + outer
            && y >= y_lo
            && y < y_hi;
        let on_top = y >= frame.y - outer && y < frame.y + inner && x >= x_lo && x < x_hi;
        let on_bottom = y >= frame.y + frame.height - inner
            && y < frame.y + frame.height + outer
            && x >= x_lo
            && x < x_hi;
        if !on_left && !on_right && !on_top && !on_bottom {
            return ResizeEdge::empty();
        }

        let mut edges = ResizeEdge::empty();
        if on_left {
            edges |= ResizeEdge::LEFT;
        }
        if on_right {
            edges |= ResizeEdge::RIGHT;
        }
        if on_bottom {
            edges |= ResizeEdge::BOTTOM;
        }
        if on_top {
            edges |= ResizeEdge::TOP;
        }

        // Titlebar centre is for dragging on Metis SSD; CSD clients own the top edge.
        if strip_top_center_for_titlebar {
            let titlebar = metis_grid::app_tile_chrome_rect(frame);
            if point_in_rect(x, y, titlebar) {
                let in_left_corner = x < frame.x + corner;
                let in_right_corner = x >= frame.x + frame.width - corner;
                if !in_left_corner && !in_right_corner {
                    edges.remove(ResizeEdge::TOP);
                }
            }
        }

        if edges.is_empty() {
            return ResizeEdge::empty();
        }
        edges
    }

    /// Frame grown by the outer resize grab halo so a frontmost window blocks edge
    /// hits on windows below when the pointer sits in the margin outside the client.
    fn resize_occlusion_rect(frame: PixelRect) -> PixelRect {
        let m = RESIZE_MARGIN_PX;
        PixelRect {
            x: frame.x - m,
            y: frame.y - m,
            width: frame.width + m * 2,
            height: frame.height + m * 2,
        }
    }

    /// Mapped window bounds expanded by the resize grab halo — used to block edge
    /// hits on windows below a frontmost window that does not itself expose edges.
    fn mapped_resize_occlusion_rect(&self, window: &smithay::desktop::Window) -> Option<PixelRect> {
        let geo = self.space.element_geometry(window)?;
        if geo.size.w <= 0 || geo.size.h <= 0 {
            return None;
        }
        Some(Self::resize_occlusion_rect(PixelRect {
            x: geo.loc.x,
            y: geo.loc.y,
            width: geo.size.w,
            height: geo.size.h,
        }))
    }

    /// Frame used for resize-band hit-testing. Metis SSD windows use the grown
    /// chrome rect; native CSD clients use their mapped client footprint.
    fn resize_frame_for_mapped_window(
        &self,
        id: u32,
        window: &smithay::desktop::Window,
    ) -> Option<PixelRect> {
        let record = self.windows.get(id)?;
        if record.fullscreen || record.maximized || self.windows.is_minimized(id) {
            return None;
        }
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return None;
        }
        if !self.window_uses_ssd(id) {
            return Some(PixelRect {
                x: loc.x,
                y: loc.y,
                width: size.w,
                height: size.h,
            });
        }
        if self.auto_hide_titlebar.contains(&id) {
            return Some(PixelRect {
                x: loc.x,
                y: loc.y,
                width: size.w,
                height: size.h,
            });
        }
        let border = metis_grid::app_tile_border_px();
        Some(PixelRect {
            x: loc.x - border,
            y: loc.y - metis_grid::APP_TILE_HEADER_PX,
            width: size.w + border * 2,
            height: size.h + metis_grid::APP_TILE_HEADER_PX + border,
        })
    }

    /// Server-side decoration frame for a mapped window, when chrome should be
    /// drawn or hit-tested. `None` for minimized/fullscreen windows and for
    /// auto-hide windows whose titlebar is not revealed.
    pub(crate) fn ssd_frame_for_mapped_window(
        &self,
        id: u32,
        window: &smithay::desktop::Window,
    ) -> Option<PixelRect> {
        if !self.should_draw_metis_ssd(id) {
            return None;
        }
        let record = self.windows.get(id)?;
        if record.fullscreen || self.windows.is_minimized(id) {
            return None;
        }
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return None;
        }
        if self.auto_hide_titlebar.contains(&id) && self.revealed_titlebar != Some(id) {
            return None;
        }
        if self.auto_hide_titlebar.contains(&id) {
            return Some(PixelRect {
                x: loc.x,
                y: loc.y,
                width: size.w,
                height: size.h,
            });
        }
        let border = metis_grid::app_tile_border_px();
        Some(PixelRect {
            x: loc.x - border,
            y: loc.y - metis_grid::APP_TILE_HEADER_PX,
            width: size.w + border * 2,
            height: size.h + metis_grid::APP_TILE_HEADER_PX + border,
        })
    }

    /// Hit-test the pointer against every mapped window's resize band. Returns the
    /// topmost window whose edge/corner is under the pointer, plus the combined
    /// edge(s). Minimized windows are skipped. Maximized and fullscreen windows
    /// do not expose edges but still occlude windows below.
    pub fn resize_edge_at(
        &self,
        loc: Point<f64, Logical>,
    ) -> Option<(u32, crate::grabs::ResizeEdge)> {
        use crate::desk_input::point_in_rect;

        if self.metis_bar_ui_hit(loc) {
            return None;
        }
        if self.screenshot_overlay_active() || self.capture_overlay_active() {
            return None;
        }
        let (x, y) = (loc.x as i32, loc.y as i32);
        // Walk mapped windows top-to-bottom so the frontmost window owns edge hits.
        for window in self.space.elements().rev() {
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.windows.is_minimized(id) {
                continue;
            }
            if self
                .windows
                .get(id)
                .is_some_and(|r| r.maximized || r.fullscreen)
            {
                if let Some(occlusion) = self.mapped_resize_occlusion_rect(window) {
                    if point_in_rect(x, y, occlusion) {
                        return None;
                    }
                }
                continue;
            }
            if let Some(frame) = self.resize_frame_for_mapped_window(id, window) {
                let edges = self.resize_edges_for_point(loc, frame, self.window_uses_ssd(id));
                if !edges.is_empty() {
                    return Some((id, edges));
                }
                if point_in_rect(x, y, Self::resize_occlusion_rect(frame)) {
                    return None;
                }
                continue;
            }
            let Some(geo) = self.space.element_geometry(window) else {
                continue;
            };
            if x >= geo.loc.x
                && x < geo.loc.x + geo.size.w
                && y >= geo.loc.y
                && y < geo.loc.y + geo.size.h
            {
                return None;
            }
        }
        None
    }

    /// Update the hovered resize edge from the pointer position so the host cursor
    /// can show the matching directional arrow. No-op while a grab owns the pointer
    /// (the active move/resize keeps its cursor). Flags a redraw on change.
    pub fn update_hover_cursor(&mut self, loc: Point<f64, Logical>) {
        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return;
        }
        // Window chrome must not react while the pointer is over the edge bar or
        // its popovers — otherwise titlebars below the bar's transparent shadow
        // pad show hover/reveal state when interacting with bar widgets.
        if self.metis_bar_ui_hit(loc) {
            if self.hover_cursor.is_some() {
                self.hover_cursor = None;
                self.schedule_redraw();
            }
            // Do not reveal auto-hide titlebars under Exclusive menus (Metis Menu,
            // NC, CC): the pointer still geometrically overlaps maximized chrome
            // and would flash SSD every motion while the translucent popover is up.
            if self.exclusive_keyboard_layer().is_none() {
                self.update_titlebar_reveal(loc);
            }
            self.tick_titlebar_press_pending(loc);
            return;
        }
        let edge = self.resize_edge_at(loc).map(|(_, e)| e);
        if edge != self.hover_cursor {
            self.hover_cursor = edge;
            self.schedule_redraw();
        }
        self.update_titlebar_reveal(loc);
        self.tick_titlebar_press_pending(loc);
    }

    fn tick_titlebar_press_pending(&mut self, loc: Point<f64, Logical>) {
        let Some((id, start, serial)) = self.titlebar_press_pending else {
            return;
        };
        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return;
        }
        let dx = loc.x - start.x;
        let dy = loc.y - start.y;
        if dx * dx + dy * dy >= 25.0 {
            self.titlebar_press_pending = None;
            self.start_titlebar_move(id, loc, serial);
        }
    }

    pub fn clear_titlebar_press_pending(&mut self) {
        self.titlebar_press_pending = None;
    }

    fn auto_hide_reveal_hit(
        &self,
        id: u32,
        geo: &smithay::utils::Rectangle<i32, Logical>,
        x: i32,
        y: i32,
    ) -> bool {
        use crate::decoration::overlay_chrome_rect;
        use crate::desk_input::point_in_rect;

        const STICKY_PAD_PX: i32 = 16;
        let header = metis_grid::APP_TILE_HEADER_PX;
        let in_x = x >= geo.loc.x && x < geo.loc.x + geo.size.w;
        if !in_x {
            return false;
        }
        let compact = self.window_uses_compact_overlay(id);
        let frame = PixelRect {
            x: geo.loc.x,
            y: geo.loc.y,
            width: geo.size.w,
            height: geo.size.h,
        };

        if self.titlebar_reveal_window == Some(id) {
            let chrome = overlay_chrome_rect(frame, self.titlebar_reveal_progress, compact);
            let in_chrome = point_in_rect(
                x,
                y,
                PixelRect {
                    x: chrome.x,
                    y: chrome.y - 4,
                    width: chrome.width,
                    height: chrome.height + STICKY_PAD_PX + 4,
                },
            );
            if in_chrome {
                return true;
            }
            // While the strip is sliding, keep the original trigger band sticky
            // so the pointer does not leave the animated chrome and thrash
            // reveal/hide. With edge-bar auto-hide, the peek strip belongs to
            // the bar — do not steal it for titlebar reveal.
            if self.windows.get(id).is_some_and(|r| r.maximized)
                && !metis_config::load_bar_config().auto_hide
            {
                if let Some(output) = self.output_for_window(id) {
                    if let Some(output_geo) = self.space.output_geometry(&output) {
                        let strip = Self::bar_config_strip_rect(&output_geo);
                        if point_in_rect(x, y, strip) {
                            return true;
                        }
                    }
                }
            }
            if compact {
                let strip_w = metis_grid::OVERLAY_CONTROLS_WIDTH_PX.min(geo.size.w.max(1));
                return y >= geo.loc.y
                    && y < geo.loc.y + header + STICKY_PAD_PX
                    && x >= geo.loc.x + geo.size.w - strip_w;
            }
            return y >= geo.loc.y && y < geo.loc.y + header + STICKY_PAD_PX;
        }

        if compact {
            let strip_w = metis_grid::OVERLAY_CONTROLS_WIDTH_PX.min(geo.size.w.max(1));
            return y >= geo.loc.y
                && y < geo.loc.y + header
                && x >= geo.loc.x + geo.size.w - strip_w;
        }

        // Maximized windows sit flush under a non-autohide edge bar; the bar's
        // shadow pad overlaps the client top. Treat pointer-in-bar-strip as a
        // titlebar reveal. With auto-hide the peek is for the edge bar only —
        // titlebar uses the window header band below.
        if self.windows.get(id).is_some_and(|r| r.maximized)
            && !metis_config::load_bar_config().auto_hide
        {
            if let Some(output) = self.output_for_window(id) {
                if let Some(output_geo) = self.space.output_geometry(&output) {
                    let strip = Self::bar_config_strip_rect(&output_geo);
                    if point_in_rect(x, y, strip) {
                        return true;
                    }
                }
            }
        }

        // When the edge bar auto-hides at the top, exclude the peek pixels so
        // moving to the absolute screen edge reveals the bar, not the titlebar.
        if metis_config::load_bar_config().auto_hide
            && matches!(
                metis_config::load_bar_config().position,
                metis_config::BarPosition::Top
            )
        {
            if let Some(output) = self.output_for_window(id) {
                if let Some(output_geo) = self.space.output_geometry(&output) {
                    let peek = Self::bar_peek_strip_rect(&output_geo);
                    if point_in_rect(x, y, peek) {
                        return false;
                    }
                }
            }
        }

        y >= geo.loc.y && y < geo.loc.y + header
    }

    /// Reveal the auto-hide titlebar overlay for the topmost auto-hide window whose
    /// pointer is in the reveal trigger or sticky chrome zone.
    fn update_titlebar_reveal(&mut self, loc: Point<f64, Logical>) {
        let (x, y) = (loc.x as i32, loc.y as i32);
        let mut revealed = None;
        // Topmost first: `Space::elements()` is bottom-to-top, so reverse.
        for window in self.space.elements().rev() {
            let Some(geo) = self.space.element_geometry(window) else {
                continue;
            };
            let in_x = x >= geo.loc.x && x < geo.loc.x + geo.size.w;
            let in_window = in_x && y >= geo.loc.y && y < geo.loc.y + geo.size.h;
            let Some(id) = self.windows.id_for_window(window) else {
                continue;
            };
            if self.auto_hide_titlebar.contains(&id) {
                if self.auto_hide_reveal_hit(id, &geo, x, y) {
                    revealed = Some(id);
                    break;
                }
                if in_window {
                    break;
                }
            } else if in_window {
                // A normal window occludes anything beneath it at this point.
                break;
            }
        }
        if revealed != self.revealed_titlebar {
            self.revealed_titlebar = revealed;
            if let Some(id) = revealed {
                self.titlebar_reveal_window = Some(id);
            }
            let _ = self.tick_titlebar_reveal_animation();
            // Hover only reveals chrome; keyboard focus stays on the window the
            // user picked until they click its titlebar (see `focus_window_chrome`).
            // Calling `focus_window_id` here re-raised a stale maximized neighbor
            // when the pointer lingered at the top edge after unmaximize.
            self.schedule_redraw();
        }
    }

    /// Handle a pointer press that may land on a window's resize band. On a hit,
    /// floats the window out of the grid and starts an interactive resize grab.
    /// Returns true when the press was consumed.
    pub fn handle_resize_press(
        &mut self,
        loc: Point<f64, Logical>,
        serial: smithay::utils::Serial,
        button: u32,
    ) -> bool {
        if self.metis_bar_ui_hit(loc) {
            return false;
        }
        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return false;
        }
        let Some((id, edges)) = self.resize_edge_at(loc) else {
            return false;
        };
        if self.is_active_scroll_window(id) && self.start_scroll_resize(id, edges, loc, serial) {
            return true;
        }
        // Fall through — vertical edges (or scroll-target miss) use normal resize.
        self.start_edge_resize(id, edges, loc, serial, button)
    }

    /// Begin an interactive edge resize for a normal (non-scroll-column) window.
    fn start_edge_resize(
        &mut self,
        id: u32,
        edges: crate::grabs::ResizeEdge,
        loc: Point<f64, Logical>,
        serial: smithay::utils::Serial,
        button: u32,
    ) -> bool {
        use smithay::input::pointer::{Focus, GrabStartData};

        // Resize grab owns geometry — end wobble first.
        self.clear_maximize_fx(id);

        let Some(record) = self.windows.get(id).cloned() else {
            return false;
        };
        let window = record.window.clone();
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return false;
        };
        let initial_window_size = window.geometry().size;

        self.space.raise_element(&window, true);
        self.floating.insert(id);
        self.clear_tiled_states(id);

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(window.clone().into()), serial);
        }

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Resizing,
                );
                state.size = Some(initial_window_size);
            });
            toplevel.send_pending_configure();
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        let start_data = GrabStartData {
            focus: None,
            button,
            location: loc,
        };
        let grab = crate::grabs::ResizeSurfaceGrab::start(
            start_data,
            window,
            edges,
            smithay::utils::Rectangle::new(initial_window_location, initial_window_size),
        );
        self.hover_cursor = Some(edges);
        self.schedule_redraw();
        pointer.set_grab(self, grab, serial, Focus::Clear);
        true
    }

    /// Begin a horizontal resize of a scroll column from a left/right border drag.
    /// The grab adjusts the target column's width live and reflows the strip.
    fn start_scroll_resize(
        &mut self,
        id: u32,
        edges: crate::grabs::ResizeEdge,
        loc: Point<f64, Logical>,
        serial: smithay::utils::Serial,
    ) -> bool {
        use crate::grabs::ResizeEdge;
        use smithay::input::pointer::{Focus, GrabStartData};

        if !edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            return false;
        }
        let Some((target_window, initial_width_px)) = self.scroll_resize_target(id, edges) else {
            return false;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        if pointer.is_grabbed() {
            return false;
        }
        // Focus the window the user grabbed so the resize reads as acting on it.
        if let Some(record) = self.windows.get(id).cloned() {
            self.space.raise_element(&record.window, true);
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Some(record.window.into()), serial);
            }
        }
        let start_data = GrabStartData {
            focus: None,
            button: 0x110,
            location: loc,
        };
        let grab = crate::grabs::ScrollResizeGrab::start(
            start_data,
            target_window,
            initial_width_px,
            loc.x,
        );
        self.hover_cursor = Some(edges);
        self.schedule_redraw();
        pointer.set_grab(self, grab, serial, Focus::Clear);
        true
    }

    /// Double-click the titlebar (anywhere outside the traffic-light buttons) to
    /// toggle maximize. The first click of a pair may start a brief move grab; the
    /// second press within the interval toggles without dragging.
    fn titlebar_double_click_toggle(&mut self, id: u32) -> bool {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);
        let now = std::time::Instant::now();
        if let Some((prev_id, prev)) = self.titlebar_last_click {
            if prev_id == id && now.duration_since(prev) <= INTERVAL {
                self.titlebar_last_click = None;
                let maxed = self.windows.get(id).map(|r| r.maximized).unwrap_or(false);
                self.set_maximized(id, !maxed);
                return true;
            }
        }
        self.titlebar_last_click = Some((id, now));
        false
    }

    /// Unmaximize only after the user actually drags the titlebar (not on click).
    pub fn unmaximize_for_titlebar_drag(&mut self, id: u32) {
        if self.windows.get(id).is_some_and(|r| r.maximized) {
            self.set_maximized(id, false);
        }
    }

    fn start_titlebar_move(
        &mut self,
        id: u32,
        loc: Point<f64, Logical>,
        serial: smithay::utils::Serial,
    ) {
        use smithay::input::pointer::{Focus, GrabStartData};

        // Move grab owns the map origin — end wobble so it cannot rubber-band.
        self.clear_maximize_fx(id);

        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        let window = record.window.clone();
        self.note_window_focus(id);
        self.space.raise_element(&window, true);
        // Manual titlebar drag floats the window out of the grid (no snap-back).
        self.floating.insert(id);
        self.clear_tiled_states(id);

        let initial_window_location = if self.windows.is_snapped(id) {
            self.restore_floating_from_snap(id, loc)
        } else {
            let was_maximized = self.windows.get(id).is_some_and(|r| r.maximized);
            if !was_maximized {
                let mut initial_window_location =
                    self.space.element_location(&window).unwrap_or_default();

                // SSD windows reserve a titlebar strip above the body when floating;
                // tabbed browsers use overlay chrome instead.
                if self.usable_zone().is_some()
                    && self.should_draw_metis_ssd(id)
                    && !self.window_uses_compact_overlay(id)
                {
                    let rect = metis_grid::PixelRect {
                        x: initial_window_location.x,
                        y: initial_window_location.y,
                        width: window.geometry().size.w,
                        height: window.geometry().size.h,
                    };
                    let clamped = self.clamp_body_below_bar(rect);
                    if clamped.y != initial_window_location.y
                        || clamped.x != initial_window_location.x
                    {
                        initial_window_location.x = clamped.x;
                        initial_window_location.y = clamped.y;
                        self.space
                            .map_element(window.clone(), initial_window_location, true);
                        self.windows.set_target_rect(id, clamped);
                    }
                }
                initial_window_location
            } else {
                self.space.element_location(&window).unwrap_or_default()
            }
        };

        let pending_maximized_demote =
            self.windows.get(id).is_some_and(|r| r.maximized) && !self.windows.is_snapped(id);

        // Focus the window so keyboard input follows the titlebar grab.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(window.clone().into()), serial);
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let start_data = GrabStartData {
            focus: None,
            button: 0x110,
            location: loc,
        };
        let grab = crate::grabs::MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
            drag_active: false,
            pending_maximized_demote,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn rect_for_window_tile(&self, id: u32) -> Option<PixelRect> {
        if self.floating.contains(&id) {
            return None;
        }
        // Scrolling workspaces position from the strip, not the tile grid.
        if let Some(frame) = self.scroll_frame_placed_for_window(id) {
            return Some(frame);
        }
        let key = self.desk_key_for_window(id);
        let desk = self.desk(&key)?;
        let tile = desk.layout.tiles.iter().find(
            |t| matches!(&t.kind, TileKind::App { window_id: Some(wid), .. } if *wid == id),
        )?;
        let metrics = match self.output_by_name(&key) {
            Some(o) => self.grid_metrics_for(&o),
            None => self.grid_metrics(),
        };
        Some(cell_to_pixels(&metrics, &tile.rect))
    }

    pub fn apply_grid_layout(&mut self, shell_layout: GridLayout, gutter_px: u32) {
        use std::collections::HashMap;

        // The shell desk editor is dormant; this path applies to the primary desk.
        let key = self.primary_key();
        let compositor_apps: HashMap<String, metis_grid::GridTile> = self
            .desk(&key)
            .map(|d| {
                d.layout
                    .tiles
                    .iter()
                    .filter(|t| matches!(t.kind, TileKind::App { .. }))
                    .map(|t| (t.id.clone(), t.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut merged = shell_layout;
        for tile in &mut merged.tiles {
            let TileKind::App { window_id, class } = &mut tile.kind else {
                continue;
            };
            let Some(existing) = compositor_apps.get(&tile.id) else {
                continue;
            };
            let TileKind::App {
                window_id: existing_wid,
                class: existing_class,
            } = &existing.kind
            else {
                continue;
            };
            if window_id.is_none() {
                *window_id = *existing_wid;
            }
            if class.as_deref().unwrap_or("").is_empty() {
                *class = existing_class.clone();
            }
        }

        for app in compositor_apps.values() {
            if !merged.tiles.iter().any(|t| t.id == app.id) {
                merged.tiles.push(app.clone());
            }
        }

        self.desk_mut_or_default(&key).layout = merged;
        self.gutter_px = gutter_px;
        self.ensure_app_tiles_for_open_windows();
        self.sync_grid_titlebar_chrome(&key);
        self.reposition_all_windows();
    }

    fn ensure_app_tiles_for_open_windows(&mut self) {
        for id in self.windows.ids() {
            self.ensure_app_tile_for_window(id);
        }
    }

    pub(crate) fn reposition_all_windows(&mut self) {
        for id in self.windows.ids() {
            self.remap_window_for_desktop(id);
        }
        self.restore_focus_stacking();
    }

    /// After bulk layout sync, put the user-focused window back on top without
    /// toggling xdg activation state on neighbors.
    fn restore_focus_stacking(&mut self) {
        let Some(id) = self.preferred_stacking_window() else {
            return;
        };
        self.raise_stacking_window(id, false);
    }

    /// Keep the window the user picked above neighbors while the pointer is over
    /// its chrome/body. Uses the window's frame geometry, not `element_under`, so
    /// a neighbor stacked too high during minimize/maximize restore cannot block
    /// the raise when the cursor reaches the chosen app.
    pub(crate) fn maintain_focus_stacking(&mut self, loc: Point<f64, Logical>) {
        use crate::desk_input::point_in_rect;

        if self.capture_overlay_active() {
            return;
        }
        if self.screenshot_overlay_active() {
            return;
        }

        if self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            return;
        }

        // Grab-less Metis menu / NC / CC: Exclusive keyboard focus lives on the
        // bar layer while the pointer still geometrically hits windows under the
        // translucent popover. Raising/focusing those windows every motion would
        // ping-pong with `handle_layer_commit` reclaiming Exclusive — flickering
        // the menu search `:focus-within` border and every SSD titlebar.
        if self.metis_bar_ui_hit(loc) {
            return;
        }
        if self.exclusive_keyboard_layer().is_some() {
            return;
        }

        // A transient X11 popup (menu / tooltip / combo dropdown) is mapped above
        // its parent toplevel. Auto-raising the registered window under the
        // pointer would restack it *above* its own override-redirect popup,
        // occluding the menu — the owning app (e.g. Steam) then treats it as
        // dismissed and closes it on the very next mouse move. Leave stacking
        // untouched while any OR popup is up.
        if self.has_mapped_override_redirect_popup() {
            return;
        }

        let Some(preferred) = self.preferred_stacking_window() else {
            return;
        };
        let Some(record) = self.windows.get(preferred).cloned() else {
            return;
        };
        if record.maximized || record.fullscreen || self.windows.is_minimized(preferred) {
            return;
        }
        let frame = self
            .ssd_frame_for_mapped_window(preferred, &record.window)
            .or_else(|| {
                let loc = self.space.element_location(&record.window)?;
                let size = record.window.geometry().size;
                Some(PixelRect {
                    x: loc.x,
                    y: loc.y,
                    width: size.w.max(1),
                    height: size.h.max(1),
                })
            });
        let Some(frame) = frame else {
            return;
        };
        let (x, y) = (loc.x as i32, loc.y as i32);
        if !point_in_rect(x, y, frame) {
            return;
        }
        if self.higher_window_client_occludes(x, y, preferred) {
            return;
        }
        self.raise_stacking_window(preferred, false);
        if self.focused_window_id() != Some(preferred) {
            let pointer_ok = self.seat.get_pointer().is_none_or(|p| !p.is_grabbed());
            let keyboard_ok = self.seat.get_keyboard().is_none_or(|k| !k.is_grabbed());
            if pointer_ok && keyboard_ok {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    keyboard.set_focus(self, Some(record.window.into()), serial);
                }
            }
        }
    }

    /// Reserve a grid slot as soon as an app registers (before its first buffer commit).
    fn ensure_app_tile_for_window(&mut self, id: u32) {
        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Free {
            return;
        }

        let tile_id = format!("app-{id}");
        // Already present, visible or stashed on this output's desk?
        if let Some(desk) = self.desk(&key) {
            if desk.layout.tiles.iter().any(|t| t.id == tile_id)
                || desk
                    .stashed_app_tiles
                    .values()
                    .any(|tiles| tiles.iter().any(|t| t.id == tile_id))
            {
                return;
            }
        }
        let class = self.windows.get(id).and_then(|r| r.app_id.clone());
        let active = self.active_workspace_for(&key);
        let desk = self.desk_mut_or_default(&key);
        let tile = metis_grid::GridTile {
            id: tile_id,
            rect: default_app_tile_rect(&desk.layout),
            kind: TileKind::App {
                window_id: Some(id),
                class,
            },
            glow: "cool".into(),
            pinned: false,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
        };
        if ws == active {
            desk.layout.tiles.push(tile);
        } else {
            desk.stashed_app_tiles.entry(ws).or_default().push(tile);
        }
        // Mirror membership into the scroll strip when this workspace scrolls.
        if self.layout_kind_for(&key, ws) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(&key, ws);
            self.refresh_scroll_offset(&key, false);
            // Slide the windows already in the strip into their new frames — a fresh
            // column shifts every column to its right, so the prior window must move
            // instead of the newcomer just painting on top of it.
            self.reposition_scroll_windows();
        } else {
            self.auto_reflow_grid_apps(&key, Some(id), true);
        }
    }

    /// Split the active grid workspace among grid-managed app windows.
    fn auto_reflow_grid_apps(
        &mut self,
        output_key: &str,
        focus_window_id: Option<u32>,
        emit: bool,
    ) {
        let ws = self.active_workspace_for(output_key);
        if self.layout_kind_for(output_key, ws) != metis_grid::LayoutKind::Grid {
            return;
        }

        self.prune_stale_app_tiles(output_key);

        let include: Vec<String> = self
            .desk(output_key)
            .map(|desk| {
                desk.layout
                    .tiles
                    .iter()
                    .filter_map(|t| {
                        if let TileKind::App {
                            window_id: Some(wid),
                            ..
                        } = &t.kind
                        {
                            self.is_window_grid_managed(*wid).then(|| t.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if include.is_empty() {
            return;
        }

        let focus_tile_id = focus_window_id.and_then(|id| self.tile_id_for_window(id));

        let desk = self.desk_mut_or_default(output_key);
        metis_grid::sanitize_layout(&mut desk.layout);
        let focus = focus_tile_id.as_deref();
        if let Err(err) = metis_grid::auto_tile_apps(&mut desk.layout, focus, &include) {
            tracing::warn!(%err, output = output_key, "auto_tile_apps failed after sanitize; retrying");
            metis_grid::sanitize_layout(&mut desk.layout);
            if let Err(err) = metis_grid::auto_tile_apps(&mut desk.layout, focus, &include) {
                tracing::warn!(%err, output = output_key, "auto_tile_apps failed after retry");
            }
        }
        self.sync_grid_titlebar_chrome(output_key);
        self.reposition_all_windows();
        self.persist_layout();
        if emit {
            self.emit_layout_changed();
        }
    }

    pub fn sync_all_app_windows(&mut self) {
        for id in self.windows.ids() {
            self.ensure_app_tile_for_window(id);
        }
        self.try_activate_all_pending();
        self.reposition_all_windows();
    }

    pub fn try_activate_all_pending(&mut self) {
        let pending: Vec<_> = self
            .windows
            .ids()
            .into_iter()
            .filter(|id| !self.windows.is_ready(*id))
            .filter_map(|id| {
                self.windows
                    .get(id)
                    .and_then(|record| record.wl_toplevel())
                    .map(|toplevel| toplevel.wl_surface().clone())
            })
            .collect();
        for surface in pending {
            self.try_activate_committed_window(&surface);
        }
    }

    fn persist_layout(&mut self) {
        // Persist the primary desk's layout (its widget positions) to `desk.json`.
        let key = self.primary_key();
        if let Some(desk) = self.desk(&key) {
            if let Err(err) = desk.layout.save_to_path(&desk_config_path()) {
                tracing::warn!(%err, "failed to persist grid layout");
            }
        }
    }

    pub fn emit_layout_changed(&self) {
        use metis_protocol::CompositorEvent;
        let key = self.primary_key();
        let layout = self
            .desk(&key)
            .map(|d| d.layout.clone())
            .unwrap_or_else(|| self.default_layout.clone());
        self.event_bus.emit(&CompositorEvent::LayoutChanged {
            layout,
            gutter_px: self.gutter_px,
            metrics: self.grid_metrics(),
        });
    }

    pub fn emit_monitor_changed(&self) {
        use metis_protocol::CompositorEvent;
        self.event_bus
            .emit(&CompositorEvent::MonitorChanged { rect: self.monitor });
    }

    pub fn emit_workspace_changed(&self, output_key: &str) {
        use metis_protocol::CompositorEvent;
        self.event_bus.emit(&CompositorEvent::WorkspaceChanged {
            output: output_key.to_string(),
            active: self.active_workspace_for(output_key),
            count: self.workspace_count(),
        });
    }

    /// Configured number of virtual workspaces (clamped to a sane 1..=12).
    pub fn workspace_count(&self) -> u32 {
        metis_config::load_bar_config().workspace_count.clamp(1, 12)
    }

    /// Configured multi-monitor workspace behavior (independent vs. linked).
    pub fn workspace_mode(&self) -> metis_config::WorkspaceMode {
        metis_config::load_bar_config().workspace_mode
    }

    /// Switch workspace honoring the configured multi-monitor mode. In `Separate`
    /// only `requested_output` changes; in `Linked` every output switches to the
    /// same workspace at once (each emits its own `WorkspaceChanged`).
    pub fn switch_workspace_routed(&mut self, requested_output: &str, target: u32) {
        if self.workspace_mode() == metis_config::WorkspaceMode::Linked {
            let keys: Vec<String> = self.space.outputs().map(|o| o.name()).collect();
            if keys.is_empty() {
                self.switch_workspace(requested_output, target);
            } else {
                for key in keys {
                    self.switch_workspace(&key, target);
                }
            }
        } else {
            self.switch_workspace(requested_output, target);
        }
    }

    /// Step to the previous/next workspace (wrapping at 1..=`workspace_count()`),
    /// honoring linked vs. separate multi-monitor mode.
    pub fn cycle_workspace_routed(&mut self, requested_output: &str, delta: i32) {
        let count = self.workspace_count();
        let current = self.active_workspace_for(requested_output);
        let target = if delta >= 0 {
            if current >= count {
                1
            } else {
                current + 1
            }
        } else if current <= 1 {
            count
        } else {
            current - 1
        };
        self.switch_workspace_routed(requested_output, target);
    }

    /// Show a different virtual workspace on a single output. Stashes that
    /// output's visible app tiles (and unmaps their windows), then restores the
    /// target workspace's tiles and remaps its windows. Other outputs and the
    /// desk widget tiles are untouched.
    pub fn switch_workspace(&mut self, output_key: &str, target: u32) {
        let target = target.clamp(1, self.workspace_count());
        let current = self.active_workspace_for(output_key);
        if target == current {
            return;
        }

        if self.layout_kind_for(output_key, current) == metis_grid::LayoutKind::Free {
            for id in self.window_ids_on_workspace(output_key, current) {
                if let Some(record) = self.windows.get(id).cloned() {
                    self.space.unmap_elem(&record.window);
                }
            }
            self.desk_mut_or_default(output_key).active_workspace = target;
            for id in self.window_ids_on_workspace(output_key, target) {
                self.ensure_app_tile_for_window(id);
                self.remap_window_for_desktop(id);
            }
            self.focus_topmost_on_active_workspace();
            self.emit_workspace_changed(output_key);
            return;
        }

        // Pull this output's app tiles out of its live grid and remember them.
        let mut stashed: Vec<metis_grid::GridTile> = Vec::new();
        {
            let desk = self.desk_mut_or_default(output_key);
            desk.layout.tiles.retain(|t| {
                if matches!(t.kind, TileKind::App { .. }) {
                    stashed.push(t.clone());
                    false
                } else {
                    true
                }
            });
        }
        // Hide the windows that just left the visible workspace.
        for tile in &stashed {
            if let TileKind::App {
                window_id: Some(wid),
                ..
            } = &tile.kind
            {
                if let Some(record) = self.windows.get(*wid).cloned() {
                    self.space.unmap_elem(&record.window);
                }
            }
        }
        {
            let desk = self.desk_mut_or_default(output_key);
            desk.stashed_app_tiles.insert(current, stashed);
            desk.active_workspace = target;
            // Restore the target workspace's app tiles.
            if let Some(tiles) = desk.stashed_app_tiles.remove(&target) {
                desk.layout.tiles.extend(tiles);
            }
        }
        self.refresh_scroll_offset(output_key, false);
        if self.layout_kind_for(output_key, target) == metis_grid::LayoutKind::Grid {
            self.auto_reflow_grid_apps(
                output_key,
                self.last_focused_window.or(self.focused_window_id()),
                false,
            );
        }
        for id in self.window_ids_on_workspace(output_key, target) {
            self.ensure_app_tile_for_window(id);
        }
        self.reposition_all_windows();
        self.focus_topmost_on_active_workspace();

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(output_key);
    }

    /// Output names sorted left-to-right (then top-to-bottom) for adjacent-monitor
    /// navigation.
    fn output_keys_left_to_right(&self) -> Vec<String> {
        let mut outputs: Vec<_> = self.space.outputs().collect();
        outputs.sort_by_key(|o| {
            self.space
                .output_geometry(o)
                .map(|g| (g.loc.x, g.loc.y))
                .unwrap_or((0, 0))
        });
        outputs.into_iter().map(|o| o.name()).collect()
    }

    fn adjacent_output_key(&self, from: &str, direction: i32) -> Option<String> {
        let keys = self.output_keys_left_to_right();
        let idx = keys.iter().position(|k| k == from)?;
        let next = idx as i32 + direction;
        if next < 0 || next >= keys.len() as i32 {
            return None;
        }
        Some(keys[next as usize].clone())
    }

    /// Remove a window's app tile from one output desk (visible layout or stash).
    fn take_app_tile_from_desk(
        &mut self,
        desk_key: &str,
        window_id: u32,
        workspace: u32,
    ) -> Option<metis_grid::GridTile> {
        let tile_id = format!("app-{window_id}");
        let desk = self.desk_mut_or_default(desk_key);
        if let Some(pos) = desk.layout.tiles.iter().position(|t| t.id == tile_id) {
            return Some(desk.layout.tiles.remove(pos));
        }
        if let Some(tiles) = desk.stashed_app_tiles.get_mut(&workspace) {
            if let Some(pos) = tiles.iter().position(|t| t.id == tile_id) {
                return Some(tiles.remove(pos));
            }
        }
        for tiles in desk.stashed_app_tiles.values_mut() {
            if let Some(pos) = tiles.iter().position(|t| t.id == tile_id) {
                return Some(tiles.remove(pos));
            }
        }
        None
    }

    fn remove_window_from_desk_scroll(&mut self, desk_key: &str, window_id: u32, workspace: u32) {
        if let Some(desk) = self.desks.get_mut(desk_key) {
            if let Some(scroll) = desk.scroll.get_mut(&workspace) {
                scroll.remove_window(window_id);
            }
        }
    }

    /// Move a window to another output, keeping its workspace number. Desk tiles
    /// and scroll membership follow the window; visibility follows the destination
    /// output's active workspace.
    pub fn move_window_to_output(&mut self, window_id: u32, target_key: &str) {
        self.move_window_to_output_inner(window_id, target_key, true);
    }

    /// Like [`move_window_to_output`](Self::move_window_to_output) but optionally
    /// skips geometry clamp/reposition (used when a snap immediately follows).
    fn move_window_to_output_inner(&mut self, window_id: u32, target_key: &str, reposition: bool) {
        if target_key.is_empty() {
            return;
        }
        if self.output_by_name(target_key).is_none() && !self.desks.contains_key(target_key) {
            return;
        }
        self.desk_mut_or_default(target_key);

        let source_key = self.desk_key_for_window(window_id);
        if source_key == target_key {
            return;
        }

        let workspace = self.windows.workspace(window_id).unwrap_or(1);
        let source_active = self.active_workspace_for(&source_key);
        let target_active = self.active_workspace_for(target_key);
        let was_visible = workspace == source_active;
        let will_be_visible = workspace == target_active;

        let mut tile = self.take_app_tile_from_desk(&source_key, window_id, workspace);
        self.remove_window_from_desk_scroll(&source_key, window_id, workspace);

        if tile.is_none() {
            let class = self.windows.get(window_id).and_then(|r| r.app_id.clone());
            tile = Some(metis_grid::GridTile {
                id: format!("app-{window_id}"),
                rect: default_app_tile_rect(
                    self.desk(target_key)
                        .map(|d| &d.layout)
                        .unwrap_or(&self.default_layout),
                ),
                kind: TileKind::App {
                    window_id: Some(window_id),
                    class,
                },
                glow: "cool".into(),
                pinned: false,
                min_w: None,
                max_w: None,
                min_h: None,
                max_h: None,
            });
        }

        self.windows.set_output(window_id, target_key.to_string());

        if let Some(tile) = tile {
            let desk = self.desk_mut_or_default(target_key);
            if will_be_visible {
                desk.layout.tiles.push(tile);
            } else {
                desk.stashed_app_tiles
                    .entry(workspace)
                    .or_default()
                    .push(tile);
            }
        }

        if self.layout_kind_for(target_key, workspace) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(target_key, workspace);
            self.refresh_scroll_offset(target_key, false);
        }

        if was_visible && !will_be_visible {
            if let Some(record) = self.windows.get(window_id).cloned() {
                self.space.unmap_elem(&record.window);
            }
            self.focus_topmost_on_active_workspace();
        } else if will_be_visible && reposition {
            if self.floating.contains(&window_id) {
                // Auto-hide / snapped windows keep their footprint — never apply
                // the ordinary floating titlebar inset (`APP_TILE_HEADER_PX`).
                if !self.auto_hide_titlebar.contains(&window_id)
                    && !self.windows.is_snapped(window_id)
                {
                    if let Some(rect) = self.windows.target_rect(window_id) {
                        let clamped = self.clamp_floating_rect_for(window_id, rect);
                        if clamped != rect {
                            self.windows.set_target_rect(window_id, clamped);
                        }
                    }
                }
                self.apply_window_rect(window_id);
            } else {
                self.apply_window_rect(window_id);
            }
            self.focus_window_id(window_id);
        }

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(&source_key);
        self.emit_workspace_changed(target_key);
    }

    /// If a window's center sits on a different output than its assigned desk,
    /// re-home it there. Called after a drag-drop or snap on another monitor.
    pub fn maybe_adopt_window_output(&mut self, window_id: u32) {
        let Some(target) = self.output_for_window(window_id).map(|o| o.name()) else {
            return;
        };
        if target == self.desk_key_for_window(window_id) {
            return;
        }
        self.move_window_to_output(window_id, &target);
    }

    /// Move the focused window one output to the left (`direction` = -1) or right (+1).
    pub fn move_window_to_adjacent_output(&mut self, window_id: u32, direction: i32) {
        let from = self.desk_key_for_window(window_id);
        let Some(target) = self.adjacent_output_key(&from, direction) else {
            return;
        };
        self.move_window_to_output(window_id, &target);
    }

    /// Move every window on `workspace` from `source_key` to `target_key` (keeping
    /// the same workspace number). Layout mode and scroll state for that workspace
    /// move with the windows. Only valid in independent per-output workspace mode.
    pub fn move_workspace_to_output(&mut self, source_key: &str, workspace: u32, target_key: &str) {
        if source_key.is_empty() || target_key.is_empty() || source_key == target_key {
            return;
        }
        if self.workspace_mode() != metis_config::WorkspaceMode::Separate {
            return;
        }
        if self.output_by_name(target_key).is_none() && !self.desks.contains_key(target_key) {
            return;
        }
        self.desk_mut_or_default(target_key);

        let ws = workspace.clamp(1, self.workspace_count());
        let source_active = self.active_workspace_for(source_key);
        let target_active = self.active_workspace_for(target_key);
        let was_visible_on_source = ws == source_active;
        let will_be_visible_on_target = ws == target_active;

        let window_ids: Vec<u32> = self
            .windows
            .ids()
            .into_iter()
            .filter(|&id| {
                self.desk_key_for_window(id) == source_key && self.windows.workspace(id) == Some(ws)
            })
            .collect();

        let (kind, scroll, mut tiles) = {
            let desk = self.desk_mut_or_default(source_key);
            let kind = desk.layout_kind.remove(&ws);
            let scroll = desk.scroll.remove(&ws);
            let mut tiles = desk.stashed_app_tiles.remove(&ws).unwrap_or_default();
            if was_visible_on_source {
                let on_ws: std::collections::HashSet<u32> = window_ids.iter().copied().collect();
                desk.layout.tiles.retain(|t| {
                    if let TileKind::App {
                        window_id: Some(wid),
                        ..
                    } = &t.kind
                    {
                        if on_ws.contains(wid) {
                            tiles.push(t.clone());
                            return false;
                        }
                    }
                    true
                });
            }
            (kind, scroll, tiles)
        };

        let default_layout = self
            .desk(target_key)
            .map(|d| &d.layout)
            .unwrap_or(&self.default_layout);
        for &id in &window_ids {
            let tile_id = format!("app-{id}");
            if tiles.iter().any(|t| t.id == tile_id) {
                continue;
            }
            let class = self.windows.get(id).and_then(|r| r.app_id.clone());
            tiles.push(metis_grid::GridTile {
                id: tile_id,
                rect: default_app_tile_rect(default_layout),
                kind: TileKind::App {
                    window_id: Some(id),
                    class,
                },
                glow: "cool".into(),
                pinned: false,
                min_w: None,
                max_w: None,
                min_h: None,
                max_h: None,
            });
        }

        if was_visible_on_source {
            for &id in &window_ids {
                if let Some(record) = self.windows.get(id).cloned() {
                    self.space.unmap_elem(&record.window);
                }
            }
        }

        for &id in &window_ids {
            self.windows.set_output(id, target_key.to_string());
        }

        {
            let desk = self.desk_mut_or_default(target_key);
            if let Some(k) = kind {
                desk.layout_kind.insert(ws, k);
            }
            if let Some(s) = scroll {
                desk.scroll.insert(ws, s);
            }
            if will_be_visible_on_target {
                desk.layout.tiles.extend(tiles);
            } else {
                desk.stashed_app_tiles.entry(ws).or_default().extend(tiles);
            }
        }

        if will_be_visible_on_target {
            self.refresh_scroll_offset(target_key, false);
            for &id in &window_ids {
                if !self.windows.is_minimized(id) {
                    self.apply_window_rect(id);
                }
            }
            self.focus_topmost_on_active_workspace();
        } else if was_visible_on_source {
            self.focus_topmost_on_active_workspace();
        }

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        self.emit_workspace_changed(source_key);
        self.emit_workspace_changed(target_key);
    }

    /// Move the active workspace on `source_key` one output left/right.
    pub fn move_active_workspace_to_adjacent_output(&mut self, source_key: &str, direction: i32) {
        let ws = self.active_workspace_for(source_key);
        let Some(target) = self.adjacent_output_key(source_key, direction) else {
            return;
        };
        self.move_workspace_to_output(source_key, ws, &target);
    }

    /// True when the workspace under the pointer uses the scrolling layout (so
    /// Super+Shift+arrow is reserved for scroll navigation).
    pub fn scroll_navigation_active(&self) -> bool {
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        self.active_layout_kind(&key) == metis_grid::LayoutKind::Scroll
    }

    /// Move a window to another workspace on its own output. When it leaves or
    /// joins that output's visible workspace its tile is stashed/restored and the
    /// window is hidden/shown.
    pub fn move_window_to_workspace(&mut self, window_id: u32, target: u32) {
        let target = target.clamp(1, self.workspace_count());
        let Some(current) = self.windows.workspace(window_id) else {
            return;
        };
        if target == current {
            return;
        }
        let key = self.desk_key_for_window(window_id);
        self.windows.set_workspace(window_id, target);
        let tile_id = format!("app-{window_id}");
        let active = self.active_workspace_for(&key);

        if current == active {
            // Leaving the visible workspace: stash its tile and hide it.
            let mut moved: Vec<metis_grid::GridTile> = Vec::new();
            {
                let desk = self.desk_mut_or_default(&key);
                desk.layout.tiles.retain(|t| {
                    if t.id == tile_id {
                        moved.push(t.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            if let Some(record) = self.windows.get(window_id).cloned() {
                self.space.unmap_elem(&record.window);
            }
            self.desk_mut_or_default(&key)
                .stashed_app_tiles
                .entry(target)
                .or_default()
                .extend(moved);
            self.reposition_all_windows();
            self.focus_topmost_on_active_workspace();
        } else if target == active {
            // Joining the visible workspace: pull its tile back into the grid.
            let desk = self.desk_mut_or_default(&key);
            if let Some(tiles) = desk.stashed_app_tiles.get_mut(&current) {
                if let Some(pos) = tiles.iter().position(|t| t.id == tile_id) {
                    let tile = tiles.remove(pos);
                    desk.layout.tiles.push(tile);
                }
            }
            self.reposition_all_windows();
        } else {
            // Hidden-to-hidden: just relocate the stashed tile.
            let desk = self.desk_mut_or_default(&key);
            let tile = desk.stashed_app_tiles.get_mut(&current).and_then(|tiles| {
                tiles
                    .iter()
                    .position(|t| t.id == tile_id)
                    .map(|pos| tiles.remove(pos))
            });
            if let Some(tile) = tile {
                desk.stashed_app_tiles.entry(target).or_default().push(tile);
            }
        }

        // Keep the scroll strips in sync: drop from the source workspace, add to
        // the target if it scrolls.
        self.remove_from_scroll_everywhere(window_id);
        if self.layout_kind_for(&key, target) == metis_grid::LayoutKind::Scroll {
            self.seed_scroll_state(&key, target);
        }
        self.refresh_scroll_offset(&key, false);

        self.damaged = true;
        self.request_redraw();
        self.emit_layout_changed();
        // Nudge the shell to reconcile its window cache so per-output/per-workspace
        // dock filtering reflects the move promptly (the active workspace itself is
        // unchanged; this just carries the refresh).
        self.emit_workspace_changed(&key);
    }

    /// Give keyboard focus to the topmost mapped window on its output's active
    /// workspace, or clear focus if no eligible window is visible.
    fn focus_topmost_on_active_workspace(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        // `space.elements()` is bottom-to-top; the last match is the topmost.
        let ordered: Vec<Window> = self.space.elements().cloned().collect();
        let candidate = ordered.into_iter().rev().find_map(|w| {
            let id = self.windows.id_for_window(&w)?;
            let key = self.desk_key_for_window(id);
            let on_active = self.windows.workspace(id) == Some(self.active_workspace_for(&key));
            if on_active && !self.windows.is_minimized(id) {
                Some((id, w))
            } else {
                None
            }
        });
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        match candidate {
            Some((id, window)) => {
                self.space.raise_element(&window, true);
                keyboard.set_focus(self, Some(window.into()), serial);
                self.event_bus
                    .emit(&metis_protocol::CompositorEvent::WindowFocused { id });
            }
            None => {
                keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
            }
        }
    }

    pub fn handle_ipc(&mut self, cmd: CompositorCommand) -> metis_protocol::CompositorEvent {
        self.handle_ipc_with_caps(cmd, IpcCaps::Full)
    }

    pub fn handle_ipc_with_caps(
        &mut self,
        cmd: CompositorCommand,
        caps: IpcCaps,
    ) -> metis_protocol::CompositorEvent {
        use metis_protocol::CompositorEvent;
        // Widgets allowlist + lock denylist (pure helpers in ipc_dispatch).
        if let crate::ipc_dispatch::IpcGate::Reject(message) =
            crate::ipc_dispatch::gate_ipc_command(&cmd, caps, self.session_is_locked())
        {
            return CompositorEvent::Error {
                message: message.into(),
            };
        }
        match cmd {
            CompositorCommand::Ping => CompositorEvent::Pong,
            CompositorCommand::GetMonitor => CompositorEvent::Monitor { rect: self.monitor },
            CompositorCommand::ListOutputs => {
                let cfg = self.output_runtime.cached();
                let mirror_source = self.resolve_mirror_source_name();
                let primary = if self.mirror_mode_active() {
                    mirror_source.clone()
                } else {
                    cfg.primary_output.clone().or_else(|| {
                        self.space
                            .outputs()
                            .find(|o| o.name() != "metis-render")
                            .map(|o| o.name())
                    })
                };
                let mut outputs: Vec<_> = self.connected_outputs();
                outputs.sort_by(|a, b| {
                    let a_pri = primary.as_deref() == Some(a.name().as_str());
                    let b_pri = primary.as_deref() == Some(b.name().as_str());
                    b_pri.cmp(&a_pri).then_with(|| {
                        let a_key = crate::output_prefs::output_geometry(self, a)
                            .map(|g| (g.loc.x, g.loc.y, a.name()))
                            .unwrap_or((0, 0, a.name()));
                        let b_key = crate::output_prefs::output_geometry(self, b)
                            .map(|g| (g.loc.x, g.loc.y, b.name()))
                            .unwrap_or((0, 0, b.name()));
                        a_key.cmp(&b_key)
                    })
                });
                let mirror_ref = mirror_source.as_deref();
                let primary_ref = primary.as_deref();
                let outputs = outputs
                    .iter()
                    .map(|o| crate::output_prefs::output_info_for(self, o, primary_ref, mirror_ref))
                    .collect();
                CompositorEvent::OutputList { outputs }
            }
            CompositorCommand::ListOutputModes { output } => {
                let (modes, current) = crate::output_modes::list_output_modes(self, &output);
                CompositorEvent::OutputModes { modes, current }
            }
            CompositorCommand::GetLayout => {
                let key = self.primary_key();
                let layout = self
                    .desk(&key)
                    .map(|d| d.layout.clone())
                    .unwrap_or_else(|| self.default_layout.clone());
                CompositorEvent::LayoutChanged {
                    layout,
                    gutter_px: self.gutter_px,
                    metrics: self.grid_metrics(),
                }
            }
            CompositorCommand::ListWindows => {
                // Use the full registry (includes minimized/unmapped). Walking
                // `space.elements()` only sees mapped windows and previously
                // skipped minimized ones, so shell reconcile wiped them from the
                // task dock and the user could not restore them.
                let focused = self.focused_window_id();
                let mut windows = self.windows.list();
                for w in &mut windows {
                    w.focused = focused == Some(w.id);
                }
                // Front-ish order: focused first, then non-minimized, then minimized.
                windows.sort_by_key(|w| {
                    (
                        if focused == Some(w.id) { 0 } else { 1 },
                        if w.minimized { 2 } else { 1 },
                        w.id,
                    )
                });
                CompositorEvent::WindowList { windows }
            }
            CompositorCommand::CaptureWindowThumbs { ids } => {
                for id in &ids {
                    self.queue_window_thumb(*id);
                }
                CompositorEvent::WindowThumbs {
                    thumbs: self.existing_window_thumbs(&ids),
                }
            }
            CompositorCommand::CaptureWorkspaceThumbs { output, workspaces } => {
                for ws in &workspaces {
                    self.queue_workspace_thumb(output.clone(), *ws);
                }
                CompositorEvent::WorkspaceThumbs {
                    thumbs: self.existing_workspace_thumbs(&output, &workspaces),
                }
            }
            CompositorCommand::MoveWindow { id, rect } => {
                self.windows.set_target_rect(id, rect);
                self.apply_window_rect(id);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::CloseWindow { id } => {
                self.close_window(id);
                CompositorEvent::WindowClosed { id }
            }
            CompositorCommand::FocusWindow { id } => {
                if let Some(record) = self.windows.get(id).cloned() {
                    self.note_window_focus(id);
                    self.space.raise_element(&record.window, true);
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    if let Some(keyboard) = self.seat.get_keyboard() {
                        keyboard.set_focus(self, Some(record.window.clone().into()), serial);
                    } else {
                        tracing::warn!("FocusWindow: seat has no keyboard");
                    }
                    self.event_bus.emit(&CompositorEvent::WindowFocused { id });
                    self.schedule_redraw();
                    CompositorEvent::WindowFocused { id }
                } else {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                }
            }
            CompositorCommand::SetMinimized { id, minimized } => {
                if self.windows.get(id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                } else {
                    if minimized {
                        self.minimize_by_id(id);
                    } else {
                        self.activate_window_by_id(id);
                    }
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::ActivateWindow { id } => {
                if self.windows.get(id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                } else {
                    self.activate_window_by_id(id);
                    CompositorEvent::WindowFocused { id }
                }
            }
            CompositorCommand::SetFullscreen { id, enabled } => {
                self.set_fullscreen(id, enabled, None);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::ApplyLayout { layout, gutter_px } => {
                self.apply_grid_layout(layout, gutter_px);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SetTileMode { tile_id, mode } => {
                self.set_tile_mode(&tile_id, mode);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SwitchWorkspace { output, id } => {
                let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                    self.output_under_pointer()
                        .map(|o| o.name())
                        .unwrap_or_else(|| self.primary_key())
                });
                self.switch_workspace_routed(&key, id);
                CompositorEvent::WorkspaceChanged {
                    output: key.clone(),
                    active: self.active_workspace_for(&key),
                    count: self.workspace_count(),
                }
            }
            CompositorCommand::MoveWindowToWorkspace {
                window_id,
                workspace,
            } => {
                if self.windows.get(window_id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {window_id} not found"),
                    }
                } else {
                    self.move_window_to_workspace(window_id, workspace);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::MoveWindowToOutput { window_id, output } => {
                if self.windows.get(window_id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {window_id} not found"),
                    }
                } else {
                    let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                        self.output_under_pointer()
                            .map(|o| o.name())
                            .unwrap_or_else(|| self.primary_key())
                    });
                    self.move_window_to_output(window_id, &key);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::MoveWorkspaceToOutput {
                output,
                workspace,
                target_output,
            } => {
                if target_output.is_empty() {
                    CompositorEvent::Error {
                        message: "target_output is required".into(),
                    }
                } else if self.workspace_mode() != metis_config::WorkspaceMode::Separate {
                    CompositorEvent::Error {
                        message: "MoveWorkspaceToOutput requires independent per-output workspaces"
                            .into(),
                    }
                } else {
                    let source = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                        self.output_under_pointer()
                            .map(|o| o.name())
                            .unwrap_or_else(|| self.primary_key())
                    });
                    let ws = workspace.unwrap_or_else(|| self.active_workspace_for(&source));
                    self.move_workspace_to_output(&source, ws, &target_output);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::SetWorkspaceLayout {
                output,
                workspace,
                kind,
            } => {
                let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                    self.output_under_pointer()
                        .map(|o| o.name())
                        .unwrap_or_else(|| self.primary_key())
                });
                // A specific non-active workspace is set quietly (it's hidden);
                // otherwise act on the output's active workspace (rebuilds the
                // strip + repositions live).
                match workspace {
                    Some(ws) if ws != self.active_workspace_for(&key) => {
                        self.set_layout_kind_on(&key, ws, kind);
                    }
                    _ => self.set_layout_kind(&key, kind),
                }
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SetDefaultLayout { kind } => {
                self.set_layout_kind_all(kind);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SubscribeEvents => CompositorEvent::Pong,
            CompositorCommand::Launch { argv, program } => {
                let argv = metis_protocol::launch_argv(&argv, &program);
                self.spawn_client_argv(&argv);
                CompositorEvent::Pong
            }
            CompositorCommand::EndSession => {
                tracing::info!("EndSession requested");
                self.end_compositor_session();
                CompositorEvent::Pong
            }
            CompositorCommand::ApplyBackground => {
                self.wallpaper.apply_config();
                let (full, regions) = self.wallpaper_layout();
                self.wallpaper.set_layout(full, regions);
                self.wallpaper.start_async_decode();
                self.damaged = true;
                self.request_redraw();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadInput => {
                let cfg = self.input_runtime.reload_from_disk();
                crate::device_input::apply_keyboard(self, &cfg);
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadKeybinds => {
                self.keybinds.reload();
                CompositorEvent::Pong
            }
            CompositorCommand::SetKeybindCapture { active } => {
                crate::keybinds::set_capture_active(active);
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadOutputs => {
                self.schedule_outputs_reload();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadPower => {
                let cfg = metis_config::load_power_config();
                self.idle.set_blank_after_minutes(cfg.blank_after_minutes);
                self.idle_reschedule();
                if self.battery_dim.apply_config(cfg.dim_on_battery) {
                    self.damaged = true;
                    self.request_redraw();
                }
                let _ = self.battery_dim.poll_battery();
                tracing::info!(
                    blank_after_minutes = cfg.blank_after_minutes,
                    dim_on_battery = cfg.dim_on_battery,
                    on_battery = self.battery_dim.on_battery,
                    "reloaded power config; idle blank + battery dim updated"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::LockSession => {
                self.lock_session();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadLock => {
                self.lock_reload();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadGaming => {
                self.gaming_config = metis_config::load_gaming_config();
                tracing::info!(
                    graphics_mode = ?self.gaming_config.graphics_mode,
                    "reloaded gaming config"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadDecorations => {
                self.decoration_overrides.reload();
                self.refresh_all_window_decoration_modes();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadLocale => {
                metis_i18n::reload();
                self.lock.clear_gpu_cache();
                tracing::info!(
                    locale = %metis_i18n::locale_info().tag,
                    "reloaded locale; cleared lock text cache"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::SetClipboard {
                mime,
                text,
                image_path,
            } => {
                if let Err(message) = self.set_clipboard_from_command(mime, text, image_path) {
                    CompositorEvent::Error { message }
                } else {
                    CompositorEvent::Pong
                }
            }
            CompositorCommand::InhibitIdle {
                cookie,
                app_name,
                reason,
            } => {
                let label = match (app_name, reason) {
                    (Some(app), Some(r)) => format!("{app}: {r}"),
                    (Some(app), None) => app,
                    (None, Some(r)) => r,
                    (None, None) => "external".to_string(),
                };
                self.idle_add_external_inhibitor(cookie, label);
                CompositorEvent::Pong
            }
            CompositorCommand::UninhibitIdle { cookie } => {
                self.idle_remove_external_inhibitor(cookie);
                CompositorEvent::Pong
            }
            CompositorCommand::BeginCaptureOverlay { app_id } => {
                self.begin_capture_overlay_portal(app_id);
                CompositorEvent::Pong
            }
            CompositorCommand::EndCaptureOverlay { app_id } => {
                self.end_capture_overlay_portal(app_id);
                CompositorEvent::Pong
            }
            CompositorCommand::BeginScreenshotOverlay => {
                self.begin_screenshot_overlay();
                CompositorEvent::Pong
            }
            CompositorCommand::EndScreenshotOverlay => {
                self.end_screenshot_overlay();
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerAbsolute { x, y } => {
                self.inject_remote_pointer_absolute(x, y);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerRelative { dx, dy } => {
                self.inject_remote_pointer_relative(dx, dy);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerButton { button, pressed } => {
                self.inject_remote_pointer_button(button, pressed);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerScroll { dx, dy } => {
                self.inject_remote_pointer_scroll(dx, dy);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemoteKey { keycode, pressed } => {
                self.inject_remote_key(keycode, pressed);
                CompositorEvent::Pong
            }
        }
    }

    /// The wallpaper layout: the whole virtual desktop's physical size plus one
    /// region per output (global physical origin + size). The wallpaper composes
    /// a single framebuffer-sized texture by cover-cropping each output's image
    /// into its region, so every monitor is filled independently.
    pub fn wallpaper_layout(
        &self,
    ) -> (
        smithay::utils::Size<i32, smithay::utils::Physical>,
        Vec<crate::wallpaper::OutputRegion>,
    ) {
        let bounds = self.desktop_bounds();
        let full = smithay::utils::Size::from((bounds.size.w, bounds.size.h)).to_physical(1);
        let regions = self
            .space
            .outputs()
            .filter_map(|o| {
                let geo = self.space.output_geometry(o)?;
                Some(crate::wallpaper::OutputRegion {
                    name: o.name(),
                    origin: (geo.loc - bounds.loc).to_physical(1),
                    size: geo.size.to_physical(1),
                })
            })
            .collect();
        (full, regions)
    }

    pub fn register_new_window(&mut self, window: Window, title: String, app_id: Option<String>) {
        let id = self.windows.register(window, title, app_id);
        // New windows open on the output under the cursor, joining that output's
        // currently-visible workspace.
        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        self.windows.set_output(id, key.clone());
        self.windows
            .set_workspace(id, self.active_workspace_for(&key));
        self.ensure_app_tile_for_window(id);
    }

    /// Bring an XWayland toplevel under full Metis management: register it in the
    /// shared window registry, give it a Metis server-side titlebar, place it as a
    /// bar-aware floating window, and announce it to the shell (dock/IPC). X11
    /// windows do not participate in the tiling grid — they are always floating.
    pub(crate) fn map_x11_toplevel(&mut self, window: X11Surface) {
        use metis_protocol::CompositorEvent;

        let is_remap = self.windows.id_for_x11_window(window.window_id()).is_some();
        tracing::info!(
            x11_window = window.window_id(),
            override_redirect = window.is_override_redirect(),
            class = %window.class(),
            title = %window.title(),
            remap = is_remap,
            "x11: map request"
        );

        if window.is_override_redirect() {
            // Menus / tooltips / drag surfaces: map at their requested location and
            // leave them undecorated and untracked. `geometry()`/`bbox()` are
            // size-only for X11 (loc is always the origin), so use the configured
            // rectangle's root-relative location instead.
            let loc = window.last_configure().loc;
            let elem = Window::new_x11_window(window);
            self.space.map_element(elem, loc, true);
            self.schedule_redraw();
            return;
        }

        if let Err(err) = window.set_mapped(true) {
            tracing::warn!(%err, "failed to map X11 window");
            return;
        }

        // A remap of an already-tracked window (e.g. an Electron app restoring from
        // its tray) re-applies geometry and re-announces to the shell — the withdraw
        // in `unmap_x11_toplevel` dropped the dock entry, so we re-emit WindowOpened.
        if let Some(existing) = self.windows.id_for_x11_window(window.window_id()) {
            self.windows.set_minimized(existing, false);
            // Cancel any pending withdraw: this remap proves the earlier unmap was
            // transient (Electron churn / a tray restore), not a real close.
            self.x11_pending_withdraw.remove(&existing);
            let was_ready = self.windows.is_ready(existing);
            self.windows.set_ready(existing, true);
            // Restoring from the tray (a client-driven remap) must surface the
            // window on the workspace the user is actually looking at. The dock
            // path does this via `activate_window_by_id`; without it here,
            // `apply_window_rect`'s visibility guard would unmap a window whose
            // stale workspace no longer matches the active one — the window would
            // flash open and immediately vanish ("opens then closes").
            let key = self.desk_key_for_window(existing);
            self.windows
                .set_workspace(existing, self.active_workspace_for(&key));
            self.apply_window_rect(existing);
            if !was_ready {
                if let Some(record) = self.windows.get(existing).cloned() {
                    let (title, app_id) = self.read_window_metadata(&record);
                    let suggested_rect = self.windows.target_rect(existing).unwrap_or(PixelRect {
                        x: 0,
                        y: 0,
                        width: 800,
                        height: 600,
                    });
                    self.event_bus.emit(&CompositorEvent::WindowOpened {
                        id: existing,
                        title,
                        app_id,
                        suggested_rect,
                    });
                }
            }
            let _ = window.set_activated(true);
            self.note_window_focus(existing);
            self.focus_window_id(existing);
            self.event_bus
                .emit(&CompositorEvent::WindowFocused { id: existing });
            self.schedule_redraw();
            return;
        }

        let elem = Window::new_x11_window(window.clone());
        // Map once so the space can resolve the element before we position it.
        self.space.map_element(elem.clone(), (0, 0), false);

        let title = {
            let t = window.title();
            if t.trim().is_empty() {
                "Application".to_string()
            } else {
                t
            }
        };
        let app_id = {
            let class = window.class();
            if class.trim().is_empty() {
                None
            } else {
                Some(class)
            }
        };

        let id = self
            .windows
            .register_x11(elem, window.clone(), title.clone(), app_id.clone());
        if let Some(surface) = window.wl_surface() {
            use smithay::reexports::wayland_server::Resource;
            self.windows
                .index_x11_surface(window.window_id(), surface.id());
        }

        let key = self
            .output_under_pointer()
            .map(|o| o.name())
            .unwrap_or_else(|| self.primary_key());
        self.windows.set_output(id, key.clone());
        self.windows
            .set_workspace(id, self.active_workspace_for(&key));
        // X11 windows are floating; never reserve a grid tile for them.
        self.floating.insert(id);
        self.refresh_window_decoration_mode(id);
        let is_splash = {
            use smithay::xwayland::xwm::WmWindowType;
            matches!(window.window_type(), Some(WmWindowType::Splash))
                || title_looks_like_splash(&title)
        };
        // Splash screens share the main app's WM_CLASS — force natural size so we
        // don't stretch/tile a small bitmap into the saved main-window geometry.
        if is_splash {
            // Prefer no Metis chrome on boot splash; keep Motif/heuristic unless the
            // user forced SSD (already applied above). Splash bitmaps often look wrong
            // under a titlebar inset.
            tracing::info!(id, %title, "x11: splash window — natural size placement");
        }
        // Match Wayland: game-rules can request true-fullscreen once mapped.
        let rule = self
            .game_rules
            .evaluate(app_id.as_deref(), Some(title.as_str()));
        if rule.fullscreen && !is_splash {
            self.pending_game_fullscreen.insert(id);
        }
        self.place_x11_window(id, window.geometry().size, app_id.as_deref(), is_splash);
        self.apply_window_rect(id);
        self.windows.set_ready(id, true);
        let _ = window.set_activated(true);

        self.persist_layout();
        self.emit_layout_changed();
        let suggested_rect = self.windows.target_rect(id).unwrap_or(PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        });
        self.event_bus.emit(&CompositorEvent::WindowOpened {
            id,
            title,
            app_id,
            suggested_rect,
        });
        self.note_window_focus(id);
        self.focus_window_id(id);
        self.event_bus.emit(&CompositorEvent::WindowFocused { id });
        if self.pending_game_fullscreen.remove(&id) {
            self.set_fullscreen(id, true, None);
        }
        self.schedule_redraw();
    }

    /// Floating placement for a freshly mapped X11 window: restore saved geometry
    /// when the app has been seen before, otherwise center the client's natural
    /// size under the bar. Splash windows always keep their natural size.
    /// Near-fullscreen / borderless game sizes map flush to the output origin so
    /// they are not inset under the edge bar and clipped.
    fn place_x11_window(
        &mut self,
        id: u32,
        natural: Size<i32, Logical>,
        app_id: Option<&str>,
        is_splash: bool,
    ) {
        if is_splash {
            let w = if natural.w > 0 {
                natural.w
            } else {
                DEFAULT_FLOAT_W / 2
            };
            let h = if natural.h > 0 {
                natural.h
            } else {
                DEFAULT_FLOAT_H / 3
            };
            let rect = self.centered_body_for_window(id, w, h);
            self.windows.set_target_rect(id, rect);
            self.windows.set_placement_chosen(id, true);
            return;
        }
        // Honor the client's requested size when available — enlarging a splash /
        // dialog to DEFAULT_FLOAT makes toolbar bitmaps tile across a huge window.
        let w = if natural.w > 0 {
            natural.w
        } else {
            DEFAULT_FLOAT_W
        };
        let h = if natural.h > 0 {
            natural.h
        } else {
            DEFAULT_FLOAT_H
        };
        let undecorated = self
            .windows
            .get(id)
            .and_then(|r| r.x11())
            .map(|x11| !x11.is_decorated())
            .unwrap_or(false);
        let is_launcher = x11_is_game_launcher(app_id);
        // Launchers (Steam splash, etc.) animate size while loading — never force
        // fullscreen or special output centering or they fight Metis in a loop.
        if !is_launcher {
            if let Some(rect) = self.borderless_output_rect_for(id, w, h, undecorated) {
                tracing::info!(
                    id,
                    ?rect,
                    undecorated,
                    "x11: borderless/near-fullscreen placement at output origin"
                );
                self.windows.set_target_rect(id, rect);
                self.windows.set_placement_chosen(id, true);
                self.pending_game_fullscreen.insert(id);
                return;
            }
            if undecorated {
                if let Some(rect) = self.centered_on_output_for(id, w, h).filter(|_| {
                    self.launch_output_for(id)
                        .and_then(|o| self.space.output_geometry(&o))
                        .is_some_and(|g| x11_large_undecorated_float(g, w, h))
                }) {
                    tracing::info!(id, ?rect, "x11: large undecorated float centered on output");
                    self.windows.set_target_rect(id, rect);
                    self.windows.set_placement_chosen(id, true);
                    return;
                }
            }
        }
        if let Some(app_id) = app_id {
            if let Some(saved) = self.window_state.get(app_id) {
                let saved_rect = saved.to_rect();
                if saved_size_is_usable(saved_rect.width, saved_rect.height) {
                    // Never restore a stale near-fullscreen save into the usable
                    // zone — that recreates the clipped borderless-window bug.
                    if let Some(rect) = self.borderless_output_rect_for(
                        id,
                        saved_rect.width,
                        saved_rect.height,
                        undecorated,
                    ) {
                        self.windows.set_target_rect(id, rect);
                        self.windows.set_placement_chosen(id, true);
                        return;
                    }
                    let rect = self.restore_body_for_window(id, saved_rect);
                    self.windows.set_target_rect(id, rect);
                    self.windows.set_placement_chosen(id, true);
                    return;
                }
                self.window_state.remove(app_id);
            }
        }
        let rect = self.centered_body_for_window(id, w, h);
        self.windows.set_target_rect(id, rect);
        self.windows.set_placement_chosen(id, true);
    }

    /// When `w`×`h` looks like borderless / fake-fullscreen for `id`'s output,
    /// return that output's full geometry (flush origin). Otherwise `None`.
    pub(crate) fn borderless_output_rect_for(
        &self,
        id: u32,
        w: i32,
        h: i32,
        undecorated: bool,
    ) -> Option<PixelRect> {
        let output = self.launch_output_for(id)?;
        let geo = self.space.output_geometry(&output)?;
        if !x11_borderless_fullscreen_intent(geo, w, h, undecorated) {
            return None;
        }
        Some(PixelRect {
            x: geo.loc.x,
            y: geo.loc.y,
            width: geo.size.w,
            height: geo.size.h,
        })
    }

    /// Center `w`×`h` on the full output (not the bar usable zone). Used for
    /// large undecorated game floats so they are not inset under the edge bar.
    pub(crate) fn centered_on_output_for(&self, id: u32, w: i32, h: i32) -> Option<PixelRect> {
        let output = self.launch_output_for(id)?;
        let geo = self.space.output_geometry(&output)?;
        let width = w.clamp(1, geo.size.w);
        let height = h.clamp(1, geo.size.h);
        Some(PixelRect {
            x: geo.loc.x + (geo.size.w - width) / 2,
            y: geo.loc.y + (geo.size.h - height) / 2,
            width,
            height,
        })
    }

    /// Undecorated X11 float large enough that bar-inset clamping would recreate
    /// the off-screen borderless-game bug.
    fn x11_keep_on_full_output(&self, id: u32, rect: PixelRect) -> bool {
        let Some(record) = self.windows.get(id) else {
            return false;
        };
        let Some(x11) = record.x11() else {
            return false;
        };
        if x11.is_decorated() {
            return false;
        }
        let Some(output) = self.launch_output_for(id) else {
            return false;
        };
        let Some(geo) = self.space.output_geometry(&output) else {
            return false;
        };
        x11_large_undecorated_float(geo, rect.width, rect.height)
    }

    /// True when `rect` already covers (nearly) an entire output — skip bar inset.
    pub(crate) fn rect_is_output_covering(&self, rect: PixelRect) -> bool {
        for output in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            if x11_borderless_fullscreen_intent(geo, rect.width, rect.height, true)
                && (rect.x - geo.loc.x).abs() <= WINDOW_GAP_PX * 2
                && (rect.y - geo.loc.y).abs() <= WINDOW_GAP_PX * 2
            {
                return true;
            }
        }
        false
    }

    /// Handle a client-initiated unmap of an X11 window. This is *deferred*: we hide
    /// the (now bufferless) element immediately, but only arm a pending-withdraw
    /// timer rather than tearing the window down. Electron apps (Claude Desktop)
    /// unmap/remap their X11 window constantly during normal operation and, notably,
    /// as part of restoring from the tray — reacting to each unmap would thrash the
    /// dock and make the window flash open and vanish. `tick_x11_withdraws` promotes
    /// a still-unmapped window to a real "close to tray" after a grace period;
    /// `map_x11_toplevel` cancels the pending withdraw if the window comes back.
    pub(crate) fn unmap_x11_toplevel(&mut self, window: &X11Surface) {
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            return;
        };
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        self.space.unmap_elem(&record.window);
        self.x11_pending_withdraw
            .entry(id)
            .or_insert_with(std::time::Instant::now);
        tracing::info!(
            id,
            x11_window = window.window_id(),
            "x11: unmap (withdraw armed)"
        );
        self.schedule_redraw();
    }

    /// Grace period before a client-unmapped X11 window is treated as withdrawn to
    /// the tray. Long enough to swallow Electron's transient unmap/remap churn, short
    /// enough that a genuine close-to-tray drops from the dock promptly.
    const X11_WITHDRAW_GRACE: std::time::Duration = std::time::Duration::from_millis(600);

    /// Promote X11 windows that have stayed unmapped past the grace period to a real
    /// withdraw: drop them from the dock/tasklist (like GNOME/KDE do for tray apps)
    /// so a stale entry can't restore to an empty frame. The registry record is kept,
    /// keyed by X11 window id, so a later remap re-announces and re-shows the window.
    /// Returns true if anything changed (so the caller can flag damage).
    fn tick_x11_withdraws(&mut self) -> bool {
        if self.x11_pending_withdraw.is_empty() {
            return false;
        }
        let now = std::time::Instant::now();
        let due: Vec<u32> = self
            .x11_pending_withdraw
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= Self::X11_WITHDRAW_GRACE)
            .map(|(id, _)| *id)
            .collect();
        if due.is_empty() {
            return false;
        }
        for id in due {
            self.x11_pending_withdraw.remove(&id);
            // A window that no longer exists, or is already mapped again, needs no
            // teardown (the remap path clears the pending entry, but guard anyway).
            if self.windows.get(id).is_none() {
                continue;
            }
            tracing::info!(id, "x11: withdraw confirmed — dropping dock entry");
            self.drop_window_fullscreen(id);
            self.windows.set_ready(id, false);
            self.windows.set_fullscreen(id, false);
            self.windows.set_maximized(id, false);
            self.clear_auto_hide(id);
            if self.last_focused_window == Some(id) {
                self.last_focused_window = None;
            }
            if self.focused_window_id() == Some(id) {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(kbd) = self.seat.get_keyboard() {
                    kbd.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
                }
            }
            self.event_bus
                .emit(&metis_protocol::CompositorEvent::WindowClosed { id });
            self.emit_layout_changed();
        }
        true
    }

    /// Tear down a destroyed X11 window: drop it from the registry, close out any
    /// fullscreen bookkeeping, and notify the shell (dock/IPC) like a Wayland close.
    pub(crate) fn destroy_x11_toplevel(&mut self, window: &X11Surface) {
        let Some(id) = self.windows.id_for_x11_window(window.window_id()) else {
            return;
        };
        tracing::info!(id, x11_window = window.window_id(), "x11: window destroyed");
        self.x11_pending_withdraw.remove(&id);
        let ready = self.windows.is_ready(id);
        self.drop_window_fullscreen(id);
        self.save_window_geometry(id);
        if let Some(record) = self.windows.unregister(id) {
            self.space.unmap_elem(&record.window);
        }
        if ready {
            self.on_window_destroyed(id);
        }
        self.schedule_redraw();
    }

    /// Send the initial xdg configure as soon as the client makes its first
    /// commit, rather than waiting for a later layout/placement pass.
    ///
    /// A Wayland client cannot attach its first buffer until it has acked the
    /// initial configure, so deferring it stalls the window's first paint. With
    /// the old behavior a toplevel only got configured as a side effect of an
    /// unrelated layout pass — terminals like foot/alacritty/kitty could hang
    /// for many seconds, or forever if nothing else happened. Priming the
    /// configure here decouples client startup from Metis's layout passes.
    ///
    /// The configure carries the real placement size (saved geometry / grid
    /// tile) so the window opens at its final size instead of a placeholder.
    pub fn ensure_initial_configure(&mut self, id: u32) {
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };
        // XWayland windows are configured/placed synchronously when they map
        // (see `activate_x11_window`); they have no xdg initial-configure gate.
        let Some(toplevel) = record.wl_toplevel() else {
            return;
        };
        if toplevel.is_initial_configure_sent() {
            return;
        }
        // Make sure metadata + placement are decided before the configure goes
        // out, so the size is correct on the very first map.
        let (title, app_id) = read_toplevel_metadata(toplevel);
        self.windows.set_metadata(id, title.clone(), app_id.clone());
        self.set_app_tile_display_name(id, &title, app_id.as_deref());
        if !self.floating.contains(&id) && !self.windows.placement_chosen(id) {
            self.place_new_window(id, app_id.as_deref());
        }
        self.refresh_window_decoration_mode(id);
        self.apply_window_rect(id);
    }

    pub fn activate_window(&mut self, id: u32) {
        use metis_protocol::CompositorEvent;

        if self.capture_overlay_active() && !self.window_is_capture_overlay(id) {
            self.enforce_capture_overlay_stacking();
            return;
        }

        // Before tile reflow/reposition — `ensure_app_tile_for_window` can call
        // `reposition_all_windows`, whose stacking restore must not fall back to a
        // maximized neighbor while this window is still being mapped.
        self.note_window_focus(id);

        let key = self.desk_key_for_window(id);
        let ws = self.windows.workspace(id).unwrap_or(1);
        let kind = self.layout_kind_for(&key, ws);
        if kind == metis_grid::LayoutKind::Free {
            // Grid tiles must not drive placement while the workspace is floating.
            self.remove_app_tile_everywhere(id);
        }

        self.ensure_app_tile_for_window(id);
        let Some(record) = self.windows.get(id).cloned() else {
            return;
        };

        let (title, app_id) = self.read_window_metadata(&record);
        self.windows.set_metadata(id, title.clone(), app_id.clone());
        self.set_app_tile_display_name(id, &title, app_id.as_deref());
        self.refresh_window_decoration_mode(id);

        let screenshot_overlay = self.maybe_register_capture_overlay(id);

        let already_ready = self.windows.is_ready(id);
        // Choose placement before the first map whenever the window is not yet
        // floating (e.g. app_id arrived after the initial configure).
        if !screenshot_overlay {
            if kind == metis_grid::LayoutKind::Free
                || (!self.floating.contains(&id) && !self.windows.placement_chosen(id))
            {
                self.place_new_window(id, app_id.as_deref());
            }
            self.apply_window_rect(id);
        }

        if already_ready {
            return;
        }

        self.windows.set_ready(id, true);

        let suggested_rect = self
            .windows
            .target_rect(id)
            .or_else(|| {
                self.rect_for_window_tile(id)
                    .map(|full| self.tile_client_rect(id, full))
            })
            .unwrap_or(PixelRect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            });

        self.persist_layout();
        self.emit_layout_changed();
        self.event_bus.emit(&CompositorEvent::WindowOpened {
            id,
            title,
            app_id,
            suggested_rect,
        });

        // A freshly mapped window becomes the active one: raise it, give it
        // keyboard focus, and report the focus to the shell. Without this the
        // taskbar starts with no focused window, so the first click on a dock
        // icon only re-focuses the (already visible) app instead of minimizing
        // it, forcing a wasted first click.
        if let Some(keyboard) = self.seat.get_keyboard() {
            self.space.raise_element(&record.window, true);
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
            self.event_bus.emit(&CompositorEvent::WindowFocused { id });
        }

        // A game rule asked for fullscreen: apply it now that the window is
        // mapped, placed, and focused. Consumed so it fires exactly once.
        if self.pending_game_fullscreen.remove(&id) {
            self.set_fullscreen(id, true, None);
        }
    }

    pub(crate) fn set_app_tile_display_name(
        &mut self,
        window_id: u32,
        title: &str,
        app_id: Option<&str>,
    ) {
        let display = app_display_name(app_id, title);
        let tile_id = format!("app-{window_id}");
        let key = self.desk_key_for_window(window_id);
        if let Some(desk) = self.desks.get_mut(&key) {
            if let Some(tile) = desk.layout.tiles.iter_mut().find(|t| t.id == tile_id) {
                if let TileKind::App {
                    window_id: wid,
                    class,
                } = &mut tile.kind
                {
                    *wid = Some(window_id);
                    *class = Some(display);
                }
            }
        }
    }

    /// Handle the XWayland keyboard-focus race on surface association.
    ///
    /// An X11 toplevel (a Proton/Wine game, e.g. a Steam title) can issue its
    /// `MapRequest` — at which point `map_x11_toplevel` gives it keyboard focus —
    /// *before* its `wl_surface` is associated by XWayland. `keyboard.set_focus`
    /// therefore delivered `wl_keyboard.enter` to a window with no live surface,
    /// so keystrokes (Esc, WASD, …) never reached the game even though pointer
    /// input worked (the pointer target is resolved per-motion, so the mouse
    /// still "looks" fine). The user sees "mouse works but Esc/keys don't."
    ///
    /// When the surface finally associates (its first commit), we index it and —
    /// if this window is still the intended keyboard-focus target — re-deliver
    /// focus so XWayland gets a fresh `enter` for the now-live surface. Setting
    /// the *same* focus target is a no-op in Smithay, so we drop focus first to
    /// force the re-enter. `committed_id` must be resolved via the space-element
    /// match (not `id_for_surface`, which is exactly what is still missing here).
    pub(crate) fn note_surface_committed_for_focus(
        &mut self,
        id: u32,
        root: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        use smithay::reexports::wayland_server::Resource;
        let (x11_window, window) = match self.windows.get(id) {
            Some(record) if record.is_x11 => {
                match record.window.x11_surface().map(|x11| x11.window_id()) {
                    Some(x11_window) => (x11_window, record.window.clone()),
                    None => return,
                }
            }
            _ => return,
        };
        // Only act on the *first* association — once indexed, the normal focus
        // path already routes keys correctly and re-focusing on every commit
        // would fight the user's real focus.
        if self.windows.id_for_surface(root).is_some() {
            return;
        }
        self.windows.index_x11_surface(x11_window, root.id());
        if self.focused_window_id() == Some(id) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.set_focus(self, None, serial);
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.set_focus(self, Some(window.into()), serial);
                tracing::info!(
                    id,
                    x11_window,
                    "focus: re-asserted keyboard focus after XWayland surface associated"
                );
            }
        }
    }

    pub fn try_activate_committed_window(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let Some(id) = self.windows.id_for_surface(surface) else {
            return;
        };
        if self.windows.is_ready(id) {
            return;
        }
        // Must read the *renderer* surface state, not `SurfaceAttributes.buffer`:
        // `on_commit_buffer_handler` (run at the top of the commit handler) consumes
        // the attribute buffer, so `SurfaceAttributes.current().buffer` is `None`
        // except on the exact frame a buffer was attached. That made activation
        // (and therefore `WindowOpened` + the `ready` flag) effectively never fire.
        let has_buffer =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
                state.buffer().is_some()
            })
            .unwrap_or(false);
        if has_buffer {
            self.activate_window(id);
        }
    }

    pub fn set_tile_mode(&mut self, tile_id: &str, mode: metis_protocol::TileMode) {
        use metis_protocol::TileMode;

        let key = self
            .desk_key_for_tile(tile_id)
            .unwrap_or_else(|| self.primary_key());
        let window_id = self.desk(&key).and_then(|d| {
            d.layout.tiles.iter().find_map(|t| {
                if t.id != tile_id {
                    return None;
                }
                if let TileKind::App {
                    window_id: Some(wid),
                    ..
                } = &t.kind
                {
                    Some(*wid)
                } else {
                    None
                }
            })
        });

        match mode {
            TileMode::Grid => {
                let layout_restored = self.tile_modes.exit(tile_id);
                if let Some(restored) = layout_restored {
                    if let Some(desk) = self.desks.get_mut(&key) {
                        if let Some(tile) = desk.layout.tile_mut(tile_id) {
                            tile.rect = restored;
                        }
                    }
                }
                if let Some(id) = window_id {
                    if self.windows.is_minimized(id) {
                        self.unminimize_window(id);
                    }
                    self.set_fullscreen(id, false, None);
                    self.set_maximized(id, false);
                }
                if layout_restored.is_some() {
                    self.reposition_all_windows();
                    self.persist_layout();
                    self.emit_layout_changed();
                }
            }
            TileMode::AppFullscreen => {
                if let Some(layout) = self.desk(&key).map(|d| d.layout.clone()) {
                    self.tile_modes
                        .enter(&layout, tile_id, metis_grid::TileMode::AppFullscreen);
                }
                if let Some(id) = window_id {
                    self.set_maximized(id, true);
                }
            }
            TileMode::Minimized => {
                if let Some(layout) = self.desk(&key).map(|d| d.layout.clone()) {
                    self.tile_modes
                        .enter(&layout, tile_id, metis_grid::TileMode::Minimized);
                }
                if let Some(id) = window_id {
                    self.minimize_window(id);
                }
            }
            TileMode::Immersive => {
                if let Some(layout) = self.desk(&key).map(|d| d.layout.clone()) {
                    self.tile_modes
                        .enter(&layout, tile_id, metis_grid::TileMode::Immersive);
                }
                tracing::info!(tile_id, "immersive mode requested (shell handles chrome)");
            }
        }
    }

    pub fn on_window_destroyed(&mut self, id: u32) {
        use metis_protocol::CompositorEvent;

        self.drop_window_fullscreen(id);
        if self.last_focused_window == Some(id) {
            self.last_focused_window = None;
        }
        self.unregister_capture_overlay(id);
        self.save_window_geometry(id);
        self.floating.remove(&id);
        self.pending_game_fullscreen.remove(&id);
        self.clear_auto_hide(id);
        let desk_key = self.desk_key_for_window(id);
        self.remove_app_tile_everywhere(id);
        self.auto_reflow_grid_apps(&desk_key, self.focused_window_id(), false);
        // Grid reflow above is a no-op on scroll workspaces; re-snap the offset and
        // slide the surviving columns over to close the gap the closed window left.
        self.refresh_all_scroll_offsets();
        self.reposition_scroll_windows();
        self.persist_layout();
        self.event_bus.emit(&CompositorEvent::WindowClosed { id });
    }

    pub fn cleanup_destroyed_windows(&mut self) {
        // Only drop registry entries whose Wayland resources are actually gone.
        // Unmapped windows (minimized, pending first commit) remain alive and must
        // not be treated as destroyed just because they are absent from the space.
        let stale: Vec<u32> = self
            .windows
            .ids()
            .into_iter()
            .filter(|id| {
                self.windows
                    .get(*id)
                    .is_some_and(|record| !record.window.alive())
            })
            .collect();

        for id in stale {
            // Remember floating app geometry before the record is dropped.
            self.save_window_geometry(id);
            if let Some(record) = self.windows.unregister(id) {
                self.space.unmap_elem(&record.window);
            }
            self.on_window_destroyed(id);
        }
    }
}

fn default_app_tile_rect(layout: &GridLayout) -> metis_grid::TileRect {
    let rows = layout.rows.max(8);
    let cols = layout.columns.max(12);
    // Open new apps as a large, centered tile rather than a small bottom-left
    // cell, so a freshly launched window is immediately usable.
    let w = (cols * 2 / 3).clamp(4, cols);
    let h = (rows * 2 / 3).clamp(3, rows);
    let col = (cols - w) / 2;
    let row = (rows - h) / 2;
    metis_grid::TileRect::new(col, row, w, h)
}

/// When re-anchoring an oversized client, keep this placement-zone edge flush
/// with the footprint (overflow spills toward the interior).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarEdgeAnchor {
    Min,
    Max,
}

/// Pick a window origin on one axis that preserves the footprint's screen-edge
/// gap when the client is larger than the footprint. Honors the footprint origin
/// when the client fits; otherwise anchors to whichever screen edge the footprint
/// hugs (so the overflow grows toward the opposite, interior side).
#[allow(clippy::too_many_arguments)]
fn anchor_axis(
    foot_min: i32,
    foot_size: i32,
    zone_min: i32,
    zone_size: i32,
    actual: i32,
    gap_min: i32,
    gap_max: i32,
    bar_edge: Option<BarEdgeAnchor>,
) -> i32 {
    if actual <= foot_size {
        return foot_min;
    }
    let foot_max = foot_min + foot_size;
    let zone_max = zone_min + zone_size;
    let touches_min = foot_min - zone_min <= gap_min;
    let touches_max = zone_max - foot_max <= gap_max;

    // Maximize hugs both zone edges — keep the bar-adjacent side fixed so any
    // overflow spills away from the edge bar instead of underneath it.
    if touches_min && touches_max {
        return match bar_edge {
            Some(BarEdgeAnchor::Max) => foot_max - actual,
            Some(BarEdgeAnchor::Min) | None => foot_min,
        };
    }
    if touches_max && !touches_min {
        foot_max - actual
    } else {
        foot_min
    }
}

/// Apply maximize-consistent edge gaps to a raw snap region. Boundary sides use
/// `gaps`; interior split lines get half the configured window gap.
fn snap_client_rect(raw: PixelRect, zone: PixelRect, gaps: ZoneGaps) -> PixelRect {
    let half = (gaps.left.max(gaps.right).max(gaps.top).max(gaps.bottom) / 2).max(0);
    let touches_left = raw.x <= zone.x;
    let touches_right = raw.x + raw.width >= zone.x + zone.width;
    let touches_top = raw.y <= zone.y;
    let touches_bottom = raw.y + raw.height >= zone.y + zone.height;

    let l = if touches_left { gaps.left } else { half };
    let r = if touches_right { gaps.right } else { half };
    let t = if touches_top { gaps.top } else { half };
    let b = if touches_bottom { gaps.bottom } else { half };

    PixelRect {
        x: raw.x + l,
        y: raw.y + t,
        width: (raw.width - l - r).max(1),
        height: (raw.height - t - b).max(1),
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        // A protocol error means a client (e.g. the shell's gtk4-layer-shell)
        // sent something invalid and was force-disconnected; surface the exact
        // object/code/message so these are diagnosable instead of silent.
        match reason {
            DisconnectReason::ProtocolError(err) => tracing::error!(
                ?client_id,
                object = %err.object_interface,
                code = err.code,
                message = %err.message,
                "client disconnected: protocol error"
            ),
            other => tracing::info!(?client_id, ?other, "client disconnected"),
        }
    }
}

pub fn desk_config_path() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "metis", "metis")
        .map(|dirs| dirs.config_dir().join("desk.json"))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/metis/desk.json"))
                .unwrap_or_else(|_| std::path::PathBuf::from(".config/metis/desk.json"))
        })
}

pub(crate) fn read_toplevel_metadata(
    surface: &smithay::wayland::shell::xdg::ToplevelSurface,
) -> (String, Option<String>) {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

    with_states(surface.wl_surface(), |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else {
            return ("Application".into(), None);
        };
        let Ok(role) = data.lock() else {
            return ("Application".into(), None);
        };
        (
            role.title
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Application".into()),
            role.app_id.clone().filter(|id| !id.is_empty()),
        )
    })
}

pub(crate) fn read_toplevel_decoration_mode(
    surface: &smithay::wayland::shell::xdg::ToplevelSurface,
) -> Option<smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode>{
    surface.with_committed_state(|state| state.and_then(|s| s.decoration_mode))
}

fn app_display_name(app_id: Option<&str>, title: &str) -> String {
    if let Some(id) = app_id.filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    let trimmed = title.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("application") {
        return "App".into();
    }
    trimmed.to_string()
}
