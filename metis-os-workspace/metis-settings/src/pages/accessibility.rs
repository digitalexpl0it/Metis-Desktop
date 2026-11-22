//! Accessibility: Vision / Hearing / Mobility.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::AccessibilityConfig;

use crate::accessibility;
use crate::ui;
use metis_i18n::tr;

struct Sections {
    screen_reader: gtk::Switch,
    magnifier: gtk::Switch,
    high_contrast: gtk::Switch,
    invert: gtk::Switch,
    color_filters: gtk::Switch,
    color_filter_type: gtk::DropDown,
    shake_pointer: gtk::Switch,
    mono_audio: gtk::Switch,
    syncing: RefCell<bool>,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("accessibility");

    let (vision_card, vision_body) = ui::section(&tr("Vision"));
    let (sr_row, screen_reader) = ui::switch_row(&tr("Screen reader"));
    vision_body.append(&sr_row);
    let (mag_row, magnifier) = ui::switch_row(&tr("Magnifier"));
    vision_body.append(&mag_row);
    let (hc_row, high_contrast) = ui::switch_row(&tr("High contrast mode"));
    vision_body.append(&hc_row);
    let (inv_row, invert) = ui::switch_row(&tr("Invert colors"));
    vision_body.append(&inv_row);
    let (cf_row, color_filters) = ui::switch_row(&tr("Color filters"));
    vision_body.append(&cf_row);
    let filter_labels: Vec<String> = metis_config::ColorFilterType::ALL
        .iter()
        .map(|k| tr(k.label()))
        .collect();
    let filter_refs: Vec<&str> = filter_labels.iter().map(|s| s.as_str()).collect();
    let color_filter_type = gtk::DropDown::from_strings(&filter_refs);
    vision_body.append(&ui::row(&tr("Color filter type"), &color_filter_type));
    let (shake_row, shake_pointer) = ui::switch_row(&tr("Shake pointer to find it"));
    let shake_hint = gtk::Label::new(Some(&tr(
        "Quickly shake the mouse to temporarily enlarge the pointer",
    )));
    shake_hint.set_xalign(0.0);
    shake_hint.add_css_class("metis-settings-hint");
    vision_body.append(&shake_row);
    vision_body.append(&shake_hint);
    content.append(&vision_card);

    let (hear_card, hear_body) = ui::section(&tr("Hearing"));
    let (mono_row, mono_audio) = ui::switch_row(&tr("Play stereo audio as mono"));
    hear_body.append(&mono_row);
    content.append(&hear_card);

    let (mob_card, mob_body) = ui::section(&tr("Mobility"));
    let osk_btn = gtk::Button::with_label(&tr("On-screen keyboard"));
    osk_btn.set_halign(gtk::Align::Start);
    osk_btn.connect_clicked(|_| accessibility::launch_on_screen_keyboard());
    let osk_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    osk_row.add_css_class("metis-settings-row");
    let osk_lbl = gtk::Label::new(Some(&tr("On-screen keyboard")));
    osk_lbl.set_xalign(0.0);
    osk_lbl.set_hexpand(true);
    osk_row.append(&osk_lbl);
    let chevron = gtk::Image::from_icon_name("go-next-symbolic");
    osk_row.append(&chevron);
    // Whole row opens the keyboard (chevron affordance).
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| accessibility::launch_on_screen_keyboard());
    osk_row.add_controller(click);
    mob_body.append(&osk_row);
    let _ = osk_btn;
    content.append(&mob_card);

    let hint = gtk::Label::new(Some(&tr(
        "Quick toggles are also on the Accessibility icon in the edge bar. Magnifier is a 2× desktop zoom around the pointer; shake the mouse to enlarge the cursor with a sonar pulse. High contrast, invert, and colour filters apply to Metis chrome; screen reader uses Orca when installed.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let sections = Rc::new(Sections {
        screen_reader,
        magnifier,
        high_contrast,
        invert,
        color_filters,
        color_filter_type,
        shake_pointer,
        mono_audio,
        syncing: RefCell::new(false),
    });

    apply_to_ui(&sections, &accessibility::load());

    let persist = {
        let sections = sections.clone();
        Rc::new(move || {
            if *sections.syncing.borrow() {
                return;
            }
            let cfg = read_from_ui(&sections);
            accessibility::save(&cfg);
        })
    };

    wire_switch(&sections.screen_reader, persist.clone());
    wire_switch(&sections.magnifier, persist.clone());
    wire_switch(&sections.high_contrast, persist.clone());
    wire_switch(&sections.invert, persist.clone());
    wire_switch(&sections.color_filters, persist.clone());
    wire_switch(&sections.shake_pointer, persist.clone());
    wire_switch(&sections.mono_audio, persist.clone());
    {
        let persist = persist.clone();
        sections
            .color_filter_type
            .connect_selected_notify(move |_| persist());
    }

    scroller.upcast()
}

fn wire_switch(sw: &gtk::Switch, persist: Rc<dyn Fn()>) {
    ui::defer_switch_active_notify(sw, move |_| persist());
}

fn apply_to_ui(sections: &Sections, cfg: &AccessibilityConfig) {
    *sections.syncing.borrow_mut() = true;
    sections.screen_reader.set_active(cfg.screen_reader);
    sections.magnifier.set_active(cfg.magnifier);
    sections.high_contrast.set_active(cfg.high_contrast);
    sections.invert.set_active(cfg.invert_colors);
    sections.color_filters.set_active(cfg.color_filters);
    sections
        .color_filter_type
        .set_selected(accessibility::color_filter_index(cfg.color_filter_type));
    sections.shake_pointer.set_active(cfg.shake_pointer);
    sections.mono_audio.set_active(cfg.mono_audio);
    *sections.syncing.borrow_mut() = false;
}

fn read_from_ui(sections: &Sections) -> AccessibilityConfig {
    AccessibilityConfig {
        screen_reader: sections.screen_reader.is_active(),
        magnifier: sections.magnifier.is_active(),
        high_contrast: sections.high_contrast.is_active(),
        invert_colors: sections.invert.is_active(),
        color_filters: sections.color_filters.is_active(),
        color_filter_type: accessibility::color_filter_from_index(
            sections.color_filter_type.selected(),
        ),
        shake_pointer: sections.shake_pointer.is_active(),
        mono_audio: sections.mono_audio.is_active(),
    }
}
