//! Software Updates preferences — check interval, sources, Check now.

use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{UpdatesConfig, load_updates_config, save_updates_config};
use metis_i18n::tr;
use metis_remote::{UpdateSnapshot, updates_check_from_config};

use crate::bg;
use crate::runtime;
use crate::ui;

struct Sections {
    enabled: gtk::Switch,
    interval: gtk::SpinButton,
    notify: gtk::Switch,
    auto_security: gtk::Switch,
    pk: gtk::Switch,
    flatpak: gtk::Switch,
    fwupd: gtk::Switch,
    last_check: gtk::Label,
    last_error: gtk::Label,
    status: gtk::Label,
    checking: Rc<std::cell::Cell<bool>>,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("updates");

    let intro = gtk::Label::new(Some(&tr(
        "Metis checks for system packages (PackageKit), Flatpak apps, and firmware \
         (fwupd). Installing may ask for your password. Flatpak and firmware use \
         their own authentication when needed.",
    )));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("metis-settings-hint");
    intro.set_margin_bottom(16);
    content.append(&intro);

    let (auto_card, auto_body) = ui::section(&tr("Automatic checks"));
    let (enabled_row, enabled) = ui::switch_row(&tr("Check for updates automatically"));
    auto_body.append(&enabled_row);
    let interval = gtk::SpinButton::with_range(1.0, 168.0, 1.0);
    interval.set_digits(0);
    auto_body.append(&ui::row(&tr("Check every (hours)"), &interval));
    let (notify_row, notify) = ui::switch_row(&tr("Notify when updates are available"));
    auto_body.append(&notify_row);
    let (sec_row, auto_security) =
        ui::switch_row(&tr("Auto-install security updates (PackageKit)"));
    auto_body.append(&sec_row);
    let sec_hint = gtk::Label::new(Some(&tr(
        "When enabled, PackageKit security updates may be applied without opening \
         the updater. Flatpak and firmware are never installed silently.",
    )));
    sec_hint.set_xalign(0.0);
    sec_hint.set_wrap(true);
    sec_hint.add_css_class("metis-settings-hint");
    auto_body.append(&sec_hint);
    content.append(&auto_card);

    let (src_card, src_body) = ui::section(&tr("Sources"));
    let (pk_row, pk) = ui::switch_row(&tr("System packages (PackageKit / distro)"));
    src_body.append(&pk_row);
    let (fp_row, flatpak) = ui::switch_row(&tr("Flatpak apps"));
    src_body.append(&fp_row);
    let (fw_row, fwupd) = ui::switch_row(&tr("Firmware (fwupd)"));
    src_body.append(&fw_row);
    content.append(&src_card);

    let (status_card, status_body) = ui::section(&tr("Status"));
    let last_check = gtk::Label::new(None);
    last_check.set_xalign(0.0);
    last_check.add_css_class("metis-settings-value");
    status_body.append(&readout(&tr("Last check"), &last_check));
    let last_error = gtk::Label::new(None);
    last_error.set_xalign(0.0);
    last_error.set_wrap(true);
    last_error.add_css_class("metis-settings-error");
    status_body.append(&readout(&tr("Last error"), &last_error));
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("metis-settings-hint");
    status_body.append(&status);
    content.append(&status_card);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::Start);
    actions.set_margin_top(8);
    let check_btn = gtk::Button::with_label(&tr("Check now"));
    check_btn.add_css_class("suggested-action");
    let open_btn = gtk::Button::with_label(&tr("Open updater"));
    actions.append(&check_btn);
    actions.append(&open_btn);
    content.append(&actions);

    let sections = Rc::new(Sections {
        enabled,
        interval,
        notify,
        auto_security,
        pk,
        flatpak,
        fwupd,
        last_check,
        last_error,
        status,
        checking: Rc::new(std::cell::Cell::new(false)),
    });

    apply_config(&sections, &load_updates_config());

    let persist = {
        let sections = sections.clone();
        move || {
            let cfg = read_config(&sections);
            if let Err(err) = save_updates_config(&cfg) {
                tracing::warn!(%err, "failed to save updates.json");
            }
        }
    };

    sections.enabled.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.interval.connect_value_changed({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.notify.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.auto_security.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.pk.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.flatpak.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });
    sections.fwupd.connect_active_notify({
        let persist = persist.clone();
        move |_| persist()
    });

    open_btn.connect_clicked(|_| {
        runtime::send("show-updater");
    });

    check_btn.connect_clicked({
        let sections = sections.clone();
        move |btn| {
            if sections.checking.get() {
                return;
            }
            sections.checking.set(true);
            btn.set_sensitive(false);
            sections.status.set_text(&tr("Checking for updates…"));
            let cfg = read_config(&sections);
            let sections_done = sections.clone();
            let btn = btn.clone();
            bg::run_bg(
                move || updates_check_from_config(&cfg),
                move |snap: UpdateSnapshot| {
                    sections_done.checking.set(false);
                    btn.set_sensitive(true);
                    let mut saved = load_updates_config();
                    saved.last_check = Some(chrono::Local::now());
                    saved.last_error = snap.error.clone();
                    let _ = save_updates_config(&saved);
                    apply_status(&sections_done, &saved, Some(&snap));
                },
            );
        }
    });

    scroller.upcast()
}

fn readout(label: &str, value: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let lab = gtk::Label::new(Some(label));
    lab.set_xalign(0.0);
    lab.set_hexpand(true);
    lab.add_css_class("metis-settings-label");
    row.append(&lab);
    row.append(value);
    row.upcast()
}

fn apply_config(sections: &Sections, cfg: &UpdatesConfig) {
    sections.enabled.set_active(cfg.enabled);
    sections
        .interval
        .set_value(f64::from(cfg.check_interval_hours));
    sections.notify.set_active(cfg.notify_on_available);
    sections.auto_security.set_active(cfg.auto_install_security);
    sections.pk.set_active(cfg.sources.packagekit);
    sections.flatpak.set_active(cfg.sources.flatpak);
    sections.fwupd.set_active(cfg.sources.fwupd);
    apply_status(sections, cfg, None);
}

fn apply_status(sections: &Sections, cfg: &UpdatesConfig, snap: Option<&UpdateSnapshot>) {
    if let Some(t) = cfg.last_check {
        sections
            .last_check
            .set_text(&t.format("%Y-%m-%d %H:%M").to_string());
    } else {
        sections.last_check.set_text(&tr("Never"));
    }
    match &cfg.last_error {
        Some(err) if !err.is_empty() => {
            sections.last_error.set_text(err);
            sections.last_error.set_visible(true);
        }
        _ => {
            sections.last_error.set_text("");
            sections.last_error.set_visible(false);
        }
    }
    if let Some(snap) = snap {
        let n = snap.total_count();
        if n == 0 {
            sections.status.set_text(&tr("Your system is up to date."));
        } else {
            sections.status.set_text(
                &tr("%1 update(s) available — open the updater to install.")
                    .replace("%1", &n.to_string()),
            );
        }
    }
}

fn read_config(sections: &Sections) -> UpdatesConfig {
    let mut cfg = load_updates_config();
    cfg.enabled = sections.enabled.is_active();
    cfg.check_interval_hours = sections.interval.value() as u32;
    cfg.notify_on_available = sections.notify.is_active();
    cfg.auto_install_security = sections.auto_security.is_active();
    cfg.sources.packagekit = sections.pk.is_active();
    cfg.sources.flatpak = sections.flatpak.is_active();
    cfg.sources.fwupd = sections.fwupd.is_active();
    cfg
}
