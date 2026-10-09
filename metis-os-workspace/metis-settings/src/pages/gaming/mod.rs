//! Gaming Platform 2.0 — graphics mode, health checks, devices, and optimize.

mod health;
mod profiles;
mod wizard;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gio::prelude::*;
use gtk::prelude::*;
use metis_config::{
    GamingConfig, GraphicsMode, XwaylandMode, load_app_config, load_gaming_config, save_app_config,
    save_gaming_config, validate_steam_library_path,
};
use metis_gaming::health::HealthCheck;
use metis_gaming::nvidia_reboot_required;
use metis_i18n::tr;

use crate::gaming::{GamingSnapshot, InputDevice, SteamInstall};
use crate::ui;

use health::{
    apply_health_check, run_optimize_pass, show_optimize_confirm_dialog, spawn_health_check,
};
use profiles::{
    refresh_gamescope_profiles_list, refresh_steam_paths_list, show_gamescope_profile_dialog,
};
use wizard::show_gaming_setup_dialog;

/// Background thread → GTK main thread (widgets are not `Send`).
pub(crate) enum GamingUiEvent {
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
    pub(crate) static GAMING_SETUP_DIALOG: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

pub(crate) struct Sections {
    pub(crate) steam: gtk::Label,
    pub(crate) gpu: gtk::Label,
    pub(crate) graphics_mode: gtk::DropDown,
    pub(crate) on_battery: gtk::Switch,
    pub(crate) auto_perf: gtk::Switch,
    pub(crate) auto_gamemode: gtk::Switch,
    pub(crate) flatpak_gpu: gtk::Switch,
    pub(crate) mangohud: gtk::Switch,
    pub(crate) gamescope_bp: gtk::Switch,
    pub(crate) xwayland_isolated: gtk::Switch,
    pub(crate) steam_paths_list: gtk::Box,
    pub(crate) gamescope_profiles_list: gtk::Box,
    pub(crate) reboot_banner: gtk::Box,
    pub(crate) health_list: gtk::Grid,
    pub(crate) gamepad_list: gtk::Box,
    pub(crate) touch_list: gtk::Box,
    pub(crate) status_box: gtk::Box,
    pub(crate) status_icon: gtk::Image,
    pub(crate) status_text: gtk::Label,
    #[allow(dead_code)] // kept for lifetime
    pub(crate) setup_btn: gtk::Button,
    pub(crate) optimize_btn: gtk::Button,
    pub(crate) seeding: Rc<RefCell<bool>>,
    pub(crate) last_health_sig: Rc<Cell<u64>>,
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
    // Long mode labels otherwise inflate the Stack natural width and grow the
    // Settings window only on this page.
    graphics_mode.set_size_request(240, -1);
    graphics_mode.set_hexpand(false);
    graphics_mode.set_halign(gtk::Align::End);
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
    reboot_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    reboot_text.set_width_chars(28);
    reboot_text.set_max_width_chars(48);
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
    status_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_text.set_width_chars(28);
    status_text.set_max_width_chars(48);
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

    let (gs_card, gs_body) =
        ui::section_with_icon(&tr("Gamescope profiles"), "applications-games-symbolic");
    let gs_hint = gtk::Label::new(Some(&tr(
        "Wrap Metis-spawned Steam launches for a specific app id with gamescope \
         (steam -applaunch <id> or steam://rungameid/<id>). Never edits Steam Launch Options. \
         Example flags: -W 1920 -H 1080 -f",
    )));
    gs_hint.set_wrap(true);
    gs_hint.set_xalign(0.0);
    gs_hint.add_css_class("metis-settings-hint");
    gs_body.append(&gs_hint);
    let gamescope_profiles_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    gamescope_profiles_list.add_css_class("metis-settings-list");
    gs_body.append(&gamescope_profiles_list);
    let gs_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    gs_actions.set_margin_top(8);
    let add_profile_btn = gtk::Button::with_label(&tr("Add profile…"));
    add_profile_btn.add_css_class("metis-settings-secondary");
    gs_actions.append(&add_profile_btn);
    gs_body.append(&gs_actions);
    content.append(&gs_card);

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
        gamescope_profiles_list,
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
    refresh_gamescope_profiles_list(&sections);

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

    {
        let sections_gs = sections.clone();
        add_profile_btn.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            show_gamescope_profile_dialog(&parent, None, sections_gs.clone());
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
