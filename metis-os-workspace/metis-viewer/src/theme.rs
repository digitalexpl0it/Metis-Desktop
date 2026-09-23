//! Opaque Metis Viewer theme from Appearance tokens.
//!
//! NEVER load `metis_config::build_stylesheet` here. That sheet sets
//! `window { background-color: transparent }` for layer-shell overlays; even with
//! `!important` overrides, GTK can leave the toplevel ARGB and the wallpaper
//! shows through. Settings works around it with a large settings-only CSS file;
//! Viewer is simpler: one opaque stylesheet only.

use std::cell::RefCell;
use std::time::Duration;

use gio::prelude::*;
use gtk::CssProvider;
use gtk::STYLE_PROVIDER_PRIORITY_USER;
use metis_config::{ThemeMode, ThemeTokens};

thread_local! {
    static PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static MONITORS: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
}

pub fn active_tokens() -> ThemeTokens {
    let mode = metis_config::load_theme_preference_for_ui();
    let name = match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => {
            if prefers_dark() {
                "dark"
            } else {
                "light"
            }
        }
    };
    metis_config::load_theme_tokens(name)
}

pub fn effective_theme_name() -> String {
    match metis_config::load_theme_preference_for_ui() {
        ThemeMode::Light => "light".into(),
        ThemeMode::Dark => "dark".into(),
        ThemeMode::System => {
            if prefers_dark() {
                "dark".into()
            } else {
                "light".into()
            }
        }
    }
}

pub fn active_mode_is_dark() -> bool {
    match metis_config::load_theme_preference_for_ui() {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => prefers_dark(),
    }
}

fn prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
}

pub fn sync_gtk_theme_env() {
    let mode = metis_config::load_theme_preference_for_ui();
    // SAFETY: single-threaded before GTK init.
    unsafe {
        match metis_config::appearance_gtk_theme_env(mode) {
            Some(theme) => std::env::set_var("GTK_THEME", theme),
            None => std::env::remove_var("GTK_THEME"),
        }
    }
}

pub fn install() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = CssProvider::new();
    // USER (800) beats APPLICATION and theme engines.
    gtk::style_context_add_provider_for_display(&display, &provider, STYLE_PROVIDER_PRIORITY_USER);
    PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    reapply();
    watch_appearance();
}

pub fn reapply() {
    let dark = active_mode_is_dark();
    let name = effective_theme_name();
    tracing::info!(theme = %name, "applying Metis Viewer theme");

    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
        settings.set_gtk_theme_name(Some(if dark { "Adwaita-dark" } else { "Adwaita" }));
    }

    let tokens = active_tokens();
    PROVIDER.with(|p| {
        if let Some(provider) = p.borrow().as_ref() {
            provider.load_from_string(&opaque_viewer_css(&tokens));
        }
    });
}

fn watch_appearance() {
    let _ = metis_config::ensure_config_dirs();
    let mut held = Vec::new();

    for path in [
        metis_config::app_config_path(),
        metis_config::config_dir().join("appearance.mode"),
    ] {
        let file = gio::File::for_path(&path);
        if let Ok(mon) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>) {
            mon.connect_changed(move |_, _, _, _| {
                glib::timeout_add_local_once(Duration::from_millis(120), reapply);
            });
            held.push(mon);
        }
    }

    let themes = gio::File::for_path(metis_config::config_dir().join("themes"));
    if let Ok(mon) =
        themes.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    {
        mon.connect_changed(move |_, _, _, _| {
            glib::timeout_add_local_once(Duration::from_millis(120), reapply);
        });
        held.push(mon);
    }

    glib::timeout_add_local(Duration::from_millis(400), || {
        use std::sync::atomic::{AtomicU8, Ordering};
        static LAST: AtomicU8 = AtomicU8::new(255);
        let tag: u8 = match effective_theme_name().as_str() {
            "light" => 0,
            "dark" => 1,
            _ => 2,
        };
        if LAST.swap(tag, Ordering::SeqCst) != tag {
            reapply();
        }
        glib::ControlFlow::Continue
    });

    MONITORS.with(|m| *m.borrow_mut() = held);
}

/// Parse-safe opaque CSS only (no `color-mix` — older GTK CSS parsers reject the
/// whole sheet on unknown functions, which left the window transparent).
fn opaque_viewer_css(t: &ThemeTokens) -> String {
    let bg = &t.bg;
    let surface = &t.surface;
    let raised = &t.surface_raised;
    let border = &t.border;
    let text = &t.text;
    let muted = &t.text_muted;
    let accent = t.accent_primary();
    let on_accent = &t.text_on_accent;
    let warning = &t.semantic.warning;
    let error = &t.semantic.error;
    let success = &t.semantic.success;
    let rl = t.radius_lg;
    let rs = t.radius_sm;
    format!(
        r#"
        window,
        window.background,
        window.csd,
        window.solid-csd,
        window.metis-viewer-window,
        .metis-viewer-window {{
            background-color: {bg};
            background-image: none;
            color: {text};
        }}

        window.metis-viewer-window > box,
        .metis-viewer-root,
        .metis-viewer-page,
        window.metis-viewer-window scrolledwindow,
        window.metis-viewer-window scrolledwindow > viewport,
        window.metis-viewer-window scrolledwindow > viewport > overshoot,
        window.metis-viewer-window scrolledwindow > viewport > undershoot {{
            background-color: {bg};
            background-image: none;
            color: {text};
        }}

        windowhandle, headerbar, .titlebar {{
            background-color: {surface};
            background-image: none;
            color: {text};
            border-bottom: 1px solid {border};
            box-shadow: none;
        }}

        /* Hide GTK CSD when Metis draws SSD — keep opaque bg, do not use opacity:0
           on containers (that punches a hole through the window). */
        window.metis-viewer-window.metis-viewer-ssd headerbar,
        window.metis-viewer-window.metis-viewer-ssd .titlebar,
        window.metis-viewer-window.metis-viewer-ssd windowhandle {{
            min-height: 0px;
            padding: 0;
            margin: 0;
            border: none;
            box-shadow: none;
            background-color: {bg};
        }}

        .metis-viewer-title {{
            font-size: 20px;
            font-weight: 650;
            color: {text};
        }}
        .metis-viewer-subtitle {{
            font-size: 13px;
            color: {muted};
        }}
        .metis-viewer-header-icon {{
            color: {accent};
            -gtk-icon-style: symbolic;
        }}

        .metis-viewer-card {{
            background-color: {surface};
            border: 1px solid {border};
            border-radius: {rl}px;
        }}
        .metis-viewer-card-title {{
            font-size: 11px;
            font-weight: 650;
            letter-spacing: 0.04em;
            text-transform: uppercase;
            color: {muted};
            padding: 12px 16px 4px;
        }}
        .metis-viewer-field {{
            padding: 8px 16px;
        }}
        .metis-viewer-field-label {{
            font-size: 12px;
            font-weight: 600;
            color: {muted};
            margin-bottom: 4px;
        }}

        entry, passwordentry {{
            min-height: 34px;
            border-radius: {rs}px;
            border: 1px solid {border};
            background-color: {raised};
            color: {text};
            caret-color: {text};
        }}
        entry > text, passwordentry > text {{
            background-color: {raised};
            color: {text};
        }}

        .metis-viewer-hint {{
            font-size: 12px;
            color: {muted};
            padding: 0 16px 10px;
        }}
        .metis-viewer-banner {{
            background-color: {surface};
            border: 1px solid {warning};
            border-radius: {rl}px;
            padding: 12px 14px;
        }}
        .metis-viewer-banner-title {{
            font-size: 13px;
            font-weight: 650;
            color: {text};
        }}
        .metis-viewer-banner-body {{
            font-size: 12px;
            font-family: monospace;
            color: {muted};
            background-color: {raised};
            padding: 6px 8px;
            border-radius: {rs}px;
            margin-top: 4px;
        }}

        .metis-viewer-status, .metis-viewer-ready, .metis-viewer-empty {{
            font-size: 12px;
            color: {muted};
        }}
        .metis-viewer-status.error {{ color: {error}; font-weight: 500; }}
        .metis-viewer-status.ok {{ color: {success}; }}

        button.suggested-action {{
            min-height: 36px;
            padding: 0 22px;
            border-radius: {rs}px;
            background-color: {accent};
            color: {on_accent};
            font-weight: 600;
            border: none;
        }}
        button.suggested-action:disabled {{
            opacity: 0.45;
        }}

        .metis-viewer-recent {{
            background-color: {surface};
            border: 1px solid {border};
            border-radius: {rl}px;
        }}
        .metis-viewer-recent-row {{
            padding: 8px 10px;
        }}
        .metis-viewer-recent-host {{
            font-weight: 600;
            color: {text};
        }}
        .metis-viewer-recent-meta {{
            color: {muted};
        }}
        "#
    )
}
