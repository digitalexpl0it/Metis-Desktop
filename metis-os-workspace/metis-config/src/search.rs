//! Desktop search preferences (`~/.config/metis/search.json`).
//!
//! Controls `metis-indexd` roots, excludes, pause, and extraction caps.
//! Index data lives under `$XDG_CACHE_HOME/metis/index/`; user tags in
//! `$XDG_DATA_HOME/metis/search-tags.sqlite`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchExtractors {
    /// UTF-8 / text / common source code.
    #[serde(default = "default_true")]
    pub plain_text: bool,
    /// PDF body via optional `pdftotext` (poppler-utils); soft-fails if missing.
    #[serde(default)]
    pub pdf: bool,
    /// HTML / HTM with tags/scripts/styles stripped to visible text.
    #[serde(default)]
    pub html: bool,
}

impl Default for SearchExtractors {
    fn default() -> Self {
        Self {
            plain_text: true,
            pdf: false,
            html: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchConfig {
    /// When false, the daemon stays idle (IPC still answers Status).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Pause crawl/extract without disabling the socket.
    #[serde(default)]
    pub paused: bool,
    /// Absolute roots to index. Empty → default `$HOME` only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Path prefixes or glob-ish path substrings to skip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excludes: Vec<String>,
    /// Skip files larger than this many bytes (content extract).
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// Max characters stored in the Tantivy body field per file.
    #[serde(default = "default_max_body_chars")]
    pub max_body_chars: usize,
    /// Content-extract effort (0 = low CPU, 100 = max workers / no delay).
    #[serde(default = "default_index_effort")]
    pub index_effort: u8,
    #[serde(default)]
    pub extractors: SearchExtractors,
}

fn default_true() -> bool {
    true
}

fn default_max_file_bytes() -> u64 {
    2 * 1024 * 1024
}

fn default_max_body_chars() -> usize {
    64 * 1024
}

fn default_index_effort() -> u8 {
    35
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            paused: false,
            roots: Vec::new(),
            excludes: default_excludes(),
            max_file_bytes: default_max_file_bytes(),
            max_body_chars: default_max_body_chars(),
            index_effort: default_index_effort(),
            extractors: SearchExtractors::default(),
        }
    }
}

/// Content worker count from `index_effort` (1–4, also capped by CPU).
pub fn index_effort_workers(effort: u8) -> usize {
    let capped = match effort.min(100) {
        0..=25 => 1,
        26..=60 => 2,
        61..=85 => 3,
        _ => 4,
    };
    let cpu_cap = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
        .min(4);
    capped.min(cpu_cap).max(1)
}

/// Inter-file delay after each content extract (ms).
pub fn index_effort_delay_ms(effort: u8) -> u64 {
    let e = u64::from(effort.min(100));
    // 0 → 40 ms, 35 → ~26 ms, 100 → 0
    (40u64.saturating_mul(100 - e)) / 100
}

fn default_excludes() -> Vec<String> {
    vec![
        "/.git/".into(),
        "/node_modules/".into(),
        "/target/".into(),
        "/.cache/".into(),
        "/Cache/".into(),
        "/.local/share/Trash/".into(),
        // Toolchain / package caches — huge and churny while developing.
        "/.cargo/registry/".into(),
        "/.cargo/git/".into(),
        "/.rustup/".into(),
        "/__pycache__/".into(),
        "/.venv/".into(),
        "/venv/".into(),
        "/.npm/".into(),
        "/.pnpm-store/".into(),
        "/.gradle/".into(),
        "/CMakeFiles/".into(),
        "/.next/".into(),
        "/.turbo/".into(),
        "/zig-cache/".into(),
        "/.ccache/".into(),
    ]
}

pub fn sanitize_search_config(cfg: &mut SearchConfig) {
    cfg.max_file_bytes = cfg.max_file_bytes.clamp(4 * 1024, 64 * 1024 * 1024);
    cfg.max_body_chars = cfg.max_body_chars.clamp(1024, 512 * 1024);
    cfg.index_effort = cfg.index_effort.min(100);
    cfg.roots.retain(|r| !r.trim().is_empty());
    cfg.excludes.retain(|e| !e.trim().is_empty());
    for root in &mut cfg.roots {
        *root = root.trim().to_string();
    }
    // Merge built-in excludes in-memory (no rewrite) so older search.json files
    // pick up new toolchain skips without a migration rewrite loop.
    for ex in default_excludes() {
        if !cfg.excludes.iter().any(|e| e == &ex) {
            cfg.excludes.push(ex);
        }
    }
}

/// Resolved absolute roots (expands empty → `$HOME`).
pub fn resolved_search_roots(cfg: &SearchConfig) -> Vec<std::path::PathBuf> {
    if cfg.roots.is_empty() {
        if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return vec![home];
        }
        return Vec::new();
    }
    cfg.roots.iter().map(std::path::PathBuf::from).collect()
}

pub fn search_config_path() -> std::path::PathBuf {
    super::config_dir().join("search.json")
}

pub fn load_search_config() -> SearchConfig {
    let path = search_config_path();
    let mut cfg = if path.exists() {
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<SearchConfig>(&t).ok())
        {
            Some(parsed) => parsed,
            None => {
                tracing::warn!("search.json parse failed — using defaults");
                SearchConfig::default()
            }
        }
    } else {
        SearchConfig::default()
    };
    sanitize_search_config(&mut cfg);
    cfg
}

pub fn save_search_config(config: &SearchConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let mut clean = config.clone();
    sanitize_search_config(&mut clean);
    crate::persist::write_json_atomic(&search_config_path(), &clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_sizes() {
        let mut cfg = SearchConfig {
            max_file_bytes: 1,
            max_body_chars: 10,
            ..Default::default()
        };
        sanitize_search_config(&mut cfg);
        assert_eq!(cfg.max_file_bytes, 4 * 1024);
        assert_eq!(cfg.max_body_chars, 1024);
    }

    #[test]
    fn sanitize_idempotent() {
        let mut cfg = SearchConfig::default();
        sanitize_search_config(&mut cfg);
        let once = cfg.clone();
        sanitize_search_config(&mut cfg);
        assert_eq!(cfg, once);
    }

    #[test]
    fn sanitize_clamps_effort() {
        let mut cfg = SearchConfig {
            index_effort: 200,
            ..Default::default()
        };
        sanitize_search_config(&mut cfg);
        assert_eq!(cfg.index_effort, 100);
    }

    #[test]
    fn effort_maps_workers_and_delay() {
        assert_eq!(index_effort_workers(0), 1);
        assert!(index_effort_workers(100) >= 1);
        assert_eq!(index_effort_delay_ms(0), 40);
        assert_eq!(index_effort_delay_ms(100), 0);
        assert!(index_effort_delay_ms(35) < index_effort_delay_ms(0));
    }

    #[test]
    fn sanitize_merges_builtin_excludes() {
        let mut cfg = SearchConfig {
            excludes: vec!["/custom/".into()],
            ..Default::default()
        };
        // Wipe the defaults Default inserted, then sanitize must re-merge.
        cfg.excludes = vec!["/custom/".into()];
        sanitize_search_config(&mut cfg);
        assert!(cfg.excludes.iter().any(|e| e == "/custom/"));
        assert!(cfg.excludes.iter().any(|e| e == "/target/"));
        assert!(cfg.excludes.iter().any(|e| e == "/.cargo/registry/"));
        let once = cfg.clone();
        sanitize_search_config(&mut cfg);
        assert_eq!(cfg, once);
    }
}
