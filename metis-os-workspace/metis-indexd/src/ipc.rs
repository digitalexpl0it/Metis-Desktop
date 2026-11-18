//! Unix-socket IPC server for `metis-protocol::search`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use metis_config::{load_search_config, save_search_config};
use metis_protocol::search::{SearchRequest, SearchResponse, indexd_socket_path, server};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::crawl::crawl_roots;
use crate::index::SearchIndex;

pub enum ControlCmd {
    /// Reserved for future internal reindex triggers.
    #[allow(dead_code)]
    Reindex,
}

pub async fn serve(index: Arc<SearchIndex>, mut control_rx: mpsc::UnboundedReceiver<ControlCmd>) {
    let path = indexd_socket_path();
    if let Err(err) = prepare_socket(&path) {
        tracing::error!(%err, "failed to bind indexd socket");
        return;
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(err) => {
            tracing::error!(%err, ?path, "bind indexd.sock");
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    tracing::info!(?path, "metis-indexd listening");

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let index = Arc::clone(&index);
                        tokio::spawn(async move {
                            if let Err(err) = handle_client(stream, index).await {
                                tracing::debug!(%err, "indexd client error");
                            }
                        });
                    }
                    Err(err) => tracing::warn!(%err, "accept failed"),
                }
            }
            Some(cmd) = control_rx.recv() => {
                match cmd {
                    ControlCmd::Reindex => {
                        let index = Arc::clone(&index);
                        tokio::task::spawn_blocking(move || {
                            let cfg = load_search_config();
                            index.reload_config(cfg.clone());
                            crawl_roots(&index, &cfg, true);
                        });
                    }
                }
            }
        }
    }
}

fn prepare_socket(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

async fn handle_client(stream: UnixStream, index: Arc<SearchIndex>) -> std::io::Result<()> {
    let std_stream = stream.into_std()?;
    std_stream.set_nonblocking(false)?;
    let mut std_stream = std_stream;
    let req = server::read_request(&mut std_stream)?;
    let resp = dispatch(index, req);
    server::write_response(&mut std_stream, &resp)?;
    Ok(())
}

fn dispatch(index: Arc<SearchIndex>, req: SearchRequest) -> SearchResponse {
    match req {
        SearchRequest::Query {
            q,
            category,
            tags,
            limit,
        } => match index.query(&q, category.as_deref(), &tags, limit) {
            Ok(hits) => SearchResponse::Hits { hits },
            Err(err) => SearchResponse::Error {
                message: err.to_string(),
            },
        },
        SearchRequest::TagAdd { path, tag } => {
            let (dev, ino) = file_ids_str(&path);
            if let Err(err) = index.tags().add_tag(&path, dev, ino, &tag) {
                return SearchResponse::Error {
                    message: err.to_string(),
                };
            }
            let _ = index.reindex_tags_for_path(&path);
            SearchResponse::Ok
        }
        SearchRequest::TagRemove { path, tag } => {
            if let Err(err) = index.tags().remove_tag(&path, &tag) {
                return SearchResponse::Error {
                    message: err.to_string(),
                };
            }
            let _ = index.reindex_tags_for_path(&path);
            SearchResponse::Ok
        }
        SearchRequest::TagsList { path } => {
            let (dev, ino) = file_ids_str(&path);
            let tags = index.tags().user_tags_for(&path, dev, ino);
            SearchResponse::Tags { tags }
        }
        SearchRequest::SuggestTags { prefix, limit } => {
            match index.tags().suggest(&prefix, limit) {
                Ok(tags) => SearchResponse::Tags { tags },
                Err(err) => SearchResponse::Error {
                    message: err.to_string(),
                },
            }
        }
        SearchRequest::Status => SearchResponse::Status(index.status_snapshot()),
        SearchRequest::ReloadConfig => {
            let cfg = load_search_config();
            index.reload_config(cfg);
            SearchResponse::Ok
        }
        SearchRequest::Pause => {
            index.paused.store(true, Ordering::SeqCst);
            let mut cfg = load_search_config();
            cfg.paused = true;
            let _ = save_search_config(&cfg);
            index.reload_config(cfg);
            SearchResponse::Ok
        }
        SearchRequest::Resume => {
            index.paused.store(false, Ordering::SeqCst);
            let mut cfg = load_search_config();
            cfg.paused = false;
            let _ = save_search_config(&cfg);
            index.reload_config(cfg);
            SearchResponse::Ok
        }
        SearchRequest::Reindex => {
            if index.crawling.load(Ordering::SeqCst) {
                return SearchResponse::Ok;
            }
            let idx = Arc::clone(&index);
            let _ = std::thread::Builder::new()
                .name("metis-indexd-reindex".into())
                .spawn(move || {
                    let cfg = load_search_config();
                    idx.reload_config(cfg.clone());
                    crawl_roots(&idx, &cfg, true);
                });
            SearchResponse::Ok
        }
    }
}

fn file_ids_str(path: &str) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(m) => (m.dev(), m.ino()),
        Err(_) => (0, 0),
    }
}
