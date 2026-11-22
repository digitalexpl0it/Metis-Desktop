//! Gaming setup dashboard — live checklist (not a linear Back/Skip/Next wizard).
//!
//! Auto-detects Steam / Vulkan / controllers / GameMode / GPU / NVIDIA / Flatpak
//! on open and after each Fix, shows what needs attention, and lets the user
//! reconfigure items that are already ready.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use metis_config::{GraphicsMode, load_gaming_config, save_gaming_config};
use metis_gaming::detect::detect_steam;
use metis_gaming::health::{
    HealthCheck, HealthItem, HealthSeverity, auto_fix_item, install_nvidia_drivers,
    run_health_check,
};
use metis_gaming::{SteamInstall, nvidia_gpu_present, nvidia_reboot_required};
use metis_i18n::tr;

use crate::ui;

use super::health::{show_nvidia_consent_dialog, show_optimize_confirm_dialog, spawn_health_check};
use super::{GAMING_SETUP_DIALOG, GamingUiEvent, Sections};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CardKind {
    Ready,
    Attention,
    Optional,
}

#[derive(Clone)]
struct DashRow {
    /// Primary health id used for Fix (or synthetic: `vulkan`, `controllers`, `gpu_mode`, `finish`).
    id: String,
    title: String,
    detail: String,
    icon: &'static str,
    kind: CardKind,
    /// Health ids to auto-fix for this row (may be multiple when merged).
    fix_ids: Vec<String>,
    nvidia_install: bool,
    flatpak_confirm: bool,
    show_gpu_dropdown: bool,
    open_steam: bool,
}

enum DashMsg {
    Probed(HealthCheck),
    FixDone { summary: String },
    Busy(bool),
}

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
        .resizable(true)
        .default_width(640)
        .default_height(580)
        .build();
    win.add_css_class("metis-settings-window");
    win.add_css_class("metis-settings-password-dialog");
    win.add_css_class("metis-settings-gaming-wizard");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_margin_top(18);
    outer.set_margin_bottom(14);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    // ── Hero ────────────────────────────────────────────────────────────
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.set_margin_bottom(12);

    let icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icon_wrap.add_css_class("metis-settings-wizard-hero-icon");
    icon_wrap.set_valign(gtk::Align::Start);
    let hero_icon = gtk::Image::from_icon_name("applications-games-symbolic");
    hero_icon.set_pixel_size(28);
    icon_wrap.append(&hero_icon);
    header.append(&icon_wrap);

    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let heading = gtk::Label::new(Some(&tr("Get ready to play")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    titles.append(&heading);
    let subtitle = gtk::Label::new(Some(&tr(
        "Live check of Steam, Vulkan, controllers, and GPU — fix what needs attention, tweak what’s already set.",
    )));
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    subtitle.set_width_chars(28);
    subtitle.set_max_width_chars(52);
    subtitle.add_css_class("metis-settings-hint");
    titles.append(&subtitle);
    header.append(&titles);

    let summary_chip = gtk::Label::new(Some(&tr("Checking…")));
    summary_chip.add_css_class("metis-settings-wizard-summary-chip");
    summary_chip.set_valign(gtk::Align::Center);
    header.append(&summary_chip);

    let header_close = gtk::Button::from_icon_name("window-close-symbolic");
    header_close.set_tooltip_text(Some(&tr("Close")));
    header_close.add_css_class("metis-settings-secondary");
    header_close.add_css_class("metis-settings-wizard-close");
    header_close.set_valign(gtk::Align::Start);
    header.append(&header_close);
    outer.append(&header);

    // ── Scrollable checklist ────────────────────────────────────────────
    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.add_css_class("metis-settings-wizard-checklist");
    list.set_hexpand(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(280)
        .child(&list)
        .build();
    ui::wire_vertical_scroll(&scroller);
    outer.append(&scroller);

    let tip = gtk::Label::new(Some(&tr(
        "Leave Steam Launch Options empty for GPU — Metis session offload handles hybrid routing.",
    )));
    tip.set_xalign(0.0);
    tip.set_wrap(true);
    tip.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    tip.add_css_class("metis-settings-hint");
    tip.set_margin_top(10);
    outer.append(&tip);

    // ── Footer ──────────────────────────────────────────────────────────
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(12);
    let refresh_btn = gtk::Button::with_label(&tr("Refresh"));
    refresh_btn.add_css_class("metis-settings-secondary");
    refresh_btn.set_tooltip_text(Some(&tr("Re-scan this machine")));
    let fix_all_btn = gtk::Button::with_label(&tr("Fix all"));
    fix_all_btn.add_css_class("metis-settings-secondary");
    fix_all_btn.set_tooltip_text(Some(&tr(
        "Install or apply every auto-fixable item that needs attention",
    )));
    let done_btn = gtk::Button::with_label(&tr("Done"));
    done_btn.add_css_class("suggested-action");
    actions.append(&refresh_btn);
    actions.append(&fix_all_btn);
    actions.append(&done_btn);
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

    let (dash_tx, dash_rx) = mpsc::channel::<DashMsg>();
    let busy = Rc::new(Cell::new(false));
    let last_rows: Rc<RefCell<Vec<DashRow>>> = Rc::new(RefCell::new(Vec::new()));
    let seeding_gpu = Rc::new(Cell::new(false));

    let rebuild: Rc<dyn Fn(HealthCheck)> = Rc::new({
        let list = list.clone();
        let summary_chip = summary_chip.clone();
        let last_rows = last_rows.clone();
        let dash_tx = dash_tx.clone();
        let busy = busy.clone();
        let parent = parent.clone();
        let seeding_gpu = seeding_gpu.clone();
        let fix_all_btn = fix_all_btn.clone();
        move |check: HealthCheck| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let rows = build_dashboard_rows(&check);
            let ready = rows.iter().filter(|r| r.kind == CardKind::Ready).count();
            let total = rows.len().max(1);
            let attention = rows.iter().filter(|r| r.kind == CardKind::Attention).count();
            summary_chip.remove_css_class("metis-settings-wizard-summary-ok");
            summary_chip.remove_css_class("metis-settings-wizard-summary-warn");
            if attention == 0 {
                summary_chip.add_css_class("metis-settings-wizard-summary-ok");
                summary_chip.set_text(&format!("{} of {} {}", ready, total, tr("ready")));
            } else {
                summary_chip.add_css_class("metis-settings-wizard-summary-warn");
                summary_chip.set_text(&format!(
                    "{} of {} {} · {} {}",
                    ready,
                    total,
                    tr("ready"),
                    attention,
                    tr("need attention")
                ));
            }
            fix_all_btn.set_sensitive(attention > 0 && !busy.get());

            for row in &rows {
                list.append(&build_row_widget(
                    row,
                    &dash_tx,
                    &busy,
                    &parent,
                    &seeding_gpu,
                ));
            }
            *last_rows.borrow_mut() = rows;
        }
    });

    // GTK poll for background messages.
    {
        let rebuild = rebuild.clone();
        let busy = busy.clone();
        let refresh_btn = refresh_btn.clone();
        let fix_all_btn = fix_all_btn.clone();
        let done_btn = done_btn.clone();
        let summary_chip = summary_chip.clone();
        let dash_tx = dash_tx.clone();
        glib::timeout_add_local(Duration::from_millis(80), move || {
            while let Ok(msg) = dash_rx.try_recv() {
                match msg {
                    DashMsg::Probed(check) => {
                        busy.set(false);
                        refresh_btn.set_sensitive(true);
                        done_btn.set_sensitive(true);
                        rebuild(check);
                    }
                    DashMsg::FixDone { summary } => {
                        busy.set(false);
                        refresh_btn.set_sensitive(true);
                        done_btn.set_sensitive(true);
                        if !summary.is_empty() {
                            summary_chip.set_tooltip_text(Some(&summary));
                        }
                        // Re-probe after every fix.
                        let dash_tx = dash_tx.clone();
                        std::thread::spawn(move || {
                            let check = run_health_check();
                            let _ = dash_tx.send(DashMsg::Probed(check));
                        });
                    }
                    DashMsg::Busy(on) => {
                        busy.set(on);
                        refresh_btn.set_sensitive(!on);
                        fix_all_btn.set_sensitive(!on);
                        done_btn.set_sensitive(!on);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    let spawn_probe: Rc<dyn Fn()> = Rc::new({
        let dash_tx = dash_tx.clone();
        let busy = busy.clone();
        move || {
            if busy.get() {
                return;
            }
            busy.set(true);
            let _ = dash_tx.send(DashMsg::Busy(true));
            let dash_tx = dash_tx.clone();
            std::thread::spawn(move || {
                let check = run_health_check();
                let _ = dash_tx.send(DashMsg::Probed(check));
            });
        }
    });

    refresh_btn.connect_clicked({
        let spawn_probe = spawn_probe.clone();
        move |_| spawn_probe()
    });

    fix_all_btn.connect_clicked({
        let last_rows = last_rows.clone();
        let dash_tx = dash_tx.clone();
        let busy = busy.clone();
        let parent = parent.clone();
        move |_| {
            if busy.get() {
                return;
            }
            let rows = last_rows.borrow().clone();
            let mut jobs: Vec<(String, bool)> = Vec::new();
            for row in &rows {
                if row.kind != CardKind::Attention {
                    continue;
                }
                if row.nvidia_install {
                    jobs.push(("nvidia_driver".into(), true));
                } else {
                    for id in &row.fix_ids {
                        jobs.push((id.clone(), row.flatpak_confirm));
                    }
                }
            }
            if jobs.is_empty() {
                return;
            }

            // NVIDIA consent first if present; then run the rest in background.
            let has_nvidia = jobs.iter().any(|(id, _)| id == "nvidia_driver");
            let non_nvidia: Vec<(String, bool)> = jobs
                .into_iter()
                .filter(|(id, _)| id != "nvidia_driver")
                .collect();

            let run_batch = {
                let dash_tx = dash_tx.clone();
                let busy = busy.clone();
                move |batch: Vec<(String, bool)>| {
                    busy.set(true);
                    let _ = dash_tx.send(DashMsg::Busy(true));
                    let dash_tx = dash_tx.clone();
                    std::thread::spawn(move || {
                        let mut notes = Vec::new();
                        for (id, _) in batch {
                            match auto_fix_item(&id) {
                                Ok(m) => notes.push(m),
                                Err(e) => notes.push(e),
                            }
                        }
                        let _ = dash_tx.send(DashMsg::FixDone {
                            summary: notes.join(" · "),
                        });
                    });
                }
            };

            if has_nvidia {
                let non_nvidia = non_nvidia.clone();
                let dash_tx = dash_tx.clone();
                let busy = busy.clone();
                show_nvidia_consent_dialog(&parent, move || {
                    busy.set(true);
                    let _ = dash_tx.send(DashMsg::Busy(true));
                    let dash_tx = dash_tx.clone();
                    let rest = non_nvidia.clone();
                    std::thread::spawn(move || {
                        let mut notes = Vec::new();
                        match install_nvidia_drivers() {
                            Ok(m) => notes.push(m),
                            Err(e) => notes.push(e),
                        }
                        for (id, _) in &rest {
                            match auto_fix_item(id) {
                                Ok(m) => notes.push(m),
                                Err(e) => notes.push(e),
                            }
                        }
                        let _ = dash_tx.send(DashMsg::FixDone {
                            summary: notes.join(" · "),
                        });
                    });
                });
            } else if non_nvidia.iter().any(|(_, confirm)| *confirm) {
                let run_batch = Rc::new(run_batch);
                let batch = non_nvidia.clone();
                show_optimize_confirm_dialog(&parent, {
                    let run_batch = run_batch.clone();
                    move || run_batch(batch.clone())
                });
            } else {
                run_batch(non_nvidia);
            }
        }
    });

    done_btn.connect_clicked({
        let close_dialog = close_dialog.clone();
        let sections = sections.clone();
        let ui_tx = ui_tx.clone();
        let busy = busy.clone();
        move |_| {
            if busy.get() {
                return;
            }
            let _ = metis_config::mark_gaming_setup_complete();
            metis_gaming::session::request_reload();
            sections.reboot_banner.set_visible(nvidia_reboot_required());
            spawn_health_check(ui_tx.clone());
            close_dialog();
        }
    });

    // Initial probe.
    spawn_probe();
    win.present();
}

fn build_row_widget(
    row: &DashRow,
    dash_tx: &mpsc::Sender<DashMsg>,
    busy: &Rc<Cell<bool>>,
    parent: &gtk::Window,
    seeding_gpu: &Rc<Cell<bool>>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card.add_css_class("metis-settings-wizard-dash-card");
    match row.kind {
        CardKind::Ready => card.add_css_class("metis-settings-wizard-dash-ready"),
        CardKind::Attention => card.add_css_class("metis-settings-wizard-dash-attention"),
        CardKind::Optional => card.add_css_class("metis-settings-wizard-dash-optional"),
    }
    card.set_hexpand(true);

    let icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icon_wrap.add_css_class("metis-settings-wizard-dash-icon");
    icon_wrap.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(row.icon);
    icon.set_pixel_size(20);
    icon_wrap.append(&icon);
    card.append(&icon_wrap);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&row.title));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-wizard-dash-title");
    title_row.append(&title);
    let pill = gtk::Label::new(Some(&pill_label(row.kind)));
    pill.add_css_class("metis-settings-wizard-pill");
    match row.kind {
        CardKind::Ready => pill.add_css_class("metis-settings-wizard-pill-ready"),
        CardKind::Attention => pill.add_css_class("metis-settings-wizard-pill-attention"),
        CardKind::Optional => pill.add_css_class("metis-settings-wizard-pill-optional"),
    }
    title_row.append(&pill);
    text.append(&title_row);
    let detail = gtk::Label::new(Some(&row.detail));
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail.set_width_chars(24);
    detail.set_max_width_chars(42);
    detail.add_css_class("metis-settings-hint");
    text.append(&detail);

    if row.show_gpu_dropdown {
        let cfg = load_gaming_config();
        let labels = [
            tr("Auto (games on discrete GPU)"),
            tr("Desktop iGPU / games dGPU"),
            tr("Always discrete GPU"),
            tr("Always integrated GPU"),
            tr("Off (manual only)"),
        ];
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dd = gtk::DropDown::from_strings(&refs);
        seeding_gpu.set(true);
        dd.set_selected(graphics_mode_to_index(cfg.graphics_mode));
        dd.set_halign(gtk::Align::Start);
        dd.set_margin_top(6);
        dd.set_size_request(260, -1);
        {
            let seeding_for_idle = seeding_gpu.clone();
            let seeding_for_notify = seeding_gpu.clone();
            let dash_tx = dash_tx.clone();
            dd.connect_selected_notify(move |dd| {
                if seeding_for_notify.get() {
                    return;
                }
                let mode = index_to_graphics_mode(dd.selected());
                let mut cfg = load_gaming_config();
                cfg.graphics_mode = mode;
                if let Err(e) = save_gaming_config(&cfg) {
                    let _ = dash_tx.send(DashMsg::FixDone {
                        summary: format!("Could not save gaming.json: {e}"),
                    });
                    return;
                }
                metis_gaming::session::request_reload();
                let _ = dash_tx.send(DashMsg::FixDone {
                    summary: tr("Graphics mode updated"),
                });
            });
            glib::idle_add_local_once(move || seeding_for_idle.set(false));
        }
        text.append(&dd);
    }
    card.append(&text);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_valign(gtk::Align::Center);
    actions.set_halign(gtk::Align::End);

    let show_fix = row.kind == CardKind::Attention
        || (row.id == "finish" && row.kind != CardKind::Ready)
        || (row.id == "gamemode" && row.kind == CardKind::Optional && !row.fix_ids.is_empty());
    if show_fix && (!row.fix_ids.is_empty() || row.nvidia_install) {
        let label = if row.nvidia_install {
            tr("Install…")
        } else if row.id == "finish" {
            tr("Apply")
        } else if row.fix_ids.iter().any(|id| id == "steam")
            || (row.id == "gamemode" && row.kind == CardKind::Optional)
        {
            tr("Install")
        } else {
            tr("Fix")
        };
        let btn = gtk::Button::with_label(&label);
        btn.add_css_class("suggested-action");
        let fix_ids = row.fix_ids.clone();
        let nvidia = row.nvidia_install;
        let flatpak = row.flatpak_confirm;
        let dash_tx = dash_tx.clone();
        let busy = busy.clone();
        let parent = parent.clone();
        btn.connect_clicked(move |btn| {
            if busy.get() {
                return;
            }
            let run_fix = {
                let dash_tx = dash_tx.clone();
                let busy = busy.clone();
                let fix_ids = fix_ids.clone();
                let btn = btn.clone();
                move || {
                    busy.set(true);
                    btn.set_sensitive(false);
                    let _ = dash_tx.send(DashMsg::Busy(true));
                    let dash_tx = dash_tx.clone();
                    let fix_ids = fix_ids.clone();
                    std::thread::spawn(move || {
                        let mut notes = Vec::new();
                        for id in &fix_ids {
                            match auto_fix_item(id) {
                                Ok(m) => notes.push(m),
                                Err(e) => notes.push(e),
                            }
                        }
                        let _ = dash_tx.send(DashMsg::FixDone {
                            summary: notes.join(" · "),
                        });
                    });
                }
            };
            if nvidia {
                let dash_tx = dash_tx.clone();
                let busy = busy.clone();
                let btn = btn.clone();
                show_nvidia_consent_dialog(&parent, move || {
                    busy.set(true);
                    btn.set_sensitive(false);
                    let _ = dash_tx.send(DashMsg::Busy(true));
                    let dash_tx = dash_tx.clone();
                    std::thread::spawn(move || {
                        let summary = match install_nvidia_drivers() {
                            Ok(m) => m,
                            Err(e) => e,
                        };
                        let _ = dash_tx.send(DashMsg::FixDone { summary });
                    });
                });
            } else if flatpak {
                show_optimize_confirm_dialog(&parent, run_fix);
            } else {
                run_fix();
            }
        });
        actions.append(&btn);
    }

    if row.open_steam {
        let open = gtk::Button::with_label(&tr("Open Steam"));
        open.add_css_class("metis-settings-secondary");
        open.connect_clicked(|_| {
            let _ = std::process::Command::new("sh")
                .args([
                    "-c",
                    "command -v steam >/dev/null && exec steam || flatpak run com.valvesoftware.Steam",
                ])
                .spawn();
        });
        actions.append(&open);
    }

    if row.kind == CardKind::Ready && !row.show_gpu_dropdown && !row.open_steam {
        let recheck = gtk::Button::with_label(&tr("Recheck"));
        recheck.add_css_class("metis-settings-secondary");
        let dash_tx = dash_tx.clone();
        let busy = busy.clone();
        recheck.connect_clicked(move |_| {
            if busy.get() {
                return;
            }
            busy.set(true);
            let _ = dash_tx.send(DashMsg::Busy(true));
            let dash_tx = dash_tx.clone();
            std::thread::spawn(move || {
                let check = run_health_check();
                let _ = dash_tx.send(DashMsg::Probed(check));
            });
        });
        actions.append(&recheck);
    }

    card.append(&actions);
    card
}

fn pill_label(kind: CardKind) -> String {
    match kind {
        CardKind::Ready => tr("Ready"),
        CardKind::Attention => tr("Needs attention"),
        CardKind::Optional => tr("Optional"),
    }
}

fn find_item<'a>(check: &'a HealthCheck, id: &str) -> Option<&'a HealthItem> {
    check.items.iter().find(|i| i.id == id)
}

fn severity_kind(sev: HealthSeverity) -> CardKind {
    match sev {
        HealthSeverity::Ok => CardKind::Ready,
        HealthSeverity::Info => CardKind::Optional,
        HealthSeverity::Warn | HealthSeverity::Error => CardKind::Attention,
    }
}

fn merge_kind(a: CardKind, b: CardKind) -> CardKind {
    use CardKind::*;
    match (a, b) {
        (Attention, _) | (_, Attention) => Attention,
        (Optional, Ready) | (Ready, Optional) | (Optional, Optional) => Optional,
        (Ready, Ready) => Ready,
    }
}

fn build_dashboard_rows(check: &HealthCheck) -> Vec<DashRow> {
    let mut rows = Vec::new();

    // Steam
    if let Some(item) = find_item(check, "steam") {
        let ready = item.severity == HealthSeverity::Ok;
        rows.push(DashRow {
            id: "steam".into(),
            title: tr("Steam"),
            detail: item.detail.clone(),
            icon: "system-software-install-symbolic",
            kind: severity_kind(item.severity),
            fix_ids: if ready {
                vec![]
            } else {
                vec!["steam".into()]
            },
            nvidia_install: false,
            flatpak_confirm: false,
            show_gpu_dropdown: false,
            open_steam: ready,
        });
    }

    // Vulkan (merged)
    {
        let v64 = find_item(check, "mesa_vulkan");
        let v32 = find_item(check, "vulkan_i386");
        let mut kind = CardKind::Ready;
        let mut details = Vec::new();
        let mut fix_ids = Vec::new();
        if let Some(i) = v64 {
            kind = merge_kind(kind, severity_kind(i.severity));
            details.push(format!("64-bit: {}", i.detail));
            if i.severity != HealthSeverity::Ok {
                fix_ids.push("mesa_vulkan".into());
            }
        }
        if let Some(i) = v32 {
            kind = merge_kind(kind, severity_kind(i.severity));
            details.push(format!("32-bit: {}", i.detail));
            if i.severity != HealthSeverity::Ok {
                fix_ids.push("vulkan_i386".into());
            }
        }
        if v64.is_some() || v32.is_some() {
            rows.push(DashRow {
                id: "vulkan".into(),
                title: tr("Vulkan"),
                detail: if details.is_empty() {
                    tr("Vulkan drivers")
                } else {
                    details.join(" · ")
                },
                icon: "video-display-symbolic",
                kind,
                fix_ids,
                nvidia_install: false,
                flatpak_confirm: false,
                show_gpu_dropdown: false,
                open_steam: false,
            });
        }
    }

    // Controllers (merged)
    {
        let devices = find_item(check, "steam_devices");
        let group = find_item(check, "input_group");
        let mut kind = CardKind::Ready;
        let mut details = Vec::new();
        let mut fix_ids = Vec::new();
        if let Some(i) = devices {
            kind = merge_kind(kind, severity_kind(i.severity));
            details.push(format!("udev: {}", i.detail));
            if i.severity != HealthSeverity::Ok {
                fix_ids.push("steam_devices".into());
            }
        }
        if let Some(i) = group {
            kind = merge_kind(kind, severity_kind(i.severity));
            details.push(format!("input group: {}", i.detail));
            if i.severity != HealthSeverity::Ok {
                fix_ids.push("input_group".into());
            }
        }
        if devices.is_some() || group.is_some() {
            rows.push(DashRow {
                id: "controllers".into(),
                title: tr("Controllers"),
                detail: if details.is_empty() {
                    tr("Controller access")
                } else {
                    details.join(" · ")
                },
                icon: "input-gaming-symbolic",
                kind,
                fix_ids,
                nvidia_install: false,
                flatpak_confirm: false,
                show_gpu_dropdown: false,
                open_steam: false,
            });
        }
    }

    // GameMode
    if let Some(item) = find_item(check, "gamemode") {
        rows.push(DashRow {
            id: "gamemode".into(),
            title: tr("GameMode"),
            detail: item.detail.clone(),
            icon: "power-profile-performance-symbolic",
            kind: severity_kind(item.severity),
            fix_ids: if item.severity == HealthSeverity::Ok {
                vec![]
            } else {
                vec!["gamemode".into()]
            },
            nvidia_install: false,
            flatpak_confirm: false,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    } else {
        // auto_gamemode off and not installed — still show as optional install.
        rows.push(DashRow {
            id: "gamemode".into(),
            title: tr("GameMode"),
            detail: tr("Not required — install for automatic performance boosts."),
            icon: "power-profile-performance-symbolic",
            kind: CardKind::Optional,
            fix_ids: vec!["gamemode".into()],
            nvidia_install: false,
            flatpak_confirm: false,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    }

    // GPU mode (always)
    {
        let cfg = load_gaming_config();
        let (kind, detail) = if cfg.graphics_mode == GraphicsMode::Auto {
            (
                CardKind::Ready,
                tr("Auto: desktop on iGPU, games on discrete when present."),
            )
        } else {
            (
                CardKind::Optional,
                format!(
                    "{} — change below anytime.",
                    graphics_mode_label(cfg.graphics_mode)
                ),
            )
        };
        rows.push(DashRow {
            id: "gpu_mode".into(),
            title: tr("GPU mode"),
            detail,
            icon: "preferences-desktop-display-symbolic",
            kind,
            fix_ids: vec![],
            nvidia_install: false,
            flatpak_confirm: false,
            show_gpu_dropdown: true,
            open_steam: false,
        });
    }

    // NVIDIA (when present in health, or GPU present)
    if let Some(item) = find_item(check, "nvidia_driver") {
        let needs = item.severity != HealthSeverity::Ok;
        rows.push(DashRow {
            id: "nvidia_driver".into(),
            title: tr("NVIDIA drivers"),
            detail: item.detail.clone(),
            icon: "drive-harddisk-solidstate-symbolic",
            kind: severity_kind(item.severity),
            fix_ids: vec![],
            nvidia_install: needs && !item.detail.to_ascii_lowercase().contains("reboot"),
            flatpak_confirm: false,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    } else if nvidia_gpu_present() {
        rows.push(DashRow {
            id: "nvidia_driver".into(),
            title: tr("NVIDIA drivers"),
            detail: tr("Driver loaded."),
            icon: "drive-harddisk-solidstate-symbolic",
            kind: CardKind::Ready,
            fix_ids: vec![],
            nvidia_install: false,
            flatpak_confirm: false,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    }

    // Flatpak / finish
    if let Some(item) = find_item(check, "flatpak_steam") {
        rows.push(DashRow {
            id: "finish".into(),
            title: tr("Flatpak Steam overrides"),
            detail: item.detail.clone(),
            icon: "emblem-ok-symbolic",
            kind: severity_kind(item.severity),
            fix_ids: vec!["flatpak_steam".into()],
            nvidia_install: false,
            flatpak_confirm: true,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    } else {
        let steam = detect_steam();
        let detail = match steam {
            SteamInstall::Flatpak => tr("Apply gaming overrides and Metis launcher wrapper."),
            SteamInstall::Native => tr("Native Steam — launcher wrapper optional."),
            SteamInstall::None => tr("Install Steam first, then apply Flatpak overrides if needed."),
        };
        let kind = match steam {
            SteamInstall::Flatpak => CardKind::Optional,
            SteamInstall::Native => CardKind::Ready,
            SteamInstall::None => CardKind::Optional,
        };
        rows.push(DashRow {
            id: "finish".into(),
            title: tr("Finish & optimize"),
            detail,
            icon: "emblem-ok-symbolic",
            kind,
            fix_ids: vec!["flatpak_steam".into()],
            nvidia_install: false,
            flatpak_confirm: true,
            show_gpu_dropdown: false,
            open_steam: false,
        });
    }

    rows
}

fn graphics_mode_to_index(mode: GraphicsMode) -> u32 {
    match mode {
        GraphicsMode::Auto => 0,
        GraphicsMode::DesktopIgpuGamesDgpu => 1,
        GraphicsMode::AlwaysDgpu => 2,
        GraphicsMode::AlwaysIgpu => 3,
        GraphicsMode::Off => 4,
    }
}

fn index_to_graphics_mode(idx: u32) -> GraphicsMode {
    match idx {
        1 => GraphicsMode::DesktopIgpuGamesDgpu,
        2 => GraphicsMode::AlwaysDgpu,
        3 => GraphicsMode::AlwaysIgpu,
        4 => GraphicsMode::Off,
        _ => GraphicsMode::Auto,
    }
}

fn graphics_mode_label(mode: GraphicsMode) -> String {
    match mode {
        GraphicsMode::Auto => tr("Auto"),
        GraphicsMode::DesktopIgpuGamesDgpu => tr("Desktop iGPU / games dGPU"),
        GraphicsMode::AlwaysDgpu => tr("Always discrete"),
        GraphicsMode::AlwaysIgpu => tr("Always integrated"),
        GraphicsMode::Off => tr("Off (manual)"),
    }
}
