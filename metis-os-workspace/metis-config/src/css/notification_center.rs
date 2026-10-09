//! Shell stylesheet fragment: `notification_center`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    /* ---- Notification Center (Phase 13) ---- */
    window.metis-nc-window {{
        background-color: transparent;
    }}
    .metis-nc-revealer {{
        background: transparent;
    }}
    .metis-nc-panel {{
        background-color: {nc_panel_bg};
        box-shadow: {dash_shadow};
        color: {text};
        padding: 8px 10px 10px 10px;
        border-radius: 0;
    }}
    /* Right-edge panel (default / bar on left, top, or bottom). */
    .metis-nc-panel.metis-nc-side-right {{
        border-left: 1px solid {border};
        border-right: none;
    }}
    .metis-nc-panel.metis-nc-side-right.metis-nc-attach-top {{
        border-top-left-radius: 0;
        border-bottom-left-radius: {rl}px;
    }}
    .metis-nc-panel.metis-nc-side-right.metis-nc-attach-bottom {{
        border-top-left-radius: {rl}px;
        border-bottom-left-radius: 0;
    }}
    .metis-nc-panel.metis-nc-side-right.metis-nc-attach-full {{
        border-top-left-radius: {rl}px;
        border-bottom-left-radius: {rl}px;
    }}
    /* Left-edge panel (bar on the right). */
    .metis-nc-panel.metis-nc-side-left {{
        border-right: 1px solid {border};
        border-left: none;
    }}
    .metis-nc-panel.metis-nc-side-left.metis-nc-attach-top {{
        border-top-right-radius: 0;
        border-bottom-right-radius: {rl}px;
    }}
    .metis-nc-panel.metis-nc-side-left.metis-nc-attach-bottom {{
        border-top-right-radius: {rl}px;
        border-bottom-right-radius: 0;
    }}
    .metis-nc-panel.metis-nc-side-left.metis-nc-attach-full {{
        border-top-right-radius: {rl}px;
        border-bottom-right-radius: {rl}px;
    }}
    .metis-nc-scrolled {{
        background: transparent;
    }}
    .metis-nc-scrolled scrollbar.vertical slider {{
        background-color: rgba({text_rgb}, 0.25);
        border-radius: 999px;
        min-width: 6px;
    }}
    .metis-nc-card {{
        background-color: {nc_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 12px;
        color: {text};
    }}
    /* Notifications: solid header only; list sits on the panel glass. */
    .metis-nc-notif-section {{
        background-color: transparent;
        background-image: none;
        border: none;
        padding: 0;
        box-shadow: none;
    }}
    .metis-nc-notif-section .metis-nc-notif-header {{
        background-color: {nc_card_bg};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 12px;
    }}
    .metis-nc-notif-section .metis-notif-scrolled,
    .metis-nc-notif-section .metis-nc-scrolled,
    .metis-nc-notif-section revealer,
    .metis-nc-notif-section revealer > * {{
        background-color: transparent;
        background-image: none;
    }}
    label.metis-nc-card-title {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
    }}
    button.metis-nc-section-btn {{
        background: rgba({text_rgb}, 0.06);
        background-image: none;
        border: 1px solid {border};
        border-radius: {rs}px;
        padding: 8px 12px;
        font-size: 14px;
        font-weight: 600;
        color: {text};
        box-shadow: none;
        outline: none;
    }}
    button.metis-nc-section-btn:hover {{
        background: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    button.metis-nc-section-btn:checked {{
        background: rgba({accent_rgb}, 0.18);
        border-color: rgba({accent_rgb}, 0.45);
        color: {text};
    }}
    .metis-nc-create-revealer {{
        margin: 0;
    }}
    .metis-nc-create-form {{
        background-color: {nc_card_bg};
        border-radius: {rm}px;
        border: 1px solid {border};
        padding: 12px 14px;
        margin-bottom: 2px;
        box-shadow: {dash_shadow};
    }}
    .metis-nc-tool-rail {{
        background: transparent;
        padding-top: 4px;
        border-top: 1px solid rgba({text_rgb}, 0.10);
        margin-top: 2px;
    }}
    button.metis-nc-tool-btn {{
        background: transparent;
        background-image: none;
        border: 1px solid transparent;
        border-radius: {rs}px;
        padding: 8px;
        color: {muted};
        box-shadow: none;
        outline: none;
        min-width: 36px;
        min-height: 36px;
    }}
    button.metis-nc-tool-btn:hover {{
        background-color: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    button.metis-nc-tool-btn:checked {{
        background-color: {accent};
        color: {text_on_accent};
        border-color: {accent};
    }}

    /* Force every button inside the NC to follow Metis tokens. Adwaita's
       prefer-dark chrome otherwise keeps charcoal fills on light panels. */
    .metis-nc-panel button,
    .metis-nc-card button {{
        background: {raised};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        box-shadow: none;
        outline: none;
    }}
    .metis-nc-panel button.metis-cal-nav,
    .metis-nc-panel button.metis-cal-day,
    .metis-nc-panel button.metis-cal-event-action,
    .metis-nc-panel button.metis-cal-today-btn,
    .metis-nc-panel button.metis-nc-tool-btn,
    .metis-nc-panel button.metis-notif-expand,
    .metis-nc-panel button.metis-notif-dismiss,
    .metis-nc-card button.metis-cal-nav,
    .metis-nc-card button.metis-cal-day,
    .metis-nc-card button.metis-cal-event-action,
    .metis-nc-card button.metis-cal-today-btn,
    .metis-nc-card button.metis-nc-tool-btn,
    .metis-nc-card button.metis-notif-expand,
    .metis-nc-card button.metis-notif-dismiss {{
        background: transparent;
        border-color: transparent;
        color: {muted};
    }}
    .metis-nc-panel button.metis-nc-section-btn,
    .metis-nc-card button.metis-nc-section-btn {{
        background: rgba({text_rgb}, 0.06);
        border: 1px solid {border};
        color: {text};
    }}
    .metis-nc-panel button.metis-nc-section-btn:checked,
    .metis-nc-card button.metis-nc-section-btn:checked {{
        background: rgba({accent_rgb}, 0.18);
        border-color: rgba({accent_rgb}, 0.45);
        color: {text};
    }}
    .metis-nc-panel button.metis-cal-today-btn,
    .metis-nc-card button.metis-cal-today-btn {{
        color: {accent};
    }}
    .metis-nc-panel button.metis-cal-day,
    .metis-nc-card button.metis-cal-day {{
        color: {text};
    }}
    .metis-nc-panel button.metis-cal-add-btn,
    .metis-nc-card button.metis-cal-add-btn {{
        background: rgba({accent_rgb}, 0.14);
        border-color: transparent;
        color: {text};
    }}
    .metis-nc-panel button.metis-sw-btn-go,
    .metis-nc-card button.metis-sw-btn-go {{
        background: {accent};
        border-color: {accent};
        color: {text_on_accent};
    }}
    .metis-nc-panel button.metis-sw-btn-stop,
    .metis-nc-card button.metis-sw-btn-stop {{
        background: rgba({text_rgb}, 0.08);
        border-color: transparent;
        color: {text};
    }}
    .metis-nc-panel button.metis-alarm-day:checked,
    .metis-nc-panel button.metis-alarm-sound-btn:checked,
    .metis-nc-panel button.metis-nc-tool-btn:checked,
    .metis-nc-card button.metis-alarm-day:checked,
    .metis-nc-card button.metis-alarm-sound-btn:checked,
    .metis-nc-card button.metis-nc-tool-btn:checked {{
        background: {accent};
        border-color: {accent};
        color: {text_on_accent};
    }}
    .metis-nc-panel button:hover,
    .metis-nc-card button:hover {{
        background: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    .metis-nc-panel button.metis-cal-selected,
    .metis-nc-card button.metis-cal-selected {{
        background: rgba({accent_rgb}, 0.10);
        box-shadow: inset 0 0 0 1px {accent};
        color: {text};
    }}
    .metis-nc-panel button.metis-sw-btn-go:hover,
    .metis-nc-card button.metis-sw-btn-go:hover {{
        background: {accent2};
        color: {text_on_accent};
    }}
    .metis-nc-panel button.metis-nc-tool-btn:checked:hover,
    .metis-nc-card button.metis-nc-tool-btn:checked:hover,
    .metis-nc-panel button.metis-alarm-day:checked:hover,
    .metis-nc-card button.metis-alarm-day:checked:hover {{
        background: {accent};
        color: {text_on_accent};
    }}
    .metis-nc-panel button image,
    .metis-nc-card button image {{
        color: inherit;
    }}
    .metis-nc-panel .metis-cal-event {{
        background-color: rgba({text_rgb}, 0.04);
        border: 1px solid {border};
        border-radius: {rs}px;
        padding: 8px 6px;
    }}
    .metis-nc-panel entry,
    .metis-nc-panel spinbutton,
    .metis-nc-card entry,
    .metis-nc-card spinbutton {{
        background-color: {raised};
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        caret-color: {text};
    }}
    .metis-nc-panel switch,
    .metis-nc-card switch,
    switch.metis-nc-switch {{
        background-color: rgba({text_rgb}, 0.18);
        border: none;
        border-radius: 999px;
        min-width: 36px;
        min-height: 18px;
        padding: 0;
    }}
    .metis-nc-panel switch:checked,
    .metis-nc-card switch:checked,
    switch.metis-nc-switch:checked {{
        background-color: {accent};
    }}
    .metis-nc-panel switch > slider,
    .metis-nc-card switch > slider,
    switch.metis-nc-switch > slider {{
        background-color: {surface_solid};
        border-radius: 999px;
        min-width: 14px;
        min-height: 14px;
        margin: 2px;
        box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
    }}
    button.metis-nc-btn {{
        background-image: none;
        background-color: transparent;
        border: 1px solid {border};
        border-radius: {rs}px;
        color: {text};
        padding: 4px 10px;
    }}
    button.metis-nc-btn:hover {{
        background-color: rgba({accent_rgb}, 0.12);
    }}
    button.metis-nc-collapse {{
        background: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        padding: 2px;
        min-width: 24px;
        min-height: 24px;
        color: {muted};
    }}
    button.metis-nc-collapse:hover {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.12);
        border-radius: {rs}px;
    }}
    button.metis-nc-collapse:checked {{
        color: {text};
    }}

    /* ---- Desktop widgets (Phase 14 wallpaper layer) ---- */
    window.metis-desktop-widgets-window {{
        background-color: transparent;
    }}
    /* Folders delete/rename confirms — Popovers (not toplevel windows).
       Toplevel dialogs inherit transparent `window` RGBA and stay hollow;
       Metis SSD then draws a ghost titlebar around them. */
"#,
    )
}
