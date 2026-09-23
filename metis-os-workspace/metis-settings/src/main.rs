//! Metis Settings — a standalone GTK4 app for configuring appearance, weather,
//! network, and calendars. Reads/writes the shared `~/.config/metis/*.json` via
//! the `metis-config` crate; the running shell picks up changes through its file
//! watchers (or an explicit `reload-*` runtime command).
//!
//! Settings UI 2.0: Home overview + mini sidebar + right-edge category sheets.

mod apps;
mod bg;
mod bluetooth;
mod dialog;
mod gaming;
mod gtk_cb;
mod home;
mod i18n_gtk;
mod motion;
mod msauth;
mod nav;
mod net;
mod pages;
mod power;
mod printers;
mod remote;
mod runtime;
mod shell;
mod sound;
mod theme;
mod ui;

use std::cell::RefCell;

use gio::prelude::*;

pub(crate) const APP_ICON_BYTES: &[u8] = include_bytes!("../../assets/metis-settings.png");

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_settings=info,warn".into()),
        )
        .init();
    // reqwest is built with `rustls-no-provider`; without this every HTTPS
    // request (sign-in, weather lookup, time sync) would panic.
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        tracing::debug!("rustls crypto provider already installed");
    }

    metis_i18n::init();
    eprintln!("metis-settings: ui=home-stack (no overlay)");

    // Prefer in-process file choosers so Import/Open dialogs follow Metis
    // light/dark (`gtk_application_prefer_dark_theme`). Portal FileChooser
    // (`xdg-desktop-portal-gtk`) often opens light Adwaita when gtk-theme is
    // plain "Adwaita". Honour an explicit override if the user set one.
    //
    // SAFETY: single-threaded before GTK init; no other threads read env yet.
    if std::env::var_os("GTK_USE_PORTAL").is_none() {
        unsafe {
            std::env::set_var("GTK_USE_PORTAL", "0");
        }
    }
    if std::env::var_os("GSK_RENDERER").is_none() {
        // Cairo avoids multi-second GL/Vulkan scroll stalls on hybrid NVIDIA
        // (Windows / Display pages). Override with METIS_SETTINGS_GSK_RENDERER.
        let renderer =
            std::env::var("METIS_SETTINGS_GSK_RENDERER").unwrap_or_else(|_| "cairo".into());
        unsafe {
            std::env::set_var("GSK_RENDERER", renderer);
        }
    }

    let app = gtk::Application::builder()
        // Must be a reverse-DNS id (GLib rejects `metis-settings`).
        .application_id("com.metis.Settings")
        // Unique instance: a second `metis-settings --page …` forwards argv to
        // the running primary via D-Bus instead of spawning another window.
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.add_main_option(
        "page",
        glib::Char::from(b'\0'),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        "Open a settings page (network, network/vpn, power, bluetooth, …)",
        Some("PAGE"),
    );

    // Keep going so remote instances can forward `--page` to the primary.
    app.connect_handle_local_options(|_app, _dict| std::ops::ControlFlow::Continue(()));

    app.connect_command_line(|app, cmdline| {
        let launch = launch_from_command_line(cmdline);
        PENDING_LAUNCH.with(|slot| {
            *slot.borrow_mut() = Some(launch);
        });
        app.activate();
        glib::ExitCode::SUCCESS
    });

    app.connect_activate(|app| {
        let launch = PENDING_LAUNCH
            .with(|slot| slot.borrow_mut().take())
            .unwrap_or_default();
        shell::open(app, launch, false);
    });

    std::process::exit(app.run().into());
}

#[derive(Debug, Clone, Default)]
pub struct PageLaunch {
    /// Settings page id (`network`, `power`, …).
    pub page: Option<String>,
    /// Optional sub-tab within that page (`vpn` for Network).
    pub tab: Option<String>,
}

fn launch_from_command_line(cmdline: &gio::ApplicationCommandLine) -> PageLaunch {
    let dict = cmdline.options_dict();
    if let Ok(Some(page)) = dict.lookup::<String>("page") {
        return normalize_launch(&page);
    }
    // Fallback for callers that put `--page` in remaining argv.
    let args = cmdline.arguments();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let arg = arg.to_string_lossy();
        if let Some(name) = arg.strip_prefix("--page=") {
            return normalize_launch(name);
        }
        if arg == "--page"
            && let Some(name) = iter.next()
        {
            return normalize_launch(&name.to_string_lossy());
        }
    }
    PageLaunch::default()
}

fn normalize_launch(raw: &str) -> PageLaunch {
    let raw = raw.trim().to_lowercase();
    let (page_raw, tab) = if let Some((p, t)) = raw.split_once('/') {
        (p, Some(t.trim().to_string()))
    } else if let Some((p, t)) = raw.split_once(':') {
        (p, Some(t.trim().to_string()))
    } else {
        (raw.as_str(), None)
    };
    let page = nav::page_ids()
        .into_iter()
        .find(|id| *id == page_raw)
        .map(str::to_string);
    PageLaunch { page, tab }
}

thread_local! {
    /// Pending `--page` from `command-line` until `activate` consumes it.
    static PENDING_LAUNCH: RefCell<Option<PageLaunch>> = const { RefCell::new(None) };
}
