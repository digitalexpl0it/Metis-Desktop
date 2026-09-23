use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

/// Metis brand icon, embedded so the launcher renders regardless of the working
/// directory the shell is started from.
const ICON_BYTES: &[u8] = include_bytes!("../../../../assets/metis_icon.png");

const ICON_SIZE: i32 = 22;

/// Far-left brand button on the edge bar. Clicking it toggles the Metis app menu.
pub struct LauncherWidget {
    root: gtk::Button,
}

impl LauncherWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().has_frame(false).build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-launcher");
        root.set_tooltip_text(Some(&metis_i18n::tr("Metis")));

        let image = gtk::Image::new();
        image.add_css_class("metis-bar-launcher-icon");
        image.set_pixel_size(ICON_SIZE);
        match load_icon() {
            Some(texture) => {
                image.set_paintable(Some(&texture));
            }
            _ => {
                // Fall back to a themed icon if the embedded asset fails to decode.
                image.set_icon_name(Some("view-grid-symbolic"));
            }
        }
        root.set_child(Some(&image));

        super::menu::install(&root);

        Self { root }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }
}

fn load_icon() -> Option<gdk::Texture> {
    let bytes = glib::Bytes::from_static(ICON_BYTES);
    match gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Some(texture),
        Err(err) => {
            tracing::warn!(%err, "failed to decode embedded launcher icon");
            None
        }
    }
}
