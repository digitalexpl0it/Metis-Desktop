//! Software Updates window — summary, Install / Later, progress, live log.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use metis_remote::UpdateProgressEvent;

use crate::services;

thread_local! {
    static WINDOW: RefCell<Option<UpdaterState>> = const { RefCell::new(None) };
}

struct UpdaterState {
    window: gtk::Window,
    summary: gtk::Label,
    list: gtk::ListBox,
    progress: gtk::ProgressBar,
    status: gtk::Label,
    log_view: gtk::TextView,
    _log_revealer: gtk::Revealer,
    reboot_banner: gtk::Box,
    install_btn: gtk::Button,
    later_btn: gtk::Button,
    applying: Rc<Cell<bool>>,
}

/// Present the updater (create once, reuse).
pub fn show() {
    WINDOW.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            refresh_content(state);
            state.window.present();
            return;
        }
        let state = build();
        refresh_content(&state);
        state.window.present();
        *cell.borrow_mut() = Some(state);
    });
}

fn build() -> UpdaterState {
    // Under Metis the compositor draws SSD; hide GTK CSD to avoid double chrome.
    let under_metis = std::env::var_os("METIS_SESSION").is_some();
    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Software Updates"))
        .default_width(520)
        .default_height(480)
        .resizable(true)
        .decorated(!under_metis)
        .build();
    window.add_css_class("metis-updater");
    window.add_css_class("background");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.add_css_class("metis-updater-root");
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let header = gtk::Label::new(Some(&metis_i18n::tr("Software Updates")));
    header.add_css_class("title-2");
    header.add_css_class("metis-updater-title");
    header.set_halign(gtk::Align::Start);
    root.append(&header);

    let summary = gtk::Label::new(None);
    summary.set_halign(gtk::Align::Start);
    summary.set_wrap(true);
    summary.add_css_class("dim-label");
    root.append(&summary);

    let reboot_banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    reboot_banner.add_css_class("metis-updater-reboot");
    reboot_banner.set_visible(false);
    let reboot_icon = gtk::Image::from_icon_name("system-reboot-symbolic");
    reboot_icon.set_pixel_size(18);
    let reboot_lbl = gtk::Label::new(Some(&metis_i18n::tr(
        "A restart is required to finish installing updates.",
    )));
    reboot_lbl.set_wrap(true);
    reboot_lbl.set_halign(gtk::Align::Start);
    reboot_lbl.set_hexpand(true);
    reboot_banner.append(&reboot_icon);
    reboot_banner.append(&reboot_lbl);
    root.append(&reboot_banner);

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .min_content_height(120)
        .build();
    scroller.add_css_class("metis-updater-scroll");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.add_css_class("metis-updater-list");
    scroller.set_child(Some(&list));
    root.append(&scroller);

    let status = gtk::Label::new(None);
    status.set_halign(gtk::Align::Start);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.add_css_class("dim-label");
    root.append(&status);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    root.append(&progress);

    let log_toggle = gtk::ToggleButton::with_label(&metis_i18n::tr("Show log"));
    log_toggle.set_halign(gtk::Align::Start);
    log_toggle.add_css_class("metis-updater-log-toggle");
    root.append(&log_toggle);

    let log_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .reveal_child(false)
        .build();
    let log_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(140)
        .vexpand(true)
        .build();
    let log_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    log_view.add_css_class("metis-updater-log");
    log_scroll.set_child(Some(&log_view));
    log_revealer.set_child(Some(&log_scroll));
    root.append(&log_revealer);

    {
        let revealer = log_revealer.clone();
        log_toggle.connect_toggled(move |btn| {
            revealer.set_reveal_child(btn.is_active());
            let label = if btn.is_active() {
                metis_i18n::tr("Hide log")
            } else {
                metis_i18n::tr("Show log")
            };
            btn.set_label(&label);
        });
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(8);
    let later_btn = gtk::Button::with_label(&metis_i18n::tr("Later"));
    later_btn.add_css_class("metis-updater-btn");
    let install_btn = gtk::Button::with_label(&metis_i18n::tr("Install"));
    install_btn.add_css_class("suggested-action");
    install_btn.add_css_class("metis-updater-btn");
    actions.append(&later_btn);
    actions.append(&install_btn);
    root.append(&actions);

    window.set_child(Some(&root));

    let applying = Rc::new(Cell::new(false));

    {
        let window = window.clone();
        later_btn.connect_clicked(move |_| {
            services::updates_snooze_hours(24);
            window.close();
        });
    }

    let state_for_install = Rc::new(RefCell::new(None::<UpdaterHandles>));
    {
        let applying = applying.clone();
        let handles_slot = state_for_install.clone();
        install_btn.connect_clicked(move |btn| {
            if applying.get() {
                return;
            }
            applying.set(true);
            btn.set_sensitive(false);
            let Some(handles) = handles_slot.borrow().clone() else {
                applying.set(false);
                btn.set_sensitive(true);
                return;
            };
            handles.progress.set_visible(true);
            handles.progress.set_fraction(0.0);
            handles.status.set_text(&metis_i18n::tr("Preparing…"));
            handles.log_buffer.set_text("");

            let applying_cb = applying.clone();
            let install_btn = btn.clone();
            let on_event: Rc<dyn Fn(UpdateProgressEvent)> = Rc::new(move |ev| match ev {
                UpdateProgressEvent::Log { line } => {
                    let mut end = handles.log_buffer.end_iter();
                    handles.log_buffer.insert(&mut end, &format!("{line}\n"));
                }
                UpdateProgressEvent::Progress { percent, item } => {
                    handles
                        .progress
                        .set_fraction(f64::from(percent.clamp(0, 100)) / 100.0);
                    if let Some(name) = item {
                        handles.status.set_text(&name);
                    }
                }
                UpdateProgressEvent::Phase { name } => {
                    handles.status.set_text(&name);
                }
                UpdateProgressEvent::Finished {
                    ok,
                    error,
                    reboot_required,
                } => {
                    applying_cb.set(false);
                    install_btn.set_sensitive(true);
                    handles.progress.set_fraction(if ok {
                        1.0
                    } else {
                        handles.progress.fraction()
                    });
                    if ok {
                        handles
                            .status
                            .set_text(&metis_i18n::tr("Updates installed"));
                    } else {
                        handles
                            .status
                            .set_text(&error.unwrap_or_else(|| metis_i18n::tr("Update failed")));
                    }
                    handles.reboot_banner.set_visible(reboot_required);
                    WINDOW.with(|cell| {
                        if let Some(state) = cell.borrow().as_ref() {
                            refresh_content(state);
                        }
                    });
                }
            });
            services::updates_start_apply(on_event);
        });
    }

    window.connect_close_request(move |_| {
        WINDOW.with(|cell| {
            *cell.borrow_mut() = None;
        });
        glib::Propagation::Proceed
    });

    let state = UpdaterState {
        window,
        summary,
        list,
        progress,
        status,
        log_view,
        _log_revealer: log_revealer,
        reboot_banner,
        install_btn: install_btn.clone(),
        later_btn,
        applying,
    };

    *state_for_install.borrow_mut() = Some(UpdaterHandles {
        progress: state.progress.clone(),
        status: state.status.clone(),
        log_buffer: state.log_view.buffer(),
        reboot_banner: state.reboot_banner.clone(),
    });

    state
}

#[derive(Clone)]
struct UpdaterHandles {
    progress: gtk::ProgressBar,
    status: gtk::Label,
    log_buffer: gtk::TextBuffer,
    reboot_banner: gtk::Box,
}

fn refresh_content(state: &UpdaterState) {
    let snap = services::updates_snapshot();
    let count = snap.total_count();

    while let Some(row) = state.list.first_child() {
        state.list.remove(&row);
    }

    if count == 0 {
        state
            .summary
            .set_text(&metis_i18n::tr("Your system is up to date."));
        state.install_btn.set_sensitive(false);
        state.later_btn.set_sensitive(false);
    } else {
        state
            .summary
            .set_text(&metis_i18n::tr("%1 update(s) available.").replace("%1", &count.to_string()));
        if !state.applying.get() {
            state.install_btn.set_sensitive(true);
            state.later_btn.set_sensitive(true);
        }
    }

    append_section(
        &state.list,
        &metis_i18n::tr("System packages"),
        &snap.packages,
    );
    append_section(&state.list, &metis_i18n::tr("Flatpak"), &snap.flatpaks);
    append_section(&state.list, &metis_i18n::tr("Firmware"), &snap.firmware);

    state.reboot_banner.set_visible(snap.reboot_required);
    if let Some(err) = &snap.error {
        if count == 0 {
            state.status.set_text(err);
        }
    }
}

fn append_section(list: &gtk::ListBox, title: &str, items: &[metis_remote::UpdateItem]) {
    if items.is_empty() {
        return;
    }
    let header = gtk::Label::new(Some(title));
    header.set_halign(gtk::Align::Start);
    header.add_css_class("heading");
    header.set_margin_top(8);
    header.set_margin_bottom(4);
    let header_row = gtk::ListBoxRow::new();
    header_row.set_selectable(false);
    header_row.set_activatable(false);
    header_row.set_child(Some(&header));
    list.append(&header_row);

    for item in items {
        let row_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row_box.set_margin_top(4);
        row_box.set_margin_bottom(4);
        row_box.set_margin_start(8);
        row_box.set_margin_end(8);
        let name = gtk::Label::new(Some(&item.name));
        name.set_halign(gtk::Align::Start);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row_box.append(&name);
        if let Some(ver) = &item.version {
            let v = gtk::Label::new(Some(ver));
            v.set_halign(gtk::Align::Start);
            v.add_css_class("dim-label");
            v.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row_box.append(&v);
        }
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        list.append(&row);
    }
}
