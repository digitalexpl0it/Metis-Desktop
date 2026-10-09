//! VPN tab: OpenVPN / WireGuard profiles, import, connect, and editors.

use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;

use crate::dialog;
use crate::net::{self, OpenVpnCreate, VpnConn, VpnKind, WireGuardCreate, WireGuardProfile};
use crate::ui;
use metis_i18n::tr;

use super::schedule_refresh;

pub(crate) fn vpn_row<F: Fn() + 'static>(
    vpn: &VpnConn,
    refresh: &Rc<F>,
    status: &gtk::Label,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_valign(gtk::Align::Center);
    if vpn.active {
        row.add_css_class("metis-settings-row-active");
    }

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(&vpn.name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(28);
    text.append(&name);
    let mut meta = vpn.kind.label().to_string();
    if vpn.active {
        meta.push_str(" · ");
        meta.push_str(&tr("Connected"));
    }
    let kind = gtk::Label::new(Some(&meta));
    kind.set_xalign(0.0);
    kind.add_css_class("metis-settings-hint");
    text.append(&kind);

    // Autoconnect under the title keeps the action strip compact so a wide
    // VPN row cannot lock Settings' minimum window width.
    let auto_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    auto_row.set_halign(gtk::Align::Start);
    auto_row.set_margin_top(2);
    let auto_lbl = gtk::Label::new(Some(&tr("Auto-connect")));
    auto_lbl.add_css_class("metis-settings-hint");
    auto_lbl.set_valign(gtk::Align::Center);
    auto_lbl.set_tooltip_text(Some(&tr(
        "Connect this VPN after login when the network is ready. Only one profile can auto-connect."
        )));
    auto_row.append(&auto_lbl);
    let auto = gtk::Switch::new();
    auto.set_active(vpn.autoconnect);
    auto.set_valign(gtk::Align::Center);
    auto.set_tooltip_text(Some(&tr(
        "Connect this VPN after login when the network is ready. Only one profile can auto-connect."
        )));
    {
        let refresh = refresh.clone();
        let status = status.clone();
        let uuid = vpn.uuid.clone();
        let was_active = vpn.active;
        auto.connect_state_set(move |sw, on| {
            let uuid_for_set = uuid.clone();
            let uuid = uuid.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let sw = sw.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_set_autoconnect(&uuid_for_set, on));
            });
            glib::timeout_add_local(Duration::from_millis(50), move || {
                match rx.try_recv() {
                    Ok(Ok(())) => {
                        let msg = if on {
                            tr("Autoconnect enabled (only this profile).")
                        } else {
                            tr("Autoconnect disabled.")
                        };
                        set_vpn_status(&status, &msg, false);
                        if on && !was_active {
                            // Connect now so the setting takes effect without
                            // waiting for the next login.
                            let uuid = uuid.clone();
                            let status = status.clone();
                            let refresh = refresh.clone();
                            set_vpn_status(&status, &tr("Connecting…"), false);
                            let (tx2, rx2) = mpsc::channel::<Result<(), String>>();
                            std::thread::spawn(move || {
                                let _ = tx2.send(net::vpn_up(&uuid));
                            });
                            glib::timeout_add_local(Duration::from_millis(50), move || {
                                match rx2.try_recv() {
                                    Ok(Ok(())) => {
                                        set_vpn_status(&status, &tr("Connected."), false);
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                    Ok(Err(e)) => {
                                        set_vpn_status(&status, &e, true);
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        refresh();
                                        glib::ControlFlow::Break
                                    }
                                }
                            });
                        } else {
                            refresh();
                        }
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        sw.set_active(!on);
                        set_vpn_status(&status, &e, true);
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        sw.set_active(!on);
                        glib::ControlFlow::Break
                    }
                }
            });
            glib::Propagation::Proceed
        });
    }
    auto_row.append(&auto);
    text.append(&auto_row);
    row.append(&text);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_valign(gtk::Align::Center);
    actions.set_halign(gtk::Align::End);
    actions.set_hexpand(false);

    if vpn.kind == VpnKind::WireGuard {
        let edit = gtk::Button::with_label(&tr("Edit"));
        edit.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            edit.connect_clicked(move |btn| {
                let parent = btn.root().and_downcast::<gtk::Window>();
                show_wireguard_edit_dialog(parent.as_ref(), &uuid, status.clone(), refresh.clone());
            });
        }
        actions.append(&edit);
    }

    if vpn.active {
        let disconnect = gtk::Button::with_label(&tr("Disconnect"));
        disconnect.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            disconnect.connect_clicked(move |btn| {
                run_vpn_toggle(false, &uuid, btn, &status, &refresh);
            });
        }
        actions.append(&disconnect);
    } else {
        let connect = gtk::Button::with_label(&tr("Connect"));
        connect.add_css_class("suggested-action");
        connect.set_valign(gtk::Align::Center);
        {
            let refresh = refresh.clone();
            let status = status.clone();
            let uuid = vpn.uuid.clone();
            connect.connect_clicked(move |btn| {
                run_vpn_toggle(true, &uuid, btn, &status, &refresh);
            });
        }
        actions.append(&connect);
    }

    let delete = gtk::Button::with_label(&tr("Delete"));
    delete.add_css_class("destructive-action");
    delete.set_valign(gtk::Align::Center);
    {
        let refresh = refresh.clone();
        let uuid = vpn.uuid.clone();
        delete.connect_clicked(move |_| {
            net::vpn_delete(&uuid);
            schedule_refresh(&refresh, 1200);
        });
    }
    actions.append(&delete);
    row.append(&actions);
    row.upcast()
}

fn run_vpn_toggle<F: Fn() + 'static>(
    connect: bool,
    uuid: &str,
    btn: &gtk::Button,
    status: &gtk::Label,
    refresh: &Rc<F>,
) {
    let busy = if connect {
        tr("Connecting…")
    } else {
        tr("Disconnecting…")
    };
    let done_label = if connect {
        tr("Connect")
    } else {
        tr("Disconnect")
    };
    btn.set_sensitive(false);
    btn.set_label(&busy);
    set_vpn_status(status, &busy, false);

    let uuid = uuid.to_string();
    let uuid_for_prompt = uuid.clone();
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        let result = if connect {
            net::vpn_up(&uuid)
        } else {
            net::vpn_down(&uuid)
        };
        let _ = tx.send(result);
    });

    let btn = btn.clone();
    let status = status.clone();
    let refresh = refresh.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
        Ok(Ok(())) => {
            let msg = if connect {
                tr("Connected.")
            } else {
                tr("Disconnected.")
            };
            set_vpn_status(&status, &msg, false);
            refresh();
            glib::ControlFlow::Break
        }
        Ok(Err(e)) => {
            btn.set_sensitive(true);
            btn.set_label(&done_label);
            if connect && net::vpn_secret_required(&e) {
                set_vpn_status(
                    &status,
                    &tr("Password required — enter your VPN password."),
                    false,
                );
                let parent = btn.root().and_downcast::<gtk::Window>();
                show_vpn_password_dialog(
                    parent.as_ref(),
                    &uuid_for_prompt,
                    status.clone(),
                    refresh.clone(),
                    btn.clone(),
                );
            } else {
                set_vpn_status(&status, &e, true);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            btn.set_sensitive(true);
            btn.set_label(&done_label);
            glib::ControlFlow::Break
        }
    });
}

fn show_vpn_password_dialog<F: Fn() + 'static>(
    parent: Option<&gtk::Window>,
    uuid: &str,
    status: gtk::Label,
    refresh: Rc<F>,
    connect_btn: gtk::Button,
) {
    let Some(parent) = parent else {
        set_vpn_status(
            &status,
            &tr("VPN password required, but no window is available to prompt."),
            true,
        );
        return;
    };

    let dialog = gtk::Window::builder()
        .title(tr("VPN password"))
        .modal(true)
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .default_width(420)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let heading = gtk::Label::new(Some(&tr("VPN password required")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    outer.append(&heading);

    let hint = gtk::Label::new(Some(&tr(
        "This profile needs a password that NetworkManager does not have yet \
         (common after moving from another desktop). Enter it to connect.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    outer.append(&hint);

    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .placeholder_text(tr("Password"))
        .build();
    outer.append(&entry);

    let remember = gtk::CheckButton::with_label(&tr("Remember password on this profile"));
    remember.set_active(true);
    outer.append(&remember);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    outer.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    btn_row.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let connect = gtk::Button::with_label(&tr("Connect"));
    connect.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&connect);
    outer.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let do_connect = {
        let dialog = dialog.clone();
        let entry = entry.clone();
        let remember = remember.clone();
        let err = err.clone();
        let status = status.clone();
        let refresh = refresh.clone();
        let connect_btn = connect_btn.clone();
        let connect = connect.clone();
        let uuid = uuid.to_string();
        Rc::new(move || {
            let password = entry.text().to_string();
            if password.trim().is_empty() {
                err.set_text(&tr("Enter the VPN password."));
                err.set_visible(true);
                return;
            }
            err.set_visible(false);
            connect.set_sensitive(false);
            connect.set_label(&tr("Connecting…"));
            connect_btn.set_sensitive(false);
            connect_btn.set_label(&tr("Connecting…"));
            set_vpn_status(&status, &tr("Connecting…"), false);

            let remember_on = remember.is_active();
            let uuid = uuid.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_up_with_password(&uuid, &password, remember_on));
            });

            let dialog = dialog.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let connect = connect.clone();
            let connect_btn = connect_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("Connected."), false);
                    refresh();
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    connect.set_sensitive(true);
                    connect.set_label(&tr("Connect"));
                    connect_btn.set_sensitive(true);
                    connect_btn.set_label(&tr("Connect"));
                    set_vpn_status(&status, &e, true);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    connect.set_sensitive(true);
                    connect.set_label(&tr("Connect"));
                    connect_btn.set_sensitive(true);
                    connect_btn.set_label(&tr("Connect"));
                    glib::ControlFlow::Break
                }
            });
        })
    };

    {
        let do_connect = do_connect.clone();
        connect.connect_clicked(move |_| do_connect());
    }
    {
        let do_connect = do_connect.clone();
        entry.connect_activate(move |_| do_connect());
    }

    dialog.present();
    let focus = entry.clone();
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

fn set_vpn_status(label: &gtk::Label, msg: &str, error: bool) {
    label.set_text(msg);
    label.set_visible(!msg.is_empty());
    if error {
        label.add_css_class("metis-settings-error");
        label.remove_css_class("metis-settings-hint");
    } else {
        label.remove_css_class("metis-settings-error");
        label.add_css_class("metis-settings-hint");
    }
}

pub(crate) fn pick_vpn_import(
    parent: Option<&gtk::Window>,
    status: gtk::Label,
    refresh: Rc<impl Fn() + 'static>,
) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title(&tr("Import VPN profile"));
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("VPN configs (*.ovpn, *.conf)")));
    filter.add_pattern("*.ovpn");
    filter.add_pattern("*.OVPN");
    filter.add_pattern("*.conf");
    filter.add_pattern("*.CONF");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    dialog.open(parent, gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let path_s = path.to_string_lossy().to_string();
        set_vpn_status(&status, &tr("Importing…"), false);
        // Brief main-thread wait — import is a single nmcli call.
        match import_vpn_file(&path_s) {
            Ok(name) => {
                set_vpn_status(&status, &format!("Imported “{name}”."), false);
                refresh();
            }
            Err(err) => set_vpn_status(&status, &err, true),
        }
    });
}

fn import_vpn_file(path: &str) -> Result<String, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ovpn" => net::vpn_import_openvpn(path),
        "conf" => net::vpn_import_wireguard(path),
        _ => {
            // Try OpenVPN first, then WireGuard.
            net::vpn_import_openvpn(path).or_else(|_| net::vpn_import_wireguard(path))
        }
    }
}

pub(crate) fn show_openvpn_sheet(status: gtk::Label, refresh: Rc<impl Fn() + 'static>) {
    if dialog::is_open() {
        return;
    }
    if !net::openvpn_plugin_present() {
        set_vpn_status(
            &status,
            &tr("OpenVPN plugin missing. Install with: sudo apt install network-manager-openvpn"),
            true,
        );
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let hint = gtk::Label::new(Some(&tr(
        "Password authentication only. For provider configs with certificates, use Import… on an .ovpn file.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-top-dialog-body");
    wrap.append(&hint);

    let name = wg_entry("Work VPN", "");
    let gateway = wg_entry("vpn.example.com", "");
    let username = wg_entry("username", "");
    let password = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .placeholder_text(tr("Password (optional)"))
        .build();
    let ca = wg_entry("/path/to/ca.crt", "");

    wrap.append(&wg_field(&tr("Name"), &name));
    wrap.append(&wg_field(&tr("Gateway"), &gateway));
    wrap.append(&wg_field(&tr("Username"), &username));

    let pw_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let pw_lbl = gtk::Label::new(Some(&tr("Password (optional)")));
    pw_lbl.set_xalign(0.0);
    pw_lbl.add_css_class("metis-settings-hint");
    pw_box.append(&pw_lbl);
    pw_box.append(&password);
    wrap.append(&pw_box);

    let ca_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    ca.set_hexpand(true);
    let browse = gtk::Button::with_label(&tr("Browse…"));
    ca_row.append(&ca);
    ca_row.append(&browse);
    let ca_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let ca_lbl = gtk::Label::new(Some(&tr("CA certificate (optional)")));
    ca_lbl.set_xalign(0.0);
    ca_lbl.add_css_class("metis-settings-hint");
    ca_box.append(&ca_lbl);
    ca_box.append(&ca_row);
    wrap.append(&ca_box);

    let remember = gtk::CheckButton::with_label(&tr("Remember password on this profile"));
    remember.set_active(true);
    wrap.append(&remember);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    // Header Cancel dismisses — only Create in the body.
    let create = gtk::Button::with_label(&tr("Create"));
    create.add_css_class("suggested-action");
    btn_row.append(&create);
    wrap.append(&btn_row);

    {
        let ca = ca.clone();
        browse.connect_clicked(move |btn| {
            let picker = gtk::FileDialog::new();
            picker.set_title(&tr("Choose CA certificate"));
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&tr("Certificates (*.crt, *.pem)")));
            filter.add_pattern("*.crt");
            filter.add_pattern("*.pem");
            filter.add_pattern("*.CER");
            filter.add_pattern("*.CRT");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            picker.set_filters(Some(&filters));
            let parent = btn.root().and_downcast::<gtk::Window>();
            let ca = ca.clone();
            picker.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                if let Some(path) = file.path() {
                    ca.set_text(&path.to_string_lossy());
                }
            });
        });
    }
    {
        let status = status.clone();
        let create_btn = create.clone();
        let name = name.clone();
        let gateway = gateway.clone();
        let username = username.clone();
        let password = password.clone();
        let ca = ca.clone();
        let remember = remember.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        create.connect_clicked(move |_| {
            create_btn.set_sensitive(false);
            create_btn.set_label(&tr("Creating…"));
            err.set_visible(false);

            let cfg = OpenVpnCreate {
                name: name.text().to_string(),
                gateway: gateway.text().to_string(),
                username: username.text().to_string(),
                password: password.text().to_string(),
                ca_path: ca.text().to_string(),
                remember_password: remember.is_active(),
            };
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_create_openvpn(cfg));
            });

            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let create_btn = create_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("OpenVPN profile created."), false);
                    refresh();
                    dialog::dismiss_silent();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Continue
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    if !dialog::present(&tr("Add OpenVPN"), &wrap, Rc::new(|| {})) {
        set_vpn_status(&status, &tr("Could not open OpenVPN sheet."), true);
        return;
    }
    let focus = name.clone();
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

pub(crate) fn show_wireguard_sheet(status: gtk::Label, refresh: Rc<impl Fn() + 'static>) {
    if dialog::is_open() {
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let name = wg_entry("Home VPN", "");
    let private_key = wg_entry("Interface private key", "");
    let address = wg_entry("10.0.0.2/32", "");
    let peer_pub = wg_entry("Peer public key", "");
    let endpoint = wg_entry("vpn.example.com:51820", "");
    let allowed = wg_entry("0.0.0.0/0, ::/0", "0.0.0.0/0, ::/0");
    let dns = wg_entry("1.1.1.1", "");

    wrap.append(&wg_field(&tr("Name"), &name));
    wrap.append(&wg_field(&tr("Private key"), &private_key));
    wrap.append(&wg_field(&tr("Address (CIDR)"), &address));
    wrap.append(&wg_field(&tr("Peer public key"), &peer_pub));
    wrap.append(&wg_field(&tr("Endpoint"), &endpoint));
    wrap.append(&wg_field(&tr("Allowed IPs"), &allowed));
    wrap.append(&wg_field(&tr("DNS (optional)"), &dns));

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    let create = gtk::Button::with_label(&tr("Create"));
    create.add_css_class("suggested-action");
    btn_row.append(&create);
    wrap.append(&btn_row);

    {
        let status = status.clone();
        let create_btn = create.clone();
        let name = name.clone();
        let private_key = private_key.clone();
        let address = address.clone();
        let peer_pub = peer_pub.clone();
        let endpoint = endpoint.clone();
        let allowed = allowed.clone();
        let dns = dns.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        create.connect_clicked(move |_| {
            create_btn.set_sensitive(false);
            create_btn.set_label(&tr("Creating…"));
            err.set_visible(false);

            let cfg = WireGuardCreate {
                name: name.text().to_string(),
                private_key: private_key.text().to_string(),
                address: address.text().to_string(),
                peer_public_key: peer_pub.text().to_string(),
                endpoint: endpoint.text().to_string(),
                allowed_ips: allowed.text().to_string(),
                dns: dns.text().to_string(),
            };

            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_create_wireguard(cfg));
            });

            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let create_btn = create_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("WireGuard connection created."), false);
                    refresh();
                    dialog::dismiss_silent();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Continue
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    create_btn.set_sensitive(true);
                    create_btn.set_label(&tr("Create"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    if !dialog::present(&tr("Add WireGuard"), &wrap, Rc::new(|| {})) {
        set_vpn_status(&status, &tr("Could not open WireGuard sheet."), true);
        return;
    }
    let focus_entry = name.clone();
    glib::idle_add_local_once(move || {
        focus_entry.grab_focus();
    });
}

fn show_wireguard_edit_dialog(
    parent: Option<&gtk::Window>,
    uuid: &str,
    status: gtk::Label,
    refresh: Rc<impl Fn() + 'static>,
) {
    let Some(parent) = parent else {
        set_vpn_status(
            &status,
            &tr("Could not open edit dialog (no parent window)."),
            true,
        );
        return;
    };
    let Some(profile) = net::vpn_get_wireguard(uuid) else {
        set_vpn_status(
            &status,
            &tr("Could not load WireGuard profile details."),
            true,
        );
        return;
    };

    let dialog = gtk::Window::builder()
        .title(tr("Edit WireGuard"))
        .modal(true)
        .transient_for(parent)
        .decorated(false)
        .resizable(false)
        .default_width(480)
        .build();
    dialog.add_css_class("metis-settings-window");
    dialog.add_css_class("metis-settings-password-dialog");

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(20);
    outer.set_margin_end(20);

    let heading = gtk::Label::new(Some(&tr("Edit WireGuard connection")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-section-title");
    outer.append(&heading);

    let name = wg_entry("Home VPN", &profile.name);
    let address = wg_entry("10.0.0.2/32", &profile.address);
    let peer_pub = wg_entry("Peer public key", &profile.peer_public_key);
    let endpoint = wg_entry("vpn.example.com:51820", &profile.endpoint);
    let allowed = wg_entry(
        "0.0.0.0/0, ::/0",
        if profile.allowed_ips.is_empty() {
            "0.0.0.0/0, ::/0"
        } else {
            &profile.allowed_ips
        },
    );
    let dns = wg_entry("1.1.1.1", &profile.dns);

    outer.append(&wg_field(&tr("Name"), &name));
    outer.append(&wg_field(&tr("Address (CIDR)"), &address));
    outer.append(&wg_field(&tr("Peer public key"), &peer_pub));
    outer.append(&wg_field(&tr("Endpoint"), &endpoint));
    outer.append(&wg_field(&tr("Allowed IPs"), &allowed));
    outer.append(&wg_field(&tr("DNS (optional)"), &dns));

    let hint = gtk::Label::new(Some(&tr(
        "Private key is unchanged. Disconnect and reconnect for peer/address edits to take effect.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    outer.append(&hint);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    outer.append(&err);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    btn_row.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let save = gtk::Button::with_label(&tr("Save"));
    save.add_css_class("suggested-action");
    btn_row.append(&cancel);
    btn_row.append(&save);
    outer.append(&btn_row);

    dialog.set_child(Some(&ui::dialog_sheet(&outer)));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let status = status.clone();
        let save_btn = save.clone();
        let uuid = uuid.to_string();
        let name = name.clone();
        let address = address.clone();
        let peer_pub = peer_pub.clone();
        let endpoint = endpoint.clone();
        let allowed = allowed.clone();
        let dns = dns.clone();
        let err = err.clone();
        let refresh = refresh.clone();
        save.connect_clicked(move |_| {
            save_btn.set_sensitive(false);
            save_btn.set_label(&tr("Saving…"));
            err.set_visible(false);

            let cfg = WireGuardProfile {
                name: name.text().to_string(),
                address: address.text().to_string(),
                peer_public_key: peer_pub.text().to_string(),
                endpoint: endpoint.text().to_string(),
                allowed_ips: allowed.text().to_string(),
                dns: dns.text().to_string(),
            };
            let uuid = uuid.clone();
            let (tx, rx) = mpsc::channel::<Result<(), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(net::vpn_update_wireguard(&uuid, cfg));
            });

            let dialog = dialog.clone();
            let status = status.clone();
            let refresh = refresh.clone();
            let err = err.clone();
            let save_btn = save_btn.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(Ok(())) => {
                    set_vpn_status(&status, &tr("WireGuard profile updated."), false);
                    refresh();
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    err.set_text(&e);
                    err.set_visible(true);
                    save_btn.set_sensitive(true);
                    save_btn.set_label(&tr("Save"));
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    save_btn.set_sensitive(true);
                    save_btn.set_label(&tr("Save"));
                    glib::ControlFlow::Break
                }
            });
        });
    }

    dialog.present();
    let focus_entry = name.clone();
    glib::idle_add_local_once(move || {
        focus_entry.grab_focus();
    });
}

fn wg_entry(placeholder: &str, value: &str) -> gtk::Entry {
    let e = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    e.set_text(value);
    // Held Backspace on an empty field must not bubble to the sidebar search.
    ui::swallow_empty_backspace(&e);
    e
}

fn wg_field(label: &str, entry: &gtk::Entry) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.add_css_class("metis-settings-hint");
    box_.append(&lbl);
    box_.append(entry);
    box_
}
