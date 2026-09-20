//! GTK4 authentication dialog.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;

use crate::agent::UiResponse;

thread_local! {
    static OPEN: RefCell<HashMap<String, gtk::Window>> = RefCell::new(HashMap::new());
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
            if let Some(w) = m.borrow().get(&cookie) {
                w.present();
            }
        });
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Authentication Required")
        .resizable(false)
        .modal(true)
        .default_width(420)
        .build();
    window.add_css_class("metis-polkit-window");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(20);
    root.set_margin_end(20);
    root.add_css_class("metis-polkit-dialog");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name("dialog-password-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("metis-polkit-icon");
    header.append(&icon);

    let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
    titles.set_hexpand(true);
    let title = gtk::Label::new(Some("Authentication Required"));
    title.set_xalign(0.0);
    title.add_css_class("metis-polkit-title");
    let msg = gtk::Label::new(Some(&message));
    msg.set_xalign(0.0);
    msg.set_wrap(true);
    msg.add_css_class("metis-polkit-message");
    titles.append(&title);
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
    let password = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .activates_default(true)
        .build();
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

    window.set_child(Some(&root));
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
            let _ = from_ui.send_blocking(UiResponse::Authenticate {
                cookie: cookie.clone(),
                username,
                password: pw,
            });
        });
    }

    {
        let cookie = cookie.clone();
        let from_ui = from_ui.clone();
        cancel.connect_clicked(move |_| {
            let _ = from_ui.send_blocking(UiResponse::Cancel {
                cookie: cookie.clone(),
            });
            close_dialog(&cookie);
        });
    }

    {
        let cookie = cookie.clone();
        let from_ui = from_ui.clone();
        window.connect_close_request(move |_| {
            let _ = from_ui.send_blocking(UiResponse::Cancel {
                cookie: cookie.clone(),
            });
            OPEN.with(|m| {
                m.borrow_mut().remove(&cookie);
            });
            glib::Propagation::Proceed
        });
    }

    OPEN.with(|m| {
        m.borrow_mut()
            .insert(cookie.clone(), window.clone().upcast());
    });

    window.present();
    password.grab_focus();
}

pub fn cancel_dialog(cookie: &str) {
    close_dialog(cookie);
}

pub fn close_dialog(cookie: &str) {
    OPEN.with(|m| {
        if let Some(w) = m.borrow_mut().remove(cookie) {
            w.destroy();
        }
    });
}

pub fn show_retry(cookie: &str, retry_message: Option<String>) {
    OPEN.with(|m| {
        let map = m.borrow();
        let Some(win) = map.get(cookie) else {
            return;
        };
        if let Some(root) = win.child() {
            restore_dialog_after_retry(&root, retry_message.as_deref());
        }
        win.present();
    });
}

fn restore_dialog_after_retry(widget: &gtk::Widget, message: Option<&str>) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        if label.has_css_class("metis-polkit-error") {
            label.set_label(message.unwrap_or("Authentication failed. Please try again."));
            label.set_visible(true);
        }
    }
    if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
        if btn.has_css_class("suggested-action") {
            btn.set_sensitive(true);
        }
    }
    if let Some(entry) = widget.downcast_ref::<gtk::PasswordEntry>() {
        entry.set_text("");
        entry.grab_focus();
    }
    if let Some(bx) = widget.downcast_ref::<gtk::Box>() {
        let mut child = bx.first_child();
        while let Some(c) = child {
            restore_dialog_after_retry(&c, message);
            child = c.next_sibling();
        }
    }
}
