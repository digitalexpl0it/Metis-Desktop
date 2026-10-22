#![no_main]
use libfuzzer_sys::fuzz_target;
use metis_config::{WidgetExtNode, validate_widget_layout};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.len() > metis_config::WIDGET_EXT_MAX_JSON_BYTES as usize {
        return;
    }
    let Ok(node) = serde_json::from_str::<WidgetExtNode>(text) else {
        return;
    };
    let _ = validate_widget_layout(&node);
});
