//! Content-extract worker pool driven by `search.json` `index_effort`.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use metis_config::{index_effort_delay_ms, index_effort_workers};

use crate::index::SearchIndex;

const MAX_CONTENT_WORKERS: usize = 4;
const CONTENT_QUEUE_CAP: usize = 8192;

/// Spawn content workers and return a sender used by crawl/watch.
pub fn spawn_content_workers(index: Arc<SearchIndex>) -> SyncSender<(PathBuf, String)> {
    let (tx, rx) = sync_channel::<(PathBuf, String)>(CONTENT_QUEUE_CAP);
    let rx = Arc::new(Mutex::new(rx));
    for id in 0..MAX_CONTENT_WORKERS {
        let index = Arc::clone(&index);
        let rx = Arc::clone(&rx);
        let _ = std::thread::Builder::new()
            .name(format!("metis-indexd-content-{id}"))
            .spawn(move || content_worker(id, index, rx));
    }
    tx
}

fn content_worker(id: usize, index: Arc<SearchIndex>, rx: Arc<Mutex<Receiver<(PathBuf, String)>>>) {
    loop {
        let effort = index.index_effort();
        let n = index_effort_workers(effort);
        if id >= n {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }

        let item = {
            let Ok(guard) = rx.lock() else {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            guard.recv_timeout(Duration::from_millis(200))
        };

        match item {
            Ok((path, root_id)) => {
                if !index.enabled.load(Ordering::SeqCst) {
                    // Drop queued work when search is disabled.
                    index.dec_content_pending();
                    continue;
                }
                if index.paused.load(Ordering::SeqCst) {
                    index.requeue_content(path, root_id);
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                if let Err(err) = index.index_content(&path, &root_id) {
                    tracing::debug!(?path, %err, "content index failed");
                }
                index.dec_content_pending();
                let delay = index_effort_delay_ms(index.index_effort());
                if delay > 0 {
                    std::thread::sleep(Duration::from_millis(delay));
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
