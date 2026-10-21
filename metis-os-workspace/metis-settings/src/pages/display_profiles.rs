//! Dock / office display topology profiles (Phase 20 §C).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use metis_config::{
    OutputProfilesConfig, OutputTopologyProfile, OutputsConfig, load_output_profiles_config,
    load_outputs_config, materialize_to_outputs, output_identity, profile_from_live,
    save_output_profiles_config, topology_fingerprint, unix_now_secs,
};
use metis_i18n::tr;
use metis_protocol::OutputInfo;

use crate::dialog::{self, ConfirmButtons};
use crate::runtime;
use crate::ui;

use super::display_arrangement::ArrangementCanvas;
use super::display_confirm;

type RefreshFn = Rc<dyn Fn()>;
type RefreshSlot = Rc<RefCell<Option<RefreshFn>>>;

struct ProfileUiCtx {
    parent: gtk::Window,
    cfg: Rc<RefCell<OutputsConfig>>,
    outputs: Rc<RefCell<Vec<OutputInfo>>>,
    canvas_slot: Rc<RefCell<Option<Rc<ArrangementCanvas>>>>,
    profiles: Rc<RefCell<OutputProfilesConfig>>,
    rebuild_detail: RefreshFn,
    refresh_save_buttons: RefreshFn,
    refresh: RefreshFn,
}

/// Build the Display setups section. Call returned `refresh` after Detect / hotplug.
pub fn build_section(
    parent: &gtk::Window,
    cfg: Rc<RefCell<OutputsConfig>>,
    outputs: Rc<RefCell<Vec<OutputInfo>>>,
    canvas_slot: Rc<RefCell<Option<Rc<ArrangementCanvas>>>>,
    rebuild_detail: Rc<dyn Fn()>,
    refresh_save_buttons: Rc<dyn Fn()>,
) -> (gtk::Widget, Rc<dyn Fn()>) {
    let (card, body) = ui::section(&tr("Display setups"));
    let hint = gtk::Label::new(Some(&tr(
        "Save the current monitor set (laptop vs dock) by EDID identity. \
         Matching setups apply automatically when you plug or unplug displays — \
         no keep/revert prompt. Restore from this list still asks you to confirm.",
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-settings-hint");
    body.append(&hint);

    let active_chip = gtk::Label::new(None);
    active_chip.set_xalign(0.0);
    active_chip.add_css_class("metis-settings-hint");
    active_chip.set_margin_top(4);
    body.append(&active_chip);

    let list_host = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list_host.add_css_class("metis-settings-list");
    list_host.add_css_class("metis-settings-zebra-list");
    body.append(&list_host);

    let save_as_btn = gtk::Button::with_label(&tr("Save this setup as…"));
    save_as_btn.add_css_class("metis-settings-secondary");
    save_as_btn.set_halign(gtk::Align::Start);
    save_as_btn.set_margin_top(8);
    body.append(&save_as_btn);

    let profiles = Rc::new(RefCell::new(load_output_profiles_config()));
    let refresh_slot: RefreshSlot = Rc::new(RefCell::new(None));

    let refresh: RefreshFn = {
        let profiles = profiles.clone();
        let outputs = outputs.clone();
        let list_host = list_host.clone();
        let active_chip = active_chip.clone();
        let parent = parent.clone();
        let cfg = cfg.clone();
        let canvas_slot = canvas_slot.clone();
        let rebuild_detail = rebuild_detail.clone();
        let refresh_save_buttons = refresh_save_buttons.clone();
        let refresh_slot = refresh_slot.clone();
        Rc::new(move || {
            *profiles.borrow_mut() = load_output_profiles_config();
            while let Some(child) = list_host.first_child() {
                list_host.remove(&child);
            }
            let connected = connected_pairs(&outputs.borrow());
            let ids: Vec<String> = connected.iter().map(|(_, id)| id.clone()).collect();
            let fp = topology_fingerprint(ids.iter().map(|s| s.as_str()));
            let store = profiles.borrow();

            let active_name = store
                .find_matching_profile(&ids)
                .map(|p| p.name.clone())
                .or_else(|| {
                    store
                        .last_applied_id
                        .as_ref()
                        .and_then(|id| store.find_by_id(id).map(|p| p.name.clone()))
                });
            if let Some(name) = active_name {
                active_chip.set_text(&format!("{}: {name}", tr("Active setup")));
            } else if store.profiles.is_empty() {
                active_chip.set_text(&tr("No saved setups yet."));
            } else {
                active_chip.set_text(&tr("No saved setup matches the displays connected now."));
            }

            let refresh_cb = refresh_slot
                .borrow()
                .clone()
                .unwrap_or_else(|| Rc::new(|| {}) as RefreshFn);

            let ctx = ProfileUiCtx {
                parent: parent.clone(),
                cfg: cfg.clone(),
                outputs: outputs.clone(),
                canvas_slot: canvas_slot.clone(),
                profiles: profiles.clone(),
                rebuild_detail: rebuild_detail.clone(),
                refresh_save_buttons: refresh_save_buttons.clone(),
                refresh: refresh_cb,
            };
            for (i, profile) in store.profiles.iter().enumerate() {
                list_host.append(&build_profile_row(profile, &fp, &ctx, i % 2 == 1));
            }
        })
    };
    *refresh_slot.borrow_mut() = Some(refresh.clone());

    {
        let refresh = refresh.clone();
        let parent = parent.clone();
        let cfg = cfg.clone();
        let outputs = outputs.clone();
        let profiles = profiles.clone();
        save_as_btn.connect_clicked(move |_| {
            prompt_save_as(&parent, &cfg, &outputs, &profiles, refresh.clone());
        });
    }

    refresh();
    (card.upcast(), refresh)
}

fn connected_pairs(list: &[OutputInfo]) -> Vec<(String, String)> {
    list.iter()
        .map(|o| {
            let id = output_identity(&o.make, &o.model, &o.serial, &o.name);
            (o.name.clone(), id)
        })
        .collect()
}

fn build_profile_row(
    profile: &OutputTopologyProfile,
    current_fp: &str,
    ctx: &ProfileUiCtx,
    alt: bool,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-settings-zebra-row");
    if alt {
        row.add_css_class("metis-settings-zebra-row-alt");
    }
    row.set_hexpand(true);

    let icon = gtk::Image::from_icon_name("video-display-symbolic");
    icon.set_pixel_size(16);
    icon.add_css_class("metis-settings-row-icon");
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let title = gtk::Label::new(Some(&profile.name));
    title.set_xalign(0.0);
    title.add_css_class("metis-settings-row-title");
    labels.append(&title);
    let matches = OutputProfilesConfig::fingerprint_for(profile) == current_fp;
    let sub = if matches {
        tr("Matches connected displays")
    } else if profile.monitors.len() == 1 {
        tr("1 display")
    } else {
        format!("{} {}", profile.monitors.len(), tr("displays"))
    };
    let sub_lbl = gtk::Label::new(Some(&sub));
    sub_lbl.set_xalign(0.0);
    sub_lbl.add_css_class("metis-settings-hint");
    labels.append(&sub_lbl);
    row.append(&labels);

    let restore = gtk::Button::with_label(&tr("Restore"));
    restore.add_css_class("metis-settings-secondary");
    {
        let ctx = ProfileUiCtx {
            parent: ctx.parent.clone(),
            cfg: ctx.cfg.clone(),
            outputs: ctx.outputs.clone(),
            canvas_slot: ctx.canvas_slot.clone(),
            profiles: ctx.profiles.clone(),
            rebuild_detail: ctx.rebuild_detail.clone(),
            refresh_save_buttons: ctx.refresh_save_buttons.clone(),
            refresh: ctx.refresh.clone(),
        };
        let profile = profile.clone();
        restore.connect_clicked(move |_| {
            restore_profile(&ctx, &profile);
        });
    }
    row.append(&restore);

    let delete = gtk::Button::with_label(&tr("Delete"));
    delete.add_css_class("metis-settings-secondary");
    {
        let profiles = ctx.profiles.clone();
        let id = profile.id.clone();
        let name = profile.name.clone();
        let refresh = ctx.refresh.clone();
        delete.connect_clicked(move |_| {
            let mut store = profiles.borrow_mut();
            if store.remove_by_id(&id)
                && let Err(err) = save_output_profiles_config(&store)
            {
                tracing::warn!(%err, profile = %name, "failed to delete display setup");
            }
            drop(store);
            refresh();
        });
    }
    row.append(&delete);
    row
}

fn prompt_save_as(
    _parent: &gtk::Window,
    cfg: &Rc<RefCell<OutputsConfig>>,
    outputs: &Rc<RefCell<Vec<OutputInfo>>>,
    profiles: &Rc<RefCell<OutputProfilesConfig>>,
    refresh: Rc<dyn Fn()>,
) {
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(&tr("e.g. Home dock")));
    entry.set_hexpand(true);
    ui::swallow_empty_backspace(&entry);

    let cfg = cfg.clone();
    let outputs = outputs.clone();
    let profiles = profiles.clone();
    let entry_for_accept = entry.clone();
    let on_accept = Rc::new(move || {
        let name = entry_for_accept.text().trim().to_string();
        if name.is_empty() {
            return;
        }
        let connected = connected_pairs(&outputs.borrow());
        if connected.is_empty() {
            return;
        }
        let live = cfg.borrow().clone();
        let mut profile = profile_from_live(name.clone(), &live, &connected);
        let mut store = profiles.borrow_mut();
        let fp = OutputProfilesConfig::fingerprint_for(&profile);
        if let Some(existing) = store
            .profiles
            .iter_mut()
            .find(|p| OutputProfilesConfig::fingerprint_for(p) == fp)
        {
            profile.id = existing.id.clone();
            profile.updated_at = unix_now_secs();
            profile.name = name;
            *existing = profile.clone();
            store.last_applied_id = Some(existing.id.clone());
        } else {
            let id = profile.id.clone();
            store.profiles.push(profile);
            store.last_applied_id = Some(id);
        }
        if let Err(err) = save_output_profiles_config(&store) {
            tracing::warn!(%err, "failed to save display setup");
        }
        drop(store);
        refresh();
    }) as Rc<dyn Fn()>;

    if !dialog::confirm_with_extra(
        &tr("Save display setup"),
        &tr(
            "Name this arrangement of monitors. It will apply automatically when the same set is connected.",
        ),
        Some(entry.upcast()),
        ConfirmButtons {
            cancel: tr("Cancel"),
            accept: tr("Save"),
            accept_destructive: false,
        },
        on_accept,
        Rc::new(|| {}) as Rc<dyn Fn()>,
    ) {
        tracing::warn!("display setup save dialog unavailable (sheet host missing)");
    }
}

fn restore_profile(ctx: &ProfileUiCtx, profile: &OutputTopologyProfile) {
    let canvas = match ctx.canvas_slot.borrow().clone() {
        Some(c) if !c.in_trial() => c,
        _ => {
            tracing::warn!("cannot restore display setup while a trial is active");
            return;
        }
    };

    let connected = connected_pairs(&ctx.outputs.borrow());
    let mut map: HashMap<String, String> = HashMap::new();
    for (connector, identity) in &connected {
        map.insert(identity.clone(), connector.clone());
    }
    let materialized = materialize_to_outputs(profile, &map, &ctx.cfg.borrow());
    if !canvas.begin_trial_with_config(materialized.clone()) {
        return;
    }
    *ctx.cfg.borrow_mut() = materialized;
    (ctx.refresh_save_buttons)();

    let on_keep = {
        let canvas = canvas.clone();
        let outputs = ctx.outputs.clone();
        let rebuild_detail = ctx.rebuild_detail.clone();
        let refresh_save_buttons = ctx.refresh_save_buttons.clone();
        let profiles = ctx.profiles.clone();
        let cfg = ctx.cfg.clone();
        let profile_id = profile.id.clone();
        Rc::new(move || {
            canvas.confirm_trial();
            let mut store = profiles.borrow_mut();
            store.last_applied_id = Some(profile_id.clone());
            let _ = save_output_profiles_config(&store);
            *outputs.borrow_mut() = runtime::list_outputs();
            *cfg.borrow_mut() = load_outputs_config();
            canvas.sync_positions();
            rebuild_detail();
            refresh_save_buttons();
        })
    };

    let on_revert = {
        let canvas = canvas.clone();
        let outputs = ctx.outputs.clone();
        let rebuild_detail = ctx.rebuild_detail.clone();
        let refresh_save_buttons = ctx.refresh_save_buttons.clone();
        let cfg = ctx.cfg.clone();
        Rc::new(move || {
            canvas.cancel_trial();
            runtime::reload_outputs_async();
            *outputs.borrow_mut() = runtime::list_outputs();
            *cfg.borrow_mut() = load_outputs_config();
            canvas.sync_positions();
            rebuild_detail();
            refresh_save_buttons();
        })
    };

    let parent = ctx.parent.clone();
    let canvas = canvas.clone();
    let outputs = ctx.outputs.clone();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        runtime::reload_outputs();
        let _ = tx.send(());
    });
    glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
        Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            *outputs.borrow_mut() = runtime::list_outputs();
            canvas.sync_positions();
            parent.queue_draw();
            display_confirm::show(&parent, on_keep.clone(), on_revert.clone());
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
    });
}
