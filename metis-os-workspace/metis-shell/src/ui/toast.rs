//! Transient on-screen notification banners ("toasts").
//!
//! A single `gtk4_layer_shell` overlay window is kept mapped for the process
//! lifetime (per the splash note about Wayland teardown races) and parked
//! off-screen when no toasts are visible. Each incoming notification appends a
//! card to a vertical stack anchored to the top-right, below the edge bar. Cards
//! auto-dismiss after the notification's `expire_timeout` with a short fade, and
//! reuse the same action/Open buttons as the popover cards.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::services::BarNotification;

/// Gap between the top of the screen / bar and the toast stack.
const TOP_MARGIN: i32 = 56;
/// Right-edge inset when the Notification Center is closed.
const RIGHT_MARGIN: i32 = 16;
/// Extra right inset while the Notification Center is open (~panel width + gap).
const RIGHT_MARGIN_WITH_PANEL: i32 = 420;
/// Fade-out duration before a card is removed.
const FADE_MS: u32 = 220;
/// Cap on simultaneously visible toasts; oldest is dropped past this.
const MAX_TOASTS: usize = 4;

struct Toast {
    window: gtk::Window,
    /// Vertical container holding one revealer per visible toast card.
    stack: gtk::Box,
}

thread_local! {
    static TOAST: RefCell<Option<Rc<RefCell<Toast>>>> = const { RefCell::new(None) };
    static PANEL_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Lazily build (or fetch) the shared toast overlay window. The window is never
/// destroyed; it is parked/hidden when empty and re-shown when a toast arrives.
fn overlay() -> Rc<RefCell<Toast>> {
    TOAST.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }

        let window = gtk::Window::builder()
            .title(metis_i18n::tr("Metis Notifications"))
            .build();
        window.add_css_class("metis-toast-window");
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace(Some("metis-toast"));
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        window.set_margin(Edge::Top, TOP_MARGIN);
        window.set_margin(Edge::Right, current_right_margin());

        let stack = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        stack.add_css_class("metis-toast-stack");
        window.set_child(Some(&stack));

        let toast = Rc::new(RefCell::new(Toast { window, stack }));
        *cell.borrow_mut() = Some(toast.clone());
        toast
    })
}

fn current_right_margin() -> i32 {
    // Only shift toasts when the Notification Center occupies the right edge.
    if PANEL_OPEN.with(Cell::get) && crate::ui::notification_center::anchors_right() {
        RIGHT_MARGIN_WITH_PANEL
    } else {
        RIGHT_MARGIN
    }
}

/// Shift toasts while the Notification Center panel occupies the right edge.
pub fn set_panel_open(open: bool) {
    PANEL_OPEN.with(|c| c.set(open));
    TOAST.with(|cell| {
        if let Some(toast) = cell.borrow().as_ref() {
            toast
                .borrow()
                .window
                .set_margin(Edge::Right, current_right_margin());
        }
    });
}

/// Show a transient toast for `note`. No-op for notifications with no
/// user-visible content. Callers gate this on Do Not Disturb.
pub fn show(note: &BarNotification) {
    let toast = overlay();

    // Slot lets a button's `on_done` dismiss *this* banner: it's filled with the
    // revealer right after construction, before any click can fire.
    let slot: Rc<RefCell<Option<gtk::Revealer>>> = Rc::new(RefCell::new(None));
    let on_done = {
        let toast = toast.clone();
        let slot = slot.clone();
        move || {
            if let Some(rev) = slot.borrow().clone() {
                dismiss(&toast, &rev);
            }
        }
    };

    let card = build_toast_card(note, on_done);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(FADE_MS)
        .reveal_child(false)
        .child(&card)
        .build();
    *slot.borrow_mut() = Some(revealer.clone());

    {
        let t = toast.borrow();
        t.stack.append(&revealer);
        t.window.set_visible(true);
        t.window.present();
        // Trim the oldest toast if we're over the cap.
        let mut count = 0;
        let mut child = t.stack.first_child();
        while let Some(c) = child {
            count += 1;
            child = c.next_sibling();
        }
        if count > MAX_TOASTS as i32
            && let Some(first) = t.stack.first_child()
        {
            t.stack.remove(&first);
        }
    }

    // Reveal on the next tick so the slide-in animation actually plays.
    {
        let revealer = revealer.clone();
        glib::idle_add_local_once(move || revealer.set_reveal_child(true));
    }

    // Auto-dismiss after the notification's requested lifetime.
    let duration = note.toast_duration_ms();
    let toast_dismiss = toast.clone();
    let revealer_dismiss = revealer.clone();
    glib::timeout_add_local_once(Duration::from_millis(duration), move || {
        dismiss(&toast_dismiss, &revealer_dismiss);
    });
}

/// Collapse a single toast card with a fade, then remove it and park the window
/// if the stack is now empty.
fn dismiss(toast: &Rc<RefCell<Toast>>, revealer: &gtk::Revealer) {
    if revealer.parent().is_none() {
        return;
    }
    revealer.set_reveal_child(false);
    let toast = toast.clone();
    let revealer = revealer.clone();
    glib::timeout_add_local_once(Duration::from_millis(FADE_MS as u64 + 20), move || {
        let t = toast.borrow();
        if revealer.parent().is_some() {
            t.stack.remove(&revealer);
        }
        if t.stack.first_child().is_none() {
            // Park off-screen rather than unmap to avoid layer-shell teardown races.
            t.window.set_visible(false);
        }
    });
}

/// Build a toast card mirroring the popover card layout (icon, title, body,
/// action buttons). `on_done` dismisses this banner once an action fires.
fn build_toast_card<F>(note: &BarNotification, on_done: F) -> gtk::Box
where
    F: Fn() + Clone + 'static,
{
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    card.add_css_class("metis-toast-card");
    card.add_css_class(&format!("metis-notif-card-{}", note.kind.css_suffix()));
    card.set_width_request(360);

    let icon = gtk::Image::from_icon_name(note.kind.icon_name());
    icon.add_css_class("metis-notif-icon");
    icon.set_valign(gtk::Align::Start);
    card.append(&icon);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();

    let title_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();

    let title = gtk::Label::builder()
        .label(&note.title)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(30)
        .build();
    title.add_css_class("metis-notif-title");
    title_row.append(&title);

    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("metis-toast-close");
    close.set_tooltip_text(Some(&metis_i18n::tr("Dismiss")));
    close.set_valign(gtk::Align::Start);
    {
        let on_done = on_done.clone();
        close.connect_clicked(move |_| on_done());
    }
    title_row.append(&close);
    text.append(&title_row);

    if !note.message.is_empty() {
        let message = gtk::Label::builder()
            .label(&note.message)
            .halign(gtk::Align::Fill)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(34)
            .build();
        message.add_css_class("metis-notif-message");
        text.append(&message);
    }

    if let Some(row) = crate::ui::bar::widgets::build_action_row(note, on_done) {
        text.append(&row);
    }

    card.append(&text);
    card
}
