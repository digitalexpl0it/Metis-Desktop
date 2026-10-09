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
        padding: 0;
        box-shadow: 0 8px 24px {shadow};
        color: {text};
        overflow: hidden;
    }}
    /* Inner margins + dismiss share notifications.css (`.metis-notif-*`). */

"#,
    )
}
