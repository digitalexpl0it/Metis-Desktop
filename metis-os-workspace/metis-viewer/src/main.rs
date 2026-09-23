//! Metis Viewer — GTK4 RDP connect UI over FreeRDP (host remains GRD).

mod freerdp;
mod theme;

use std::cell::RefCell;
use std::process::Child;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use metis_config::{ViewerHost, remember_host, remove_recent};
use metis_i18n::tr;

#[derive(Debug, Clone, Default)]
struct CliPrefill {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_viewer=info,warn".into()),
        )
        .init();

    // Running as root makes ~/.config/metis root-owned and breaks Appearance saves
    // for the real user — Viewer then forever reads a stale "dark" preference.
    if effective_uid() == 0 {
        eprintln!(
            "metis-viewer: refuse to run as root.\n\
             Launch as your normal user (not sudo), e.g. metis-viewer\n\
             or Settings → Remote access → Connect with Metis Viewer…"
        );
        std::process::exit(1);
    }

    metis_i18n::init();

    // Before GTK init — inherited Adwaita:dark from an older spawn must not
    // override Appearance → Light.
    theme::sync_gtk_theme_env();

    let prefill = parse_cli();

    let app = gtk::Application::builder()
        .application_id("com.metis.Viewer")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        build_ui(app, prefill.clone());
    });

    // Hand GApplication only argv0 so custom flags are not rejected.
    let argv0: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&argv0);
}

fn effective_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(2)) // effective uid
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(0)
}

fn running_under_metis() -> bool {
    if std::env::var_os("METIS_SESSION").is_some() {
        return true;
    }
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_ascii_lowercase().contains("metis"))
        .unwrap_or(false)
}

fn parse_cli() -> CliPrefill {
    let mut out = CliPrefill::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => out.host = args.next(),
            "--port" => {
                if let Some(p) = args.next() {
                    out.port = p.parse().ok();
                }
            }
            "--user" | "--username" => out.user = args.next(),
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metis-viewer [--host HOST] [--port PORT] [--user USER]\n\
                     Connect to an RDP host via FreeRDP (wlfreerdp3 / xfreerdp…)."
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("metis-viewer: unknown argument {other} (try --help)");
                std::process::exit(2);
            }
        }
    }
    out
}

type ConnectFn = Rc<dyn Fn()>;

fn build_ui(app: &gtk::Application, prefill: CliPrefill) {
    if let Some(win) = app.active_window() {
        theme::reapply();
        win.present();
        return;
    }

    theme::install();

    let under_metis = running_under_metis();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(tr("Metis Viewer"))
        .default_width(440)
        .default_height(560)
        .resizable(true)
        .decorated(!under_metis)
        .build();
    window.add_css_class("metis-viewer-window");
    window.set_opacity(1.0);
    if under_metis {
        window.add_css_class("metis-viewer-ssd");
        window.set_decorated(false);
        window.set_titlebar(gtk::Widget::NONE);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("metis-viewer-root");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_opacity(1.0);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 16);
    page.add_css_class("metis-viewer-page");
    page.set_margin_top(20);
    page.set_margin_bottom(24);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_hexpand(true);
    page.set_vexpand(true);

    // Under Metis SSD the compositor already shows the window title — icon + subtitle only.
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let icon = gtk::Image::from_icon_name("network-workgroup-symbolic");
    icon.set_pixel_size(36);
    icon.add_css_class("metis-viewer-header-icon");
    header.append(&icon);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    if !under_metis {
        let title = gtk::Label::new(Some(&tr("Remote Desktop")));
        title.set_xalign(0.0);
        title.add_css_class("metis-viewer-title");
        titles.append(&title);
    }
    let subtitle = gtk::Label::new(Some(&tr(
        "Connect to a Metis or RDP host. Sharing is enabled on the host under \
         Settings → Remote access.",
    )));
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("metis-viewer-subtitle");
    titles.append(&subtitle);
    titles.set_hexpand(true);
    header.append(&titles);
    page.append(&header);

    let freerdp = freerdp::resolve_freerdp();
    if freerdp.is_none() {
        page.append(&missing_freerdp_banner());
    }

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("metis-viewer-card");
    let card_title = gtk::Label::new(Some(&tr("Connection")));
    card_title.set_xalign(0.0);
    card_title.add_css_class("metis-viewer-card-title");
    card.append(&card_title);

    let host_entry = gtk::Entry::new();
    host_entry.set_placeholder_text(Some(&tr("Hostname or IP")));
    host_entry.set_hexpand(true);
    if let Some(h) = &prefill.host {
        host_entry.set_text(h);
    }

    let port_entry = gtk::Entry::new();
    port_entry.set_placeholder_text(Some("3389"));
    port_entry.set_input_purpose(gtk::InputPurpose::Digits);
    port_entry.set_max_length(5);
    port_entry.set_width_chars(5);
    port_entry.set_max_width_chars(5);
    port_entry.set_text(&prefill.port.unwrap_or(3389).to_string());

    let host_port = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    host_port.add_css_class("metis-viewer-field");
    let host_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    host_col.set_hexpand(true);
    let host_lbl = gtk::Label::new(Some(&tr("Host")));
    host_lbl.set_xalign(0.0);
    host_lbl.add_css_class("metis-viewer-field-label");
    host_col.append(&host_lbl);
    host_col.append(&host_entry);
    let port_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let port_lbl = gtk::Label::new(Some(&tr("Port")));
    port_lbl.set_xalign(0.0);
    port_lbl.add_css_class("metis-viewer-field-label");
    port_col.append(&port_lbl);
    port_col.append(&port_entry);
    host_port.append(&host_col);
    host_port.append(&port_col);
    card.append(&host_port);

    let user_entry = gtk::Entry::new();
    user_entry.set_placeholder_text(Some(&tr("Username")));
    if let Some(u) = &prefill.user {
        user_entry.set_text(u);
    } else if let Ok(u) = std::env::var("USER")
        && !u.is_empty()
    {
        user_entry.set_text(&u);
    }
    card.append(&field_box(&tr("Username"), &user_entry));

    let pass_entry = gtk::PasswordEntry::new();
    pass_entry.set_show_peek_icon(true);
    pass_entry.set_placeholder_text(Some(&tr("Optional")));
    card.append(&field_box(&tr("Password"), &pass_entry));
    let pass_hint = gtk::Label::new(Some(&tr(
        "Leave blank to let FreeRDP prompt. Passwords are never saved. If filled, \
         they are passed on the FreeRDP command line briefly (visible in /proc).",
    )));
    pass_hint.set_xalign(0.0);
    pass_hint.set_wrap(true);
    pass_hint.add_css_class("metis-viewer-hint");
    card.append(&pass_hint);
    page.append(&card);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("metis-viewer-status");
    if let Some(bin) = &freerdp {
        status.set_text(&format!("{} {}", tr("Ready —"), bin.display()));
        status.add_css_class("metis-viewer-ready");
    } else {
        set_status(
            &status,
            &tr("Connect disabled — install FreeRDP first."),
            StatusKind::Error,
        );
    }
    page.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-viewer-actions");
    actions.set_halign(gtk::Align::End);
    let connect_btn = gtk::Button::with_label(&tr("Connect"));
    connect_btn.add_css_class("suggested-action");
    connect_btn.set_sensitive(freerdp.is_some());
    if freerdp.is_none() {
        connect_btn.set_tooltip_text(Some(&tr("Install FreeRDP to enable Connect")));
    }
    actions.append(&connect_btn);
    page.append(&actions);

    let recent_header = gtk::Label::new(Some(&tr("Recent")));
    recent_header.set_xalign(0.0);
    recent_header.add_css_class("metis-viewer-card-title");
    recent_header.set_margin_top(4);
    page.append(&recent_header);

    let recent_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    recent_box.add_css_class("metis-viewer-recent");
    recent_box.set_visible(false);
    let recent_list = gtk::ListBox::new();
    recent_list.set_selection_mode(gtk::SelectionMode::None);
    recent_list.set_show_separators(false);
    recent_box.append(&recent_list);
    page.append(&recent_box);

    let recent_empty = gtk::Label::new(Some(&tr("No recent hosts yet.")));
    recent_empty.set_xalign(0.0);
    recent_empty.add_css_class("metis-viewer-empty");
    page.append(&recent_empty);

    let connect_busy = Rc::new(RefCell::new(false));
    let watched_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    // Filled after do_connect is built so recent rows can invoke it.
    let connect_slot: Rc<RefCell<Option<ConnectFn>>> = Rc::new(RefCell::new(None));

    let do_connect: ConnectFn = Rc::new({
        let connect_busy = connect_busy.clone();
        let connect_btn = connect_btn.clone();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let pass_entry = pass_entry.clone();
        let status = status.clone();
        let recent_list = recent_list.clone();
        let recent_box = recent_box.clone();
        let recent_empty = recent_empty.clone();
        let watched_child = watched_child.clone();
        let connect_slot = connect_slot.clone();

        move || {
            if *connect_busy.borrow() {
                return;
            }
            if freerdp::resolve_freerdp().is_none() {
                set_status(
                    &status,
                    &freerdp::freerdp_install_hint_full(),
                    StatusKind::Error,
                );
                return;
            }
            *connect_busy.borrow_mut() = true;
            connect_btn.set_sensitive(false);

            let host = host_entry.text().to_string();
            let port_text = port_entry.text().to_string();
            let username = user_entry.text().to_string();
            let password = pass_entry.text().to_string();

            let port: u16 = match port_text.trim().parse() {
                Ok(0) | Err(_) => {
                    set_status(
                        &status,
                        &tr("Enter a valid port (1–65535)."),
                        StatusKind::Error,
                    );
                    *connect_busy.borrow_mut() = false;
                    connect_btn.set_sensitive(true);
                    return;
                }
                Ok(p) => p,
            };
            if host.trim().is_empty() {
                set_status(
                    &status,
                    &tr("Enter a host name or IP address."),
                    StatusKind::Error,
                );
                *connect_busy.borrow_mut() = false;
                connect_btn.set_sensitive(true);
                return;
            }

            let req = freerdp::ConnectRequest {
                host: host.clone(),
                port,
                username: username.clone(),
                password: if password.is_empty() {
                    None
                } else {
                    Some(password)
                },
            };

            match freerdp::spawn_freerdp(req) {
                Ok(spawned) => {
                    set_status(
                        &status,
                        &format!("{} {}", tr("Connecting with"), spawned.binary.display()),
                        StatusKind::Ok,
                    );
                    let entry = ViewerHost {
                        host: host.trim().to_string(),
                        port,
                        username: username.trim().to_string(),
                    };
                    if let Err(e) = remember_host(entry) {
                        tracing::warn!("viewer.json save failed: {e}");
                    }
                    let on_connect = connect_slot.borrow().clone();
                    refill_recent(
                        &recent_list,
                        &recent_box,
                        &recent_empty,
                        &host_entry,
                        &port_entry,
                        &user_entry,
                        on_connect,
                    );

                    let started = Instant::now();
                    *watched_child.borrow_mut() = Some(spawned.child);
                    let watched = watched_child.clone();
                    let status_watch = status.clone();
                    let busy = connect_busy.clone();
                    let btn = connect_btn.clone();
                    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                        let mut slot = watched.borrow_mut();
                        let Some(child) = slot.as_mut() else {
                            *busy.borrow_mut() = false;
                            btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                            return glib::ControlFlow::Break;
                        };
                        match freerdp::poll_early_failure(child, started) {
                            freerdp::EarlyWatch::Running => glib::ControlFlow::Continue,
                            freerdp::EarlyWatch::Done => {
                                *slot = None;
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                            freerdp::EarlyWatch::Failed(msg) => {
                                *slot = None;
                                set_status(&status_watch, &msg, StatusKind::Error);
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                        }
                    });
                }
                Err(e) => {
                    set_status(&status, &e, StatusKind::Error);
                    *connect_busy.borrow_mut() = false;
                    connect_btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                }
            }
        }
    });

    *connect_slot.borrow_mut() = Some(do_connect.clone());

    {
        let do_connect = do_connect.clone();
        connect_btn.connect_clicked(move |_| do_connect());
    }
    for entry in [&host_entry, &port_entry, &user_entry] {
        let do_connect = do_connect.clone();
        entry.connect_activate(move |_| do_connect());
    }
    {
        let do_connect = do_connect.clone();
        pass_entry.connect_activate(move |_| do_connect());
    }

    refill_recent(
        &recent_list,
        &recent_box,
        &recent_empty,
        &host_entry,
        &port_entry,
        &user_entry,
        Some(do_connect),
    );

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .build();
    scroller.add_css_class("metis-viewer-page");
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    scroller.set_opacity(1.0);
    root.append(&scroller);
    window.set_child(Some(&root));
    window.connect_map(|_| {
        theme::reapply();
    });
    window.present();
    host_entry.grab_focus();
}

fn missing_freerdp_banner() -> gtk::Box {
    let banner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    banner.add_css_class("metis-viewer-banner");
    let title = gtk::Label::new(Some(&tr("FreeRDP is required")));
    title.set_xalign(0.0);
    title.add_css_class("metis-viewer-banner-title");
    let body = gtk::Label::new(Some(&freerdp::freerdp_install_hint()));
    body.set_xalign(0.0);
    body.set_selectable(true);
    body.add_css_class("metis-viewer-banner-body");
    let alt = gtk::Label::new(Some(&tr(
        "Or install freerdp2-x11 if Wayland FreeRDP is unavailable. Select the \
         command above to copy it.",
    )));
    alt.set_xalign(0.0);
    alt.set_wrap(true);
    alt.add_css_class("metis-viewer-subtitle");
    alt.set_margin_top(6);
    banner.append(&title);
    banner.append(&body);
    banner.append(&alt);
    banner
}

fn field_box(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.add_css_class("metis-viewer-field");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.add_css_class("metis-viewer-field-label");
    col.append(&lbl);
    col.append(widget);
    col
}

enum StatusKind {
    Ok,
    Error,
}

fn set_status(label: &gtk::Label, text: &str, kind: StatusKind) {
    label.set_text(text);
    label.remove_css_class("error");
    label.remove_css_class("ok");
    label.remove_css_class("metis-viewer-ready");
    match kind {
        StatusKind::Error => label.add_css_class("error"),
        StatusKind::Ok => label.add_css_class("ok"),
    }
}

fn refill_recent(
    list: &gtk::ListBox,
    card: &gtk::Box,
    empty: &gtk::Label,
    host_entry: &gtk::Entry,
    port_entry: &gtk::Entry,
    user_entry: &gtk::Entry,
    on_connect: Option<ConnectFn>,
) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let cfg = metis_config::load_viewer_config();
    if cfg.recent.is_empty() {
        card.set_visible(false);
        empty.set_visible(true);
        return;
    }
    empty.set_visible(false);
    card.set_visible(true);

    for entry in cfg.recent {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);

        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        outer.add_css_class("metis-viewer-recent-row");

        let btn = gtk::Button::new();
        btn.set_has_frame(false);
        btn.set_hexpand(true);
        btn.set_tooltip_text(Some(&tr("Fill and connect")));
        let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        col.set_hexpand(true);
        let host_l = gtk::Label::new(Some(&format!("{}:{}", entry.host, entry.port)));
        host_l.set_xalign(0.0);
        host_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        host_l.add_css_class("metis-viewer-recent-host");
        let meta = if entry.username.is_empty() {
            tr("No username").to_string()
        } else {
            entry.username.clone()
        };
        let meta_l = gtk::Label::new(Some(&meta));
        meta_l.set_xalign(0.0);
        meta_l.add_css_class("metis-viewer-recent-meta");
        col.append(&host_l);
        col.append(&meta_l);
        btn.set_child(Some(&col));

        {
            let h = host_entry.clone();
            let p = port_entry.clone();
            let u = user_entry.clone();
            let e = entry.clone();
            let connect = on_connect.clone();
            btn.connect_clicked(move |_| {
                h.set_text(&e.host);
                p.set_text(&e.port.to_string());
                u.set_text(&e.username);
                if let Some(f) = &connect {
                    f();
                }
            });
        }
        outer.append(&btn);

        let trash = gtk::Button::from_icon_name("user-trash-symbolic");
        trash.set_has_frame(false);
        trash.set_tooltip_text(Some(&tr("Remove from recent")));
        trash.add_css_class("flat");
        {
            let list = list.clone();
            let card = card.clone();
            let empty = empty.clone();
            let host_entry = host_entry.clone();
            let port_entry = port_entry.clone();
            let user_entry = user_entry.clone();
            let e = entry.clone();
            let connect = on_connect.clone();
            trash.connect_clicked(move |_| {
                if let Err(err) = remove_recent(&e) {
                    tracing::warn!("viewer.json remove failed: {err}");
                }
                refill_recent(
                    &list,
                    &card,
                    &empty,
                    &host_entry,
                    &port_entry,
                    &user_entry,
                    connect.clone(),
                );
            });
        }
        outer.append(&trash);

        row.set_child(Some(&outer));
        list.append(&row);
    }
}
