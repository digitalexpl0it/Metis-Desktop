//! Metis GTK stylesheets built from [`ThemeTokens`].
//!
//! Shell UI uses [`build_stylesheet`]. Settings uses
//! [`build_settings_app_stylesheet`] (opaque window + appearance preview + chrome).

mod appearance_preview;
mod bar;
mod base;
mod clocks;
mod dashboard;
mod desktop_widgets;
mod menu;
mod notification_center;
mod notifications;
mod onboarding;
mod osd_network;
mod screenshot_overlay;
mod settings;
mod task_view;
mod toasts;
mod updater;
mod vars;

use crate::theme::ThemeTokens;
use vars::CssVars;

/// Full shell / desktop-widgets stylesheet (layer-shell transparent windows).
pub fn build_stylesheet(theme: &ThemeTokens) -> String {
    let v = CssVars::from_theme(theme);
    let mut out = String::with_capacity(96 * 1024);
    out.push_str(&base::stylesheet(&v));
    out.push_str(&bar::stylesheet(&v));
    out.push_str(&updater::stylesheet(&v));
    out.push_str(&notifications::stylesheet(&v));
    out.push_str(&toasts::stylesheet(&v));
    out.push_str(&osd_network::stylesheet(&v));
    out.push_str(&clocks::stylesheet(&v));
    out.push_str(&onboarding::stylesheet(&v));
    out.push_str(&appearance_preview::stylesheet(&v));
    out.push_str(&menu::stylesheet(&v));
    out.push_str(&dashboard::stylesheet(&v));
    out.push_str(&screenshot_overlay::stylesheet(&v));
    out.push_str(&task_view::stylesheet(&v));
    out.push_str(&notification_center::stylesheet(&v));
    out.push_str(&desktop_widgets::stylesheet(&v));
    out
}

/// `.metis-style-*` preview cards (Settings Appearance + shell onboarding).
pub fn build_appearance_preview_stylesheet(theme: &ThemeTokens) -> String {
    let v = CssVars::from_theme(theme);
    appearance_preview::stylesheet(&v)
}

/// Opaque toplevel / FileChooser / ColorChooser overrides for xdg Settings windows.
pub fn build_settings_opaque_window(theme: &ThemeTokens) -> String {
    format!(
        "\nwindow, window.background, window.dialog, window.csd,\n         colorchooser, fontchooser {{\n             background-color: {bg} !important;\n             color: {text};\n         }}\n         window.metis-settings-password-dialog,\n         window.metis-settings-widget-dialog {{\n             background-color: transparent !important;\n         }}\n         colorchooser scrolledwindow, colorchooser viewport,\n         colorchooser grid, colorchooser box,\n         fontchooser scrolledwindow, fontchooser viewport,\n         fontchooser listview, fontchooser listview > row {{\n             background-color: {surface} !important;\n             color: {text};\n         }}\n",
        bg = theme.bg,
        text = theme.text,
        surface = theme.surface,
    )
}

/// Settings chrome only (no shell bar/menu/dashboard rules).
pub fn build_settings_chrome_stylesheet(theme: &ThemeTokens) -> String {
    settings::build_settings_chrome_stylesheet(theme)
}

/// Full Settings app stylesheet: opaque window + appearance preview + chrome.
pub fn build_settings_app_stylesheet(theme: &ThemeTokens) -> String {
    let mut out = build_settings_opaque_window(theme);
    out.push_str(&build_appearance_preview_stylesheet(theme));
    out.push_str(&build_settings_chrome_stylesheet(theme));
    out
}
