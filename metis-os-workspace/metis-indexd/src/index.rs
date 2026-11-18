//! Tantivy open / index / query.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use metis_config::SearchConfig;
use metis_protocol::search::{
    SEARCH_PROTOCOL_VERSION, SearchCategoryStat, SearchHit, SearchRootStat, SearchStatus,
};
use tantivy::collector::TopDocs;
use tantivy::query::{
    BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, QueryParser, TermQuery,
};
use tantivy::schema::{Term, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, TantivyDocument, doc};

use crate::categorize::{
    auto_tags, categorize, extension_of, guess_mime, merge_tags, normalize_tag,
};
use crate::extract::{Extractor, build_extractors, extract_body};
use crate::schema::{IndexFields, build_schema};
use crate::tags::TagStore;

/// Marker written during the metadata phase until content extract finishes.
pub const CONTENT_PENDING_HASH: &str = "pending";

pub struct SearchIndex {
    pub index: Index,
    pub fields: IndexFields,
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    tags: Arc<TagStore>,
    extractors: Mutex<Vec<Box<dyn Extractor>>>,
    cfg: Mutex<SearchConfig>,
    pub indexed_docs: AtomicU64,
    pub paused: AtomicBool,
    pub enabled: AtomicBool,
    pub last_error: Mutex<Option<String>>,
    pending: Mutex<u32>,
    pub crawling: AtomicBool,
    pub crawl_done: AtomicU64,
    pub crawl_total: AtomicU64,
    pub with_content: AtomicU64,
    pub content_pending: AtomicU64,
    index_effort: AtomicU8,
    /// Bumped on every `reload_config` so the session watcher can resync roots.
    pub config_epoch: AtomicU64,
    content_tx: Mutex<Option<SyncSender<(PathBuf, String)>>>,
    root_counts: Mutex<HashMap<String, u64>>,
    category_counts: Mutex<HashMap<String, u64>>,
}

impl SearchIndex {
    pub fn open(dir: &Path, tags: Arc<TagStore>, cfg: SearchConfig) -> tantivy::Result<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| tantivy::TantivyError::SystemError(format!("create index dir: {e}")))?;
        let (schema, fields) = build_schema();
        let index = Index::open_or_create(tantivy::directory::MmapDirectory::open(dir)?, schema)?;
        let writer = index.writer(50_000_000)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let extractors = build_extractors(&cfg.extractors);
        let enabled = cfg.enabled;
        let paused = cfg.paused;
        let effort = cfg.index_effort.min(100);
        Ok(Self {
            index,
            fields,
            writer: Mutex::new(writer),
            reader,
            tags,
            extractors: Mutex::new(extractors),
            cfg: Mutex::new(cfg),
            indexed_docs: AtomicU64::new(0),
            paused: AtomicBool::new(paused),
            enabled: AtomicBool::new(enabled),
            last_error: Mutex::new(None),
            pending: Mutex::new(0),
            crawling: AtomicBool::new(false),
            crawl_done: AtomicU64::new(0),
            crawl_total: AtomicU64::new(0),
            with_content: AtomicU64::new(0),
            content_pending: AtomicU64::new(0),
            index_effort: AtomicU8::new(effort),
            config_epoch: AtomicU64::new(0),
            content_tx: Mutex::new(None),
            root_counts: Mutex::new(HashMap::new()),
            category_counts: Mutex::new(HashMap::new()),
        })
    }

    pub fn set_content_sender(&self, tx: SyncSender<(PathBuf, String)>) {
        if let Ok(mut g) = self.content_tx.lock() {
            *g = Some(tx);
        }
    }

    pub fn index_effort(&self) -> u8 {
        self.index_effort.load(Ordering::SeqCst)
    }

    pub fn enqueue_content(&self, path: PathBuf, root_id: String) {
        let tx = match self.content_tx.lock() {
            Ok(g) => g.clone(),
            Err(_) => None,
        };
        let Some(tx) = tx else {
            // No workers yet — extract inline.
            let _ = self.index_content(&path, &root_id);
            return;
        };
        self.content_pending.fetch_add(1, Ordering::SeqCst);
        if tx.send((path, root_id)).is_err() {
            self.content_pending.fetch_sub(1, Ordering::SeqCst);
            tracing::debug!("content queue closed");
        }
    }

    /// Put a job back on the queue without changing `content_pending` (pause path).
    pub fn requeue_content(&self, path: PathBuf, root_id: String) {
        let tx = match self.content_tx.lock() {
            Ok(g) => g.clone(),
            Err(_) => None,
        };
        if let Some(tx) = tx {
            let _ = tx.send((path, root_id));
        }
    }

    pub fn dec_content_pending(&self) {
        let prev = self
            .content_pending
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                Some(v.saturating_sub(1))
            });
        if matches!(prev, Ok(1)) {
            let _ = self.commit_now();
            self.rebuild_facet_counts();
        }
    }

    pub fn begin_crawl(&self, total_estimate: u64) {
        self.crawling.store(true, Ordering::SeqCst);
        self.crawl_done.store(0, Ordering::SeqCst);
        self.crawl_total.store(total_estimate, Ordering::SeqCst);
        if let Ok(mut g) = self.root_counts.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.category_counts.lock() {
            g.clear();
        }
        self.with_content.store(0, Ordering::SeqCst);
    }

    pub fn end_crawl(&self) {
        self.crawling.store(false, Ordering::SeqCst);
        self.crawl_total
            .store(self.crawl_done.load(Ordering::SeqCst), Ordering::SeqCst);
        self.refresh_doc_count();
        self.rebuild_facet_counts();
    }

    pub fn note_visited(&self) {
        self.crawl_done.fetch_add(1, Ordering::SeqCst);
    }

    pub fn note_indexed(&self, root_id: &str, category: &str, has_body: bool) {
        if has_body {
            self.with_content.fetch_add(1, Ordering::SeqCst);
        }
        if let Ok(mut g) = self.root_counts.lock() {
            *g.entry(root_id.to_string()).or_insert(0) += 1;
        }
        if let Ok(mut g) = self.category_counts.lock() {
            *g.entry(category.to_string()).or_insert(0) += 1;
        }
    }

    pub fn status_snapshot(&self) -> SearchStatus {
        let mut roots: Vec<SearchRootStat> = self
            .root_counts
            .lock()
            .map(|g| {
                g.iter()
                    .map(|(path, docs)| SearchRootStat {
                        path: path.clone(),
                        docs: *docs,
                    })
                    .collect()
            })
            .unwrap_or_default();
        roots.sort_by(|a, b| b.docs.cmp(&a.docs).then_with(|| a.path.cmp(&b.path)));

        let mut categories: Vec<SearchCategoryStat> = self
            .category_counts
            .lock()
            .map(|g| {
                g.iter()
                    .map(|(category, docs)| SearchCategoryStat {
                        category: category.clone(),
                        docs: *docs,
                    })
                    .collect()
            })
            .unwrap_or_default();
        categories.sort_by(|a, b| {
            b.docs
                .cmp(&a.docs)
                .then_with(|| a.category.cmp(&b.category))
        });

        let paused = self.paused.load(Ordering::SeqCst);
        let enabled = self.enabled.load(Ordering::SeqCst);
        // Pause/disable must win over a non-empty content queue so Settings
        // does not keep showing "Indexing…" while work is held.
        let crawling = enabled
            && !paused
            && (self.crawling.load(Ordering::SeqCst)
                || self.content_pending.load(Ordering::SeqCst) > 0);

        SearchStatus {
            version: SEARCH_PROTOCOL_VERSION,
            enabled,
            paused,
            indexed_docs: self.indexed_docs.load(Ordering::SeqCst),
            last_error: self.last_error.lock().ok().and_then(|g| g.clone()),
            crawling,
            crawl_done: self.crawl_done.load(Ordering::SeqCst),
            crawl_total: self.crawl_total.load(Ordering::SeqCst),
            with_content: self.with_content.load(Ordering::SeqCst),
            content_pending: self.content_pending.load(Ordering::SeqCst),
            roots,
            categories,
        }
    }

    pub fn reload_config(&self, cfg: SearchConfig) {
        self.enabled.store(cfg.enabled, Ordering::SeqCst);
        self.paused.store(cfg.paused, Ordering::SeqCst);
        self.index_effort
            .store(cfg.index_effort.min(100), Ordering::SeqCst);
        let extractors = build_extractors(&cfg.extractors);
        if let Ok(mut g) = self.extractors.lock() {
            *g = extractors;
        }
        if let Ok(mut g) = self.cfg.lock() {
            *g = cfg;
        }
        self.config_epoch.fetch_add(1, Ordering::SeqCst);
    }

    pub fn set_error(&self, msg: impl Into<String>) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = Some(msg.into());
        }
    }

    pub fn clear_error(&self) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = None;
        }
    }

    pub fn config_snapshot(&self) -> SearchConfig {
        self.cfg.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// True when stored size+mtime match and content phase already finished.
    pub fn is_unchanged(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        if !meta.is_file() {
            return false;
        }
        let mtime = meta_mtime_secs(&meta);
        let path_str = path.to_string_lossy();
        matches!(
            self.lookup_stored(path_str.as_ref()),
            Some((size, stored_mtime, hash))
                if size == meta.len()
                    && stored_mtime == mtime
                    && hash != CONTENT_PENDING_HASH
                    && !hash.is_empty()
        )
    }

    /// Metadata-only write (empty body, `pending` hash). Returns `false` if skipped.
    pub fn index_metadata(&self, path: &Path, root_id: &str) -> tantivy::Result<bool> {
        self.index_metadata_force(path, root_id, false)
    }

    /// Like [`index_metadata`], but `force` bypasses the size+mtime skip gate.
    pub fn index_metadata_force(
        &self,
        path: &Path,
        root_id: &str,
        force: bool,
    ) -> tantivy::Result<bool> {
        if !self.enabled.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if !force && self.is_unchanged(path) {
            return Ok(false);
        }
        self.write_document(path, root_id, None, true)?;
        Ok(true)
    }

    /// Content extract + rewrite (no size/mtime skip — caller already decided).
    pub fn index_content(&self, path: &Path, root_id: &str) -> tantivy::Result<()> {
        if !self.enabled.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        let cfg = self.config_snapshot();
        let body = {
            let extractors = self.extractors.lock().unwrap_or_else(|e| e.into_inner());
            extract_body(path, cfg.max_file_bytes, cfg.max_body_chars, &extractors)
                .unwrap_or_default()
        };
        self.write_document(path, root_id, Some(body), false)
    }

    /// Full index (metadata + extract). Skips when unchanged.
    #[cfg(test)]
    pub fn index_file(&self, path: &Path, root_id: &str) -> tantivy::Result<()> {
        if !self.enabled.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.is_unchanged(path) {
            return Ok(());
        }
        let cfg = self.config_snapshot();
        let body = {
            let extractors = self.extractors.lock().unwrap_or_else(|e| e.into_inner());
            extract_body(path, cfg.max_file_bytes, cfg.max_body_chars, &extractors)
                .unwrap_or_default()
        };
        self.write_document(path, root_id, Some(body), true)
    }

    /// Force rewrite for tag updates (never skip; keep prior body when present).
    pub fn reindex_tags_for_path(&self, path: &str) -> tantivy::Result<()> {
        let p = PathBuf::from(path);
        if !p.exists() {
            return Ok(());
        }
        let prior_body = self.lookup_body(path).unwrap_or_default();
        let body = if prior_body.is_empty() {
            let cfg = self.config_snapshot();
            let extractors = self.extractors.lock().unwrap_or_else(|e| e.into_inner());
            extract_body(&p, cfg.max_file_bytes, cfg.max_body_chars, &extractors)
                .unwrap_or_default()
        } else {
            prior_body
        };
        self.write_document(&p, "refresh", Some(body), true)?;
        self.commit_now()?;
        Ok(())
    }

    fn write_document(
        &self,
        path: &Path,
        root_id: &str,
        body: Option<String>,
        count_as_index: bool,
    ) -> tantivy::Result<()> {
        let meta = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(()),
        };
        let path_str = path.to_string_lossy().to_string();
        let basename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let ext = extension_of(path);
        let mime = guess_mime(path);
        let category = categorize(path, &mime);
        let mtime = meta_mtime_secs(&meta);
        let (dev, ino) = file_ids(path);
        let user = self.tags.user_tags_for(&path_str, dev, ino);
        let auto = auto_tags(path, category, mtime);
        let tags = merge_tags(&auto, &user);

        let (body, hash, has_body) = match body {
            Some(b) => {
                let has = !b.trim().is_empty();
                let hash = content_hash(&path_str, meta.len(), mtime, &b);
                (b, hash, has)
            }
            None => (String::new(), CONTENT_PENDING_HASH.to_string(), false),
        };

        let fields = &self.fields;
        let mut document = doc!(
            fields.path => path_str.as_str(),
            fields.basename => basename.as_str(),
            fields.extension => ext.as_str(),
            fields.mime => mime.as_str(),
            fields.size_u64 => meta.len(),
            fields.mtime_i64 => mtime,
            fields.dev => dev,
            fields.ino => ino,
            fields.content_hash => hash.as_str(),
            fields.body => body.as_str(),
            fields.lang => "",
            fields.category => category.as_str(),
            fields.root_id => root_id,
        );
        for tag in &tags {
            document.add_text(fields.tags, tag);
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| tantivy::TantivyError::SystemError("writer lock poisoned".into()))?;
        writer.delete_term(Term::from_field_text(fields.path, &path_str));
        writer.add_document(document)?;
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| tantivy::TantivyError::SystemError("pending lock poisoned".into()))?;
        *pending += 1;
        if *pending >= 64 {
            writer.commit()?;
            *pending = 0;
            self.reader.reload()?;
            self.refresh_doc_count();
        }
        drop(pending);
        drop(writer);
        if count_as_index && self.crawling.load(Ordering::SeqCst) {
            self.note_indexed(root_id, category.as_str(), has_body);
        }
        Ok(())
    }

    fn lookup_stored(&self, path: &str) -> Option<(u64, i64, String)> {
        let searcher = self.searcher().ok()?;
        let term = Term::from_field_text(self.fields.path, path);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;
        let (_score, addr) = top.into_iter().next()?;
        let retrieved: TantivyDocument = searcher.doc(addr).ok()?;
        let size = retrieved
            .get_first(self.fields.size_u64)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mtime = retrieved
            .get_first(self.fields.mtime_i64)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let hash = retrieved
            .get_first(self.fields.content_hash)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some((size, mtime, hash))
    }

    fn lookup_body(&self, path: &str) -> Option<String> {
        let searcher = self.searcher().ok()?;
        let term = Term::from_field_text(self.fields.path, path);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;
        let (_score, addr) = top.into_iter().next()?;
        let retrieved: TantivyDocument = searcher.doc(addr).ok()?;
        retrieved
            .get_first(self.fields.body)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    /// Rebuild root/category/with_content tallies from the live index.
    pub fn rebuild_facet_counts(&self) {
        let Ok(searcher) = self.searcher() else {
            return;
        };
        let mut roots: HashMap<String, u64> = HashMap::new();
        let mut categories: HashMap<String, u64> = HashMap::new();
        let mut with_content = 0u64;
        let fields = &self.fields;
        for (seg_ord, segment) in searcher.segment_readers().iter().enumerate() {
            let seg_ord = seg_ord as u32;
            for doc_id in 0..segment.max_doc() {
                if segment.is_deleted(doc_id) {
                    continue;
                }
                let addr = tantivy::DocAddress::new(seg_ord, doc_id);
                let Ok(retrieved): Result<TantivyDocument, _> = searcher.doc(addr) else {
                    continue;
                };
                if let Some(root) = retrieved
                    .get_first(fields.root_id)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty() && *s != "refresh")
                {
                    *roots.entry(root.to_string()).or_insert(0) += 1;
                }
                if let Some(cat) = retrieved
                    .get_first(fields.category)
                    .and_then(|v| v.as_str())
                {
                    *categories.entry(cat.to_string()).or_insert(0) += 1;
                }
                let body = retrieved
                    .get_first(fields.body)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !body.trim().is_empty() {
                    with_content += 1;
                }
            }
        }
        if let Ok(mut g) = self.root_counts.lock() {
            *g = roots;
        }
        if let Ok(mut g) = self.category_counts.lock() {
            *g = categories;
        }
        self.with_content.store(with_content, Ordering::SeqCst);
    }

    pub fn remove_path(&self, path: &str) -> tantivy::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| tantivy::TantivyError::SystemError("writer lock poisoned".into()))?;
        writer.delete_term(Term::from_field_text(self.fields.path, path));
        writer.commit()?;
        self.reader.reload()?;
        self.refresh_doc_count();
        Ok(())
    }

    /// Remove `path` and every indexed doc under `path/` (directory delete / move-out).
    pub fn remove_under_prefix(&self, path: &str) -> tantivy::Result<()> {
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return Ok(());
        }
        let prefix = format!("{path}/");
        let searcher = self.searcher()?;
        let mut doomed: Vec<String> = Vec::new();
        for (seg_ord, segment) in searcher.segment_readers().iter().enumerate() {
            let seg_ord = seg_ord as u32;
            for doc_id in 0..segment.max_doc() {
                if segment.is_deleted(doc_id) {
                    continue;
                }
                let addr = tantivy::DocAddress::new(seg_ord, doc_id);
                let Ok(retrieved): Result<TantivyDocument, _> = searcher.doc(addr) else {
                    continue;
                };
                let Some(p) = retrieved
                    .get_first(self.fields.path)
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                if p == path || p.starts_with(&prefix) {
                    doomed.push(p.to_string());
                }
            }
        }
        if doomed.is_empty() {
            return self.remove_path(path);
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| tantivy::TantivyError::SystemError("writer lock poisoned".into()))?;
        for p in &doomed {
            writer.delete_term(Term::from_field_text(self.fields.path, p));
        }
        writer.commit()?;
        self.reader.reload()?;
        self.refresh_doc_count();
        self.rebuild_facet_counts();
        Ok(())
    }

    pub fn commit_now(&self) -> tantivy::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| tantivy::TantivyError::SystemError("writer lock poisoned".into()))?;
        writer.commit()?;
        if let Ok(mut p) = self.pending.lock() {
            *p = 0;
        }
        self.reader.reload()?;
        self.refresh_doc_count();
        Ok(())
    }

    pub fn refresh_doc_count(&self) {
        if let Ok(searcher) = self.searcher() {
            self.indexed_docs
                .store(searcher.num_docs(), Ordering::SeqCst);
        }
    }

    fn searcher(&self) -> tantivy::Result<Searcher> {
        Ok(self.reader.searcher())
    }

    pub fn query(
        &self,
        q: &str,
        category: Option<&str>,
        tags: &[String],
        limit: u32,
    ) -> tantivy::Result<Vec<SearchHit>> {
        let searcher = self.searcher()?;
        let limit = limit.clamp(1, 200) as usize;
        let fields = &self.fields;

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let q = q.trim();
        if !q.is_empty() {
            // Basename/path prefix + tokenized body — not pure fuzzy, but "idea"
            // must hit idea_notes.txt (underscores / partial names).
            clauses.push((Occur::Must, build_text_query(&self.index, fields, q)));
        }
        if let Some(cat) = category.filter(|c| !c.is_empty()) {
            // Documents also matches legacy `code` docs from older index writes.
            let cats: &[&str] = if cat == "documents" {
                &["documents", "code"]
            } else {
                &[cat]
            };
            let mut cat_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
            for c in cats {
                let term = Term::from_field_text(fields.category, c);
                cat_clauses.push((
                    Occur::Should,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ));
            }
            clauses.push((Occur::Must, Box::new(BooleanQuery::new(cat_clauses))));
        }
        for tag in tags {
            let t = normalize_tag(tag);
            if t.is_empty() {
                continue;
            }
            let term = Term::from_field_text(fields.tags, &t);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    term,
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ));
        }

        if clauses.is_empty() {
            return Ok(Vec::new());
        }

        let query = BooleanQuery::new(clauses);
        let top = searcher.search(&query, &TopDocs::with_limit(limit))?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let retrieved: TantivyDocument = searcher.doc(addr)?;
            let path = retrieved
                .get_first(fields.path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let basename = retrieved
                .get_first(fields.basename)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let category = retrieved
                .get_first(fields.category)
                .and_then(|v| v.as_str())
                .unwrap_or("other")
                .to_string();
            let mime = retrieved
                .get_first(fields.mime)
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let size_bytes = retrieved
                .get_first(fields.size_u64)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let mut tag_list: Vec<String> = retrieved
                .get_all(fields.tags)
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            tag_list.sort();
            tag_list.dedup();
            let snippet = retrieved
                .get_first(fields.body)
                .and_then(|v| v.as_str())
                .map(|body| make_snippet(body, q));
            let mode = file_mode(&path);
            hits.push(SearchHit {
                path,
                basename,
                category,
                tags: tag_list,
                snippet,
                mime,
                size_bytes,
                mode,
                score,
            });
        }
        Ok(hits)
    }

    pub fn tags(&self) -> Arc<TagStore> {
        Arc::clone(&self.tags)
    }
}

/// Cheap name-first query: tokenized basename + prefix terms; body only when
/// the query is long enough. No path-wide regex (too expensive on large trees).
fn build_text_query(index: &Index, fields: &IndexFields, q: &str) -> Box<dyn Query> {
    let mut should: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let q_chars = q.chars().count();

    let name_parser = QueryParser::for_index(index, vec![fields.basename]);
    if let Ok(parsed) = name_parser.parse_query(q) {
        should.push((Occur::Should, Box::new(BoostQuery::new(parsed, 4.0))));
    }
    if q_chars >= 3 {
        let body_parser = QueryParser::for_index(index, vec![fields.body]);
        if let Ok(parsed) = body_parser.parse_query(q) {
            should.push((Occur::Should, Box::new(BoostQuery::new(parsed, 0.6))));
        }
    }

    let q_lower = q.to_ascii_lowercase();
    // Prefix each alphanumeric run on basename tokens (Tantivy 0.22 has no PrefixQuery).
    for part in query_parts(&q_lower) {
        if part.len() < 2 {
            continue;
        }
        let term = Term::from_field_text(fields.basename, &part);
        should.push((
            Occur::Should,
            Box::new(BoostQuery::new(
                Box::new(FuzzyTermQuery::new_prefix(term, 0, true)),
                3.5,
            )),
        ));
    }

    if should.is_empty() {
        let term = Term::from_field_text(fields.basename, "\0");
        return Box::new(TermQuery::new(
            term,
            tantivy::schema::IndexRecordOption::Basic,
        ));
    }
    Box::new(BooleanQuery::new(should))
}

fn query_parts(q: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    for c in q.chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let compact: String = q.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if compact.len() >= 2 && !parts.iter().any(|p| p == &compact) {
        parts.push(compact);
    }
    parts
}

fn make_snippet(body: &str, q: &str) -> String {
    let needle = q
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|t| t.len() >= 2)
        .unwrap_or(q);
    if needle.is_empty() {
        return body.chars().take(240).collect();
    }
    let lower: String = body.to_ascii_lowercase();
    let nlow = needle.to_ascii_lowercase();
    let chars: Vec<char> = body.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let nlow_chars: Vec<char> = nlow.chars().collect();
    let pos = lower_chars
        .windows(nlow_chars.len())
        .position(|w| w == nlow_chars.as_slice());
    if let Some(pos) = pos {
        let start = pos.saturating_sub(40);
        let end = (pos + nlow_chars.len() + 120).min(chars.len());
        let snippet: String = chars[start..end].iter().collect();
        return if start > 0 {
            format!("…{snippet}")
        } else {
            snippet
        };
    }
    chars.into_iter().take(240).collect()
}

fn content_hash(path: &str, size: u64, mtime: i64, body: &str) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    size.hash(&mut h);
    mtime.hash(&mut h);
    body.len().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn meta_mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn file_ids(path: &Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(m) => (m.dev(), m.ino()),
        Err(_) => (0, 0),
    }
}

fn file_mode(path: &str) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::TagStore;
    use metis_config::SearchConfig;
    use std::io::Write;
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn index_and_query_roundtrip() {
        let dir = tempdir().unwrap();
        let tags = Arc::new(TagStore::open(&dir.path().join("tags.sqlite")).unwrap());
        let idx = SearchIndex::open(
            &dir.path().join("idx"),
            tags.clone(),
            SearchConfig::default(),
        )
        .unwrap();
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "unique_metis_search_token alpha beta").unwrap();
        let path = dir.path().join("sample.rs");
        std::fs::copy(f.path(), &path).unwrap();
        tags.add_tag(path.to_str().unwrap(), 0, 0, "unit").unwrap();
        idx.index_file(&path, "test").unwrap();
        idx.commit_now().unwrap();
        let hits = idx
            .query("unique_metis_search_token", None, &[], 10)
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].path.ends_with("sample.rs"));
        let tagged = idx.query("", None, &["unit".into()], 10).unwrap();
        assert!(!tagged.is_empty());
    }

    #[test]
    fn basename_prefix_finds_idea_notes() {
        let dir = tempdir().unwrap();
        let tags = Arc::new(TagStore::open(&dir.path().join("tags.sqlite")).unwrap());
        let idx =
            SearchIndex::open(&dir.path().join("idx"), tags, SearchConfig::default()).unwrap();
        let path = dir.path().join("idea_notes.txt");
        std::fs::write(&path, "some brainstorming about widgets\n").unwrap();
        idx.index_file(&path, "test").unwrap();
        idx.commit_now().unwrap();

        for q in ["idea", "idea_n", "idea_notes", "notes"] {
            let hits = idx.query(q, None, &[], 10).unwrap();
            assert!(
                hits.iter().any(|h| h.basename == "idea_notes.txt"),
                "query {q:?} should find idea_notes.txt, got {:?}",
                hits.iter().map(|h| &h.basename).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn skip_unchanged_after_index() {
        let dir = tempdir().unwrap();
        let tags = Arc::new(TagStore::open(&dir.path().join("tags.sqlite")).unwrap());
        let idx =
            SearchIndex::open(&dir.path().join("idx"), tags, SearchConfig::default()).unwrap();
        let path = dir.path().join("stable.txt");
        std::fs::write(&path, "hello world\n").unwrap();
        idx.index_file(&path, "test").unwrap();
        idx.commit_now().unwrap();
        assert!(idx.is_unchanged(&path));
        assert!(!idx.index_metadata(&path, "test").unwrap());

        // Touch mtime by rewriting.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "hello world changed\n").unwrap();
        assert!(!idx.is_unchanged(&path));
    }
}
