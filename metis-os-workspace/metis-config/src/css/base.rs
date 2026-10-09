//! Shell stylesheet fragment: `base`.

use super::vars::CssVars;

pub(crate) fn stylesheet(v: &CssVars) -> String {
    v.render(
        r#"
    window {{
        background-color: transparent;
        {font_decls}
    }}

"#,
    )
}
