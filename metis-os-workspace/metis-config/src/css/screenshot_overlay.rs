//! Shell stylesheet fragment: `screenshot_overlay`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
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

"#,
    )
}
