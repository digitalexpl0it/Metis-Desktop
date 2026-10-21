use std::cell::{Cell, RefCell};

use gtk::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, STYLE_PROVIDER_PRIORITY_USER};

use crate::config;
use metis_config::{
    BarBorder, BarFill, BarFillMode, BarGradientDirection, BarPosition, BorderMode, ThemeTokens,
    build_stylesheet, parse_hex_rgb,
};

thread_local! {
    static THEME_STATE: RefCell<ThemeState> = RefCell::new(ThemeState {
        tokens: ThemeTokens::dark_default(),
        provider: CssProvider::new(),
    });
    static BAR_BG_PROVIDER: CssProvider = CssProvider::new();
    static BAR_OPACITY: Cell<f32> = const { Cell::new(1.0) };
    static BAR_BORDER: RefCell<BarBorder> = RefCell::new(BarBorder::default());
    static BAR_FILL: RefCell<BarFill> = RefCell::new(BarFill::default());
    static BAR_POSITION: Cell<BarPosition> = const { Cell::new(BarPosition::Top) };
    static MENU_BG_PROVIDER: CssProvider = CssProvider::new();
    static MENU_OPACITY: Cell<f32> = const { Cell::new(1.0) };
    /// Wallpaper-sampled lift for CC / NC (Phase 21 §D).
    static AMBIENT_PROVIDER: CssProvider = CssProvider::new();
    static AMBIENT_RGB: Cell<Option<(u8, u8, u8)>> = const { Cell::new(None) };
    static AMBIENT_ENABLED: Cell<bool> = const { Cell::new(true) };
}

struct ThemeState {
    tokens: ThemeTokens,
    provider: CssProvider,
}

pub fn init_theme() -> ThemeTokens {
    // Keep org.gnome.desktop.interface in sync for Nautilus / other GTK apps.
    metis_config::sync_session_appearance_from_config();
    apply_icon_theme_from_config();
    AMBIENT_ENABLED.with(|e| e.set(metis_config::load_bar_config().ambient_tint));
    let tokens = load_active_theme();
    apply_tokens(&tokens);
    tokens
}

/// Apply Freedesktop icon theme from `icons.json` to this process's GTK settings.
pub fn apply_icon_theme_from_config() {
    let cfg = metis_config::load_icons_config();
    let Some(theme) = cfg.theme.as_deref().filter(|t| !t.is_empty()) else {
        return;
    };
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_icon_theme_name(Some(theme));
    }
    metis_config::apply_session_icon_theme(theme);
}

pub fn active_tokens() -> ThemeTokens {
    THEME_STATE.with(|state| state.borrow().tokens.clone())
}

fn load_active_theme() -> ThemeTokens {
    metis_config::resolve_ui_theme(detect_system_prefers_dark()).tokens
}

fn detect_system_prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
}

/// Match GTK's built-in Adwaita variant to the saved theme preference so native
/// widget chrome (scrollbars, undershoot, popovers) stays consistent after a
/// live `reload-theme` — mirrors `metis-settings::theme::reapply`.
fn sync_gtk_theme_variant(_tokens: &ThemeTokens) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(
            metis_config::resolve_ui_theme(detect_system_prefers_dark()).is_dark,
        );
    }
}

fn apply_tokens(tokens: &ThemeTokens) {
    sync_gtk_theme_variant(tokens);
    let css = build_stylesheet(tokens);
    THEME_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.tokens = tokens.clone();
        state.provider.load_from_string(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &state.provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    // Re-apply the bar + menu background opacity (and bar border) on top of the
    // (possibly new) surface/accent colours so theme edits don't reset the
    // configured transparency or border.
    apply_bar_appearance(
        BAR_OPACITY.with(Cell::get),
        &BAR_BORDER.with(|b| b.borrow().clone()),
        &BAR_FILL.with(|f| f.borrow().clone()),
        BAR_POSITION.with(Cell::get),
    );
    apply_menu_opacity(MENU_OPACITY.with(Cell::get));
    // Re-apply ambient lift after tokens so theme reload keeps the wallpaper tint.
    reapply_ambient_provider();
    crate::ui::desktop_widgets::on_theme_changed();
    crate::ui::dashboard::on_theme_changed();
    crate::ui::screenshot::on_theme_changed();
    crate::ui::notification_center::on_theme_changed();
    crate::ui::updater::on_theme_changed();
}

/// Mix fraction of wallpaper RGB into frosted Control Center / Notification Center.
/// Strong enough to read on typical wallpapers; still a lift, not a retheme.
const AMBIENT_MIX: f32 = 0.18;

/// Apply or clear the wallpaper ambient tint from a compositor event.
pub fn apply_ambient_tint(r: u8, g: u8, b: u8, enabled: bool) {
    AMBIENT_ENABLED.with(|e| e.set(enabled));
    if !enabled {
        AMBIENT_RGB.with(|c| c.set(None));
        clear_ambient_provider();
        return;
    }
    if !metis_config::load_bar_config().ambient_tint {
        AMBIENT_RGB.with(|c| c.set(None));
        clear_ambient_provider();
        return;
    }
    AMBIENT_RGB.with(|c| c.set(Some((r, g, b))));
    reapply_ambient_provider();
}

/// Honour `bar.json` `ambient_tint` on live reload (Settings Edge bar switch).
pub fn sync_ambient_from_bar_config() {
    let on = metis_config::load_bar_config().ambient_tint;
    AMBIENT_ENABLED.with(|e| e.set(on));
    if !on {
        clear_ambient_provider();
    } else {
        reapply_ambient_provider();
    }
}

fn clear_ambient_provider() {
    AMBIENT_PROVIDER.with(|provider| {
        provider.load_from_string("");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
    tracing::info!("ambient tint CSS cleared");
    crate::ui::dashboard::on_theme_changed();
    crate::ui::notification_center::on_theme_changed();
}

fn reapply_ambient_provider() {
    if !AMBIENT_ENABLED.with(Cell::get) || !metis_config::load_bar_config().ambient_tint {
        clear_ambient_provider();
        return;
    }
    let Some((ar, ag, ab)) = AMBIENT_RGB.with(Cell::get) else {
        clear_ambient_provider();
        return;
    };
    let tokens = active_tokens();
    let is_light = tokens.mode.eq_ignore_ascii_case("light");
    let panel_alpha = if is_light { 0.72_f32 } else { 0.82 };
    let surface = parse_rgb_triplet(&tokens.surface_rgb());
    let (sr, sg, sb) = surface.unwrap_or((30, 30, 46));
    let t = AMBIENT_MIX;
    let r = ((sr as f32) * (1.0 - t) + (ar as f32) * t).round() as u8;
    let g = ((sg as f32) * (1.0 - t) + (ag as f32) * t).round() as u8;
    let b = ((sb as f32) * (1.0 - t) + (ab as f32) * t).round() as u8;
    // GTK CssProvider does NOT support `!important` (parser: "Junk at end of
    // value for background-color") — rely on USER priority over APPLICATION.
    let css = format!(
        ".metis-dashboard-root, .metis-nc-panel {{\
            background-color: rgba({r}, {g}, {b}, {panel_alpha:.3});\
         }}"
    );
    AMBIENT_PROVIDER.with(|provider| {
        provider.load_from_string(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
    tracing::info!(r, g, b, panel_alpha, "ambient tint CSS applied");
    crate::ui::dashboard::on_theme_changed();
    crate::ui::notification_center::on_theme_changed();
}

fn parse_rgb_triplet(s: &str) -> Option<(u8, u8, u8)> {
    let mut parts = s.split(',').map(|p| p.trim().parse::<u8>().ok());
    let r = parts.next()??;
    let g = parts.next()??;
    let b = parts.next()??;
    Some((r, g, b))
}

/// Apply the edge bar's fill (theme / solid / gradient), opacity, and border.
///
/// We override only the `.metis-bar-pill` surface via a dedicated, higher-priority
/// provider — never `window.set_opacity()`, which would also fade the icons and
/// text. For a gradient/accent border on the rounded pill we use the layered
/// `background-clip` trick (a `padding-box` fill layer over a `border-box`
/// gradient under a transparent border) so the stroke follows the pill's rounded
/// corners, which `border-image` cannot do.
pub fn apply_bar_appearance(
    opacity: f32,
    border: &BarBorder,
    fill: &BarFill,
    position: BarPosition,
) {
    let alpha = opacity.clamp(0.0, 1.0);
    BAR_OPACITY.with(|o| o.set(alpha));
    BAR_BORDER.with(|b| *b.borrow_mut() = border.clone());
    BAR_FILL.with(|f| *f.borrow_mut() = fill.clone());
    BAR_POSITION.with(|p| p.set(position));

    let fill_paint = resolve_fill_paint(fill, alpha, position);
    let width = border.width_px.max(0.0);

    let css = if width <= 0.0 {
        match &fill_paint {
            FillPaint::Color(c) => format!(
                ".metis-bar-pill {{ border: none; background-image: none; \
                 background-color: {c}; }}"
            ),
            FillPaint::Gradient { dir, stops } => format!(
                ".metis-bar-pill {{ border: none; background-color: transparent; \
                 background-image: linear-gradient({dir}, {stops}); }}"
            ),
        }
    } else {
        match border.mode {
            BorderMode::Solid => match &fill_paint {
                FillPaint::Color(c) => format!(
                    ".metis-bar-pill {{ border: {width}px solid {color}; \
                     background-image: none; background-color: {c}; }}",
                    color = border.color,
                ),
                FillPaint::Gradient { dir, stops } => format!(
                    ".metis-bar-pill {{ border: {width}px solid {color}; \
                     background-color: transparent; \
                     background-image: linear-gradient({dir}, {stops}); }}",
                    color = border.color,
                ),
            },
            BorderMode::Accent | BorderMode::Gradient => {
                let dir = match position {
                    BarPosition::Left | BarPosition::Right => "to bottom",
                    BarPosition::Top | BarPosition::Bottom => "to right",
                };
                // GTK's CSS engine does not reliably confine the lower gradient
                // layer to the border ring (background-clip is ignored), so an
                // opaque gradient bleeds across the whole pill and the opacity
                // slider has no visible effect. When the fill is translucent,
                // fall back to a solid accent stroke + fill color instead.
                if alpha < 1.0 - f32::EPSILON || matches!(fill_paint, FillPaint::Gradient { .. }) {
                    let stroke = bar_border_primary_color(border);
                    match &fill_paint {
                        FillPaint::Color(c) => format!(
                            ".metis-bar-pill {{ border: {width}px solid {stroke}; \
                             background-image: none; background-color: {c}; }}"
                        ),
                        FillPaint::Gradient { dir: fdir, stops } => format!(
                            ".metis-bar-pill {{ border: {width}px solid {stroke}; \
                             background-color: transparent; \
                             background-image: linear-gradient({fdir}, {stops}); }}"
                        ),
                    }
                } else {
                    let stops = bar_border_stops(border);
                    let FillPaint::Color(fill_c) = &fill_paint else {
                        unreachable!("gradient fill already handled above");
                    };
                    format!(
                        ".metis-bar-pill {{ \
                         border: {width}px solid transparent; \
                         background-color: transparent; \
                         background-image: linear-gradient({fill_c}, {fill_c}), \
                         linear-gradient({dir}, {stops}); \
                         background-clip: padding-box, border-box; \
                         background-origin: padding-box, border-box; }}"
                    )
                }
            }
        }
    };

    BAR_BG_PROVIDER.with(|provider| {
        provider.load_from_string(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
}

enum FillPaint {
    Color(String),
    Gradient { dir: &'static str, stops: String },
}

fn resolve_fill_paint(fill: &BarFill, alpha: f32, position: BarPosition) -> FillPaint {
    match fill.mode {
        BarFillMode::Theme => {
            let surface_rgb = active_tokens().surface_rgb();
            FillPaint::Color(format!("rgba({surface_rgb}, {alpha:.3})"))
        }
        BarFillMode::Solid => FillPaint::Color(hex_to_rgba_css(&fill.color, alpha)),
        BarFillMode::Gradient => {
            let dir = gradient_css_dir(fill.gradient_direction, position);
            // Bake bar opacity into every stop so translucent gradients still
            // show all colours (hex-only stops only work at opacity 1.0).
            let stops = fill
                .gradient
                .iter()
                .map(|h| h.trim())
                .filter(|h| !h.is_empty())
                .map(|h| hex_to_rgba_css(h, alpha))
                .collect::<Vec<_>>();
            let stops = if stops.len() < 2 {
                let fallback = fill
                    .gradient
                    .first()
                    .map(String::as_str)
                    .unwrap_or("#1a1b26");
                format!(
                    "{}, {}",
                    hex_to_rgba_css(fallback, alpha),
                    hex_to_rgba_css(fallback, alpha)
                )
            } else {
                stops.join(", ")
            };
            FillPaint::Gradient { dir, stops }
        }
    }
}

fn gradient_css_dir(dir: BarGradientDirection, position: BarPosition) -> &'static str {
    match dir {
        BarGradientDirection::Auto => match position {
            BarPosition::Left | BarPosition::Right => "to bottom",
            BarPosition::Top | BarPosition::Bottom => "to right",
        },
        BarGradientDirection::ToRight => "to right",
        BarGradientDirection::ToLeft => "to left",
        BarGradientDirection::ToBottom => "to bottom",
        BarGradientDirection::ToTop => "to top",
    }
}

fn hex_to_rgba_css(hex: &str, alpha: f32) -> String {
    let [r, g, b] = parse_hex_rgb(hex);
    format!("rgba({r}, {g}, {b}, {alpha:.3})")
}

/// Primary stroke colour for accent/gradient border modes (first accent or
/// gradient stop).
fn bar_border_primary_color(border: &BarBorder) -> String {
    match border.mode {
        BorderMode::Accent => active_tokens().accent_primary().to_string(),
        BorderMode::Gradient => border
            .gradient
            .first()
            .cloned()
            .unwrap_or_else(|| "#00F2FE".into()),
        BorderMode::Solid => border.color.clone(),
    }
}

/// Build the comma-separated CSS gradient stops for the bar border at full
/// opacity (used only when the fill is opaque — see `apply_bar_appearance`).
fn bar_border_stops(border: &BarBorder) -> String {
    let mut hexes: Vec<String> = match border.mode {
        BorderMode::Accent => active_tokens().accent.clone(),
        BorderMode::Gradient => border.gradient.clone(),
        BorderMode::Solid => vec![border.color.clone()],
    };
    hexes.retain(|s| !s.trim().is_empty());
    if hexes.is_empty() {
        hexes.push("#00F2FE".to_string());
    }
    if hexes.len() == 1 {
        hexes.push(hexes[0].clone());
    }
    hexes.join(", ")
}

/// Apply the start-menu popover's configurable background transparency.
///
/// Like [`apply_bar_opacity`], this overrides only the `.metis-menu-panel`
/// background alpha via a dedicated, higher-priority provider — never
/// `popover.set_opacity()` — so the menu's text, icons, and tiles stay fully
/// opaque while only the panel surface dims.
///
/// Also forces the GtkPopover chrome transparent (light Adwaita otherwise
/// paints an opaque `popover.background` / `contents` under the panel).
pub fn apply_menu_opacity(opacity: f32) {
    let alpha = opacity.clamp(0.0, 1.0);
    MENU_OPACITY.with(|o| o.set(alpha));
    let raised_rgb = active_tokens().surface_raised_rgb();
    let css = format!(
        "popover.metis-menu-popover,\
         popover.metis-menu-popover.background,\
         popover.metis-menu-popover contents {{\
            background-color: transparent;\
            background-image: none;\
            border: none;\
            box-shadow: none;\
         }}\
         .metis-menu-panel {{\
            background-color: rgba({raised_rgb}, {alpha:.3});\
         }}\
         popover.metis-menu-popover > arrow {{\
            background-color: rgba({raised_rgb}, {alpha:.3});\
         }}"
    );
    MENU_BG_PROVIDER.with(|provider| {
        provider.load_from_string(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
}

pub fn reload_stylesheet() {
    let tokens = load_active_theme();
    apply_tokens(&tokens);
}

/// Seed `themes/dark.json` / `themes/light.json` only when missing.
///
/// Must not overwrite existing files — Settings Appearance edits (accent,
/// semantic colours, font) live there and would be wiped on every login.
pub fn export_embedded_themes_to_config() -> std::io::Result<()> {
    config::ensure_config_dirs()?;
    let dark = config::theme_file_path_for_name("dark");
    if !dark.exists() {
        write_theme_file(&dark, &ThemeTokens::dark_default())?;
    }
    let light = config::theme_file_path_for_name("light");
    if !light.exists() {
        write_theme_file(&light, &ThemeTokens::light_default())?;
    }
    Ok(())
}

fn write_theme_file(path: &std::path::Path, tokens: &ThemeTokens) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(tokens).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
