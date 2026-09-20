//! Windows-style "keep these display settings?" confirmation with auto-revert timer.
//! Prefers Settings v2 top-drop dialogs; falls back to a transient window.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use metis_i18n::tr;

use crate::dialog;

const CONFIRM_SECONDS: u32 = 15;

/// Apply the new arrangement, then ask the user to keep or revert it. Reverts
/// automatically when the countdown reaches zero.
pub fn show(parent: &gtk::Window, on_keep: Rc<dyn Fn()>, on_revert: Rc<dyn Fn()>) {
    parent.queue_draw();
    if let Some(surface) = parent.surface() {
        surface.queue_render();
    }

    let title = tr("Keep these display settings?");
    let body_label = gtk::Label::new(None);
    body_label.set_wrap(true);
    body_label.set_xalign(0.0);
    body_label.add_css_class("metis-settings-top-dialog-body");

    let remaining = Rc::new(RefCell::new(CONFIRM_SECONDS));
    let timer_id: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let resolved = Rc::new(std::cell::Cell::new(false));

    let update_body = {
        let body_label = body_label.clone();
        let remaining = remaining.clone();
        Rc::new(move || {
            let secs = *remaining.borrow();
            body_label.set_label(
                &tr(
                    "Your display settings have been applied. If everything still looks correct, \
                     click Keep changes.\n\nOtherwise the previous settings will be restored in \
                     %1 seconds.",
                )
                .replace("%1", &secs.to_string()),
            );
        })
    };
    update_body();

    let stop_timer = {
        let timer_id = timer_id.clone();
        Rc::new(move || {
            if let Some(id) = timer_id.borrow_mut().take() {
                id.remove();
            }
        })
    };

    let finish = {
        let resolved = resolved.clone();
        let stop_timer = stop_timer.clone();
        let on_keep = on_keep.clone();
        let on_revert = on_revert.clone();
        let parent = parent.clone();
        Rc::new(move |keep: bool| {
            if resolved.get() {
                return;
            }
            resolved.set(true);
            stop_timer();
            if keep {
                on_keep();
            } else {
                on_revert();
            }
            parent.queue_draw();
            if let Some(surface) = parent.surface() {
                surface.queue_render();
            }
        }) as Rc<dyn Fn(bool)>
    };

    let buttons = dialog::ConfirmButtons {
        cancel: tr("Revert"),
        accept: tr("Keep changes"),
        accept_destructive: false,
    };

    let used_top = dialog::confirm_with_extra(
        &title,
        "",
        Some(body_label.clone().upcast()),
        buttons,
        {
            let finish = finish.clone();
            Rc::new(move || finish(true))
        },
        {
            let finish = finish.clone();
            Rc::new(move || finish(false))
        },
    );

    if used_top {
        *timer_id.borrow_mut() = Some(glib::timeout_add_seconds_local(1, {
            let remaining = remaining.clone();
            let update_body = update_body.clone();
            let finish = finish.clone();
            move || {
                let next = remaining.borrow().saturating_sub(1);
                *remaining.borrow_mut() = next;
                update_body();
                if next == 0 {
                    finish(false);
                    dialog::dismiss_silent();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }
        }));
        return;
    }

    show_window_fallback(
        parent,
        title,
        body_label,
        remaining,
        timer_id,
        update_body,
        finish,
    );
}

fn show_window_fallback(
    parent: &gtk::Window,
    title: String,
    body: gtk::Label,
    remaining: Rc<RefCell<u32>>,
    timer_id: Rc<RefCell<Option<glib::SourceId>>>,
    update_body: Rc<dyn Fn()>,
    finish: Rc<dyn Fn(bool)>,
) {
    let dialog = gtk::Window::builder()
        .title(&title)
        .modal(true)
        .transient_for(parent)
        .resizable(false)
        .default_width(440)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-confirm-dialog");

    let sheet = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(24)
        .margin_end(24)
        .build();
    sheet.add_css_class("metis-settings-dialog-sheet");

    let heading = gtk::Label::new(Some(&title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    sheet.append(&heading);
    sheet.append(&body);

    let btn_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();

    let revert_btn = gtk::Button::with_label(&tr("Revert"));
    revert_btn.add_css_class("metis-settings-secondary");
    let keep_btn = gtk::Button::with_label(&tr("Keep changes"));
    keep_btn.add_css_class("suggested-action");
    btn_row.append(&revert_btn);
    btn_row.append(&keep_btn);
    sheet.append(&btn_row);
    dialog.set_child(Some(&sheet));

    let finish_and_close = {
        let finish = finish.clone();
        let dialog = dialog.clone();
        Rc::new(move |keep: bool| {
            finish(keep);
            dialog.set_modal(false);
            let dialog = dialog.clone();
            glib::idle_add_local_once(move || {
                dialog.destroy();
            });
        })
    };

    *timer_id.borrow_mut() = Some(glib::timeout_add_seconds_local(1, {
        let remaining = remaining.clone();
        let update_body = update_body.clone();
        let finish_and_close = finish_and_close.clone();
        move || {
            let next = remaining.borrow().saturating_sub(1);
            *remaining.borrow_mut() = next;
            update_body();
            if next == 0 {
                finish_and_close(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        }
    }));

    keep_btn.connect_clicked({
        let finish_and_close = finish_and_close.clone();
        move |_| finish_and_close(true)
    });
    revert_btn.connect_clicked({
        let finish_and_close = finish_and_close.clone();
        move |_| finish_and_close(false)
    });
    dialog.connect_close_request({
        let finish_and_close = finish_and_close.clone();
        move |_| {
            finish_and_close(false);
            glib::Propagation::Proceed
        }
    });

    dialog.present();
    dialog.queue_draw();
}
