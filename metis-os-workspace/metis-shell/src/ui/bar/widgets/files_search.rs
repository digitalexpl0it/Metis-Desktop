//! Edge-bar file search — layer-shell overlay backed by `metis-indexd`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use gio::prelude::*;
use gtk::gdk;
use gtk::gdk_pixbuf;
use gtk::glib;
use gtk::glib::object::IsA;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::{BarPosition, load_bar_config};
use metis_protocol::search::{
    SearchCategory, SearchHit, SearchRequest, SearchResponse, search_request,
};

use crate::ui::icons;

const SLIDE_MS: u32 = 280;
const PANEL_GAP: i32 = 6;
const HIDDEN_EXTRA: i32 = 420;
const MAIN_WIDTH: i32 = 620;
const MAIN_HEIGHT: i32 = 520;
const PREVIEW_WIDTH: i32 = 340;
const THUMB_LIST: i32 = 48;
const THUMB_PREVIEW: i32 = 160;
/// Hard cap for preview media so tall images cannot grow the overlay.
const PREVIEW_MEDIA_MAX: i32 = 160;
const QUERY_DEBOUNCE_MS: u64 = 180;
const MAX_THUMB_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PDF_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OFFICE_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INFLIGHT_THUMBS: usize = 2;
const PDF_PREVIEW_TIMEOUT: Duration = Duration::from_secs(8);
const OFFICE_PREVIEW_TIMEOUT: Duration = Duration::from_secs(15);
const OFFICE_PREVIEW_CHARS: usize = 12_000;

thread_local! {
    static OVERLAY: RefCell<Option<Rc<SearchOverlay>>> = const { RefCell::new(None) };
}

static THUMB_INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static PREVIEW_TMP_SEQ: AtomicU64 = AtomicU64::new(0);
static LIBREOFFICE_BIN: OnceLock<Option<&'static str>> = OnceLock::new();

struct SearchOverlay {
    window: gtk::Window,
    /// Outer shell holding main card + optional preview.
    shell: gtk::Box,
    preview: gtk::Box,
    preview_body: gtk::Box,
    entry: gtk::Entry,
    chips: gtk::Box,
    results: gtk::Box,
    status: gtk::Label,
    category: Cell<Option<&'static str>>,
    tag_filter: RefCell<Vec<String>>,
    selected_path: RefCell<Option<String>>,
    preview_open: Cell<bool>,
    last_hits: RefCell<Vec<SearchHit>>,
    /// Last hits from an unscoped (All) query — used for instant chip filtering.
    broad_hits: RefCell<Vec<SearchHit>>,
    slide_side: Cell<BarPosition>,
    slide_margin: Cell<i32>,
    open_margin: Cell<i32>,
    anim_gen: Cell<u32>,
    query_gen: Cell<u32>,
    debounce: RefCell<Option<glib::SourceId>>,
    /// Skip chip toggled → schedule while programmatically resetting radios.
    suppress_chip: Cell<bool>,
    /// Path → list/preview thumb hosts (main-thread only; workers send pixels by path).
    thumb_hosts: RefCell<HashMap<String, glib::WeakRef<gtk::Box>>>,
    open: Cell<bool>,
}

pub struct FilesSearchWidget {
    root: gtk::Button,
}

impl FilesSearchWidget {
    pub fn new() -> Self {
        let root = gtk::Button::builder().has_frame(false).build();
        root.add_css_class("metis-bar-widget");
        root.add_css_class("metis-bar-sys-icon");
        root.add_css_class("metis-bar-files-search");
        root.set_tooltip_text(Some(&metis_i18n::tr("Search files")));
        root.set_child(Some(&icons::image("edit-find-symbolic")));
        root.connect_clicked(|_| toggle());
        Self { root }
    }

    pub fn root(&self) -> &gtk::Button {
        &self.root
    }
}

/// Toggle File search (bar button and Super+S / `toggle-files-search`).
pub fn toggle() {
    OVERLAY.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref()
            && existing.open.get()
        {
            dismiss();
            return;
        }
        show();
    });
}

fn dismiss() {
    OVERLAY.with(|cell| {
        let Some(overlay) = cell.borrow().as_ref().cloned() else {
            return;
        };
        if !overlay.open.get() {
            return;
        }
        if let Some(id) = overlay.debounce.borrow_mut().take() {
            id.remove();
        }
        // Invalidate in-flight query / thumb applies.
        overlay
            .query_gen
            .set(overlay.query_gen.get().wrapping_add(1));
        close_preview(&overlay);
        overlay.open.set(false);
        let hidden = hidden_margin_for(&overlay);
        animate_slide(overlay.clone(), hidden, move |o| {
            o.window.set_visible(false);
            o.window.set_keyboard_mode(KeyboardMode::None);
        });
    });
}

fn show() {
    crate::ui::bar::dropdown::close_all();
    crate::ui::notification_center::dismiss();

    let overlay = OVERLAY.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }
        let built = build_overlay();
        *cell.borrow_mut() = Some(built.clone());
        built
    });

    apply_edge_layout(&overlay);
    overlay.category.set(None);
    overlay.tag_filter.borrow_mut().clear();
    *overlay.selected_path.borrow_mut() = None;
    overlay.last_hits.borrow_mut().clear();
    overlay.broad_hits.borrow_mut().clear();
    close_preview(&overlay);
    overlay.suppress_chip.set(true);
    overlay.entry.set_text("");
    overlay.suppress_chip.set(false);
    overlay
        .status
        .set_text(&metis_i18n::tr("Type to search indexed files"));
    clear_box(&overlay.results);
    overlay.thumb_hosts.borrow_mut().clear();
    overlay.window.set_cursor_from_name(Some("default"));
    rebuild_chips(&overlay);

    let open_at = overlay.open_margin.get();
    let hidden = hidden_margin_for(&overlay);
    set_slide_margin(&overlay, hidden);
    overlay.window.set_keyboard_mode(KeyboardMode::Exclusive);
    overlay.window.set_visible(true);
    overlay.open.set(true);
    overlay.entry.grab_focus();

    animate_slide(overlay.clone(), open_at, |_| {});
    schedule_query(&overlay, false);
}

fn bar_layout() -> (BarPosition, i32) {
    let cfg = load_bar_config();
    let open = metis_config::bar::bar_pill_inset(&cfg) + PANEL_GAP;
    (cfg.position, open)
}

fn apply_edge_layout(overlay: &SearchOverlay) {
    let (position, open_margin) = bar_layout();
    overlay.open_margin.set(open_margin);
    overlay.slide_side.set(position);

    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        overlay.window.set_anchor(edge, true);
        overlay.window.set_margin(edge, 0);
    }

    overlay.shell.set_margin_top(0);
    overlay.shell.set_margin_bottom(0);
    overlay.shell.set_margin_start(0);
    overlay.shell.set_margin_end(0);

    match position {
        BarPosition::Top => {
            overlay.shell.set_halign(gtk::Align::Center);
            overlay.shell.set_valign(gtk::Align::Start);
        }
        BarPosition::Bottom => {
            overlay.shell.set_halign(gtk::Align::Center);
            overlay.shell.set_valign(gtk::Align::End);
        }
        BarPosition::Left => {
            overlay.shell.set_halign(gtk::Align::Start);
            overlay.shell.set_valign(gtk::Align::Center);
        }
        BarPosition::Right => {
            overlay.shell.set_halign(gtk::Align::End);
            overlay.shell.set_valign(gtk::Align::Center);
        }
    }
}

fn hidden_margin_for(overlay: &SearchOverlay) -> i32 {
    -(HIDDEN_EXTRA).saturating_add(overlay.open_margin.get().max(0))
}

fn set_slide_margin(overlay: &SearchOverlay, margin: i32) {
    overlay.slide_margin.set(margin);
    overlay.shell.set_margin_top(0);
    overlay.shell.set_margin_bottom(0);
    overlay.shell.set_margin_start(0);
    overlay.shell.set_margin_end(0);
    match overlay.slide_side.get() {
        BarPosition::Top => overlay.shell.set_margin_top(margin),
        BarPosition::Bottom => overlay.shell.set_margin_bottom(margin),
        BarPosition::Left => overlay.shell.set_margin_start(margin),
        BarPosition::Right => overlay.shell.set_margin_end(margin),
    }
}

fn ease_out_back(t: f64) -> f64 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn animate_slide(
    overlay: Rc<SearchOverlay>,
    target: i32,
    on_done: impl FnOnce(&SearchOverlay) + 'static,
) {
    let r#gen = overlay.anim_gen.get().wrapping_add(1);
    overlay.anim_gen.set(r#gen);
    let start = overlay.slide_margin.get();
    let delta = target - start;
    if delta == 0 {
        on_done(&overlay);
        return;
    }
    let start_at = glib::monotonic_time();
    let duration_us = (SLIDE_MS as i64) * 1000;
    let mut on_done = Some(on_done);
    let bounce = target >= overlay.open_margin.get().saturating_sub(2);

    glib::timeout_add_local(Duration::from_millis(16), move || {
        if overlay.anim_gen.get() != r#gen {
            return glib::ControlFlow::Break;
        }
        let elapsed = glib::monotonic_time() - start_at;
        let t = (elapsed as f64 / duration_us as f64).clamp(0.0, 1.0);
        let eased = if bounce {
            ease_out_back(t)
        } else {
            1.0 - (1.0 - t).powi(3)
        };
        let margin = start + (delta as f64 * eased).round() as i32;
        set_slide_margin(&overlay, margin);
        if t >= 1.0 {
            set_slide_margin(&overlay, target);
            if let Some(cb) = on_done.take() {
                cb(&overlay);
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn build_overlay() -> Rc<SearchOverlay> {
    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Metis File Search"))
        .decorated(false)
        .build();
    window.add_css_class("metis-files-search-window");
    window.set_can_focus(true);
    window.init_layer_shell();
    window.set_namespace(Some("metis-files-search"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_exclusive_zone(-1);

    let root = gtk::Overlay::new();
    root.set_hexpand(true);
    root.set_vexpand(true);

    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.add_css_class("metis-files-search-backdrop");
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    let backdrop_click = gtk::GestureClick::new();
    backdrop_click.set_button(gdk::BUTTON_PRIMARY);
    backdrop_click.connect_released(|_, _, _, _| dismiss());
    backdrop.add_controller(backdrop_click);
    root.set_child(Some(&backdrop));

    let shell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    shell.add_css_class("metis-files-search-shell");
    let shell_click = gtk::GestureClick::new();
    shell_click.set_button(gdk::BUTTON_PRIMARY);
    shell_click.connect_released(|g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
    });
    shell.add_controller(shell_click);

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    card.add_css_class("metis-files-search-card");
    // Fixed size so empty category filters don't shrink the layer surface
    // (that thrash shows a resize cursor and can lock the UI).
    card.set_size_request(MAIN_WIDTH, MAIN_HEIGHT);
    card.set_hexpand(false);
    card.set_vexpand(false);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&metis_i18n::tr("Search files")));
    title.add_css_class("metis-bar-section-title");
    title.set_halign(gtk::Align::Start);
    title.set_hexpand(true);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat");
    close.connect_clicked(|_| dismiss());
    header.append(&title);
    header.append(&close);
    card.append(&header);

    let entry = gtk::Entry::builder()
        .placeholder_text(metis_i18n::tr("Search by name, content, or tag:…"))
        .hexpand(true)
        .build();
    entry.add_css_class("metis-files-search-entry");
    card.append(&entry);

    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chips.add_css_class("metis-files-search-chips");
    chips.set_halign(gtk::Align::Start);
    let chips_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&chips)
        .build();
    chips_scroll.set_propagate_natural_height(true);
    card.append(&chips_scroll);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .height_request(360)
        .min_content_height(360)
        .vexpand(true)
        .hexpand(true)
        .build();
    scrolled.set_size_request(-1, 360);
    let results = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    results.add_css_class("metis-files-search-results");
    scrolled.set_child(Some(&results));
    card.append(&scrolled);

    let status = gtk::Label::new(Some(&metis_i18n::tr("Type to search indexed files")));
    status.set_xalign(0.0);
    status.add_css_class("metis-muted-label");
    card.append(&status);

    let preview = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    preview.add_css_class("metis-files-search-preview");
    preview.set_hexpand(true);
    preview.set_vexpand(false);
    preview.set_valign(gtk::Align::Fill);
    preview.set_size_request(PREVIEW_WIDTH, MAIN_HEIGHT);
    preview.set_overflow(gtk::Overflow::Hidden);
    preview.set_visible(false);

    let preview_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let preview_title = gtk::Label::new(Some(&metis_i18n::tr("Preview")));
    preview_title.add_css_class("metis-bar-section-title");
    preview_title.set_halign(gtk::Align::Start);
    preview_title.set_hexpand(true);
    let preview_close = gtk::Button::from_icon_name("window-close-symbolic");
    preview_close.add_css_class("flat");
    preview_header.append(&preview_title);
    preview_header.append(&preview_close);
    preview.append(&preview_header);

    let preview_body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    preview_body.set_hexpand(true);
    // Cap natural width so long path/content children cannot grow the pane.
    preview_body.set_size_request(PREVIEW_WIDTH - 32, -1);

    // Scroll the body inside a fixed-height pane so media/tags cannot stretch
    // the search+preview shell off-screen.
    let body_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(MAIN_HEIGHT - 56)
        .child(&preview_body)
        .build();
    body_scroll.set_propagate_natural_height(false);
    body_scroll.set_propagate_natural_width(false);
    body_scroll.add_css_class("metis-files-search-preview-body-scroll");
    preview.append(&body_scroll);

    // Always reserve the preview column so open/close (and empty chips) never
    // resize the layer surface — that thrash shows a resize cursor and can lock UI.
    let preview_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_slot.set_size_request(PREVIEW_WIDTH, MAIN_HEIGHT);
    preview_slot.set_hexpand(false);
    preview_slot.set_vexpand(false);
    preview_slot.set_overflow(gtk::Overflow::Hidden);
    preview_slot.append(&preview);

    shell.set_size_request(MAIN_WIDTH + 10 + PREVIEW_WIDTH, MAIN_HEIGHT);
    shell.set_vexpand(false);
    shell.set_overflow(gtk::Overflow::Hidden);
    shell.append(&card);
    shell.append(&preview_slot);
    root.add_overlay(&shell);
    window.set_child(Some(&root));

    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(|_, key, _, _| {
        if key == gdk::Key::Escape {
            let closed_preview = OVERLAY.with(|cell| {
                if let Some(o) = cell.borrow().as_ref()
                    && o.preview_open.get()
                {
                    close_preview(o);
                    return true;
                }
                false
            });
            if !closed_preview {
                dismiss();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    let overlay = Rc::new(SearchOverlay {
        window,
        shell,
        preview: preview.clone(),
        preview_body,
        entry: entry.clone(),
        chips,
        results,
        status,
        category: Cell::new(None),
        tag_filter: RefCell::new(Vec::new()),
        selected_path: RefCell::new(None),
        preview_open: Cell::new(false),
        last_hits: RefCell::new(Vec::new()),
        broad_hits: RefCell::new(Vec::new()),
        slide_side: Cell::new(BarPosition::Top),
        slide_margin: Cell::new(-HIDDEN_EXTRA),
        open_margin: Cell::new(48),
        anim_gen: Cell::new(0),
        query_gen: Cell::new(0),
        debounce: RefCell::new(None),
        suppress_chip: Cell::new(false),
        thumb_hosts: RefCell::new(HashMap::new()),
        open: Cell::new(false),
    });

    {
        let overlay = overlay.clone();
        preview_close.connect_clicked(move |_| close_preview(&overlay));
    }
    {
        let overlay = overlay.clone();
        entry.connect_changed(move |_| {
            if overlay.suppress_chip.get() {
                return;
            }
            schedule_query(&overlay, true);
        });
    }

    rebuild_chips(&overlay);
    overlay
}

fn rebuild_chips(overlay: &Rc<SearchOverlay>) {
    clear_box(&overlay.chips);
    overlay.suppress_chip.set(true);

    let all = gtk::ToggleButton::with_label(&metis_i18n::tr("All"));
    all.add_css_class("metis-files-search-chip");
    all.set_cursor_from_name(Some("default"));
    all.set_active(overlay.category.get().is_none());
    {
        let overlay = overlay.clone();
        all.connect_toggled(move |btn| {
            if overlay.suppress_chip.get() || !btn.is_active() {
                return;
            }
            on_chip_selected(&overlay, None);
        });
    }
    overlay.chips.append(&all);

    for cat in SearchCategory::filter_chips() {
        let btn = gtk::ToggleButton::with_label(cat.label());
        btn.add_css_class("metis-files-search-chip");
        btn.set_cursor_from_name(Some("default"));
        btn.set_group(Some(&all));
        btn.set_active(overlay.category.get() == Some(cat.as_str()));
        {
            let overlay = overlay.clone();
            let id = cat.as_str();
            btn.connect_toggled(move |b| {
                if overlay.suppress_chip.get() || !b.is_active() {
                    return;
                }
                on_chip_selected(&overlay, Some(id));
            });
        }
        overlay.chips.append(&btn);
    }

    overlay.suppress_chip.set(false);
}

/// Defer all chip work off the ToggleButton handler — never clear/rebuild
/// widgets or touch layout while GTK is still delivering the toggle event.
/// Filtering is client-side against the last unscoped result page (no IPC).
fn on_chip_selected(overlay: &Rc<SearchOverlay>, category: Option<&'static str>) {
    overlay.category.set(category);
    overlay.window.set_cursor_from_name(Some("default"));
    let overlay = overlay.clone();
    glib::idle_add_local_once(move || {
        if !overlay.open.get() {
            return;
        }
        overlay.window.set_cursor_from_name(Some("default"));
        if let Err(err) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Chip path: never reopen preview (sync IO + layout thrash).
            apply_local_category_view(&overlay, false);
        })) {
            tracing::error!(
                ?err,
                "files search chip filter panicked — edge bar kept alive"
            );
        }
    });
}

/// Paint the result list from cached broad hits filtered by the active chip.
/// When `restore_preview` is false (chip clicks), preview is closed and not
/// rebuilt — avoids UI-thread file reads / pdftoppm / LibreOffice on toggle.
fn apply_local_category_view(overlay: &Rc<SearchOverlay>, restore_preview: bool) {
    let cat = overlay.category.get();
    let source = if cat.is_none() {
        overlay.broad_hits.borrow().clone()
    } else {
        let broad = overlay.broad_hits.borrow();
        if !broad.is_empty() {
            broad.clone()
        } else {
            overlay.last_hits.borrow().clone()
        }
    };
    let filtered: Vec<SearchHit> = match cat {
        None => source,
        Some(want) => source
            .into_iter()
            .filter(|h| hit_matches_category(h, want))
            .collect(),
    };
    let qid = overlay.query_gen.get();
    let selected = overlay.selected_path.borrow().clone();
    if !restore_preview {
        close_preview(overlay);
    }
    overlay.thumb_hosts.borrow_mut().clear();
    clear_box(&overlay.results);
    *overlay.last_hits.borrow_mut() = filtered.clone();
    if filtered.is_empty() {
        overlay.status.set_text(&metis_i18n::tr("No matches"));
        if restore_preview {
            close_preview(overlay);
        }
    } else {
        overlay
            .status
            .set_text(&metis_i18n::tr("%1 result(s)").replace("%1", &filtered.len().to_string()));
        render_grouped_results(overlay, &filtered, qid);
        if restore_preview && let Some(path) = selected {
            if let Some(hit) = filtered.iter().find(|h| h.path == path) {
                show_preview(overlay, hit, qid);
            } else {
                close_preview(overlay);
            }
        }
    }
}

fn hit_matches_category(hit: &SearchHit, want: &str) -> bool {
    let got = SearchCategory::from_str_lossy(&hit.category).as_str();
    if got == want {
        return true;
    }
    // Legacy "code" docs count as Documents.
    want == "documents" && (hit.category == "code" || got == "documents")
}

fn schedule_query(overlay: &Rc<SearchOverlay>, debounce: bool) {
    if debounce {
        schedule_query_ms(overlay, QUERY_DEBOUNCE_MS);
    } else {
        if let Some(id) = overlay.debounce.borrow_mut().take() {
            id.remove();
        }
        dispatch_query(overlay.clone());
    }
}

fn schedule_query_ms(overlay: &Rc<SearchOverlay>, ms: u64) {
    if let Some(id) = overlay.debounce.borrow_mut().take() {
        id.remove();
    }
    let overlay_weak = Rc::downgrade(overlay);
    let id = glib::timeout_add_local(Duration::from_millis(ms), move || {
        if let Some(overlay) = overlay_weak.upgrade() {
            *overlay.debounce.borrow_mut() = None;
            dispatch_query(overlay);
        }
        glib::ControlFlow::Break
    });
    *overlay.debounce.borrow_mut() = Some(id);
}

fn dispatch_query(overlay: Rc<SearchOverlay>) {
    if !overlay.open.get() {
        return;
    }
    let qid = overlay.query_gen.get().wrapping_add(1);
    overlay.query_gen.set(qid);

    let mut q = overlay.entry.text().to_string();
    let mut tags = overlay.tag_filter.borrow().clone();
    let mut rest = String::new();
    for tok in q.split_whitespace() {
        if let Some(t) = tok.strip_prefix("tag:") {
            let t = t.trim().to_ascii_lowercase();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        } else {
            if !rest.is_empty() {
                rest.push(' ');
            }
            rest.push_str(tok);
        }
    }
    q = rest;
    // Always fetch unscoped hits so chip switches filter locally (no empty
    // category round-trip that used to shrink the layer surface and lock UI).
    let req = SearchRequest::Query {
        q,
        category: None,
        tags,
        limit: 60,
    };

    // Don't stomp a just-painted chip filter with "Searching…" — only when empty.
    if overlay.last_hits.borrow().is_empty() && overlay.broad_hits.borrow().is_empty() {
        overlay.status.set_text(&metis_i18n::tr("Searching…"));
    }

    let _ = std::thread::Builder::new()
        .name("metis-files-search".into())
        .spawn(move || {
            let resp = search_request(&req);
            // Rc is !Send — re-enter via the thread-local overlay on the UI thread.
            let _ = glib::idle_add_once(move || {
                // Clone Rc out of OVERLAY before applying — avoid nested borrows.
                let Some(overlay) = overlay_rc_clone() else {
                    return;
                };
                if overlay.query_gen.get() != qid || !overlay.open.get() {
                    return;
                }
                if let Err(err) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    apply_query_response(&overlay, resp, qid);
                })) {
                    tracing::error!(?err, "files search apply panicked — edge bar kept alive");
                }
            });
        });
}

/// Take an `Rc` clone of the overlay without holding the thread-local borrow.
fn overlay_rc_clone() -> Option<Rc<SearchOverlay>> {
    OVERLAY.with(|cell| match cell.try_borrow() {
        Ok(b) => b.as_ref().cloned(),
        Err(err) => {
            tracing::error!(%err, "files search overlay already borrowed");
            None
        }
    })
}

fn apply_query_response(overlay: &Rc<SearchOverlay>, resp: SearchResponse, _qid: u32) {
    overlay.window.set_cursor_from_name(Some("default"));
    match resp {
        SearchResponse::Hits { hits } => {
            *overlay.broad_hits.borrow_mut() = hits;
            // Paint through the active chip filter (All = full page).
            apply_local_category_view(overlay, true);
        }
        SearchResponse::Error { message } => {
            overlay.thumb_hosts.borrow_mut().clear();
            clear_box(&overlay.results);
            overlay.last_hits.borrow_mut().clear();
            overlay.status.set_text(&format!(
                "{} — {}",
                metis_i18n::tr("Search unavailable"),
                message
            ));
        }
        _ => {
            overlay
                .status
                .set_text(&metis_i18n::tr("Unexpected search response"));
        }
    }
}

fn render_grouped_results(overlay: &Rc<SearchOverlay>, hits: &[SearchHit], qid: u32) {
    let mut groups: BTreeMap<&'static str, Vec<&SearchHit>> = BTreeMap::new();
    for hit in hits {
        let key = SearchCategory::from_str_lossy(&hit.category).as_str();
        groups.entry(key).or_default().push(hit);
    }

    let order = ["images", "video", "audio", "documents", "archives", "other"];
    let mut keys: Vec<&'static str> = groups.keys().copied().collect();
    keys.sort_by_key(|k| order.iter().position(|o| *o == *k).unwrap_or(order.len()));

    let selected = overlay.selected_path.borrow().clone();
    let query = overlay.entry.text().to_string();
    let mut zebra_i = 0usize;
    for key in keys {
        let Some(items) = groups.get(key) else {
            continue;
        };
        let title = SearchCategory::from_str_lossy(key).label();
        let header = gtk::Label::new(Some(&format!("{title} · {}", items.len())));
        header.set_xalign(0.0);
        header.add_css_class("metis-files-search-group-title");
        overlay.results.append(&header);

        for hit in items {
            let selected = selected.as_deref() == Some(hit.path.as_str());
            let zebra = zebra_i % 2 == 1;
            zebra_i += 1;
            overlay
                .results
                .append(&hit_row(overlay, hit, selected, zebra, &query, qid));
        }
    }
}

fn hit_row(
    overlay: &Rc<SearchOverlay>,
    hit: &SearchHit,
    selected: bool,
    zebra: bool,
    query: &str,
    qid: u32,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-files-search-row");
    if zebra {
        row.add_css_class("zebra");
    }
    if selected {
        row.add_css_class("selected");
    }

    let thumb_host = icon_placeholder(hit, THUMB_LIST);
    maybe_queue_list_thumb(overlay, &thumb_host, hit, qid);
    row.append(&thumb_host);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&hit.basename));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("metis-files-search-name");
    text.append(&name);

    let folder = Path::new(&hit.path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| hit.path.clone());
    let mut meta = format!(
        "{}  ·  {}  ·  {}",
        shorten_home(&folder),
        format_size(hit.size_bytes),
        format_mode(hit.mode.or_else(|| live_mode(&hit.path)))
    );
    if match_looks_like_content(hit, query) {
        meta.push_str("  ·  ");
        meta.push_str(&metis_i18n::tr("In content"));
    }
    let meta_lab = gtk::Label::new(Some(&meta));
    meta_lab.set_xalign(0.0);
    meta_lab.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    meta_lab.add_css_class("metis-files-search-meta");
    text.append(&meta_lab);
    row.append(&text);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    actions.add_css_class("metis-files-search-actions");
    actions.set_valign(gtk::Align::Center);

    let open = action_btn(&metis_i18n::tr("Open"));
    let path_open = hit.path.clone();
    open.connect_clicked(move |_| {
        open_path(&path_open);
        dismiss();
    });
    actions.append(&open);

    let copy = action_btn(&metis_i18n::tr("Copy"));
    let path_copy = hit.path.clone();
    copy.connect_clicked(move |_| {
        copy_path(&path_copy);
    });
    actions.append(&copy);

    let folder_btn = action_btn(&metis_i18n::tr("Open folder"));
    let path_folder = hit.path.clone();
    folder_btn.connect_clicked(move |_| {
        reveal_path(&path_folder);
        dismiss();
    });
    actions.append(&folder_btn);
    row.append(&actions);

    // Preview on thumb + text only so action buttons keep working.
    attach_preview_click(&thumb_host, overlay, hit, &row);
    attach_preview_click(&text, overlay, hit, &row);

    row
}

fn attach_preview_click(
    target: &impl IsA<gtk::Widget>,
    overlay: &Rc<SearchOverlay>,
    hit: &SearchHit,
    row: &gtk::Box,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    let overlay = overlay.clone();
    let hit = hit.clone();
    let row_weak = row.downgrade();
    gesture.connect_released(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        *overlay.selected_path.borrow_mut() = Some(hit.path.clone());
        mark_selected_row(&overlay.results, row_weak.upgrade().as_ref());
        show_preview(&overlay, &hit, overlay.query_gen.get());
    });
    target.as_ref().add_controller(gesture);
}

fn mark_selected_row(results: &gtk::Box, selected: Option<&gtk::Box>) {
    let mut child = results.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.has_css_class("metis-files-search-row") {
            widget.remove_css_class("selected");
        }
    }
    if let Some(row) = selected {
        row.add_css_class("selected");
    }
}

fn action_btn(label: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(label);
    btn.add_css_class("flat");
    btn
}

fn icon_placeholder(hit: &SearchHit, size: i32) -> gtk::Box {
    let cat = SearchCategory::from_str_lossy(&hit.category);
    let icon_name = category_icon(&cat, hit.mime.as_deref());
    let img = gtk::Image::from_icon_name(icon_name);
    img.set_pixel_size(size.saturating_sub(8).max(24));
    img.add_css_class("metis-files-search-icon");
    img.set_halign(gtk::Align::Center);
    img.set_valign(gtk::Align::Center);
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrap.add_css_class("metis-files-search-thumb");
    wrap.set_size_request(size, size);
    wrap.set_halign(gtk::Align::Center);
    wrap.set_valign(gtk::Align::Center);
    wrap.append(&img);
    wrap
}

fn maybe_queue_list_thumb(overlay: &Rc<SearchOverlay>, host: &gtk::Box, hit: &SearchHit, qid: u32) {
    let cat = SearchCategory::from_str_lossy(&hit.category);
    let wants_image = matches!(cat, SearchCategory::Images) || mime_is_image(hit.mime.as_deref());
    let wants_video = matches!(cat, SearchCategory::Video) || mime_is_video(hit.mime.as_deref());
    if !wants_image && !wants_video {
        return;
    }
    let path = hit.path.clone();
    let host = host.clone();
    let overlay = overlay.clone();
    queue_thumb_job(overlay, host, path, THUMB_LIST, qid, wants_image);
}

fn queue_thumb_job(
    overlay: Rc<SearchOverlay>,
    host: gtk::Box,
    path: String,
    size: i32,
    qid: u32,
    prefer_file: bool,
) {
    overlay
        .thumb_hosts
        .borrow_mut()
        .insert(path.clone(), host.downgrade());

    if THUMB_INFLIGHT.load(Ordering::Relaxed) >= MAX_INFLIGHT_THUMBS {
        let overlay_weak = Rc::downgrade(&overlay);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(overlay) = overlay_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if overlay.query_gen.get() != qid {
                return glib::ControlFlow::Break;
            }
            let Some(host) = overlay
                .thumb_hosts
                .borrow()
                .get(&path)
                .and_then(|w| w.upgrade())
            else {
                return glib::ControlFlow::Break;
            };
            queue_thumb_job(overlay, host, path.clone(), size, qid, prefer_file);
            glib::ControlFlow::Break
        });
        return;
    }

    THUMB_INFLIGHT.fetch_add(1, Ordering::Relaxed);
    let path_key = path.clone();
    let _ = std::thread::Builder::new()
        .name("metis-search-thumb".into())
        .spawn(move || {
            let pixels = decode_thumb_pixels(&path, size, prefer_file);
            THUMB_INFLIGHT.fetch_sub(1, Ordering::Relaxed);
            let Some(pixels) = pixels else {
                return;
            };
            let _ = glib::idle_add_once(move || {
                let Some(overlay) = overlay_rc_clone() else {
                    return;
                };
                if overlay.query_gen.get() != qid || !overlay.open.get() {
                    return;
                }
                let Some(host) = overlay
                    .thumb_hosts
                    .borrow()
                    .get(&path_key)
                    .and_then(|w| w.upgrade())
                else {
                    return;
                };
                if let Some(pic) = picture_from_pixels(pixels, size) {
                    while let Some(child) = host.first_child() {
                        host.remove(&child);
                    }
                    host.append(&pic);
                }
            });
        });
}

struct ThumbPixels {
    width: i32,
    height: i32,
    has_alpha: bool,
    rowstride: usize,
    bytes: glib::Bytes,
}

fn append_contents_pane(parent: &gtk::Box, text: &str) {
    let sn_title = gtk::Label::new(Some(&metis_i18n::tr("Contents")));
    sn_title.set_xalign(0.0);
    sn_title.add_css_class("metis-files-search-group-title");
    parent.append(&sn_title);

    let buf = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    buf.set_text(text);
    let view = gtk::TextView::builder()
        .buffer(&buf)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .monospace(true)
        .build();
    view.add_css_class("metis-files-search-preview-body");
    view.set_left_margin(4);
    view.set_right_margin(4);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    let sn_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(160)
        .vexpand(true)
        .hexpand(true)
        .child(&view)
        .build();
    sn_scroll.add_css_class("metis-files-search-preview-scroll");
    parent.append(&sn_scroll);
}

fn fill_contents_host(host: &gtk::Box, text: Option<&str>) {
    while let Some(child) = host.first_child() {
        host.remove(&child);
    }
    match text {
        Some(t) if !t.trim().is_empty() => append_contents_pane(host, t),
        _ => {
            let empty = gtk::Label::new(Some(&metis_i18n::tr("No text preview")));
            empty.set_xalign(0.0);
            empty.add_css_class("metis-muted-label");
            host.append(&empty);
        }
    }
}

fn queue_pdf_preview_job(overlay: Rc<SearchOverlay>, host: gtk::Box, path: String, qid: u32) {
    overlay
        .thumb_hosts
        .borrow_mut()
        .insert(path.clone(), host.downgrade());

    if THUMB_INFLIGHT.load(Ordering::Relaxed) >= MAX_INFLIGHT_THUMBS {
        let overlay_weak = Rc::downgrade(&overlay);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(overlay) = overlay_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if overlay.query_gen.get() != qid {
                return glib::ControlFlow::Break;
            }
            let Some(host) = overlay
                .thumb_hosts
                .borrow()
                .get(&path)
                .and_then(|w| w.upgrade())
            else {
                return glib::ControlFlow::Break;
            };
            queue_pdf_preview_job(overlay, host, path.clone(), qid);
            glib::ControlFlow::Break
        });
        return;
    }

    THUMB_INFLIGHT.fetch_add(1, Ordering::Relaxed);
    let path_key = path.clone();
    let _ = std::thread::Builder::new()
        .name("metis-pdf-preview".into())
        .spawn(move || {
            let pixels = decode_pdf_first_page(&path, THUMB_PREVIEW);
            THUMB_INFLIGHT.fetch_sub(1, Ordering::Relaxed);
            let Some(pixels) = pixels else {
                return;
            };
            let _ = glib::idle_add_once(move || {
                let Some(overlay) = overlay_rc_clone() else {
                    return;
                };
                if overlay.query_gen.get() != qid || !overlay.open.get() {
                    return;
                }
                let selected = overlay.selected_path.borrow().clone();
                if selected.as_deref() != Some(path_key.as_str()) {
                    return;
                }
                let Some(host) = overlay
                    .thumb_hosts
                    .borrow()
                    .get(&path_key)
                    .and_then(|w| w.upgrade())
                else {
                    return;
                };
                if let Some(pic) = picture_from_pixels(pixels, THUMB_PREVIEW) {
                    while let Some(child) = host.first_child() {
                        host.remove(&child);
                    }
                    host.append(&pic);
                }
            });
        });
}

fn decode_pdf_first_page(path: &str, size: i32) -> Option<ThumbPixels> {
    if !safe_thumb_path(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PDF_PREVIEW_BYTES {
        return None;
    }

    let tmp = make_preview_tmpdir()?;
    let out_prefix = tmp.join("page");
    let path_owned = path.to_string();
    let out_prefix_owned = out_prefix.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("metis-pdftoppm".into())
        .spawn(move || {
            let output = std::process::Command::new("pdftoppm")
                .args([
                    "-singlefile",
                    "-png",
                    "-f",
                    "1",
                    "-l",
                    "1",
                    "-scale-to",
                    &size.to_string(),
                ])
                .arg(&path_owned)
                .arg(&out_prefix_owned)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = tx.send(output);
        });

    let result = match rx.recv_timeout(PDF_PREVIEW_TIMEOUT) {
        Ok(Ok(status)) if status.success() => {
            let png = out_prefix.with_extension("png");
            if png.is_file() {
                let png_str = png.to_string_lossy().into_owned();
                gdk_pixbuf::Pixbuf::from_file_at_scale(&png_str, size, size, true)
                    .ok()
                    .map(|pixbuf| ThumbPixels {
                        width: pixbuf.width(),
                        height: pixbuf.height(),
                        has_alpha: pixbuf.has_alpha(),
                        rowstride: usize::try_from(pixbuf.rowstride()).unwrap_or_else(|_| {
                            usize::try_from(pixbuf.width() * pixbuf.n_channels())
                                .unwrap_or_default()
                        }),
                        bytes: pixbuf.read_pixel_bytes(),
                    })
            } else {
                None
            }
        }
        _ => None,
    };
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn queue_office_preview_job(overlay: Rc<SearchOverlay>, host: gtk::Box, path: String, qid: u32) {
    // Reuse thumb_hosts keyed by a contents: prefix so we don't collide with PDF thumbs.
    let host_key = format!("contents:{path}");
    overlay
        .thumb_hosts
        .borrow_mut()
        .insert(host_key.clone(), host.downgrade());

    let path_for_worker = path.clone();
    let _ = std::thread::Builder::new()
        .name("metis-office-preview".into())
        .spawn(move || {
            let text = extract_office_text(&path_for_worker);
            let _ = glib::idle_add_once(move || {
                let Some(overlay) = overlay_rc_clone() else {
                    return;
                };
                if overlay.query_gen.get() != qid || !overlay.open.get() {
                    return;
                }
                let selected = overlay.selected_path.borrow().clone();
                if selected.as_deref() != Some(path.as_str()) {
                    return;
                }
                let Some(host) = overlay
                    .thumb_hosts
                    .borrow()
                    .get(&host_key)
                    .and_then(|w| w.upgrade())
                else {
                    return;
                };
                fill_contents_host(&host, text.as_deref());
            });
        });
}

fn find_libreoffice() -> Option<&'static str> {
    *LIBREOFFICE_BIN.get_or_init(|| {
        ["soffice", "libreoffice"].into_iter().find(|&bin| {
            std::process::Command::new(bin)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    })
}

fn make_preview_tmpdir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let seq = PREVIEW_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = base
        .join("metis")
        .join(format!("preview-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn extract_office_text(path: &str) -> Option<String> {
    if !safe_thumb_path(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_OFFICE_PREVIEW_BYTES {
        return None;
    }
    let soffice = find_libreoffice()?;
    let tmp = make_preview_tmpdir()?;
    let outdir = tmp.clone();
    let profile = tmp.join("lo-profile");
    let _ = std::fs::create_dir_all(&profile);
    let profile_uri = format!("file://{}", profile.display());

    let path_owned = path.to_string();
    let outdir_owned = outdir.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("metis-soffice".into())
        .spawn(move || {
            let status = std::process::Command::new(soffice)
                .args([
                    "--headless",
                    "--norestore",
                    "--nolockcheck",
                    &format!("-env:UserInstallation={profile_uri}"),
                    "--convert-to",
                    "txt:Text",
                    "--outdir",
                ])
                .arg(&outdir_owned)
                .arg(&path_owned)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = tx.send(status);
        });

    let text = match rx.recv_timeout(OFFICE_PREVIEW_TIMEOUT) {
        Ok(Ok(status)) if status.success() => {
            let stem = Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned());
            let named = stem.map(|s| outdir.join(format!("{s}.txt")));
            if let Some(txt) = named.filter(|p| p.is_file()) {
                read_converted_txt(&txt)
            } else {
                // Some filters emit different names — pick any .txt in outdir.
                let mut found = None;
                if let Ok(rd) = std::fs::read_dir(&outdir) {
                    for ent in rd.flatten() {
                        if ent.path().extension().and_then(|e| e.to_str()) == Some("txt") {
                            found = Some(ent.path());
                            break;
                        }
                    }
                }
                found.and_then(|p| read_converted_txt(&p))
            }
        }
        _ => None,
    };
    let _ = std::fs::remove_dir_all(&tmp);
    text
}

fn read_converted_txt(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    let lossy = String::from_utf8_lossy(&data);
    let text: String = lossy.chars().take(OFFICE_PREVIEW_CHARS).collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn decode_thumb_pixels(path: &str, size: i32, prefer_file: bool) -> Option<ThumbPixels> {
    let candidates: Vec<String> = if prefer_file {
        let mut v = vec![path.to_string()];
        if let Some(t) = xdg_thumbnail_path(path) {
            v.push(t.to_string_lossy().into_owned());
        }
        v
    } else {
        let t = xdg_thumbnail_path(path)?;
        vec![t.to_string_lossy().into_owned()]
    };

    for cand in candidates {
        if !safe_thumb_path(&cand) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&cand) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_THUMB_BYTES {
            continue;
        }
        let Ok(pixbuf) = gdk_pixbuf::Pixbuf::from_file_at_scale(&cand, size, size, true) else {
            continue;
        };
        return Some(ThumbPixels {
            width: pixbuf.width(),
            height: pixbuf.height(),
            has_alpha: pixbuf.has_alpha(),
            rowstride: usize::try_from(pixbuf.rowstride()).unwrap_or_else(|_| {
                usize::try_from(pixbuf.width() * pixbuf.n_channels()).unwrap_or_default()
            }),
            bytes: pixbuf.read_pixel_bytes(),
        });
    }
    None
}

fn safe_thumb_path(path: &str) -> bool {
    let p = Path::new(path);
    // Refuse odd paths; metadata checks is_file (no dirs). Symlink to huge
    // files still capped by MAX_THUMB_BYTES after metadata.
    !path.is_empty() && p.is_absolute()
}

fn picture_from_pixels(pixels: ThumbPixels, size: i32) -> Option<gtk::Picture> {
    let format = if pixels.has_alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let tex = gdk::MemoryTexture::new(
        pixels.width,
        pixels.height,
        format,
        &pixels.bytes,
        pixels.rowstride,
    );
    let pic = gtk::Picture::for_paintable(&tex);
    // Contain keeps tall/narrow images inside the box; Cover + natural size
    // previously let extreme aspect ratios stretch the preview column.
    pic.set_content_fit(gtk::ContentFit::Contain);
    pic.set_can_shrink(true);
    pic.set_size_request(size.min(PREVIEW_MEDIA_MAX), size.min(PREVIEW_MEDIA_MAX));
    pic.set_hexpand(true);
    pic.set_vexpand(false);
    pic.add_css_class("metis-files-search-thumb");
    Some(pic)
}

fn xdg_thumbnail_path(path: &str) -> Option<std::path::PathBuf> {
    let uri = gio::File::for_path(path).uri();
    let digest = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())?;
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    let base = cache.join("thumbnails");
    for dir in ["large", "x-large", "normal"] {
        let candidate = base.join(dir).join(format!("{digest}.png"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Heuristic: query isn't in the path/basename, so this hit came from file contents.
fn match_looks_like_content(hit: &SearchHit, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.len() < 2 {
        return false;
    }
    let path = hit.path.to_ascii_lowercase();
    let base = hit.basename.to_ascii_lowercase();
    if path.contains(&q) || base.contains(&q) {
        return false;
    }
    let compact_q: String = q.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let compact_base: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if compact_q.len() >= 2 && compact_base.contains(&compact_q) {
        return false;
    }
    true
}

fn mime_is_image(mime: Option<&str>) -> bool {
    mime.is_some_and(|m| m.starts_with("image/"))
}

fn mime_is_video(mime: Option<&str>) -> bool {
    mime.is_some_and(|m| m.starts_with("video/"))
}

fn mime_is_pdf(path: &str, mime: Option<&str>) -> bool {
    mime.is_some_and(|m| m.contains("pdf"))
        || Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

fn is_office_doc(path: &str, mime: Option<&str>) -> bool {
    if let Some(m) = mime {
        let m = m.to_ascii_lowercase();
        if m.contains("msword")
            || m.contains("officedocument")
            || m.contains("ms-excel")
            || m.contains("ms-powerpoint")
            || m.contains("opendocument")
            || m == "application/rtf"
            || m == "text/rtf"
        {
            return true;
        }
    }
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
            )
        })
}

fn category_icon(cat: &SearchCategory, mime: Option<&str>) -> &'static str {
    if let Some(m) = mime {
        if m.starts_with("image/") {
            return "image-x-generic-symbolic";
        }
        if m.starts_with("video/") {
            return "video-x-generic-symbolic";
        }
        if m.starts_with("audio/") {
            return "audio-x-generic-symbolic";
        }
        if m.contains("pdf") {
            return "application-pdf-symbolic";
        }
    }
    match cat {
        SearchCategory::Images => "image-x-generic-symbolic",
        SearchCategory::Video => "video-x-generic-symbolic",
        SearchCategory::Audio => "audio-x-generic-symbolic",
        SearchCategory::Documents => "x-office-document-symbolic",
        SearchCategory::Code => "text-x-script-symbolic",
        SearchCategory::Archives => "package-x-generic-symbolic",
        SearchCategory::Downloads => "folder-download-symbolic",
        SearchCategory::Desktop => "user-desktop-symbolic",
        SearchCategory::Other => "text-x-generic-symbolic",
    }
}

fn show_preview(overlay: &Rc<SearchOverlay>, hit: &SearchHit, qid: u32) {
    clear_box(&overlay.preview_body);

    let name = gtk::Label::new(Some(&hit.basename));
    name.set_xalign(0.0);
    name.set_wrap(true);
    name.add_css_class("metis-files-search-name");
    overlay.preview_body.append(&name);

    let folder = Path::new(&hit.path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    // Long paths must not grow the preview width — single-line TextView in a
    // horizontal ScrolledWindow (same idea as the Contents pane).
    let path_buf = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    path_buf.set_text(&shorten_home(&folder));
    let path_view = gtk::TextView::builder()
        .buffer(&path_buf)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::None)
        .hexpand(true)
        .build();
    path_view.add_css_class("metis-files-search-path");
    path_view.set_left_margin(4);
    path_view.set_right_margin(4);
    path_view.set_top_margin(2);
    path_view.set_bottom_margin(2);
    let path_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .min_content_height(28)
        .child(&path_view)
        .build();
    path_scroll.set_propagate_natural_width(false);
    path_scroll.add_css_class("metis-files-search-path-scroll");
    overlay.preview_body.append(&path_scroll);

    let meta = format!(
        "{}  ·  {}",
        format_size(hit.size_bytes),
        format_mode(hit.mode.or_else(|| live_mode(&hit.path)))
    );
    let meta_lab = gtk::Label::new(Some(&meta));
    meta_lab.set_xalign(0.0);
    meta_lab.set_wrap(true);
    meta_lab.add_css_class("metis-files-search-meta");
    overlay.preview_body.append(&meta_lab);

    let cat = SearchCategory::from_str_lossy(&hit.category);
    let media_host = icon_placeholder(hit, 96);
    media_host.add_css_class("metis-files-search-preview-thumb");
    media_host.set_hexpand(true);
    media_host.set_halign(gtk::Align::Fill);
    media_host.set_valign(gtk::Align::Center);
    media_host.set_size_request(-1, PREVIEW_MEDIA_MAX);
    media_host.set_overflow(gtk::Overflow::Hidden);
    overlay.preview_body.append(&media_host);

    let wants_image = matches!(cat, SearchCategory::Images) || mime_is_image(hit.mime.as_deref());
    let wants_video = matches!(cat, SearchCategory::Video) || mime_is_video(hit.mime.as_deref());
    let wants_pdf = mime_is_pdf(&hit.path, hit.mime.as_deref());
    let wants_office = is_office_doc(&hit.path, hit.mime.as_deref());
    if wants_image || wants_video {
        queue_thumb_job(
            overlay.clone(),
            media_host.clone(),
            hit.path.clone(),
            THUMB_PREVIEW,
            qid,
            wants_image,
        );
    } else if wants_pdf {
        queue_pdf_preview_job(overlay.clone(), media_host, hit.path.clone(), qid);
    }

    if let Some(text) = preview_text(hit) {
        append_contents_pane(&overlay.preview_body, &text);
    } else if wants_office {
        let contents_host = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        contents_host.set_hexpand(true);
        contents_host.set_vexpand(true);
        let sn_title = gtk::Label::new(Some(&metis_i18n::tr("Contents")));
        sn_title.set_xalign(0.0);
        sn_title.add_css_class("metis-files-search-group-title");
        contents_host.append(&sn_title);
        let loading = gtk::Label::new(Some(&metis_i18n::tr("Loading preview…")));
        loading.set_xalign(0.0);
        loading.add_css_class("metis-muted-label");
        loading.add_css_class("metis-files-search-contents-body");
        contents_host.append(&loading);
        overlay.preview_body.append(&contents_host);
        queue_office_preview_job(overlay.clone(), contents_host, hit.path.clone(), qid);
    } else if !wants_image && !wants_video && !wants_pdf {
        let empty = gtk::Label::new(Some(&metis_i18n::tr("No text preview")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-muted-label");
        overlay.preview_body.append(&empty);
    }

    append_preview_tags(overlay, hit, qid);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.add_css_class("metis-files-search-actions");
    let open = action_btn(&metis_i18n::tr("Open"));
    let path = hit.path.clone();
    open.connect_clicked(move |_| {
        open_path(&path);
        dismiss();
    });
    actions.append(&open);
    let copy = action_btn(&metis_i18n::tr("Copy"));
    let path = hit.path.clone();
    copy.connect_clicked(move |_| copy_path(&path));
    actions.append(&copy);
    let folder = action_btn(&metis_i18n::tr("Open folder"));
    let path = hit.path.clone();
    folder.connect_clicked(move |_| {
        reveal_path(&path);
        dismiss();
    });
    actions.append(&folder);
    overlay.preview_body.append(&actions);

    overlay.preview.set_visible(true);
    overlay.preview_open.set(true);
}

fn is_auto_tag(tag: &str) -> bool {
    tag.starts_with("cat:") || tag.starts_with("dir:") || tag.starts_with("year:")
}

fn append_preview_tags(overlay: &Rc<SearchOverlay>, hit: &SearchHit, qid: u32) {
    let title = gtk::Label::new(Some(&metis_i18n::tr("Tags")));
    title.set_xalign(0.0);
    title.add_css_class("metis-files-search-group-title");
    overlay.preview_body.append(&title);

    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chips.add_css_class("metis-files-search-tag-chips");
    chips.set_halign(gtk::Align::Start);
    let chips_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&chips)
        .build();
    chips_scroll.set_propagate_natural_height(true);
    overlay.preview_body.append(&chips_scroll);

    let mut tags = hit.tags.clone();
    tags.sort();
    tags.dedup();
    if tags.is_empty() {
        let none = gtk::Label::new(Some(&metis_i18n::tr("No tags")));
        none.add_css_class("metis-muted-label");
        chips.append(&none);
    } else {
        for tag in &tags {
            let auto = is_auto_tag(tag);
            let btn = gtk::Button::with_label(tag);
            btn.add_css_class("flat");
            btn.add_css_class("metis-files-search-tag-chip");
            if auto {
                btn.add_css_class("auto");
                btn.set_tooltip_text(Some(&metis_i18n::tr("Auto tag — click to filter")));
            } else {
                btn.set_tooltip_text(Some(&metis_i18n::tr("Click to filter · use − to remove")));
            }
            {
                let overlay = overlay.clone();
                let tag = tag.clone();
                btn.connect_clicked(move |_| {
                    append_tag_filter(&overlay, &tag);
                });
            }
            chips.append(&btn);
            if !auto {
                let rm = gtk::Button::from_icon_name("list-remove-symbolic");
                rm.add_css_class("flat");
                rm.add_css_class("metis-files-search-tag-remove");
                rm.set_tooltip_text(Some(&metis_i18n::tr("Remove tag")));
                let overlay = overlay.clone();
                let path = hit.path.clone();
                let tag = tag.clone();
                rm.connect_clicked(move |_| {
                    tag_mutate_async(&overlay, path.clone(), tag.clone(), false, qid);
                });
                chips.append(&rm);
            }
        }
    }

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    add_row.add_css_class("metis-files-search-tag-add");
    let entry = gtk::Entry::builder()
        .placeholder_text(metis_i18n::tr("Add tag…"))
        .hexpand(true)
        .build();
    entry.add_css_class("metis-files-search-entry");
    let add_btn = action_btn(&metis_i18n::tr("Add"));
    {
        let overlay = overlay.clone();
        let path = hit.path.clone();
        let entry = entry.clone();
        add_btn.connect_clicked(move |_| {
            let tag = entry.text().trim().to_string();
            if tag.is_empty() {
                return;
            }
            entry.set_text("");
            tag_mutate_async(&overlay, path.clone(), tag, true, qid);
        });
    }
    {
        let overlay = overlay.clone();
        let path = hit.path.clone();
        entry.connect_activate(move |e| {
            let tag = e.text().trim().to_string();
            if tag.is_empty() {
                return;
            }
            e.set_text("");
            tag_mutate_async(&overlay, path.clone(), tag, true, qid);
        });
    }
    {
        let entry_changed = entry.clone();
        entry_changed.connect_changed(move |e| {
            let prefix = e.text().trim().to_string();
            if prefix.len() < 2 {
                e.set_tooltip_text(Some(&metis_i18n::tr("Add tag…")));
                return;
            }
            let _ = std::thread::Builder::new()
                .name("metis-tag-suggest".into())
                .spawn(move || {
                    let resp = search_request(&SearchRequest::SuggestTags { prefix, limit: 8 });
                    let hint = match resp {
                        SearchResponse::Tags { tags } if !tags.is_empty() => Some(format!(
                            "{}: {}",
                            metis_i18n::tr("Suggestions"),
                            tags.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                        )),
                        _ => None,
                    };
                    let _ = glib::idle_add_once(move || {
                        let Some(overlay) = overlay_rc_clone() else {
                            return;
                        };
                        if !overlay.open.get() || !overlay.preview_open.get() {
                            return;
                        }
                        if let Some(hint) = hint {
                            apply_tag_suggest_tooltip(&overlay, &hint);
                        }
                    });
                });
        });
    }
    add_row.append(&entry);
    add_row.append(&add_btn);
    overlay.preview_body.append(&add_row);
}

fn append_tag_filter(overlay: &Rc<SearchOverlay>, tag: &str) {
    let token = format!("tag:{tag}");
    let current = overlay.entry.text().to_string();
    if current.split_whitespace().any(|t| t == token) {
        return;
    }
    let next = if current.trim().is_empty() {
        token
    } else {
        format!("{current} {token}")
    };
    overlay.suppress_chip.set(true);
    overlay.entry.set_text(&next);
    overlay.suppress_chip.set(false);
    schedule_query(overlay, true);
}

fn apply_tag_suggest_tooltip(overlay: &SearchOverlay, hint: &str) {
    let mut child = overlay.preview_body.first_child();
    while let Some(widget) = child {
        if let Ok(entry) = widget.clone().downcast::<gtk::Entry>() {
            entry.set_tooltip_text(Some(hint));
        } else if let Ok(box_) = widget.clone().downcast::<gtk::Box>() {
            let mut inner = box_.first_child();
            while let Some(w) = inner {
                if let Ok(entry) = w.clone().downcast::<gtk::Entry>() {
                    entry.set_tooltip_text(Some(hint));
                }
                inner = w.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
}

fn tag_mutate_async(_overlay: &Rc<SearchOverlay>, path: String, tag: String, add: bool, qid: u32) {
    let req = if add {
        SearchRequest::TagAdd {
            path: path.clone(),
            tag: tag.clone(),
        }
    } else {
        SearchRequest::TagRemove {
            path: path.clone(),
            tag: tag.clone(),
        }
    };
    let _ = std::thread::Builder::new()
        .name("metis-tag-mutate".into())
        .spawn(move || {
            let resp = search_request(&req);
            let _ = glib::idle_add_once(move || {
                let Some(overlay) = overlay_rc_clone() else {
                    return;
                };
                if overlay.query_gen.get() != qid || !overlay.open.get() {
                    return;
                }
                match resp {
                    SearchResponse::Ok => {
                        let updated = {
                            let mut hits = overlay.last_hits.borrow_mut();
                            if let Some(hit) = hits.iter_mut().find(|h| h.path == path) {
                                if add {
                                    if !hit.tags.iter().any(|t| t == &tag) {
                                        hit.tags.push(tag.clone());
                                    }
                                } else {
                                    hit.tags.retain(|t| t != &tag);
                                }
                                Some(hit.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(updated) = updated {
                            // Keep broad cache in sync for chip filters.
                            if let Some(hit) = overlay
                                .broad_hits
                                .borrow_mut()
                                .iter_mut()
                                .find(|h| h.path == path)
                            {
                                hit.tags = updated.tags.clone();
                            }
                            show_preview(&overlay, &updated, qid);
                        }
                    }
                    SearchResponse::Error { message } => {
                        tracing::warn!(%message, %path, add, "tag mutate failed");
                    }
                    _ => {}
                }
            });
        });
}

fn close_preview(overlay: &SearchOverlay) {
    overlay.preview.set_visible(false);
    overlay.preview_open.set(false);
    match overlay.selected_path.try_borrow_mut() {
        Ok(mut path) => *path = None,
        Err(err) => tracing::error!(%err, "files search selected_path already borrowed"),
    }
    clear_box(&overlay.preview_body);
}

/// Prefer a live UTF-8 read of the file; fall back to the index snippet.
fn preview_text(hit: &SearchHit) -> Option<String> {
    // Prefer the index snippet on the UI thread — avoid sync multi‑MB reads
    // that freeze chip/result updates. Live file read only for small files.
    if let Some(live) = read_text_preview(&hit.path, 12_000) {
        return Some(live);
    }
    hit.snippet
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn read_text_preview(path: &str, max_chars: usize) -> Option<String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    // Keep sync preview cheap — large files use index snippet or Office async.
    const MAX_SYNC_PREVIEW_BYTES: u64 = 256 * 1024;
    if !meta.is_file() || meta.len() > MAX_SYNC_PREVIEW_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    if data.iter().take(512).any(|b| *b == 0) {
        return None;
    }
    let lossy = String::from_utf8_lossy(&data);
    let replacement = lossy.chars().filter(|c| *c == '\u{FFFD}').count();
    if replacement > 8 && replacement * 20 > lossy.chars().count() {
        return None;
    }
    let text: String = lossy.chars().take(max_chars).collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if bytes == 0 {
        "—".into()
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_mode(mode: Option<u32>) -> String {
    let Some(mode) = mode else {
        return "—".into();
    };
    let perms = mode & 0o777;
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (perms >> shift) & 0o7;
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        s.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

fn live_mode(path: &str) -> Option<u32> {
    std::fs::metadata(path).ok().map(|m| m.permissions().mode())
}

fn shorten_home(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Some(rest) = path.strip_prefix(&home)
    {
        if rest.is_empty() {
            return "~".into();
        }
        return format!("~{rest}");
    }
    path.to_string()
}

fn copy_path(path: &str) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let clipboard = display.clipboard();
    clipboard.set_text(path);
}

fn open_path(path: &str) {
    let file = gio::File::for_path(path);
    if let Err(err) = gio::AppInfo::launch_default_for_uri(&file.uri(), gio::AppLaunchContext::NONE)
    {
        tracing::warn!(%err, %path, "open file failed");
    }
}

fn reveal_path(path: &str) {
    let parent = Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| Path::new(path).to_path_buf());
    if let Some(fm) = metis_config::resolve_file_manager() {
        let _ = std::process::Command::new(fm).arg(&parent).spawn();
        return;
    }
    open_path(parent.to_string_lossy().as_ref());
}

fn clear_box(box_: &gtk::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}
