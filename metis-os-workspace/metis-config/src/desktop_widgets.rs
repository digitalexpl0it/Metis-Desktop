//! Free-floating desktop widgets (`~/.config/metis/desktop-widgets.json`).
//!
//! Phase 14: optional wallpaper-layer panels (Folders, Apps, Clock, …). Master
//! switch defaults to **off** so fresh installs stay wallpaper-clean. Geometry is
//! per-instance and per-output; `desk.json` remains app-grid only.
//!
//! Chrome (fill + border) has **global defaults** with optional **per-instance
//! overrides** (`None` / missing = inherit).

use serde::{Deserialize, Serialize};

/// Built-in desktop widget kinds (v1 + platform placeholder) and extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopWidgetKind {
    Folders,
    Apps,
    Clock,
    System,
    Weather,
    /// System-audio spectrum / wave visualizer.
    Equalizer,
    /// Temporary card for platform bring-up (move / resize / lock).
    Placeholder,
    /// Declarative JSON pack under `…/metis/widgets/<extension_id>/`.
    Extension,
}

impl DesktopWidgetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            Self::Apps => "Apps",
            Self::Clock => "Clock",
            Self::System => "System",
            Self::Weather => "Weather",
            Self::Equalizer => "Equalizer",
            Self::Placeholder => "Placeholder",
            Self::Extension => "Extension",
        }
    }

    /// Builtin kinds the Settings UI can add (extensions are listed separately).
    pub fn addable() -> &'static [DesktopWidgetKind] {
        &[
            DesktopWidgetKind::Placeholder,
            DesktopWidgetKind::Folders,
            DesktopWidgetKind::Apps,
            DesktopWidgetKind::Clock,
            DesktopWidgetKind::System,
            DesktopWidgetKind::Weather,
            DesktopWidgetKind::Equalizer,
        ]
    }
}

fn default_desktop_path() -> String {
    "~/Desktop".into()
}

fn default_w() -> u32 {
    320
}

fn default_h() -> u32 {
    240
}

fn default_background_opacity() -> f32 {
    0.40
}

fn default_border_width() -> f32 {
    1.0
}

/// Layout mode for Folders / Apps widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopWidgetView {
    #[default]
    Grid,
    List,
}

impl DesktopWidgetView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::List => "List",
        }
    }

    pub fn all() -> &'static [DesktopWidgetView] {
        &[Self::Grid, Self::List]
    }
}

fn is_default_view(view: &DesktopWidgetView) -> bool {
    *view == DesktopWidgetView::Grid
}

fn default_show_title() -> bool {
    true
}

fn is_default_show_title(show: &bool) -> bool {
    *show
}

/// Equalizer visual style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqualizerVizStyle {
    SpectrumLines,
    #[default]
    Bars,
    NeonWave,
    /// Frequency bands as rays around a centre.
    Radial,
}

impl EqualizerVizStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::SpectrumLines => "Spectrum lines",
            Self::Bars => "Bars",
            Self::NeonWave => "Neon wave",
            Self::Radial => "Radial",
        }
    }

    pub fn all() -> &'static [EqualizerVizStyle] {
        &[
            Self::SpectrumLines,
            Self::Bars,
            Self::NeonWave,
            Self::Radial,
        ]
    }
}

fn is_default_viz_style(s: &EqualizerVizStyle) -> bool {
    *s == EqualizerVizStyle::Bars
}

/// How Bars style paints each column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqualizerBarShape {
    /// Discrete rounded segments (current default look).
    #[default]
    Segmented,
    /// Continuous solid columns with a soft tip glow.
    Solid,
    /// Circular dots stacked up the column.
    Dots,
    /// Smaller, denser circular dots.
    DenseDots,
}

impl EqualizerBarShape {
    pub fn label(self) -> &'static str {
        match self {
            Self::Segmented => "Segmented (dotted)",
            Self::Solid => "Solid",
            Self::Dots => "Dots",
            Self::DenseDots => "Dense dots",
        }
    }

    pub fn all() -> &'static [EqualizerBarShape] {
        &[Self::Segmented, Self::Solid, Self::Dots, Self::DenseDots]
    }
}

fn is_default_bar_shape(s: &EqualizerBarShape) -> bool {
    *s == EqualizerBarShape::Segmented
}

/// Equalizer colour mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqualizerColorMode {
    Solid,
    #[default]
    Multi,
    Theme,
}

impl EqualizerColorMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Multi => "Gradient",
            Self::Theme => "Theme colours",
        }
    }

    pub fn all() -> &'static [EqualizerColorMode] {
        &[Self::Solid, Self::Multi, Self::Theme]
    }
}

fn is_default_color_mode(m: &EqualizerColorMode) -> bool {
    *m == EqualizerColorMode::Multi
}

fn default_solid_color() -> String {
    "#00e5ff".into()
}

fn is_default_solid_color(c: &str) -> bool {
    c.is_empty() || c.eq_ignore_ascii_case("#00e5ff")
}

fn default_gradient_start() -> String {
    "#ffe033".into()
}

fn is_default_gradient_start(c: &str) -> bool {
    c.is_empty() || c.eq_ignore_ascii_case("#ffe033")
}

fn default_gradient_end() -> String {
    "#26d9ff".into()
}

fn is_default_gradient_end(c: &str) -> bool {
    c.is_empty() || c.eq_ignore_ascii_case("#26d9ff")
}

fn default_peak_color() -> String {
    "#ff59d9".into()
}

fn is_default_peak_color(c: &str) -> bool {
    c.is_empty() || c.eq_ignore_ascii_case("#ff59d9")
}

fn default_bar_count() -> u32 {
    48
}

fn is_default_bar_count(n: &u32) -> bool {
    *n == default_bar_count()
}

fn default_show_reflection() -> bool {
    true
}

fn is_default_show_reflection(v: &bool) -> bool {
    *v
}

fn default_bar_gradient() -> bool {
    true
}

fn is_default_bar_gradient(v: &bool) -> bool {
    *v
}

fn default_show_peaks() -> bool {
    true
}

fn is_default_show_peaks(v: &bool) -> bool {
    *v
}

/// Global card chrome defaults (fill + border). Empty colour strings mean
/// “use the active theme surface / border tint”.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopWidgetChrome {
    /// Card fill opacity (0 = invisible fill, 1 = solid). Text stays opaque.
    #[serde(default = "default_background_opacity")]
    pub background_opacity: f32,
    /// Optional `#RRGGBB` fill. Empty → theme surface colour.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub background_color: String,
    /// Border thickness in CSS px. `0` disables the border.
    #[serde(default = "default_border_width")]
    pub border_width: f32,
    /// Optional `#RRGGBB` border. Empty → theme-tinted hairline.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub border_color: String,
}

impl Default for DesktopWidgetChrome {
    fn default() -> Self {
        Self {
            background_opacity: default_background_opacity(),
            background_color: String::new(),
            border_width: default_border_width(),
            border_color: String::new(),
        }
    }
}

impl DesktopWidgetChrome {
    pub fn sanitize(mut self) -> Self {
        self.background_opacity = self.background_opacity.clamp(0.0, 1.0);
        self.border_width = self.border_width.clamp(0.0, 12.0);
        self.background_color = normalize_hex(&self.background_color);
        self.border_color = normalize_hex(&self.border_color);
        self
    }
}

/// Per-instance chrome overrides. `None` inherits the global [`DesktopWidgetChrome`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DesktopWidgetChromeOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
}

impl DesktopWidgetChromeOverride {
    pub fn is_empty(&self) -> bool {
        self.background_opacity.is_none()
            && self.background_color.is_none()
            && self.border_width.is_none()
            && self.border_color.is_none()
    }

    pub fn sanitize(mut self) -> Self {
        if let Some(o) = self.background_opacity.as_mut() {
            *o = o.clamp(0.0, 1.0);
        }
        if let Some(w) = self.border_width.as_mut() {
            *w = w.clamp(0.0, 12.0);
        }
        if let Some(c) = self.background_color.as_mut() {
            *c = normalize_hex(c);
        }
        if let Some(c) = self.border_color.as_mut() {
            *c = normalize_hex(c);
        }
        self
    }
}

/// Resolved chrome after merging global defaults with an instance override.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDesktopWidgetChrome {
    pub background_opacity: f32,
    /// `#RRGGBB` or empty (theme surface).
    pub background_color: String,
    pub border_width: f32,
    /// `#RRGGBB` or empty (theme border tint).
    pub border_color: String,
}

impl DesktopWidgetChrome {
    pub fn resolve(&self, ov: &DesktopWidgetChromeOverride) -> ResolvedDesktopWidgetChrome {
        ResolvedDesktopWidgetChrome {
            background_opacity: ov
                .background_opacity
                .unwrap_or(self.background_opacity)
                .clamp(0.0, 1.0),
            background_color: ov
                .background_color
                .clone()
                .unwrap_or_else(|| self.background_color.clone()),
            border_width: ov
                .border_width
                .unwrap_or(self.border_width)
                .clamp(0.0, 12.0),
            border_color: ov
                .border_color
                .clone()
                .unwrap_or_else(|| self.border_color.clone()),
        }
    }
}

fn normalize_hex(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    let h = t.trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("#{h}")
    } else if h.len() == 3 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let mut out = String::from("#");
        for c in h.chars() {
            out.push(c);
            out.push(c);
        }
        out
    } else {
        // Keep as-is only if it already looks like #RRGGBB; otherwise drop.
        String::new()
    }
}

fn is_default_path(path: &str) -> bool {
    path.is_empty() || path == "~/Desktop"
}

/// One placed widget instance on a monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopWidgetInstance {
    pub id: String,
    pub kind: DesktopWidgetKind,
    /// Output connector name (`DP-1`, …). Empty / missing = primary monitor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_w")]
    pub w: u32,
    #[serde(default = "default_h")]
    pub h: u32,
    #[serde(default)]
    pub locked: bool,
    /// Folders widget: directory to list (`~/Desktop` by default).
    #[serde(
        default = "default_desktop_path",
        skip_serializing_if = "is_default_path"
    )]
    pub path: String,
    /// Apps widget: desktop ids to show (separate from start-menu pins).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<String>,
    /// Folders / Apps: grid or list layout. Default grid.
    #[serde(default, skip_serializing_if = "is_default_view")]
    pub view: DesktopWidgetView,
    /// Show the card title bar (kind name). Off = chrome-only body; edit mode
    /// still keeps a thin drag strip so widgets remain movable.
    #[serde(
        default = "default_show_title",
        skip_serializing_if = "is_default_show_title"
    )]
    pub show_title: bool,
    /// Optional Pango font description for text-heavy widgets
    /// (Clock / Weather / System). Empty → theme default.
    /// Example: `"Cantarell Bold 28"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub font: String,
    /// Optional body text / icon colour (`#RRGGBB`). Empty → theme text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_color: String,
    /// Optional accent colour (`#RRGGBB`) for progress fills / highlights.
    /// Empty → theme accent. Used by System (and similar) widgets.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accent_color: String,
    /// Equalizer: visual style.
    #[serde(default, skip_serializing_if = "is_default_viz_style")]
    pub viz_style: EqualizerVizStyle,
    /// Equalizer bars: column paint style (segmented / solid / dots).
    #[serde(default, skip_serializing_if = "is_default_bar_shape")]
    pub bar_shape: EqualizerBarShape,
    /// Equalizer: colour mode.
    #[serde(default, skip_serializing_if = "is_default_color_mode")]
    pub color_mode: EqualizerColorMode,
    /// Equalizer: solid colour when `color_mode` is Solid.
    #[serde(
        default = "default_solid_color",
        skip_serializing_if = "is_default_solid_color"
    )]
    pub solid_color: String,
    /// Equalizer: gradient start (`#RRGGBB`) when `color_mode` is Gradient.
    #[serde(
        default = "default_gradient_start",
        skip_serializing_if = "is_default_gradient_start"
    )]
    pub gradient_start: String,
    /// Equalizer: gradient end (`#RRGGBB`) when `color_mode` is Gradient.
    #[serde(
        default = "default_gradient_end",
        skip_serializing_if = "is_default_gradient_end"
    )]
    pub gradient_end: String,
    /// Equalizer: band / bar count (16–96).
    #[serde(
        default = "default_bar_count",
        skip_serializing_if = "is_default_bar_count"
    )]
    pub bar_count: u32,
    /// Equalizer bars: vertical segment tint along each bar. Off = flat fill.
    #[serde(
        default = "default_bar_gradient",
        skip_serializing_if = "is_default_bar_gradient"
    )]
    pub bar_gradient: bool,
    /// Equalizer bars: draw floating peak-hold caps.
    #[serde(
        default = "default_show_peaks",
        skip_serializing_if = "is_default_show_peaks"
    )]
    pub show_peaks: bool,
    /// Equalizer bars: peak-hold cap colour.
    #[serde(
        default = "default_peak_color",
        skip_serializing_if = "is_default_peak_color"
    )]
    pub peak_color: String,
    /// Equalizer bars: mirrored reflection. Neon wave: mirrored lower half.
    #[serde(
        default = "default_show_reflection",
        skip_serializing_if = "is_default_show_reflection"
    )]
    pub show_reflection: bool,
    /// Optional chrome overrides (inherit global when empty / unset).
    #[serde(default, skip_serializing_if = "DesktopWidgetChromeOverride::is_empty")]
    pub chrome: DesktopWidgetChromeOverride,
    /// Extension pack id when [`Self::kind`] is [`DesktopWidgetKind::Extension`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extension_id: String,
    /// Schema-driven settings for extension widgets.
    #[serde(default, skip_serializing_if = "serde_json_map_is_empty")]
    pub extension_settings: serde_json::Map<String, serde_json::Value>,
}

fn serde_json_map_is_empty(m: &serde_json::Map<String, serde_json::Value>) -> bool {
    m.is_empty()
}

impl DesktopWidgetInstance {
    pub fn new(kind: DesktopWidgetKind) -> Self {
        let (w, h) = match kind {
            DesktopWidgetKind::Clock => (280, 140),
            DesktopWidgetKind::Weather => (300, 180),
            DesktopWidgetKind::System => (300, 200),
            DesktopWidgetKind::Folders | DesktopWidgetKind::Apps => (360, 280),
            DesktopWidgetKind::Equalizer => (480, 160),
            DesktopWidgetKind::Placeholder | DesktopWidgetKind::Extension => {
                (default_w(), default_h())
            }
        };
        // Apps feel denser as a list; Folders default to an icon grid.
        let view = match kind {
            DesktopWidgetKind::Apps => DesktopWidgetView::List,
            _ => DesktopWidgetView::Grid,
        };
        Self {
            id: new_instance_id(),
            kind,
            output: String::new(),
            x: 80,
            y: 80,
            w,
            h,
            locked: false,
            path: default_desktop_path(),
            pins: Vec::new(),
            view,
            show_title: true,
            font: String::new(),
            text_color: String::new(),
            accent_color: String::new(),
            viz_style: EqualizerVizStyle::Bars,
            bar_shape: EqualizerBarShape::Segmented,
            color_mode: EqualizerColorMode::Multi,
            solid_color: default_solid_color(),
            gradient_start: default_gradient_start(),
            gradient_end: default_gradient_end(),
            bar_count: default_bar_count(),
            bar_gradient: true,
            show_peaks: true,
            peak_color: default_peak_color(),
            show_reflection: true,
            chrome: DesktopWidgetChromeOverride::default(),
            extension_id: String::new(),
            extension_settings: serde_json::Map::new(),
        }
    }

    /// Create an extension instance from a discovered manifest.
    pub fn new_extension(manifest: &crate::widget_ext::WidgetExtManifest) -> Self {
        let mut inst = Self::new(DesktopWidgetKind::Extension);
        inst.extension_id = manifest.id.clone();
        inst.w = manifest.default_size[0].max(120);
        inst.h = manifest.default_size[1].max(80);
        if let Some([mw, mh]) = manifest.min_size {
            inst.w = inst.w.max(mw);
            inst.h = inst.h.max(mh);
        }
        inst.extension_settings =
            crate::widget_ext::default_extension_settings(&manifest.settings_schema);
        inst
    }

    /// Display title for lists / card chrome.
    pub fn display_title(&self) -> String {
        if self.kind == DesktopWidgetKind::Extension {
            if let Some(ext) = crate::widget_ext::find_widget_extension(&self.extension_id) {
                return ext.manifest.name;
            }
            if !self.extension_id.is_empty() {
                return self.extension_id.clone();
            }
        }
        self.kind.label().to_string()
    }

    /// True for kinds that honour the optional [`Self::font`] /
    /// [`Self::text_color`] / [`Self::accent_color`] overrides.
    pub fn supports_text_style(&self) -> bool {
        matches!(
            self.kind,
            DesktopWidgetKind::Clock | DesktopWidgetKind::Weather | DesktopWidgetKind::System
        )
    }

    /// True for kinds that honour the optional [`Self::font`] override.
    pub fn supports_font(&self) -> bool {
        self.supports_text_style()
    }
}

fn new_instance_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("dw-{nanos:x}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DesktopWidgetsConfig {
    /// Master switch. Default off — wallpaper stays clean until the user opts in.
    #[serde(default)]
    pub enabled: bool,
    /// When true, unlocked instances can be moved / resized on the desktop.
    #[serde(default)]
    pub edit_mode: bool,
    /// Global card chrome defaults. Legacy configs may still have a top-level
    /// `background_opacity`; that is migrated into `chrome` on load.
    #[serde(default)]
    pub chrome: DesktopWidgetChrome,
    /// Legacy flat opacity (pre-chrome object). Migrated into [`Self::chrome`]
    /// and not written back.
    #[serde(default, skip_serializing)]
    pub background_opacity: Option<f32>,
    #[serde(default)]
    pub instances: Vec<DesktopWidgetInstance>,
}

pub fn desktop_widgets_config_path() -> std::path::PathBuf {
    super::config_dir().join("desktop-widgets.json")
}

pub fn load_desktop_widgets_config() -> DesktopWidgetsConfig {
    let path = desktop_widgets_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return sanitize(cfg);
    }
    DesktopWidgetsConfig::default()
}

pub fn save_desktop_widgets_config(cfg: &DesktopWidgetsConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let cfg = sanitize(cfg.clone());
    let json = serde_json::to_string_pretty(&cfg).map_err(std::io::Error::other)?;
    let path = desktop_widgets_config_path();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

fn sanitize(mut cfg: DesktopWidgetsConfig) -> DesktopWidgetsConfig {
    // Migrate legacy top-level background_opacity into chrome when present.
    if let Some(legacy) = cfg.background_opacity.take() {
        // Only adopt if chrome still looks like a fresh default opacity and the
        // user had customized the flat field (or any load of the old shape).
        cfg.chrome.background_opacity = legacy;
    }
    cfg.chrome = cfg.chrome.sanitize();
    let mut seen = std::collections::HashSet::new();
    cfg.instances.retain(|inst| {
        if inst.id.is_empty() || !seen.insert(inst.id.clone()) {
            return false;
        }
        true
    });
    for inst in &mut cfg.instances {
        inst.w = inst.w.clamp(160, 2400);
        inst.h = inst.h.clamp(120, 1800);
        if inst.path.trim().is_empty() {
            inst.path = default_desktop_path();
        }
        inst.bar_count = inst.bar_count.clamp(16, 96);
        inst.solid_color = {
            let n = normalize_hex(&inst.solid_color);
            if n.is_empty() {
                default_solid_color()
            } else {
                n
            }
        };
        inst.gradient_start = {
            let n = normalize_hex(&inst.gradient_start);
            if n.is_empty() {
                default_gradient_start()
            } else {
                n
            }
        };
        inst.gradient_end = {
            let n = normalize_hex(&inst.gradient_end);
            if n.is_empty() {
                default_gradient_end()
            } else {
                n
            }
        };
        inst.peak_color = {
            let n = normalize_hex(&inst.peak_color);
            if n.is_empty() {
                default_peak_color()
            } else {
                n
            }
        };
        inst.text_color = normalize_hex(&inst.text_color);
        inst.accent_color = normalize_hex(&inst.accent_color);
        inst.chrome = std::mem::take(&mut inst.chrome).sanitize();
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let cfg = DesktopWidgetsConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.instances.is_empty());
        assert!((cfg.chrome.background_opacity - 0.4).abs() < f32::EPSILON);
        assert!((cfg.chrome.border_width - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn round_trip_placeholder() {
        let mut cfg = DesktopWidgetsConfig {
            enabled: true,
            edit_mode: true,
            chrome: DesktopWidgetChrome {
                background_opacity: 0.25,
                background_color: "#112233".into(),
                border_width: 0.0,
                border_color: String::new(),
            },
            background_opacity: None,
            instances: vec![DesktopWidgetInstance::new(DesktopWidgetKind::Placeholder)],
        };
        cfg = sanitize(cfg);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: DesktopWidgetsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.instances.len(), 1);
        assert_eq!(back.instances[0].kind, DesktopWidgetKind::Placeholder);
        assert!((back.chrome.background_opacity - 0.25).abs() < f32::EPSILON);
        assert_eq!(back.chrome.background_color, "#112233");
        assert!(back.chrome.border_width.abs() < f32::EPSILON);
    }

    #[test]
    fn migrate_legacy_background_opacity() {
        let json = r#"{"enabled":true,"edit_mode":false,"background_opacity":0.1,"instances":[]}"#;
        let cfg: DesktopWidgetsConfig = serde_json::from_str(json).unwrap();
        let cfg = sanitize(cfg);
        assert!((cfg.chrome.background_opacity - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_override() {
        let global = DesktopWidgetChrome {
            background_opacity: 0.4,
            background_color: String::new(),
            border_width: 1.0,
            border_color: "#ff0000".into(),
        };
        let ov = DesktopWidgetChromeOverride {
            background_opacity: Some(0.0),
            background_color: None,
            border_width: Some(0.0),
            border_color: None,
        };
        let r = global.resolve(&ov);
        assert!(r.background_opacity.abs() < f32::EPSILON);
        assert!(r.border_width.abs() < f32::EPSILON);
        assert_eq!(r.border_color, "#ff0000");
    }
}
