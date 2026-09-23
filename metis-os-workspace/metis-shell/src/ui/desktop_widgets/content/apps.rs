//! Apps widget — start-menu pins by default; grid or list; A–Z by name.

use gtk::gio;
use gtk::prelude::*;
use metis_config::{DesktopWidgetInstance, DesktopWidgetView, load_menu_config};

use crate::services::applications;

const TILE_ICON: i32 = 48;
const TILE_WIDTH: i32 = 96;
const LIST_ICON: i32 = 20;

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    root.set_vexpand(true);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();

    let (pins, from_menu) = resolve_pins(inst);
    let sorted = sort_pins_az(&pins);

    if from_menu && !sorted.is_empty() {
        let badge = gtk::Label::new(Some(&metis_i18n::tr("Following start-menu pins")));
        badge.set_xalign(0.0);
        badge.add_css_class("metis-dw-hint");
        root.append(&badge);
    }

    match inst.view {
        DesktopWidgetView::Grid => {
            let flow = gtk::FlowBox::builder()
                .valign(gtk::Align::Start)
                .max_children_per_line(8)
                .min_children_per_line(2)
                .selection_mode(gtk::SelectionMode::None)
                .homogeneous(true)
                .column_spacing(4)
                .row_spacing(4)
                .build();
            flow.add_css_class("metis-dw-folder-grid");
            scroll.set_child(Some(&flow));
            root.append(&scroll);

            if sorted.is_empty() {
                flow.insert(&empty_hint(), -1);
            } else {
                for id in &sorted {
                    flow.insert(&app_tile(id), -1);
                }
            }
        }
        DesktopWidgetView::List => {
            let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
            list.add_css_class("metis-dw-list");
            scroll.set_child(Some(&list));
            root.append(&scroll);

            if sorted.is_empty() {
                list.append(&empty_hint());
            } else {
                for id in &sorted {
                    list.append(&app_row(id));
                }
            }
        }
    }

    root.upcast()
}

fn empty_hint() -> gtk::Label {
    let empty = gtk::Label::new(Some(&metis_i18n::tr(
        "No pinned apps yet.\n\
         Right-click an app in the Metis start menu and choose Pin — \
         they show here automatically.",
    )));
    empty.set_wrap(true);
    empty.set_xalign(0.0);
    empty.add_css_class("metis-dw-hint");
    empty
}

/// Empty widget `pins` → live `menu.json` pins. Non-empty → dedicated list.
fn resolve_pins(inst: &DesktopWidgetInstance) -> (Vec<String>, bool) {
    if !inst.pins.is_empty() {
        return (inst.pins.clone(), false);
    }
    (load_menu_config().pinned, true)
}

fn sort_pins_az(pins: &[String]) -> Vec<String> {
    let mut items: Vec<(String, String)> = pins
        .iter()
        .map(|id| {
            let name = applications::resolve_entry_for_id(id)
                .map(|e| e.name)
                .unwrap_or_else(|| prettify(id));
            (name.to_lowercase(), id.clone())
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    items.into_iter().map(|(_, id)| id).collect()
}

fn app_row(id: &str) -> gtk::Widget {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-dw-row");
    btn.set_has_frame(false);
    btn.set_halign(gtk::Align::Fill);
    btn.set_hexpand(true);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let (name, icon) = resolve_name_icon(id);
    let image = gtk::Image::from_gicon(&icon);
    image.set_pixel_size(LIST_ICON);
    row.append(&image);
    let label = gtk::Label::new(Some(&name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    btn.set_child(Some(&row));

    let id = id.to_string();
    btn.connect_clicked(move |_| {
        applications::launch_id(&id);
    });

    btn.upcast()
}

fn app_tile(id: &str) -> gtk::Widget {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-dw-folder-tile");
    btn.set_has_frame(false);
    btn.set_hexpand(true);
    btn.set_size_request(TILE_WIDTH, -1);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_halign(gtk::Align::Center);

    let (name, icon) = resolve_name_icon(id);
    btn.set_tooltip_text(Some(&name));
    let image = gtk::Image::from_gicon(&icon);
    image.set_pixel_size(TILE_ICON);
    image.set_halign(gtk::Align::Center);
    col.append(&image);

    let label = gtk::Label::new(Some(&name));
    label.add_css_class("metis-dw-folder-name");
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_justify(gtk::Justification::Center);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(12);
    label.set_xalign(0.5);
    col.append(&label);
    btn.set_child(Some(&col));

    let id = id.to_string();
    btn.connect_clicked(move |_| {
        applications::launch_id(&id);
    });

    btn.upcast()
}

fn resolve_name_icon(id: &str) -> (String, gio::Icon) {
    match applications::resolve_entry_for_id(id) {
        Some(entry) => {
            let icon = entry.icon.clone().unwrap_or_else(|| {
                gio::ThemedIcon::new(applications::FALLBACK_ICON_NAME).upcast::<gio::Icon>()
            });
            (entry.name, icon)
        }
        None => (
            prettify(id),
            gio::ThemedIcon::new(applications::FALLBACK_ICON_NAME).upcast::<gio::Icon>(),
        ),
    }
}

fn prettify(id: &str) -> String {
    id.trim_end_matches(".desktop").replace(['.', '-'], " ")
}
