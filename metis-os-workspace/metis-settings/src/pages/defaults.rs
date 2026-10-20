//! Settings → System → Default applications.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use metis_config::{DefaultAppRole, candidate_apps_for_role, query_default_app, set_default_app};
use metis_i18n::tr;

use crate::runtime;
use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("defaults");

    let (card, body) = ui::section_with_icon(
        &tr("Default applications"),
        "preferences-desktop-default-applications-symbolic",
    );

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("metis-settings-hint");

    for role in DefaultAppRole::ALL {
        body.append(&role_row(*role, status.clone()));
    }

    body.append(&ui::hint(&tr(
        "Writes FreeDesktop defaults (mimeapps.list via xdg-mime / xdg-settings). \
         Terminal and File manager also update Metis Menu quick launchers.",
    )));
    body.append(&status);
    content.append(&card);

    scroller.upcast()
}

fn role_row(role: DefaultAppRole, status: gtk::Label) -> gtk::Widget {
    let candidates = Rc::new(candidate_apps_for_role(role));
    let labels: Vec<String> = {
        let mut v = vec![tr("— Not set —")];
        v.extend(candidates.iter().map(|(_, name)| name.clone()));
        v
    };
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dd = gtk::DropDown::from_strings(&label_refs);

    let current = query_default_app(role);
    let selected = current
        .as_deref()
        .and_then(|cur| {
            candidates
                .iter()
                .position(|(id, _)| id.eq_ignore_ascii_case(cur))
                .map(|i| i + 1)
        })
        .unwrap_or(0);
    let armed = Rc::new(Cell::new(false));
    // Closures hold `labels` so the DropDown string model stays alive.
    let labels_keep = labels;
    {
        let candidates = candidates.clone();
        let status = status.clone();
        let armed = armed.clone();
        dd.connect_selected_notify(move |dd| {
            let _ = &labels_keep;
            if !armed.get() {
                return;
            }
            let idx = dd.selected() as usize;
            if idx == 0 {
                status.set_text(&tr("Choose an application to set as default"));
                return;
            }
            let Some((id, name)) = candidates.get(idx - 1) else {
                return;
            };
            match set_default_app(role, id) {
                Ok(()) => {
                    status.set_text(&format!("{} → {name}", tr(role.label())));
                    runtime::send("reload-bar");
                }
                Err(err) => status.set_text(&err),
            }
        });
    }
    dd.set_selected(selected as u32);
    armed.set(true);
    ui::row_with_icon(role.icon(), &tr(role.label()), &dd).upcast()
}
