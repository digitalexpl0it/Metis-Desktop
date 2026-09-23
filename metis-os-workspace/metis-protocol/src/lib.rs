//! Shared JSON IPC contracts and runtime helpers between compositor and shell.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod rate_limit;

pub use metis_grid::{GridLayout, GridMetrics, LayoutKind, MonitorRect, PixelRect};
pub use rate_limit::{
    EVENT_SUBSCRIBE_ACCEPTS_PER_SEC, EVENT_SUBSCRIBER_CAP, IPC_MAX_ACCEPTS_PER_DRAIN,
    IPC_REQUESTS_PER_SEC, RATE_WINDOW, RUNTIME_CMD_DISPATCH_PER_SEC, RUNTIME_CMD_WRITES_PER_SEC,
    SlidingWindow, try_admit_runtime_command_dispatch, try_admit_runtime_command_widgets_dispatch,
    try_admit_runtime_command_widgets_write, try_admit_runtime_command_write,
};

/// Commands sent from the Metis shell to the compositor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CompositorCommand {
    Ping,
    GetMonitor,
    /// List the connected outputs (name + geometry), so the settings app can offer
    /// per-display options (e.g. per-output wallpaper).
    ListOutputs,
    /// List DRM video modes for one output (resolution + refresh). Returns the
    /// current mode and every mode the connector advertises.
    ListOutputModes {
        output: String,
    },
    GetLayout,
    ListWindows,
    /// Render per-window thumbnails into `$XDG_RUNTIME_DIR/metis/thumbs/{id}.png`
    /// on the next GL frame (queued). Reply lists paths already on disk; the
    /// shell may briefly wait for missing files after the redraw.
    CaptureWindowThumbs {
        ids: Vec<u32>,
    },
    /// Render live mini-desktop PNGs for Task View shelf tiles:
    /// `$XDG_RUNTIME_DIR/metis/thumbs/ws-{output}-{id}.png` (wallpaper +
    /// non-minimized windows on that workspace). Queued for the next GL frame.
    CaptureWorkspaceThumbs {
        output: String,
        workspaces: Vec<u32>,
    },
    MoveWindow {
        id: u32,
        rect: PixelRect,
    },
    CloseWindow {
        id: u32,
    },
    FocusWindow {
        id: u32,
    },
    /// Minimize or restore a window by id (works for grid and floating windows).
    SetMinimized {
        id: u32,
        minimized: bool,
    },
    /// Bring a window to the foreground: unminimize (if needed), raise, and focus.
    /// Used by the taskbar to surface a background/minimized app.
    ActivateWindow {
        id: u32,
    },
    SetFullscreen {
        id: u32,
        enabled: bool,
    },
    ApplyLayout {
        layout: GridLayout,
        gutter_px: u32,
    },
    SetTileMode {
        tile_id: String,
        mode: TileMode,
    },
    /// Switch the active virtual workspace (1-based) on a specific output. Out-of-range
    /// ids are clamped to the configured workspace count. `output` is an output name
    /// (as reported by `ListOutputs`); `None`/empty targets the output under the
    /// pointer. Each output owns an independent set of workspaces.
    SwitchWorkspace {
        #[serde(default)]
        output: Option<String>,
        id: u32,
    },
    /// Move a window to another virtual workspace (1-based). If the target is not
    /// the active workspace the window is hidden until that workspace is shown.
    MoveWindowToWorkspace {
        window_id: u32,
        workspace: u32,
    },
    /// Move a window to another output (monitor). Keeps its workspace number on
    /// the destination output. `output` is an output name from `ListOutputs`;
    /// `None`/empty targets the output under the pointer.
    MoveWindowToOutput {
        window_id: u32,
        #[serde(default)]
        output: Option<String>,
    },
    /// Move every window on a workspace to another output (same workspace number).
    /// `output`/`workspace` default to the output under the pointer and its active
    /// workspace. Requires independent per-output workspace mode.
    MoveWorkspaceToOutput {
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        workspace: Option<u32>,
        target_output: String,
    },
    /// Set the layout mode (grid vs. scrolling) of a workspace. `output` is an
    /// output name (`None`/empty targets the output under the pointer); `workspace`
    /// `None` targets that output's currently-active workspace.
    SetWorkspaceLayout {
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        workspace: Option<u32>,
        kind: LayoutKind,
    },
    /// Apply a layout mode to every workspace on every output at once (used when
    /// the settings "New workspace layout" default changes, so it acts as a live
    /// global on/off rather than only seeding future workspaces).
    SetDefaultLayout {
        kind: LayoutKind,
    },
    /// Spawn a client. Prefer `argv` (no shell). If `argv` is empty, `program` is
    /// split with [`split_command_line`] — never passed to `sh -c`.
    Launch {
        #[serde(default)]
        argv: Vec<String>,
        #[serde(default)]
        program: String,
    },
    /// End the Metis session: stop the compositor event loop so the session host
    /// (run script / display manager) tears the session down cleanly. Used by the
    /// app menu's "Log Out" action.
    EndSession,
    /// Re-read `wallpaper.json` and apply the desktop background live (picture,
    /// solid colour, or gradient).
    ApplyBackground,
    /// Re-read `input.json` and apply mouse/touchpad/keyboard settings live.
    ReloadInput,
    /// Re-read `keybinds.json` and apply desktop shortcut bindings live.
    ReloadKeybinds,
    /// While Settings is capturing a new shortcut, suppress global keybind dispatch
    /// so Super+L etc. do not fire mid-edit.
    SetKeybindCapture {
        active: bool,
    },
    /// Re-read `outputs.json` and apply per-output scale (and related prefs) live.
    ReloadOutputs,
    /// Re-read `power.json` and apply idle preferences live (currently the screen
    /// blank timeout that drives the compositor's idle blanker).
    ReloadPower,
    /// Lock the session now: the compositor enters its locked mode (renders the
    /// lock screen, captures all input, hides clients) until the user
    /// authenticates.
    LockSession,
    /// Re-read `lock.json` and re-decode the lock-screen background live.
    ReloadLock,
    /// Re-read `gaming.json` and apply graphics/offload preferences live.
    ReloadGaming,
    /// Re-read `decorations.json` and re-apply per-app SSD/CSD overrides live.
    ReloadDecorations,
    /// Re-read `locale.json` and reload Fluent compositor UI strings.
    ReloadLocale,
    SubscribeEvents,
    /// Set the Wayland clipboard from the shell (text or image file on disk).
    SetClipboard {
        mime: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        image_path: Option<String>,
    },
    /// Suppress idle blanking/suspend while an external client (a D-Bus
    /// `org.freedesktop.ScreenSaver` / portal `Inhibit` caller such as a video
    /// player or game) holds an inhibitor. `cookie` is the opaque handle the
    /// inhibit service handed back to the caller; the same cookie releases it via
    /// [`CompositorCommand::UninhibitIdle`]. Wayland `zwp_idle_inhibit` surfaces
    /// are tracked separately inside the compositor.
    InhibitIdle {
        cookie: u32,
        #[serde(default)]
        app_name: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Release an idle inhibitor previously taken via
    /// [`CompositorCommand::InhibitIdle`]. Unknown cookies are ignored.
    UninhibitIdle {
        cookie: u32,
    },
    /// Elevate capture UI from a portal screenshot/screencast request.
    BeginCaptureOverlay {
        #[serde(default)]
        app_id: Option<String>,
    },
    /// Tear down portal-driven capture overlay elevation.
    EndCaptureOverlay {
        #[serde(default)]
        app_id: Option<String>,
    },
    /// Native Metis screenshot UI is active (shell layer namespace `metis-screenshot`).
    BeginScreenshotOverlay,
    /// Tear down native screenshot overlay tracking.
    EndScreenshotOverlay,
    /// Inject remote-desktop pointer motion (absolute desktop coordinates).
    InjectRemotePointerAbsolute {
        x: f64,
        y: f64,
    },
    /// Inject remote-desktop pointer motion (relative delta in logical pixels).
    InjectRemotePointerRelative {
        dx: f64,
        dy: f64,
    },
    /// Inject remote-desktop pointer button (Linux evdev button code).
    InjectRemotePointerButton {
        button: u32,
        pressed: bool,
    },
    /// Inject remote-desktop scroll delta (logical pixels).
    InjectRemotePointerScroll {
        dx: f64,
        dy: f64,
    },
    /// Inject remote-desktop keyboard key (evdev keycode, 8 = ESC).
    InjectRemoteKey {
        keycode: u32,
        pressed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileMode {
    Grid,
    Immersive,
    AppFullscreen,
    Minimized,
}

/// Events emitted by the compositor to the shell.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum CompositorEvent {
    Pong,
    Monitor {
        rect: MonitorRect,
    },
    /// Reply to `ListOutputs`: every connected output, primary first.
    OutputList {
        outputs: Vec<OutputInfo>,
    },
    /// Reply to `ListOutputModes`: advertised modes for one output.
    OutputModes {
        modes: Vec<OutputModeInfo>,
        current: Option<OutputModeInfo>,
    },
    LayoutChanged {
        layout: GridLayout,
        gutter_px: u32,
        metrics: GridMetrics,
    },
    WindowList {
        windows: Vec<WindowInfo>,
    },
    /// Reply to `CaptureWindowThumbs`: paths that already exist (may be stale
    /// until the queued GL capture finishes).
    WindowThumbs {
        thumbs: Vec<WindowThumb>,
    },
    /// Reply to `CaptureWorkspaceThumbs`: paths already on disk (may be stale
    /// until the queued GL capture finishes).
    WorkspaceThumbs {
        thumbs: Vec<WorkspaceThumb>,
    },
    WindowOpened {
        id: u32,
        title: String,
        app_id: Option<String>,
        suggested_rect: PixelRect,
    },
    WindowClosed {
        id: u32,
    },
    WindowFocused {
        id: u32,
    },
    WindowMinimized {
        id: u32,
        minimized: bool,
    },
    /// True fullscreen on `output` — shell hides the edge bar until `visible` is true.
    EdgeBarVisible {
        output: String,
        visible: bool,
    },
    WindowFullscreen {
        id: u32,
        fullscreen: bool,
        #[serde(default)]
        output: String,
    },
    WindowMetadata {
        id: u32,
        title: String,
        app_id: Option<String>,
    },
    LayoutApplied,
    MonitorChanged {
        rect: MonitorRect,
    },
    /// The active virtual workspace changed (1-based) on `output`, with the current
    /// total count. Each output reports its own active workspace independently.
    WorkspaceChanged {
        #[serde(default)]
        output: String,
        active: u32,
        count: u32,
    },
    Error {
        message: String,
    },
    /// Clipboard contents changed (text preview and/or image path under runtime dir).
    ClipboardChanged {
        mime: String,
        #[serde(default)]
        preview_text: Option<String>,
        #[serde(default)]
        image_path: Option<String>,
    },
    /// Game or launcher session started/ended (Phase 11 gaming daemon).
    GameSession {
        active: bool,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        pid: Option<u32>,
    },
    /// A physical display was plugged in or unplugged (DRM hotplug).
    OutputHotplug {
        connected: bool,
        name: String,
        #[serde(default)]
        make: String,
        #[serde(default)]
        model: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app_id: Option<String>,
    pub rect: PixelRect,
    pub fullscreen: bool,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub focused: bool,
    /// Name of the output (monitor) the window is currently on (e.g. `metis-0`).
    /// Empty when not yet known (an event-folded entry before the next reconcile).
    #[serde(default)]
    pub output: String,
    /// Virtual workspace the window belongs to (1-based).
    #[serde(default)]
    pub workspace: u32,
}

/// One cached window thumbnail on disk (PNG).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowThumb {
    pub id: u32,
    pub path: String,
}

/// One cached workspace mini-desktop thumbnail on disk (PNG).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceThumb {
    pub workspace: u32,
    pub path: String,
}

/// A video mode (resolution + refresh) for one output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutputModeInfo {
    pub width: i32,
    pub height: i32,
    /// Refresh rate in millihertz (60_000 = 60 Hz), matching Smithay `output::Mode`.
    pub refresh_millihz: i32,
    #[serde(default)]
    pub preferred: bool,
}

/// A connected output, as reported to the settings app for per-display options.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputInfo {
    /// Compositor output name (e.g. `metis-0`). This is the key used in
    /// `wallpaper.json`'s `per_output` map and `outputs.json`.
    pub name: String,
    /// Whether this is the primary (first) output.
    #[serde(default)]
    pub primary: bool,
    /// Output position and size in global logical pixels.
    pub rect: MonitorRect,
    /// Current fractional scale (1.0 = 100%).
    #[serde(default = "default_output_scale")]
    pub scale: f64,
    /// Whether this output is currently enabled (mapped and visible to clients).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// EDID make when known (may be empty under nested winit).
    #[serde(default)]
    pub make: String,
    /// EDID model when known.
    #[serde(default)]
    pub model: String,
    /// This output clones another when mirror mode is active.
    #[serde(default)]
    pub mirrored: bool,
    /// This output is the mirror source (duplicate mode).
    #[serde(default)]
    pub mirror_source: bool,
    /// DRM driver advertises VRR / adaptive sync on this connector.
    #[serde(default)]
    pub vrr_available: bool,
    /// VRR is currently active on the CRTC (may differ from saved pref until apply).
    #[serde(default)]
    pub vrr_active: bool,
    /// DRM connector exposes HDR signaling (`Colorspace` BT.2020 and/or
    /// `HDR_OUTPUT_METADATA`).
    #[serde(default)]
    pub hdr_available: bool,
    /// HDR output signaling is currently applied on this connector.
    #[serde(default)]
    pub hdr_active: bool,
}

fn default_output_scale() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

pub fn ipc_socket_path() -> std::path::PathBuf {
    runtime_dir().join("compositor.sock")
}

pub fn events_socket_path() -> std::path::PathBuf {
    runtime_dir().join("compositor-events.sock")
}

pub fn runtime_command_path() -> std::path::PathBuf {
    runtime_dir().join("command")
}

/// Flag file: present while the edge bar is auto-hidden (visual slide).
/// Written by the shell; read by the compositor for hot-edge reveal.
pub fn bar_auto_hidden_path() -> std::path::PathBuf {
    runtime_dir().join("bar-auto-hidden")
}

pub fn bar_auto_hidden_flag() -> bool {
    bar_auto_hidden_path().is_file()
}

pub fn set_bar_auto_hidden_flag(hidden: bool) -> std::io::Result<()> {
    let path = bar_auto_hidden_path();
    if hidden {
        let _ = ensure_runtime_dir()?;
        write_private_file(&path, "1\n")
    } else if path.exists() {
        std::fs::remove_file(&path)
    } else {
        Ok(())
    }
}

/// Runtime command file for the desktop-widgets shell process (isolated from the
/// edge bar so Settings reloads do not race the bar poller).
pub fn runtime_command_path_widgets() -> std::path::PathBuf {
    runtime_dir().join("command-widgets")
}

/// `$XDG_RUNTIME_DIR/metis`. Refuses the old `/tmp/metis` fallback — if
/// `XDG_RUNTIME_DIR` is unset, returns a non-writable sentinel so binds/connects
/// fail closed instead of creating a world-accessible control plane.
pub fn runtime_dir() -> std::path::PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => std::path::PathBuf::from(dir).join("metis"),
        None => {
            eprintln!("metis: XDG_RUNTIME_DIR is unset; refusing insecure /tmp/metis fallback");
            std::path::PathBuf::from("/var/empty/metis-no-xdg-runtime-dir")
        }
    }
}

/// Create `$XDG_RUNTIME_DIR/metis` with mode `0700`. Errors if `XDG_RUNTIME_DIR`
/// is unset.
pub fn ensure_runtime_dir() -> std::io::Result<std::path::PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR is unset (refusing /tmp/metis fallback)",
        )
    })?;
    let dir = std::path::PathBuf::from(base).join("metis");
    std::fs::create_dir_all(&dir)?;
    set_mode(&dir, 0o700)?;
    Ok(dir)
}

/// Set Unix permission bits on `path` (no-op semantics on non-Unix).
pub fn set_mode(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

/// Atomically write `contents` to `path` with mode `0600`, creating parent dirs
/// as `0700` when they are the Metis runtime dir.
pub fn write_private_file(
    path: &std::path::Path,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if parent.ends_with("metis") {
            let _ = ensure_runtime_dir();
        } else {
            std::fs::create_dir_all(parent)?;
        }
    }
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_ref())?;
    file.sync_all()?;
    set_mode(path, 0o600)?;
    Ok(())
}

/// Maximum length of a runtime command line (verb + args), excluding newline.
pub const MAX_RUNTIME_COMMAND_LEN: usize = 512;

/// Verbs accepted on `$XDG_RUNTIME_DIR/metis/command` (edge bar).
pub const BAR_RUNTIME_VERBS: &[&str] = &[
    "close-popovers",
    "toggle-menu",
    "hw",
    "dismiss-screenshot",
    "reload-bar",
    "reveal-edge-bar",
    "bar-edge-hover",
    "bar-edge-leave",
    "reload-dashboard",
    "reload-desktop-widgets",
    "screenshot",
    "reload-theme",
    "reload-graphics-profile",
    "reload-weather",
    "reload-calendars",
    "reload-gaming",
    "reload-locale",
    "optimize-gaming",
    "show-onboarding",
    "show-updater",
    "settings",
    "window-switcher-next",
    "window-switcher-prev",
    "window-switcher-activate",
    "dismiss-window-switcher",
    "workspace-overview",
    "dismiss-workspace-overview",
    "task-view-next",
    "task-view-prev",
    "task-view-activate",
    "dismiss-task-view",
];

/// Verbs accepted on `$XDG_RUNTIME_DIR/metis/command-widgets`.
pub const WIDGETS_RUNTIME_VERBS: &[&str] = &[
    "reload-desktop-widgets",
    "reload-theme",
    "reload-locale",
    "reload-weather",
];

/// Parsed runtime command file line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCommand<'a> {
    pub verb: &'a str,
    pub arg: &'a str,
}

/// Parse and validate a bar/widgets runtime command line.
pub fn parse_runtime_command<'a>(
    line: &'a str,
    allowlist: &[&str],
) -> Result<RuntimeCommand<'a>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty runtime command".into());
    }
    if trimmed.len() > MAX_RUNTIME_COMMAND_LEN {
        return Err(format!(
            "runtime command exceeds {MAX_RUNTIME_COMMAND_LEN} bytes"
        ));
    }
    if trimmed.bytes().any(|b| b == 0) {
        return Err("runtime command contains NUL".into());
    }
    let (verb, arg) = trimmed
        .split_once(char::is_whitespace)
        .map(|(v, a)| (v, a.trim()))
        .unwrap_or((trimmed, ""));
    if !allowlist.contains(&verb) {
        return Err(format!("unknown runtime command verb: {verb}"));
    }
    Ok(RuntimeCommand { verb, arg })
}

pub fn write_runtime_command(action: &str) -> std::io::Result<()> {
    parse_runtime_command(action, BAR_RUNTIME_VERBS)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if !try_admit_runtime_command_write() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "runtime command write rate limited",
        ));
    }
    write_private_file(&runtime_command_path(), format!("{action}\n"))
}

/// Write a one-shot command for the desktop-widgets process.
pub fn write_runtime_command_widgets(action: &str) -> std::io::Result<()> {
    parse_runtime_command(action, WIDGETS_RUNTIME_VERBS)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if !try_admit_runtime_command_widgets_write() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "runtime command-widgets write rate limited",
        ));
    }
    write_private_file(&runtime_command_path_widgets(), format!("{action}\n"))
}

/// True when the Unix peer's UID matches this process's effective UID.
#[cfg(unix)]
pub fn peer_uid_is_euid(stream: &std::os::unix::net::UnixStream) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.uid == unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
pub fn peer_uid_is_euid(_stream: &std::os::unix::net::UnixStream) -> std::io::Result<bool> {
    Ok(true)
}

/// Outcome of [`accept_same_euid`].
#[derive(Debug)]
pub enum AcceptPeer {
    /// Peer UID matches this process's euid.
    Ready(std::os::unix::net::UnixStream),
    /// Non-blocking listener has no pending connection.
    WouldBlock,
    /// Connection accepted then dropped (foreign UID or peercred failure).
    Rejected,
}

/// Accept one connection from `listener`, enforcing same-euid via `SO_PEERCRED`.
///
/// Prefer this over raw `listener.accept()` so IPC listeners cannot forget the
/// UID gate (Phase 15 / local trust model). Callers should loop until
/// [`AcceptPeer::WouldBlock`], and log [`AcceptPeer::Rejected`] as needed.
pub fn accept_same_euid(
    listener: &std::os::unix::net::UnixListener,
) -> std::io::Result<AcceptPeer> {
    match listener.accept() {
        Ok((stream, _)) => match peer_uid_is_euid(&stream) {
            Ok(true) => Ok(AcceptPeer::Ready(stream)),
            Ok(false) | Err(_) => Ok(AcceptPeer::Rejected),
        },
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(AcceptPeer::WouldBlock),
        Err(e) => Err(e),
    }
}

/// Split a command line into argv without invoking a shell.
///
/// Supports simple single/double quotes. Does **not** expand `$VAR`, backticks,
/// or globs — callers that need path resolution must do it in Rust.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if in_double => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    argv.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        argv.push(cur);
    }
    argv
}

/// Resolve [`CompositorCommand::Launch`] into argv (prefer explicit `argv`).
pub fn launch_argv(argv: &[String], program: &str) -> Vec<String> {
    if !argv.is_empty() {
        argv.to_vec()
    } else if program.trim().is_empty() {
        Vec::new()
    } else {
        split_command_line(program)
    }
}

/// Send one JSON command to the compositor IPC socket and read the reply line.
pub fn send_compositor_command(cmd: &CompositorCommand) -> std::io::Result<CompositorEvent> {
    use std::io::{BufRead, BufReader, Write};

    let path = ipc_socket_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("Metis compositor not running at {}: {e}", path.display()),
        )
    })?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(400)))?;
    let payload = serde_json::to_string(cmd).map_err(std::io::Error::other)?;
    writeln!(stream, "{payload}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let line = response.trim();
    if line.is_empty() {
        return Err(std::io::Error::other("empty compositor response"));
    }
    serde_json::from_str(line).map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::{Mutex, OnceLock};

    /// Serialize env mutations across tests in this crate.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn runtime_dir_uses_xdg_runtime_dir() {
        let _guard = env_lock();
        let tmp = tempfile_dir("metis-protocol-runtime");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };
        assert_eq!(runtime_dir(), tmp.join("metis"));
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        assert_eq!(
            runtime_dir(),
            std::path::PathBuf::from("/var/empty/metis-no-xdg-runtime-dir")
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ensure_runtime_dir_sets_0700_and_fails_closed() {
        let _guard = env_lock();
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        assert!(ensure_runtime_dir().is_err());

        let tmp = tempfile_dir("metis-protocol-ensure");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };
        let dir = ensure_runtime_dir().expect("ensure");
        assert!(dir.ends_with("metis"));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_private_file_is_0600() {
        let _guard = env_lock();
        let tmp = tempfile_dir("metis-protocol-private");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };
        let path = runtime_dir().join("probe.txt");
        write_private_file(&path, b"secret\n").expect("write");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret\n");
        write_private_file(&path, b"overwrite\n").expect("overwrite");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "overwrite\n");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn split_command_line_handles_quotes_without_shell() {
        assert!(split_command_line("").is_empty());
        assert!(split_command_line("   ").is_empty());
        assert_eq!(
            split_command_line("kitty --class foo"),
            vec!["kitty", "--class", "foo"]
        );
        assert_eq!(
            split_command_line(r#"foo "bar baz" 'qux quux'"#),
            vec!["foo", "bar baz", "qux quux"]
        );
        assert_eq!(split_command_line(r#"echo "a\"b""#), vec!["echo", r#"a"b"#]);
        // No shell expansion — `$HOME` stays literal.
        assert_eq!(split_command_line("echo $HOME"), vec!["echo", "$HOME"]);
    }

    #[test]
    fn launch_argv_prefers_explicit_argv() {
        assert_eq!(
            launch_argv(&["a".into(), "b".into()], "ignored args"),
            vec!["a", "b"]
        );
        assert!(launch_argv(&[], "").is_empty());
        assert_eq!(launch_argv(&[], "foo bar"), vec!["foo", "bar"]);
    }

    #[test]
    fn runtime_command_allowlist_and_limits() {
        assert!(parse_runtime_command("close-popovers", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("window-switcher-next", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("workspace-overview", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("hw volume-up", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("show-onboarding", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("show-updater", BAR_RUNTIME_VERBS).is_ok());
        assert!(parse_runtime_command("rm -rf /", BAR_RUNTIME_VERBS).is_err());
        assert!(parse_runtime_command("EndSession", BAR_RUNTIME_VERBS).is_err());
        assert!(
            parse_runtime_command(&"a".repeat(MAX_RUNTIME_COMMAND_LEN + 1), BAR_RUNTIME_VERBS)
                .is_err()
        );
        assert_eq!(
            parse_runtime_command("reload-theme", WIDGETS_RUNTIME_VERBS)
                .unwrap()
                .verb,
            "reload-theme"
        );
        assert!(parse_runtime_command("toggle-menu", WIDGETS_RUNTIME_VERBS).is_err());
        assert!(write_runtime_command("not-a-verb").is_err());
    }

    #[test]
    fn accept_same_euid_accepts_local_peer() {
        let dir = tempfile_dir("metis-protocol-sock");
        let sock = dir.join("t.sock");
        let listener = UnixListener::bind(&sock).expect("bind");
        listener.set_nonblocking(true).unwrap();
        let _client = UnixStream::connect(&sock).expect("connect");
        match accept_same_euid(&listener).expect("accept") {
            AcceptPeer::Ready(_) => {}
            other => panic!("expected Ready, got {other:?}"),
        }
        match accept_same_euid(&listener).expect("wouldblock") {
            AcceptPeer::WouldBlock => {}
            other => panic!("expected WouldBlock, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(prefix: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
