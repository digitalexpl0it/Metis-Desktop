//! Gaming setup wizard and NVIDIA consent dialogs.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use metis_config::{GraphicsMode, load_gaming_config, save_gaming_config};
use metis_gaming::health::{auto_fix_item, install_nvidia_drivers};
use metis_gaming::{nvidia_gpu_present, nvidia_reboot_required};
use metis_i18n::tr;

use crate::ui;

use super::health::{show_nvidia_consent_dialog, spawn_health_check};
use super::{GAMING_SETUP_DIALOG, GamingUiEvent, Sections};

pub(crate) fn show_gaming_setup_dialog(
    parent: &gtk::Window,
    sections: Rc<Sections>,
    ui_tx: mpsc::Sender<GamingUiEvent>,
) {
    if let Some(existing) = GAMING_SETUP_DIALOG.with(|d| d.borrow().clone()) {
        existing.present();
        return;
    }

    let win = gtk::Window::builder()
        .title(tr("Gaming setup"))
        .transient_for(parent)
        .modal(true)
        .decorated(false)
        .resizable(false)
        .default_width(520)
        .default_height(420)
        .build();
    win.add_css_class("metis-settings-window");
    win.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_bottom(12);
    let heading = gtk::Label::new(Some(&tr("Gaming setup")));
    heading.set_xalign(0.0);
    heading.set_hexpand(true);
    heading.add_css_class("metis-settings-section-title");
    header.append(&heading);
    let header_close = gtk::Button::with_label(&tr("Close"));
    header_close.add_css_class("metis-settings-secondary");
    header.append(&header_close);
    outer.append(&header);

    let step_label = gtk::Label::new(None);
    step_label.set_xalign(0.0);
    step_label.add_css_class("metis-settings-value");
    step_label.set_margin_bottom(8);
    outer.append(&step_label);

    let body = gtk::Label::new(None);
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.add_css_class("metis-settings-hint");
    body.set_margin_bottom(10);
    outer.append(&body);

    let status = gtk::Label::new(Some(&tr(
        "Leave Steam Launch Options empty for GPU — Metis session offload handles hybrid routing.",
    )));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("metis-settings-value");
    status.set_margin_top(8);
    outer.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(16);
    let back_btn = gtk::Button::with_label(&tr("Back"));
    back_btn.add_css_class("metis-settings-secondary");
    let skip_btn = gtk::Button::with_label(&tr("Skip"));
    skip_btn.add_css_class("metis-settings-secondary");
    let next_btn = gtk::Button::with_label(&tr("Next"));
    next_btn.add_css_class("suggested-action");
    actions.append(&back_btn);
    actions.append(&skip_btn);
    actions.append(&next_btn);
    outer.append(&actions);

    win.set_child(Some(&ui::dialog_sheet(&outer)));

    GAMING_SETUP_DIALOG.with(|slot| *slot.borrow_mut() = Some(win.clone()));
    {
        let win = win.clone();
        win.connect_close_request(move |_| {
            GAMING_SETUP_DIALOG.with(|slot| *slot.borrow_mut() = None);
            glib::Propagation::Proceed
        });
    }

    let close_dialog: Rc<dyn Fn()> = Rc::new({
        let win = win.clone();
        move || {
            GAMING_SETUP_DIALOG.with(|slot| *slot.borrow_mut() = None);
            win.close();
        }
    });
    header_close.connect_clicked({
        let close_dialog = close_dialog.clone();
        move |_| close_dialog()
    });

    let step = Rc::new(Cell::new(0u32));
    let busy = Rc::new(Cell::new(false));
    let need_nvidia = nvidia_gpu_present() && !metis_gaming::detect::nvidia_driver_loaded();

    let refresh_step: Rc<dyn Fn()> = Rc::new({
        let step_label = step_label.clone();
        let body = body.clone();
        let next_btn = next_btn.clone();
        let back_btn = back_btn.clone();
        let skip_btn = skip_btn.clone();
        let step = step.clone();
        move || {
            let i = step.get();
            let last = wizard_finish_step(need_nvidia);
            let (title, text, next_label) = wizard_step_copy(i, need_nvidia);
            step_label.set_text(&title);
            body.set_text(&text);
            next_btn.set_label(&next_label);
            back_btn.set_sensitive(i > 0);
            skip_btn.set_visible(i < last);
        }
    });
    refresh_step();

    enum WizardMsg {
        Status(String),
        Advance,
    }
    let (wiz_tx, wiz_rx) = mpsc::channel::<WizardMsg>();
    {
        let status = status.clone();
        let busy = busy.clone();
        let next_btn = next_btn.clone();
        let skip_btn = skip_btn.clone();
        let step = step.clone();
        let refresh_step = refresh_step.clone();
        let close_dialog = close_dialog.clone();
        let sections = sections.clone();
        let ui_tx = ui_tx.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            while let Ok(msg) = wiz_rx.try_recv() {
                match msg {
                    WizardMsg::Status(text) => {
                        status.set_text(&text);
                        busy.set(false);
                        next_btn.set_sensitive(true);
                        skip_btn.set_sensitive(true);
                    }
                    WizardMsg::Advance => {
                        let i = step.get();
                        let last = wizard_finish_step(need_nvidia);
                        if i >= last {
                            let _ = metis_config::mark_gaming_setup_complete();
                            metis_gaming::session::request_reload();
                            sections.reboot_banner.set_visible(nvidia_reboot_required());
                            spawn_health_check(ui_tx.clone());
                            close_dialog();
                        } else {
                            step.set(i + 1);
                            refresh_step();
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    back_btn.connect_clicked({
        let step = step.clone();
        let refresh_step = refresh_step.clone();
        let busy = busy.clone();
        move |_| {
            if busy.get() {
                return;
            }
            let i = step.get();
            if i > 0 {
                step.set(i - 1);
                refresh_step();
            }
        }
    });

    skip_btn.connect_clicked({
        let wiz_tx = wiz_tx.clone();
        let busy = busy.clone();
        move |_| {
            if busy.get() {
                return;
            }
            let _ = wiz_tx.send(WizardMsg::Advance);
        }
    });

    next_btn.connect_clicked({
        let step = step.clone();
        let busy = busy.clone();
        let wiz_tx = wiz_tx.clone();
        let parent = parent.clone();
        let skip_btn = skip_btn.clone();
        let status = status.clone();
        move |btn| {
            if busy.get() {
                return;
            }
            let i = step.get();
            busy.set(true);
            btn.set_sensitive(false);
            skip_btn.set_sensitive(false);
            let wiz_tx = wiz_tx.clone();
            let skip_btn = skip_btn.clone();
            match wizard_step_action(i, need_nvidia) {
                WizardAction::Background(task) => {
                    status.set_text(&tr("Working…"));
                    std::thread::spawn(move || {
                        let msg = task();
                        let _ = wiz_tx.send(WizardMsg::Status(msg));
                        let _ = wiz_tx.send(WizardMsg::Advance);
                    });
                }
                WizardAction::NvidiaConsent => {
                    busy.set(false);
                    btn.set_sensitive(true);
                    skip_btn.set_sensitive(true);
                    show_nvidia_consent_dialog(&parent, {
                        let wiz_tx = wiz_tx.clone();
                        let busy = busy.clone();
                        let btn = btn.clone();
                        let skip_btn = skip_btn.clone();
                        move || {
                            busy.set(true);
                            btn.set_sensitive(false);
                            skip_btn.set_sensitive(false);
                            let wiz_tx = wiz_tx.clone();
                            std::thread::spawn(move || {
                                let msg = match install_nvidia_drivers() {
                                    Ok(m) => m,
                                    Err(e) => e,
                                };
                                let _ = wiz_tx.send(WizardMsg::Status(msg));
                                let _ = wiz_tx.send(WizardMsg::Advance);
                            });
                        }
                    });
                }
            }
        }
    });

    win.present();
}

#[derive(Clone, Copy)]
enum WizardAction {
    Background(fn() -> String),
    NvidiaConsent,
}

/// Index of the final Flatpak / finish step.
fn wizard_finish_step(need_nvidia: bool) -> u32 {
    if need_nvidia { 6 } else { 5 }
}

fn wizard_step_copy(step: u32, need_nvidia: bool) -> (String, String, String) {
    match step {
        0 => (
            tr("Step 1 — Steam"),
            tr(
                "Install Steam if missing. Native steam-installer is preferred when \
                 steam_prefer_native is on; otherwise Flatpak Steam.",
            ),
            tr("Install / check Steam"),
        ),
        1 => (
            tr("Step 2 — Vulkan"),
            tr(
                "Install mesa-vulkan-drivers (64-bit) and mesa-vulkan-drivers:i386 for Proton. \
                 Leave per-game Launch Options empty for GPU — Metis offloads hybrid sessions.",
            ),
            tr("Fix Vulkan"),
        ),
        2 => (
            tr("Step 3 — Controllers"),
            tr("Install steam-devices udev rules and add your user to the input group."),
            tr("Fix controllers"),
        ),
        3 => (
            tr("Step 4 — GameMode"),
            tr("Install GameMode so Metis can register game sessions with gamemoded."),
            tr("Install GameMode"),
        ),
        4 => (
            tr("Step 5 — GPU mode"),
            tr("Confirm graphics mode Auto (desktop on iGPU, games on discrete when present)."),
            tr("Set Auto"),
        ),
        5 if need_nvidia => (
            tr("Step 6 — NVIDIA drivers"),
            tr(
                "Install recommended proprietary drivers via ubuntu-drivers (admin password). \
                 A reboot is required afterward. Never runs silently at login.",
            ),
            tr("Install drivers…"),
        ),
        s if s == wizard_finish_step(need_nvidia) => (
            tr(if need_nvidia {
                "Step 7 — You're ready"
            } else {
                "Step 6 — You're ready"
            }),
            tr(
                "Apply Flatpak gaming overrides and launcher wrapper. You're ready to play — \
                 GPU Launch Options stay empty; Metis session offload handles hybrid routing.",
            ),
            tr("Finish setup"),
        ),
        _ => (
            tr("Gaming setup"),
            tr("Follow the steps to finish gaming setup."),
            tr("Next"),
        ),
    }
}

fn wizard_step_action(step: u32, need_nvidia: bool) -> WizardAction {
    match step {
        0 => WizardAction::Background(|| match auto_fix_item("steam") {
            Ok(m) => m,
            Err(e) => e,
        }),
        1 => WizardAction::Background(|| {
            let mut notes = Vec::new();
            match auto_fix_item("mesa_vulkan") {
                Ok(m) => notes.push(m),
                Err(e) => notes.push(e),
            }
            match auto_fix_item("vulkan_i386") {
                Ok(m) => notes.push(m),
                Err(e) => notes.push(e),
            }
            notes.join(" · ")
        }),
        2 => WizardAction::Background(|| {
            let mut notes = Vec::new();
            match auto_fix_item("steam_devices") {
                Ok(m) => notes.push(m),
                Err(e) => notes.push(e),
            }
            match auto_fix_item("input_group") {
                Ok(m) => notes.push(m),
                Err(e) => notes.push(e),
            }
            notes.join(" · ")
        }),
        3 => WizardAction::Background(|| match auto_fix_item("gamemode") {
            Ok(m) => m,
            Err(e) => e,
        }),
        4 => WizardAction::Background(|| {
            let mut cfg = load_gaming_config();
            cfg.graphics_mode = GraphicsMode::Auto;
            match save_gaming_config(&cfg) {
                Ok(()) => {
                    metis_gaming::session::request_reload();
                    "Graphics mode set to Auto".into()
                }
                Err(e) => format!("Could not save gaming.json: {e}"),
            }
        }),
        5 if need_nvidia => WizardAction::NvidiaConsent,
        s if s == wizard_finish_step(need_nvidia) => WizardAction::Background(|| {
            let mut notes = Vec::new();
            match metis_gaming::optimize_flatpak_gaming() {
                Ok(results) if results.is_empty() => {
                    notes.push("No Flatpak gaming apps to optimize".into());
                }
                Ok(results) => {
                    notes.push(format!("Optimized {} Flatpak app(s)", results.len()));
                }
                Err(err) => notes.push(format!("Flatpak optimize failed: {err}")),
            }
            match metis_gaming::ensure_steam_launcher() {
                Ok(path) => notes.push(format!("Launcher: {}", path.display())),
                Err(err) => notes.push(format!("Launcher: {err}")),
            }
            notes.join(" · ")
        }),
        _ => WizardAction::Background(|| "Done".into()),
    }
}
