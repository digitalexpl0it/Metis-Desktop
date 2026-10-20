//! Session startup applications — master toggle + ordered desktop-id list.
//! Persists to `~/.config/metis/startup.json` (compositor reads at session start).
//! Also hosts the crash-restore preference (`session.json`).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use metis_config::{StartupConfig, StartupEntry};

use crate::apps::{self, AppEntry};
use crate::ui;
use metis_i18n::tr;

pub fn build() -> gtk::Widget {
    apps::watch_app_index();

    let (scroller, content) = ui::page_for("startup");
    let cfg = Rc::new(RefCell::new(metis_config::load_startup_config()));
    let session_cfg = Rc::new(RefCell::new(metis_config::load_session_config()));

    let hint = gtk::Label::new(Some(&tr(
        "Applications listed here launch once after you sign in. Nothing starts \
         until you add apps. Custom command lines are not supported.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    // ---- Crash recovery ---------------------------------------------------
    let (restore_card, restore_body) =
        ui::section_with_icon(&tr("Crash recovery"), "view-refresh-symbolic");
    let restore = gtk::Switch::new();
    restore.set_active(session_cfg.borrow().restore_on_crash);
    restore.set_halign(gtk::Align::End);
    restore_body.append(&ui::row(&tr("Restore windows after crash"), &restore));
    let restore_hint = gtk::Label::new(Some(&tr(
        "If the compositor exits uncleanly, Metis keeps your login session and \
         relaunches open apps best-effort. Clean logout always returns to the \
         greeter. XWayland and exclusive fullscreen apps may not restore perfectly.",
    )));
    restore_hint.set_xalign(0.0);
    restore_hint.set_wrap(true);
    restore_hint.add_css_class("metis-settings-hint");
    restore_body.append(&restore_hint);
    content.append(&restore_card);
    {
        let session_cfg = session_cfg.clone();
        restore.connect_active_notify(move |s| {
            session_cfg.borrow_mut().restore_on_crash = s.is_active();
            persist_session(&session_cfg.borrow());
        });
    }

    // ---- Master switch ----------------------------------------------------
    let (master_card, master_body) = ui::section_with_icon(&tr("Startup"), "system-run-symbolic");
    let master = gtk::Switch::new();
    master.set_active(cfg.borrow().enabled);
    master.set_halign(gtk::Align::End);
    master_body.append(&ui::row(&tr("Run startup applications"), &master));
    content.append(&master_card);
    {
        let cfg = cfg.clone();
        master.connect_active_notify(move |s| {
            cfg.borrow_mut().enabled = s.is_active();
            persist(&cfg.borrow());
        });
    }

    // ---- Application list -------------------------------------------------
    let (list_card, list_body) =
        ui::section_with_icon(&tr("Applications"), "application-x-executable-symbolic");

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("metis-settings-list");
    list_body.append(&list);

    let add_btn = gtk::MenuButton::builder()
        .label(tr("Add application…"))
        .halign(gtk::Align::Start)
        .build();
    let popover = build_app_picker(cfg.clone(), list.clone());
    add_btn.set_popover(Some(&popover));
    list_body.append(&add_btn);
    content.append(&list_card);

    rebuild_list(&list, &cfg);

    {
        let cfg = cfg.clone();
        let list = list.clone();
        apps::register_refresh(Rc::new(move || {
            rebuild_list(&list, &cfg);
        }));
    }

    scroller.upcast()
}

fn rebuild_list(list: &gtk::ListBox, cfg: &Rc<RefCell<StartupConfig>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let entries = cfg.borrow().entries.clone();
    if entries.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No startup applications.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        list.append(&empty);
        return;
    }

    let apps = apps::list_apps();
    for (idx, entry) in entries.iter().enumerate() {
        let meta = apps.iter().find(|a| a.id == entry.id);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        if idx % 2 == 1 {
            row.add_css_class("metis-widget-list-row-alt");
        } else {
            row.add_css_class("metis-widget-list-row");
        }

        let image = gtk::Image::new();
        image.set_pixel_size(28);
        if let Some(app) = meta {
            if let Some(icon) = &app.icon {
                image.set_from_gicon(icon);
            } else {
                image.set_icon_name(Some("application-x-executable-symbolic"));
            }
        } else {
            image.set_icon_name(Some("dialog-warning-symbolic"));
        }
        row.append(&image);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        let name = gtk::Label::new(Some(
            meta.map(|a| a.name.as_str()).unwrap_or(entry.id.as_str()),
        ));
        name.set_xalign(0.0);
        name.add_css_class("heading");
        labels.append(&name);
        if meta.is_none() {
            let missing = gtk::Label::new(Some(&tr("App not found — will be skipped at login")));
            missing.set_xalign(0.0);
            missing.add_css_class("metis-settings-hint");
            labels.append(&missing);
        } else {
            let id_lbl = gtk::Label::new(Some(&entry.id));
            id_lbl.set_xalign(0.0);
            id_lbl.add_css_class("metis-settings-hint");
            labels.append(&id_lbl);
        }
        row.append(&labels);

        let enable = gtk::Switch::new();
        enable.set_active(entry.enabled);
        enable.set_valign(gtk::Align::Center);
        {
            let cfg = cfg.clone();
            let list = list.clone();
            let id = entry.id.clone();
            enable.connect_state_set(move |_, state| {
                if let Some(e) = cfg.borrow_mut().entries.iter_mut().find(|e| e.id == id) {
                    e.enabled = state;
                }
                persist(&cfg.borrow());
                rebuild_list(&list, &cfg);
                glib::Propagation::Proceed
            });
        }
        row.append(&enable);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_valign(gtk::Align::Center);
        remove.set_tooltip_text(Some(&tr("Remove")));
        {
            let cfg = cfg.clone();
            let list = list.clone();
            let id = entry.id.clone();
            remove.connect_clicked(move |_| {
                cfg.borrow_mut().entries.retain(|e| e.id != id);
                persist(&cfg.borrow());
                rebuild_list(&list, &cfg);
            });
        }
        row.append(&remove);

        list.append(&row);
    }
}

fn build_app_picker(cfg: Rc<RefCell<StartupConfig>>, list: gtk::ListBox) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(true);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    outer.set_margin_top(10);
    outer.set_margin_bottom(10);
    outer.set_margin_start(10);
    outer.set_margin_end(10);
    outer.set_size_request(320, 360);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search applications…")));
    search.set_hexpand(true);
    outer.append(&search);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    scrolled.set_min_content_height(280);

    let picker_list = gtk::ListBox::new();
    picker_list.set_selection_mode(gtk::SelectionMode::None);
    picker_list.add_css_class("metis-settings-list");
    scrolled.set_child(Some(&picker_list));
    outer.append(&scrolled);

    popover.set_child(Some(&outer));

    let refill = {
        let picker_list = picker_list.clone();
        let cfg = cfg.clone();
        let list = list.clone();
        let popover = popover.clone();
        Rc::new(move |query: String| {
            refill_picker(&picker_list, &cfg, &list, &popover, &query);
        }) as Rc<dyn Fn(String)>
    };

    {
        let refill = refill.clone();
        search.connect_search_changed(move |s| {
            refill(s.text().to_string());
        });
    }

    {
        let refill = refill.clone();
        popover.connect_show(move |_| {
            refill(String::new());
        });
    }

    popover
}

fn refill_picker(
    picker_list: &gtk::ListBox,
    cfg: &Rc<RefCell<StartupConfig>>,
    list: &gtk::ListBox,
    popover: &gtk::Popover,
    query: &str,
) {
    while let Some(child) = picker_list.first_child() {
        picker_list.remove(&child);
    }

    let q = query.trim().to_ascii_lowercase();
    let already: std::collections::HashSet<String> =
        cfg.borrow().entries.iter().map(|e| e.id.clone()).collect();

    let mut apps: Vec<AppEntry> = apps::list_apps()
        .into_iter()
        .filter(|a| !already.contains(&a.id))
        .filter(|a| {
            q.is_empty()
                || a.name.to_ascii_lowercase().contains(&q)
                || a.id.to_ascii_lowercase().contains(&q)
        })
        .collect();
    apps.truncate(80);

    if apps.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No matching applications.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        picker_list.append(&empty);
        return;
    }

    for app in apps {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(4);
        row.set_margin_bottom(4);

        let image = gtk::Image::new();
        image.set_pixel_size(24);
        if let Some(icon) = &app.icon {
            image.set_from_gicon(icon);
        } else {
            image.set_icon_name(Some("application-x-executable-symbolic"));
        }
        row.append(&image);

        let name = gtk::Label::new(Some(&app.name));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        row.append(&name);

        let btn = gtk::Button::with_label(&tr("Add"));
        btn.add_css_class("flat");
        {
            let cfg = cfg.clone();
            let list = list.clone();
            let popover = popover.clone();
            let id = app.id.clone();
            btn.connect_clicked(move |_| {
                {
                    let mut c = cfg.borrow_mut();
                    if c.entries.iter().any(|e| e.id == id) {
                        return;
                    }
                    c.entries.push(StartupEntry {
                        id: id.clone(),
                        enabled: true,
                        delay_seconds: 0,
                    });
                }
                persist(&cfg.borrow());
                rebuild_list(&list, &cfg);
                popover.popdown();
            });
        }
        row.append(&btn);
        picker_list.append(&row);
    }
}

fn persist(cfg: &StartupConfig) {
    if let Err(err) = metis_config::save_startup_config(cfg) {
        tracing::warn!(%err, "failed to save startup.json");
    }
}

fn persist_session(cfg: &metis_config::SessionConfig) {
    if let Err(err) = metis_config::save_session_config(cfg) {
        tracing::warn!(%err, "failed to save session.json");
    }
}
