//! Shell stylesheet fragment: `osd_network`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Hardware-key OSD (volume / brightness / media, bottom-center) ---- */
    window.metis-osd-window {{
        background-color: transparent;
    }}
    .metis-osd-card {{
        background-color: {toast_card_bg};
        border-radius: 18px;
        border: 1px solid {border};
        padding: 20px 24px;
        box-shadow: {dash_shadow};
        color: {text};
        min-width: 220px;
    }}
    .metis-osd-card.muted {{
        opacity: 0.72;
    }}
    .metis-osd-icon {{
        color: {text};
    }}
    .metis-osd-title {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}
    .metis-osd-percent {{
        font-size: 12px;
        color: {muted};
    }}
    .metis-osd-level trough {{
        min-height: 8px;
        border-radius: 6px;
        /* Neutral track — never tint with accent, or a cyan theme makes the
           empty portion look identical to the filled bar. */
        background-color: rgba({text_rgb}, 0.18);
    }}
    .metis-osd-level block.empty {{
        min-height: 8px;
        border-radius: 6px;
        background-color: transparent;
    }}
    .metis-osd-level block.filled {{
        min-height: 8px;
        border-radius: 6px;
        background-color: {accent};
    }}
    .metis-osd-card.muted .metis-osd-level block.filled {{
        background-color: {muted};
    }}

    /* Kind tint: solid card fill + icon/accent colour — never `transparent`
       (that opens a hole through the ARGB toast / NC surface onto the wallpaper). */
    .metis-notif-card-error,
    .metis-toast-card.metis-notif-card-error {{
        background-color: {toast_card_bg};
        background-image: none;
        border-left-color: {c_error};
    }}
    .metis-notif-card-error .metis-notif-icon:not(.metis-notif-icon-app) {{
        color: {c_error};
    }}

    .metis-notif-card-notify,
    .metis-toast-card.metis-notif-card-notify {{
        background-color: {toast_card_bg};
        background-image: none;
        border-left-color: {c_warning};
    }}
    .metis-notif-card-notify .metis-notif-icon:not(.metis-notif-icon-app) {{
        color: {c_warning};
    }}

    .metis-notif-card-success,
    .metis-toast-card.metis-notif-card-success {{
        background-color: {toast_card_bg};
        background-image: none;
        border-left-color: {c_success};
    }}
    .metis-notif-card-success .metis-notif-icon:not(.metis-notif-icon-app) {{
        color: {c_success};
    }}

    .metis-notif-card-info,
    .metis-toast-card.metis-notif-card-info {{
        background-color: {toast_card_bg};
        background-image: none;
        border-left-color: {c_info};
    }}
    .metis-notif-card-info .metis-notif-icon:not(.metis-notif-icon-app) {{
        color: {c_info};
    }}

    .metis-notif-card-payment,
    .metis-toast-card.metis-notif-card-payment {{
        background-color: {toast_card_bg};
        background-image: none;
        border-left-color: {c_payment};
    }}
    .metis-notif-card-payment .metis-notif-icon:not(.metis-notif-icon-app) {{
        color: {c_payment};
    }}

    .metis-clipboard-panel {{
        min-width: 380px;
    }}

    .metis-clipboard-search {{
        margin-bottom: 4px;
    }}

    .metis-clipboard-list {{
        margin: 0;
    }}

    .metis-clipboard-row {{
        padding: 6px 4px;
        border-bottom: 1px solid rgba({text_rgb}, 0.08);
    }}

    .metis-clipboard-active-marker {{
        color: {accent};
        font-size: 10px;
    }}

    .metis-clipboard-inactive-marker {{
        color: transparent;
        font-size: 10px;
    }}

    .metis-clipboard-body {{
        padding: 4px 6px;
        border-radius: 6px;
    }}

    .metis-clipboard-body:hover {{
        background-color: rgba({text_rgb}, 0.06);
    }}

    .metis-clipboard-preview {{
        color: {text};
        font-size: 13px;
    }}

    .metis-clipboard-row-action,
    .metis-clipboard-icon-btn,
    .metis-clipboard-footer-btn {{
        padding: 4px;
        min-width: 28px;
        min-height: 28px;
        border-radius: 6px;
        background-color: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        color: {muted};
    }}

    .metis-clipboard-row-action image,
    .metis-clipboard-icon-btn image,
    .metis-clipboard-footer-btn image {{
        color: {muted};
        -gtk-icon-style: symbolic;
    }}

    .metis-clipboard-row-action:hover,
    .metis-clipboard-icon-btn:hover,
    .metis-clipboard-footer-btn:hover {{
        background-color: rgba({text_rgb}, 0.08);
        color: {text};
    }}

    .metis-clipboard-row-action:hover image,
    .metis-clipboard-icon-btn:hover image,
    .metis-clipboard-footer-btn:hover image {{
        color: {text};
    }}

    .metis-clipboard-pinned,
    .metis-clipboard-pinned image {{
        color: {accent};
    }}

    .metis-clipboard-footer {{
        padding-top: 6px;
        border-top: 1px solid rgba({text_rgb}, 0.08);
    }}

    .metis-clipboard-settings-menu {{
        min-width: 220px;
        padding: 6px;
    }}

    .metis-bar-dropdown-panel .metis-clipboard-settings-item,
    .metis-clipboard-settings-menu .metis-clipboard-settings-item {{
        background-color: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        padding: 8px 10px;
        border-radius: {rs}px;
        color: {text};
    }}

    .metis-clipboard-settings-item:hover {{
        background-color: rgba({text_rgb}, 0.08);
    }}

    .metis-clipboard-settings-item.metis-clipboard-settings-active {{
        background-color: rgba({accent_rgb}, 0.14);
    }}

    .metis-clipboard-settings-item.metis-clipboard-settings-active
    .metis-clipboard-settings-label {{
        font-weight: 600;
    }}

    .metis-clipboard-settings-label {{
        color: {text};
        font-size: 13px;
    }}

    .metis-clipboard-settings-check {{
        min-width: 18px;
        color: {accent};
        -gtk-icon-style: symbolic;
    }}

    .metis-bar-volume-scale {{
        min-width: 180px;
        padding: 2px 0;
    }}

    .metis-bar-volume-scale trough {{
        background-color: rgba(255, 255, 255, 0.12);
        border: none;
        border-radius: 999px;
        min-height: 5px;
    }}

    .metis-bar-volume-scale highlight {{
        background-color: {accent};
        border-radius: 999px;
        min-height: 5px;
    }}

    .metis-bar-volume-scale slider {{
        background-color: #ffffff;
        border: none;
        border-radius: 999px;
        min-width: 15px;
        min-height: 15px;
        margin: -6px;
        box-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    }}

    .metis-bar-volume-scale value {{
        color: {muted};
        font-size: 12px;
        margin-left: 8px;
    }}

    .metis-bar-audio-mute {{
        padding: 6px;
        margin: 0;
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

    .metis-bar-audio-mute:hover {{
        color: {accent};
        background-color: rgba({accent_rgb}, 0.12);
    }}

    .metis-bar-audio-mute:active {{
        background-color: rgba({accent_rgb}, 0.20);
    }}

    .metis-net-eth-row {{
        padding: 6px 4px;
        border-bottom: 1px solid {border};
        color: {text};
    }}

    .metis-net-row {{
        padding: 6px 6px;
        border: none;
        outline: none;
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {text};
        border-radius: {rs}px;
    }}

    .metis-net-row:hover {{
        background-color: rgba({accent_rgb}, 0.12);
    }}

    .metis-net-lock {{
        color: {muted};
    }}

    .metis-net-active {{
        color: {accent};
    }}

    .metis-bar-vpn-active {{
        color: {accent};
    }}

    .metis-bar-vpn {{
        padding: 0 10px;
        min-width: 28px;
    }}

    .metis-net-status {{
        padding: 6px 4px;
        color: {muted};
        font-size: 12px;
    }}

    .metis-net-refresh {{
        padding: 4px;
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

    .metis-net-refresh:hover {{
        color: {accent};
        background-color: rgba({accent_rgb}, 0.12);
    }}

    .metis-net-connect {{
        padding: 8px 4px 2px 4px;
        border-top: 1px solid {border};
    }}

    .metis-net-connect-title {{
        color: {text};
        font-weight: 600;
    }}

    .metis-net-password {{
        border-radius: {rs}px;
    }}

    .metis-net-connect-btn {{
        background-color: {accent};
        background-image: none;
        color: {on_accent};
        font-weight: 600;
        border-radius: {rs}px;
        padding: 4px 12px;
        border: 1px solid {accent};
        box-shadow: none;
    }}

    .metis-net-cancel {{
        border-radius: {rs}px;
        padding: 4px 12px;
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        box-shadow: none;
    }}

    .metis-net-cancel:hover {{
        background-color: {raised};
    }}

    .metis-bt-device-list {{
        padding: 2px 0 4px 0;
    }}

    .metis-bt-device-row {{
        padding: 6px 4px;
        border-radius: {rs}px;
        color: {text};
    }}

    .metis-bt-device-row:hover {{
        background-color: rgba({accent_rgb}, 0.10);
    }}

    .metis-bt-battery-icon {{
        color: {muted};
    }}

    .metis-bt-battery-label {{
        color: {muted};
        font-size: 12px;
        font-feature-settings: "tnum";
    }}

    .metis-bt-battery-low {{
        color: {c_warning};
    }}

    .metis-bt-device-row:hover .metis-bt-battery-low {{
        color: {c_warning};
    }}

    .metis-bar-weather {{
        padding: 0 8px;
    }}

    .metis-weather-bar-label {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}

    .metis-weather-primary {{
        padding: 2px 2px 6px 2px;
    }}

    .metis-weather-loc {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}

    .metis-weather-temp {{
        font-size: 34px;
        font-weight: 300;
        color: {text};
    }}

    .metis-weather-cond {{
        font-size: 13px;
        color: {text};
    }}

    .metis-weather-hl {{
        font-size: 12px;
        color: {muted};
    }}

    .metis-weather-hourly {{
        padding: 6px 0;
        border-top: 1px solid {border};
        border-bottom: 1px solid {border};
    }}

    .metis-weather-hour {{
        padding: 2px 0;
    }}

    .metis-weather-hour-label {{
        font-size: 11px;
        color: {muted};
    }}

    .metis-weather-hour-temp {{
        font-size: 12px;
        font-weight: 600;
        color: {text};
    }}

    .metis-weather-sep {{
        background-color: {border};
        min-height: 1px;
    }}

    .metis-weather-other {{
        padding: 4px 2px;
    }}

    .metis-weather-other-temp {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}

    .metis-weather-status {{
        font-size: 12px;
        color: {muted};
    }}

    .metis-weather-attrib {{
        font-size: 10px;
        color: {muted};
        opacity: 0.7;
    }}

    .metis-bar-dropdown-panel switch {{
        background-color: rgba({text_rgb}, 0.14);
        border: none;
        border-radius: 999px;
        min-width: 40px;
        min-height: 22px;
    }}

    .metis-bar-dropdown-panel switch:checked {{
        background-color: {accent};
        background-image: linear-gradient(135deg, {accent}, {accent2});
    }}

    .metis-bar-dropdown-panel switch > slider {{
        background-color: {surface_solid};
        border-radius: 999px;
        min-width: 18px;
        min-height: 18px;
        box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
    }}

    .metis-bar-dropdown-panel separator {{
        background-color: {border};
        min-height: 1px;
    }}

    .metis-bar-popover-panel {{
        background-color: transparent;
        border: none;
        border-radius: 0;
        box-shadow: none;
    }}

    .metis-bar-calendar {{
        margin: 0;
    }}

    .metis-cal-today-legacy {{
        background-color: rgba({accent_rgb}, 0.85);
        color: {on_accent};
        font-weight: 700;
    }}

    .metis-bar-section-title {{
        font-size: 11px;
        font-weight: 700;
        color: {accent};
        letter-spacing: 0.04em;
        text-transform: uppercase;
    }}

    .metis-bar-tz-name {{
        font-size: 12px;
        color: {muted};
    }}

    .metis-bar-tz-time {{
        font-size: 13px;
        font-weight: 600;
        color: {text};
    }}

"#,
    )
}
