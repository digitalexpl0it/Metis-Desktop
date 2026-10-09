//! Gaming health checks, Flatpak optimize, and status summary.

use std::rc::Rc;
use std::sync::mpsc;

use gtk::prelude::*;
use metis_gaming::health::{
    HealthCheck, HealthSeverity, auto_fix_item, install_nvidia_drivers, run_health_check,
};
use metis_gaming::nvidia_reboot_required;
use metis_i18n::tr;

use crate::ui;

use super::{GamingUiEvent, Sections};

pub(crate) fn spawn_health_check(tx: mpsc::Sender<GamingUiEvent>) {
    std::thread::spawn(move || {
        let check = run_health_check();
        let _ = tx.send(GamingUiEvent::HealthCheck(check));
    });
}

/// Permission-review dialog before writing Flatpak gaming overrides.
pub(crate) fn show_optimize_confirm_dialog(parent: &gtk::Window, on_confirm: impl Fn() + 'static) {
    let title = tr("Apply Flatpak gaming overrides?");
    let dialog = gtk::Window::builder()
        .title(&title)
        .modal(true)
        .transient_for(parent)
        .resizable(false)
        .default_width(480)
        .build();
    dialog.add_css_class("metis-settings-window");

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    root.append(&heading);

    let body = gtk::Label::new(Some(&tr(
        "Metis will run flatpak override --user for installed Steam, Lutris, and Heroic apps. \
         This widens their sandboxes with:",
    )));
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.add_css_class("metis-settings-hint");
    root.append(&body);

    let flags = gtk::Label::new(Some(
        "• Steam: --device=all, --socket=wayland, --socket=pulseaudio, --share=network\n\
         • Lutris / Heroic: --device=all, --share=network\n\
         • Optional: GPU offload env vars when enabled in Settings",
    ));
    flags.set_xalign(0.0);
    flags.add_css_class("metis-settings-value");
    root.append(&flags);

    let warn = gtk::Label::new(Some(&tr("Cancel leaves Flatpak permissions unchanged.")));
    warn.set_wrap(true);
    warn.set_xalign(0.0);
    warn.add_css_class("metis-settings-hint");
    root.append(&warn);

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("metis-settings-secondary");
    let confirm = gtk::Button::with_label(&tr("Apply overrides"));
    confirm.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&confirm);
    root.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&root)));

    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.close()
    });
    confirm.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
            on_confirm();
        }
    });

    dialog.present();
}

/// Flatpak overrides + input-group membership + compositor optimize hook.
pub(crate) fn run_optimize_pass() -> String {
    let mut notes = Vec::new();

    match metis_gaming::optimize_flatpak_gaming() {
        Ok(results) if results.is_empty() => {
            notes.push("No Flatpak Steam/Lutris/Heroic installs to optimize".into());
        }
        Ok(results) => {
            notes.push(format!("Optimized {} Flatpak app(s)", results.len()));
        }
        Err(err) => notes.push(format!("Flatpak optimize failed: {err}")),
    }

    match metis_gaming::ensure_steam_launcher() {
        Ok(path) => notes.push(format!("Steam launcher ready ({})", path.display())),
        Err(err) => tracing::debug!(%err, "ensure_steam_launcher"),
    }

    match auto_fix_item("input_group") {
        Ok(msg) => {
            if !msg.contains("Already in") {
                notes.push(msg);
            }
        }
        Err(err) => notes.push(err),
    }

    metis_gaming::session::request_optimize();
    if notes.is_empty() {
        "Optimize finished.".into()
    } else {
        notes.join(" · ")
    }
}
pub(crate) fn health_signature(check: &HealthCheck) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for item in &check.items {
        item.id.hash(&mut hasher);
        format!("{:?}", item.severity).hash(&mut hasher);
        item.detail.hash(&mut hasher);
        item.auto_fixable.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn apply_health_check(
    sections: &Rc<Sections>,
    check: &HealthCheck,
    ui_tx: mpsc::Sender<GamingUiEvent>,
) {
    sections.reboot_banner.set_visible(nvidia_reboot_required());
    let sig = health_signature(check);
    if sig == sections.last_health_sig.get() {
        update_health_summary(sections, check);
        return;
    }
    sections.last_health_sig.set(sig);

    while let Some(child) = sections.health_list.first_child() {
        sections.health_list.remove(&child);
    }
    for (i, item) in check.items.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("metis-settings-row");
        row.add_css_class("metis-settings-health-item");
        row.set_hexpand(true);
        row.set_halign(gtk::Align::Fill);
        let icon = match item.severity {
            HealthSeverity::Ok => "emblem-ok-symbolic",
            HealthSeverity::Info => "dialog-information-symbolic",
            HealthSeverity::Warn => "dialog-warning-symbolic",
            HealthSeverity::Error => "dialog-error-symbolic",
        };
        row.append(&gtk::Image::from_icon_name(icon));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Label::new(Some(&item.label));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(22);
        let detail = gtk::Label::new(Some(&item.detail));
        detail.set_xalign(0.0);
        detail.set_wrap(true);
        detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.set_width_chars(18);
        detail.set_max_width_chars(24);
        detail.add_css_class("metis-settings-hint");
        text.append(&title);
        text.append(&detail);
        row.append(&text);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_valign(gtk::Align::Center);
        if item.auto_fixable {
            let fix = gtk::Button::with_label(&tr("Fix"));
            fix.set_tooltip_text(Some(&tr(
                "Install or apply this fix (may ask for your password)",
            )));
            let fix_id = item.id.to_string();
            let ui_tx = ui_tx.clone();
            fix.connect_clicked(move |btn| {
                let run_fix = {
                    let btn = btn.clone();
                    let ui_tx = ui_tx.clone();
                    let fix_id = fix_id.clone();
                    move || {
                        btn.set_sensitive(false);
                        let ui_tx = ui_tx.clone();
                        let fix_id = fix_id.clone();
                        std::thread::spawn(move || {
                            let summary = match auto_fix_item(&fix_id) {
                                Ok(msg) => msg,
                                Err(err) => err,
                            };
                            let _ = ui_tx.send(GamingUiEvent::FixDone { summary });
                        });
                    }
                };
                if fix_id == "flatpak_steam" {
                    let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok())
                    else {
                        run_fix();
                        return;
                    };
                    show_optimize_confirm_dialog(&parent, run_fix);
                } else {
                    run_fix();
                }
            });
            actions.append(&fix);
        } else if item.id == "nvidia_driver"
            && matches!(item.severity, HealthSeverity::Error)
            && !item.detail.contains("reboot")
        {
            let install = gtk::Button::with_label(&tr("Install…"));
            install.set_tooltip_text(Some(&tr(
                "Install recommended NVIDIA drivers (asks for confirmation and password)",
            )));
            let ui_tx = ui_tx.clone();
            install.connect_clicked(move |btn| {
                let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                    return;
                };
                let btn = btn.clone();
                let ui_tx = ui_tx.clone();
                show_nvidia_consent_dialog(&parent, move || {
                    btn.set_sensitive(false);
                    let ui_tx = ui_tx.clone();
                    std::thread::spawn(move || {
                        let summary = match install_nvidia_drivers() {
                            Ok(msg) => msg,
                            Err(err) => err,
                        };
                        let _ = ui_tx.send(GamingUiEvent::FixDone { summary });
                    });
                });
            });
            actions.append(&install);
        }
        if let Some(hint) = item.fix_hint.as_ref() {
            let copy = gtk::Button::with_label(&tr("Copy command"));
            let hint = hint.clone();
            copy.set_tooltip_text(Some(hint.as_str()));
            copy.connect_clicked(move |btn| {
                btn.clipboard().set_text(&hint);
                btn.set_label(&tr("Copied"));
            });
            actions.append(&copy);
        }
        if actions.first_child().is_some() {
            row.append(&actions);
        }
        let col = (i % 2) as i32;
        let row_i = (i / 2) as i32;
        sections.health_list.attach(&row, col, row_i, 1, 1);
    }
    update_health_summary(sections, check);
}

pub(crate) fn update_health_summary(sections: &Rc<Sections>, check: &HealthCheck) {
    let issues = check
        .items
        .iter()
        .filter(|i| matches!(i.severity, HealthSeverity::Warn | HealthSeverity::Error))
        .count();
    let infos = check
        .items
        .iter()
        .filter(|i| matches!(i.severity, HealthSeverity::Info))
        .count();

    sections
        .status_box
        .remove_css_class("metis-settings-gaming-status-ok");
    sections
        .status_box
        .remove_css_class("metis-settings-gaming-status-warn");

    if issues == 0 && infos == 0 {
        sections
            .status_icon
            .set_icon_name(Some("emblem-ok-symbolic"));
        sections
            .status_text
            .set_text(&tr("Ready for gaming — all checks passed."));
        sections
            .status_box
            .add_css_class("metis-settings-gaming-status-ok");
    } else if issues == 0 {
        sections
            .status_icon
            .set_icon_name(Some("dialog-information-symbolic"));
        sections.status_text.set_text(&tr(&format!(
            "Mostly ready — {infos} optional improvement(s) below."
        )));
        sections
            .status_box
            .add_css_class("metis-settings-gaming-status-warn");
    } else {
        let auto = check.items.iter().any(|i| {
            i.auto_fixable && matches!(i.severity, HealthSeverity::Warn | HealthSeverity::Error)
        });
        sections
            .status_icon
            .set_icon_name(Some("dialog-warning-symbolic"));
        if auto {
            sections.status_text.set_text(&tr(&format!(
                "{issues} issue(s) found — use Fix to install, or Copy command to run it yourself."
            )));
        } else {
            sections.status_text.set_text(&tr(&format!(
                "{issues} issue(s) found — use Copy command on each row (these need a manual step)."
            )));
        }
        sections
            .status_box
            .add_css_class("metis-settings-gaming-status-warn");
    }
}

pub(crate) fn show_nvidia_consent_dialog(parent: &gtk::Window, on_confirm: impl Fn() + 'static) {
    let title = tr("Install recommended NVIDIA drivers?");
    let dialog = gtk::Window::builder()
        .title(&title)
        .modal(true)
        .transient_for(parent)
        .resizable(false)
        .default_width(480)
        .build();
    dialog.add_css_class("metis-settings-window");

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(24)
        .margin_end(24)
        .build();

    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    root.append(&heading);

    let body = gtk::Label::new(Some(&tr(
        "Metis will run ubuntu-drivers install (recommended package only) after you approve \
         with your admin password. A reboot is required afterward. Matching 32-bit NVIDIA GL \
         packages are installed best-effort when the driver series can be detected.",
    )));
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.add_css_class("metis-settings-hint");
    root.append(&body);

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("metis-settings-secondary");
    let confirm = gtk::Button::with_label(&tr("Install drivers"));
    confirm.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&confirm);
    root.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&root)));

    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.close()
    });
    confirm.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
            on_confirm();
        }
    });

    dialog.present();
}
