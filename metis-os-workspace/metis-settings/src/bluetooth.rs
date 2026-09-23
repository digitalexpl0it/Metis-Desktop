//! Bluetooth adapter and device management via `bluetoothctl` (BlueZ).

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Connected,
    Paired,
    Available,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BtDevice {
    pub address: String,
    pub name: String,
    pub state: DeviceState,
    pub battery_percent: Option<u8>,
    /// Charging state when a source reports it (kernel HID `status` / UPower
    /// `state`); `None` when unknown (typical for plain Bluetooth devices).
    pub battery_charging: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BluetoothSnapshot {
    pub adapter_present: bool,
    pub powered: bool,
    pub discovering: bool,
    pub adapter_name: String,
    pub devices: Vec<BtDevice>,
}

pub fn load_snapshot() -> BluetoothSnapshot {
    let show = btctl(&["show"]);
    let adapter_present = show.contains("Controller") && !show.contains("No default controller");
    if !adapter_present {
        return BluetoothSnapshot::default();
    }
    let powered = show.contains("Powered: yes");
    let discovering = show.contains("Discovering: yes");
    let adapter_name = parse_field(&show, "Alias:").unwrap_or_else(|| "Bluetooth".into());
    let devices = list_devices();
    BluetoothSnapshot {
        adapter_present: true,
        powered,
        discovering,
        adapter_name,
        devices,
    }
}

pub fn set_powered(on: bool) {
    let _ = btctl(&["power", if on { "on" } else { "off" }]);
}

/// Fire-and-forget adapter power toggle (never blocks the GTK main thread).
pub fn set_powered_async(on: bool) {
    std::thread::spawn(move || set_powered(on));
}

pub fn start_scan() {
    let _ = btctl(&["scan", "on"]);
}

pub fn stop_scan() {
    let _ = btctl(&["scan", "off"]);
}

pub fn pair_and_connect(address: &str) {
    let _ = btctl(&["pair", address]);
    let _ = btctl(&["trust", address]);
    let _ = btctl(&["connect", address]);
}

pub fn disconnect(address: &str) {
    let _ = btctl(&["disconnect", address]);
}

pub fn remove_device(address: &str) {
    let _ = btctl(&["remove", address]);
}

fn list_devices() -> Vec<BtDevice> {
    let text = btctl(&["devices"]);
    let upower = read_upower_bt_batteries();
    let solaar = read_solaar_batteries();
    let mut devices = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("Device ") else {
            continue;
        };
        let mut parts = rest.splitn(2, ' ');
        let address = parts.next().unwrap_or("").to_string();
        let name = parts.next().unwrap_or(&address).to_string();
        if address.is_empty() {
            continue;
        }
        let info = btctl(&["info", &address]);
        let state = if info.contains("Connected: yes") {
            DeviceState::Connected
        } else if info.contains("Paired: yes") {
            DeviceState::Paired
        } else {
            DeviceState::Available
        };
        // Source priority: kernel HID battery → UPower → BlueZ Battery1. The
        // first two can also report charging; BlueZ's percentage cannot.
        let (mut battery_percent, mut battery_charging) = read_hid_battery_for_address(&address)
            .or_else(|| upower.get(&address.to_ascii_uppercase()).copied())
            .filter(|b| b.percent.is_some())
            .map(|b| (b.percent, b.charging))
            .unwrap_or_else(|| {
                let pct = parse_field(&info, "Battery Percentage:")
                    .and_then(|s| parse_battery_percentage(&s));
                (pct, None)
            });
        // Solaar (Logitech HID++) is the only source that knows charging state
        // for Logitech devices over Bluetooth; overlay it by name match.
        if let Some(sb) = match_solaar(&solaar, &name) {
            if sb.charging.is_some() {
                battery_charging = sb.charging;
            }
            if battery_percent.is_none() {
                battery_percent = sb.percent;
            }
        }
        devices.push(BtDevice {
            address,
            name,
            state,
            battery_percent,
            battery_charging,
        });
    }
    devices.sort_by(|a, b| {
        fn rank(s: &DeviceState) -> u8 {
            match s {
                DeviceState::Connected => 0,
                DeviceState::Paired => 1,
                DeviceState::Available => 2,
            }
        }
        rank(&a.state)
            .cmp(&rank(&b.state))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices
}

/// Battery level plus charging state when the source can report it.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceBattery {
    pub percent: Option<u8>,
    pub charging: Option<bool>,
}

/// Kernel HID battery (`/sys/class/power_supply/hid-<mac>-battery`): reads
/// `capacity` and maps `status` to a charging flag. Most accurate when present.
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

/// Enumerate UPower peripheral batteries once, keyed by uppercased MAC parsed
/// from the `…_dev_AA_BB_…` device path.
fn read_upower_bt_batteries() -> HashMap<String, DeviceBattery> {
    let mut map = HashMap::new();
    let listing = upower(&["-e"]);
    for path in listing.lines() {
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

/// Parse `percentage:`/`state:` from `upower -i <path>`.
fn read_upower_device(path: &str) -> Option<DeviceBattery> {
    let text = upower(&["-i", path]);
    if text.is_empty() {
        return None;
    }
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
                _ => None,
            };
        }
    }
    (batt.percent.is_some() || batt.charging.is_some()).then_some(batt)
}

/// Find the Solaar reading whose name matches a BlueZ device name (either is a
/// superset of the other, e.g. "Wireless Mobile Mouse MX Anywhere 2S" ⊇ "MX
/// Anywhere 2S").
fn match_solaar<'a>(
    solaar: &'a HashMap<String, DeviceBattery>,
    name: &str,
) -> Option<&'a DeviceBattery> {
    let key = name.to_ascii_lowercase();
    solaar
        .iter()
        .find(|(n, _)| n.contains(&key) || key.contains(n.as_str()))
        .map(|(_, b)| b)
}

/// Run `solaar show` and parse each device's battery line, keyed by lowercased
/// name. Empty when Solaar isn't installed.
fn read_solaar_batteries() -> HashMap<String, DeviceBattery> {
    let mut map = HashMap::new();
    let text = run_with_timeout("solaar", &["show"], Duration::from_secs(4));
    if text.is_empty() {
        return map;
    }
    let mut current: Option<String> = None;
    for line in text.lines() {
        let is_header = !line.is_empty()
            && !line.starts_with(char::is_whitespace)
            && !line.starts_with("solaar version");
        if is_header {
            current = Some(line.trim().to_ascii_lowercase());
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("Battery:")
            && let Some(name) = &current
        {
            map.insert(name.clone(), parse_solaar_battery(rest));
        }
    }
    map
}

/// Parse a Solaar battery line body like ` N/A, full, next level 0%.`.
fn parse_solaar_battery(rest: &str) -> DeviceBattery {
    let mut batt = DeviceBattery::default();
    let mut segs = rest.split(',');
    if let Some(level) = segs.next() {
        let level = level.trim().trim_end_matches('.');
        if let Some(pct) = level.strip_suffix('%')
            && let Ok(v) = pct.trim().parse::<f32>()
        {
            batt.percent = Some(v.round().clamp(0.0, 100.0) as u8);
        }
    }
    if let Some(status) = segs.next() {
        let status = status.trim().to_ascii_lowercase();
        // Order matters: "discharging" contains "charging".
        batt.charging = if status.contains("discharg") {
            Some(false)
        } else if status.contains("recharg") || status.contains("charging") || status == "full" {
            Some(true)
        } else {
            None
        };
    }
    batt
}

fn upower(args: &[&str]) -> String {
    run_with_timeout("upower", args, Duration::from_secs(2))
}

/// Spawn `bin args`, returning captured stdout or empty on failure/timeout.
fn run_with_timeout(bin: &str, args: &[&str], timeout: Duration) -> String {
    let mut cmd = Command::new(bin);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default();
            }
            Ok(None) => {}
            Err(_) => return String::new(),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Parse a BlueZ battery field like `0x40 (64)` — prefer the decimal in parens,
/// then fall back to decoding the `0xNN` hex value, then a bare integer.
fn parse_battery_percentage(field: &str) -> Option<u8> {
    let field = field.trim();
    if let (Some(start), Some(end)) = (field.find('('), field.find(')'))
        && start < end
        && let Ok(v) = field[start + 1..end].trim().parse::<u16>()
    {
        return Some(v.min(100) as u8);
    }
    let token = field.split_whitespace().next()?;
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        return u16::from_str_radix(hex, 16).ok().map(|v| v.min(100) as u8);
    }
    token
        .trim_end_matches('%')
        .parse::<u16>()
        .ok()
        .map(|v| v.min(100) as u8)
}

fn parse_field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find(|l| l.trim_start().starts_with(key))
        .and_then(|l| l.split_once(key).map(|(_, v)| v.trim().to_string()))
        .filter(|s| !s.is_empty())
}

fn btctl(args: &[&str]) -> String {
    let mut cmd = Command::new("bluetoothctl");
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default();
            }
            Ok(None) => {}
            Err(_) => return String::new(),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
