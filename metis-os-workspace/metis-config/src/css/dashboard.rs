//! Shell stylesheet fragment: `dashboard`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    window.metis-dashboard-window {{
        background: transparent;
        overflow: hidden;
    }}
    .metis-dashboard-root {{
        background-color: {dash_panel_bg};
        border-radius: {rl}px;
        border: 1px solid {border};
        box-shadow: {dash_shadow};
        overflow: hidden;
        min-height: 0;
        min-width: 0;
        margin-top: 4px;
        color: {text};
    }}
    .metis-dashboard-host {{
        min-height: 0;
        min-width: 0;
        overflow: hidden;
        border-radius: {rl}px;
    }}
    .metis-dashboard-root-bottom {{
        border-radius: {rl}px;
        margin-top: 0;
        margin-bottom: 4px;
        box-shadow: {dash_shadow_up};
    }}
    .metis-dashboard-root-left {{
        border-radius: {rl}px;
        margin-top: 0;
        margin-bottom: 0;
        margin-start: 4px;
        margin-end: 0;
        box-shadow: {dash_shadow};
    }}
    .metis-dashboard-root-right {{
        border-radius: {rl}px;
        margin-top: 0;
        margin-bottom: 0;
        margin-start: 0;
        margin-end: 4px;
        box-shadow: {dash_shadow};
    }}
    .metis-dashboard-header {{
        padding: 8px 14px 6px 14px;
        border-bottom: 1px solid {border};
        background-color: transparent;
    }}
    .metis-dashboard-root-bottom .metis-dashboard-header {{
        border-bottom: none;
        border-top: 1px solid {border};
    }}
    .metis-dashboard-stack {{
        min-height: 0;
        background-color: transparent;
    }}
    .metis-dashboard-title {{
        font-size: 15px;
        font-weight: 600;
        color: {text};
    }}
    button.metis-dashboard-close {{
        min-width: 32px;
        min-height: 32px;
        padding: 4px;
        color: {muted};
        background-image: none;
        background-color: transparent;
        border: none;
        box-shadow: none;
        border-radius: {rs}px;
    }}
    button.metis-dashboard-close:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.10);
    }}
    button.metis-dashboard-close:active {{
        background-color: rgba({text_rgb}, 0.16);
    }}
    .metis-dash-tabs,
    stackswitcher.metis-dash-tabs {{
        padding: 2px;
        background-color: transparent;
        background-image: none;
        box-shadow: none;
        border: none;
    }}
    /* Class lives on the StackSwitcher itself — nest selectors never matched,
       so Adwaita prefer-dark kept charcoal Overview/Processes chips in light mode. */
    stackswitcher.metis-dash-tabs > button,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button,
    .metis-dashboard-root stackswitcher > button {{
        padding: 5px 14px;
        min-height: 0;
        border-radius: 999px;
        font-size: 12px;
        font-weight: 500;
        color: {muted};
        background-image: none;
        background-color: rgba({text_rgb}, 0.06);
        box-shadow: none;
        border: 1px solid transparent;
        outline: none;
        -gtk-icon-filter: none;
    }}
    stackswitcher.metis-dash-tabs > button label,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button label,
    .metis-dashboard-root stackswitcher > button label {{
        color: {muted};
    }}
    stackswitcher.metis-dash-tabs > button:hover,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button:hover,
    .metis-dashboard-root stackswitcher > button:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.10);
    }}
    stackswitcher.metis-dash-tabs > button:hover label,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button:hover label,
    .metis-dashboard-root stackswitcher > button:hover label {{
        color: {text};
    }}
    stackswitcher.metis-dash-tabs > button:checked,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button:checked,
    .metis-dashboard-root stackswitcher > button:checked {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.20);
        border-color: rgba({accent_rgb}, 0.50);
    }}
    stackswitcher.metis-dash-tabs > button:checked label,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button:checked label,
    .metis-dashboard-root stackswitcher > button:checked label {{
        color: {text};
        font-weight: 600;
    }}
    stackswitcher.metis-dash-tabs > button:checked:hover,
    .metis-dashboard-root stackswitcher.metis-dash-tabs > button:checked:hover,
    .metis-dashboard-root stackswitcher > button:checked:hover {{
        background-color: rgba({accent_rgb}, 0.26);
    }}
    .metis-dashboard-proc-header label {{
        font-size: 11px;
        font-weight: 600;
        color: {muted};
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }}
    .metis-dashboard-filter {{
        min-width: 140px;
    }}
    .metis-dashboard-card {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        min-width: 280px;
    }}
    .metis-dashboard-card-title {{
        font-size: 12px;
        font-weight: 600;
        color: {muted};
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }}
    .metis-dashboard-search {{
        border-radius: {rs}px;
    }}
    .metis-dashboard-process-row {{
        border-bottom: 1px solid rgba({accent_rgb}, 0.08);
    }}
    .metis-dashboard-process-metis {{
        color: {accent};
        font-weight: 600;
    }}
    .metis-dash-health-row {{
        margin-bottom: 2px;
    }}
    .metis-dash-health {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 8px 12px;
    }}
    image.metis-dash-card-icon {{
        -gtk-icon-size: 16px;
        color: {accent};
        opacity: 0.92;
    }}
    .metis-dash-value-inline {{
        font-size: 15px;
        font-weight: 600;
        color: {text};
    }}
    .metis-dash-chart-cpu {{
        min-height: 120px;
    }}
    paned.metis-dash-paned separator {{
        background-color: {border};
        min-width: 1px;
    }}
    .metis-dash-pane-left {{
        min-width: 280px;
    }}
    .metis-dash-legend {{
        margin-top: 2px;
    }}
    .metis-dash-legend-label {{
        font-size: 10px;
        color: {muted};
    }}
    .metis-dash-mid {{
        margin-top: 2px;
    }}
    .metis-dash-mid > widget:nth-child(1) {{
        min-width: 280px;
    }}
    .metis-dash-session-value {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
        line-height: 1.35;
    }}
    .metis-dash-session-grid {{
        margin-top: 2px;
    }}
    .metis-dash-session-key {{
        font-size: 12px;
        font-weight: 500;
        color: {muted};
        min-width: 96px;
    }}
    button.metis-dash-proc-expand {{
        min-width: 24px;
        min-height: 24px;
        padding: 0;
        background: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        color: {muted};
    }}
    button.metis-dash-proc-expand:hover {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.12);
    }}
    button.metis-dash-proc-expand image {{
        color: inherit;
        -gtk-icon-filter: none;
    }}
    .metis-dash-process-page {{
        min-height: 0;
    }}
    .metis-dash-proc-panel {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        min-height: 0;
    }}
    .metis-dashboard-scroll scrollbar,
    .metis-dashboard-scroll scrollbar.vertical,
    .metis-dashboard-scroll scrollbar.horizontal {{
        background-color: transparent;
        border: none;
        box-shadow: none;
        background-image: none;
    }}
    .metis-dashboard-scroll scrollbar trough {{
        background-color: transparent;
        border: none;
        box-shadow: none;
        background-image: none;
    }}
    .metis-dashboard-scroll scrollbar slider {{
        min-width: 8px;
        min-height: 8px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.22);
        background-image: none;
        border: none;
        box-shadow: none;
    }}
    .metis-dashboard-scroll scrollbar slider:hover {{
        background-color: rgba({text_rgb}, 0.34);
    }}
    .metis-dashboard-scroll scrollbar slider:active {{
        background-color: rgba({accent_rgb}, 0.45);
    }}
    .metis-dash-disk-grid {{
        margin-top: 4px;
    }}
    .metis-dash-disk-tile {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rs}px;
        min-width: 180px;
    }}
    .metis-dash-overview {{
        min-height: 360px;
    }}
    .metis-dash-overview-body {{
        min-height: 0;
    }}
    button.metis-dash-sort {{
        background: transparent;
        border: none;
        padding: 2px 0;
        font-size: 11px;
        font-weight: 600;
        color: {muted};
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }}
    button.metis-dash-sort.metis-dash-sort-active {{
        color: {accent};
    }}
    .metis-dash-proc-cols {{
        grid-template-columns: minmax(140px, 2.2fr) 64px 88px 64px 64px 80px 36px;
    }}
    .metis-dash-proc-cols > label,
    .metis-dash-proc-cols > button {{
        min-width: 0;
    }}
    button.metis-dash-sort {{
        width: 100%;
    }}
    button.metis-dash-sort.metis-dash-sort-end {{
        margin-left: auto;
    }}
    button.metis-dash-sort.metis-dash-sort-end label {{
        margin-left: auto;
    }}
    .metis-dash-health-value {{
        font-size: 14px;
        font-weight: 600;
    }}
    .metis-dash-health-good {{
        color: {c_success};
    }}
    .metis-dash-health-warn {{
        color: {c_warning};
    }}
    .metis-dash-health-crit {{
        color: {c_error};
    }}
    popover.metis-dash-popover {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}
    popover.metis-dash-popover contents {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 0;
        color: {text};
        box-shadow: {dash_shadow};
    }}
    popover.metis-dash-popover > arrow {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
    }}
    .metis-dash-popover-title {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}
    button.metis-dash-menu-item {{
        border-radius: 0;
        border: none;
        background-image: none;
        background-color: transparent;
        color: {text};
        padding: 8px 12px;
        min-height: 32px;
        box-shadow: none;
    }}
    button.metis-dash-menu-item:hover {{
        background-color: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    button.metis-dash-menu-item label {{
        font-size: 13px;
        color: {text};
    }}
    .metis-dash-context-menu {{
        padding: 2px 0;
        background: transparent;
        color: {text};
    }}
    .metis-dash-metrics {{
        margin-top: 4px;
    }}
    .metis-dash-card {{
        background-color: {dash_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 10px 12px;
    }}
    .metis-dash-card-title {{
        font-size: 11px;
        font-weight: 600;
        color: {muted};
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }}
    .metis-dash-value {{
        font-size: 20px;
        font-weight: 600;
        color: {text};
    }}
    .metis-dash-sub {{
        font-size: 12px;
        color: {muted};
    }}
    .metis-dash-muted {{
        color: {muted};
        font-size: 12px;
    }}
    .metis-dash-gauge {{
        margin-top: 2px;
    }}
    .metis-dash-gauge-value {{
        font-size: 15px;
        font-weight: 600;
        color: {text};
        margin-bottom: 2px;
    }}
    .metis-dash-gauge-card {{
        min-width: 108px;
        padding: 10px 10px 8px;
    }}
    .metis-dash-temp-gauges {{
        flex-shrink: 0;
    }}
    .metis-dash-system-row {{
        align-items: start;
    }}
    .metis-dash-chart {{
        margin-top: 2px;
        min-height: 64px;
    }}
    levelbar.metis-dash-meter {{
        min-height: 6px;
        border-radius: 999px;
    }}
    levelbar.metis-dash-meter block {{
        background-color: {accent};
        border-radius: 999px;
    }}
    levelbar.metis-dash-meter block.empty {{
        background-color: rgba({accent_rgb}, 0.12);
    }}
    .metis-dash-kv-grid {{
        margin-top: 6px;
    }}
    .metis-dash-kv-key {{
        font-size: 12px;
        color: {muted};
        min-width: 88px;
    }}
    .metis-dash-kv {{
        font-size: 13px;
        color: {text};
    }}
    .metis-dash-search {{
        border-radius: {rs}px;
        color: {text};
        background-color: rgba({text_rgb}, 0.06);
        border: 1px solid {border};
        background-image: none;
        box-shadow: none;
    }}
    .metis-dash-search text,
    .metis-dash-search > text {{
        color: {text};
        background: transparent;
        caret-color: {text};
    }}
    .metis-dash-search text placeholder,
    .metis-dash-search > text > placeholder {{
        color: {muted};
    }}
    .metis-dash-search:focus-within {{
        border-color: rgba({accent_rgb}, 0.45);
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-dash-filter {{
        min-width: 140px;
        color: {text};
        background-color: rgba({text_rgb}, 0.06);
        border: 1px solid {border};
        border-radius: {rs}px;
        background-image: none;
        box-shadow: none;
    }}
    .metis-dash-filter button,
    .metis-dash-filter > button,
    dropdown.metis-dash-filter > button {{
        background: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        color: {text};
    }}
    .metis-dash-filter label,
    dropdown.metis-dash-filter label {{
        color: {text};
    }}
    .metis-dash-filter arrow,
    dropdown.metis-dash-filter arrow {{
        color: {muted};
        -gtk-icon-filter: none;
    }}
    /* DropDown list popover is often a transient popup (not nested under
       .metis-dashboard-root in the CSS path) — pin Metis tokens so Adwaita
       prefer-dark does not leave a charcoal menu in light themes. */
    window.metis-dashboard-window popover.menu contents,
    window.metis-dashboard-window popover contents,
    .metis-dashboard-root popover.menu contents,
    .metis-dashboard-root popover contents,
    popover.menu contents {{
        background-color: {dash_card_bg};
        color: {text};
        border: 1px solid {border};
        border-radius: {rm}px;
        box-shadow: {dash_shadow};
        padding: 4px;
    }}
    window.metis-dashboard-window popover.menu listview,
    window.metis-dashboard-window popover listview,
    window.metis-dashboard-window popover.menu listview row,
    window.metis-dashboard-window popover listview row,
    .metis-dashboard-root popover listview,
    .metis-dashboard-root popover listview row,
    popover.menu listview,
    popover.menu listview row {{
        background-color: transparent;
        background-image: none;
        color: {text};
        border-radius: {rs}px;
        padding: 4px 8px;
        box-shadow: none;
    }}
    window.metis-dashboard-window popover.menu listview row:hover,
    window.metis-dashboard-window popover listview row:hover,
    window.metis-dashboard-window popover.menu listview row:selected,
    window.metis-dashboard-window popover listview row:selected,
    .metis-dashboard-root popover listview row:hover,
    .metis-dashboard-root popover listview row:selected,
    popover.menu listview row:hover,
    popover.menu listview row:selected {{
        background-color: rgba({accent_rgb}, 0.14);
        color: {text};
    }}
    window.metis-dashboard-window popover.menu label,
    window.metis-dashboard-window popover label,
    .metis-dashboard-root popover.menu label,
    .metis-dashboard-root popover label,
    popover.menu label {{
        color: {text};
    }}
    button.metis-dash-monitor-btn {{
        background-image: none;
        background-color: rgba({text_rgb}, 0.06);
        border: 1px solid {border};
        border-radius: {rs}px;
        color: {text};
        box-shadow: none;
    }}
    button.metis-dash-monitor-btn:hover {{
        background-color: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    button.metis-dash-monitor-btn label {{
        color: inherit;
    }}
    .metis-dash-table-head label {{
        font-size: 11px;
        font-weight: 600;
        color: {muted};
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }}
    list.metis-dash-table {{
        background: transparent;
        color: {text};
        border: none;
        box-shadow: none;
    }}
    list.metis-dash-table row.metis-dash-table-row {{
        padding: 0;
        border: none;
        background-image: none;
        color: {text};
    }}
    list.metis-dash-table row.metis-dash-table-row label {{
        color: {text};
    }}
    list.metis-dash-table row.metis-dash-table-row label.metis-dash-proc-name {{
        color: {text};
        font-weight: 500;
    }}
    list.metis-dash-table row.metis-dash-table-row label.metis-dash-muted {{
        color: {muted};
    }}
    list.metis-dash-table row.metis-dash-table-row label.metis-dash-process-metis {{
        color: {accent};
        font-weight: 600;
    }}
    list.metis-dash-table row.metis-dash-table-row:nth-child(odd) {{
        background-color: rgba({accent_rgb}, 0.04);
    }}
    list.metis-dash-table row.metis-dash-table-row:nth-child(even) {{
        background-color: transparent;
    }}
    list.metis-dash-table row.metis-dash-table-row:hover {{
        background-color: rgba({accent_rgb}, 0.10);
    }}
    list.metis-dash-table row.metis-dash-table-row button.flat,
    list.metis-dash-table row.metis-dash-table-row button {{
        background-image: none;
        background-color: transparent;
        border: none;
        box-shadow: none;
        color: {muted};
        min-width: 28px;
        min-height: 28px;
        padding: 2px;
        border-radius: {rs}px;
    }}
    list.metis-dash-table row.metis-dash-table-row button:hover {{
        background-color: rgba({accent_rgb}, 0.14);
        color: {text};
    }}
    list.metis-dash-table row.metis-dash-table-row button image {{
        color: inherit;
        -gtk-icon-filter: none;
    }}
    .metis-dash-process-metis {{
        color: {accent};
        font-weight: 600;
    }}
    /* Force Control Center chrome onto Metis tokens (Adwaita prefer-dark otherwise
       leaves light/white labels on the frosted light panel). */
    .metis-dashboard-root label {{
        color: {text};
    }}
    .metis-dashboard-root label.metis-dash-muted,
    .metis-dashboard-root .metis-dash-muted {{
        color: {muted};
    }}
    .metis-dashboard-root label.metis-dash-proc-name,
    .metis-dashboard-root .metis-dash-proc-name {{
        color: {text};
        font-weight: 500;
    }}
    .metis-dashboard-root label.metis-dash-process-metis,
    .metis-dashboard-root .metis-dash-process-metis {{
        color: {accent};
    }}
    .metis-dashboard-root label.metis-dash-card-title,
    .metis-dashboard-root .metis-dash-card-title,
    .metis-dashboard-root label.metis-dash-legend-label,
    .metis-dashboard-root .metis-dash-legend-label,
    .metis-dashboard-root label.metis-dash-session-key,
    .metis-dashboard-root .metis-dash-session-key,
    .metis-dashboard-root label.metis-dash-kv-key,
    .metis-dashboard-root .metis-dash-kv-key,
    .metis-dashboard-root label.metis-dash-sub,
    .metis-dashboard-root .metis-dash-sub {{
        color: {muted};
    }}
    .metis-dashboard-root button.metis-dash-sort {{
        color: {muted};
        background-image: none;
        background-color: transparent;
        border: none;
        box-shadow: none;
    }}
    .metis-dashboard-root button.metis-dash-sort label {{
        color: inherit;
    }}
    .metis-dashboard-root button.metis-dash-sort.metis-dash-sort-active {{
        color: {accent};
    }}
    .metis-dashboard-root button.metis-dash-sort.metis-dash-sort-active label {{
        color: {accent};
    }}
    label.dim-label {{
        color: {muted};
        font-size: 12px;
    }}

"#,
    )
}
