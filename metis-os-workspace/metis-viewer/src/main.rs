//! Metis Viewer — GTK4 client for RDP (FreeRDP) and Metis Remote (RUDP).

mod credentials;
mod freerdp;
mod options;
mod rudp_audio;
mod rudp_session;
mod theme;

use std::cell::{Cell, RefCell};
use std::process::Child;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use metis_config::{
    ViewerHost, ViewerHostsView, ViewerProtocol, load_viewer_config, remember_host, remove_recent,
    save_hosts_view, set_viewer_pending_placement,
};
use metis_i18n::tr;
use options::OptionsUi;

#[derive(Debug, Clone, Default)]
struct CliPrefill {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    rudp: bool,
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
            "--rudp" => out.rudp = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metis-viewer [--rudp] [--host HOST] [--port PORT] [--user USER]\n\
                     RDP via FreeRDP (default) or Metis Remote with --rudp (UDP, default port 7843)."
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
type CancelFn = Rc<dyn Fn()>;
type CancelSlot = Rc<RefCell<Option<CancelFn>>>;
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

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some(&tr("Search hosts")));
    search_entry.set_tooltip_text(Some(&tr("Search hosts")));
    search_entry.add_css_class("metis-viewer-hosts-search");
    search_entry.set_width_chars(18);
    search_entry.set_max_width_chars(28);
    toolbar.append(&search_entry);

    let initial_view = load_viewer_config().hosts_view;
    let hosts_view = Rc::new(Cell::new(initial_view));
    let view_toggle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    view_toggle.add_css_class("metis-viewer-view-toggle");
    view_toggle.add_css_class("linked");
    let tile_btn = gtk::ToggleButton::new();
    tile_btn.set_icon_name("view-grid-symbolic");
    tile_btn.set_tooltip_text(Some(&tr("Tile view")));
    tile_btn.add_css_class("metis-viewer-view-btn");
    tile_btn.set_active(initial_view == ViewerHostsView::Tile);
    let list_btn = gtk::ToggleButton::new();
    list_btn.set_icon_name("view-list-symbolic");
    list_btn.set_tooltip_text(Some(&tr("List view")));
    list_btn.add_css_class("metis-viewer-view-btn");
    list_btn.set_active(initial_view == ViewerHostsView::List);
    list_btn.set_group(Some(&tile_btn));
    view_toggle.append(&tile_btn);
    view_toggle.append(&list_btn);
    toolbar.append(&view_toggle);

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

    let initial_protocol = if prefill.rudp {
        ViewerProtocol::Rudp
    } else {
        ViewerProtocol::Rdp
    };

    let freerdp_banner = missing_freerdp_banner();
    freerdp_banner.set_visible(!freerdp_ok && initial_protocol == ViewerProtocol::Rdp);
    panel.append(&freerdp_banner);

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
    port_entry.set_text(
        &prefill
            .port
            .unwrap_or_else(|| initial_protocol.default_port())
            .to_string(),
    );

    let protocol_dropdown =
        gtk::DropDown::from_strings(&[ViewerProtocol::Rdp.label(), ViewerProtocol::Rudp.label()]);
    protocol_dropdown.set_selected(match initial_protocol {
        ViewerProtocol::Rdp => 0,
        ViewerProtocol::Rudp => 1,
    });
    let label_entry = gtk::Entry::new();
    label_entry.set_placeholder_text(Some(&tr("Optional display name")));
    label_entry.set_hexpand(true);

    // Protocol | Label
    panel.append(&two_col_fields(
        icon_field(
            &tr("Protocol"),
            "network-workgroup-symbolic",
            &protocol_dropdown,
        ),
        icon_field(&tr("Label"), "tag-symbolic", &label_entry),
    ));

    // Host | Port
    panel.append(&two_col_fields(
        icon_field(&tr("Host"), "network-server-symbolic", &host_entry),
        icon_field(
            &tr("Port"),
            "network-transmit-receive-symbolic",
            &port_entry,
        ),
    ));

    let user_entry = gtk::Entry::new();
    user_entry.set_placeholder_text(Some(&tr("Username")));
    user_entry.set_hexpand(true);
    if let Some(u) = &prefill.user {
        user_entry.set_text(u);
    } else if let Ok(u) = std::env::var("USER")
        && !u.is_empty()
    {
        user_entry.set_text(&u);
    }

    let pass_entry = gtk::PasswordEntry::new();
    pass_entry.set_show_peek_icon(true);
    pass_entry.set_placeholder_text(Some(&tr("Optional")));
    pass_entry.set_hexpand(true);

    // Username | Password
    panel.append(&two_col_fields(
        icon_field(&tr("Username"), "avatar-default-symbolic", &user_entry),
        icon_field(&tr("Password"), "dialog-password-symbolic", &pass_entry),
    ));
    let pass_hint = gtk::Label::new(None);
    pass_hint.set_xalign(0.0);
    pass_hint.set_wrap(true);
    pass_hint.add_css_class("metis-viewer-hint");
    panel.append(&pass_hint);

    // When editing a saved host, migrate the keyring item if the endpoint changes.
    let editing_secret: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // Card being edited/reconnected — `None` means Add host (always insert new).
    let editing_host: Rc<RefCell<Option<ViewerHost>>> = Rc::new(RefCell::new(None));
    // True after the user types in the password field — async keyring fill must
    // never overwrite in-progress keystrokes (that looked like a 5s stutter).
    let password_dirty: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let suppress_pass_dirty: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    {
        let dirty = password_dirty.clone();
        let suppress = suppress_pass_dirty.clone();
        pass_entry.connect_changed(move |_| {
            if !suppress.get() {
                dirty.set(true);
            }
        });
    }

    // RDP-only: FreeRDP advanced notebook in a fixed-height scroller.
    // Do NOT wrap host/password fields in a propagate-natural-height scroll —
    // that remeasures the whole form on every keystroke and freezes typing.
    let options_ui = Rc::new(OptionsUi::build());
    let rdp_options = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rdp_options.add_css_class("metis-viewer-rdp-options");
    let options_header = gtk::Label::new(Some(&tr("Advanced Desktop Settings")));
    options_header.set_xalign(0.0);
    options_header.add_css_class("metis-viewer-card-title");
    rdp_options.append(&options_header);
    let options_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .build();
    options_scroll.add_css_class("metis-viewer-rdp-options-scroll");
    // Fixed height: scroll inside, don't grow the Viewer window.
    options_scroll.set_size_request(-1, 200);
    options_scroll.set_child(Some(&options_ui.root));
    rdp_options.append(&options_scroll);
    panel.append(&rdp_options);

    let rudp_hint = gtk::Label::new(Some(&tr(
        "Metis Remote uses your local PAM password. Video is hardware-encoded \
         on the host; no FreeRDP options apply.",
    )));
    rudp_hint.set_xalign(0.0);
    rudp_hint.set_wrap(true);
    rudp_hint.add_css_class("metis-viewer-hint");
    panel.append(&rudp_hint);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-viewer-actions");
    actions.set_halign(gtk::Align::End);
    let save_btn = gtk::Button::with_label(&tr("Save"));
    save_btn.add_css_class("metis-viewer-secondary");
    let connect_btn = gtk::Button::with_label(&tr("Connect"));
    connect_btn.add_css_class("suggested-action");
    actions.append(&save_btn);
    actions.append(&connect_btn);
    panel.append(&actions);

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(220);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(&panel));
    page.append(&revealer);

    let sync_protocol_ui = {
        let port_entry = port_entry.clone();
        let pass_hint = pass_hint.clone();
        let rdp_options = rdp_options.clone();
        let rudp_hint = rudp_hint.clone();
        let connect_btn = connect_btn.clone();
        let freerdp_banner = freerdp_banner.clone();
        Rc::new(move |proto: ViewerProtocol| {
            let cur = port_entry.text();
            if cur.is_empty() || cur.as_str() == "3389" || cur.as_str() == "7843" {
                port_entry.set_text(&proto.default_port().to_string());
            }
            match proto {
                ViewerProtocol::Rdp => {
                    pass_hint.set_text(&tr(
                        "Leave blank to use a saved keyring password or let FreeRDP prompt. \
                         Passwords are stored in your system keyring, never in viewer.json.",
                    ));
                    rdp_options.set_visible(true);
                    rudp_hint.set_visible(false);
                    freerdp_banner.set_visible(!freerdp_ok);
                    connect_btn.set_sensitive(freerdp_ok);
                }
                ViewerProtocol::Rudp => {
                    pass_hint.set_text(&tr(
                        "Local PAM password on the host. Stored in your system keyring \
                         (never in viewer.json).",
                    ));
                    rdp_options.set_visible(false);
                    rudp_hint.set_visible(true);
                    freerdp_banner.set_visible(false);
                    connect_btn.set_sensitive(true);
                }
            }
        })
    };
    sync_protocol_ui(initial_protocol);
    {
        let sync_protocol_ui = sync_protocol_ui.clone();
        protocol_dropdown.connect_selected_notify(move |dd| {
            let proto = if dd.selected() == 1 {
                ViewerProtocol::Rudp
            } else {
                ViewerProtocol::Rdp
            };
            sync_protocol_ui(proto);
        });
    }

    // Cancel / dismiss: while connecting, X aborts; otherwise it clears the banner.
    let cancel_action: CancelSlot = Rc::new(RefCell::new(None));
    // Greys out the matching host card while a connect is in flight.
    let connecting_key: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // Always visible — card connects close the panel, so status must live outside it.
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    status_row.add_css_class("metis-viewer-status-row");
    status_row.set_visible(false);
    let connect_spinner = gtk::Spinner::new();
    connect_spinner.set_visible(false);
    connect_spinner.set_valign(gtk::Align::Center);
    let status_banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_banner.add_css_class("metis-viewer-status-banner");
    status_banner.set_hexpand(true);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_hexpand(true);
    status.add_css_class("metis-viewer-status");
    let status_dismiss = gtk::Button::from_icon_name("window-close-symbolic");
    status_dismiss.set_has_frame(false);
    status_dismiss.set_tooltip_text(Some(&tr("Dismiss")));
    status_dismiss.add_css_class("flat");
    status_dismiss.add_css_class("metis-viewer-status-dismiss");
    status_dismiss.set_valign(gtk::Align::Start);
    status_dismiss.set_visible(false);
    status_banner.append(&status);
    status_banner.append(&status_dismiss);
    status_row.append(&connect_spinner);
    status_row.append(&status_banner);
    {
        let status_row = status_row.clone();
        let connect_spinner = connect_spinner.clone();
        let cancel_action = cancel_action.clone();
        status_dismiss.connect_clicked(move |btn| {
            if let Some(cancel) = cancel_action.borrow().clone() {
                cancel();
                return;
            }
            clear_status(&status_row, &connect_spinner);
            btn.set_visible(false);
        });
    }
    if !freerdp_ok {
        set_status(
            &status_row,
            &status_banner,
            &status,
            &connect_spinner,
            &status_dismiss,
            &tr("Connect disabled — install FreeRDP first."),
            StatusKind::Error,
        );
    }
    page.append(&status_row);

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

    let hosts_sections = gtk::Box::new(gtk::Orientation::Vertical, 16);
    hosts_sections.add_css_class("metis-viewer-hosts-sections");
    hosts_sections.set_hexpand(true);
    hosts_inner.append(&hosts_sections);

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
        "Add a host for Metis Remote or classic RDP. Metis Remote is enabled on \
         the host under Settings → Metis Remote; RDP under Remote access.",
    )));
    empty_body.set_wrap(true);
    empty_body.set_justify(gtk::Justification::Center);
    empty_body.set_max_width_chars(40);
    empty_body.add_css_class("metis-viewer-empty");
    hosts_empty.append(&empty_icon);
    hosts_empty.append(&empty_title);
    hosts_empty.append(&empty_body);
    hosts_inner.append(&hosts_empty);

    let hosts_no_match = gtk::Box::new(gtk::Orientation::Vertical, 8);
    hosts_no_match.add_css_class("metis-viewer-empty-state");
    hosts_no_match.set_halign(gtk::Align::Center);
    hosts_no_match.set_valign(gtk::Align::Center);
    hosts_no_match.set_hexpand(true);
    hosts_no_match.set_vexpand(true);
    hosts_no_match.set_visible(false);
    let no_match_title = gtk::Label::new(Some(&tr("No matching hosts")));
    no_match_title.add_css_class("metis-viewer-empty-title");
    let no_match_body = gtk::Label::new(Some(&tr("Try a different search.")));
    no_match_body.add_css_class("metis-viewer-empty");
    hosts_no_match.append(&no_match_title);
    hosts_no_match.append(&no_match_body);
    hosts_inner.append(&hosts_no_match);

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
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        move || {
            revealer.set_reveal_child(false);
            add_btn.set_label(&tr("Add host"));
            *editing_secret.borrow_mut() = None;
            *editing_host.borrow_mut() = None;
        }
    });

    {
        let open_panel = open_panel.clone();
        let close_panel = close_panel.clone();
        let pass_entry = pass_entry.clone();
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        let password_dirty = password_dirty.clone();
        let suppress_pass_dirty = suppress_pass_dirty.clone();
        add_btn.connect_clicked(move |_| {
            if revealer_r.is_child_revealed() {
                close_panel();
            } else {
                // Fresh "Add host" — don't keep a previous edit's password.
                *editing_secret.borrow_mut() = None;
                *editing_host.borrow_mut() = None;
                password_dirty.set(false);
                suppress_pass_dirty.set(true);
                pass_entry.set_text("");
                suppress_pass_dirty.set(false);
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
    let rudp_cancel: Rc<RefCell<Option<rudp_session::RudpConnectCancel>>> =
        Rc::new(RefCell::new(None));

    let refresh_hosts: RefreshFn = {
        let hosts_sections = hosts_sections.clone();
        let hosts_empty = hosts_empty.clone();
        let hosts_no_match = hosts_no_match.clone();
        let search_entry = search_entry.clone();
        let hosts_view = hosts_view.clone();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let pass_entry = pass_entry.clone();
        let protocol_dropdown = protocol_dropdown.clone();
        let connect_slot = connect_slot.clone();
        let open_panel = open_panel.clone();
        let options_ui = options_ui.clone();
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        let password_dirty = password_dirty.clone();
        let suppress_pass_dirty = suppress_pass_dirty.clone();
        let connecting_key = connecting_key.clone();
        Rc::new(move || {
            let on_connect = connect_slot.borrow().clone();
            let connecting = connecting_key.borrow().clone();
            refill_hosts_list(
                &hosts_sections,
                &hosts_empty,
                &hosts_no_match,
                &search_entry.text(),
                hosts_view.get(),
                &host_entry,
                &port_entry,
                &user_entry,
                &label_entry,
                &pass_entry,
                &protocol_dropdown,
                options_ui.clone(),
                editing_secret.clone(),
                editing_host.clone(),
                password_dirty.clone(),
                suppress_pass_dirty.clone(),
                on_connect,
                Some(open_panel.clone()),
                connecting.as_deref(),
            );
        })
    };

    {
        let refresh_hosts = refresh_hosts.clone();
        search_entry.connect_search_changed(move |_| refresh_hosts());
    }
    {
        let hosts_view = hosts_view.clone();
        let refresh_hosts = refresh_hosts.clone();
        let tile_btn = tile_btn.clone();
        tile_btn.connect_toggled(move |btn| {
            if !btn.is_active() {
                return;
            }
            hosts_view.set(ViewerHostsView::Tile);
            if let Err(err) = save_hosts_view(ViewerHostsView::Tile) {
                tracing::warn!("viewer.json hosts_view save failed: {err}");
            }
            refresh_hosts();
        });
    }
    {
        let hosts_view = hosts_view.clone();
        let refresh_hosts = refresh_hosts.clone();
        list_btn.connect_toggled(move |btn| {
            if !btn.is_active() {
                return;
            }
            hosts_view.set(ViewerHostsView::List);
            if let Err(err) = save_hosts_view(ViewerHostsView::List) {
                tracing::warn!("viewer.json hosts_view save failed: {err}");
            }
            refresh_hosts();
        });
    }

    let do_connect: ConnectFn = Rc::new({
        let connect_busy = connect_busy.clone();
        let connect_btn = connect_btn.clone();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let pass_entry = pass_entry.clone();
        let status = status.clone();
        let status_row = status_row.clone();
        let status_banner = status_banner.clone();
        let connect_spinner = connect_spinner.clone();
        let status_dismiss = status_dismiss.clone();
        let watched_child = watched_child.clone();
        let refresh_hosts = refresh_hosts.clone();
        let close_panel = close_panel.clone();
        let open_panel = open_panel.clone();
        let options_ui = options_ui.clone();
        let protocol_dropdown = protocol_dropdown.clone();
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        let password_dirty = password_dirty.clone();
        let suppress_pass_dirty = suppress_pass_dirty.clone();
        let connecting_key = connecting_key.clone();
        let cancel_action = cancel_action.clone();
        let rudp_cancel = rudp_cancel.clone();

        move || {
            if *connect_busy.borrow() {
                return;
            }
            let protocol = if protocol_dropdown.selected() == 1 {
                ViewerProtocol::Rudp
            } else {
                ViewerProtocol::Rdp
            };
            if protocol == ViewerProtocol::Rdp && freerdp::resolve_freerdp().is_none() {
                set_status(
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
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
            let mut password = pass_entry.text().to_string();
            let options = options_ui.collect();

            let port: u16 = match port_text.trim().parse() {
                Ok(0) | Err(_) => {
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
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
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
                    &tr("Enter a host name or IP address."),
                    StatusKind::Error,
                );
                open_panel();
                *connect_busy.borrow_mut() = false;
                connect_btn.set_sensitive(true);
                return;
            }

            let entry = ViewerHost {
                host: host.trim().to_string(),
                port,
                username: username.trim().to_string(),
                label: label.trim().to_string(),
                protocol,
                options: options.clone(),
            };
            let account = credentials::secret_account(&entry);
            if password.is_empty()
                && let Some(saved) = credentials::cached_password(&account)
            {
                password = saved;
                suppress_pass_dirty.set(true);
                pass_entry.set_text(&password);
                suppress_pass_dirty.set(false);
                password_dirty.set(false);
            }
            let previous_secret = editing_secret.borrow().clone();
            let previous_host = editing_host.borrow().clone();
            if let Err(e) =
                credentials::save_host_password(&account, &password, previous_secret.as_deref())
            {
                tracing::warn!("viewer keyring save failed: {e}");
            }
            *editing_secret.borrow_mut() = Some(account.clone());
            if let Err(e) = remember_host(entry.clone(), previous_host.as_ref()) {
                tracing::warn!("viewer.json save failed: {e}");
            }
            *editing_host.borrow_mut() = Some(entry.clone());
            *connecting_key.borrow_mut() = Some(account.clone());
            refresh_hosts();

            if protocol == ViewerProtocol::Rudp {
                if username.trim().is_empty() {
                    *connecting_key.borrow_mut() = None;
                    refresh_hosts();
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &tr("Enter a username for Metis Remote."),
                        StatusKind::Error,
                    );
                    open_panel();
                    *connect_busy.borrow_mut() = false;
                    connect_btn.set_sensitive(true);
                    return;
                }
                if password.is_empty() {
                    *connecting_key.borrow_mut() = None;
                    refresh_hosts();
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &tr("Enter the host PAM password for Metis Remote."),
                        StatusKind::Error,
                    );
                    open_panel();
                    *connect_busy.borrow_mut() = false;
                    connect_btn.set_sensitive(true);
                    return;
                }
                let Some(parent) = host_entry.root().and_downcast::<gtk::Window>() else {
                    *connecting_key.borrow_mut() = None;
                    refresh_hosts();
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &tr("Could not open session window."),
                        StatusKind::Error,
                    );
                    *connect_busy.borrow_mut() = false;
                    connect_btn.set_sensitive(true);
                    return;
                };

                set_connecting(
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
                    &format!(
                        "{} {}…",
                        tr("Connecting to"),
                        connect_target_display(&label, host.trim(), port)
                    ),
                );
                // Keep the edit panel closed for card-click connect; status lives
                // outside it. Only Edit / Add host / validation gaps open the panel.

                let status_cb = status.clone();
                let status_row_cb = status_row.clone();
                let status_banner_cb = status_banner.clone();
                let spinner_cb = connect_spinner.clone();
                let status_dismiss_cb = status_dismiss.clone();
                let busy = connect_busy.clone();
                let btn = connect_btn.clone();
                let close = close_panel.clone();
                let connecting_key_cb = connecting_key.clone();
                let refresh_cb = refresh_hosts.clone();
                let cancel_action_cb = cancel_action.clone();
                let rudp_cancel_cb = rudp_cancel.clone();
                let handle = rudp_session::open_rudp_session(
                    &parent,
                    host.trim().to_string(),
                    port,
                    username.trim().to_string(),
                    password,
                    move |outcome| {
                        *rudp_cancel_cb.borrow_mut() = None;
                        *cancel_action_cb.borrow_mut() = None;
                        *connecting_key_cb.borrow_mut() = None;
                        refresh_cb();
                        *busy.borrow_mut() = false;
                        btn.set_sensitive(true);
                        match outcome {
                            Ok(()) => {
                                clear_status(&status_row_cb, &spinner_cb);
                                close();
                            }
                            Err(msg) if rudp_session::is_cancelled_message(&msg) => {
                                clear_status(&status_row_cb, &spinner_cb);
                            }
                            Err(msg) => {
                                set_status(
                                    &status_row_cb,
                                    &status_banner_cb,
                                    &status_cb,
                                    &spinner_cb,
                                    &status_dismiss_cb,
                                    &msg,
                                    StatusKind::Error,
                                );
                                notify_desktop(&tr("Connection failed"), &msg, "critical");
                            }
                        }
                    },
                );
                *rudp_cancel.borrow_mut() = handle;
                {
                    let rudp_cancel = rudp_cancel.clone();
                    *cancel_action.borrow_mut() = Some(Rc::new(move || {
                        if let Some(h) = rudp_cancel.borrow().as_ref() {
                            h.cancel();
                        }
                    }) as Rc<dyn Fn()>);
                }
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

            let target = connect_target_display(&label, host.trim(), port);
            set_connecting(
                &status_row,
                &status_banner,
                &status,
                &connect_spinner,
                &status_dismiss,
                &format!("{} {target}…", tr("Connecting to")),
            );

            match freerdp::spawn_freerdp(req) {
                Ok(spawned) => {
                    set_connecting(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &format!(
                            "{} {target} ({})",
                            tr("Connecting to"),
                            spawned.binary.display()
                        ),
                    );
                    close_panel();

                    let started = Instant::now();
                    *watched_child.borrow_mut() = Some(spawned.child);
                    let rdp_cancelled = Rc::new(Cell::new(false));
                    {
                        let watched = watched_child.clone();
                        let cancelled = rdp_cancelled.clone();
                        let busy = connect_busy.clone();
                        let btn = connect_btn.clone();
                        let connecting_key = connecting_key.clone();
                        let refresh_hosts = refresh_hosts.clone();
                        let status_row = status_row.clone();
                        let connect_spinner = connect_spinner.clone();
                        let cancel_slot = cancel_action.clone();
                        let cancel_clear = cancel_action.clone();
                        *cancel_slot.borrow_mut() = Some(Rc::new(move || {
                            cancelled.set(true);
                            if let Some(mut child) = watched.borrow_mut().take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            *cancel_clear.borrow_mut() = None;
                            *connecting_key.borrow_mut() = None;
                            refresh_hosts();
                            clear_status(&status_row, &connect_spinner);
                            *busy.borrow_mut() = false;
                            btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                        })
                            as Rc<dyn Fn()>);
                    }
                    let watched = watched_child.clone();
                    let status_watch = status.clone();
                    let status_row_watch = status_row.clone();
                    let status_banner_watch = status_banner.clone();
                    let spinner_watch = connect_spinner.clone();
                    let status_dismiss_watch = status_dismiss.clone();
                    let busy = connect_busy.clone();
                    let btn = connect_btn.clone();
                    let connecting_key_watch = connecting_key.clone();
                    let refresh_watch = refresh_hosts.clone();
                    let cancel_action_watch = cancel_action.clone();
                    let rdp_cancelled = rdp_cancelled.clone();
                    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                        let mut slot = watched.borrow_mut();
                        let Some(child) = slot.as_mut() else {
                            *cancel_action_watch.borrow_mut() = None;
                            *connecting_key_watch.borrow_mut() = None;
                            refresh_watch();
                            *busy.borrow_mut() = false;
                            btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                            return glib::ControlFlow::Break;
                        };
                        match freerdp::poll_early_failure(child, started) {
                            freerdp::EarlyWatch::Running => glib::ControlFlow::Continue,
                            freerdp::EarlyWatch::Done => {
                                *slot = None;
                                *cancel_action_watch.borrow_mut() = None;
                                *connecting_key_watch.borrow_mut() = None;
                                refresh_watch();
                                clear_status(&status_row_watch, &spinner_watch);
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                            freerdp::EarlyWatch::Failed(msg) => {
                                *slot = None;
                                *cancel_action_watch.borrow_mut() = None;
                                *connecting_key_watch.borrow_mut() = None;
                                refresh_watch();
                                if rdp_cancelled.get() {
                                    clear_status(&status_row_watch, &spinner_watch);
                                } else {
                                    set_status(
                                        &status_row_watch,
                                        &status_banner_watch,
                                        &status_watch,
                                        &spinner_watch,
                                        &status_dismiss_watch,
                                        &msg,
                                        StatusKind::Error,
                                    );
                                    notify_desktop(&tr("Connection failed"), &msg, "critical");
                                }
                                *busy.borrow_mut() = false;
                                btn.set_sensitive(freerdp::resolve_freerdp().is_some());
                                glib::ControlFlow::Break
                            }
                        }
                    });
                }
                Err(e) => {
                    *connecting_key.borrow_mut() = None;
                    *cancel_action.borrow_mut() = None;
                    refresh_hosts();
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &e,
                        StatusKind::Error,
                    );
                    notify_desktop(&tr("Connection failed"), &e, "critical");
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
        let status_row = status_row.clone();
        let status_banner = status_banner.clone();
        let connect_spinner = connect_spinner.clone();
        let status_dismiss = status_dismiss.clone();
        let refresh_hosts = refresh_hosts.clone();
        let close_panel = close_panel.clone();
        let options_ui = options_ui.clone();
        let protocol_dropdown = protocol_dropdown.clone();
        let pass_entry = pass_entry.clone();
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        save_btn.connect_clicked(move |_| {
            let host = host_entry.text();
            let port_text = port_entry.text();
            let username = user_entry.text();
            let label = label_entry.text();
            let password = pass_entry.text().to_string();
            let port: u16 = match port_text.trim().parse() {
                Ok(0) | Err(_) => {
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &tr("Enter a valid port (1–65535)."),
                        StatusKind::Error,
                    );
                    return;
                }
                Ok(p) => p,
            };
            if host.trim().is_empty() {
                set_status(
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
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
                protocol: if protocol_dropdown.selected() == 1 {
                    ViewerProtocol::Rudp
                } else {
                    ViewerProtocol::Rdp
                },
                options: options_ui.collect(),
            };
            let account = credentials::secret_account(&entry);
            let previous_secret = editing_secret.borrow().clone();
            let previous_host = editing_host.borrow().clone();
            if let Err(e) =
                credentials::save_host_password(&account, &password, previous_secret.as_deref())
            {
                set_status(
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
                    &tr(&format!("Could not save password to keyring: {e}")),
                    StatusKind::Error,
                );
                return;
            }
            match remember_host(entry.clone(), previous_host.as_ref()) {
                Ok(()) => {
                    *editing_secret.borrow_mut() = Some(account);
                    *editing_host.borrow_mut() = Some(entry);
                    set_status(
                        &status_row,
                        &status_banner,
                        &status,
                        &connect_spinner,
                        &status_dismiss,
                        &tr("Host saved (password in system keyring)."),
                        StatusKind::Ok,
                    );
                    refresh_hosts();
                    close_panel();
                }
                Err(e) => set_status(
                    &status_row,
                    &status_banner,
                    &status,
                    &connect_spinner,
                    &status_dismiss,
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
         Passwords are kept in your system keyring, never in viewer.json. Host sharing is configured \
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
         the Metis-native RDP host (metis-rdp-host) using wlfreerdp / xfreerdp.",
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

fn field_label(label: &str, icon: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("metis-viewer-field-label-row");
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(14);
    img.add_css_class("metis-viewer-field-icon");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.add_css_class("metis-viewer-field-label");
    row.append(&img);
    row.append(&lbl);
    row
}

fn icon_field(label: &str, icon: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.add_css_class("metis-viewer-field");
    col.set_hexpand(true);
    col.append(&field_label(label, icon));
    col.append(widget);
    col
}

/// Two equal columns sharing one padded field row.
fn two_col_fields(left: gtk::Box, right: gtk::Box) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-viewer-field-row");
    left.remove_css_class("metis-viewer-field");
    right.remove_css_class("metis-viewer-field");
    left.add_css_class("metis-viewer-field-col");
    right.add_css_class("metis-viewer-field-col");
    left.set_hexpand(true);
    right.set_hexpand(true);
    row.append(&left);
    row.append(&right);
    row
}

enum StatusKind {
    Ok,
    Error,
}

fn stop_connect_spinner(spinner: &gtk::Spinner) {
    spinner.stop();
    spinner.set_visible(false);
}

fn clear_banner_classes(banner: &gtk::Box, label: &gtk::Label) {
    banner.remove_css_class("error");
    banner.remove_css_class("ok");
    label.remove_css_class("error");
    label.remove_css_class("ok");
    label.remove_css_class("metis-viewer-ready");
}

fn set_connecting(
    row: &gtk::Box,
    banner: &gtk::Box,
    label: &gtk::Label,
    spinner: &gtk::Spinner,
    dismiss: &gtk::Button,
    text: &str,
) {
    spinner.set_visible(true);
    spinner.start();
    dismiss.set_tooltip_text(Some(&tr("Cancel connection")));
    dismiss.set_visible(true);
    label.set_text(text);
    row.set_visible(true);
    clear_banner_classes(banner, label);
    banner.add_css_class("ok");
    label.add_css_class("ok");
}

fn set_status(
    row: &gtk::Box,
    banner: &gtk::Box,
    label: &gtk::Label,
    spinner: &gtk::Spinner,
    dismiss: &gtk::Button,
    text: &str,
    kind: StatusKind,
) {
    stop_connect_spinner(spinner);
    dismiss.set_tooltip_text(Some(&tr("Dismiss")));
    dismiss.set_visible(true);
    label.set_text(text);
    row.set_visible(true);
    clear_banner_classes(banner, label);
    match kind {
        StatusKind::Error => {
            banner.add_css_class("error");
            label.add_css_class("error");
        }
        StatusKind::Ok => {
            banner.add_css_class("ok");
            label.add_css_class("ok");
        }
    }
}

fn clear_status(row: &gtk::Box, spinner: &gtk::Spinner) {
    stop_connect_spinner(spinner);
    row.set_visible(false);
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
fn apply_host_to_form(
    entry: &ViewerHost,
    host_entry: &gtk::Entry,
    port_entry: &gtk::Entry,
    user_entry: &gtk::Entry,
    label_entry: &gtk::Entry,
    pass_entry: &gtk::PasswordEntry,
    protocol_dropdown: &gtk::DropDown,
    options_ui: &OptionsUi,
    editing_secret: &Rc<RefCell<Option<String>>>,
    editing_host: &Rc<RefCell<Option<ViewerHost>>>,
    password_dirty: &Rc<Cell<bool>>,
    suppress_pass_dirty: &Rc<Cell<bool>>,
) {
    host_entry.set_text(&entry.host);
    port_entry.set_text(&entry.port.to_string());
    user_entry.set_text(&entry.username);
    label_entry.set_text(&entry.label);
    protocol_dropdown.set_selected(match entry.protocol {
        ViewerProtocol::Rdp => 0,
        ViewerProtocol::Rudp => 1,
    });
    options_ui.apply_host(entry);
    let account = credentials::secret_account(entry);
    *editing_secret.borrow_mut() = Some(account.clone());
    *editing_host.borrow_mut() = Some(entry.clone());
    // Clear without marking dirty, then fill from cache/keyring off-thread.
    password_dirty.set(false);
    suppress_pass_dirty.set(true);
    pass_entry.set_text("");
    suppress_pass_dirty.set(false);
    credentials::fill_password_async(
        pass_entry.clone(),
        account,
        password_dirty.clone(),
        editing_secret.clone(),
        suppress_pass_dirty.clone(),
    );
}

fn host_matches_search(entry: &ViewerHost, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let hay = [
        entry.label.as_str(),
        entry.host.as_str(),
        entry.username.as_str(),
        entry.protocol.label(),
    ]
    .join(" ")
    .to_lowercase();
    hay.contains(&q)
}

/// Status-line target: `"Label (host:port)"` when a label is set, else `"host:port"`.
fn connect_target_display(label: &str, host: &str, port: u16) -> String {
    let endpoint = format!("{host}:{port}");
    let label = label.trim();
    if label.is_empty() || label.eq_ignore_ascii_case(&endpoint) || label.eq_ignore_ascii_case(host)
    {
        endpoint
    } else {
        format!("{label} ({endpoint})")
    }
}

fn host_title_meta(entry: &ViewerHost) -> (String, String) {
    let title = if entry.label.is_empty() {
        format!("{}:{}", entry.host, entry.port)
    } else {
        entry.label.clone()
    };
    let proto = entry.protocol.label();
    let meta = if entry.label.is_empty() {
        if entry.username.is_empty() {
            proto.to_string()
        } else {
            format!("{proto} · {}", entry.username)
        }
    } else {
        let user = if entry.username.is_empty() {
            String::new()
        } else {
            format!(" · {}", entry.username)
        };
        format!("{proto} · {}:{}{user}", entry.host, entry.port)
    };
    (title, meta)
}

#[allow(clippy::too_many_arguments)]
fn refill_hosts_list(
    sections: &gtk::Box,
    empty: &gtk::Box,
    no_match: &gtk::Box,
    search: &str,
    view: ViewerHostsView,
    host_entry: &gtk::Entry,
    port_entry: &gtk::Entry,
    user_entry: &gtk::Entry,
    label_entry: &gtk::Entry,
    pass_entry: &gtk::PasswordEntry,
    protocol_dropdown: &gtk::DropDown,
    options_ui: Rc<OptionsUi>,
    editing_secret: Rc<RefCell<Option<String>>>,
    editing_host: Rc<RefCell<Option<ViewerHost>>>,
    password_dirty: Rc<Cell<bool>>,
    suppress_pass_dirty: Rc<Cell<bool>>,
    on_connect: Option<ConnectFn>,
    open_panel: Option<Rc<dyn Fn()>>,
    connecting_key: Option<&str>,
) {
    while let Some(child) = sections.first_child() {
        sections.remove(&child);
    }

    let cfg = load_viewer_config();
    if cfg.recent.is_empty() {
        sections.set_visible(false);
        empty.set_visible(true);
        no_match.set_visible(false);
        return;
    }

    // Warm the session password cache in the background so Edit/Connect
    // do not stall the GTK loop on Secret Service.
    for entry in &cfg.recent {
        let account = credentials::secret_account(entry);
        if credentials::cached_password(&account).is_none() {
            let _ = std::thread::Builder::new()
                .name("metis-viewer-keyring-warm".into())
                .spawn(move || {
                    let _ = credentials::load_password_blocking(&account);
                });
        }
    }

    let filtered: Vec<ViewerHost> = cfg
        .recent
        .into_iter()
        .filter(|e| host_matches_search(e, search))
        .collect();
    if filtered.is_empty() {
        sections.set_visible(false);
        empty.set_visible(false);
        no_match.set_visible(true);
        return;
    }

    empty.set_visible(false);
    no_match.set_visible(false);
    sections.set_visible(true);

    let groups: [(ViewerProtocol, &str); 2] = [
        (ViewerProtocol::Rudp, ViewerProtocol::Rudp.label()),
        (ViewerProtocol::Rdp, ViewerProtocol::Rdp.label()),
    ];

    for (proto, section_title) in groups {
        let group: Vec<ViewerHost> = filtered
            .iter()
            .filter(|e| e.protocol == proto)
            .cloned()
            .collect();
        if group.is_empty() {
            continue;
        }

        let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
        section.add_css_class("metis-viewer-hosts-section");
        let heading = gtk::Label::new(Some(section_title));
        heading.set_xalign(0.0);
        heading.add_css_class("metis-viewer-hosts-section-title");
        section.append(&heading);

        let items_parent: gtk::Widget = match view {
            ViewerHostsView::Tile => {
                let grid = gtk::FlowBox::new();
                grid.add_css_class("metis-viewer-hosts-grid");
                grid.set_selection_mode(gtk::SelectionMode::None);
                grid.set_homogeneous(true);
                grid.set_column_spacing(12);
                grid.set_row_spacing(12);
                grid.set_max_children_per_line(3);
                grid.set_min_children_per_line(1);
                grid.set_hexpand(true);
                grid.upcast()
            }
            ViewerHostsView::List => {
                let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
                list.add_css_class("metis-viewer-hosts-list");
                list.set_hexpand(true);
                list.upcast()
            }
        };

        for entry in group {
            let card = build_host_card(
                &entry,
                view,
                host_entry,
                port_entry,
                user_entry,
                label_entry,
                pass_entry,
                protocol_dropdown,
                options_ui.clone(),
                editing_secret.clone(),
                editing_host.clone(),
                password_dirty.clone(),
                suppress_pass_dirty.clone(),
                on_connect.clone(),
                open_panel.clone(),
                sections,
                empty,
                no_match,
                search,
                connecting_key,
            );
            if let Some(grid) = items_parent.downcast_ref::<gtk::FlowBox>() {
                grid.append(&card);
            } else if let Some(list) = items_parent.downcast_ref::<gtk::Box>() {
                list.append(&card);
            }
        }
        section.append(&items_parent);
        sections.append(&section);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_host_card(
    entry: &ViewerHost,
    view: ViewerHostsView,
    host_entry: &gtk::Entry,
    port_entry: &gtk::Entry,
    user_entry: &gtk::Entry,
    label_entry: &gtk::Entry,
    pass_entry: &gtk::PasswordEntry,
    protocol_dropdown: &gtk::DropDown,
    options_ui: Rc<OptionsUi>,
    editing_secret: Rc<RefCell<Option<String>>>,
    editing_host: Rc<RefCell<Option<ViewerHost>>>,
    password_dirty: Rc<Cell<bool>>,
    suppress_pass_dirty: Rc<Cell<bool>>,
    on_connect: Option<ConnectFn>,
    open_panel: Option<Rc<dyn Fn()>>,
    sections: &gtk::Box,
    empty: &gtk::Box,
    no_match: &gtk::Box,
    search: &str,
    connecting_key: Option<&str>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    match view {
        ViewerHostsView::Tile => card.add_css_class("metis-viewer-host-card"),
        ViewerHostsView::List => card.add_css_class("metis-viewer-host-row"),
    }
    card.set_hexpand(true);
    let is_connecting = connecting_key.is_some_and(|k| k == credentials::secret_account(entry));
    if is_connecting {
        card.add_css_class("connecting");
    }

    let btn = gtk::Button::new();
    btn.set_has_frame(false);
    btn.set_hexpand(true);
    btn.set_tooltip_text(Some(&tr("Connect")));
    btn.add_css_class("metis-viewer-host-card-body");
    btn.set_sensitive(!is_connecting);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body.set_hexpand(true);
    let icon_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon_wrap.add_css_class("metis-viewer-host-card-icon-wrap");
    icon_wrap.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(entry.protocol.icon_name());
    icon.set_pixel_size(if view == ViewerHostsView::List {
        18
    } else {
        22
    });
    icon.add_css_class("metis-viewer-host-card-icon");
    icon_wrap.append(&icon);
    body.append(&icon_wrap);

    let (title, meta) = host_title_meta(entry);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_hexpand(true);
    col.set_valign(gtk::Align::Center);
    let title_l = gtk::Label::new(Some(&title));
    title_l.set_xalign(0.0);
    title_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_l.add_css_class("metis-viewer-host-card-title");
    let meta_l = gtk::Label::new(Some(&meta));
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
        let pw = pass_entry.clone();
        let proto_dd = protocol_dropdown.clone();
        let opts = options_ui.clone();
        let editing = editing_secret.clone();
        let editing_host_ref = editing_host.clone();
        let dirty = password_dirty.clone();
        let suppress = suppress_pass_dirty.clone();
        let e = entry.clone();
        let connect = on_connect.clone();
        btn.connect_clicked(move |_| {
            apply_host_to_form(
                &e,
                &h,
                &p,
                &u,
                &l,
                &pw,
                &proto_dd,
                &opts,
                &editing,
                &editing_host_ref,
                &dirty,
                &suppress,
            );
            let account = credentials::secret_account(&e);
            if pw.text().is_empty()
                && let Some(saved) = credentials::cached_password(&account)
            {
                suppress.set(true);
                pw.set_text(&saved);
                suppress.set(false);
                dirty.set(false);
            }
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
    edit.set_valign(gtk::Align::Center);
    edit.set_sensitive(!is_connecting);
    {
        let h = host_entry.clone();
        let p = port_entry.clone();
        let u = user_entry.clone();
        let l = label_entry.clone();
        let pw = pass_entry.clone();
        let proto_dd = protocol_dropdown.clone();
        let opts = options_ui.clone();
        let editing = editing_secret.clone();
        let editing_host_ref = editing_host.clone();
        let dirty = password_dirty.clone();
        let suppress = suppress_pass_dirty.clone();
        let e = entry.clone();
        let open = open_panel.clone();
        edit.connect_clicked(move |_| {
            apply_host_to_form(
                &e,
                &h,
                &p,
                &u,
                &l,
                &pw,
                &proto_dd,
                &opts,
                &editing,
                &editing_host_ref,
                &dirty,
                &suppress,
            );
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
    trash.set_valign(gtk::Align::Center);
    trash.set_sensitive(!is_connecting);
    let connecting_owned = connecting_key.map(str::to_string);
    {
        let sections = sections.clone();
        let empty = empty.clone();
        let no_match = no_match.clone();
        let search = search.to_string();
        let host_entry = host_entry.clone();
        let port_entry = port_entry.clone();
        let user_entry = user_entry.clone();
        let label_entry = label_entry.clone();
        let protocol_dropdown = protocol_dropdown.clone();
        let options_ui = options_ui.clone();
        let e = entry.clone();
        let connect = on_connect.clone();
        let open = open_panel.clone();
        let pass_entry = pass_entry.clone();
        let editing_secret = editing_secret.clone();
        let editing_host = editing_host.clone();
        let password_dirty = password_dirty.clone();
        let suppress_pass_dirty = suppress_pass_dirty.clone();
        trash.connect_clicked(move |_| {
            credentials::delete_password(&credentials::secret_account(&e));
            if let Err(err) = remove_recent(&e) {
                tracing::warn!("viewer.json remove failed: {err}");
            }
            refill_hosts_list(
                &sections,
                &empty,
                &no_match,
                &search,
                view,
                &host_entry,
                &port_entry,
                &user_entry,
                &label_entry,
                &pass_entry,
                &protocol_dropdown,
                options_ui.clone(),
                editing_secret.clone(),
                editing_host.clone(),
                password_dirty.clone(),
                suppress_pass_dirty.clone(),
                connect.clone(),
                open.clone(),
                connecting_owned.as_deref(),
            );
        });
    }
    card.append(&trash);
    card
}
