//! Root walk + inotify-driven session updates (incremental indexing).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use metis_config::{SearchConfig, resolved_search_roots};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

use crate::index::SearchIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FsChange {
    /// Create or content/metadata modify.
    Upsert,
    /// Create of a directory (walk existing children once).
    DirCreate,
    /// Removed or moved out of a watched root.
    Remove,
}

pub fn should_skip(path: &Path, excludes: &[String]) -> bool {
    let s = path.to_string_lossy();
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
    {
        return true;
    }
    for ex in excludes {
        if s.contains(ex) {
            return true;
        }
    }
    for ancestor in path.ancestors() {
        if ancestor.join(".nomedia").exists() {
            return true;
        }
    }
    false
}

/// Map a path to the configured search root that contains it (longest match).
pub fn resolve_root_id(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            path.parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string())
        })
}

/// Walk configured roots. When `force` is false (login crawl), unchanged
/// size+mtime docs are skipped. When `force` is true (`Reindex`), every file
/// is re-queued so extractor toggles can refresh bodies.
pub fn crawl_roots(index: &SearchIndex, cfg: &SearchConfig, force: bool) {
    if !cfg.enabled || cfg.paused {
        return;
    }
    let roots = resolved_search_roots(cfg);
    // Indeterminate until the walk finishes — a pre-count doubles crawl time on large trees.
    index.begin_crawl(0);

    'roots: for root in &roots {
        if !root.exists() {
            continue;
        }
        let root_id = root.to_string_lossy().to_string();
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !index.enabled.load(Ordering::SeqCst) || index.paused.load(Ordering::SeqCst) {
                break 'roots;
            }
            let path = entry.path();
            if !entry.file_type().is_file() {
                continue;
            }
            if should_skip(path, &cfg.excludes) {
                continue;
            }
            index.note_visited();
            if !force && index.is_unchanged(path) {
                continue;
            }
            match index.index_metadata_force(path, &root_id, force) {
                Ok(true) => {
                    index.enqueue_content(path.to_path_buf(), root_id.clone());
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::debug!(?path, %err, "index_metadata failed");
                }
            }
        }
    }
    if let Err(err) = index.commit_now() {
        index.set_error(format!("commit: {err}"));
    } else {
        index.clear_error();
    }
    index.end_crawl();
}

/// Session-long inotify watch: create / modify / delete → incremental index updates.
pub async fn watch_loop(index: Arc<SearchIndex>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(PathBuf, FsChange)>();

    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let change = match event.kind {
                    EventKind::Create(notify::event::CreateKind::Folder) => FsChange::DirCreate,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any => {
                        FsChange::Upsert
                    }
                    EventKind::Remove(_) => FsChange::Remove,
                    _ => return,
                };
                for path in event.paths {
                    let _ = tx.send((path, change));
                }
            }
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(err) => {
            tracing::warn!(%err, "inotify watcher unavailable");
            return;
        }
    };

    let mut watched: HashSet<PathBuf> = HashSet::new();
    let mut last_epoch = u64::MAX; // force initial sync
    sync_watches(&index, &mut watcher, &mut watched, &mut last_epoch);

    let mut batch: Vec<(PathBuf, FsChange)> = Vec::new();
    let mut flush = tokio::time::interval(Duration::from_millis(750));
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!(roots = watched.len(), "session file watcher started");

    loop {
        tokio::select! {
            Some(evt) = rx.recv() => {
                batch.push(evt);
                if batch.len() >= 128 {
                    flush_batch(&index, &mut batch);
                }
            }
            _ = flush.tick() => {
                sync_watches(&index, &mut watcher, &mut watched, &mut last_epoch);
                if !batch.is_empty() {
                    flush_batch(&index, &mut batch);
                }
            }
        }
    }
}

fn sync_watches(
    index: &SearchIndex,
    watcher: &mut RecommendedWatcher,
    watched: &mut HashSet<PathBuf>,
    last_epoch: &mut u64,
) {
    let epoch = index.config_epoch.load(Ordering::SeqCst);
    if epoch == *last_epoch {
        return;
    }
    *last_epoch = epoch;
    let cfg = index.config_snapshot();
    let desired: HashSet<PathBuf> = resolved_search_roots(&cfg).into_iter().collect();

    for old in watched.difference(&desired).cloned().collect::<Vec<_>>() {
        if let Err(err) = watcher.unwatch(&old) {
            tracing::debug!(?old, %err, "unwatch failed");
        }
        watched.remove(&old);
    }
    for root in desired.difference(watched).cloned().collect::<Vec<_>>() {
        if root.exists() {
            match watcher.watch(&root, RecursiveMode::Recursive) {
                Ok(()) => {
                    watched.insert(root);
                }
                Err(err) => {
                    tracing::warn!(?root, %err, "watch failed");
                }
            }
        }
    }
}

fn flush_batch(index: &SearchIndex, batch: &mut Vec<(PathBuf, FsChange)>) {
    if !index.enabled.load(Ordering::SeqCst) {
        batch.clear();
        return;
    }
    // Pause: keep events so they apply when resumed (session-incremental).
    if index.paused.load(Ordering::SeqCst) {
        return;
    }

    let cfg = index.config_snapshot();
    let roots = resolved_search_roots(&cfg);
    let excludes = &cfg.excludes;

    // Prefer Remove over Upsert for the same path in one batch.
    batch.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            use std::cmp::Ordering as Cmp;
            match (a.1, b.1) {
                (FsChange::Remove, FsChange::Remove) => Cmp::Equal,
                (FsChange::Remove, _) => Cmp::Greater,
                (_, FsChange::Remove) => Cmp::Less,
                _ => Cmp::Equal,
            }
        })
    });
    batch.dedup_by(|a, b| a.0 == b.0);

    for (path, change) in batch.drain(..) {
        if should_skip(&path, excludes) && change != FsChange::Remove {
            continue;
        }
        let in_root = roots.iter().any(|r| path.starts_with(r));
        match change {
            FsChange::Remove => {
                let key = path.to_string_lossy();
                if let Err(err) = index.remove_under_prefix(key.as_ref()) {
                    tracing::debug!(?path, %err, "watch remove failed");
                }
            }
            FsChange::DirCreate => {
                if !in_root || !path.is_dir() {
                    continue;
                }
                index_dir_tree(index, &path, &roots, excludes);
            }
            FsChange::Upsert => {
                if !in_root {
                    continue;
                }
                if path.is_file() {
                    if index.is_unchanged(&path) {
                        continue;
                    }
                    let root_id = resolve_root_id(&path, &roots);
                    match index.index_metadata(&path, &root_id) {
                        Ok(true) => index.enqueue_content(path, root_id),
                        Ok(false) => {}
                        Err(err) => {
                            tracing::debug!(?path, %err, "watch metadata index failed");
                        }
                    }
                }
                // Directory modify: ignore — child file events carry the work.
            }
        }
    }
    let _ = index.commit_now();
}

fn index_dir_tree(index: &SearchIndex, dir: &Path, roots: &[PathBuf], excludes: &[String]) {
    let root_id = resolve_root_id(dir, roots);
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !index.enabled.load(Ordering::SeqCst) || index.paused.load(Ordering::SeqCst) {
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if should_skip(path, excludes) {
            continue;
        }
        if index.is_unchanged(path) {
            continue;
        }
        match index.index_metadata(path, &root_id) {
            Ok(true) => index.enqueue_content(path.to_path_buf(), root_id.clone()),
            Ok(false) => {}
            Err(err) => {
                tracing::debug!(?path, %err, "watch dir index failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_root_picks_longest_prefix() {
        let roots = vec![PathBuf::from("/home/me"), PathBuf::from("/home/me/docs")];
        assert_eq!(
            resolve_root_id(Path::new("/home/me/docs/a.txt"), &roots),
            "/home/me/docs"
        );
        assert_eq!(
            resolve_root_id(Path::new("/home/me/x.txt"), &roots),
            "/home/me"
        );
    }
}
