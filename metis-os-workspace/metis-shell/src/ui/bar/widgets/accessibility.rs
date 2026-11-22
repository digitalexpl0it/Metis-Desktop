//! Accessibility edge-bar icon + quick-toggle overlay.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::ui::icons;

pub struct AccessibilityWidget {
    root: gtk::Button,
    updating: Rc<Cell<bool>>,
    screen_reader: gtk::Switch,
    magnifier: gtk::Switch,
    invert: gtk::Switch,
    high_contrast: gtk::Switch,
}

impl AccessibilityWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-accessibility");
        root.add_css_class("metis-bar-sys-icon");
        root.set_tooltip_text(Some(&metis_i18n::tr("Accessibility")));

        let icon = icons::image("preferences-desktop-accessibility-symbolic");
        root.set_child(Some(&icon));

        let panel = super::super::dropdown::build_panel();
        panel.set_spacing(8);
        panel.set_width_request(260);

        let title = gtk::Label::new(Some(&metis_i18n::tr("Accessibility")));
        title.set_xalign(0.0);
        title.add_css_class("metis-bar-section-title");
        panel.append(&title);

        let updating = Rc::new(Cell::new(false));

        let screen_reader = toggle_row(
            &panel,
            &metis_i18n::tr("Screen reader"),
            updating.clone(),
            crate::services::set_screen_reader,
        );
        let magnifier = toggle_row(
            &panel,
            &metis_i18n::tr("Magnifier"),
            updating.clone(),
            crate::services::set_magnifier,
        );
        let invert = toggle_row(
            &panel,
            &metis_i18n::tr("Invert colors"),
            updating.clone(),
            crate::services::set_invert_colors,
        );
        let high_contrast = toggle_row(
            &panel,
            &metis_i18n::tr("High contrast"),
            updating.clone(),
            crate::services::set_high_contrast,
        );

        let settings_btn = gtk::Button::with_label(&metis_i18n::tr("Accessibility settings…"));
        settings_btn.connect_clicked(|_| {
            if let Err(err) =
                crate::compositor::launch_program("metis-settings --page accessibility")
            {
                tracing::warn!(%err, "failed to open accessibility settings");
            }
            super::super::dropdown::request_close_all();
        });
        panel.append(&settings_btn);

        let screen_reader_s = screen_reader.clone();
        let magnifier_s = magnifier.clone();
        let invert_s = invert.clone();
        let high_contrast_s = high_contrast.clone();
        let updating_s = updating.clone();
        super::super::dropdown::wire_toggle_prepare(&root, &panel, move || {
            let cfg = metis_config::load_accessibility_config();
            updating_s.set(true);
            screen_reader_s.set_active(cfg.screen_reader);
            magnifier_s.set_active(cfg.magnifier);
            invert_s.set_active(cfg.invert_colors);
            high_contrast_s.set_active(cfg.high_contrast);
            updating_s.set(false);
        });

        Self {
            root,
            updating,
            screen_reader,
            magnifier,
            invert,
            high_contrast,
        }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }

    /// Sync switches from disk (e.g. after Settings changes).
    #[allow(dead_code)]
    pub fn refresh_from_config(&self) {
        let cfg = metis_config::load_accessibility_config();
        self.updating.set(true);
        self.screen_reader.set_active(cfg.screen_reader);
        self.magnifier.set_active(cfg.magnifier);
        self.invert.set_active(cfg.invert_colors);
        self.high_contrast.set_active(cfg.high_contrast);
        self.updating.set(false);
    }
}

fn toggle_row(
    panel: &gtk::Box,
    label: &str,
    updating: Rc<Cell<bool>>,
    on_toggle: impl Fn(bool) + 'static,
) -> gtk::Switch {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-a11y-row");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_hexpand(true);
    row.append(&lbl);
    let sw = gtk::Switch::new();
    sw.set_valign(gtk::Align::Center);
    sw.add_css_class("metis-net-switch");
    {
        let updating = updating.clone();
        sw.connect_state_set(move |_, state| {
            if !updating.get() {
                on_toggle(state);
            }
            glib::Propagation::Proceed
        });
    }
    row.append(&sw);
    panel.append(&row);
    sw
}
