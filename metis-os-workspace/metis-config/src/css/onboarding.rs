//! Shell stylesheet fragment: `onboarding`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    let mut out = String::new();
    out.push_str(&v.render(
        r#"
    /* ---- Startup splash (centered overlay layer) ---- */
    window.metis-splash-window {{
        background-color: transparent;
    }}
    .metis-splash-card {{
        padding: 40px 56px 34px 56px;
        border-radius: 28px;
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        box-shadow: {popover_shadow},
                    inset 0 1px 0 rgba({text_rgb}, 0.05);
    }}
    .metis-splash-label {{
        font-size: 12px;
        letter-spacing: 0.4px;
        color: {muted};
    }}
    .metis-splash-progress {{
        min-height: 6px;
    }}
    .metis-splash-progress trough {{
        min-height: 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.12);
        border: none;
    }}
    .metis-splash-progress progress {{
        min-height: 6px;
        border-radius: 999px;
        background-image: linear-gradient(to right,
            {accent} 0%, {accent2} 100%);
        border: none;
    }}

    /* ---- First-run onboarding (content-sized overlay — splash pattern) ---- */
    window.metis-onboarding-window {{
        background-color: transparent;
    }}
    .metis-onboarding-card {{
        padding: 28px 36px 24px 36px;
        border-radius: 24px;
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        box-shadow: {popover_shadow},
                    inset 0 1px 0 rgba({text_rgb}, 0.05);
        min-width: 520px;
        max-width: 520px;
    }}
    .metis-onboarding-body {{
        min-width: 448px;
        max-width: 448px;
        min-height: 300px;
        max-height: 300px;
    }}
    .metis-onboarding-step-content {{
        min-width: 448px;
        max-width: 448px;
    }}
    .metis-onboarding-title {{
        font-size: 22px;
        font-weight: 700;
        color: {text};
    }}
    .metis-onboarding-subtitle {{
        font-size: 14px;
        color: {muted};
        line-height: 1.45;
        max-width: 448px;
    }}
    .metis-onboarding-skip {{
        font-size: 13px;
        color: {muted};
    }}
    .metis-onboarding-skip:hover {{
        color: {text};
    }}
    .metis-onboarding-stepper {{
        min-height: 28px;
    }}
    .metis-onboarding-dot {{
        min-width: 10px;
        min-height: 10px;
        border-radius: 999px;
        background-color: {overlay_dot};
        margin: 0 5px;
    }}
    .metis-onboarding-dot-active {{
        background-color: {accent};
        min-width: 12px;
        min-height: 12px;
    }}
    .metis-onboarding-dot-done {{
        background-color: rgba({accent_rgb}, 0.55);
        min-width: 10px;
        min-height: 10px;
    }}
    .metis-onboarding-preview-tile {{
        border-radius: 12px;
        border: 2px solid transparent;
        padding: 4px;
        min-width: 0;
        min-height: 0;
    }}
    .metis-onboarding-preview-tile:checked {{
        border-color: {accent};
    }}
    /* Theme picker previews (also used by Settings → Appearance). */
"#,
    ));
    out.push_str(&v.render(
        r#"
    .metis-onboarding-wall-grid {{
        margin-top: 4px;
    }}
    .metis-onboarding-wall-pick {{
        padding: 0;
        min-width: 0;
        min-height: 0;
        border: none;
        border-radius: 8px;
        background: transparent;
        box-shadow: none;
        overflow: hidden;
    }}
    .metis-onboarding-wall-img {{
        border-radius: 8px;
    }}
    .metis-onboarding-wall-pick:hover {{
        outline: 2px solid rgba({accent_rgb}, 0.85);
        outline-offset: 1px;
    }}
    .metis-onboarding-wall-pick.selected {{
        outline: 2px solid {accent};
        outline-offset: 1px;
    }}
    .metis-onboarding-hint {{
        font-size: 12px;
        color: {muted};
        max-width: 448px;
    }}
    .metis-onboarding-wifi-list {{
        border-radius: 8px;
        overflow: hidden;
    }}
    .metis-onboarding-wifi-row {{
        padding: 7px 10px;
        border-radius: 6px;
        background-color: transparent;
    }}
    .metis-onboarding-wifi-row-alt {{
        background-color: color-mix(in srgb, {text} 7%, transparent);
    }}
    .metis-onboarding-wifi-row:hover {{
        background-color: color-mix(in srgb, {accent} 14%, transparent);
    }}
    .metis-onboarding-wifi-sheet {{
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        border-radius: 12px;
        padding: 12px 14px;
        box-shadow: {popover_shadow};
    }}
    .metis-onboarding-wifi-sheet-title {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}
    .metis-onboarding-keybind {{
        font-family: monospace;
        font-size: 13px;
        color: {text};
    }}
    .metis-onboarding-optional-list {{
        margin-top: 2px;
    }}
    .metis-onboarding-optional-row {{
        padding: 4px 0;
        min-height: 36px;
    }}
    .metis-onboarding-optional-title {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}
    .metis-onboarding-optional-installed {{
        opacity: 0.55;
    }}
    .metis-onboarding-optional-installed .metis-onboarding-optional-title {{
        color: {muted};
    }}
    .metis-onboarding-nav button {{
        min-width: 96px;
    }}

    .metis-cal-head-weekday {{
        font-size: 13px;
        color: {muted};
    }}
    .metis-cal-head-date {{
        font-size: 22px;
        font-weight: 700;
        color: {text};
    }}
    .metis-cal-title {{
        font-size: 13px;
        font-weight: 700;
        color: {text};
    }}
    .metis-cal-nav, .metis-cal-today-btn {{
        padding: 2px 8px;
        min-height: 0;
        border: 1px solid transparent;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {muted};
        border-radius: {rs}px;
    }}
    .metis-cal-today-btn {{
        font-size: 11px;
        font-weight: 700;
        color: {accent};
    }}
    .metis-cal-nav:hover, .metis-cal-today-btn:hover {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.14);
    }}

    .metis-cal-weekday {{
        font-size: 10px;
        font-weight: 700;
        color: {muted};
        padding: 2px 0;
    }}
    button.metis-cal-day {{
        padding: 2px 0;
        min-width: 36px;
        min-height: 34px;
        border: none;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {text};
        border-radius: {rs}px;
    }}
    button.metis-cal-day:hover {{
        background-color: rgba({text_rgb}, 0.07);
    }}
    button.metis-cal-adjacent {{
        opacity: 0.32;
    }}
    button.metis-cal-today .metis-cal-daynum {{
        color: {accent};
        font-weight: 700;
    }}
    button.metis-cal-selected {{
        background-color: rgba({accent_rgb}, 0.10);
        box-shadow: inset 0 0 0 1px {accent};
    }}
    .metis-cal-daynum {{
        font-size: 12px;
    }}
    .metis-cal-dot {{
        min-width: 5px;
        min-height: 5px;
        background-color: {accent};
        border-radius: 999px;
        margin-top: 1px;
    }}

    .metis-cal-empty {{
        font-size: 12px;
        color: {muted};
        font-style: italic;
    }}
    .metis-cal-add-btn {{
        padding: 4px 10px;
        min-height: 0;
        border: none;
        background-color: rgba({accent_rgb}, 0.14);
        color: {text};
        border-radius: {rs}px;
        box-shadow: none;
    }}
    .metis-cal-add-btn:hover {{
        background-color: rgba({accent_rgb}, 0.24);
    }}

    .metis-bar-dropdown-panel button.metis-cal-event-action {{
        padding: 4px;
        min-width: 28px;
        min-height: 28px;
        border: 1px solid {border};
        background-color: {surface_solid};
        background-image: none;
        box-shadow: none;
        color: {muted};
        border-radius: {rs}px;
    }}
    .metis-bar-dropdown-panel button.metis-cal-event-action:hover {{
        color: {text};
        background-color: {raised};
        border-color: {border};
    }}
    .metis-bar-dropdown-panel button.metis-cal-event-action image {{
        color: {muted};
        -gtk-icon-style: symbolic;
    }}
    .metis-bar-dropdown-panel button.metis-cal-event-action:hover image {{
        color: {text};
    }}

    .metis-bar-dropdown-panel .metis-cal-form button {{
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        box-shadow: none;
        padding: 6px 12px;
    }}
    .metis-bar-dropdown-panel .metis-cal-form button:hover {{
        background-color: {raised};
    }}
    .metis-bar-dropdown-panel .metis-cal-form button.metis-cal-add-btn {{
        background-color: rgba({accent_rgb}, 0.14);
        border-color: rgba({accent_rgb}, 0.45);
        color: {text};
    }}
    .metis-bar-dropdown-panel .metis-cal-form button.metis-cal-add-btn:hover {{
        background-color: rgba({accent_rgb}, 0.24);
    }}

    .metis-bar-dropdown-panel button.metis-cal-add-btn {{
        background-color: rgba({accent_rgb}, 0.14);
        background-image: none;
        color: {text};
        border: 1px solid rgba({accent_rgb}, 0.45);
        border-radius: {rs}px;
        box-shadow: none;
        padding: 4px 10px;
        min-height: 0;
    }}
    .metis-bar-dropdown-panel button.metis-cal-add-btn:hover {{
        background-color: rgba({accent_rgb}, 0.24);
        border-color: rgba({accent_rgb}, 0.55);
    }}
    .metis-bar-dropdown-panel button.metis-cal-add-btn label {{
        color: {text};
    }}
    .metis-bar-dropdown-panel button.metis-cal-add-btn image {{
        color: {text};
        -gtk-icon-style: symbolic;
    }}

    .metis-bar-dropdown-panel button.metis-cal-nav,
    .metis-bar-dropdown-panel button.metis-cal-today-btn {{
        background-color: transparent;
        background-image: none;
        border: 1px solid transparent;
        box-shadow: none;
        color: {muted};
    }}
    .metis-bar-dropdown-panel button.metis-cal-nav:hover,
    .metis-bar-dropdown-panel button.metis-cal-today-btn:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.08);
        border-color: {border};
    }}
    .metis-bar-dropdown-panel button.metis-cal-nav image,
    .metis-bar-dropdown-panel button.metis-cal-today-btn image {{
        color: {muted};
        -gtk-icon-style: symbolic;
    }}
    .metis-bar-dropdown-panel button.metis-cal-nav:hover image,
    .metis-bar-dropdown-panel button.metis-cal-today-btn:hover image {{
        color: {text};
    }}

    .metis-bar-dropdown-panel checkbutton,
    .metis-bar-dropdown-panel checkbutton label {{
        color: {text};
    }}

    .metis-bar-dropdown-panel spinbutton {{
        background-color: {surface_solid};
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
    }}
    .metis-bar-dropdown-panel spinbutton text {{
        color: {text};
        background-color: transparent;
    }}

    .metis-cal-event {{
        padding: 6px 4px;
        border-radius: {rs}px;
    }}
    .metis-cal-event:hover {{
        background-color: rgba({text_rgb}, 0.05);
    }}
    .metis-cal-event-color {{
        background-color: {accent};
        border-radius: 999px;
    }}
    .metis-cal-event-title {{
        font-size: 13px;
        color: {text};
    }}
    .metis-cal-event-sub {{
        font-size: 11px;
        color: {muted};
    }}
    .metis-cal-event-action {{
        padding: 2px;
        min-height: 0;
        min-width: 0;
        border: none;
        background-color: transparent;
        box-shadow: none;
        color: {muted};
        border-radius: {rs}px;
    }}
    .metis-cal-event-action:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.09);
    }}

    .metis-clock-cards {{
        margin-top: 2px;
    }}
    .metis-clock-card {{
        padding: 10px 12px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
        border-radius: {rm}px;
    }}
    .metis-clock-card-name {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
    }}
    .metis-clock-card-offset {{
        font-size: 11px;
        color: {muted};
    }}
    .metis-clock-card-time {{
        font-size: 18px;
        font-weight: 700;
        color: {accent};
    }}
    .metis-entry-error {{
        box-shadow: inset 0 0 0 1px #ff5c5c;
    }}

    .metis-clock-digits {{
        font-size: 30px;
        font-weight: 700;
        color: {text};
        font-feature-settings: "tnum";
    }}
    .metis-clock-btn {{
        padding: 4px 12px;
        min-height: 0;
        border: 1px solid {border};
        border-radius: {rs}px;
        color: {text};
        background-color: {surface_solid};
        background-image: none;
        box-shadow: none;
    }}
    .metis-clock-btn:hover {{
        background-color: {raised};
    }}
    .metis-clock-lap {{
        font-size: 12px;
        color: {muted};
    }}
    .metis-clock-alarm {{
        padding: 6px 8px;
        background-color: rgba({text_rgb}, 0.05);
        border-radius: {rs}px;
    }}
    .metis-clock-alarm-time {{
        font-size: 16px;
        font-weight: 700;
        color: {text};
    }}

    /* ---- Add-event form + Calendars account management ---- */
    .metis-cal-form {{
        padding: 8px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
        border-radius: {rm}px;
    }}
    .metis-acct-form {{
        padding: 12px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
        border-radius: {rm}px;
    }}
    .metis-acct-row {{
        padding: 10px 12px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
        border-radius: {rm}px;
    }}
    .metis-acct-name {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
    }}
    .metis-acct-status {{
        font-size: 11px;
        color: {muted};
    }}

    /* ---- App menu popover ---- */
"#,
    ));
    out
}
