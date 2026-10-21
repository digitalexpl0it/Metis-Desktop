//! `SSH_ASKPASS` helper for OpenSSH (`metis-ssh-askpass`).
//!
//! Prints the password to stdout when the user confirms. Used by `ssh-add` /
//! git over SSH when no TTY is available.

use std::cell::RefCell;
use std::env;
use std::process::ExitCode;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

fn main() -> ExitCode {
    let prompt = env::args()
        .nth(1)
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("SSH_ASKPASS_PROMPT").ok())
        .unwrap_or_else(|| "Enter passphrase for SSH key:".into());

    let app = gtk::Application::builder()
        .application_id("com.metis.SshAskpass")
        .build();

    let result: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let result_set = result.clone();

    app.connect_activate(move |app| {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Metis SSH")
            .default_width(360)
            .resizable(false)
            .build();

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
        vbox.set_margin_top(16);
        vbox.set_margin_bottom(16);
        vbox.set_margin_start(16);
        vbox.set_margin_end(16);

        let label = gtk::Label::new(Some(&prompt));
        label.set_wrap(true);
        label.set_xalign(0.0);
        vbox.append(&label);

        let entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .hexpand(true)
            .build();
        vbox.append(&entry);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("suggested-action");
        buttons.append(&cancel);
        buttons.append(&ok);
        vbox.append(&buttons);

        window.set_child(Some(&vbox));

        let win = window.clone();
        cancel.connect_clicked(move |_| {
            win.close();
        });

        let accept = {
            let win = window.clone();
            let entry = entry.clone();
            let result_set = result_set.clone();
            Rc::new(move || {
                *result_set.borrow_mut() = Some(entry.text().to_string());
                win.close();
            })
        };
        let accept_click = accept.clone();
        ok.connect_clicked(move |_| accept_click());
        let accept_activate = accept.clone();
        entry.connect_activate(move |_| accept_activate());

        window.connect_close_request(move |w| {
            if let Some(a) = w.application() {
                a.quit();
            }
            glib::Propagation::Proceed
        });

        window.present();
        entry.grab_focus();
    });

    let code = app.run();
    if code != glib::ExitCode::SUCCESS {
        return ExitCode::from(1);
    }
    if let Some(pw) = result.borrow().clone() {
        print!("{pw}");
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
