//! Per-widget configure options (extension / equalizer / text / chrome overrides).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetChromeOverride, DesktopWidgetInstance, DesktopWidgetKind, DesktopWidgetView,
    DesktopWidgetsConfig, EqualizerBarShape, EqualizerColorMode, EqualizerVizStyle,
    WidgetExtSettingType, find_widget_extension,
};

use crate::gtk_cb::OptFn0Cell;
use crate::pages::appearance_common::{
    color_dialog_button, font_picker_button, hex_to_rgba, rgba_to_hex,
};
use crate::ui;
use metis_i18n::tr;

use super::{mutate_from_disk, mutate_from_disk_debounced};

pub(crate) fn extension_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let id_label = gtk::Label::new(Some(&format!("id: {}", inst.extension_id)));
    id_label.set_xalign(0.0);
    id_label.set_selectable(true);
    id_label.add_css_class("metis-settings-hint");
    col.append(&id_label);

    let path_hint = if let Some(ext) = find_widget_extension(&inst.extension_id) {
        format!("Pack: {}", ext.root.display())
    } else {
        format!(
            "Pack missing — place under ~/.local/share/metis/widgets/{}/",
            if inst.extension_id.is_empty() {
                "<id>"
            } else {
                &inst.extension_id
            }
        )
    };
    let path_l = gtk::Label::new(Some(&path_hint));
    path_l.set_xalign(0.0);
    path_l.set_wrap(true);
    path_l.set_selectable(true);
    path_l.add_css_class("metis-settings-hint");
    col.append(&path_l);

    let Some(ext) = find_widget_extension(&inst.extension_id) else {
        return col.upcast();
    };
    if ext.manifest.settings_schema.is_empty() {
        let none = gtk::Label::new(Some(&tr("This extension has no settings.")));
        none.set_xalign(0.0);
        none.add_css_class("metis-settings-hint");
        col.append(&none);
        return col.upcast();
    }

    for setting in &ext.manifest.settings_schema {
        let key = setting.key.clone();
        let label = if setting.label.is_empty() {
            setting.key.clone()
        } else {
            setting.label.clone()
        };
        let current = inst
            .extension_settings
            .get(&key)
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        match setting.setting_type {
            WidgetExtSettingType::Bool => {
                let sw = gtk::Switch::new();
                sw.set_active(current.as_bool().unwrap_or(false));
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                sw.connect_state_set(move |_, on| {
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings
                                .insert(key.clone(), serde_json::Value::Bool(on));
                        }
                    });
                    glib::Propagation::Proceed
                });
                col.append(&ui::row(&label, &sw));
            }
            WidgetExtSettingType::Number => {
                let entry = gtk::Entry::new();
                let text = match &current {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => "0".into(),
                };
                entry.set_text(&text);
                entry.set_input_purpose(gtk::InputPurpose::Number);
                entry.set_hexpand(true);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                entry.connect_changed(move |e| {
                    let raw = e.text().to_string();
                    let val = raw
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::String(raw));
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings.insert(key.clone(), val);
                        }
                    });
                });
                col.append(&ui::row(&label, &entry));
            }
            WidgetExtSettingType::String => {
                let entry = gtk::Entry::new();
                let text = match &current {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                entry.set_text(&text);
                entry.set_hexpand(true);
                let id = inst.id.clone();
                let key = key.clone();
                let cfg = cfg.clone();
                entry.connect_changed(move |e| {
                    let text = e.text().to_string();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.extension_settings
                                .insert(key.clone(), serde_json::Value::String(text));
                        }
                    });
                });
                col.append(&ui::row(&label, &entry));
            }
        }
    }
    col.upcast()
}

pub(crate) fn equalizer_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    // Style — changing it rebuilds the option rows for that style.
    {
        let labels: Vec<&str> = EqualizerVizStyle::all().iter().map(|s| s.label()).collect();
        let dd = gtk::DropDown::from_strings(&labels);
        let selected = EqualizerVizStyle::all()
            .iter()
            .position(|s| *s == inst.viz_style)
            .unwrap_or(1) as u32;
        dd.set_selected(selected);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let rebuild_body = rebuild_body.clone();
            dd.connect_selected_notify(move |dd| {
                let style = EqualizerVizStyle::all()
                    .get(dd.selected() as usize)
                    .copied()
                    .unwrap_or(EqualizerVizStyle::Bars);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.viz_style = style;
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        col.append(&ui::row_with_icon(
            "multimedia-equalizer-symbolic",
            &tr("Style"),
            &dd,
        ));
    }

    // Colour mode — rebuild so solid / gradient pickers show correctly.
    {
        let labels: Vec<&str> = EqualizerColorMode::all()
            .iter()
            .map(|m| m.label())
            .collect();
        let dd = gtk::DropDown::from_strings(&labels);
        let selected = EqualizerColorMode::all()
            .iter()
            .position(|m| *m == inst.color_mode)
            .unwrap_or(1) as u32;
        dd.set_selected(selected);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let rebuild_body = rebuild_body.clone();
            dd.connect_selected_notify(move |dd| {
                let mode = EqualizerColorMode::all()
                    .get(dd.selected() as usize)
                    .copied()
                    .unwrap_or(EqualizerColorMode::Multi);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.color_mode = mode;
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        col.append(&ui::row_with_icon(
            "preferences-color-symbolic",
            &tr("Colour mode"),
            &dd,
        ));
    }

    match inst.color_mode {
        EqualizerColorMode::Solid => {
            let color = color_dialog_button();
            color.set_rgba(&hex_to_rgba(&inst.solid_color));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                color.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.solid_color = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Solid colour"),
                &color,
            ));
        }
        EqualizerColorMode::Multi => {
            let start = color_dialog_button();
            start.set_rgba(&hex_to_rgba(&inst.gradient_start));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                start.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.gradient_start = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Gradient start"),
                &start,
            ));

            let end = color_dialog_button();
            end.set_rgba(&hex_to_rgba(&inst.gradient_end));
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                end.connect_rgba_notify(move |btn| {
                    let hex = rgba_to_hex(&btn.rgba());
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.gradient_end = hex;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "color-select-symbolic",
                &tr("Gradient end"),
                &end,
            ));
        }
        EqualizerColorMode::Theme => {
            let hint = gtk::Label::new(Some(&tr(
                "Uses the active Appearance accent and secondary colours.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
    }

    // Density — useful for all styles.
    {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 16.0, 96.0, 1.0);
        scale.set_value(inst.bar_count as f64);
        scale.set_draw_value(true);
        scale.set_digits(0);
        scale.set_hexpand(true);
        ui::forward_wheel_to_page_scroller(&scale);
        let id = inst.id.clone();
        {
            let cfg = cfg.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                let n = s.value().round() as u32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.bar_count = n.clamp(16, 96);
                    }
                });
            });
        }
        let density_label = match inst.viz_style {
            EqualizerVizStyle::Bars => "Bars / density",
            EqualizerVizStyle::SpectrumLines => "Lines / density",
            EqualizerVizStyle::NeonWave => "Points / density",
            EqualizerVizStyle::Radial => "Rays / density",
        };
        col.append(&ui::row_with_icon(
            "view-continuous-symbolic",
            density_label,
            &scale,
        ));
    }

    // Style-specific controls.
    match inst.viz_style {
        EqualizerVizStyle::Bars => {
            {
                let labels: Vec<&str> =
                    EqualizerBarShape::all().iter().map(|s| s.label()).collect();
                let dd = gtk::DropDown::from_strings(&labels);
                let selected = EqualizerBarShape::all()
                    .iter()
                    .position(|s| *s == inst.bar_shape)
                    .unwrap_or(0) as u32;
                dd.set_selected(selected);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    dd.connect_selected_notify(move |dd| {
                        let shape = EqualizerBarShape::all()
                            .get(dd.selected() as usize)
                            .copied()
                            .unwrap_or(EqualizerBarShape::Segmented);
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.bar_shape = shape;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "view-list-bullet-symbolic",
                    &tr("Bar shape"),
                    &dd,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.bar_gradient);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.bar_gradient = on;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "color-select-symbolic",
                    &tr("Bar height gradient"),
                    &sw,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.show_peaks);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    let rebuild_body = rebuild_body.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.show_peaks = on;
                            }
                        });
                        if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                            rebuild();
                        }
                    });
                }
                col.append(&ui::row_with_icon("go-top-symbolic", &tr("Peak caps"), &sw));
            }
            if inst.show_peaks {
                let color = color_dialog_button();
                color.set_rgba(&hex_to_rgba(&inst.peak_color));
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    color.connect_rgba_notify(move |btn| {
                        let hex = rgba_to_hex(&btn.rgba());
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.peak_color = hex;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "color-select-symbolic",
                    &tr("Peak colour"),
                    &color,
                ));
            }
            {
                let sw = gtk::Switch::new();
                sw.set_active(inst.show_reflection);
                sw.set_halign(gtk::Align::End);
                let id = inst.id.clone();
                {
                    let cfg = cfg.clone();
                    sw.connect_active_notify(move |sw| {
                        let on = sw.is_active();
                        mutate_from_disk(&cfg, |disk| {
                            if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                                inst.show_reflection = on;
                            }
                        });
                    });
                }
                col.append(&ui::row_with_icon(
                    "object-flip-vertical-symbolic",
                    &tr("Bar reflection"),
                    &sw,
                ));
            }
        }
        EqualizerVizStyle::SpectrumLines => {
            let hint = gtk::Label::new(Some(&tr(
                "Spectrum lines use the colour mode above across the frequency range.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
        EqualizerVizStyle::NeonWave => {
            let sw = gtk::Switch::new();
            sw.set_active(inst.show_reflection);
            sw.set_halign(gtk::Align::End);
            let id = inst.id.clone();
            {
                let cfg = cfg.clone();
                sw.connect_active_notify(move |sw| {
                    let on = sw.is_active();
                    mutate_from_disk(&cfg, |disk| {
                        if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                            inst.show_reflection = on;
                        }
                    });
                });
            }
            col.append(&ui::row_with_icon(
                "object-flip-vertical-symbolic",
                &tr("Mirror wave"),
                &sw,
            ));
        }
        EqualizerVizStyle::Radial => {
            let hint = gtk::Label::new(Some(&tr(
                "Rays radiate from the centre. Colour mode tints around the ring.",
            )));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("metis-settings-hint");
            col.append(&hint);
        }
    }

    let hint = gtk::Label::new(Some(&tr(
        "Listens to the default audio output (PipeWire/Pulse monitor). \
         Play music or a movie — silent sinks show a quiet idle decay.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    col.append(&hint);

    col.upcast()
}

pub(crate) fn text_style_options(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.append(&font_row(inst, cfg.clone()));

    // Text colour
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let use_theme = gtk::CheckButton::with_label(&tr("Theme colour"));
        use_theme.set_active(inst.text_color.trim().is_empty());
        let color = color_dialog_button();
        if !inst.text_color.trim().is_empty() {
            color.set_rgba(&hex_to_rgba(&inst.text_color));
        }
        color.set_sensitive(!inst.text_color.trim().is_empty());
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let color = color.clone();
            let rebuild_body = rebuild_body.clone();
            use_theme.connect_toggled(move |btn| {
                let theme = btn.is_active();
                color.set_sensitive(!theme);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.text_color = if theme {
                            String::new()
                        } else {
                            rgba_to_hex(&color.rgba())
                        };
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let use_theme = use_theme.clone();
            color.connect_rgba_notify(move |btn| {
                if use_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.text_color = hex;
                    }
                });
            });
        }
        row.append(&use_theme);
        row.append(color.upcast_ref());
        col.append(&ui::row_with_icon(
            "color-select-symbolic",
            &tr("Text colour"),
            &row,
        ));
    }

    // Accent (progress bars on System; optional highlight elsewhere).
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let use_theme = gtk::CheckButton::with_label(&tr("Theme accent"));
        use_theme.set_active(inst.accent_color.trim().is_empty());
        let color = color_dialog_button();
        if !inst.accent_color.trim().is_empty() {
            color.set_rgba(&hex_to_rgba(&inst.accent_color));
        }
        color.set_sensitive(!inst.accent_color.trim().is_empty());
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let color = color.clone();
            let rebuild_body = rebuild_body.clone();
            use_theme.connect_toggled(move |btn| {
                let theme = btn.is_active();
                color.set_sensitive(!theme);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.accent_color = if theme {
                            String::new()
                        } else {
                            rgba_to_hex(&color.rgba())
                        };
                    }
                });
                if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                    rebuild();
                }
            });
        }
        {
            let cfg = cfg.clone();
            let id = inst.id.clone();
            let use_theme = use_theme.clone();
            color.connect_rgba_notify(move |btn| {
                if use_theme.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.accent_color = hex;
                    }
                });
            });
        }
        row.append(&use_theme);
        row.append(color.upcast_ref());
        let accent_label = match inst.kind {
            DesktopWidgetKind::System => "Bar accent",
            _ => "Accent colour",
        };
        col.append(&ui::row_with_icon(
            "preferences-color-symbolic",
            accent_label,
            &row,
        ));
    }

    let hint = gtk::Label::new(Some(&tr(
        "Font picks family, weight, and size. Text colour tints labels and icons; \
         accent colours the System progress fills.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    col.append(&hint);

    col.upcast()
}

pub(crate) fn font_row(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let use_theme = gtk::CheckButton::with_label(&tr("Theme font"));
    use_theme.set_active(inst.font.trim().is_empty());

    let font_btn = font_picker_button();
    if !inst.font.trim().is_empty() {
        font_btn.set_font_desc(&gtk::pango::FontDescription::from_string(&inst.font));
    }
    font_btn.set_sensitive(!inst.font.trim().is_empty());

    {
        let cfg = cfg.clone();
        let id = inst.id.clone();
        let font_btn = font_btn.clone();
        use_theme.connect_toggled(move |btn| {
            let theme = btn.is_active();
            font_btn.set_sensitive(!theme);
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    if theme {
                        inst.font.clear();
                    } else if inst.font.trim().is_empty() {
                        let desc = font_btn.font_desc().unwrap_or_default();
                        inst.font = desc.to_string();
                    }
                }
            });
        });
    }
    {
        let cfg = cfg.clone();
        let id = inst.id.clone();
        let use_theme = use_theme.clone();
        font_btn.connect_font_desc_notify(move |btn| {
            if use_theme.is_active() {
                return;
            }
            let desc = btn.font_desc().unwrap_or_default();
            let font = desc.to_string();
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.font = font;
                }
            });
        });
    }

    row.append(&use_theme);
    row.append(font_btn.upcast_ref());
    ui::row_with_icon("font-x-generic-symbolic", &tr("Font"), &row).upcast()
}

pub(crate) fn view_mode_row(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
) -> gtk::Widget {
    let labels: Vec<&str> = DesktopWidgetView::all().iter().map(|v| v.label()).collect();
    let dd = gtk::DropDown::from_strings(&labels);
    let selected = DesktopWidgetView::all()
        .iter()
        .position(|v| *v == inst.view)
        .unwrap_or(0) as u32;
    dd.set_selected(selected);

    let id = inst.id.clone();
    {
        let cfg = cfg.clone();
        dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let view = DesktopWidgetView::all()
                .get(idx)
                .copied()
                .unwrap_or(DesktopWidgetView::Grid);
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.view = view;
                }
            });
        });
    }

    ui::row_with_icon("view-grid-symbolic", &tr("View"), &dd).upcast()
}

pub(crate) fn instance_chrome_overrides(
    inst: &DesktopWidgetInstance,
    cfg: Rc<RefCell<DesktopWidgetsConfig>>,
    chrome_debounce: Rc<RefCell<Option<glib::SourceId>>>,
    rebuild_body: OptFn0Cell,
) -> gtk::Widget {
    let expander = gtk::Expander::new(Some(&tr("Look overrides (optional)")));
    expander.set_expanded(!inst.chrome.is_empty());
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.set_margin_top(6);
    expander.set_child(Some(&body));

    let id = inst.id.clone();

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Background opacity"));
        enable.set_active(inst.chrome.background_opacity.is_some());
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        scale.set_value(inst.chrome.background_opacity.unwrap_or(0.4) as f64);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_digits(2);
        scale.set_sensitive(inst.chrome.background_opacity.is_some());
        ui::forward_wheel_to_page_scroller(&scale);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let scale = scale.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                scale.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_opacity =
                            if on { Some(scale.value() as f32) } else { None };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                if !enable.is_active() {
                    return;
                }
                let v = s.value() as f32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_opacity = Some(v.clamp(0.0, 1.0));
                    }
                });
            });
        }
        row.append(&enable);
        row.append(&scale);
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Background colour"));
        let has = inst
            .chrome
            .background_color
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        enable.set_active(has);
        let color = color_dialog_button();
        if let Some(hex) = inst
            .chrome
            .background_color
            .as_ref()
            .filter(|c| !c.is_empty())
        {
            color.set_rgba(&hex_to_rgba(hex));
        }
        color.set_sensitive(has);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let color = color.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                color.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_color = if on {
                            Some(rgba_to_hex(&color.rgba()))
                        } else {
                            None
                        };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            color.connect_rgba_notify(move |btn| {
                if !enable.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.background_color = Some(hex);
                    }
                });
            });
        }
        row.append(&enable);
        row.append(color.upcast_ref());
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Border width"));
        enable.set_active(inst.chrome.border_width.is_some());
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 12.0, 0.5);
        scale.set_value(inst.chrome.border_width.unwrap_or(1.0) as f64);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_digits(1);
        scale.set_sensitive(inst.chrome.border_width.is_some());
        ui::forward_wheel_to_page_scroller(&scale);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let scale = scale.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                scale.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_width =
                            if on { Some(scale.value() as f32) } else { None };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            let chrome_debounce = chrome_debounce.clone();
            scale.connect_value_changed(move |s| {
                if !enable.is_active() {
                    return;
                }
                let v = s.value() as f32;
                let id = id.clone();
                mutate_from_disk_debounced(&cfg, &chrome_debounce, move |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_width = Some(v.clamp(0.0, 12.0));
                    }
                });
            });
        }
        row.append(&enable);
        row.append(&scale);
        body.append(&row);
    }

    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let enable = gtk::CheckButton::with_label(&tr("Border colour"));
        let has = inst
            .chrome
            .border_color
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        enable.set_active(has);
        let color = color_dialog_button();
        if let Some(hex) = inst.chrome.border_color.as_ref().filter(|c| !c.is_empty()) {
            color.set_rgba(&hex_to_rgba(hex));
        }
        color.set_sensitive(has);
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let color = color.clone();
            enable.connect_toggled(move |btn| {
                let on = btn.is_active();
                color.set_sensitive(on);
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_color = if on {
                            Some(rgba_to_hex(&color.rgba()))
                        } else {
                            None
                        };
                    }
                });
            });
        }
        {
            let cfg = cfg.clone();
            let id = id.clone();
            let enable = enable.clone();
            color.connect_rgba_notify(move |btn| {
                if !enable.is_active() {
                    return;
                }
                let hex = rgba_to_hex(&btn.rgba());
                mutate_from_disk(&cfg, |disk| {
                    if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                        inst.chrome.border_color = Some(hex);
                    }
                });
            });
        }
        row.append(&enable);
        row.append(color.upcast_ref());
        body.append(&row);
    }

    let clear = gtk::Button::with_label(&tr("Clear all overrides"));
    {
        let cfg = cfg.clone();
        let id = id.clone();
        let rebuild_body = rebuild_body.clone();
        clear.connect_clicked(move |_| {
            mutate_from_disk(&cfg, |disk| {
                if let Some(inst) = disk.instances.iter_mut().find(|i| i.id == id) {
                    inst.chrome = DesktopWidgetChromeOverride::default();
                }
            });
            if let Some(rebuild) = rebuild_body.borrow().as_ref() {
                rebuild();
            }
        });
    }
    body.append(&clear);

    expander.upcast()
}
