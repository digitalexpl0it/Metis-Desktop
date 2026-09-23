use gtk::prelude::*;

const ICON_SIZE: i32 = 18;

/// Bar icons resolve from the host GTK icon theme (symbolic names).
pub fn image(name: &str) -> gtk::Image {
    let image = gtk::Image::new();
    image.add_css_class("metis-bar-icon");
    image.set_icon_name(Some(name));
    image.set_pixel_size(ICON_SIZE);
    image
}

pub fn set_icon(image: &gtk::Image, name: &str) {
    image.set_icon_name(Some(name));
    image.set_pixel_size(ICON_SIZE);
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
