//! First-run onboarding wizard: a centered layer-shell overlay that walks the user
//! through theme, wallpaper, clock, edge bar, network, weather, desktop widgets,
//! gaming, and optional host packages before marking `onboarding_complete` in
//! `config.json`.
//!
//! Progress is resumable: `onboarding_step` is written on each Next/Back so a
//! session restart mid-wizard continues where the user left off.
//!
//! Like the startup splash, the layer surface is parked off-screen on dismiss,
//! then dropped (same lifecycle as `splash.rs`) so we never call `destroy()` or
//! `set_visible(false)` while another layer surface is reconfiguring.

mod helpers;
mod optional;
mod steps;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use helpers::{
    BODY_HEIGHT, BODY_INNER_WIDTH, CARD_WIDTH, apply_onboarding_gaming_prefs, monitor_size,
};
use steps::widget_for_step;

pub(crate) const STEP_COUNT: usize = 12;
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
        tr("Network"),
        tr("Weather"),
        tr("Desktop widgets"),
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

pub(crate) fn mark_bar_config_dirty() {
    BAR_CONFIG_DIRTY.set(true);
}

pub(crate) fn mark_weather_reload_pending() {
    WEATHER_RELOAD_PENDING.set(true);
}

pub(crate) fn gaming_auto_gpu() -> bool {
    GAMING_AUTO_GPU.get()
}

pub(crate) fn set_gaming_auto_gpu(v: bool) {
    GAMING_AUTO_GPU.set(v);
}

pub(crate) fn gaming_optimize() -> bool {
    GAMING_OPTIMIZE.get()
}

pub(crate) fn set_gaming_optimize(v: bool) {
    GAMING_OPTIMIZE.set(v);
}

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
            o.window.set_keyboard_mode(KeyboardMode::OnDemand);
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
    // OnDemand so search / Wi-Fi password fields can receive keys.
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_namespace(Some("metis-onboarding"));
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
            let w = o.window.width().min(CARD_WIDTH + 8);
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

    o.body.append(&widget_for_step(o.step));

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

/// Refresh wizard chrome after the language step changes locale (no full rebuild).
pub(crate) fn refresh_chrome_after_locale_change() {
    ONBOARDING.with(|cell| {
        if let Some(ob) = cell.borrow().as_ref() {
            let o = ob.borrow_mut();
            apply_onboarding_direction(&o.window);
            o.title.set_text(&metis_i18n::tr("Language & region"));
            o.next_btn.set_label(&metis_i18n::tr("Next"));
            o.back_btn.set_label(&metis_i18n::tr("Back"));
        }
    });
}
