use gtk::gdk;
use gtk::prelude::*;

const ICON_SIZE: i32 = 18;

/// Bar icons resolve from the host GTK icon theme (symbolic names).
/// Incomplete packs (or cursor themes mis-selected) often omit Metis names —
/// fall back through common aliases so the bar never shows a blank glyph.
pub fn image(name: &str) -> gtk::Image {
    let image = gtk::Image::new();
    image.add_css_class("metis-bar-icon");
    set_icon(&image, name);
    image
}

pub fn set_icon(image: &gtk::Image, name: &str) {
    image.set_icon_name(Some(resolve_icon_name(name)));
    image.set_pixel_size(ICON_SIZE);
}

fn resolve_icon_name(name: &str) -> &str {
    if icon_theme_has(name) {
        return name;
    }
    for alt in icon_fallbacks(name) {
        if icon_theme_has(alt) {
            return alt;
        }
    }
    // Last resort: keep the original so GTK can still Inherits= chain.
    name
}

fn icon_theme_has(name: &str) -> bool {
    let Some(display) = gdk::Display::default() else {
        return true;
    };
    gtk::IconTheme::for_display(&display).has_icon(name)
}

fn icon_fallbacks(name: &str) -> &'static [&'static str] {
    match name {
        "battery-level-100-charging-symbolic" => &[
            "battery-full-charging-symbolic",
            "battery-good-charging-symbolic",
            "battery-full-symbolic",
        ],
        "battery-level-100-symbolic" => &["battery-full-symbolic", "battery-good-symbolic"],
        "battery-level-80-symbolic" => &["battery-good-symbolic", "battery-full-symbolic"],
        "battery-level-50-symbolic" => &["battery-medium-symbolic", "battery-good-symbolic"],
        "battery-level-20-symbolic" => &["battery-low-symbolic", "battery-caution-symbolic"],
        "battery-level-10-symbolic" => &[
            "battery-caution-symbolic",
            "battery-empty-symbolic",
            "battery-low-symbolic",
        ],
        "network-wireless-symbolic" => &["network-wireless-signal-excellent-symbolic"],
        "network-offline-symbolic" => &[
            "network-wireless-offline-symbolic",
            "network-wired-offline-symbolic",
        ],
        "bluetooth-active-symbolic" => &["bluetooth-symbolic"],
        "bluetooth-disabled-symbolic" => &["bluetooth-symbolic"],
        "notifications-disabled-symbolic" => &[
            "notification-disabled-symbolic",
            "preferences-system-notifications-symbolic",
        ],
        "preferences-system-notifications-symbolic" => &["appointment-soon-symbolic"],
        "software-update-available-symbolic" => &["system-software-update-symbolic"],
        "audio-volume-medium-symbolic" => &["audio-volume-high-symbolic"],
        "microphone-sensitivity-medium-symbolic" => &["microphone-sensitivity-high-symbolic"],
        "microphone-sensitivity-low-symbolic" => &["microphone-sensitivity-medium-symbolic"],
        "edit-paste-symbolic" => &["edit-copy-symbolic"],
        _ => &[],
    }
}

pub mod names {
    pub const CLIPBOARD: &str = "edit-paste-symbolic";
    pub const NOTIFICATION: &str = "preferences-system-notifications-symbolic";
    pub const NOTIFICATION_DND: &str = "notifications-disabled-symbolic";

    pub fn battery(percent: u8, charging: bool) -> &'static str {
        if charging {
            return "battery-level-100-charging-symbolic";
        }
        match percent {
            90..=100 => "battery-level-100-symbolic",
            60..=89 => "battery-level-80-symbolic",
            30..=59 => "battery-level-50-symbolic",
            10..=29 => "battery-level-20-symbolic",
            _ => "battery-level-10-symbolic",
        }
    }

    pub fn network(connected: bool) -> &'static str {
        if connected {
            "network-wireless-symbolic"
        } else {
            "network-offline-symbolic"
        }
    }

    pub fn notification(dnd: bool) -> &'static str {
        if dnd { NOTIFICATION_DND } else { NOTIFICATION }
    }

    pub fn bluetooth(powered: bool, connected: bool) -> &'static str {
        if !powered {
            "bluetooth-disabled-symbolic"
        } else if connected {
            "bluetooth-active-symbolic"
        } else {
            "bluetooth-symbolic"
        }
    }

    pub fn volume(percent: u8, muted: bool) -> &'static str {
        if muted || percent == 0 {
            "audio-volume-muted-symbolic"
        } else if percent < 35 {
            "audio-volume-low-symbolic"
        } else if percent < 70 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }
    }

    pub fn mic(percent: u8, muted: bool) -> &'static str {
        if muted || percent == 0 {
            "microphone-sensitivity-muted-symbolic"
        } else if percent < 35 {
            "microphone-sensitivity-low-symbolic"
        } else if percent < 70 {
            "microphone-sensitivity-medium-symbolic"
        } else {
            "microphone-sensitivity-high-symbolic"
        }
    }
}
