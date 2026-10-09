//! Shared Start-menu chrome: headers, rail, list/pinned population, and rows.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::services::{AppEntry, applications};

use super::pin::PinContext;

pub(crate) const APP_ICON_SIZE: i32 = 24;
pub(crate) const PIN_ICON_SIZE: i32 = 34;
pub(crate) const GRID_ICON_SIZE: i32 = 40;
pub(crate) const CHIP_ICON_SIZE: i32 = 28;
pub(crate) const FREQUENT_LIMIT: usize = 8;
/// Alphabetical rows appended per idle slice so opening the menu never blocks the
/// GTK main loop (and the nested compositor Wayland socket) for one giant rebuild.
pub(crate) const MENU_ALPHA_CHUNK: usize = 32;

pub(crate) fn build_user_header(
    cfg: &metis_config::MenuConfig,
) -> (gtk::Box, gtk::Image, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-menu-user");
    row.set_hexpand(true);

    let avatar = gtk::Image::new();
    avatar.add_css_class("metis-menu-user-avatar");
    avatar.set_pixel_size(40);
    let name = gtk::Label::builder()
        .label(cfg.resolved_display_name())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    name.add_css_class("metis-menu-user-name");
    refresh_user_header(&avatar, &name);
    row.append(&avatar);
    row.append(&name);

    (row, avatar, name)
}

pub(crate) fn refresh_user_header(avatar: &gtk::Image, name: &gtk::Label) {
    let cfg = metis_config::load_menu_config();
    name.set_label(&cfg.resolved_display_name());
    // Clear then reload so GTK does not keep a stale paintable when ~/.face
    // is overwritten in place.
    avatar.clear();
    if let Some(path) = cfg.resolved_avatar_path() {
        avatar.set_from_file(Some(path));
    } else {
        avatar.set_icon_name(Some("avatar-default-symbolic"));
    }
}

pub(crate) fn build_rail(overlay: &gtk::Overlay, tip: &gtk::Label) -> gtk::Box {
    use metis_i18n::tr;

    let rail = gtk::Box::new(gtk::Orientation::Vertical, 6);
    rail.add_css_class("metis-menu-rail");

    rail.append(&rail_button(
        overlay,
        tip,
        "system-file-manager-symbolic",
        &tr("Files"),
        || launch_quick_action_argv(launch_file_manager_argv()),
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "utilities-terminal-symbolic",
        &tr("Terminal"),
        || launch_quick_action_argv(launch_terminal_argv().unwrap_or_default()),
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "preferences-system-symbolic",
        &tr("Settings"),
        || {
            crate::ui::bar::dropdown::close_all();
            activate_or_launch_settings();
        },
    ));

    // Controller-friendly Steam Big Picture, shown only when Steam is installed
    // (native on PATH or Flatpak). Absent entirely on non-gaming setups.
    if let Some(cmd) = applications::steam_big_picture_command() {
        rail.append(&rail_button(
            overlay,
            tip,
            "input-gaming-symbolic",
            &tr("Big Picture"),
            move || {
                launch_quick_action_argv(metis_protocol::split_command_line(&cmd));
            },
        ));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    rail.append(&spacer);

    rail.append(&rail_button(
        overlay,
        tip,
        "system-lock-screen-symbolic",
        &tr("Lock"),
        || {
            if let Err(err) = crate::compositor::lock_session() {
                tracing::warn!(%err, "compositor lock_session failed");
            }
            crate::ui::bar::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "weather-clear-night-symbolic",
        &tr("Suspend"),
        || {
            run_detached("systemctl", &["suspend"]);
            crate::ui::bar::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-log-out-symbolic",
        &tr("Log Out"),
        || {
            if let Err(err) = crate::compositor::end_session() {
                tracing::warn!(%err, "compositor end_session failed — falling back to loginctl");
                if std::env::var_os("XDG_SESSION_ID").is_some() {
                    run_detached("loginctl", &["terminate-session", "self"]);
                } else if let Ok(user) = std::env::var("USER") {
                    run_detached("loginctl", &["terminate-user", &user]);
                }
            }
            crate::ui::bar::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-reboot-symbolic",
        &tr("Restart"),
        || {
            run_detached("systemctl", &["reboot"]);
            crate::ui::bar::dropdown::request_close_all();
        },
    ));
    rail.append(&rail_button(
        overlay,
        tip,
        "system-shutdown-symbolic",
        &tr("Shut Down"),
        || {
            run_detached("systemctl", &["poweroff"]);
            crate::ui::bar::dropdown::request_close_all();
        },
    ));

    rail
}

pub(crate) fn activate_or_launch_settings() {
    const SETTINGS_APP_ID: &str = "com.metis.Settings";
    let existing = match crate::compositor::list_windows() {
        Ok(windows) => windows
            .into_iter()
            .filter(|window| {
                window
                    .app_id
                    .as_deref()
                    .is_some_and(|app_id| app_id.eq_ignore_ascii_case(SETTINGS_APP_ID))
            })
            .min_by_key(|window| (!window.focused, window.id)),
        Err(err) => {
            tracing::debug!(%err, "could not query open windows before launching Settings");
            None
        }
    };

    if let Some(window) = existing {
        match crate::compositor::activate_window(window.id) {
            Ok(()) => return,
            Err(err) => {
                tracing::warn!(%err, id = window.id, "failed to restore existing Metis Settings");
            }
        }
    }
    if let Err(err) = crate::compositor::launch_program("metis-settings") {
        tracing::warn!(%err, "failed to launch metis-settings");
    }
}

pub(crate) fn rail_button(
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
    icon: &str,
    label: &str,
    on_click: impl Fn() + 'static,
) -> gtk::Button {
    let btn = gtk::Button::builder().has_frame(false).build();
    btn.add_css_class("metis-menu-rail-btn");
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(18);
    btn.set_child(Some(&image));
    btn.connect_clicked(move |_| on_click());
    attach_tooltip(&btn, label, overlay, tip);
    btn
}

/// Tooltip for an icon-only rail control.
///
/// GTK's built-in tooltips don't behave on this non-autohide, grab-less
/// layer-shell popover (and in the nested session they now present as a separate
/// window the compositor stacks *behind* the translucent menu), so we drive our
/// own using a single shared `Label` living in the menu's `GtkOverlay`. Because it
/// is part of the menu's own surface it always paints on top of the panel; we just
/// move it next to the hovered button after a short delay. The accessible label is
/// set directly so screen readers still get the name.
pub(crate) fn attach_tooltip(
    widget: &impl IsA<gtk::Widget>,
    text: &str,
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
) {
    widget
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(text)]);

    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let motion = gtk::EventControllerMotion::new();
    {
        let widget_weak = widget.clone().upcast::<gtk::Widget>().downgrade();
        let overlay_weak = overlay.downgrade();
        let tip = tip.clone();
        let text = text.to_string();
        let timer = timer.clone();
        motion.connect_enter(move |_, _, _| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            let widget_weak = widget_weak.clone();
            let overlay_weak = overlay_weak.clone();
            let tip = tip.clone();
            let text = text.clone();
            let timer_inner = timer.clone();
            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
                    *timer_inner.borrow_mut() = None;
                    let (Some(w), Some(ov)) = (widget_weak.upgrade(), overlay_weak.upgrade())
                    else {
                        return;
                    };
                    tip.set_label(&text);
                    // Position the tooltip just to the right of the button, vertically
                    // centered, in the overlay's coordinate space.
                    let anchor =
                        gtk::graphene::Point::new(w.width() as f32, w.height() as f32 / 2.0);
                    if let Some(p) = w.compute_point(&ov, &anchor) {
                        tip.set_margin_start((p.x() as i32 + 8).max(0));
                        tip.set_margin_top((p.y() as i32 - 14).max(0));
                    }
                    tip.set_visible(true);
                });
            *timer.borrow_mut() = Some(id);
        });
    }
    {
        let tip = tip.clone();
        let timer = timer.clone();
        motion.connect_leave(move |_| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            tip.set_visible(false);
        });
    }
    widget.add_controller(motion);
}

pub(crate) fn restore_search_focus(search: &gtk::SearchEntry, had_focus: bool) {
    if !had_focus {
        return;
    }
    let search = search.clone();
    glib::idle_add_local_once(move || {
        if !search.has_focus() {
            search.grab_focus();
        }
    });
}

pub(crate) fn populate_center(
    container: &gtk::Box,
    header: &gtk::Label,
    query: &str,
    apps: &[AppEntry],
    search: &gtk::SearchEntry,
    list_generation: &Rc<Cell<u64>>,
    pin_ctx: &Rc<PinContext>,
) {
    let generation = list_generation.get().wrapping_add(1);
    list_generation.set(generation);
    let keep_search_focus = search.has_focus();
    clear_box(container);
    let q = query.trim();
    if q.is_empty() {
        header.set_text(&metis_i18n::tr("Frequent Apps"));
        for entry in applications::frequent_from(apps, FREQUENT_LIMIT) {
            container.append(&app_row(&entry, pin_ctx));
        }
        append_alpha_chunk(
            container,
            apps,
            list_generation,
            generation,
            0,
            '\0',
            pin_ctx,
        );
    } else {
        header.set_text(&metis_i18n::tr("Search Results"));
        let results = applications::search_in(apps, q);
        if results.is_empty() {
            let empty = gtk::Label::new(Some(&metis_i18n::tr("No matching applications")));
            empty.add_css_class("metis-menu-empty");
            empty.set_halign(gtk::Align::Start);
            container.append(&empty);
        }
        for entry in results {
            container.append(&app_row(&entry, pin_ctx));
        }
    }
    restore_search_focus(search, keep_search_focus);
}

/// Append a slice of the alphabetical app list, scheduling the rest on idle.
pub(crate) fn append_alpha_chunk(
    container: &gtk::Box,
    apps: &[AppEntry],
    list_generation: &Rc<Cell<u64>>,
    generation: u64,
    start: usize,
    mut last_letter: char,
    pin_ctx: &Rc<PinContext>,
) {
    if list_generation.get() != generation {
        return;
    }
    if start >= apps.len() {
        return;
    }
    let end = (start + MENU_ALPHA_CHUNK).min(apps.len());
    for entry in &apps[start..end] {
        let letter = entry
            .name
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('#');
        if letter != last_letter {
            last_letter = letter;
            container.append(&letter_header(letter));
        }
        container.append(&app_row(entry, pin_ctx));
    }
    if end < apps.len() {
        let container = container.clone();
        let apps = apps.to_vec();
        let list_generation = list_generation.clone();
        let pin_ctx = pin_ctx.clone();
        glib::idle_add_local_once(move || {
            append_alpha_chunk(
                &container,
                &apps,
                &list_generation,
                generation,
                end,
                last_letter,
                &pin_ctx,
            );
        });
    }
}

pub(crate) fn populate_pinned(
    flow: &gtk::FlowBox,
    hint: &gtk::Label,
    apps: &[AppEntry],
    pin_ctx: &Rc<PinContext>,
) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    let pinned = applications::pinned_entries_from(apps);
    if pinned.is_empty() {
        hint.set_visible(true);
        flow.set_visible(false);
        return;
    }
    hint.set_visible(false);
    flow.set_visible(true);
    for entry in pinned {
        flow.append(&pinned_tile(&entry, pin_ctx));
    }
}

/// Drive a scrolled window's vertical adjustment from wheel events. Attached in
/// Capture phase on the `ScrolledWindow` so it intercepts the whole subtree
/// before child row buttons (which otherwise swallow scroll) and covers the blank
/// gutter beside row labels.
pub(crate) fn wire_vertical_scroll(widget: &impl IsA<gtk::Widget>, scroll: &gtk::ScrolledWindow) {
    let ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let vadj = scroll.vadjustment();
    ctrl.connect_scroll(move |_, _, dy| {
        let page = vadj.page_size();
        let upper = vadj.upper();
        let lower = vadj.lower();
        if upper - lower <= page {
            return glib::Propagation::Proceed;
        }
        let max = (upper - page).max(lower);
        let new_val = (vadj.value() + dy).clamp(lower, max);
        if (new_val - vadj.value()).abs() > f64::EPSILON {
            vadj.set_value(new_val);
        }
        glib::Propagation::Stop
    });
    widget.add_controller(ctrl);
}

pub(crate) fn app_row(entry: &AppEntry, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let row = gtk::Button::builder().has_frame(false).build();
    row.add_css_class("metis-menu-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    hbox.set_hexpand(true);
    hbox.append(&app_image(entry, APP_ICON_SIZE));

    let label = gtk::Label::new(Some(&entry.name));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    hbox.append(&label);
    row.set_child(Some(&hbox));

    {
        let entry = entry.clone();
        row.connect_clicked(move |_| {
            // Close synchronously *before* launching: the new window grabs focus,
            // which otherwise swallows the deferred (idle) popdown and leaves the
            // menu hanging open over the app.
            crate::ui::bar::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&row, &entry.id, pin_ctx);
    row
}

pub(crate) fn pinned_tile(entry: &AppEntry, pin_ctx: &Rc<PinContext>) -> gtk::Button {
    let tile = gtk::Button::builder().has_frame(false).build();
    tile.add_css_class("metis-menu-tile");

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_halign(gtk::Align::Center);
    vbox.append(&app_image(entry, PIN_ICON_SIZE));

    let label = gtk::Label::new(Some(&entry.name));
    label.add_css_class("metis-menu-tile-label");
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(10);
    vbox.append(&label);
    tile.set_child(Some(&vbox));

    {
        let entry = entry.clone();
        tile.connect_clicked(move |_| {
            // See `app_row`: pop down before the launched window steals focus.
            crate::ui::bar::dropdown::close_all();
            applications::launch(&entry);
        });
    }
    attach_pin_target(&tile, &entry.id, pin_ctx);
    tile
}

/// Register a row/tile so the overlay-level secondary-click handler can resolve
/// which app was under the cursor (coordinates stay in overlay space).
pub(crate) fn attach_pin_target(
    widget: &impl IsA<gtk::Widget>,
    id: &str,
    pin_ctx: &Rc<PinContext>,
) {
    pin_ctx.register_target(widget, id);
}

pub(crate) fn app_image(entry: &AppEntry, size: i32) -> gtk::Image {
    let image = gtk::Image::new();
    applications::set_app_icon(&image, entry, size);
    image
}

pub(crate) fn letter_header(letter: char) -> gtk::Label {
    let label = gtk::Label::new(Some(&letter.to_string()));
    label.add_css_class("metis-menu-letter");
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label
}

pub(crate) fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Escape a value for safe interpolation inside a double-quoted shell word.
pub(crate) fn launch_terminal_argv() -> Option<Vec<String>> {
    metis_config::resolve_terminal().map(|t| vec![t])
}

pub(crate) fn launch_file_manager_argv() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    if let Some(fm) = metis_config::resolve_file_manager() {
        vec![fm, home]
    } else {
        vec!["xdg-open".into(), home]
    }
}

pub(crate) fn launch_quick_action_argv(argv: Vec<String>) {
    // Close before the launched window grabs focus (see `app_row`).
    crate::ui::bar::dropdown::close_all();
    if argv.is_empty() {
        tracing::warn!("quick action: no executable resolved");
        return;
    }
    if let Err(err) = crate::compositor::launch_argv(argv) {
        tracing::warn!(%err, "failed to launch quick action");
    }
}

pub(crate) fn run_detached(cmd: &str, args: &[&str]) {
    if let Err(err) = std::process::Command::new(cmd).args(args).spawn() {
        tracing::warn!(%err, cmd, "failed to spawn power action");
    }
}
