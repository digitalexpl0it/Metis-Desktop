//! Metis desktop search indexer (`metis-indexd`).

mod categorize;
mod crawl;
mod extract;
mod index;
mod ipc;
mod pipeline;
mod schema;
mod tags;

use std::sync::Arc;

use metis_config::load_search_config;
use tracing_subscriber::EnvFilter;

use crate::crawl::{crawl_roots, watch_loop};
use crate::index::SearchIndex;
use crate::ipc::{ControlCmd, serve};
use crate::pipeline::spawn_content_workers;
use crate::schema::default_index_dir;
use crate::tags::{TagStore, default_tags_db_path};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cfg = load_search_config();
    let tags = match TagStore::open(&default_tags_db_path()) {
        Ok(t) => Arc::new(t),
        Err(err) => {
            tracing::error!(%err, "failed to open tag store");
            std::process::exit(1);
        }
    };
    let index_dir = default_index_dir();
    let index = match SearchIndex::open(&index_dir, Arc::clone(&tags), cfg.clone()) {
        Ok(i) => Arc::new(i),
        Err(err) => {
            tracing::error!(%err, ?index_dir, "failed to open Tantivy index");
            std::process::exit(1);
        }
    };
    index.refresh_doc_count();
    index.rebuild_facet_counts();

    let content_tx = spawn_content_workers(Arc::clone(&index));
    index.set_content_sender(content_tx);

    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel::<ControlCmd>();
    let _ = control_tx; // reserved for future internal triggers

    let crawl_index = Arc::clone(&index);
    let crawl_cfg = cfg.clone();
    tokio::task::spawn_blocking(move || {
        if crawl_cfg.enabled && !crawl_cfg.paused {
            tracing::info!("initial crawl starting");
            crawl_roots(&crawl_index, &crawl_cfg, false);
            tracing::info!(
                docs = crawl_index
                    .indexed_docs
                    .load(std::sync::atomic::Ordering::SeqCst),
                "initial crawl done"
            );
        }
    });

    let watch_index = Arc::clone(&index);
    tokio::spawn(async move {
        watch_loop(watch_index).await;
    });

    let serve_index = Arc::clone(&index);
    serve(serve_index, control_rx).await;
}
