use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::services::notifications::BarNotification;
use crate::services::workspaces;

#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
enum AudioCommand {
    SetVolumeAbsolute(u8),
    SetVolumeRelative(i8),
    SetMute(bool),
    SetMicVolumeAbsolute(u8),
    SetMicMute(bool),
}

static AUDIO_CMD_TX: OnceLock<Sender<AudioCommand>> = OnceLock::new();

/// Set by the NetworkManager D-Bus watcher so the poll loop refreshes network
/// state immediately instead of waiting for the next slow tick.
static NETWORK_DIRTY: AtomicBool = AtomicBool::new(true);

/// Ignore `nmcli radio wifi` writes until this instant. Display modesets / HDMI
/// hotplug briefly make NetworkManager look flaky; a false "off" sync must not
/// permanently kill the radio.
static WIFI_RADIO_SUPPRESS_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// One-shot gate: `nmcli radio wifi off` is refused unless a real pointer/touch
/// arm set this immediately beforehand. HDMI modeset used to synthesize GTK
/// switch notifies that called `wifi_set_radio(false)`.
static WIFI_USER_OFF_ARMED: AtomicBool = AtomicBool::new(false);

/// Set by [`wifi_set_radio`] when an armed off is queued; consumed in
/// [`spawn_nmcli`] so orphan `radio wifi off` invocations cannot slip through.
static WIFI_OFF_SPAWN_ALLOWED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
enum NetworkCommand {
    Scan,
    Connect {
        ssid: String,
        password: Option<String>,
    },
    SetRadio(bool),
    VpnUp(String),
    VpnUpWithPassword {
        target: String,
        password: String,
        remember: bool,
    },
    VpnDown(String),
}

static NETWORK_CMD_TX: OnceLock<Sender<NetworkCommand>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct VpnOpState {
    /// UUID/name currently connecting or disconnecting.
    pending: Option<(String, Instant, bool)>,
    /// Last failed connect/disconnect message for the popover.
    last_error: Option<String>,
    /// Profile that needs a password before connect can succeed.
    needs_password: Option<String>,
}

static VPN_OP: Mutex<VpnOpState> = Mutex::new(VpnOpState {
    pending: None,
    last_error: None,
    needs_password: None,
});

/// Last known UUID → display name pairs for VPN toast labels.
static LAST_VPN_NAMES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// A single visible Wi-Fi network (deduped by SSID).
#[derive(Debug, Clone, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
    pub active: bool,
}

/// Read-only wired status for the network popover.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EthernetStatus {
    /// Whether an ethernet device exists at all (row is hidden when false).
    pub present: bool,
    pub connected: bool,
    pub label: String,
}

/// A NetworkManager VPN / WireGuard profile for the bar popover.
#[derive(Debug, Clone, PartialEq)]
pub struct VpnStatus {
    pub name: String,
    pub uuid: String,
    pub kind: String,
    pub active: bool,
    /// True while an up/down nmcli is in flight for this profile.
    pub pending: bool,
}

/// Bar-facing VPN operation feedback (spinner target + last error).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VpnFeedback {
    pub pending_target: Option<String>,
    pub connecting: bool,
    pub last_error: Option<String>,
    /// UUID/name that needs a password prompt in the popover.
    pub needs_password: Option<String>,
}

/// A single connected Bluetooth device, with battery level when the device
/// reports one (mice, keyboards, headsets via the BlueZ `Battery1` interface).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BluetoothDevice {
    /// Device MAC address (`AA:BB:CC:DD:EE:FF`) — stable key for battery alerts.
    pub address: String,
    pub name: String,
    /// Battery charge 0–100 when the device exposes it, else `None`.
    pub battery_percent: Option<u8>,
    /// Charging state when the source reports it (kernel HID `status` or UPower
    /// `state`). `None` means unknown — most devices over plain Bluetooth cannot
    /// signal charging, since the BT Battery Service has no such characteristic.
    pub battery_charging: Option<bool>,
}

/// Bluetooth adapter status for the conditional bar indicator.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BluetoothStatus {
    /// Whether a Bluetooth adapter exists at all (widget hidden when false).
    pub adapter_present: bool,
    pub powered: bool,
    pub connected: bool,
    /// First connected device's name, kept for the compact bar tooltip.
    pub device_name: Option<String>,
    /// All currently-connected devices (with battery where reported).
    pub devices: Vec<BluetoothDevice>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarSnapshot {
    pub battery_percent: Option<u8>,
    pub battery_charging: bool,
    pub bluetooth: BluetoothStatus,
    pub wifi: Vec<WifiNetwork>,
    pub ethernet: EthernetStatus,
    pub wifi_enabled: bool,
    pub vpn: Vec<VpnStatus>,
    pub vpn_feedback: VpnFeedback,
    pub volume_percent: u8,
    pub volume_muted: bool,
    pub mic_percent: u8,
    pub mic_muted: bool,
    pub notifications: Vec<BarNotification>,
    pub workspaces: workspaces::WorkspaceSnapshot,
}

pub fn spawn_bar_pollers() -> Receiver<BarSnapshot> {
    let (tx, rx) = mpsc::channel();
    let (audio_tx, audio_rx) = mpsc::channel();
    let _ = AUDIO_CMD_TX.set(audio_tx);
    let (network_tx, network_rx) = mpsc::channel();
    let _ = NETWORK_CMD_TX.set(network_tx);
    spawn_vpn_session_autoconnect();
    spawn_network_dbus_watcher();
    thread::Builder::new()
        .name("metis-bar-poll".into())
        .spawn(move || poll_loop(tx, audio_rx, network_rx))
        .expect("spawn bar poller");
    rx
}

/// Subscribe to NetworkManager D-Bus signals so Wi-Fi/VPN/Ethernet changes wake
/// the poll loop. Falls back silently to timed nmcli scans if D-Bus is unavailable.
fn spawn_network_dbus_watcher() {
    thread::Builder::new()
        .name("metis-nm-dbus".into())
        .spawn(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::debug!(%err, "nm dbus watcher: no tokio runtime");
                    return;
                }
            };
            rt.block_on(async {
                let Ok(conn) = zbus::Connection::system().await else {
                    tracing::debug!("nm dbus watcher: system bus unavailable");
                    return;
                };
                let proxy = match zbus::Proxy::new(
                    &conn,
                    "org.freedesktop.NetworkManager",
                    "/org/freedesktop/NetworkManager",
                    "org.freedesktop.NetworkManager",
                )
                .await
                {
                    Ok(p) => p,
                    Err(err) => {
                        tracing::debug!(%err, "nm dbus watcher: proxy failed");
                        return;
                    }
                };
                // StateChanged is the stable NM signal for connectivity changes.
                let mut state_changed = match proxy.receive_signal("StateChanged").await {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::debug!(%err, "nm dbus watcher: StateChanged subscribe failed");
                        return;
                    }
                };
                use futures_util::StreamExt;
                while state_changed.next().await.is_some() {
                    NETWORK_DIRTY.store(true, Ordering::Relaxed);
                }
            });
        })
        .ok();
}

/// After login, NetworkManager often leaves WireGuard/`autoconnect=yes` idle
/// until the underlay (Wi‑Fi/Ethernet) is fully up. Bring the exclusive
/// autoconnect VPN profile up once connectivity looks ready.
fn spawn_vpn_session_autoconnect() {
    thread::Builder::new()
        .name("metis-vpn-auto".into())
        .spawn(|| {
            for attempt in 0..24 {
                let delay = if attempt == 0 {
                    Duration::from_secs(4)
                } else {
                    Duration::from_secs(2)
                };
                thread::sleep(delay);
                if !network_underlay_ready() {
                    continue;
                }
                let Some((uuid, _name)) = vpn_autoconnect_target() else {
                    return;
                };
                if vpn_uuid_active(&uuid) {
                    return;
                }
                tracing::info!(%uuid, "bringing up VPN autoconnect profile");
                begin_vpn_op(uuid.clone(), true);
                spawn_nmcli_vpn(
                    vec!["connection".into(), "up".into(), uuid.clone()],
                    Duration::from_secs(45),
                    uuid,
                    true,
                    None,
                );
                return;
            }
            tracing::debug!("VPN autoconnect: gave up waiting for network underlay");
        })
        .ok();
}

fn network_underlay_ready() -> bool {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "STATE", "general"]);
    let Some(output) = run_command(&mut cmd) else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let state = text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // "connected" is ideal; "connecting" means underlay is coming up — keep waiting.
    state == "connected"
}

/// UUID + name of the VPN profile with autoconnect enabled (highest priority wins).
fn vpn_autoconnect_target() -> Option<(String, String)> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args([
        "-t",
        "-f",
        "NAME,UUID,TYPE,AUTOCONNECT,AUTOCONNECT-PRIORITY",
        "connection",
        "show",
    ]);
    let output = run_command(&mut cmd)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut best: Option<(i32, String, String)> = None;
    for line in text.lines() {
        let f = nmcli_split(line);
        if f.len() < 4 {
            continue;
        }
        if !is_vpn_connection_type(&f[2]) {
            continue;
        }
        let auto =
            f[3].eq_ignore_ascii_case("yes") || f[3].eq_ignore_ascii_case("true") || f[3] == "1";
        if !auto {
            continue;
        }
        let prio: i32 = f.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        match &best {
            Some((bp, _, _)) if *bp >= prio => {}
            _ => best = Some((prio, f[1].clone(), f[0].clone())),
        }
    }
    best.map(|(_, uuid, name)| (uuid, name))
}

fn vpn_uuid_active(uuid: &str) -> bool {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "UUID,TYPE", "connection", "show", "--active"]);
    let Some(output) = run_command(&mut cmd) else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| {
        let f = nmcli_split(line);
        f.len() >= 2 && f[0] == uuid && is_vpn_connection_type(&f[1])
    })
}

fn poll_loop(
    tx: Sender<BarSnapshot>,
    audio_rx: Receiver<AudioCommand>,
    network_rx: Receiver<NetworkCommand>,
) {
    let mut tick: u64 = 0;
    let mut cached = BarSnapshot {
        workspaces: workspaces::workspace_snapshot(),
        ..Default::default()
    };
    let mut last_sent = cached.clone();
    let mut wifi_scan_grace_until: Option<std::time::Instant> = None;

    loop {
        // A user audio action (slider/mute/scroll) forces an immediate volume
        // read-back below so *every* bar reflects the change within one poll cycle
        // instead of waiting for the next 800ms volume tick — without this, bars
        // other than the one whose popup is open lagged by several seconds.
        let audio_changed = drain_audio_commands(&audio_rx);
        drain_network_commands(&network_rx, &mut wifi_scan_grace_until);
        let network_dirty = NETWORK_DIRTY.swap(false, Ordering::Relaxed);

        // Battery/BT: slow fallback (~12–24 s). Network: D-Bus dirty or timed tick.
        if tick.is_multiple_of(30) {
            cached.battery_percent = read_battery_percent();
            cached.battery_charging = read_battery_charging();
            cached.bluetooth = read_bluetooth_status();
        }
        if network_dirty || tick.is_multiple_of(5) {
            cached.wifi_enabled = read_wifi_radio_enabled();
            if let Some(eth) = read_ethernet_status() {
                cached.ethernet = eth;
            }
            if let Some(vpn) = read_vpn_status() {
                if let Ok(mut names) = LAST_VPN_NAMES.lock() {
                    *names = vpn
                        .iter()
                        .map(|v| (v.uuid.clone(), v.name.clone()))
                        .collect();
                }
                cached.vpn = vpn;
            }
            cached.vpn_feedback = read_vpn_feedback(&cached.vpn);
            if cached.wifi_enabled {
                if let Some(networks) = read_wifi_networks() {
                    let in_scan_grace = wifi_scan_grace_until
                        .is_some_and(|until| std::time::Instant::now() < until);
                    cached.wifi = stabilize_wifi_list(&cached.wifi, networks, in_scan_grace);
                }
                // On nmcli timeout keep the last good Wi-Fi list so the bar icon
                // does not flash offline between poll ticks.
            } else {
                cached.wifi.clear();
            }
        }
        if tick.is_multiple_of(2) || audio_changed {
            // Keep the last good reading when pactl times out / reports nothing,
            // so a transient failure doesn't snap sliders to 0 or flip mute state.
            if let Some(v) = read_volume_percent() {
                cached.volume_percent = v;
            }
            if let Some(m) = read_volume_muted() {
                cached.volume_muted = m;
            }
            if let Some(v) = read_mic_percent() {
                cached.mic_percent = v;
            }
            if let Some(m) = read_mic_muted() {
                cached.mic_muted = m;
            }
        }
        cached.workspaces = workspaces::workspace_snapshot();

        let changed = cached != last_sent;
        if changed {
            last_sent = cached.clone();
            if tx.send(cached.clone()).is_err() {
                break;
            }
        }
        tick = tick.wrapping_add(1);
        let sleep_ms = if audio_changed || network_dirty {
            200
        } else if changed {
            400
        } else {
            800
        };
        thread::sleep(Duration::from_millis(sleep_ms));
    }
}

/// Returns true if at least one audio command was handled, so the caller can
/// force an immediate volume read-back (fast cross-bar feedback).
fn drain_audio_commands(rx: &Receiver<AudioCommand>) -> bool {
    let mut handled = false;
    while let Ok(cmd) = rx.try_recv() {
        handled = true;
        match cmd {
            AudioCommand::SetVolumeAbsolute(pct) => run_set_volume_absolute(pct),
            AudioCommand::SetVolumeRelative(delta) => run_set_volume_relative(delta),
            AudioCommand::SetMute(muted) => run_set_mute(muted),
            AudioCommand::SetMicVolumeAbsolute(pct) => run_set_mic_volume_absolute(pct),
            AudioCommand::SetMicMute(muted) => run_set_mic_mute(muted),
        }
    }
    handled
}

fn queue_audio(cmd: AudioCommand) {
    if let Some(tx) = AUDIO_CMD_TX.get() {
        let _ = tx.send(cmd);
    }
}

fn drain_network_commands(
    rx: &Receiver<NetworkCommand>,
    wifi_scan_grace_until: &mut Option<std::time::Instant>,
) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            NetworkCommand::Scan => {
                *wifi_scan_grace_until = Some(std::time::Instant::now() + Duration::from_secs(4));
                spawn_nmcli(
                    vec!["dev".into(), "wifi".into(), "rescan".into()],
                    Duration::from_secs(10),
                );
            }
            NetworkCommand::Connect { ssid, password } => run_wifi_connect(ssid, password),
            NetworkCommand::SetRadio(on) => {
                let state = if on { "on" } else { "off" };
                tracing::info!(on, "wifi: applying nmcli radio wifi {state}");
                spawn_nmcli(
                    vec!["radio".into(), "wifi".into(), state.into()],
                    Duration::from_secs(5),
                );
            }
            NetworkCommand::VpnUp(target) => {
                begin_vpn_op(target.clone(), true);
                spawn_nmcli_vpn(
                    vec!["connection".into(), "up".into(), target.clone()],
                    Duration::from_secs(45),
                    target,
                    true,
                    None,
                );
            }
            NetworkCommand::VpnUpWithPassword {
                target,
                password,
                remember,
            } => {
                begin_vpn_op(target.clone(), true);
                spawn_nmcli_vpn(
                    vec!["connection".into(), "up".into(), target.clone()],
                    Duration::from_secs(45),
                    target,
                    true,
                    Some((password, remember)),
                );
            }
            NetworkCommand::VpnDown(target) => {
                begin_vpn_op(target.clone(), false);
                spawn_nmcli_vpn(
                    vec!["connection".into(), "down".into(), target.clone()],
                    Duration::from_secs(20),
                    target,
                    false,
                    None,
                );
            }
        }
    }
}

fn queue_network(cmd: NetworkCommand) {
    if let Some(tx) = NETWORK_CMD_TX.get() {
        let _ = tx.send(cmd);
    }
}

/// Trigger a Wi-Fi rescan; refreshed results arrive on a later poll tick.
pub fn wifi_scan() {
    queue_network(NetworkCommand::Scan);
}

/// Connect to `ssid`, optionally with a password (for secured networks).
pub fn wifi_connect(ssid: String, password: Option<String>) {
    queue_network(NetworkCommand::Connect { ssid, password });
}

/// Arm the next Wi-Fi radio-off. Call from a pointer/touch press on the switch
/// *before* GTK emits the state change. Unarmed `wifi_set_radio(false)` is a no-op.
pub fn arm_user_wifi_radio_toggle() {
    WIFI_USER_OFF_ARMED.store(true, Ordering::SeqCst);
}

/// Enable or disable the Wi-Fi radio from an **explicit user toggle** only.
///
/// Turning the radio **off** requires a prior [`arm_user_wifi_radio_toggle`] from
/// a real click. Returns `false` if an unarmed off was refused (caller should
/// keep the switch on).
pub fn wifi_set_radio(on: bool) -> bool {
    if !on {
        if !WIFI_USER_OFF_ARMED.swap(false, Ordering::SeqCst) {
            tracing::warn!("wifi_set_radio(false) ignored — not user-armed");
            return false;
        }
        WIFI_OFF_SPAWN_ALLOWED.store(true, Ordering::SeqCst);
    } else {
        // Click-to-enable also arms; drop leftover so a later phantom off cannot pass.
        WIFI_USER_OFF_ARMED.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = WIFI_RADIO_SUPPRESS_UNTIL.lock() {
            *slot = None;
        }
    }
    queue_network(NetworkCommand::SetRadio(on));
    true
}

/// Turn the Wi-Fi radio on if needed (Rescan). Never turns it off.
#[allow(dead_code)] // network control helper
pub fn wifi_ensure_radio_on() {
    if wifi_radio_writes_suppressed() {
        return;
    }
    queue_network(NetworkCommand::SetRadio(true));
}

/// Block Wi-Fi radio *writes* during display hotplug. Also used as a signal to
/// the bar UI not to trust a transient "disabled" reading.
pub fn suppress_wifi_radio_writes(for_secs: u64) {
    let until = Instant::now() + Duration::from_secs(for_secs.max(1));
    if let Ok(mut slot) = WIFI_RADIO_SUPPRESS_UNTIL.lock() {
        let next = slot.map_or(until, |prev| prev.max(until));
        *slot = Some(next);
    }
}

/// True while HDMI/modeset suppress is active — UI must not treat "radio off"
/// as authoritative, and must not apply a switch-off.
pub fn wifi_hotplug_suppressed() -> bool {
    wifi_radio_writes_suppressed()
}

fn wifi_radio_writes_suppressed() -> bool {
    WIFI_RADIO_SUPPRESS_UNTIL
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .is_some_and(|until| Instant::now() < until)
}

/// Bring a NetworkManager VPN / WireGuard profile up (by UUID or name).
pub fn vpn_up(target: String) {
    queue_network(NetworkCommand::VpnUp(target));
}

/// Bring a VPN up with an explicit password (and optionally remember it on the profile).
pub fn vpn_up_with_password(target: String, password: String, remember: bool) {
    queue_network(NetworkCommand::VpnUpWithPassword {
        target,
        password,
        remember,
    });
}

/// Clear a pending "needs password" prompt (e.g. user cancelled).
pub fn vpn_clear_password_prompt() {
    if let Ok(mut st) = VPN_OP.lock() {
        st.needs_password = None;
        if st
            .last_error
            .as_ref()
            .is_some_and(|e| vpn_secret_required(e))
        {
            st.last_error = None;
        }
    }
}

/// Take a NetworkManager VPN / WireGuard profile down.
pub fn vpn_down(target: String) {
    queue_network(NetworkCommand::VpnDown(target));
}

/// Enable or disable the Bluetooth adapter radio.
pub fn bluetooth_set_powered(on: bool) {
    let state = if on { "on" } else { "off" };
    spawn_bluetoothctl(vec!["power".into(), state.into()], Duration::from_secs(8));
}

fn run_wifi_connect(ssid: String, password: Option<String>) {
    let mut args = vec![
        "dev".to_string(),
        "wifi".to_string(),
        "connect".to_string(),
        ssid,
    ];
    if let Some(pw) = password {
        if !pw.is_empty() {
            args.push("password".to_string());
            args.push(pw);
        }
    }
    // Association + DHCP can take many seconds, well past the 600ms read budget,
    // so this runs detached. Success surfaces via the next snapshot's active SSID.
    run_wifi_connect_owned(args);
}

fn run_wifi_connect_owned(args: Vec<String>) {
    spawn_nmcli(args, Duration::from_secs(25));
}

/// Spawn an `nmcli` invocation on a detached thread, killing it after `timeout`.
fn spawn_nmcli(args: Vec<String>, timeout: Duration) {
    // Last-line defense: never run `radio wifi off` unless a user-armed
    // `wifi_set_radio(false)` already queued it (arm bit already consumed there).
    // Orphan/stray callers must not kill Wi-Fi during HDMI modeset.
    if args.len() >= 3
        && args[0] == "radio"
        && args[1] == "wifi"
        && args[2].eq_ignore_ascii_case("off")
    {
        // Only SetRadio(false) from wifi_set_radio reaches here after consuming
        // the arm bit — mark with a side channel so we can still refuse orphans.
        if !WIFI_OFF_SPAWN_ALLOWED.swap(false, Ordering::SeqCst) {
            tracing::error!(
                "spawn_nmcli blocked orphan `radio wifi off` \
                 (HDMI/modeset must never disable Wi-Fi)"
            );
            return;
        }
    }
    thread::spawn(move || {
        let mut cmd = std::process::Command::new("nmcli");
        cmd.args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let Ok(mut child) = cmd.spawn() else {
            return;
        };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {}
                Err(_) => return,
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

fn begin_vpn_op(target: String, connecting: bool) {
    if let Ok(mut st) = VPN_OP.lock() {
        st.pending = Some((target, Instant::now(), connecting));
        st.last_error = None;
        st.needs_password = None;
    }
}

fn finish_vpn_op(target: &str, connecting: bool, error: Option<String>) {
    if let Ok(mut st) = VPN_OP.lock() {
        if st.pending.as_ref().is_some_and(|(t, _, _)| t == target) {
            st.pending = None;
        }
        if let Some(err) = error.clone() {
            if connecting && vpn_secret_required(&err) {
                st.needs_password = Some(target.to_string());
                st.last_error = Some("Password required.".into());
            } else {
                st.needs_password = None;
                st.last_error = Some(err);
            }
        } else {
            st.last_error = None;
            st.needs_password = None;
        }
    }
    // Don't toast a password prompt — the popover asks instead.
    if error.as_ref().is_some_and(|e| vpn_secret_required(e)) {
        return;
    }
    notify_vpn_result(target, connecting, error);
}

fn vpn_already_inactive(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("not an active connection")
        || e.contains("no active connection")
        || e.contains("not active")
}

fn vpn_secret_required(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("secrets were required")
        || e.contains("secret was required")
        || e.contains("no secrets")
        || e.contains("missing secret")
        || e.contains("password is required")
        || e.contains("password required")
        || e.contains("need password")
        || e.contains("authentication required")
        || (e.contains("password") && (e.contains("required") || e.contains("provide")))
}

fn notify_vpn_result(target: &str, connecting: bool, error: Option<String>) {
    let label = vpn_display_name(target);
    glib::idle_add_once(move || {
        let (kind, title, message) = if let Some(err) = error {
            (
                crate::services::NotificationKind::Error,
                "VPN".to_string(),
                format!("{label}: {err}"),
            )
        } else if connecting {
            (
                crate::services::NotificationKind::Success,
                "VPN connected".to_string(),
                label,
            )
        } else {
            (
                crate::services::NotificationKind::Information,
                "VPN disconnected".to_string(),
                label,
            )
        };
        let note = crate::services::BarNotification::internal(kind, title, message);
        if !crate::services::do_not_disturb() {
            crate::ui::toast::show(&note);
        }
        crate::services::push_notification(note);
    });
}

fn vpn_display_name(target: &str) -> String {
    if let Ok(guard) = LAST_VPN_NAMES.lock() {
        if let Some(name) = guard
            .iter()
            .find(|(uuid, name)| uuid == target || name == target)
            .map(|(_, name)| name.clone())
        {
            return name;
        }
    }
    target.to_string()
}

fn read_vpn_feedback(vpn: &[VpnStatus]) -> VpnFeedback {
    let Ok(mut st) = VPN_OP.lock() else {
        return VpnFeedback::default();
    };
    // Clear pending once the profile reaches the expected active state, or after timeout.
    if let Some((ref target, started, connecting)) = st.pending.clone() {
        let matched = vpn.iter().find(|v| v.uuid == *target || v.name == *target);
        let settled = matched.is_some_and(|v| v.active == connecting);
        if settled || started.elapsed() > Duration::from_secs(50) {
            if !settled && started.elapsed() > Duration::from_secs(50) {
                st.last_error = Some(if connecting {
                    "VPN connect timed out.".into()
                } else {
                    "VPN disconnect timed out.".into()
                });
            }
            st.pending = None;
        }
    }
    VpnFeedback {
        pending_target: st.pending.as_ref().map(|(t, _, _)| t.clone()),
        connecting: st.pending.as_ref().is_some_and(|(_, _, c)| *c),
        last_error: st.last_error.clone(),
        needs_password: st.needs_password.clone(),
    }
}

/// Spawn nmcli for VPN up/down, capturing stderr so the bar can show failures.
/// When `password` is set, writes a temporary passwd-file and optionally remembers
/// the secret on the NM profile after a successful connect.
fn spawn_nmcli_vpn(
    mut args: Vec<String>,
    timeout: Duration,
    target: String,
    connecting: bool,
    password: Option<(String, bool)>,
) {
    thread::spawn(move || {
        use std::io::Read;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let passwd_path = if let Some((ref pw, _)) = password {
            let path = std::env::temp_dir().join(format!(
                "metis-vpn-passwd-{}-{}.tmp",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "vpn.secrets.password:{pw}");
                    let _ = writeln!(file, "vpn.secrets.cert-pass:{pw}");
                    args.push("passwd-file".into());
                    args.push(path.to_string_lossy().into_owned());
                    Some(path)
                }
                Err(_) => {
                    finish_vpn_op(
                        &target,
                        connecting,
                        Some("Could not prepare VPN password.".into()),
                    );
                    return;
                }
            }
        } else {
            None
        };

        let mut cmd = std::process::Command::new("nmcli");
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let Ok(mut child) = cmd.spawn() else {
            if let Some(path) = passwd_path {
                let _ = std::fs::remove_file(path);
            }
            finish_vpn_op(&target, connecting, Some("Could not run nmcli.".into()));
            return;
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_string(&mut stdout);
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_string(&mut stderr);
                    }
                    let _ = child.wait();
                    if let Some(path) = passwd_path {
                        let _ = std::fs::remove_file(path);
                    }
                    if status.success() {
                        if let Some((pw, true)) = password {
                            remember_vpn_password(&target, &pw);
                        }
                        finish_vpn_op(&target, connecting, None);
                    } else {
                        let msg = if !stderr.trim().is_empty() {
                            stderr.trim().to_string()
                        } else if !stdout.trim().is_empty() {
                            stdout.trim().to_string()
                        } else {
                            "VPN operation failed.".into()
                        };
                        // Stale UI: disconnecting an already-down profile is fine.
                        if !connecting && vpn_already_inactive(&msg) {
                            finish_vpn_op(&target, connecting, None);
                        } else {
                            let clean = msg.trim_start_matches("Error:").trim().to_string();
                            finish_vpn_op(&target, connecting, Some(clean));
                        }
                    }
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    if let Some(path) = passwd_path {
                        let _ = std::fs::remove_file(path);
                    }
                    finish_vpn_op(&target, connecting, Some("VPN operation failed.".into()));
                    return;
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(path) = passwd_path {
                    let _ = std::fs::remove_file(path);
                }
                finish_vpn_op(&target, connecting, Some("VPN operation timed out.".into()));
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

fn remember_vpn_password(target: &str, password: &str) {
    let secret = format!("password={password}");
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["connection", "modify", target, "vpn.secrets", &secret]);
    let ok = run_command_with_timeout(&mut cmd, Duration::from_secs(8))
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        let alt = format!("password:{password}");
        let mut cmd2 = std::process::Command::new("nmcli");
        cmd2.args(["connection", "modify", target, "+vpn.secrets", &alt]);
        let _ = run_command_with_timeout(&mut cmd2, Duration::from_secs(8));
    }
    let mut flags = std::process::Command::new("nmcli");
    flags.args([
        "connection",
        "modify",
        target,
        "+vpn.data",
        "password-flags=0",
    ]);
    let _ = run_command_with_timeout(&mut flags, Duration::from_secs(5));
}

/// Spawn a `bluetoothctl` invocation on a detached thread.
fn spawn_bluetoothctl(args: Vec<String>, timeout: Duration) {
    thread::spawn(move || {
        let mut cmd = std::process::Command::new("bluetoothctl");
        cmd.args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let Ok(mut child) = cmd.spawn() else {
            return;
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {}
                Err(_) => return,
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

/// Split a terse (`-t`) nmcli line into fields, honoring `\:` / `\\` escapes.
fn nmcli_split(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ':' => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    fields
}

/// One-shot Wi-Fi / ethernet read for onboarding (and other UI that is not on
/// the bar poll tick). Prefer the poller's cached snapshot when possible.
pub fn network_snapshot_for_ui() -> (bool, EthernetStatus, Vec<WifiNetwork>) {
    let wifi_enabled = read_wifi_radio_enabled();
    let ethernet = read_ethernet_status().unwrap_or_default();
    let wifi = if wifi_enabled {
        read_wifi_networks().unwrap_or_default()
    } else {
        Vec::new()
    };
    (wifi_enabled, ethernet, wifi)
}

fn read_wifi_radio_enabled() -> bool {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "WIFI", "radio"]);
    run_command(&mut cmd)
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .map(str::trim)
                .any(|line| line.eq_ignore_ascii_case("enabled"))
                || text.trim().is_empty()
        })
        .unwrap_or(true)
}

fn read_wifi_networks() -> Option<Vec<WifiNetwork>> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "ACTIVE,SSID,SIGNAL,SECURITY", "dev", "wifi"]);
    let output = run_command_with_timeout(&mut cmd, Duration::from_millis(2_000))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut nets: Vec<WifiNetwork> = Vec::new();
    for line in text.lines() {
        let fields = nmcli_split(line);
        if fields.len() < 4 {
            continue;
        }
        let active = fields[0] == "yes";
        let ssid = fields[1].clone();
        if ssid.is_empty() {
            continue;
        }
        let signal = fields[2].parse().unwrap_or(0);
        let security = fields[3].trim();
        let secured = !security.is_empty() && security != "--";
        if let Some(existing) = nets.iter_mut().find(|n| n.ssid == ssid) {
            // Keep the strongest reading and remember if any BSS is active/secured.
            existing.active = existing.active || active;
            if signal > existing.signal {
                existing.signal = signal;
                existing.secured = secured;
            }
            continue;
        }
        nets.push(WifiNetwork {
            ssid,
            signal,
            secured,
            active,
        });
    }
    nets.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    Some(nets)
}

/// During an nmcli rescan the active SSID can briefly disappear or report 0 signal.
/// Hold the last known connection so the bar icon does not flicker offline.
fn stabilize_wifi_list(
    previous: &[WifiNetwork],
    mut networks: Vec<WifiNetwork>,
    in_scan_grace: bool,
) -> Vec<WifiNetwork> {
    let prev_active = previous.iter().find(|n| n.active);

    if networks.is_empty() {
        if prev_active.is_some() {
            return previous.to_vec();
        }
        return networks;
    }

    if !in_scan_grace {
        return networks;
    }

    let Some(prev_active) = prev_active else {
        return networks;
    };
    if networks.iter().any(|n| n.active) {
        return networks;
    }
    if let Some(current) = networks.iter_mut().find(|n| n.ssid == prev_active.ssid) {
        current.active = true;
        if current.signal == 0 {
            current.signal = prev_active.signal;
        }
        networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
        return networks;
    }
    networks.push(WifiNetwork {
        ssid: prev_active.ssid.clone(),
        signal: prev_active.signal,
        secured: prev_active.secured,
        active: true,
    });
    networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    networks
}

fn read_ethernet_status() -> Option<EthernetStatus> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "dev", "status"]);
    let output = run_command(&mut cmd)?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let fields = nmcli_split(line);
        if fields.len() < 4 || fields[1] != "ethernet" {
            continue;
        }
        let connected = fields[2].starts_with("connected");
        let label = if connected {
            let conn = fields[3].clone();
            if conn.is_empty() || conn == "--" {
                "Connected".to_string()
            } else {
                conn
            }
        } else {
            "Not connected".to_string()
        };
        return Some(EthernetStatus {
            present: true,
            connected,
            label,
        });
    }
    Some(EthernetStatus::default())
}

fn read_vpn_status() -> Option<Vec<VpnStatus>> {
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["-t", "-f", "NAME,UUID,TYPE,DEVICE", "connection", "show"]);
    let output = run_command(&mut cmd)?;
    let text = String::from_utf8_lossy(&output.stdout);

    let mut active = std::collections::HashSet::new();
    {
        let mut active_cmd = std::process::Command::new("nmcli");
        active_cmd.args([
            "-t",
            "-f",
            "NAME,UUID,TYPE",
            "connection",
            "show",
            "--active",
        ]);
        if let Some(out) = run_command(&mut active_cmd) {
            let active_text = String::from_utf8_lossy(&out.stdout);
            for line in active_text.lines() {
                let f = nmcli_split(line);
                if f.len() >= 3 && is_vpn_connection_type(&f[2]) {
                    active.insert(f[1].clone());
                }
            }
        }
    }

    let mut out = Vec::new();
    for line in text.lines() {
        let f = nmcli_split(line);
        if f.len() < 3 || f[0].is_empty() {
            continue;
        }
        if !is_vpn_connection_type(&f[2]) {
            continue;
        }
        let kind = vpn_kind_label(&f[2]);
        let is_active =
            active.contains(&f[1]) || (f.len() >= 4 && !f[3].is_empty() && f[3] != "--");
        out.push(VpnStatus {
            name: f[0].clone(),
            uuid: f[1].clone(),
            kind: kind.to_string(),
            active: is_active,
            pending: false,
        });
    }
    // Stamp pending from in-flight ops so the popover can show a spinner.
    if let Ok(st) = VPN_OP.lock() {
        if let Some((ref target, _, _)) = st.pending {
            for v in &mut out {
                if v.uuid == *target || v.name == *target {
                    v.pending = true;
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Some(out)
}

fn is_vpn_connection_type(ctype: &str) -> bool {
    let t = ctype.to_ascii_lowercase();
    t == "wireguard" || t == "vpn" || t.starts_with("vpn")
}

fn vpn_kind_label(ctype: &str) -> &'static str {
    let t = ctype.to_ascii_lowercase();
    if t == "wireguard" {
        "WireGuard"
    } else if t.contains("openvpn") {
        "OpenVPN"
    } else {
        "VPN"
    }
}

fn read_battery_percent() -> Option<u8> {
    let capacity = std::fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
        .or_else(|_| std::fs::read_to_string("/sys/class/power_supply/BAT1/capacity"))
        .ok()?;
    capacity.trim().parse().ok()
}

fn read_battery_charging() -> bool {
    let status = std::fs::read_to_string("/sys/class/power_supply/BAT0/status")
        .or_else(|_| std::fs::read_to_string("/sys/class/power_supply/BAT1/status"))
        .unwrap_or_default();
    status.trim().eq_ignore_ascii_case("charging")
        || status.trim().eq_ignore_ascii_case("fully charged")
}

fn read_bluetooth_status() -> BluetoothStatus {
    if !bluetooth_adapter_present() {
        return BluetoothStatus::default();
    }

    let mut cmd = std::process::Command::new("bluetoothctl");
    cmd.args(["show"]);
    let Some(output) = run_command(&mut cmd) else {
        return BluetoothStatus {
            adapter_present: true,
            ..BluetoothStatus::default()
        };
    };
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains("Controller") || text.contains("No default controller") {
        return BluetoothStatus::default();
    }
    let powered = text.contains("Powered: yes");
    let mut devices = Vec::new();
    if powered {
        let mut devices_cmd = std::process::Command::new("bluetoothctl");
        devices_cmd.args(["devices", "Connected"]);
        if let Some(o) = run_command(&mut devices_cmd) {
            // Enumerate UPower once per poll so each device is a cheap map lookup
            // rather than a fresh `upower -i` spawn.
            let upower = read_upower_bt_batteries();
            let dev_text = String::from_utf8_lossy(&o.stdout);
            for line in dev_text.lines() {
                if let Some(rest) = line.strip_prefix("Device ") {
                    let mut parts = rest.splitn(2, ' ');
                    let Some(address) = parts.next() else {
                        continue;
                    };
                    let name = parts.next().unwrap_or(address).to_string();
                    let battery = read_bluetooth_device_battery(address, &upower);
                    devices.push(BluetoothDevice {
                        address: address.to_string(),
                        name,
                        battery_percent: battery.percent,
                        battery_charging: battery.charging,
                    });
                }
            }
        }
        apply_solaar_overrides(&mut devices);
    }
    let device_name = devices.first().map(|d| d.name.clone());
    BluetoothStatus {
        adapter_present: true,
        powered,
        connected: !devices.is_empty(),
        device_name,
        devices,
    }
}

/// Battery reading for a single peripheral: charge level plus charging state
/// when the underlying source can report it.
#[derive(Debug, Clone, Copy, Default)]
struct DeviceBattery {
    /// Charge 0–100, or `None` when no source reports a level.
    percent: Option<u8>,
    /// `Some(true/false)` when charging is known, `None` when unknown.
    charging: Option<bool>,
}

/// Read a connected device's battery (and charging state, where available).
///
/// Source priority, most→least accurate:
/// 1. Kernel HID battery (`/sys/class/power_supply/hid-<mac>-battery`) — exposes
///    `capacity` and a `status` we can map to charging; no extra BlueZ config.
/// 2. UPower — aggregates peripheral batteries and reports a `state` (charging /
///    discharging / fully-charged) for devices/drivers that support it.
/// 3. BlueZ `Battery1` via `bluetoothctl info` ("Battery Percentage: 0xNN (NN)")
///    — percentage only; the BT Battery Service has no charging characteristic.
fn read_bluetooth_device_battery(
    address: &str,
    upower: &HashMap<String, DeviceBattery>,
) -> DeviceBattery {
    if let Some(batt) = read_hid_battery_for_address(address) {
        return batt;
    }
    if let Some(batt) = upower.get(&address.to_ascii_uppercase()) {
        if batt.percent.is_some() {
            return *batt;
        }
    }
    let mut cmd = std::process::Command::new("bluetoothctl");
    cmd.args(["info", address]);
    let percent = run_command(&mut cmd).and_then(|output| {
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().find_map(|line| {
            line.trim()
                .strip_prefix("Battery Percentage:")
                .and_then(parse_battery_percentage)
        })
    });
    DeviceBattery {
        percent,
        charging: None,
    }
}

/// Enumerate UPower peripheral batteries once, keyed by uppercased MAC.
///
/// UPower device paths embed the address as `…_dev_AA_BB_CC_DD_EE_FF`; we use
/// that to filter to Bluetooth/peripheral devices (skipping the laptop battery,
/// AC line, and DisplayDevice) and to map each back to its BlueZ address.
fn read_upower_bt_batteries() -> HashMap<String, DeviceBattery> {
    let mut map = HashMap::new();
    let mut enum_cmd = std::process::Command::new("upower");
    enum_cmd.arg("-e");
    let Some(output) = run_command(&mut enum_cmd) else {
        return map;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for path in text.lines() {
        let path = path.trim();
        let Some(idx) = path.find("_dev_") else {
            continue;
        };
        let mac = path[idx + "_dev_".len()..]
            .replace('_', ":")
            .to_ascii_uppercase();
        if !mac.contains(':') {
            continue;
        }
        if let Some(batt) = read_upower_device(path) {
            map.insert(mac, batt);
        }
    }
    map
}

/// Parse `percentage:`/`state:` out of `upower -i <path>` for one device.
fn read_upower_device(path: &str) -> Option<DeviceBattery> {
    let mut cmd = std::process::Command::new("upower");
    cmd.args(["-i", path]);
    let output = run_command(&mut cmd)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut batt = DeviceBattery::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("percentage:") {
            if let Ok(v) = rest.trim().trim_end_matches('%').parse::<f32>() {
                batt.percent = Some(v.round().clamp(0.0, 100.0) as u8);
            }
        } else if let Some(rest) = line.strip_prefix("state:") {
            batt.charging = match rest.trim() {
                "charging" | "fully-charged" => Some(true),
                "discharging" | "pending-discharge" => Some(false),
                // "unknown" / "pending-charge" carry no reliable signal.
                _ => None,
            };
        }
    }
    (batt.percent.is_some() || batt.charging.is_some()).then_some(batt)
}

/// Cached Solaar (`solaar show`) battery readings, keyed by lowercased device
/// name. Solaar talks Logitech HID++ directly and is the only source that knows
/// charging state for Logitech peripherals over Bluetooth — but `solaar show`
/// takes ~2s, so we cache it and refresh off the poll thread.
struct SolaarCache {
    fetched_at: Option<Instant>,
    refreshing: bool,
    by_name: HashMap<String, DeviceBattery>,
}

static SOLAAR_CACHE: OnceLock<Mutex<SolaarCache>> = OnceLock::new();

/// How long a cached Solaar reading stays fresh before a background refresh.
const SOLAAR_TTL: Duration = Duration::from_secs(20);

/// Overlay Solaar's charging state (and level when BlueZ/UPower lacked one) onto
/// connected devices, matched by name. Best-effort: a no-op if Solaar is absent.
fn apply_solaar_overrides(devices: &mut [BluetoothDevice]) {
    if devices.is_empty() {
        return;
    }
    let solaar = solaar_overrides();
    if solaar.is_empty() {
        return;
    }
    for dev in devices.iter_mut() {
        let key = dev.name.to_ascii_lowercase();
        let matched = solaar.iter().find(|(name, _)| {
            // Solaar's name ("Wireless Mobile Mouse MX Anywhere 2S") is usually a
            // superset of the BlueZ name ("MX Anywhere 2S"); accept either way.
            name.contains(&key) || key.contains(name.as_str())
        });
        if let Some((_, batt)) = matched {
            if batt.charging.is_some() {
                dev.battery_charging = batt.charging;
            }
            if dev.battery_percent.is_none() {
                dev.battery_percent = batt.percent;
            }
        }
    }
}

/// Return the cached Solaar map, kicking off a background refresh when stale.
fn solaar_overrides() -> HashMap<String, DeviceBattery> {
    let cache = SOLAAR_CACHE.get_or_init(|| {
        Mutex::new(SolaarCache {
            fetched_at: None,
            refreshing: false,
            by_name: HashMap::new(),
        })
    });
    let Ok(mut guard) = cache.lock() else {
        return HashMap::new();
    };
    let stale = guard.fetched_at.is_none_or(|at| at.elapsed() >= SOLAAR_TTL);
    if stale && !guard.refreshing {
        guard.refreshing = true;
        thread::spawn(move || {
            let map = read_solaar_batteries();
            if let Ok(mut guard) = cache.lock() {
                guard.by_name = map;
                guard.fetched_at = Some(Instant::now());
                guard.refreshing = false;
            }
        });
    }
    guard.by_name.clone()
}

/// Run `solaar show` and parse each device's battery line, keyed by lowercased
/// name. Returns empty if Solaar isn't installed or times out.
fn read_solaar_batteries() -> HashMap<String, DeviceBattery> {
    let mut map = HashMap::new();
    let mut cmd = std::process::Command::new("solaar");
    cmd.arg("show");
    let Some(output) = run_command_with_timeout(&mut cmd, Duration::from_secs(4)) else {
        return map;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current: Option<String> = None;
    for line in text.lines() {
        // Device blocks start with a column-0, non-empty header line.
        let is_header = !line.is_empty()
            && !line.starts_with(char::is_whitespace)
            && !line.starts_with("solaar version");
        if is_header {
            current = Some(line.trim().to_ascii_lowercase());
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("Battery:") {
            if let Some(name) = &current {
                map.insert(name.clone(), parse_solaar_battery(rest));
            }
        }
    }
    map
}

/// Parse a Solaar battery line body like ` N/A, full, next level 0%.` or
/// ` 90%, discharging, next level 50%.` into level + charging state.
fn parse_solaar_battery(rest: &str) -> DeviceBattery {
    let mut batt = DeviceBattery::default();
    let mut segs = rest.split(',');
    if let Some(level) = segs.next() {
        let level = level.trim().trim_end_matches('.');
        if let Some(pct) = level.strip_suffix('%') {
            if let Ok(v) = pct.trim().parse::<f32>() {
                batt.percent = Some(v.round().clamp(0.0, 100.0) as u8);
            }
        }
    }
    if let Some(status) = segs.next() {
        let status = status.trim().to_ascii_lowercase();
        // Order matters: "discharging" contains "charging".
        batt.charging = if status.contains("discharg") {
            Some(false)
        } else if status.contains("recharg") || status.contains("charging") || status == "full" {
            // HID++ reports "full" only while topped up on a charger.
            Some(true)
        } else {
            None
        };
    }
    batt
}

/// Parse a BlueZ battery field like `0x40 (64)` — prefer the decimal in parens,
/// falling back to decoding the `0xNN` hex value.
fn parse_battery_percentage(field: &str) -> Option<u8> {
    let field = field.trim();
    if let (Some(start), Some(end)) = (field.find('('), field.find(')')) {
        if start < end {
            if let Ok(v) = field[start + 1..end].trim().parse::<u16>() {
                return Some(v.min(100) as u8);
            }
        }
    }
    let token = field.split_whitespace().next()?;
    let hex = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))?;
    u16::from_str_radix(hex, 16).ok().map(|v| v.min(100) as u8)
}

/// Match a BlueZ MAC against kernel HID power-supply entries, whose names embed
/// the address as `hid-AA:BB:CC:DD:EE:FF-battery` (case-insensitive). Reads the
/// `capacity` and maps `status` (Charging/Full/Discharging) to a charging flag.
fn read_hid_battery_for_address(address: &str) -> Option<DeviceBattery> {
    let target = address.to_ascii_lowercase();
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_ascii_lowercase();
        if name.starts_with("hid-") && name.contains(&target) {
            let cap = std::fs::read_to_string(entry.path().join("capacity")).ok()?;
            let percent = cap.trim().parse::<u16>().ok().map(|v| v.min(100) as u8)?;
            let charging = match std::fs::read_to_string(entry.path().join("status")) {
                Ok(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "charging" | "full" => Some(true),
                    "discharging" | "not charging" => Some(false),
                    _ => None,
                },
                Err(_) => None,
            };
            return Some(DeviceBattery {
                percent: Some(percent),
                charging,
            });
        }
    }
    None
}

fn bluetooth_adapter_present() -> bool {
    std::fs::read_dir("/sys/class/bluetooth")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().starts_with("hci"))
        })
        .unwrap_or(false)
}

fn run_command(cmd: &mut std::process::Command) -> Option<std::process::Output> {
    run_command_with_timeout(cmd, Duration::from_millis(600))
}

fn run_command_with_timeout(
    cmd: &mut std::process::Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {}
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn read_volume_percent() -> Option<u8> {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["get-sink-volume", "@DEFAULT_SINK@"]);
    run_command(&mut cmd).and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout);
        text.split_whitespace()
            .nth(4)
            .and_then(|s| s.trim_end_matches('%').parse().ok())
    })
}

fn read_volume_muted() -> Option<bool> {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["get-sink-mute", "@DEFAULT_SINK@"]);
    run_command(&mut cmd).and_then(|o| parse_mute(&String::from_utf8_lossy(&o.stdout)))
}

fn parse_mute(text: &str) -> Option<bool> {
    if text.contains("yes") {
        Some(true)
    } else if text.contains("no") {
        Some(false)
    } else {
        None
    }
}

fn read_mic_percent() -> Option<u8> {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["get-source-volume", "@DEFAULT_SOURCE@"]);
    run_command(&mut cmd).and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout);
        text.split_whitespace()
            .nth(4)
            .and_then(|s| s.trim_end_matches('%').parse().ok())
    })
}

fn read_mic_muted() -> Option<bool> {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["get-source-mute", "@DEFAULT_SOURCE@"]);
    run_command(&mut cmd).and_then(|o| parse_mute(&String::from_utf8_lossy(&o.stdout)))
}

pub fn set_volume_relative(delta: i8) {
    queue_audio(AudioCommand::SetVolumeRelative(delta));
}

pub fn set_mic_volume_absolute(percent: u8) {
    queue_audio(AudioCommand::SetMicVolumeAbsolute(percent));
}

pub fn set_mic_mute(muted: bool) {
    queue_audio(AudioCommand::SetMicMute(muted));
}

pub fn set_volume_absolute(percent: u8) {
    queue_audio(AudioCommand::SetVolumeAbsolute(percent));
}

pub fn set_mute(muted: bool) {
    queue_audio(AudioCommand::SetMute(muted));
}

#[allow(dead_code)] // audio control helper
pub fn toggle_mute() {
    let _ = std::process::Command::new("pactl")
        .args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
        .status();
}

fn run_set_volume_relative(delta: i8) {
    let sign = if delta >= 0 { "+" } else { "" };
    let mut cmd = std::process::Command::new("pactl");
    cmd.args([
        "set-sink-volume",
        "@DEFAULT_SINK@",
        &format!("{sign}{delta}%"),
    ]);
    let _ = run_command(&mut cmd);
}

fn run_set_volume_absolute(percent: u8) {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args([
        "set-sink-volume",
        "@DEFAULT_SINK@",
        &format!("{}%", percent.min(100)),
    ]);
    let _ = run_command(&mut cmd);
}

fn run_set_mute(muted: bool) {
    let flag = if muted { "yes" } else { "no" };
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["set-sink-mute", "@DEFAULT_SINK@", flag]);
    let _ = run_command(&mut cmd);
}

fn run_set_mic_volume_absolute(percent: u8) {
    let mut cmd = std::process::Command::new("pactl");
    cmd.args([
        "set-source-volume",
        "@DEFAULT_SOURCE@",
        &format!("{}%", percent.min(100)),
    ]);
    let _ = run_command(&mut cmd);
}

fn run_set_mic_mute(muted: bool) {
    let flag = if muted { "yes" } else { "no" };
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(["set-source-mute", "@DEFAULT_SOURCE@", flag]);
    let _ = run_command(&mut cmd);
}
