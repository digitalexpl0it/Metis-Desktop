//! Shell stylesheet fragment: `appearance_preview`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
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
"#,
    )
}
