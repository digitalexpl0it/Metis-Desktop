//! Apply the same Metis theme tokens the shell uses, so the settings window looks
//! native to the desktop. Builds CSS from `metis_config::build_settings_app_stylesheet` pieces
//! (opaque window + appearance preview + Settings chrome — not the shell sheet).

use std::cell::RefCell;

use gtk::CssProvider;
use gtk::STYLE_PROVIDER_PRIORITY_APPLICATION;

use metis_config::{ResolvedUiTheme, ThemeMode, ThemeTokens};

thread_local! {
    /// The two display providers (opaque/preview base + settings chrome), kept
    /// so the theme can be re-applied live when the mode/colours change.
    static PROVIDERS: RefCell<Option<(CssProvider, CssProvider)>> = const { RefCell::new(None) };
}

fn prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
}

fn resolve_active() -> ResolvedUiTheme {
    metis_config::resolve_ui_theme(prefers_dark())
}

pub fn install() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let base = CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &base,
        STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let extra = CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &extra,
        STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    PROVIDERS.with(|p| *p.borrow_mut() = Some((base, extra)));
    reapply();
}

/// Re-read the active theme and reload both providers. Call after the user changes
/// the theme mode or any colour so the settings window (and its titlebar) update
/// live — mirroring the shell's own live theme reload.
pub fn reapply() {
    // Keep session gsettings / portal FileChooser in sync with on-disk Appearance.
    metis_config::sync_session_appearance_from_config();
    let resolved = resolve_active();
    reapply_tokens(&resolved.tokens, resolved.is_dark);
}

/// Apply a specific mode's tokens even when `config.json` could not be saved
/// (e.g. root-owned file). Keeps Settings looking correct while surfacing the
/// save error; other apps still need a successful save.
pub fn reapply_for_mode(mode: ThemeMode) {
    let resolved = metis_config::resolve_ui_theme_for_mode(mode, prefers_dark());
    reapply_tokens(&resolved.tokens, resolved.is_dark);
}

fn reapply_tokens(tokens: &ThemeTokens, dark: bool) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    PROVIDERS.with(|p| {
        if let Some((base, extra)) = p.borrow().as_ref() {
            // Opaque window + appearance preview on `base`; Settings chrome on `extra`.
            let mut base_css = metis_config::build_settings_opaque_window(tokens);
            base_css.push_str(&metis_config::build_appearance_preview_stylesheet(tokens));
            base.load_from_string(&base_css);
            extra.load_from_string(&metis_config::build_settings_chrome_stylesheet(tokens));
        }
    });
}
