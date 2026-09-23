//! Folders widget — files/folders (default `~/Desktop`), grid or list.
//!
//! - Directories open in the file manager
//! - `.desktop` entries show their app icon (name keeps the `.desktop` suffix)
//!   and launch the application
//! - Other files open via `xdg-open`
//! - Right-click: Open, Open with…, Rename, Delete, New Folder
//! - Sort: folders A–Z, then files A–Z

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::gio::prelude::{AppInfoExt, FileExt};
use gtk::glib;
use gtk::prelude::*;
use metis_config::{DesktopWidgetInstance, DesktopWidgetView};

const MAX_ENTRIES: usize = 120;
const TILE_ICON: i32 = 48;
const TILE_WIDTH: i32 = 96;
const LIST_ICON: i32 = 22;

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    root.set_vexpand(true);

    let path_label = gtk::Label::new(None);
    path_label.add_css_class("metis-dw-hint");
    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    root.append(&path_label);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();

    let path = expand_path(&inst.path);
    path_label.set_text(&display_path(&path));
    let path_rc = Rc::new(path.clone());

    let rebuild: Rc<dyn Fn()> = match inst.view {
        DesktopWidgetView::Grid => {
            let flow = gtk::FlowBox::builder()
                .valign(gtk::Align::Start)
                .max_children_per_line(8)
                .min_children_per_line(2)
                .selection_mode(gtk::SelectionMode::None)
                .homogeneous(true)
                .column_spacing(4)
                .row_spacing(4)
                .build();
            flow.add_css_class("metis-dw-folder-grid");
            scroll.set_child(Some(&flow));
            root.append(&scroll);
            attach_background_menu_flow(&flow, path_rc.clone());
            let flow_rc = flow.clone();
            let path_for = path_rc.clone();
            Rc::new(move || rebuild_grid(&flow_rc, &path_for))
        }
        DesktopWidgetView::List => {
            let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
            list.add_css_class("metis-dw-list");
            scroll.set_child(Some(&list));
            root.append(&scroll);
            attach_background_menu_box(&list, path_rc.clone());
            let list_rc = list.clone();
            let path_for = path_rc.clone();
            Rc::new(move || rebuild_list(&list_rc, &path_for))
        }
    };
    rebuild();

    let file = gio::File::for_path(&path);
    if let Ok(monitor) =
        file.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
    {
        let rebuild = rebuild.clone();
        monitor.connect_changed(move |_, _, _, _| {
            let rebuild = rebuild.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                rebuild();
            });
        });
        let keep = Rc::new(RefCell::new(Some(monitor)));
        root.connect_destroy(move |_| {
            let _ = keep.borrow_mut().take();
        });
    }

    root.upcast()
}

fn rebuild_grid(flow: &gtk::FlowBox, path: &Path) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }

    if !path.exists() {
        let empty = gtk::Label::new(Some(
            &metis_i18n::tr("Folder not found:\n%1").replace("%1", &path.display().to_string()),
        ));
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        empty.add_css_class("metis-dw-hint");
        flow.insert(&empty, -1);
        return;
    }

    let entries = match read_entries(path) {
        Ok(e) => e,
        Err(err) => {
            let empty = gtk::Label::new(Some(
                &metis_i18n::tr("Could not read folder:\n%1").replace("%1", &err.to_string()),
            ));
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            empty.add_css_class("metis-dw-hint");
            flow.insert(&empty, -1);
            return;
        }
    };

    if entries.is_empty() {
        let empty = gtk::Label::new(Some(&metis_i18n::tr(
            "This folder is empty.\nRight-click for New Folder.",
        )));
        empty.set_wrap(true);
        empty.set_xalign(0.5);
        empty.add_css_class("metis-dw-hint");
        flow.insert(&empty, -1);
        return;
    }

    let parent = path.to_path_buf();
    for (i, entry) in entries.iter().enumerate() {
        if i >= MAX_ENTRIES {
            let more = gtk::Label::new(Some(
                &metis_i18n::tr("…and %1 more")
                    .replace("%1", &entries.len().saturating_sub(MAX_ENTRIES).to_string()),
            ));
            more.add_css_class("metis-dw-hint");
            flow.insert(&more, -1);
            break;
        }
        flow.insert(&entry_tile(entry, &parent), -1);
    }
}

fn rebuild_list(list: &gtk::Box, path: &Path) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if !path.exists() {
        let empty = gtk::Label::new(Some(
            &metis_i18n::tr("Folder not found:\n%1").replace("%1", &path.display().to_string()),
        ));
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        empty.add_css_class("metis-dw-hint");
        list.append(&empty);
        return;
    }

    let entries = match read_entries(path) {
        Ok(e) => e,
        Err(err) => {
            let empty = gtk::Label::new(Some(
                &metis_i18n::tr("Could not read folder:\n%1").replace("%1", &err.to_string()),
            ));
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            empty.add_css_class("metis-dw-hint");
            list.append(&empty);
            return;
        }
    };

    if entries.is_empty() {
        let empty = gtk::Label::new(Some(&metis_i18n::tr(
            "This folder is empty.\nRight-click for New Folder.",
        )));
        empty.set_wrap(true);
        empty.set_xalign(0.5);
        empty.add_css_class("metis-dw-hint");
        list.append(&empty);
        return;
    }

    let parent = path.to_path_buf();
    for (i, entry) in entries.iter().enumerate() {
        if i >= MAX_ENTRIES {
            let more = gtk::Label::new(Some(
                &metis_i18n::tr("…and %1 more")
                    .replace("%1", &entries.len().saturating_sub(MAX_ENTRIES).to_string()),
            ));
            more.add_css_class("metis-dw-hint");
            list.append(&more);
            break;
        }
        list.append(&entry_row(entry, &parent));
    }
}

#[derive(Clone)]
struct DirEntry {
    path: PathBuf,
    /// On-disk basename (includes `.desktop` when present).
    name: String,
    is_dir: bool,
    is_desktop: bool,
}

fn read_entries(path: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for ent in std::fs::read_dir(path)? {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let meta = ent.metadata()?;
        let path = ent.path();
        let is_desktop = !meta.is_dir()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("desktop"));
        let item = DirEntry {
            path,
            name,
            is_dir: meta.is_dir(),
            is_desktop,
        };
        if item.is_dir {
            dirs.push(item);
        } else {
            files.push(item);
        }
    }
    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());
    dirs.append(&mut files);
    Ok(dirs)
}

fn entry_tile(entry: &DirEntry, parent_dir: &Path) -> gtk::Widget {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-dw-folder-tile");
    btn.set_has_frame(false);
    btn.set_hexpand(true);
    btn.set_size_request(TILE_WIDTH, -1);
    btn.set_tooltip_text(Some(&entry.name));

    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_halign(gtk::Align::Center);
    col.set_valign(gtk::Align::Start);

    let icon = resolve_icon(entry);
    icon.set_pixel_size(TILE_ICON);
    icon.set_halign(gtk::Align::Center);
    col.append(&icon);

    let label = gtk::Label::new(Some(&entry.name));
    label.add_css_class("metis-dw-folder-name");
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_justify(gtk::Justification::Center);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(12);
    label.set_xalign(0.5);
    col.append(&label);

    btn.set_child(Some(&col));

    let path = entry.path.clone();
    let is_dir = entry.is_dir;
    let is_desktop = entry.is_desktop;
    btn.connect_clicked(move |_| {
        open_path(&path, is_dir, is_desktop);
    });

    attach_entry_menu(&btn, entry, parent_dir);

    btn.upcast()
}

fn entry_row(entry: &DirEntry, parent_dir: &Path) -> gtk::Widget {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-dw-row");
    btn.set_has_frame(false);
    btn.set_halign(gtk::Align::Fill);
    btn.set_hexpand(true);
    btn.set_tooltip_text(Some(&entry.name));

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = resolve_icon(entry);
    icon.set_pixel_size(LIST_ICON);
    row.append(&icon);

    let label = gtk::Label::new(Some(&entry.name));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    btn.set_child(Some(&row));

    let path = entry.path.clone();
    let is_dir = entry.is_dir;
    let is_desktop = entry.is_desktop;
    btn.connect_clicked(move |_| {
        open_path(&path, is_dir, is_desktop);
    });

    attach_entry_menu(&btn, entry, parent_dir);

    btn.upcast()
}

fn resolve_icon(entry: &DirEntry) -> gtk::Image {
    if entry.is_dir {
        return gtk::Image::from_icon_name("folder");
    }
    if entry.is_desktop {
        if let Some(info) = gio_unix::DesktopAppInfo::from_filename(&entry.path)
            && let Some(icon) = AppInfoExt::icon(&info)
        {
            return gtk::Image::from_gicon(&icon);
        }
        return gtk::Image::from_icon_name("application-x-executable");
    }
    let file = gio::File::for_path(&entry.path);
    if let Ok(info) = file.query_info(
        "standard::icon",
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    ) && let Some(icon) = info.icon()
    {
        return gtk::Image::from_gicon(&icon);
    }
    let (ctype, _) = gio::content_type_guess(Some(entry.path.as_os_str()), None);
    gtk::Image::from_gicon(&gio::content_type_get_icon(&ctype))
}

fn open_path(path: &Path, is_dir: bool, is_desktop: bool) {
    if is_dir {
        crate::services::open_in_file_manager(path);
        return;
    }
    if is_desktop {
        launch_desktop_file(path);
        return;
    }
    open_with_default(path);
}

fn launch_desktop_file(path: &Path) {
    let Some(info) = gio_unix::DesktopAppInfo::from_filename(path) else {
        tracing::warn!(path = %path.display(), "not a valid .desktop file");
        open_with_default(path);
        return;
    };
    let exec = info
        .commandline()
        .map(|c| clean_exec(&c.to_string_lossy()))
        .filter(|s| !s.is_empty());
    if let Some(exec) = exec {
        if let Err(err) = crate::compositor::launch_program(&exec) {
            tracing::warn!(%err, exec = %exec, "failed to launch .desktop");
        }
        return;
    }
    if let Err(err) = info.launch(&[], None::<&gio::AppLaunchContext>) {
        tracing::warn!(%err, path = %path.display(), "DesktopAppInfo::launch failed");
        open_with_default(path);
    }
}

fn clean_exec(exec: &str) -> String {
    metis_protocol::split_command_line(exec)
        .into_iter()
        .filter(|tok| !(tok.len() == 2 && tok.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

fn open_with_default(path: &Path) {
    if let Err(err) =
        crate::compositor::launch_argv(["xdg-open".to_string(), path.display().to_string()])
    {
        tracing::warn!(%err, path = %path.display(), "xdg-open failed");
    }
}

fn open_with_picker(path: &Path) {
    let file = gio::File::for_path(path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    let parent = active_window();
    launcher.launch(parent.as_ref(), None::<&gio::Cancellable>, |_| {});
}

fn active_window() -> Option<gtk::Window> {
    gtk::Application::default().active_window()
}

fn menu_button(label: &str, popover: &gtk::Popover, on_click: Rc<dyn Fn()>) -> gtk::Button {
    let item = gtk::Button::with_label(label);
    item.set_halign(gtk::Align::Fill);
    item.add_css_class("flat");
    item.add_css_class("metis-dw-menu-item");
    let pop = popover.clone();
    item.connect_clicked(move |_| {
        pop.popdown();
        on_click();
    });
    item
}

fn attach_entry_menu(btn: &gtk::Button, entry: &DirEntry, parent_dir: &Path) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();

    let entry = entry.clone();
    let parent_dir = parent_dir.to_path_buf();
    let btn_weak = btn.downgrade();

    gesture.connect_pressed(move |_, _, _, _| {
        let Some(btn) = btn_weak.upgrade() else {
            return;
        };
        let popover = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(true)
            .build();
        popover.set_parent(&btn);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
        panel.set_margin_start(6);
        panel.set_margin_end(6);
        panel.set_margin_top(6);
        panel.set_margin_bottom(6);
        popover.set_child(Some(&panel));

        {
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            let is_desktop = entry.is_desktop;
            panel.append(&menu_button(
                &metis_i18n::tr("Open"),
                &popover,
                Rc::new(move || open_path(&path, is_dir, is_desktop)),
            ));
        }
        if !entry.is_dir {
            let path = entry.path.clone();
            panel.append(&menu_button(
                &metis_i18n::tr("Open with…"),
                &popover,
                Rc::new(move || open_with_picker(&path)),
            ));
        }
        {
            let path = entry.path.clone();
            let parent = parent_dir.clone();
            let old_name = entry.name.clone();
            let anchor = btn.clone();
            panel.append(&menu_button(
                &metis_i18n::tr("Rename…"),
                &popover,
                Rc::new(move || {
                    let anchor = anchor.clone();
                    let parent = parent.clone();
                    let path = path.clone();
                    let old_name = old_name.clone();
                    // Wait for the context menu to pop down before opening ours.
                    glib::idle_add_local_once(move || {
                        prompt_rename(&anchor, &parent, &path, &old_name);
                    });
                }),
            ));
        }
        {
            let path = entry.path.clone();
            let name = entry.name.clone();
            let anchor = btn.clone();
            panel.append(&menu_button(
                &metis_i18n::tr("Delete"),
                &popover,
                Rc::new(move || {
                    let anchor = anchor.clone();
                    let path = path.clone();
                    let name = name.clone();
                    glib::idle_add_local_once(move || {
                        confirm_delete(&anchor, &path, &name);
                    });
                }),
            ));
        }
        {
            let parent = parent_dir.clone();
            panel.append(&menu_button(
                &metis_i18n::tr("New Folder"),
                &popover,
                Rc::new(move || create_new_folder(&parent)),
            ));
        }
        {
            let parent = parent_dir.clone();
            panel.append(&menu_button(
                &metis_i18n::tr("Open in File Manager"),
                &popover,
                Rc::new(move || crate::services::open_in_file_manager(&parent)),
            ));
        }

        popover.popup();
    });

    btn.add_controller(gesture);
}

fn attach_background_menu_flow(flow: &gtk::FlowBox, parent_dir: Rc<PathBuf>) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Bubble)
        .build();

    let flow_weak = flow.downgrade();
    gesture.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }
        let Some(flow) = flow_weak.upgrade() else {
            return;
        };
        if let Some(child) = flow.child_at_pos(x as i32, y as i32)
            && child
                .child()
                .and_then(|c| c.downcast::<gtk::Button>().ok())
                .is_some()
        {
            return;
        }
        show_background_menu(flow.upcast_ref::<gtk::Widget>(), &parent_dir, x, y);
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });

    flow.add_controller(gesture);
}

fn attach_background_menu_box(list: &gtk::Box, parent_dir: Rc<PathBuf>) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Bubble)
        .build();

    let list_weak = list.downgrade();
    gesture.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }
        let Some(list) = list_weak.upgrade() else {
            return;
        };
        show_background_menu(list.upcast_ref::<gtk::Widget>(), &parent_dir, x, y);
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });

    list.add_controller(gesture);
}

fn show_background_menu(parent: &impl IsA<gtk::Widget>, parent_dir: &Path, x: f64, y: f64) {
    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .build();
    popover.set_parent(parent);
    let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
    panel.set_margin_start(6);
    panel.set_margin_end(6);
    panel.set_margin_top(6);
    panel.set_margin_bottom(6);
    popover.set_child(Some(&panel));

    let dir = parent_dir.to_path_buf();
    panel.append(&menu_button(
        &metis_i18n::tr("New Folder"),
        &popover,
        Rc::new(move || create_new_folder(&dir)),
    ));
    let dir = parent_dir.to_path_buf();
    panel.append(&menu_button(
        &metis_i18n::tr("Open in File Manager"),
        &popover,
        Rc::new(move || crate::services::open_in_file_manager(&dir)),
    ));

    popover.popup();
}

fn create_new_folder(parent: &Path) {
    let base = parent.join("New Folder");
    let mut path = base.clone();
    let mut n = 2;
    while path.exists() {
        path = parent.join(format!("New Folder {n}"));
        n += 1;
    }
    if let Err(err) = std::fs::create_dir(&path) {
        tracing::warn!(%err, path = %path.display(), "failed to create folder");
        toast_error(&metis_i18n::tr("Could not create folder: %1").replace("%1", &err.to_string()));
    }
}

fn confirm_delete(anchor: &impl IsA<gtk::Widget>, path: &Path, name: &str) {
    let path = path.to_path_buf();
    let title = metis_i18n::tr("Delete \"%1\"?").replace("%1", name);

    // Stay inside the desktop-widgets layer-shell surface via a Popover.
    // A separate gtk::Window is an RGBA buffer under the shell stylesheet
    // (`window { background-color: transparent }`); CSS never fills those pixels,
    // and Metis SSD draws a hollow/ghost titlebar around them.
    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .build();
    popover.add_css_class("metis-dw-confirm");
    popover.set_parent(anchor);

    let (host, sheet) = opaque_confirm_panel(320);
    let heading = gtk::Label::new(Some(&title));
    heading.set_wrap(true);
    heading.set_xalign(0.0);
    heading.add_css_class("metis-dw-confirm-title");
    sheet.append(&heading);

    let detail = gtk::Label::new(Some(&metis_i18n::tr("This cannot be undone.")));
    detail.set_wrap(true);
    detail.set_xalign(0.0);
    detail.add_css_class("metis-dw-confirm-detail");
    sheet.append(&detail);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(4);
    let cancel = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
    let delete = gtk::Button::with_label(&metis_i18n::tr("Delete"));
    delete.add_css_class("destructive-action");
    actions.append(&cancel);
    actions.append(&delete);
    sheet.append(&actions);

    popover.set_child(Some(&host));

    {
        let popover = popover.clone();
        cancel.connect_clicked(move |_| popover.popdown());
    }
    {
        let popover = popover.clone();
        delete.connect_clicked(move |_| {
            let res = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(err) = res {
                tracing::warn!(%err, path = %path.display(), "delete failed");
                toast_error(
                    &metis_i18n::tr("Could not delete: %1").replace("%1", &err.to_string()),
                );
            }
            popover.popdown();
        });
    }

    popover.popup();
}

fn prompt_rename(anchor: &impl IsA<gtk::Widget>, parent: &Path, path: &Path, old_name: &str) {
    let parent = parent.to_path_buf();
    let path = path.to_path_buf();
    let old_name = old_name.to_string();

    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .build();
    popover.add_css_class("metis-dw-confirm");
    popover.set_parent(anchor);

    let (host, sheet) = opaque_confirm_panel(300);
    let heading = gtk::Label::new(Some(&metis_i18n::tr("Rename")));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-dw-confirm-title");
    sheet.append(&heading);

    let entry = gtk::Entry::new();
    entry.set_text(&old_name);
    entry.set_hexpand(true);
    sheet.append(&entry);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
    let ok = gtk::Button::with_label(&metis_i18n::tr("Rename"));
    ok.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&ok);
    sheet.append(&actions);

    popover.set_child(Some(&host));

    {
        let popover = popover.clone();
        cancel.connect_clicked(move |_| popover.popdown());
    }
    let do_rename = Rc::new({
        let popover = popover.clone();
        let entry = entry.clone();
        move || {
            let new_name = entry.text().to_string();
            let new_name = new_name.trim();
            if new_name.is_empty() || new_name == old_name {
                popover.popdown();
                return;
            }
            let dest = parent.join(new_name);
            if dest.exists() {
                toast_error(&metis_i18n::tr("A file with that name already exists."));
                return;
            }
            if let Err(err) = std::fs::rename(&path, &dest) {
                tracing::warn!(%err, "rename failed");
                toast_error(
                    &metis_i18n::tr("Could not rename: %1").replace("%1", &err.to_string()),
                );
                return;
            }
            popover.popdown();
        }
    });
    {
        let do_rename = do_rename.clone();
        ok.connect_clicked(move |_| do_rename());
    }
    {
        let do_rename = do_rename.clone();
        entry.connect_activate(move |_| do_rename());
    }

    popover.popup();
    entry.grab_focus();
}

/// Popover panel with a Cairo-painted opaque backdrop under the content.
/// Same cell stacking in a Grid: paint fills the allocation; sheet sizes it.
fn opaque_confirm_panel(min_width: i32) -> (gtk::Grid, gtk::Box) {
    let (r, g, b) = surface_rgb01();

    let host = gtk::Grid::new();
    host.set_size_request(min_width, -1);
    host.add_css_class("metis-dw-confirm-sheet");

    let paint = gtk::DrawingArea::new();
    paint.set_hexpand(true);
    paint.set_vexpand(true);
    paint.set_can_target(false);
    paint.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgb(r, g, b);
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
    });

    let sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sheet.set_margin_start(16);
    sheet.set_margin_end(16);
    sheet.set_margin_top(14);
    sheet.set_margin_bottom(14);
    sheet.set_hexpand(true);

    host.attach(&paint, 0, 0, 1, 1);
    host.attach(&sheet, 0, 0, 1, 1);
    (host, sheet)
}

fn surface_rgb01() -> (f64, f64, f64) {
    let tokens = crate::ui::theme::active_tokens();
    let h = tokens.surface.trim().trim_start_matches('#');
    if h.len() != 6 {
        return (30.0 / 255.0, 30.0 / 255.0, 36.0 / 255.0);
    }
    let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(30) as f64 / 255.0;
    let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(30) as f64 / 255.0;
    let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(36) as f64 / 255.0;
    (r, g, b)
}

/// Theme-reload hook (confirm UI is Cairo-painted; stylesheet rules are static).
pub(crate) fn refresh_opaque_dialog_css() {}

fn toast_error(message: &str) {
    crate::ui::toast::show(&crate::services::BarNotification::internal(
        crate::services::NotificationKind::Error,
        metis_i18n::tr("Folders"),
        message,
    ));
}

fn expand_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if (trimmed.is_empty() || trimmed == "~/Desktop" || trimmed == "~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join("Desktop");
    }
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(trimmed)
}

fn display_path(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        if let Ok(rel) = path.strip_prefix(&home_path) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}
