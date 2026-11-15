//! MIME / path → category + auto-tags.

use metis_protocol::search::SearchCategory;
use std::path::Path;

pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn guess_mime(path: &Path) -> String {
    let ext = extension_of(path);
    match ext.as_str() {
        "txt" | "md" | "rst" | "log" => "text/plain",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "text/javascript",
        "json" | "toml" | "yaml" | "yml" | "xml" | "html" | "htm" | "css" | "scss" => "text/plain",
        "c" | "h" | "cc" | "cpp" | "hpp" | "go" | "java" | "kt" | "swift" | "sh" | "bash"
        | "zsh" | "rb" | "php" | "lua" | "sql" | "cmake" | "makefile" => "text/x-code",
        "pdf" => "application/pdf",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => "image/*",
        "mp4" | "mkv" | "webm" | "avi" | "mov" => "video/*",
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" => "audio/*",
        "zip" | "tar" | "gz" | "xz" | "7z" | "rar" | "bz2" => "application/archive",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub fn categorize(path: &Path, mime: &str) -> SearchCategory {
    // Content-type first — Downloads/Desktop are locations, not file kinds.
    if mime.starts_with("image/") {
        return SearchCategory::Images;
    }
    if mime.starts_with("video/") {
        return SearchCategory::Video;
    }
    if mime.starts_with("audio/") {
        return SearchCategory::Audio;
    }
    if mime.contains("archive")
        || matches!(
            extension_of(path).as_str(),
            "zip" | "tar" | "gz" | "xz" | "7z" | "rar" | "bz2" | "tgz"
        )
    {
        return SearchCategory::Archives;
    }
    // Source / config / plain text all land under Documents (no separate Code chip).
    if mime.contains("pdf")
        || mime.contains("document")
        || mime.starts_with("text/")
        || mime.contains("code")
        || mime.contains("javascript")
        || mime.contains("rust")
        || mime.contains("python")
        || is_code_ext(&extension_of(path))
        || matches!(
            extension_of(path).as_str(),
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "rst" | "epub"
        )
    {
        return SearchCategory::Documents;
    }
    SearchCategory::Other
}

fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "lua"
            | "sh"
            | "bash"
            | "toml"
            | "cmake"
            | "sql"
            | "json"
            | "yaml"
            | "yml"
            | "css"
            | "scss"
            | "html"
            | "htm"
            | "xml"
            | "vue"
            | "svelte"
    )
}

/// Auto tags applied at index time (`cat:…`, dir hint, `year:YYYY`).
pub fn auto_tags(path: &Path, category: SearchCategory, mtime_secs: i64) -> Vec<String> {
    let mut tags = vec![format!("cat:{}", category.as_str())];
    if let Some(parent) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        let hint = parent.trim();
        if !hint.is_empty() && hint.len() < 48 && !hint.starts_with('.') {
            tags.push(format!("dir:{}", hint.to_ascii_lowercase()));
        }
    }
    if mtime_secs > 0 {
        // Approximate UTC year from unix seconds (good enough for auto-tag).
        let year = 1970 + (mtime_secs / 31_556_926);
        if (1970..2100).contains(&year) {
            tags.push(format!("year:{year}"));
        }
    }
    tags
}

/// Merge auto + user tags, dedupe, sort.
pub fn merge_tags(auto: &[String], user: &[String]) -> Vec<String> {
    let mut out: Vec<String> = auto
        .iter()
        .chain(user.iter())
        .map(|t| normalize_tag(t))
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rust_file_is_documents() {
        let p = PathBuf::from("/home/u/proj/src/main.rs");
        let mime = guess_mime(&p);
        assert_eq!(categorize(&p, &mime), SearchCategory::Documents);
    }

    #[test]
    fn downloads_photo_stays_image() {
        let p = PathBuf::from("/home/u/Downloads/photo.jpg");
        let mime = guess_mime(&p);
        assert_eq!(categorize(&p, &mime), SearchCategory::Images);
    }

    #[test]
    fn merge_dedupes() {
        let merged = merge_tags(
            &["cat:code".into(), "year:2024".into()],
            &["Work".into(), "cat:code".into()],
        );
        assert!(merged.contains(&"work".into()));
        assert_eq!(merged.iter().filter(|t| *t == "cat:code").count(), 1);
    }
}
