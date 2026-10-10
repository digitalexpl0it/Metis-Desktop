//! Top-slide sheets for Settings (color / font pickers, confirms).
//!
//! Hosted as GtkOverlay children so the sheet floats over chrome without
//! pushing layout. When closed, overlays are invisible and `can_target=false`
//! so they cannot steal clicks (the old Overlay pitfall on GTK 4.22).
//!
//! Hosts stack: the main Settings window installs one, and modal Configure
//! dialogs install another on top so Pick a Color / Font land on the active
//! window (not behind a modal on the parent).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use metis_i18n::tr;

use crate::motion;

struct SheetHost {
    dimmer: gtk::Box,
    revealer: gtk::Revealer,
    title: gtk::Label,
    body: gtk::Box,
    on_cancel: Option<Rc<dyn Fn()>>,
}

thread_local! {
    /// Stack of sheet hosts — topmost is used by [`show_sheet`] / [`dismiss`].
    static HOSTS: RefCell<Vec<SheetHost>> = const { RefCell::new(Vec::new()) };
}

/// Install a top-sheet host on `overlay` (chrome is the overlay's child).
/// Push onto the host stack so nested modals can own their own sheet.
pub fn install(overlay: &gtk::Overlay) {
    // Full-bleed dimmer — paints a scrim and dismisses on click. Never use
    // opacity on the chrome itself (wallpaper bleeds through translucent UI).
    let dimmer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dimmer.add_css_class("metis-settings-dialog-dimmer");
    dimmer.set_hexpand(true);
    dimmer.set_vexpand(true);
    dimmer.set_halign(gtk::Align::Fill);
    dimmer.set_valign(gtk::Align::Fill);
    dimmer.set_visible(false);
    dimmer.set_can_target(false);
    {
        let click = gtk::GestureClick::new();
        click.connect_released(|_, _, _, _| {
            dismiss(true);
        });
        dimmer.add_controller(click);
    }

    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("metis-settings-top-dialog");
    card.set_halign(gtk::Align::Center);
    card.set_hexpand(false);
    card.set_margin_top(12);
    card.set_margin_bottom(8);
    card.set_margin_start(24);
    card.set_margin_end(24);
    card.set_size_request(440, -1);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("metis-settings-secondary");
    cancel.add_css_class("flat");
    let title = gtk::Label::new(None);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Center);
    title.add_css_class("metis-settings-top-dialog-title");
    header.append(&cancel);
    header.append(&title);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_size_request(72, -1);
    header.append(&spacer);
    card.append(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("metis-settings-top-sheet-body");
    card.append(&body);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(motion::ms(200))
        .reveal_child(false)
        .child(&card)
        .build();
    revealer.set_halign(gtk::Align::Fill);
    revealer.set_valign(gtk::Align::Start);
    revealer.set_hexpand(true);
    revealer.set_vexpand(false);
    revealer.set_visible(false);
    revealer.set_can_target(false);

    // Dimmer under the sheet so the card stays fully opaque and clickable.
    overlay.add_overlay(&dimmer);
    overlay.add_overlay(&revealer);

    cancel.connect_clicked(|_| dismiss(true));

    {
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                dismiss(true);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        card.add_controller(key);
    }

    HOSTS.with(|hosts| {
        hosts.borrow_mut().push(SheetHost {
            dimmer,
            revealer,
            title,
            body,
            on_cancel: None,
        });
    });
}

/// Pop the most recently installed host (call when a modal Configure window
/// closes so sheets return to the main Settings overlay).
pub fn uninstall() {
    dismiss_silent();
    HOSTS.with(|hosts| {
        hosts.borrow_mut().pop();
    });
}

fn show_sheet(title: &str, content: &impl IsA<gtk::Widget>, on_cancel: Rc<dyn Fn()>) {
    HOSTS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        let Some(host) = hosts.last_mut() else {
            return;
        };
        while let Some(child) = host.body.first_child() {
            host.body.remove(&child);
        }
        host.title.set_label(title);
        host.body.append(content);
        host.on_cancel.replace(on_cancel);

        host.dimmer.set_visible(true);
        host.dimmer.set_can_target(true);
        host.revealer.set_transition_duration(motion::ms(200));
        host.revealer.set_visible(true);
        host.revealer.set_can_target(true);
        host.revealer.set_reveal_child(true);
    });
}

/// True while a top-sheet is revealed (or animating closed still visible).
pub fn is_open() -> bool {
    HOSTS.with(|hosts| {
        hosts
            .borrow()
            .last()
            .map(|h| h.revealer.is_visible() || h.revealer.reveals_child())
            .unwrap_or(false)
    })
}

/// Close the sheet. If `run_cancel` is true, invoke the cancel callback.
pub fn dismiss(run_cancel: bool) {
    HOSTS.with(|hosts| {
        let mut hosts = hosts.borrow_mut();
        let Some(host) = hosts.last_mut() else {
            return;
        };
        let cancel = if run_cancel {
            host.on_cancel.take()
        } else {
            host.on_cancel = None;
            None
        };
        host.revealer.set_reveal_child(false);
        host.revealer.set_can_target(false);
        host.dimmer.set_can_target(false);
        host.dimmer.set_visible(false);
        let rev = host.revealer.clone();
        let delay = u64::from(host.revealer.transition_duration()).saturating_add(40);
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            rev.set_visible(false);
        });
        if let Some(cb) = cancel {
            cb();
        }
    });
}

pub fn dismiss_silent() {
    dismiss(false);
}

fn host_ready() -> bool {
    HOSTS.with(|hosts| !hosts.borrow().is_empty())
}

/// Open a top-slide colour picker. `on_done(None)` = cancelled.
///
/// Uses the deprecated ColorChooserWidget deliberately so the picker lives
/// inside our top-sheet (ColorDialog always opens a separate window).
#[allow(deprecated)]
pub fn pick_color(initial: gdk::RGBA, on_done: Rc<dyn Fn(Option<gdk::RGBA>)>) {
    if !host_ready() {
        on_done(None);
        return;
    }

    let chooser = gtk::ColorChooserWidget::new();
    chooser.set_use_alpha(false);
    chooser.set_rgba(&initial);
    chooser.add_css_class("metis-settings-color-chooser");
    chooser.set_halign(gtk::Align::Fill);
    chooser.set_hexpand(true);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .max_content_height(360)
        .child(&chooser)
        .build();
    scroll.set_hexpand(true);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let select = gtk::Button::with_label(&tr("Select"));
    select.add_css_class("suggested-action");
    actions.append(&select);

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 12);
    wrap.add_css_class("metis-settings-top-sheet-body");
    wrap.set_hexpand(true);
    wrap.append(&scroll);
    wrap.append(&actions);

    let resolved = Rc::new(Cell::new(false));
    let finish = {
        let resolved = resolved.clone();
        let done = on_done.clone();
        Rc::new(move |rgba: Option<gdk::RGBA>| {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            dismiss(false);
            done(rgba);
        })
    };

    {
        let chooser = chooser.clone();
        let finish = finish.clone();
        select.connect_clicked(move |_| {
            finish(Some(chooser.rgba()));
        });
    }
    {
        let finish = finish.clone();
        chooser.connect_color_activated(move |_chooser, rgba| {
            finish(Some(*rgba));
        });
    }

    let on_cancel = {
        let finish = finish.clone();
        Rc::new(move || finish(None)) as Rc<dyn Fn()>
    };
    show_sheet(&tr("Pick a Color"), &wrap, on_cancel);
}

/// Open a top-slide font picker. `on_done(None)` = cancelled.
///
/// Uses the deprecated FontChooserWidget deliberately so the picker lives
/// inside our top-sheet (FontDialog always opens a separate window).
#[allow(deprecated)]
pub fn pick_font(
    initial: Option<pango::FontDescription>,
    on_done: Rc<dyn Fn(Option<pango::FontDescription>)>,
) {
    if !host_ready() {
        on_done(None);
        return;
    }

    let chooser = gtk::FontChooserWidget::new();
    chooser.add_css_class("metis-settings-font-chooser");
    if let Some(desc) = initial.as_ref() {
        chooser.set_font_desc(desc);
    }
    chooser.set_size_request(400, 300);
    chooser.set_halign(gtk::Align::Fill);
    chooser.set_hexpand(true);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let select = gtk::Button::with_label(&tr("Select"));
    select.add_css_class("suggested-action");
    actions.append(&select);

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 12);
    wrap.add_css_class("metis-settings-top-sheet-body");
    wrap.set_hexpand(true);
    wrap.append(&chooser);
    wrap.append(&actions);

    let resolved = Rc::new(Cell::new(false));
    let finish = {
        let resolved = resolved.clone();
        let done = on_done.clone();
        Rc::new(move |desc: Option<pango::FontDescription>| {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            dismiss(false);
            done(desc);
        })
    };

    {
        let chooser = chooser.clone();
        let finish = finish.clone();
        select.connect_clicked(move |_| {
            finish(chooser.font_desc());
        });
    }

    let on_cancel = {
        let finish = finish.clone();
        Rc::new(move || finish(None)) as Rc<dyn Fn()>
    };
    show_sheet(&tr("Pick a Font"), &wrap, on_cancel);
}

/// Present arbitrary content in the top-slide sheet. Caller owns actions and
/// dismisses via [`dismiss`] / [`dismiss_silent`]. Returns false if the host
/// is not installed.
pub fn present(title: &str, content: &impl IsA<gtk::Widget>, on_cancel: Rc<dyn Fn()>) -> bool {
    if !host_ready() {
        return false;
    }
    show_sheet(title, content, on_cancel);
    true
}

pub struct ConfirmButtons {
    pub cancel: String,
    pub accept: String,
    pub accept_destructive: bool,
}

impl Default for ConfirmButtons {
    fn default() -> Self {
        Self {
            cancel: tr("Cancel"),
            accept: tr("OK"),
            accept_destructive: false,
        }
    }
}

/// Top-slide OK/Cancel. Returns false if the host is not installed.
pub fn confirm_with_extra(
    title: &str,
    body: &str,
    extra_child: Option<gtk::Widget>,
    buttons: ConfirmButtons,
    on_accept: Rc<dyn Fn()>,
    on_cancel: Rc<dyn Fn()>,
) -> bool {
    if !host_ready() {
        return false;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    if !body.is_empty() {
        let label = gtk::Label::new(Some(body));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.add_css_class("metis-settings-top-dialog-body");
        wrap.append(&label);
    }
    if let Some(extra) = extra_child {
        wrap.append(&extra);
    }
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label(&buttons.cancel);
    cancel_btn.add_css_class("metis-settings-secondary");
    let accept_btn = gtk::Button::with_label(&buttons.accept);
    if buttons.accept_destructive {
        accept_btn.add_css_class("destructive-action");
    } else {
        accept_btn.add_css_class("suggested-action");
    }
    actions.append(&cancel_btn);
    actions.append(&accept_btn);
    wrap.append(&actions);

    let resolved = Rc::new(Cell::new(false));
    {
        let resolved = resolved.clone();
        let on_accept = on_accept.clone();
        accept_btn.connect_clicked(move |_| {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            dismiss(false);
            on_accept();
        });
    }
    {
        let resolved = resolved.clone();
        let on_cancel = on_cancel.clone();
        cancel_btn.connect_clicked(move |_| {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            dismiss(false);
            on_cancel();
        });
    }

    let on_cancel_sheet = {
        let resolved = resolved.clone();
        let on_cancel = on_cancel.clone();
        Rc::new(move || {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            on_cancel();
        }) as Rc<dyn Fn()>
    };

    show_sheet(title, &wrap, on_cancel_sheet);
    true
}

#[allow(dead_code)]
pub fn confirm(
    title: &str,
    body: &str,
    buttons: ConfirmButtons,
    on_accept: Rc<dyn Fn()>,
    on_cancel: Rc<dyn Fn()>,
) -> bool {
    confirm_with_extra(title, body, None, buttons, on_accept, on_cancel)
}
