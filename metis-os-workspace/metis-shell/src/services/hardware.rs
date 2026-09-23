//! Multimedia / hardware-key backend: volume, microphone, display + keyboard
//! backlight, and MPRIS media transport.
//!
//! The compositor intercepts the `XF86*` keysyms and forwards an intent over the
//! runtime-command file (`hw <action>`); the shell resolves it here and flashes
//! the on-screen level overlay ([`crate::ui::osd`]).
//!
//! All D-Bus and subprocess work runs on a dedicated worker thread with its own
//! Tokio runtime so a slow PipeWire/logind round-trip never stalls the GTK main
//! loop. Results are marshalled back to the main thread with
//! [`glib::idle_add_once`], which runs the closure on the default main context.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

/// Percentage step applied per volume / brightness key press.
const VOLUME_STEP: i32 = 5;
const BRIGHTNESS_STEP: i32 = 5;
const KBD_BACKLIGHT_STEP: i32 = 1;
/// Seek offset (milliseconds) for the rewind / fast-forward media keys.
const SEEK_MS: i64 = 10_000;

#[derive(Debug, Clone, Copy)]
enum MediaAction {
    PlayPause,
    Stop,
    Next,
    Prev,
    SeekMs(i64),
}

#[derive(Debug, Clone, Copy)]
enum HwCommand {
    VolumeStep(i32),
    VolumeMuteToggle,
    MicMuteToggle,
    BrightnessStep(i32),
    KbdBacklightStep(i32),
    KbdBacklightToggle,
    Media(MediaAction),
}

static TX: OnceLock<UnboundedSender<HwCommand>> = OnceLock::new();

/// Spawn the hardware worker thread once. Safe to call repeatedly.
pub fn init() {
    let _ = TX.get_or_init(|| {
        let (tx, mut rx) = unbounded_channel::<HwCommand>();
        std::thread::Builder::new()
            .name("metis-hw".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        tracing::error!(%err, "hardware worker: tokio runtime build failed");
                        return;
                    }
                };
                rt.block_on(async move {
                    let mut buses = Buses::default();
                    while let Some(cmd) = rx.recv().await {
                        handle(&mut buses, cmd).await;
                    }
                });
            })
            .map_err(|err| tracing::error!(%err, "hardware worker: thread spawn failed"))
            .ok();
        tx
    });
}

/// Resolve a `hw <action>` runtime command. Returns `true` when recognized.
pub fn dispatch(action: &str) -> bool {
    let cmd = match action {
        "volume-up" => HwCommand::VolumeStep(VOLUME_STEP),
        "volume-down" => HwCommand::VolumeStep(-VOLUME_STEP),
        "volume-mute" => HwCommand::VolumeMuteToggle,
        "mic-mute" => HwCommand::MicMuteToggle,
        "brightness-up" => HwCommand::BrightnessStep(BRIGHTNESS_STEP),
        "brightness-down" => HwCommand::BrightnessStep(-BRIGHTNESS_STEP),
        "kbd-backlight-up" => HwCommand::KbdBacklightStep(KBD_BACKLIGHT_STEP),
        "kbd-backlight-down" => HwCommand::KbdBacklightStep(-KBD_BACKLIGHT_STEP),
        "kbd-backlight-toggle" => HwCommand::KbdBacklightToggle,
        "media-playpause" => HwCommand::Media(MediaAction::PlayPause),
        "media-stop" => HwCommand::Media(MediaAction::Stop),
        "media-next" => HwCommand::Media(MediaAction::Next),
        "media-prev" => HwCommand::Media(MediaAction::Prev),
        "media-forward" => HwCommand::Media(MediaAction::SeekMs(SEEK_MS)),
        "media-rewind" => HwCommand::Media(MediaAction::SeekMs(-SEEK_MS)),
        _ => return false,
    };
    init();
    if let Some(tx) = TX.get()
        && let Err(err) = tx.send(cmd)
    {
        tracing::warn!(%err, "hardware worker channel closed");
    }
    true
}

/// Lazily-connected D-Bus handles reused across commands.
#[derive(Default)]
struct Buses {
    system: Option<zbus::Connection>,
    session: Option<zbus::Connection>,
}

impl Buses {
    async fn system(&mut self) -> Option<&zbus::Connection> {
        if self.system.is_none() {
            match zbus::Connection::system().await {
                Ok(conn) => self.system = Some(conn),
                Err(err) => {
                    tracing::warn!(%err, "hardware: system bus connect failed");
                    return None;
                }
            }
        }
        self.system.as_ref()
    }

    async fn session(&mut self) -> Option<&zbus::Connection> {
        if self.session.is_none() {
            match zbus::Connection::session().await {
                Ok(conn) => self.session = Some(conn),
                Err(err) => {
                    tracing::warn!(%err, "hardware: session bus connect failed");
                    return None;
                }
            }
        }
        self.session.as_ref()
    }
}

async fn handle(buses: &mut Buses, cmd: HwCommand) {
    match cmd {
        HwCommand::VolumeStep(delta) => {
            pactl(&["set-sink-volume", "@DEFAULT_SINK@", &signed_percent(delta)]);
            report_volume();
        }
        HwCommand::VolumeMuteToggle => {
            pactl(&["set-sink-mute", "@DEFAULT_SINK@", "toggle"]);
            report_volume();
        }
        HwCommand::MicMuteToggle => {
            pactl(&["set-source-mute", "@DEFAULT_SOURCE@", "toggle"]);
            let muted = sink_muted(true).unwrap_or(false);
            let pct = source_volume().unwrap_or(0);
            osd(
                if muted {
                    "microphone-sensitivity-muted-symbolic"
                } else {
                    "microphone-sensitivity-high-symbolic"
                },
                "Microphone",
                if muted { None } else { Some(pct as f64) },
                muted,
            );
        }
        HwCommand::BrightnessStep(delta) => {
            adjust_backlight(buses, BacklightKind::Display, delta).await;
        }
        HwCommand::KbdBacklightStep(delta) => {
            adjust_backlight(buses, BacklightKind::Keyboard, delta).await;
        }
        HwCommand::KbdBacklightToggle => {
            toggle_kbd_backlight(buses).await;
        }
        HwCommand::Media(action) => {
            media(buses, action).await;
        }
    }
}

// ---- Audio (PipeWire / PulseAudio via `pactl`) --------------------------------

fn pactl(args: &[&str]) {
    if let Err(err) = std::process::Command::new("pactl").args(args).status() {
        tracing::warn!(%err, ?args, "pactl command failed");
    }
}

fn signed_percent(delta: i32) -> String {
    if delta >= 0 {
        format!("+{delta}%")
    } else {
        format!("{delta}%")
    }
}

fn report_volume() {
    let muted = sink_muted(false).unwrap_or(false);
    let pct = sink_volume().unwrap_or(0);
    let icon = if muted || pct == 0 {
        "audio-volume-muted-symbolic"
    } else if pct < 34 {
        "audio-volume-low-symbolic"
    } else if pct < 67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    };
    osd(icon, "Volume", Some(pct as f64), muted);
}

fn sink_volume() -> Option<u8> {
    parse_volume(&pactl_read(&["get-sink-volume", "@DEFAULT_SINK@"])?)
}

fn source_volume() -> Option<u8> {
    parse_volume(&pactl_read(&["get-source-volume", "@DEFAULT_SOURCE@"])?)
}

fn sink_muted(source: bool) -> Option<bool> {
    let args: [&str; 2] = if source {
        ["get-source-mute", "@DEFAULT_SOURCE@"]
    } else {
        ["get-sink-mute", "@DEFAULT_SINK@"]
    };
    let text = pactl_read(&args)?;
    if text.contains("yes") {
        Some(true)
    } else if text.contains("no") {
        Some(false)
    } else {
        None
    }
}

fn pactl_read(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("pactl")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_volume(text: &str) -> Option<u8> {
    // "Volume: front-left: 45678 /  70% / -9.something dB, ..." — first `%` token.
    text.split_whitespace()
        .find_map(|tok| tok.strip_suffix('%').and_then(|n| n.parse::<u16>().ok()))
        .map(|v| v.min(150) as u8)
}

// ---- Backlight (logind SetBrightness) -----------------------------------------

#[derive(Clone, Copy)]
enum BacklightKind {
    Display,
    Keyboard,
}

impl BacklightKind {
    fn class_dir(self) -> &'static str {
        match self {
            Self::Display => "/sys/class/backlight",
            Self::Keyboard => "/sys/class/leds",
        }
    }

    fn subsystem(self) -> &'static str {
        match self {
            Self::Display => "backlight",
            Self::Keyboard => "leds",
        }
    }

    fn osd(self) -> (&'static str, &'static str) {
        match self {
            Self::Display => ("display-brightness-symbolic", "Brightness"),
            Self::Keyboard => ("keyboard-brightness-symbolic", "Keyboard backlight"),
        }
    }
}

/// Locate the best backlight device directory. Prefer the device with the
/// largest `max_brightness` (typically `intel_backlight` / `amdgpu_bl*`). For
/// keyboards we require the `kbd_backlight` LED so we do not grab caps-lock /
/// power LEDs.
fn backlight_device(kind: BacklightKind) -> Option<PathBuf> {
    let dir = Path::new(kind.class_dir());
    let mut entries: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| match kind {
            BacklightKind::Display => true,
            BacklightKind::Keyboard => p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("kbd_backlight")),
        })
        .filter_map(|p| {
            let max = read_sysfs_u32(&p.join("max_brightness")).unwrap_or(0);
            (max > 0).then_some((max, p))
        })
        .collect();
    // Highest max first; name as tie-breaker for stability.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    entries.into_iter().next().map(|(_, p)| p)
}

fn read_sysfs_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn brightness_step(max: u32, delta_pct: i32) -> i32 {
    // `|raw|.max(1)` must be applied to the magnitude *before* the sign —
    // otherwise a negative step is clamped to `+1` by `.max(1)` and then
    // negated to `-1`, making brightness-down a no-op on 0–24000 devices.
    let magnitude = ((max as i64 * delta_pct.unsigned_abs() as i64) / 100).max(1) as i32;
    magnitude * delta_pct.signum()
}

async fn adjust_backlight(buses: &mut Buses, kind: BacklightKind, delta_pct: i32) {
    let Some(dev) = backlight_device(kind) else {
        tracing::debug!(class = kind.class_dir(), "no backlight device found");
        return;
    };
    let max = read_sysfs_u32(&dev.join("max_brightness")).unwrap_or(0);
    let cur = read_sysfs_u32(&dev.join("brightness")).unwrap_or(0);
    if max == 0 {
        return;
    }
    let step = brightness_step(max, delta_pct);
    // Displays keep a 1% floor so the panel never blanks from a hotkey; keyboard
    // backlights may go fully dark.
    let floor = match kind {
        BacklightKind::Display => (max / 100).max(1) as i32,
        BacklightKind::Keyboard => 0,
    };
    let next = (cur as i32 + step).clamp(floor, max as i32) as u32;
    let name = dev
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if !set_brightness(buses, kind.subsystem(), &name, &dev, next).await {
        tracing::warn!(
            device = %dev.display(),
            from = cur,
            to = next,
            "brightness write failed (logind, sysfs, and brightnessctl)"
        );
        return;
    }
    // Prefer the value the kernel reports after the write — some drivers snap.
    let applied = read_sysfs_u32(&dev.join("brightness")).unwrap_or(next);
    let pct = (applied as f64 / max as f64) * 100.0;
    let (icon, label) = kind.osd();
    osd(icon, label, Some(pct), false);
}

async fn toggle_kbd_backlight(buses: &mut Buses) {
    let Some(dev) = backlight_device(BacklightKind::Keyboard) else {
        return;
    };
    let max = read_sysfs_u32(&dev.join("max_brightness")).unwrap_or(0);
    let cur = read_sysfs_u32(&dev.join("brightness")).unwrap_or(0);
    if max == 0 {
        return;
    }
    let next = if cur > 0 { 0 } else { max };
    let name = dev
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if !set_brightness(buses, "leds", &name, &dev, next).await {
        return;
    }
    let applied = read_sysfs_u32(&dev.join("brightness")).unwrap_or(next);
    let pct = (applied as f64 / max as f64) * 100.0;
    osd(
        "keyboard-brightness-symbolic",
        "Keyboard backlight",
        Some(pct),
        applied == 0,
    );
}

/// Write brightness through the best available path:
/// 1. logind `Session.SetBrightness` (no special perms when the session owns the seat)
/// 2. direct sysfs write (works when the user is in `video`/`input` or udev ACL grants rw)
/// 3. `brightnessctl` if installed (setuid helper on many distros)
async fn set_brightness(
    buses: &mut Buses,
    subsystem: &str,
    name: &str,
    device_dir: &Path,
    value: u32,
) -> bool {
    if set_brightness_logind(buses, subsystem, name, value).await {
        return true;
    }
    if set_brightness_sysfs(device_dir, value) {
        return true;
    }
    set_brightness_cli(subsystem, name, value)
}

async fn set_brightness_logind(buses: &mut Buses, subsystem: &str, name: &str, value: u32) -> bool {
    let Some(conn) = buses.system().await else {
        return false;
    };
    let path = session_object_path();
    let proxy = match zbus::Proxy::new(
        conn,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.Session",
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(err) => {
            tracing::debug!(%err, "logind session proxy unavailable");
            return false;
        }
    };
    match proxy
        .call::<_, _, ()>("SetBrightness", &(subsystem, name, value))
        .await
    {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(%err, subsystem, name, value, "logind SetBrightness failed");
            false
        }
    }
}

fn set_brightness_sysfs(device_dir: &Path, value: u32) -> bool {
    let path = device_dir.join("brightness");
    match std::fs::write(&path, value.to_string()) {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(%err, path = %path.display(), value, "sysfs brightness write failed");
            false
        }
    }
}

fn set_brightness_cli(subsystem: &str, name: &str, value: u32) -> bool {
    // `brightnessctl --device=<name> --class=<subsystem> set <value>`
    let status = std::process::Command::new("brightnessctl")
        .args([
            &format!("--class={subsystem}"),
            &format!("--device={name}"),
            "set",
            &value.to_string(),
        ])
        .status();
    match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            tracing::debug!(?s, subsystem, name, value, "brightnessctl exited non-zero");
            false
        }
        Err(err) => {
            tracing::debug!(%err, "brightnessctl not available");
            false
        }
    }
}

/// Object path of the caller's login session. Prefers `XDG_SESSION_ID`; falls
/// back to logind's `auto` alias for the current session.
fn session_object_path() -> String {
    match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.trim().is_empty() => {
            format!("/org/freedesktop/login1/session/{}", id.trim())
        }
        _ => "/org/freedesktop/login1/session/auto".to_string(),
    }
}

// ---- Media transport (MPRIS) --------------------------------------------------

async fn media(buses: &mut Buses, action: MediaAction) {
    let Some(conn) = buses.session().await else {
        return;
    };
    let Some(target) = active_player(conn).await else {
        tracing::debug!("no MPRIS media player available");
        return;
    };
    let proxy = match zbus::Proxy::new(
        conn,
        target.as_str(),
        "/org/mpris/MediaPlayer2",
        "org.mpris.MediaPlayer2.Player",
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(err) => {
            tracing::warn!(%err, "MPRIS player proxy failed");
            return;
        }
    };
    let (method, arg_us): (&str, Option<i64>) = match action {
        MediaAction::PlayPause => ("PlayPause", None),
        MediaAction::Stop => ("Stop", None),
        MediaAction::Next => ("Next", None),
        MediaAction::Prev => ("Previous", None),
        MediaAction::SeekMs(ms) => ("Seek", Some(ms * 1000)),
    };
    let result = match arg_us {
        Some(us) => proxy.call::<_, _, ()>(method, &(us,)).await,
        None => proxy.call::<_, _, ()>(method, &()).await,
    };
    if let Err(err) = result {
        tracing::warn!(%err, method, "MPRIS command failed");
        return;
    }
    let (icon, label) = media_osd(action);
    osd(icon, label, None, false);
}

/// Choose an MPRIS target, preferring a player that is currently `Playing`.
async fn active_player(conn: &zbus::Connection) -> Option<String> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await.ok()?;
    let names = dbus.list_names().await.ok()?;
    let players: Vec<String> = names
        .into_iter()
        .map(|n| n.as_str().to_string())
        .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
        .collect();
    if players.is_empty() {
        return None;
    }
    for name in &players {
        if let Ok(proxy) = zbus::Proxy::new(
            conn,
            name.as_str(),
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
        )
        .await
            && let Ok(status) = proxy.get_property::<String>("PlaybackStatus").await
            && status == "Playing"
        {
            return Some(name.clone());
        }
    }
    players.into_iter().next()
}

fn media_osd(action: MediaAction) -> (&'static str, &'static str) {
    match action {
        MediaAction::PlayPause => ("media-playback-start-symbolic", "Play / Pause"),
        MediaAction::Stop => ("media-playback-stop-symbolic", "Stop"),
        MediaAction::Next => ("media-skip-forward-symbolic", "Next track"),
        MediaAction::Prev => ("media-skip-backward-symbolic", "Previous track"),
        MediaAction::SeekMs(ms) if ms >= 0 => ("media-seek-forward-symbolic", "Fast-forward"),
        MediaAction::SeekMs(_) => ("media-seek-backward-symbolic", "Rewind"),
    }
}

// ---- OSD bridge ---------------------------------------------------------------

/// Flash the on-screen overlay from the worker thread. `level` (0–100) draws the
/// progress bar; `None` shows an icon-only card (media transport). The closure
/// runs on the GTK main thread via the default main context.
fn osd(icon: &'static str, title: &'static str, level: Option<f64>, muted: bool) {
    let title = title.to_string();
    glib::idle_add_once(move || {
        crate::ui::osd::show(icon, &title, level, muted);
    });
}
