//! Apply the same Metis theme tokens the shell uses, so the settings window looks
//! native to the desktop. Builds CSS from the shared `metis_config` stylesheet.

use std::cell::RefCell;

use gtk::CssProvider;
use gtk::STYLE_PROVIDER_PRIORITY_APPLICATION;

use metis_config::{ThemeMode, ThemeTokens};

thread_local! {
    /// The two display providers (shared bar stylesheet + settings chrome), kept
    /// so the theme can be re-applied live when the mode/colours change.
    static PROVIDERS: RefCell<Option<(CssProvider, CssProvider)>> = const { RefCell::new(None) };
}

/// Resolve the currently active theme tokens (honouring the saved mode, with a
/// GTK fallback for `system`).
pub fn active_tokens() -> ThemeTokens {
    let mode = metis_config::load_theme_preference().unwrap_or(ThemeMode::Dark);
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

fn prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
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
    reapply_tokens(&active_tokens(), active_mode_is_dark());
}

/// Apply a specific mode's tokens even when `config.json` could not be saved
/// (e.g. root-owned file). Keeps Settings looking correct while surfacing the
/// save error; other apps still need a successful save.
pub fn reapply_for_mode(mode: ThemeMode) {
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
    let tokens = metis_config::load_theme_tokens(name);
    let dark =
        matches!(mode, ThemeMode::Dark) || (matches!(mode, ThemeMode::System) && prefers_dark());
    reapply_tokens(&tokens, dark);
}

fn reapply_tokens(tokens: &ThemeTokens, dark: bool) {
    // Flip GTK's built-in Adwaita variant so default widget chrome (dropdowns,
    // popovers, scales, switches, scrollbars) switches light/dark too — our CSS
    // only restyles our own classes, not GTK's internal widget nodes.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    // Keep session gsettings / portal FileChooser in sync with Appearance.
    // Caller is responsible for which mode was chosen (may differ from disk).
    PROVIDERS.with(|p| {
        if let Some((base, extra)) = p.borrow().as_ref() {
            // The shared stylesheet sets `window { background-color: transparent }`
            // so the shell's layer-shell overlays (bar, popovers) can show through.
            // The settings app is a real opaque toplevel, though — without forcing it
            // solid, wallpaper bleeds through and every scroll frame re-composites the
            // desktop (hitch / "pause then catch up"). Append opaque override LAST in
            // the *same* provider; dialog sheets re-assert transparent in settings_css.
            let mut css = metis_config::build_stylesheet(tokens);
            css.push_str(&format!(
                "\nwindow, window.background, window.dialog, window.csd,\n\
                 colorchooser, fontchooser {{\n\
                     background-color: {bg} !important;\n\
                     color: {text};\n\
                 }}\n\
                 window.metis-settings-password-dialog,\n\
                 window.metis-settings-widget-dialog {{\n\
                     background-color: transparent !important;\n\
                 }}\n\
                 colorchooser scrolledwindow, colorchooser viewport,\n\
                 colorchooser grid, colorchooser box,\n\
                 fontchooser scrolledwindow, fontchooser viewport,\n\
                 fontchooser listview, fontchooser listview > row {{\n\
                     background-color: {surface} !important;\n\
                     color: {text};\n\
                 }}\n",
                bg = tokens.bg,
                text = tokens.text,
                surface = tokens.surface,
            ));
            base.load_from_data(&css);
            extra.load_from_data(&settings_css(tokens));
        }
    });
}

/// Whether the active theme resolves to a dark variant.
fn active_mode_is_dark() -> bool {
    match metis_config::load_theme_preference().unwrap_or(ThemeMode::Dark) {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => prefers_dark(),
    }
}

/// Settings-window chrome that isn't part of the shared bar stylesheet.
fn settings_css(t: &ThemeTokens) -> String {
    let bg = &t.bg;
    let surface = &t.surface;
    let raised = &t.surface_raised;
    let border = &t.border;
    let text = &t.text;
    let muted = &t.text_muted;
    let accent = t.accent_primary();
    // Derive live so cyan/white accents never keep a stale white `text_on_accent`.
    let on_accent = t.on_accent_ink();
    let error = &t.semantic.error;
    let warning = &t.semantic.warning;
    let success = &t.semantic.success;
    let rl = t.radius_lg;
    let rs = t.radius_sm;
    let accent2 = t.accent_secondary();
    let text_rgb = t.text_rgb();
    format!(
        r#"
        /* The shared bar stylesheet makes every `window` transparent for the
           layer-shell overlays; in the settings app we want solid windows so
           spawned dialogs (e.g. the colour picker) aren't see-through. */
        window {{ background-color: {bg} !important; color: {text}; }}
        window.dialog,
        window.csd,
        window.aboutdialog,
        .colorchooser,
        colorchooser,
        fontchooser,
        .fontchooser {{
            background-color: {bg} !important;
            color: {text};
        }}

        /* Window + CSD titlebar so the whole frame tracks the active theme. */
        .metis-settings-window {{ background-color: {bg} !important; color: {text}; }}
        window.metis-settings-confirm-dialog {{
            background-color: {bg} !important;
            color: {text};
        }}

        /* Modal sheets: the window buffer stays transparent so pixels outside the
           rounded card are true alpha (not a solid grey fill). Metis SSD already
           draws the drop shadow — a CSS box-shadow here painted opaque "ears" in
           the square corners outside border-radius. */
        window.metis-settings-password-dialog,
        window.metis-settings-widget-dialog,
        window.metis-settings-window.metis-settings-password-dialog,
        window.metis-settings-window.metis-settings-widget-dialog {{
            background-color: transparent !important;
            color: {text};
        }}

        /* In-window color/font choosers (top-slide sheet) — force opaque chrome. */
        colorchooser,
        colorchooser > box,
        colorchooser grid,
        colorchooser scrolledwindow,
        colorchooser scrolledwindow > viewport,
        colorchooser viewport,
        .metis-settings-color-chooser,
        .metis-settings-color-chooser > box,
        .metis-settings-color-chooser grid,
        .metis-settings-color-chooser scrolledwindow,
        .metis-settings-color-chooser scrolledwindow > viewport,
        .metis-settings-color-chooser viewport {{
            background-color: {surface};
            color: {text};
        }}
        colorchooser listview,
        colorchooser listview > row,
        .metis-settings-color-chooser listview,
        .metis-settings-color-chooser listview > row {{
            background-color: {raised};
            color: {text};
        }}
        fontchooser,
        fontchooser > box,
        fontchooser scrolledwindow,
        fontchooser scrolledwindow > viewport,
        fontchooser viewport,
        .metis-settings-font-chooser,
        .metis-settings-font-chooser > box,
        .metis-settings-font-chooser scrolledwindow,
        .metis-settings-font-chooser scrolledwindow > viewport,
        .metis-settings-font-chooser viewport {{
            background-color: {surface};
            color: {text};
        }}
        fontchooser listview,
        fontchooser listview > row,
        fontchooser listview > row:hover,
        fontchooser listview > row:selected,
        .metis-settings-font-chooser listview,
        .metis-settings-font-chooser listview > row,
        .metis-settings-font-chooser listview > row:hover,
        .metis-settings-font-chooser listview > row:selected {{
            background-color: {raised};
            color: {text};
        }}
        fontchooser listview > row:selected,
        .metis-settings-font-chooser listview > row:selected {{
            background-color: color-mix(in srgb, {accent} 22%, {raised});
            color: {text};
        }}
        fontchooser listview > row label,
        .metis-settings-font-chooser listview > row label {{
            color: {text};
        }}
        .metis-settings-font-chooser scale {{
            min-height: 22px;
            color: {text};
        }}
        .metis-settings-font-chooser scale trough,
        fontchooser scale trough {{
            min-height: 6px;
            background-color: {border};
        }}
        /* FontChooser uses a teardrop mark icon — keep the thumb chrome transparent
           so we don't get a solid square (global scale slider uses {accent}). */
        .metis-settings-font-chooser scale slider,
        fontchooser scale slider {{
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            border-radius: 0;
            min-width: 18px;
            min-height: 18px;
            padding: 0;
            color: {accent};
            -gtk-icon-filter: none;
        }}
        .metis-settings-font-chooser scale slider:hover,
        fontchooser scale slider:hover,
        .metis-settings-font-chooser scale slider:active,
        fontchooser scale slider:active {{
            background-color: transparent;
            background-image: none;
            box-shadow: none;
        }}
        .metis-settings-font-chooser scale marks,
        fontchooser scale marks {{
            color: {muted};
        }}
        window.dialog scrolledwindow,
        window.dialog scrolledwindow > viewport,
        window.dialog listview,
        window.dialog listview > row {{
            background-color: {surface};
            color: {text};
        }}
        window.dialog listview > row label {{
            color: {text};
        }}
        .metis-settings-dialog-sheet {{
            background-color: {surface};
            border: 1px solid {border};
            border-radius: 12px;
        }}
        .metis-settings-password-dialog .metis-settings-section-title,
        .metis-settings-widget-dialog .metis-settings-section-title {{
            padding: 0;
            text-transform: none;
            letter-spacing: 0;
            font-size: 15px;
            font-weight: 600;
            color: {text};
        }}
        .metis-settings-password-dialog .metis-settings-row,
        .metis-settings-widget-dialog .metis-settings-row {{
            padding: 6px 0;
            border-top: none;
        }}
        .metis-settings-password-dialog .metis-settings-hint,
        .metis-settings-widget-dialog .metis-settings-hint {{
            padding: 0;
        }}
        .metis-widget-add-row {{
            padding: 4px 16px 12px;
        }}
        .metis-widget-list {{
            margin: 0 12px 12px;
            border: 1px solid {border};
            border-radius: {rl}px;
            background-color: {raised};
        }}
        .metis-widget-list-row {{
            padding: 10px 12px;
            background-color: transparent;
            transition: background-color 140ms ease;
        }}
        .metis-widget-list-row-alt {{
            background-color: color-mix(in srgb, {surface} 55%, transparent);
        }}
        .metis-widget-list-row:hover {{
            background-color: color-mix(in srgb, {accent} 10%, {raised});
        }}
        .metis-widget-list-icon {{
            color: {accent};
            -gtk-icon-style: symbolic;
        }}
        .metis-widget-list-title {{
            color: {text};
            font-size: 13px;
            font-weight: 600;
        }}
        .metis-widget-list-subtitle {{
            color: {muted};
            font-size: 11px;
        }}
        .metis-widget-configure-btn {{
            min-width: 34px;
            min-height: 34px;
            padding: 0;
        }}
        .metis-widget-configure-btn image {{
            color: {muted};
            -gtk-icon-style: symbolic;
        }}
        .metis-widget-configure-btn:hover image {{
            color: {accent};
        }}
        windowhandle, headerbar, .titlebar {{
            background-color: {surface};
            background-image: none;
            color: {text};
            border-bottom: 1px solid {border};
            box-shadow: none;
        }}
        headerbar label, .titlebar label {{ color: {text}; }}
        headerbar button, windowcontrols button {{
            color: {text};
            background-color: transparent;
            box-shadow: none;
            border: none;
        }}
        headerbar button:hover, windowcontrols button:hover {{ background-color: {raised}; }}
        windowcontrols button image {{ color: {text}; }}

        .metis-settings-root {{ background-color: {bg}; }}

        /* Dividers between sidebar/content + any separators: theme-coloured, flat. */
        separator {{
            background-color: {border};
            background-image: none;
            min-width: 1px;
            min-height: 1px;
            border: none;
            box-shadow: none;
        }}
        .metis-settings-sidebar {{
            box-shadow: none;
            border: none;
            background-color: {surface};
            padding-bottom: 12px;
            min-width: 248px;
        }}
        /* Kill GTK's scroll edge fades (undershoot) and bounce glows
           (overshoot) — and strip any transition so a late overshoot frame
           cannot hitch the GL renderer when the scrollbar hits top/bottom. */
        undershoot.top, undershoot.bottom, undershoot.left, undershoot.right,
        overshoot.top, overshoot.bottom, overshoot.left, overshoot.right {{
            background-color: transparent;
            background-image: none;
            box-shadow: none;
            border: none;
            opacity: 0;
            min-width: 0;
            min-height: 0;
            padding: 0;
            margin: 0;
            transition: none;
            animation: none;
        }}
        .metis-settings-scroller overshoot,
        .metis-settings-scroller undershoot,
        .metis-settings-nav-scroll overshoot,
        .metis-settings-nav-scroll undershoot {{
            opacity: 0;
            transition: none;
            animation: none;
        }}

        /* Tokenized scrollbars — Adwaita prefer-dark alone leaves dark chrome
           when Metis light mode is active. */
        .metis-settings-scroller scrollbar,
        .metis-settings-nav-scroll scrollbar,
        .metis-settings-schedule-presets scrollbar,
        scrollbar {{
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-width: 10px;
            min-height: 10px;
        }}
        scrollbar.vertical {{
            min-width: 10px;
        }}
        scrollbar trough {{
            background-color: transparent;
            border: none;
        }}
        scrollbar slider {{
            min-width: 7px;
            min-height: 24px;
            border-radius: 999px;
            background-color: rgba({text_rgb}, 0.22);
            border: none;
            box-shadow: none;
        }}
        scrollbar slider:hover {{
            background-color: rgba({text_rgb}, 0.36);
        }}
    .metis-settings-sidebar-title {{
        font-size: 22px;
        font-weight: 800;
        color: {text};
        letter-spacing: -0.02em;
    }}
    image.metis-settings-sidebar-icon {{
        border-radius: 8px;
    }}
        .metis-settings-search {{
            min-height: 32px;
            border-radius: 8px;
            background-color: color-mix(in srgb, {raised} 85%, {surface});
            border: 1px solid color-mix(in srgb, {border} 80%, transparent);
            transition: border-color 140ms ease, box-shadow 140ms ease;
        }}
        .metis-settings-search:focus-within {{
            border-color: {accent};
            box-shadow: 0 0 0 3px color-mix(in srgb, {accent} 22%, transparent);
        }}
        .metis-settings-nav-scroll {{
            background-color: {surface};
        }}
        .metis-settings-nav list,
        .metis-settings-sidebar list {{
            background-color: {surface};
            padding: 4px 10px 8px;
        }}
        .metis-settings-nav-section-row {{
            background-color: transparent;
            min-height: 0;
            padding: 0;
            margin-top: 10px;
        }}
        .metis-settings-nav-section {{
            color: {muted};
            font-size: 11px;
            font-weight: 700;
            letter-spacing: 0.06em;
            text-transform: uppercase;
            padding: 8px 8px 4px;
        }}
        .metis-settings-nav-row {{
            border-radius: {rl}px;
            padding: 0;
            margin: 2px 0;
            min-height: 36px;
        }}
        .metis-settings-nav-row-inner {{
            padding: 6px 10px 6px 8px;
        }}
        /* MacOS-style coloured icon badges — padding (not fixed size) keeps glyphs centred. */
        .metis-settings-nav-icon-wrap {{
            border-radius: 7px;
            padding: 6px;
            background-color: color-mix(in srgb, {muted} 18%, {raised});
        }}
        .metis-settings-nav-icon {{
            color: {text};
            -gtk-icon-style: symbolic;
        }}
        .metis-settings-nav-label {{ color: {text}; font-size: 13px; font-weight: 500; }}
        .metis-settings-nav-row:hover {{ background-color: {raised}; }}
        .metis-settings-nav-row:selected {{
            background-color: color-mix(in srgb, {accent} 18%, {raised});
        }}
        .metis-settings-nav-row:selected .metis-settings-nav-label {{
            color: {text};
            font-weight: 700;
        }}
        .metis-settings-nav-row:selected .metis-settings-nav-icon {{
            color: {text};
        }}

        .metis-settings-content {{ background-color: {bg}; }}
        .metis-settings-scroller,
        .metis-settings-scroller > viewport,
        .metis-settings-scroller viewport {{
            background-color: {bg};
        }}
        .metis-settings-root {{
            background-color: {bg};
        }}

        .metis-settings-page {{ background-color: {bg}; }}
        .metis-settings-page-header {{
            margin-bottom: 8px;
            padding-bottom: 8px;
        }}
        .metis-settings-page-icon-wrap {{
            border-radius: 14px;
            padding: 12px;
            background-color: color-mix(in srgb, {muted} 18%, {surface});
            box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
        }}
        .metis-settings-page-icon {{
            color: {accent};
            -gtk-icon-style: symbolic;
        }}

        /* Hue badges (nav + page header) — after base wraps so colours always win. */
        .metis-settings-nav-icon-wrap.metis-nav-hue-blue,
        .metis-settings-page-icon-wrap.metis-nav-hue-blue {{
            background-color: color-mix(in srgb, #0a84ff 28%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-purple,
        .metis-settings-page-icon-wrap.metis-nav-hue-purple {{
            background-color: color-mix(in srgb, #bf5af2 28%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-pink,
        .metis-settings-page-icon-wrap.metis-nav-hue-pink {{
            background-color: color-mix(in srgb, #ff375f 26%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-orange,
        .metis-settings-page-icon-wrap.metis-nav-hue-orange {{
            background-color: color-mix(in srgb, #ff9f0a 28%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-teal,
        .metis-settings-page-icon-wrap.metis-nav-hue-teal {{
            background-color: color-mix(in srgb, #64d2ff 28%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-green,
        .metis-settings-page-icon-wrap.metis-nav-hue-green {{
            background-color: color-mix(in srgb, #30d158 28%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-yellow,
        .metis-settings-page-icon-wrap.metis-nav-hue-yellow {{
            background-color: color-mix(in srgb, #ffd60a 30%, {raised});
        }}
        .metis-settings-nav-icon-wrap.metis-nav-hue-gray,
        .metis-settings-page-icon-wrap.metis-nav-hue-gray {{
            background-color: color-mix(in srgb, {muted} 22%, {raised});
        }}
        .metis-nav-hue-blue .metis-settings-page-icon {{ color: #0a84ff; }}
        .metis-nav-hue-purple .metis-settings-page-icon {{ color: #bf5af2; }}
        .metis-nav-hue-pink .metis-settings-page-icon {{ color: #ff375f; }}
        .metis-nav-hue-orange .metis-settings-page-icon {{ color: #ff9f0a; }}
        .metis-nav-hue-teal .metis-settings-page-icon {{ color: #64d2ff; }}
        .metis-nav-hue-green .metis-settings-page-icon {{ color: #30d158; }}
        .metis-nav-hue-gray .metis-settings-page-icon {{ color: {muted}; }}
        .metis-nav-hue-yellow .metis-settings-page-icon {{ color: #ffd60a; }}
        .metis-settings-title {{
            font-size: 28px;
            font-weight: 800;
            color: {text};
            letter-spacing: -0.03em;
        }}
        .metis-settings-subtitle {{
            font-size: 13px;
            color: {muted};
            line-height: 1.35;
        }}
        .metis-settings-section {{
            background-color: {surface};
            border: 1px solid {border};
            border-radius: {rl}px;
            padding: 0;
            margin-bottom: 14px;
            box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
        }}
        .metis-settings-section-header {{
            margin: 14px 16px 6px;
        }}
        .metis-settings-section-title {{
            font-size: 11px;
            font-weight: 700;
            color: {muted};
            letter-spacing: 0.05em;
            text-transform: uppercase;
            padding: 14px 16px 8px;
        }}
        .metis-settings-section-header .metis-settings-section-title {{
            padding: 0;
        }}
        .metis-settings-schedule-times {{
            padding: 4px 0 8px;
        }}
        .metis-settings-schedule-popover {{
            padding: 0;
        }}
        .metis-settings-schedule-entry {{
            min-height: 36px;
            font-size: 14px;
            font-feature-settings: "tnum";
        }}
        .metis-settings-schedule-presets {{
            border-radius: {rs}px;
            background-color: {raised};
        }}
        .metis-settings-schedule-list row {{
            min-height: 40px;
        }}
        .metis-settings-schedule-preset-row:hover {{
            background-color: {surface};
        }}
        label.metis-settings-schedule-preset {{
            color: {text};
            font-size: 14px;
            font-weight: 500;
            font-feature-settings: "tnum";
            letter-spacing: 0.01em;
        }}
        .metis-settings-schedule-preset-row:selected label.metis-settings-schedule-preset {{
            color: {on_accent};
            font-weight: 600;
        }}
        .metis-settings-section-body {{
            padding: 0 0 12px;
        }}
        .metis-settings-section-body > .metis-settings-section {{
            margin: 4px 12px 10px;
        }}
        .metis-settings-section-body > .metis-settings-list {{
            margin: 0 12px 12px;
        }}
        .metis-settings-section-body > .metis-settings-inset {{
            margin: 4px 16px 12px;
        }}
        .metis-settings-section-body > .metis-settings-list + .metis-settings-inset {{
            margin-top: 10px;
        }}
        .metis-settings-section-body > .metis-settings-inset + .metis-settings-list {{
            margin-top: 0;
        }}
        .metis-settings-section-body > button {{
            margin: 8px 16px 12px;
        }}
        .metis-settings-section-body > .metis-settings-actions {{
            margin: 8px 16px 12px;
        }}
        .metis-settings-section-body > .metis-settings-hint,
        .metis-settings-section-body > label.metis-settings-hint {{
            padding: 4px 16px 10px;
        }}
        .metis-settings-section-body > label.metis-settings-error {{
            padding: 10px 16px 8px;
            margin: 0;
        }}
        .metis-settings-inset .metis-settings-hint {{
            padding-left: 0;
            padding-right: 0;
        }}
        .metis-settings-inset .metis-settings-row {{
            padding-left: 0;
            padding-right: 0;
        }}
        .metis-settings-inset .metis-settings-actions {{
            margin-top: 4px;
            padding: 0;
        }}
        .metis-settings-banner {{
            margin: 0 16px 12px;
            padding: 12px 14px;
            border-radius: {rs}px;
            background-color: color-mix(in srgb, {raised} 85%, transparent);
            border: 1px solid color-mix(in srgb, {border} 70%, transparent);
        }}
        label.metis-settings-error {{
            color: {error};
            font-size: 13px;
            font-weight: 500;
        }}
        .metis-settings-actions {{
            padding: 4px 16px 14px;
        }}
        .metis-settings-actions button {{
        }}
        .metis-settings-actions > .metis-settings-hint,
        .metis-settings-actions > label.metis-settings-hint {{
            padding: 4px 0 0;
        }}
        .metis-settings-gaming-status {{
            margin: 0 16px 12px;
            padding: 12px 14px;
            border-radius: {rs}px;
            background-color: color-mix(in srgb, {raised} 80%, transparent);
            border: 1px solid color-mix(in srgb, {border} 70%, transparent);
        }}
        .metis-settings-gaming-status.metis-settings-gaming-status-ok {{
            background-color: color-mix(in srgb, {success} 12%, {surface});
            border-color: color-mix(in srgb, {success} 35%, transparent);
        }}
        .metis-settings-gaming-status.metis-settings-gaming-status-warn {{
            background-color: color-mix(in srgb, {warning} 12%, {surface});
            border-color: color-mix(in srgb, {warning} 35%, transparent);
        }}
        .metis-settings-gaming-status-icon {{ -gtk-icon-style: symbolic; }}
        .metis-settings-gaming-status.metis-settings-gaming-status-ok .metis-settings-gaming-status-icon {{
            color: {success};
        }}
        .metis-settings-gaming-status.metis-settings-gaming-status-warn .metis-settings-gaming-status-icon {{
            color: {warning};
        }}
        .metis-settings-gaming-status-text {{
            color: {text};
            font-size: 13px;
            font-weight: 600;
        }}
        .metis-settings-health-grid {{
            padding: 4px 8px 8px;
        }}
        .metis-settings-health-grid .metis-settings-health-item {{
            border-top: none;
            padding: 8px 10px;
            border-radius: {rs}px;
        }}
        .metis-settings-health-grid .metis-settings-health-item .metis-settings-hint {{
            padding: 0;
        }}
        .metis-display-arrangement {{
            padding: 8px 12px 12px;
        }}
        .metis-settings-section-icon {{ color: {accent}; }}
        .metis-settings-row {{
            padding: 10px 16px;
            border-top: 1px solid color-mix(in srgb, {border} 65%, transparent);
            transition: background-color 140ms ease;
        }}
        .metis-settings-section-body > box.metis-settings-row:first-child,
        .metis-settings-section-body > .metis-settings-row:first-child {{
            border-top: none;
        }}
        .metis-settings-row:hover {{ background-color: color-mix(in srgb, {raised} 70%, transparent); }}
        .metis-settings-row label {{ color: {text}; }}
        .metis-settings-row-icon {{ color: {muted}; -gtk-icon-style: symbolic; }}
        .metis-settings-hint {{
            color: {muted};
            font-size: 12px;
            padding: 0 16px 12px;
        }}
        .metis-keybind-chord {{
            font-family: monospace;
            font-size: 12px;
            color: {text};
            background-color: {raised};
            border: 1px solid {border};
            border-radius: {rs}px;
            padding: 4px 8px;
        }}
        .metis-keybind-reserved {{
            opacity: 0.85;
        }}
        .metis-keybind-editable {{
            padding: 0 8px 8px;
        }}
        .metis-keybind-capture {{
            padding: 4px 8px 0;
        }}
        .metis-keybind-capture-entry {{
            font-family: monospace;
            font-size: 12px;
            min-width: 140px;
        }}
        .metis-settings-display-chip {{
            padding: 8px 12px;
            border-radius: {rs}px;
            border: 1px solid {border};
            background-color: {raised};
            background-image: none;
            box-shadow: none;
            color: {text};
        }}
        .metis-settings-display-chip:hover {{ background-color: {surface}; }}
        .metis-settings-display-chip image {{ color: {muted}; -gtk-icon-style: symbolic; }}
        .metis-settings-display-chip label {{ color: {text}; font-size: 13px; }}
        .metis-settings-display-chip-active {{
            border-color: {accent};
            background-color: {surface};
        }}
        .metis-settings-display-chip-active image {{ color: {accent}; }}
        .metis-display-arrangement-canvas {{
            background-color: {raised};
            border: 1px solid {border};
            border-radius: {rl}px;
        }}
        .metis-display-arrangement-viewport {{
            min-width: 200px;
        }}
        /* Cap resolution dropdown natural width so long mode strings can't
           lock the Settings window from shrinking. */
        .metis-settings-shrink-dropdown {{
            min-width: 140px;
        }}
        .metis-display-block {{
            border-radius: {rs}px;
            border: 2px solid transparent;
            background-color: {surface};
            /* Flat tiles — box-shadows hitch the GL scroller when this card
               is in the damaged region (Display page scroll lock-up). */
            box-shadow: none;
            transition: border-color 120ms ease;
        }}
        .metis-display-block-dragging {{
            opacity: 0.96;
        }}
        .metis-display-block-selected {{
            border-color: {accent};
        }}
        .metis-display-block-menubar {{
            background-color: rgba(255, 255, 255, 0.92);
            border-radius: {rs}px {rs}px 0 0;
            min-height: 6px;
        }}
        .metis-display-block-label {{
            color: {text};
            font-size: 11px;
            font-weight: 600;
            padding: 6px 8px;
        }}
        .metis-display-block-0 {{ background-color: color-mix(in srgb, {accent} 22%, {surface}); }}
        .metis-display-block-1 {{ background-color: color-mix(in srgb, #2ec4b6 22%, {surface}); }}
        .metis-display-block-2 {{ background-color: color-mix(in srgb, #e76f51 22%, {surface}); }}
        .metis-display-block-3 {{ background-color: color-mix(in srgb, #9b5de5 22%, {surface}); }}
        .metis-settings-value {{ color: {text}; font-weight: 600; font-feature-settings: "tnum"; }}
        .metis-bt-battery-low {{ color: {warning}; font-weight: 700; }}
        .metis-settings-list {{
            background-color: {raised};
            border: 1px solid {border};
            border-radius: {rl}px;
            padding: 8px 12px;
        }}
        .metis-settings-list row {{ padding: 8px 10px; background-color: transparent; }}
        .metis-settings-list,
        .metis-settings-list row,
        .metis-settings-list label {{ color: {text}; }}
        .metis-settings-list row:hover {{ background-color: {surface}; }}

        /* Zebra lists (Wi-Fi scan, Bluetooth devices) — text-mix works in light + dark. */
        .metis-settings-zebra-list,
        .metis-settings-wifi-list {{
            padding: 6px;
        }}
        .metis-settings-zebra-list > box.metis-settings-zebra-row,
        .metis-settings-wifi-list > box.metis-settings-wifi-row {{
            padding: 8px 10px;
            border-radius: {rs}px;
            border-top: none;
            background-color: transparent;
            min-height: 36px;
        }}
        .metis-settings-zebra-list > box.metis-settings-zebra-row-alt,
        .metis-settings-wifi-list > box.metis-settings-wifi-row-alt {{
            background-color: color-mix(in srgb, {text} 7%, {raised});
        }}
        .metis-settings-zebra-list > box.metis-settings-zebra-row:hover,
        .metis-settings-wifi-list > box.metis-settings-wifi-row:hover {{
            background-color: color-mix(in srgb, {accent} 14%, {raised});
        }}
        .metis-settings-zebra-list > box.metis-settings-zebra-row label,
        .metis-settings-wifi-list > box.metis-settings-wifi-row label {{
            color: {text};
        }}
        .metis-settings-zebra-list > box.metis-settings-zebra-row .metis-settings-hint,
        .metis-settings-wifi-list > box.metis-settings-wifi-row .metis-settings-hint {{
            color: {muted};
            padding: 0;
        }}

        /* App titlebars — virtualized ListView; only visible rows exist. */
        .metis-settings-app-scroll {{
            background-color: transparent;
            border: none;
        }}
        listview.metis-settings-app-list {{
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            padding: 0 0 8px;
            color: {text};
        }}
        listview.metis-settings-app-list > row {{
            background-color: transparent;
            background-image: none;
            border: none;
            border-radius: 0;
            box-shadow: none;
            padding: 0;
            margin: 0;
            min-height: 0;
        }}
        listview.metis-settings-app-list > row:hover {{
            background-color: color-mix(in srgb, {accent} 14%, {raised});
        }}
        listview.metis-settings-app-list .metis-settings-app-row-odd {{
            background-color: color-mix(in srgb, {raised} 72%, transparent);
        }}
        listview.metis-settings-app-list > row .metis-settings-row {{
            border-top: none;
            background-color: transparent;
            padding: 8px 16px;
        }}
        listview.metis-settings-app-list > row .metis-settings-row:hover {{
            background-color: transparent;
        }}
        listview.metis-settings-app-list label {{
            color: {text};
        }}
        listview.metis-settings-app-list .metis-settings-app-badge {{
            color: {muted};
            font-size: 12px;
            padding: 0;
        }}
        listview.metis-settings-app-list dropdown,
        listview.metis-settings-app-list dropdown > button {{
            background-color: {raised};
            color: {text};
            border: 1px solid {border};
        }}

        /* Legacy ListBox selectors kept for any leftover lists. */
        list.metis-settings-app-list {{
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            padding: 0 0 8px;
            color: {text};
        }}
        list.metis-settings-app-list > row {{
            background-color: transparent;
            background-image: none;
            border: none;
            border-radius: 0;
            box-shadow: none;
            padding: 0;
            margin: 0;
            min-height: 0;
        }}
        list.metis-settings-app-list > row:nth-child(odd) {{
            background-color: color-mix(in srgb, {raised} 72%, transparent);
        }}
        list.metis-settings-app-list > row:nth-child(even) {{
            background-color: transparent;
        }}
        list.metis-settings-app-list > row:hover {{
            background-color: color-mix(in srgb, {accent} 14%, {raised});
        }}
        list.metis-settings-app-list > row .metis-settings-row {{
            border-top: none;
            background-color: transparent;
            padding: 8px 16px;
        }}
        list.metis-settings-app-list > row .metis-settings-row:hover {{
            background-color: transparent;
        }}
        list.metis-settings-app-list label {{
            color: {text};
        }}
        list.metis-settings-app-list .metis-settings-app-badge {{
            color: {muted};
            font-size: 12px;
            padding: 0;
        }}
        list.metis-settings-app-list dropdown,
        list.metis-settings-app-list dropdown > button {{
            background-color: {raised};
            color: {text};
            border: 1px solid {border};
        }}

        /* Dropdowns (e.g. the Mode selector) + their popups. */
        dropdown, dropdown > button {{
            background-color: {raised};
            background-image: none;
            color: {text};
            border: 1px solid {border};
            border-radius: {rs}px;
            box-shadow: none;
        }}
        dropdown > button:hover {{ background-color: {surface}; }}
        dropdown arrow, dropdown button image {{ color: {text}; }}
        popover > contents, popover.background > contents, popover.menu > contents {{
            background-color: {raised};
            color: {text};
            border: 1px solid {border};
            border-radius: {rs}px;
        }}
        popover listview,
        popover listview row,
        popover.menu listview,
        popover.menu listview row,
        popover row {{
            background-color: transparent;
            color: {text};
        }}
        popover listview row label,
        popover.menu listview row label,
        popover row label,
        popover label {{
            color: {text};
        }}
        /* Solid accent selection — force label/image ink to contrast (the plain
           `popover label {{ color: text }}` rule otherwise keeps white on cyan). */
        popover listview row:selected,
        popover listview row:selected:hover,
        popover.menu listview row:selected,
        popover.menu listview row:selected:hover,
        popover row:selected,
        popover row:selected:hover {{
            background-color: {accent};
            color: {on_accent};
        }}
        popover listview row:selected label,
        popover listview row:selected cell,
        popover listview row:selected image,
        popover.menu listview row:selected label,
        popover.menu listview row:selected image,
        popover row:selected label,
        popover row:selected image {{
            color: {on_accent};
        }}
        /* Hover (not selected): soft tint + normal text — always readable. */
        popover listview row:hover,
        popover.menu listview row:hover,
        popover row:hover {{
            background-color: color-mix(in srgb, {accent} 22%, {raised});
            color: {text};
        }}
        popover listview row:hover label,
        popover.menu listview row:hover label,
        popover row:hover label {{
            color: {text};
        }}
        popover listview row:selected:hover label,
        popover.menu listview row:selected:hover label,
        popover row:selected:hover label {{
            color: {on_accent};
        }}

        /* Sliders + switches — tokenized so light mode does not keep Adwaita dark chrome. */
        scale trough {{ background-color: {border}; }}
        scale highlight {{ background-color: {accent}; }}
        scale value {{ color: {muted}; }}
        scale slider {{
            background-color: {accent};
            border-radius: 999px;
            min-width: 16px;
            min-height: 16px;
            border: none;
            box-shadow: none;
        }}
        switch {{
            background-color: rgba({text_rgb}, 0.14);
            border: none;
            border-radius: 999px;
            min-width: 40px;
            min-height: 22px;
            transition: none;
        }}
        switch:checked {{
            background-color: {accent};
            background-image: linear-gradient(135deg, {accent}, {accent2});
        }}
        switch > slider {{
            background-color: {surface};
            border-radius: 999px;
            min-width: 18px;
            min-height: 18px;
            box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
            transition: none;
        }}
        label.metis-settings-switch-label {{
        }}

        /* Text inputs (search boxes, CalDAV fields, etc.). */
        entry, entry.flat, spinbutton {{
            background-color: {raised};
            background-image: none;
            color: {text};
            border: 1px solid {border};
            border-radius: {rs}px;
            box-shadow: none;
            caret-color: {text};
            padding: 6px 10px;
            min-height: 32px;
        }}
        entry text, spinbutton text {{ color: {text}; background-color: transparent; }}
        entry text placeholder, entry > text > placeholder {{ color: {muted}; opacity: 1; }}
        entry:focus-within {{ border-color: {accent}; }}
        entry image, entry > image {{ color: {muted}; }}

        /* Generic buttons (Search, Rescan, Connect, trash, …). The more specific
           headerbar/dropdown rules above keep their own styling. */
        .metis-settings-window button,
        button {{
            background-color: {raised};
            background-image: none;
            background: {raised};
            color: {text};
            border: 1px solid {border};
            border-radius: {rs}px;
            box-shadow: none;
            transition: background-color 140ms ease, border-color 140ms ease, transform 100ms ease;
        }}
        .metis-settings-window button:hover,
        button:hover {{
            background-color: color-mix(in srgb, {raised} 82%, {text} 18%);
            background: color-mix(in srgb, {raised} 82%, {text} 18%);
            color: {text};
        }}
        .metis-settings-window button:hover label,
        button:hover label {{
            color: {text};
        }}
        .metis-settings-window button:active,
        .metis-settings-window button:checked,
        button:active, button:checked {{ background-color: {surface}; background: {surface}; transform: scale(0.98); }}
        button label {{ color: {text}; }}
        button image {{ color: {text}; }}
        button:disabled {{ color: {muted}; }}
        button:disabled label {{ color: {muted}; }}
        button.metis-settings-secondary {{
            background-color: {raised};
            background: {raised};
            border: 1px solid {border};
            color: {text};
        }}
        /* Primary action buttons stay accent-coloured (beat generic button:hover). */
        .metis-settings-window button.suggested-action,
        button.suggested-action {{
            background-color: {accent};
            background: {accent};
            border-color: {accent};
            color: {on_accent};
            box-shadow: 0 2px 8px color-mix(in srgb, {accent} 35%, transparent);
        }}
        .metis-settings-window button.suggested-action:hover,
        button.suggested-action:hover {{
            background-color: color-mix(in srgb, {accent} 82%, white);
            background: color-mix(in srgb, {accent} 82%, white);
            color: {on_accent};
        }}
        .metis-settings-window button.suggested-action label,
        .metis-settings-window button.suggested-action:hover label,
        button.suggested-action label,
        button.suggested-action:hover label {{
            color: {on_accent};
        }}
        button.destructive-action image {{ color: {error}; }}
        /* Flat buttons (Add Picture…) have no chrome until hovered. */
        button.flat {{ background-color: transparent; border-color: transparent; }}
        button.flat:hover {{ background-color: {raised}; }}

        /* Appearance · Style preview buttons */
        .metis-style-button {{
            background-color: transparent;
            background-image: none;
            border: 2px solid transparent;
            border-radius: {rl}px;
            padding: 6px;
            box-shadow: none;
        }}
        .metis-style-button:hover {{ background-color: {raised}; }}
        .metis-style-button:checked, .metis-style-button:active {{
            background-color: transparent;
            border-color: {accent};
        }}
        .metis-style-preview {{ border-radius: 10px; background-color: {surface}; }}
        .metis-style-preview picture {{ border-radius: 10px; }}
        .metis-style-fallback-light {{ background-color: #f2f2f4; }}
        .metis-style-fallback-dark {{ background-color: #1c1c20; }}
        .metis-style-mock-light {{
            background-color: #ffffff;
            border-radius: 7px;
            border-top: 9px solid #e6e6e9;
            box-shadow: 0 3px 8px rgba(0,0,0,0.35);
        }}
        .metis-style-mock-dark {{
            background-color: #2b2b30;
            border-radius: 7px;
            border-top: 9px solid #3a3a40;
            box-shadow: 0 3px 8px rgba(0,0,0,0.45);
        }}
        .metis-style-caption {{ color: {text}; font-weight: 600; }}

        /* Appearance · Wallpaper grid */
        .metis-wallpaper-grid {{ padding: 4px; }}
        .metis-wallpaper-grid flowboxchild {{
            padding: 0;
            background-color: transparent;
            border-radius: 10px;
        }}
        .metis-wallpaper-thumb {{
            background-color: transparent;
            background-image: none;
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 0;
            box-shadow: none;
        }}
        .metis-wallpaper-thumb:hover {{ border-color: {border}; background-color: transparent; }}
        .metis-wallpaper-thumb.selected {{ border-color: {accent}; }}
        .metis-wallpaper-image {{ border-radius: 8px; }}
        .metis-wallpaper-thumb-spinner {{
            color: {accent};
        }}
        .metis-wallpaper-check {{
            color: {on_accent};
            background-color: {accent};
            border-radius: 999px;
            padding: 4px;
        }}

        .metis-settings-row colorswatch {{
            border-radius: 6px;
            min-width: 48px;
            min-height: 22px;
            border: 1px solid {border};
            box-shadow: none;
            padding: 0;
        }}
        /* Compact ColorDialogButton — default Adwaita chrome is oversized and
           stacks a second frame around the swatch. */
        colordialogbutton,
        .metis-settings-row colordialogbutton {{
            min-width: 56px;
            min-height: 28px;
            padding: 0;
            border-radius: {rs}px;
            background-color: {raised};
            background-image: none;
            border: 1px solid {border};
            box-shadow: none;
        }}
        colordialogbutton > button,
        .metis-settings-row colordialogbutton > button {{
            min-width: 0;
            min-height: 0;
            padding: 3px;
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            transform: none;
        }}
        colordialogbutton > button:hover,
        colordialogbutton > button:active,
        colordialogbutton > button:checked {{
            background-color: transparent;
            transform: none;
            box-shadow: none;
        }}
        colordialogbutton colorswatch {{
            min-width: 48px;
            min-height: 22px;
            border-radius: 4px;
            border: 1px solid {border};
            box-shadow: none;
        }}
        button.metis-accent2-hint {{ color: {accent2}; }}

        /* Segmented pill tabs (e.g. Network: Wireless / Wired / Proxy). */
        .metis-settings-tabs {{
            padding: 3px;
            background-color: {raised};
            border: 1px solid {border};
            border-radius: 999px;
        }}
        button.metis-settings-tab {{
            padding: 6px 18px;
            min-height: 0;
            border-radius: 999px;
            border: 1px solid transparent;
            background-color: transparent;
            background-image: none;
            box-shadow: none;
            color: {muted};
        }}
        button.metis-settings-tab:hover {{
            background-color: {surface};
            color: {text};
        }}
        button.metis-settings-tab:checked,
        button.metis-settings-tab:active {{
            background-color: {accent};
            border-color: {accent};
            color: {on_accent};
        }}
        button.metis-settings-tab:checked label {{ color: {on_accent}; font-weight: 700; }}

        /* ---- Settings UI 2.0: Home + mini sidebar + sheets + top dialogs ---- */
        .metis-settings-mini-sidebar {{
            background-color: {surface};
            min-width: 64px;
            padding: 4px 0;
        }}
        .metis-settings-mini-btn {{
            background-color: transparent;
            background-image: none;
            background: transparent;
            border: none;
            border-radius: 12px;
            padding: 4px;
            min-width: 0;
            min-height: 0;
            box-shadow: none;
            transform: none;
        }}
        .metis-settings-mini-btn:hover {{
            background-color: color-mix(in srgb, {accent} 12%, {raised});
            background: color-mix(in srgb, {accent} 12%, {raised});
        }}
        .metis-settings-window button.metis-settings-mini-btn:active,
        .metis-settings-window button.metis-settings-mini-btn:focus,
        button.metis-settings-mini-btn:active,
        button.metis-settings-mini-btn:focus {{
            background-color: color-mix(in srgb, {accent} 16%, {raised});
            background: color-mix(in srgb, {accent} 16%, {raised});
            transform: none;
            outline: none;
            box-shadow: none;
        }}
        .metis-settings-mini-btn.metis-settings-mini-active {{
            background-color: color-mix(in srgb, {accent} 20%, {raised});
            background: color-mix(in srgb, {accent} 20%, {raised});
        }}
        .metis-settings-mini-badge {{
            border-radius: 10px;
            padding: 8px;
        }}
        .metis-settings-mini-icon {{
            color: {text};
            -gtk-icon-style: symbolic;
        }}
        .metis-settings-mini-home .metis-settings-mini-badge {{
            background-color: color-mix(in srgb, {muted} 18%, {raised});
        }}

        .metis-settings-home {{
            background-color: {bg};
        }}
        .metis-settings-home-title {{
            font-size: 28px;
            font-weight: 800;
            letter-spacing: -0.03em;
            color: {text};
        }}
        .metis-settings-home-subtitle {{
            font-size: 14px;
            color: {muted};
        }}
        .metis-settings-home-search {{
        }}
        .metis-settings-home-section {{
            font-size: 12px;
            font-weight: 700;
            letter-spacing: 0.06em;
            text-transform: uppercase;
            color: {muted};
            margin-top: 8px;
        }}
        .metis-settings-home-tile {{
            background-color: {surface};
            background-image: none;
            background: {surface};
            border: 1px solid {border};
            border-radius: {rl}px;
            box-shadow: 0 1px 2px alpha(black, 0.06);
            transition: border-color 140ms ease, box-shadow 140ms ease;
            padding: 0;
            transform: none;
        }}
        .metis-settings-home-tile:hover {{
            border-color: color-mix(in srgb, {accent} 45%, {border});
            box-shadow: 0 6px 18px alpha(black, 0.12);
            background-color: {surface};
            background: {surface};
            transform: none;
        }}
        /* Kill the default GTK/Adwaita pressed flash (often reads as a solid
           accent/pink block over the whole tile before the page slides). */
        .metis-settings-window button.metis-settings-home-tile:active,
        .metis-settings-window button.metis-settings-home-tile:checked,
        .metis-settings-window button.metis-settings-home-tile:focus,
        .metis-settings-window button.metis-settings-home-tile:focus-visible,
        button.metis-settings-home-tile:active,
        button.metis-settings-home-tile:checked,
        button.metis-settings-home-tile:focus,
        button.metis-settings-home-tile:focus-visible {{
            background-color: {surface};
            background-image: none;
            background: {surface};
            border-color: color-mix(in srgb, {accent} 45%, {border});
            box-shadow: 0 1px 2px alpha(black, 0.06);
            transform: none;
            outline: none;
        }}
        .metis-settings-home-tile-badge {{
            border-radius: 12px;
            padding: 10px;
        }}
        .metis-settings-home-tile-icon {{
            color: {text};
            -gtk-icon-style: symbolic;
        }}
        .metis-settings-home-tile-title {{
            font-size: 15px;
            font-weight: 700;
            color: {text};
        }}
        .metis-settings-home-tile-blurb {{
            font-size: 12px;
            color: {muted};
        }}
        .metis-settings-home-result {{
            background-color: {surface};
            background-image: none;
            background: {surface};
            border: 1px solid {border};
            border-radius: {rl}px;
            padding: 0;
            box-shadow: none;
            transform: none;
        }}
        .metis-settings-home-result:hover {{
            border-color: color-mix(in srgb, {accent} 40%, {border});
            background-color: color-mix(in srgb, {accent} 8%, {surface});
            background: color-mix(in srgb, {accent} 8%, {surface});
        }}
        .metis-settings-window button.metis-settings-home-result:active,
        .metis-settings-window button.metis-settings-home-result:focus,
        button.metis-settings-home-result:active,
        button.metis-settings-home-result:focus {{
            background-color: {surface};
            background: {surface};
            transform: none;
            outline: none;
        }}
        .metis-settings-home-result-title {{
            font-size: 13px;
            font-weight: 600;
            color: {text};
        }}
        .metis-settings-home-result-sub {{
            font-size: 11px;
            color: {muted};
        }}

        .metis-settings-sheet-dimmer {{
            background-color: alpha(black, 0.38);
            background-image: none;
            border: none;
            border-radius: 0;
            box-shadow: none;
            opacity: 1;
        }}
        .metis-settings-sheet-dimmer:hover {{
            background-color: alpha(black, 0.38);
        }}
        .metis-settings-category-sheet {{
            background-color: {bg};
            border: none;
            box-shadow: none;
            min-width: 0;
        }}
        .metis-settings-sheet-title {{
            font-size: 18px;
            font-weight: 750;
            color: {text};
            letter-spacing: -0.02em;
        }}
        .metis-settings-sheet-close {{
            min-width: 34px;
            min-height: 34px;
            padding: 0;
            border-radius: 999px;
        }}
        .metis-settings-sheet-pages-scroll,
        .metis-settings-sheet-pages-scroll > viewport,
        .metis-settings-sheet-pages-scroll viewport {{
            background-color: {surface};
            color: {text};
        }}
        /* Class is on the ListBox itself (`list.metis-…`), not a child `list`. */
        list.metis-settings-sheet-pages,
        .metis-settings-sheet-pages {{
            background-color: {surface};
            background-image: none;
            color: {text};
            padding: 8px;
            border: none;
            box-shadow: none;
        }}
        list.metis-settings-sheet-pages > row,
        .metis-settings-sheet-page-row {{
            border-radius: {rl}px;
            margin: 2px 0;
            background-color: transparent;
            color: {text};
        }}
        list.metis-settings-sheet-pages > row:hover,
        .metis-settings-sheet-page-row:hover {{
            background-color: {raised};
        }}
        list.metis-settings-sheet-pages > row:selected,
        .metis-settings-sheet-page-row:selected {{
            background-color: color-mix(in srgb, {accent} 18%, {raised});
        }}
        list.metis-settings-sheet-pages > row label,
        .metis-settings-sheet-page-label {{
            font-size: 12px;
            font-weight: 550;
            color: {text};
        }}

        .metis-settings-dialog-dimmer {{
            background-color: alpha(black, 0.45);
            background-image: none;
            border: none;
            border-radius: 0;
            box-shadow: none;
        }}
        .metis-settings-dialog-dimmer:hover {{
            background-color: alpha(black, 0.45);
        }}
        .metis-settings-top-dialog {{
            background-color: {surface};
            border: 1px solid {border};
            border-radius: 14px;
            padding: 16px 18px 14px;
            box-shadow: 0 12px 40px alpha(black, 0.35);
            min-width: 400px;
            max-width: 480px;
        }}
        .metis-settings-top-dialog-title {{
            font-size: 16px;
            font-weight: 700;
            color: {text};
        }}
        .metis-settings-top-dialog-body {{
            font-size: 13px;
            color: {muted};
            line-height: 1.45;
        }}
        .metis-settings-top-sheet-body {{
            background-color: {surface};
            color: {text};
        }}
        button.metis-color-swatch {{
            min-width: 56px;
            min-height: 28px;
            padding: 3px;
            border-radius: {rs}px;
            background-color: {raised};
            background-image: none;
            border: 1px solid {border};
            box-shadow: none;
        }}
        button.metis-color-swatch:hover {{
            border-color: {accent};
        }}
        box.metis-color-swatch-chip {{
            border: 1px solid {border};
            border-radius: 4px;
        }}
        button.metis-font-picker {{
            min-height: 28px;
            padding: 4px 10px;
            border-radius: {rs}px;
            background-color: {raised};
            background-image: none;
            border: 1px solid {border};
            box-shadow: none;
            color: {text};
        }}
        button.metis-font-picker:hover {{
            border-color: {accent};
        }}
        .metis-font-picker-label {{
            color: {text};
        }}
        "#
    )
}
