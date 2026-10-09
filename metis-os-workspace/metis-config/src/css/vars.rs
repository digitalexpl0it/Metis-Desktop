//! Derived CSS tokens for Metis stylesheets.

use crate::theme::ThemeTokens;

#[derive(Debug, Clone)]
pub(crate) struct CssVars {
    pub accent: String,
    pub accent2: String,
    pub accent2_rgb: String,
    pub accent_rgb: String,
    pub border: String,
    pub c_error: String,
    pub c_error_rgb: String,
    pub c_info: String,
    pub c_info_rgb: String,
    pub c_payment: String,
    pub c_payment_rgb: String,
    pub c_success: String,
    pub c_success_rgb: String,
    pub c_warning: String,
    pub c_warning_rgb: String,
    pub dash_card_bg: String,
    pub dash_panel_bg: String,
    pub dash_shadow: String,
    pub dash_shadow_up: String,
    pub font_decls: String,
    pub launcher_icon_shadow: String,
    pub muted: String,
    pub nc_card_bg: String,
    pub nc_panel_bg: String,
    pub notif_card_bg: String,
    pub on_accent: String,
    pub overlay_card_bg: String,
    pub overlay_dot: String,
    pub raised: String,
    pub raised_rgb: String,
    pub rl: String,
    pub rm: String,
    pub rs: String,
    pub screenshot_toolbar_bg: String,
    pub shadow: String,
    pub surface: String,
    pub surface_rgb: String,
    pub surface_solid: String,
    pub text: String,
    pub text_on_accent: String,
    pub text_rgb: String,
    pub toast_card_bg: String,
    pub tray_pixmap_filter: String,
}

impl CssVars {
    pub(crate) fn from_theme(theme: &ThemeTokens) -> Self {
        let accent = theme.accent_primary().to_string();
        let accent_rgb = theme.accent_rgb();
        let accent2 = theme.accent_secondary().to_string();
        let accent2_rgb = theme.accent_secondary_rgb();
        let on_accent = theme.on_accent_ink();
        let text_rgb = theme.text_rgb();
        let surface_solid = theme.surface.clone();
        let surface = theme.surface_rgba();
        let raised = theme.surface_raised.clone();
        let shadow = theme.shadow_ambient.clone();
        let launcher_icon_shadow = if theme.mode.eq_ignore_ascii_case("light") {
            "-gtk-icon-shadow: 0 1px 3px rgba(0, 0, 0, 0.55);".to_string()
        } else {
            String::new()
        };
        let tray_pixmap_filter = if theme.mode.eq_ignore_ascii_case("light") {
            "filter: brightness(0); opacity: 0.88;".to_string()
        } else {
            String::new()
        };
        let rs = theme.radius_sm.to_string();
        let rm = theme.radius_md.to_string();
        let rl = theme.radius_lg.to_string();
        let c_error = theme.semantic.error.clone();
        let c_warning = theme.semantic.warning.clone();
        let c_success = theme.semantic.success.clone();
        let c_info = theme.semantic.info.clone();
        let c_payment = theme.semantic.payment.clone();
        let c_error_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.error);
        let c_warning_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.warning);
        let c_success_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.success);
        let c_info_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.info);
        let c_payment_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.payment);
        let surface_rgb = theme.surface_rgb();
        let raised_rgb = theme.surface_raised_rgb();
        let is_light = theme.mode.eq_ignore_ascii_case("light");
        let dash_panel_bg = if is_light {
            format!("rgba({surface_rgb}, 0.72)")
        } else {
            format!("rgba({surface_rgb}, 0.82)")
        };
        let dash_card_bg = if is_light {
            format!("rgba({raised_rgb}, 0.94)")
        } else {
            format!("rgba({raised_rgb}, 0.90)")
        };
        let overlay_card_bg = if is_light {
            format!("rgba({raised_rgb}, 0.96)")
        } else {
            format!("rgba({surface_rgb}, 0.92)")
        };
        let overlay_dot = if is_light {
            format!("rgba({text_rgb}, 0.22)")
        } else {
            "rgba(255, 255, 255, 0.18)".to_string()
        };
        let dash_shadow = if is_light {
            format!("0 12px 40px {shadow}")
        } else {
            "0 12px 32px rgba(0, 0, 0, 0.42)".to_string()
        };
        let dash_shadow_up = if is_light {
            format!("0 -12px 40px {shadow}")
        } else {
            "0 -12px 32px rgba(0, 0, 0, 0.42)".to_string()
        };
        let screenshot_toolbar_bg = dash_panel_bg.clone();
        let nc_panel_bg = dash_panel_bg.clone();
        let nc_card_bg = dash_card_bg.clone();
        let toast_card_bg = if is_light {
            format!("rgba({raised_rgb}, 0.96)")
        } else {
            format!("rgba({raised_rgb}, 0.94)")
        };
        let notif_card_bg = toast_card_bg.clone();
        let text_on_accent = theme.text_on_accent.clone();
        let font_decls = theme.font_declarations();

        Self {
            accent,
            accent2,
            accent2_rgb,
            accent_rgb,
            border: theme.border.clone(),
            c_error,
            c_error_rgb,
            c_info,
            c_info_rgb,
            c_payment,
            c_payment_rgb,
            c_success,
            c_success_rgb,
            c_warning,
            c_warning_rgb,
            dash_card_bg,
            dash_panel_bg,
            dash_shadow,
            dash_shadow_up,
            font_decls,
            launcher_icon_shadow,
            muted: theme.text_muted.clone(),
            nc_card_bg,
            nc_panel_bg,
            notif_card_bg,
            on_accent,
            overlay_card_bg,
            overlay_dot,
            raised,
            raised_rgb,
            rl,
            rm,
            rs,
            screenshot_toolbar_bg,
            shadow,
            surface,
            surface_rgb,
            surface_solid,
            text: theme.text.clone(),
            text_on_accent,
            text_rgb,
            toast_card_bg,
            tray_pixmap_filter,
        }
    }

    pub(crate) fn render(&self, template: &str) -> String {
        let mut out = template.replace("{{", "").replace("}}", "");
        out = out.replace("{accent}", &self.accent);
        out = out.replace("{accent2}", &self.accent2);
        out = out.replace("{accent2_rgb}", &self.accent2_rgb);
        out = out.replace("{accent_rgb}", &self.accent_rgb);
        out = out.replace("{border}", &self.border);
        out = out.replace("{c_error}", &self.c_error);
        out = out.replace("{c_error_rgb}", &self.c_error_rgb);
        out = out.replace("{c_info}", &self.c_info);
        out = out.replace("{c_info_rgb}", &self.c_info_rgb);
        out = out.replace("{c_payment}", &self.c_payment);
        out = out.replace("{c_payment_rgb}", &self.c_payment_rgb);
        out = out.replace("{c_success}", &self.c_success);
        out = out.replace("{c_success_rgb}", &self.c_success_rgb);
        out = out.replace("{c_warning}", &self.c_warning);
        out = out.replace("{c_warning_rgb}", &self.c_warning_rgb);
        out = out.replace("{dash_card_bg}", &self.dash_card_bg);
        out = out.replace("{dash_panel_bg}", &self.dash_panel_bg);
        out = out.replace("{dash_shadow}", &self.dash_shadow);
        out = out.replace("{dash_shadow_up}", &self.dash_shadow_up);
        out = out.replace("{font_decls}", &self.font_decls);
        out = out.replace("{launcher_icon_shadow}", &self.launcher_icon_shadow);
        out = out.replace("{muted}", &self.muted);
        out = out.replace("{nc_card_bg}", &self.nc_card_bg);
        out = out.replace("{nc_panel_bg}", &self.nc_panel_bg);
        out = out.replace("{notif_card_bg}", &self.notif_card_bg);
        out = out.replace("{on_accent}", &self.on_accent);
        out = out.replace("{overlay_card_bg}", &self.overlay_card_bg);
        out = out.replace("{overlay_dot}", &self.overlay_dot);
        out = out.replace("{raised}", &self.raised);
        out = out.replace("{raised_rgb}", &self.raised_rgb);
        out = out.replace("{rl}", &self.rl);
        out = out.replace("{rm}", &self.rm);
        out = out.replace("{rs}", &self.rs);
        out = out.replace("{screenshot_toolbar_bg}", &self.screenshot_toolbar_bg);
        out = out.replace("{shadow}", &self.shadow);
        out = out.replace("{surface}", &self.surface);
        out = out.replace("{surface_rgb}", &self.surface_rgb);
        out = out.replace("{surface_solid}", &self.surface_solid);
        out = out.replace("{text}", &self.text);
        out = out.replace("{text_on_accent}", &self.text_on_accent);
        out = out.replace("{text_rgb}", &self.text_rgb);
        out = out.replace("{toast_card_bg}", &self.toast_card_bg);
        out = out.replace("{tray_pixmap_filter}", &self.tray_pixmap_filter);
        out.replace("", "{").replace("", "}")
    }
}
