//! Parsed com.canonical.dbusmenu layouts for tray context menus.

use zbus::zvariant::{Dict, OwnedValue, Structure, Value};

use super::tray_dbus_types::MenuLayout;

#[derive(Debug, Clone)]
pub struct TrayMenu {
    #[allow(dead_code)] // parsed from DBusMenu layout root id
    pub id: u32,
    pub submenus: Vec<MenuItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuType {
    Separator,
    Standard,
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub menu_type: MenuType,
    /// `submenu` when this row is a submenu container (show children only).
    pub children_display: Option<String>,
    pub submenu: Vec<MenuItem>,
}

impl Default for MenuItem {
    fn default() -> Self {
        Self {
            id: 0,
            label: String::new(),
            enabled: true,
            visible: true,
            menu_type: MenuType::Standard,
            children_display: None,
            submenu: Vec::new(),
        }
    }
}

pub fn parse_menu_layout(layout: MenuLayout) -> TrayMenu {
    let submenus = layout
        .fields
        .submenus
        .iter()
        .filter_map(parse_menu_item)
        .collect();
    TrayMenu {
        id: layout.id,
        submenus,
    }
}

fn parse_menu_item(value: &OwnedValue) -> Option<MenuItem> {
    parse_menu_value(value)
}

fn parse_menu_value(value: &Value<'_>) -> Option<MenuItem> {
    let structure = match value.downcast_ref::<Structure>() {
        Ok(s) => s,
        Err(_) => return None,
    };
    let mut fields = structure.fields().iter();
    let mut item = MenuItem::default();

    if let Some(Value::I32(id)) = fields.next() {
        item.id = *id;
    }

    let Value::Dict(dict) = fields.next()? else {
        return Some(item);
    };

    if let Some(label) = dict_str(dict, "label") {
        item.label = label;
    }
    if let Some(enabled) = dict_bool(dict, "enabled") {
        item.enabled = enabled;
    }
    if let Some(visible) = dict_bool(dict, "visible") {
        item.visible = visible;
    }
    if let Some(display) = dict_str(dict, "children-display") {
        item.children_display = Some(display);
    }
    if let Some(kind) = dict_str(dict, "type") {
        item.menu_type = match kind.as_str() {
            "separator" => MenuType::Separator,
            _ => MenuType::Standard,
        };
    }

    if let Some(Value::Array(arr)) = fields.next() {
        for child in arr.iter() {
            if let Some(sub) = parse_menu_value(child) {
                item.submenu.push(sub);
            }
        }
    }

    Some(item)
}

fn dict_str(dict: &Dict<'_, '_>, key: &str) -> Option<String> {
    dict.get::<&str, &str>(&key)
        .ok()
        .flatten()
        .map(|label| label.replace('_', ""))
}

fn dict_bool(dict: &Dict<'_, '_>, key: &str) -> Option<bool> {
    dict.get::<&str, bool>(&key).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};

    use crate::services::tray_dbus_types::SubMenuLayout;

    fn item_owned(
        id: i32,
        props: HashMap<&'static str, Value<'static>>,
        children: Vec<Value<'static>>,
    ) -> OwnedValue {
        OwnedValue::try_from(Value::new((id, props, children))).expect("menu item value")
    }

    #[test]
    fn parse_menu_layout_strips_mnemonics_and_nested_items() {
        let mut quit_props = HashMap::new();
        quit_props.insert("label", Value::from("_Quit"));
        quit_props.insert("enabled", Value::from(true));
        quit_props.insert("type", Value::from("standard"));

        let mut sep_props = HashMap::new();
        sep_props.insert("type", Value::from("separator"));

        let mut parent_props = HashMap::new();
        parent_props.insert("label", Value::from("_File"));
        parent_props.insert("children-display", Value::from("submenu"));

        let quit_child = Value::new((2i32, quit_props, Vec::<Value<'static>>::new()));
        let sep_child = Value::new((3i32, sep_props, Vec::<Value<'static>>::new()));
        let parent = item_owned(1, parent_props, vec![quit_child, sep_child]);

        let layout = MenuLayout {
            id: 0,
            fields: SubMenuLayout {
                id: 0,
                fields: HashMap::new(),
                submenus: vec![parent],
            },
        };
        let menu = parse_menu_layout(layout);
        assert_eq!(menu.id, 0);
        assert_eq!(menu.submenus.len(), 1);
        let file = &menu.submenus[0];
        assert_eq!(file.id, 1);
        assert_eq!(file.label, "File");
        assert_eq!(file.children_display.as_deref(), Some("submenu"));
        assert_eq!(file.submenu.len(), 2);
        assert_eq!(file.submenu[0].label, "Quit");
        assert_eq!(file.submenu[1].menu_type, MenuType::Separator);
    }

    #[test]
    fn parse_menu_layout_skips_non_structures() {
        let junk = OwnedValue::try_from(Value::new("not-a-menu-item")).expect("string value");
        let layout = MenuLayout {
            id: 7,
            fields: SubMenuLayout {
                id: 0,
                fields: HashMap::new(),
                submenus: vec![junk],
            },
        };
        let menu = parse_menu_layout(layout);
        assert_eq!(menu.id, 7);
        assert!(menu.submenus.is_empty());
    }
}
