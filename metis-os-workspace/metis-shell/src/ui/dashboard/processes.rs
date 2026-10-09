//! Process list tree, sorting, and context menus.

use gtk::gdk;
use gtk::prelude::*;
use metis_config::load_dashboard_config;
use metis_i18n::tr;
use std::collections::{HashMap, HashSet};

use crate::services::{ProcessClass, ProcessRow, format_bytes, kill_process, kill_process_tree};

use super::views;
use super::{
    DASHBOARD, PROCESS_CONTEXT_MENU, ProcessClassFilter, ProcessSortColumn, SortDirection,
};

pub(crate) struct ProcessTreeEntry {
    pub(crate) proc: ProcessRow,
    depth: usize,
    child_count: usize,
}

pub(crate) fn flatten_process_tree(
    pid: u32,
    depth: usize,
    children: &HashMap<u32, Vec<u32>>,
    by_pid: &HashMap<u32, &ProcessRow>,
    expanded: &HashSet<u32>,
    out: &mut Vec<ProcessTreeEntry>,
) {
    let Some(proc) = by_pid.get(&pid) else {
        return;
    };
    let kids = children.get(&pid).map(|v| v.as_slice()).unwrap_or(&[]);
    out.push(ProcessTreeEntry {
        proc: (*proc).clone(),
        depth,
        child_count: kids.len(),
    });
    if kids.is_empty() || !expanded.contains(&pid) {
        return;
    }
    for child in kids {
        flatten_process_tree(*child, depth + 1, children, by_pid, expanded, out);
    }
}

pub(crate) fn compare_process_rows(
    a: &ProcessRow,
    b: &ProcessRow,
    col: ProcessSortColumn,
    dir: SortDirection,
) -> std::cmp::Ordering {
    let ord = match col {
        ProcessSortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        ProcessSortColumn::Pid => a.pid.cmp(&b.pid),
        ProcessSortColumn::User => a.user.to_lowercase().cmp(&b.user.to_lowercase()),
        ProcessSortColumn::Kind => class_order(a.class).cmp(&class_order(b.class)),
        ProcessSortColumn::Cpu => a
            .cpu_percent
            .partial_cmp(&b.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal),
        ProcessSortColumn::Memory => a.memory_bytes.cmp(&b.memory_bytes),
    };
    match dir {
        SortDirection::Asc => ord,
        SortDirection::Desc => ord.reverse(),
    }
}

pub(crate) fn process_list_sig(procs: &[ProcessRow]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    procs.len().hash(&mut hasher);
    for proc in procs.iter().take(48) {
        proc.pid.hash(&mut hasher);
        (proc.cpu_percent as u32).hash(&mut hasher);
        proc.memory_bytes.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn set_sort_label(btn: &gtk::Button, title: &str, active: bool, dir: SortDirection) {
    let suffix = if active {
        match dir {
            SortDirection::Asc => " ↑",
            SortDirection::Desc => " ↓",
        }
    } else {
        ""
    };
    btn.set_label(&format!("{title}{suffix}"));
    btn.set_sensitive(true);
    if active {
        btn.add_css_class("metis-dash-sort-active");
    } else {
        btn.remove_css_class("metis-dash-sort-active");
    }
}

pub(crate) fn default_sort_direction(col: ProcessSortColumn) -> SortDirection {
    match col {
        ProcessSortColumn::Name | ProcessSortColumn::User | ProcessSortColumn::Kind => {
            SortDirection::Asc
        }
        ProcessSortColumn::Pid | ProcessSortColumn::Cpu | ProcessSortColumn::Memory => {
            SortDirection::Desc
        }
    }
}

pub(crate) fn class_order(class: ProcessClass) -> u8 {
    match class {
        ProcessClass::Metis => 0,
        ProcessClass::UserApp => 1,
        ProcessClass::System => 2,
    }
}

pub(crate) fn wire_process_sort(headers: &views::ProcessHeader, list: &gtk::ListBox) {
    wire_sort_btn(&headers.name, ProcessSortColumn::Name, list);
    wire_sort_btn(&headers.pid, ProcessSortColumn::Pid, list);
    wire_sort_btn(&headers.user, ProcessSortColumn::User, list);
    wire_sort_btn(&headers.kind, ProcessSortColumn::Kind, list);
    wire_sort_btn(&headers.cpu, ProcessSortColumn::Cpu, list);
    wire_sort_btn(&headers.memory, ProcessSortColumn::Memory, list);
}

pub(crate) fn wire_sort_btn(btn: &gtk::Button, col: ProcessSortColumn, list: &gtk::ListBox) {
    let list = list.clone();
    btn.connect_clicked(move |_| {
        DASHBOARD.with(|d| {
            let holder = d.borrow();
            let Some(dash) = holder.as_ref() else {
                return;
            };
            {
                let mut column = dash.sort_column.borrow_mut();
                let mut dir = dash.sort_direction.borrow_mut();
                if *column == col {
                    *dir = match *dir {
                        SortDirection::Asc => SortDirection::Desc,
                        SortDirection::Desc => SortDirection::Asc,
                    };
                } else {
                    *column = col;
                    *dir = default_sort_direction(col);
                }
            }
            dash.rebuild_process_list(&list);
            dash.refresh_sort_headers();
        });
    });
}

pub(crate) fn matches_class_filter(filter: ProcessClassFilter, class: ProcessClass) -> bool {
    match filter {
        ProcessClassFilter::All => true,
        ProcessClassFilter::UserApps => {
            matches!(class, ProcessClass::UserApp | ProcessClass::Metis)
        }
        ProcessClassFilter::System => class == ProcessClass::System,
    }
}

pub(crate) fn process_row(entry: &ProcessTreeEntry) -> gtk::ListBoxRow {
    let proc = &entry.proc;
    let row = gtk::ListBoxRow::new();
    row.add_css_class("metis-dash-table-row");
    let grid = gtk::Grid::builder()
        .column_spacing(8)
        .margin_top(5)
        .margin_bottom(5)
        .build();
    grid.add_css_class("metis-dash-proc-cols");

    let name_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    name_box.set_hexpand(true);
    name_box.set_halign(gtk::Align::Fill);
    name_box.set_margin_start((entry.depth as i32) * 14);

    if entry.child_count > 0 {
        let expanded = DASHBOARD.with(|d| {
            d.borrow()
                .as_ref()
                .is_some_and(|dash| dash.expanded_processes.borrow().contains(&proc.pid))
        });
        let toggle = gtk::Button::from_icon_name(if expanded {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        });
        toggle.add_css_class("flat");
        toggle.add_css_class("metis-dash-proc-expand");
        let expand_tooltip = if expanded {
            tr("Collapse child processes")
        } else {
            tr("Expand child processes")
        };
        toggle.set_tooltip_text(Some(&expand_tooltip));
        let pid_toggle = proc.pid;
        toggle.connect_clicked(move |_| {
            DASHBOARD.with(|d| {
                let holder = d.borrow();
                let Some(dash) = holder.as_ref() else {
                    return;
                };
                {
                    let mut expanded = dash.expanded_processes.borrow_mut();
                    if !expanded.remove(&pid_toggle) {
                        expanded.insert(pid_toggle);
                    }
                }
                dash.rebuild_process_list(&dash.processes.list);
            });
        });
        name_box.append(&toggle);
    } else {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(24, -1);
        name_box.append(&spacer);
    }

    let name = gtk::Label::new(Some(&proc.name));
    name.set_halign(gtk::Align::Start);
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    match proc.class {
        ProcessClass::Metis => name.add_css_class("metis-dash-process-metis"),
        ProcessClass::System => name.add_css_class("metis-dash-muted"),
        ProcessClass::UserApp => name.add_css_class("metis-dash-proc-name"),
    }
    name_box.append(&name);

    let pid = gtk::Label::new(Some(&proc.pid.to_string()));
    pid.add_css_class("metis-dash-muted");
    pid.set_halign(gtk::Align::Start);

    let user = gtk::Label::new(Some(&proc.user));
    user.add_css_class("metis-dash-muted");
    user.set_halign(gtk::Align::Start);
    user.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let kind = gtk::Label::new(Some(&class_label(proc.class)));
    kind.add_css_class("metis-dash-muted");
    kind.set_halign(gtk::Align::Start);

    let cpu = gtk::Label::new(Some(&format!("{:.1}%", proc.cpu_percent)));
    cpu.add_css_class("metis-dash-muted");
    cpu.set_halign(gtk::Align::End);
    cpu.set_xalign(1.0);

    let mem = gtk::Label::new(Some(&format_bytes(proc.memory_bytes)));
    mem.add_css_class("metis-dash-muted");
    mem.set_halign(gtk::Align::End);
    mem.set_xalign(1.0);

    pid.set_width_request(64);
    user.set_width_request(88);
    kind.set_width_request(64);
    cpu.set_width_request(64);
    mem.set_width_request(80);
    grid.attach(&name_box, 0, 0, 1, 1);
    grid.attach(&pid, 1, 0, 1, 1);
    grid.attach(&user, 2, 0, 1, 1);
    grid.attach(&kind, 3, 0, 1, 1);
    grid.attach(&cpu, 4, 0, 1, 1);
    grid.attach(&mem, 5, 0, 1, 1);

    let kill = gtk::Button::from_icon_name("process-stop-symbolic");
    kill.add_css_class("flat");
    kill.set_sensitive(proc.killable);
    let kill_tooltip = if proc.killable {
        tr("End task")
    } else {
        tr("Cannot end processes owned by another user")
    };
    kill.set_tooltip_text(Some(&kill_tooltip));
    let pid_val = proc.pid;
    kill.connect_clicked(move |btn| {
        let cfg = load_dashboard_config();
        if cfg.confirm_before_kill {
            confirm_kill_process(btn, pid_val, false, false);
        } else if let Err(err) = kill_process(pid_val, false) {
            tracing::warn!(%err, pid = pid_val, "end task failed");
        }
    });

    grid.attach(&kill, 6, 0, 1, 1);

    if proc.killable {
        attach_process_context_menu(&row, proc.pid, &proc.name, entry.child_count > 0);
    }

    row.set_child(Some(&grid));
    row
}

pub(crate) fn process_context_menu_open() -> bool {
    PROCESS_CONTEXT_MENU.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|p| p.is_visible() || p.parent().is_some())
    })
}

pub(crate) fn dismiss_process_context_menu() {
    // Take the popover out before popdown — the closed handler also touches
    // PROCESS_CONTEXT_MENU, so holding RefMut across popdown aborts the shell.
    let popover = PROCESS_CONTEXT_MENU.with(|slot| slot.borrow_mut().take());
    if let Some(popover) = popover
        && popover.parent().is_some()
    {
        popover.popdown();
        if popover.parent().is_some() {
            popover.unparent();
        }
    }
}

pub(crate) fn attach_process_context_menu(
    row: &gtk::ListBoxRow,
    pid: u32,
    name: &str,
    has_children: bool,
) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let row_widget: gtk::Widget = row.clone().upcast();
    let name = name.to_string();
    gesture.connect_pressed(move |_, _n, x, y| {
        show_process_context_menu(&row_widget, pid, &name, has_children, x, y);
    });
    row.add_controller(gesture);
}

pub(crate) fn show_process_context_menu(
    anchor: &gtk::Widget,
    pid: u32,
    name: &str,
    has_children: bool,
    x: f64,
    y: f64,
) {
    dismiss_process_context_menu();
    crate::ui::bar::close_bar_popovers();

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 2);
    panel.add_css_class("metis-dash-context-menu");
    panel.set_margin_top(6);
    panel.set_margin_bottom(6);
    panel.set_margin_start(4);
    panel.set_margin_end(4);
    panel.set_width_request(220);

    let header = gtk::Label::new(Some(name));
    header.set_xalign(0.0);
    header.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.add_css_class("metis-dash-popover-title");
    header.set_margin_start(8);
    header.set_margin_end(8);
    header.set_margin_top(4);
    panel.append(&header);

    append_process_menu_item(&panel, &tr("End task"), {
        let anchor = anchor.clone();
        move || {
            dismiss_process_context_menu();
            confirm_kill_process(&anchor, pid, false, false);
        }
    });
    append_process_menu_item(&panel, &tr("Force quit"), {
        let anchor = anchor.clone();
        move || {
            dismiss_process_context_menu();
            confirm_kill_process(&anchor, pid, true, false);
        }
    });
    if has_children {
        append_process_menu_item(&panel, &tr("End process tree"), {
            let anchor = anchor.clone();
            move || {
                dismiss_process_context_menu();
                confirm_kill_process(&anchor, pid, false, true);
            }
        });
        append_process_menu_item(&panel, &tr("Force quit tree"), {
            let anchor = anchor.clone();
            move || {
                dismiss_process_context_menu();
                confirm_kill_process(&anchor, pid, true, true);
            }
        });
    }
    append_process_menu_item(&panel, &format!("{} ({pid})", tr("Copy PID")), move || {
        copy_text_to_clipboard(&pid.to_string());
        dismiss_process_context_menu();
    });

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-dash-popover");
    popover.set_parent(anchor);
    let rect = gdk::Rectangle::new(x.round() as i32, y.round() as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));

    PROCESS_CONTEXT_MENU.with(|slot| *slot.borrow_mut() = Some(popover.clone()));
    crate::ui::bar::register_bar_popover(&popover);

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        PROCESS_CONTEXT_MENU.with(|slot| {
            let clear = slot
                .borrow()
                .as_ref()
                .is_some_and(|p| p.downgrade() == weak);
            if clear {
                *slot.borrow_mut() = None;
            }
        });
        if let Some(p) = weak.upgrade()
            && p.parent().is_some()
        {
            p.unparent();
        }
    });

    let popover_show = popover.clone();
    glib::idle_add_local_once(move || {
        popover_show.popup();
    });
}

pub(crate) fn append_process_menu_item<F>(panel: &gtk::Box, label: &str, action: F)
where
    F: Fn() + 'static,
{
    let item = gtk::Button::builder()
        .label(label)
        .has_frame(false)
        .halign(gtk::Align::Fill)
        .build();
    item.add_css_class("metis-dash-menu-item");
    if let Some(child) = item.child()
        && let Ok(lbl) = child.downcast::<gtk::Label>()
    {
        lbl.set_halign(gtk::Align::Start);
        lbl.set_xalign(0.0);
    }
    item.connect_clicked(move |_| action());
    panel.append(&item);
}

pub(crate) fn confirm_kill_process(
    anchor: &impl IsA<gtk::Widget>,
    pid: u32,
    force: bool,
    tree: bool,
) {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.set_margin_top(10);
    panel.set_margin_bottom(10);
    panel.set_margin_start(12);
    panel.set_margin_end(12);
    panel.set_width_request(260);

    let title = match (force, tree) {
        (true, true) => tr("Force quit process tree?"),
        (true, false) => tr("Force quit?"),
        (false, true) => tr("End process tree?"),
        (false, false) => tr("End task?"),
    };
    let body = match (force, tree) {
        (true, true) => format!(
            "{} {pid} {}? {}",
            tr("Send SIGKILL to process"),
            tr("and its child processes"),
            tr("This cannot be undone.")
        ),
        (true, false) => format!(
            "{} {pid}? {}",
            tr("Send SIGKILL to process"),
            tr("This cannot be undone.")
        ),
        (false, true) => format!(
            "{} {pid} {}?",
            tr("Send SIGTERM to process"),
            tr("and its child processes")
        ),
        (false, false) => format!("{} {pid}?", tr("Send SIGTERM to process")),
    };

    let t = gtk::Label::new(Some(&title));
    t.set_xalign(0.0);
    t.add_css_class("metis-dash-popover-title");
    panel.append(&t);
    let b = gtk::Label::new(Some(&body));
    b.set_xalign(0.0);
    b.set_wrap(true);
    b.add_css_class("metis-dash-muted");
    panel.append(&b);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(4);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let ok_label = match (force, tree) {
        (true, true) => tr("Force quit tree"),
        (true, false) => tr("Force quit"),
        (false, true) => tr("End tree"),
        (false, false) => tr("End task"),
    };
    let ok = gtk::Button::with_label(&ok_label);
    if force {
        ok.add_css_class("destructive-action");
    }
    actions.append(&cancel);
    actions.append(&ok);
    panel.append(&actions);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(crate::ui::bar::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-dash-popover");
    popover.set_parent(anchor);
    crate::ui::bar::register_bar_popover(&popover);

    let pop_cancel = popover.clone();
    cancel.connect_clicked(move |_| pop_cancel.popdown());
    let pop_ok = popover.clone();
    ok.connect_clicked(move |_| {
        let result = if tree {
            DASHBOARD.with(|d| {
                let holder = d.borrow();
                let procs = holder
                    .as_ref()
                    .map(|dash| dash.snapshot.borrow().processes.clone())
                    .unwrap_or_default();
                kill_process_tree(pid, &procs, force)
            })
        } else {
            kill_process(pid, force)
        };
        if let Err(err) = result {
            tracing::warn!(%err, pid, force, tree, "kill failed");
        }
        pop_ok.popdown();
    });
    let popover_show = popover.clone();
    glib::idle_add_local_once(move || {
        popover_show.popup();
    });
}

pub(crate) fn copy_text_to_clipboard(text: &str) {
    let display = gdk::Display::default();
    let Some(display) = display else {
        return;
    };
    let clipboard = display.clipboard();
    clipboard.set_text(text);
}

pub(crate) fn class_label(class: ProcessClass) -> String {
    match class {
        ProcessClass::Metis => tr("Metis"),
        ProcessClass::UserApp => tr("App"),
        ProcessClass::System => tr("System"),
    }
}

pub(crate) fn launch_process_monitor() {
    let dash = metis_config::load_dashboard_config();
    let candidates: Vec<String> = match dash
        .process_monitor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(chosen) => vec![chosen.to_string()],
        None => metis_config::KNOWN_PROCESS_MONITORS
            .iter()
            .map(|(bin, _)| (*bin).to_string())
            .collect(),
    };
    for bin in &candidates {
        if !metis_config::binary_in_path(bin) {
            continue;
        }
        let argv = if metis_config::process_monitor_needs_terminal(bin) {
            let Some(term) = metis_config::resolve_terminal() else {
                continue;
            };
            metis_config::argv_in_terminal(&term, bin)
        } else {
            vec![bin.clone()]
        };
        if let Err(err) = crate::compositor::launch_argv(argv) {
            tracing::warn!(%err, bin, "failed to launch process monitor");
            continue;
        }
        return;
    }
    let _ = std::process::Command::new("notify-send")
        .args([
            "-a",
            "Metis",
            "No process monitor found",
            "Install btop, htop, or a system monitor — or set one in Settings → Control Center.",
        ])
        .spawn();
}
