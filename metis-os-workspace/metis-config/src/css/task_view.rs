//! Shell stylesheet fragment: `task_view`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Task View (Super+Tab) ---- */
    window.metis-task-view {{
        background: transparent;
    }}
    .metis-task-view-backdrop {{
        background-color: rgba(0, 0, 0, 0.48);
    }}
    .metis-task-view-scroll {{
        background: transparent;
    }}
    .metis-task-view-scroll > viewport {{
        background: transparent;
    }}
    .metis-task-view-apps {{
        background: transparent;
        padding: 8px;
    }}
    .metis-task-view-card {{
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        border-radius: {rl}px;
        padding: 0;
        min-width: 200px;
        min-height: 140px;
        box-shadow: {dash_shadow};
        overflow: hidden;
    }}
    .metis-task-view-card:hover {{
        border-color: rgba({accent_rgb}, 0.45);
    }}
    .metis-task-view-card.selected {{
        border: 1px solid {accent};
        box-shadow: 0 0 0 2px rgba({accent_rgb}, 0.35), {dash_shadow};
    }}
    .metis-task-view-card.dragging {{
        opacity: 0.45;
    }}
    .metis-task-view-drag-preview {{
        background-color: {overlay_card_bg};
        border: 1px solid {accent};
        border-radius: {rl}px;
        overflow: hidden;
        box-shadow: {dash_shadow};
        opacity: 0.95;
    }}
    .metis-task-view-drag-preview-header {{
        background-color: {raised};
        padding: 4px 8px;
        border-bottom: 1px solid {border};
    }}
    label.metis-task-view-drag-preview-title {{
        color: {text};
        font-size: 11px;
        font-weight: 600;
    }}
    .metis-task-view-drag-preview-body {{
        background-color: rgba({text_rgb}, 0.06);
        min-height: 72px;
    }}
    .metis-task-view-card-header {{
        background-color: {raised};
        padding: 4px 4px 4px 8px;
        border-bottom: 1px solid {border};
    }}
    label.metis-task-view-card-title {{
        color: {text};
        font-size: 11px;
        font-weight: 600;
    }}
    button.metis-task-view-card-close {{
        min-width: 22px;
        min-height: 22px;
        padding: 0;
        margin: 0;
        border: none;
        border-radius: {rs}px;
        background: transparent;
        color: {muted};
        box-shadow: none;
    }}
    button.metis-task-view-card-close:hover {{
        background-color: rgba({c_error_rgb}, 0.90);
        color: #ffffff;
    }}
    button.metis-task-view-card-close:active {{
        background-color: {c_error};
        color: #ffffff;
    }}
    .metis-task-view-card-preview {{
        background-color: rgba({text_rgb}, 0.06);
        min-height: 100px;
    }}
    .metis-window-thumb {{
        border-radius: 0;
    }}
    .metis-task-view-shelf-bar {{
        background: transparent;
        padding: 0;
    }}
    .metis-task-view-shelf {{
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        border-radius: {rl}px;
        padding: 10px 16px 8px 16px;
        box-shadow: {dash_shadow};
    }}
    .metis-task-view-shelf-tile {{
        background: transparent;
        border: none;
        padding: 0;
        min-height: 0;
    }}
    .metis-task-view-shelf-tile.active .metis-task-view-shelf-preview {{
        border-color: {accent};
    }}
    .metis-task-view-shelf-tile.active label.metis-task-view-shelf-label {{
        color: {accent};
        font-weight: 700;
        border-bottom: 2px solid {accent};
        padding-bottom: 1px;
    }}
    .metis-task-view-shelf-tile.drop-hover .metis-task-view-shelf-preview {{
        border-color: {accent};
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-task-view-shelf-preview {{
        background-color: {raised};
        border: 1px solid {border};
        border-radius: {rs}px;
        padding: 0;
        min-width: 140px;
        min-height: 72px;
        max-height: 72px;
        overflow: hidden;
    }}
    .metis-task-view-shelf-thumb {{
        border-radius: {rs}px;
    }}
    label.metis-task-view-shelf-fallback {{
        color: rgba({text_rgb}, 0.28);
        font-size: 22px;
        font-weight: 700;
    }}
    label.metis-task-view-shelf-label {{
        color: {muted};
        font-size: 11px;
        margin-top: 2px;
    }}

"#,
    )
}
