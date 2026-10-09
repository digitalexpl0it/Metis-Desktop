//! Shell stylesheet fragment: `menu`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    .metis-menu-panel {{
        padding: 14px;
    }}

    /* Tooltip for the icon-only rail: a label inside the menu's GtkOverlay (drawn
       on the menu's own surface, so it can't stack behind the translucent panel
       like a separate popup would). */
    .metis-menu-tooltip-label {{
        padding: 4px 9px;
        border-radius: {rs}px;
        border: 1px solid {border};
        background-color: {raised};
        color: {text};
        font-size: 12px;
    }}

    /* Pin/Unpin sheet — same overlay trick as the rail tooltip. */
    .metis-menu-pin-context {{
        padding: 4px;
        border-radius: {rs}px;
        border: 1px solid {border};
        background-color: {raised};
    }}

    .metis-menu-rail {{
        padding: 2px 10px 2px 0;
        margin-right: 4px;
        border-right: 1px solid {border};
    }}

    .metis-menu-rail-btn {{
        padding: 8px;
        min-width: 0;
        min-height: 0;
        border: none;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {muted};
        border-radius: {rs}px;
    }}
    .metis-menu-rail-btn:hover {{
        color: {accent};
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-menu-rail-btn:active {{
        background-color: rgba({accent_rgb}, 0.22);
    }}

    /* Keep the whole panel (rail + center + divider + pinned + padding) under
       ~580px so it fits within a single narrow output's placement area (e.g. a
       640px monitor or a split dev output) instead of overflowing onto the
       neighbouring display, which leaves the popover unable to open. */
    .metis-menu-center {{
        min-width: 268px;
    }}

    .metis-menu-pinned {{
        min-width: 204px;
        margin-left: 4px;
    }}

    .metis-menu-divider {{
        background-color: {border};
        min-width: 1px;
        margin: 4px 8px;
    }}

    .metis-menu-scroll {{
        min-height: 420px;
    }}
    /* Dark Adwaita draws visible undershoot/overshoot edges and opaque troughs
       on GtkScrolledWindow — kill them so the gutter stays scrollable and flat. */
    .metis-menu-scroll undershoot.top,
    .metis-menu-scroll undershoot.bottom,
    .metis-menu-scroll undershoot.left,
    .metis-menu-scroll undershoot.right,
    .metis-menu-scroll overshoot.top,
    .metis-menu-scroll overshoot.bottom,
    .metis-menu-scroll overshoot.left,
    .metis-menu-scroll overshoot.right {{
        background-color: transparent;
        background-image: none;
        box-shadow: none;
        border: none;
    }}
    .metis-menu-scroll scrollbar {{
        background-color: transparent;
        border: none;
        box-shadow: none;
    }}
    .metis-menu-scroll scrollbar trough {{
        background-color: transparent;
        border: none;
        box-shadow: none;
    }}
    .metis-menu-scroll scrollbar slider {{
        min-width: 7px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.22);
    }}
    .metis-menu-scroll scrollbar slider:hover {{
        background-color: rgba({text_rgb}, 0.34);
    }}

    .metis-menu-row {{
        padding: 7px 8px;
        border: none;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {text};
        border-radius: {rs}px;
    }}
    .metis-menu-row:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-menu-row:active {{
        background-color: rgba({accent_rgb}, 0.22);
    }}

    .metis-menu-letter {{
        font-size: 11px;
        font-weight: 700;
        color: {muted};
        padding: 8px 8px 2px 8px;
    }}

    .metis-menu-empty {{
        font-size: 12px;
        color: {muted};
        padding: 10px 8px;
    }}

    .metis-menu-search {{
        margin-top: 4px;
        border-radius: {rs}px;
        background-color: {surface_solid};
        border: 1px solid {border};
        color: {text};
        caret-color: {text};
        box-shadow: none;
    }}
    .metis-menu-search-row {{
        margin-bottom: 2px;
    }}
    .metis-menu-search-row .metis-menu-search {{
        margin-top: 0;
    }}
    .metis-menu-user {{
        padding: 2px 2px 10px 2px;
        border-bottom: 1px solid {border};
        margin-bottom: 4px;
    }}
    .metis-menu-user-avatar {{
        border-radius: 999px;
        min-width: 40px;
        min-height: 40px;
    }}
    .metis-menu-user-name {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
    }}
    /* Style presets mostly share the same panel; Whisker/Mint keep search flush. */
    .metis-menu-style-whisker .metis-menu-scroll,
    .metis-menu-style-mint .metis-menu-scroll {{
        min-height: 400px;
    }}
    .metis-menu-search > text {{
        background-color: transparent;
        color: {text};
    }}
    .metis-menu-search > text > placeholder {{
        color: {muted};
    }}
    .metis-menu-search image {{
        color: {muted};
    }}
    .metis-menu-search:focus-within {{
        border-color: {accent};
    }}

    /* Layout pack: Bracket / Ladder categories */
    .metis-menu-categories {{
        background-color: transparent;
        border: none;
    }}
    .metis-menu-categories row {{
        border-radius: {rs}px;
        margin: 1px 4px;
        padding: 0;
        color: {text};
    }}
    .metis-menu-categories row:hover {{
        background-color: rgba({accent_rgb}, 0.12);
    }}
    .metis-menu-categories row:selected {{
        background-color: rgba({accent_rgb}, 0.22);
        color: {text};
    }}
    .metis-menu-style-bracket .metis-menu-scroll,
    .metis-menu-style-ladder .metis-menu-scroll,
    .metis-menu-style-ledger .metis-menu-scroll {{
        min-height: 420px;
        min-width: 280px;
    }}
    .metis-menu-style-bracket .metis-menu-body,
    .metis-menu-style-ladder .metis-menu-body {{
        min-width: 520px;
    }}

    /* Mosaic / Plaza / Crest / Chip grids */
    .metis-menu-style-mosaic .metis-menu-scroll,
    .metis-menu-style-plaza .metis-menu-scroll,
    .metis-menu-style-crest .metis-menu-scroll {{
        min-height: 360px;
        min-width: 420px;
    }}
    .metis-menu-style-chip .metis-menu-scroll {{
        min-height: 320px;
        min-width: 360px;
    }}
    .metis-menu-app-grid {{
        padding: 4px;
    }}
    .metis-menu-app-grid-dense .metis-menu-tile {{
        padding: 6px 4px;
    }}
    .metis-menu-app-grid-dense .metis-menu-tile-label {{
        font-size: 10px;
    }}

    /* Crest centered avatar header */
    .metis-menu-crest-header {{
        padding: 8px 8px 12px 8px;
        border-bottom: 1px solid {border};
        margin-bottom: 4px;
    }}
    .metis-menu-crest-avatar {{
        border-radius: 999px;
        min-width: 72px;
        min-height: 72px;
    }}

    /* Plaza footer: profile + power */
    .metis-menu-plaza-footer {{
        padding: 8px 4px 2px 4px;
        border-top: 1px solid {border};
        margin-top: 4px;
    }}
    .metis-menu-plaza-footer .metis-menu-user-avatar {{
        min-width: 28px;
        min-height: 28px;
    }}

    /* Ramp: Dash-like wider center */
    .metis-menu-style-ramp .metis-menu-center {{
        min-width: 320px;
    }}
    .metis-menu-style-ramp .metis-menu-scroll {{
        min-height: 400px;
    }}

    .metis-menu-pinned-flow {{
        padding: 2px 2px 2px 2px;
    }}

    .metis-menu-tile {{
        padding: 10px 6px;
        border: none;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {text};
        border-radius: {rm}px;
    }}
    .metis-menu-tile:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-menu-tile:active {{
        background-color: rgba({accent_rgb}, 0.22);
    }}

    .metis-menu-tile-label {{
        font-size: 11px;
        color: {text};
        margin-top: 2px;
    }}

"#,
    )
}
