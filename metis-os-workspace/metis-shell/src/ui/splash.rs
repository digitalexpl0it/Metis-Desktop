//! Centered startup splash: the Metis logo over a translucent card with a loading
//! progress bar. Shown on the overlay layer while the desktop comes up, then it
//! ramps to 100% and fades itself out.
//!
//! IMPORTANT: the layer surface is never destroyed or unmapped. Tearing down a
//! layer surface (`destroy()` / `set_visible(false)`) races the Wayland teardown
//! and disconnects the whole shell (`Broken pipe`). Instead, once finished, the
//! window fades to transparent and is parked far off-screen while staying mapped.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Metis wordmark, embedded so the splash renders regardless of the working dir.
const LOGO_BYTES: &[u8] = include_bytes!("../../assets/metis_logo.png");

/// Startup chime, embedded and played once alongside the splash.
const SOUND_BYTES: &[u8] = include_bytes!("../../assets/startup.mp3");

/// Minimum time the splash stays up (from when it becomes visible) so it never
/// just flashes.
const MIN_SHOW: Duration = Duration::from_millis(1100);
/// How long the bar takes to crawl to ~92% on its own (roughly the wallpaper load).
const RAMP: Duration = Duration::from_millis(2600);
/// Hard ceiling — the splash always finishes by here even if no ready signal lands.
const MAX_SHOW: Duration = Duration::from_millis(4000);
/// Fade-out duration once progress reaches 100%.
const FADE: Duration = Duration::from_millis(360);

struct Splash {
    window: gtk::Window,
    progress: gtk::ProgressBar,
    /// Set once the card has been measured and centered (animation clock origin).
    start: Option<Instant>,
    centered: bool,
    finish_requested: bool,
    fade_start: Option<Instant>,
    done: bool,
}

thread_local! {
    static SPLASH: RefCell<Option<Rc<RefCell<Splash>>>> = const { RefCell::new(None) };
    /// Keeps the startup chime alive until playback ends (GTK media backend).
    static STARTUP_SOUND: RefCell<Option<gtk::MediaFile>> = const { RefCell::new(None) };
    /// One-shot callback invoked once the splash fade finishes and the surface is parked.
    static ON_COMPLETE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

fn monitor_size() -> (i32, i32) {
    if let Some(display) = gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gdk::Monitor>()
    {
        let g = monitor.geometry();
        if g.width() > 0 && g.height() > 0 {
            return (g.width(), g.height());
        }
    }
    (1280, 720)
}

/// Play the embedded startup chime once via GTK's media backend. Best-effort:
/// if no media backend (GStreamer) is available it degrades silently.
///
/// NOTE: the GStreamer media backend only supports files/URIs — creating a
/// `MediaFile` from an input stream makes it `g_assert_not_reached()` and abort
/// the whole shell. So the embedded bytes are materialized to a temp file first.
fn play_startup_sound() {
    let Some(path) = materialize_sound() else {
        return;
    };
    let media = gtk::MediaFile::for_filename(&path);
    media.set_volume(1.0);

    // Surface backend errors (e.g. missing codec) without crashing.
    media.connect_error_notify(|m| {
        if let Some(err) = m.error() {
            tracing::warn!(%err, "startup sound playback failed");
        }
    });
    // Release the handle once the chime finishes.
    media.connect_ended_notify(|_| {
        STARTUP_SOUND.with(|cell| *cell.borrow_mut() = None);
    });

    media.play();
    STARTUP_SOUND.with(|cell| *cell.borrow_mut() = Some(media));
}

/// Write the embedded chime to a stable temp path (once per boot) so the
/// file-based media backend can open it. Returns the path, or `None` on failure.
fn materialize_sound() -> Option<std::path::PathBuf> {
    let path = std::env::temp_dir().join("metis-startup.mp3");
    let needs_write = std::fs::metadata(&path)
        .map(|m| m.len() != SOUND_BYTES.len() as u64)
        .unwrap_or(true);
    if needs_write && let Err(err) = std::fs::write(&path, SOUND_BYTES) {
        tracing::warn!(%err, "failed to write startup chime to temp file");
        return None;
    }
    Some(path)
}

/// Build and show the splash overlay, then start the progress/fade animation.
pub fn show() {
    // Defer audio so GStreamer/plugin init cannot block GTK setup on the main thread.
    glib::idle_add_local_once(play_startup_sound);

    let window = gtk::Window::builder().title("Metis").build();
    window.add_css_class("metis-splash-window");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_namespace(Some("metis-splash"));
    // Anchor a corner so we can position (and later park) the card via margins.
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    // Start transparent; we reveal it once it has been measured and centered to
    // avoid a one-frame flash in the top-left corner.
    window.set_opacity(0.0);

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .build();
    card.add_css_class("metis-splash-card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);

    let logo = gtk::Image::new();
    logo.add_css_class("metis-splash-logo");
    logo.set_pixel_size(232);
    if let Some(texture) = load_logo() {
        logo.set_paintable(Some(&texture));
    }
    logo.set_halign(gtk::Align::Center);
    card.append(&logo);

    let progress = gtk::ProgressBar::new();
    progress.add_css_class("metis-splash-progress");
    progress.set_fraction(0.04);
    progress.set_hexpand(true);
    progress.set_vexpand(false);
    progress.set_halign(gtk::Align::Fill);
    let progress_frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    progress_frame.set_size_request(280, 8);
    progress_frame.set_halign(gtk::Align::Center);
    progress_frame.append(&progress);
    card.append(&progress_frame);

    let label = gtk::Label::new(Some(&metis_i18n::tr("Starting Metis…")));
    label.add_css_class("metis-splash-label");
    label.set_halign(gtk::Align::Center);
    card.append(&label);

    window.set_child(Some(&card));

    // Defer the map until layer-shell config is applied (avoids a 0-size commit).
    let show_window = window.clone();
    glib::idle_add_local_once(move || {
        show_window.set_visible(true);
        show_window.present();
    });

    let splash = Rc::new(RefCell::new(Splash {
        window,
        progress,
        start: None,
        centered: false,
        finish_requested: false,
        fade_start: None,
        done: false,
    }));
    SPLASH.with(|cell| *cell.borrow_mut() = Some(splash.clone()));

    glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut s = splash.borrow_mut();
        if s.done {
            return glib::ControlFlow::Break;
        }

        // Phase 1: wait for the card to be measured, then center + reveal it.
        if !s.centered {
            let w = s.window.width();
            let h = s.window.height();
            if w > 1 && h > 1 {
                let (mon_w, mon_h) = monitor_size();
                s.window.set_margin(Edge::Left, ((mon_w - w) / 2).max(0));
                s.window.set_margin(Edge::Top, ((mon_h - h) / 2).max(0));
                s.window.set_opacity(1.0);
                s.centered = true;
                s.start = Some(Instant::now());
            }
            return glib::ControlFlow::Continue;
        }

        let elapsed = s.start.map(|t| t.elapsed()).unwrap_or_default();

        // Phase 2: fill the bar; begin fade once finished (or the ceiling hits).
        if s.fade_start.is_none() {
            let auto_finish = elapsed >= MAX_SHOW;
            let ready = (s.finish_requested || auto_finish) && elapsed >= MIN_SHOW;
            let frac = target_fraction(elapsed, ready);
            s.progress.set_fraction(frac);
            if ready && frac >= 0.999 {
                s.fade_start = Some(Instant::now());
            }
            return glib::ControlFlow::Continue;
        }

        // Phase 3: fade out, then park off-screen (never destroy/unmap).
        let fade_elapsed = s.fade_start.map(|t| t.elapsed()).unwrap_or(FADE);
        let t = (fade_elapsed.as_secs_f64() / FADE.as_secs_f64()).clamp(0.0, 1.0);
        s.window.set_opacity(1.0 - t);
        if t >= 1.0 {
            s.done = true;
            let (_, mon_h) = monitor_size();
            s.window.set_opacity(0.0);
            s.window.set_margin(Edge::Top, mon_h + 400);
            SPLASH.with(|cell| *cell.borrow_mut() = None);
            ON_COMPLETE.with(|cell| {
                if let Some(f) = cell.borrow_mut().take() {
                    f();
                }
            });
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

/// Register a one-shot callback for when the splash fade finishes. If the splash
/// already completed, the callback runs immediately on the GTK main loop.
pub fn on_complete(f: impl FnOnce() + 'static) {
    let already_done = SPLASH.with(|cell| cell.borrow().is_none());
    if already_done {
        glib::idle_add_local_once(f);
        return;
    }
    ON_COMPLETE.with(|cell| *cell.borrow_mut() = Some(Box::new(f)));
}

/// Signal that the desktop is ready; the splash will ramp to 100% and fade out
/// (respecting the minimum on-screen time).
pub fn finish() {
    SPLASH.with(|cell| {
        if let Some(splash) = cell.borrow().as_ref() {
            splash.borrow_mut().finish_requested = true;
        }
    });
}

/// Crawl to ~92% over `RAMP` while loading; snap toward 100% once ready.
fn target_fraction(elapsed: Duration, ready: bool) -> f64 {
    if ready {
        return 1.0;
    }
    let t = (elapsed.as_secs_f64() / RAMP.as_secs_f64()).clamp(0.0, 1.0);
    // ease-out so it decelerates as it approaches the cap.
    let eased = 1.0 - (1.0 - t).powi(2);
    0.04 + eased * 0.88
}

fn load_logo() -> Option<gdk::Texture> {
    let bytes = glib::Bytes::from_static(LOGO_BYTES);
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Some(texture),
        Err(err) => {
            tracing::warn!(%err, "failed to decode embedded splash logo");
            None
        }
    }
}
