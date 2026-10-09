//! Shell stylesheet fragment: `desktop_widgets`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    popover.metis-dw-confirm {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}
    popover.metis-dw-confirm contents {{
        background-color: {surface_solid} !important;
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 0;
        box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    }}
    .metis-dw-confirm-sheet {{
        background-color: {surface_solid};
        color: {text};
        border-radius: {rm}px;
    }}
    .metis-dw-confirm-title {{
        font-weight: 600;
        font-size: 1.05rem;
        color: {text};
    }}
    .metis-dw-confirm-detail {{
        color: {muted};
    }}
    .metis-desktop-widgets-canvas {{
        background-color: transparent;
    }}
    .metis-dw-card {{
        background-color: transparent;
        border: none;
        border-radius: {rm}px;
        color: {text};
        padding: 8px 10px 6px 10px;
    }}
    .metis-dw-card.metis-dw-edit {{
        /* Edit outline stays visible even when fill/border chrome is transparent. */
        outline: 1px dashed rgba({accent_rgb}, 0.65);
        outline-offset: -1px;
    }}
    .metis-dw-card.metis-dw-locked {{
        opacity: 0.92;
    }}
    .metis-dw-header {{
        margin-bottom: 4px;
        min-height: 28px;
    }}
    .metis-dw-header.metis-dw-header-slim {{
        min-height: 18px;
        margin-bottom: 2px;
        opacity: 0.85;
    }}
    .metis-dw-title {{
        font-weight: 600;
        font-size: 0.95rem;
        color: {text};
    }}
    .metis-dw-badge {{
        font-size: 0.72rem;
        color: {muted};
        padding: 1px 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-dw-body {{
        min-height: 40px;
    }}
    .metis-dw-hint {{
        color: {muted};
        font-size: 0.85rem;
        opacity: 0.9;
    }}
    .metis-dw-list {{
        padding: 0;
    }}
    button.metis-dw-row {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 4px 6px;
        color: {text};
    }}
    button.metis-dw-row:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-folder-grid {{
        padding: 2px;
    }}
    button.metis-dw-folder-tile {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 6px 4px;
        color: {text};
    }}
    button.metis-dw-folder-tile:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-folder-name {{
        font-size: 0.72rem;
        color: {text};
    }}
    button.metis-dw-menu-item {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 6px 10px;
        color: {text};
    }}
    button.metis-dw-menu-item:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-clock-time {{
        font-size: 2.4rem;
        font-weight: 600;
        color: {text};
    }}
    .metis-dw-clock-date {{
        font-size: 0.95rem;
        color: {muted};
    }}
    .metis-dw-metric-label {{
        font-weight: 600;
        color: {text};
        font-size: 0.85rem;
    }}
    progressbar.metis-dw-progress trough {{
        min-height: 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.12);
    }}
    progressbar.metis-dw-progress progress {{
        min-height: 6px;
        border-radius: 999px;
        background-color: {accent};
    }}
    .metis-dw-resize {{
        background-image: none;
        background-color: rgba({accent_rgb}, 0.22);
        border: 1px solid rgba({accent_rgb}, 0.50);
        border-radius: 4px;
        color: {text};
        padding: 0;
        min-width: 20px;
        min-height: 20px;
        font-size: 0.7rem;
    }}
    .metis-dw-resize:hover {{
        background-color: rgba({accent_rgb}, 0.38);
    }}
"#,
    )
}
