//! Settings → System → File search (`search.json` + metis-indexd controls).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use metis_config::{
    SearchConfig, SearchExtractors, load_search_config, resolved_search_roots, save_search_config,
};
use metis_i18n::tr;
use metis_protocol::search::{SearchRequest, SearchResponse, SearchStatus, search_request};

use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("search");

    content.append(&ui::hint(&tr(
        "Metis indexes file names and text content via metis-indexd. Search from the \
         edge-bar icon next to the Metis menu. User tags survive renames when possible.",
    )));

    let cfg = load_search_config();

    // ---- Indexing toggles ----
    let (gen_card, gen_body) =
        ui::section_with_icon(&tr("Indexing"), "folder-saved-search-symbolic");
    let (enabled_row, enabled) = ui::switch_row(&tr("Enable desktop search index"));
    enabled.set_active(cfg.enabled);
    gen_body.append(&enabled_row);
    let (paused_row, paused) = ui::switch_row(&tr("Pause indexing"));
    paused.set_active(cfg.paused);
    gen_body.append(&paused_row);
    gen_body.append(&ui::hint(&tr(
        "Enable and Pause apply immediately (no Save needed). Pause holds content \
         extraction; Disable stops indexing and drops the extract queue. Search of \
         already-indexed files still works while paused.",
    )));

    let effort = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    effort.set_value(f64::from(cfg.index_effort));
    effort.set_size_request(200, -1);
    effort.set_draw_value(true);
    effort.set_digits(0);
    ui::forward_wheel_to_page_scroller(&effort);
    gen_body.append(&ui::row_with_icon(
        "preferences-system-symbolic",
        &tr("Index effort"),
        &effort,
    ));
    gen_body.append(&ui::hint(&tr(
        "Low keeps content extraction light (fewer workers, short pauses). High finishes text \
         indexing faster. File names stay searchable either way. Login crawls only refresh \
         files that changed.",
    )));
    content.append(&gen_card);

    // ---- Roots (list UI, seeded with home) ----
    let (roots_card, roots_body) = ui::section_with_icon(&tr("Indexed folders"), "folder-symbolic");
    roots_body.append(&ui::hint(&tr(
        "Folders Metis crawls. Your home folder is included by default.",
    )));
    let roots_list = gtk::ListBox::new();
    roots_list.add_css_class("metis-settings-list");
    roots_list.add_css_class("metis-search-path-list");
    roots_list.set_selection_mode(gtk::SelectionMode::None);
    let roots_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(140)
        .height_request(140)
        .child(&roots_list)
        .build();
    roots_scroll.add_css_class("metis-settings-list-scroll");
    roots_scroll.add_css_class("metis-search-path-scroll");
    roots_body.append(&roots_scroll);

    let roots_state: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(display_roots(&cfg)));
    rebuild_path_list(&roots_list, &roots_state);

    let add_root = gtk::Button::with_label(&tr("Add folder…"));
    add_root.add_css_class("flat");
    {
        let roots_state = roots_state.clone();
        let roots_list = roots_list.clone();
        add_root.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk::Window>() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title(tr("Choose folder to index"))
                .modal(true)
                .build();
            let roots_state = roots_state.clone();
            let roots_list = roots_list.clone();
            dialog.select_folder(Some(&parent), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    let s = path.to_string_lossy().to_string();
                    let mut g = roots_state.borrow_mut();
                    if !g.iter().any(|p| p == &s) {
                        g.push(s);
                    }
                    drop(g);
                    rebuild_path_list(&roots_list, &roots_state);
                }
            });
        });
    }
    roots_body.append(&add_root);
    content.append(&roots_card);

    // ---- Excludes ----
    let (ex_card, ex_body) =
        ui::section_with_icon(&tr("Excluded paths"), "edit-clear-all-symbolic");
    ex_body.append(&ui::hint(&tr(
        "Skip any path containing these substrings (for example /.git/ or /node_modules/).",
    )));
    let ex_list = gtk::ListBox::new();
    ex_list.add_css_class("metis-settings-list");
    ex_list.add_css_class("metis-search-path-list");
    ex_list.set_selection_mode(gtk::SelectionMode::None);
    let ex_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(120)
        .height_request(120)
        .child(&ex_list)
        .build();
    ex_scroll.add_css_class("metis-settings-list-scroll");
    ex_scroll.add_css_class("metis-search-path-scroll");
    ex_body.append(&ex_scroll);

    let excludes_state: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(cfg.excludes.clone()));
    rebuild_path_list(&ex_list, &excludes_state);

    let ex_entry = gtk::Entry::builder()
        .placeholder_text(tr("Add exclude substring…"))
        .hexpand(true)
        .build();
    ex_entry.add_css_class("metis-settings-inset");
    let ex_add = gtk::Button::with_label(&tr("Add"));
    let ex_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    ex_row.add_css_class("metis-settings-inset");
    ex_row.set_margin_top(4);
    ex_row.append(&ex_entry);
    ex_row.append(&ex_add);
    {
        let excludes_state = excludes_state.clone();
        let ex_list = ex_list.clone();
        let ex_entry = ex_entry.clone();
        ex_add.connect_clicked(move |_| {
            let text = ex_entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            let mut g = excludes_state.borrow_mut();
            if !g.iter().any(|p| p == &text) {
                g.push(text);
            }
            drop(g);
            ex_entry.set_text("");
            rebuild_path_list(&ex_list, &excludes_state);
        });
    }
    ex_body.append(&ex_row);
    content.append(&ex_card);

    // ---- Content extractors ----
    let (ext_card, ext_body) =
        ui::section_with_icon(&tr("Content extractors"), "text-x-generic-symbolic");
    ext_body.append(&ui::hint(&tr(
        "What text Metis stores for content search. Changing extractors applies on Save; \
         reindex to refresh already-indexed files.",
    )));
    let (plain_row, plain_sw) = ui::switch_row(&tr("Plain text and source code"));
    plain_sw.set_active(cfg.extractors.plain_text);
    ext_body.append(&plain_row);
    let (html_row, html_sw) = ui::switch_row(&tr("HTML (strip tags)"));
    html_sw.set_active(cfg.extractors.html);
    ext_body.append(&html_row);
    let (pdf_row, pdf_sw) = ui::switch_row(&tr("PDF (pdftotext)"));
    pdf_sw.set_active(cfg.extractors.pdf);
    ext_body.append(&pdf_row);
    ext_body.append(&ui::hint(&tr(
        "PDF extraction needs the pdftotext tool (poppler-utils). If it is missing, \
         PDF files stay name-only until you install it.",
    )));
    content.append(&ext_card);

    // ---- Tag browser ----
    let (tags_card, tags_body) = ui::section_with_icon(&tr("Tags"), "user-bookmarks-symbolic");
    tags_body.append(&ui::hint(&tr(
        "Browse user tags stored by Metis. Add or remove tags from a file in the \
         File search preview. Auto-tags (cat:, dir:, year:) are managed by the indexer.",
    )));
    let tag_filter = gtk::Entry::builder()
        .placeholder_text(tr("Filter tags…"))
        .hexpand(true)
        .build();
    tag_filter.add_css_class("metis-settings-inset");
    tags_body.append(&tag_filter);

    let tags_list = gtk::ListBox::new();
    tags_list.add_css_class("metis-settings-list");
    tags_list.set_selection_mode(gtk::SelectionMode::Single);
    let tags_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(120)
        .height_request(120)
        .child(&tags_list)
        .build();
    tags_scroll.add_css_class("metis-settings-list-scroll");
    tags_body.append(&tags_scroll);

    let tag_files_list = gtk::ListBox::new();
    tag_files_list.add_css_class("metis-settings-list");
    tag_files_list.set_selection_mode(gtk::SelectionMode::Single);
    let tag_files_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(140)
        .height_request(140)
        .child(&tag_files_list)
        .build();
    tag_files_scroll.add_css_class("metis-settings-list-scroll");
    tags_body.append(&ui::hint(&tr("Files with selected tag")));
    tags_body.append(&tag_files_scroll);

    let tag_remove = gtk::Button::with_label(&tr("Remove tag from selected file"));
    tag_remove.add_css_class("flat");
    tag_remove.set_sensitive(false);
    tags_body.append(&tag_remove);

    let selected_tag: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let selected_path: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    refresh_tag_suggestions(&tags_list, "", &selected_tag);

    {
        let tags_list = tags_list.clone();
        let selected_tag = selected_tag.clone();
        let tag_files_list = tag_files_list.clone();
        let selected_path = selected_path.clone();
        let tag_remove = tag_remove.clone();
        tag_filter.connect_changed(move |entry| {
            let prefix = entry.text().trim().to_string();
            refresh_tag_suggestions(&tags_list, &prefix, &selected_tag);
            clear_listbox(&tag_files_list);
            *selected_path.borrow_mut() = None;
            tag_remove.set_sensitive(false);
        });
    }
    {
        let selected_tag = selected_tag.clone();
        let tag_files_list = tag_files_list.clone();
        let selected_path = selected_path.clone();
        let tag_remove = tag_remove.clone();
        tags_list.connect_row_selected(move |_list, row| {
            let tag = row.and_then(|r| {
                r.child()
                    .and_then(|c| c.downcast::<gtk::Label>().ok())
                    .map(|l| l.text().to_string())
            });
            *selected_tag.borrow_mut() = tag.clone();
            *selected_path.borrow_mut() = None;
            tag_remove.set_sensitive(false);
            clear_listbox(&tag_files_list);
            // Skip empty / hint rows ("No user tags yet", errors).
            let Some(tag) = tag.filter(|t| !t.is_empty() && !t.contains(' ')) else {
                return;
            };
            match search_request(&SearchRequest::Query {
                q: String::new(),
                category: None,
                tags: vec![tag],
                limit: 40,
            }) {
                SearchResponse::Hits { hits } => {
                    if hits.is_empty() {
                        let empty = gtk::Label::new(Some(&tr("No files with this tag")));
                        empty.add_css_class("metis-settings-hint");
                        empty.set_xalign(0.0);
                        empty.set_margin_start(8);
                        empty.set_margin_top(8);
                        tag_files_list.append(&empty);
                    } else {
                        for hit in hits {
                            let lab = gtk::Label::new(Some(&hit.path));
                            lab.set_xalign(0.0);
                            lab.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                            lab.set_margin_start(6);
                            lab.set_margin_end(6);
                            lab.set_margin_top(4);
                            lab.set_margin_bottom(4);
                            tag_files_list.append(&lab);
                        }
                    }
                }
                SearchResponse::Error { message } => {
                    let err =
                        gtk::Label::new(Some(&format!("{}: {message}", tr("Search unavailable"))));
                    err.add_css_class("metis-settings-hint");
                    err.set_xalign(0.0);
                    err.set_margin_start(8);
                    tag_files_list.append(&err);
                }
                _ => {}
            }
        });
    }
    {
        let selected_path = selected_path.clone();
        let tag_remove = tag_remove.clone();
        tag_files_list.connect_row_selected(move |_list, row| {
            let path = row
                .and_then(|r| {
                    r.child()
                        .and_then(|c| c.downcast::<gtk::Label>().ok())
                        .map(|l| l.text().to_string())
                })
                .filter(|p| p.starts_with('/'));
            *selected_path.borrow_mut() = path.clone();
            tag_remove.set_sensitive(path.is_some());
        });
    }
    {
        let selected_tag = selected_tag.clone();
        let selected_path = selected_path.clone();
        let tags_list = tags_list.clone();
        let tag_files_list = tag_files_list.clone();
        let tag_filter = tag_filter.clone();
        let tag_remove_btn = tag_remove.clone();
        tag_remove.connect_clicked(move |_| {
            let Some(tag) = selected_tag.borrow().clone() else {
                return;
            };
            let Some(path) = selected_path.borrow().clone() else {
                return;
            };
            match search_request(&SearchRequest::TagRemove {
                path: path.clone(),
                tag: tag.clone(),
            }) {
                SearchResponse::Ok => {
                    *selected_path.borrow_mut() = None;
                    tag_remove_btn.set_sensitive(false);
                    // Refresh file list for this tag.
                    clear_listbox(&tag_files_list);
                    if let SearchResponse::Hits { hits } = search_request(&SearchRequest::Query {
                        q: String::new(),
                        category: None,
                        tags: vec![tag],
                        limit: 40,
                    }) {
                        for hit in hits {
                            let lab = gtk::Label::new(Some(&hit.path));
                            lab.set_xalign(0.0);
                            lab.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                            lab.set_margin_start(6);
                            lab.set_margin_end(6);
                            tag_files_list.append(&lab);
                        }
                    }
                    refresh_tag_suggestions(&tags_list, tag_filter.text().trim(), &selected_tag);
                }
                SearchResponse::Error { message } => {
                    tracing::warn!(%message, %path, "tag remove failed");
                }
                _ => {}
            }
        });
    }
    content.append(&tags_card);

    // ---- Live status / progress / chart ----
    let (stat_card, stat_body) =
        ui::section_with_icon(&tr("Index status"), "view-statistics-symbolic");

    let headline = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    headline.add_css_class("metis-settings-inset");
    headline.set_margin_bottom(4);
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let docs_pill = make_pill(&tr("0 files"));
    let content_pill = make_pill(&tr("0 with text"));
    let state_pill = make_pill(&tr("Idle"));
    headline.append(&spinner);
    headline.append(&docs_pill);
    headline.append(&content_pill);
    headline.append(&state_pill);
    stat_body.append(&headline);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_fraction(0.0);
    progress.set_visible(false);
    progress.add_css_class("metis-search-progress");
    let progress_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    progress_wrap.add_css_class("metis-settings-inset");
    progress_wrap.append(&progress);
    stat_body.append(&progress_wrap);

    let chart_data: Rc<RefCell<Vec<(String, u64)>>> = Rc::new(RefCell::new(Vec::new()));
    let chart_hit: Rc<RefCell<PieHitMap>> = Rc::new(RefCell::new(PieHitMap::default()));
    let chart = gtk::DrawingArea::builder()
        .content_width(440)
        .content_height(240)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build();
    chart.add_css_class("metis-search-chart");
    chart.set_tooltip_text(Some(&tr("Hover a slice for details")));
    {
        let chart_data = chart_data.clone();
        let chart_hit = chart_hit.clone();
        chart.set_draw_func(move |_area, cr, w, h| {
            let hit = draw_category_pie(cr, w, h, &chart_data.borrow());
            *chart_hit.borrow_mut() = hit;
        });
    }
    {
        let chart_hit = chart_hit.clone();
        let chart_for_tip = chart.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| {
            if let Some(tip) = chart_hit.borrow().tooltip_at(x, y) {
                chart_for_tip.set_tooltip_text(Some(&tip));
            } else {
                chart_for_tip.set_tooltip_text(Some(&tr("Hover a slice for details")));
            }
        });
        chart.add_controller(motion);
    }
    let chart_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chart_wrap.add_css_class("metis-settings-inset");
    chart_wrap.set_halign(gtk::Align::Fill);
    chart_wrap.append(&chart);
    stat_body.append(&chart_wrap);

    let status_hint = gtk::Label::new(None);
    status_hint.set_xalign(0.0);
    status_hint.set_wrap(true);
    status_hint.add_css_class("metis-settings-hint");
    stat_body.append(&status_hint);

    content.append(&stat_card);
    // Sit directly under Indexing (before folders / extractors / tags).
    content.reorder_child_after(&stat_card, Some(&gen_card));

    // ---- Actions ----
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_margin_top(8);
    let save_btn = gtk::Button::with_label(&tr("Save"));
    save_btn.add_css_class("suggested-action");
    let reindex_btn = gtk::Button::with_label(&tr("Reindex now"));
    actions.append(&save_btn);
    actions.append(&reindex_btn);
    content.append(&actions);

    let ui_refs = StatusUi {
        spinner: spinner.clone(),
        docs_pill: docs_pill.clone(),
        content_pill: content_pill.clone(),
        state_pill: state_pill.clone(),
        progress: progress.clone(),
        chart: chart.clone(),
        chart_data: chart_data.clone(),
        status_hint: status_hint.clone(),
        enabled: enabled.clone(),
        paused: paused.clone(),
        syncing_switches: Rc::new(Cell::new(false)),
    };
    refresh_status_ui(&ui_refs);

    // Enable / Pause apply live — do not wait for Save.
    {
        let ui_refs = ui_refs.clone();
        enabled.connect_active_notify(move |sw| {
            if ui_refs.syncing_switches.get() {
                return;
            }
            let mut cfg = load_search_config();
            cfg.enabled = sw.is_active();
            if let Err(err) = save_search_config(&cfg) {
                ui_refs
                    .status_hint
                    .set_text(&format!("{}: {err}", tr("Save failed")));
                return;
            }
            let _ = search_request(&SearchRequest::ReloadConfig);
            let msg = if sw.is_active() {
                tr("Desktop search enabled")
            } else {
                tr("Desktop search disabled — indexing stopped")
            };
            ui_refs.status_hint.set_text(&msg);
            refresh_status_ui(&ui_refs);
        });
    }
    {
        let ui_refs = ui_refs.clone();
        paused.connect_active_notify(move |sw| {
            if ui_refs.syncing_switches.get() {
                return;
            }
            let resp = if sw.is_active() {
                search_request(&SearchRequest::Pause)
            } else {
                search_request(&SearchRequest::Resume)
            };
            match resp {
                SearchResponse::Ok => {
                    let msg = if sw.is_active() {
                        tr("Indexing paused")
                    } else {
                        tr("Indexing resumed")
                    };
                    ui_refs.status_hint.set_text(&msg);
                }
                SearchResponse::Error { message } => {
                    ui_refs
                        .status_hint
                        .set_text(&format!("{}: {message}", tr("Indexer unavailable")));
                }
                _ => {}
            }
            refresh_status_ui(&ui_refs);
        });
    }

    // Poll while the page is mapped so reindex progress updates live.
    let poll_alive = Rc::new(Cell::new(true));
    {
        let ui_refs = ui_refs.clone();
        let poll_alive = poll_alive.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(750), move || {
            if !poll_alive.get() {
                return glib::ControlFlow::Break;
            }
            refresh_status_ui(&ui_refs);
            glib::ControlFlow::Continue
        });
    }
    scroller.connect_destroy(move |_| {
        poll_alive.set(false);
    });

    {
        let enabled = enabled.clone();
        let paused = paused.clone();
        let roots_state = roots_state.clone();
        let excludes_state = excludes_state.clone();
        let plain_sw = plain_sw.clone();
        let html_sw = html_sw.clone();
        let pdf_sw = pdf_sw.clone();
        let effort = effort.clone();
        let ui_refs = ui_refs.clone();
        save_btn.connect_clicked(move |_| {
            let roots = roots_state.borrow().clone();
            let home = default_home_path();
            // Persist empty roots when the only entry is the default home.
            let roots = if roots.len() == 1 && roots[0] == home {
                Vec::new()
            } else {
                roots
            };
            let prev = load_search_config();
            let cfg = SearchConfig {
                enabled: enabled.is_active(),
                paused: paused.is_active(),
                roots,
                excludes: excludes_state.borrow().clone(),
                max_file_bytes: prev.max_file_bytes,
                max_body_chars: prev.max_body_chars,
                index_effort: effort.value().round().clamp(0.0, 100.0) as u8,
                extractors: SearchExtractors {
                    plain_text: plain_sw.is_active(),
                    html: html_sw.is_active(),
                    pdf: pdf_sw.is_active(),
                },
            };
            if let Err(err) = save_search_config(&cfg) {
                ui_refs
                    .status_hint
                    .set_text(&format!("{}: {err}", tr("Save failed")));
                return;
            }
            let _ = search_request(&SearchRequest::ReloadConfig);
            ui_refs
                .status_hint
                .set_text(&tr("Saved search preferences"));
            refresh_status_ui(&ui_refs);
        });
    }

    {
        let ui_refs = ui_refs.clone();
        reindex_btn.connect_clicked(move |_| {
            match search_request(&SearchRequest::Reindex) {
                SearchResponse::Ok => {
                    ui_refs.status_hint.set_text(&tr("Reindex started…"));
                    ui_refs.spinner.set_visible(true);
                    ui_refs.spinner.start();
                }
                SearchResponse::Error { message } => {
                    ui_refs
                        .status_hint
                        .set_text(&format!("{}: {message}", tr("Reindex failed")));
                }
                _ => ui_refs.status_hint.set_text(&tr("Unexpected response")),
            }
            refresh_status_ui(&ui_refs);
        });
    }

    scroller.upcast()
}

#[derive(Clone)]
struct StatusUi {
    spinner: gtk::Spinner,
    docs_pill: gtk::Label,
    content_pill: gtk::Label,
    state_pill: gtk::Label,
    progress: gtk::ProgressBar,
    chart: gtk::DrawingArea,
    chart_data: Rc<RefCell<Vec<(String, u64)>>>,
    status_hint: gtk::Label,
    enabled: gtk::Switch,
    paused: gtk::Switch,
    /// Suppress switch notify while syncing from Status IPC.
    syncing_switches: Rc<Cell<bool>>,
}

#[derive(Clone, Default)]
struct PieHitMap {
    cx: f64,
    cy: f64,
    radius: f64,
    /// (start_angle, end_angle, label)
    slices: Vec<(f64, f64, String)>,
}

impl PieHitMap {
    fn tooltip_at(&self, x: f64, y: f64) -> Option<String> {
        if self.radius <= 0.0 || self.slices.is_empty() {
            return None;
        }
        let dx = x - self.cx;
        let dy = y - self.cy;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > self.radius || dist < 1.0 {
            return None;
        }
        // Match Cairo: 0 = east, + = clockwise with y-down. Pie spans [-PI/2, 3PI/2).
        let mut ang = dy.atan2(dx);
        if ang < -std::f64::consts::FRAC_PI_2 {
            ang += std::f64::consts::TAU;
        }
        for (a0, a1, label) in &self.slices {
            if ang >= *a0 && ang < *a1 {
                return Some(label.clone());
            }
        }
        self.slices.last().map(|(_, _, l)| l.clone())
    }
}

fn make_pill(text: &str) -> gtk::Label {
    let lab = gtk::Label::new(Some(text));
    lab.add_css_class("metis-search-stat-pill");
    lab.set_halign(gtk::Align::Start);
    lab
}

fn clear_listbox(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn refresh_tag_suggestions(
    list: &gtk::ListBox,
    prefix: &str,
    selected_tag: &Rc<RefCell<Option<String>>>,
) {
    clear_listbox(list);
    *selected_tag.borrow_mut() = None;
    match search_request(&SearchRequest::SuggestTags {
        prefix: prefix.to_string(),
        limit: 80,
    }) {
        SearchResponse::Tags { tags } => {
            if tags.is_empty() {
                let empty = gtk::Label::new(Some(&tr("No user tags yet")));
                empty.add_css_class("metis-settings-hint");
                empty.set_xalign(0.0);
                empty.set_margin_start(8);
                empty.set_margin_top(8);
                empty.set_margin_bottom(8);
                list.append(&empty);
                return;
            }
            for tag in tags {
                let lab = gtk::Label::new(Some(&tag));
                lab.set_xalign(0.0);
                lab.set_margin_start(8);
                lab.set_margin_end(8);
                lab.set_margin_top(4);
                lab.set_margin_bottom(4);
                list.append(&lab);
            }
        }
        SearchResponse::Error { message } => {
            let err = gtk::Label::new(Some(&format!("{}: {message}", tr("Search unavailable"))));
            err.add_css_class("metis-settings-hint");
            err.set_xalign(0.0);
            err.set_margin_start(8);
            list.append(&err);
        }
        _ => {
            let err = gtk::Label::new(Some(&tr("Search unavailable")));
            err.add_css_class("metis-settings-hint");
            err.set_xalign(0.0);
            err.set_margin_start(8);
            list.append(&err);
        }
    }
}

fn default_home_path() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/home".into())
}

fn display_roots(cfg: &SearchConfig) -> Vec<String> {
    if cfg.roots.is_empty() {
        resolved_search_roots(cfg)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    } else {
        cfg.roots.clone()
    }
}

fn rebuild_path_list(list: &gtk::ListBox, state: &Rc<RefCell<Vec<String>>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let paths = state.borrow().clone();
    if paths.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No paths configured")));
        empty.add_css_class("metis-settings-hint");
        empty.set_xalign(0.0);
        empty.set_margin_start(8);
        empty.set_margin_end(8);
        empty.set_margin_top(12);
        empty.set_margin_bottom(12);
        list.append(&empty);
        return;
    }
    for (idx, path) in paths.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(4);
        row.set_margin_end(4);
        let lab = gtk::Label::new(Some(path));
        lab.set_xalign(0.0);
        lab.set_hexpand(true);
        lab.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        lab.add_css_class("metis-search-path-label");
        let rm = gtk::Button::from_icon_name("list-remove-symbolic");
        rm.add_css_class("flat");
        rm.set_tooltip_text(Some(&tr("Remove")));
        let state = state.clone();
        let list_for_rm = list.clone();
        rm.connect_clicked(move |_| {
            let mut g = state.borrow_mut();
            if idx < g.len() {
                g.remove(idx);
            }
            drop(g);
            rebuild_path_list(&list_for_rm, &state);
        });
        row.append(&lab);
        row.append(&rm);
        list.append(&row);
    }
}

fn refresh_status_ui(ui: &StatusUi) {
    match search_request(&SearchRequest::Status) {
        SearchResponse::Status(s) => apply_status(ui, &s),
        SearchResponse::Error { message } => {
            ui.spinner.stop();
            ui.spinner.set_visible(false);
            ui.progress.set_visible(false);
            ui.state_pill.set_text(&tr("Unavailable"));
            ui.status_hint
                .set_text(&format!("{}: {message}", tr("Indexer unavailable")));
        }
        _ => {}
    }
}

fn apply_status(ui: &StatusUi, s: &SearchStatus) {
    ui.docs_pill
        .set_text(&format!("{} {}", format_count(s.indexed_docs), tr("files")));
    ui.content_pill.set_text(&format!(
        "{} {}",
        format_count(s.with_content),
        tr("with text")
    ));

    ui.syncing_switches.set(true);
    if ui.enabled.is_active() != s.enabled {
        ui.enabled.set_active(s.enabled);
    }
    if ui.paused.is_active() != s.paused {
        ui.paused.set_active(s.paused);
    }
    ui.syncing_switches.set(false);

    // Disabled / paused win over an in-flight crawl for the state pill.
    if !s.enabled {
        ui.spinner.stop();
        ui.spinner.set_visible(false);
        ui.progress.set_visible(false);
        ui.state_pill.set_text(&tr("Disabled"));
    } else if s.paused {
        ui.spinner.stop();
        ui.spinner.set_visible(false);
        ui.progress.set_visible(false);
        ui.state_pill.set_text(&tr("Paused"));
    } else if s.crawling {
        ui.spinner.set_visible(true);
        ui.spinner.start();
        if s.content_pending > 0 && s.crawl_done > 0 && s.crawl_done >= s.crawl_total {
            ui.state_pill.set_text(&tr("Extracting text…"));
        } else {
            ui.state_pill.set_text(&tr("Indexing…"));
        }
        ui.progress.set_visible(true);
        if s.crawl_total > 0 {
            let frac = (s.crawl_done as f64 / s.crawl_total as f64).clamp(0.0, 1.0);
            ui.progress.set_fraction(frac);
            let text = if s.content_pending > 0 {
                format!(
                    "{} / {} · {} {}",
                    format_count(s.crawl_done),
                    format_count(s.crawl_total),
                    format_count(s.content_pending),
                    tr("text pending")
                )
            } else {
                format!(
                    "{} / {}",
                    format_count(s.crawl_done),
                    format_count(s.crawl_total)
                )
            };
            ui.progress.set_text(Some(&text));
        } else {
            ui.progress.set_fraction(0.0);
            ui.progress.pulse();
            ui.progress.set_text(Some(&format!(
                "{} {}",
                format_count(s.crawl_done),
                tr("files processed")
            )));
        }
    } else {
        ui.spinner.stop();
        ui.spinner.set_visible(false);
        ui.progress.set_visible(false);
        ui.state_pill.set_text(&tr("Ready"));
    }

    let cats: Vec<(String, u64)> = s
        .categories
        .iter()
        .map(|c| (category_label(&c.category), c.docs))
        .collect();
    *ui.chart_data.borrow_mut() = cats;
    ui.chart.queue_draw();

    if let Some(err) = &s.last_error {
        ui.status_hint
            .set_text(&format!("{}: {err}", tr("Last error")));
    } else if !s.crawling {
        ui.status_hint.set_text(&format!(
            "{} — {} {}",
            tr("Index ready"),
            format_count(s.indexed_docs),
            tr("documents")
        ));
    }
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        let mut s = n.to_string();
        let mut i = s.len() as isize - 3;
        while i > 0 {
            s.insert(i as usize, ',');
            i -= 3;
        }
        s
    }
}

fn category_label(raw: &str) -> String {
    match raw {
        "documents" | "code" => tr("Documents"),
        "images" => tr("Images"),
        "video" => tr("Video"),
        "audio" => tr("Audio"),
        "archives" => tr("Archives"),
        "other" | "downloads" | "desktop" => tr("Other"),
        _ => raw.to_string(),
    }
}

fn draw_category_pie(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    data: &[(String, u64)],
) -> PieHitMap {
    let w = width as f64;
    let h = height as f64;
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    let _ = cr.paint();

    if data.is_empty() {
        cr.set_source_rgba(0.7, 0.7, 0.7, 0.85);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(13.0);
        cr.move_to(24.0, h / 2.0);
        let _ = cr.show_text(&tr("Category breakdown appears after reindex"));
        return PieHitMap::default();
    }

    // Merge same display labels, then top slices + Other.
    let mut merged: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for (name, count) in data {
        *merged.entry(name.clone()).or_insert(0) += *count;
    }
    let mut slices: Vec<(String, u64)> = merged.into_iter().collect();
    slices.sort_by_key(|a| std::cmp::Reverse(a.1));
    const MAX_SLICES: usize = 6;
    let head = if slices.len() > MAX_SLICES {
        let rest_sum: u64 = slices[MAX_SLICES - 1..].iter().map(|(_, n)| *n).sum();
        let mut head = slices[..MAX_SLICES - 1].to_vec();
        if rest_sum > 0 {
            if let Some(existing) = head.iter_mut().find(|(n, _)| n == &tr("Other")) {
                existing.1 += rest_sum;
            } else {
                head.push((tr("Other"), rest_sum));
            }
        }
        head
    } else {
        slices
    };
    let total: u64 = head.iter().map(|(_, n)| *n).sum::<u64>().max(1);

    // Layout: large pie on the left-center, legend vertically centered on the right.
    let pie_r = (h * 0.42).clamp(64.0, 96.0);
    let cx = (w * 0.32).clamp(pie_r + 16.0, w * 0.45);
    let cy = h / 2.0;
    let colors: [(f64, f64, f64); 6] = [
        (0.00, 0.78, 0.95), // cyan
        (0.45, 0.55, 0.98), // indigo
        (0.20, 0.75, 0.55), // green
        (0.95, 0.65, 0.25), // amber
        (0.75, 0.45, 0.95), // violet
        (0.55, 0.60, 0.68), // grey
    ];

    // Minimum visible sweep so tiny categories still show; renormalize to TAU.
    const MIN_SWEEP: f64 = 0.045;
    let mut sweeps: Vec<f64> = head
        .iter()
        .map(|(_, c)| {
            let raw = (*c as f64 / total as f64) * std::f64::consts::TAU;
            if *c > 0 { raw.max(MIN_SWEEP) } else { 0.0 }
        })
        .collect();
    let sweep_sum: f64 = sweeps.iter().sum::<f64>().max(f64::EPSILON);
    for s in &mut sweeps {
        *s = *s / sweep_sum * std::f64::consts::TAU;
    }

    let mut hit = PieHitMap {
        cx,
        cy,
        radius: pie_r,
        slices: Vec::with_capacity(head.len()),
    };

    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (i, ((name, count), sweep)) in head.iter().zip(sweeps.iter()).enumerate() {
        let (r, g, b) = colors[i % colors.len()];
        cr.set_source_rgba(r, g, b, 0.92);
        cr.move_to(cx, cy);
        cr.arc(cx, cy, pie_r, angle, angle + sweep);
        cr.close_path();
        let _ = cr.fill();
        // Separators so adjacent slices stay distinct.
        cr.set_source_rgba(0.12, 0.14, 0.18, 0.55);
        cr.set_line_width(1.5);
        cr.move_to(cx, cy);
        cr.line_to(cx + pie_r * angle.cos(), cy + pie_r * angle.sin());
        let _ = cr.stroke();

        let tip = format!(
            "{} — {} ( {:.1}% )",
            name,
            format_count(*count),
            (*count as f64 / total as f64) * 100.0
        );
        hit.slices.push((angle, angle + sweep, tip));
        angle += sweep;
    }
    cr.set_source_rgba(0.2, 0.25, 0.3, 0.4);
    cr.set_line_width(1.25);
    cr.arc(cx, cy, pie_r, 0.0, std::f64::consts::TAU);
    let _ = cr.stroke();

    let legend_x = (cx + pie_r + 28.0).min(w - 160.0);
    let row_h = 22.0;
    let legend_h = head.len() as f64 * row_h;
    let mut ly = ((h - legend_h) / 2.0 + 14.0).max(18.0);
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(12.5);
    for (i, (name, count)) in head.iter().enumerate() {
        let (r, g, b) = colors[i % colors.len()];
        cr.set_source_rgba(r, g, b, 0.95);
        cr.rectangle(legend_x, ly - 9.0, 12.0, 12.0);
        let _ = cr.fill();
        cr.set_source_rgba(0.88, 0.92, 0.96, 0.98);
        cr.move_to(legend_x + 18.0, ly);
        let label = format!("{name}  {}", format_count(*count));
        let _ = cr.show_text(&label);
        ly += row_h;
    }
    hit
}
