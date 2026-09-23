//! Declarative JSON extension widgets (Phase 14 §E).
//!
//! Action fields are **not** settings-interpolated (labels/copy text may be).
//! Labels may use `{host.*}` and `{helper.*}` live binds, refreshed on a short
//! timer. URI / launch targets are re-validated at click time.
//!
//! Pack helpers (optional `manifest.helper`) are spawned argv-only from the
//! widgets process — never via compositor IPC (Phase 15 §D posture).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use metis_config::{
    DesktopWidgetInstance, HostBindValues, WIDGET_EXT_MAX_COPY, WidgetExtAction,
    WidgetExtLabelStyle, WidgetExtNode, find_widget_extension, interpolate_settings,
    interpolate_template, is_safe_launch_exec, is_safe_launch_id, is_safe_open_uri,
    load_widget_layout, resolve_helper_exec, run_helper_snapshot, template_needs_host,
    validate_action,
};
use sysinfo::{Disks, System};

use crate::services::applications;

struct HostBoundLabel {
    label: gtk::Label,
    template: String,
    settings: serde_json::Map<String, serde_json::Value>,
    /// Pack id when the label uses `{helper.*}` (for per-pack helper cache).
    extension_id: Option<String>,
}

struct HelperCacheEntry {
    values: std::collections::BTreeMap<String, String>,
    next_poll: Instant,
    poll_every: Duration,
    exec: PathBuf,
}

thread_local! {
    static HOST_BOUNDS: RefCell<Vec<HostBoundLabel>> = const { RefCell::new(Vec::new()) };
    static HOST_TIMER: Cell<bool> = const { Cell::new(false) };
    static HOST_SYS: RefCell<Option<System>> = const { RefCell::new(None) };
    static HELPER_CACHE: RefCell<HashMap<String, HelperCacheEntry>> =
        RefCell::new(HashMap::new());
}

pub fn build(inst: &DesktopWidgetInstance) -> gtk::Widget {
    let Some(ext) = find_widget_extension(&inst.extension_id) else {
        return missing_card(&inst.extension_id);
    };
    let layout = match load_widget_layout(&ext.root) {
        Ok(n) => n,
        Err(err) => {
            tracing::warn!(
                id = %inst.extension_id,
                %err,
                "extension widget.json failed to load"
            );
            return error_card(&format!("Invalid widget.json: {err}"));
        }
    };
    if let Some(helper) = &ext.manifest.helper {
        if let Some(exec) = resolve_helper_exec(&ext.root, helper) {
            let poll = Duration::from_secs(u64::from(helper.poll_seconds.clamp(2, 120)));
            HELPER_CACHE.with(|cache| {
                cache.borrow_mut().insert(
                    inst.extension_id.clone(),
                    HelperCacheEntry {
                        values: run_helper_snapshot(&exec).unwrap_or_default(),
                        next_poll: Instant::now() + poll,
                        poll_every: poll,
                        exec,
                    },
                );
            });
        } else {
            tracing::warn!(
                id = %inst.extension_id,
                exec = %helper.exec,
                "extension helper exec rejected or missing"
            );
        }
    }
    let settings = inst.extension_settings.clone();
    let host = sample_host_binds(Some(&inst.extension_id));
    let root = build_node(
        &layout,
        &settings,
        Some(&host),
        Some(inst.extension_id.as_str()),
    );
    ensure_host_timer();
    root
}

fn missing_card(id: &str) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let title = gtk::Label::new(Some(&metis_i18n::tr("Extension not found")));
    title.set_xalign(0.0);
    title.add_css_class("metis-dw-title");
    let hint = gtk::Label::new(Some(&format!(
        "{} — install under ~/.local/share/metis/widgets/{id}/",
        if id.is_empty() { "(no id)" } else { id }
    )));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-dw-hint");
    col.append(&title);
    col.append(&hint);
    col.upcast()
}

fn error_card(msg: &str) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let hint = gtk::Label::new(Some(msg));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("metis-dw-hint");
    col.append(&hint);
    col.upcast()
}

fn build_node(
    node: &WidgetExtNode,
    settings: &serde_json::Map<String, serde_json::Value>,
    host: Option<&HostBindValues>,
    extension_id: Option<&str>,
) -> gtk::Widget {
    match node {
        WidgetExtNode::Column { spacing, children } => {
            let col = gtk::Box::new(gtk::Orientation::Vertical, (*spacing).max(0));
            for child in children {
                col.append(&build_node(child, settings, host, extension_id));
            }
            col.upcast()
        }
        WidgetExtNode::Row { spacing, children } => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, (*spacing).max(0));
            for child in children {
                row.append(&build_node(child, settings, host, extension_id));
            }
            row.upcast()
        }
        WidgetExtNode::List { spacing, children } => {
            let col = gtk::Box::new(gtk::Orientation::Vertical, (*spacing).max(0));
            col.add_css_class("metis-dw-ext-list");
            for child in children {
                col.append(&build_node(child, settings, host, extension_id));
            }
            col.upcast()
        }
        WidgetExtNode::Label { text, style } => {
            let rendered = interpolate_template(text, settings, host);
            let label = gtk::Label::new(Some(&rendered));
            label.set_xalign(0.0);
            label.set_wrap(true);
            label.set_use_markup(false);
            match style {
                WidgetExtLabelStyle::Title => label.add_css_class("metis-dw-title"),
                WidgetExtLabelStyle::Muted => label.add_css_class("metis-dw-hint"),
                WidgetExtLabelStyle::Body => label.add_css_class("metis-dw-body"),
            }
            if template_needs_host(text) {
                register_host_label(
                    label.clone(),
                    text.clone(),
                    settings.clone(),
                    extension_id.map(str::to_string),
                );
            }
            label.upcast()
        }
        WidgetExtNode::Icon { name, pixel_size } => {
            let icon = gtk::Image::from_icon_name(name);
            icon.set_pixel_size((*pixel_size).clamp(8, 128));
            icon.upcast()
        }
        WidgetExtNode::Separator => {
            let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
            sep.upcast()
        }
        WidgetExtNode::Button { label, on_click } => {
            // Button captions: settings only (no live host binds on buttons).
            let rendered = interpolate_settings(label, settings);
            let btn = gtk::Button::with_label(&rendered);
            btn.set_halign(gtk::Align::Fill);
            btn.add_css_class("metis-dw-ext-button");
            let action = on_click.clone();
            let settings = settings.clone();
            btn.connect_clicked(move |_| {
                run_action(&action, &settings);
            });
            btn.upcast()
        }
    }
}

fn register_host_label(
    label: gtk::Label,
    template: String,
    settings: serde_json::Map<String, serde_json::Value>,
    extension_id: Option<String>,
) {
    HOST_BOUNDS.with(|list| {
        list.borrow_mut().push(HostBoundLabel {
            label,
            template,
            settings,
            extension_id,
        });
    });
}

fn ensure_host_timer() {
    if HOST_TIMER.get() {
        return;
    }
    HOST_TIMER.set(true);
    glib::timeout_add_seconds_local(1, || {
        refresh_host_bounds();
        glib::ControlFlow::Continue
    });
}

fn refresh_host_bounds() {
    HELPER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let now = Instant::now();
        for entry in cache.values_mut() {
            if now < entry.next_poll {
                continue;
            }
            entry.values = run_helper_snapshot(&entry.exec).unwrap_or_default();
            entry.next_poll = now + entry.poll_every;
        }
    });

    HOST_BOUNDS.with(|list| {
        list.borrow_mut().retain(|bound| bound.label.is_visible());
        for bound in list.borrow().iter() {
            let host = sample_host_binds(bound.extension_id.as_deref());
            let text = interpolate_template(&bound.template, &bound.settings, Some(&host));
            bound.label.set_text(&text);
        }
    });
}

fn sample_host_binds(extension_id: Option<&str>) -> HostBindValues {
    let now = chrono::Local::now();
    let time = now.format("%H:%M:%S").to_string();
    let date = now.format("%a %b %-d").to_string();

    let (weather_temp, weather_unit, weather_summary) =
        match crate::services::last_weather_snapshot() {
            Some(snap) if !snap.locations.is_empty() => {
                let loc = &snap.locations[0];
                let unit = if snap.fahrenheit { "°F" } else { "°C" };
                (
                    format!("{:.0}", loc.temp),
                    unit.to_string(),
                    loc.label.clone(),
                )
            }
            _ => ("—".into(), String::new(), String::new()),
        };

    let (sys_cpu, sys_mem, sys_disk) = HOST_SYS.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.is_none() {
            *guard = Some(System::new());
        }
        let sys = guard.as_mut().expect("sys");
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu = format!("{:.0}%", sys.global_cpu_usage().clamp(0.0, 100.0));
        let mem = if sys.total_memory() > 0 {
            format!(
                "{:.0}%",
                (sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0
            )
        } else {
            "—".into()
        };
        let disks = Disks::new_with_refreshed_list();
        let disk = disks
            .list()
            .iter()
            .find(|d| d.mount_point() == std::path::Path::new("/"))
            .map(|d| {
                let total = d.total_space().max(1);
                let used = total.saturating_sub(d.available_space());
                format!("{:.0}%", (used as f64 / total as f64) * 100.0)
            })
            .unwrap_or_else(|| "—".into());
        (cpu, mem, disk)
    });

    let helper = extension_id
        .and_then(|id| HELPER_CACHE.with(|cache| cache.borrow().get(id).map(|e| e.values.clone())))
        .unwrap_or_default();

    HostBindValues {
        time,
        date,
        weather_temp,
        weather_unit,
        weather_summary,
        sys_cpu,
        sys_mem,
        sys_disk,
        helper,
    }
}

fn run_action(action: &WidgetExtAction, settings: &serde_json::Map<String, serde_json::Value>) {
    if let Err(err) = validate_action(action) {
        tracing::warn!(%err, "extension action blocked");
        return;
    }
    match action {
        WidgetExtAction::OpenUri { uri } => {
            let uri = uri.trim();
            if !is_safe_open_uri(uri) {
                tracing::warn!(uri = %uri, "extension open_uri blocked");
                return;
            }
            open_https_uri(uri);
        }
        WidgetExtAction::Launch { id, exec } => {
            let id = id.trim();
            let exec = exec.trim();
            if !id.is_empty() {
                if !is_safe_launch_id(id) {
                    tracing::warn!(id = %id, "extension launch id blocked");
                    return;
                }
                applications::launch_id(id);
            } else if !exec.is_empty() {
                if !is_safe_launch_exec(exec) {
                    tracing::warn!(exec = %exec, "extension launch exec blocked");
                    return;
                }
                if let Err(err) = crate::compositor::launch_argv([exec]) {
                    tracing::warn!(%err, exec = %exec, "extension launch failed");
                }
            } else {
                tracing::warn!("extension launch action missing id and exec");
            }
        }
        WidgetExtAction::CopyText { text } => {
            let text = interpolate_settings(text, settings);
            let clipped = if text.len() > WIDGET_EXT_MAX_COPY {
                let mut end = WIDGET_EXT_MAX_COPY;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                tracing::warn!("extension copy_text truncated");
                &text[..end]
            } else {
                text.as_str()
            };
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(clipped);
            }
        }
    }
}

fn open_https_uri(uri: &str) {
    let launcher = gtk::UriLauncher::new(uri);
    let uri_owned = uri.to_string();
    launcher.launch(
        None::<&gtk::Window>,
        None::<&gio::Cancellable>,
        move |res| {
            if let Err(err) = res {
                tracing::warn!(%err, uri = %uri_owned, "extension UriLauncher failed");
            }
        },
    );
}
