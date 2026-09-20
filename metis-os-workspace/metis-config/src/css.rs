use crate::theme::ThemeTokens;

pub fn build_stylesheet(theme: &ThemeTokens) -> String {
    let accent = theme.accent_primary();
    // Bare `r, g, b` triplets let the stylesheet inline accent-tinted rgba() with
    // per-rule opacities, so hover/selection states track the theme accent.
    let accent_rgb = theme.accent_rgb();
    let accent2 = theme.accent_secondary();
    let accent2_rgb = theme.accent_secondary_rgb();
    let on_accent = theme.text_on_accent.clone();
    let text_rgb = theme.text_rgb();
    let surface_solid = theme.surface.clone();
    let surface = theme.surface_rgba();
    let raised = theme.surface_raised.clone();
    let shadow = theme.shadow_ambient.clone();
    // The gradient brand icon washes out against the pale light-mode bar, so add a
    // soft drop shadow under it to lift it off the surface. A dark shadow is
    // effectively invisible on the dark theme, so it's only emitted for light.
    let launcher_icon_shadow = if theme.mode.eq_ignore_ascii_case("light") {
        "-gtk-icon-shadow: 0 1px 3px rgba(0, 0, 0, 0.55);"
    } else {
        ""
    };
    let tray_pixmap_filter = if theme.mode.eq_ignore_ascii_case("light") {
        "filter: brightness(0); opacity: 0.88;"
    } else {
        ""
    };
    let rs = theme.radius_sm;
    let rm = theme.radius_md;
    let rl = theme.radius_lg;
    // Semantic status palette (notifications + state highlights).
    let c_error = theme.semantic.error.clone();
    let c_warning = theme.semantic.warning.clone();
    let c_success = theme.semantic.success.clone();
    let c_info = theme.semantic.info.clone();
    let c_payment = theme.semantic.payment.clone();
    let c_error_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.error);
    let c_warning_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.warning);
    let c_success_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.success);
    let c_info_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.info);
    let c_payment_rgb = crate::theme::rgb_triplet_from_hex(&theme.semantic.payment);

    // Control center: frosted panel over the desktop, slightly more opaque cards.
    let surface_rgb = theme.surface_rgb();
    let raised_rgb = theme.surface_raised_rgb();
    let is_light = theme.mode.eq_ignore_ascii_case("light");
    let dash_panel_bg = if is_light {
        format!("rgba({surface_rgb}, 0.72)")
    } else {
        format!("rgba({surface_rgb}, 0.82)")
    };
    let dash_card_bg = if is_light {
        format!("rgba({raised_rgb}, 0.94)")
    } else {
        format!("rgba({raised_rgb}, 0.90)")
    };
    // Splash / onboarding overlays: never hardcode dark charcoal — `{text}` /
    // `{muted}` follow the active theme, so a fixed dark card is unreadable in
    // light mode.
    let overlay_card_bg = if is_light {
        format!("rgba({raised_rgb}, 0.96)")
    } else {
        format!("rgba({surface_rgb}, 0.92)")
    };
    let overlay_dot = if is_light {
        format!("rgba({text_rgb}, 0.22)")
    } else {
        "rgba(255, 255, 255, 0.18)".to_string()
    };
    let dash_shadow = if is_light {
        format!("0 12px 40px {shadow}")
    } else {
        "0 12px 32px rgba(0, 0, 0, 0.42)".to_string()
    };
    let dash_shadow_up = if is_light {
        format!("0 -12px 40px {shadow}")
    } else {
        "0 -12px 32px rgba(0, 0, 0, 0.42)".to_string()
    };
    let screenshot_toolbar_bg = dash_panel_bg.clone();
    let nc_panel_bg = dash_panel_bg.clone();
    let nc_card_bg = dash_card_bg.clone();
    let toast_card_bg = dash_card_bg.clone();
    // Desktop widget fill/border come from per-card chrome (desktop-widgets.json),
    // not the theme stylesheet.
    // Notification rows inside the NC / legacy popover — follow raised surface,
    // never a hardcoded dark charcoal (that looked like dark-mode in light theme).
    let notif_card_bg = dash_card_bg.clone();
    let text_on_accent = theme.text_on_accent.clone();

    // Optional DE-wide font family/size; empty unless the user customized them.
    let font_decls = theme.font_declarations();

    format!(
        r#"
    window {{
        background-color: transparent;
        {font_decls}
    }}

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

    .metis-bar-dropdown-panel {{
        background-color: {raised};
        border: 1px solid {border};
        border-radius: {rl}px;
        padding: 14px 16px;
        color: {text};
        box-shadow: {shadow},
                    inset 0 1px 0 rgba({text_rgb}, 0.05);
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

    /* Window picker + context menu rows inside task popovers. */
    .metis-bar-task-pick,
    .metis-bar-task-menu-item,
    .metis-bar-tray-menu-item {{
        background-color: transparent;
        border-radius: {rs}px;
        padding: 6px 10px;
        color: {text};
    }}

    .metis-bar-task-pick:hover,
    .metis-bar-task-menu-item:hover,
    .metis-bar-tray-menu-item:hover {{
        background-color: rgba({accent_rgb}, 0.18);
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
    .metis-bar-volumes-menu-title {{
        max-width: 220px;
    }}
    .metis-bar-volumes-menu-item {{
        padding: 6px 10px;
        border-radius: {rs}px;
    }}
    .metis-bar-volumes-menu-item:hover {{
        background-color: rgba({text_rgb}, 0.08);
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

    .metis-notif-dnd-label {{
        font-size: 11px;
        color: {muted};
    }}

    popover.metis-bar-popover {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}

    popover.metis-bar-popover contents {{
        padding: 0;
        border: none;
        background-color: transparent;
    }}

    popover.metis-bar-popover > arrow {{
        background-color: {raised};
        border: 1px solid {border};
        min-width: 16px;
        min-height: 8px;
    }}

    popover.metis-notif-popover {{
        padding: 0;
    }}

    .metis-notif-scrolled {{
        min-width: 0;
    }}

    .metis-nc-scrolled {{
        background: transparent;
        min-width: 0;
    }}
    .metis-nc-scrolled scrollbar.vertical {{
        min-width: 8px;
        margin: 2px 0;
    }}
    .metis-nc-scrolled scrollbar.vertical slider {{
        background-color: rgba({text_rgb}, 0.25);
        border-radius: 999px;
        min-width: 6px;
    }}
    .metis-nc-scrolled scrollbar.vertical slider:hover {{
        background-color: rgba({text_rgb}, 0.4);
    }}
    .metis-cal-events-scroll {{
        min-height: 0;
    }}

    .metis-notif-scrolled scrollbar.vertical {{
        min-width: 8px;
        margin: 4px 2px;
    }}

    .metis-notif-scrolled scrollbar.vertical slider {{
        min-width: 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.18);
    }}

    .metis-notif-scrolled scrollbar.vertical slider:hover {{
        background-color: rgba({text_rgb}, 0.28);
    }}

    .metis-notif-empty {{
        font-size: 12px;
        color: {muted};
        padding: 24px 8px;
    }}

    .metis-notif-card {{
        background-color: {notif_card_bg};
        border-radius: {rm}px;
        border: 1px solid {border};
        padding: 12px 14px;
        color: {text};
    }}

    .metis-notif-icon {{
        -gtk-icon-size: 20px;
        margin-top: 1px;
        color: {text};
    }}

    .metis-notif-count {{
        min-width: 18px;
        padding: 1px 7px;
        border-radius: 999px;
        font-size: 11px;
        font-weight: 700;
        color: {text};
        background-color: rgba({text_rgb}, 0.12);
    }}

    .metis-notif-clear {{
        padding: 5px 14px;
        border-radius: 8px;
        font-size: 12px;
        font-weight: 600;
        color: {muted};
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        border: 1px solid {border};
        box-shadow: none;
    }}
    .metis-notif-clear:hover {{
        color: {text};
        background-color: rgba({text_rgb}, 0.10);
    }}
    .metis-notif-clear:disabled {{
        opacity: 0.45;
    }}

    .metis-notif-accent {{
        border-radius: 10px 0 0 10px;
        min-width: 5px;
    }}

    .metis-notif-diamond {{
        min-width: 40px;
        min-height: 40px;
        border-radius: 6px;
        border: 1.5px solid currentColor;
        transform: rotate(45deg);
    }}

    .metis-notif-diamond-icon {{
        font-size: 15px;
        font-weight: 700;
        transform: rotate(-45deg);
    }}

    .metis-notif-title {{
        font-size: 14px;
        font-weight: 700;
        color: {text};
    }}

    .metis-notif-message {{
        font-size: 12px;
        color: {muted};
        line-height: 1.35;
    }}

    .metis-notif-actions {{
        margin-top: 6px;
    }}

    .metis-notif-action {{
        padding: 5px 14px;
        border-radius: 8px;
        font-size: 12px;
        font-weight: 600;
        color: {text};
        background-color: rgba({text_rgb}, 0.06);
        background-image: none;
        border: 1px solid {border};
        box-shadow: none;
    }}
    .metis-notif-action:hover {{
        background-color: rgba({text_rgb}, 0.12);
    }}
    .metis-notif-action.suggested-action {{
        color: {text};
        border-color: rgba({accent_rgb}, 0.55);
        background-color: rgba({accent_rgb}, 0.22);
    }}
    .metis-notif-action.suggested-action:hover {{
        background-color: rgba({accent_rgb}, 0.34);
    }}

    .metis-notif-card-clickable:hover {{
        background-color: rgba({accent_rgb}, 0.08);
    }}

    /* ---- Toast banners (transient overlay, top-right) ---- */
    window.metis-toast-window {{
        background-color: transparent;
    }}
    .metis-toast-stack {{
        margin: 0;
    }}
    .metis-toast-card {{
        background-color: {toast_card_bg};
        border-radius: 12px;
        border: 1px solid {border};
        padding: 14px 16px;
        box-shadow: {dash_shadow};
        color: {text};
    }}
    button.metis-toast-close {{
        background: transparent;
        background-image: none;
        border: none;
        box-shadow: none;
        padding: 2px;
        min-width: 24px;
        min-height: 24px;
        color: {muted};
    }}
    button.metis-toast-close:hover {{
        color: {text};
        background-color: rgba({accent_rgb}, 0.12);
        border-radius: {rs}px;
    }}

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

    .metis-notif-card-error {{
        box-shadow: 0 0 18px rgba({c_error_rgb}, 0.18);
        border-color: rgba({c_error_rgb}, 0.40);
    }}

    .metis-notif-card-error .metis-notif-accent {{
        background-color: {c_error};
    }}

    .metis-notif-card-error .metis-notif-title,
    .metis-notif-card-error .metis-notif-icon {{
        color: {c_error};
    }}

    .metis-notif-card-notify {{
        box-shadow: 0 0 18px rgba({c_warning_rgb}, 0.18);
        border-color: rgba({c_warning_rgb}, 0.40);
    }}

    .metis-notif-card-notify .metis-notif-accent {{
        background-color: {c_warning};
    }}

    .metis-notif-card-notify .metis-notif-title,
    .metis-notif-card-notify .metis-notif-icon {{
        color: {c_warning};
    }}

    .metis-notif-card-success {{
        box-shadow: 0 0 18px rgba({c_success_rgb}, 0.18);
        border-color: rgba({c_success_rgb}, 0.40);
    }}

    .metis-notif-card-success .metis-notif-accent {{
        background-color: {c_success};
    }}

    .metis-notif-card-success .metis-notif-title,
    .metis-notif-card-success .metis-notif-icon {{
        color: {c_success};
    }}

    .metis-notif-card-info {{
        box-shadow: 0 0 18px rgba({c_info_rgb}, 0.18);
        border-color: rgba({c_info_rgb}, 0.40);
    }}

    .metis-notif-card-info .metis-notif-accent {{
        background-color: {c_info};
    }}

    .metis-notif-card-info .metis-notif-title,
    .metis-notif-card-info .metis-notif-icon {{
        color: {c_info};
    }}

    .metis-notif-card-payment {{
        box-shadow: 0 0 18px rgba({c_payment_rgb}, 0.18);
        border-color: rgba({c_payment_rgb}, 0.40);
    }}

    .metis-notif-card-payment .metis-notif-accent {{
        background-color: {c_payment};
    }}

    .metis-notif-card-payment .metis-notif-title,
    .metis-notif-card-payment .metis-notif-icon {{
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
        background-color: {surface};
        border: 1px solid {border};
        border-radius: {rm}px;
        box-shadow: {shadow};
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

    /* ---- Startup splash (centered overlay layer) ---- */
    window.metis-splash-window {{
        background-color: transparent;
    }}
    .metis-splash-card {{
        padding: 40px 56px 34px 56px;
        border-radius: 28px;
        background-color: {overlay_card_bg};
        border: 1px solid {border};
        box-shadow: {shadow},
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
        box-shadow: {shadow},
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
    .metis-style-fallback-light {{
        background-color: #f2f2f4;
    }}
    .metis-style-fallback-dark {{
        background-color: #1c1c20;
    }}
    .metis-style-mock-light {{
        background-color: #ffffff;
        border-radius: 7px;
        border-top: 9px solid #e6e6e9;
        box-shadow: 0 3px 8px rgba(0, 0, 0, 0.20);
    }}
    .metis-style-mock-dark {{
        background-color: #2b2b30;
        border-radius: 7px;
        border-top: 9px solid #3a3a40;
        box-shadow: 0 3px 8px rgba(0, 0, 0, 0.45);
    }}
    .metis-style-caption {{
        color: {text};
        font-weight: 600;
    }}
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
        box-shadow: {shadow};
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

    window.metis-screenshot-window {{
        background: transparent;
    }}
    .metis-screenshot-canvas {{
        background: transparent;
    }}
    .metis-screenshot-toolbar-wrap {{
        background: transparent;
    }}
    .metis-screenshot-toolbar {{
        background-color: {screenshot_toolbar_bg};
        border: 1px solid {border};
        border-radius: 999px;
        padding: 8px 12px;
        box-shadow: {dash_shadow};
    }}
    .metis-screenshot-mode {{
        background: transparent;
        border-radius: 999px;
        padding: 2px;
    }}
    button.metis-screenshot-mode-btn {{
        background-color: transparent;
        background-image: none;
        border: none;
        border-radius: 999px;
        min-width: 36px;
        min-height: 36px;
        padding: 0;
        color: {text};
        box-shadow: none;
        outline: none;
    }}
    button.metis-screenshot-mode-btn:hover {{
        background-color: rgba({accent_rgb}, 0.12);
        background-image: none;
    }}
    button.metis-screenshot-mode-btn:checked {{
        background-color: {accent};
        background-image: none;
        color: {text_on_accent};
        border-radius: 999px;
    }}
    button.metis-screenshot-mode-btn image {{
        color: {text};
        -gtk-icon-style: symbolic;
    }}
    button.metis-screenshot-mode-btn:checked image {{
        -gtk-icon-filter: none;
        color: {text_on_accent};
    }}
    .metis-screenshot-mode stackswitcher {{
        background: transparent;
        border-radius: 999px;
    }}
    .metis-screenshot-mode stackswitcher button {{
        background: transparent;
        border: none;
        border-radius: 999px;
        padding: 6px 14px;
        color: {text};
        box-shadow: none;
    }}
    .metis-screenshot-mode stackswitcher button:hover {{
        background-color: rgba({accent_rgb}, 0.12);
    }}
    .metis-screenshot-mode stackswitcher button:checked {{
        background-color: {accent};
        color: {text_on_accent};
    }}
    .metis-screenshot-mode stackswitcher button:checked label {{
        color: {text_on_accent};
    }}
    /* Options gear is a MenuButton — style the menubutton and its inner button
       (Adwaita leaves a fixed grey chip otherwise that ignores Metis theme). */
    menubutton.metis-screenshot-icon,
    menubutton.metis-screenshot-icon > button,
    button.metis-screenshot-icon {{
        background-color: transparent;
        background-image: none;
        border: none;
        border-radius: 999px;
        padding: 6px;
        color: {text};
        box-shadow: none;
        outline: none;
    }}
    menubutton.metis-screenshot-icon > button > .arrow {{
        min-width: 0;
        min-height: 0;
        padding: 0;
        margin: 0;
        opacity: 0;
    }}
    menubutton.metis-screenshot-icon:hover > button,
    menubutton.metis-screenshot-icon > button:hover,
    button.metis-screenshot-icon:hover {{
        background-color: rgba({accent_rgb}, 0.12);
        background-image: none;
    }}
    menubutton.metis-screenshot-icon:checked > button,
    menubutton.metis-screenshot-icon > button:checked,
    button.metis-screenshot-icon:checked {{
        background-color: rgba({accent_rgb}, 0.22);
        background-image: none;
        color: {accent};
    }}
    menubutton.metis-screenshot-icon image,
    menubutton.metis-screenshot-icon > button image,
    button.metis-screenshot-icon image {{
        color: {text};
        -gtk-icon-style: symbolic;
    }}
    menubutton.metis-screenshot-icon:checked image,
    menubutton.metis-screenshot-icon:checked > button image {{
        color: {accent};
    }}
    button.metis-screenshot-capture {{
        background-color: {accent};
        background-image: none;
        color: {text_on_accent};
        border: none;
        border-radius: 999px;
        padding: 10px 22px;
        font-weight: 600;
        box-shadow: none;
        outline: none;
    }}
    button.metis-screenshot-capture label {{
        color: {text_on_accent};
    }}
    button.metis-screenshot-capture:hover {{
        background-image: linear-gradient(
            180deg,
            rgba(255, 255, 255, 0.12),
            rgba(255, 255, 255, 0.0)
        );
        background-color: {accent};
    }}
    button.metis-screenshot-capture:active {{
        background-color: {accent};
        opacity: 0.88;
    }}
    label.metis-screenshot-size {{
        background-color: {raised};
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        padding: 4px 10px;
        font-size: 12px;
        font-weight: 600;
    }}
    popover.metis-screenshot-popover {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}
    popover.metis-screenshot-popover contents {{
        background-color: {surface_solid};
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 0;
        box-shadow: {dash_shadow};
        color: {text};
    }}
    popover.metis-screenshot-popover > arrow {{
        background-color: {surface_solid};
        border: 1px solid {border};
    }}
    .metis-screenshot-options {{
        background-color: transparent;
        color: {text};
    }}
    label.metis-screenshot-option-label {{
        font-size: 13px;
        color: {text};
    }}
    .metis-screenshot-after-seg {{
        border-radius: {rs}px;
    }}
    .metis-screenshot-after-seg button.metis-screenshot-after-btn,
    popover.metis-screenshot-popover button.metis-screenshot-after-btn {{
        background: {raised};
        background-image: none;
        border: 1px solid {border};
        color: {text};
        padding: 6px 10px;
        font-size: 12px;
        box-shadow: none;
        outline: none;
    }}
    .metis-screenshot-after-seg button.metis-screenshot-after-btn:hover,
    popover.metis-screenshot-popover button.metis-screenshot-after-btn:hover {{
        background: rgba({accent_rgb}, 0.12);
        color: {text};
    }}
    .metis-screenshot-after-seg button.metis-screenshot-after-btn:checked,
    popover.metis-screenshot-popover button.metis-screenshot-after-btn:checked {{
        background: {accent};
        color: {text_on_accent};
        border-color: {accent};
    }}
    popover.metis-screenshot-popover switch,
    .metis-screenshot-options switch {{
        background-color: rgba({text_rgb}, 0.14);
        background-image: none;
        border: none;
        border-radius: 999px;
        min-width: 40px;
        min-height: 22px;
        padding: 0;
    }}
    popover.metis-screenshot-popover switch:checked,
    .metis-screenshot-options switch:checked {{
        background-color: {accent};
        background-image: none;
    }}
    popover.metis-screenshot-popover switch > slider,
    .metis-screenshot-options switch > slider {{
        background-color: {surface_solid};
        border-radius: 999px;
        min-width: 18px;
        min-height: 18px;
        box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
    }}
    popover.metis-screenshot-popover spinbutton,
    .metis-screenshot-options spinbutton {{
        background-color: {raised};
        color: {text};
        border: 1px solid {border};
        border-radius: {rs}px;
        caret-color: {text};
        box-shadow: none;
    }}
    popover.metis-screenshot-popover spinbutton text,
    .metis-screenshot-options spinbutton text {{
        color: {text};
        background-color: transparent;
    }}
    popover.metis-screenshot-popover spinbutton button,
    .metis-screenshot-options spinbutton button {{
        background: transparent;
        background-image: none;
        color: {muted};
        border: none;
        box-shadow: none;
    }}
    popover.metis-screenshot-popover spinbutton button:hover,
    .metis-screenshot-options spinbutton button:hover {{
        background: rgba({accent_rgb}, 0.12);
        color: {text};
    }}

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
    label.metis-nc-card-title {{
        font-size: 14px;
        font-weight: 600;
        color: {text};
    }}
    .metis-nc-tool-rail {{
        background: transparent;
    }}
    button.metis-nc-tool-btn {{
        background: transparent;
        background-image: none;
        border: 1px solid transparent;
        border-radius: {rs}px;
        padding: 6px;
        color: {muted};
        box-shadow: none;
        outline: none;
        min-width: 32px;
        min-height: 32px;
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
    .metis-nc-card button.metis-cal-nav,
    .metis-nc-card button.metis-cal-day,
    .metis-nc-card button.metis-cal-event-action,
    .metis-nc-card button.metis-cal-today-btn,
    .metis-nc-card button.metis-nc-tool-btn {{
        background: transparent;
        border-color: transparent;
        color: {muted};
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
    popover.metis-dw-confirm {{
        background-color: transparent;
        padding: 0;
        border: none;
        box-shadow: none;
    }}
    popover.metis-dw-confirm contents {{
        background-color: {surface_solid} !important;
        border: 1px solid {border};
        border-radius: {rm}px;
        padding: 0;
        box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    }}
    .metis-dw-confirm-sheet {{
        background-color: {surface_solid};
        color: {text};
        border-radius: {rm}px;
    }}
    .metis-dw-confirm-title {{
        font-weight: 600;
        font-size: 1.05rem;
        color: {text};
    }}
    .metis-dw-confirm-detail {{
        color: {muted};
    }}
    .metis-desktop-widgets-canvas {{
        background-color: transparent;
    }}
    .metis-dw-card {{
        background-color: transparent;
        border: none;
        border-radius: {rm}px;
        color: {text};
        padding: 8px 10px 6px 10px;
    }}
    .metis-dw-card.metis-dw-edit {{
        /* Edit outline stays visible even when fill/border chrome is transparent. */
        outline: 1px dashed rgba({accent_rgb}, 0.65);
        outline-offset: -1px;
    }}
    .metis-dw-card.metis-dw-locked {{
        opacity: 0.92;
    }}
    .metis-dw-header {{
        margin-bottom: 4px;
        min-height: 28px;
    }}
    .metis-dw-header.metis-dw-header-slim {{
        min-height: 18px;
        margin-bottom: 2px;
        opacity: 0.85;
    }}
    .metis-dw-title {{
        font-weight: 600;
        font-size: 0.95rem;
        color: {text};
    }}
    .metis-dw-badge {{
        font-size: 0.72rem;
        color: {muted};
        padding: 1px 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.08);
    }}
    .metis-dw-body {{
        min-height: 40px;
    }}
    .metis-dw-hint {{
        color: {muted};
        font-size: 0.85rem;
        opacity: 0.9;
    }}
    .metis-dw-list {{
        padding: 0;
    }}
    button.metis-dw-row {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 4px 6px;
        color: {text};
    }}
    button.metis-dw-row:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-folder-grid {{
        padding: 2px;
    }}
    button.metis-dw-folder-tile {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 6px 4px;
        color: {text};
    }}
    button.metis-dw-folder-tile:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-folder-name {{
        font-size: 0.72rem;
        color: {text};
    }}
    button.metis-dw-menu-item {{
        background-image: none;
        background-color: transparent;
        border: none;
        border-radius: {rs}px;
        padding: 6px 10px;
        color: {text};
    }}
    button.metis-dw-menu-item:hover {{
        background-color: rgba({accent_rgb}, 0.14);
    }}
    .metis-dw-clock-time {{
        font-size: 2.4rem;
        font-weight: 600;
        color: {text};
    }}
    .metis-dw-clock-date {{
        font-size: 0.95rem;
        color: {muted};
    }}
    .metis-dw-metric-label {{
        font-weight: 600;
        color: {text};
        font-size: 0.85rem;
    }}
    progressbar.metis-dw-progress trough {{
        min-height: 6px;
        border-radius: 999px;
        background-color: rgba({text_rgb}, 0.12);
    }}
    progressbar.metis-dw-progress progress {{
        min-height: 6px;
        border-radius: 999px;
        background-color: {accent};
    }}
    .metis-dw-resize {{
        background-image: none;
        background-color: rgba({accent_rgb}, 0.22);
        border: 1px solid rgba({accent_rgb}, 0.50);
        border-radius: 4px;
        color: {text};
        padding: 0;
        min-width: 20px;
        min-height: 20px;
        font-size: 0.7rem;
    }}
    .metis-dw-resize:hover {{
        background-color: rgba({accent_rgb}, 0.38);
    }}
"#,
        surface = surface,
        border = theme.border,
        rm = rm,
        rs = rs,
        rl = rl,
        raised = raised,
        surface_solid = surface_solid,
        accent = accent,
        text = theme.text,
        muted = theme.text_muted,
        c_success = c_success,
        c_warning = c_warning,
        c_error = c_error,
        c_error_rgb = c_error_rgb,
        screenshot_toolbar_bg = screenshot_toolbar_bg,
        text_on_accent = text_on_accent,
        nc_panel_bg = nc_panel_bg,
        nc_card_bg = nc_card_bg,
        toast_card_bg = toast_card_bg,
        notif_card_bg = notif_card_bg,
        dash_shadow = dash_shadow,
        text_rgb = text_rgb,
        accent_rgb = accent_rgb,
        overlay_card_bg = overlay_card_bg,
        overlay_dot = overlay_dot,
        accent2 = accent2,
    )
}
