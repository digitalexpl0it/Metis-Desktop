//! Shared widgets and config helpers for onboarding steps.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gtk::gdk;
use gtk::prelude::*;
use metis_config::{BackgroundKind, BarPosition, ThemeMode, WeatherConfig};

/// Metis wordmark (same asset as the splash).
const LOGO_BYTES: &[u8] = include_bytes!("../../../assets/metis_logo.png");

/// Fixed body height so step swaps do not resize the card.
pub(crate) const BODY_HEIGHT: i32 = 300;
/// Fixed card width (content-sized layer surface — see splash.rs).
pub(crate) const CARD_WIDTH: i32 = 520;
/// Inner step width inside card padding.
pub(crate) const BODY_INNER_WIDTH: i32 = CARD_WIDTH - 72;
/// Wallpaper thumbnail size (two-column grid).
pub(crate) const WALL_W: i32 = 196;
pub(crate) const WALL_H: i32 = 110;

pub(crate) fn wrapping_check(label: &str, active: bool) -> gtk::CheckButton {
    let check = gtk::CheckButton::new();
    check.set_active(active);
    check.set_halign(gtk::Align::Fill);
    check.set_hexpand(false);
    check.set_size_request(BODY_INNER_WIDTH, -1);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_wrap(true);
    lbl.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    lbl.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    lbl.set_max_width_chars(40);
    lbl.set_width_request(BODY_INNER_WIDTH.saturating_sub(40));
    check.set_child(Some(&lbl));
    check
}

pub(crate) fn apply_onboarding_gaming_prefs() {
    let mut cfg = metis_config::load_gaming_config();
    if super::gaming_auto_gpu() {
        cfg.graphics_mode = metis_config::GraphicsMode::Auto;
        cfg.flatpak_gpu_env = true;
    }
    let _ = metis_config::save_gaming_config(&cfg);
    if super::gaming_optimize() {
        std::thread::spawn(|| {
            let _ = metis_gaming::optimize_flatpak_gaming();
            let _ = metis_gaming::ensure_steam_launcher();
        });
    }
    let _ = metis_config::mark_gaming_setup_complete();
    metis_gaming::session::request_reload();
}

pub(crate) fn keybind_row(key: &str, desc: &str) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(2);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("metis-onboarding-keybind");
    k.set_width_chars(14);
    k.set_xalign(0.0);
    let d = gtk::Label::new(Some(desc));
    d.add_css_class("metis-onboarding-subtitle");
    d.set_xalign(0.0);
    d.set_hexpand(true);
    row.append(&k);
    row.append(&d);
    row.upcast()
}

pub(crate) fn step_shell() -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.add_css_class("metis-onboarding-step-content");
    col.set_width_request(BODY_INNER_WIDTH);
    col.set_hexpand(false);
    col.set_vexpand(false);
    col.set_halign(gtk::Align::Fill);
    col.set_overflow(gtk::Overflow::Hidden);
    col
}

pub(crate) fn labeled_row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(4);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_width_chars(14);
    lbl.set_halign(gtk::Align::Start);
    widget.set_hexpand(true);
    widget.set_halign(gtk::Align::End);
    row.append(&lbl);
    row.append(widget);
    row.upcast()
}

pub(crate) fn theme_preview_button(
    label: &str,
    dark: bool,
    wallpaper: Option<&Path>,
) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    btn.add_css_class("metis-onboarding-preview-tile");
    btn.set_hexpand(false);
    btn.set_vexpand(false);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
    vbox.set_hexpand(false);
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(120, 76);

    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_size_request(120, 76);
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

    let mock = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mock.add_css_class(if dark {
        "metis-style-mock-dark"
    } else {
        "metis-style-mock-light"
    });
    mock.set_halign(gtk::Align::Center);
    mock.set_valign(gtk::Align::Center);
    mock.set_size_request(72, 44);
    overlay.add_overlay(&mock);

    vbox.append(&overlay);
    let caption = gtk::Label::new(Some(label));
    caption.add_css_class("metis-style-caption");
    vbox.append(&caption);
    btn.set_child(Some(&vbox));
    btn
}

pub(crate) fn apply_theme(mode: ThemeMode) {
    if let Err(err) = crate::config::save_theme_preference(mode) {
        tracing::warn!(%err, "failed to save theme preference");
    }
    metis_config::apply_session_appearance_gsettings(mode);
    let _ = crate::ui::theme::init_theme();
}

pub(crate) fn apply_wallpaper(path: &str) {
    let mut cfg = crate::config::load_wallpaper_config();
    cfg.kind = BackgroundKind::Image;
    cfg.path = Some(path.to_string());
    if let Err(err) = crate::config::save_wallpaper_config(&cfg) {
        tracing::warn!(%err, "failed to save wallpaper.json");
    }
    if let Err(err) = crate::compositor::apply_background() {
        tracing::warn!(%err, "failed to apply background");
    }
}

pub(crate) fn update_bar<F>(apply: F)
where
    F: FnOnce(&mut metis_config::BarConfig),
{
    let old = crate::config::load_bar_config();
    let mut cfg = old.clone();
    apply(&mut cfg);
    if cfg == old {
        return;
    }
    if let Err(err) = crate::config::save_bar_config(&cfg) {
        tracing::warn!(%err, "failed to save bar.json");
    }
    super::mark_bar_config_dirty();
}

pub(crate) fn save_weather(cfg: &WeatherConfig) {
    let old = crate::config::load_weather_config();
    if cfg == &old {
        return;
    }
    if let Err(err) = crate::config::save_weather_config(cfg) {
        tracing::warn!(%err, "failed to save weather.json");
    }
    super::mark_weather_reload_pending();
}
pub(crate) fn current_wallpaper_path() -> Option<PathBuf> {
    if let Some(p) = crate::config::load_wallpaper_config().path {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    metis_config::list_bundled_wallpapers().into_iter().next()
}

pub(crate) fn bar_position_index(pos: BarPosition) -> u32 {
    match pos {
        BarPosition::Top => 0,
        BarPosition::Bottom => 1,
        BarPosition::Left => 2,
        BarPosition::Right => 3,
    }
}

pub(crate) fn index_to_bar_position(idx: u32) -> BarPosition {
    match idx {
        1 => BarPosition::Bottom,
        2 => BarPosition::Left,
        3 => BarPosition::Right,
        _ => BarPosition::Top,
    }
}

pub(crate) fn monitor_size() -> (i32, i32) {
    if let Some(display) = gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gdk::Monitor>()
    {
        let g = monitor.geometry();
        if g.width() > 0 && g.height() > 0 {
            return (g.width(), g.height());
        }
    }
    (1280, 720)
}

pub(crate) fn load_logo() -> Option<gdk::Texture> {
    let bytes = glib::Bytes::from_static(LOGO_BYTES);
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Some(texture),
        Err(err) => {
            tracing::warn!(%err, "failed to decode onboarding logo");
            None
        }
    }
}

pub(crate) fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GeoResult {
    pub(crate) name: String,
    pub(crate) detail: String,
    pub(crate) lat: f64,
    pub(crate) lon: f64,
}

pub(crate) fn geocode_search(query: &str) -> Vec<GeoResult> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=en&format=json",
        urlencode(query)
    );
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(%err, "geocode: client build failed");
            return Vec::new();
        }
    };
    let json: serde_json::Value = match client.get(&url).send().and_then(|r| r.json()) {
        Ok(j) => j,
        Err(err) => {
            tracing::warn!(%err, "geocode: request failed");
            return Vec::new();
        }
    };
    let Some(results) = json.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let name = r.get("name")?.as_str()?.to_string();
            let lat = r.get("latitude")?.as_f64()?;
            let lon = r.get("longitude")?.as_f64()?;
            let admin = r.get("admin1").and_then(|v| v.as_str()).unwrap_or("");
            let country = r.get("country").and_then(|v| v.as_str()).unwrap_or("");
            let detail = [admin, country]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            Some(GeoResult {
                name,
                detail,
                lat,
                lon,
            })
        })
        .collect()
}

pub(crate) fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
