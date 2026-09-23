//! Settings → System → Language & region.

use std::rc::Rc;

use gtk::prelude::*;

use metis_config::{LocaleConfig, load_locale_config, save_locale_config};
use metis_i18n::{self as i18n, tr};

use crate::runtime;
use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("locale");

    let (card, body) = ui::section(&tr("Language"));
    let choices = Rc::new(i18n::known_language_choices());
    let labels: Vec<String> = choices
        .iter()
        .map(|(tag, name)| {
            if tag.is_empty() {
                tr(name)
            } else {
                name.clone()
            }
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let lang_dd = gtk::DropDown::from_strings(&label_refs);

    let cfg = load_locale_config();
    let selected = match cfg.locale.as_deref() {
        None | Some("") => 0usize,
        Some(current) => choices
            .iter()
            .position(|(tag, _)| {
                !tag.is_empty()
                    && (tag == current
                        || current.starts_with(&format!("{tag}_"))
                        || tag.as_str() == current.split(['_', '-']).next().unwrap_or(""))
            })
            .unwrap_or(0),
    };
    lang_dd.set_selected(selected as u32);
    body.append(&ui::row(&tr("Language"), &lang_dd));

    let (formats_row, formats_sw) = ui::switch_row(&tr("Formats follow language"));
    formats_sw.set_active(cfg.formats_from_locale);
    body.append(&formats_row);

    let hint = ui::hint(&tr(
        "Applies across Metis Settings, the edge bar, Control Center, screenshots, notifications, onboarding, and the lock screen. Other apps follow the system locale.",
    ));
    body.append(&hint);

    let apply = gtk::Button::with_label(&tr("Apply"));
    apply.add_css_class("suggested-action");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_halign(gtk::Align::End);
    actions.append(&apply);
    body.append(&actions);

    content.append(&card);

    {
        let choices = choices.clone();
        let formats_sw = formats_sw.clone();
        let labels_keep = labels; // keep DropDown model strings alive
        apply.connect_clicked(move |btn| {
            let _ = &labels_keep;
            let idx = lang_dd.selected() as usize;
            let locale = choices.get(idx).and_then(|(tag, _)| {
                if tag.is_empty() {
                    None
                } else {
                    Some(tag.clone())
                }
            });
            let cfg = LocaleConfig {
                locale,
                formats_from_locale: formats_sw.is_active(),
            };
            if let Err(err) = save_locale_config(&cfg) {
                tracing::warn!(%err, "failed to save locale.json");
                return;
            }
            i18n::reload();
            crate::i18n_gtk::apply_gtk_direction();
            runtime::send("reload-locale");
            runtime::send_widgets("reload-locale");
            runtime::reload_locale_async();
            // Widgets keep construction-time labels — idle-rebuild Settings chrome
            // (never rebuild synchronously inside this click handler).
            crate::i18n_gtk::rebuild_ui_for_locale("locale");
            let _ = btn;
        });
    }

    scroller.upcast()
}
