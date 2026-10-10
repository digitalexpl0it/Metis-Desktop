//! Shell stylesheet fragment: `bar`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(r#"
    .metis-bar-outer {{
        background-color: transparent;
        /* Same ~200ms ease-out feel as Metis titlebar reveal. Pixel translate
           values are injected per-bar (GTK CSS rejects calc() in transform). */
        transition: transform 200ms cubic-bezier(0.33, 1, 0.68, 1);
    }}

    .metis-bar-window {{
        background-color: transparent;
        overflow: hidden;
    }}

    .metis-bar-pill {{
        background-color: {surface};
        padding: 0 14px;
        color: {text};
    }}

    .metis-bar-full {{
        border-radius: 999px;
        padding: 0 20px;
    }}

    /* Shadow always faces the desktop (inner side) so it never needs room
       between the bar and the screen edge — distance maps 1:1 to margin_top. */
    .metis-bar-full.metis-bar-edge-bottom {{
        box-shadow: 0 -3px 10px rgba(0, 0, 0, 0.42), 0 -1px 3px rgba(0, 0, 0, 0.30);
    }}
    .metis-bar-full.metis-bar-edge-top {{
        box-shadow: 0 3px 10px rgba(0, 0, 0, 0.42), 0 1px 3px rgba(0, 0, 0, 0.30);
    }}
    .metis-bar-full.metis-bar-edge-left {{
        box-shadow: 3px 0 10px rgba(0, 0, 0, 0.42), 1px 0 3px rgba(0, 0, 0, 0.30);
    }}
    .metis-bar-full.metis-bar-edge-right {{
        box-shadow: -3px 0 10px rgba(0, 0, 0, 0.42), -1px 0 3px rgba(0, 0, 0, 0.30);
    }}

    .metis-bar-floating {{
        border-radius: 999px;
        padding: 0 14px;
    }}
    .metis-bar-floating.metis-bar-edge-bottom {{
        box-shadow: 0 -3px 10px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255, 255, 255, 0.06) inset;
    }}
    .metis-bar-floating.metis-bar-edge-top {{
        box-shadow: 0 3px 10px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255, 255, 255, 0.06) inset;
    }}
    .metis-bar-floating.metis-bar-edge-left {{
        box-shadow: 3px 0 10px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255, 255, 255, 0.06) inset;
    }}
    .metis-bar-floating.metis-bar-edge-right {{
        box-shadow: -3px 0 10px rgba(0, 0, 0, 0.45), 0 1px 0 rgba(255, 255, 255, 0.06) inset;
    }}

    /* Bar widget buttons share one geometry across every interaction state so
       the icon never shifts on hover or press; only decoration changes. */
    .metis-bar-widget,
    button.metis-bar-widget,
    button.metis-bar-widget:hover,
    button.metis-bar-widget:active,
    button.metis-bar-widget:checked,
    button.metis-bar-widget:focus,
    menubutton.metis-bar-widget,
    menubutton.metis-bar-widget > button,
    menubutton.metis-bar-widget:hover > button {{
        padding: 0 8px;
        margin: 0;
        min-height: 0;
        border: none;
        outline: none;
        border-radius: {rs}px;
    }}

    .metis-bar-widget,
    button.metis-bar-widget,
    menubutton.metis-bar-widget,
    menubutton.metis-bar-widget > button {{
        background-image: none;
        background-color: transparent;
        box-shadow: none;
        color: {text};
    }}

    /* Hover: cyan gradient rising from the bottom into the grey highlight, with
       a thin 1px cyan line under the icon box (inset shadow adds no layout). */
    button.metis-bar-widget:hover,
    menubutton.metis-bar-widget:hover > button {{
        background-image: linear-gradient(to top,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset 0 -1px 0 0 rgba({accent_rgb}, 0.95);
        border-radius: {rs}px {rs}px 0 0;
    }}

    /* Match the hover style exactly so the open icon looks identical whether or
       not the pointer is over it. Specificity is raised to beat `:hover`. */
    button.metis-bar-dropdown-active,
    button.metis-bar-widget.metis-bar-dropdown-active,
    button.metis-bar-widget.metis-bar-dropdown-active:hover,
    menubutton.metis-bar-widget.metis-bar-dropdown-active > button,
    menubutton.metis-bar-widget.metis-bar-dropdown-active:hover > button {{
        background-image: linear-gradient(to top,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset 0 -1px 0 0 rgba({accent_rgb}, 0.95);
        border-radius: {rs}px {rs}px 0 0;
    }}

    /* The clock MenuButton uses a custom time/date child; never reserve space for
       the default dropdown arrow. */
    menubutton.metis-bar-widget > button > .arrow {{
        min-width: 0;
        min-height: 0;
        -gtk-icon-size: 0;
        margin: 0;
        padding: 0;
    }}

    .metis-bar-launcher {{
        padding: 0 6px;
        margin-right: 2px;
    }}

    .metis-bar-launcher-icon {{
        -gtk-icon-style: regular;
        {launcher_icon_shadow}
    }}

    .metis-bar-sys-icon {{
        padding: 0 5px;
        border: none;
        border-radius: {rs}px;
        min-height: 0;
        background-color: transparent;
    }}

    .metis-bar-notifications {{
        padding: 0 4px;
        background-color: transparent;
    }}

    .metis-bar-notifications:hover {{
        background-color: transparent;
    }}

    .metis-bar-notif-overlay {{
        background-color: transparent;
        min-width: 18px;
        min-height: 18px;
    }}

    .metis-bar-notif-badge {{
        font-size: 8px;
        font-weight: 700;
        color: {on_accent};
        background-color: {accent};
        border-radius: 999px;
        min-width: 12px;
        min-height: 12px;
        padding: 0 3px;
        border: 1px solid rgba(0, 0, 0, 0.35);
        box-shadow: none;
    }}

    .metis-bar-dropdown-revealer {{
        background-color: transparent;
    }}

    .metis-bar-dropdown-shell {{
        background-color: transparent;
    }}

    /* Chrome (fill / border / soft shadow) lives on
       `popover.metis-bar-popover contents` so the blur has room outside the
       opaque card. Keep this panel transparent — only content padding. */
    .metis-bar-dropdown-panel {{
        background-color: transparent;
        border: none;
        border-radius: 0;
        padding: 14px 16px;
        color: {text};
        box-shadow: none;
    }}

    /* Popover form controls — drive from Metis theme tokens so entries and
       buttons stay light in light mode (GTK Adwaita defaults are not enough
       inside layer-shell popovers). */
    .metis-bar-dropdown-panel entry,
    .metis-bar-dropdown-panel searchentry,
    .metis-bar-dropdown-panel spinbutton,
    .metis-bar-dropdown-panel .metis-clipboard-search,
    .metis-bar-dropdown-panel .metis-menu-search,
    .metis-bar-dropdown-panel .metis-net-password {{
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        box-shadow: none;
        caret-color: {text};
    }}
    .metis-bar-dropdown-panel entry text,
    .metis-bar-dropdown-panel searchentry text,
    .metis-bar-dropdown-panel spinbutton text {{
        background-color: transparent;
        color: {text};
    }}
    .metis-bar-dropdown-panel entry text placeholder,
    .metis-bar-dropdown-panel searchentry text placeholder,
    .metis-bar-dropdown-panel entry > text > placeholder,
    .metis-bar-dropdown-panel searchentry > text > placeholder {{
        color: {muted};
        opacity: 1;
    }}
    .metis-bar-dropdown-panel entry:focus-within,
    .metis-bar-dropdown-panel searchentry:focus-within,
    .metis-bar-dropdown-panel spinbutton:focus-within,
    .metis-bar-dropdown-panel .metis-menu-search:focus-within,
    .metis-bar-dropdown-panel .metis-clipboard-search:focus-within {{
        border-color: {accent};
    }}
    .metis-bar-dropdown-panel entry image,
    .metis-bar-dropdown-panel searchentry image {{
        color: {muted};
    }}

    /* Footer / settings links that sit directly on the panel root */
    .metis-bar-dropdown-panel > button {{
        background-color: {surface_solid};
        background-image: none;
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        box-shadow: none;
        padding: 6px 12px;
    }}
    .metis-bar-dropdown-panel > button:hover {{
        background-color: {raised};
    }}
    .metis-bar-dropdown-panel > button label {{
        color: {text};
    }}

    .metis-bar-clock {{
        margin-left: 4px;
        padding: 0 8px 0 4px;
        min-height: 0;
    }}

    .metis-bar-clock-bell-wrap {{
        background-color: transparent;
        margin-left: 2px;
    }}

    .metis-bar-clock-bell {{
        opacity: 0.92;
    }}

    .metis-bar-clock-compact {{
        margin-left: 0;
        padding: 0 4px;
    }}

    .metis-bar-clock-icon {{
        opacity: 0.95;
    }}

    .metis-bar-weather-compact {{
        padding: 0 4px;
    }}

    .metis-bar-clock-time {{
        font-size: 13px;
        font-weight: 600;
        letter-spacing: 0.02em;
        color: {text};
    }}

    .metis-bar-clock-date {{
        font-size: 11px;
        color: {muted};
    }}

    .metis-bar-pill-vertical {{
        border-radius: {rm}px;
        /* Cross-axis padding must stay minimal or the strip blows out wider than
           the horizontal bar's height. Overrides .metis-bar-pill / .metis-bar-full. */
        padding: 10px 0;
    }}

    .metis-bar-pill-vertical.metis-bar-full {{
        padding: 12px 0;
    }}

    .metis-bar-outer-vertical {{
        min-width: 0;
    }}

    /* Vertical strip: center icons, tight horizontal insets, inner-edge hover. */
    .metis-bar-pill-vertical .metis-bar-widget,
    .metis-bar-pill-vertical button.metis-bar-widget,
    .metis-bar-pill-vertical menubutton.metis-bar-widget,
    .metis-bar-pill-vertical menubutton.metis-bar-widget > button {{
        padding: 4px 0;
    }}

    .metis-bar-pill-vertical .metis-bar-launcher {{
        margin-right: 0;
        padding: 4px 0;
    }}

    .metis-bar-pill-vertical .metis-bar-workspaces {{
        padding: 2px 0;
    }}

    .metis-bar-pill-vertical .metis-bar-sys-icon {{
        padding: 4px 0;
    }}

    .metis-bar-pill-vertical button.metis-bar-widget:hover,
    .metis-bar-pill-vertical menubutton.metis-bar-widget:hover > button {{
        background-image: linear-gradient(to right,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset -1px 0 0 0 rgba({accent_rgb}, 0.95);
        border-radius: {rs}px 0 0 {rs}px;
    }}

    .metis-bar-pill-vertical button.metis-bar-widget.metis-bar-dropdown-active,
    .metis-bar-pill-vertical button.metis-bar-widget.metis-bar-dropdown-active:hover,
    .metis-bar-pill-vertical menubutton.metis-bar-widget.metis-bar-dropdown-active > button,
    .metis-bar-pill-vertical menubutton.metis-bar-widget.metis-bar-dropdown-active:hover > button {{
        background-image: linear-gradient(to right,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset -1px 0 0 0 rgba({accent_rgb}, 0.95);
        border-radius: {rs}px 0 0 {rs}px;
    }}

    /* Right-edge bar: mirror hover onto the screen-inner side. */
    .metis-bar-pill-vertical-right button.metis-bar-widget:hover,
    .metis-bar-pill-vertical-right menubutton.metis-bar-widget:hover > button {{
        background-image: linear-gradient(to left,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset 1px 0 0 0 rgba({accent_rgb}, 0.95);
        border-radius: 0 {rs}px {rs}px 0;
    }}

    .metis-bar-pill-vertical-right button.metis-bar-widget.metis-bar-dropdown-active,
    .metis-bar-pill-vertical-right button.metis-bar-widget.metis-bar-dropdown-active:hover,
    .metis-bar-pill-vertical-right menubutton.metis-bar-widget.metis-bar-dropdown-active > button,
    .metis-bar-pill-vertical-right menubutton.metis-bar-widget.metis-bar-dropdown-active:hover > button {{
        background-image: linear-gradient(to left,
            rgba({accent_rgb}, 0.34) 0%,
            rgba({accent2_rgb}, 0.12) 45%,
            rgba(255, 255, 255, 0.06) 100%);
        box-shadow: inset 1px 0 0 0 rgba({accent_rgb}, 0.95);
        border-radius: 0 {rs}px {rs}px 0;
    }}

    .metis-bar-workspaces {{
        padding: 2px 6px;
    }}

    button.metis-bar-control-center-btn {{
        padding: 4px 5px;
        margin-inline-start: 2px;
        min-width: 0;
        min-height: 0;
        border-radius: 6px;
        background-color: transparent;
        border: none;
        box-shadow: none;
    }}

    button.metis-bar-control-center-btn:hover {{
        background-color: rgba({text_rgb}, 0.10);
    }}

    button.metis-bar-control-center-btn:active {{
        background-color: rgba({text_rgb}, 0.16);
    }}

    button.metis-bar-control-center-btn image {{
        opacity: 0.88;
    }}

    .metis-bar-ws-dot {{
        min-width: 7px;
        min-height: 7px;
        padding: 0;
        margin: 0;
        border-radius: 999px;
        background-color: transparent;
        border: 1.5px solid rgba({text_rgb}, 0.55);
    }}

    /* Taskbar / running-apps dock. The ScrolledWindow hosts a horizontal row of
       app buttons; its scrollbar gutter is hidden so overflow scrolls without a
       visible track stealing bar height. */
    .metis-bar-tasks {{
        background-color: transparent;
        min-height: 0;
    }}

    .metis-bar-tasks scrollbar,
    .metis-bar-tasks scrollbar.horizontal {{
        min-height: 0;
        margin: 0;
        padding: 0;
        opacity: 0;
    }}

    .metis-bar-tasks-vertical scrollbar.vertical {{
        min-width: 0;
        margin: 0;
        padding: 0;
        opacity: 0;
    }}

    .metis-bar-tasks-row {{
        background-color: transparent;
        padding: 0 2px;
    }}

    .metis-bar-tasks-row-vertical {{
        padding: 2px 0;
    }}

    /* Each app entry reuses the shared bar-widget geometry; the indicator dot and
       focus highlight are layered on top via state classes. */
    .metis-bar-task {{
        padding: 0 6px;
        background-color: transparent;
    }}

    .metis-bar-pill-vertical .metis-bar-task {{
        padding: 6px 0;
    }}

    /* Running-app underline dot, centered under the icon. */
    .metis-bar-task-dot {{
        min-width: 5px;
        min-height: 5px;
        margin-bottom: 1px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.45);
    }}

    .metis-bar-task.running .metis-bar-task-dot {{
        background-color: rgba({accent_rgb}, 0.9);
    }}

    /* The focused app gets a wider, brighter accent pill under the icon. */
    .metis-bar-task.focused .metis-bar-task-dot {{
        min-width: 12px;
        background-color: {accent};
    }}

    .metis-bar-task.focused {{
        background-image: linear-gradient(to top,
            rgba({accent_rgb}, 0.28) 0%,
            rgba({accent2_rgb}, 0.10) 45%,
            rgba(255, 255, 255, 0.05) 100%);
        border-radius: {rs}px {rs}px 0 0;
    }}

    .metis-bar-pill-vertical .metis-bar-task.focused {{
        background-image: linear-gradient(to right,
            rgba({accent_rgb}, 0.28) 0%,
            rgba({accent2_rgb}, 0.10) 45%,
            rgba(255, 255, 255, 0.05) 100%);
        border-radius: {rs}px 0 0 {rs}px;
    }}

    .metis-bar-pill-vertical-right .metis-bar-task.focused {{
        background-image: linear-gradient(to left,
            rgba({accent_rgb}, 0.28) 0%,
            rgba({accent2_rgb}, 0.10) 45%,
            rgba(255, 255, 255, 0.05) 100%);
        border-radius: 0 {rs}px {rs}px 0;
    }}

    /* A fully-minimized app reads as dimmed until restored. */
    .metis-bar-task.minimized {{
        opacity: 0.55;
    }}

    /* Pulse while a launch is pending (single-flight gate) so users do not
       double-click slow Electron/Flatpak starts. */
    @keyframes metis-bar-task-starting-pulse {{
        0%, 100% {{ opacity: 1.0; }}
        50% {{ opacity: 0.45; }}
    }}

    .metis-bar-task.metis-bar-task-starting image {{
        animation: metis-bar-task-starting-pulse 1.1s ease-in-out infinite;
    }}

    .metis-bar-task.metis-bar-task-starting .metis-bar-task-dot {{
        background-color: rgba({accent_rgb}, 0.75);
        min-width: 8px;
    }}

    /* Flat list rows (Folders-style). Must beat `.metis-bar-dropdown-panel >
       button` chip chrome — task/volumes menus put rows as direct children. */
    .metis-bar-dropdown-panel > button.metis-bar-task-menu-item,
    .metis-bar-dropdown-panel > button.metis-bar-volumes-menu-item,
    button.metis-bar-task-menu-item,
    button.metis-bar-volumes-menu-item,
    .metis-bar-task-pick,
    .metis-bar-tray-menu-item {{
        background-image: none;
        background-color: transparent;
        border: none;
        box-shadow: none;
        border-radius: {rs}px;
        padding: 6px 10px;
        color: {text};
    }}

    .metis-bar-dropdown-panel > button.metis-bar-task-menu-item:hover,
    .metis-bar-dropdown-panel > button.metis-bar-volumes-menu-item:hover,
    button.metis-bar-task-menu-item:hover,
    button.metis-bar-volumes-menu-item:hover,
    .metis-bar-task-pick:hover,
    .metis-bar-tray-menu-item:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}

    .metis-bar-tray-pinned {{
        margin-right: 2px;
    }}

    .metis-bar-tray-item {{
        padding: 2px;
        border-radius: {rs}px;
        min-width: 0;
        min-height: 0;
    }}

    .metis-bar-tray-item image.metis-bar-tray-icon {{
        color: {text};
        -gtk-icon-style: symbolic;
    }}

    .metis-bar-tray-item image.metis-bar-tray-pixmap {{
        {tray_pixmap_filter}
    }}

    .metis-bar-volumes {{
        spacing: 2px;
    }}
    button.metis-bar-volume-item,
    button.metis-bar-volume-item:hover,
    button.metis-bar-volume-item:active,
    button.metis-bar-volume-item:checked,
    button.metis-bar-volume-item:focus,
    button.metis-bar-volume-item.metis-bar-dropdown-active,
    button.metis-bar-volume-item.metis-bar-dropdown-active:hover {{
        /* Fixed geometry so opening the context popover (:checked) never
           grows the icon or expands the edge-bar pill. */
        min-width: 28px;
        max-width: 28px;
        min-height: 28px;
        max-height: 28px;
        padding: 2px;
        margin: 0;
    }}
    button.metis-bar-volume-item image {{
        -gtk-icon-size: 18px;
    }}
    .metis-bar-volumes-menu-panel {{
        min-width: 140px;
        max-width: 240px;
    }}
    /* Volume/updates context menus — tighter padding; chrome is on popover contents. */
    popover.metis-bar-volumes-menu .metis-bar-dropdown-panel {{
        background-color: transparent;
        border: none;
        border-radius: 0;
        padding: 10px 12px;
        color: {text};
        box-shadow: none;
    }}
    .metis-bar-volumes-menu-title {{
        max-width: 220px;
    }}

"#)
}
