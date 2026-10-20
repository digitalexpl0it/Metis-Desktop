//! Software Updates window — Metis SSD xdg_toplevel with a forced-opaque surface.
//!
//! Must paint with fully-opaque `rgb()` (global shell CSS makes every `window`
//! transparent for the bar / OSD). Polkit auth is a layer-shell Overlay, so this
//! window can stay mapped while Authentication Required is shown on top.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use metis_remote::{UpdateApplyScope, UpdateItem, UpdateProgressEvent, UpdateSourceKind};

use crate::services;
use crate::ui::theme;

const DEFAULT_WIDTH: i32 = 520;
/// Cap on the updates list viewport (then it scrolls).
const LIST_MAX_HEIGHT: i32 = 280;
/// Estimated row / section heights for ScrolledWindow min size (ListBox often
/// reports 0 natural height until after the first allocate, which clipped the
/// list on first open).
const LIST_ROW_HEIGHT: i32 = 58;
const LIST_SECTION_HEIGHT: i32 = 32;
const LIST_PAD: i32 = 8;

thread_local! {
    static WINDOW: RefCell<Option<Rc<UpdaterState>>> = const { RefCell::new(None) };
    static OPAQUE_CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

struct UpdaterState {
    window: gtk::Window,
    hero_title: gtk::Label,
    hero_sub: gtk::Label,
    select_all: gtk::CheckButton,
    select_hint: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    progress: gtk::ProgressBar,
    /// Current package / app under the progress bar during install.
    progress_item: gtk::Label,
    status: gtk::Label,
    reboot_banner: gtk::Box,
    install_btn: gtk::Button,
    later_btn: gtk::Button,
    applying: Rc<Cell<bool>>,
    /// Checked item keys (`source\0ordinal\0id`). Default: everything selected.
    selected: Rc<RefCell<HashSet<String>>>,
    /// Unique selectable rows currently shown (for select-all sync).
    item_total: Rc<Cell<usize>>,
    /// Suppress select-all ↔ row sync feedback loops.
    syncing: Rc<Cell<bool>>,
    /// Per-package row status widgets (mini progress → Installed pill).
    row_status: Rc<RefCell<HashMap<String, RowStatusWidgets>>>,
}

/// Right-side status chrome for one update row.
#[derive(Clone)]
struct RowStatusWidgets {
    row: gtk::ListBoxRow,
    install_btn: gtk::Button,
    progress: gtk::ProgressBar,
    installed: gtk::Label,
    security: Option<gtk::Label>,
}

/// Stable per-row key. Ordinal keeps PackageKit duplicate IDs selectable
/// independently so "Select all" matches the visible checklist.
fn item_key(item: &UpdateItem, ordinal: usize) -> String {
    let src = match item.source {
        UpdateSourceKind::PackageKit => "PackageKit",
        UpdateSourceKind::Distro => "Distro",
        UpdateSourceKind::Flatpak => "Flatpak",
        UpdateSourceKind::Firmware => "Firmware",
    };
    format!("{src}\0{ordinal}\0{}", item.id)
}

fn package_id_from_key(key: &str) -> Option<(&str, &str)> {
    // source \0 ordinal \0 id
    let (src, rest) = key.split_once('\0')?;
    let (_ordinal, id) = rest.split_once('\0')?;
    Some((src, id))
}

fn source_icon(kind: UpdateSourceKind) -> &'static str {
    match kind {
        UpdateSourceKind::PackageKit | UpdateSourceKind::Distro => "package-x-generic-symbolic",
        UpdateSourceKind::Flatpak => "application-x-addon-symbolic",
        UpdateSourceKind::Firmware => "drive-harddisk-solidstate-symbolic",
    }
}

/// Present the updater (create once, reuse).
pub fn show() {
    // Clone out of the RefCell *before* the if/else so the temporary `borrow()`
    // does not live across `borrow_mut()` (that panic killed the whole edge bar).
    // Re-applied each time so the reused window follows theme changes.
    ensure_opaque_css();
    let existing = WINDOW.with(|cell| cell.borrow().clone());
    if let Some(state) = existing {
        refresh_content(&state);
        state.window.set_visible(true);
        state.window.present();
        schedule_fit_window(&state.window);
    } else {
        let state = Rc::new(build());
        refresh_content(&state);
        state.window.present();
        schedule_fit_window(&state.window);
        WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(state);
        });
    }
    // Soft re-check only when we have nothing pending — otherwise a PackageKit-only
    // pass can blank a list Settings just found (Flatpak/fwupd or a flaky pkcon).
    if services::updates_pending_count() == 0 {
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), || {
            services::updates_request_check(true);
        });
    }
}

/// Settings → Appearance sends `reload-theme`; refresh the USER-priority opaque
/// stylesheet so light/dark switches apply to an already-open updater.
pub fn on_theme_changed() {
    let open = WINDOW.with(|cell| cell.borrow().is_some());
    if open {
        ensure_opaque_css();
    }
}

/// Resize the toplevel to the child's natural height. GTK will grow a mapped
/// window for new content, but it will not shrink when the list shortens —
/// without an explicit size, finishing an install can leave a tall empty window.
fn fit_window_to_content(window: &gtk::Window) {
    if window.is_maximized() {
        window.unmaximize();
    }
    if window.is_fullscreen() {
        window.unfullscreen();
    }
    let Some(child) = window.child() else {
        return;
    };
    let (_, nat_h, _, _) = child.measure(gtk::Orientation::Vertical, DEFAULT_WIDTH);
    if nat_h > 0 {
        window.set_default_size(DEFAULT_WIDTH, nat_h);
    }
}

fn schedule_fit_window(window: &gtk::Window) {
    let window = window.clone();
    // After the current layout pass so ListBox rows report real sizes.
    glib::idle_add_local_once(move || {
        fit_window_to_content(&window);
    });
}

fn schedule_fit_window_after(window: &gtk::Window, delay: std::time::Duration) {
    let window = window.clone();
    glib::timeout_add_local_once(delay, move || {
        fit_window_to_content(&window);
    });
}

fn list_min_height(item_count: usize, section_count: usize) -> i32 {
    if item_count == 0 {
        return 0;
    }
    let h = (section_count as i32)
        .saturating_mul(LIST_SECTION_HEIGHT)
        .saturating_add((item_count as i32).saturating_mul(LIST_ROW_HEIGHT))
        .saturating_add(LIST_PAD);
    h.clamp(LIST_ROW_HEIGHT + LIST_SECTION_HEIGHT, LIST_MAX_HEIGHT)
}

fn ensure_opaque_css() {
    let tokens = theme::active_tokens();
    let surface_rgb = tokens.surface_rgb();
    let raised_rgb = tokens.surface_raised_rgb();
    let text = tokens.text.clone();
    let muted = tokens.text_muted.clone();
    let accent = tokens.accent_primary().to_string();
    let accent2 = tokens.accent_secondary().to_string();
    let text_rgb = tokens.text_rgb();
    let css = format!(
        r#"
        window.metis-updater,
        window.metis-updater.background {{
            background-color: rgb({raised_rgb});
            color: {text};
        }}
        window.metis-updater > box.metis-updater-shell,
        .metis-updater-shell {{
            background-color: rgb({raised_rgb});
            color: {text};
        }}
        .metis-updater-root {{
            background-color: rgb({raised_rgb});
            color: {text};
        }}
        .metis-updater-list {{
            background-color: rgb({surface_rgb});
            color: {text};
        }}
        window.metis-updater label.metis-updater-hero-title,
        window.metis-updater label.metis-updater-item-name {{
            color: {text};
        }}
        window.metis-updater label.metis-updater-hero-sub,
        window.metis-updater label.metis-updater-item-meta,
        window.metis-updater label.metis-updater-select-hint,
        window.metis-updater label.metis-updater-section,
        window.metis-updater label.metis-updater-status,
        window.metis-updater label.metis-updater-progress-item {{
            color: {muted};
        }}
        window.metis-updater label.metis-updater-progress-item {{
            font-feature-settings: "tnum";
            margin-top: 2px;
            margin-bottom: 4px;
        }}
        window.metis-updater checkbutton.metis-updater-check,
        window.metis-updater checkbutton.metis-updater-check label,
        window.metis-updater checkbutton.metis-updater-select-all,
        window.metis-updater checkbutton.metis-updater-select-all label {{
            color: {text};
        }}
        window.metis-updater checkbutton.metis-updater-check check,
        window.metis-updater checkbutton.metis-updater-select-all check {{
            min-width: 22px;
            min-height: 22px;
            border-radius: 999px;
            border: 2px solid rgba({text_rgb}, 0.40);
            background-color: rgba({text_rgb}, 0.06);
            color: transparent;
            -gtk-icon-size: 12px;
        }}
        window.metis-updater checkbutton.metis-updater-check:checked check,
        window.metis-updater checkbutton.metis-updater-check check:checked,
        window.metis-updater checkbutton.metis-updater-select-all:checked check,
        window.metis-updater checkbutton.metis-updater-select-all check:checked {{
            background-color: {accent};
            border-color: {accent};
            color: #ffffff;
            -gtk-icon-source: -gtk-icontheme("object-select-symbolic");
        }}
        window.metis-updater button.metis-updater-row-btn,
        window.metis-updater button.metis-updater-row-btn label,
        window.metis-updater button.metis-updater-btn,
        window.metis-updater button.metis-updater-btn label {{
            color: {text};
            background-image: none;
        }}
        window.metis-updater button.metis-updater-row-btn {{
            background-color: rgba({text_rgb}, 0.08);
            border: 1px solid rgba({text_rgb}, 0.18);
            min-width: 72px;
        }}
        window.metis-updater button.metis-updater-btn:not(.metis-updater-btn-primary) {{
            background-color: rgb({surface_rgb});
            border: 1px solid rgba({text_rgb}, 0.18);
        }}
        window.metis-updater button.metis-updater-btn-primary,
        window.metis-updater button.metis-updater-btn-primary label {{
            background-color: {accent};
            color: #ffffff;
            border: 1px solid {accent};
            box-shadow: none;
            background-image: none;
        }}
        window.metis-updater button.metis-updater-btn-primary:hover,
        window.metis-updater button.metis-updater-btn-primary:hover label {{
            background-color: {accent2};
            color: #ffffff;
            border-color: {accent2};
            box-shadow: none;
            background-image: none;
        }}
        window.metis-updater label.metis-updater-badge {{
            color: {accent};
            font-size: 0.85em;
            font-weight: 600;
            padding: 2px 8px;
            border-radius: 999px;
            background-color: rgba({text_rgb}, 0.08);
            border: 1px solid rgba({text_rgb}, 0.14);
        }}
        window.metis-updater label.metis-updater-installed {{
            color: #ffffff;
            font-size: 0.8em;
            font-weight: 600;
            padding: 3px 10px;
            border-radius: 999px;
            background-color: {accent};
            border: 1px solid {accent};
        }}
        @keyframes metis-updater-stripe {{
            from {{ background-position: 0 0; }}
            to {{ background-position: 24px 0; }}
        }}
        window.metis-updater progressbar.metis-updater-row-progress {{
            min-width: 88px;
            min-height: 12px;
        }}
        window.metis-updater progressbar.metis-updater-row-progress trough {{
            min-height: 12px;
            border-radius: 999px;
            background-color: rgba({text_rgb}, 0.12);
        }}
        window.metis-updater progressbar.metis-updater-row-progress progress {{
            min-height: 12px;
            border-radius: 999px;
            background-color: {accent};
        }}
        window.metis-updater progressbar.metis-updater-row-progress-active progress {{
            min-height: 12px;
            border-radius: 999px;
            background-color: {accent};
            background-image: repeating-linear-gradient(
                -45deg,
                {accent},
                {accent} 5px,
                #ffffff 5px,
                #ffffff 10px
            );
            background-size: 20px 20px;
            animation: metis-updater-stripe 0.55s linear infinite;
        }}
        "#
    );
    let Some(display) = gdk::Display::default() else {
        tracing::warn!("no GDK display — updater keeps the shell theme");
        return;
    };
    OPAQUE_CSS.with(|slot| {
        let mut binding = slot.borrow_mut();
        let fresh = binding.is_none();
        let provider = binding.get_or_insert_with(gtk::CssProvider::new);
        provider.load_from_string(&css);
        if fresh {
            // USER beats the shell's APPLICATION-level `window { transparent }`.
            gtk::style_context_add_provider_for_display(
                &display,
                provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    });
}

fn build() -> UpdaterState {
    let under_metis = std::env::var_os("METIS_SESSION").is_some();
    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Software Updates"))
        .default_width(DEFAULT_WIDTH)
        .default_height(-1)
        // Fixed dialog — maximize / free resize stretch the layout badly.
        .resizable(false)
        // Under Metis the compositor draws SSD (move / close). Elsewhere CSD.
        .decorated(!under_metis)
        .build();
    window.add_css_class("metis-updater");
    window.add_css_class("background");
    // Belt-and-suspenders: if SSD maximize still fires, bounce back to fit.
    {
        let w = window.clone();
        window.connect_maximized_notify(move |win| {
            if win.is_maximized() {
                win.unmaximize();
                schedule_fit_window_after(&w, std::time::Duration::from_millis(50));
            }
        });
    }
    {
        let w = window.clone();
        window.connect_fullscreened_notify(move |win| {
            if win.is_fullscreen() {
                win.unfullscreen();
                schedule_fit_window_after(&w, std::time::Duration::from_millis(50));
            }
        });
    }

    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("metis-updater-shell");
    shell.set_hexpand(true);
    shell.set_halign(gtk::Align::Fill);
    shell.set_valign(gtk::Align::Start);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.add_css_class("metis-updater-root");
    root.set_hexpand(true);
    root.set_halign(gtk::Align::Fill);
    root.set_valign(gtk::Align::Start);
    root.set_size_request(DEFAULT_WIDTH, -1);

    // —— Hero ————————————————————————————————————————————————
    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    hero.add_css_class("metis-updater-hero");
    hero.set_halign(gtk::Align::Fill);

    let icon_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon_wrap.add_css_class("metis-updater-hero-icon");
    icon_wrap.set_valign(gtk::Align::Center);
    icon_wrap.set_halign(gtk::Align::Center);
    icon_wrap.set_size_request(52, 52);
    let hero_icon = gtk::Image::from_icon_name("software-update-available-symbolic");
    hero_icon.set_pixel_size(28);
    hero_icon.set_halign(gtk::Align::Center);
    hero_icon.set_valign(gtk::Align::Center);
    hero_icon.set_hexpand(true);
    hero_icon.set_vexpand(true);
    icon_wrap.append(&hero_icon);
    hero.append(&icon_wrap);

    let hero_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    hero_text.set_hexpand(true);
    hero_text.set_valign(gtk::Align::Center);
    let hero_title = gtk::Label::new(Some(&metis_i18n::tr("Software Updates")));
    hero_title.set_halign(gtk::Align::Start);
    hero_title.add_css_class("metis-updater-hero-title");
    let hero_sub = gtk::Label::new(None);
    hero_sub.set_halign(gtk::Align::Start);
    hero_sub.set_wrap(true);
    hero_sub.add_css_class("metis-updater-hero-sub");
    hero_text.append(&hero_title);
    hero_text.append(&hero_sub);
    hero.append(&hero_text);
    root.append(&hero);

    let reboot_banner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    reboot_banner.add_css_class("metis-updater-reboot");
    reboot_banner.set_visible(false);
    let reboot_icon = gtk::Image::from_icon_name("system-reboot-symbolic");
    reboot_icon.set_pixel_size(18);
    let reboot_lbl = gtk::Label::new(Some(&metis_i18n::tr(
        "A restart is required to finish installing updates.",
    )));
    reboot_lbl.set_wrap(true);
    reboot_lbl.set_halign(gtk::Align::Start);
    reboot_lbl.set_hexpand(true);
    reboot_banner.append(&reboot_icon);
    reboot_banner.append(&reboot_lbl);
    root.append(&reboot_banner);

    // —— Selection toolbar ————————————————————————————————————
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("metis-updater-toolbar");
    let select_all = gtk::CheckButton::with_label(&metis_i18n::tr("Select all"));
    select_all.set_active(true);
    select_all.add_css_class("metis-updater-select-all");
    select_all.add_css_class("metis-updater-check");
    let select_hint = gtk::Label::new(None);
    select_hint.set_halign(gtk::Align::End);
    select_hint.set_hexpand(true);
    select_hint.add_css_class("metis-updater-select-hint");
    toolbar.append(&select_all);
    toolbar.append(&select_hint);
    root.append(&toolbar);

    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(false)
        .propagate_natural_height(true)
        .max_content_height(LIST_MAX_HEIGHT)
        // Non-zero floor: ListBox natural height is often 0 on first map, which
        // collapsed this viewport to a few pixels (list looked "cut off").
        .min_content_height(LIST_SECTION_HEIGHT + LIST_ROW_HEIGHT)
        .build();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.add_css_class("metis-updater-scroll");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("metis-updater-list");
    scroller.set_child(Some(&list));
    root.append(&scroller);

    let status = gtk::Label::new(None);
    status.set_halign(gtk::Align::Start);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.add_css_class("metis-updater-status");
    status.set_visible(false);
    root.append(&status);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    progress.add_css_class("metis-updater-progress");
    root.append(&progress);

    let progress_item = gtk::Label::new(None);
    progress_item.set_halign(gtk::Align::Start);
    progress_item.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    progress_item.set_hexpand(true);
    progress_item.add_css_class("metis-updater-progress-item");
    progress_item.set_visible(false);
    root.append(&progress_item);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    actions.add_css_class("metis-updater-actions");
    actions.set_halign(gtk::Align::End);
    let later_btn = gtk::Button::with_label(&metis_i18n::tr("Later"));
    later_btn.add_css_class("metis-updater-btn");
    let install_btn = gtk::Button::with_label(&metis_i18n::tr("Update all"));
    // Do NOT add Adwaita's `suggested-action` — it paints its own inset fill on
    // top of our chrome and reads as a button stacked on a button.
    install_btn.add_css_class("metis-updater-btn");
    install_btn.add_css_class("metis-updater-btn-primary");
    actions.append(&later_btn);
    actions.append(&install_btn);
    root.append(&actions);

    shell.append(&root);
    window.set_child(Some(&shell));

    let applying = Rc::new(Cell::new(false));
    let selected = Rc::new(RefCell::new(HashSet::new()));
    let syncing = Rc::new(Cell::new(false));

    {
        // Weak ref, no WINDOW access: `close()` synchronously emits
        // `close_request`, and touching the RefCell here panicked the shell.
        let window_weak = window.downgrade();
        later_btn.connect_clicked(move |_| {
            if let Some(window) = window_weak.upgrade() {
                window.close();
            }
        });
    }

    let state_for_install = Rc::new(RefCell::new(None::<UpdaterHandles>));
    {
        let applying = applying.clone();
        let handles_slot = state_for_install.clone();
        let selected = selected.clone();
        let parent = window.clone();
        let list = list.clone();
        install_btn.connect_clicked(move |btn| {
            let scope = scope_from_list(&list, &selected.borrow());
            begin_apply(&applying, btn, &handles_slot, &parent, scope);
        });
    }

    // Hide instead of destroy: an in-flight install still holds the widgets,
    // and `show()` reuses the same window. No RefCell access on this path.
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });

    let row_status = Rc::new(RefCell::new(HashMap::new()));
    let state = UpdaterState {
        window,
        hero_title,
        hero_sub,
        select_all: select_all.clone(),
        select_hint,
        list,
        scroller,
        progress,
        progress_item,
        status,
        reboot_banner,
        install_btn: install_btn.clone(),
        later_btn,
        applying,
        selected,
        item_total: Rc::new(Cell::new(0)),
        syncing: syncing.clone(),
        row_status: row_status.clone(),
    };

    *state_for_install.borrow_mut() = Some(UpdaterHandles {
        progress: state.progress.clone(),
        progress_item: state.progress_item.clone(),
        status: state.status.clone(),
        reboot_banner: state.reboot_banner.clone(),
        row_status,
        list: state.list.clone(),
        scroller: state.scroller.clone(),
    });

    {
        let selected = state.selected.clone();
        let syncing = syncing.clone();
        let list = state.list.clone();
        let install_btn = state.install_btn.clone();
        let select_hint = state.select_hint.clone();
        let item_total = state.item_total.clone();
        select_all.connect_toggled(move |btn| {
            if syncing.get() {
                return;
            }
            syncing.set(true);
            let on = btn.is_active();
            let mut sel = selected.borrow_mut();
            let mut child = list.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                let Ok(row) = widget.downcast::<gtk::ListBoxRow>() else {
                    continue;
                };
                if let Some(check) = row_check(&row) {
                    check.set_active(on);
                    if let Some(key) = row_key(&row) {
                        if on {
                            sel.insert(key);
                        } else {
                            sel.remove(&key);
                        }
                    }
                }
            }
            drop(sel);
            syncing.set(false);
            let n = selected.borrow().len();
            let total = item_total.get();
            if total > 0 && n == total {
                install_btn.set_label(&metis_i18n::tr("Update all"));
                install_btn.set_sensitive(true);
                select_hint.set_text(&metis_i18n::tr("%1 selected").replace("%1", &n.to_string()));
            } else {
                update_selection_chrome(&install_btn, &select_hint, &selected.borrow(), true);
            }
        });
    }

    {
        let refresh: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(|| {
            WINDOW.with(|cell| {
                if let Some(state) = cell.borrow().as_ref()
                    && state.window.is_visible()
                {
                    refresh_content(state);
                }
            });
        });
        services::register_updates_updater_refresh(refresh);
    }

    state
}

fn begin_apply(
    applying: &Rc<Cell<bool>>,
    btn: &gtk::Button,
    handles_slot: &Rc<RefCell<Option<UpdaterHandles>>>,
    parent: &gtk::Window,
    scope: UpdateApplyScope,
) {
    if applying.get() {
        return;
    }
    if scope.is_empty_selection() {
        return;
    }
    applying.set(true);
    btn.set_sensitive(false);
    let Some(handles) = handles_slot.borrow().clone() else {
        applying.set(false);
        btn.set_sensitive(true);
        return;
    };

    let apply_total = scope_item_count(&scope).max(1);
    let completed = Rc::new(Cell::new(0usize));
    // Short names in list order — used to mark earlier rows Installed when
    // PackageKit reports a later package (dependency order ≠ A→Z).
    let apply_order = Rc::new(apply_order_names(
        &handles.list,
        &handles.row_status.borrow(),
        &scope,
    ));
    prepare_selected_rows(&handles.row_status.borrow(), &scope);

    handles.progress.set_visible(true);
    handles.progress.set_fraction(0.0);
    handles.progress.set_show_text(true);
    set_count_progress(&handles, 0, apply_total);
    handles.progress_item.set_visible(true);
    handles.progress_item.set_text("");
    handles.status.set_visible(true);
    // Flatpak-only installs usually need no polkit; don't imply a password
    // prompt that will never appear.
    let needs_auth_hint = match &scope {
        UpdateApplyScope::All => true,
        UpdateApplyScope::Selected {
            packages,
            flatpaks,
            firmware,
        } => !packages.is_empty() || !firmware.is_empty() || flatpaks.is_empty(),
    };
    handles.status.set_text(&if needs_auth_hint {
        metis_i18n::tr("Waiting for authentication…")
    } else {
        metis_i18n::tr("Updating Flatpak apps…")
    });

    // Polkit auth is a layer-shell Overlay above this xdg window — leave the
    // updater visible so progress resumes in place after authentication.

    // Pulse the main bar only until the first package starts (auth wait).
    let pulse_stop = Rc::new(Cell::new(false));
    let saw_item = Rc::new(Cell::new(false));
    let conffile_pending = Rc::new(Cell::new(false));
    let track = ApplyTrack {
        current_item: Rc::new(RefCell::new(None)),
        completed,
        apply_total,
        saw_item: saw_item.clone(),
        pulse_stop: pulse_stop.clone(),
        apply_order,
    };
    {
        let progress = handles.progress.clone();
        let stop = pulse_stop.clone();
        let saw = saw_item.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            if stop.get() {
                return glib::ControlFlow::Break;
            }
            if !saw.get() {
                progress.pulse();
            }
            glib::ControlFlow::Continue
        });
    }

    let applying_cb = applying.clone();
    let install_btn = btn.clone();
    let parent_win = parent.clone();
    let scope_for_events = scope.clone();
    let on_event: Rc<dyn Fn(UpdateProgressEvent)> = Rc::new(move |ev| {
        match ev {
            UpdateProgressEvent::Log { .. } => {
                // Log UI removed — events are still emitted by the worker.
            }
            UpdateProgressEvent::Progress { percent, item } => {
                if let Some(name) = item {
                    // Ignore size/count tokens that slipped through, and names
                    // that are not in this apply set.
                    if find_row_status(&handles.row_status.borrow(), &name).is_some() {
                        advance_current_item(&handles, &track, &name);
                        row_set_active(&handles.row_status.borrow(), &name, percent);
                        scroll_row_into_view(&handles, &name);
                    } else if let Some(cur) = track.current_item.borrow().as_deref() {
                        row_set_active(&handles.row_status.borrow(), cur, percent);
                    }
                } else if let Some(name) = track.current_item.borrow().as_deref() {
                    row_set_active(&handles.row_status.borrow(), name, percent);
                }
            }
            UpdateProgressEvent::Phase { name } => {
                handles.status.set_text(&name);
                if !track.saw_item.get() {
                    set_count_progress(&handles, track.completed.get(), track.apply_total);
                }
            }
            UpdateProgressEvent::ConffileConflict {
                package,
                config_path,
            } => {
                conffile_pending.set(true);
                track.pulse_stop.set(true);
                handles.status.set_text(&metis_i18n::tr(
                    "Configuration file conflict — choose an option…",
                ));
                set_count_progress(&handles, track.completed.get(), track.apply_total);
                let applying = applying_cb.clone();
                let install_btn = install_btn.clone();
                let handles = handles.clone();
                present_conffile_dialog(
                    &parent_win,
                    &package,
                    config_path.as_deref(),
                    move |choice| match choice {
                        None => {
                            applying.set(false);
                            install_btn.set_sensitive(true);
                            handles.status.set_text(&metis_i18n::tr(
                                "Update paused — finish the config conflict to continue",
                            ));
                            WINDOW.with(|cell| {
                                if let Some(state) = cell.borrow().as_ref() {
                                    refresh_content(state);
                                }
                            });
                        }
                        Some(choice) => {
                            handles
                                .status
                                .set_text(&metis_i18n::tr("Finishing package configuration…"));
                            handles.progress.set_visible(true);
                            handles.progress.pulse();
                            let applying = applying.clone();
                            let install_btn = install_btn.clone();
                            let handles = handles.clone();
                            let on_done: Rc<dyn Fn(Result<(), String>)> = Rc::new(move |result| {
                                applying.set(false);
                                install_btn.set_sensitive(true);
                                match result {
                                    Ok(()) => {
                                        set_count_progress(&handles, apply_total, apply_total);
                                        handles.progress_item.set_visible(false);
                                        handles
                                            .status
                                            .set_text(&metis_i18n::tr("Updates installed"));
                                    }
                                    Err(err) => {
                                        handles.status.set_text(&err);
                                    }
                                }
                                WINDOW.with(|cell| {
                                    if let Some(state) = cell.borrow().as_ref() {
                                        refresh_content(state);
                                        state.status.set_visible(true);
                                    }
                                });
                            });
                            services::updates_resolve_conffile(choice, on_done);
                        }
                    },
                );
            }
            UpdateProgressEvent::Finished {
                ok,
                error,
                reboot_required,
            } => {
                // Conffile dialog owns the rest of the lifecycle.
                if conffile_pending.get() {
                    return;
                }
                track.pulse_stop.set(true);
                applying_cb.set(false);
                install_btn.set_sensitive(true);
                if ok {
                    if let Some(prev) = track.current_item.borrow_mut().take() {
                        row_set_installed(&handles.row_status.borrow(), &prev);
                    }
                    mark_scope_installed(&handles.row_status.borrow(), &scope_for_events);
                    track.completed.set(track.apply_total);
                    set_count_progress(&handles, track.apply_total, track.apply_total);
                    handles.progress_item.set_text("");
                    handles.progress_item.set_visible(false);
                    handles
                        .status
                        .set_text(&metis_i18n::tr("Updates installed"));
                } else {
                    handles
                        .status
                        .set_text(&error.unwrap_or_else(|| metis_i18n::tr("Update failed")));
                }
                let status_msg = handles.status.text().to_string();
                let progress_frac = handles.progress.fraction();
                let progress_text = handles.progress.text().map(|s| s.to_string());
                handles.reboot_banner.set_visible(reboot_required);
                WINDOW.with(|cell| {
                    if let Some(state) = cell.borrow().as_ref() {
                        refresh_content(state);
                        state.status.set_visible(true);
                        state.status.set_text(&status_msg);
                        state.progress.set_visible(true);
                        state.progress.set_fraction(progress_frac);
                        state.progress.set_show_text(true);
                        state.progress_item.set_visible(false);
                        if let Some(text) = progress_text.as_deref() {
                            state.progress.set_text(Some(text));
                        }
                        state
                            .reboot_banner
                            .set_visible(reboot_required || state.reboot_banner.is_visible());
                    }
                });
            }
        }
    });
    services::updates_start_apply_scope(scope, on_event);
}

fn scope_item_count(scope: &UpdateApplyScope) -> usize {
    match scope {
        UpdateApplyScope::All => services::updates_pending_count(),
        UpdateApplyScope::Selected {
            packages,
            flatpaks,
            firmware,
        } => packages.len() + flatpaks.len() + firmware.len(),
    }
}

fn set_count_progress(handles: &UpdaterHandles, completed: usize, total: usize) {
    let total = total.max(1);
    let done = completed.min(total);
    handles.progress.set_fraction(done as f64 / total as f64);
    handles
        .progress
        .set_text(Some(&format!("{done} / {total}")));
}

struct ApplyTrack {
    current_item: Rc<RefCell<Option<String>>>,
    completed: Rc<Cell<usize>>,
    apply_total: usize,
    saw_item: Rc<Cell<bool>>,
    pulse_stop: Rc<Cell<bool>>,
    apply_order: Rc<Vec<String>>,
}

fn advance_current_item(handles: &UpdaterHandles, track: &ApplyTrack, name: &str) {
    track.saw_item.set(true);
    track.pulse_stop.set(true);
    let map = handles.row_status.borrow();
    let mut slot = track.current_item.borrow_mut();
    if slot.as_deref() == Some(name) {
        return;
    }
    // Mark every listed package before this one as done (PackageKit may skip
    // reporting some names when it jumps ahead for deps).
    if let Some(idx) = track
        .apply_order
        .iter()
        .position(|p| p.eq_ignore_ascii_case(name))
    {
        for prev in track.apply_order.iter().take(idx) {
            row_set_installed_widgets(map.get(prev));
        }
        track.completed.set(idx.min(track.apply_total));
    } else if let Some(prev) = slot.as_deref() {
        row_set_installed(&map, prev);
        track.completed.set(
            track
                .completed
                .get()
                .saturating_add(1)
                .min(track.apply_total),
        );
    }
    *slot = Some(name.to_string());
    drop(slot);
    drop(map);
    handles.progress_item.set_visible(true);
    handles.progress_item.set_text(name);
    set_count_progress(handles, track.completed.get(), track.apply_total);
}

fn apply_order_names(
    list: &gtk::ListBox,
    map: &HashMap<String, RowStatusWidgets>,
    scope: &UpdateApplyScope,
) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    let push_id = |ordered: &mut Vec<String>, seen: &mut HashSet<String>, id: &str| {
        let key = id.trim().to_ascii_lowercase();
        let short = key.split(';').next().unwrap_or(&key).to_string();
        let flatpak = key.split('/').nth(1).map(str::to_string);
        for candidate in [short, key].into_iter().chain(flatpak) {
            if map.contains_key(&candidate) && seen.insert(candidate.clone()) {
                ordered.push(candidate);
                return;
            }
        }
    };
    match scope {
        UpdateApplyScope::All => {
            // Visual top→bottom order from the list.
            let mut child = list.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                let Ok(row) = widget.downcast::<gtk::ListBoxRow>() else {
                    continue;
                };
                let Some(key) = row_key(&row) else {
                    continue;
                };
                if let Some((_, id)) = package_id_from_key(&key) {
                    push_id(&mut ordered, &mut seen, id);
                }
            }
        }
        UpdateApplyScope::Selected {
            packages,
            flatpaks,
            firmware,
        } => {
            // Scope vectors are already list-ordered from `scope_from_list`.
            for id in packages
                .iter()
                .chain(flatpaks.iter())
                .chain(firmware.iter())
            {
                push_id(&mut ordered, &mut seen, id);
            }
        }
    }
    ordered
}

fn row_match_keys(item: &UpdateItem) -> Vec<String> {
    let mut keys = Vec::new();
    let push = |keys: &mut Vec<String>, raw: &str| {
        let k = raw.trim().to_ascii_lowercase();
        if !k.is_empty() && !keys.iter().any(|e| e == &k) {
            keys.push(k);
        }
    };
    push(&mut keys, &item.name);
    push(&mut keys, &item.id);
    if let Some(base) = item.id.split(';').next() {
        push(&mut keys, base);
    }
    // Flatpak refs: app/org.Foo.Bar/x86_64/stable → org.foo.bar
    if let Some(app_id) = item.id.split('/').nth(1) {
        push(&mut keys, app_id);
    }
    keys
}

fn find_row_status<'a>(
    map: &'a HashMap<String, RowStatusWidgets>,
    hint: &str,
) -> Option<&'a RowStatusWidgets> {
    let hint = hint.trim().to_ascii_lowercase();
    if hint.len() < 2 {
        return None;
    }
    if let Some(row) = map.get(&hint) {
        return Some(row);
    }
    let short = hint.split(';').next().unwrap_or(&hint);
    if short != hint
        && let Some(row) = map.get(short)
    {
        return Some(row);
    }
    if let Some(app_id) = hint.split('/').nth(1)
        && let Some(row) = map.get(app_id)
    {
        return Some(row);
    }
    None
}

fn row_set_idle(row: &RowStatusWidgets) {
    row.progress.set_visible(false);
    row.installed.set_visible(false);
    row.install_btn.set_visible(true);
    row.install_btn.set_sensitive(true);
    if let Some(badge) = &row.security {
        badge.set_visible(true);
    }
}

fn row_set_waiting(row: &RowStatusWidgets) {
    if let Some(badge) = &row.security {
        badge.set_visible(false);
    }
    row.install_btn.set_visible(false);
    row.installed.set_visible(false);
    row.progress.set_visible(true);
    row.progress.set_fraction(0.12);
    row.progress
        .add_css_class("metis-updater-row-progress-active");
}

fn row_set_active(map: &HashMap<String, RowStatusWidgets>, hint: &str, percent: u32) {
    let Some(row) = find_row_status(map, hint) else {
        return;
    };
    if let Some(badge) = &row.security {
        badge.set_visible(false);
    }
    row.install_btn.set_visible(false);
    row.installed.set_visible(false);
    row.progress.set_visible(true);
    row.progress
        .add_css_class("metis-updater-row-progress-active");
    // PackageKit often reports a global %, not per-package — keep a visible
    // stripe even when percent is 0, and clamp so the bar never looks empty.
    let frac = if percent == 0 {
        0.45
    } else {
        (f64::from(percent.clamp(12, 100)) / 100.0).clamp(0.2, 1.0)
    };
    row.progress.set_fraction(frac);
}

fn row_set_installed(map: &HashMap<String, RowStatusWidgets>, hint: &str) {
    row_set_installed_widgets(find_row_status(map, hint));
}

fn row_set_installed_widgets(row: Option<&RowStatusWidgets>) {
    let Some(row) = row else {
        return;
    };
    if let Some(badge) = &row.security {
        badge.set_visible(false);
    }
    row.install_btn.set_visible(false);
    row.progress.set_visible(false);
    row.progress
        .remove_css_class("metis-updater-row-progress-active");
    row.installed.set_visible(true);
}

fn prepare_selected_rows(map: &HashMap<String, RowStatusWidgets>, scope: &UpdateApplyScope) {
    // Reset everything to idle first, then arm the selected set.
    let mut seen = HashSet::new();
    for row in map.values() {
        let ptr = row.row.as_ptr() as usize;
        if seen.insert(ptr) {
            row_set_idle(row);
        }
    }
    match scope {
        UpdateApplyScope::All => {
            seen.clear();
            for row in map.values() {
                let ptr = row.row.as_ptr() as usize;
                if seen.insert(ptr) {
                    row_set_waiting(row);
                }
            }
        }
        UpdateApplyScope::Selected {
            packages,
            flatpaks,
            firmware,
        } => {
            for id in packages
                .iter()
                .chain(flatpaks.iter())
                .chain(firmware.iter())
            {
                if let Some(row) = find_row_status(map, id) {
                    row_set_waiting(row);
                }
            }
        }
    }
}

fn mark_scope_installed(map: &HashMap<String, RowStatusWidgets>, scope: &UpdateApplyScope) {
    match scope {
        UpdateApplyScope::All => {
            let mut seen = HashSet::new();
            for row in map.values() {
                let ptr = row.row.as_ptr() as usize;
                if seen.insert(ptr) {
                    row_set_installed_widgets(Some(row));
                }
            }
        }
        UpdateApplyScope::Selected {
            packages,
            flatpaks,
            firmware,
        } => {
            for id in packages
                .iter()
                .chain(flatpaks.iter())
                .chain(firmware.iter())
            {
                row_set_installed(map, id);
            }
        }
    }
}

fn scroll_row_into_view(handles: &UpdaterHandles, hint: &str) {
    let map = handles.row_status.borrow();
    let Some(widgets) = find_row_status(&map, hint) else {
        return;
    };
    let row = widgets.row.clone();
    let list = handles.list.clone();
    let scroller = handles.scroller.clone();
    drop(map);
    // After the row is allocated so compute_bounds is meaningful.
    glib::idle_add_local_once(move || {
        let Some(bounds) = row.compute_bounds(&list) else {
            return;
        };
        let adj = scroller.vadjustment();
        let row_top = f64::from(bounds.y());
        let row_bottom = row_top + f64::from(bounds.height());
        let view_top = adj.value();
        let view_bottom = view_top + adj.page_size();
        if row_top < view_top {
            adj.set_value(row_top.max(0.0));
        } else if row_bottom > view_bottom {
            adj.set_value((row_bottom - adj.page_size()).max(0.0));
        }
    });
}

fn present_conffile_dialog(
    parent: &gtk::Window,
    package: &str,
    config_path: Option<&str>,
    on_choice: impl Fn(Option<metis_remote::ConfFileChoice>) + 'static,
) {
    let detail = match config_path {
        Some(path) => metis_i18n::tr(
            "The package “%1” wants to replace a configuration file you changed:\n\n%2\n\nKeep your version, or install the package maintainer’s version?",
        )
        .replace("%1", package)
        .replace("%2", path),
        None => metis_i18n::tr(
            "The package “%1” has a configuration file conflict. Keep your version, or install the package maintainer’s version?",
        )
        .replace("%1", package),
    };
    let keep = metis_i18n::tr("Keep my version");
    let pkg = metis_i18n::tr("Use package version");
    let cancel = metis_i18n::tr("Cancel");
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(metis_i18n::tr("Configuration file conflict"))
        .detail(detail)
        .buttons([keep.as_str(), pkg.as_str(), cancel.as_str()])
        .cancel_button(2)
        .default_button(0)
        .build();
    dialog.choose(
        Some(parent),
        Option::<&gtk::gio::Cancellable>::None,
        move |result| {
            let choice = match result {
                Ok(0) => Some(metis_remote::ConfFileChoice::KeepLocal),
                Ok(1) => Some(metis_remote::ConfFileChoice::UsePackage),
                _ => None,
            };
            on_choice(choice);
        },
    );
}

#[derive(Clone)]
struct UpdaterHandles {
    progress: gtk::ProgressBar,
    progress_item: gtk::Label,
    status: gtk::Label,
    reboot_banner: gtk::Box,
    row_status: Rc<RefCell<HashMap<String, RowStatusWidgets>>>,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
}

/// Build apply scope by walking the list top→bottom so PackageKit gets ids in
/// on-screen order (deps may still reorder, but we prefer list order).
fn scope_from_list(list: &gtk::ListBox, selected: &HashSet<String>) -> UpdateApplyScope {
    let mut packages = Vec::new();
    let mut flatpaks = Vec::new();
    let mut firmware = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        let Ok(row) = widget.downcast::<gtk::ListBoxRow>() else {
            continue;
        };
        let Some(key) = row_key(&row) else {
            continue;
        };
        if !selected.contains(&key) {
            continue;
        }
        let Some((src, id)) = package_id_from_key(&key) else {
            continue;
        };
        match src {
            "PackageKit" | "Distro" => packages.push(id.to_string()),
            "Flatpak" => flatpaks.push(id.to_string()),
            "Firmware" => firmware.push(id.to_string()),
            _ => {}
        }
    }
    dedup_preserve(&mut packages);
    dedup_preserve(&mut flatpaks);
    dedup_preserve(&mut firmware);
    UpdateApplyScope::Selected {
        packages,
        flatpaks,
        firmware,
    }
}

fn dedup_preserve(ids: &mut Vec<String>) {
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
}

fn scope_for_item(item: &UpdateItem) -> UpdateApplyScope {
    match item.source {
        UpdateSourceKind::PackageKit | UpdateSourceKind::Distro => UpdateApplyScope::Selected {
            packages: vec![item.id.clone()],
            flatpaks: Vec::new(),
            firmware: Vec::new(),
        },
        UpdateSourceKind::Flatpak => UpdateApplyScope::Selected {
            packages: Vec::new(),
            flatpaks: vec![item.id.clone()],
            firmware: Vec::new(),
        },
        UpdateSourceKind::Firmware => UpdateApplyScope::Selected {
            packages: Vec::new(),
            flatpaks: Vec::new(),
            firmware: vec![item.id.clone()],
        },
    }
}

fn update_selection_chrome(
    install_btn: &gtk::Button,
    hint: &gtk::Label,
    selected: &HashSet<String>,
    has_updates: bool,
) {
    let n = selected.len();
    if !has_updates {
        install_btn.set_label(&metis_i18n::tr("Update all"));
        install_btn.set_sensitive(false);
        hint.set_text("");
        return;
    }
    hint.set_text(&if n == 0 {
        metis_i18n::tr("Nothing selected")
    } else {
        metis_i18n::tr("%1 selected").replace("%1", &n.to_string())
    });
    let label = if n == 0 {
        metis_i18n::tr("Update selected")
    } else {
        metis_i18n::tr("Update selected (%1)").replace("%1", &n.to_string())
    };
    install_btn.set_label(&label);
    install_btn.set_sensitive(n > 0);
}

fn refresh_content(state: &UpdaterState) {
    let snap = services::updates_snapshot();
    let count = snap.total_count();

    while let Some(row) = state.list.first_child() {
        state.list.remove(&row);
    }
    state.row_status.borrow_mut().clear();

    // Drop keys that are no longer pending; newly seen items default to selected.
    {
        let mut selected = state.selected.borrow_mut();
        let mut live = HashSet::new();
        for (ordinal, item) in snap
            .packages
            .iter()
            .chain(snap.flatpaks.iter())
            .chain(snap.firmware.iter())
            .enumerate()
        {
            let key = item_key(item, ordinal);
            live.insert(key.clone());
            selected.insert(key);
        }
        selected.retain(|k| live.contains(k));
        state.item_total.set(live.len());
    }

    if count == 0 {
        state
            .hero_sub
            .set_text(&metis_i18n::tr("Your system is up to date."));
        state.hero_title.set_text(&metis_i18n::tr("You're all set"));
        state.install_btn.set_sensitive(false);
        state.later_btn.set_sensitive(true);
        state.later_btn.set_label(&metis_i18n::tr("Close"));
        state.scroller.set_visible(false);
        state.select_all.set_visible(false);
        state.select_hint.set_visible(false);
        state.install_btn.set_label(&metis_i18n::tr("Update all"));
    } else {
        state
            .hero_title
            .set_text(&metis_i18n::tr("Software Updates"));
        state.hero_sub.set_text(
            &metis_i18n::tr("%1 update(s) ready to install.").replace("%1", &count.to_string()),
        );
        state.later_btn.set_label(&metis_i18n::tr("Later"));
        state.scroller.set_visible(true);
        state.select_all.set_visible(true);
        state.select_hint.set_visible(true);
        if !state.applying.get() {
            state.later_btn.set_sensitive(true);
        }
    }

    let mut ordinal = 0usize;
    let mut zebra = 0usize;
    append_section(
        state,
        &metis_i18n::tr("System packages"),
        &snap.packages,
        &mut ordinal,
        &mut zebra,
    );
    append_section(
        state,
        &metis_i18n::tr("Flatpak"),
        &snap.flatpaks,
        &mut ordinal,
        &mut zebra,
    );
    append_section(
        state,
        &metis_i18n::tr("Firmware"),
        &snap.firmware,
        &mut ordinal,
        &mut zebra,
    );

    let section_count = usize::from(!snap.packages.is_empty())
        + usize::from(!snap.flatpaks.is_empty())
        + usize::from(!snap.firmware.is_empty());
    let min_h = list_min_height(count, section_count);
    state.scroller.set_min_content_height(min_h);
    state.scroller.set_max_content_height(LIST_MAX_HEIGHT);

    // Sync select-all without firing its toggled handler.
    state.syncing.set(true);
    let selected_n = state.selected.borrow().len();
    let item_total = state.item_total.get();
    state
        .select_all
        .set_active(item_total > 0 && selected_n == item_total);
    state.syncing.set(false);

    let all_selected = item_total > 0 && selected_n == item_total;
    if count == 0 {
        update_selection_chrome(
            &state.install_btn,
            &state.select_hint,
            &state.selected.borrow(),
            false,
        );
    } else if all_selected {
        state.install_btn.set_label(&metis_i18n::tr("Update all"));
        state.install_btn.set_sensitive(!state.applying.get());
        state
            .select_hint
            .set_text(&metis_i18n::tr("%1 selected").replace("%1", &selected_n.to_string()));
    } else {
        update_selection_chrome(
            &state.install_btn,
            &state.select_hint,
            &state.selected.borrow(),
            true,
        );
        if state.applying.get() {
            state.install_btn.set_sensitive(false);
        }
    }

    state.reboot_banner.set_visible(snap.reboot_required);
    if let Some(err) = &snap.error {
        if count == 0 {
            state.status.set_visible(true);
            state.status.set_text(err);
        }
    } else if !state.applying.get() {
        state.status.set_visible(false);
        state.progress.set_visible(false);
    }

    // Remeasure after rows are attached — ListBox natural height lags one frame.
    if state.window.is_visible() {
        schedule_fit_window(&state.window);
    }
}

fn append_section(
    state: &UpdaterState,
    title: &str,
    items: &[UpdateItem],
    ordinal: &mut usize,
    zebra: &mut usize,
) {
    if items.is_empty() {
        return;
    }
    let header = gtk::Label::new(Some(title));
    header.set_halign(gtk::Align::Start);
    header.add_css_class("metis-updater-section");
    let header_row = gtk::ListBoxRow::new();
    header_row.set_selectable(false);
    header_row.set_activatable(false);
    header_row.add_css_class("metis-updater-section-row");
    header_row.set_child(Some(&header));
    state.list.append(&header_row);

    for item in items {
        let key = item_key(item, *ordinal);
        *ordinal += 1;
        let checked = state.selected.borrow().contains(&key);

        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row_box.add_css_class("metis-updater-item");
        row_box.set_hexpand(true);

        let check = gtk::CheckButton::new();
        check.set_active(checked);
        check.set_valign(gtk::Align::Center);
        check.add_css_class("metis-updater-check");
        row_box.append(&check);

        let icon = gtk::Image::from_icon_name(source_icon(item.source));
        icon.set_pixel_size(20);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("metis-updater-item-icon");
        row_box.append(&icon);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let name = gtk::Label::new(Some(&item.name));
        name.set_halign(gtk::Align::Start);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("metis-updater-item-name");
        text.append(&name);

        let mut meta_parts = Vec::new();
        if let Some(ver) = &item.version {
            meta_parts.push(ver.clone());
        }
        if !meta_parts.is_empty() {
            let meta = gtk::Label::new(Some(&meta_parts.join(" · ")));
            meta.set_halign(gtk::Align::Start);
            meta.add_css_class("metis-updater-item-meta");
            meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&meta);
        } else if let Some(summary) = &item.summary {
            let meta = gtk::Label::new(Some(summary));
            meta.set_halign(gtk::Align::Start);
            meta.add_css_class("metis-updater-item-meta");
            meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&meta);
        }
        row_box.append(&text);

        let security = if item.security {
            let badge = gtk::Label::new(Some(&metis_i18n::tr("Security")));
            badge.add_css_class("metis-updater-badge");
            badge.set_valign(gtk::Align::Center);
            row_box.append(&badge);
            Some(badge)
        } else {
            None
        };

        let row_install = gtk::Button::with_label(&metis_i18n::tr("Install"));
        row_install.add_css_class("metis-updater-row-btn");
        row_install.set_valign(gtk::Align::Center);
        row_install.set_sensitive(!state.applying.get());
        row_box.append(&row_install);

        let mini = gtk::ProgressBar::new();
        mini.add_css_class("metis-updater-row-progress");
        mini.set_valign(gtk::Align::Center);
        mini.set_show_text(false);
        mini.set_fraction(0.0);
        mini.set_visible(false);
        row_box.append(&mini);

        let installed = gtk::Label::new(Some(&metis_i18n::tr("Installed")));
        installed.add_css_class("metis-updater-installed");
        installed.set_valign(gtk::Align::Center);
        installed.set_visible(false);
        row_box.append(&installed);

        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.add_css_class("metis-updater-item-row");
        if *zebra % 2 == 1 {
            row.add_css_class("metis-updater-zebra");
        }
        *zebra += 1;
        // Widget name carries the selection key for select-all walks.
        check.set_widget_name(&key.replace('\0', "::"));
        row.set_child(Some(&row_box));

        let widgets = RowStatusWidgets {
            row: row.clone(),
            install_btn: row_install.clone(),
            progress: mini,
            installed,
            security,
        };
        {
            let mut map = state.row_status.borrow_mut();
            for match_key in row_match_keys(item) {
                map.insert(match_key, widgets.clone());
            }
        }

        {
            let selected = state.selected.clone();
            let syncing = state.syncing.clone();
            let select_all = state.select_all.clone();
            let install_btn = state.install_btn.clone();
            let select_hint = state.select_hint.clone();
            let item_total = state.item_total.clone();
            let applying = state.applying.clone();
            let key_c = key.clone();
            check.connect_toggled(move |btn| {
                if syncing.get() {
                    return;
                }
                {
                    let mut sel = selected.borrow_mut();
                    if btn.is_active() {
                        sel.insert(key_c.clone());
                    } else {
                        sel.remove(&key_c);
                    }
                }
                let n = selected.borrow().len();
                let total = item_total.get();
                syncing.set(true);
                select_all.set_active(total > 0 && n == total);
                syncing.set(false);
                if total > 0 && n == total {
                    install_btn.set_label(&metis_i18n::tr("Update all"));
                    install_btn.set_sensitive(!applying.get());
                    select_hint
                        .set_text(&metis_i18n::tr("%1 selected").replace("%1", &n.to_string()));
                } else {
                    update_selection_chrome(&install_btn, &select_hint, &selected.borrow(), true);
                    if applying.get() {
                        install_btn.set_sensitive(false);
                    }
                }
            });
        }

        {
            let applying = state.applying.clone();
            let handles_slot = Rc::new(RefCell::new(Some(UpdaterHandles {
                progress: state.progress.clone(),
                progress_item: state.progress_item.clone(),
                status: state.status.clone(),
                reboot_banner: state.reboot_banner.clone(),
                row_status: state.row_status.clone(),
                list: state.list.clone(),
                scroller: state.scroller.clone(),
            })));
            let primary = state.install_btn.clone();
            let parent = state.window.clone();
            let item = item.clone();
            row_install.connect_clicked(move |_| {
                let scope = scope_for_item(&item);
                begin_apply(&applying, &primary, &handles_slot, &parent, scope);
            });
        }

        state.list.append(&row);
    }
}

fn row_check(row: &gtk::ListBoxRow) -> Option<gtk::CheckButton> {
    let child = row.child()?;
    let box_ = child.downcast::<gtk::Box>().ok()?;
    box_.first_child()?.downcast::<gtk::CheckButton>().ok()
}

fn row_key(row: &gtk::ListBoxRow) -> Option<String> {
    let check = row_check(row)?;
    let name = check.widget_name();
    if name.is_empty() {
        return None;
    }
    Some(name.replace("::", "\0"))
}
