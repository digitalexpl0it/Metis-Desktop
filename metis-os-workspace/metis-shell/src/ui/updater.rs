//! Software Updates window — Metis SSD xdg_toplevel with a forced-opaque surface.
//!
//! Must paint with fully-opaque `rgb()` (global shell CSS makes every `window`
//! transparent for the bar / OSD). Polkit auth is a layer-shell Overlay, so this
//! window can stay mapped while Authentication Required is shown on top.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
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
/// Log panel height while revealed.
const LOG_HEIGHT: i32 = 140;

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
    status: gtk::Label,
    log_view: gtk::TextView,
    log_revealer: gtk::Revealer,
    log_toggle: gtk::ToggleButton,
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
    // Defer the soft check so opening the window never races PackageKit I/O
    // against the first pointer frame.
    glib::timeout_add_local_once(std::time::Duration::from_secs(2), || {
        services::updates_request_check(false);
    });
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
/// window for new content, but it will not shrink it when a Revealer collapses —
/// without an explicit size, Hide log left a tall empty window.
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
    // After the current layout pass so ListBox / Revealer report real sizes.
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
        window.metis-updater label.metis-updater-section {{
            color: {muted};
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
        window.metis-updater button.metis-updater-log-toggle,
        window.metis-updater button.metis-updater-log-toggle:checked,
        window.metis-updater button.metis-updater-log-toggle:hover,
        window.metis-updater button.metis-updater-log-toggle:checked:hover {{
            background-color: rgba({text_rgb}, 0.06);
            background-image: none;
            color: {text};
            border: 1px solid rgba({text_rgb}, 0.16);
            box-shadow: none;
        }}
        window.metis-updater button.metis-updater-log-toggle label,
        window.metis-updater button.metis-updater-log-toggle:checked label,
        window.metis-updater button.metis-updater-log-toggle:hover label {{
            color: {text};
        }}
        window.metis-updater textview.metis-updater-log,
        window.metis-updater textview.metis-updater-log text,
        window.metis-updater textview.metis-updater-log > border,
        window.metis-updater .metis-updater-log {{
            background-color: rgb({surface_rgb});
            color: {text};
            caret-color: {text};
        }}
        window.metis-updater textview.metis-updater-log text {{
            background-color: rgb({surface_rgb});
            color: {text};
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

    let log_toggle = gtk::ToggleButton::with_label(&metis_i18n::tr("Show log"));
    log_toggle.set_halign(gtk::Align::Start);
    log_toggle.add_css_class("metis-updater-log-toggle");
    root.append(&log_toggle);

    let log_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(160)
        .reveal_child(false)
        .vexpand(false)
        .hexpand(true)
        .build();
    let log_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(LOG_HEIGHT)
        .max_content_height(LOG_HEIGHT)
        .propagate_natural_height(true)
        .vexpand(false)
        .hexpand(true)
        .build();
    log_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    let log_view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    log_view.add_css_class("metis-updater-log");
    log_scroll.set_child(Some(&log_view));
    log_revealer.set_child(Some(&log_scroll));
    root.append(&log_revealer);

    {
        let revealer = log_revealer.clone();
        let window = window.clone();
        log_toggle.connect_toggled(move |btn| {
            let open = btn.is_active();
            revealer.set_reveal_child(open);
            btn.set_label(&if open {
                metis_i18n::tr("Hide log")
            } else {
                metis_i18n::tr("Show log")
            });
            if open {
                // Grow with the reveal animation.
                schedule_fit_window_after(&window, std::time::Duration::from_millis(180));
            } else {
                // Shrink only after the slide-up finishes — resizing mid-transition
                // left the tall empty gap under Hide log.
                schedule_fit_window_after(&window, std::time::Duration::from_millis(200));
            }
        });
    }

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
        install_btn.connect_clicked(move |btn| {
            let scope = scope_from_selection(&selected.borrow());
            begin_apply(&applying, btn, &handles_slot, &parent, scope);
        });
    }

    // Hide instead of destroy: an in-flight install still holds the widgets,
    // and `show()` reuses the same window. No RefCell access on this path.
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });

    let state = UpdaterState {
        window,
        hero_title,
        hero_sub,
        select_all: select_all.clone(),
        select_hint,
        list,
        scroller,
        progress,
        status,
        log_view,
        log_revealer: log_revealer.clone(),
        log_toggle: log_toggle.clone(),
        reboot_banner,
        install_btn: install_btn.clone(),
        later_btn,
        applying,
        selected,
        item_total: Rc::new(Cell::new(0)),
        syncing: syncing.clone(),
    };

    *state_for_install.borrow_mut() = Some(UpdaterHandles {
        progress: state.progress.clone(),
        status: state.status.clone(),
        log_buffer: state.log_view.buffer(),
        log_revealer: state.log_revealer.clone(),
        log_toggle: state.log_toggle.clone(),
        reboot_banner: state.reboot_banner.clone(),
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
    handles.progress.set_visible(true);
    handles.progress.set_fraction(0.0);
    handles.progress.set_show_text(true);
    handles.progress.set_text(Some(&metis_i18n::tr("Waiting…")));
    handles.status.set_visible(true);
    handles
        .status
        .set_text(&metis_i18n::tr("Waiting for authentication…"));
    handles.log_buffer.set_text("");
    // Keep log collapsed — TextView inserts during install thrash the
    // main loop and make the pointer unusable.
    handles.log_revealer.set_reveal_child(false);
    handles.log_toggle.set_active(false);

    // Polkit auth is a layer-shell Overlay above this xdg window — leave the
    // updater visible so progress resumes in place after authentication.

    // Pulse while PackageKit is silent (auth + CR-only progress before first %).
    let pulse_stop = Rc::new(Cell::new(false));
    let saw_percent = Rc::new(Cell::new(false));
    let conffile_pending = Rc::new(Cell::new(false));
    {
        let progress = handles.progress.clone();
        let stop = pulse_stop.clone();
        let saw = saw_percent.clone();
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
    let pending_log: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let on_event: Rc<dyn Fn(UpdateProgressEvent)> = Rc::new(move |ev| {
        match ev {
            UpdateProgressEvent::Log { line } => {
                // Only buffer; flush when the user has the log open.
                if handles.log_revealer.reveals_child() {
                    pending_log.borrow_mut().push_str(&line);
                    pending_log.borrow_mut().push('\n');
                    if pending_log.borrow().len() >= 1024 {
                        flush_log(&handles.log_buffer, &pending_log);
                    }
                }
            }
            UpdateProgressEvent::Progress { percent, item } => {
                if handles.log_revealer.reveals_child() {
                    flush_log(&handles.log_buffer, &pending_log);
                }
                if percent > 0 {
                    saw_percent.set(true);
                    handles
                        .progress
                        .set_fraction(f64::from(percent.clamp(0, 100)) / 100.0);
                    handles
                        .progress
                        .set_text(Some(&format!("{}%", percent.clamp(0, 100))));
                }
                if let Some(name) = item {
                    handles.status.set_text(&name);
                }
            }
            UpdateProgressEvent::Phase { name } => {
                if handles.log_revealer.reveals_child() {
                    flush_log(&handles.log_buffer, &pending_log);
                }
                handles.status.set_text(&name);
                if !saw_percent.get() {
                    handles.progress.set_text(Some(&metis_i18n::tr("Working…")));
                }
            }
            UpdateProgressEvent::ConffileConflict {
                package,
                config_path,
            } => {
                if handles.log_revealer.reveals_child() {
                    flush_log(&handles.log_buffer, &pending_log);
                }
                conffile_pending.set(true);
                pulse_stop.set(true);
                handles
                    .status
                    .set_text(&metis_i18n::tr("Configuration file conflict — choose an option…"));
                handles.progress.set_text(Some("…"));
                let applying = applying_cb.clone();
                let install_btn = install_btn.clone();
                let handles = handles.clone();
                present_conffile_dialog(&parent_win, &package, config_path.as_deref(), move |choice| {
                    match choice {
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
                                        handles.progress.set_fraction(1.0);
                                        handles.progress.set_text(Some("100%"));
                                        handles.status.set_text(&metis_i18n::tr(
                                            "Updates installed",
                                        ));
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
                    }
                });
            }
            UpdateProgressEvent::Finished {
                ok,
                error,
                reboot_required,
            } => {
                if handles.log_revealer.reveals_child() {
                    flush_log(&handles.log_buffer, &pending_log);
                }
                // Conffile dialog owns the rest of the lifecycle.
                if conffile_pending.get() {
                    return;
                }
                pulse_stop.set(true);
                applying_cb.set(false);
                install_btn.set_sensitive(true);
                if ok {
                    handles.progress.set_fraction(1.0);
                    handles.progress.set_text(Some("100%"));
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
                handles.reboot_banner.set_visible(reboot_required);
                WINDOW.with(|cell| {
                    if let Some(state) = cell.borrow().as_ref() {
                        refresh_content(state);
                        state.status.set_visible(true);
                        state.status.set_text(&status_msg);
                        state.progress.set_visible(true);
                        state.progress.set_fraction(progress_frac);
                        state.progress.set_show_text(true);
                        if ok {
                            state.progress.set_text(Some("100%"));
                        }
                        state.reboot_banner.set_visible(
                            reboot_required || state.reboot_banner.is_visible(),
                        );
                    }
                });
            }
        }
    });
    services::updates_start_apply_scope(scope, on_event);
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
    dialog.choose(Some(parent), Option::<&gtk::gio::Cancellable>::None, move |result| {
        let choice = match result {
            Ok(0) => Some(metis_remote::ConfFileChoice::KeepLocal),
            Ok(1) => Some(metis_remote::ConfFileChoice::UsePackage),
            _ => None,
        };
        on_choice(choice);
    });
}

fn flush_log(buffer: &gtk::TextBuffer, pending: &Rc<RefCell<String>>) {
    let chunk = {
        let mut p = pending.borrow_mut();
        if p.is_empty() {
            return;
        }
        std::mem::take(&mut *p)
    };
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, &chunk);
}

#[derive(Clone)]
struct UpdaterHandles {
    progress: gtk::ProgressBar,
    status: gtk::Label,
    log_buffer: gtk::TextBuffer,
    log_revealer: gtk::Revealer,
    log_toggle: gtk::ToggleButton,
    reboot_banner: gtk::Box,
}

fn scope_from_selection(selected: &HashSet<String>) -> UpdateApplyScope {
    let mut packages = Vec::new();
    let mut flatpaks = Vec::new();
    let mut firmware = Vec::new();
    for key in selected {
        let Some((src, id)) = package_id_from_key(key) else {
            continue;
        };
        match src {
            "PackageKit" | "Distro" => packages.push(id.to_string()),
            "Flatpak" => flatpaks.push(id.to_string()),
            "Firmware" => firmware.push(id.to_string()),
            _ => {}
        }
    }
    // Duplicate PackageKit IDs can appear as separate rows; apply once each.
    packages.sort();
    packages.dedup();
    flatpaks.sort();
    flatpaks.dedup();
    firmware.sort();
    firmware.dedup();
    UpdateApplyScope::Selected {
        packages,
        flatpaks,
        firmware,
    }
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

        if item.security {
            let badge = gtk::Label::new(Some(&metis_i18n::tr("Security")));
            badge.add_css_class("metis-updater-badge");
            badge.set_valign(gtk::Align::Center);
            row_box.append(&badge);
        }

        let row_install = gtk::Button::with_label(&metis_i18n::tr("Install"));
        row_install.add_css_class("metis-updater-row-btn");
        row_install.set_valign(gtk::Align::Center);
        row_install.set_sensitive(!state.applying.get());
        row_box.append(&row_install);

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

        {
            let selected = state.selected.clone();
            let syncing = state.syncing.clone();
            let select_all = state.select_all.clone();
            let install_btn = state.install_btn.clone();
            let select_hint = state.select_hint.clone();
            let item_total = state.item_total.clone();
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
                    install_btn.set_sensitive(true);
                    select_hint
                        .set_text(&metis_i18n::tr("%1 selected").replace("%1", &n.to_string()));
                } else {
                    update_selection_chrome(&install_btn, &select_hint, &selected.borrow(), true);
                }
            });
        }

        {
            let applying = state.applying.clone();
            let handles_slot = Rc::new(RefCell::new(Some(UpdaterHandles {
                progress: state.progress.clone(),
                status: state.status.clone(),
                log_buffer: state.log_view.buffer(),
                log_revealer: state.log_revealer.clone(),
                log_toggle: state.log_toggle.clone(),
                reboot_banner: state.reboot_banner.clone(),
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
