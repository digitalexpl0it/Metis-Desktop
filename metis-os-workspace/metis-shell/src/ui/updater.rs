//! Software Updates window — Metis SSD xdg_toplevel with a forced-opaque surface.
//!
//! Must NOT be a layer-shell Overlay: those sit above polkit and block the
//! auth dialog. Must paint with fully-opaque `rgb()` (global shell CSS makes
//! every `window` transparent for the bar / OSD).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use metis_remote::UpdateProgressEvent;

use crate::services;
use crate::ui::theme;

const DEFAULT_WIDTH: i32 = 460;

thread_local! {
    static WINDOW: RefCell<Option<Rc<UpdaterState>>> = const { RefCell::new(None) };
    static OPAQUE_CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

struct UpdaterState {
    window: gtk::Window,
    summary: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    progress: gtk::ProgressBar,
    status: gtk::Label,
    log_view: gtk::TextView,
    log_revealer: gtk::Revealer,
    log_toggle: gtk::ToggleButton,
    reboot_banner: gtk::Box,
    install_btn: gtk::Button,
    later_btn: gtk::Button,
    applying: Rc<Cell<bool>>,
}

/// Present the updater (create once, reuse).
pub fn show() {
    // Clone out of the RefCell *before* the if/else so the temporary `borrow()`
    // does not live across `borrow_mut()` (that panic killed the whole edge bar).
    // Re-applied each time so the reused window follows theme changes.
    ensure_opaque_css();
    let existing = WINDOW.with(|cell| cell.borrow().clone());
    if let Some(state) = existing {
        refresh_content(&state);
        state.window.set_visible(true);
        state.window.present();
    } else {
        let state = Rc::new(build());
        refresh_content(&state);
        state.window.present();
        WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(state);
        });
    }
    // Defer the soft check so opening the window never races PackageKit I/O
    // against the first pointer frame.
    glib::timeout_add_local_once(std::time::Duration::from_secs(2), || {
        services::updates_request_check(false);
    });
}

fn ensure_opaque_css() {
    let tokens = theme::active_tokens();
    let surface_rgb = tokens.surface_rgb();
    let raised_rgb = tokens.surface_raised_rgb();
    let text = tokens.text.clone();
    let css = format!(
        r#"
        window.metis-updater {{
            background-color: rgb({surface_rgb});
            color: {text};
        }}
        window.metis-updater > box.metis-updater-shell {{
            background-color: rgb({surface_rgb});
            color: {text};
        }}
        .metis-updater-root {{
            background-color: rgb({surface_rgb});
            color: {text};
        }}
        .metis-updater-list {{
            background-color: rgb({raised_rgb});
        }}
        "#
    );
    let Some(display) = gdk::Display::default() else {
        tracing::warn!("no GDK display — updater keeps the shell theme");
        return;
    };
    OPAQUE_CSS.with(|slot| {
        let mut binding = slot.borrow_mut();
        let fresh = binding.is_none();
        let provider = binding.get_or_insert_with(gtk::CssProvider::new);
        provider.load_from_string(&css);
        if fresh {
            // USER beats the shell's APPLICATION-level `window { transparent }`.
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
}

fn build() -> UpdaterState {
    let under_metis = std::env::var_os("METIS_SESSION").is_some();
    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Software Updates"))
        .default_width(DEFAULT_WIDTH)
        .default_height(-1)
        .resizable(true)
        // Under Metis the compositor draws SSD (move / close). Elsewhere CSD.
        .decorated(!under_metis)
        .build();
    window.add_css_class("metis-updater");
    window.add_css_class("background");

    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("metis-updater-shell");
    shell.set_hexpand(true);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("metis-updater-root");
    root.set_hexpand(true);

    let summary = gtk::Label::new(None);
    summary.set_halign(gtk::Align::Start);
    summary.set_wrap(true);
    summary.add_css_class("dim-label");
    summary.add_css_class("metis-updater-summary");
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
        .hexpand(true)
        .vexpand(false)
        .propagate_natural_height(true)
        .max_content_height(240)
        .min_content_height(0)
        .build();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
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
    status.set_visible(false);
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
        .transition_duration(180)
        .reveal_child(false)
        .vexpand(false)
        .build();
    let log_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(120)
        .max_content_height(160)
        .vexpand(false)
        .hexpand(true)
        .build();
    log_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
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
        let window = window.clone();
        log_toggle.connect_toggled(move |btn| {
            let open = btn.is_active();
            revealer.set_reveal_child(open);
            btn.set_label(&if open {
                metis_i18n::tr("Hide log")
            } else {
                metis_i18n::tr("Show log")
            });
            if !open {
                window.set_default_size(DEFAULT_WIDTH, -1);
            }
        });
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    actions.add_css_class("metis-updater-actions");
    actions.set_halign(gtk::Align::End);
    let later_btn = gtk::Button::with_label(&metis_i18n::tr("Later"));
    later_btn.add_css_class("metis-updater-btn");
    let install_btn = gtk::Button::with_label(&metis_i18n::tr("Install"));
    install_btn.add_css_class("suggested-action");
    install_btn.add_css_class("metis-updater-btn");
    actions.append(&later_btn);
    actions.append(&install_btn);
    root.append(&actions);

    shell.append(&root);
    window.set_child(Some(&shell));

    let applying = Rc::new(Cell::new(false));

    {
        // Weak ref, no WINDOW access: `close()` synchronously emits
        // `close_request`, and touching the RefCell here panicked the shell.
        let window_weak = window.downgrade();
        later_btn.connect_clicked(move |_| {
            if let Some(window) = window_weak.upgrade() {
                window.close();
            }
        });
    }

    let state_for_install = Rc::new(RefCell::new(None::<UpdaterHandles>));
    {
        let applying = applying.clone();
        let handles_slot = state_for_install.clone();
        let window_for_auth = window.clone();
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
            handles.status.set_visible(true);
            handles
                .status
                .set_text(&metis_i18n::tr("Waiting for authentication…"));
            handles.log_buffer.set_text("");
            // Keep log collapsed — TextView inserts during install thrash the
            // main loop and make the pointer unusable.
            handles.log_revealer.set_reveal_child(false);
            handles.log_toggle.set_active(false);

            // Drop under polkit: Overlay previously covered the password dialog.
            // Hiding the xdg window lets Authentication Required take focus.
            window_for_auth.set_visible(false);

            let applying_cb = applying.clone();
            let install_btn = btn.clone();
            let window_show = window_for_auth.clone();
            let pending_log: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
            let revealed = Rc::new(Cell::new(false));
            let on_event: Rc<dyn Fn(UpdateProgressEvent)> = Rc::new(move |ev| {
                // First real progress → auth done; bring the window back.
                if !revealed.get() {
                    match &ev {
                        UpdateProgressEvent::Progress { .. }
                        | UpdateProgressEvent::Phase { .. }
                        | UpdateProgressEvent::Finished { .. } => {
                            revealed.set(true);
                            window_show.set_visible(true);
                            window_show.present();
                        }
                        UpdateProgressEvent::Log { .. } => {}
                    }
                }
                match ev {
                    UpdateProgressEvent::Log { line } => {
                        // Only buffer; flush when the user has the log open.
                        if handles.log_revealer.reveals_child() {
                            pending_log.borrow_mut().push_str(&line);
                            pending_log.borrow_mut().push('\n');
                            if pending_log.borrow().len() >= 1024 {
                                flush_log(&handles.log_buffer, &pending_log);
                            }
                        }
                    }
                    UpdateProgressEvent::Progress { percent, item } => {
                        if handles.log_revealer.reveals_child() {
                            flush_log(&handles.log_buffer, &pending_log);
                        }
                        handles
                            .progress
                            .set_fraction(f64::from(percent.clamp(0, 100)) / 100.0);
                        if let Some(name) = item {
                            handles.status.set_text(&name);
                        }
                    }
                    UpdateProgressEvent::Phase { name } => {
                        if handles.log_revealer.reveals_child() {
                            flush_log(&handles.log_buffer, &pending_log);
                        }
                        handles.status.set_text(&name);
                    }
                    UpdateProgressEvent::Finished {
                        ok,
                        error,
                        reboot_required,
                    } => {
                        if handles.log_revealer.reveals_child() {
                            flush_log(&handles.log_buffer, &pending_log);
                        }
                        applying_cb.set(false);
                        install_btn.set_sensitive(true);
                        window_show.set_visible(true);
                        window_show.present();
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
                            handles.status.set_text(
                                &error.unwrap_or_else(|| metis_i18n::tr("Update failed")),
                            );
                        }
                        handles.reboot_banner.set_visible(reboot_required);
                        WINDOW.with(|cell| {
                            if let Some(state) = cell.borrow().as_ref() {
                                refresh_content(state);
                            }
                        });
                    }
                }
            });
            services::updates_start_apply(on_event);
        });
    }

    // Hide instead of destroy: an in-flight install still holds the widgets,
    // and `show()` reuses the same window. No RefCell access on this path.
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });

    let state = UpdaterState {
        window,
        summary,
        list,
        scroller,
        progress,
        status,
        log_view,
        log_revealer: log_revealer.clone(),
        log_toggle: log_toggle.clone(),
        reboot_banner,
        install_btn: install_btn.clone(),
        later_btn,
        applying,
    };

    *state_for_install.borrow_mut() = Some(UpdaterHandles {
        progress: state.progress.clone(),
        status: state.status.clone(),
        log_buffer: state.log_view.buffer(),
        log_revealer: state.log_revealer.clone(),
        log_toggle: state.log_toggle.clone(),
        reboot_banner: state.reboot_banner.clone(),
    });

    {
        let refresh: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(|| {
            WINDOW.with(|cell| {
                if let Some(state) = cell.borrow().as_ref()
                    && state.window.is_visible()
                {
                    refresh_content(state);
                }
            });
        });
        services::register_updates_updater_refresh(refresh);
    }

    state
}

fn flush_log(buffer: &gtk::TextBuffer, pending: &Rc<RefCell<String>>) {
    let chunk = {
        let mut p = pending.borrow_mut();
        if p.is_empty() {
            return;
        }
        std::mem::take(&mut *p)
    };
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &chunk);
}

#[derive(Clone)]
struct UpdaterHandles {
    progress: gtk::ProgressBar,
    status: gtk::Label,
    log_buffer: gtk::TextBuffer,
    log_revealer: gtk::Revealer,
    log_toggle: gtk::ToggleButton,
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
        state.later_btn.set_sensitive(true);
        state.later_btn.set_label(&metis_i18n::tr("Close"));
        state.scroller.set_visible(false);
    } else {
        state
            .summary
            .set_text(&metis_i18n::tr("%1 update(s) available.").replace("%1", &count.to_string()));
        state.later_btn.set_label(&metis_i18n::tr("Later"));
        state.scroller.set_visible(true);
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
            state.status.set_visible(true);
            state.status.set_text(err);
        }
    } else if !state.applying.get() {
        state.status.set_visible(false);
    }
}

fn append_section(list: &gtk::ListBox, title: &str, items: &[metis_remote::UpdateItem]) {
    if items.is_empty() {
        return;
    }
    let header = gtk::Label::new(Some(title));
    header.set_halign(gtk::Align::Start);
    header.add_css_class("heading");
    header.add_css_class("metis-updater-section");
    let header_row = gtk::ListBoxRow::new();
    header_row.set_selectable(false);
    header_row.set_activatable(false);
    header_row.add_css_class("metis-updater-section-row");
    header_row.set_child(Some(&header));
    list.append(&header_row);

    for item in items {
        let row_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row_box.add_css_class("metis-updater-item");
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
