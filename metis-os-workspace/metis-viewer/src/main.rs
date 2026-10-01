//! Metis Viewer — GTK4 RDP client over FreeRDP (hosts-first UI).

mod freerdp;
mod options;
mod theme;

use std::cell::RefCell;
use std::process::Child;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use metis_config::{ViewerHost, remember_host, remove_recent, set_viewer_pending_placement};
use metis_i18n::tr;
use options::OptionsUi;

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

    if effective_uid() == 0 {
        eprintln!(
            "metis-viewer: refuse to run as root.\n\
             Launch as your normal user (not sudo), e.g. metis-viewer\n\
             or Settings → Remote access → Connect with Metis Viewer…"
        );
        std::process::exit(1);
    }

    metis_i18n::init();
    theme::sync_gtk_theme_env();

    let prefill = parse_cli();

    let app = gtk::Application::builder()
        .application_id("com.metis.Viewer")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        build_ui(app, prefill.clone());
    });

    let argv0: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&argv0);
}

fn effective_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(2))
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
type RefreshFn = Rc<dyn Fn()>;

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
        .default_width(720)
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

    let freerdp_ok = freerdp::resolve_freerdp().is_some();

    let content_stack = gtk::Stack::new();
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);
    content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    content_stack.set_transition_duration(120);

    let (hosts_page, open_add_panel) = build_hosts_page(&prefill, freerdp_ok);
    content_stack.add_named(&hosts_page, Some("hosts"));
    content_stack.add_named(&build_settings_page(freerdp_ok), Some("settings"));
    content_stack.add_named(&build_about_page(), Some("about"));
    content_stack.set_visible_child_name("hosts");

    let rail = gtk::Box::new(gtk::Orientation::Vertical, 8);
    rail.add_css_class("metis-viewer-rail");
    rail.set_hexpand(false);
    rail.set_vexpand(true);

    let btn_hosts = rail_button("network-workgroup-symbolic", &tr("Hosts"), true);
    let btn_settings = rail_button("emblem-system-symbolic", &tr("Settings"), false);
    let btn_about = rail_button("help-about-symbolic", &tr("About"), false);
    btn_settings.set_group(Some(&btn_hosts));
    btn_about.set_group(Some(&btn_hosts));
    rail.append(&btn_hosts);
    rail.append(&btn_settings);
    rail.append(&btn_about);
    let rail_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rail_spacer.set_vexpand(true);
    rail.append(&rail_spacer);

    {
        let stack = content_stack.clone();
        btn_hosts.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name("hosts");
            }
        });
    }
    {
        let stack = content_stack.clone();
        btn_settings.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name("settings");
            }
        });
    }
    {
        let stack = content_stack.clone();
        btn_about.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name("about");
            }
        });
    }

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.add_css_class("metis-viewer-root");
    layout.set_hexpand(true);
    layout.set_vexpand(true);
    layout.set_opacity(1.0);
    layout.append(&rail);
    let sep = gtk::Separator::new(gtk::Orientation::Vertical);
    sep.add_css_class("metis-viewer-rail-sep");
    layout.append(&sep);
    layout.append(&content_stack);

    window.set_child(Some(&layout));
    window.connect_map(|_| {
        theme::reapply();
    });
    window.present();

    // CLI prefill → open the add panel with fields filled.
    if prefill.host.as_ref().is_some_and(|h| !h.trim().is_empty()) {
        open_add_panel();
    }
}

fn rail_button(icon: &str, tooltip: &str, active: bool) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("metis-viewer-rail-btn");
    btn.set_icon_name(icon);
    btn.set_tooltip_text(Some(tooltip));
    btn.set_active(active);
    btn
}

fn build_hosts_page(prefill: &CliPrefill, freerdp_ok: bool) -> (gtk::Widget, Rc<dyn Fn()>) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("metis-viewer-page");
    page.set_hexpand(true);
    page.set_vexpand(true);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toolbar.add_css_class("metis-viewer-toolbar");
    let heading = gtk::Label::new(Some(&tr("Hosts")));
    heading.set_xalign(0.0);
    heading.set_hexpand(true);
    heading.add_css_class("metis-viewer-page-title");
    toolbar.append(&heading);
    let add_btn = gtk::Button::with_label(&tr("Add host"));
    add_btn.add_css_class("suggested-action");
    add_btn.set_tooltip_text(Some(&tr("Show connection fields")));
    toolbar.append(&add_btn);
    page.append(&toolbar);

    // --- Slide-down connection panel ---
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("metis-viewer-card");
    panel.add_css_class("metis-viewer-add-panel");

    let panel_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    panel_header.add_css_class("metis-viewer-add-panel-header");
    let panel_title = gtk::Label::new(Some(&tr("New connection")));
    panel_title.set_xalign(0.0);
    panel_title.set_hexpand(true);
    panel_title.add_css_class("metis-viewer-card-title");
    panel_title.set_margin_top(0);
    panel_title.set_margin_bottom(0);
    panel_header.append(&panel_title);
    let cancel_btn = gtk::Button::with_label(&tr("Cancel"));
    cancel_btn.add_css_class("metis-viewer-secondary");
    panel_header.append(&cancel_btn);
    panel.append(&panel_header);

    if !freerdp_ok {
        panel.append(&missing_freerdp_banner());
    }

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
    panel.append(&host_port);

    let user_entry = gtk::Entry::new();
    user_entry.set_placeholder_text(Some(&tr("Username")));
    if let Some(u) = &prefill.user {
        user_entry.set_text(u);
    } else if let Ok(u) = std::env::var("USER")
        && !u.is_empty()
    {
        user_entry.set_text(&u);
    }
    panel.append(&field_box(&tr("Username"), &user_entry));

    let label_entry = gtk::Entry::new();
    label_entry.set_placeholder_text(Some(&tr("Optional display name")));
    label_entry.set_hexpand(true);
    panel.append(&field_box(&tr("Label"), &label_entry));

    let pass_entry = gtk::PasswordEntry::new();
    pass_entry.set_show_peek_icon(true);
    pass_entry.set_placeholder_text(Some(&tr("Optional")));
    panel.append(&field_box(&tr("Password"), &pass_entry));
    let pass_hint = gtk::Label::new(Some(&tr(
        "Leave blank to let FreeRDP prompt. Passwords are never saved.",
    )));
    pass_hint.set_xalign(0.0);
    pass_hint.set_wrap(true);
    pass_hint.add_css_class("metis-viewer-hint");
    panel.append(&pass_hint);

    let options_ui = Rc::new(OptionsUi::build());
    let options_header = gtk::Label::new(Some(&tr("Advanced Desktop Settings")));
    options_header.set_xalign(0.0);
    options_header.add_css_class("metis-viewer-card-title");
    panel.append(&options_header);
    panel.append(&options_ui.root);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-viewer-actions");
    actions.set_halign(gtk::Align::End);
    let save_btn = gtk::Button::with_label(&tr("Save"));
    save_btn.add_css_class("metis-viewer-secondary");
    let connect_btn = gtk::Button::with_label(&tr("Connect"));
    connect_btn.add_css_class("suggested-action");
    connect_btn.set_sensitive(freerdp_ok);
    actions.append(&save_btn);
    actions.append(&connect_btn);
    panel.append(&actions);

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(220);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(&panel));
    page.append(&revealer);

    // Always visible — card connects close the panel, so status must live outside it.
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("metis-viewer-status");
    status.set_visible(false);
    if !freerdp_ok {
        set_status(
            &status,
            &tr("Connect disabled — install FreeRDP first."),
            StatusKind::Error,
        );
    }
    page.append(&status);

    let hosts_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .build();
    hosts_scroll.add_css_class("metis-viewer-hosts-scroll");

    let hosts_inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
    hosts_inner.add_css_class("metis-viewer-hosts-body");
    hosts_inner.set_hexpand(true);

    let hosts_grid = gtk::FlowBox::new();
    hosts_grid.add_css_class("metis-viewer-hosts-grid");
    hosts_grid.set_selection_mode(gtk::SelectionMode::None);
    hosts_grid.set_homogeneous(true);
    hosts_grid.set_column_spacing(12);
    hosts_grid.set_row_spacing(12);
    hosts_grid.set_max_children_per_line(3);
    hosts_grid.set_min_children_per_line(1);
    hosts_grid.set_hexpand(true);
    hosts_inner.append(&hosts_grid);

    let hosts_empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
    hosts_empty.add_css_class("metis-viewer-empty-state");
    hosts_empty.set_halign(gtk::Align::Center);
    hosts_empty.set_valign(gtk::Align::Center);
    hosts_empty.set_hexpand(true);
    hosts_empty.set_vexpand(true);
    let empty_icon = gtk::Image::from_icon_name("network-workgroup-symbolic");
    empty_icon.set_pixel_size(48);
    empty_icon.add_css_class("metis-viewer-empty-icon");
    let empty_title = gtk::Label::new(Some(&tr("No saved hosts")));
    empty_title.add_css_class("metis-viewer-empty-title");
    let empty_body = gtk::Label::new(Some(&tr(
        "Add a host to connect with FreeRDP. Sharing is enabled on the remote \
         machine under Settings → Remote access.",
    )));
    empty_body.set_wrap(true);
    empty_body.set_justify(gtk::Justification::Center);
    empty_body.set_max_width_chars(40);
    empty_body.add_css_class("metis-viewer-empty");
    hosts_empty.append(&empty_icon);
    hosts_empty.append(&empty_title);
    hosts_empty.append(&empty_body);
    hosts_inner.append(&hosts_empty);

    hosts_scroll.set_child(Some(&hosts_inner));
    page.append(&hosts_scroll);

    let revealer_r = revealer.clone();
    let add_btn_r = add_btn.clone();
    let open_panel: Rc<dyn Fn()> = Rc::new({
        let revealer = revealer.clone();
        let add_btn = add_btn.clone();
        let host_entry = host_entry.clone();
        move || {
            revealer.set_reveal_child(true);
            add_btn.set_label(&tr("Hide"));
            host_entry.grab_focus();
        }
    });
    let close_panel: Rc<dyn Fn()> = Rc::new({
        let revealer = revealer.clone();
        let add_btn = add_btn.clone();
        move || {
            revealer.set_reveal_child(false);
            add_btn.set_label(&tr("Add host"));
        }
    });

    {
        let open_panel = open_panel.clone();
        let close_panel = close_panel.clone();
        add_btn.connect_clicked(move |_| {
            if revealer_r.is_child_revealed() {
                close_panel();
            } else {
                open_panel();
            }
        });
        let _ = &add_btn_r;
    }
    {
        let close_panel = close_panel.clone();
        cancel_btn.connect_clicked(move |_| close_panel());
    }

    let connect_busy = Rc::new(RefCell::new(false));
    let watched_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let connect_slot: Rc<RefCell<Option<ConnectFn>>> = Rc::new(RefCell::new(None));

    let refresh_hosts: RefreshFn = {
        let hosts_grid = hosts_grid.clone();
        let hosts_empty = hosts_empty.clone();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let connect_slot = connect_slot.clone();
        let open_panel = open_panel.clone();
        let options_ui = options_ui.clone();
        Rc::new(move || {
            let on_connect = connect_slot.borrow().clone();
            refill_hosts_grid(
                &hosts_grid,
                &hosts_empty,
                &host_entry,
                &port_entry,
                &user_entry,
                &label_entry,
                options_ui.clone(),
                on_connect,
                Some(open_panel.clone()),
            );
        })
    };

    let do_connect: ConnectFn = Rc::new({
        let connect_busy = connect_busy.clone();
        let connect_btn = connect_btn.clone();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let pass_entry = pass_entry.clone();
        let status = status.clone();
        let watched_child = watched_child.clone();
        let refresh_hosts = refresh_hosts.clone();
        let close_panel = close_panel.clone();
        let open_panel = open_panel.clone();
        let options_ui = options_ui.clone();

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
                open_panel();
                return;
            }
            *connect_busy.borrow_mut() = true;
            connect_btn.set_sensitive(false);

            let host = host_entry.text().to_string();
            let port_text = port_entry.text().to_string();
            let username = user_entry.text().to_string();
            let label = label_entry.text().to_string();
            let password = pass_entry.text().to_string();
            let options = options_ui.collect();

            let port: u16 = match port_text.trim().parse() {
                Ok(0) | Err(_) => {
                    set_status(
                        &status,
                        &tr("Enter a valid port (1–65535)."),
                        StatusKind::Error,
                    );
                    open_panel();
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
                open_panel();
                *connect_busy.borrow_mut() = false;
                connect_btn.set_sensitive(true);
                return;
            }

            set_viewer_pending_placement(options.placement);
            let req = freerdp::ConnectRequest {
                host: host.clone(),
                port,
                username: username.clone(),
                password: if password.is_empty() {
                    None
                } else {
                    Some(password)
                },
                options: options.clone(),
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
                        label: label.trim().to_string(),
                        options,
                    };
                    if let Err(e) = remember_host(entry) {
                        tracing::warn!("viewer.json save failed: {e}");
                    }
                    refresh_hosts();
                    close_panel();

                    let started = Instant::now();
                    *watched_child.borrow_mut() = Some(spawned.child);
                    let watched = watched_child.clone();
                    let status_watch = status.clone();
                    let busy = connect_busy.clone();
                    let btn = connect_btn.clone();
                    let reopen = open_panel.clone();
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
                                status_watch.set_visible(false);
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                            freerdp::EarlyWatch::Failed(msg) => {
                                *slot = None;
                                set_status(&status_watch, &msg, StatusKind::Error);
                                notify_desktop(&tr("Connection failed"), &msg, "critical");
                                reopen();
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                        }
                    });
                }
                Err(e) => {
                    set_status(&status, &e, StatusKind::Error);
                    notify_desktop(&tr("Connection failed"), &e, "critical");
                    open_panel();
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
    {
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let status = status.clone();
        let refresh_hosts = refresh_hosts.clone();
        let close_panel = close_panel.clone();
        let options_ui = options_ui.clone();
        save_btn.connect_clicked(move |_| {
            let host = host_entry.text();
            let port_text = port_entry.text();
            let username = user_entry.text();
            let label = label_entry.text();
            let port: u16 = match port_text.trim().parse() {
                Ok(0) | Err(_) => {
                    set_status(
                        &status,
                        &tr("Enter a valid port (1–65535)."),
                        StatusKind::Error,
                    );
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
                return;
            }
            let entry = ViewerHost {
                host: host.trim().to_string(),
                port,
                username: username.trim().to_string(),
                label: label.trim().to_string(),
                options: options_ui.collect(),
            };
            match remember_host(entry) {
                Ok(()) => {
                    set_status(&status, &tr("Host saved."), StatusKind::Ok);
                    refresh_hosts();
                    close_panel();
                }
                Err(e) => set_status(
                    &status,
                    &tr(&format!("Could not save host: {e}")),
                    StatusKind::Error,
                ),
            }
        });
    }
    for entry in [&host_entry, &port_entry, &user_entry] {
        let do_connect = do_connect.clone();
        entry.connect_activate(move |_| do_connect());
    }
    {
        let do_connect = do_connect.clone();
        pass_entry.connect_activate(move |_| do_connect());
    }

    refresh_hosts();

    (page.upcast(), open_panel)
}

fn build_settings_page(freerdp_ok: bool) -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 16);
    page.add_css_class("metis-viewer-page");
    page.set_margin_top(20);
    page.set_margin_bottom(24);
    page.set_margin_start(24);
    page.set_margin_end(24);

    let title = gtk::Label::new(Some(&tr("Settings")));
    title.set_xalign(0.0);
    title.add_css_class("metis-viewer-page-title");
    page.append(&title);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("metis-viewer-card");
    let card_title = gtk::Label::new(Some(&tr("FreeRDP")));
    card_title.set_xalign(0.0);
    card_title.add_css_class("metis-viewer-card-title");
    card.append(&card_title);

    let bin_label = gtk::Label::new(Some(&match freerdp::resolve_freerdp() {
        Some(p) => format!("{} {}", tr("Client:"), p.display()),
        None => tr("Client: not installed").to_string(),
    }));
    bin_label.set_xalign(0.0);
    bin_label.set_selectable(true);
    bin_label.add_css_class("metis-viewer-settings-value");
    card.append(&bin_label);

    if !freerdp_ok {
        let hint = gtk::Label::new(Some(&freerdp::freerdp_install_hint()));
        hint.set_xalign(0.0);
        hint.set_selectable(true);
        hint.add_css_class("metis-viewer-banner-body");
        card.append(&hint);
    }

    let notes = gtk::Label::new(Some(&tr(
        "Metis Viewer spawns FreeRDP with /cert:ignore and dynamic resolution. \
         Passwords are never written to viewer.json. Host sharing is configured \
         on the remote machine (Settings → Remote access).",
    )));
    notes.set_xalign(0.0);
    notes.set_wrap(true);
    notes.add_css_class("metis-viewer-hint");
    card.append(&notes);
    page.append(&card);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .vexpand(true)
        .hexpand(true)
        .build();
    scroll.upcast()
}

fn build_about_page() -> gtk::Widget {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 16);
    page.add_css_class("metis-viewer-page");
    page.set_margin_top(20);
    page.set_margin_bottom(24);
    page.set_margin_start(24);
    page.set_margin_end(24);

    let title = gtk::Label::new(Some(&tr("About")));
    title.set_xalign(0.0);
    title.add_css_class("metis-viewer-page-title");
    page.append(&title);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("metis-viewer-card");
    card.add_css_class("metis-viewer-about-card");

    let icon = gtk::Image::from_icon_name("network-workgroup-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("metis-viewer-header-icon");
    card.append(&icon);

    let name = gtk::Label::new(Some(&tr("Metis Viewer")));
    name.set_xalign(0.0);
    name.add_css_class("metis-viewer-title");
    card.append(&name);

    let blurb = gtk::Label::new(Some(&tr(
        "First-party RDP client for Metis. Connects to GNOME Remote Desktop or \
         the experimental Metis-native FreeRDP host using wlfreerdp / xfreerdp.",
    )));
    blurb.set_xalign(0.0);
    blurb.set_wrap(true);
    blurb.add_css_class("metis-viewer-subtitle");
    card.append(&blurb);

    let ver = gtk::Label::new(Some(&format!(
        "{} {}",
        tr("Version"),
        env!("CARGO_PKG_VERSION")
    )));
    ver.set_xalign(0.0);
    ver.add_css_class("metis-viewer-hint");
    card.append(&ver);

    page.append(&card);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .vexpand(true)
        .hexpand(true)
        .build();
    scroll.upcast()
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
    banner.append(&title);
    banner.append(&body);
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
    label.set_visible(true);
    label.remove_css_class("error");
    label.remove_css_class("ok");
    label.remove_css_class("metis-viewer-ready");
    match kind {
        StatusKind::Error => label.add_css_class("error"),
        StatusKind::Ok => label.add_css_class("ok"),
    }
}

/// Desktop notification via `notify-send` so Metis Notification Center picks it up.
fn notify_desktop(title: &str, body: &str, urgency: &str) {
    use std::process::{Command, Stdio};
    let sent = Command::new("notify-send")
        .args([
            "-a",
            "Metis Viewer",
            "-u",
            urgency,
            "--hint=string:desktop-entry:metis-viewer",
            "--icon=computer",
            title,
            body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if sent {
        return;
    }
    let app = gtk::Application::default();
    let note = gio::Notification::new(title);
    note.set_body(Some(body));
    note.set_priority(if urgency == "critical" {
        gio::NotificationPriority::Urgent
    } else {
        gio::NotificationPriority::Normal
    });
    app.send_notification(Some("metis-viewer-status"), &note);
}

#[allow(clippy::too_many_arguments)]
fn refill_hosts_grid(
    grid: &gtk::FlowBox,
    empty: &gtk::Box,
    host_entry: &gtk::Entry,
    port_entry: &gtk::Entry,
    user_entry: &gtk::Entry,
    label_entry: &gtk::Entry,
    options_ui: Rc<OptionsUi>,
    on_connect: Option<ConnectFn>,
    open_panel: Option<Rc<dyn Fn()>>,
) {
    while let Some(child) = grid.child_at_index(0) {
        grid.remove(&child);
    }
    let cfg = metis_config::load_viewer_config();
    if cfg.recent.is_empty() {
        grid.set_visible(false);
        empty.set_visible(true);
        return;
    }
    empty.set_visible(false);
    grid.set_visible(true);

    for entry in cfg.recent {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        card.add_css_class("metis-viewer-host-card");
        card.set_hexpand(true);

        let btn = gtk::Button::new();
        btn.set_has_frame(false);
        btn.set_hexpand(true);
        btn.set_tooltip_text(Some(&tr("Connect")));
        btn.add_css_class("metis-viewer-host-card-body");

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        body.set_hexpand(true);
        let icon_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        icon_wrap.add_css_class("metis-viewer-host-card-icon-wrap");
        icon_wrap.set_valign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("computer-symbolic");
        icon.set_pixel_size(22);
        icon.add_css_class("metis-viewer-host-card-icon");
        icon_wrap.append(&icon);
        body.append(&icon_wrap);

        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_hexpand(true);
        col.set_valign(gtk::Align::Center);
        let title = if entry.label.is_empty() {
            format!("{}:{}", entry.host, entry.port)
        } else {
            entry.label.clone()
        };
        let title_l = gtk::Label::new(Some(&title));
        title_l.set_xalign(0.0);
        title_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_l.add_css_class("metis-viewer-host-card-title");
        let endpoint = if entry.label.is_empty() {
            if entry.username.is_empty() {
                tr("No username").to_string()
            } else {
                entry.username.clone()
            }
        } else {
            let user = if entry.username.is_empty() {
                String::new()
            } else {
                format!(" · {}", entry.username)
            };
            format!("{}:{}{user}", entry.host, entry.port)
        };
        let meta_l = gtk::Label::new(Some(&endpoint));
        meta_l.set_xalign(0.0);
        meta_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        meta_l.add_css_class("metis-viewer-host-card-meta");
        col.append(&title_l);
        col.append(&meta_l);
        body.append(&col);
        btn.set_child(Some(&body));

        {
            let h = host_entry.clone();
            let p = port_entry.clone();
            let u = user_entry.clone();
            let l = label_entry.clone();
            let opts = options_ui.clone();
            let e = entry.clone();
            let connect = on_connect.clone();
            btn.connect_clicked(move |_| {
                h.set_text(&e.host);
                p.set_text(&e.port.to_string());
                u.set_text(&e.username);
                l.set_text(&e.label);
                opts.apply_host(&e);
                if let Some(f) = &connect {
                    f();
                }
            });
        }
        card.append(&btn);

        let edit = gtk::Button::from_icon_name("document-edit-symbolic");
        edit.set_has_frame(false);
        edit.set_tooltip_text(Some(&tr("Edit")));
        edit.add_css_class("flat");
        edit.add_css_class("metis-viewer-host-card-remove");
        edit.set_valign(gtk::Align::Start);
        {
            let h = host_entry.clone();
            let p = port_entry.clone();
            let u = user_entry.clone();
            let l = label_entry.clone();
            let opts = options_ui.clone();
            let e = entry.clone();
            let open = open_panel.clone();
            edit.connect_clicked(move |_| {
                h.set_text(&e.host);
                p.set_text(&e.port.to_string());
                u.set_text(&e.username);
                l.set_text(&e.label);
                opts.apply_host(&e);
                if let Some(f) = &open {
                    f();
                }
            });
        }
        card.append(&edit);

        let trash = gtk::Button::from_icon_name("user-trash-symbolic");
        trash.set_has_frame(false);
        trash.set_tooltip_text(Some(&tr("Remove")));
        trash.add_css_class("flat");
        trash.add_css_class("metis-viewer-host-card-remove");
        trash.set_valign(gtk::Align::Start);
        {
            let grid = grid.clone();
            let empty = empty.clone();
            let host_entry = host_entry.clone();
            let port_entry = port_entry.clone();
            let user_entry = user_entry.clone();
            let label_entry = label_entry.clone();
            let options_ui = options_ui.clone();
            let e = entry.clone();
            let connect = on_connect.clone();
            let open = open_panel.clone();
            trash.connect_clicked(move |_| {
                if let Err(err) = remove_recent(&e) {
                    tracing::warn!("viewer.json remove failed: {err}");
                }
                refill_hosts_grid(
                    &grid,
                    &empty,
                    &host_entry,
                    &port_entry,
                    &user_entry,
                    &label_entry,
                    options_ui.clone(),
                    connect.clone(),
                    open.clone(),
                );
            });
        }
        card.append(&trash);

        grid.append(&card);
    }
}
