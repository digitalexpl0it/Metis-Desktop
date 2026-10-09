//! Shell stylesheet fragment: `notifications`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    popover.metis-bar-popover {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}

    popover.metis-bar-popover contents {{
        padding: 0;
        border: none;
        background-color: transparent;
    }}

    popover.metis-bar-popover > arrow {{
        background-color: {raised};
        border: 1px solid {border};
        min-width: 16px;
        min-height: 8px;
    }}

    popover.metis-notif-popover {{
        padding: 0;
    }}

    .metis-notif-scrolled {{
        min-width: 0;
    }}

    .metis-nc-scrolled {{
        background: transparent;
        min-width: 0;
    }}
    .metis-nc-scrolled scrollbar.vertical {{
        min-width: 8px;
        margin: 2px 0;
    }}
    .metis-nc-scrolled scrollbar.vertical slider {{
        background-color: rgba({text_rgb}, 0.25);
        border-radius: 999px;
        min-width: 6px;
    }}
    .metis-nc-scrolled scrollbar.vertical slider:hover {{
        background-color: rgba({text_rgb}, 0.4);
    }}
    .metis-cal-events-scroll {{
        min-height: 0;
    }}

    .metis-notif-scrolled scrollbar.vertical {{
        min-width: 8px;
        margin: 4px 2px;
    }}

    .metis-notif-scrolled scrollbar.vertical slider {{
        min-width: 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.18);
    }}

    .metis-notif-scrolled scrollbar.vertical slider:hover {{
        background-color: rgba({text_rgb}, 0.28);
    }}

    .metis-notif-empty {{
        font-size: 12px;
        color: {muted};
        padding: 24px 8px;
    }}

    .metis-notif-card {{
        background-color: {notif_card_bg};
        background-image: none;
        border-radius: 16px;
        border: 1px solid {border};
        padding: 0;
        color: {text};
        box-shadow: 0 8px 24px {shadow};
        overflow: hidden;
    }}

    .metis-notif-icon-wrap {{
        min-width: 28px;
        padding: 0;
        background: transparent;
        box-shadow: none;
    }}

    .metis-notif-icon {{
        -gtk-icon-size: 28px;
        color: {text};
    }}
    .metis-notif-icon-app {{
        color: {text};
    }}

    .metis-notif-count {{
        min-width: 18px;
        padding: 1px 7px;
        border-radius: 999px;
        font-size: 11px;
        font-weight: 700;
        color: {text};
        background-color: rgba({text_rgb}, 0.12);
    }}

    .metis-notif-clear {{
        padding: 5px 14px;
        border-radius: 8px;
        font-size: 12px;
        font-weight: 600;
        color: {muted};
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        border: 1px solid {border};
        box-shadow: none;
    }}
    .metis-notif-clear:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.10);
    }}
    .metis-notif-clear:disabled {{
        opacity: 0.45;
    }}

    .metis-notif-accent {{
        min-width: 4px;
        max-width: 4px;
        background-color: {accent};
    }}

    /* Inner content inset — padding lives here so the kind strip stays edge-flush.
       Icon gets a right gap so title/body are not flush against the glyph. */
    .metis-notif-card > .metis-notif-icon-wrap,
    .metis-toast-card > .metis-notif-icon-wrap {{
        margin: 14px 14px 14px 12px;
    }}
    .metis-notif-card > .metis-notif-body,
    .metis-toast-card > .metis-notif-body {{
        margin: 14px 16px 14px 0;
    }}


    .metis-notif-diamond {{
        min-width: 40px;
        min-height: 40px;
        border-radius: 6px;
        border: 1.5px solid currentColor;
        transform: rotate(45deg);
    }}

    .metis-notif-diamond-icon {{
        font-size: 15px;
        font-weight: 700;
        transform: rotate(-45deg);
    }}

    .metis-notif-title {{
        font-size: 14px;
        font-weight: 700;
        color: {text};
    }}

    .metis-notif-message {{
        font-size: 12px;
        color: rgba({text_rgb}, 0.78);
        line-height: 1.4;
    }}

    .metis-notif-actions {{
        margin-top: 6px;
    }}

    .metis-notif-action {{
        padding: 5px 14px;
        border-radius: 8px;
        font-size: 12px;
        font-weight: 600;
        color: {text};
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        border: 1px solid {border};
        box-shadow: none;
    }}
    .metis-notif-action:hover {{
        background-color: rgba({text_rgb}, 0.12);
    }}
    .metis-notif-action.suggested-action {{
        color: {text};
        border-color: rgba({accent_rgb}, 0.55);
        background-color: rgba({accent_rgb}, 0.22);
    }}
    .metis-notif-action.suggested-action:hover {{
        background-color: rgba({accent_rgb}, 0.34);
    }}

    .metis-notif-card-clickable:hover {{
        background-color: rgba({accent_rgb}, 0.08);
        background-image: none;
    }}

    button.metis-notif-expand,
    button.metis-notif-dismiss {{
        background: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        padding: 2px;
        min-width: 24px;
        min-height: 24px;
        color: {muted};
    }}
    button.metis-notif-expand:hover,
    button.metis-notif-dismiss:hover {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.12);
        border-radius: {rs}px;
    }}

"#,
    )
}
