//! First-run onboarding wizard: a centered layer-shell overlay that walks the user
//! through theme, wallpaper, clock, edge bar, weather, gaming, and optional host
//! packages before marking `onboarding_complete` in `config.json`.
//!
//! Progress is resumable: `onboarding_step` is written on each Next/Back so a
//! session restart mid-wizard continues where the user left off.
//!
//! Like the startup splash, the layer surface is parked off-screen on dismiss,
//! then dropped (same lifecycle as `splash.rs`) so we never call `destroy()` or
//! `set_visible(false)` while another layer surface is reconfiguring.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::{
    BackgroundKind, BarDisplays, BarPosition, ThemeMode, WeatherConfig, WeatherLocation,
};

/// Metis wordmark (same asset as the splash).
const LOGO_BYTES: &[u8] = include_bytes!("../../assets/metis_logo.png");

const STEP_COUNT: usize = 10;
const FADE: Duration = Duration::from_millis(320);

fn step_titles() -> [String; STEP_COUNT] {
    use metis_i18n::tr;
    [
        tr("Language & region"),
        tr("Welcome to Metis"),
        tr("Choose your style"),
        tr("Pick a wallpaper"),
        tr("Clock format"),
        tr("Edge bar"),
        tr("Weather"),
        tr("Gaming"),
        tr("Optional software"),
        tr("You're all set"),
    ]
}

struct Onboarding {
    window: gtk::Window,
    title: gtk::Label,
    body: gtk::Box,
    stepper: Vec<gtk::Box>,
    back_btn: gtk::Button,
    next_btn: gtk::Button,
    step: usize,
    centered: bool,
    fading: bool,
    fade_start: Option<Instant>,
    parked: bool,
}

/// Fixed body height so step swaps do not resize the card.
const BODY_HEIGHT: i32 = 300;
/// Fixed card width (content-sized layer surface — see splash.rs).
const CARD_WIDTH: i32 = 520;
/// Inner step width inside card padding.
const BODY_INNER_WIDTH: i32 = CARD_WIDTH - 72;
/// Wallpaper thumbnail size (two-column grid).
const WALL_W: i32 = 196;
const WALL_H: i32 = 110;

/// True while the onboarding layer surface is visible or fading — bar rebuilds are
/// suppressed until the overlay is parked off-screen (see `handoff_after_park`).
static ONBOARDING_ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static ONBOARDING: RefCell<Option<Rc<RefCell<Onboarding>>>> = const { RefCell::new(None) };
    /// `bar.json` was edited during the wizard — apply after the overlay window drops.
    static BAR_CONFIG_DIRTY: Cell<bool> = const { Cell::new(false) };
    static WEATHER_RELOAD_PENDING: Cell<bool> = const { Cell::new(false) };
    static PARK_HANDOFF_DONE: Cell<bool> = const { Cell::new(false) };
    static GAMING_OPTIMIZE: Cell<bool> = const { Cell::new(true) };
    static GAMING_AUTO_GPU: Cell<bool> = const { Cell::new(true) };
}

/// Whether the onboarding overlay is on-screen or fading out.
pub fn is_active() -> bool {
    ONBOARDING_ACTIVE.load(Ordering::Acquire)
}

fn mark_active() {
    PARK_HANDOFF_DONE.with(|f| f.set(false));
    BAR_CONFIG_DIRTY.with(|f| f.set(false));
    WEATHER_RELOAD_PENDING.with(|f| f.set(false));
    ONBOARDING_ACTIVE.store(true, Ordering::Release);
}

/// Show the wizard when first-run is pending and onboarding is not disabled.
pub fn show_if_needed() {
    if std::env::var("METIS_NO_ONBOARDING")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        tracing::info!("onboarding skipped (METIS_NO_ONBOARDING)");
        if let Err(err) = metis_config::ensure_kitty_defaults() {
            tracing::warn!(%err, "failed to seed kitty.conf defaults");
        }
        return;
    }
    if crate::config::load_app_config().onboarding_complete {
        // Existing sessions: still seed kitty defaults if the user never had a conf.
        if let Err(err) = metis_config::ensure_kitty_defaults() {
            tracing::warn!(%err, "failed to seed kitty.conf defaults");
        }
        return;
    }
    let step = metis_config::clamped_onboarding_step(STEP_COUNT);
    if step > 0 {
        tracing::info!(step, "resuming onboarding mid-wizard");
    }
    show_at_step(step);
}

/// Present the onboarding overlay (first run or re-triggered from Settings).
///
/// Settings "Run setup again" always starts at step 0. First-run / mid-wizard
/// resume uses the persisted `onboarding_step` via [`show_if_needed`].
pub fn show() {
    // Explicit re-open: clear complete flag and start from the beginning so a
    // crash mid re-run still re-enters via `show_if_needed`.
    if let Err(err) = metis_config::reset_onboarding_progress() {
        tracing::warn!(%err, "failed to reset onboarding progress");
    }
    show_at_step(0);
}

fn show_at_step(initial_step: usize) {
    mark_active();
    let initial_step = initial_step.min(STEP_COUNT.saturating_sub(1));
    ONBOARDING.with(|cell| {
        if let Some(ob) = cell.borrow().as_ref() {
            let mut o = ob.borrow_mut();
            if o.fading {
                return;
            }
            o.parked = false;
            o.centered = false;
            o.step = initial_step;
            o.window.set_margin(Edge::Top, 0);
            o.window.set_margin(Edge::Left, 0);
            o.window.set_opacity(0.0);
            o.window.set_keyboard_mode(KeyboardMode::None);
            o.window.set_visible(true);
            refresh_step(&mut o);
        }
    });

    if ONBOARDING.with(|cell| cell.borrow().is_some()) {
        return;
    }

    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Metis Setup"))
        .build();
    window.add_css_class("metis-onboarding-window");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_namespace("metis-onboarding");
    // Content-sized surface anchored top-left, centered via margins (splash pattern).
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_opacity(0.0);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 16);
    card.add_css_class("metis-onboarding-card");
    card.set_hexpand(false);
    card.set_vexpand(false);
    card.set_size_request(CARD_WIDTH, -1);
    card.set_width_request(CARD_WIDTH);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.set_hexpand(true);
    let header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_spacer.set_hexpand(true);
    let skip_btn = gtk::Button::with_label(&metis_i18n::tr("Skip"));
    skip_btn.add_css_class("flat");
    skip_btn.add_css_class("metis-onboarding-skip");
    skip_btn.set_halign(gtk::Align::End);
    header.append(&header_spacer);
    header.append(&skip_btn);
    card.append(&header);

    let title = gtk::Label::new(None);
    title.add_css_class("metis-onboarding-title");
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    card.append(&title);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("metis-onboarding-body");
    body.set_margin_top(4);
    body.set_margin_bottom(4);
    body.set_size_request(BODY_INNER_WIDTH, BODY_HEIGHT);
    body.set_width_request(BODY_INNER_WIDTH);
    body.set_hexpand(false);
    body.set_vexpand(false);
    body.set_overflow(gtk::Overflow::Hidden);
    card.append(&body);

    let stepper_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    stepper_row.add_css_class("metis-onboarding-stepper");
    stepper_row.set_halign(gtk::Align::Center);
    stepper_row.set_margin_top(8);
    let mut stepper = Vec::with_capacity(STEP_COUNT);
    for _ in 0..STEP_COUNT {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("metis-onboarding-dot");
        stepper_row.append(&dot);
        stepper.push(dot);
    }
    card.append(&stepper_row);

    let nav = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    nav.add_css_class("metis-onboarding-nav");
    nav.set_margin_top(8);
    nav.set_halign(gtk::Align::Fill);
    let back_btn = gtk::Button::with_label(&metis_i18n::tr("Back"));
    back_btn.set_halign(gtk::Align::Start);
    let nav_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_spacer.set_hexpand(true);
    let next_btn = gtk::Button::with_label(&metis_i18n::tr("Next"));
    next_btn.add_css_class("suggested-action");
    next_btn.set_halign(gtk::Align::End);
    nav.append(&back_btn);
    nav.append(&nav_spacer);
    nav.append(&next_btn);
    card.append(&nav);

    window.set_child(Some(&card));

    let ob = Rc::new(RefCell::new(Onboarding {
        window: window.clone(),
        title,
        body,
        stepper,
        back_btn: back_btn.clone(),
        next_btn: next_btn.clone(),
        step: initial_step,
        centered: false,
        fading: false,
        fade_start: None,
        parked: false,
    }));

    {
        let ob = ob.clone();
        skip_btn.connect_clicked(move |_| dismiss(ob.clone()));
    }
    {
        let ob = ob.clone();
        back_btn.connect_clicked(move |_| {
            let mut o = ob.borrow_mut();
            if o.step > 0 {
                o.step -= 1;
                refresh_step(&mut o);
            }
        });
    }
    {
        let ob = ob.clone();
        next_btn.connect_clicked(move |_| {
            let finish = ob.borrow().step + 1 >= STEP_COUNT;
            if finish {
                dismiss(ob.clone());
            } else {
                let mut o = ob.borrow_mut();
                o.step += 1;
                refresh_step(&mut o);
            }
        });
    }

    ONBOARDING.with(|cell| *cell.borrow_mut() = Some(ob.clone()));
    refresh_step(&mut ob.borrow_mut());

    let show_window = window.clone();
    glib::idle_add_local_once(move || {
        show_window.set_visible(true);
    });

    let ob_anim = ob.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut o = ob_anim.borrow_mut();

        // Phase 1: measure the card, center on screen, then reveal (splash pattern).
        if !o.centered && !o.parked && !o.fading {
            let w = o.window.width();
            let h = o.window.height();
            if w > 1 && h > 1 {
                let (mon_w, mon_h) = monitor_size();
                o.window.set_margin(Edge::Left, ((mon_w - w) / 2).max(0));
                o.window.set_margin(Edge::Top, ((mon_h - h) / 2).max(0));
                o.window.set_opacity(1.0);
                o.centered = true;
            }
        }

        if !o.fading {
            return glib::ControlFlow::Continue;
        }
        let fade_elapsed = o.fade_start.map(|t| t.elapsed()).unwrap_or(FADE);
        let t = (fade_elapsed.as_secs_f64() / FADE.as_secs_f64()).clamp(0.0, 1.0);
        o.window.set_opacity(1.0 - t);
        if t >= 1.0 && !PARK_HANDOFF_DONE.replace(true) {
            o.fading = false;
            o.parked = true;
            o.centered = false;
            park_off_screen(&o.window);
            handoff_after_park();
            // Drop the shell handle so the layer window is destroyed after parking,
            // matching the splash teardown path (see splash.rs Phase 3).
            ONBOARDING.with(|cell| *cell.borrow_mut() = None);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn handoff_after_park() {
    let bar_dirty = BAR_CONFIG_DIRTY.get();
    let needs_weather = WEATHER_RELOAD_PENDING.get();

    glib::idle_add_local_once(move || {
        // Overlay window is gone — safe to touch bar layer surfaces again.
        ONBOARDING_ACTIVE.store(false, Ordering::Release);

        if bar_dirty {
            BAR_CONFIG_DIRTY.set(false);
            glib::timeout_add_local_once(Duration::from_millis(200), || {
                crate::ui::bar::close_popovers();
                crate::ui::bar::apply_bar_config_now();
            });
        }
        if needs_weather {
            WEATHER_RELOAD_PENDING.set(false);
            crate::services::weather::weather_refresh();
        }
    });
}

fn park_off_screen(window: &gtk::Window) {
    window.set_opacity(0.0);
    window.set_keyboard_mode(KeyboardMode::None);
    let (_, mon_h) = monitor_size();
    window.set_margin(Edge::Top, mon_h + 400);
}

fn dismiss(ob: Rc<RefCell<Onboarding>>) {
    apply_onboarding_gaming_prefs();
    if let Err(err) = metis_config::ensure_kitty_defaults() {
        tracing::warn!(%err, "failed to seed kitty.conf defaults");
    }
    if let Err(err) = crate::config::mark_onboarding_complete() {
        tracing::warn!(%err, "failed to mark onboarding complete");
    }
    let mut o = ob.borrow_mut();
    if o.fading || o.parked {
        return;
    }
    // Drop keyboard grab before fade so the bar keeps working.
    o.window.set_keyboard_mode(KeyboardMode::None);
    o.fading = true;
    o.fade_start = Some(Instant::now());
}

fn refresh_step(o: &mut Onboarding) {
    let titles = step_titles();
    o.title.set_text(&titles[o.step]);
    o.back_btn.set_sensitive(o.step > 0);
    let next_label = if o.step + 1 >= STEP_COUNT {
        metis_i18n::tr("Finish")
    } else {
        metis_i18n::tr("Next")
    };
    o.next_btn.set_label(&next_label);
    o.back_btn.set_label(&metis_i18n::tr("Back"));

    while let Some(child) = o.body.first_child() {
        o.body.remove(&child);
    }

    let widget = match o.step {
        0 => build_language(),
        1 => build_welcome(),
        2 => build_theme(),
        3 => build_wallpaper(),
        4 => build_clock(),
        5 => build_edge_bar(),
        6 => build_weather(),
        7 => build_gaming(),
        8 => build_optional_software(),
        9 => build_finish(),
        _ => gtk::Label::new(Some("")).upcast(),
    };
    o.body.append(&widget);

    // Content-sized layer surface — re-center when step height/width changes.
    o.centered = false;
    o.window.queue_resize();

    for (i, dot) in o.stepper.iter().enumerate() {
        dot.remove_css_class("metis-onboarding-dot-active");
        dot.remove_css_class("metis-onboarding-dot-done");
        if i < o.step {
            dot.add_css_class("metis-onboarding-dot-done");
        } else if i == o.step {
            dot.add_css_class("metis-onboarding-dot-active");
        }
    }

    apply_onboarding_direction(&o.window);

    if let Err(err) = metis_config::save_onboarding_step(o.step as u32) {
        tracing::warn!(%err, step = o.step, "failed to persist onboarding step");
    }
}

fn apply_onboarding_direction(window: &gtk::Window) {
    let dir = if metis_i18n::is_rtl() {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    };
    window.set_direction(dir);
}

fn build_language() -> gtk::Widget {
    use metis_config::save_locale_config;
    use metis_i18n::tr;

    let col = step_shell();
    let intro = gtk::Label::new(Some(&tr(
        "Choose the language used by Metis. You can change this later in Settings.",
    )));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("metis-onboarding-copy");
    col.append(&intro);

    let choices = metis_i18n::known_language_choices();
    let labels: Vec<String> = choices
        .iter()
        .map(|(tag, name)| {
            if tag.is_empty() {
                tr(name)
            } else {
                name.clone()
            }
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    let cfg = metis_config::load_locale_config();
    let selected = match cfg.locale.as_deref() {
        None | Some("") => 0usize,
        Some(current) => choices
            .iter()
            .position(|(tag, _)| {
                !tag.is_empty()
                    && (tag == current
                        || current.starts_with(&format!("{tag}_"))
                        || tag.as_str() == current.split(['_', '-']).next().unwrap_or(""))
            })
            .unwrap_or(0),
    };
    dd.set_selected(selected as u32);
    dd.set_margin_top(12);
    col.append(&dd);

    {
        let choices = choices.clone();
        let labels_keep = labels;
        dd.connect_selected_notify(move |dd| {
            let _ = &labels_keep;
            let idx = dd.selected() as usize;
            let locale = choices.get(idx).and_then(|(tag, _)| {
                if tag.is_empty() {
                    None
                } else {
                    Some(tag.clone())
                }
            });
            let mut cfg = metis_config::load_locale_config();
            if cfg.locale == locale {
                return;
            }
            cfg.locale = locale;
            let _ = save_locale_config(&cfg);
            metis_i18n::reload();
            // Update chrome only — do not rebuild this step (would re-enter notify).
            ONBOARDING.with(|cell| {
                if let Some(ob) = cell.borrow().as_ref() {
                    let o = ob.borrow_mut();
                    apply_onboarding_direction(&o.window);
                    o.title.set_text(&metis_i18n::tr("Language & region"));
                    o.next_btn.set_label(&metis_i18n::tr("Next"));
                    o.back_btn.set_label(&metis_i18n::tr("Back"));
                }
            });
        });
    }

    col.upcast()
}

fn build_welcome() -> gtk::Widget {
    let col = step_shell();
    col.set_halign(gtk::Align::Center);

    let logo = gtk::Image::new();
    logo.set_pixel_size(160);
    if let Some(texture) = load_logo() {
        logo.set_paintable(Some(&texture));
    }
    logo.set_halign(gtk::Align::Center);
    col.append(&logo);

    let text = gtk::Label::new(Some(&metis_i18n::tr(
        "Metis is a fast, modern desktop built on Wayland.\n\
         This quick setup will personalize your workspace — you can change\n\
         everything later in Settings.",
    )));
    text.add_css_class("metis-onboarding-subtitle");
    text.set_halign(gtk::Align::Center);
    text.set_justify(gtk::Justification::Center);
    col.append(&text);

    col.upcast()
}

fn build_theme() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Pick light or dark — your desktop updates live behind this card.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_width_request(BODY_INNER_WIDTH);
    hint.set_max_width_chars(42);
    col.append(&hint);

    let wp = current_wallpaper_path();
    let chooser = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    chooser.set_halign(gtk::Align::Center);
    chooser.set_hexpand(false);
    chooser.set_margin_top(8);

    let light_label = metis_i18n::tr("Light");
    let dark_label = metis_i18n::tr("Dark");
    let light_btn = theme_preview_button(&light_label, false, wp.as_deref());
    let dark_btn = theme_preview_button(&dark_label, true, wp.as_deref());
    dark_btn.set_group(Some(&light_btn));

    let mode = crate::config::load_theme_preference().unwrap_or(ThemeMode::Light);
    match mode {
        ThemeMode::Light => light_btn.set_active(true),
        _ => dark_btn.set_active(true),
    }

    chooser.append(&light_btn);
    chooser.append(&dark_btn);
    col.append(&chooser);

    light_btn.connect_toggled(move |b| {
        if b.is_active() {
            apply_theme(ThemeMode::Light);
        }
    });
    dark_btn.connect_toggled(move |b| {
        if b.is_active() {
            apply_theme(ThemeMode::Dark);
        }
    });

    col.upcast()
}

fn build_wallpaper() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Choose a bundled background — applied instantly.",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    col.append(&hint);

    let wallpapers = metis_config::list_bundled_wallpapers();
    if wallpapers.is_empty() {
        let empty = gtk::Label::new(Some(&metis_i18n::tr(
            "No bundled wallpapers found. Reinstall Metis (wallpapers ship under \
             /usr/share/metis/wallpapers), or add images in Settings → Appearance.",
        )));
        empty.add_css_class("metis-onboarding-hint");
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        empty.set_margin_top(12);
        col.append(&empty);
        return col.upcast();
    }

    let grid = gtk::Grid::new();
    grid.set_column_spacing(10);
    grid.set_row_spacing(10);
    grid.set_halign(gtk::Align::Center);
    grid.set_hexpand(false);
    grid.set_width_request(WALL_W * 2 + 10);
    grid.add_css_class("metis-onboarding-wall-grid");

    let current = current_wallpaper_path();

    for (i, path) in wallpapers.into_iter().enumerate() {
        let btn = gtk::Button::new();
        btn.add_css_class("flat");
        btn.add_css_class("metis-onboarding-wall-pick");
        if current.as_ref() == Some(&path) {
            btn.add_css_class("selected");
        }

        let img = gtk::Image::new();
        img.add_css_class("metis-onboarding-wall-img");
        img.set_halign(gtk::Align::Center);
        img.set_valign(gtk::Align::Center);
        if let Ok(texture) = gdk::Texture::from_filename(&path) {
            img.set_paintable(Some(&texture));
        }
        img.set_pixel_size(WALL_H);
        btn.set_child(Some(&img));
        btn.set_size_request(WALL_W, WALL_H);

        let path_str = path.to_string_lossy().into_owned();
        let grid_ref = grid.clone();
        btn.connect_clicked(move |b| {
            apply_wallpaper(&path_str);
            let mut child = grid_ref.first_child();
            while let Some(c) = child {
                let next = c.next_sibling();
                if let Ok(pick) = c.downcast::<gtk::Button>() {
                    pick.remove_css_class("selected");
                }
                child = next;
            }
            b.add_css_class("selected");
        });

        let col_idx = (i % 2) as i32;
        let row_idx = (i / 2) as i32;
        grid.attach(&btn, col_idx, row_idx, 1, 1);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(BODY_HEIGHT - 40)
        .max_content_height(BODY_HEIGHT - 40)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&grid)
        .build();
    scroll.set_propagate_natural_height(false);
    scroll.set_width_request(BODY_INNER_WIDTH);
    scroll.set_hexpand(false);
    scroll.set_vexpand(false);
    scroll.set_size_request(BODY_INNER_WIDTH, BODY_HEIGHT - 40);
    scroll.set_overflow(gtk::Overflow::Hidden);
    col.append(&scroll);

    col.upcast()
}

fn build_clock() -> gtk::Widget {
    let col = step_shell();

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "How should the edge-bar clock display time?",
    )));
    hint.add_css_class("metis-onboarding-subtitle");
    hint.set_xalign(0.0);
    col.append(&hint);

    let bar = crate::config::load_bar_config();
    let is_24h = bar.clock.time_format == "%H:%M";

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    row.set_halign(gtk::Align::Center);
    row.set_margin_top(8);

    let btn_12 = gtk::ToggleButton::with_label(&metis_i18n::tr("12-hour (3:45 PM)"));
    let btn_24 = gtk::ToggleButton::with_label(&metis_i18n::tr("24-hour (15:45)"));
    btn_24.set_group(Some(&btn_12));

    if is_24h {
        btn_24.set_active(true);
    } else {
        btn_12.set_active(true);
    }

    row.append(&btn_12);
    row.append(&btn_24);
    col.append(&row);

    btn_12.connect_toggled(move |b| {
        if b.is_active() {
            update_bar(|c| c.clock.time_format = "%I:%M %p".into());
        }
    });
    btn_24.connect_toggled(move |b| {
        if b.is_active() {
            update_bar(|c| c.clock.time_format = "%H:%M".into());
        }
    });

    col.upcast()
}

fn build_edge_bar() -> gtk::Widget {
    let col = step_shell();
    let bar = crate::config::load_bar_config();

    let position_labels = [
        metis_i18n::tr("Top"),
        metis_i18n::tr("Bottom"),
        metis_i18n::tr("Left"),
        metis_i18n::tr("Right"),
    ];
    let position_refs: Vec<&str> = position_labels.iter().map(|s| s.as_str()).collect();
    let position_dd = gtk::DropDown::from_strings(&position_refs);
    position_dd.set_selected(bar_position_index(bar.position));
    col.append(&labeled_row(&metis_i18n::tr("Position"), &position_dd));

    let display_labels = [
        metis_i18n::tr("All displays"),
        metis_i18n::tr("Primary display only"),
    ];
    let display_refs: Vec<&str> = display_labels.iter().map(|s| s.as_str()).collect();
    let displays_dd = gtk::DropDown::from_strings(&display_refs);
    displays_dd.set_selected(match bar.displays {
        BarDisplays::Primary => 1,
        _ => 0,
    });
    col.append(&labeled_row(&metis_i18n::tr("Show bar on"), &displays_dd));

    let opacity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.3, 1.0, 0.01);
    opacity.set_value(bar.opacity as f64);
    opacity.set_size_request(220, -1);
    opacity.set_draw_value(true);
    col.append(&labeled_row(&metis_i18n::tr("Opacity"), &opacity));

    let blur = gtk::Switch::new();
    blur.set_active(bar.blur);
    blur.set_halign(gtk::Align::End);
    col.append(&labeled_row(&metis_i18n::tr("Backdrop blur"), &blur));

    position_dd.connect_selected_notify(move |dd| {
        let pos = index_to_bar_position(dd.selected());
        update_bar(|c| c.position = pos);
    });
    displays_dd.connect_selected_notify(move |dd| {
        let displays = if dd.selected() == 1 {
            BarDisplays::Primary
        } else {
            BarDisplays::All
        };
        update_bar(|c| c.displays = displays);
    });
    opacity.connect_value_changed(move |s| {
        let v = s.value() as f32;
        update_bar(|c| c.opacity = v);
    });
    blur.connect_active_notify(move |s| {
        update_bar(|c| c.blur = s.is_active());
    });

    col.upcast()
}

fn build_weather() -> gtk::Widget {
    let col = step_shell();
    let cfg = Rc::new(RefCell::new(crate::config::load_weather_config()));

    let auto_sw = gtk::Switch::new();
    auto_sw.set_active(cfg.borrow().auto_detect);
    auto_sw.set_halign(gtk::Align::End);
    col.append(&labeled_row(
        &metis_i18n::tr("Detect my location"),
        &auto_sw,
    ));

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Or search for a city to pin a location (overrides auto-detect).",
    )));
    hint.add_css_class("metis-onboarding-hint");
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    col.append(&hint);

    let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::builder()
        .placeholder_text(metis_i18n::tr("Search for a city…"))
        .hexpand(true)
        .build();
    let search_btn = gtk::Button::with_label(&metis_i18n::tr("Search"));
    search_row.append(&entry);
    search_row.append(&search_btn);
    col.append(&search_row);

    let results = gtk::ListBox::new();
    results.set_selection_mode(gtk::SelectionMode::None);
    results.set_margin_top(4);
    let results_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(72)
        .max_content_height(72)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&results)
        .build();
    col.append(&results_scroll);

    let (tx, rx) = mpsc::channel::<Vec<GeoResult>>();
    {
        let results = results.clone();
        let cfg = cfg.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            if let Ok(items) = rx.try_recv() {
                clear_list_box(&results);
                if items.is_empty() {
                    let row = gtk::ListBoxRow::new();
                    let lbl = gtk::Label::new(Some(&metis_i18n::tr(
                        "No results — try another spelling.",
                    )));
                    lbl.add_css_class("metis-onboarding-hint");
                    lbl.set_xalign(0.0);
                    row.set_child(Some(&lbl));
                    results.append(&row);
                } else {
                    for item in items {
                        let row = gtk::ListBoxRow::new();
                        let btn = gtk::Button::new();
                        btn.add_css_class("flat");
                        let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
                        let name = gtk::Label::new(Some(&item.name));
                        name.set_xalign(0.0);
                        name.set_halign(gtk::Align::Start);
                        v.append(&name);
                        if !item.detail.is_empty() {
                            let sub = gtk::Label::new(Some(&item.detail));
                            sub.add_css_class("metis-onboarding-hint");
                            sub.set_xalign(0.0);
                            v.append(&sub);
                        }
                        btn.set_child(Some(&v));
                        let cfg = cfg.clone();
                        btn.connect_clicked(move |_| {
                            let mut c = cfg.borrow_mut();
                            c.auto_detect = false;
                            c.locations = vec![WeatherLocation {
                                name: item.name.clone(),
                                latitude: item.lat,
                                longitude: item.lon,
                            }];
                            save_weather(&c);
                        });
                        row.set_child(Some(&btn));
                        results.append(&row);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let cfg = cfg.clone();
        auto_sw.connect_active_notify(move |s| {
            cfg.borrow_mut().auto_detect = s.is_active();
            if s.is_active() {
                cfg.borrow_mut().locations.clear();
            }
            save_weather(&cfg.borrow());
        });
    }

    search_btn.connect_clicked(move |_| {
        let query = entry.text().to_string();
        if query.trim().is_empty() {
            return;
        }
        let tx = tx.clone();
        std::thread::spawn(move || {
            let results = geocode_search(&query);
            let _ = tx.send(results);
        });
    });

    col.upcast()
}

fn build_gaming() -> gtk::Widget {
    let col = step_shell();

    let hybrid = metis_config::detect_hybrid_gpu(None).is_some();
    let steam = match metis_gaming::detect_steam() {
        metis_gaming::SteamInstall::Native => metis_i18n::tr("native Steam"),
        metis_gaming::SteamInstall::Flatpak => metis_i18n::tr("Flatpak Steam"),
        metis_gaming::SteamInstall::None => metis_i18n::tr("not installed"),
    };
    let hybrid_status = if hybrid {
        metis_i18n::tr("detected")
    } else {
        metis_i18n::tr("not detected")
    };
    let gamemode_status = if metis_gaming::gamemode_installed() {
        metis_i18n::tr("installed")
    } else {
        metis_i18n::tr("optional — install gamemode")
    };
    let vulkan_status = if metis_gaming::i386_vulkan_likely_missing()
        || metis_gaming::mesa_vulkan_amd64_missing()
    {
        metis_i18n::tr("needs Fix in Settings → Gaming")
    } else {
        metis_i18n::tr("looks OK")
    };
    let nvidia_status = if metis_gaming::nvidia_gpu_present() {
        if metis_gaming::detect::nvidia_driver_loaded() {
            metis_i18n::tr("driver loaded")
        } else {
            metis_i18n::tr("driver missing — use Settings → Gaming (consent install)")
        }
    } else {
        metis_i18n::tr("not detected")
    };
    let summary = gtk::Label::new(Some(&format!(
        "{}\n{}\n{}\n{}\n{}",
        metis_i18n::tr("Hybrid GPU: %1").replace("%1", &hybrid_status),
        metis_i18n::tr("Steam: %1").replace("%1", &steam),
        metis_i18n::tr("GameMode: %1").replace("%1", &gamemode_status),
        metis_i18n::tr("Vulkan: %1").replace("%1", &vulkan_status),
        metis_i18n::tr("NVIDIA: %1").replace("%1", &nvidia_status),
    )));
    summary.add_css_class("metis-onboarding-subtitle");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    col.append(&summary);

    let auto_gpu =
        gtk::CheckButton::with_label(&metis_i18n::tr("Enable automatic GPU switching for games"));
    auto_gpu.set_active(GAMING_AUTO_GPU.get());
    auto_gpu.connect_active_notify(|s| GAMING_AUTO_GPU.set(s.is_active()));
    col.append(&auto_gpu);

    let optimize = gtk::CheckButton::with_label(&metis_i18n::tr(
        "Optimize Flatpak Steam / Lutris / Heroic (--device=all, network, Wayland)",
    ));
    optimize.set_active(GAMING_OPTIMIZE.get());
    optimize.connect_active_notify(|s| GAMING_OPTIMIZE.set(s.is_active()));
    col.append(&optimize);

    let hint = gtk::Label::new(Some(&metis_i18n::tr(
        "Metis routes games onto the discrete GPU automatically — leave Steam Launch Options \
         empty for GPU. Drivers and packages are never installed silently; use Settings → Gaming \
         → Run gaming setup or Fix for Steam, Vulkan, controllers, and NVIDIA (consent).",
    )));
    hint.add_css_class("metis-onboarding-hint");
    hint.set_xalign(0.0);
    hint.set_margin_top(8);
    hint.set_wrap(true);
    col.append(&hint);

    col.upcast()
}

fn apply_onboarding_gaming_prefs() {
    let mut cfg = metis_config::load_gaming_config();
    if GAMING_AUTO_GPU.get() {
        cfg.graphics_mode = metis_config::GraphicsMode::Auto;
        cfg.flatpak_gpu_env = true;
    }
    let _ = metis_config::save_gaming_config(&cfg);
    if GAMING_OPTIMIZE.get() {
        std::thread::spawn(|| {
            let _ = metis_gaming::optimize_flatpak_gaming();
            let _ = metis_gaming::ensure_steam_launcher();
        });
    }
    let _ = metis_config::mark_gaming_setup_complete();
    metis_gaming::session::request_reload();
}

#[derive(Clone)]
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
    binary_on_path("gnome-keyring-daemon")
        || binary_on_path("kwalletd6")
        || binary_on_path("kwalletd5")
        || binary_on_path("keepassxc")
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
    // Only offer keyring when no Secret Service provider is present.
    if !secret_service_available() {
        features.push(OptionalFeature {
            id: "keyring",
            title: "Keyring",
            subtitle: "Secure credentials for apps (recommended)",
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
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("metis-remote");
            if sibling.is_file() {
                return sibling.to_string_lossy().into_owned();
            }
        }
    }
    "metis-remote".into()
}

fn build_optional_software() -> gtk::Widget {
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

fn build_finish() -> gtk::Widget {
    let col = step_shell();

    let summary = gtk::Label::new(Some(&metis_i18n::tr(
        "Your desktop is ready. Here are a few shortcuts to get started:",
    )));
    summary.add_css_class("metis-onboarding-subtitle");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    col.append(&summary);

    let cfg = metis_config::load_keybinds_config();
    let mod_label = cfg.mod_key.as_str();
    let close = cfg
        .chord_for(metis_config::KeybindAction::CloseWindow)
        .display();
    let layout_free = cfg
        .chord_for(metis_config::KeybindAction::LayoutFree)
        .display();
    let ws1 = cfg
        .chord_for(metis_config::KeybindAction::Workspace1)
        .display();
    let keybinds = [
        (
            metis_i18n::tr("Click the brand icon"),
            metis_i18n::tr("Open the app launcher"),
        ),
        (
            layout_free,
            metis_i18n::tr("Disable tiling / return to free desktop"),
        ),
        (close, metis_i18n::tr("Close the focused window")),
        (
            format!("{mod_label} + 1 … 9"),
            metis_i18n::tr("Switch workspace"),
        ),
    ];
    // Keep a note that defaults use the configured Metis modifier.
    let _ = ws1;
    for (key, desc) in &keybinds {
        col.append(&keybind_row(key, desc));
    }

    let display_hint = gtk::Label::new(Some(&metis_i18n::tr(
        "For monitor arrangement, resolution, and refresh rate, open\n\
         Settings → Display.",
    )));
    display_hint.add_css_class("metis-onboarding-hint");
    display_hint.set_xalign(0.0);
    display_hint.set_margin_top(8);
    display_hint.set_wrap(true);
    col.append(&display_hint);

    col.upcast()
}

fn keybind_row(key: &str, desc: &str) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(2);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("metis-onboarding-keybind");
    k.set_width_chars(14);
    k.set_xalign(0.0);
    let d = gtk::Label::new(Some(desc));
    d.add_css_class("metis-onboarding-subtitle");
    d.set_xalign(0.0);
    d.set_hexpand(true);
    row.append(&k);
    row.append(&d);
    row.upcast()
}

fn step_shell() -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.add_css_class("metis-onboarding-step-content");
    col.set_width_request(BODY_INNER_WIDTH);
    col.set_hexpand(false);
    col.set_vexpand(false);
    col.set_halign(gtk::Align::Fill);
    col.set_overflow(gtk::Overflow::Hidden);
    col
}

fn labeled_row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(4);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_width_chars(14);
    lbl.set_halign(gtk::Align::Start);
    widget.set_hexpand(true);
    widget.set_halign(gtk::Align::End);
    row.append(&lbl);
    row.append(widget);
    row.upcast()
}

fn theme_preview_button(label: &str, dark: bool, wallpaper: Option<&Path>) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("metis-onboarding-preview-tile");
    btn.set_hexpand(false);
    btn.set_vexpand(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    vbox.set_hexpand(false);
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(120, 76);

    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_size_request(120, 76);
    if let Some(path) = wallpaper {
        pic.set_filename(Some(path));
    } else {
        pic.add_css_class(if dark {
            "metis-style-fallback-dark"
        } else {
            "metis-style-fallback-light"
        });
    }
    overlay.set_child(Some(&pic));

    let mock = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mock.add_css_class(if dark {
        "metis-style-mock-dark"
    } else {
        "metis-style-mock-light"
    });
    mock.set_halign(gtk::Align::Center);
    mock.set_valign(gtk::Align::Center);
    mock.set_size_request(72, 44);
    overlay.add_overlay(&mock);

    vbox.append(&overlay);
    let caption = gtk::Label::new(Some(label));
    caption.add_css_class("metis-style-caption");
    vbox.append(&caption);
    btn.set_child(Some(&vbox));
    btn
}

fn apply_theme(mode: ThemeMode) {
    if let Err(err) = crate::config::save_theme_preference(mode) {
        tracing::warn!(%err, "failed to save theme preference");
    }
    metis_config::apply_session_appearance_gsettings(mode);
    let _ = crate::ui::theme::init_theme();
}

fn apply_wallpaper(path: &str) {
    let mut cfg = crate::config::load_wallpaper_config();
    cfg.kind = BackgroundKind::Image;
    cfg.path = Some(path.to_string());
    if let Err(err) = crate::config::save_wallpaper_config(&cfg) {
        tracing::warn!(%err, "failed to save wallpaper.json");
    }
    if let Err(err) = crate::compositor::apply_background() {
        tracing::warn!(%err, "failed to apply background");
    }
}

fn update_bar<F>(apply: F)
where
    F: FnOnce(&mut metis_config::BarConfig),
{
    let old = crate::config::load_bar_config();
    let mut cfg = old.clone();
    apply(&mut cfg);
    if cfg == old {
        return;
    }
    if let Err(err) = crate::config::save_bar_config(&cfg) {
        tracing::warn!(%err, "failed to save bar.json");
    }
    BAR_CONFIG_DIRTY.set(true);
}

fn save_weather(cfg: &WeatherConfig) {
    let old = crate::config::load_weather_config();
    if cfg == &old {
        return;
    }
    if let Err(err) = crate::config::save_weather_config(cfg) {
        tracing::warn!(%err, "failed to save weather.json");
    }
    WEATHER_RELOAD_PENDING.set(true);
}
fn current_wallpaper_path() -> Option<PathBuf> {
    if let Some(p) = crate::config::load_wallpaper_config().path {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    metis_config::list_bundled_wallpapers().into_iter().next()
}

fn bar_position_index(pos: BarPosition) -> u32 {
    match pos {
        BarPosition::Top => 0,
        BarPosition::Bottom => 1,
        BarPosition::Left => 2,
        BarPosition::Right => 3,
    }
}

fn index_to_bar_position(idx: u32) -> BarPosition {
    match idx {
        1 => BarPosition::Bottom,
        2 => BarPosition::Left,
        3 => BarPosition::Right,
        _ => BarPosition::Top,
    }
}

fn monitor_size() -> (i32, i32) {
    if let Some(display) = gdk::Display::default() {
        if let Some(obj) = display.monitors().item(0) {
            if let Ok(monitor) = obj.downcast::<gdk::Monitor>() {
                let g = monitor.geometry();
                if g.width() > 0 && g.height() > 0 {
                    return (g.width(), g.height());
                }
            }
        }
    }
    (1280, 720)
}

fn load_logo() -> Option<gdk::Texture> {
    let bytes = glib::Bytes::from_static(LOGO_BYTES);
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Some(texture),
        Err(err) => {
            tracing::warn!(%err, "failed to decode onboarding logo");
            None
        }
    }
}

fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

#[derive(Debug, Clone)]
struct GeoResult {
    name: String,
    detail: String,
    lat: f64,
    lon: f64,
}

fn geocode_search(query: &str) -> Vec<GeoResult> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=en&format=json",
        urlencode(query)
    );
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "geocode: client build failed");
            return Vec::new();
        }
    };
    let json: serde_json::Value = match client.get(&url).send().and_then(|r| r.json()) {
        Ok(j) => j,
        Err(err) => {
            tracing::warn!(%err, "geocode: request failed");
            return Vec::new();
        }
    };
    let Some(results) = json.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let name = r.get("name")?.as_str()?.to_string();
            let lat = r.get("latitude")?.as_f64()?;
            let lon = r.get("longitude")?.as_f64()?;
            let admin = r.get("admin1").and_then(|v| v.as_str()).unwrap_or("");
            let country = r.get("country").and_then(|v| v.as_str()).unwrap_or("");
            let detail = [admin, country]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            Some(GeoResult {
                name,
                detail,
                lat,
                lon,
            })
        })
        .collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
