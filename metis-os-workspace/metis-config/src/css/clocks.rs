//! Shell stylesheet fragment: `clocks`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Clock / calendar popover: pill tabs ---- */
    .metis-clock-tabs {{
        padding: 2px;
    }}
    .metis-clock-tab {{
        padding: 5px 14px;
        min-height: 0;
        border-radius: 999px;
        color: {muted};
        background-image: none;
        background-color: rgba({text_rgb}, 0.05);
        box-shadow: none;
        border: 1px solid transparent;
    }}
    .metis-clock-tab image {{
        -gtk-icon-size: 15px;
    }}
    .metis-clock-tab:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.09);
    }}
    .metis-clock-tab:checked {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.18);
        border-color: rgba({accent_rgb}, 0.55);
        box-shadow: none;
    }}
    .metis-clock-tab:checked image {{
        color: {accent};
    }}

    /* ---- Stopwatch page ---- */
    .metis-sw-digits {{
        font-size: 46px;
        font-weight: 700;
        color: {text};
        font-feature-settings: "tnum";
        letter-spacing: 0.01em;
    }}
    .metis-sw-btn {{
        padding: 10px 28px;
        min-height: 0;
        border: none;
        border-radius: 999px;
        font-weight: 700;
        box-shadow: none;
        background-image: none;
    }}
    .metis-sw-btn-go {{
        background-color: {accent};
        color: {on_accent};
    }}
    .metis-sw-btn-go:hover {{
        background-color: {accent2};
    }}
    .metis-sw-btn-stop {{
        background-color: rgba({text_rgb}, 0.08);
        color: {text};
    }}
    .metis-sw-btn-stop:hover {{
        background-color: rgba({text_rgb}, 0.14);
    }}
    .metis-sw-btn:disabled {{
        opacity: 0.45;
    }}
    .metis-sw-lap {{
        padding: 8px 12px;
        border-radius: {rs}px;
        background-color: rgba({text_rgb}, 0.05);
    }}
    .metis-sw-lap-total {{
        font-feature-settings: "tnum";
        color: {text};
        font-weight: 600;
    }}
    .metis-sw-lap-delta {{
        font-feature-settings: "tnum";
        color: {accent};
        font-size: 12px;
    }}
    .metis-sw-lap-name {{
        color: {muted};
        font-size: 12px;
    }}

    /* ---- Timer page ---- */
    .metis-timer-digits {{
        font-size: 36px;
        font-weight: 700;
        color: {text};
        font-feature-settings: "tnum";
    }}
    .metis-timer-section {{
        font-size: 11px;
        font-weight: 700;
        color: {muted};
        letter-spacing: 0.06em;
    }}
    .metis-timer-preset {{
        padding: 6px 0;
        min-height: 0;
        border: 1px solid {border};
        border-radius: {rs}px;
        background-color: rgba({text_rgb}, 0.05);
        background-image: none;
        color: {text};
        box-shadow: none;
        font-weight: 600;
        font-size: 12px;
    }}
    .metis-timer-preset:hover {{
        background-color: rgba({accent_rgb}, 0.16);
        border-color: rgba({accent_rgb}, 0.45);
    }}
    .metis-timer-stepper {{
        padding: 4px;
        border-radius: {rm}px;
        background-color: rgba({text_rgb}, 0.05);
    }}
    .metis-timer-step-btn {{
        min-width: 44px;
        min-height: 26px;
        padding: 0;
        border: none;
        border-radius: {rs}px;
        background-color: rgba({text_rgb}, 0.07);
        background-image: none;
        box-shadow: none;
        color: {muted};
    }}
    .metis-timer-step-btn:hover {{
        background-color: rgba({accent_rgb}, 0.18);
        color: {text};
    }}
    .metis-timer-step-value {{
        font-size: 38px;
        font-weight: 700;
        color: {text};
        font-feature-settings: "tnum";
        padding: 2px 0;
    }}
    .metis-timer-colon {{
        font-size: 34px;
        font-weight: 700;
        color: {muted};
        padding: 0 2px;
    }}

    /* ---- Alarm page ---- */
    .metis-alarm-form {{
        padding: 14px;
        border-radius: {rm}px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
    }}
    .metis-alarm-ampm {{
        padding: 8px 16px;
        min-height: 0;
        border: 1px solid {border};
        border-radius: {rs}px;
        background-color: rgba({text_rgb}, 0.07);
        background-image: none;
        box-shadow: none;
        color: {text};
        font-weight: 700;
        margin-left: 6px;
    }}
    .metis-alarm-ampm:hover {{
        background-color: rgba({accent_rgb}, 0.18);
    }}
    .metis-alarm-caption {{
        font-size: 12px;
        font-weight: 700;
        color: {muted};
        letter-spacing: 0.04em;
    }}
    .metis-alarm-day {{
        min-width: 34px;
        min-height: 34px;
        padding: 0;
        border-radius: 999px;
        border: 1px solid {border};
        background-color: rgba({text_rgb}, 0.05);
        background-image: none;
        box-shadow: none;
        color: {muted};
        font-weight: 700;
    }}
    .metis-alarm-day:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.09);
    }}
    .metis-alarm-day:checked {{
        color: {on_accent};
        background-color: {accent};
        border-color: {accent};
    }}
    .metis-clock-card-main {{
        background-color: rgba({accent_rgb}, 0.08);
        border-color: rgba({accent_rgb}, 0.35);
    }}
    .metis-clock-card-time-main {{
        font-size: 24px;
    }}

    .metis-alarm-sound-seg button {{
        padding: 6px 10px;
        min-height: 0;
        border: 1px solid {border};
        background-color: rgba({text_rgb}, 0.05);
        background-image: none;
        box-shadow: none;
        color: {muted};
        font-weight: 600;
    }}
    .metis-alarm-sound-seg button:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.09);
    }}
    .metis-alarm-sound-seg button:checked {{
        color: {on_accent};
        background-color: {accent};
        border-color: {accent};
    }}

    /* ---- Inline timezone picker ---- */
    .metis-tz-picker {{
        padding: 10px;
        border-radius: {rm}px;
        background-color: rgba({text_rgb}, 0.05);
        border: 1px solid {border};
    }}
    .metis-tz-scroll {{
        border-radius: {rs}px;
        background-color: {surface_solid};
        border: 1px solid {border};
    }}
    .metis-tz-list {{
        background-color: transparent;
    }}
    .metis-tz-list row {{
        padding: 0;
        background-color: transparent;
    }}
    .metis-tz-list row:hover {{
        background-color: rgba({accent_rgb}, 0.16);
    }}
    .metis-tz-row {{
        padding: 7px 12px;
        color: {text};
    }}

    /* ---- Stopwatch laps / picker scrollbars (always visible) ---- */
    .metis-sw-laps-scroll scrollbar,
    .metis-tz-scroll scrollbar {{
        background-color: transparent;
    }}
    .metis-sw-laps-scroll scrollbar slider,
    .metis-tz-scroll scrollbar slider {{
        min-width: 7px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.22);
    }}
    .metis-sw-laps-scroll scrollbar slider:hover,
    .metis-tz-scroll scrollbar slider:hover {{
        background-color: rgba({text_rgb}, 0.34);
    }}

    /* ---- Running-timer HUD (layer-shell overlay under the bar) ---- */
    window.metis-timer-hud-window {{
        background-color: transparent;
    }}
    .metis-timer-hud {{
        padding: 8px 12px;
        border-radius: 999px;
        background-color: {raised};
        border: 1px solid rgba({accent_rgb}, 0.45);
        box-shadow: {shadow};
        color: {text};
    }}
    .metis-timer-hud-grip {{
        color: {muted};
        opacity: 0.7;
    }}
    .metis-timer-hud-icon {{
        color: {accent};
    }}
    .metis-timer-hud-time {{
        font-size: 18px;
        font-weight: 700;
        font-feature-settings: "tnum";
        color: {text};
        padding: 0 4px;
    }}
    .metis-timer-hud-btn {{
        min-width: 28px;
        min-height: 28px;
        padding: 2px;
        border: none;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.08);
        background-image: none;
        box-shadow: none;
        color: {text};
    }}
    .metis-timer-hud-btn:hover {{
        background-color: rgba({accent_rgb}, 0.22);
    }}

"#,
    )
}
