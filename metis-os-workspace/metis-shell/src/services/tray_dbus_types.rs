//! Shared D-Bus parsing helpers for the system tray.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

#[derive(Debug, zbus::zvariant::Type, serde::Deserialize)]
pub struct MenuLayout {
    pub id: u32,
    pub fields: SubMenuLayout,
}

#[derive(Debug, zbus::zvariant::Type, serde::Deserialize)]
pub struct SubMenuLayout {
    #[allow(dead_code)] // required for DBusMenu deserialization
    pub id: i32,
    #[allow(dead_code)] // required for DBusMenu deserialization
    pub fields: HashMap<String, OwnedValue>,
    pub submenus: Vec<OwnedValue>,
}

#[derive(Debug, Clone)]
pub struct IconPixmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

pub struct ParsedItemProps {
    pub id: String,
    pub title: String,
    pub tooltip_title: String,
    pub tooltip_subtitle: String,
    pub icon_name: Option<String>,
    pub icon_theme_path: Option<String>,
    pub icon_pixmap: Option<IconPixmap>,
    pub menu: Option<String>,
    pub item_is_menu: bool,
}

#[derive(Clone, Debug)]
pub struct ServiceParts {
    pub bus_name: String,
    pub object_path: String,
}

const DEFAULT_SNI_PATH: &str = "/StatusNotifierItem";

pub fn service_parts(item_id: &str) -> Option<ServiceParts> {
    let rest = item_id.strip_prefix(':')?;
    let bus_len = rest
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if bus_len == 0 {
        return None;
    }
    let unique_name = format!(":{}", &rest[..bus_len]);
    let service = item_id[unique_name.len()..].trim();

    if service.is_empty() {
        return Some(ServiceParts {
            bus_name: unique_name,
            object_path: DEFAULT_SNI_PATH.into(),
        });
    }

    if service.starts_with('/') {
        // Registered with an object path on the sender's unique connection.
        Some(ServiceParts {
            bus_name: unique_name,
            object_path: normalize_object_path(service),
        })
    } else if service.starts_with(':') {
        Some(ServiceParts {
            bus_name: service.to_string(),
            object_path: DEFAULT_SNI_PATH.into(),
        })
    } else {
        // Registered with a well-known bus name (Flameshot, most Qt/KDE apps).
        Some(ServiceParts {
            bus_name: service.to_string(),
            object_path: DEFAULT_SNI_PATH.into(),
        })
    }
}

/// The unique connection name (`:1.x`) that registered an item id. Item ids are
/// `{unique}{service}` (see [`service_parts`]); the sender is always the leading
/// `:digits.digits` run. Used to drop an item when its owning connection's name
/// vanishes, regardless of whether the item registered under its unique name
/// (Claude) or a well-known name (Flameshot / most Qt/KDE apps).
pub fn item_unique_name(item_id: &str) -> Option<String> {
    let rest = item_id.strip_prefix(':')?;
    let bus_len = rest
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if bus_len == 0 {
        return None;
    }
    Some(format!(":{}", &rest[..bus_len]))
}

fn normalize_object_path(service: &str) -> String {
    if service.is_empty() {
        DEFAULT_SNI_PATH.into()
    } else if service.starts_with('/') {
        service.to_string()
    } else {
        format!("/{service}")
    }
}

pub fn parse_item_props(props: &HashMap<String, OwnedValue>) -> ParsedItemProps {
    let id = get_string(props, "Id")
        .filter(|s| !s.is_empty())
        .or_else(|| get_string(props, "Title"))
        .unwrap_or_else(|| "tray-item".into());
    let title = get_string(props, "Title").unwrap_or_default();
    let (tooltip_title, tooltip_subtitle) = get_tooltip_strings(props);
    ParsedItemProps {
        id,
        title,
        tooltip_title,
        tooltip_subtitle,
        icon_name: get_string(props, "IconName"),
        icon_theme_path: get_string(props, "IconThemePath"),
        icon_pixmap: get_icon_pixmap(props),
        menu: get_object_path(props, "Menu"),
        item_is_menu: get_bool(props, "ItemIsMenu").unwrap_or(false),
    }
}

/// Pick a human-readable label for tooltips and menus. Electron/Chromium apps
/// often leave `Title` empty and put an internal id like `chrome_status_icon_1`
/// in `Id`, while the real name lives in `ToolTip`.
pub fn resolve_tray_display_title(
    title: &str,
    id: &str,
    tooltip_title: &str,
    tooltip_subtitle: &str,
    bus_name: &str,
) -> String {
    for candidate in [title, tooltip_title, tooltip_subtitle] {
        if is_user_visible_tray_label(candidate) {
            return candidate.to_string();
        }
    }
    if is_user_visible_tray_label(id) {
        return id.to_string();
    }
    if let Some(name) = friendly_name_from_bus(bus_name) {
        return name;
    }
    if !tooltip_title.is_empty() {
        return tooltip_title.to_string();
    }
    if !title.is_empty() {
        return title.to_string();
    }
    humanize_tray_id(id)
}

fn is_user_visible_tray_label(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    if is_internal_tray_label(s) {
        return false;
    }
    true
}

fn is_internal_tray_label(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("chrome_status_icon")
        || lower.starts_with("libappindicator-")
        || lower.contains("statusnotifieritem")
        || (lower.contains('_')
            && !s.contains(' ')
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()))
}

fn friendly_name_from_bus(bus_name: &str) -> Option<String> {
    let bus_name = bus_name.trim();
    if bus_name.is_empty() || bus_name.starts_with(':') {
        return None;
    }
    let last = bus_name.rsplit('.').next()?;
    if last.contains("StatusNotifierItem") {
        return None;
    }
    let name = humanize_identifier(last);
    if name.is_empty() { None } else { Some(name) }
}

fn humanize_tray_id(id: &str) -> String {
    let stripped = id
        .strip_prefix("chrome_status_icon_")
        .or_else(|| id.strip_prefix("chrome_status_icon"))
        .unwrap_or(id);
    humanize_identifier(stripped)
}

fn humanize_identifier(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_was_sep = false;
    for ch in raw.chars() {
        if ch == '-' || ch == '_' {
            if !out.is_empty() && !prev_was_sep {
                out.push(' ');
            }
            prev_was_sep = true;
            continue;
        }
        if (prev_was_sep || out.is_empty()) && ch.is_ascii_lowercase() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
        prev_was_sep = false;
    }
    out.trim().to_string()
}

/// `ToolTip` is `(icon_name, icon_pixmap[], title, subtitle)` per the SNI spec.
fn get_tooltip_strings(props: &HashMap<String, OwnedValue>) -> (String, String) {
    let Some(val) = props.get("ToolTip") else {
        return (String::new(), String::new());
    };
    let Value::Structure(s) = &**val else {
        return (String::new(), String::new());
    };
    let mut fields = s.fields().iter();
    let _icon_name = fields.next();
    let _icon_pixmap = fields.next();
    let title = fields
        .next()
        .and_then(|v| match v {
            Value::Str(s) => Some(s.as_str().to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let subtitle = fields
        .next()
        .and_then(|v| match v {
            Value::Str(s) => Some(s.as_str().to_string()),
            _ => None,
        })
        .unwrap_or_default();
    (title, subtitle)
}

fn get_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|v| match &**v {
        Value::Str(s) => Some(s.as_str().to_string()),
        _ => None,
    })
}

fn get_object_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|v| match &**v {
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        Value::Str(s) => Some(s.as_str().to_string()),
        _ => None,
    })
}

fn get_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| match &**v {
        Value::Bool(b) => Some(*b),
        _ => None,
    })
}

fn get_icon_pixmap(props: &HashMap<String, OwnedValue>) -> Option<IconPixmap> {
    let Value::Array(arr) = &**props.get("IconPixmap")? else {
        return None;
    };
    let mut best: Option<(i32, IconPixmap)> = None;
    for entry in arr.iter() {
        let Value::Structure(s) = entry else {
            continue;
        };
        let mut fields = s.fields().iter();
        let Value::I32(width) = fields.next()? else {
            continue;
        };
        let Value::I32(height) = fields.next()? else {
            continue;
        };
        let pixels_val = fields.next()?;
        let Value::Array(pixel_arr) = pixels_val else {
            continue;
        };
        let mut pixels = Vec::new();
        for p in pixel_arr.iter() {
            let Value::U8(byte) = p else {
                continue;
            };
            pixels.push(*byte);
        }
        let pixmap = IconPixmap {
            width: *width,
            height: *height,
            pixels,
        };
        let dist = (height - 22).abs();
        if best.as_ref().is_none_or(|(h, _)| (h - 22).abs() > dist) {
            best = Some((*height, pixmap));
        }
    }
    best.map(|(_, pm)| pm)
}

#[cfg(test)]
mod tests {
    use super::{resolve_tray_display_title, service_parts};

    #[test]
    fn parses_ayatana_item_id() {
        let parts = service_parts(":1.45/org/ayatana/NotificationItem/123").unwrap();
        assert_eq!(parts.bus_name, ":1.45");
        assert_eq!(parts.object_path, "/org/ayatana/NotificationItem/123");
    }

    #[test]
    fn parses_kde_well_known_service_name() {
        let parts = service_parts(":1.282org.kde.StatusNotifierItem-229658-1").unwrap();
        assert_eq!(parts.bus_name, "org.kde.StatusNotifierItem-229658-1");
        assert_eq!(parts.object_path, "/StatusNotifierItem");
    }

    #[test]
    fn parses_kde_item_id_without_leading_slash() {
        let parts = service_parts(":1.250org.kde.StatusNotifierItem-205764-1").unwrap();
        assert_eq!(parts.bus_name, "org.kde.StatusNotifierItem-205764-1");
        assert_eq!(parts.object_path, "/StatusNotifierItem");
    }

    #[test]
    fn parses_unique_name_only_fallback() {
        let parts = service_parts(":1.253").unwrap();
        assert_eq!(parts.bus_name, ":1.253");
        assert_eq!(parts.object_path, "/StatusNotifierItem");
    }

    #[test]
    fn parses_duplicate_unique_service() {
        let parts = service_parts(":1.285:1.285").unwrap();
        assert_eq!(parts.bus_name, ":1.285");
        assert_eq!(parts.object_path, "/StatusNotifierItem");
    }

    #[test]
    fn resolves_electron_tray_tooltip() {
        let title = resolve_tray_display_title(
            "",
            "chrome_status_icon_1",
            "Cursor",
            "",
            "org.chromium.StatusNotifierItem-1-1",
        );
        assert_eq!(title, "Cursor");
    }

    #[test]
    fn skips_internal_title_for_tooltip() {
        let title = resolve_tray_display_title(
            "chrome_status_icon_1",
            "chrome_status_icon_1",
            "Cursor",
            "",
            "",
        );
        assert_eq!(title, "Cursor");
    }

    #[test]
    fn friendly_name_from_bus_name() {
        let title =
            resolve_tray_display_title("", "chrome_status_icon_1", "", "", "co.anysphere.Cursor");
        assert_eq!(title, "Cursor");
    }
}
