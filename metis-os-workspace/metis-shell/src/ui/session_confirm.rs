//! Center-screen confirmation for session power actions (log out / restart /
//! shut down). Avoids accidental clicks on the Metis Menu power rail.

use std::cell::RefCell;

use gtk::gdk::Key;
use gtk::glib::Propagation;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_i18n::tr;

/// Destructive session action that requires an explicit confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    LogOut,
    Restart,
    ShutDown,
}

impl SessionAction {
    fn title(self) -> String {
        match self {
            Self::LogOut => tr("Log out?"),
            Self::Restart => tr("Restart?"),
            Self::ShutDown => tr("Shut down?"),
        }
    }

    fn detail(self) -> String {
        match self {
            Self::LogOut => tr("Close all apps and return to the login screen."),
            Self::Restart => tr("Close all apps and reboot this computer."),
            Self::ShutDown => tr("Close all apps and power off this computer."),
        }
    }

    fn confirm_label(self) -> String {
        match self {
            Self::LogOut => tr("Log Out"),
            Self::Restart => tr("Restart"),
            Self::ShutDown => tr("Shut Down"),
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            Self::LogOut => "system-log-out-symbolic",
            Self::Restart => "system-reboot-symbolic",
            Self::ShutDown => "system-shutdown-symbolic",
        }
    }

    fn perform(self) {
        match self {
            Self::LogOut => perform_log_out(),
            Self::Restart => run_detached("systemctl", &["reboot"]),
            Self::ShutDown => run_detached("systemctl", &["poweroff"]),
        }
    }
}

thread_local! {
    static OVERLAY: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// Show a centered confirm dialog. Closes any open menu/dropdown first.
pub fn ask(action: SessionAction) {
    crate::ui::bar::close_popovers();
    dismiss();

    let window = gtk::Window::builder()
        .title(tr("Session"))
        .decorated(false)
        .build();
    window.add_css_class("metis-session-confirm-window");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-session-confirm"));
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_exclusive_zone(-1);
        window.set_margin(edge, 0);
    }

    let chrome = gtk::Overlay::new();
    chrome.add_css_class("metis-session-confirm-root");

    let backdrop = gtk::Button::new();
    backdrop.set_has_frame(false);
    backdrop.add_css_class("metis-session-confirm-backdrop");
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    chrome.set_child(Some(&backdrop));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.add_css_class("metis-session-confirm-card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_can_target(true);

    let icon = gtk::Image::from_icon_name(action.icon_name());
    icon.set_pixel_size(40);
    icon.add_css_class("metis-session-confirm-icon");
    icon.set_halign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&action.title()));
    title.add_css_class("metis-session-confirm-title");
    title.set_halign(gtk::Align::Center);

    let detail = gtk::Label::new(Some(&action.detail()));
    detail.add_css_class("metis-session-confirm-detail");
    detail.set_wrap(true);
    detail.set_justify(gtk::Justification::Center);
    detail.set_max_width_chars(36);
    detail.set_halign(gtk::Align::Center);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::Center);
    buttons.add_css_class("metis-session-confirm-actions");

    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("metis-session-confirm-cancel");

    let confirm = gtk::Button::with_label(&action.confirm_label());
    confirm.add_css_class("metis-session-confirm-ok");

    buttons.append(&cancel);
    buttons.append(&confirm);

    card.append(&icon);
    card.append(&title);
    card.append(&detail);
    card.append(&buttons);
    chrome.add_overlay(&card);
    window.set_child(Some(&chrome));

    let win_dismiss = window.clone();
    backdrop.connect_clicked(move |_| {
        win_dismiss.close();
    });

    let win_cancel = window.clone();
    cancel.connect_clicked(move |_| {
        win_cancel.close();
    });

    let win_confirm = window.clone();
    confirm.connect_clicked(move |_| {
        win_confirm.close();
        action.perform();
    });

    let key = gtk::EventControllerKey::new();
    let win_esc = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == Key::Escape {
            win_esc.close();
            Propagation::Stop
        } else {
            Propagation::Proceed
        }
    });
    window.add_controller(key);

    window.connect_close_request(|win| {
        OVERLAY.with(|cell| {
            if cell
                .borrow()
                .as_ref()
                .is_some_and(|existing| existing == win)
            {
                *cell.borrow_mut() = None;
            }
        });
        Propagation::Proceed
    });

    OVERLAY.with(|cell| *cell.borrow_mut() = Some(window.clone()));
    window.present();
    // Prefer Cancel as default focus so Enter alone does not power off.
    cancel.grab_focus();
}

/// Dismiss a pending confirm without performing the action.
pub fn dismiss() {
    OVERLAY.with(|cell| {
        if let Some(win) = cell.borrow_mut().take() {
            win.close();
        }
    });
}

fn perform_log_out() {
    if let Err(err) = crate::compositor::end_session() {
        tracing::warn!(%err, "compositor end_session failed — falling back to loginctl");
        if std::env::var_os("XDG_SESSION_ID").is_some() {
            run_detached("loginctl", &["terminate-session", "self"]);
        } else if let Ok(user) = std::env::var("USER") {
            run_detached("loginctl", &["terminate-user", &user]);
        }
    }
}

fn run_detached(cmd: &str, args: &[&str]) {
    if let Err(err) = std::process::Command::new(cmd).args(args).spawn() {
        tracing::warn!(%err, cmd, "failed to spawn power action");
    }
}
