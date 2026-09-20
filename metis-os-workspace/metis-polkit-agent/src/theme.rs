//! Apply Metis appearance tokens to the agent dialogs.

use gtk::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};
use metis_config::{ThemeMode, ThemeTokens};

pub fn install() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let tokens = active_tokens();
    let dark = active_mode_is_dark();
    provider.load_from_data(&stylesheet(&tokens));

    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
        if let Some(gtk_theme) = metis_config::appearance_gtk_theme_env(if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        }) {
            unsafe {
                std::env::set_var("GTK_THEME", gtk_theme);
            }
        }
    }
}

fn active_mode_is_dark() -> bool {
    match metis_config::load_theme_preference().unwrap_or(ThemeMode::Dark) {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => gtk::Settings::default()
            .map(|s| s.is_gtk_application_prefer_dark_theme())
            .unwrap_or(true),
    }
}

fn active_tokens() -> ThemeTokens {
    let name = if active_mode_is_dark() {
        "dark"
    } else {
        "light"
    };
    metis_config::load_theme_tokens(name)
}

fn stylesheet(tokens: &ThemeTokens) -> String {
    let bg = &tokens.bg;
    let surface = &tokens.surface;
    let text = &tokens.text;
    let muted = &tokens.text_muted;
    let accent = tokens.accent_primary();
    let on_accent = tokens.on_accent_ink();
    let danger = &tokens.semantic.error;
    format!(
        r#"
        window.metis-polkit-window {{
            background-color: {bg};
            color: {text};
        }}
        .metis-polkit-dialog {{
            background-color: {surface};
            border-radius: 12px;
            margin: 16px;
            padding: 20px;
        }}
        .metis-polkit-title {{
            font-weight: 600;
            font-size: 1.1em;
            color: {text};
        }}
        .metis-polkit-action {{
            color: {muted};
            font-size: 0.85em;
            font-family: monospace;
        }}
        .metis-polkit-error {{
            color: {danger};
            font-size: 0.9em;
        }}
        .metis-polkit-icon {{
            color: {accent};
        }}
        window.metis-polkit-window button.suggested-action {{
            background-color: {accent};
            background: {accent};
            border: none;
            border-radius: 8px;
            padding: 8px 16px;
            color: {on_accent};
        }}
        window.metis-polkit-window button.suggested-action:hover {{
            background-color: color-mix(in srgb, {accent} 82%, white);
            background: color-mix(in srgb, {accent} 82%, white);
            color: {on_accent};
        }}
        window.metis-polkit-window button.suggested-action label,
        window.metis-polkit-window button.suggested-action:hover label {{
            color: {on_accent};
            opacity: 1;
        }}
        window.metis-polkit-window button.suggested-action:disabled,
        window.metis-polkit-window button.suggested-action:disabled label {{
            opacity: 0.55;
        }}
        entry, entry.metis-polkit-password {{
            border-radius: 8px;
            padding: 6px 10px;
        }}
        "#
    )
}
