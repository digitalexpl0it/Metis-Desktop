use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Semantic status colors used by notifications and state highlights. Declared
/// with serde defaults so older `themes/*.json` (written before this palette
/// existed) keep parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticColors {
    #[serde(default = "default_error")]
    pub error: String,
    #[serde(default = "default_warning")]
    pub warning: String,
    #[serde(default = "default_success")]
    pub success: String,
    #[serde(default = "default_info")]
    pub info: String,
    #[serde(default = "default_payment")]
    pub payment: String,
}

impl Default for SemanticColors {
    fn default() -> Self {
        Self {
            error: default_error(),
            warning: default_warning(),
            success: default_success(),
            info: default_info(),
            payment: default_payment(),
        }
    }
}

fn default_error() -> String {
    "#ef4444".into()
}
fn default_warning() -> String {
    "#f59e0b".into()
}
fn default_success() -> String {
    "#10b981".into()
}
fn default_info() -> String {
    "#3b82f6".into()
}
fn default_payment() -> String {
    "#84cc16".into()
}
fn default_text_on_accent() -> String {
    "#0a0e14".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeTokens {
    pub name: String,
    pub mode: String,
    pub bg: String,
    pub surface: String,
    pub surface_raised: String,
    pub border: String,
    pub text: String,
    pub text_muted: String,
    pub accent: Vec<String>,
    pub radius_sm: u32,
    pub radius_md: u32,
    pub radius_lg: u32,
    pub gutter_px: u32,
    pub card_opacity: f32,
    pub scrim_opacity: f32,
    pub glow_intensity: f32,
    pub shadow_ambient: String,
    pub glow_cool: String,
    pub glow_warm: String,
    pub glow_violet: String,
    #[serde(default)]
    pub semantic: SemanticColors,
    #[serde(default = "default_text_on_accent")]
    pub text_on_accent: String,
    /// Optional UI font family (empty = system default). Applied DE-wide via the
    /// shared stylesheet's base `window` rule.
    #[serde(default)]
    pub font_family: String,
    /// Optional UI font size in points (0 = use each widget's default size).
    #[serde(default)]
    pub font_size_pt: u32,
}

impl ThemeTokens {
    /// Build the optional base font declarations (family/size) injected into the
    /// stylesheet's root `window` rule. Empty when neither is customized, so the
    /// default theme renders exactly as before.
    pub fn font_declarations(&self) -> String {
        let mut decls = String::new();
        let family = self.font_family.trim();
        if !family.is_empty() {
            decls.push_str(&format!("font-family: \"{family}\";"));
        }
        if self.font_size_pt > 0 {
            decls.push_str(&format!("font-size: {}pt;", self.font_size_pt));
        }
        decls
    }
}

impl ThemeTokens {
    pub fn dark_default() -> Self {
        serde_json::from_str(include_str!("../resources/themes/dark.json"))
            .expect("embedded dark theme must parse")
    }

    pub fn light_default() -> Self {
        serde_json::from_str(include_str!("../resources/themes/light.json"))
            .expect("embedded light theme must parse")
    }

    pub fn accent_primary(&self) -> &str {
        self.accent.first().map(String::as_str).unwrap_or("#00F2FE")
    }

    /// The secondary accent (`accent[1]`), used for gradients and toggles. Falls
    /// back to the primary accent when a theme only declares one accent.
    pub fn accent_secondary(&self) -> &str {
        self.accent
            .get(1)
            .map(String::as_str)
            .unwrap_or_else(|| self.accent_primary())
    }

    /// The primary accent as a bare `r, g, b` triplet, so the stylesheet can
    /// inline it into `rgba(<triplet>, <alpha>)` with per-rule opacities.
    pub fn accent_rgb(&self) -> String {
        rgb_triplet_from_hex(self.accent_primary())
    }

    /// The secondary accent as a bare `r, g, b` triplet.
    pub fn accent_secondary_rgb(&self) -> String {
        rgb_triplet_from_hex(self.accent_secondary())
    }

    pub fn surface_rgba(&self) -> String {
        rgba_from_hex(&self.surface, self.card_opacity)
    }

    /// The base surface colour as a bare `r, g, b` triplet, so callers can inline
    /// it into `rgba(<triplet>, <alpha>)` with a runtime-chosen opacity (e.g. the
    /// edge bar's configurable background transparency).
    pub fn surface_rgb(&self) -> String {
        rgb_triplet_from_hex(&self.surface)
    }

    /// The raised surface colour as a bare `r, g, b` triplet, used for popover /
    /// dropdown panels (e.g. the start menu's configurable background opacity).
    pub fn surface_raised_rgb(&self) -> String {
        rgb_triplet_from_hex(&self.surface_raised)
    }

    /// The foreground/text colour as a bare `r, g, b` triplet, so the stylesheet
    /// can build `rgba(<triplet>, <alpha>)` outlines that stay legible in both
    /// light and dark themes (e.g. the edge bar's workspace dots).
    pub fn text_rgb(&self) -> String {
        rgb_triplet_from_hex(&self.text)
    }

    pub fn bg_rgba(&self) -> String {
        rgba_from_hex(&self.bg, self.card_opacity)
    }

    /// Near-black or white ink that stays readable on top of `hex` (WCAG-ish
    /// luminance). Used for labels on solid accent fills (dropdown selection,
    /// suggested-action buttons, etc.).
    pub fn contrast_ink(hex: &str) -> String {
        let h = hex.trim_start_matches('#');
        let (r, g, b) = if h.len() == 6 {
            (
                u8::from_str_radix(&h[0..2], 16).unwrap_or(0) as f32,
                u8::from_str_radix(&h[2..4], 16).unwrap_or(0) as f32,
                u8::from_str_radix(&h[4..6], 16).unwrap_or(0) as f32,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        // Perceived luminance (0–255). Light accents (cyan, white, yellow) need
        // dark ink; dark accents need white.
        let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
        if luminance > 150.0 {
            "#0a0e14".to_string()
        } else {
            "#ffffff".to_string()
        }
    }

    /// Ink colour for drawing on the primary accent, derived live from the
    /// accent hex (ignores a stale `text_on_accent` in older theme files).
    pub fn on_accent_ink(&self) -> String {
        Self::contrast_ink(self.accent_primary())
    }

    /// Apply accessibility vision overrides (high contrast, invert, colour filters)
    /// to Metis GTK chrome tokens. Does not rewrite theme files on disk.
    pub fn apply_accessibility(&mut self, cfg: &crate::accessibility::AccessibilityConfig) {
        if cfg.high_contrast {
            if self.mode == "light" {
                self.bg = "#ffffff".into();
                self.surface = "#ffffff".into();
                self.surface_raised = "#f2f2f2".into();
                self.border = "#000000".into();
                self.text = "#000000".into();
                self.text_muted = "#222222".into();
            } else {
                self.bg = "#000000".into();
                self.surface = "#000000".into();
                self.surface_raised = "#121212".into();
                self.border = "#ffffff".into();
                self.text = "#ffffff".into();
                self.text_muted = "#dddddd".into();
            }
            self.card_opacity = 1.0;
        }
        if cfg.invert_colors {
            std::mem::swap(&mut self.bg, &mut self.text);
            std::mem::swap(&mut self.surface, &mut self.text_muted);
            let border = invert_hex_color(&self.border);
            self.border = border;
            self.surface_raised = invert_hex_color(&self.surface_raised);
        }
        if cfg.color_filters {
            match cfg.color_filter_type {
                crate::accessibility::ColorFilterType::Greyscale => {
                    self.bg = greyscale_hex(&self.bg);
                    self.surface = greyscale_hex(&self.surface);
                    self.surface_raised = greyscale_hex(&self.surface_raised);
                    self.border = greyscale_hex(&self.border);
                    self.text = greyscale_hex(&self.text);
                    self.text_muted = greyscale_hex(&self.text_muted);
                    self.accent = self.accent.iter().map(|c| greyscale_hex(c)).collect();
                }
                // Approximate dichromacy matrices applied to accent only so UI
                // chrome stays readable while signalling the filter is active.
                crate::accessibility::ColorFilterType::Protanopia
                | crate::accessibility::ColorFilterType::Deuteranopia
                | crate::accessibility::ColorFilterType::Tritanopia => {
                    self.accent = self
                        .accent
                        .iter()
                        .map(|c| simulate_cvd(c, cfg.color_filter_type))
                        .collect();
                }
            }
        }
        self.text_on_accent = Self::contrast_ink(self.accent_primary());
    }
}

fn parse_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return (0, 0, 0);
    }
    (
        u8::from_str_radix(&h[0..2], 16).unwrap_or(0),
        u8::from_str_radix(&h[2..4], 16).unwrap_or(0),
        u8::from_str_radix(&h[4..6], 16).unwrap_or(0),
    )
}

fn format_rgb(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn invert_hex_color(hex: &str) -> String {
    let (r, g, b) = parse_rgb(hex);
    format_rgb(255 - r, 255 - g, 255 - b)
}

fn greyscale_hex(hex: &str) -> String {
    let (r, g, b) = parse_rgb(hex);
    let y = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)).round() as u8;
    format_rgb(y, y, y)
}

fn simulate_cvd(hex: &str, kind: crate::accessibility::ColorFilterType) -> String {
    let (r, g, b) = parse_rgb(hex);
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;
    let (nr, ng, nb) = match kind {
        crate::accessibility::ColorFilterType::Protanopia => {
            (0.567 * rf + 0.433 * gf, 0.558 * rf + 0.442 * gf, bf)
        }
        crate::accessibility::ColorFilterType::Deuteranopia => {
            (0.625 * rf + 0.375 * gf, 0.7 * rf + 0.3 * gf, bf)
        }
        crate::accessibility::ColorFilterType::Tritanopia => {
            (rf, 0.95 * gf + 0.05 * bf, 0.433 * gf + 0.567 * bf)
        }
        crate::accessibility::ColorFilterType::Greyscale => {
            let y = 0.299 * rf + 0.587 * gf + 0.114 * bf;
            (y, y, y)
        }
    };
    format_rgb(
        (nr.clamp(0.0, 1.0) * 255.0).round() as u8,
        (ng.clamp(0.0, 1.0) * 255.0).round() as u8,
        (nb.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Parse a `#rrggbb` string into a bare `r, g, b` triplet (for inlining into a
/// CSS `rgba(<triplet>, <alpha>)`). Falls back to the dark accent on bad input.
pub(crate) fn rgb_triplet_from_hex(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return "0, 242, 254".to_string();
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(242);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(254);
    format!("{r}, {g}, {b}")
}

/// Convert a `#rrggbb` string plus an alpha into a CSS `rgba(...)` string.
pub(crate) fn rgba_from_hex(hex: &str, opacity: f32) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return format!("rgba(18, 18, 20, {opacity})");
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(18);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(18);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(20);
    format!("rgba({r}, {g}, {b}, {opacity})")
}
