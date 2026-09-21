//! Settings → System → About — Metis version, components, author, and host OS.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::prelude::*;
use metis_i18n::tr;

use crate::ui;

const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
const AUTHOR: &str = "DigitlExpl0it";
const GITHUB_URL: &str = "https://github.com/digitalexpl0it/Metis-Desktop";

/// Binaries installed with the Metis session (order matches install scripts).
const METIS_COMPONENTS: &[(&str, &str)] = &[
    ("metis-compositor", "Compositor"),
    ("metis-shell", "Shell / edge bar"),
    ("metis-settings", "Settings"),
    ("metis-portal", "xdg-desktop-portal"),
    ("metis-polkit-agent", "PolicyKit agent"),
    ("metis-remote", "Remote helpers"),
    ("metis-viewer", "Remote viewer"),
    ("metis-screenshot", "Screenshot"),
    ("metis-gamingd", "Gaming daemon"),
    ("metis-session", "Session launcher"),
];

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("about");

    let product = product_version_from_cargo(CARGO_VERSION);
    let blurb = gtk::Label::new(Some(&tr(
        "Metis Desktop — a Wayland desktop environment built in Rust.",
    )));
    blurb.set_xalign(0.0);
    blurb.set_wrap(true);
    blurb.add_css_class("metis-settings-hint");
    content.append(&blurb);

    let (ver_card, ver_body) = ui::section_with_icon(&tr("Version"), "help-about-symbolic");
    ver_body.append(&info_row(
        &tr("Metis"),
        &format!("{product}  (crate {CARGO_VERSION})"),
    ));
    ver_body.append(&info_row(&tr("Author"), AUTHOR));
    let link_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    link_row.set_hexpand(true);
    let link_lbl = gtk::Label::new(Some(&tr("Source")));
    link_lbl.set_xalign(0.0);
    link_lbl.set_hexpand(true);
    let link_btn = gtk::LinkButton::with_label(GITHUB_URL, GITHUB_URL);
    link_btn.set_uri(GITHUB_URL);
    link_btn.set_halign(gtk::Align::End);
    link_row.append(&link_lbl);
    link_row.append(&link_btn);
    link_row.add_css_class("metis-settings-row");
    ver_body.append(&link_row);
    content.append(&ver_card);

    let (apps_card, apps_body) = ui::section_with_icon(
        &tr("Installed components"),
        "application-x-executable-symbolic",
    );
    for (bin, label) in METIS_COMPONENTS {
        let status = match resolve_component(bin) {
            Some(path) => format!("{} — {}", tr("Installed"), path.display()),
            None => tr("Not found"),
        };
        apps_body.append(&info_row(label, &status));
    }
    content.append(&apps_card);

    let host = collect_host_info();
    let (os_card, os_body) = ui::section_with_icon(&tr("System"), "computer-symbolic");
    os_body.append(&info_row(&tr("Operating system"), &host.distro));
    os_body.append(&info_row(&tr("Kernel"), &host.kernel));
    os_body.append(&info_row(&tr("Architecture"), &host.arch));
    os_body.append(&info_row(&tr("Hostname"), &host.hostname));
    os_body.append(&info_row(&tr("Processor"), &host.cpu));
    os_body.append(&info_row(&tr("CPU cores"), &host.cpu_cores));
    os_body.append(&info_row(&tr("Memory"), &host.memory));
    content.append(&os_card);

    scroller.upcast()
}

fn info_row(label: &str, value: &str) -> gtk::Box {
    let val = gtk::Label::new(Some(value));
    val.set_xalign(1.0);
    val.set_wrap(true);
    val.set_max_width_chars(42);
    val.set_selectable(true);
    val.add_css_class("metis-settings-hint");
    val.set_halign(gtk::Align::End);
    ui::row(label, &val)
}

/// Map Cargo SemVer `0.1.N` → product/GitHub `0.1.0.N` (see `sync-version.sh`).
fn product_version_from_cargo(cargo: &str) -> String {
    let (base, pre) = match cargo.split_once('-') {
        Some((b, p)) => (b, Some(p)),
        None => (cargo, None),
    };
    let parts: Vec<&str> = base.split('.').collect();
    let product = if parts.len() == 3 {
        format!("{}.{}.0.{}", parts[0], parts[1], parts[2])
    } else {
        base.to_string()
    };
    match pre {
        Some(p) => format!("{product}{p}"),
        None => product,
    }
}

fn resolve_component(name: &str) -> Option<PathBuf> {
    for dir in ["/usr/local/bin", "/usr/bin", "/usr/libexec"] {
        let path = Path::new(dir).join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    which(name)
}

fn which(name: &str) -> Option<PathBuf> {
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

struct HostInfo {
    distro: String,
    kernel: String,
    arch: String,
    hostname: String,
    cpu: String,
    cpu_cores: String,
    memory: String,
}

fn collect_host_info() -> HostInfo {
    let distro = os_release_pretty_name().unwrap_or_else(|| tr("Unknown"));
    let kernel = uname_field("-r").unwrap_or_else(|| tr("Unknown"));
    let arch = uname_field("-m").unwrap_or_else(|| tr("Unknown"));
    let hostname = fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| uname_field("-n"))
        .unwrap_or_else(|| tr("Unknown"));
    let cpu = cpu_model().unwrap_or_else(|| tr("Unknown"));
    let cores = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "?".into());
    let memory = mem_total_pretty().unwrap_or_else(|| tr("Unknown"));
    HostInfo {
        distro,
        kernel,
        arch,
        hostname,
        cpu,
        cpu_cores: cores,
        memory,
    }
}

fn os_release_pretty_name() -> Option<String> {
    let text = fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn uname_field(flag: &str) -> Option<String> {
    let out = Command::new("uname").arg(flag).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn cpu_model() -> Option<String> {
    let text = fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            let val = rest.trim().trim_start_matches(':').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    // ARM / other
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Hardware") {
            let val = rest.trim().trim_start_matches(':').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn mem_total_pretty() -> Option<String> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            let gib = kb as f64 / 1024.0 / 1024.0;
            return Some(format!("{gib:.1} GiB"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::product_version_from_cargo;

    #[test]
    fn maps_workspace_semver_to_product() {
        assert_eq!(product_version_from_cargo("0.1.18"), "0.1.0.18");
        assert_eq!(product_version_from_cargo("0.1.18-a"), "0.1.0.18a");
    }
}
