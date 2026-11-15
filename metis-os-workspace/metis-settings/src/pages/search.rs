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

    let roots_pills = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    roots_pills.add_css_class("metis-settings-inset");
    roots_pills.set_margin_top(4);
    roots_pills.set_margin_bottom(4);
    roots_pills.add_css_class("metis-search-root-pills");
    stat_body.append(&roots_pills);

    let chart_data: Rc<RefCell<Vec<(String, u64)>>> = Rc::new(RefCell::new(Vec::new()));
    let chart = gtk::DrawingArea::builder()
        .content_height(160)
        .hexpand(true)
        .build();
    chart.add_css_class("metis-search-chart");
    {
        let chart_data = chart_data.clone();
        chart.set_draw_func(move |_area, cr, w, h| {
            draw_category_chart(cr, w, h, &chart_data.borrow());
        });
    }
    let chart_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chart_wrap.add_css_class("metis-settings-inset");
    chart_wrap.append(&chart);
    stat_body.append(&chart_wrap);

    let status_hint = gtk::Label::new(None);
    status_hint.set_xalign(0.0);
    status_hint.set_wrap(true);
    status_hint.add_css_class("metis-settings-hint");
    stat_body.append(&status_hint);

    content.append(&stat_card);

    // ---- Actions ----
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("metis-settings-actions");
    actions.set_margin_top(8);
    let save_btn = gtk::Button::with_label(&tr("Save"));
    save_btn.add_css_class("suggested-action");
    let reindex_btn = gtk::Button::with_label(&tr("Reindex now"));
    let pause_btn = gtk::Button::with_label(&tr("Pause"));
    let resume_btn = gtk::Button::with_label(&tr("Resume"));
    actions.append(&save_btn);
    actions.append(&reindex_btn);
    actions.append(&pause_btn);
    actions.append(&resume_btn);
    content.append(&actions);

    let ui_refs = StatusUi {
        spinner: spinner.clone(),
        docs_pill: docs_pill.clone(),
        content_pill: content_pill.clone(),
        state_pill: state_pill.clone(),
        progress: progress.clone(),
        roots_pills: roots_pills.clone(),
        chart: chart.clone(),
        chart_data: chart_data.clone(),
        status_hint: status_hint.clone(),
        paused: paused.clone(),
    };
    refresh_status_ui(&ui_refs);

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
    {
        let ui_refs = ui_refs.clone();
        pause_btn.connect_clicked(move |_| {
            let _ = search_request(&SearchRequest::Pause);
            ui_refs.paused.set_active(true);
            refresh_status_ui(&ui_refs);
        });
    }
    {
        let ui_refs = ui_refs.clone();
        resume_btn.connect_clicked(move |_| {
            let _ = search_request(&SearchRequest::Resume);
            ui_refs.paused.set_active(false);
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
    roots_pills: gtk::Box,
    chart: gtk::DrawingArea,
    chart_data: Rc<RefCell<Vec<(String, u64)>>>,
    status_hint: gtk::Label,
    paused: gtk::Switch,
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

    if s.crawling {
        ui.spinner.set_visible(true);
        ui.spinner.start();
        ui.state_pill.set_text(&tr("Indexing…"));
        ui.progress.set_visible(true);
        if s.crawl_total > 0 {
            let frac = (s.crawl_done as f64 / s.crawl_total as f64).clamp(0.0, 1.0);
            ui.progress.set_fraction(frac);
            ui.progress.set_text(Some(&format!(
                "{} / {}",
                format_count(s.crawl_done),
                format_count(s.crawl_total)
            )));
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
        if s.paused {
            ui.state_pill.set_text(&tr("Paused"));
        } else if s.enabled {
            ui.state_pill.set_text(&tr("Ready"));
        } else {
            ui.state_pill.set_text(&tr("Disabled"));
        }
    }

    while let Some(child) = ui.roots_pills.first_child() {
        ui.roots_pills.remove(&child);
    }
    if s.roots.is_empty() {
        let lab = gtk::Label::new(Some(&tr(
            "Per-folder counts appear after the next reindex.",
        )));
        lab.add_css_class("metis-settings-hint");
        lab.set_xalign(0.0);
        ui.roots_pills.append(&lab);
    } else {
        for root in &s.roots {
            let short = shorten_path(&root.path);
            let pill = make_pill(&format!("{short} · {}", format_count(root.docs)));
            ui.roots_pills.append(&pill);
        }
    }

    let cats: Vec<(String, u64)> = s
        .categories
        .iter()
        .map(|c| (c.category.clone(), c.docs))
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

fn shorten_path(path: &str) -> String {
    let home = default_home_path();
    if let Some(rest) = path.strip_prefix(&home) {
        if rest.is_empty() {
            return "~".into();
        }
        return format!("~{rest}");
    }
    path.to_string()
}

fn draw_category_chart(cr: &cairo::Context, width: i32, height: i32, data: &[(String, u64)]) {
    let w = width as f64;
    let h = height as f64;
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    let _ = cr.paint();

    if data.is_empty() {
        cr.set_source_rgba(0.7, 0.7, 0.7, 0.85);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(13.0);
        cr.move_to(12.0, h / 2.0);
        let _ = cr.show_text(&tr("Category breakdown appears after reindex"));
        return;
    }

    let max = data.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1) as f64;
    let rows = data.len().min(8) as f64;
    let row_h = ((h - 16.0) / rows).clamp(18.0, 28.0);
    let label_w = 110.0;
    let bar_x = label_w + 12.0;
    let bar_max = (w - bar_x - 56.0).max(40.0);

    // Accent-ish cyan bars (token-independent so the DrawingArea stays readable).
    for (i, (name, count)) in data.iter().take(8).enumerate() {
        let y = 10.0 + i as f64 * row_h;
        cr.set_source_rgba(0.85, 0.9, 0.95, 0.95);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(12.0);
        cr.move_to(8.0, y + row_h * 0.65);
        let _ = cr.show_text(name);

        let frac = (*count as f64 / max).clamp(0.05, 1.0);
        let bw = bar_max * frac;
        cr.set_source_rgba(0.0, 0.78, 0.95, 0.85);
        cr.rectangle(bar_x, y + 4.0, bw, row_h - 8.0);
        let _ = cr.fill();

        cr.set_source_rgba(0.85, 0.9, 0.95, 0.9);
        cr.move_to(bar_x + bw + 8.0, y + row_h * 0.65);
        let _ = cr.show_text(&format_count(*count));
    }
}
