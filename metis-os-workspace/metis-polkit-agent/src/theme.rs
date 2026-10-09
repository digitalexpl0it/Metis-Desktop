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
    let resolved = resolve_active();
    provider.load_from_string(&stylesheet(&resolved.tokens));

    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(resolved.is_dark);
        if let Some(gtk_theme) = metis_config::appearance_gtk_theme_env(if resolved.is_dark {
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

fn prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
}

fn resolve_active() -> metis_config::ResolvedUiTheme {
    let mode = metis_config::load_theme_preference_for_ui();
    metis_config::resolve_ui_theme_for_mode(mode, prefers_dark())
}

fn stylesheet(tokens: &ThemeTokens) -> String {
    let surface = &tokens.surface;
    let text = &tokens.text;
    let muted = &tokens.text_muted;
    let accent = tokens.accent_primary();
    let on_accent = tokens.on_accent_ink();
    let danger = &tokens.semantic.error;
    format!(
        r#"
        window.metis-polkit-window {{
            background-color: transparent;
            color: {text};
        }}
        .metis-polkit-dialog {{
            background-color: {surface};
            border-radius: 12px;
            margin: 8px 16px 16px 16px;
            padding: 20px;
            border: 1px solid color-mix(in srgb, {text} 12%, transparent);
            box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
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
