//! Gaming Platform 2.0 — graphics mode, health checks, devices, and optimize.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gio::prelude::*;
use gtk::prelude::*;
use metis_config::{
    load_app_config, load_gaming_config, save_app_config, save_gaming_config,
    validate_steam_library_path, GamingConfig, GraphicsMode, XwaylandMode,
};
use metis_gaming::health::{
    auto_fix_item, install_nvidia_drivers, run_health_check, HealthCheck, HealthSeverity,
};
use metis_gaming::{nvidia_gpu_present, nvidia_reboot_required};

use crate::gaming::{GamingSnapshot, InputDevice, SteamInstall};
use crate::ui;
use metis_i18n::tr;

/// Background thread → GTK main thread (widgets are not `Send`).
enum GamingUiEvent {
    HealthCheck(HealthCheck),
    /// Optimizer finished; `summary` is shown in the status banner.
    OptimizeDone {
        summary: String,
    },
    /// Per-row Fix finished; refresh health + show `summary`.
    FixDone {
        summary: String,
    },
}

thread_local! {
    static GAMING_SETUP_DIALOG: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

struct Sections {
    steam: gtk::Label,
    gpu: gtk::Label,
    graphics_mode: gtk::DropDown,
    on_battery: gtk::Switch,
    auto_perf: gtk::Switch,
    auto_gamemode: gtk::Switch,
    flatpak_gpu: gtk::Switch,
    mangohud: gtk::Switch,
    gamescope_bp: gtk::Switch,
    xwayland_isolated: gtk::Switch,
    steam_paths_list: gtk::Box,
    reboot_banner: gtk::Box,
    health_list: gtk::Grid,
    gamepad_list: gtk::Box,
    touch_list: gtk::Box,
    status_box: gtk::Box,
    status_icon: gtk::Image,
    status_text: gtk::Label,
    #[allow(dead_code)] // kept for lifetime
    setup_btn: gtk::Button,
    optimize_btn: gtk::Button,
    seeding: Rc<RefCell<bool>>,
    last_health_sig: Rc<Cell<u64>>,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("gaming");
    let cfg = load_gaming_config();
    let seeding = Rc::new(RefCell::new(true));

    let (mode_card, mode_body) = ui::section_with_icon(&tr("Graphics"), "video-display-symbolic");

    let graphics_mode = {
        let __dd_labels = [
            tr("Auto (games on discrete GPU)"),
            tr("Desktop iGPU / games dGPU"),
            tr("Always discrete GPU"),
            tr("Always integrated GPU"),
            tr("Off (manual only)"),
        ];
        let __dd_refs: Vec<&str> = __dd_labels.iter().map(|s| s.as_str()).collect();
        gtk::DropDown::from_strings(&__dd_refs)
    };
    graphics_mode.set_selected(graphics_mode_to_index(cfg.graphics_mode));
    mode_body.append(&ui::row(&tr("Graphics mode"), &graphics_mode));

    let on_battery = gtk::Switch::new();
    on_battery.set_active(cfg.on_battery_prefer_igpu);
    on_battery.set_halign(gtk::Align::End);
    mode_body.append(&ui::row(&tr("Prefer iGPU on battery"), &on_battery));

    let auto_perf = gtk::Switch::new();
    auto_perf.set_active(cfg.auto_performance_profile);
    auto_perf.set_halign(gtk::Align::End);
    mode_body.append(&ui::row(
        &tr("Performance profile while gaming"),
        &auto_perf,
    ));

    let auto_gamemode = gtk::Switch::new();
    auto_gamemode.set_active(cfg.auto_gamemode);
    auto_gamemode.set_halign(gtk::Align::End);
    mode_body.append(&ui::row(&tr("Auto GameMode"), &auto_gamemode));

    let flatpak_gpu = gtk::Switch::new();
    flatpak_gpu.set_active(cfg.flatpak_gpu_env);
    flatpak_gpu.set_halign(gtk::Align::End);
    mode_body.append(&ui::row(&tr("Flatpak GPU offload env"), &flatpak_gpu));

    let mangohud = gtk::Switch::new();
    mangohud.set_active(cfg.mangohud_for_games);
    mangohud.set_halign(gtk::Align::End);
    mangohud.set_tooltip_text(Some(&tr(
        "When Metis launches Steam or Big Picture and mangohud is installed, set MANGOHUD=1. \
         Does not edit Steam Launch Options.",
    )));
    mode_body.append(&ui::row(
        &tr("MangoHud for Metis Steam launches"),
        &mangohud,
    ));

    let gamescope_bp = gtk::Switch::new();
    gamescope_bp.set_active(cfg.gamescope_big_picture);
    gamescope_bp.set_halign(gtk::Align::End);
    gamescope_bp.set_tooltip_text(Some(&tr(
        "When Metis launches Big Picture and gamescope is installed, wrap with gamescope --. \
         Does not edit Steam Properties.",
    )));
    mode_body.append(&ui::row(&tr("Gamescope for Big Picture"), &gamescope_bp));

    let app_cfg = load_app_config();
    let xwayland_isolated = gtk::Switch::new();
    xwayland_isolated.set_active(app_cfg.xwayland_mode == XwaylandMode::Isolated);
    xwayland_isolated.set_halign(gtk::Align::End);
    xwayland_isolated.set_tooltip_text(Some(&tr(
        "Soft isolation: Steam/Proton and similar Metis launches get a separate XWayland. \
         Not a sandbox — restart the Metis session after changing. Same-UID apps can still \
         open either display.",
    )));
    mode_body.append(&ui::row(
        &tr("Isolated X11 (gaming bucket)"),
        &xwayland_isolated,
    ));
    let x11_hint = gtk::Label::new(Some(&tr(
        "Restart the Metis session to apply X11 isolation changes. Soft bucketing only — \
         not a security sandbox. Leave Steam Launch Options empty for GPU — Metis session \
         offload handles hybrid routing.",
    )));
    x11_hint.set_wrap(true);
    x11_hint.set_xalign(0.0);
    x11_hint.add_css_class("metis-settings-hint");
    mode_body.append(&x11_hint);

    let reboot_banner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    reboot_banner.add_css_class("metis-settings-gaming-status");
    reboot_banner.set_visible(nvidia_reboot_required());
    let reboot_icon = gtk::Image::from_icon_name("system-reboot-symbolic");
    reboot_icon.set_pixel_size(18);
    let reboot_text = gtk::Label::new(Some(&tr(
        "NVIDIA drivers were installed — reboot to load them before gaming.",
    )));
    reboot_text.set_xalign(0.0);
    reboot_text.set_wrap(true);
    reboot_text.set_hexpand(true);
    reboot_banner.append(&reboot_icon);
    reboot_banner.append(&reboot_text);
    mode_body.append(&reboot_banner);

    let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    status_box.add_css_class("metis-settings-gaming-status");
    status_box.set_valign(gtk::Align::Center);
    let status_icon = gtk::Image::from_icon_name("dialog-information-symbolic");
    status_icon.add_css_class("metis-settings-gaming-status-icon");
    status_icon.set_pixel_size(18);
    let status_text = gtk::Label::new(Some(&tr("Checking gaming health…")));
    status_text.set_xalign(0.0);
    status_text.set_hexpand(true);
    status_text.set_wrap(true);
    status_text.add_css_class("metis-settings-gaming-status-text");
    status_box.append(&status_icon);
    status_box.append(&status_text);
    mode_body.append(&status_box);

    let actions = gtk::Box::new(gtk::Orientation::Vertical, 8);
    actions.add_css_class("metis-settings-actions");

    let optimize_btn = gtk::Button::with_label(&tr("Optimize now"));
    optimize_btn.add_css_class("suggested-action");
    optimize_btn.set_halign(gtk::Align::Start);
    optimize_btn.set_tooltip_text(Some(&tr(
        "Apply Flatpak gaming overrides and add you to the input group if needed",
    )));
    actions.append(&optimize_btn);

    let setup_btn = gtk::Button::with_label(&tr("Run gaming setup"));
    setup_btn.set_halign(gtk::Align::Start);
    setup_btn.set_tooltip_text(Some(&tr(
        "Guided setup: Steam, Vulkan, controllers, GameMode, GPU mode, drivers, Flatpak",
    )));
    actions.append(&setup_btn);
    mode_body.append(&actions);

    content.append(&mode_card);

    let (paths_card, paths_body) =
        ui::section_with_icon(&tr("Steam library paths"), "folder-symbolic");
    let paths_hint = gtk::Label::new(Some(&tr(
        "Extra host folders granted to Flatpak Steam (--filesystem). Paths must resolve under \
         your home directory, /mnt, /media, or /run/media.",
    )));
    paths_hint.set_wrap(true);
    paths_hint.set_xalign(0.0);
    paths_hint.add_css_class("metis-settings-hint");
    paths_body.append(&paths_hint);
    let steam_paths_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    steam_paths_list.add_css_class("metis-settings-list");
    paths_body.append(&steam_paths_list);
    let paths_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    paths_actions.set_margin_top(8);
    let add_path_btn = gtk::Button::with_label(&tr("Add folder…"));
    add_path_btn.add_css_class("metis-settings-secondary");
    let optimize_paths_btn = gtk::Button::with_label(&tr("Optimize Flatpak Steam"));
    optimize_paths_btn.add_css_class("suggested-action");
    paths_actions.append(&add_path_btn);
    paths_actions.append(&optimize_paths_btn);
    paths_body.append(&paths_actions);
    content.append(&paths_card);

    let (health_card, health_body) = ui::section(&tr("Health check"));
    let health_list = gtk::Grid::new();
    health_list.add_css_class("metis-settings-health-grid");
    health_list.set_column_spacing(12);
    health_list.set_row_spacing(2);
    health_list.set_column_homogeneous(true);
    health_list.set_hexpand(true);
    health_list.set_halign(gtk::Align::Fill);
    health_body.append(&health_list);
    content.append(&health_card);

    let (session_card, session_body) =
        ui::section_with_icon(&tr("Session"), "applications-games-symbolic");
    let steam = value_label(&tr("Checking…"));
    session_body.append(&readout_row(&tr("Steam"), &steam));
    let gpu = value_label("");
    gpu.set_wrap(true);
    gpu.add_css_class("metis-settings-hint");
    session_body.append(&readout_row(&tr("GPU"), &gpu));
    content.append(&session_card);

    let (pad_card, pad_body) = ui::section_with_icon(&tr("Gamepads"), "input-gamepad-symbolic");
    let gamepad_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    gamepad_list.add_css_class("metis-settings-list");
    pad_body.append(&gamepad_list);
    content.append(&pad_card);

    let (touch_card, touch_body) =
        ui::section_with_icon(&tr("Touchscreens"), "input-touchpad-symbolic");
    let touch_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    touch_list.add_css_class("metis-settings-list");
    touch_body.append(&touch_list);
    content.append(&touch_card);

    let sections = Rc::new(Sections {
        steam,
        gpu,
        graphics_mode,
        on_battery,
        auto_perf,
        auto_gamemode,
        flatpak_gpu,
        mangohud,
        gamescope_bp,
        xwayland_isolated,
        steam_paths_list,
        reboot_banner,
        health_list,
        gamepad_list,
        touch_list,
        status_box,
        status_icon,
        status_text,
        setup_btn: setup_btn.clone(),
        optimize_btn: optimize_btn.clone(),
        seeding: seeding.clone(),
        last_health_sig: Rc::new(Cell::new(0)),
    });
    refresh_steam_paths_list(&sections);

    let persist_cfg = {
        let seeding = sections.seeding.clone();
        Rc::new(move |mutate: Box<dyn FnOnce(&mut GamingConfig)>| {
            if *seeding.borrow() {
                return;
            }
            let mut cfg = load_gaming_config();
            mutate(&mut cfg);
            if save_gaming_config(&cfg).is_ok() {
                crate::runtime::reload_gaming_async();
            }
        })
    };

    {
        let persist_cfg = persist_cfg.clone();
        let seeding = sections.seeding.clone();
        sections.graphics_mode.connect_selected_notify(move |dd| {
            if *seeding.borrow() {
                return;
            }
            let idx = dd.selected();
            persist_cfg(Box::new(move |c| {
                c.graphics_mode = index_to_graphics_mode(idx)
            }));
        });
    }
    connect_switch_persist(
        &sections.on_battery,
        sections.seeding.clone(),
        persist_cfg.clone(),
        |c, v| {
            c.on_battery_prefer_igpu = v;
        },
    );
    connect_switch_persist(
        &sections.auto_perf,
        sections.seeding.clone(),
        persist_cfg.clone(),
        |c, v| {
            c.auto_performance_profile = v;
        },
    );
    connect_switch_persist(
        &sections.auto_gamemode,
        sections.seeding.clone(),
        persist_cfg.clone(),
        |c, v| {
            c.auto_gamemode = v;
        },
    );
    connect_switch_persist(
        &sections.flatpak_gpu,
        sections.seeding.clone(),
        persist_cfg.clone(),
        |c, v| {
            c.flatpak_gpu_env = v;
        },
    );
    connect_switch_persist(
        &sections.mangohud,
        sections.seeding.clone(),
        persist_cfg.clone(),
        |c, v| {
            c.mangohud_for_games = v;
        },
    );
    connect_switch_persist(
        &sections.gamescope_bp,
        sections.seeding.clone(),
        persist_cfg,
        |c, v| {
            c.gamescope_big_picture = v;
        },
    );

    {
        let seeding = sections.seeding.clone();
        sections.xwayland_isolated.connect_active_notify(move |sw| {
            if *seeding.borrow() {
                return;
            }
            let mut cfg = load_app_config();
            cfg.xwayland_mode = if sw.is_active() {
                XwaylandMode::Isolated
            } else {
                XwaylandMode::Shared
            };
            if let Err(err) = save_app_config(&cfg) {
                tracing::warn!(%err, "failed to save xwayland_mode");
            }
        });
    }

    let (tx, rx) = mpsc::channel::<GamingSnapshot>();
    let (ui_tx, ui_rx) = mpsc::channel::<GamingUiEvent>();

    {
        let ui_tx = ui_tx.clone();
        optimize_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                tracing::warn!("gaming optimize: no parent window");
                return;
            };
            let btn = btn.clone();
            let ui_tx = ui_tx.clone();
            show_optimize_confirm_dialog(&parent, move || {
                btn.set_sensitive(false);
                btn.set_label(&tr("Optimizing…"));
                let ui_tx = ui_tx.clone();
                std::thread::spawn(move || {
                    let summary = run_optimize_pass();
                    let _ = ui_tx.send(GamingUiEvent::OptimizeDone { summary });
                });
            });
        });
    }

    {
        let sections_s = sections.clone();
        let ui_tx = ui_tx.clone();
        setup_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                tracing::warn!("gaming setup: no parent window");
                return;
            };
            show_gaming_setup_dialog(&parent, sections_s.clone(), ui_tx.clone());
        });
    }

    {
        let sections_paths = sections.clone();
        add_path_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title(tr("Steam library folder"))
                .modal(true)
                .build();
            let sections_paths = sections_paths.clone();
            dialog.select_folder(
                Some(&parent),
                None::<&gio::Cancellable>,
                move |result| {
                    let Ok(folder) = result else {
                        return;
                    };
                    let Some(path) = folder.path() else {
                        return;
                    };
                    let raw = path.display().to_string();
                    match validate_steam_library_path(&raw) {
                        Some(canon) => {
                            let mut cfg = load_gaming_config();
                            let s = canon.to_string_lossy().into_owned();
                            if !cfg.extra_steam_paths.contains(&s) {
                                cfg.extra_steam_paths.push(s);
                                if save_gaming_config(&cfg).is_ok() {
                                    crate::runtime::reload_gaming_async();
                                    refresh_steam_paths_list(&sections_paths);
                                }
                            }
                        }
                        None => {
                            sections_paths.status_text.set_text(&tr(
                                "That folder is not allowed — use a path under home, /mnt, /media, or /run/media.",
                            ));
                        }
                    }
                },
            );
        });
    }
    {
        let ui_tx = ui_tx.clone();
        optimize_paths_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            let btn = btn.clone();
            let ui_tx = ui_tx.clone();
            show_optimize_confirm_dialog(&parent, move || {
                let ui_tx = ui_tx.clone();
                std::thread::spawn(move || {
                    let summary = run_optimize_pass();
                    let _ = ui_tx.send(GamingUiEvent::OptimizeDone { summary });
                });
            });
            let _ = &btn;
        });
    }

    let refresh_devices = {
        let tx = tx.clone();
        Rc::new(move || {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::gaming::load_snapshot());
            });
        })
    };

    {
        let sections = sections.clone();
        let ui_tx_poll = ui_tx.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            while let Ok(snapshot) = rx.try_recv() {
                apply_snapshot(&sections, &snapshot);
            }
            while let Ok(evt) = ui_rx.try_recv() {
                match evt {
                    GamingUiEvent::HealthCheck(check) => {
                        apply_health_check(&sections, &check, ui_tx_poll.clone());
                    }
                    GamingUiEvent::OptimizeDone { summary } => {
                        sections.optimize_btn.set_sensitive(true);
                        sections.optimize_btn.set_label(&tr("Optimize now"));
                        sections.status_text.set_text(&summary);
                        spawn_health_check(ui_tx_poll.clone());
                    }
                    GamingUiEvent::FixDone { summary } => {
                        sections.status_text.set_text(&summary);
                        spawn_health_check(ui_tx_poll.clone());
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    spawn_health_check(ui_tx.clone());
    refresh_devices();
    sections.seeding.replace(false);

    glib::timeout_add_seconds_local(4, move || {
        refresh_devices();
        glib::ControlFlow::Continue
    });

    scroller.upcast()
}

fn spawn_health_check(tx: mpsc::Sender<GamingUiEvent>) {
    std::thread::spawn(move || {
        let check = run_health_check();
        let _ = tx.send(GamingUiEvent::HealthCheck(check));
    });
}

/// Permission-review dialog before writing Flatpak gaming overrides.
fn show_optimize_confirm_dialog(parent: &gtk::Window, on_confirm: impl Fn() + 'static) {
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
fn run_optimize_pass() -> String {
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

fn show_gaming_setup_dialog(
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
    if need_nvidia {
        6
    } else {
        5
    }
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

fn show_nvidia_consent_dialog(parent: &gtk::Window, on_confirm: impl Fn() + 'static) {
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

fn refresh_steam_paths_list(sections: &Rc<Sections>) {
    while let Some(child) = sections.steam_paths_list.first_child() {
        sections.steam_paths_list.remove(&child);
    }
    let cfg = load_gaming_config();
    if cfg.extra_steam_paths.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No extra library paths configured.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        sections.steam_paths_list.append(&empty);
        return;
    }
    for path in cfg.extra_steam_paths {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("metis-settings-row");
        let label = gtk::Label::new(Some(&path));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        row.append(&label);
        let remove = gtk::Button::with_label(&tr("Remove"));
        remove.add_css_class("metis-settings-secondary");
        let path_rm = path.clone();
        let sections_rm = Rc::clone(sections);
        remove.connect_clicked(move |_| {
            let mut cfg = load_gaming_config();
            cfg.extra_steam_paths.retain(|p| p != &path_rm);
            if save_gaming_config(&cfg).is_ok() {
                crate::runtime::reload_gaming_async();
                refresh_steam_paths_list(&sections_rm);
                sections_rm.status_text.set_text(&tr(
                    "Path removed — run Optimize Flatpak Steam to update overrides.",
                ));
            }
        });
        row.append(&remove);
        sections.steam_paths_list.append(&row);
    }
}

fn connect_switch_persist(
    sw: &gtk::Switch,
    seeding: Rc<RefCell<bool>>,
    persist: crate::gtk_cb::GamingPersist,
    set: fn(&mut GamingConfig, bool),
) {
    sw.connect_active_notify(move |s| {
        if *seeding.borrow() {
            return;
        }
        let active = s.is_active();
        persist(Box::new(move |c| set(c, active)));
    });
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

fn health_signature(check: &HealthCheck) -> u64 {
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

fn apply_health_check(
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
        let detail = gtk::Label::new(Some(&item.detail));
        detail.set_xalign(0.0);
        detail.set_wrap(true);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.set_max_width_chars(28);
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

fn update_health_summary(sections: &Rc<Sections>, check: &HealthCheck) {
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
            .set_from_icon_name(Some("emblem-ok-symbolic"));
        sections
            .status_text
            .set_text(&tr("Ready for gaming — all checks passed."));
        sections
            .status_box
            .add_css_class("metis-settings-gaming-status-ok");
    } else if issues == 0 {
        sections
            .status_icon
            .set_from_icon_name(Some("dialog-information-symbolic"));
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
            .set_from_icon_name(Some("dialog-warning-symbolic"));
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

fn index_to_graphics_mode(idx: u32) -> GraphicsMode {
    match idx {
        1 => GraphicsMode::DesktopIgpuGamesDgpu,
        2 => GraphicsMode::AlwaysDgpu,
        3 => GraphicsMode::AlwaysIgpu,
        4 => GraphicsMode::Off,
        _ => GraphicsMode::Auto,
    }
}

fn value_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("metis-settings-value");
    label
}

fn readout_row(title: &str, value: &gtk::Label) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    row.append(&title);
    row.append(value);
    row
}

fn apply_snapshot(sections: &Rc<Sections>, snapshot: &GamingSnapshot) {
    sections.steam.set_text(&match snapshot.steam {
        SteamInstall::Native => tr("Installed (native)"),
        SteamInstall::Flatpak => tr("Installed (Flatpak)"),
        SteamInstall::None => tr("Not detected"),
    });
    sections.gpu.set_text(&snapshot.gpu_hint);
    rebuild_device_list(
        &sections.gamepad_list,
        &snapshot.gamepads,
        &tr("No gamepads detected"),
    );
    rebuild_device_list(
        &sections.touch_list,
        &snapshot.touchscreens,
        &tr("No touchscreens detected"),
    );
}

fn rebuild_device_list(list: &gtk::Box, devices: &[InputDevice], empty_label: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    if devices.is_empty() {
        let label = gtk::Label::new(Some(empty_label));
        label.set_xalign(0.0);
        label.add_css_class("metis-settings-hint");
        list.append(&label);
        return;
    }
    for dev in devices {
        list.append(&device_row(dev));
    }
}

fn device_row(dev: &InputDevice) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.add_css_class("metis-settings-row");
    let title = gtk::Label::new(Some(&dev.name));
    title.set_xalign(0.0);
    row.append(&title);
    row
}
