//! Optional host packages step.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use super::helpers::{BODY_INNER_WIDTH, step_shell};

struct OptionalFeature {
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    packages: &'static [&'static str],
    installed: bool,
}

fn binary_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn secret_service_available() -> bool {
    binary_on_path("metis-secretsd")
        || binary_on_path("pass-secret-service")
        || binary_on_path("keepassxc")
        || binary_on_path("kwalletd6")
        || binary_on_path("kwalletd5")
        || binary_on_path("gnome-keyring-daemon")
        || Path::new("/usr/share/dbus-1/services/org.freedesktop.secrets.service").exists()
        || Path::new("/usr/local/share/dbus-1/services/org.freedesktop.secrets.service").exists()
}

fn probe_optional_features() -> Vec<OptionalFeature> {
    let mut features = vec![
        OptionalFeature {
            id: "remote",
            title: "Remote desktop",
            subtitle: "Share this session over RDP",
            packages: &["gnome-remote-desktop"],
            installed: binary_on_path("grdctl") || binary_on_path("gnome-remote-desktop"),
        },
        OptionalFeature {
            id: "flatpak",
            title: "Flatpak",
            subtitle: "Run sandboxed apps and Steam Flatpak",
            packages: &["flatpak"],
            installed: binary_on_path("flatpak"),
        },
        OptionalFeature {
            id: "gamemode",
            title: "GameMode",
            subtitle: "Boost CPU performance while gaming",
            packages: &["gamemode"],
            installed: metis_gaming::gamemode_installed(),
        },
        OptionalFeature {
            id: "bluetooth",
            title: "Bluetooth",
            subtitle: "Adapters and devices in Settings",
            packages: &["bluez", "bluetooth"],
            installed: binary_on_path("bluetoothctl"),
        },
        OptionalFeature {
            id: "printers",
            title: "Printers",
            subtitle: "CUPS and printer settings UI",
            packages: &["cups", "system-config-printer"],
            installed: std::process::Command::new("lpstat")
                .arg("-v")
                .output()
                .is_ok(),
        },
    ];
    // Metis ships metis-secretsd; only nudge when no provider binary/service exists.
    if !secret_service_available() {
        features.push(OptionalFeature {
            id: "keyring",
            title: "Keyring",
            subtitle: "Secret Service provider (Metis normally ships metis-secretsd)",
            packages: &["gnome-keyring"],
            installed: false,
        });
    }
    features
}

fn apt_install_command(packages: &[&str]) -> String {
    format!("sudo apt install -y {}", packages.join(" "))
}

fn run_pkexec_apt_install(packages: &[String]) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    if !binary_on_path("pkexec") {
        return Err(format!(
            "{}\n{}",
            metis_i18n::tr("pkexec not found. Install manually:"),
            apt_install_command(&packages.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        ));
    }
    let bin = metis_remote_bin();
    let status = std::process::Command::new("pkexec")
        .arg(&bin)
        .arg("pk-apt-install")
        .args(packages)
        .status()
        .map_err(|e| metis_i18n::tr("failed to start pkexec: %1").replace("%1", &e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}\n{}",
            metis_i18n::tr("Install cancelled or failed. You can run:"),
            apt_install_command(&packages.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        ))
    }
}

fn metis_remote_bin() -> String {
    const INSTALLED: &str = "/usr/bin/metis-remote";
    if std::path::Path::new(INSTALLED).is_file() {
        return INSTALLED.into();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("metis-remote");
        if sibling.is_file() {
            return sibling.to_string_lossy().into_owned();
        }
    }
    "metis-remote".into()
}

pub(crate) fn build_optional_software() -> gtk::Widget {
    let col = step_shell();
    col.set_spacing(6);

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Turn on extras, then Install selected. Installed items are greyed out.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_width_request(BODY_INNER_WIDTH);
    col.append(&hint);

    // Fixed-height scroll so this step cannot grow the content-sized overlay.
    const LIST_HEIGHT: i32 = 168;
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("metis-onboarding-optional-list");
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(LIST_HEIGHT)
        .max_content_height(LIST_HEIGHT)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&list)
        .build();
    scroll.set_propagate_natural_height(false);
    scroll.set_size_request(BODY_INNER_WIDTH, LIST_HEIGHT);
    scroll.set_hexpand(false);
    scroll.set_vexpand(false);
    scroll.set_overflow(gtk::Overflow::Hidden);
    col.append(&scroll);

    let status = gtk::Label::new(None);
    status.add_css_class("metis-onboarding-hint");
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_lines(2);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_selectable(true);
    status.set_height_request(28);
    col.append(&status);

    let install_btn = gtk::Button::with_label(&metis_i18n::tr("Install selected"));
    install_btn.add_css_class("suggested-action");
    install_btn.set_halign(gtk::Align::Start);
    install_btn.set_sensitive(false);
    col.append(&install_btn);

    let features = Rc::new(RefCell::new(probe_optional_features()));
    let switches: Rc<RefCell<Vec<(String, gtk::Switch, bool)>>> = Rc::new(RefCell::new(Vec::new()));

    let refresh_list = {
        let list = list.clone();
        let features = features.clone();
        let switches = switches.clone();
        let install_btn = install_btn.clone();
        let status = status.clone();
        Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            switches.borrow_mut().clear();

            for feat in features.borrow().iter() {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.add_css_class("metis-onboarding-optional-row");
                if feat.installed {
                    row.add_css_class("metis-onboarding-optional-installed");
                }
                row.set_hexpand(true);

                let text_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
                text_col.set_hexpand(true);
                let title = gtk::Label::new(Some(&metis_i18n::tr(feat.title)));
                title.add_css_class("metis-onboarding-optional-title");
                title.set_xalign(0.0);
                let sub = gtk::Label::new(Some(&if feat.installed {
                    metis_i18n::tr("Installed")
                } else {
                    metis_i18n::tr(feat.subtitle)
                }));
                sub.add_css_class("metis-onboarding-hint");
                sub.set_xalign(0.0);
                text_col.append(&title);
                text_col.append(&sub);
                row.append(&text_col);

                let sw = gtk::Switch::new();
                sw.set_valign(gtk::Align::Center);
                if feat.installed {
                    sw.set_active(true);
                    sw.set_sensitive(false);
                } else {
                    sw.set_active(false);
                    sw.set_sensitive(true);
                }
                row.append(&sw);
                list.append(&row);

                switches
                    .borrow_mut()
                    .push((feat.id.to_string(), sw.clone(), feat.installed));
            }

            let update_btn = Rc::new({
                let switches = switches.clone();
                let install_btn = install_btn.clone();
                move || {
                    let any = switches
                        .borrow()
                        .iter()
                        .any(|(_, sw, installed)| !*installed && sw.is_active());
                    install_btn.set_sensitive(any);
                }
            });
            update_btn();
            for (_, sw, installed) in switches.borrow().iter() {
                if *installed {
                    continue;
                }
                let update_btn = update_btn.clone();
                sw.connect_active_notify(move |_| {
                    update_btn();
                });
            }
            status.set_text("");
        })
    };

    refresh_list();

    {
        let features = features.clone();
        let switches = switches.clone();
        let install_btn = install_btn.clone();
        let status = status.clone();
        let refresh_list = refresh_list.clone();
        let list = list.clone();
        install_btn.connect_clicked(move |btn| {
            let mut pkgs: Vec<String> = Vec::new();
            let feats = features.borrow();
            for (id, sw, installed) in switches.borrow().iter() {
                if *installed || !sw.is_active() {
                    continue;
                }
                if let Some(feat) = feats.iter().find(|f| f.id == *id) {
                    for p in feat.packages {
                        if !pkgs.iter().any(|x| x == *p) {
                            pkgs.push((*p).to_string());
                        }
                    }
                }
            }
            drop(feats);
            if pkgs.is_empty() {
                return;
            }

            btn.set_sensitive(false);
            list.set_sensitive(false);
            status.set_text(&metis_i18n::tr("Installing… authenticate if prompted."));

            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            let pkgs_thread = pkgs.clone();
            std::thread::spawn(move || {
                let _ = tx.send(run_pkexec_apt_install(&pkgs_thread));
            });

            let features = features.clone();
            let refresh_list = refresh_list.clone();
            let status = status.clone();
            let list = list.clone();
            let btn = btn.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    *features.borrow_mut() = probe_optional_features();
                    refresh_list();
                    list.set_sensitive(true);
                    status.set_text(&metis_i18n::tr("Installed successfully."));
                    glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    list.set_sensitive(true);
                    btn.set_sensitive(true);
                    status.set_text(&err);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    list.set_sensitive(true);
                    btn.set_sensitive(true);
                    status.set_text(&metis_i18n::tr("Install failed unexpectedly."));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    col.upcast()
}
