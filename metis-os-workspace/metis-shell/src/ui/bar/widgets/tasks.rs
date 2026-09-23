//! Taskbar / running-apps dock for the edge bar.
//!
//! Shows one button per running application (windows grouped by resolved app
//! identity), plus any apps the user has pinned to the dock (`taskbar_pinned` in
//! `bar.json`). A running indicator dot and a focus highlight track live window
//! state fed from the compositor via `services::windows`. Clicking toggles
//! focus/minimize for single-window apps, opens a window picker for multi-window
//! apps, and launches pinned-but-not-running apps. Right-click pins/unpins or
//! closes. Overflow scrolls horizontally.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gio::prelude::*;
use gtk::gdk;
use gtk::prelude::*;

use crate::config::{load_bar_config, save_bar_config};
use crate::services::windows::{self, WindowsSnapshot};
use crate::services::{AppEntry, applications};

use metis_protocol::WindowInfo;

const TASK_ICON_SIZE: i32 = 20;

thread_local! {
    /// The single live transient popover (window picker or right-click menu). It is
    /// parented to a task button, but those buttons are destroyed wholesale on every
    /// dock rebuild — so we must pop it down and unparent it *before* the rebuild,
    /// or GTK finalizes a button that still owns the popover and crashes the shell.
    static TASK_POPOVER: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
    /// A small per-window menu nested *inside* the open picker (a child popup of
    /// `TASK_POPOVER`). Tracked separately so it can always be torn down before its
    /// parent picker is, otherwise GTK finalizes a row that still owns this popover.
    static ROW_MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

/// Content fingerprint of the (already output/workspace-filtered) dock: everything
/// that affects what's drawn. Windows are sorted by id so a reconcile that merely
/// reorders the list doesn't count as a change (which would needlessly rebuild and
/// close any open popover). The signature is per-widget, so each output's dock
/// dedups against its own last render.
fn dock_signature(windows: &[WindowInfo], focused: Option<u32>, pinned: &[String]) -> String {
    let mut wins: Vec<&WindowInfo> = windows.iter().collect();
    wins.sort_by_key(|w| w.id);
    let mut sig = format!("f:{focused:?}|");
    for w in wins {
        sig.push_str(&format!(
            "{}:{}:{};",
            w.id,
            w.minimized as u8,
            w.app_id.as_deref().unwrap_or(""),
        ));
    }
    sig.push('|');
    sig.push_str(&pinned.join(","));
    sig.push('|');
    sig.push_str(&crate::services::launch_pending::pending_ids().join(","));
    sig
}

/// Close + unparent the nested per-window menu, if any. Safe to call repeatedly.
fn dismiss_row_menu() {
    ROW_MENU.with(|cell| {
        if let Some(p) = cell.borrow_mut().take() {
            p.popdown();
            if p.parent().is_some() {
                p.unparent();
            }
        }
    });
}

/// Close + unparent the current transient popover, if any. Safe to call repeatedly.
fn dismiss_task_popover() {
    // The nested row menu is a child of the picker; tear it down first so it never
    // outlives the row it is parented to.
    dismiss_row_menu();
    TASK_POPOVER.with(|cell| {
        if let Some(p) = cell.borrow_mut().take() {
            p.popdown();
            if p.parent().is_some() {
                p.unparent();
            }
        }
    });
}

pub struct TasksWidget {
    root: gtk::ScrolledWindow,
    #[allow(dead_code)] // kept for lifetime — holds refresh closure alive
    refresh: Rc<dyn Fn()>,
}

impl TasksWidget {
    /// `output` is the compositor output name this bar lives on; the dock shows
    /// only windows on that output's currently-visible workspace. `None` (a bar
    /// not bound to a specific output) shows everything.
    pub fn new(vertical: bool, output: Option<String>) -> Self {
        let row = gtk::Box::new(
            if vertical {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            },
            2,
        );
        row.add_css_class("metis-bar-tasks-row");
        if vertical {
            row.add_css_class("metis-bar-tasks-row-vertical");
        }

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(if vertical {
                gtk::PolicyType::Never
            } else {
                gtk::PolicyType::Automatic
            })
            .vscrollbar_policy(if vertical {
                gtk::PolicyType::Automatic
            } else {
                gtk::PolicyType::Never
            })
            .child(&row)
            .build();
        root.add_css_class("metis-bar-tasks");
        if vertical {
            root.add_css_class("metis-bar-tasks-vertical");
        }
        // Size to content on the cross axis; the long axis is filled by hexpand /
        // vexpand from the bar so the dock grows with monitor resolution and
        // scrolls when icons overflow the allocated strip.
        if vertical {
            root.set_propagate_natural_width(true);
            root.set_propagate_natural_height(false);
            root.set_min_content_height(48);
        } else {
            root.set_propagate_natural_width(false);
            root.set_propagate_natural_height(true);
            root.set_min_content_width(48);
        }

        // Per-widget dedup signature (each output's dock has different content, so a
        // shared global guard would make multiple bars thrash each other's renders).
        // Fresh per widget, so a bar rebuilt after a bar.json change repaints once.
        let last_sig: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let refresh: Rc<dyn Fn()> = {
            let row = row.clone();
            let output = output.clone();
            let last_sig = last_sig.clone();
            Rc::new(move || {
                let snap = windows::snapshot();
                let pinned = load_bar_config().taskbar_pinned;
                rebuild(&row, &snap, &pinned, output.as_deref(), &last_sig);
            })
        };

        windows::register_refresh(refresh.clone());
        crate::services::launch_pending::register_refresh(refresh.clone());

        let app_index_refresh: Rc<dyn Fn()> = {
            let refresh = refresh.clone();
            let last_sig = last_sig.clone();
            Rc::new(move || {
                // Force a dock repaint so running apps pick up newly installed icons.
                *last_sig.borrow_mut() = None;
                refresh();
            })
        };
        applications::register_refresh(app_index_refresh);

        refresh();

        Self { root, refresh }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Repaint from the current window store (used by the bar's fan-out).
    #[allow(dead_code)] // bar tasks refresh hook
    pub fn update(&self, _snapshot: &WindowsSnapshot) {
        (self.refresh)();
    }
}

/// One dock entry: a pinned app and/or a set of running windows for one app.
struct Group {
    /// Id stored in `taskbar_pinned` when pinnable (resolved desktop id or app_id).
    pin_id: Option<String>,
    name: String,
    icon: gio::Icon,
    exec: Option<String>,
    windows: Vec<WindowInfo>,
    pinned: bool,
    /// True when `name`/`icon` came from a real `.desktop` entry. When false the
    /// group is showing a best-effort placeholder, so a live window's title/icon
    /// should override it on merge.
    resolved: bool,
}

/// Turn a reverse-DNS Wayland `app_id` into a friendly label when no `.desktop`
/// entry exists (e.g. `com.metis.Settings` -> `Settings`), so tooltips never
/// surface the raw id.
fn prettify_app_id(id: &str) -> String {
    let base = id.strip_suffix(".desktop").unwrap_or(id);
    let last = base.rsplit('.').next().unwrap_or(base);
    let cleaned = last.replace(['-', '_'], " ");
    let mut out = String::with_capacity(cleaned.len());
    for (i, word) in cleaned.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() { id.to_string() } else { out }
}

fn fallback_icon() -> gio::Icon {
    gio::ThemedIcon::new(applications::FALLBACK_ICON_NAME).upcast()
}

fn icon_or_fallback(entry: &AppEntry) -> gio::Icon {
    entry.icon.clone().unwrap_or_else(fallback_icon)
}

fn matches_app_id(entry: &AppEntry, needle: &str) -> bool {
    let base = entry
        .id
        .strip_suffix(".desktop")
        .unwrap_or(&entry.id)
        .to_lowercase();
    base == needle
        || entry.id.to_lowercase() == needle
        || entry
            .wm_class
            .as_deref()
            .is_some_and(|w| w.to_lowercase() == needle)
        || entry
            .flatpak_id
            .as_deref()
            .is_some_and(|f| f.to_lowercase() == needle)
}

fn rebuild(
    row: &gtk::Box,
    snap: &WindowsSnapshot,
    pinned: &[String],
    output: Option<&str>,
    last_sig: &RefCell<Option<String>>,
) {
    // Show only the windows on this bar's output and its currently-visible
    // workspace. Entries whose output/workspace aren't known yet (an event-folded
    // open before the next reconcile) are kept so nothing flickers out.
    let active_ws = crate::services::active_workspace_for(output);
    let visible: Vec<WindowInfo> = snap
        .windows
        .iter()
        .filter(|w| {
            let out_ok = match output {
                Some(o) => w.output.is_empty() || w.output == o,
                None => true,
            };
            let ws_ok = w.workspace == 0 || w.workspace == active_ws;
            out_ok && ws_ok
        })
        .cloned()
        .collect();

    // Skip no-op rebuilds (e.g. the idle 5-second reconcile) so an open picker /
    // right-click menu isn't dismissed when nothing actually changed. The active
    // workspace is part of the signature so switching workspaces always repaints.
    let sig = format!(
        "ws:{active_ws}|{}",
        dock_signature(&visible, snap.focused, pinned)
    );
    if last_sig.borrow().as_deref() == Some(sig.as_str()) {
        return;
    }
    *last_sig.borrow_mut() = Some(sig);

    // Tear down any open picker/menu first: its button is about to be destroyed and
    // a popover still parented to a finalized button crashes GTK.
    dismiss_task_popover();
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }

    // Enumerate installed apps once per rebuild for icon/name/exec resolution.
    let apps = applications::list_apps();
    let resolve = |app_id: &str| -> Option<&AppEntry> {
        let needle = app_id.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        apps.iter().find(|e| matches_app_id(e, &needle))
    };
    // When a window never reports an `app_id` (GTK sets it late, X11 apps, etc.)
    // fall back to matching its title against an installed app name so it still
    // gets a real icon where possible.
    let resolve_by_title = |title: &str| -> Option<&AppEntry> {
        let needle = title.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        apps.iter().find(|e| e.name.to_lowercase() == needle)
    };

    tracing::info!(
        windows = visible.len(),
        pinned = pinned.len(),
        "tasks: rebuilding dock"
    );

    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    // Pinned entries first, in saved order (even when not currently running).
    for id in pinned {
        // Prefer the visible app index, then a direct .desktop lookup so
        // OnlyShowIn=GNOME apps (Terminal, …) still get a real Exec line.
        let entry = apps
            .iter()
            .find(|e| matches_app_id(e, &id.to_lowercase()))
            .cloned()
            .or_else(|| applications::resolve_entry_for_id(id));
        // Index by the resolved desktop id when possible so running windows
        // (whose key is also the desktop id) merge into this pin.
        let key = entry
            .as_ref()
            .map(|e| e.id.to_lowercase())
            .unwrap_or_else(|| id.to_lowercase());
        if index.contains_key(&key) {
            continue;
        }
        let (name, icon, exec, resolved, pin_id) = match entry {
            Some(e) => (
                e.name.clone(),
                icon_or_fallback(&e),
                Some(e.exec.clone()),
                true,
                Some(e.id.clone()),
            ),
            None => (
                prettify_app_id(id),
                fallback_icon(),
                None,
                false,
                Some(id.clone()),
            ),
        };
        index.insert(key, groups.len());
        groups.push(Group {
            pin_id,
            name,
            icon,
            exec,
            windows: Vec::new(),
            pinned: true,
            resolved,
        });
    }

    // Then running windows, merged into pinned groups or appended as new ones.
    for w in &visible {
        let (pin_id, key, name, icon, exec, resolved) = match w.app_id.as_deref() {
            Some(app_id) if !app_id.is_empty() => {
                if let Some(e) = resolve(app_id) {
                    (
                        Some(e.id.clone()),
                        e.id.to_lowercase(),
                        e.name.clone(),
                        icon_or_fallback(e),
                        Some(e.exec.clone()),
                        true,
                    )
                } else {
                    // No matching desktop entry: prefer the human-readable window
                    // title over the raw app_id (e.g. "Metis Settings" rather than
                    // "com.metis.Settings") for the label and tooltip.
                    (
                        Some(app_id.to_string()),
                        app_id.to_lowercase(),
                        window_label(w),
                        applications::resolve_icon_for_app_id(Some(app_id)),
                        None,
                        false,
                    )
                }
            }
            _ => {
                if let Some(e) = resolve_by_title(&w.title) {
                    (
                        Some(e.id.clone()),
                        e.id.to_lowercase(),
                        e.name.clone(),
                        icon_or_fallback(e),
                        Some(e.exec.clone()),
                        true,
                    )
                } else {
                    (
                        None,
                        format!("win:{}", w.id),
                        window_label(w),
                        fallback_icon(),
                        None,
                        false,
                    )
                }
            }
        };

        if let Some(&i) = index.get(&key) {
            // A pinned group created from an unresolved app_id shows a placeholder
            // name; once its window is live, adopt the real window title / desktop
            // entry so the tooltip and Exec line are usable.
            if !groups[i].resolved {
                groups[i].name = window_label(w);
                let resolved = w
                    .app_id
                    .as_deref()
                    .and_then(&resolve)
                    .or_else(|| resolve_by_title(&w.title));
                if let Some(e) = resolved {
                    groups[i].icon = icon_or_fallback(e);
                    groups[i].exec = Some(e.exec.clone());
                    groups[i].pin_id = Some(e.id.clone());
                    groups[i].resolved = true;
                }
            } else if groups[i].exec.is_none() {
                // Pinned with icon/name but missing Exec (e.g. OnlyShowIn hide) —
                // fill in a launch command once we can resolve the desktop entry.
                let resolved = w.app_id.as_deref().and_then(&resolve).or_else(|| {
                    groups[i]
                        .pin_id
                        .as_deref()
                        .and_then(|id| apps.iter().find(|e| matches_app_id(e, &id.to_lowercase())))
                });
                if let Some(e) = resolved {
                    groups[i].exec = Some(e.exec.clone());
                    groups[i].pin_id = Some(e.id.clone());
                }
            }
            groups[i].windows.push(w.clone());
        } else {
            index.insert(key, groups.len());
            groups.push(Group {
                pin_id,
                name,
                icon,
                exec,
                windows: vec![w.clone()],
                pinned: false,
                resolved,
            });
        }
    }

    let mut shown = 0;
    for group in &groups {
        if !group.pinned && group.windows.is_empty() {
            continue;
        }
        row.append(&task_button(group, snap.focused));
        shown += 1;
    }
    tracing::info!(buttons = shown, "tasks: dock rebuilt");
}

fn window_label(w: &WindowInfo) -> String {
    if w.title.trim().is_empty() {
        "Application".to_string()
    } else {
        w.title.clone()
    }
}

fn task_button(group: &Group, focused: Option<u32>) -> gtk::Button {
    let btn = gtk::Button::builder().has_frame(false).build();
    btn.add_css_class("metis-bar-widget");
    btn.add_css_class("metis-bar-task");

    let running = !group.windows.is_empty();
    let starting = group
        .pin_id
        .as_deref()
        .is_some_and(crate::services::launch_pending::is_pending);
    let is_focused = focused.is_some_and(|f| group.windows.iter().any(|w| w.id == f));
    let all_minimized = running && group.windows.iter().all(|w| w.minimized);
    if running {
        btn.add_css_class("running");
    }
    if starting {
        btn.add_css_class("metis-bar-task-starting");
    }
    if is_focused {
        btn.add_css_class("focused");
    }
    if all_minimized {
        btn.add_css_class("minimized");
    }

    let overlay = gtk::Overlay::new();
    let image = gtk::Image::from_gicon(&group.icon);
    image.set_pixel_size(TASK_ICON_SIZE);
    overlay.set_child(Some(&image));

    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("metis-bar-task-dot");
    dot.set_halign(gtk::Align::Center);
    dot.set_valign(gtk::Align::End);
    dot.set_can_target(false);
    dot.set_visible(running || starting);
    overlay.add_overlay(&dot);

    btn.set_child(Some(&overlay));

    let tip = if group.windows.len() > 1 {
        format!("{} ({})", group.name, group.windows.len())
    } else {
        group.name.clone()
    };
    btn.set_tooltip_text(Some(&tip));

    {
        let windows = group.windows.clone();
        let exec = group.exec.clone();
        let pin_id = group.pin_id.clone();
        let name = group.name.clone();
        let btn_weak = btn.downgrade();
        btn.connect_clicked(move |_| match windows.len() {
            0 => {
                // Prefer desktop-id launch so OnlyShowIn=GNOME pins (Terminal, …)
                // still start even when Exec was missing at pin time.
                if let Some(id) = &pin_id {
                    applications::launch_id(id);
                } else if let Some(exec) = &exec {
                    if let Err(err) = crate::compositor::launch_program(exec) {
                        tracing::warn!(%err, "failed to launch pinned app");
                    }
                } else {
                    tracing::warn!(%name, "pinned app has no launch command");
                }
            }
            1 => toggle_window(&windows[0], focused),
            _ => {
                if let Some(btn) = btn_weak.upgrade() {
                    show_picker(&btn, &windows, &name, focused);
                }
            }
        });
    }

    attach_context_menu(&btn, group);
    btn
}

fn toggle_window(w: &WindowInfo, focused: Option<u32>) {
    let is_focused = focused == Some(w.id);
    let result = if is_focused && !w.minimized {
        crate::compositor::set_minimized(w.id, true)
    } else {
        crate::compositor::activate_window(w.id)
    };
    if let Err(err) = result {
        tracing::warn!(%err, id = w.id, "failed to toggle window");
    }
}

/// Window picker for a multi-window app: one row per window, click toggles it.
fn show_picker(parent: &gtk::Button, windows: &[WindowInfo], name: &str, focused: Option<u32>) {
    let panel = super::super::dropdown::build_panel();
    panel.add_css_class("metis-bar-tasks-picker");
    panel.set_spacing(2);
    panel.set_width_request(240);

    let title = gtk::Label::builder()
        .label(name)
        .halign(gtk::Align::Start)
        .build();
    title.add_css_class("metis-bar-section-title");
    panel.append(&title);

    let popover = transient_popover(parent, &panel);

    // Number windows 1..n by ascending id so the pill matches the "(n)" the
    // compositor appends to that window's titlebar.
    let mut ordered: Vec<&WindowInfo> = windows.iter().collect();
    ordered.sort_by_key(|w| w.id);
    let multi = ordered.len() > 1;

    for (i, w) in ordered.iter().enumerate() {
        let row = gtk::Button::builder().has_frame(false).build();
        row.add_css_class("metis-bar-task-pick");
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if multi {
            let num = gtk::Label::new(Some(&(i + 1).to_string()));
            num.add_css_class("metis-bar-task-pick-num");
            num.set_valign(gtk::Align::Center);
            hbox.append(&num);
        }
        let label = gtk::Label::new(Some(&window_label(w)));
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(28);
        if focused == Some(w.id) {
            row.add_css_class("focused");
        }
        if w.minimized {
            row.add_css_class("minimized");
        }
        hbox.append(&label);
        row.set_child(Some(&hbox));

        // Right-click a row to open a small menu nested over this picker (a child
        // popup of the row) so closing a single window is a deliberate menu choice
        // rather than a stray click — and the picker stays put behind it.
        {
            let gesture = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let row_w = row.clone();
            let id = w.id;
            let menu_title = if multi {
                format!("{} ({})", window_label(w), i + 1)
            } else {
                window_label(w)
            };
            gesture.connect_pressed(move |_, _, _, _| {
                show_window_menu(&row_w, id, &menu_title);
            });
            row.add_controller(gesture);
        }

        let w = (*w).clone();
        let popover = popover.clone();
        row.connect_clicked(move |_| {
            popover.popdown();
            toggle_window(&w, focused);
        });
        panel.append(&row);
    }

    glib::idle_add_local_once(move || popover.popup());
}

/// Per-window action menu opened by right-clicking a picker row. It is a child
/// popup of the row, so it appears nested over the still-open picker rather than
/// tearing it down — and input stays on the live picker surface chain.
fn show_window_menu(row: &gtk::Button, id: u32, title: &str) {
    // Only one nested menu at a time.
    dismiss_row_menu();

    let panel = super::super::dropdown::build_panel();
    panel.add_css_class("metis-bar-tasks-menu");
    panel.set_spacing(2);
    panel.set_width_request(180);

    let header = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .build();
    header.add_css_class("metis-bar-section-title");
    panel.append(&header);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(row);
    super::super::dropdown::register(&popover);
    ROW_MENU.with(|cell| *cell.borrow_mut() = Some(popover.clone()));

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        let weak = weak.clone();
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade()
                && p.parent().is_some()
            {
                p.unparent();
            }
        });
    });

    let item = menu_item("Close window");
    item.connect_clicked(move |_| {
        // Close the menu and the picker behind it, then close the window.
        dismiss_task_popover();
        if let Err(err) = crate::compositor::close_window(id) {
            tracing::warn!(%err, id, "failed to close window from picker menu");
        }
    });
    panel.append(&item);

    glib::idle_add_local_once(move || popover.popup());
}

/// Right-click menu: pin/unpin from the dock and close the app's windows.
fn attach_context_menu(btn: &gtk::Button, group: &Group) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();

    let pin_id = group.pin_id.clone();
    let pinned = group.pinned;
    let exec = group.exec.clone();
    let window_ids: Vec<u32> = group.windows.iter().map(|w| w.id).collect();
    let btn_weak = btn.downgrade();

    gesture.connect_pressed(move |_, _, _, _| {
        let Some(btn) = btn_weak.upgrade() else {
            return;
        };
        let panel = super::super::dropdown::build_panel();
        panel.add_css_class("metis-bar-tasks-menu");
        panel.set_spacing(2);
        panel.set_width_request(180);

        let popover = transient_popover(&btn, &panel);

        // Launch another instance. Only offered when we know how to start the app
        // (a resolved `.desktop` exec or pin id).
        if exec.is_some() || pin_id.is_some() {
            let item = menu_item("New window");
            let popover_c = popover.clone();
            let exec = exec.clone();
            let pin_id = pin_id.clone();
            item.connect_clicked(move |_| {
                popover_c.popdown();
                if let Some(id) = &pin_id {
                    applications::launch_id_new_window(id);
                } else if let Some(exec) = &exec
                    && let Err(err) = crate::compositor::launch_program(exec)
                {
                    tracing::warn!(%err, "failed to launch new app window");
                }
            });
            panel.append(&item);
        }

        if let Some(id) = pin_id.clone() {
            let label = if pinned {
                "Unpin from taskbar"
            } else {
                "Pin to taskbar"
            };
            let item = menu_item(label);
            item.connect_clicked(move |_| {
                // Toggling the pin rewrites bar.json, whose file monitor fires a
                // higher-priority 250ms timeout that rebuilds the whole bar. Pop the
                // popover down *and unparent it synchronously* now, otherwise that
                // rebuild finalizes this menu's button while the popover is still
                // parented to it (a GTK GDK_IS_SURFACE crash). connect_closed's idle
                // unparent runs too late (lower priority than the timeout).
                dismiss_task_popover();
                toggle_taskbar_pin(&id);
            });
            panel.append(&item);
        }

        if !window_ids.is_empty() {
            let label = if window_ids.len() > 1 {
                "Close all windows"
            } else {
                "Close window"
            };
            let item = menu_item(label);
            let ids = window_ids.clone();
            let popover_c = popover.clone();
            item.connect_clicked(move |_| {
                popover_c.popdown();
                for id in &ids {
                    if let Err(err) = crate::compositor::close_window(*id) {
                        tracing::warn!(%err, id, "failed to close window");
                    }
                }
            });
            panel.append(&item);
        }

        glib::idle_add_local_once(move || popover.popup());
    });

    btn.add_controller(gesture);
}

fn menu_item(label: &str) -> gtk::Button {
    let item = gtk::Button::builder().label(label).has_frame(false).build();
    item.add_css_class("metis-bar-task-menu-item");
    if let Some(child) = item.child()
        && let Ok(lbl) = child.downcast::<gtk::Label>()
    {
        lbl.set_halign(gtk::Align::Start);
        lbl.set_xalign(0.0);
    }
    item
}

/// Build a non-autohide popover parented to `parent` that unparents itself when
/// closed (task buttons are rebuilt on every window change, so popovers must not
/// outlive their button). Registered with the dropdown manager so the
/// compositor "close-popovers" signal and single-open behavior still apply.
fn transient_popover(parent: &impl IsA<gtk::Widget>, panel: &gtk::Box) -> gtk::Popover {
    // Only one transient popover at a time; tear the previous one down cleanly
    // (it may be parented to a button that's about to be replaced).
    dismiss_task_popover();
    super::super::dropdown::close_all();
    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(panel)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(parent);
    super::super::dropdown::register(&popover);
    TASK_POPOVER.with(|cell| *cell.borrow_mut() = Some(popover.clone()));

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        let weak = weak.clone();
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade()
                && p.parent().is_some()
            {
                p.unparent();
            }
        });
    });

    popover
}

fn toggle_taskbar_pin(id: &str) {
    let mut cfg = load_bar_config();
    let needle = id.to_lowercase();
    if let Some(pos) = cfg
        .taskbar_pinned
        .iter()
        .position(|p| p.to_lowercase() == needle)
    {
        cfg.taskbar_pinned.remove(pos);
    } else {
        cfg.taskbar_pinned.push(id.to_string());
    }
    if let Err(err) = save_bar_config(&cfg) {
        tracing::warn!(%err, "failed to persist bar.json after taskbar pin toggle");
        return;
    }
    // Don't wait for the bar.json file monitor / 5s window reconcile — unpin
    // used to sit on the dock for several seconds until the next refresh.
    crate::services::windows::refresh_taskbars();
}
