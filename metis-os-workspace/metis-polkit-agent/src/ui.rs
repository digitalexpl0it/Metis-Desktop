//! GTK4 authentication dialog — layer-shell Overlay so it always sits above
//! xdg windows (updater, Settings, …) without them needing to hide.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::agent::UiResponse;

/// Gap below the edge bar for the top-center auth card.
const TOP_MARGIN: i32 = 56;
/// Slide in/out duration (matches shell toasts).
const SLIDE_MS: u32 = 280;

thread_local! {
    static OPEN: RefCell<HashMap<String, AuthSurface>> = RefCell::new(HashMap::new());
}

struct AuthSurface {
    window: gtk::Window,
    revealer: gtk::Revealer,
}

pub fn present_auth_dialog(
    app: &gtk::Application,
    cookie: String,
    message: String,
    action_id: String,
    usernames: Vec<String>,
    from_ui: async_channel::Sender<UiResponse>,
) {
    if OPEN.with(|m| m.borrow().contains_key(&cookie)) {
        OPEN.with(|m| {
            if let Some(s) = m.borrow().get(&cookie) {
                s.revealer.set_reveal_child(true);
                s.window.present();
            }
        });
        return;
    }

    // Undecorated layer surface — Metis draws no SSD for Overlay namespaces.
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Authentication Required")
        .resizable(false)
        .decorated(false)
        .default_width(420)
        .build();
    window.add_css_class("metis-polkit-window");

    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-polkit"));
    // Top-center: only Top anchored so the compositor centers horizontally.
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, TOP_MARGIN);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.add_css_class("metis-polkit-dialog");
    root.set_width_request(420);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name("dialog-password-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("metis-polkit-icon");
    header.append(&icon);

    // Body uses the polkit message as the only heading — window title already
    // says "Authentication Required".
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
    titles.set_hexpand(true);
    let msg = gtk::Label::new(Some(&message));
    msg.set_xalign(0.0);
    msg.set_wrap(true);
    msg.add_css_class("metis-polkit-title");
    titles.append(&msg);
    if !action_id.is_empty() {
        let action = gtk::Label::new(Some(&action_id));
        action.set_xalign(0.0);
        action.add_css_class("metis-polkit-action");
        titles.append(&action);
    }
    header.append(&titles);
    root.append(&header);

    let user_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let user_lbl = gtk::Label::new(Some("User"));
    user_lbl.set_xalign(0.0);
    user_lbl.set_width_chars(10);
    user_row.append(&user_lbl);

    let refs: Vec<&str> = usernames.iter().map(String::as_str).collect();
    let user_dd = gtk::DropDown::from_strings(&refs);
    user_dd.set_hexpand(true);
    user_dd.set_sensitive(usernames.len() > 1);
    user_row.append(&user_dd);
    root.append(&user_row);

    let pass_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let pass_lbl = gtk::Label::new(Some("Password"));
    pass_lbl.set_xalign(0.0);
    pass_lbl.set_width_chars(10);
    pass_row.append(&pass_lbl);
    // Plain Entry is lighter than PasswordEntry (no peek-icon / a11y chatter).
    let password = gtk::Entry::builder()
        .visibility(false)
        .input_purpose(gtk::InputPurpose::Password)
        .hexpand(true)
        .activates_default(true)
        .build();
    password.add_css_class("metis-polkit-password");
    pass_row.append(&password);
    root.append(&pass_row);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-polkit-error");
    err.set_visible(false);
    root.append(&err);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("flat");
    let auth = gtk::Button::with_label("Authenticate");
    auth.add_css_class("suggested-action");
    auth.set_receives_default(true);
    actions.append(&cancel);
    actions.append(&auth);
    root.append(&actions);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(SLIDE_MS)
        .reveal_child(false)
        .child(&root)
        .build();
    window.set_child(Some(&revealer));
    window.set_default_widget(Some(&auth));

    {
        let cookie = cookie.clone();
        let usernames = usernames.clone();
        let from_ui = from_ui.clone();
        let password = password.clone();
        let user_dd = user_dd.clone();
        let err = err.clone();
        let auth_btn = auth.clone();
        auth.connect_clicked(move |_| {
            let idx = user_dd.selected() as usize;
            let username = usernames
                .get(idx)
                .cloned()
                .or_else(|| usernames.first().cloned())
                .unwrap_or_default();
            if username.is_empty() {
                err.set_label("No user selected.");
                err.set_visible(true);
                return;
            }
            let pw = password.text().to_string();
            if pw.is_empty() {
                err.set_label("Enter your password.");
                err.set_visible(true);
                return;
            }
            err.set_visible(false);
            auth_btn.set_sensitive(false);
            // Never block the GTK main loop on channel send.
            let from_ui = from_ui.clone();
            let cookie = cookie.clone();
            glib::spawn_future_local(async move {
                let _ = from_ui
                    .send(UiResponse::Authenticate {
                        cookie,
                        username,
                        password: pw,
                    })
                    .await;
            });
        });
    }

    {
        let cookie = cookie.clone();
        let from_ui = from_ui.clone();
        cancel.connect_clicked(move |_| {
            let from_ui = from_ui.clone();
            let cookie = cookie.clone();
            glib::spawn_future_local(async move {
                let _ = from_ui
                    .send(UiResponse::Cancel {
                        cookie: cookie.clone(),
                    })
                    .await;
                close_dialog(&cookie);
            });
        });
    }

    {
        let cookie = cookie.clone();
        let from_ui = from_ui.clone();
        window.connect_close_request(move |_| {
            let from_ui = from_ui.clone();
            let cookie_for_send = cookie.clone();
            glib::spawn_future_local(async move {
                let _ = from_ui
                    .send(UiResponse::Cancel {
                        cookie: cookie_for_send,
                    })
                    .await;
            });
            // Slide up then destroy — don't Proceed destroy immediately.
            dismiss_surface(&cookie);
            glib::Propagation::Stop
        });
    }

    let surface = AuthSurface {
        window: window.clone().upcast(),
        revealer: revealer.clone(),
    };
    OPEN.with(|m| {
        m.borrow_mut().insert(cookie.clone(), surface);
    });

    window.present();
    // Reveal after map so SlideDown animates from the top edge.
    glib::idle_add_local_once(move || {
        revealer.set_reveal_child(true);
        password.grab_focus();
    });
}

pub fn cancel_dialog(cookie: &str) {
    close_dialog(cookie);
}

pub fn close_dialog(cookie: &str) {
    dismiss_surface(cookie);
}

fn dismiss_surface(cookie: &str) {
    let surface = OPEN.with(|m| m.borrow_mut().remove(cookie));
    let Some(surface) = surface else {
        return;
    };
    if surface.revealer.reveals_child() {
        surface.revealer.set_reveal_child(false);
        let window = surface.window;
        glib::timeout_add_local_once(Duration::from_millis(u64::from(SLIDE_MS)), move || {
            window.destroy();
        });
    } else {
        surface.window.destroy();
    }
}

pub fn show_retry(cookie: &str, retry_message: Option<String>) {
    OPEN.with(|m| {
        let map = m.borrow();
        let Some(surface) = map.get(cookie) else {
            return;
        };
        if let Some(root) = surface.revealer.child() {
            restore_dialog_after_retry(&root, retry_message.as_deref());
        }
        surface.revealer.set_reveal_child(true);
        surface.window.present();
    });
}

fn restore_dialog_after_retry(widget: &gtk::Widget, message: Option<&str>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>()
        && label.has_css_class("metis-polkit-error")
    {
        label.set_label(message.unwrap_or("Authentication failed. Please try again."));
        label.set_visible(true);
    }
    if let Some(btn) = widget.downcast_ref::<gtk::Button>()
        && btn.has_css_class("suggested-action")
    {
        btn.set_sensitive(true);
    }
    if let Some(entry) = widget.downcast_ref::<gtk::Entry>()
        && entry.has_css_class("metis-polkit-password")
    {
        entry.set_text("");
        entry.grab_focus();
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        restore_dialog_after_retry(&c, message);
        child = c.next_sibling();
    }
}
