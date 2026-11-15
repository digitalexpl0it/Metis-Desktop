//! Shell stylesheet: file-search layer-shell overlay.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    window.metis-files-search-window {{
        background: transparent;
    }}
    .metis-files-search-backdrop {{
        background-color: rgba(0, 0, 0, 0.28);
    }}
    .metis-files-search-shell {{
        background-color: transparent;
    }}
    .metis-files-search-card {{
        background-color: {dash_panel_bg};
        border: 1px solid {border};
        border-radius: {rl}px;
        box-shadow: {dash_shadow};
        color: {text};
        padding: 14px 16px;
        min-width: 520px;
        min-height: 520px;
    }}
    .metis-files-search-preview {{
        background-color: {surface_solid};
        border: 1px solid {border};
        border-radius: {rl}px;
        box-shadow: {dash_shadow};
        color: {text};
        padding: 14px 16px;
        min-width: 300px;
        max-width: 380px;
        max-height: 520px;
    }}
    .metis-files-search-preview-body-scroll {{
        min-height: 0;
    }}
    .metis-files-search-path-scroll {{
        background-color: {surface_solid};
        border: 1px solid {border};
        border-radius: {rs}px;
        min-height: 28px;
        max-height: 36px;
    }}
    textview.metis-files-search-path,
    textview.metis-files-search-path text {{
        background-color: {surface_solid};
        background-image: none;
        color: {muted};
        font-size: 0.85em;
        caret-color: {muted};
    }}
    .metis-files-search-card .metis-bar-section-title,
    .metis-files-search-preview .metis-bar-section-title {{
        color: {text};
        font-weight: 600;
        letter-spacing: 0.04em;
    }}
    .metis-files-search-card .metis-muted-label,
    .metis-files-search-preview .metis-muted-label {{
        color: {muted};
    }}
    .metis-files-search-entry {{
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: 999px;
        box-shadow: none;
        caret-color: {text};
        min-height: 36px;
        padding: 6px 14px;
    }}
    .metis-files-search-entry text {{
        background-color: transparent;
        color: {text};
    }}
    .metis-files-search-entry text placeholder,
    .metis-files-search-entry > text > placeholder {{
        color: {muted};
        opacity: 1;
    }}
    .metis-files-search-entry:focus-within {{
        border-color: {accent};
    }}
    button.metis-files-search-chip,
    .metis-files-search-chip {{
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: 999px;
        padding: 4px 16px;
        min-height: 28px;
        box-shadow: none;
        outline: none;
    }}
    button.metis-files-search-chip label,
    .metis-files-search-chip label {{
        color: {text};
        font-size: 0.92em;
    }}
    button.metis-files-search-chip:hover,
    .metis-files-search-chip:hover {{
        background-color: rgba({text_rgb}, 0.10);
    }}
    button.metis-files-search-chip:checked,
    .metis-files-search-chip:checked {{
        background-color: rgba({accent_rgb}, 0.22);
        border-color: {accent};
    }}
    button.metis-files-search-chip:checked label,
    .metis-files-search-chip:checked label {{
        color: {text};
        font-weight: 600;
    }}
    .metis-files-search-group-title {{
        color: {muted};
        font-weight: 700;
        font-size: 0.78em;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        padding: 10px 4px 4px;
    }}
    .metis-files-search-row {{
        background-color: rgba({text_rgb}, 0.03);
        border-radius: {rs}px;
        padding: 8px 6px;
    }}
    .metis-files-search-row.zebra {{
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-files-search-row:hover,
    .metis-files-search-row.selected {{
        background-color: rgba({text_rgb}, 0.12);
    }}
    .metis-files-search-row.selected {{
        border: 1px solid rgba({accent_rgb}, 0.35);
    }}
    .metis-files-search-name {{
        color: {text};
        font-weight: 600;
    }}
    .metis-files-search-meta {{
        color: {muted};
        font-size: 0.85em;
    }}
    .metis-files-search-thumb {{
        border-radius: {rs}px;
        background-color: rgba({text_rgb}, 0.06);
        min-width: 48px;
        min-height: 48px;
    }}
    .metis-files-search-icon {{
        color: {muted};
        min-width: 48px;
        min-height: 48px;
    }}
    .metis-files-search-actions button.flat {{
        background-color: transparent;
        background-image: none;
        color: {text};
        border: none;
        box-shadow: none;
        padding: 4px 8px;
        border-radius: 999px;
    }}
    .metis-files-search-actions button.flat:hover {{
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-files-search-actions button.flat label {{
        color: {text};
        font-size: 0.88em;
    }}
    .metis-files-search-card button.flat,
    .metis-files-search-preview button.flat {{
        background-color: transparent;
        background-image: none;
        color: {text};
        border: none;
        box-shadow: none;
    }}
    .metis-files-search-card button.flat:hover,
    .metis-files-search-preview button.flat:hover {{
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-files-search-card button.flat label,
    .metis-files-search-preview button.flat label {{
        color: {text};
    }}
    .metis-files-search-preview-scroll {{
        background-color: {surface_solid};
        border: 1px solid {border};
        border-radius: {rs}px;
        min-height: 160px;
    }}
    .metis-files-search-preview-body,
    textview.metis-files-search-preview-body,
    textview.metis-files-search-preview-body text {{
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        font-family: monospace;
        font-size: 0.85em;
        caret-color: {text};
    }}
    .metis-files-search-preview-thumb {{
        border-radius: {rm}px;
        background-color: rgba({text_rgb}, 0.06);
        min-height: 80px;
        max-height: 160px;
    }}
    .metis-files-search-preview-thumb picture,
    .metis-files-search-preview-thumb image {{
        max-height: 160px;
    }}
    .metis-files-search-results {{
        color: {text};
        min-height: 360px;
    }}
    .metis-files-search-tag-chips {{
        margin-top: 2px;
    }}
    button.metis-files-search-tag-chip {{
        background-color: rgba({text_rgb}, 0.08);
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: 999px;
        padding: 2px 10px;
        min-height: 24px;
        box-shadow: none;
    }}
    button.metis-files-search-tag-chip label {{
        color: {text};
        font-size: 0.82em;
    }}
    button.metis-files-search-tag-chip.auto {{
        background-color: rgba({accent_rgb}, 0.12);
        border-color: rgba({accent_rgb}, 0.35);
    }}
    button.metis-files-search-tag-remove {{
        min-width: 22px;
        min-height: 22px;
        padding: 0;
    }}
    .metis-files-search-tag-add {{
        margin-top: 6px;
    }}
"#,
    )
}
