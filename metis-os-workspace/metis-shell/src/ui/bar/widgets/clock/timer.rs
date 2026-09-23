use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::{notify, play_alarm_sound};

pub struct TimerPage {
    pub widget: gtk::Widget,
}

struct Inner {
    // Configured duration (from the steppers), in seconds.
    hours: Cell<u32>,
    minutes: Cell<u32>,
    seconds: Cell<u32>,
    // Run state.
    end: Cell<Option<Instant>>,
    remaining: Cell<Duration>,
    generation: Cell<u64>,
    running: Cell<bool>,
    label: gtk::Label,
    h_lbl: gtk::Label,
    m_lbl: gtk::Label,
    s_lbl: gtk::Label,
    primary: gtk::Button,
    reset_btn: gtk::Button,
    setup: gtk::Box,
    overlay: RefCell<Option<TimerOverlay>>,
}

/// Logical-pixel size of the floating HUD's monitor, with a sane fallback.
fn monitor_size() -> (i32, i32) {
    if let Some(display) = gtk::gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gtk::gdk::Monitor>()
    {
        let g = monitor.geometry();
        if g.width() > 0 && g.height() > 0 {
            return (g.width(), g.height());
        }
    }
    (1280, 720)
}

/// Smallest gap (px) the HUD keeps below the edge bar. A Neutral layer surface
/// is already positioned below the bar's exclusive zone, so its top margin is
/// measured from the bar's bottom edge — a tiny value sits it just under the bar.
const HUD_MIN_TOP: i32 = 4;

/// Vertical space (px) the edge bar occupies from the top of the output. The
/// HUD's top margin is relative to this, so it must be subtracted when clamping
/// against the bottom of the screen.
fn bar_offset() -> i32 {
    let cfg = crate::config::load_bar_config();
    (cfg.margin_top + cfg.height) as i32
}

/// A small, draggable, always-on-top layer-shell HUD that floats just under the
/// edge bar while a timer is counting down, with Pause/Resume and Stop controls.
///
/// The window is created once and shown/hidden across runs — destroying and
/// recreating a layer surface on every timer finish proved fragile.
struct TimerOverlay {
    window: gtk::Window,
    label: gtk::Label,
    pause_btn: gtk::Button,
    /// Current on-screen position (left, top margins), kept in sync by the drag
    /// handler so the HUD reappears where the user left it.
    pos: Rc<Cell<(i32, i32)>>,
}

impl TimerOverlay {
    fn new(inner: &Weak<Inner>) -> Self {
        let min_top = HUD_MIN_TOP;
        let bar_off = bar_offset();
        let init_top = min_top + 4;
        let window = gtk::Window::builder().build();
        window.add_css_class("metis-timer-hud-window");
        window.init_layer_shell();
        // Overlay keeps it above ordinary application windows; anchoring to the
        // top-left corner lets us position it freely via margins (drag-to-move).
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);

        let (mon_w, _) = monitor_size();
        let init_left = ((mon_w - 220) / 2).max(0);
        window.set_margin(Edge::Top, init_top);
        window.set_margin(Edge::Left, init_left);
        // Our own record of the current position (avoids reading back margins).
        let pos = Rc::new(Cell::new((init_left, init_top)));

        let hud = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        hud.add_css_class("metis-timer-hud");

        // NOTE: deliberately no tooltips on the HUD. GTK tooltips spawn child
        // xdg_popups on this layer surface; if one is alive when the surface is
        // torn down (timer end), the orphaned popup triggers a Wayland protocol
        // error that disconnects the whole shell.
        let grip = gtk::Image::from_icon_name("open-menu-symbolic");
        grip.add_css_class("metis-timer-hud-grip");
        hud.append(&grip);

        let icon = gtk::Image::from_icon_name("alarm-symbolic");
        icon.add_css_class("metis-timer-hud-icon");
        hud.append(&icon);

        let label = gtk::Label::new(Some("00:00:00"));
        label.add_css_class("metis-timer-hud-time");
        hud.append(&label);

        let pause_btn = gtk::Button::from_icon_name("media-playback-pause-symbolic");
        pause_btn.add_css_class("metis-timer-hud-btn");
        {
            let inner = inner.clone();
            pause_btn.connect_clicked(move |_| {
                if let Some(inner) = inner.upgrade() {
                    if inner.running.get() {
                        inner.pause();
                    } else {
                        inner.start();
                    }
                }
            });
        }
        hud.append(&pause_btn);

        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("metis-timer-hud-btn");
        {
            let inner = inner.clone();
            close_btn.connect_clicked(move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.reset();
                }
            });
        }
        hud.append(&close_btn);

        // Drag-to-move: translate pointer deltas into layer-shell margins,
        // clamped so the HUD stays on-screen and never rides over the bar.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((init_left, init_top)));
        {
            let start = start.clone();
            let pos = pos.clone();
            drag.connect_drag_begin(move |_, _, _| start.set(pos.get()));
        }
        {
            let window = window.clone();
            let pos = pos.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let (sx, sy) = start.get();
                let (mon_w, mon_h) = monitor_size();
                let w = window.width().max(1);
                let h = window.height().max(1);
                // Top margin is relative to the bar's bottom edge, so the usable
                // height is the screen minus the bar offset.
                let max_top = (mon_h - bar_off - h).max(min_top);
                let nl = (sx + dx as i32).clamp(0, (mon_w - w).max(0));
                let nt = (sy + dy as i32).clamp(min_top, max_top);
                window.set_margin(Edge::Left, nl);
                window.set_margin(Edge::Top, nt);
                pos.set((nl, nt));
            });
        }
        hud.add_controller(drag);

        window.set_child(Some(&hud));

        Self {
            window,
            label,
            pause_btn,
            pos,
        }
    }

    /// Bring the HUD on-screen at its remembered position. The window is created
    /// once and never unmapped; `present()` maps it the first time only.
    fn show(&self) {
        let (l, t) = self.pos.get();
        self.window.set_margin(Edge::Left, l);
        self.window.set_margin(Edge::Top, t);
        self.window.present();
    }

    /// "Hide" by parking the window below the screen rather than destroying the
    /// layer surface. Tearing the surface down (set_visible(false)) at
    /// timer-finish raced the Wayland teardown and disconnected the whole shell;
    /// keeping it mapped and off-screen avoids that entirely.
    fn hide(&self) {
        let (_, mon_h) = monitor_size();
        self.window.set_margin(Edge::Top, mon_h + 200);
    }

    fn set_time(&self, text: &str) {
        self.label.set_label(text);
    }

    fn set_paused(&self, paused: bool) {
        self.pause_btn.set_icon_name(if paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
    }
}

impl TimerPage {
    pub fn new() -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .halign(gtk::Align::Fill)
            .hexpand(true)
            .build();
        root.add_css_class("metis-timer-page");
        // Fit the Notification Center tools card (~356px content width).
        root.set_width_request(-1);

        let label = gtk::Label::new(Some("00:00:00"));
        label.add_css_class("metis-timer-digits");
        label.set_halign(gtk::Align::Center);
        root.append(&label);

        // ---- Setup area (quick start + steppers), hidden while running ----
        let setup = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();

        let quick_title = section_title(&metis_i18n::tr("Quick Start"));
        setup.append(&quick_title);
        let quick = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(4)
            .max_children_per_line(4)
            .column_spacing(6)
            .row_spacing(6)
            .homogeneous(true)
            .build();
        setup.append(&quick);

        let set_title = section_title(&metis_i18n::tr("Set Timer"));
        setup.append(&set_title);
        let steppers = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::Center)
            .build();
        let (h_col, h_lbl) = stepper();
        let (m_col, m_lbl) = stepper();
        let (s_col, s_lbl) = stepper();
        steppers.append(&h_col);
        steppers.append(&colon());
        steppers.append(&m_col);
        steppers.append(&colon());
        steppers.append(&s_col);
        setup.append(&steppers);
        root.append(&setup);

        // ---- Controls ----
        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        let primary = gtk::Button::with_label(&metis_i18n::tr("Start"));
        primary.add_css_class("metis-sw-btn");
        primary.add_css_class("metis-sw-btn-go");
        let reset_btn = gtk::Button::with_label(&metis_i18n::tr("Reset"));
        reset_btn.add_css_class("metis-sw-btn");
        reset_btn.add_css_class("metis-sw-btn-stop");
        reset_btn.set_sensitive(false);
        controls.append(&primary);
        controls.append(&reset_btn);
        root.append(&controls);

        let inner = Rc::new(Inner {
            hours: Cell::new(0),
            minutes: Cell::new(0),
            seconds: Cell::new(0),
            end: Cell::new(None),
            remaining: Cell::new(Duration::ZERO),
            generation: Cell::new(0),
            running: Cell::new(false),
            label,
            h_lbl,
            m_lbl,
            s_lbl,
            primary: primary.clone(),
            reset_btn: reset_btn.clone(),
            setup,
            overlay: RefCell::new(None),
        });

        // Stepper buttons (the +/- are the first/last children of each column).
        wire_stepper(&inner, &h_col, Field::Hours);
        wire_stepper(&inner, &m_col, Field::Minutes);
        wire_stepper(&inner, &s_col, Field::Seconds);

        for (label, secs) in [
            (metis_i18n::tr("1 m"), 60u32),
            (metis_i18n::tr("2 m"), 120),
            (metis_i18n::tr("3 m"), 180),
            (metis_i18n::tr("5 m"), 300),
            (metis_i18n::tr("15 m"), 900),
            (metis_i18n::tr("30 m"), 1800),
            (metis_i18n::tr("45 m"), 2700),
            (metis_i18n::tr("1 h"), 3600),
        ] {
            let btn = gtk::Button::with_label(&label);
            btn.add_css_class("metis-timer-preset");
            let inner = inner.clone();
            btn.connect_clicked(move |_| {
                inner.set_total(secs);
                inner.start();
            });
            quick.append(&btn);
        }

        {
            let inner = inner.clone();
            primary.connect_clicked(move |_| {
                if inner.running.get() {
                    inner.pause();
                } else {
                    inner.start();
                }
            });
        }
        {
            let inner = inner.clone();
            reset_btn.connect_clicked(move |_| inner.reset());
        }

        inner.refresh_setup_labels();

        Self {
            widget: root.upcast(),
        }
    }
}

#[derive(Clone, Copy)]
enum Field {
    Hours,
    Minutes,
    Seconds,
}

impl Inner {
    fn configured_secs(&self) -> u32 {
        self.hours.get() * 3600 + self.minutes.get() * 60 + self.seconds.get()
    }

    fn set_total(&self, secs: u32) {
        self.hours.set(secs / 3600);
        self.minutes.set((secs % 3600) / 60);
        self.seconds.set(secs % 60);
        self.refresh_setup_labels();
    }

    fn bump(&self, field: Field, up: bool) {
        let step = |v: u32, max: u32| {
            if up {
                (v + 1) % (max + 1)
            } else {
                (v + max) % (max + 1)
            }
        };
        match field {
            Field::Hours => self.hours.set(step(self.hours.get(), 23)),
            Field::Minutes => self.minutes.set(step(self.minutes.get(), 59)),
            Field::Seconds => self.seconds.set(step(self.seconds.get(), 59)),
        }
        self.refresh_setup_labels();
    }

    fn refresh_setup_labels(&self) {
        self.h_lbl.set_label(&format!("{:02}", self.hours.get()));
        self.m_lbl.set_label(&format!("{:02}", self.minutes.get()));
        self.s_lbl.set_label(&format!("{:02}", self.seconds.get()));
        if !self.running.get() && self.end.get().is_none() {
            self.label
                .set_label(&fmt(Duration::from_secs(self.configured_secs() as u64)));
        }
    }

    fn start(self: &Rc<Self>) {
        let remaining = if self.remaining.get() > Duration::ZERO {
            self.remaining.get()
        } else {
            Duration::from_secs(self.configured_secs() as u64)
        };
        if remaining.is_zero() {
            return;
        }
        self.running.set(true);
        self.end.set(Some(Instant::now() + remaining));
        self.primary.set_label(&metis_i18n::tr("Pause"));
        self.reset_btn.set_sensitive(true);
        self.setup.set_visible(false);

        // Starting a timer dismisses the Notification Center so the floating HUD
        // is the only thing left on screen.
        crate::ui::bar::dropdown::close_all();
        crate::ui::notification_center::dismiss();

        // Show (or update) the floating HUD under the bar.
        {
            let mut overlay = self.overlay.borrow_mut();
            if overlay.is_none() {
                *overlay = Some(TimerOverlay::new(&Rc::downgrade(self)));
            }
            if let Some(o) = overlay.as_ref() {
                o.set_paused(false);
                o.set_time(&fmt(remaining));
                o.show();
            }
        }

        let generation = self.generation.get();
        let inner = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if inner.generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            inner.tick()
        });
    }

    fn tick(self: &Rc<Self>) -> glib::ControlFlow {
        let Some(end) = self.end.get() else {
            return glib::ControlFlow::Break;
        };
        let now = Instant::now();
        if now >= end {
            self.running.set(false);
            self.end.set(None);
            self.remaining.set(Duration::ZERO);
            self.generation.set(self.generation.get().wrapping_add(1));
            self.label.set_label("00:00:00");
            self.primary.set_label(&metis_i18n::tr("Start"));
            self.reset_btn.set_sensitive(false);
            self.setup.set_visible(true);
            self.close_overlay();
            notify(
                &metis_i18n::tr("Timer finished"),
                &metis_i18n::tr("Your Metis timer is up."),
            );
            play_alarm_sound();
            return glib::ControlFlow::Break;
        }
        let left = end - now;
        self.label.set_label(&fmt(left));
        if let Some(o) = self.overlay.borrow().as_ref() {
            o.set_time(&fmt(left));
        }
        glib::ControlFlow::Continue
    }

    fn pause(&self) {
        self.running.set(false);
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(end) = self.end.take() {
            self.remaining
                .set(end.saturating_duration_since(Instant::now()));
        }
        self.primary.set_label(&metis_i18n::tr("Resume"));
        if let Some(o) = self.overlay.borrow().as_ref() {
            o.set_paused(true);
        }
    }

    fn reset(&self) {
        self.running.set(false);
        self.generation.set(self.generation.get().wrapping_add(1));
        self.end.set(None);
        self.remaining.set(Duration::ZERO);
        self.primary.set_label(&metis_i18n::tr("Start"));
        self.reset_btn.set_sensitive(false);
        self.setup.set_visible(true);
        self.close_overlay();
        self.refresh_setup_labels();
    }

    fn close_overlay(&self) {
        if let Some(o) = self.overlay.borrow().as_ref() {
            o.hide();
        }
    }
}

fn wire_stepper(inner: &Rc<Inner>, col: &gtk::Box, field: Field) {
    let up = col.first_child().and_downcast::<gtk::Button>();
    let down = col.last_child().and_downcast::<gtk::Button>();
    if let Some(up) = up {
        let inner = inner.clone();
        up.connect_clicked(move |_| inner.bump(field, true));
    }
    if let Some(down) = down {
        let inner = inner.clone();
        down.connect_clicked(move |_| inner.bump(field, false));
    }
}

fn stepper() -> (gtk::Box, gtk::Label) {
    let col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    col.add_css_class("metis-timer-stepper");
    let up = gtk::Button::from_icon_name("list-add-symbolic");
    up.add_css_class("metis-timer-step-btn");
    let value = gtk::Label::new(Some("00"));
    value.add_css_class("metis-timer-step-value");
    let down = gtk::Button::from_icon_name("list-remove-symbolic");
    down.add_css_class("metis-timer-step-btn");
    col.append(&up);
    col.append(&value);
    col.append(&down);
    (col, value)
}

fn colon() -> gtk::Label {
    let l = gtk::Label::new(Some(":"));
    l.add_css_class("metis-timer-colon");
    l.set_valign(gtk::Align::Center);
    l
}

fn section_title(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Center)
        .build();
    l.add_css_class("metis-timer-section");
    l
}

fn fmt(d: Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_millis() > 0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
