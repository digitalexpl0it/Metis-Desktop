//! Appearance: theme mode (light/dark), accent + semantic colours (written to
//! `themes/*.json`), and the interface font. The shell's file watchers apply
//! colour changes live; a `reload-theme` runtime command re-themes on a mode
//! switch (since the mode lives in `config.json`, which isn't watched).
//!
//! Wallpaper, edge bar, and window decoration each have their own sibling page
//! (`background`, `edgebar`, `windows`).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;

use metis_config::ThemeMode;

use crate::pages::appearance_common::{
    color_dialog_button, current_wallpaper, font_picker_button, hex_to_rgba, rgba_to_hex,
    ColorSwatchButton,
};
use crate::{runtime, ui};
use metis_i18n::tr;

struct State {
    name: String,
    tokens: metis_config::ThemeTokens,
}

#[derive(Clone)]
struct ColorButtons {
    accent: ColorSwatchButton,
    accent2: ColorSwatchButton,
    error: ColorSwatchButton,
    warning: ColorSwatchButton,
    success: ColorSwatchButton,
    info: ColorSwatchButton,
    payment: ColorSwatchButton,
    text: ColorSwatchButton,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("appearance");

    let mode = metis_config::load_theme_preference().unwrap_or(ThemeMode::Dark);
    let name = effective_name(&mode);
    let state = Rc::new(RefCell::new(State {
        tokens: metis_config::load_theme_tokens(&name),
        name,
    }));
    // Guards programmatic `set_rgba` (during refresh) from triggering a save.
    let suppress = Rc::new(Cell::new(false));
    // Style buttons fire `toggled` when we set the initial active state below.
    let suppress_mode = Rc::new(Cell::new(true));

    // Current wallpaper drives the live preview thumbnails in the style buttons.
    let current_wp = current_wallpaper();

    // ---- Style (light / dark) --------------------------------------------
    let (mode_card, mode_body) =
        ui::section_with_icon(&tr("Style"), "applications-graphics-symbolic");
    let chooser = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    chooser.set_halign(gtk::Align::Center);
    chooser.set_margin_top(4);
    chooser.set_margin_bottom(4);
    let light_btn = style_button("Light", false, current_wp.as_deref());
    let dark_btn = style_button("Dark", true, current_wp.as_deref());
    dark_btn.set_group(Some(&light_btn));
    match mode {
        ThemeMode::Light => light_btn.set_active(true),
        // System falls back to Dark in the picker (auto-detect was unreliable).
        _ => dark_btn.set_active(true),
    }
    suppress_mode.set(false);
    chooser.append(&light_btn);
    chooser.append(&dark_btn);
    mode_body.append(&chooser);
    content.append(&mode_card);

    // ---- Colors -----------------------------------------------------------
    let (color_card, color_body) =
        ui::section_with_icon(&tr("Colors"), "applications-graphics-symbolic");
    let buttons = ColorButtons {
        accent: color_dialog_button(),
        accent2: color_dialog_button(),
        error: color_dialog_button(),
        warning: color_dialog_button(),
        success: color_dialog_button(),
        info: color_dialog_button(),
        payment: color_dialog_button(),
        text: color_dialog_button(),
    };
    color_body.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Accent"),
        &buttons.accent,
    ));
    color_body.append(&ui::row_with_icon(
        "starred-symbolic",
        &tr("Accent (secondary)"),
        &buttons.accent2,
    ));
    color_body.append(&ui::row_with_icon(
        "dialog-error-symbolic",
        &tr("Error"),
        &buttons.error,
    ));
    color_body.append(&ui::row_with_icon(
        "dialog-warning-symbolic",
        &tr("Warning"),
        &buttons.warning,
    ));
    color_body.append(&ui::row_with_icon(
        "emblem-ok-symbolic",
        &tr("Success"),
        &buttons.success,
    ));
    color_body.append(&ui::row_with_icon(
        "dialog-information-symbolic",
        &tr("Info"),
        &buttons.info,
    ));
    color_body.append(&ui::row_with_icon(
        "emblem-system-symbolic",
        &tr("Payment"),
        &buttons.payment,
    ));
    content.append(&color_card);

    refresh_buttons(&buttons, &state.borrow().tokens, &suppress);

    // Wire each color button to its token field. Changing the primary accent also
    // re-derives the on-accent text color so labels stay readable on it (e.g. a
    // black accent flips on-accent text to white).
    wire_color(&buttons.accent, &state, &suppress, |t, hex| {
        t.text_on_accent = metis_config::ThemeTokens::contrast_ink(&hex);
        set_accent(t, 0, hex);
    });
    wire_color(&buttons.accent2, &state, &suppress, |t, hex| {
        set_accent(t, 1, hex)
    });
    wire_color(&buttons.error, &state, &suppress, |t, hex| {
        t.semantic.error = hex
    });
    wire_color(&buttons.warning, &state, &suppress, |t, hex| {
        t.semantic.warning = hex
    });
    wire_color(&buttons.success, &state, &suppress, |t, hex| {
        t.semantic.success = hex
    });
    wire_color(&buttons.info, &state, &suppress, |t, hex| {
        t.semantic.info = hex
    });
    wire_color(&buttons.payment, &state, &suppress, |t, hex| {
        t.semantic.payment = hex
    });

    // ---- Font -------------------------------------------------------------
    // Text color edits the theme's `text` token (recolors body text DE-wide).
    wire_color(&buttons.text, &state, &suppress, |t, hex| t.text = hex);

    let (font_card, font_body) = ui::section_with_icon(&tr("Font"), "font-x-generic-symbolic");
    let font_btn = font_picker_button();
    {
        let st = state.borrow();
        let fam = st.tokens.font_family.trim();
        let pt = st.tokens.font_size_pt;
        if !fam.is_empty() || pt > 0 {
            let mut desc = gtk::pango::FontDescription::new();
            if !fam.is_empty() {
                desc.set_family(fam);
            }
            let size_pt = if pt > 0 { pt } else { 11 };
            desc.set_size(size_pt as i32 * gtk::pango::SCALE);
            font_btn.set_font_desc(&desc);
        }
    }
    font_body.append(&ui::row_with_icon(
        "font-x-generic-symbolic",
        &tr("Family & size"),
        &font_btn,
    ));
    font_body.append(&ui::row_with_icon(
        "applications-graphics-symbolic",
        &tr("Text color"),
        &buttons.text,
    ));
    content.append(&font_card);
    {
        let state = state.clone();
        let suppress_font = Rc::new(Cell::new(true));
        let suppress_font_flag = suppress_font.clone();
        font_btn.connect_font_desc_notify(move |btn| {
            if suppress_font_flag.get() {
                return;
            }
            let desc = btn.font_desc();
            let family = desc
                .as_ref()
                .and_then(|d| d.family())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let size = desc.as_ref().map(|d| d.size()).unwrap_or(0);
            let pt = if size > 0 {
                (size / gtk::pango::SCALE) as u32
            } else {
                0
            };
            let name = {
                let mut s = state.borrow_mut();
                s.tokens.font_family = family;
                s.tokens.font_size_pt = pt;
                s.name.clone()
            };
            let tokens = state.borrow().tokens.clone();
            if let Err(err) = metis_config::save_theme_tokens(&name, &tokens) {
                tracing::warn!(%err, "failed to save theme tokens");
            }
            crate::theme::reapply();
            runtime::send("reload-theme");
        });
        suppress_font.set(false);
    }

    // Style buttons: persist preference, re-target the colour editor, re-theme.
    let apply_mode: Rc<dyn Fn(ThemeMode)> = {
        let state = state.clone();
        let buttons = buttons.clone();
        let suppress = suppress.clone();
        let suppress_mode = suppress_mode.clone();
        let save_error = gtk::Label::new(None);
        save_error.set_xalign(0.0);
        save_error.set_wrap(true);
        save_error.add_css_class("metis-settings-error");
        save_error.set_visible(false);
        mode_body.append(&save_error);
        let save_error = save_error.clone();
        Rc::new(move |mode: ThemeMode| {
            if suppress_mode.get() {
                return;
            }
            if let Err(err) = metis_config::save_theme_preference(mode) {
                tracing::warn!(%err, "failed to save theme preference");
                // Stamp still written inside save_theme_preference before the
                // config.json write — but if ensure_config_dirs failed earlier,
                // force the stamp so Viewer can follow this click.
                let _ = metis_config::write_appearance_mode_stamp(mode);
                save_error.set_text(&format!(
                    "{} ({err}). {} ~/.config/metis (e.g. sudo chown -R \"$USER:$USER\" ~/.config/metis).",
                    tr("Could not save theme preference"),
                    tr("Check ownership of"),
                ));
                save_error.set_visible(true);
                // Apply the chosen mode live even when disk is unwritable — otherwise
                // reapply() would re-read stale "dark" from config.json.
                metis_config::apply_session_appearance_gsettings(mode);
                let name = effective_name(&mode);
                let tokens = metis_config::load_theme_tokens(&name);
                refresh_buttons(&buttons, &tokens, &suppress);
                {
                    let mut s = state.borrow_mut();
                    s.name = name;
                    s.tokens = tokens;
                }
                crate::theme::reapply_for_mode(mode);
                runtime::send("reload-theme");
                return;
            }
            save_error.set_visible(false);
            save_error.set_text("");
            metis_config::apply_session_appearance_gsettings(mode);
            let name = effective_name(&mode);
            let tokens = metis_config::load_theme_tokens(&name);
            refresh_buttons(&buttons, &tokens, &suppress);
            {
                let mut s = state.borrow_mut();
                s.name = name;
                s.tokens = tokens;
            }
            // Re-theme this settings window (incl. titlebar) live, then nudge the
            // shell/compositor to reload too.
            crate::theme::reapply();
            runtime::send("reload-theme");
        })
    };
    {
        let apply_mode = apply_mode.clone();
        light_btn.connect_toggled(move |b| {
            if b.is_active() {
                apply_mode(ThemeMode::Light);
            }
        });
    }
    {
        let apply_mode = apply_mode.clone();
        dark_btn.connect_toggled(move |b| {
            if b.is_active() {
                apply_mode(ThemeMode::Dark);
            }
        });
    }

    // ---- Re-run first-run wizard ------------------------------------------
    let (setup_card, setup_body) = ui::section_with_icon(&tr("Setup"), "system-run-symbolic");
    let setup_hint = gtk::Label::new(Some(&tr(
        "Reopen the first-run setup wizard to walk through theme, wallpaper, \
         clock, edge bar, and weather again.",
    )));
    setup_hint.set_xalign(0.0);
    setup_hint.set_wrap(true);
    setup_hint.add_css_class("metis-settings-hint");
    setup_body.append(&setup_hint);
    let setup_btn = gtk::Button::with_label(&tr("Run setup again"));
    setup_btn.set_halign(gtk::Align::Start);
    setup_btn.set_margin_top(8);
    setup_btn.connect_clicked(|_| runtime::send("show-onboarding"));
    setup_body.append(&setup_btn);
    content.append(&setup_card);

    scroller.upcast()
}

fn set_accent(tokens: &mut metis_config::ThemeTokens, idx: usize, hex: String) {
    while tokens.accent.len() <= idx {
        tokens.accent.push(hex.clone());
    }
    tokens.accent[idx] = hex;
}

fn wire_color<F>(
    button: &ColorSwatchButton,
    state: &Rc<RefCell<State>>,
    suppress: &Rc<Cell<bool>>,
    apply: F,
) where
    F: Fn(&mut metis_config::ThemeTokens, String) + 'static,
{
    let state = state.clone();
    let suppress = suppress.clone();
    button.connect_rgba_notify(move |btn| {
        if suppress.get() {
            return;
        }
        let hex = rgba_to_hex(&btn.rgba());
        let name = {
            let mut s = state.borrow_mut();
            apply(&mut s.tokens, hex);
            s.name.clone()
        };
        let tokens = state.borrow().tokens.clone();
        if let Err(err) = metis_config::save_theme_tokens(&name, &tokens) {
            tracing::warn!(%err, "failed to save theme tokens");
        }
        crate::theme::reapply();
        runtime::send("reload-theme");
    });
}

fn refresh_buttons(b: &ColorButtons, t: &metis_config::ThemeTokens, suppress: &Rc<Cell<bool>>) {
    suppress.set(true);
    b.accent.set_rgba(&hex_to_rgba(t.accent_primary()));
    b.accent2.set_rgba(&hex_to_rgba(t.accent_secondary()));
    b.error.set_rgba(&hex_to_rgba(&t.semantic.error));
    b.warning.set_rgba(&hex_to_rgba(&t.semantic.warning));
    b.success.set_rgba(&hex_to_rgba(&t.semantic.success));
    b.info.set_rgba(&hex_to_rgba(&t.semantic.info));
    b.payment.set_rgba(&hex_to_rgba(&t.semantic.payment));
    b.text.set_rgba(&hex_to_rgba(&t.text));
    suppress.set(false);
}

fn effective_name(mode: &ThemeMode) -> String {
    match mode {
        ThemeMode::Light => "light".to_string(),
        ThemeMode::Dark => "dark".to_string(),
        ThemeMode::System => {
            let dark = gtk::Settings::default()
                .map(|s| s.is_gtk_application_prefer_dark_theme())
                .unwrap_or(true);
            if dark { "dark" } else { "light" }.to_string()
        }
    }
}

// ---- Style preview buttons ------------------------------------------------

/// A large toggle showing the current wallpaper with a mock window in the given
/// (light/dark) tone, plus a caption — mirrors GNOME's Style chooser.
fn style_button(label: &str, dark: bool, wallpaper: Option<&Path>) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("metis-style-button");

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let overlay = gtk::Overlay::new();
    overlay.add_css_class("metis-style-preview");
    overlay.set_size_request(150, 94);

    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_size_request(150, 94);
    if let Some(path) = wallpaper {
        pic.set_filename(Some(path));
    } else {
        pic.add_css_class(if dark {
            "metis-style-fallback-dark"
        } else {
            "metis-style-fallback-light"
        });
    }
    overlay.set_child(Some(&pic));

    // Mock window floated over the wallpaper to convey the light/dark surface.
    let mock = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mock.add_css_class(if dark {
        "metis-style-mock-dark"
    } else {
        "metis-style-mock-light"
    });
    mock.set_halign(gtk::Align::Center);
    mock.set_valign(gtk::Align::Center);
    mock.set_size_request(82, 52);
    overlay.add_overlay(&mock);

    vbox.append(&overlay);

    let caption = gtk::Label::new(Some(&tr(label)));
    caption.add_css_class("metis-style-caption");
    vbox.append(&caption);

    btn.set_child(Some(&vbox));
    btn
}
