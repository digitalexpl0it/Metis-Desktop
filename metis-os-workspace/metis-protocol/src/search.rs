//! Desktop search IPC (`metis-indexd` ↔ shell / Settings).
//!
//! Length-prefixed JSON frames over `$XDG_RUNTIME_DIR/metis/indexd.sock`.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::{ensure_runtime_dir, runtime_dir};

pub const SEARCH_PROTOCOL_VERSION: u32 = 1;

pub fn indexd_socket_path() -> PathBuf {
    runtime_dir().join("indexd.sock")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchCategory {
    Documents,
    Images,
    Video,
    Audio,
    Archives,
    Code,
    Downloads,
    Desktop,
    Other,
}

impl SearchCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::Images => "images",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Archives => "archives",
            Self::Code => "code",
            Self::Downloads => "downloads",
            Self::Desktop => "desktop",
            Self::Other => "other",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Images => "Images",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Archives => "Archives",
            // Legacy wire values — display under Documents / Other.
            Self::Code => "Documents",
            Self::Downloads | Self::Desktop | Self::Other => "Other",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "documents" | "code" => Self::Documents,
            "images" => Self::Images,
            "video" => Self::Video,
            "audio" => Self::Audio,
            "archives" => Self::Archives,
            // Folder-based legacy categories collapse to Other for grouping.
            "downloads" | "desktop" => Self::Other,
            _ => Self::Other,
        }
    }

    /// Categories shown as filter chips (All is separate in the UI).
    pub fn filter_chips() -> &'static [Self] {
        &[
            Self::Documents,
            Self::Images,
            Self::Video,
            Self::Audio,
            Self::Archives,
        ]
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Documents,
            Self::Images,
            Self::Video,
            Self::Audio,
            Self::Archives,
            Self::Code,
            Self::Downloads,
            Self::Desktop,
            Self::Other,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub basename: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Body snippet for preview pane only (not shown in the result list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default)]
    pub size_bytes: u64,
    /// Unix mode bits (e.g. 0o100644), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default)]
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRootStat {
    pub path: String,
    pub docs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchCategoryStat {
    pub category: String,
    pub docs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchStatus {
    pub version: u32,
    pub enabled: bool,
    pub paused: bool,
    pub indexed_docs: u64,
    pub last_error: Option<String>,
    /// True while an initial crawl / reindex is in progress.
    #[serde(default)]
    pub crawling: bool,
    /// Files processed in the current crawl (0 when idle).
    #[serde(default)]
    pub crawl_done: u64,
    /// Estimated files to process (0 = unknown / indeterminate).
    #[serde(default)]
    pub crawl_total: u64,
    /// Documents that have extracted text body.
    #[serde(default)]
    pub with_content: u64,
    /// Per indexed root path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<SearchRootStat>,
    /// Per category facet counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<SearchCategoryStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchRequest {
    Query {
        q: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    TagAdd {
        path: String,
        tag: String,
    },
    TagRemove {
        path: String,
        tag: String,
    },
    TagsList {
        path: String,
    },
    SuggestTags {
        #[serde(default)]
        prefix: String,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Status,
    /// Re-read `search.json` and rebuild extractors (no full crawl).
    ReloadConfig,
    Pause,
    Resume,
    Reindex,
}

fn default_limit() -> u32 {
    50
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchResponse {
    Hits { hits: Vec<SearchHit> },
    Tags { tags: Vec<String> },
    Status(SearchStatus),
    Ok,
    Error { message: String },
}

fn write_frame(stream: &mut UnixStream, value: &impl Serialize) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"))?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

fn read_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// One-shot request to `metis-indexd`. Soft-fail with `Error` if the socket is down.
pub fn search_request(req: &SearchRequest) -> SearchResponse {
    match search_request_inner(req) {
        Ok(resp) => resp,
        Err(err) => SearchResponse::Error {
            message: err.to_string(),
        },
    }
}

fn search_request_inner(req: &SearchRequest) -> std::io::Result<SearchResponse> {
    let _ = ensure_runtime_dir()?;
    let path = indexd_socket_path();
    let mut stream = UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_frame(&mut stream, req)?;
    let raw = read_frame(&mut stream)?;
    serde_json::from_slice(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Server-side helpers used by `metis-indexd`.
pub mod server {
    use super::*;

    pub fn write_response(stream: &mut UnixStream, resp: &SearchResponse) -> std::io::Result<()> {
        write_frame(stream, resp)
    }

    pub fn read_request(stream: &mut UnixStream) -> std::io::Result<SearchRequest> {
        let raw = read_frame(stream)?;
        serde_json::from_slice(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_roundtrip() {
        for c in SearchCategory::filter_chips() {
            assert_eq!(SearchCategory::from_str_lossy(c.as_str()), *c);
        }
        assert_eq!(
            SearchCategory::from_str_lossy("code"),
            SearchCategory::Documents
        );
        assert_eq!(
            SearchCategory::from_str_lossy("downloads"),
            SearchCategory::Other
        );
    }

    #[test]
    fn request_json_smoke() {
        let req = SearchRequest::Query {
            q: "hello".into(),
            category: Some("code".into()),
            tags: vec!["work".into()],
            limit: 10,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["type"], "query");
        let back: SearchRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, req);
    }
}
