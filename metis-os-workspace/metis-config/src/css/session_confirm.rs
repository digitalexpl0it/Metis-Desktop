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
    .metis-session-confirm-backdrop {{
        background-color: rgba(0, 0, 0, 0.45);
        border: none;
        border-radius: 0;
        box-shadow: none;
        min-width: 1px;
        min-height: 1px;
    }}
    .metis-session-confirm-backdrop:hover,
    .metis-session-confirm-backdrop:active,
    .metis-session-confirm-backdrop:focus {{
        background-color: rgba(0, 0, 0, 0.45);
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
    .metis-session-confirm-cancel {{
        min-width: 110px;
        padding: 8px 16px;
        border-radius: 10px;
        background-color: rgba({text_rgb}, 0.08);
        color: {text};
        border: 1px solid {border};
    }}
    .metis-session-confirm-cancel:hover {{
        background-color: rgba({text_rgb}, 0.14);
    }}
    .metis-session-confirm-cancel label {{
        color: {text};
    }}
    .metis-session-confirm-ok {{
        min-width: 110px;
        padding: 8px 16px;
        border-radius: 10px;
        background-color: {accent};
        color: #ffffff;
        border: none;
    }}
    .metis-session-confirm-ok:hover {{
        filter: brightness(1.08);
    }}
    .metis-session-confirm-ok label {{
        color: #ffffff;
        font-weight: 600;
    }}
"#,
    )
}
