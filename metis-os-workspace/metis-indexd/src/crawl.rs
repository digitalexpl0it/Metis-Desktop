//! Root walk + inotify-driven updates.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use metis_config::{SearchConfig, resolved_search_roots};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

use crate::index::SearchIndex;

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

pub fn crawl_roots(index: &SearchIndex, cfg: &SearchConfig) {
    if !cfg.enabled || cfg.paused {
        return;
    }
    let roots = resolved_search_roots(cfg);
    // Indeterminate until the walk finishes — a pre-count doubles crawl time on large trees.
    index.begin_crawl(0);

    for root in &roots {
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
                break;
            }
            let path = entry.path();
            if !entry.file_type().is_file() {
                continue;
            }
            if should_skip(path, &cfg.excludes) {
                continue;
            }
            if let Err(err) = index.index_file(path, &root_id) {
                tracing::debug!(?path, %err, "index_file failed");
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

pub async fn watch_loop(index: Arc<SearchIndex>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let cfg = index.config_snapshot();
    let roots = resolved_search_roots(&cfg);
    let excludes = cfg.excludes.clone();

    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let interesting = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                if !interesting {
                    return;
                }
                for path in event.paths {
                    let _ = tx.send(path);
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

    for root in &roots {
        if root.exists()
            && let Err(err) = watcher.watch(root, RecursiveMode::Recursive)
        {
            tracing::warn!(?root, %err, "watch failed");
        }
    }

    let mut batch: Vec<PathBuf> = Vec::new();
    let mut flush = tokio::time::interval(Duration::from_millis(750));
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(path) = rx.recv() => {
                batch.push(path);
                if batch.len() >= 128 {
                    flush_batch(&index, &excludes, &mut batch);
                }
            }
            _ = flush.tick() => {
                if !batch.is_empty() {
                    flush_batch(&index, &excludes, &mut batch);
                }
            }
        }
    }
}

fn flush_batch(index: &SearchIndex, excludes: &[String], batch: &mut Vec<PathBuf>) {
    if !index.enabled.load(Ordering::SeqCst) || index.paused.load(Ordering::SeqCst) {
        batch.clear();
        return;
    }
    batch.sort();
    batch.dedup();
    for path in batch.drain(..) {
        if should_skip(&path, excludes) {
            continue;
        }
        if path.is_file() {
            let root_id = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            if let Err(err) = index.index_file(&path, &root_id) {
                tracing::debug!(?path, %err, "watch index failed");
            }
        } else if !path.exists() {
            let _ = index.remove_path(&path.to_string_lossy());
        }
    }
    let _ = index.commit_now();
}
