//! Remote access — desktop session sharing (RDP via gnome-remote-desktop).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::dialog;
use crate::remote::{self, RemoteSnapshot};
use crate::ui;
use metis_i18n::tr;

struct Sections {
    enable_sw: gtk::Switch,
    lan_sw: gtk::Switch,
    status_label: gtk::Label,
    status_spinner: gtk::Spinner,
    address_label: gtk::Label,
    port_label: gtk::Label,
    username_label: gtk::Label,
    hint_label: gtk::Label,
    error_label: gtk::Label,
    firewall_status: gtk::Label,
    retry_fw_btn: gtk::Button,
    action_error: Rc<RefCell<Option<String>>>,
    password_banner: gtk::Box,
    change_pw_btn: gtk::Button,
    install_banner: gtk::Box,
    toggling: Rc<Cell<bool>>,
    lan_toggling: Rc<Cell<bool>>,
    /// True while enable/disable CLI is in flight — ignore status poll for the switch.
    enable_pending: Rc<Cell<bool>>,
    lan_pending: Rc<Cell<bool>>,
    /// True while a background firewall apply is expected.
    firewall_pending: Rc<Cell<bool>>,
    /// Spinner after enable returns until RDP is listening (switch stays usable).
    warming_up: Rc<Cell<bool>>,
    /// Last known / intended sharing on-state (switch + config), for LAN warn.
    share_wanted: Rc<Cell<bool>>,
}

pub fn build(parent: &gtk::Window) -> gtk::Widget {
    let (scroller, content) = ui::page_for("remote");

    let intro = gtk::Label::new(Some(&tr(
        "Session sharing lets another device view and control the Metis session \
         you are already logged into. Remote login to start a separate session \
         will be a different option when it is available.",
    )));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("metis-settings-hint");
    intro.set_margin_bottom(16);
    content.append(&intro);

    let install_banner = gtk::Box::new(gtk::Orientation::Vertical, 6);
    install_banner.add_css_class("metis-settings-banner");
    install_banner.set_margin_bottom(12);
    install_banner.set_visible(false);
    let install_text = gtk::Label::new(Some(&tr(
        "Install gnome-remote-desktop to enable desktop session sharing:\n\
         sudo apt install gnome-remote-desktop",
    )));
    install_text.set_xalign(0.0);
    install_text.set_wrap(true);
    install_text.add_css_class("metis-settings-hint");
    install_banner.append(&install_text);
    content.append(&install_banner);

    let password_banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    password_banner.add_css_class("metis-settings-banner");
    password_banner.set_margin_bottom(12);
    password_banner.set_visible(false);
    let pw_text = gtk::Label::new(Some(&tr(
        "Set a password before enabling desktop session sharing.",
    )));
    pw_text.set_xalign(0.0);
    pw_text.set_hexpand(true);
    pw_text.set_wrap(true);
    password_banner.append(&pw_text);
    let set_pw_btn = gtk::Button::with_label(&tr("Set password…"));
    set_pw_btn.add_css_class("suggested-action");
    password_banner.append(&set_pw_btn);
    content.append(&password_banner);

    let (share_card, share_body) = ui::section(&tr("Desktop session sharing"));

    let error_label = gtk::Label::new(None);
    error_label.set_xalign(0.0);
    error_label.set_wrap(true);
    error_label.add_css_class("metis-settings-error");
    error_label.set_margin_bottom(12);
    error_label.set_visible(false);
    share_body.append(&error_label);

    let (enable_row, enable_sw) = ui::switch_row(&tr("Allow desktop session sharing"));
    share_body.append(&enable_row);
    content.append(&share_card);

    let (status_card, status_body) = ui::section(&tr("Connection"));
    let status_label = gtk::Label::new(Some(&tr("Checking…")));
    status_label.set_xalign(0.0);
    status_label.add_css_class("metis-settings-value");
    status_label.set_hexpand(true);
    let status_spinner = gtk::Spinner::new();
    status_spinner.set_visible(false);
    let status_value = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_value.append(&status_spinner);
    status_value.append(&status_label);
    status_body.append(&readout_row_widget(&tr("Status"), &status_value));

    let address_label = gtk::Label::new(None);
    address_label.set_xalign(0.0);
    address_label.set_selectable(true);
    status_body.append(&readout_row(&tr("Address"), &address_label));

    let port_label = gtk::Label::new(None);
    port_label.set_xalign(0.0);
    status_body.append(&readout_row(&tr("Port"), &port_label));

    let username_label = gtk::Label::new(None);
    username_label.set_xalign(0.0);
    status_body.append(&readout_row(&tr("Username"), &username_label));

    let change_pw_btn = gtk::Button::with_label(&tr("Change password…"));
    change_pw_btn.set_halign(gtk::Align::Start);
    change_pw_btn.set_visible(false);

    let copy_btn = gtk::Button::with_label(&tr("Copy connection address"));
    copy_btn.set_halign(gtk::Align::Start);

    let viewer_btn = gtk::Button::with_label(&tr("Connect with Metis Viewer…"));
    viewer_btn.set_halign(gtk::Align::Start);

    let clients_hint = gtk::Label::new(Some(&tr(
        "Connect with Metis Viewer, Microsoft Remote Desktop, Remmina, or FreeRDP. \
         Use the username and password you set above — empty credentials will not work.",
    )));
    clients_hint.set_xalign(0.0);
    clients_hint.set_wrap(true);
    clients_hint.add_css_class("metis-settings-hint");

    let actions = gtk::Box::new(gtk::Orientation::Vertical, 8);
    actions.add_css_class("metis-settings-actions");
    actions.append(&change_pw_btn);
    actions.append(&copy_btn);
    actions.append(&viewer_btn);
    actions.append(&clients_hint);
    status_body.append(&actions);
    content.append(&status_card);

    let (sec_card, sec_body) = ui::section(&tr("Security"));
    let (lan_row, lan_sw) = ui::switch_row(&tr("LAN only (firewall)"));
    lan_row.set_margin_top(2);
    sec_body.append(&lan_row);

    let firewall_status = gtk::Label::new(None);
    firewall_status.set_xalign(0.0);
    firewall_status.set_wrap(true);
    firewall_status.add_css_class("metis-settings-hint");
    firewall_status.set_margin_start(16);
    firewall_status.set_margin_end(16);
    firewall_status.set_margin_top(4);
    firewall_status.set_margin_bottom(4);
    sec_body.append(&firewall_status);

    let retry_fw_btn = gtk::Button::with_label(&tr("Retry firewall apply"));
    retry_fw_btn.set_halign(gtk::Align::Start);
    retry_fw_btn.set_margin_start(16);
    retry_fw_btn.set_margin_end(16);
    retry_fw_btn.set_margin_bottom(8);
    retry_fw_btn.set_visible(false);
    sec_body.append(&retry_fw_btn);

    let hint_label = gtk::Label::new(Some(&tr(
        "When LAN only is on and sharing is enabled, Metis applies firewall rules \
         automatically (nftables preferred; ufw only if active). A PolicyKit password \
         dialog may appear. Use a strong password. While locked (Super+L), RDP listen \
         pauses. Clipboard sync includes text and images.",
    )));
    hint_label.set_xalign(0.0);
    hint_label.set_wrap(true);
    hint_label.add_css_class("metis-settings-hint");
    hint_label.set_margin_top(4);
    sec_body.append(&hint_label);
    content.append(&sec_card);

    // RustDesk third-party preset (detect / open / notes — not metis-remote).
    let (rd_card, rd_body) = ui::section(&tr("RustDesk (third-party)"));
    let rd_status = gtk::Label::new(None);
    rd_status.set_xalign(0.0);
    rd_status.add_css_class("metis-settings-value");
    rd_body.append(&readout_row(&tr("Status"), &rd_status));

    let rd_hint = gtk::Label::new(Some(&tr(
        "ID/relay remote access outside LAN RDP. Prefer PipeWire / portal capture \
         on Metis (Smithay) — proprietary or wlroots-only screencopy may show a \
         black screen. Default ports 21115–21119 (TCP; 21116 also UDP). Do not \
         expose those ports to the public internet without understanding relay trust.",
    )));
    rd_hint.set_xalign(0.0);
    rd_hint.set_wrap(true);
    rd_hint.add_css_class("metis-settings-hint");
    rd_hint.set_margin_top(4);
    rd_body.append(&rd_hint);

    let rd_open_btn = gtk::Button::with_label(&tr("Open RustDesk"));
    rd_open_btn.set_halign(gtk::Align::Start);
    let rd_enable_btn = gtk::Button::with_label(&tr("Use as Metis backend"));
    rd_enable_btn.set_halign(gtk::Align::Start);
    rd_enable_btn.set_tooltip_text(Some(&tr(
        "Runs `metis-remote rustdesk enable` — starts RustDesk and optional LAN firewall. \
         GNOME Remote Desktop remains the default session-sharing host.",
    )));
    let rd_disable_btn = gtk::Button::with_label(&tr("Stop Metis backend"));
    rd_disable_btn.set_halign(gtk::Align::Start);
    let rd_copy_btn = gtk::Button::with_label(&tr("Copy install instructions"));
    rd_copy_btn.set_halign(gtk::Align::Start);
    let rd_actions = gtk::Box::new(gtk::Orientation::Vertical, 8);
    rd_actions.add_css_class("metis-settings-actions");
    rd_actions.append(&rd_open_btn);
    rd_actions.append(&rd_enable_btn);
    rd_actions.append(&rd_disable_btn);
    rd_actions.append(&rd_copy_btn);
    rd_body.append(&rd_actions);
    content.append(&rd_card);

    let (login_card, login_body) = ui::section(&tr("Remote login"));
    let login_hint = gtk::Label::new(Some(&tr(
        "Sign in remotely to start a new desktop session (for example xrdp) — planned \
         for a later milestone. This page only covers sharing the session you are \
         already in.",
    )));
    login_hint.set_xalign(0.0);
    login_hint.set_wrap(true);
    login_hint.add_css_class("metis-settings-hint");
    login_body.append(&login_hint);
    content.append(&login_card);

    let password_ui_open = Rc::new(Cell::new(false));

    let toggling = Rc::new(Cell::new(false));
    let lan_toggling = Rc::new(Cell::new(false));
    let enable_pending = Rc::new(Cell::new(false));
    let lan_pending = Rc::new(Cell::new(false));
    let warming_up = Rc::new(Cell::new(false));
    let share_wanted = Rc::new(Cell::new(false));
    let firewall_pending = Rc::new(Cell::new(false));
    let action_error = Rc::new(RefCell::new(None::<String>));
    let sections = Rc::new(Sections {
        enable_sw,
        lan_sw,
        status_label,
        status_spinner,
        address_label,
        port_label,
        username_label,
        hint_label,
        error_label,
        firewall_status: firewall_status.clone(),
        retry_fw_btn: retry_fw_btn.clone(),
        action_error: action_error.clone(),
        password_banner,
        change_pw_btn: change_pw_btn.clone(),
        install_banner,
        toggling: toggling.clone(),
        lan_toggling: lan_toggling.clone(),
        enable_pending: enable_pending.clone(),
        lan_pending: lan_pending.clone(),
        firewall_pending: firewall_pending.clone(),
        warming_up: warming_up.clone(),
        share_wanted: share_wanted.clone(),
    });

    let (tx, rx) = mpsc::channel::<RemoteSnapshot>();
    let (action_tx, action_rx) = mpsc::channel::<(bool, Result<(), String>)>();
    let (lan_tx, lan_rx) = mpsc::channel::<(bool, Result<(), String>)>();
    let (cred_tx, cred_rx) = mpsc::channel::<Result<(), String>>();
    let refresh = {
        let tx = tx.clone();
        Rc::new(move || {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(remote::load_snapshot());
            });
        })
    };

    {
        let sections_poll = sections.clone();
        let refresh_after_toggle = refresh.clone();
        let password_ui_open_poll = password_ui_open.clone();
        glib::timeout_add_local(Duration::from_millis(200), move || {
            while let Ok(result) = cred_rx.try_recv() {
                match result {
                    Ok(()) => refresh_after_toggle(),
                    Err(err) => {
                        *sections_poll.action_error.borrow_mut() = Some(err.clone());
                        sections_poll.error_label.set_text(&err);
                        sections_poll.error_label.set_visible(true);
                    }
                }
            }
            while let Ok((active, result)) = action_rx.try_recv() {
                sections_poll.enable_pending.set(false);
                if let Err(err) = result {
                    sections_poll.warming_up.set(false);
                    set_busy(&sections_poll, false);
                    sections_poll.share_wanted.set(!active);
                    sections_poll.toggling.set(true);
                    sections_poll.enable_sw.set_active(!active);
                    sections_poll.toggling.set(false);
                    *sections_poll.action_error.borrow_mut() = Some(err.clone());
                    sections_poll.error_label.set_text(&err);
                    sections_poll.error_label.set_visible(true);
                    remote::notify_sharing(
                        &tr("Desktop sharing"),
                        &if active {
                            tr("Could not enable desktop sharing.")
                        } else {
                            tr("Could not disable desktop sharing.")
                        },
                    );
                } else {
                    sections_poll.share_wanted.set(active);
                    *sections_poll.action_error.borrow_mut() = None;
                    sections_poll.error_label.set_visible(false);
                    if active {
                        sections_poll.warming_up.set(true);
                        set_busy(&sections_poll, true);
                        sections_poll
                            .status_label
                            .set_text(&tr("Starting — waiting for RDP…"));
                        sections_poll.enable_sw.set_sensitive(true);
                        sections_poll.lan_sw.set_sensitive(true);
                        remote::notify_sharing(
                            &tr("Desktop sharing on"),
                            &tr("Session sharing is starting. Clients can connect when status shows ready."),
                        );
                        let snap = remote::load_snapshot();
                        if snap.lan_only && !snap.firewall_applied {
                            sections_poll.firewall_pending.set(true);
                        }
                    } else {
                        sections_poll.firewall_pending.set(false);
                        sections_poll.warming_up.set(false);
                        set_busy(&sections_poll, false);
                        sections_poll.status_label.set_text(&tr("Stopped"));
                        sections_poll.address_label.set_text(&tr("—"));
                        sections_poll.port_label.set_text(&tr("—"));
                        sections_poll.enable_sw.set_sensitive(true);
                        sections_poll.lan_sw.set_sensitive(true);
                        remote::notify_sharing(
                            &tr("Desktop sharing off"),
                            &tr("Remote connections to this session are disabled."),
                        );
                    }
                }
                refresh_after_toggle();
            }
            while let Ok((lan_only, result)) = lan_rx.try_recv() {
                sections_poll.lan_pending.set(false);
                if let Err(err) = result {
                    sections_poll.lan_toggling.set(true);
                    sections_poll.lan_sw.set_active(!lan_only);
                    sections_poll.lan_toggling.set(false);
                    sections_poll.firewall_pending.set(false);
                    *sections_poll.action_error.borrow_mut() = Some(err.clone());
                    sections_poll.error_label.set_text(&err);
                    sections_poll.error_label.set_visible(true);
                } else {
                    *sections_poll.action_error.borrow_mut() = None;
                    sections_poll.error_label.set_visible(false);
                    let sharing_on = sections_poll.share_wanted.get()
                        || sections_poll.enable_sw.is_active()
                        || sections_poll.warming_up.get();
                    if lan_only && sharing_on {
                        sections_poll.firewall_pending.set(true);
                    } else if !lan_only {
                        sections_poll.firewall_pending.set(false);
                    }
                }
                refresh_after_toggle();
            }
            if !password_ui_open_poll.get() {
                if let Ok(snap) = rx.try_recv() {
                    render(&sections_poll, &snap);
                }
            } else {
                while rx.try_recv().is_ok() {}
            }
            glib::ControlFlow::Continue
        });
        let refresh_periodic = refresh.clone();
        let password_ui_open_periodic = password_ui_open.clone();
        let enable_pending_poll = enable_pending.clone();
        let warming_up_poll = warming_up.clone();
        let firewall_pending_poll = firewall_pending.clone();
        // While enable/disable is running (or waiting for RDP listen / firewall),
        // poll every second so status updates promptly; otherwise every 5s.
        glib::timeout_add_local(Duration::from_secs(1), {
            let mut tick = 0u32;
            move || {
                tick = tick.wrapping_add(1);
                if password_ui_open_periodic.get() {
                    return glib::ControlFlow::Continue;
                }
                if enable_pending_poll.get()
                    || warming_up_poll.get()
                    || firewall_pending_poll.get()
                    || tick.is_multiple_of(5)
                {
                    refresh_periodic();
                }
                glib::ControlFlow::Continue
            }
        });
    }

    {
        let sections_sw = sections.clone();
        let action_tx = action_tx.clone();
        let toggling = sections.toggling.clone();
        let enable_pending_gate = sections.enable_pending.clone();
        let enable_pending = sections.enable_pending.clone();
        ui::defer_switch_active_notify_when(
            &sections.enable_sw,
            move || !toggling.get() && !enable_pending_gate.get(),
            move |active| {
                enable_pending.set(true);
                sections_sw.warming_up.set(false);
                sections_sw.share_wanted.set(active);
                *sections_sw.action_error.borrow_mut() = None;
                sections_sw.error_label.set_visible(false);
                set_busy(&sections_sw, true);
                if active {
                    sections_sw
                        .status_label
                        .set_text(&tr("Starting desktop sharing…"));
                } else {
                    sections_sw
                        .status_label
                        .set_text(&tr("Stopping desktop sharing…"));
                }
                let action_tx = action_tx.clone();
                std::thread::spawn(move || {
                    let result = if active {
                        remote::enable_sharing()
                    } else {
                        remote::disable_sharing()
                    };
                    let _ = action_tx.send((active, result));
                });
            },
        );
    }

    {
        let sections_lan = sections.clone();
        let lan_tx = lan_tx.clone();
        let parent = parent.clone();
        let lan_toggling = sections.lan_toggling.clone();
        let lan_pending_gate = sections.lan_pending.clone();
        let lan_pending = sections.lan_pending.clone();
        ui::defer_switch_active_notify_when(
            &sections.lan_sw,
            move || !lan_toggling.get() && !lan_pending_gate.get(),
            move |lan_only| {
                // Warn whenever turning LAN-only off while sharing is intended on.
                // Use share_wanted (not only the switch) so a mid-disable UI still prompts.
                let sharing_on = sections_lan.share_wanted.get()
                    || sections_lan.enable_sw.is_active()
                    || sections_lan.enable_pending.get()
                    || sections_lan.warming_up.get();
                if !lan_only && sharing_on {
                    let sections_lan = sections_lan.clone();
                    let lan_tx = lan_tx.clone();
                    let lan_pending = lan_pending.clone();
                    confirm_disable_lan_only(&parent, move |confirmed| {
                        if !confirmed {
                            sections_lan.lan_toggling.set(true);
                            sections_lan.lan_sw.set_active(true);
                            sections_lan.lan_toggling.set(false);
                            return;
                        }
                        lan_pending.set(true);
                        *sections_lan.action_error.borrow_mut() = None;
                        let lan_tx = lan_tx.clone();
                        std::thread::spawn(move || {
                            let result = remote::set_lan_only(false);
                            let _ = lan_tx.send((false, result));
                        });
                    });
                    return;
                }
                lan_pending.set(true);
                *sections_lan.action_error.borrow_mut() = None;
                let lan_tx = lan_tx.clone();
                std::thread::spawn(move || {
                    let result = remote::set_lan_only(lan_only);
                    let _ = lan_tx.send((lan_only, result));
                });
            },
        );
    }

    let open_password = {
        let cred_tx = cred_tx.clone();
        let password_ui_open = password_ui_open.clone();
        Rc::new(move || {
            show_password_sheet(cred_tx.clone(), password_ui_open.clone());
        })
    };

    for btn in [&set_pw_btn, &change_pw_btn] {
        let open_password = open_password.clone();
        btn.connect_clicked(move |_| open_password());
    }

    {
        let sections_fw = sections.clone();
        let refresh_fw = refresh.clone();
        let (fw_tx, fw_rx) = mpsc::channel::<Result<(), String>>();
        sections.retry_fw_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            btn.set_label(&tr("Applying…"));
            sections_fw.firewall_pending.set(true);
            sections_fw.firewall_status.set_text(&tr(
                "Applying firewall rules… A password dialog may appear.",
            ));
            let fw_tx = fw_tx.clone();
            std::thread::spawn(move || {
                let _ = fw_tx.send(remote::apply_firewall());
            });
        });
        let sections_poll = sections.clone();
        let retry_btn = sections.retry_fw_btn.clone();
        glib::timeout_add_local(Duration::from_millis(200), move || {
            if let Ok(result) = fw_rx.try_recv() {
                retry_btn.set_sensitive(true);
                retry_btn.set_label(&tr("Retry firewall apply"));
                sections_poll.firewall_pending.set(false);
                match result {
                    Ok(()) => {
                        *sections_poll.action_error.borrow_mut() = None;
                        remote::notify_sharing(
                            &tr("LAN firewall applied"),
                            &tr("RDP port 3389 is restricted to private / link-local addresses."),
                        );
                    }
                    Err(err) => {
                        *sections_poll.action_error.borrow_mut() = Some(err.clone());
                        sections_poll.firewall_status.set_text(&err);
                        remote::notify_sharing(&tr("LAN firewall failed"), &err);
                    }
                }
                refresh_fw();
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let sections_copy = sections.clone();
        copy_btn.connect_clicked(move |_| {
            let text = remote::connection_hint(&remote::load_snapshot());
            let display = gtk::gdk::Display::default();
            if let Some(display) = display {
                display.clipboard().set_text(&text);
            }
            sections_copy
                .hint_label
                .set_text(&format!("Copied: {text}"));
        });
    }

    {
        let sections_viewer = sections.clone();
        viewer_btn.connect_clicked(move |_| {
            let snap = remote::load_snapshot();
            let warnings = remote::viewer_launch_warnings(&snap);
            let hint = remote::connection_hint(&snap);
            let (host, port) = remote::parse_connection_hint(&hint);
            let user = snap.username.as_deref();
            match remote::open_viewer(Some(&host), Some(port), user) {
                Ok(()) => {
                    if warnings.is_empty() {
                        sections_viewer
                            .hint_label
                            .set_text(&tr("Opening Metis Viewer…"));
                    } else {
                        sections_viewer.hint_label.set_text(&format!(
                            "{} {}",
                            tr("Opening Metis Viewer…"),
                            warnings.join(" ")
                        ));
                    }
                }
                Err(err) => {
                    *sections_viewer.action_error.borrow_mut() = Some(err.clone());
                    sections_viewer.error_label.set_text(&err);
                    sections_viewer.error_label.set_visible(true);
                }
            }
        });
    }

    // RustDesk preset: detect / open / copy install instructions.
    let refresh_rustdesk = {
        let rd_status = rd_status.clone();
        let rd_open_btn = rd_open_btn.clone();
        let rd_enable_btn = rd_enable_btn.clone();
        Rc::new(move || {
            let install = remote::detect_rustdesk();
            let backend = remote::rustdesk_backend_status();
            let label = if let Some(snap) = &backend {
                if snap.backend_selected && snap.running {
                    format!("{} — Metis backend active", tr(install.status_label()))
                } else if snap.backend_selected {
                    format!("{} — Metis backend selected", tr(install.status_label()))
                } else {
                    tr(install.status_label()).to_string()
                }
            } else {
                tr(install.status_label()).to_string()
            };
            rd_status.set_text(&label);
            rd_open_btn.set_sensitive(install.is_installed());
            rd_enable_btn.set_sensitive(install.is_installed());
            if install.is_installed() {
                rd_open_btn.set_tooltip_text(None);
            } else {
                rd_open_btn.set_tooltip_text(Some(&tr(
                    "Install RustDesk first (Copy install instructions).",
                )));
            }
        })
    };
    refresh_rustdesk();

    {
        let sections_rd = sections.clone();
        let refresh_rustdesk = refresh_rustdesk.clone();
        rd_open_btn.connect_clicked(move |_| {
            let install = remote::detect_rustdesk();
            match remote::open_rustdesk(&install) {
                Ok(()) => {
                    sections_rd.hint_label.set_text(&tr("Opening RustDesk…"));
                    refresh_rustdesk();
                }
                Err(err) => {
                    *sections_rd.action_error.borrow_mut() = Some(err.clone());
                    sections_rd.error_label.set_text(&err);
                    sections_rd.error_label.set_visible(true);
                }
            }
        });
    }

    {
        let sections_rd = sections.clone();
        let refresh_rustdesk = refresh_rustdesk.clone();
        rd_enable_btn.connect_clicked(move |_| match remote::rustdesk_enable() {
            Ok(()) => {
                sections_rd.hint_label.set_text(&tr(
                    "RustDesk Metis backend enabled (optional). GRD remains the default host.",
                ));
                refresh_rustdesk();
            }
            Err(err) => {
                *sections_rd.action_error.borrow_mut() = Some(err.clone());
                sections_rd.error_label.set_text(&err);
                sections_rd.error_label.set_visible(true);
            }
        });
    }

    {
        let sections_rd = sections.clone();
        let refresh_rustdesk = refresh_rustdesk.clone();
        rd_disable_btn.connect_clicked(move |_| match remote::rustdesk_disable(false) {
            Ok(()) => {
                sections_rd
                    .hint_label
                    .set_text(&tr("RustDesk Metis backend preference cleared."));
                refresh_rustdesk();
            }
            Err(err) => {
                *sections_rd.action_error.borrow_mut() = Some(err.clone());
                sections_rd.error_label.set_text(&err);
                sections_rd.error_label.set_visible(true);
            }
        });
    }

    {
        let sections_rd = sections.clone();
        rd_copy_btn.connect_clicked(move |_| {
            let text = remote::rustdesk_install_instructions();
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(text);
            }
            sections_rd
                .hint_label
                .set_text(&tr("Copied RustDesk install instructions."));
        });
    }

    // Cheap re-detect when the page is already open (e.g. user installed Flatpak).
    {
        let refresh_rustdesk = refresh_rustdesk.clone();
        glib::timeout_add_local(Duration::from_secs(5), move || {
            refresh_rustdesk();
            glib::ControlFlow::Continue
        });
    }

    refresh();
    scroller.upcast()
}

fn set_busy(sections: &Sections, busy: bool) {
    if busy {
        sections.status_spinner.set_visible(true);
        sections.status_spinner.start();
        // Only lock the sharing switch while the CLI is in flight — leave LAN
        // usable, and unlock as soon as enable/disable returns.
        if sections.enable_pending.get() {
            sections.enable_sw.set_sensitive(false);
        }
    } else {
        sections.status_spinner.stop();
        sections.status_spinner.set_visible(false);
    }
}

fn readout_row(title: &str, value: &gtk::Label) -> gtk::Box {
    value.add_css_class("metis-settings-value");
    value.set_hexpand(true);
    readout_row_widget(title, value)
}

fn readout_row_widget(title: &str, value: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let title = gtk::Label::new(Some(title));
    title.set_xalign(0.0);
    title.set_width_chars(10);
    row.append(&title);
    row.append(value);
    row
}

/// Confirm turning off LAN-only while desktop sharing is active.
fn confirm_disable_lan_only(parent: &gtk::Window, on_done: impl Fn(bool) + 'static) {
    let dialog = gtk::Window::builder()
        .title(tr("Allow RDP beyond the LAN?"))
        .modal(true)
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .default_width(420)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let body = gtk::Label::new(Some(&tr(
        "Turning off LAN only removes Metis firewall rules for port 3389. \
         Remote desktop may then be reachable from outside your private network \
         unless you have other protections (VPN, host firewall).",
    )));
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.add_css_class("metis-settings-hint");
    outer.append(&body);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    btn_row.set_margin_top(16);
    let cancel = gtk::Button::with_label(&tr("Keep LAN only"));
    cancel.add_css_class("metis-settings-secondary");
    let proceed = gtk::Button::with_label(&tr("Turn off LAN only"));
    proceed.add_css_class("destructive-action");
    btn_row.append(&cancel);
    btn_row.append(&proceed);
    outer.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    let on_done = Rc::new(on_done);
    let finished = Rc::new(Cell::new(false));
    let finish = {
        let on_done = on_done.clone();
        let finished = finished.clone();
        let dialog = dialog.clone();
        Rc::new(move |confirmed: bool| {
            if finished.replace(true) {
                return;
            }
            on_done(confirmed);
            dialog.destroy();
        })
    };
    cancel.connect_clicked({
        let finish = finish.clone();
        move |_| finish(false)
    });
    proceed.connect_clicked({
        let finish = finish.clone();
        move |_| finish(true)
    });
    dialog.connect_close_request({
        let finish = finish.clone();
        move |_| {
            finish(false);
            glib::Propagation::Stop
        }
    });

    dialog.present();
}

fn render(sections: &Sections, snap: &RemoteSnapshot) {
    sections.install_banner.set_visible(!snap.available);
    sections
        .password_banner
        .set_visible(snap.available && !snap.password_set);
    sections
        .change_pw_btn
        .set_visible(snap.available && snap.password_set);

    // Clear warm-up once RDP is listening, or if sharing was turned off.
    if sections.warming_up.get()
        && (snap.rdp_enabled || !snap.config_enabled || !sections.share_wanted.get())
    {
        sections.warming_up.set(false);
    }

    let pending = sections.enable_pending.get();
    let warming = sections.warming_up.get();
    let busy = pending || warming;

    if busy {
        set_busy(sections, true);
        if warming && !pending {
            sections
                .enable_sw
                .set_sensitive(snap.available && snap.password_set);
            sections.lan_sw.set_sensitive(snap.available);
        }
    } else {
        set_busy(sections, false);
        sections
            .enable_sw
            .set_sensitive(snap.available && snap.password_set);
        sections.lan_sw.set_sensitive(snap.available);
    }

    sections.toggling.set(true);
    if !pending && sections.enable_sw.is_active() != snap.config_enabled {
        // While warming up, keep the switch on even if a stale poll arrives.
        if !(warming && sections.share_wanted.get()) {
            sections.enable_sw.set_active(snap.config_enabled);
        }
    }
    sections.toggling.set(false);
    if !pending {
        if warming {
            sections.share_wanted.set(true);
        } else {
            sections.share_wanted.set(snap.config_enabled);
        }
    }

    sections.lan_toggling.set(true);
    if !sections.lan_pending.get() && sections.lan_sw.is_active() != snap.lan_only {
        sections.lan_sw.set_active(snap.lan_only);
    }
    sections.lan_toggling.set(false);

    if pending {
        if sections.enable_sw.is_active() {
            sections
                .status_label
                .set_text(&tr("Starting desktop sharing…"));
            sections
                .address_label
                .set_text(&remote::connection_hint(snap));
            sections.port_label.set_text(&snap.port.to_string());
        } else {
            sections
                .status_label
                .set_text(&tr("Stopping desktop sharing…"));
        }
    } else if warming {
        if snap.rdp_enabled {
            sections
                .status_label
                .set_text(&tr("Running — ready for connections"));
            sections.warming_up.set(false);
            set_busy(sections, false);
        } else {
            sections
                .status_label
                .set_text(&tr("Starting — waiting for RDP…"));
        }
        sections
            .address_label
            .set_text(&remote::connection_hint(snap));
        sections.port_label.set_text(&snap.port.to_string());
        if snap.password_set {
            sections
                .username_label
                .set_text(&tr("Use your session sharing password"));
        }
    } else if !snap.available {
        sections.status_label.set_text(&tr("Not available"));
        sections.address_label.set_text(&tr("—"));
        sections.port_label.set_text(&tr("—"));
        sections.username_label.set_text(&tr("—"));
    } else {
        let sharing_password = tr("Use your session sharing password");
        let dash = tr("—");
        let username = snap
            .username
            .as_deref()
            .filter(|u| !u.eq_ignore_ascii_case("(hidden)"))
            .map(String::from)
            .unwrap_or_else(|| {
                if snap.password_set {
                    sharing_password.clone()
                } else {
                    dash.clone()
                }
            });
        if snap.rdp_enabled {
            sections
                .status_label
                .set_text(&tr("Running — ready for connections"));
            sections
                .address_label
                .set_text(&remote::connection_hint(snap));
            sections.port_label.set_text(&snap.port.to_string());
            sections.username_label.set_text(&username);
        } else if snap.config_enabled {
            sections
                .status_label
                .set_text(&tr("Enabled — RDP not listening (locked or starting)"));
            sections
                .address_label
                .set_text(&remote::connection_hint(snap));
            sections.port_label.set_text(&snap.port.to_string());
            sections.username_label.set_text(&username);
        } else {
            sections.status_label.set_text(&tr("Stopped"));
            sections.address_label.set_text(&tr("—"));
            sections.port_label.set_text(&tr("—"));
            sections.username_label.set_text(&username);
        }
    }

    render_firewall_status(sections, snap);

    if let Some(err) = snap
        .error
        .as_deref()
        .or(sections.action_error.borrow().as_deref())
    {
        sections.error_label.set_text(err);
        sections.error_label.set_visible(true);
    } else {
        sections.error_label.set_visible(false);
    }
}

fn render_firewall_status(sections: &Sections, snap: &RemoteSnapshot) {
    let sharing_on =
        snap.config_enabled || sections.share_wanted.get() || sections.warming_up.get();

    if snap.firewall_applied {
        sections.firewall_pending.set(false);
    } else if sections.firewall_pending.get() {
        // Background apply finished with a persisted error — stop spinning.
        if snap
            .firewall_detail
            .as_deref()
            .is_some_and(|d| !d.contains("not applied yet"))
        {
            sections.firewall_pending.set(false);
        }
    }

    let (status_text, show_retry) = if !snap.lan_only {
        (
            tr("LAN only is off — RDP may be reachable beyond your private network."),
            false,
        )
    } else if !sharing_on {
        (
            tr("LAN only is on. Firewall rules apply automatically when you enable desktop sharing."),
            false,
        )
    } else if sections.firewall_pending.get() && !snap.firewall_applied {
        (
            tr("Applying firewall rules… A password dialog may appear."),
            false,
        )
    } else if snap.firewall_applied {
        let backend = if snap.firewall_backend.is_empty() {
            "firewall".to_string()
        } else {
            snap.firewall_backend.clone()
        };
        (format!("LAN-only rules are active ({backend})."), false)
    } else {
        let detail = snap
            .firewall_detail
            .clone()
            .unwrap_or_else(|| tr("LAN-only is on, but firewall rules are not applied yet."));
        (detail, snap.available && !sections.enable_pending.get())
    };

    sections.firewall_status.set_text(&status_text);
    sections.firewall_status.set_visible(true);
    if show_retry {
        sections.retry_fw_btn.set_label(&tr("Retry firewall apply"));
        sections.retry_fw_btn.set_sensitive(true);
    }
    sections.retry_fw_btn.set_visible(show_retry);
}

/// Top-slide sheet for session sharing username + password (same host as
/// color/font pickers).
fn show_password_sheet(
    cred_tx: mpsc::Sender<Result<(), String>>,
    password_ui_open: Rc<Cell<bool>>,
) {
    if password_ui_open.get() || dialog::is_open() {
        return;
    }
    password_ui_open.set(true);

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let hint = gtk::Label::new(Some(&tr(
        "Choose the username and password RDP clients use to join this session.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-top-dialog-body");
    wrap.append(&hint);

    let user_entry = gtk::Entry::new();
    user_entry.set_placeholder_text(Some(&tr("Username")));
    user_entry.set_hexpand(true);
    if let Ok(user) = std::env::var("USER") {
        user_entry.set_text(&user);
    }
    ui::swallow_empty_backspace(&user_entry);
    wrap.append(&user_entry);

    let pass_entry = gtk::Entry::new();
    pass_entry.set_placeholder_text(Some(&tr("Password")));
    pass_entry.set_visibility(false);
    pass_entry.set_input_purpose(gtk::InputPurpose::Password);
    pass_entry.set_hexpand(true);
    ui::swallow_empty_backspace(&pass_entry);
    wrap.append(&pass_entry);

    let confirm_entry = gtk::Entry::new();
    confirm_entry.set_placeholder_text(Some(&tr("Confirm password")));
    confirm_entry.set_visibility(false);
    confirm_entry.set_input_purpose(gtk::InputPurpose::Password);
    confirm_entry.set_hexpand(true);
    ui::swallow_empty_backspace(&confirm_entry);
    wrap.append(&confirm_entry);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    // Header Cancel dismisses the sheet — only Save belongs in the body.
    let save = gtk::Button::with_label(&tr("Save"));
    save.add_css_class("suggested-action");
    btn_row.append(&save);
    wrap.append(&btn_row);

    let (save_tx, save_rx) = mpsc::channel::<Result<(), String>>();

    save.connect_clicked({
        let save = save.clone();
        let user_entry = user_entry.clone();
        let pass_entry = pass_entry.clone();
        let confirm_entry = confirm_entry.clone();
        let err = err.clone();
        let save_tx = save_tx.clone();
        move |_| {
            let user = user_entry.text().to_string();
            let pass = pass_entry.text().to_string();
            let confirm = confirm_entry.text().to_string();
            if pass != confirm {
                err.set_text(&tr("Passwords do not match"));
                err.set_visible(true);
                return;
            }
            if pass.len() < 8 {
                err.set_text(&tr("Use at least 8 characters"));
                err.set_visible(true);
                return;
            }
            err.set_visible(false);
            save.set_sensitive(false);
            save.set_label(&tr("Saving…"));
            let save_tx = save_tx.clone();
            std::thread::spawn(move || {
                let result = remote::set_credentials(&user, &pass);
                let mut pass = pass;
                remote::scrub_password(&mut pass);
                let _ = save_tx.send(result);
            });
        }
    });

    let cred_tx_done = cred_tx.clone();
    let password_ui_open_poll = password_ui_open.clone();
    let save_btn = save.clone();
    let err_poll = err.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Ok(result) = save_rx.try_recv() else {
            return glib::ControlFlow::Continue;
        };
        save_btn.set_sensitive(true);
        save_btn.set_label(&tr("Save"));
        match result {
            Ok(()) => {
                let _ = cred_tx_done.send(Ok(()));
                password_ui_open_poll.set(false);
                dialog::dismiss_silent();
                glib::ControlFlow::Break
            }
            Err(e) => {
                err_poll.set_text(&e);
                err_poll.set_visible(true);
                glib::ControlFlow::Continue
            }
        }
    });

    let opened = dialog::present(&tr("Session sharing password"), &wrap, {
        let password_ui_open = password_ui_open.clone();
        Rc::new(move || {
            password_ui_open.set(false);
        })
    });
    if !opened {
        password_ui_open.set(false);
        return;
    }
    user_entry.grab_focus();
}
