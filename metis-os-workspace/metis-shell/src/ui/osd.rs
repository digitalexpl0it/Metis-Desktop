//! Centered on-screen display (OSD) for hardware-key feedback.
//!
//! A single `gtk4_layer_shell` overlay is kept mapped for the process lifetime
//! (mirroring the toast overlay, which avoids Wayland layer-surface teardown
//! races) and parked hidden when idle. Volume / brightness keys flash a card
//! with an icon, a label, and a level bar; media transport keys show an
//! icon-only card. Each `show` restarts the auto-hide timer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// How long the overlay stays visible after the last key press.
const VISIBLE_MS: u64 = 1300;
/// Distance from the bottom edge of the screen.
const BOTTOM_MARGIN: i32 = 120;

struct Osd {
    window: gtk::Window,
    card: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    level: gtk::LevelBar,
    percent: gtk::Label,
    /// Bumped on every `show`; the auto-hide timer only fires for its own token.
    generation: Cell<u64>,
}

thread_local! {
    static OSD: RefCell<Option<Rc<Osd>>> = const { RefCell::new(None) };
}

fn overlay() -> Rc<Osd> {
    OSD.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }

        let window = gtk::Window::builder()
            .title(metis_i18n::tr("Metis OSD"))
            .build();
        window.add_css_class("metis-osd-window");
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace(Some("metis-osd"));
        // Bottom-center: anchor only to the bottom edge so the compositor centers
        // the surface horizontally.
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, BOTTOM_MARGIN);

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        card.add_css_class("metis-osd-card");

        let icon = gtk::Image::new();
        icon.add_css_class("metis-osd-icon");
        icon.set_pixel_size(44);
        icon.set_halign(gtk::Align::Center);
        card.append(&icon);

        let title = gtk::Label::new(None);
        title.add_css_class("metis-osd-title");
        title.set_halign(gtk::Align::Center);
        card.append(&title);

        let level = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(100.0)
            .build();
        level.add_css_class("metis-osd-level");
        level.set_width_request(220);
        card.append(&level);

        let percent = gtk::Label::new(None);
        percent.add_css_class("metis-osd-percent");
        percent.set_halign(gtk::Align::Center);
        card.append(&percent);

        window.set_child(Some(&card));

        let osd = Rc::new(Osd {
            window,
            card,
            icon,
            title,
            level,
            percent,
            generation: Cell::new(0),
        });
        *cell.borrow_mut() = Some(osd.clone());
        osd
    })
}

/// Flash the OSD. `level` in 0–100 shows the progress bar and a percentage;
/// `None` renders an icon-only card (media transport). `muted` dims the card and
/// swaps to the muted icon supplied by the caller.
pub fn show(icon: &str, title: &str, level: Option<f64>, muted: bool) {
    let osd = overlay();

    osd.icon.set_icon_name(Some(icon));
    osd.title.set_label(title);

    match level {
        Some(value) => {
            let value = value.clamp(0.0, 100.0);
            osd.level.set_visible(true);
            osd.level.set_value(value);
            osd.percent.set_visible(true);
            osd.percent.set_label(&format!("{}%", value.round() as i32));
        }
        None => {
            osd.level.set_visible(false);
            osd.percent.set_visible(false);
        }
    }

    if muted {
        osd.card.add_css_class("muted");
    } else {
        osd.card.remove_css_class("muted");
    }

    osd.window.set_visible(true);
    osd.window.present();

    let token = osd.generation.get().wrapping_add(1);
    osd.generation.set(token);
    let osd_hide = osd.clone();
    glib::timeout_add_local_once(Duration::from_millis(VISIBLE_MS), move || {
        // Only the most recent show hides the window; a newer press keeps it up.
        if osd_hide.generation.get() == token {
            osd_hide.window.set_visible(false);
        }
    });
}
