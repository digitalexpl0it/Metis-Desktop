//! Shell stylesheet fragment: `updater`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* Software Updates — opaque xdg_toplevel under Metis SSD. Solid rgb() so
       the shell's global `window {{ transparent }}` cannot leave an alpha hole.
       Polkit auth uses layer-shell Overlay above this window. */
    window.metis-updater,
    window.metis-updater.background {{
        background-color: rgb({raised_rgb});
        color: {text};
    }}
    .metis-updater-shell {{
        background-color: rgb({raised_rgb});
        color: {text};
    }}
    .metis-updater-root {{
        background-color: rgb({raised_rgb});
        color: {text};
        padding: 20px 22px 16px 22px;
    }}
    .metis-updater-hero {{
        padding: 2px 2px 4px 2px;
    }}
    .metis-updater-hero-icon {{
        min-width: 52px;
        min-height: 52px;
        border-radius: 16px;
        background: linear-gradient(145deg, rgba({accent_rgb}, 0.28), rgba({accent_rgb}, 0.08));
        border: 1px solid rgba({accent_rgb}, 0.35);
        color: {accent};
    }}
    .metis-updater-hero-icon image {{
        margin: auto;
        color: {accent};
        -gtk-icon-style: symbolic;
    }}
    .metis-updater-hero-title,
    label.metis-updater-hero-title {{
        font-size: 1.28em;
        font-weight: 700;
        letter-spacing: -0.01em;
        color: {text};
    }}
    .metis-updater-hero-sub,
    label.metis-updater-hero-sub {{
        font-size: 0.95em;
        color: {muted};
        margin-top: 1px;
    }}
    .metis-updater-toolbar {{
        padding: 0 2px;
        margin-top: 2px;
    }}
    .metis-updater-select-hint,
    label.metis-updater-select-hint {{
        color: {muted};
        font-size: 0.88em;
    }}
    /* Circular accent checkmarks (Select all + per-row).
       Force label + indicator colors — Adwaita otherwise paints dark glyphs. */
    window.metis-updater checkbutton.metis-updater-check,
    window.metis-updater checkbutton.metis-updater-select-all {{
        padding: 0;
        margin: 0;
        outline: none;
        box-shadow: none;
        color: {text};
        font-weight: 500;
        font-size: 0.92em;
    }}
    window.metis-updater checkbutton.metis-updater-check label,
    window.metis-updater checkbutton.metis-updater-select-all label {{
        color: {text};
        opacity: 1;
    }}
    window.metis-updater checkbutton.metis-updater-check check,
    window.metis-updater checkbutton.metis-updater-select-all check {{
        min-width: 22px;
        min-height: 22px;
        padding: 0;
        margin: 0;
        border-radius: 999px;
        border: 2px solid rgba({text_rgb}, 0.40);
        background-color: rgba({text_rgb}, 0.06);
        box-shadow: none;
        color: transparent;
        -gtk-icon-size: 12px;
    }}
    window.metis-updater checkbutton.metis-updater-check:hover check,
    window.metis-updater checkbutton.metis-updater-select-all:hover check {{
        border-color: rgba({accent_rgb}, 0.80);
        background-color: rgba({accent_rgb}, 0.12);
    }}
    window.metis-updater checkbutton.metis-updater-check:checked check,
    window.metis-updater checkbutton.metis-updater-check check:checked,
    window.metis-updater checkbutton.metis-updater-select-all:checked check,
    window.metis-updater checkbutton.metis-updater-select-all check:checked {{
        background-color: {accent};
        border-color: {accent};
        /* White tick — text_on_accent is near-black in dark theme (for cyan
           buttons) and reads as an invisible check glyph here. */
        color: #ffffff;
        -gtk-icon-source: -gtk-icontheme("object-select-symbolic");
        box-shadow: 0 0 0 3px rgba({accent_rgb}, 0.18);
    }}
    window.metis-updater checkbutton.metis-updater-check:checked:hover check,
    window.metis-updater checkbutton.metis-updater-select-all:checked:hover check {{
        background-color: {accent2};
        border-color: {accent2};
        color: #ffffff;
    }}
    .metis-updater-scroll {{
        background-color: transparent;
        border-radius: {rm}px;
        margin: 2px 0 0 0;
    }}
    .metis-updater-list {{
        background-color: rgb({surface_rgb}) !important;
        border: 1px solid {border};
        border-radius: {rm}px;
        color: {text};
        padding: 4px 0;
        box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
    }}
    .metis-updater-list row {{
        background-color: transparent;
        color: {text};
        padding: 0;
        margin: 0;
        border-radius: 0;
    }}
    .metis-updater-list row.metis-updater-item-row.metis-updater-zebra {{
        background-color: rgba({text_rgb}, 0.045);
    }}
    .metis-updater-list row.metis-updater-item-row:hover {{
        background-color: rgba({accent_rgb}, 0.08);
    }}
    .metis-updater-list row.metis-updater-item-row.metis-updater-zebra:hover {{
        background-color: rgba({accent_rgb}, 0.10);
    }}
    .metis-updater-section-row {{
        padding: 10px 14px 4px 14px;
        background-color: transparent;
    }}
    .metis-updater-section,
    label.metis-updater-section {{
        font-weight: 600;
        font-size: 0.78em;
        letter-spacing: 0.06em;
        text-transform: uppercase;
        color: {muted};
    }}
    .metis-updater-item {{
        padding: 10px 14px;
        min-height: 48px;
    }}
    .metis-updater-item-name,
    label.metis-updater-item-name {{
        font-weight: 600;
        font-size: 0.98em;
        color: {text};
    }}
    .metis-updater-item-meta,
    label.metis-updater-item-meta {{
        font-size: 0.84em;
        color: {muted};
    }}
    .metis-updater-item-icon {{
        color: {muted};
        opacity: 0.9;
    }}
    .metis-updater-badge,
    label.metis-updater-badge {{
        font-size: 0.72em;
        font-weight: 700;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        padding: 3px 8px;
        border-radius: 999px;
        background-color: rgba({accent_rgb}, 0.18);
        color: {accent};
        border: 1px solid rgba({accent_rgb}, 0.35);
    }}
    button.metis-updater-row-btn {{
        border-radius: {rs}px;
        padding: 5px 12px;
        min-width: 0;
        font-size: 0.88em;
        font-weight: 600;
        color: {text};
        background-color: rgba({text_rgb}, 0.08);
        border: 1px solid {border};
    }}
    button.metis-updater-row-btn label {{
        color: {text};
    }}
    button.metis-updater-row-btn:hover {{
        background-color: rgba({accent_rgb}, 0.16);
        border-color: rgba({accent_rgb}, 0.45);
        color: {text};
    }}
    button.metis-updater-row-btn:hover label {{
        color: {text};
    }}
    .metis-updater-status,
    label.metis-updater-status {{
        color: {muted};
        font-size: 0.9em;
        padding: 0 2px;
    }}
    .metis-updater-progress {{
        margin: 2px 0;
    }}
    .metis-updater-progress trough {{
        border-radius: 999px;
        min-height: 8px;
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-updater-progress progress {{
        border-radius: 999px;
        min-height: 8px;
        background-color: {accent};
    }}
    .metis-updater-actions {{
        margin-top: 6px;
        padding-top: 2px;
    }}
    .metis-updater-reboot {{
        background-color: rgba({accent_rgb}, 0.12);
        border: 1px solid rgba({accent_rgb}, 0.28);
        border-radius: {rm}px;
        padding: 12px 14px;
        color: {text};
        margin: 2px 0;
    }}
    .metis-updater-log,
    textview.metis-updater-log,
    textview.metis-updater-log text,
    textview.metis-updater-log > border {{
        background-color: rgb({surface_rgb}) !important;
        color: {text};
        caret-color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        padding: 10px 12px;
        font-family: monospace;
        font-size: 11px;
        margin-top: 6px;
        box-shadow: none;
    }}
    textview.metis-updater-log text {{
        background-color: rgb({surface_rgb}) !important;
        color: {text};
        padding: 0;
        border: none;
    }}
    button.metis-updater-btn {{
        border-radius: {rs}px;
        padding: 9px 18px;
        color: {text};
        background-color: rgb({surface_rgb});
        border: 1px solid {border};
        min-width: 96px;
        font-weight: 600;
    }}
    button.metis-updater-btn label {{
        color: {text};
    }}
    button.metis-updater-btn:hover {{
        background-color: rgba({text_rgb}, 0.08);
        color: {text};
    }}
    button.metis-updater-btn:hover label {{
        color: {text};
    }}
    button.metis-updater-btn.suggested-action,
    button.metis-updater-btn-primary {{
        background-color: {accent};
        background-image: none;
        color: #ffffff;
        border: 1px solid {accent};
        box-shadow: none;
    }}
    button.metis-updater-btn.suggested-action label,
    button.metis-updater-btn-primary label {{
        color: #ffffff;
        background: none;
    }}
    button.metis-updater-btn.suggested-action:hover,
    button.metis-updater-btn-primary:hover {{
        background-color: {accent2};
        background-image: none;
        color: #ffffff;
        border-color: {accent2};
        box-shadow: none;
    }}
    button.metis-updater-btn.suggested-action:hover label,
    button.metis-updater-btn-primary:hover label {{
        color: #ffffff;
    }}
    button.metis-updater-btn:disabled,
    button.metis-updater-btn:disabled label {{
        opacity: 0.55;
        box-shadow: none;
    }}
    /* ToggleButton paints a filled :checked chrome — keep Show/Hide log flat. */
    button.metis-updater-log-toggle,
    button.metis-updater-log-toggle:checked,
    button.metis-updater-log-toggle:hover,
    button.metis-updater-log-toggle:checked:hover,
    button.metis-updater-log-toggle:active {{
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        color: {text};
        border: 1px solid rgba({text_rgb}, 0.16);
        border-radius: {rs}px;
        padding: 4px 10px;
        margin-top: 0;
        font-size: 0.88em;
        box-shadow: none;
        outline: none;
    }}
    button.metis-updater-log-toggle label,
    button.metis-updater-log-toggle:checked label,
    button.metis-updater-log-toggle:hover label {{
        color: {text};
    }}
    button.metis-updater-log-toggle:hover,
    button.metis-updater-log-toggle:checked:hover {{
        background-color: rgba({text_rgb}, 0.10);
        border-color: rgba({text_rgb}, 0.22);
    }}

    .metis-bar-task-pick.focused {{
        background-color: rgba({accent_rgb}, 0.26);
    }}

    .metis-bar-task-pick.minimized {{
        opacity: 0.6;
    }}

    /* Per-window number pill, shown when an app has multiple windows so the
       picker row correlates with the matching "(n)" in the window's titlebar. */
    .metis-bar-task-pick-num {{
        min-width: 18px;
        padding: 0 5px;
        border-radius: 9px;
        background-color: rgba({accent_rgb}, 0.85);
        color: {on_accent};
        font-size: 11px;
        font-weight: 700;
    }}

    .metis-bar-icon {{
        -gtk-icon-style: symbolic;
        background-color: transparent;
        color: {text};
    }}

    .metis-bar-ws-dot-idle {{
        opacity: 0.5;
    }}

    .metis-bar-ws-dot:hover {{
        background-color: rgba({text_rgb}, 0.30);
    }}

    .metis-bar-ws-dot-active {{
        background-color: {text};
        border-color: {text};
        box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
    }}

    /* Ephemeral FreeRDP dedicated desk — accent so it stands out from 1..=N. */
    .metis-bar-ws-dot-remote {{
        border-color: rgba({accent_rgb}, 0.85);
        background-color: rgba({accent_rgb}, 0.18);
    }}

    .metis-bar-ws-dot-remote:hover {{
        background-color: rgba({accent_rgb}, 0.35);
    }}

    .metis-bar-ws-dot-remote.metis-bar-ws-dot-active {{
        background-color: rgba({accent_rgb}, 0.95);
        border-color: rgba({accent_rgb}, 1);
        box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
    }}

    .metis-notif-dnd-label {{
        font-size: 11px;
        color: {muted};
    }}

"#,
    )
}
