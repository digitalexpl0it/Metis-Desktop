//! Shared helpers for the Appearance-family pages (Appearance, Background, Edge
//! bar, Windows). These pages all read/write `themes/*.json`, `wallpaper.json`,
//! and `bar.json` and share the same colour-button + persistence plumbing.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gtk::CssProvider;
use gtk::STYLE_PROVIDER_PRIORITY_APPLICATION;
use gtk::gdk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::dialog;
use crate::gtk_cb::OptBarConfigMutate;
use crate::runtime;

static SWATCH_SEQ: AtomicU64 = AtomicU64::new(1);

type ColorSwatchListener = Rc<dyn Fn(&ColorSwatchButton)>;
type ColorSwatchListeners = Rc<RefCell<Vec<ColorSwatchListener>>>;
type FontPickerListener = Rc<dyn Fn(&FontPickerButton)>;
type FontPickerListeners = Rc<RefCell<Vec<FontPickerListener>>>;

/// Compact swatch that opens the in-window top-slide colour picker (not a
/// separate ColorDialog window).
#[derive(Clone)]
pub struct ColorSwatchButton {
    button: gtk::Button,
    chip_class: String,
    provider: CssProvider,
    rgba: Rc<RefCell<gdk::RGBA>>,
    listeners: ColorSwatchListeners,
}

impl ColorSwatchButton {
    pub fn new() -> Self {
        let id = SWATCH_SEQ.fetch_add(1, Ordering::Relaxed);
        let chip_class = format!("metis-swatch-{id}");
        let rgba = Rc::new(RefCell::new(gdk::RGBA::new(0.0, 0.6, 0.8, 1.0)));
        let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        swatch.add_css_class("metis-color-swatch-chip");
        swatch.add_css_class(&chip_class);
        swatch.set_size_request(48, 22);
        swatch.set_halign(gtk::Align::Center);
        swatch.set_valign(gtk::Align::Center);

        let provider = CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION + 5,
            );
        }

        let button = gtk::Button::new();
        button.add_css_class("metis-color-swatch");
        button.set_focus_on_click(false);
        button.set_child(Some(&swatch));

        let this = Self {
            button: button.clone(),
            chip_class,
            provider,
            rgba: rgba.clone(),
            listeners: Rc::new(RefCell::new(Vec::new())),
        };
        this.apply_swatch_css();

        let me = this.clone();
        button.connect_clicked(move |_| {
            let initial = *me.rgba.borrow();
            let me = me.clone();
            dialog::pick_color(
                initial,
                Rc::new(move |picked| {
                    let Some(rgba) = picked else {
                        return;
                    };
                    me.set_rgba(&rgba);
                }),
            );
        });

        this
    }

    pub fn upcast_ref(&self) -> &gtk::Button {
        &self.button
    }

    pub fn rgba(&self) -> gdk::RGBA {
        *self.rgba.borrow()
    }

    pub fn set_rgba(&self, rgba: &gdk::RGBA) {
        *self.rgba.borrow_mut() = *rgba;
        self.apply_swatch_css();
        let listeners = self.listeners.borrow().clone();
        for cb in listeners {
            cb(self);
        }
    }

    pub fn connect_rgba_notify<F>(&self, f: F)
    where
        F: Fn(&ColorSwatchButton) + 'static,
    {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.button.set_sensitive(sensitive);
    }

    fn apply_swatch_css(&self) {
        let hex = rgba_to_hex(&self.rgba.borrow());
        self.provider.load_from_string(&format!(
            "box.{} {{
                background-color: {hex};
                min-width: 48px;
                min-height: 22px;
                border-radius: 4px;
            }}",
            self.chip_class
        ));
    }
}

impl AsRef<gtk::Widget> for ColorSwatchButton {
    fn as_ref(&self) -> &gtk::Widget {
        self.button.upcast_ref()
    }
}

impl std::ops::Deref for ColorSwatchButton {
    type Target = gtk::Button;
    fn deref(&self) -> &Self::Target {
        &self.button
    }
}

/// Font button that opens the in-window top-slide font picker.
#[derive(Clone)]
pub struct FontPickerButton {
    button: gtk::Button,
    label: gtk::Label,
    desc: Rc<RefCell<Option<pango::FontDescription>>>,
    listeners: FontPickerListeners,
}

impl FontPickerButton {
    pub fn new() -> Self {
        let label = gtk::Label::new(Some("Sans 11"));
        label.add_css_class("metis-font-picker-label");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(28);

        let button = gtk::Button::new();
        button.add_css_class("metis-font-picker");
        button.set_focus_on_click(false);
        button.set_child(Some(&label));

        let this = Self {
            button: button.clone(),
            label,
            desc: Rc::new(RefCell::new(None)),
            listeners: Rc::new(RefCell::new(Vec::new())),
        };

        let me = this.clone();
        button.connect_clicked(move |_| {
            let initial = me.desc.borrow().clone();
            let me = me.clone();
            dialog::pick_font(
                initial,
                Rc::new(move |picked| {
                    let Some(desc) = picked else {
                        return;
                    };
                    me.set_font_desc(&desc);
                }),
            );
        });

        this
    }

    pub fn upcast_ref(&self) -> &gtk::Button {
        &self.button
    }

    pub fn font_desc(&self) -> Option<pango::FontDescription> {
        self.desc.borrow().clone()
    }

    pub fn set_font_desc(&self, desc: &pango::FontDescription) {
        *self.desc.borrow_mut() = Some(desc.clone());
        let text = desc.to_string();
        self.label
            .set_text(if text.is_empty() { "Sans 11" } else { &text });
        let listeners = self.listeners.borrow().clone();
        for cb in listeners {
            cb(self);
        }
    }

    pub fn connect_font_desc_notify<F>(&self, f: F)
    where
        F: Fn(&FontPickerButton) + 'static,
    {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    pub fn set_sensitive(&self, sensitive: bool) {
        self.button.set_sensitive(sensitive);
    }
}

impl AsRef<gtk::Widget> for FontPickerButton {
    fn as_ref(&self) -> &gtk::Widget {
        self.button.upcast_ref()
    }
}

impl std::ops::Deref for FontPickerButton {
    type Target = gtk::Button;
    fn deref(&self) -> &Self::Target {
        &self.button
    }
}

/// A colour swatch that opens the top-slide picker (opaque hex tokens).
pub fn color_dialog_button() -> ColorSwatchButton {
    ColorSwatchButton::new()
}

pub fn font_picker_button() -> FontPickerButton {
    FontPickerButton::new()
}

pub fn hex_to_rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or_else(|_| gdk::RGBA::new(0.0, 0.95, 1.0, 1.0))
}

pub fn rgba_to_hex(rgba: &gdk::RGBA) -> String {
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        to_u8(rgba.red()),
        to_u8(rgba.green()),
        to_u8(rgba.blue())
    )
}

/// Set the `idx`-th gradient stop in a stop list, growing it if needed so a sparse
/// config still accepts edits to later stops.
pub fn set_stops(stops: &mut Vec<String>, idx: usize, hex: String) {
    while stops.len() <= idx {
        stops.push(hex.clone());
    }
    stops[idx] = hex;
}

/// Read `bar.json` fresh, let `apply` overwrite just the caller's fields, then
/// persist and nudge a live reload.
///
/// The Edge bar and Windows pages each own a *disjoint* subset of `bar.json`
/// and hold independent in-memory copies. Re-reading the on-disk config here (and
/// mutating only the fields the caller touches) means neither page — nor other
/// writers like the dock's `taskbar_pinned` pin/unpin — clobbers the others.
///
/// Debounced so rapid scale ticks (or scroll landing on a scale) cannot flood
/// the shell with `reload-bar` and freeze the session for seconds.
pub fn update_bar<F>(apply: F)
where
    F: FnOnce(&mut metis_config::BarConfig) + 'static,
{
    thread_local! {
        static PENDING: OptBarConfigMutate = const { RefCell::new(None) };
        static DEBOUNCE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    }

    PENDING.with(|slot| {
        let prev = slot.borrow_mut().take();
        *slot.borrow_mut() = Some(Box::new(move |cfg: &mut metis_config::BarConfig| {
            if let Some(prev) = prev {
                prev(cfg);
            }
            apply(cfg);
        }));
    });

    DEBOUNCE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(id) = slot.take() {
            id.remove();
        }
        let id = glib::timeout_add_local(std::time::Duration::from_millis(120), || {
            DEBOUNCE.with(|slot| *slot.borrow_mut() = None);
            let apply = PENDING.with(|slot| slot.borrow_mut().take());
            if let Some(apply) = apply {
                let mut on_disk = metis_config::load_bar_config();
                apply(&mut on_disk);
                if let Err(err) = metis_config::save_bar_config(&on_disk) {
                    tracing::warn!(%err, "failed to save bar.json");
                }
                runtime::send("reload-bar");
            }
            glib::ControlFlow::Break
        });
        *slot = Some(id);
    });
}

/// The wallpaper currently in use (falls back to the first discoverable one),
/// used both for the Style previews and the Background picker's selection state.
pub fn current_wallpaper() -> Option<PathBuf> {
    if let Some(p) = metis_config::load_wallpaper_config().path {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    list_wallpapers().into_iter().next()
}

/// Where a discovered wallpaper came from (picker section order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperSection {
    /// `~/.config/metis/wallpapers` imports.
    User,
    /// Packaged / bundled Metis defaults.
    Metis,
    /// Distro / DE backgrounds (`/usr/share/backgrounds`, …).
    System,
}

impl WallpaperSection {
    pub fn title(self) -> String {
        match self {
            Self::User => metis_i18n::tr("Your pictures"),
            Self::Metis => metis_i18n::tr("Metis"),
            Self::System => metis_i18n::tr("System"),
        }
    }
}

/// Collect selectable wallpapers grouped for the Background picker:
/// user imports → Metis bundled → system (GNOME/Ubuntu/…) backgrounds.
pub fn list_wallpaper_sections() -> Vec<(WallpaperSection, Vec<PathBuf>)> {
    let mut seen = HashSet::new();
    let mut sections = Vec::new();

    let mut user = Vec::new();
    metis_config::collect_wallpaper_images(
        &metis_config::wallpaper_store_dir(),
        &mut user,
        &mut seen,
    );
    if !user.is_empty() {
        sections.push((WallpaperSection::User, user));
    }

    let mut metis = Vec::new();
    for dir in metis_config::bundled_wallpaper_dirs() {
        metis_config::collect_wallpaper_images(&dir, &mut metis, &mut seen);
    }
    if !metis.is_empty() {
        sections.push((WallpaperSection::Metis, metis));
    }

    let mut system = Vec::new();
    for dir in metis_config::system_wallpaper_dirs() {
        // Depth 2 picks up Ubuntu's `backgrounds/contest/` without descending
        // into deep KDE size-variant trees.
        metis_config::collect_wallpaper_images_depth(&dir, &mut system, &mut seen, 2);
    }
    if !system.is_empty() {
        sections.push((WallpaperSection::System, system));
    }

    sections
}

/// Flat list in the same order as [`list_wallpaper_sections`].
pub fn list_wallpapers() -> Vec<PathBuf> {
    list_wallpaper_sections()
        .into_iter()
        .flat_map(|(_, paths)| paths)
        .collect()
}
