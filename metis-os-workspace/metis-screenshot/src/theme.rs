//! Opaque stylesheet for the editor and small layer-shell surfaces.
//!
//! Do not use `metis_config::build_stylesheet`: it deliberately makes layer-shell
//! windows transparent, which is unsuitable for the editor's opaque toplevel.

use metis_config::ThemeTokens;

pub fn sync_gtk_theme_env() {
    let mode = metis_config::load_theme_preference_for_ui();
    // SAFETY: called before GTK initializes and before worker threads start.
    unsafe {
        match metis_config::appearance_gtk_theme_env(mode) {
            Some(value) => std::env::set_var("GTK_THEME", value),
            None => std::env::remove_var("GTK_THEME"),
        }
    }
}

pub fn active_tokens() -> ThemeTokens {
    metis_config::load_theme_tokens(match metis_config::load_theme_preference_for_ui() {
        metis_config::ThemeMode::Light => "light",
        metis_config::ThemeMode::Dark => "dark",
        metis_config::ThemeMode::System => "dark",
    })
}

pub fn install() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let tokens = active_tokens();
    let accent = tokens.accent_primary().to_string();
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        r#"
        window.metis-screenshot-root,
        .metis-screenshot-root {{
            background: {bg};
            color: {text};
        }}
        window.metis-screenshot-ssd headerbar,
        window.metis-screenshot-ssd .titlebar,
        window.metis-screenshot-ssd windowhandle {{
            min-height: 0;
            padding: 0;
            margin: 0;
            border: none;
            background: none;
            box-shadow: none;
        }}

        .metis-shot-bar {{
            background: {surface};
            border: 1px solid {border};
            border-radius: {radius_lg}px;
            padding: 6px;
        }}
        .metis-shot-bar separator {{
            background: {border};
            margin: 4px 6px;
        }}

        .metis-shot-tool {{
            min-width: 34px;
            min-height: 34px;
            padding: 0;
            border: none;
            border-radius: {radius_md}px;
            background: none;
            box-shadow: none;
            color: {text};
            transition: background 120ms ease, color 120ms ease;
        }}
        .metis-shot-tool:hover {{ background: {raised}; }}
        .metis-shot-tool:checked,
        .metis-shot-tool:active {{
            background: {accent};
            color: {on_accent};
        }}

        .metis-shot-action {{
            min-height: 32px;
            padding: 4px 12px;
            border: 1px solid {border};
            border-radius: {radius_md}px;
            background: {raised};
            color: {text};
            box-shadow: none;
        }}
        .metis-shot-action:hover {{ background: {surface}; }}
        .metis-shot-action.flat {{
            border: none;
            background: none;
            min-width: 34px;
            padding: 4px 8px;
        }}
        .metis-shot-action.flat:hover {{ background: {raised}; }}
        .metis-shot-action.suggested {{
            background: {accent};
            color: {on_accent};
            border-color: {accent};
            font-weight: 600;
        }}

        .metis-shot-swatch {{
            min-width: 26px;
            min-height: 26px;
            padding: 0;
            border-radius: 999px;
            border: 2px solid transparent;
            box-shadow: none;
        }}
        .metis-shot-swatch:checked {{ border-color: {text}; }}

        .metis-shot-stage {{
            background: {bg};
            border: 1px solid {border};
            border-radius: {radius_lg}px;
        }}
        .metis-shot-stage scrolledwindow,
        .metis-shot-stage viewport {{ background: none; }}

        .metis-shot-status {{
            color: {muted};
            padding: 0 4px;
        }}
        .metis-shot-status.warn {{ color: {warn}; }}
        .metis-shot-text-results {{
            background: {raised};
            border: 1px solid {border};
            border-radius: {radius_lg}px;
        }}
        .metis-shot-text-results textview,
        .metis-shot-text-results text {{
            background: {raised};
            color: {text};
        }}
        .metis-shot-text-results text selection {{
            background: {accent};
            color: {on_accent};
        }}

        .metis-screenshot-pill {{
            background: {surface};
            color: {text};
            border: 1px solid {border};
            border-radius: {radius_lg}px;
            padding: 8px;
        }}
        .metis-screenshot-pill button.suggested-action {{
            background: {accent};
            color: {on_accent};
            border-radius: {radius_sm}px;
        }}
        "#,
        bg = tokens.bg,
        surface = tokens.surface,
        raised = tokens.surface_raised,
        border = tokens.border,
        text = tokens.text,
        muted = tokens.text_muted,
        warn = tokens.semantic.warning,
        accent = accent,
        on_accent = tokens.text_on_accent,
        radius_sm = tokens.radius_sm,
        radius_md = tokens.radius_md,
        radius_lg = tokens.radius_lg,
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );
}
