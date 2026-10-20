//! Shell stylesheet fragment: session log-out / power confirm overlay.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Session power confirm (centered Overlay) ---- */
    window.metis-session-confirm-window {{
        background-color: transparent;
    }}
    .metis-session-confirm-root {{
        background-color: transparent;
    }}
    button.metis-session-confirm-backdrop {{
        background-color: rgba(0, 0, 0, 0.45);
        background-image: none;
        border: none;
        border-radius: 0;
        box-shadow: none;
        outline: none;
        min-width: 1px;
        min-height: 1px;
    }}
    button.metis-session-confirm-backdrop:hover,
    button.metis-session-confirm-backdrop:active,
    button.metis-session-confirm-backdrop:focus,
    button.metis-session-confirm-backdrop:checked {{
        background-color: rgba(0, 0, 0, 0.45);
        background-image: none;
        box-shadow: none;
        outline: none;
    }}
    .metis-session-confirm-card {{
        background-color: {raised};
        border-radius: 18px;
        border: 1px solid {border};
        padding: 28px 32px;
        box-shadow: {dash_shadow};
        color: {text};
        min-width: 320px;
        max-width: 420px;
    }}
    .metis-session-confirm-icon {{
        color: {accent};
        -gtk-icon-style: symbolic;
    }}
    .metis-session-confirm-title {{
        font-size: 18px;
        font-weight: 700;
        color: {text};
    }}
    .metis-session-confirm-detail {{
        font-size: 13px;
        color: {muted};
    }}
    .metis-session-confirm-actions {{
        margin-top: 8px;
    }}
    /* Defeat Adwaita button chrome — color alone on the parent is ignored. */
    button.metis-session-confirm-cancel {{
        min-width: 110px;
        padding: 8px 16px;
        border-radius: 10px;
        background-color: rgba({text_rgb}, 0.08);
        background-image: none;
        color: {text};
        border: 1px solid {border};
        box-shadow: none;
        outline: none;
    }}
    button.metis-session-confirm-cancel:hover {{
        background-color: rgba({text_rgb}, 0.14);
        background-image: none;
    }}
    button.metis-session-confirm-cancel:active,
    button.metis-session-confirm-cancel:focus {{
        background-color: rgba({text_rgb}, 0.18);
        background-image: none;
        box-shadow: none;
        outline: none;
    }}
    button.metis-session-confirm-cancel label {{
        color: {text};
    }}
    button.metis-session-confirm-ok {{
        min-width: 110px;
        padding: 8px 16px;
        border-radius: 10px;
        background-color: {accent};
        background-image: none;
        color: #ffffff;
        border: none;
        box-shadow: none;
        outline: none;
    }}
    button.metis-session-confirm-ok:hover {{
        background-color: {accent};
        background-image: none;
        filter: brightness(1.08);
    }}
    button.metis-session-confirm-ok:active,
    button.metis-session-confirm-ok:focus {{
        background-color: {accent};
        background-image: none;
        box-shadow: none;
        outline: none;
    }}
    button.metis-session-confirm-ok label {{
        color: #ffffff;
        font-weight: 600;
    }}
"#,
    )
}
