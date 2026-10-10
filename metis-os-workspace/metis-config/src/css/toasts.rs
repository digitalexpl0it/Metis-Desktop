//! Shell stylesheet fragment: `toasts`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Toast banners (transient overlay, top-right) ---- */
    window.metis-toast-window,
    window.metis-toast-window > * {{
        background-color: transparent;
        background-image: none;
        box-shadow: none;
    }}
    .metis-toast-stack,
    .metis-toast-stack > revealer,
    .metis-toast-stack > revealer > * {{
        background-color: transparent;
        background-image: none;
        box-shadow: none;
    }}
    .metis-toast-stack {{
        margin: 0;
    }}
    .metis-toast-card {{
        background-color: {toast_card_bg};
        background-image: none;
        border-radius: 16px;
        border: 1px solid {border};
        border-left: 4px solid {accent};
        padding: 0;
        /* Margin = shadow bleed room on the transparent toast layer surface. */
        margin: 4px 10px 12px 10px;
        box-shadow: {popover_shadow};
        color: {text};
        overflow: hidden;
    }}
    /* Inner margins + dismiss share notifications.css (`.metis-notif-*`). */

"#,
    )
}
