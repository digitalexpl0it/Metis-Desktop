mod loader;

pub use loader::{
    active_tokens, apply_ambient_tint, apply_bar_appearance, apply_icon_theme_from_config,
    apply_menu_opacity, export_embedded_themes_to_config, init_theme, reload_stylesheet,
    sync_ambient_from_bar_config,
};
// Theme tokens, semantic colors, mode, and the stylesheet builder now live in the
// shared `metis-config` crate; re-export so `crate::ui::theme::...` keeps working
// for any external reference.
#[allow(unused_imports)]
pub use metis_config::{SemanticColors, ThemeMode, ThemeTokens, build_stylesheet};

pub fn install_theme() {
    let _ = init_theme();
}
