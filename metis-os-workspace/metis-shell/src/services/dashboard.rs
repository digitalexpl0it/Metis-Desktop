//! Background system metrics for the pull-down dashboard.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use metis_config::load_dashboard_config;
use sysinfo::{Disks, ProcessesToUpdate, System};

static POLL_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_polling_active(active: bool) {
    POLL_ACTIVE.store(active, Ordering::Relaxed);
}

#[allow(dead_code)] // dashboard poll gate query
pub fn polling_active() -> bool {
    POLL_ACTIVE.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DashboardSnapshot {
    pub hardware: HardwareInfo,
    pub cpu_percent: f32,
    pub cpu_per_core: Vec<f32>,
    pub cpu_history: Vec<f32>,
    pub cpu_core_histories: Vec<Vec<f32>>,
    pub mem_percent_history: Vec<f32>,
    pub swap_percent_history: Vec<f32>,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub disks: Vec<DiskMount>,
    pub network_rx_bps: u64,
    pub network_tx_bps: u64,
    pub net_rx_history: Vec<f64>,
    pub net_tx_history: Vec<f64>,
    pub ethernet_rx_bps: u64,
    pub ethernet_tx_bps: u64,
    pub wifi_rx_bps: u64,
    pub wifi_tx_bps: u64,
    pub eth_rx_history: Vec<f64>,
    pub eth_tx_history: Vec<f64>,
    pub wifi_rx_history: Vec<f64>,
    pub wifi_tx_history: Vec<f64>,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub disk_read_history: Vec<f64>,
    pub disk_write_history: Vec<f64>,
    pub load_avg: [f64; 3],
    pub uptime_secs: u64,
    pub firewall: FirewallStatus,
    pub health: HealthSummary,
    pub processes: Vec<ProcessRow>,
    pub cpu_temp_celsius: Option<f32>,
    pub gpu_temps: Vec<GpuTempReading>,
    /// Battery charge 0–100 when a BAT* supply exists.
    pub battery_percent: Option<f32>,
    pub battery_charging: bool,
    pub battery_history: Vec<f32>,
    /// Recent journal lines (or empty when unavailable).
    pub log_lines: Vec<String>,
    pub logs_available: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpuTempReading {
    pub label: String,
    pub temp_celsius: f32,
    /// Discrete GPU utilization (0–100%), when the driver exposes it.
    pub util_percent: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HardwareInfo {
    pub hostname: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub kernel: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FirewallStatus {
    pub active: bool,
    pub backend: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HealthSummary {
    pub cpu: HealthLevel,
    pub memory: HealthLevel,
    pub disk: HealthLevel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HealthLevel {
    #[default]
    Good,
    Moderate,
    Critical,
}

impl HealthLevel {
    #[allow(dead_code)] // dashboard health badge API
    pub fn label(self) -> &'static str {
        match self {
            HealthLevel::Good => "Healthy",
            HealthLevel::Moderate => "Elevated",
            HealthLevel::Critical => "High",
        }
    }

    #[allow(dead_code)] // dashboard health badge API
    pub fn css_class(self) -> &'static str {
        match self {
            HealthLevel::Good => "metis-dash-health-good",
            HealthLevel::Moderate => "metis-dash-health-warn",
            HealthLevel::Critical => "metis-dash-health-crit",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskMount {
    pub mount_point: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessClass {
    UserApp,
    System,
    Metis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub user: String,
    pub uid: u32,
    pub class: ProcessClass,
    pub killable: bool,
}

pub fn spawn_dashboard_pollers() -> Receiver<DashboardSnapshot> {
    set_polling_active(true);
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("metis-dashboard-poll".into())
        .spawn(move || poll_loop(tx))
        .ok();
    rx
}

fn poll_loop(tx: mpsc::Sender<DashboardSnapshot>) {
    let mut sys = System::new();
    sys.refresh_memory();
    let hardware = HardwareInfo::load();
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(false);
    let mut cpu_history: Vec<f32> = Vec::with_capacity(90);
    let mut cpu_core_histories: Vec<Vec<f32>> = Vec::new();
    let mut mem_history: Vec<f32> = Vec::with_capacity(90);
    let mut swap_history: Vec<f32> = Vec::with_capacity(90);
    let mut net_rx_history: Vec<f64> = Vec::with_capacity(90);
    let mut net_tx_history: Vec<f64> = Vec::with_capacity(90);
    let mut eth_rx_history: Vec<f64> = Vec::with_capacity(90);
    let mut eth_tx_history: Vec<f64> = Vec::with_capacity(90);
    let mut wifi_rx_history: Vec<f64> = Vec::with_capacity(90);
    let mut wifi_tx_history: Vec<f64> = Vec::with_capacity(90);
    let mut disk_read_history: Vec<f64> = Vec::with_capacity(90);
    let mut disk_write_history: Vec<f64> = Vec::with_capacity(90);
    let mut battery_history: Vec<f32> = Vec::with_capacity(90);
    let mut log_lines: Vec<String> = Vec::new();
    let mut logs_available = false;
    let mut last_net = read_net_breakdown();
    let mut last_disk_io = read_disk_io_sectors();
    let mut last_io_at = Instant::now();
    let mut last_net_at = Instant::now();
    let mut last_sent = DashboardSnapshot::default();
    let mut firewall_cache = FirewallStatus::default();
    let mut firewall_at = Instant::now() - Duration::from_secs(60);
    let mut process_tick: u32 = 0;
    let mut log_tick: u32 = 0;

    loop {
        let cfg = load_dashboard_config();
        let interval = Duration::from_millis(cfg.refresh_interval_ms as u64);
        if !cfg.enabled || !POLL_ACTIVE.load(Ordering::Relaxed) {
            thread::sleep(interval);
            continue;
        }

        sys.refresh_cpu_usage();
        sys.refresh_memory();
        disks.refresh(false);
        // Full process enumeration is expensive; refresh it every third poll
        // (~3 s at the default 1 s interval). CPU/memory/network stay live.
        process_tick = process_tick.wrapping_add(1);
        let refresh_processes = process_tick.is_multiple_of(3);
        if refresh_processes {
            sys.refresh_processes(ProcessesToUpdate::All, true);
        }

        let cpu_percent = sys.global_cpu_usage();
        let cpu_per_core: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        push_history_f32(&mut cpu_history, cpu_percent, 90);
        sync_core_histories(&mut cpu_core_histories, &cpu_per_core);

        let mem_pct = if sys.total_memory() > 0 {
            (sys.used_memory() as f64 / sys.total_memory() as f64 * 100.0) as f32
        } else {
            0.0
        };
        push_history_f32(&mut mem_history, mem_pct, 90);

        let swap_pct = if sys.total_swap() > 0 {
            (sys.used_swap() as f64 / sys.total_swap() as f64 * 100.0) as f32
        } else {
            0.0
        };
        push_history_f32(&mut swap_history, swap_pct, 90);

        let now = Instant::now();
        let net = read_net_breakdown();
        let elapsed = now.duration_since(last_net_at).as_secs_f64().max(0.001);
        let (
            network_rx_bps,
            network_tx_bps,
            ethernet_rx_bps,
            ethernet_tx_bps,
            wifi_rx_bps,
            wifi_tx_bps,
        ) = if let (Some(prev), Some(cur)) = (&last_net, &net) {
            let total_rx = rate_bytes(cur.total_rx, prev.total_rx, elapsed);
            let total_tx = rate_bytes(cur.total_tx, prev.total_tx, elapsed);
            (
                total_rx,
                total_tx,
                rate_bytes(cur.eth_rx, prev.eth_rx, elapsed),
                rate_bytes(cur.eth_tx, prev.eth_tx, elapsed),
                rate_bytes(cur.wifi_rx, prev.wifi_rx, elapsed),
                rate_bytes(cur.wifi_tx, prev.wifi_tx, elapsed),
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };
        last_net = net;
        last_net_at = now;
        push_history_f64(&mut net_rx_history, network_rx_bps as f64, 90);
        push_history_f64(&mut net_tx_history, network_tx_bps as f64, 90);
        push_history_f64(&mut eth_rx_history, ethernet_rx_bps as f64, 90);
        push_history_f64(&mut eth_tx_history, ethernet_tx_bps as f64, 90);
        push_history_f64(&mut wifi_rx_history, wifi_rx_bps as f64, 90);
        push_history_f64(&mut wifi_tx_history, wifi_tx_bps as f64, 90);

        let io_elapsed = now.duration_since(last_io_at).as_secs_f64().max(0.001);
        let disk_io = read_disk_io_sectors();
        let (disk_read_bps, disk_write_bps) =
            if let (Some(prev), Some(cur)) = (&last_disk_io, &disk_io) {
                (
                    rate_bytes(cur.0, prev.0, io_elapsed) * 512,
                    rate_bytes(cur.1, prev.1, io_elapsed) * 512,
                )
            } else {
                (0, 0)
            };
        last_disk_io = disk_io;
        last_io_at = now;
        push_history_f64(&mut disk_read_history, disk_read_bps as f64, 90);
        push_history_f64(&mut disk_write_history, disk_write_bps as f64, 90);

        if now.duration_since(firewall_at) > Duration::from_secs(8) {
            firewall_cache = read_firewall_status();
            firewall_at = now;
        }

        let (battery_percent, battery_charging) = match read_battery_sample() {
            Some((pct, charging)) => {
                push_history_f32(&mut battery_history, pct, 90);
                (Some(pct), charging)
            }
            None => {
                battery_history.clear();
                (None, false)
            }
        };

        // Journal is slower / more expensive — refresh about every 4 s at 1 Hz.
        log_tick = log_tick.wrapping_add(1);
        if log_tick % 4 == 1 {
            let (lines, ok) = read_journal_tail(40);
            log_lines = lines;
            logs_available = ok;
        }

        let disk_mounts = collect_disks(&disks);
        let health = compute_health(cpu_percent, mem_pct, &disk_mounts);

        let snapshot = DashboardSnapshot {
            hardware: hardware.clone(),
            cpu_percent,
            cpu_per_core,
            cpu_history: cpu_history.clone(),
            cpu_core_histories: cpu_core_histories.clone(),
            mem_percent_history: mem_history.clone(),
            swap_percent_history: swap_history.clone(),
            memory_used_bytes: sys.used_memory(),
            memory_total_bytes: sys.total_memory(),
            swap_used_bytes: sys.used_swap(),
            swap_total_bytes: sys.total_swap(),
            disks: disk_mounts,
            network_rx_bps,
            network_tx_bps,
            net_rx_history: net_rx_history.clone(),
            net_tx_history: net_tx_history.clone(),
            ethernet_rx_bps,
            ethernet_tx_bps,
            wifi_rx_bps,
            wifi_tx_bps,
            eth_rx_history: eth_rx_history.clone(),
            eth_tx_history: eth_tx_history.clone(),
            wifi_rx_history: wifi_rx_history.clone(),
            wifi_tx_history: wifi_tx_history.clone(),
            disk_read_bps,
            disk_write_bps,
            disk_read_history: disk_read_history.clone(),
            disk_write_history: disk_write_history.clone(),
            load_avg: read_load_avg(),
            uptime_secs: read_uptime_secs(),
            firewall: firewall_cache.clone(),
            health,
            processes: if refresh_processes {
                collect_processes(&sys)
            } else {
                last_sent.processes.clone()
            },
            cpu_temp_celsius: read_cpu_temp_celsius(),
            gpu_temps: read_gpu_temps(),
            battery_percent,
            battery_charging,
            battery_history: battery_history.clone(),
            log_lines: log_lines.clone(),
            logs_available,
        };

        if snapshot != last_sent {
            last_sent = snapshot.clone();
            let _ = tx.send(snapshot);
        }

        thread::sleep(interval);
    }
}

impl HardwareInfo {
    fn load() -> Self {
        Self {
            hostname: read_hostname(),
            cpu_model: read_cpu_model(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            kernel: read_kernel(),
        }
    }
}

fn compute_health(cpu: f32, mem_pct: f32, disks: &[DiskMount]) -> HealthSummary {
    let disk_pct = disks
        .iter()
        .filter(|d| d.mount_point == "/")
        .map(|d| pct(d.used_bytes, d.total_bytes))
        .next()
        .or_else(|| {
            disks
                .iter()
                .map(|d| pct(d.used_bytes, d.total_bytes))
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        })
        .unwrap_or(0.0) as f32;

    HealthSummary {
        cpu: level_from_pct(cpu, 70.0, 90.0),
        memory: level_from_pct(mem_pct, 75.0, 92.0),
        disk: level_from_pct(disk_pct, 80.0, 95.0),
    }
}

fn level_from_pct(value: f32, warn: f32, crit: f32) -> HealthLevel {
    if value >= crit {
        HealthLevel::Critical
    } else if value >= warn {
        HealthLevel::Moderate
    } else {
        HealthLevel::Good
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

fn sync_core_histories(store: &mut Vec<Vec<f32>>, current: &[f32]) {
    while store.len() < current.len() {
        store.push(Vec::with_capacity(90));
    }
    store.truncate(current.len());
    for (hist, &value) in store.iter_mut().zip(current) {
        push_history_f32(hist, value, 90);
    }
}

fn push_history_f32(history: &mut Vec<f32>, value: f32, cap: usize) {
    history.push(value);
    if history.len() > cap {
        history.drain(0..history.len() - cap);
    }
}

fn push_history_f64(history: &mut Vec<f64>, value: f64, cap: usize) {
    history.push(value);
    if history.len() > cap {
        history.drain(0..history.len() - cap);
    }
}

fn read_battery_sample() -> Option<(f32, bool)> {
    let capacity = fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
        .or_else(|_| fs::read_to_string("/sys/class/power_supply/BAT1/capacity"))
        .ok()?;
    let pct: f32 = capacity.trim().parse::<u8>().ok()? as f32;
    let status = fs::read_to_string("/sys/class/power_supply/BAT0/status")
        .or_else(|_| fs::read_to_string("/sys/class/power_supply/BAT1/status"))
        .unwrap_or_default();
    let charging = status.trim().eq_ignore_ascii_case("charging")
        || status.trim().eq_ignore_ascii_case("fully charged");
    Some((pct, charging))
}

/// Last `n` journal lines. Returns `(lines, available)`.
fn read_journal_tail(n: usize) -> (Vec<String>, bool) {
    let n = n.clamp(1, 200);
    let output = Command::new("journalctl")
        .args([
            "-n",
            &n.to_string(),
            "--no-pager",
            "-o",
            "short-iso",
            "--user",
        ])
        .output();
    let Ok(out) = output else {
        return (Vec::new(), false);
    };
    if !out.status.success() {
        // Retry without --user for system journal access.
        let output = Command::new("journalctl")
            .args(["-n", &n.to_string(), "--no-pager", "-o", "short-iso"])
            .output();
        let Ok(out) = output else {
            return (Vec::new(), false);
        };
        if !out.status.success() {
            return (Vec::new(), false);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        return (lines, true);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    (lines, true)
}

fn read_hostname() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "localhost".into())
}

fn read_cpu_model() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into())
}

fn read_kernel() -> String {
    fs::read_to_string("/proc/version")
        .ok()
        .and_then(|l| l.lines().next().map(str::to_string))
        .unwrap_or_else(|| "Linux".into())
}

fn read_uptime_secs() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn read_load_avg() -> [f64; 3] {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| {
            let mut parts = s.split_whitespace();
            let a = parts.next()?.parse().ok()?;
            let b = parts.next()?.parse().ok()?;
            let c = parts.next()?.parse().ok()?;
            Some([a, b, c])
        })
        .unwrap_or([0.0, 0.0, 0.0])
}

fn read_firewall_status() -> FirewallStatus {
    if let Ok(out) = std::process::Command::new("ufw").args(["status"]).output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let active = text.contains("Status: active");
        let summary = text
            .lines()
            .find(|l| l.starts_with("Status:"))
            .unwrap_or("UFW")
            .to_string();
        return FirewallStatus {
            active,
            backend: "ufw".into(),
            summary,
        };
    }
    if let Ok(out) = std::process::Command::new("systemctl")
        .args(["is-active", "firewalld"])
        .output()
    {
        let active = String::from_utf8_lossy(&out.stdout).trim() == "active";
        if active {
            return FirewallStatus {
                active: true,
                backend: "firewalld".into(),
                summary: "firewalld is active".into(),
            };
        }
    }
    if fs::metadata("/usr/sbin/nft").is_ok() || fs::metadata("/sbin/nft").is_ok() {
        return FirewallStatus {
            active: false,
            backend: "nftables".into(),
            summary: "No active firewall service detected (nftables present)".into(),
        };
    }
    FirewallStatus {
        active: false,
        backend: "none".into(),
        summary: "No firewall service detected".into(),
    }
}

fn collect_disks(disks: &Disks) -> Vec<DiskMount> {
    let mut out = Vec::new();
    for disk in disks.list() {
        let mount = disk.mount_point().to_string_lossy().into_owned();
        if mount.is_empty() {
            continue;
        }
        if mount.starts_with("/dev") || mount.starts_with("/sys") || mount.starts_with("/proc") {
            continue;
        }
        let total = disk.total_space();
        if total == 0 {
            continue;
        }
        let used = total.saturating_sub(disk.available_space());
        out.push(DiskMount {
            mount_point: mount,
            used_bytes: used,
            total_bytes: total,
        });
    }
    out.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    out.dedup_by(|a, b| a.mount_point == b.mount_point);
    out
}

fn collect_processes(sys: &System) -> Vec<ProcessRow> {
    let my_uid = current_uid();
    let own_pid = std::process::id();
    let cpu_count = sys.cpus().len().max(1);
    let mut rows: Vec<ProcessRow> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| {
            let name = proc_.name().to_string_lossy().into_owned();
            let uid = proc_.user_id().map(|u| **u);
            let uid = uid_for_pid(pid.as_u32(), uid);
            let user = username_for_uid(uid);
            let is_metis = name.contains("metis-")
                || name == "metis-compositor"
                || name == "metis-shell"
                || name == "metis-settings";
            let class = if is_metis {
                ProcessClass::Metis
            } else if uid == my_uid {
                ProcessClass::UserApp
            } else {
                ProcessClass::System
            };
            let killable = uid == my_uid && pid.as_u32() != own_pid;
            let parent_pid = proc_.parent().map(|p| p.as_u32());
            ProcessRow {
                pid: pid.as_u32(),
                parent_pid,
                name,
                cpu_percent: normalize_process_cpu(proc_.cpu_usage(), cpu_count),
                memory_bytes: proc_.memory(),
                user,
                uid,
                class,
                killable,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

/// sysinfo reports per-process CPU as 100% per core (multi-threaded jobs can exceed
/// 100%). Scale to a 0–100% share of total system capacity for the process list.
fn normalize_process_cpu(raw: f32, cpu_count: usize) -> f32 {
    let cores = cpu_count.max(1) as f32;
    (raw / cores).clamp(0.0, 100.0)
}

fn uid_for_pid(pid: u32, sysinfo_uid: Option<u32>) -> u32 {
    if let Some(uid) = sysinfo_uid.filter(|&u| u != u32::MAX) {
        return uid;
    }
    read_proc_uid(pid).unwrap_or(u32::MAX)
}

/// Real UID from `/proc/<pid>/status` when sysinfo cannot read process credentials.
fn read_proc_uid(pid: u32) -> Option<u32> {
    let text = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn username_for_uid(uid: u32) -> String {
    if uid == u32::MAX {
        return "?".into();
    }
    use nix::unistd::User;
    User::from_uid(uid.into())
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string())
}

fn current_uid() -> libc::uid_t {
    unsafe { libc::getuid() }
}

#[derive(Debug, Clone, Copy, Default)]
struct NetBreakdown {
    total_rx: u64,
    total_tx: u64,
    eth_rx: u64,
    eth_tx: u64,
    wifi_rx: u64,
    wifi_tx: u64,
}

fn rate_bytes(cur: u64, prev: u64, elapsed: f64) -> u64 {
    (cur.saturating_sub(prev) as f64 / elapsed) as u64
}

fn is_wifi_iface(name: &str) -> bool {
    name.starts_with("wl") || name.starts_with("wlan")
}

fn is_ethernet_iface(name: &str) -> bool {
    name.starts_with("eth") || name.starts_with("en")
}

fn read_net_breakdown() -> Option<NetBreakdown> {
    let text = fs::read_to_string("/proc/net/dev").ok()?;
    let mut out = NetBreakdown::default();
    for line in text.lines().skip(2) {
        let mut parts = line.split_whitespace();
        let iface = parts.next()?;
        let iface = iface.trim_end_matches(':');
        if iface == "lo" {
            continue;
        }
        let r: u64 = parts.next()?.parse().ok()?;
        let _ = parts.next()?;
        let _ = parts.next()?;
        let _ = parts.next()?;
        let _ = parts.next()?;
        let _ = parts.next()?;
        let _ = parts.next()?;
        let t: u64 = parts.next()?.parse().ok()?;
        out.total_rx = out.total_rx.saturating_add(r);
        out.total_tx = out.total_tx.saturating_add(t);
        if is_wifi_iface(iface) {
            out.wifi_rx = out.wifi_rx.saturating_add(r);
            out.wifi_tx = out.wifi_tx.saturating_add(t);
        } else if is_ethernet_iface(iface) {
            out.eth_rx = out.eth_rx.saturating_add(r);
            out.eth_tx = out.eth_tx.saturating_add(t);
        }
    }
    Some(out)
}

/// Aggregate read/write sectors across physical block devices.
fn read_disk_io_sectors() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/diskstats").ok()?;
    let mut read_sectors = 0u64;
    let mut write_sectors = 0u64;
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 14 {
            continue;
        }
        let name = parts[2];
        if name.starts_with("loop") || name.starts_with("ram") {
            continue;
        }
        if let (Ok(r), Ok(w)) = (parts[5].parse::<u64>(), parts[9].parse::<u64>()) {
            read_sectors = read_sectors.saturating_add(r);
            write_sectors = write_sectors.saturating_add(w);
        }
    }
    Some((read_sectors, write_sectors))
}

fn read_cpu_temp_celsius() -> Option<f32> {
    read_hwmon_temp_matching(|name| {
        let n = name.to_lowercase();
        n.contains("k10temp")
            || n.contains("coretemp")
            || n.contains("zenpower")
            || n.contains("cpu_thermal")
            || n.contains("acpitz")
            || n == "pch_cannonlake"
    })
    .or_else(|| {
        read_thermal_zone_temp_matching(|zone_type| {
            let t = zone_type.to_lowercase();
            t.contains("x86_pkg_temp")
                || t.contains("acpitz")
                || t.contains("cpu")
                || t.contains("soc_thermal")
        })
    })
}

fn read_gpu_temps() -> Vec<GpuTempReading> {
    let mut readings = Vec::new();
    collect_drm_discrete_gpu_temps(&mut readings);
    collect_standalone_discrete_hwmon_temps(&mut readings);
    collect_nvidia_smi_temps(&mut readings);
    readings
}

fn collect_drm_discrete_gpu_temps(out: &mut Vec<GpuTempReading>) {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return;
    };
    let mut cards: Vec<_> = dir
        .flatten()
        .filter(|entry| drm_card_name(&entry.file_name().to_string_lossy()))
        .collect();
    cards.sort_by_key(|entry| entry.file_name());

    for entry in cards {
        let device = entry.path().join("device");
        if is_integrated_gpu_device(&device) {
            continue;
        }
        let label = drm_gpu_label(&device);
        let hwmon_dir = device.join("hwmon");
        let Ok(hwmons) = fs::read_dir(hwmon_dir) else {
            continue;
        };
        for hwmon in hwmons.flatten() {
            if let Some(temp) = read_hwmon_highest_temp(&hwmon.path()) {
                let util = read_gpu_busy_percent(&device);
                push_gpu_temp(out, label, temp, util);
                break;
            }
        }
    }
}

fn collect_standalone_discrete_hwmon_temps(out: &mut Vec<GpuTempReading>) {
    let Ok(dir) = fs::read_dir("/sys/class/hwmon") else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if hwmon_under_drm_device(&path) {
            continue;
        }
        let name = fs::read_to_string(path.join("name"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !hwmon_is_discrete_gpu(&name) {
            continue;
        }
        if let Some(device) = hwmon_pci_device(&path)
            && is_integrated_gpu_device(&device)
        {
            continue;
        }
        if let Some(temp) = read_hwmon_highest_temp(&path) {
            let label = discrete_hwmon_label(&name);
            let util = hwmon_pci_device(&path)
                .as_deref()
                .and_then(read_gpu_busy_percent);
            push_gpu_temp(out, label, temp, util);
        }
    }
}

/// Proprietary NVIDIA on many laptops exposes no `hwmon` — query `nvidia-smi` instead.
fn collect_nvidia_smi_temps(out: &mut Vec<GpuTempReading>) {
    if !nvidia_discrete_present() {
        return;
    }
    if out
        .iter()
        .any(|r| r.label.to_lowercase().contains("nvidia"))
    {
        return;
    }

    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,temperature.gpu,utilization.gpu,name",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut parts = line.split(',').map(str::trim);
        let _index = parts.next();
        let Some(temp_str) = parts.next() else {
            continue;
        };
        let Some(util_str) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let Ok(temp) = temp_str.parse::<f32>() else {
            continue;
        };
        let util = util_str
            .trim_end_matches('%')
            .parse::<f32>()
            .ok()
            .filter(|v| (0.0..=100.0).contains(v));
        if !(1.0..150.0).contains(&temp) {
            continue;
        }
        push_gpu_temp(out, shorten_nvidia_name(name), temp, util);
    }
}

fn read_gpu_busy_percent(device: &Path) -> Option<f32> {
    let text = fs::read_to_string(device.join("gpu_busy_percent")).ok()?;
    let value = text.trim().parse::<f32>().ok()?;
    (0.0..=100.0).contains(&value).then_some(value)
}

fn push_gpu_temp(
    out: &mut Vec<GpuTempReading>,
    label: String,
    temp_celsius: f32,
    util_percent: Option<f32>,
) {
    if out.iter().any(|r| r.label == label) {
        return;
    }
    out.push(GpuTempReading {
        label,
        temp_celsius,
        util_percent,
    });
}

fn is_integrated_gpu_device(device: &Path) -> bool {
    let Some(vendor) = read_pci_id(device.join("vendor")) else {
        return false;
    };

    // Intel iGPU
    if vendor == 0x8086 {
        return true;
    }

    // AMD: primary/boot VGA is typically the APU graphics block.
    if vendor == 0x1002 {
        return read_boot_vga(device);
    }

    false
}

fn read_pci_id(path: PathBuf) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

fn read_boot_vga(device: &Path) -> bool {
    fs::read_to_string(device.join("boot_vga"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn nvidia_discrete_present() -> bool {
    if fs::read_dir("/proc/driver/nvidia/gpus")
        .map(|entries| entries.flatten().next().is_some())
        .unwrap_or(false)
    {
        return true;
    }
    drm_has_vendor(0x10de)
}

fn drm_has_vendor(vendor_id: u32) -> bool {
    let Ok(dir) = fs::read_dir("/sys/class/drm") else {
        return false;
    };
    dir.flatten().any(|entry| {
        if !drm_card_name(&entry.file_name().to_string_lossy()) {
            return false;
        }
        read_pci_id(entry.path().join("device/vendor")) == Some(vendor_id)
    })
}

fn hwmon_pci_device(hwmon: &Path) -> Option<PathBuf> {
    let mut cursor = hwmon.canonicalize().ok()?;
    loop {
        if cursor.join("vendor").is_file() && cursor.join("class").is_file() {
            return Some(cursor);
        }
        if !cursor.pop() {
            break;
        }
    }
    None
}

fn discrete_hwmon_label(name: &str) -> String {
    let n = name.to_lowercase();
    if n.contains("nvidia") {
        "NVIDIA GPU".to_string()
    } else if n.contains("amdgpu") {
        "AMD GPU".to_string()
    } else if n.contains("nouveau") {
        "NVIDIA GPU".to_string()
    } else if n.contains("radeon") {
        "AMD GPU".to_string()
    } else {
        name.to_string()
    }
}

fn shorten_nvidia_name(name: &str) -> String {
    name.trim()
        .strip_prefix("NVIDIA ")
        .unwrap_or(name)
        .to_string()
}

fn read_nvidia_proc_model(device: &Path) -> Option<String> {
    if read_pci_id(device.join("vendor"))? != 0x10de {
        return None;
    }
    let slot = device.file_name()?.to_string_lossy().to_string();
    let info_path = format!("/proc/driver/nvidia/gpus/{slot}/information");
    let model = fs::read_to_string(info_path).ok()?;
    for line in model.lines() {
        if let Some(rest) = line.strip_prefix("Model:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(shorten_nvidia_name(name));
            }
        }
    }
    None
}

fn drm_card_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("card") else {
        return false;
    };
    !rest.is_empty() && !rest.contains('-') && rest.chars().all(|c| c.is_ascii_digit())
}

fn drm_gpu_label(device: &Path) -> String {
    if let Some(model) = read_nvidia_proc_model(device) {
        return model;
    }
    if let Ok(product) = fs::read_to_string(device.join("product")) {
        let product = product.trim();
        if !product.is_empty() && !product.eq_ignore_ascii_case("unknown") {
            return product.to_string();
        }
    }
    match read_pci_id(device.join("vendor")) {
        Some(0x10de) => "NVIDIA GPU".to_string(),
        Some(0x1002) => "AMD GPU".to_string(),
        _ => "GPU".to_string(),
    }
}

fn hwmon_under_drm_device(hwmon: &Path) -> bool {
    hwmon
        .canonicalize()
        .ok()
        .is_some_and(|path| path.to_string_lossy().contains("/drm/card"))
}

fn hwmon_is_discrete_gpu(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("amdgpu") || n.contains("nvidia") || n.contains("nouveau") || n.contains("radeon")
}

fn read_hwmon_temp_matching<F>(pred: F) -> Option<f32>
where
    F: Fn(&str) -> bool,
{
    let dir = fs::read_dir("/sys/class/hwmon").ok()?;
    let mut best = None;
    for entry in dir.flatten() {
        let path = entry.path();
        let name = fs::read_to_string(path.join("name"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !pred(&name) {
            continue;
        }
        if let Some(temp) = read_hwmon_highest_temp(&path) {
            best = Some(best.map(|b: f32| b.max(temp)).unwrap_or(temp));
        }
    }
    best
}

fn read_hwmon_highest_temp(hwmon: &std::path::Path) -> Option<f32> {
    let mut best = None;
    for i in 1..=12 {
        let path = hwmon.join(format!("temp{i}_input"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(milli) = text.trim().parse::<i64>() else {
            continue;
        };
        if !(1..150_000).contains(&milli) {
            continue;
        }
        let c = milli as f32 / 1000.0;
        best = Some(best.map(|b: f32| b.max(c)).unwrap_or(c));
    }
    best
}

fn read_thermal_zone_temp_matching<F>(pred: F) -> Option<f32>
where
    F: Fn(&str) -> bool,
{
    let dir = fs::read_dir("/sys/class/thermal").ok()?;
    let mut best = None;
    for entry in dir.flatten() {
        let path = entry.path();
        let zone_type = fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !pred(&zone_type) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path.join("temp")) else {
            continue;
        };
        let Ok(milli) = text.trim().parse::<i64>() else {
            continue;
        };
        if !(1..150_000).contains(&milli) {
            continue;
        }
        let c = milli as f32 / 1000.0;
        best = Some(best.map(|b: f32| b.max(c)).unwrap_or(c));
    }
    best
}

pub fn short_kernel_version(full: &str) -> String {
    full.strip_prefix("Linux version ")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or(full)
        .to_string()
}

pub fn kill_process(pid: u32, force: bool) -> Result<(), String> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid as NixPid;

    if pid == std::process::id() {
        return Err("Refusing to kill the shell".into());
    }
    let sig = if force {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    kill(NixPid::from_raw(pid as i32), sig).map_err(|err| format!("kill: {err}"))
}

/// Signal `root` and every descendant listed in `all` (children first), skipping
/// non-killable PIDs and the shell itself. Best-effort: collects the first error
/// after attempting the rest.
pub fn kill_process_tree(root: u32, all: &[ProcessRow], force: bool) -> Result<(), String> {
    let by_parent = process_children_index(all);
    let mut order = Vec::new();
    collect_descendants_postorder(root, &by_parent, &mut order);
    order.push(root);

    let own = std::process::id();
    let killable: std::collections::HashMap<u32, bool> =
        all.iter().map(|p| (p.pid, p.killable)).collect();
    let mut first_err: Option<String> = None;
    for pid in order {
        if pid == own {
            continue;
        }
        if !killable.get(&pid).copied().unwrap_or(false) {
            continue;
        }
        if let Err(err) = kill_process(pid, force)
            && first_err.is_none()
        {
            first_err = Some(err);
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn process_children_index(all: &[ProcessRow]) -> std::collections::HashMap<u32, Vec<u32>> {
    let pids: std::collections::HashSet<u32> = all.iter().map(|p| p.pid).collect();
    let mut map: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for proc in all {
        if let Some(ppid) = proc.parent_pid
            && pids.contains(&ppid)
        {
            map.entry(ppid).or_default().push(proc.pid);
        }
    }
    map
}

fn collect_descendants_postorder(
    pid: u32,
    by_parent: &std::collections::HashMap<u32, Vec<u32>>,
    out: &mut Vec<u32>,
) {
    if let Some(children) = by_parent.get(&pid) {
        for child in children {
            collect_descendants_postorder(*child, by_parent, out);
            out.push(*child);
        }
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_rate(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}
